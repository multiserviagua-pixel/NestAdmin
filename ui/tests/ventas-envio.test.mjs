import assert from "node:assert/strict";
import { test } from "node:test";
import { flagsOf, isKnownKind, salesSendFlags, sortForReview, tabOf } from "../src/pendientes-text.ts";
import { taskTitle } from "../src/tasks-text.ts";
import { SEND_ERROR_CODES, parseSalesError, salesErrorText } from "../src/ventas-text.ts";
import {
  SEND_CONFIRM_UNVERIFIED_BUTTON,
  SEND_KIND,
  SEND_UNVERIFIED_TEXT,
  authorizationText,
  canSendByMail,
  formatSize,
  openSendStatus,
  sendApproveBlock,
  sendAttachment,
  sendConfirmText,
  sendDocumentLabel,
  sendHistoryRows,
  sendIsUnverifiable,
  sendNotices,
  sendOutcome,
  sendRecipient,
  sendTaskTitle,
} from "../src/ventas-envio-text.ts";

/** Resultado de una tarea de envío como lo deja el backend (datos inventados). */
function result(over = {}) {
  return {
    type: SEND_KIND,
    version: 1,
    document: { id: 7, kind: "invoice", number: "F-2026-0001", revision: 1, status: "issued" },
    customer_id: 3,
    customer_email_at_prepare: "compras@cliente.test",
    draft: {
      to: ["compras@cliente.test"],
      subject: "Factura F-2026-0001 — Taller Demo SL",
      body: "Hola:\n\nLe adjuntamos la factura F-2026-0001.",
      attachments: [{ name: "F-2026-0001.pdf", mime: "application/pdf", size: 48213, sha256: "a".repeat(64) }],
    },
    account: { id: "simulador", provider: "simulator", label: "Correo simulado", address: "demo@example.test" },
    checks: { sendable: true, pdf: "ok", account: "ok", recipient: "ok" },
    warnings: { recipient_differs: false },
    ...over,
  };
}

test("los códigos del envío tienen texto claro y no dejan pasar el detalle del backend", () => {
  assert.equal(salesErrorText("[recipient_missing] el cliente no tiene correo"), "El cliente no tiene correo: escriba el destinatario para poder enviarlo.");
  assert.equal(salesErrorText("[no_mail_account] no hay cuenta"), "No hay ninguna cuenta de correo conectada para enviar. Conecte una en Cuentas de correo.");
  assert.equal(salesErrorText("[send_in_progress] este documento ya tiene un envío por correo abierto: termínelo o descártelo"), "Ya hay un envío en curso para este documento.");
  assert.equal(salesErrorText("[stale_version] el contenido cambió"), "El envío cambió mientras lo revisaba: vuelva a leerlo y apruébelo de nuevo.");
  assert.match(salesErrorText("[status_not_sendable] un documento en estado «draft»"), /^El documento no está en un estado que permita enviarlo por correo/);
  assert.match(salesErrorText("[subject_invalid] x"), /^El asunto no es válido/);
  assert.match(salesErrorText("[recipient_invalid] x"), /^El destinatario no es una dirección de correo válida/);
  assert.match(salesErrorText("[body_empty] x"), /^El cuerpo del correo está vacío/);
  assert.match(salesErrorText("[account_required] x"), /^Elija la cuenta/);
  assert.match(salesErrorText("[account_unavailable] x"), /^La cuenta de correo elegida no está disponible/);
  assert.match(salesErrorText("[document_voided] x"), /anulado/);
  assert.match(salesErrorText("[document_revised] x"), /revisión nueva/);
  assert.match(salesErrorText("[pdf_changed] x"), /^El PDF del documento ya no es el que revisó/);
  // El tamaño del PDF sí es útil: el encabezado conserva el detalle del backend.
  assert.equal(
    salesErrorText("[attachment_too_large] el PDF pesa 3,4 MB y el máximo para enviarlo por correo es 3 MB: use un membrete más ligero"),
    "El PDF supera los 3 MB y no se puede enviar: el PDF pesa 3,4 MB y el máximo para enviarlo por correo es 3 MB: use un membrete más ligero",
  );
  assert.equal(salesErrorText("[attachment_too_large]"), "El PDF supera los 3 MB y no se puede enviar");
  // Todos los códigos del diseño están cubiertos y ninguno cae en «Error desconocido».
  const expected = [
    "no_mail_account", "account_required", "account_unavailable", "recipient_missing", "recipient_invalid", "subject_invalid", "body_empty",
    "attachment_too_large", "status_not_sendable", "document_revised", "document_voided", "pdf_changed", "send_in_progress", "stale_version", "unavailable", "account_has_pending_sends", "attempt_unverifiable",
  ];
  assert.deepEqual([...SEND_ERROR_CODES].sort(), [...expected].sort());
  for (const code of expected) assert.notEqual(salesErrorText(`[${code}] detalle`), "detalle");
  // Los códigos genéricos de siempre siguen igual.
  assert.equal(salesErrorText("[not_found] nada"), "No encontrado: nada");
  assert.deepEqual(parseSalesError("[send_in_progress] x"), { code: "send_in_progress", detail: "x" });
});

test("solo se ofrece enviar un documento emitido, vigente y en un estado enviable", () => {
  const doc = (over) => ({ kind: "invoice", status: "issued", number: "F-2026-0001", superseded_by: null, ...over });
  assert.equal(canSendByMail(doc({})), true);
  assert.equal(canSendByMail(doc({ status: "sent" })), true, "se puede reenviar");
  for (const status of ["draft", "voided", "accepted", "rejected", "expired", "converted", "invoiced", "delivered"]) {
    assert.equal(canSendByMail(doc({ status })), false, status);
  }
  assert.equal(canSendByMail(doc({ kind: "delivery_note", status: "delivered" })), true, "el albarán entregado también");
  assert.equal(canSendByMail(doc({ kind: "quote", status: "delivered" })), false);
  assert.equal(canSendByMail(doc({ number: null })), false);
  assert.equal(canSendByMail(doc({ superseded_by: 9 })), false, "una revisión posterior vigente es la que se envía");
});

test("el aviso de destinatario distinto sale solo cuando el backend lo marca y no bloquea", () => {
  assert.deepEqual(sendNotices(result()), []);
  const differs = sendNotices(result({ warnings: { recipient_differs: true }, draft: { ...result().draft, to: ["otro@cliente.test"] } }));
  assert.equal(differs.length, 1);
  assert.equal(differs[0].tone, "warn");
  assert.match(differs[0].text, /^El destinatario no coincide con el correo del cliente \(compras@cliente\.test\)\./);
  // No bloquea: con un destinatario válido distinto se puede aprobar.
  assert.equal(sendApproveBlock(result({ warnings: { recipient_differs: true } })), null);
  assert.equal(salesSendFlags(result({ warnings: { recipient_differs: true } })).map((f) => f.label).join(), "Destinatario distinto");
});

test("un cliente sin correo se dice y bloquea aprobar hasta escribir el destinatario", () => {
  const none = result({ customer_email_at_prepare: null, draft: { ...result().draft, to: [] }, checks: { sendable: true, pdf: "ok", account: "ok", recipient: "missing" } });
  const notices = sendNotices(none);
  assert.equal(notices.length, 1);
  assert.match(notices[0].text, /^El cliente no tiene correo: escriba el destinatario/);
  assert.equal(sendApproveBlock(none), "Sin destinatario: escríbalo para poder enviar.");
  // Escrito a mano, el bloqueo desaparece y el aviso lo recuerda.
  assert.equal(sendApproveBlock(none, { to: " a@b.test " }), null);
  assert.match(sendNotices({ ...none, draft: { ...none.draft, to: ["a@b.test"] } })[0].text, /el destinatario lo ha escrito usted/);
  assert.equal(salesSendFlags(none)[0].label, "Sin destinatario");
});

test("sin cuenta o con el PDF demasiado grande no se puede aprobar", () => {
  const noAccount = result({ account: null, checks: { sendable: true, pdf: "ok", account: "missing", recipient: "ok" } });
  assert.equal(sendApproveBlock(noAccount), "Sin cuenta de correo: elija una para poder enviar.");
  assert.equal(sendApproveBlock(noAccount, { account: "simulador" }), null);
  assert.ok(sendNotices(noAccount).some((n) => n.tone === "error" && /No hay cuenta elegida/.test(n.text)));
  const big = result({ checks: { sendable: true, pdf: "too_large", account: "ok", recipient: "ok" } });
  assert.match(sendApproveBlock(big), /supera los 3 MB/);
  assert.ok(salesSendFlags(big).some((f) => f.label === "PDF demasiado grande" && f.tone === "error"));
  const unconfirmed = result({ recovery: { version: 1, needs_confirmation: true } });
  assert.match(sendApproveBlock(unconfirmed), /confirme primero que el intento anterior no se envió/);
  assert.equal(salesSendFlags(unconfirmed)[0].label, "Requiere confirmación");
});

test("el título de la tarea nombra el documento y el destinatario", () => {
  assert.equal(sendTaskTitle(result()), "Enviar Factura F-2026-0001 a compras@cliente.test");
  assert.equal(taskTitle({ id: 5, kind: SEND_KIND, input_ref: "sales_document:7", result: result() }), "Enviar Factura F-2026-0001 a compras@cliente.test");
  assert.equal(sendTaskTitle(result({ draft: { ...result().draft, to: [] } })), "Enviar Factura F-2026-0001 (sin destinatario)");
  assert.equal(sendDocumentLabel(result({ document: { id: 7, kind: "quote", number: "P-2026-0002", revision: 2, status: "sent" } })), "Presupuesto P-2026-0002 (revisión 2)");
  assert.equal(sendDocumentLabel({}), "Documento n.º ?");
  assert.equal(sendRecipient({ draft: { to: ["  x@y.test "] } }), "x@y.test");
  assert.equal(sendRecipient({ draft: { to: [] } }), null);
});

test("el envío se revisa en la pestaña de correos y los que tienen incidencias van primero", () => {
  assert.equal(isKnownKind(SEND_KIND), true);
  assert.equal(tabOf(SEND_KIND), "emails");
  const clean = { id: 1, kind: SEND_KIND, created_at: 10, result: result() };
  const flagged = { id: 2, kind: SEND_KIND, created_at: 20, result: result({ warnings: { recipient_differs: true } }) };
  assert.deepEqual(flagsOf(clean), []);
  assert.equal(flagsOf(flagged).length, 1);
  assert.deepEqual(sortForReview([clean, flagged]).map((t) => t.id), [2, 1]);
});

test("adjunto, tamaño, confirmación y resultado del envío", () => {
  assert.deepEqual(sendAttachment(result()), { name: "F-2026-0001.pdf", size: 48213 });
  assert.equal(sendAttachment({}), null);
  assert.equal(formatSize(512), "512 B");
  assert.equal(formatSize(48213), "48 KB");
  assert.equal(formatSize(2_500_000), "2,5 MB");
  assert.equal(formatSize(-1), "tamaño desconocido");
  const text = sendConfirmText(result(), "compras@cliente.test", "Correo simulado · demo@example.test");
  assert.match(text, /Factura F-2026-0001/);
  assert.match(text, /a compras@cliente\.test/);
  assert.match(text, /Correo simulado · demo@example\.test/);
  assert.match(text, /¿Enviar ahora\?$/);
  assert.deepEqual(sendOutcome({ id: 1, kind: SEND_KIND, state: "completed", result: result() }), {
    kind: "ok",
    text: "Enviado a compras@cliente.test. La entrega al destinatario no se comprueba.",
  });
  assert.equal(sendOutcome({ id: 1, kind: SEND_KIND, state: "completed", result: result({ status_error: "el documento no pasó a enviado" }) }).kind, "warn");
  const failed = sendOutcome({ id: 1, kind: SEND_KIND, state: "failed", error: { code: "unavailable", message: "el servicio no respondió" }, result: result() });
  assert.equal(failed.kind, "error");
  assert.match(failed.text, /No consta que haya salido/);
  assert.match(failed.text, /Comprobar si salió/);
  assert.equal(sendOutcome({ id: 1, kind: SEND_KIND, state: "awaiting_review", result: result({ review_reason: "el PDF cambió" }) }).kind, "warn");
});

test("el detalle del documento distingue revisión, fallo y envío en curso", () => {
  const task = (state, over = {}) => ({ id: 1, kind: SEND_KIND, state, result: result(over), error: state === "failed" ? { code: "unavailable", message: "sin respuesta" } : null });
  assert.equal(openSendStatus(task("awaiting_review")).action, "review");
  assert.match(openSendStatus(task("awaiting_review")).text, /Se aprueba en Pendientes/);
  const failed = openSendStatus(task("failed"));
  assert.equal(failed.action, "check");
  assert.equal(failed.tone, "error");
  assert.match(failed.text, /sin respuesta/);
  assert.equal(openSendStatus(task("running")).action, "wait");
});

test("el historial muestra fecha, destinatario, cuenta y quién lo autorizó, lo más reciente primero", () => {
  assert.equal(authorizationText("aprobación"), "Aprobado por usted");
  assert.equal(authorizationText("regla cliente-1 v3"), "Regla automática «cliente-1» (versión 3)");
  assert.equal(authorizationText("otra cosa"), "Autorización: otra cosa");
  const send = (id, at, over = {}) => ({
    id, document_id: 7, task_id: 1, version: 1, idempotency_key: `k${id}`, account: "simulador", account_address: "demo@example.test",
    recipient: "compras@cliente.test", subject: "Factura", attachment_name: "F-2026-0001.pdf", attachment_size: 48213, attachment_sha256: "a".repeat(64),
    authorization: "aprobación", sent_message_id: null, sent_at: at, ...over,
  });
  // 2026-03-01 10:00:00 UTC y 2026-03-02 09:30:00 UTC, en milisegundos como los da el núcleo (now_ms).
  const rows = sendHistoryRows([send(1, 1772359200000), send(2, 1772443800000, { account_address: null, authorization: "regla cliente-1 v1" })], "UTC");
  assert.equal(rows.length, 2);
  assert.equal(rows[0].when, "2/3/26, 9:30");
  assert.equal(rows[1].when, "1/3/26, 10:00");
  assert.equal(rows[0].account, "simulador");
  assert.equal(rows[0].authorization, "Regla automática «cliente-1» (versión 1)");
  assert.equal(rows[1].recipient, "compras@cliente.test");
  assert.equal(rows[1].account, "demo@example.test");
  assert.equal(rows[1].authorization, "Aprobado por usted");
  assert.equal(rows[1].attachment, "F-2026-0001.pdf (48 KB)");
  assert.deepEqual(sendHistoryRows([]), []);
});

test("sin poder comprobar si salió, la tarjeta ofrece la confirmación explícita y no deja aprobar", () => {
  const unverifiable = result({ recovery: { version: 1, error: "attempt_unverifiable", needs_confirmation: false } });
  assert.equal(sendIsUnverifiable(unverifiable), true);
  assert.equal(sendIsUnverifiable(result({ recovery: { version: 1, error: "unavailable", needs_confirmation: false } })), false);
  assert.equal(sendIsUnverifiable(result()), false);
  assert.match(sendApproveBlock(unverifiable) ?? "", /No se puede comprobar si el intento anterior salió/);
  assert.equal(SEND_CONFIRM_UNVERIFIED_BUTTON, "No se puede comprobar si salió; confirmo que no se envió");
  assert.match(SEND_UNVERIFIED_TEXT, /Enviados/);
  assert.match(salesErrorText("[attempt_unverifiable] x"), /No se puede comprobar si el envío salió/);
  assert.match(salesErrorText("[account_has_pending_sends] x"), /resuélvalos antes de quitarla/);
});
