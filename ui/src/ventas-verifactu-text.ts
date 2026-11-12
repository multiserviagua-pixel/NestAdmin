// Textos y reglas puras de la remisión de Verifactu a la AEAT (E5, D247–D261), sin DOM: se
// prueban con `node --test`. Solo tipos borrables. Todos los ejemplos son inventados.
//
// Las marcas de tiempo del núcleo vienen en milisegundos Unix: aquí no se multiplican por 1000.
// Los segundos de espera (`seconds`, `wait_remaining_seconds`) son segundos.

import type { VerifactuBatchView, VerifactuEnvironment, VerifactuIssueView, VerifactuPumpReport, VerifactuRecordView, VerifactuStatusView, VerifactuSubsanation } from "./types";
import { sentAtText } from "./ventas-envio-text.ts";

type Tone = "neutral" | "info" | "ok" | "warn" | "error";

export const REMIT_NOW_BUTTON = "Remitir ahora";
export const SUBSANATE_BUTTON = "Subsanar";
export const NO_STATUS_FOR_RECORD = "Sin estado de remisión";
export const NO_AEAT_DETAIL = "La AEAT no devolvió código ni descripción.";

/** Tipo de las tareas de remisión en Tareas (D250). */
export const SUBMIT_VERIFACTU_KIND = "submit_verifactu";
export const REMISSION_TASK_LABEL = "Remisión Verifactu";
export const REMISSION_TASK_HINT = "Se reenvía desde Ventas, pestaña «Registros Verifactu», con «Remitir ahora».";

/** Una tarea de remisión no se reintenta desde Tareas: la remisión la lleva la pestaña de registros. */
export function canRetryTask(kind: string): boolean {
  return kind !== SUBMIT_VERIFACTU_KIND;
}

// ---- Plurales y esperas ----

function count(n: number, one: string, many: string): string {
  return `${n} ${n === 1 ? one : many}`;
}

/** Espera restante: `45 s`, `1 min`, `1 min 5 s`. Lo que no es un número válido se trata como 0. */
export function waitText(seconds: number): string {
  const total = Number.isFinite(seconds) && seconds > 0 ? Math.ceil(seconds) : 0;
  if (total < 60) return `${total} s`;
  const minutes = Math.floor(total / 60);
  const rest = total % 60;
  return rest === 0 ? `${minutes} min` : `${minutes} min ${rest} s`;
}

// ---- Entorno ----

export const ENVIRONMENT_LABEL: Record<VerifactuEnvironment, string> = {
  simulador: "Simulador local",
  preproduccion: "Preproducción de la AEAT",
  produccion: "Producción de la AEAT",
};

export const ENVIRONMENT_UNAVAILABLE_NOTE = "Llegará con el certificado y la declaración responsable (T18b): todavía no se puede elegir.";
export const ENVIRONMENT_SIMULATOR_NOTE =
  "No se envía nada a la AEAT: un simulador en este equipo hace de servidor. Las facturas emitidas con él llevan para siempre el QR de pruebas.";

/** La nota del simulador solo sale con el entorno simulador; sin entorno leído o en otro, nada. */
export function environmentNote(environment: string | null | undefined): string | null {
  return environment === "simulador" ? ENVIRONMENT_SIMULATOR_NOTE : null;
}

export const PRODUCTION_LEGACY_TEXT = "Producción anterior; no se remite al simulador";

/** Texto propio del entorno de un registro que no es el vigente. */
export function recordEnvironmentText(environment: string): string {
  if (environment === "produccion_heredado") return PRODUCTION_LEGACY_TEXT;
  const label = ENVIRONMENT_LABEL[environment as VerifactuEnvironment];
  return `${label ?? `Entorno «${environment}»`}; no se remite desde el entorno vigente`;
}

/** Línea de la cabecera: el entorno y qué significa. */
export function environmentLine(environment: string): string {
  if (environment === "simulador") return "Simulador local: no se envía nada a la AEAT";
  const label = ENVIRONMENT_LABEL[environment as VerifactuEnvironment];
  return label ? `${label}: los registros se remiten a la AEAT` : `Entorno «${environment}»`;
}

export interface EnvironmentOption {
  value: VerifactuEnvironment;
  label: string;
  selectable: boolean;
  /** Por qué no se puede elegir, o `null`. */
  note: string | null;
}

/** Los tres entornos con su disponibilidad (lo que dice el backend manda). */
export function environmentOptions(selectable: VerifactuEnvironment[]): EnvironmentOption[] {
  return (["simulador", "preproduccion", "produccion"] as VerifactuEnvironment[]).map((value) => {
    const ok = selectable.includes(value);
    return { value, label: ENVIRONMENT_LABEL[value], selectable: ok, note: ok ? null : ENVIRONMENT_UNAVAILABLE_NOTE };
  });
}

// ---- Estado de cada registro ----

/** `Código 1100: descripción`; sin ninguno de los dos se dice explícitamente. */
export function aeatText(code: number | null, description: string | null): string {
  const text = (description ?? "").trim();
  if (code === null && !text) return NO_AEAT_DETAIL;
  if (code === null) return text;
  return text ? `Código ${code}: ${text}` : `Código ${code}`;
}

export interface RecordStatusText {
  label: string;
  tone: Tone;
  /** Explicación bajo la etiqueta, o `null`. */
  detail: string | null;
}

export const UNEXPECTED_DUPLICATE_TEXT =
  "La AEAT dice que ya tenía este registro, pero no consta que lo enviara esta aplicación, así que no se da por aceptado. Compruebe en la sede electrónica de la AEAT si el registro consta y de dónde viene; no se reenvía solo.";

export function recordStatusText(r: VerifactuRecordView): RecordStatusText {
  switch (r.status) {
    case "pending":
      return { label: "Pendiente", tone: "info", detail: "Todavía no se ha remitido a la AEAT." };
    case "sending":
      return { label: "En envío", tone: "info", detail: "Está en un lote en curso; si no hay respuesta se reenvía igual, sin duplicarlo." };
    case "accepted":
      return { label: "Aceptado", tone: "ok", detail: r.duplicate ? "La AEAT ya lo tenía de un intento anterior sin respuesta: cuenta como aceptado." : null };
    case "accepted_with_errors":
      return { label: "Aceptado con errores", tone: "warn", detail: `${aeatText(r.aeat_code, r.aeat_description)}. La AEAT lo acepta señalando un defecto; no se corrige desde aquí.` };
    case "rejected":
      return { label: "Rechazado", tone: "error", detail: aeatText(r.aeat_code, r.aeat_description) };
    case "subsanated": {
      const by = r.subsanation_record_id === null ? "Lo subsana otro registro." : `Lo subsana el registro ${r.subsanation_record_id}.`;
      const was = r.aeat_code === null && !r.aeat_description ? "" : `Rechazo previo: ${aeatText(r.aeat_code, r.aeat_description)}. `;
      return { label: "Subsanado", tone: "neutral", detail: `${was}${by}` };
    }
    case "unexpected_duplicate":
      return { label: "Duplicado inesperado", tone: "error", detail: UNEXPECTED_DUPLICATE_TEXT };
    case "other_environment":
      return { label: "Otro entorno", tone: "neutral", detail: recordEnvironmentText(r.environment) };
    default:
      return { label: String(r.status), tone: "neutral", detail: null };
  }
}

/**
 * «Subsanar» solo se ofrece en un rechazado sin subsanar del entorno vigente: uno de otro
 * entorno (p. ej. producción heredada) no tiene acciones. Sin `current` solo cuenta el estado.
 */
export function canSubsanate(r: VerifactuRecordView, current?: string): boolean {
  if (r.status !== "rejected") return false;
  if (r.environment === "produccion_heredado") return false;
  return current === undefined || r.environment === current;
}

/** Registro rechazado al que subsana cada registro de subsanación (`nuevo -> rechazado`). */
export function subsanationSources(records: Pick<VerifactuRecordView, "record_id" | "subsanation_record_id">[]): Map<number, number> {
  const map = new Map<number, number>();
  for (const r of records) if (r.subsanation_record_id !== null) map.set(r.subsanation_record_id, r.record_id);
  return map;
}

/** «Alta», «Anulación» y, si el registro subsana a otro, «Alta (subsanación del registro 5)». */
export function recordKindText(kind: "alta" | "anulacion", subsanatesRecordId: number | null | undefined): string {
  const base = kind === "alta" ? "Alta" : "Anulación";
  return subsanatesRecordId === null || subsanatesRecordId === undefined ? base : `${base} (subsanación del registro ${subsanatesRecordId})`;
}

export function subsanateConfirmText(r: VerifactuRecordView): string {
  const what = r.kind === "anulacion" ? `la anulación de la factura ${r.invoice_number}` : `el alta de la factura ${r.invoice_number}`;
  return `Va a subsanar ${what} (registro ${r.record_id}), rechazada por la AEAT. ${aeatText(r.aeat_code, r.aeat_description)}.\n\nSe crea un registro nuevo de subsanación con los mismos datos, encadenado como cualquier otro, y se remite a la AEAT; el registro rechazado queda como estaba. Si el problema eran los datos de la factura, subsanar no los corrige: la vía es una factura rectificativa.\n\n¿Subsanar?`;
}

export function subsanateResultText(done: VerifactuSubsanation): string {
  return done.created
    ? `Se creó el registro ${done.record_id} de subsanación; se remitirá a la AEAT en el siguiente envío.`
    : `El registro ${done.record_id} ya subsanaba este rechazo: no se creó otro.`;
}

// ---- Cabecera: último lote, espera, paro, pendientes ----

export interface BatchText {
  /** Fecha y hora (milisegundos Unix); la del fin si el lote cerró, la del inicio si sigue abierto. */
  when: string;
  result: string;
  tone: Tone;
  /** `CSV: ...`, o `null` si la AEAT no lo devolvió. */
  csv: string | null;
}

export function batchText(batch: VerifactuBatchView, timeZone?: string): BatchText {
  const when = sentAtText(batch.finished_at ?? batch.started_at, timeZone);
  const records = count(batch.record_count, "registro", "registros");
  if (batch.outcome === null) {
    return { when, tone: "warn", csv: null, result: `Lote ${batch.id} (${records}) sin respuesta tras ${count(batch.attempts, "intento", "intentos")}: se reenviará igual, sin duplicar nada.` };
  }
  if (batch.outcome === "fault") {
    return { when, tone: "error", csv: null, result: `Lote ${batch.id} (${records}): error de esquema (Fault). ${aeatText(batch.error_code, batch.error_description)}. Sus registros vuelven a estar pendientes.` };
  }
  const state = batch.send_state ? ` Estado del envío: ${batch.send_state}.` : "";
  return { when, tone: "ok", csv: batch.csv ? `CSV: ${batch.csv}` : null, result: `Lote ${batch.id} (${records}): la AEAT respondió.${state}` };
}

/** Siguiente envío permitido o espera que queda. */
export function nextSendText(status: Pick<VerifactuStatusView, "next_send_at" | "wait_remaining_seconds" | "last_batch">, timeZone?: string): string {
  const remaining = status.wait_remaining_seconds;
  if (status.next_send_at === null) {
    return status.last_batch ? "Se puede remitir ya: no hay espera." : "Todavía no se ha enviado ningún lote: se puede remitir en cuanto haya registros.";
  }
  if (remaining !== null && remaining > 0) {
    // La etiqueta «Siguiente envío:» la pone la cabecera: aquí solo va el valor.
    return `${sentAtText(status.next_send_at, timeZone)} (faltan ${waitText(remaining)})`;
  }
  return "Se puede remitir ya: ha pasado la espera entre lotes.";
}

/** Aviso de paro por `Fault` seguidos, o `null`. */
export function haltedText(status: Pick<VerifactuStatusView, "halted" | "consecutive_faults">): string | null {
  if (!status.halted) return null;
  return `La remisión automática está parada: la AEAT devolvió ${status.consecutive_faults} errores de esquema (Fault) seguidos. No se reintenta sola; «${REMIT_NOW_BUTTON}» lo intenta una vez más.`;
}

/** Recuentos de la cabecera. */
export function pendingText(status: Pick<VerifactuStatusView, "pending" | "sending" | "rejected" | "unexpected_duplicates">): string {
  const parts = [status.pending === 0 ? "No hay registros pendientes de remitir" : count(status.pending, "registro pendiente de remitir", "registros pendientes de remitir")];
  if (status.sending > 0) parts.push(`${status.sending} en envío`);
  if (status.rejected > 0) parts.push(`${count(status.rejected, "rechazado", "rechazados")} sin subsanar`);
  if (status.unexpected_duplicates > 0) parts.push(count(status.unexpected_duplicates, "duplicado inesperado", "duplicados inesperados"));
  return `${parts.join("; ")}.`;
}

/** Último error de preparación o de transporte, con su código estable; `null` si no hay. */
export function lastErrorText(error: VerifactuIssueView | null | undefined): string | null {
  if (!error) return null;
  const times = error.occurrences > 1 ? ` (${error.occurrences} veces seguidas)` : "";
  return `Último error de la remisión [${error.code}]${times}: ${error.message}. ${LAST_ERROR_FOLLOW_UP}`;
}

const LAST_ERROR_FOLLOW_UP = "Se vuelve a intentar sola tras la espera; «Remitir ahora» lo intenta ya.";

// ---- Refresco de la pestaña ----

/** Cada cuánto se vuelve a leer el estado mientras haya algo en envío, pendiente o en espera. */
export const AUTO_REFRESH_MS = 5000;

export function autoRefreshDelayMs(status: Pick<VerifactuStatusView, "pending" | "sending" | "wait_remaining_seconds"> | null | undefined): number | null {
  if (!status) return null;
  const waiting = (status.wait_remaining_seconds ?? 0) > 0;
  return status.pending > 0 || status.sending > 0 || waiting ? AUTO_REFRESH_MS : null;
}

/** Firma del estado visible: si cambia, el aviso de la última pasada deja de ser cierto. */
export function statusSignature(status: Pick<VerifactuStatusView, "records" | "last_batch" | "pending" | "sending" | "halted"> & Partial<Pick<VerifactuStatusView, "authorization_rejected" | "authorization_code">> | null | undefined): string {
  if (!status) return "";
  return JSON.stringify([status.records.map((r) => [r.record_id, r.status]), status.last_batch?.id ?? null, status.last_batch?.outcome ?? null, status.pending, status.sending, status.halted, status.authorization_rejected ?? false, status.authorization_code ?? null]);
}

export const RESUME_AUTHORIZATION_BUTTON = "Ya está arreglado, reanudar";

/** Explicación en lenguaje llano de cada código de autorización de la AEAT (D281). */
export function authorizationCodeText(code: number | null): string {
  switch (code) {
    case 4112:
      return "el certificado no está autorizado para este obligado. Dé de alta el apoderamiento o la colaboración social, o cambie de certificado.";
    case 4105:
    case 4117:
    case 4123:
    case 4124:
      return "el representante indicado no es válido para este obligado o este certificado. Revise los datos del representante o dé de alta la representación.";
    default:
      return "el certificado o el representante no están autorizados.";
  }
}

/** Alerta persistente de la remisión parada por autorización; `null` si no lo está. */
export function authorizationAlertText(status: Pick<VerifactuStatusView, "authorization_rejected" | "authorization_code">): string | null {
  if (!status.authorization_rejected) return null;
  const code = status.authorization_code === null ? "" : ` (código ${status.authorization_code})`;
  return `La AEAT rechazó la autorización${code}: ${authorizationCodeText(status.authorization_code)} La remisión de esta base está parada y no se reintenta sola hasta que cambie el certificado o la configuración, o declare que ya lo arregló.`;
}

export const RESUME_AUTHORIZATION_CONFIRM = "¿Confirma que ya ha dado de alta el apoderamiento o la colaboración, o ha cambiado de certificado? La remisión se reanudará y se intentará enviar ahora.";

// ---- Resultado de «Remitir ahora» ----

/** Qué pasó con la red cuando no hubo respuesta. */
export function transportText(code: string): string {
  switch (code) {
    case "unavailable":
      return "no hay conexión con el servicio o no está disponible";
    case "invalid_response":
      return "la respuesta no se entiende";
    case "rate_limited":
      return "el servicio pide esperar antes de otro envío";
    case "unauthorized":
      return "el servicio no autoriza el envío";
    default:
      return "error de transporte";
  }
}

export interface PumpText {
  tone: Tone;
  text: string;
  /** Marca (ms Unix) a partir de la cual el aviso ya no es cierto (la espera «faltan N s»). */
  expiresAt?: number;
}

/** El aviso de «Remitir ahora» se muestra mientras no haya caducado. */
export function pumpVisible(pump: PumpText | null, now: number): boolean {
  return pump !== null && (pump.expiresAt === undefined || now < pump.expiresAt);
}

export function pumpReportText(report: VerifactuPumpReport, now: number = Date.now()): PumpText {
  switch (report.state) {
    case "idle":
      return { tone: "neutral", text: "No hay registros pendientes de remitir." };
    case "busy":
      return { tone: "info", text: "Ya hay una remisión en curso: espere a que termine." };
    case "waiting":
      return { tone: "info", text: `Todavía no toca enviar: la AEAT pide una espera entre lotes y faltan ${waitText(report.seconds)}.`, expiresAt: now + Math.max(0, report.seconds) * 1000 };
    case "halted":
      return { tone: "warn", text: "La remisión está parada porque la AEAT devolvió varios errores de esquema (Fault) seguidos. Revise el último lote antes de insistir." };
    case "authorization_rejected":
      return { tone: "error", text: `La AEAT no autoriza este certificado o este representante (código ${report.code}). La remisión de esta base está parada y no se reintenta hasta que cambie el certificado o la configuración, o declare que ya lo arregló.` };
    case "needs_user":
      return { tone: "warn", text: `El certificado necesita su intervención (PIN, tarjeta o confirmación). El lote ${report.batch_id} sigue pendiente; no se ha enviado nada.` };
    case "not_sent":
      return { tone: "warn", text: `No se pudo enviar el lote ${report.batch_id}: ${report.message} No cuenta como intento y el lote se reenviará igual.` };
    case "environment_unavailable":
      return { tone: "warn", text: `El entorno «${report.environment}» todavía no tiene remisión disponible: hoy solo funciona el simulador.` };
    case "sent": {
      const parts = [count(report.accepted, "aceptado", "aceptados")];
      if (report.accepted_with_errors > 0) parts.push(count(report.accepted_with_errors, "aceptado con errores", "aceptados con errores"));
      if (report.rejected > 0) parts.push(count(report.rejected, "rechazado", "rechazados"));
      if (report.unexpected_duplicates > 0) parts.push(count(report.unexpected_duplicates, "duplicado inesperado", "duplicados inesperados"));
      if (report.missing > 0) parts.push(count(report.missing, "sin respuesta (sigue pendiente)", "sin respuesta (siguen pendientes)"));
      const tone: Tone = report.rejected > 0 || report.unexpected_duplicates > 0 ? "error" : report.accepted_with_errors > 0 || report.missing > 0 ? "warn" : "ok";
      return { tone, text: `Lote ${report.batch_id} enviado: ${parts.join(", ")}. La AEAT pide esperar ${waitText(report.wait_seconds)} antes del siguiente.` };
    }
    case "fault": {
      const code = report.code === null ? "" : ` (código ${report.code})`;
      return { tone: "error", text: `La AEAT devolvió un error de esquema (Fault) en el lote ${report.batch_id}${code}: ${report.description}. Sus registros vuelven a estar pendientes.` };
    }
    case "no_response": {
      const detail = report.message ? ` (${report.message})` : "";
      return { tone: "warn", text: `Sin respuesta de la AEAT para el lote ${report.batch_id}: ${transportText(report.code)}${detail}. El lote sigue abierto y se reenviará igual; no se duplica nada.` };
    }
    default:
      return { tone: "neutral", text: "Resultado desconocido." };
  }
}
