// Textos y formato del chat de la aplicación (E1, D181), sin DOM: se prueban con `node --test`.
// En esta etapa el chat contesta con una búsqueda por palabras hecha en el equipo, sin IA.
// Solo tipos borrables (sin `enum`). Todos los ejemplos son inventados.

export interface HitLike {
  kind: string;
  title: string;
  party: string;
  number: string;
  doc_date: string | null;
  total_minor: number | null;
  currency: string;
  index_status: string;
  snippet: string;
}

export interface CountsLike {
  pending_index: number;
  no_text: number;
  failed: number;
}

export const CHAT_BUTTON = "Chat";
export const CHAT_BUTTON_TITLE = "Preguntar al chat (Ctrl+K)";
export const CHAT_TITLE = "Chat";
export const CHAT_LABEL = "Chat de NestAdmin";
export const CHAT_CLOSE = "Cerrar el chat";
export const CHAT_INPUT_LABEL = "Su pregunta";
export const CHAT_INPUT_PLACEHOLDER = "Pregunte por un documento, una factura o un dato";
export const CHAT_SEND = "Preguntar";
export const CHAT_BUSY = "Buscando en este equipo…";
export const CHAT_EMPTY_TITLE = "Pregunte con sus palabras";
export const CHAT_EMPTY_HINT = "Nada se busca mientras escribe: la búsqueda empieza al pulsar Intro o «Preguntar».";
export const CHAT_EXAMPLES = [
  "¿Dónde está la factura de Suministros Ejemplo?",
  "Correo de Taller Demo sobre un presupuesto",
  "Facturas de marzo de 2026",
];
export const CHAT_EXAMPLES_LABEL = "Ejemplos de pregunta";

/** Aviso fijo de cada respuesta: lo que es y lo que no es. No promete fecha ni activación. */
export const SEARCH_NOTICE =
  "Es una búsqueda por palabras hecha en este equipo, no una respuesta de la IA. El asistente con IA llegará en una versión posterior.";

export const REINDEX_BUTTON = "Volver a intentar los que fallaron";
export const REINDEX_BUSY = "Volviendo a intentarlo…";

export const YOU_LABEL = "Usted";
export const ASSISTANT_LABEL = "NestAdmin";

export function plural(n: number, one: string, many: string): string {
  return n === 1 ? one : many;
}

/** Lo buscado, entre comillas angulares. */
export function termsText(terms: readonly string[]): string {
  return terms.map((t) => `«${t}»`).join(", ");
}

/** Respuesta según lo que coincidió. `total` es el número de coincidencias totales. */
export function answerText(matched: string, total: number, terms: readonly string[]): string {
  if (matched === "none" || total <= 0) {
    if (terms.length === 0)
      return "No encontré palabras útiles en su pregunta para buscar. Indíqueme el proveedor, el número de factura o el mes.";
    return `No encontré nada con ${termsText(terms)}. Pruebe con otras palabras, por ejemplo el proveedor, el número o el mes.`;
  }
  const count = `${total} ${plural(total, "documento", "documentos")}`;
  if (matched === "any") return `No encontré nada con todas las palabras; esto coincide con alguna. Hay ${count}.`;
  return total === 1 ? "Encontré 1 documento con todas las palabras." : `Encontré ${count} con todas las palabras.`;
}

/** Líneas de recuentos del índice; vacío si no hay nada que avisar. */
export function countLines(page: CountsLike): string[] {
  const lines: string[] = [];
  if (page.pending_index > 0)
    lines.push(
      `Aún se ${plural(page.pending_index, "está preparando", "están preparando")} ${page.pending_index} ${plural(page.pending_index, "documento", "documentos")}.`,
    );
  if (page.no_text > 0) lines.push(`${page.no_text} sin texto legible.`);
  if (page.failed > 0) lines.push(`${page.failed} no se ${plural(page.failed, "pudo", "pudieron")} leer.`);
  return lines;
}

/** «Mostrando 50 de N» solo cuando hay más resultados que los devueltos. */
export function shownLine(shown: number, total: number): string | null {
  return total > shown ? `Mostrando ${shown} de ${total}` : null;
}

export function kindLabel(kind: string): string {
  return kind === "invoice" ? "Factura" : kind === "email" ? "Correo" : "Documento";
}

/** Fecha «AAAA-MM-DD» (con hora opcional) como «DD/MM/AAAA»; si no se reconoce, tal cual. */
export function formatDate(value: string | null | undefined): string | null {
  if (value === null || value === undefined) return null;
  const text = value.trim();
  if (!text) return null;
  const m = /^(\d{4})-(\d{2})-(\d{2})(?:$|[T ])/.exec(text);
  return m ? `${m[3]}/${m[2]}/${m[1]}` : text;
}

const SYMBOL: Record<string, string> = { EUR: "€", USD: "$", GBP: "£" };

/** Céntimos enteros como «1.234,56 €», sin pasar por coma flotante. Sin importe: `null`. */
export function formatMinor(minor: number | null | undefined, currency: string | null | undefined): string | null {
  if (minor === null || minor === undefined || !Number.isFinite(minor)) return null;
  const cents = Math.round(minor);
  const abs = Math.abs(cents);
  const units = Math.floor(abs / 100)
    .toString()
    .replace(/\B(?=(\d{3})+(?!\d))/g, ".");
  const number = `${cents < 0 ? "-" : ""}${units},${String(abs % 100).padStart(2, "0")}`;
  const code = (currency ?? "").trim().toUpperCase();
  if (!/^[A-Z]{3}$/.test(code)) return number;
  return `${number} ${SYMBOL[code] ?? code}`;
}

export function indexNote(status: string): string | null {
  if (status === "no_text") return "Sin texto legible";
  if (status === "failed") return "No se pudo leer";
  return null;
}

export interface CardView {
  kind: string;
  title: string;
  /** Parte, número, fecha e importe, solo los que constan. */
  meta: string[];
  note: string | null;
  snippet: string;
}

export function cardView(hit: HitLike): CardView {
  const meta: string[] = [];
  if (hit.party.trim()) meta.push(hit.party.trim());
  if (hit.number.trim()) meta.push(`N.º ${hit.number.trim()}`);
  const date = formatDate(hit.doc_date);
  if (date) meta.push(date);
  const amount = formatMinor(hit.total_minor, hit.currency);
  if (amount) meta.push(amount);
  return {
    kind: kindLabel(hit.kind),
    title: hit.title.trim() || "Sin título",
    meta,
    note: indexNote(hit.index_status),
    snippet: hit.snippet.trim(),
  };
}

export function cardLabel(hit: HitLike): string {
  const v = cardView(hit);
  return `Abrir ${v.kind.toLowerCase()}: ${[v.title, ...v.meta].join(", ")}`;
}

export const NOT_OPENABLE = "Esta tarea ya no está en Pendientes ni tiene vista de revisión.";
export const NOT_FOUND_TASK = "Esta tarea ya no está en la lista de la aplicación.";

export function searchErrorText(error: unknown): string {
  const text = String(error ?? "").trim();
  return text ? `No se pudo buscar: ${text}` : "No se pudo buscar. Vuelva a intentarlo.";
}

export function reindexLine(n: number): string {
  if (n <= 0) return "No había documentos que volver a intentar.";
  return `Se ${plural(n, "volverá", "volverán")} a intentar ${n} ${plural(n, "documento", "documentos")}. Aparecerán en las búsquedas cuando estén listos.`;
}

export function reindexErrorText(error: unknown): string {
  const text = String(error ?? "").trim();
  return text ? `No se pudo reintentar: ${text}` : "No se pudo reintentar. Vuelva a intentarlo.";
}
