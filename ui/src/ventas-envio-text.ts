// Textos y reglas puras del envío de documentos de venta por correo (E4b, D232–D241), sin DOM: se
// prueban con `node --test`. Solo tipos borrables. Todos los ejemplos son inventados.
//
// La revisión del envío vive en Pendientes (una sola vía de aprobación); el detalle del documento
// solo lo prepara, lleva a la tarjeta y muestra el historial.

import type { SalesDocument, SalesKind, SalesSend, SalesStatus } from "./types";
import { KIND_LABEL } from "./ventas-text.ts";

/** Tipo de la tarea de envío de un documento de venta. */
export const SEND_KIND = "send_sales_document";

export const SEND_BUTTON = "Enviar por correo";
export const SEND_OPEN_BUTTON = "Ver el envío pendiente";
export const SEND_CHECK_BUTTON = "Comprobar si salió";
export const SEND_APPROVE_BUTTON = "Aprobar y enviar";
export const SEND_SAVE_BUTTON = "Guardar cambios";
export const SEND_DISCARD_BUTTON = "Descartar";
export const SEND_CONFIRM_NOT_SENT_BUTTON = "Confirmo que no se envió: preparar uno nuevo";
export const SEND_OPEN_PDF_BUTTON = "Abrir el PDF";
export const SEND_HISTORY_TITLE = "Envíos por correo";
export const SEND_HISTORY_EMPTY = "Todavía no se ha enviado por correo.";

// ---- Qué documentos se pueden enviar ----

/** Estados enviables por tipo (D233): el albarán admite además `delivered`. */
export function isSendableStatus(kind: SalesKind, status: SalesStatus): boolean {
  if (kind === "delivery_note") return status === "issued" || status === "sent" || status === "delivered";
  return status === "issued" || status === "sent";
}

/** «Enviar por correo» solo existe para un documento emitido, vigente y en un estado enviable. */
export function canSendByMail(doc: Pick<SalesDocument, "kind" | "status" | "number" | "superseded_by">): boolean {
  return doc.number !== null && doc.superseded_by === null && isSendableStatus(doc.kind, doc.status);
}

// ---- Lo que dice la tarea ----

/** Lo mínimo de la tarea de envío que necesita esta lógica. */
export interface SendTaskLike {
  id: number;
  kind: string;
  state: string;
  error?: { code?: string | null; message: string } | null;
  result: Record<string, any> | null;
}

export function isSendTask(task: { kind: string }): boolean {
  return task.kind === SEND_KIND;
}

/** «Factura F-2026-0001» (con « (revisión N)» desde la segunda). */
export function sendDocumentLabel(result: Record<string, any> | null | undefined): string {
  const d = result?.document ?? {};
  const kind = KIND_LABEL[d.kind as SalesKind] ?? "Documento";
  const number = typeof d.number === "string" && d.number ? d.number : `n.º ${d.id ?? "?"}`;
  const revision = typeof d.revision === "number" && d.revision > 1 ? ` (revisión ${d.revision})` : "";
  return `${kind} ${number}${revision}`;
}

/** Destinatario del borrador, o `null` si no consta. */
export function sendRecipient(result: Record<string, any> | null | undefined): string | null {
  const to = result?.draft?.to;
  return Array.isArray(to) && typeof to[0] === "string" && to[0].trim() ? to[0].trim() : null;
}

/** Título de la tarea en Pendientes y Tareas: «Enviar Factura F-2026-0001 a compras@…». */
export function sendTaskTitle(result: Record<string, any> | null | undefined): string {
  const to = sendRecipient(result);
  return `Enviar ${sendDocumentLabel(result)} ${to ? `a ${to}` : "(sin destinatario)"}`;
}

export interface SendNotice {
  tone: "warn" | "error";
  text: string;
}

/**
 * Avisos de la tarjeta de revisión: el cliente sin correo, el destinatario que no coincide con el
 * del cliente (no bloquea), la cuenta ausente y el PDF que no se pudo preparar.
 */
export function sendNotices(result: Record<string, any> | null | undefined): SendNotice[] {
  const r = result ?? {};
  const notices: SendNotice[] = [];
  const to = sendRecipient(r);
  const customer = typeof r.customer_email_at_prepare === "string" && r.customer_email_at_prepare ? r.customer_email_at_prepare : null;
  if (!customer) {
    notices.push({ tone: "warn", text: `El cliente no tiene correo${to ? "; el destinatario lo ha escrito usted" : ": escriba el destinatario para poder enviarlo"}.` });
  }
  if (r.warnings?.recipient_differs === true) {
    notices.push({ tone: "warn", text: `El destinatario no coincide con el correo del cliente${customer ? ` (${customer})` : ""}. Compruébelo antes de enviar.` });
  }
  if (r.checks?.account === "missing") notices.push({ tone: "error", text: "No hay cuenta elegida: elija una cuenta de correo conectada." });
  if (r.checks?.pdf === "too_large") notices.push({ tone: "error", text: "El PDF supera los 3 MB y no se puede enviar." });
  else if (r.checks?.pdf === "failed") notices.push({ tone: "error", text: "No se pudo preparar el PDF del documento." });
  if (r.pdf_changed) notices.push({ tone: "warn", text: "El PDF del documento cambió desde la última revisión: compruébelo de nuevo antes de aprobar." });
  return notices;
}

/** ¿Se puede aprobar con lo que hay? (la validación completa es la del backend). */
export function sendApproveBlock(result: Record<string, any> | null | undefined, live?: { to?: string; account?: string }): string | null {
  const r = result ?? {};
  const to = live?.to !== undefined ? live.to.trim() : sendRecipient(r);
  const account = live?.account !== undefined ? live.account.trim() : r.account?.id;
  if (!to) return "Sin destinatario: escríbalo para poder enviar.";
  if (!account) return "Sin cuenta de correo: elija una para poder enviar.";
  if (r.checks?.pdf === "too_large") return "El PDF supera los 3 MB y no se puede enviar.";
  if (r.checks?.pdf === "failed") return "No se pudo preparar el PDF: no se puede enviar.";
  if (r.recovery?.needs_confirmation === true) return "Para aprobar, confirme primero que el intento anterior no se envió.";
  return null;
}

/** Adjunto de la tarjeta: nombre y tamaño. `null` si no consta. */
export function sendAttachment(result: Record<string, any> | null | undefined): { name: string; size: number } | null {
  const a = result?.draft?.attachments?.[0];
  return a && typeof a.name === "string" ? { name: a.name, size: Number(a.size) || 0 } : null;
}

/** «182 KB» / «1,2 MB» (decimales con coma, sin depender de la configuración regional). */
export function formatSize(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) return "tamaño desconocido";
  if (bytes < 1000) return `${bytes} B`;
  if (bytes < 1_000_000) return `${Math.round(bytes / 1000)} KB`;
  return `${(bytes / 1_000_000).toFixed(1).replace(".", ",")} MB`;
}

/** Texto de la confirmación previa al envío: destinatario, documento y cuenta. */
export function sendConfirmText(result: Record<string, any> | null | undefined, to: string, accountLabel: string): string {
  return `Se enviará ${sendDocumentLabel(result)} con su PDF adjunto a ${to}, desde ${accountLabel}. Un correo enviado no se puede retirar.\n\n¿Enviar ahora?`;
}

/** Confirmación del descarte de un envío pendiente. */
export function sendDiscardText(result: Record<string, any> | null | undefined): string {
  return `Se descartará el envío de ${sendDocumentLabel(result)}. El documento no cambia y no se envía nada.\n\n¿Descartarlo?`;
}

/** Aviso tras aprobar según cómo terminó la tarea. */
export function sendOutcome(task: SendTaskLike): { kind: "ok" | "warn" | "error"; text: string } {
  const r = task.result ?? {};
  if (task.state === "completed") {
    const to = sendRecipient(r);
    const extra = r.status_error ? ` Aviso: ${r.status_error}` : "";
    return { kind: r.status_error ? "warn" : "ok", text: `Enviado${to ? ` a ${to}` : ""}. La entrega al destinatario no se comprueba.${extra}` };
  }
  if (task.state === "failed") {
    return { kind: "error", text: `No se pudo enviar: ${task.error?.message ?? "la acción falló"}. No consta que haya salido; use «${SEND_CHECK_BUTTON}» antes de repetirlo.` };
  }
  return { kind: "warn", text: `No se ha enviado: ${r.review_reason ?? "requiere revisión"}` };
}

// ---- Detalle del documento: tarea abierta e historial ----

export interface OpenSend {
  tone: "info" | "warn" | "error";
  text: string;
  /** Qué ofrece el detalle: ir a la tarjeta, comprobar si salió o esperar. */
  action: "review" | "check" | "wait";
}

/** Estado de la tarea de envío abierta de un documento. */
export function openSendStatus(task: SendTaskLike): OpenSend {
  if (task.state === "awaiting_review") {
    const why = task.result?.review_reason;
    return { tone: "info", action: "review", text: `Hay un envío pendiente de revisión${why ? `: ${why}` : ""}. Se aprueba en Pendientes.` };
  }
  if (task.state === "failed") {
    return { tone: "error", action: "check", text: `El último intento falló: ${task.error?.message ?? "la acción falló"}. No consta que haya salido: compruébelo antes de repetirlo.` };
  }
  return { tone: "info", action: "wait", text: "El envío está en curso." };
}

/** Quién autorizó el envío. */
export function authorizationText(authorization: string): string {
  if (authorization === "aprobación") return "Aprobado por usted";
  const rule = /^regla (\S+) v(\d+)$/.exec(authorization);
  return rule ? `Regla automática «${rule[1]}» (versión ${rule[2]})` : `Autorización: ${authorization}`;
}

/** Fecha y hora de un envío (segundos Unix). */
export function sentAtText(seconds: number, timeZone?: string): string {
  const date = new Date(seconds * 1000);
  if (Number.isNaN(date.getTime())) return "Sin fecha";
  return new Intl.DateTimeFormat("es-ES", { dateStyle: "short", timeStyle: "short", ...(timeZone ? { timeZone } : {}) }).format(date);
}

export interface SendHistoryRow {
  when: string;
  recipient: string;
  account: string;
  authorization: string;
  attachment: string;
}

/** Filas del historial, del más reciente al más antiguo. */
export function sendHistoryRows(sends: SalesSend[], timeZone?: string): SendHistoryRow[] {
  return [...sends]
    .sort((a, b) => b.sent_at - a.sent_at || b.id - a.id)
    .map((s) => ({
      when: sentAtText(s.sent_at, timeZone),
      recipient: s.recipient,
      account: s.account_address ?? s.account,
      authorization: authorizationText(s.authorization),
      attachment: `${s.attachment_name} (${formatSize(s.attachment_size)})`,
    }));
}
