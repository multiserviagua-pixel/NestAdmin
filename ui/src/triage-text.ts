// Lógica y textos del triaje de correo en la interfaz (D157-D160), sin DOM: se prueban con `node --test`.

export type NoticeTone = "info" | "warn" | "error";

export interface TriageNotice {
  key: "bounce" | "no_reply" | "weak" | "draft_flags" | "unchecked";
  tone: NoticeTone;
  text: string;
}

/** Lo mínimo de una tarea que necesita esta lógica. */
export interface TriageTask {
  id: number;
  kind: string;
  state: string;
  input_ref?: string;
  created_at: number;
  updated_at?: number;
  result: Record<string, any> | null;
}

export const INFORMATIVE_TAB = "Informativos";
export const INFORMATIVE_INTRO = "Avisos automáticos que no piden respuesta. No se envían a la IA ni se borran.";
export const INFORMATIVE_EMPTY = "No hay correos informativos. Aquí aparecerán los avisos automáticos que no requieren respuesta.";
export const ARCHIVE_LABEL = "Archivar";
export const ARCHIVE_TITLE = "Cierra el correo sin enviar nada. No cuenta como rechazo para las reglas.";
export const ARCHIVED_NOTICE = "Correo archivado";
export const REDRAFT_LABEL = "Redactar respuesta";
export const REDRAFT_TITLE = "La IA prepara un borrador; siempre pasa por su revisión";
export const REDRAFT_NOTICE = "Borrador en preparación: aparecerá en Pendientes";
export const REDRAFT_CONFIRM_YES = "Redactar igualmente";
export const REDRAFT_CONFIRM_NO = "No redactar";
export const NO_REPLY_TEXT = "Probablemente no requiere respuesta. No se ha preparado borrador.";
export const BOUNCE_TEXT = "Posible fallo de entrega: un correo enviado no llegó. Revise el destinatario.";
export const WEAK_TEXT = "Parece un envío masivo: ninguna regla lo responde sin su revisión";
export const UNCHECKED_TEXT = "No se pudieron comprobar los datos del remitente: las reglas no enviarán este correo solas.";

const STRONG_REASON: Record<string, string> = {
  noreply_address: "Enviado desde una dirección que no admite respuestas",
  auto_submitted: "El remitente lo marca como mensaje automático",
  precedence_junk: "El remitente lo marca como correo no solicitado",
};
const STRONG_ORDER = ["noreply_address", "auto_submitted", "precedence_junk"];
const BOUNCE_CODES = ["bounce_address", "null_return_path"];
const WEAK_CODES = ["list_id", "list_unsubscribe", "precedence_bulk", "precedence_list"];
const FLAG_NAME: Record<string, string> = { url: "enlaces", email: "correos", phone: "teléfonos", iban: "IBAN" };
const FLAG_ORDER = ["url", "email", "phone", "iban"];

const codesOf = (value: unknown): string[] => (Array.isArray(value) ? value.filter((c): c is string => typeof c === "string") : []);

/** Informativo (D157): correo cerrado por la ruta local, sin IA y sin acción. */
export function isInformative(task: TriageTask): boolean {
  return task.kind === "email" && task.state === "completed" && task.result?.triage?.route === "informativo";
}

/** Informativos, los más recientes primero (`created_at`, luego `id`). */
export function informativeTasks<T extends TriageTask>(tasks: readonly T[]): T[] {
  return tasks.filter(isInformative).sort((a, b) => b.created_at - a.created_at || b.id - a.id);
}

/** Códigos de señal de un correo: los de `triage` y los del mensaje, sin repetir. */
export function signalCodes(result: Record<string, any> | null): string[] {
  const all = [...codesOf(result?.triage?.codes), ...codesOf(result?.message?.signals)];
  return [...new Set(all)];
}

/** Señales fuertes presentes (D157), en orden fijo. */
export function strongCodes(result: Record<string, any> | null): string[] {
  const codes = signalCodes(result);
  return STRONG_ORDER.filter((c) => codes.includes(c));
}

export const REPLIED_REASON = "Ya respondió en este hilo";
export const PREFERENCE_REASON = "Usted lo marcó: este remitente va siempre a Informativos";
export const PREFERENCE_DOMAIN_REASON = "Usted lo marcó: este dominio va siempre a Informativos";

/** Motivo legible por el que un correo es informativo. Sin motivo conocido, se dice. */
export function informativeReason(result: Record<string, any> | null): string {
  // El hilo y la preferencia van antes que las señales (D161): son lo que decidió.
  if (result?.triage?.source === "thread" || result?.triage?.already_replied === true) return REPLIED_REASON;
  if (result?.triage?.source === "preference") return result.triage.preference_scope === "domain" ? PREFERENCE_DOMAIN_REASON : PREFERENCE_REASON;
  const strong = strongCodes(result);
  return strong.length ? strong.map((c) => STRONG_REASON[c]).join(" · ") : "Motivo no consta";
}

export function isBounce(result: Record<string, any> | null): boolean {
  return result?.triage?.route === "rebote" || signalCodes(result).some((c) => BOUNCE_CODES.includes(c));
}

export function hasWeakSignal(result: Record<string, any> | null): boolean {
  return signalCodes(result).some((c) => WEAK_CODES.includes(c));
}

/** Señales sin comprobar (D157): `message.signals` ausente o nulo. */
export function signalsUnchecked(result: Record<string, any> | null): boolean {
  return !Array.isArray(result?.message?.signals);
}

/** «enlaces, correos o IBAN»: solo los tipos presentes, unidos con comas y «o». */
export function draftFlagList(flags: unknown): string[] {
  const present = codesOf(flags);
  const known = FLAG_ORDER.filter((f) => present.includes(f)).map((f) => FLAG_NAME[f]!);
  const unknown = present.filter((f) => !FLAG_ORDER.includes(f));
  return [...known, ...unknown];
}

export function draftFlagsText(flags: unknown): string | null {
  const list = draftFlagList(flags);
  if (!list.length) return null;
  const joined = list.length === 1 ? list[0]! : `${list.slice(0, -1).join(", ")} o ${list[list.length - 1]}`;
  return `El borrador incluye ${joined}. Compruébelos antes de enviar; ninguna regla lo enviará solo.`;
}

export function hasDraft(result: Record<string, any> | null): boolean {
  const draft = result?.draft;
  return draft !== null && typeof draft === "object" && typeof draft.body === "string" && draft.body.trim() !== "";
}

/** Avisos de un correo en revisión, según el resultado del triaje. */
export function triageNotices(result: Record<string, any> | null): TriageNotice[] {
  const notices: TriageNotice[] = [];
  const bounce = isBounce(result);
  if (bounce) notices.push({ key: "bounce", tone: "error", text: BOUNCE_TEXT });
  else if (result?.needs_reply === false) notices.push({ key: "no_reply", tone: "info", text: NO_REPLY_TEXT });
  if (hasWeakSignal(result)) notices.push({ key: "weak", tone: "warn", text: WEAK_TEXT });
  const flags = draftFlagsText(result?.draft_flags);
  if (flags) notices.push({ key: "draft_flags", tone: "warn", text: flags });
  if (signalsUnchecked(result)) notices.push({ key: "unchecked", tone: "warn", text: UNCHECKED_TEXT });
  return notices;
}

export interface ReviewEmailActions {
  /** Hay borrador que editar y aprobar. */
  editor: boolean;
  approve: boolean;
  /** Por qué no hay «Aprobar y enviar», si no lo hay. */
  approveHint: string | null;
  archive: boolean;
  redraft: boolean;
}

/** Qué ofrece el detalle de un correo en revisión. */
export function reviewEmailActions(result: Record<string, any> | null): ReviewEmailActions {
  const editor = hasDraft(result);
  const noAnswer = isBounce(result) || result?.needs_reply === false;
  // Una tarea que ya intentó enviar no se archiva: se confirma o se descarta (D159).
  const attempted = Boolean(result?.attempt) || Boolean(result?.recovery);
  const redrafted = result?.redraft_of !== undefined && result?.redraft_of !== null;
  return {
    editor,
    approve: editor,
    approveHint: editor ? null : "No hay borrador que enviar. Puede archivar el correo, descartarlo o pedir una respuesta.",
    archive: !attempted,
    // Un correo cerrado porque ya se respondió fuera de NestAdmin (D178) no admite otra respuesta.
    redraft: noAnswer && !redrafted && result?.closed?.reason !== "replied_outside",
  };
}

/** Un remitente con señal fuerte: pedir respuesta exige confirmación. */
export function redraftNeedsConfirm(result: Record<string, any> | null): boolean {
  return strongCodes(result).length > 0;
}

export function redraftConfirmText(from: unknown): string {
  const who = typeof from === "string" && from.trim() ? from : "este remitente";
  return `¿Redactar una respuesta a ${who}? Esta dirección no admite respuestas: es probable que nadie la lea.`;
}

/** Tarea de redacción a mano ya pedida para este correo (D159: una sola por correo). */
export function redraftFor<T extends TriageTask>(tasks: readonly T[], id: number): T | null {
  return (
    tasks.find((t) => {
      if (t.state === "cancelled" || t.kind !== "email") return false;
      if (t.result?.redraft_of === id) return true;
      try {
        return JSON.parse(t.input_ref ?? "null")?.redraft_of === id;
      } catch {
        return false;
      }
    }) ?? null
  );
}
