import assert from "node:assert/strict";
import { test } from "node:test";
import { VERIFACTU_ERROR_CODES, salesErrorText, verifactuReadonlyText } from "../src/ventas-text.ts";
import { sentAtText } from "../src/ventas-envio-text.ts";
import {
  AUTO_REFRESH_MS,
  ENVIRONMENT_SIMULATOR_NOTE,
  ENVIRONMENT_UNAVAILABLE_NOTE,
  NO_AEAT_DETAIL,
  PRODUCTION_LEGACY_TEXT,
  REMISSION_TASK_LABEL,
  REMIT_NOW_BUTTON,
  SUBMIT_VERIFACTU_KIND,
  UNEXPECTED_DUPLICATE_TEXT,
  aeatText,
  autoRefreshDelayMs,
  batchText,
  canRetryTask,
  canSubsanate,
  environmentNote,
  lastErrorText,
  pumpVisible,
  recordKindText,
  statusSignature,
  subsanationSources,
  environmentLine,
  environmentOptions,
  haltedText,
  nextSendText,
  pendingText,
  pumpReportText,
  recordStatusText,
  subsanateConfirmText,
  subsanateResultText,
  transportText,
  waitText,
} from "../src/ventas-verifactu-text.ts";

/** Marca en milisegundos de un instante inventado: 2026-03-01 10:00:00 UTC. */
const T0 = Date.UTC(2026, 2, 1, 10, 0, 0);

function record(over = {}) {
  return {
    record_id: 3,
    document_id: 12,
    kind: "alta",
    invoice_number: "F-2026-0003",
    issue_date: "01-03-2026",
    status: "pending",
    aeat_code: null,
    aeat_description: null,
    duplicate: false,
    batch_id: null,
    subsanation_record_id: null,
    environment: "simulador",
    ...over,
  };
}

function batch(over = {}) {
  return {
    id: 4,
    environment: "simulador",
    record_count: 2,
    started_at: T0,
    attempts: 1,
    outcome: "responded",
    send_state: "Correcto",
    error_code: null,
    error_description: null,
    wait_seconds: 60,
    csv: "CSV-DEMO-0004",
    finished_at: T0 + 1500,
    ...over,
  };
}

test("cada estado del registro tiene su chip y su texto", () => {
  const expected = {
    pending: ["Pendiente", "info"],
    sending: ["En envío", "info"],
    accepted: ["Aceptado", "ok"],
    accepted_with_errors: ["Aceptado con errores", "warn"],
    rejected: ["Rechazado", "error"],
    subsanated: ["Subsanado", "neutral"],
    unexpected_duplicate: ["Duplicado inesperado", "error"],
  };
  for (const [status, [label, tone]] of Object.entries(expected)) {
    const text = recordStatusText(record({ status }));
    assert.equal(text.label, label, status);
    assert.equal(text.tone, tone, status);
  }
  assert.equal(recordStatusText(record({ status: "pending" })).detail, "Todavía no se ha remitido a la AEAT.");
  assert.match(recordStatusText(record({ status: "sending" })).detail, /sin duplicarlo/);
});

test("aceptado, aceptado con errores y rechazado muestran el código y la descripción de la AEAT", () => {
  assert.equal(recordStatusText(record({ status: "accepted" })).detail, null);
  assert.match(recordStatusText(record({ status: "accepted", duplicate: true })).detail, /ya lo tenía de un intento anterior/);
  const withErrors = recordStatusText(record({ status: "accepted_with_errors", aeat_code: 2000, aeat_description: "Falta un dato opcional" }));
  assert.match(withErrors.detail, /^Código 2000: Falta un dato opcional\./);
  assert.match(withErrors.detail, /no se corrige desde aquí/);
  assert.equal(recordStatusText(record({ status: "rejected", aeat_code: 1100, aeat_description: "NIF no censado" })).detail, "Código 1100: NIF no censado");
  // Sin código ni descripción no se inventa nada: se dice.
  assert.equal(recordStatusText(record({ status: "rejected" })).detail, NO_AEAT_DETAIL);
  assert.equal(aeatText(1100, null), "Código 1100");
  assert.equal(aeatText(null, "solo texto"), "solo texto");
});

test("un registro subsanado dice qué registro lo subsana; un duplicado inesperado, qué hacer", () => {
  const done = recordStatusText(record({ status: "subsanated", aeat_code: 1100, aeat_description: "Importe incorrecto", subsanation_record_id: 9 }));
  assert.equal(done.detail, "Rechazo previo: Código 1100: Importe incorrecto. Lo subsana el registro 9.");
  assert.equal(recordStatusText(record({ status: "subsanated", subsanation_record_id: 9 })).detail, "Lo subsana el registro 9.");
  const dup = recordStatusText(record({ status: "unexpected_duplicate", aeat_code: 3000 }));
  assert.equal(dup.detail, UNEXPECTED_DUPLICATE_TEXT);
  assert.match(dup.detail, /no se da por aceptado/);
  assert.match(dup.detail, /sede electrónica/);
});

test("solo un rechazado sin subsanar se puede subsanar, y la confirmación explica el efecto", () => {
  for (const status of ["pending", "sending", "accepted", "accepted_with_errors", "subsanated", "unexpected_duplicate"]) {
    assert.equal(canSubsanate(record({ status })), false, status);
  }
  assert.equal(canSubsanate(record({ status: "rejected" })), true);
  const text = subsanateConfirmText(record({ status: "rejected", aeat_code: 1100, aeat_description: "NIF no censado" }));
  assert.match(text, /el alta de la factura F-2026-0003/);
  assert.match(text, /Código 1100: NIF no censado/);
  assert.match(text, /registro nuevo de subsanación con los mismos datos/);
  assert.match(text, /el registro rechazado queda como estaba/);
  assert.match(text, /factura rectificativa/);
  assert.match(subsanateConfirmText(record({ status: "rejected", kind: "anulacion" })), /la anulación de la factura F-2026-0003/);
  assert.match(subsanateResultText({ rejected_record_id: 3, record_id: 8, created: true }), /Se creó el registro 8/);
  assert.match(subsanateResultText({ rejected_record_id: 3, record_id: 8, created: false }), /no se creó otro/);
});

test("cada resultado de «Remitir ahora» tiene su texto", () => {
  assert.equal(REMIT_NOW_BUTTON, "Remitir ahora");
  assert.deepEqual(pumpReportText({ state: "idle" }), { tone: "neutral", text: "No hay registros pendientes de remitir." });
  assert.match(pumpReportText({ state: "busy" }).text, /Ya hay una remisión en curso/);
  const waiting = pumpReportText({ state: "waiting", seconds: 45, until: T0 + 45_000 });
  assert.equal(waiting.tone, "info");
  assert.match(waiting.text, /faltan 45 s/);
  assert.match(pumpReportText({ state: "waiting", seconds: 65, until: T0 }).text, /faltan 1 min 5 s/);
  const halted = pumpReportText({ state: "halted" });
  assert.equal(halted.tone, "warn");
  assert.match(halted.text, /parada.*Fault/);
  const unavailable = pumpReportText({ state: "environment_unavailable", environment: "produccion" });
  assert.match(unavailable.text, /«produccion».*solo funciona el simulador/);
  const sent = pumpReportText({ state: "sent", batch_id: 4, accepted: 2, accepted_with_errors: 0, rejected: 0, unexpected_duplicates: 0, missing: 0, wait_seconds: 60 });
  assert.deepEqual(sent, { tone: "ok", text: "Lote 4 enviado: 2 aceptados. La AEAT pide esperar 1 min antes del siguiente." });
  const mixed = pumpReportText({ state: "sent", batch_id: 5, accepted: 1, accepted_with_errors: 1, rejected: 1, unexpected_duplicates: 1, missing: 1, wait_seconds: 60 });
  assert.equal(mixed.tone, "error");
  assert.match(mixed.text, /1 aceptado, 1 aceptado con errores, 1 rechazado, 1 duplicado inesperado, 1 sin respuesta \(sigue pendiente\)/);
  assert.equal(pumpReportText({ state: "sent", batch_id: 6, accepted: 1, accepted_with_errors: 2, rejected: 0, unexpected_duplicates: 0, missing: 0, wait_seconds: 30 }).tone, "warn");
  const fault = pumpReportText({ state: "fault", batch_id: 7, code: 4104, description: "Error en la cabecera" });
  assert.equal(fault.tone, "error");
  assert.match(fault.text, /lote 7 \(código 4104\): Error en la cabecera\. Sus registros vuelven a estar pendientes/);
  assert.match(pumpReportText({ state: "fault", batch_id: 7, code: null, description: "x" }).text, /lote 7: x/);
  const none = pumpReportText({ state: "no_response", batch_id: 8, code: "unavailable", message: "sin conexión (simulado)" });
  assert.equal(none.tone, "warn");
  assert.match(none.text, /Sin respuesta de la AEAT para el lote 8: no hay conexión con el servicio o no está disponible \(sin conexión \(simulado\)\)\. El lote sigue abierto y se reenviará igual; no se duplica nada\./);
});

test("los errores de transporte y de la remisión tienen texto propio", () => {
  assert.match(transportText("unavailable"), /no hay conexión/);
  assert.match(transportText("invalid_response"), /no se entiende/);
  assert.match(transportText("rate_limited"), /esperar/);
  assert.match(transportText("unauthorized"), /no autoriza/);
  assert.equal(transportText("otra"), "error de transporte");
  for (const code of ["environment_unavailable", "verifactu_environment_busy", "verifactu_batch_open", "verifactu_batch_rejected", "verifactu_environment_changed", "verifactu_fault", "invalid_response", "rate_limited", "unauthorized"]) {
    assert.ok(VERIFACTU_ERROR_CODES.includes(code), code);
    const text = salesErrorText(`[${code}] detalle del servidor`);
    assert.ok(text.length > 20 && !text.includes("detalle del servidor") && !text.includes("["), `${code}: ${text}`);
  }
  assert.match(salesErrorText("[environment_unavailable] el entorno «produccion» no está disponible"), /certificado y la declaración responsable/);
  assert.match(salesErrorText("[verifactu_environment_busy] no se cambia"), /pendientes, en envío o rechazados sin subsanar/);
  // El código `unavailable` es el del correo y no se redefine aquí.
  assert.match(salesErrorText("[unavailable] x"), /correo/);
});

test("el último lote se muestra con fecha en milisegundos, resultado y CSV", () => {
  const zone = "UTC";
  const done = batchText(batch(), zone);
  assert.equal(done.when, sentAtText(T0 + 1500, zone));
  // Si se multiplicara por 1000 por error, el año sería absurdo.
  assert.match(done.when, /26/);
  assert.doesNotMatch(done.when, /\d{5,}/);
  assert.equal(done.tone, "ok");
  assert.equal(done.csv, "CSV: CSV-DEMO-0004");
  assert.match(done.result, /^Lote 4 \(2 registros\): la AEAT respondió\. Estado del envío: Correcto\.$/);
  assert.equal(batchText(batch({ csv: null }), zone).csv, null);
  const open = batchText(batch({ outcome: null, finished_at: null, send_state: null, csv: null, attempts: 2 }), zone);
  assert.equal(open.when, sentAtText(T0, zone));
  assert.equal(open.tone, "warn");
  assert.match(open.result, /sin respuesta tras 2 intentos: se reenviará igual/);
  const fault = batchText(batch({ outcome: "fault", error_code: 4104, error_description: "Cabecera no válida", csv: null }), zone);
  assert.equal(fault.tone, "error");
  assert.match(fault.result, /error de esquema \(Fault\)\. Código 4104: Cabecera no válida\./);
  assert.match(batchText(batch({ record_count: 1 }), zone).result, /\(1 registro\)/);
});

test("la espera restante y el siguiente envío permitido", () => {
  assert.equal(waitText(0), "0 s");
  assert.equal(waitText(45), "45 s");
  assert.equal(waitText(44.2), "45 s");
  assert.equal(waitText(60), "1 min");
  assert.equal(waitText(125), "2 min 5 s");
  assert.equal(waitText(-3), "0 s");
  assert.equal(waitText(Number.NaN), "0 s");
  const zone = "UTC";
  const waiting = nextSendText({ next_send_at: T0 + 45_000, wait_remaining_seconds: 45, last_batch: batch() }, zone);
  // La etiqueta «Siguiente envío:» la pone la cabecera: el texto no la repite.
  assert.equal(waiting, `${sentAtText(T0 + 45_000, zone)} (faltan 45 s)`);
  assert.doesNotMatch(waiting, /Siguiente envío/);
  assert.match(nextSendText({ next_send_at: T0, wait_remaining_seconds: 0, last_batch: batch() }, zone), /ha pasado la espera/);
  assert.match(nextSendText({ next_send_at: null, wait_remaining_seconds: null, last_batch: null }, zone), /Todavía no se ha enviado ningún lote/);
  assert.match(nextSendText({ next_send_at: null, wait_remaining_seconds: null, last_batch: batch() }, zone), /no hay espera/);
});

test("el paro por Fault y los recuentos de pendientes", () => {
  assert.equal(haltedText({ halted: false, consecutive_faults: 1 }), null);
  const halted = haltedText({ halted: true, consecutive_faults: 3 });
  assert.match(halted, /parada.*3 errores de esquema \(Fault\) seguidos.*«Remitir ahora»/);
  assert.equal(pendingText({ pending: 0, sending: 0, rejected: 0, unexpected_duplicates: 0 }), "No hay registros pendientes de remitir.");
  assert.equal(pendingText({ pending: 1, sending: 0, rejected: 0, unexpected_duplicates: 0 }), "1 registro pendiente de remitir.");
  assert.equal(pendingText({ pending: 3, sending: 2, rejected: 1, unexpected_duplicates: 2 }), "3 registros pendientes de remitir; 2 en envío; 1 rechazado sin subsanar; 2 duplicados inesperados.");
});

test("el entorno: el simulador dice que no se envía nada y los demás no se pueden elegir", () => {
  assert.equal(environmentLine("simulador"), "Simulador local: no se envía nada a la AEAT");
  assert.match(environmentLine("produccion"), /^Producción de la AEAT/);
  assert.equal(environmentLine("raro"), "Entorno «raro»");
  const options = environmentOptions(["simulador"]);
  assert.deepEqual(options.map((o) => [o.value, o.selectable]), [["simulador", true], ["preproduccion", false], ["produccion", false]]);
  assert.equal(options[0].note, null);
  assert.equal(options[1].note, ENVIRONMENT_UNAVAILABLE_NOTE);
  assert.match(ENVIRONMENT_UNAVAILABLE_NOTE, /certificado y la declaración responsable \(T18b\)/);
});

test("un registro de otro entorno (producción heredada) no se muestra como pendiente ni ofrece acciones", () => {
  const legacy = record({ status: "other_environment", environment: "produccion_heredado" });
  const text = recordStatusText(legacy);
  assert.equal(text.label, "Otro entorno");
  assert.equal(text.detail, PRODUCTION_LEGACY_TEXT);
  assert.match(text.detail, /no se remite al simulador/);
  assert.equal(canSubsanate(legacy, "simulador"), false);
  // Ni siquiera rechazado: el heredado no tiene acciones.
  assert.equal(canSubsanate(record({ status: "rejected", environment: "produccion_heredado" }), "simulador"), false);
  assert.equal(canSubsanate(record({ status: "rejected", environment: "produccion" }), "simulador"), false);
  assert.equal(canSubsanate(record({ status: "rejected", environment: "simulador" }), "simulador"), true);
});

test("el registro de subsanación dice a cuál subsana", () => {
  const sources = subsanationSources([record({ record_id: 5, status: "subsanated", subsanation_record_id: 9 }), record({ record_id: 9 })]);
  assert.equal(sources.get(9), 5);
  assert.equal(sources.has(5), false);
  assert.equal(recordKindText("alta", sources.get(9)), "Alta (subsanación del registro 5)");
  assert.equal(recordKindText("anulacion", 5), "Anulación (subsanación del registro 5)");
  assert.equal(recordKindText("alta", sources.get(5)), "Alta");
  assert.equal(recordKindText("anulacion", null), "Anulación");
});

test("el último error de la remisión se muestra con su código y texto", () => {
  assert.equal(lastErrorText(null), null);
  const once = lastErrorText({ code: "verifactu_record_invalid", message: "no consta la razón social del emisor", at: T0, occurrences: 1 });
  assert.match(once, /^Último error de la remisión \[verifactu_record_invalid\]: no consta la razón social del emisor\./);
  assert.doesNotMatch(once, /veces seguidas/);
  assert.match(lastErrorText({ code: "unauthorized", message: "HTTP 401", at: T0, occurrences: 4 }), /\[unauthorized\] \(4 veces seguidas\): HTTP 401/);
});

test("la nota del simulador solo sale con el entorno simulador y el aviso de la pestaña depende del entorno", () => {
  assert.equal(environmentNote("simulador"), ENVIRONMENT_SIMULATOR_NOTE);
  for (const other of ["preproduccion", "produccion", null, undefined]) assert.equal(environmentNote(other), null, String(other));
  assert.match(verifactuReadonlyText("simulador"), /simulador de este equipo, no a la AEAT/);
  assert.doesNotMatch(verifactuReadonlyText("simulador"), /solos a la AEAT/);
  assert.match(verifactuReadonlyText("produccion"), /solos a la AEAT/);
  assert.doesNotMatch(verifactuReadonlyText(null), /remiten solos a la AEAT/);
});

test("la tarea de remisión tiene etiqueta propia y no se reintenta desde Tareas", () => {
  assert.equal(SUBMIT_VERIFACTU_KIND, "submit_verifactu");
  assert.equal(REMISSION_TASK_LABEL, "Remisión Verifactu");
  assert.equal(canRetryTask("submit_verifactu"), false);
  assert.equal(canRetryTask("invoice"), true);
  assert.equal(canRetryTask("email"), true);
});

test("el aviso «faltan N s» caduca y el estado se relee solo mientras haya algo en curso", () => {
  const now = T0;
  const waiting = pumpReportText({ state: "waiting", seconds: 51, until: now + 51_000 }, now);
  assert.equal(pumpVisible(waiting, now + 50_000), true);
  assert.equal(pumpVisible(waiting, now + 51_000), false);
  assert.equal(pumpVisible(waiting, now + 10 * 60_000), false);
  assert.equal(pumpVisible(null, now), false);
  // Los demás avisos no caducan por tiempo.
  assert.equal(pumpVisible(pumpReportText({ state: "idle" }, now), now + 10 * 60_000), true);
  assert.equal(autoRefreshDelayMs(null), null);
  assert.equal(autoRefreshDelayMs({ pending: 0, sending: 0, wait_remaining_seconds: 0 }), null);
  assert.equal(autoRefreshDelayMs({ pending: 0, sending: 0, wait_remaining_seconds: null }), null);
  assert.equal(autoRefreshDelayMs({ pending: 1, sending: 0, wait_remaining_seconds: null }), AUTO_REFRESH_MS);
  assert.equal(autoRefreshDelayMs({ pending: 0, sending: 2, wait_remaining_seconds: 0 }), AUTO_REFRESH_MS);
  assert.equal(autoRefreshDelayMs({ pending: 0, sending: 0, wait_remaining_seconds: 30 }), AUTO_REFRESH_MS);
});

test("la firma del estado cambia cuando cambia un registro o el lote", () => {
  const base = { records: [record({ status: "sending" })], last_batch: batch(), pending: 0, sending: 1, halted: false };
  assert.equal(statusSignature(base), statusSignature({ ...base }));
  assert.notEqual(statusSignature(base), statusSignature({ ...base, records: [record({ status: "accepted" })], sending: 0 }));
  assert.notEqual(statusSignature(base), statusSignature({ ...base, last_batch: batch({ id: 5 }) }));
  assert.equal(statusSignature(null), "");
});

test("los resultados de remisión con certificado tienen su texto y no cuentan como intento (D280, D281)", () => {
  const rejected = pumpReportText({ state: "authorization_rejected", code: 4112 });
  assert.equal(rejected.tone, "error");
  assert.match(rejected.text, /4112.*parada.*no se reintenta/);
  const needsUser = pumpReportText({ state: "needs_user", batch_id: 3 });
  assert.equal(needsUser.tone, "warn");
  assert.match(needsUser.text, /PIN.*lote 3 sigue pendiente/);
  const notSent = pumpReportText({ state: "not_sent", batch_id: 2, code: "network_disabled", message: "Sin red real." });
  assert.equal(notSent.tone, "warn");
  assert.match(notSent.text, /lote 2.*No cuenta como intento/);
});
