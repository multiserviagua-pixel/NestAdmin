// E7, D167-D175: los textos de «Aprender a responder» solo afirman lo que los datos sostienen.
// Todos los ejemplos son inventados.
import assert from "node:assert/strict";
import { test } from "node:test";
import {
  INTERRUPTED_LINE,
  WITHDRAWN_LINE,
  acceptButton,
  acceptedLine,
  batchFailureLine,
  batchStaleLine,
  countLines,
  deactivateConfirm,
  deactivatedLine,
  destinationLabel,
  doneLine,
  evidenceText,
  failureLine,
  heldLine,
  isExternalDestination,
  learnDestinationLine,
  learnPhase,
  learnPhaseLabel,
  learnPhaseLine,
  learnPhaseTone,
  learnedChips,
  nothingText,
  proposalBlockedLine,
  rodajeNotice,
  sendButtonLabel,
} from "../src/learn-text.ts";

const counts = (over = {}) => ({ scanned: 24, automatic: 18, no_own_reply: 3, no_sent_folder: 0, pairs: 3, ...over });
const status = (over = {}) => ({
  state: "awaiting_consent",
  interrupted: false,
  read_done: true,
  counts: counts(),
  pairs: 3,
  destination: { provider: "ollama_cloud", model: "gemma4:31b", host: "ollama.com" },
  held: null,
  ai: null,
  fallback: null,
  failure: null,
  error: null,
  ...over,
});
const catName = (c) => ({ solicitud_factura: "solicitud de factura" })[c] ?? c;

test("una fase por cada estado del backend", () => {
  assert.equal(learnPhase(status({ state: "not_started", read_done: false })), "waiting_read");
  assert.equal(learnPhase(status({ state: "not_started" })), "starting");
  assert.equal(learnPhase(status({ state: "collected" })), "collecting");
  assert.equal(learnPhase(status()), "awaiting_consent");
  assert.equal(learnPhase(status({ state: "sent" })), "running");
  assert.equal(learnPhase(status({ state: "sent", interrupted: true })), "interrupted");
  assert.equal(learnPhase(status({ state: "done" })), "done");
  assert.equal(learnPhase(status({ state: "nothing" })), "nothing");
  assert.equal(learnPhase(status({ state: "held" })), "held");
});

test("un fallo, un consentimiento retirado y un error de bandeja tienen fase propia", () => {
  assert.equal(learnPhase(status({ failure: "unavailable,no_local_ai" })), "failed");
  assert.equal(learnPhase(status({ failure: "consent_withdrawn" })), "consent_withdrawn");
  assert.equal(learnPhase(status({ state: "collected", failure: "unavailable" })), "failed");
  assert.equal(learnPhase(status({ state: "collected", error: { code: "unauthorized", message: "x" } })), "failed");
});

test("la acción en curso de la ventana manda sobre el estado guardado", () => {
  assert.equal(learnPhase(status(), "collect"), "collecting");
  assert.equal(learnPhase(status(), "retry"), "collecting");
  assert.equal(learnPhase(status(), "run"), "running");
});

test("etiquetas y tonos: nada que aprender es un resultado normal, no una advertencia", () => {
  assert.equal(learnPhaseLabel("nothing"), "Nada que aprender");
  assert.equal(learnPhaseTone("nothing"), "ok");
  assert.equal(learnPhaseTone("done"), "ok");
  assert.equal(learnPhaseTone("failed"), "warn");
  assert.equal(learnPhaseTone("held"), "warn");
  assert.equal(learnPhaseTone("waiting_read"), "neutral");
  assert.match(learnPhaseLine("collecting"), /Recogiendo/);
  assert.equal(learnPhaseLine("done"), "");
});

test("recuentos con singular y plural, y siempre los pares", () => {
  const many = countLines(counts());
  assert.equal(many[0], "24 correos revisados.");
  assert.match(many[1], /^18 automáticos excluidos/);
  assert.equal(many[2], "3 sin respuesta suya.");
  assert.match(many.at(-1), /^3 pares/);
  const one = countLines(counts({ scanned: 1, automatic: 1, no_own_reply: 1, pairs: 1 }));
  assert.equal(one[0], "1 correo revisado.");
  assert.match(one[1], /^1 automático excluido/);
  assert.match(one.at(-1), /^1 par /);
  const none = countLines(counts({ pairs: 0 }));
  assert.match(none.at(-1), /^0 pares/);
});

test("solo cuenta lo que ocurrió: Enviados y descartes aparecen si hay", () => {
  assert.ok(!countLines(counts()).some((l) => l.includes("Enviados")));
  assert.ok(countLines(counts({ no_sent_folder: 5 })).some((l) => l.startsWith("5 no se pudieron emparejar") && l.includes("Enviados")));
  assert.ok(countLines(counts({ quote_dropped: 1 })).some((l) => l.startsWith("1 descartado")));
  assert.ok(countLines(counts({ fetch_errors: 2 })).some((l) => l.startsWith("2 no se pudieron leer")));
});

test("«Nada que aprender» da los tres recuentos y no habla de error", () => {
  const n = nothingText(counts({ scanned: 24, automatic: 24, no_own_reply: 0, no_sent_folder: 0, pairs: 0 }));
  assert.equal(n.title, "Nada que aprender");
  assert.match(n.lines[0], /24 automáticos, 0 sin respuesta suya y 0 porque su cuenta no permite leer Enviados/);
  assert.match(n.lines[1], /No es un error/);
  assert.ok(!n.lines[1].includes("casi solo avisos"));
  const some = nothingText(counts({ scanned: 1, automatic: 1, no_own_reply: 1, no_sent_folder: 1 }));
  assert.match(some.lines[0], /^De 1 correo revisado/);
  assert.match(some.lines[0], /1 automático, 1 sin respuesta suya y 1 porque/);
  assert.match(nothingText(null).lines[0], /No es un error/);
});

test("el destino dice proveedor, modelo y servidor, y la IA local no sale del equipo", () => {
  assert.equal(destinationLabel({ provider: "ollama_cloud", model: "gemma4:31b", host: "ollama.com" }), "Ollama cloud, modelo gemma4:31b, servidor ollama.com");
  assert.match(learnDestinationLine(status().destination), /^Se enviaría a Ollama cloud, modelo gemma4:31b, servidor ollama.com\.$/);
  assert.match(learnDestinationLine({ provider: "ollama", model: "gemma4:e4b", host: null }), /Nada sale de este equipo/);
  assert.match(learnDestinationLine(null), /No hay un proveedor/);
  assert.equal(isExternalDestination({ host: "ollama.com" }), true);
  assert.equal(isExternalDestination({ host: null }), false);
  assert.equal(isExternalDestination(null), false);
  assert.equal(sendButtonLabel("ollama_cloud"), "Enviar a Ollama cloud");
  assert.equal(sendButtonLabel("anthropic"), "Enviar a Claude (Anthropic)");
});

test("retenido: dice el destino de antes y el de ahora", () => {
  const held = status({
    state: "held",
    held: { reason: "destination_changed", authorized: { provider: "ollama_cloud", model: "gemma4:31b", host: "ollama.com" }, now: { provider: "openai_compatible", model: "modelo-ejemplo", host: "api.ejemplo.test" } },
  });
  const line = heldLine(held);
  assert.match(line, /antes: Ollama cloud, modelo gemma4:31b, servidor ollama.com/);
  assert.match(line, /ahora: un servicio compatible con OpenAI, modelo modelo-ejemplo, servidor api.ejemplo.test/);
  assert.match(line, /No ha salido nada/);
  assert.match(heldLine(status({ held: null })), /Vuelva a generar la vista previa/);
});

test("motivo de fallo: códigos traducidos y sin el texto del servicio", () => {
  const f = failureLine(status({ failure: "unavailable,no_local_ai" }));
  assert.match(f, /el servicio no respondió/);
  assert.match(f, /no hay una IA local preparada/);
  assert.match(f, /No se ha creado ninguna regla/);
  assert.match(failureLine(status({ failure: "unauthorized" })), /credenciales no son válidas/);
  assert.match(failureLine(status({ failure: "raro" })), /error «raro»/);
  assert.match(failureLine(status({ failure: "no_consent" })), /falta su consentimiento/);
  const mailbox = failureLine(status({ error: { code: "timeout", message: "texto del servicio con datos" } }));
  assert.match(mailbox, /No se pudo leer su bandeja: el servicio tardó demasiado/);
  assert.ok(!mailbox.includes("datos"));
});

test("consentimiento retirado y corte tienen texto propio", () => {
  assert.match(WITHDRAWN_LINE, /Retiró el consentimiento/);
  assert.match(WITHDRAWN_LINE, /no ha salido nada/);
  assert.match(INTERRUPTED_LINE, /Reintentar/);
});

test("terminado: con qué IA y cuántos pares, sin prometer reglas", () => {
  assert.match(doneLine(status({ state: "done", ai: "external" })), /Terminado con la IA externa elegida: 3 pares analizados/);
  assert.match(doneLine(status({ state: "done", ai: "local", fallback: "unavailable" })), /la IA local \(el proveedor externo no respondió: el servicio no respondió/);
  assert.match(doneLine(status({ state: "done", ai: "external", counts: counts({ pairs: 1 }) })), /1 par analizados/);
  assert.match(doneLine(status({ state: "done", ai: "external" })), /ninguna regla/);
});

test("evidencia de una regla: veces, dirección, categoría y origen", () => {
  assert.equal(
    evidenceText({ learned: 4, count: 4, subject: "ana@ejemplo.test", category: "solicitud_factura", addresses: 1 }, catName),
    "Usted respondió 4 veces a ana@ejemplo.test sobre solicitud de factura (primera carga).",
  );
  assert.match(evidenceText({ learned: 1, count: 1, subject: "ana@ejemplo.test", category: null, addresses: 1 }, catName), /^Usted respondió 1 vez a ana@ejemplo.test \(primera carga\)\.$/);
  assert.match(evidenceText({ learned: 5, count: 5, subject: "ejemplo.test", category: "documentacion", addresses: 2 }, catName), /5 veces a 2 direcciones de ejemplo.test sobre documentacion/);
});

test("rodaje y botón de la tanda", () => {
  assert.equal(rodajeNotice(3), "Las 3 primeras respuestas de cada regla irán a revisión (rodaje). Después se enviarán solas si pasan las comprobaciones.");
  assert.equal(acceptButton(2), "Aceptar y activar las marcadas (2)");
  assert.equal(acceptButton(0), "Aceptar y activar las marcadas");
});

test("el fallo de la tanda nombra la regla y pide desmarcarla", () => {
  const line = batchFailureLine({ id: "x", name: "Respuestas a ana@ejemplo.test", code: "rule_exists" });
  assert.match(line, /No se activó ninguna regla: «Respuestas a ana@ejemplo.test» ya existe una regla igual/);
  assert.match(line, /Desmárquela/);
  assert.match(batchFailureLine({ id: "x", name: "R", code: "nuevo" }), /falló \(nuevo\)/);
  const whole = batchFailureLine({ id: null, name: null, code: "batch_write_failed" });
  assert.match(whole, /No se activó ninguna regla: no se pudo guardar la tanda/);
  assert.doesNotMatch(whole, /Desmárquela|«/);
  const stale = batchStaleLine("La tanda de reglas aprendidas ha cambiado desde que la vio");
  assert.match(stale, /ha cambiado desde que la vio\. Se ha recargado la lista/);
  assert.match(batchStaleLine(""), /^Se ha recargado la lista/);
});

test("resultado de aceptar: activadas y ya activas, con singular y plural", () => {
  assert.equal(acceptedLine({ activated: 2, already_active: 0 }, 3), "2 reglas activadas. Las 3 primeras respuestas de cada una irán a revisión.");
  assert.equal(acceptedLine({ activated: 1, already_active: 1 }, 3), "1 regla activada y 1 ya estaba activa. Las 3 primeras respuestas de cada una irán a revisión.");
});

test("desactivar las aprendidas: confirmación y resultado", () => {
  assert.equal(deactivateConfirm(2), "Desactivar 2 reglas aprendidas. Las respuestas volverán a revisión.");
  assert.equal(deactivateConfirm(1), "Desactivar 1 regla aprendida. Las respuestas volverán a revisión.");
  assert.equal(deactivatedLine(2), "2 reglas aprendidas desactivadas.");
  assert.equal(deactivatedLine(1), "1 regla aprendida desactivada.");
  assert.match(deactivatedLine(0), /no se cambió nada/);
});

test("propuesta de perfil: el motivo del bloqueo", () => {
  assert.match(proposalBlockedLine([{ code: "iban" }]), /contiene un IBAN/);
  assert.match(proposalBlockedLine([{ code: "card" }, { code: "too_long" }]), /contiene un número de tarjeta y supera el tope/);
  assert.match(proposalBlockedLine([{ code: "iban" }, { code: "iban" }]), /contiene un IBAN\. /);
});

test("etiquetas de las tarjetas de regla solo con datos del backend", () => {
  assert.deepEqual(learnedChips(null), { learned: false, rodaje: null });
  assert.deepEqual(learnedChips({ evidence_origin: "approved" }), { learned: false, rodaje: null });
  assert.deepEqual(learnedChips({ evidence_origin: "learned" }), { learned: true, rodaje: null });
  assert.deepEqual(learnedChips({ evidence_origin: "learned", rodaje: { left: 3, of: 3 } }), { learned: true, rodaje: "En rodaje: 0 de 3" });
  assert.deepEqual(learnedChips({ evidence_origin: "learned", rodaje: { left: 1, of: 3 } }), { learned: true, rodaje: "En rodaje: 2 de 3" });
  assert.deepEqual(learnedChips({ evidence_origin: "learned", rodaje: { left: 0, of: 3 } }), { learned: true, rodaje: null });
});
