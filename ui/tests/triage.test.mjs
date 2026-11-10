import assert from "node:assert/strict";
import { test } from "node:test";
import {
  draftFlagsText,
  hasDraft,
  informativeReason,
  informativeTasks,
  isBounce,
  isInformative,
  redraftConfirmText,
  redraftFor,
  redraftNeedsConfirm,
  reviewEmailActions,
  signalsUnchecked,
  triageNotices,
} from "../src/triage-text.ts";

const info = (id, created_at, codes = ["noreply_address"], state = "completed") => ({
  id,
  kind: "email",
  state,
  created_at,
  result: { triage: { route: "informativo", codes, source: "local" }, message: { signals: codes } },
});

test("solo es informativo un correo completado con ruta informativo", () => {
  assert.equal(isInformative(info(1, 1)), true);
  assert.equal(isInformative(info(1, 1, ["noreply_address"], "awaiting_review")), false);
  assert.equal(isInformative({ id: 2, kind: "invoice", state: "completed", created_at: 1, result: { triage: { route: "informativo" } } }), false);
  assert.equal(isInformative({ id: 3, kind: "email", state: "completed", created_at: 1, result: null }), false);
  assert.equal(isInformative({ id: 4, kind: "email", state: "completed", created_at: 1, result: { triage: { route: "rebote" } } }), false);
});

test("informativos: más recientes primero, desempate por id", () => {
  const list = informativeTasks([info(1, 10), info(2, 30), info(3, 30), { id: 9, kind: "email", state: "completed", created_at: 99, result: {} }]);
  assert.deepEqual(list.map((t) => t.id), [3, 2, 1]);
});

test("motivos legibles de los informativos", () => {
  assert.equal(informativeReason(info(1, 1, ["noreply_address"]).result), "Enviado desde una dirección que no admite respuestas");
  assert.equal(informativeReason(info(1, 1, ["auto_submitted"]).result), "El remitente lo marca como mensaje automático");
  assert.equal(informativeReason(info(1, 1, ["precedence_junk"]).result), "El remitente lo marca como correo no solicitado");
  assert.equal(informativeReason(info(1, 1, ["list_id", "auto_submitted"]).result), "El remitente lo marca como mensaje automático");
  assert.equal(informativeReason(info(1, 1, []).result), "Motivo no consta");
  assert.equal(informativeReason(null), "Motivo no consta");
});

test("rebote, débiles y sin comprobar", () => {
  assert.equal(isBounce({ message: { signals: ["null_return_path"] } }), true);
  assert.equal(isBounce({ message: { signals: ["bounce_address"] } }), true);
  assert.equal(isBounce({ triage: { route: "rebote", codes: [] }, message: { signals: [] } }), true);
  assert.equal(isBounce({ message: { signals: ["list_id"] } }), false);
  assert.equal(signalsUnchecked({ message: {} }), true);
  assert.equal(signalsUnchecked({ message: { signals: null } }), true);
  assert.equal(signalsUnchecked({ message: { signals: [] } }), false);
  assert.equal(signalsUnchecked(null), true);
});

test("avisos del correo en revisión, con los textos acordados", () => {
  const keys = (r) => triageNotices(r).map((n) => n.key);
  assert.deepEqual(keys({ message: { signals: [] }, needs_reply: true }), []);
  const bounce = triageNotices({ triage: { route: "rebote", codes: ["bounce_address"] }, message: { signals: ["bounce_address"] }, needs_reply: false });
  assert.deepEqual(bounce.map((n) => n.key), ["bounce"]);
  assert.equal(bounce[0].text, "Posible fallo de entrega: un correo enviado no llegó. Revise el destinatario.");
  const none = triageNotices({ message: { signals: [] }, needs_reply: false });
  assert.equal(none[0].text, "Probablemente no requiere respuesta. No se ha preparado borrador.");
  const weak = triageNotices({ message: { signals: ["precedence_list"] }, needs_reply: true });
  assert.equal(weak[0].text, "Parece un envío masivo: ninguna regla lo responde sin su revisión");
  assert.deepEqual(keys({ message: {}, needs_reply: true }), ["unchecked"]);
  assert.equal(triageNotices({ message: {} })[0].text, "No se pudieron comprobar los datos del remitente: las reglas no enviarán este correo solas.");
  assert.deepEqual(keys({ message: { signals: [] }, needs_reply: true, draft_flags: ["url"] }), ["draft_flags"]);
});

test("draft_flags: solo los tipos presentes, con comas y «o»", () => {
  assert.equal(draftFlagsText([]), null);
  assert.equal(draftFlagsText(undefined), null);
  assert.equal(draftFlagsText(["url"]), "El borrador incluye enlaces. Compruébelos antes de enviar; ninguna regla lo enviará solo.");
  assert.equal(draftFlagsText(["iban", "email"]), "El borrador incluye correos o IBAN. Compruébelos antes de enviar; ninguna regla lo enviará solo.");
  assert.equal(draftFlagsText(["iban", "phone", "url", "email"]), "El borrador incluye enlaces, correos, teléfonos o IBAN. Compruébelos antes de enviar; ninguna regla lo enviará solo.");
});

test("botones del correo en revisión", () => {
  const normal = reviewEmailActions({ needs_reply: true, draft: { to: ["a@b.test"], body: "Hola" } });
  assert.deepEqual([normal.editor, normal.approve, normal.archive, normal.redraft], [true, true, true, false]);
  const noReply = reviewEmailActions({ needs_reply: false });
  assert.deepEqual([noReply.editor, noReply.approve, noReply.archive, noReply.redraft], [false, false, true, true]);
  assert.ok(noReply.approveHint);
  const bounce = reviewEmailActions({ triage: { route: "rebote" }, needs_reply: false });
  assert.equal(bounce.redraft, true);
  // Cerrado como respondido fuera de NestAdmin (D178): no se ofrece pedir otra respuesta.
  assert.equal(reviewEmailActions({ needs_reply: false, closed: { reason: "replied_outside", replied_at: null } }).redraft, false);
  const attempted = reviewEmailActions({ needs_reply: true, draft: { body: "x" }, attempt: { version: 1 } });
  assert.equal(attempted.archive, false);
  assert.equal(reviewEmailActions({ needs_reply: true, draft: { body: "   " } }).editor, false);
  assert.equal(hasDraft({ draft: { body: "Hola" } }), true);
  assert.equal(hasDraft({}), false);
});

test("confirmación al redactar desde un remitente con señal fuerte", () => {
  assert.equal(redraftNeedsConfirm(info(1, 1, ["noreply_address"]).result), true);
  assert.equal(redraftNeedsConfirm({ message: { signals: ["list_id"] } }), false);
  assert.equal(redraftNeedsConfirm({ triage: { route: "rebote", codes: ["bounce_address"] }, message: { signals: ["bounce_address"] } }), false);
  assert.equal(
    redraftConfirmText("no-reply@avisos.test"),
    "¿Redactar una respuesta a no-reply@avisos.test? Esta dirección no admite respuestas: es probable que nadie la lea.",
  );
  assert.match(redraftConfirmText(null), /este remitente/);
});

test("redacción pedida: se detecta por la entrada o el resultado, y no la cancelada", () => {
  const tasks = [
    { id: 5, kind: "email", state: "awaiting_review", created_at: 1, input_ref: JSON.stringify({ redraft_of: 2 }), result: null },
    { id: 6, kind: "email", state: "cancelled", created_at: 1, input_ref: JSON.stringify({ redraft_of: 3 }), result: null },
    { id: 7, kind: "email", state: "awaiting_review", created_at: 1, input_ref: "{}", result: { redraft_of: 4 } },
  ];
  assert.equal(redraftFor(tasks, 2)?.id, 5);
  assert.equal(redraftFor(tasks, 3), null);
  assert.equal(redraftFor(tasks, 4)?.id, 7);
  assert.equal(redraftFor(tasks, 9), null);
});
