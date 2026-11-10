// Textos y formato del chat (E1, D181) y del asistente con IA (E2, D182-D184, D188), sin DOM: se prueban
// con `node --test`. Sin asistente utilizable, el chat contesta con una búsqueda por palabras hecha en el equipo.
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
  /** Estado de la tarea; solo hace falta para avisar de las que esperan a la IA. */
  task_state?: string;
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
export const CHAT_EMPTY_HINT = "Nada se envía ni se busca mientras escribe: la pregunta se hace al pulsar Intro o «Preguntar».";
export const CHAT_EXAMPLES = [
  "¿Dónde está la factura de Suministros Ejemplo?",
  "Correo de Taller Demo sobre un presupuesto",
  "Facturas de marzo de 2026",
];
export const CHAT_EXAMPLES_LABEL = "Ejemplos de pregunta";

/** Aviso fijo de cada búsqueda por palabras: lo que es y lo que no es. */
export const SEARCH_NOTICE = "Es una búsqueda por palabras hecha en este equipo, no una respuesta de la IA.";

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

/** Tipo por el que se filtró la búsqueda: `null` si no se filtró. */
export type AnswerKind = "invoice" | "email" | null | undefined;

function kindNoun(kind: AnswerKind, n: number): string {
  if (kind === "invoice") return plural(n, "factura", "facturas");
  if (kind === "email") return plural(n, "correo", "correos");
  return plural(n, "documento", "documentos");
}

function noneOf(kind: AnswerKind): string {
  return kind === "invoice" ? "ninguna factura" : kind === "email" ? "ningún correo" : "nada";
}

/** Respuesta según lo que coincidió. `total` es el número de coincidencias totales. */
export function answerText(matched: string, total: number, terms: readonly string[], kind?: AnswerKind): string {
  if (matched === "none" || total <= 0) {
    if (terms.length === 0) {
      if (kind) return `No encontré ${noneOf(kind)} en este equipo.`;
      return "No encontré palabras útiles en su pregunta para buscar. Indíqueme el proveedor, el número de factura o el mes.";
    }
    return `No encontré ${noneOf(kind)} con ${termsText(terms)}. Pruebe con otras palabras, por ejemplo el proveedor, el número o el mes.`;
  }
  const count = `${total} ${kindNoun(kind, total)}`;
  // Sin palabras, solo el tipo: se listan los más recientes.
  if (terms.length === 0)
    return total === 1
      ? `${kind === "invoice" ? "Esta es la factura" : `Este es el ${kindNoun(kind, 1)}`} más reciente.`
      : `Estas son las ${count} más recientes.`;
  if (matched === "any")
    return `No encontré ${noneOf(kind)} con todas las palabras; esto coincide con alguna. Hay ${count}.`;
  return `Encontré ${count} con todas las palabras.`;
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

/** Etiqueta de una tarea que espera en cola sin IA utilizable (D179): solo se ha indexado en el equipo. */
export const QUEUED_NOTE = "En espera de la IA: aún no se ha leído";

export function indexNote(status: string, taskState?: string): string | null {
  if (status === "no_text") return "Sin texto legible";
  if (status === "failed") return "No se pudo leer";
  if (taskState === "queued") return QUEUED_NOTE;
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
    note: indexNote(hit.index_status, hit.task_state),
    snippet: hit.snippet.trim(),
  };
}

export function cardLabel(hit: HitLike): string {
  const v = cardView(hit);
  return `Abrir ${v.kind.toLowerCase()}: ${[v.title, ...v.meta].join(", ")}`;
}

/** Acción secundaria de la tarjeta de una factura: abre la copia guardada con el programa del sistema. */
export const OPEN_ORIGINAL_LABEL = "Abrir el documento";
export const OPEN_ORIGINAL_PREVIEW = "Vista previa: en la aplicación se abriría la copia guardada con el programa del sistema.";

export function canOpenOriginal(hit: Pick<HitLike, "kind">): boolean {
  return hit.kind === "invoice";
}

export function originalLabel(hit: HitLike): string {
  return `${OPEN_ORIGINAL_LABEL}: ${cardView(hit).title}`;
}

/** El mensaje del fondo tal cual (por ejemplo, que la copia ya no está); con vacío, uno genérico. */
export function originalErrorText(error: unknown): string {
  const text = String(error ?? "").trim();
  return text || "No se pudo abrir el documento.";
}
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

// ---- Asistente con IA (E2) ----

/** Estructuras mínimas (solo tipos borrables): las reales están en `types.ts`. */
export interface WhoLike {
  provider: string | null;
  model: string | null;
  local: boolean;
  fallback: { from: string; reason: string } | null;
}

export interface LimitsLike {
  steps: number;
  history_turns: number;
  turn_chars: number;
  fragments: number;
  fragment_chars: number;
  data_chars: number;
}

export interface ConsentLike {
  state: string;
  provider: string | null;
  model: string | null;
  host: string | null;
  at: number | null;
}

export interface SentLike {
  step: number;
  provider: string;
  model: string;
  host: string;
  local: boolean;
}

export const ASSISTANT_BUSY = "Pensando…";
export const CHAT_CANCEL = "Cancelar";
export const CHAT_CANCELLING = "Cancelando…";
export const CANCELLED_TEXT = "Pregunta cancelada.";
export const LOADING_PREVIOUS = "Recuperando la conversación anterior…";
export const CLEAR_CONVERSATION = "Borrar conversación";
export const CLEAR_CONVERSATION_CONFIRM =
  "Se borrará esta conversación y las propuestas sin confirmar que tenga. Solo existe en este equipo y no se puede recuperar. ¿Borrar?";
export const WORDS_BUTTON = "Buscar por palabras";
export const SETTINGS_BUTTON = "Abrir Configuración";
export const WORDS_INTRO = "Mientras tanto, esto es lo que encuentra una búsqueda por palabras:";
export const NOTHING_CHANGED = "No se ha cambiado nada en NestAdmin.";

const PROVIDER_NAME: Record<string, string> = {
  ollama: "Ollama (local)",
  ollama_cloud: "Ollama cloud",
  openai_compatible: "Compatible con OpenAI",
  anthropic: "Claude · Anthropic",
};

export function providerName(provider: string | null | undefined): string {
  if (!provider) return "el proveedor de IA";
  return PROVIDER_NAME[provider] ?? provider;
}

const FALLBACK_REASON: Record<string, string> = {
  timeout: "no respondió a tiempo",
  rate_limited: "alcanzó su límite de uso",
  unavailable: "no estaba disponible",
};

/** Quién contestó, en una línea discreta bajo la respuesta; vacío si no consta. */
export function whoAnswered(who: WhoLike): string {
  if (who.fallback) {
    const reason = FALLBACK_REASON[who.fallback.reason] ?? "no respondió";
    return `El proveedor externo (${providerName(who.fallback.from)}) ${reason} y contestó la IA local de este equipo.`;
  }
  if (who.local) return "Respondió la IA local de este equipo";
  if (who.provider && who.model) return `Respondió ${providerName(who.provider)} · ${who.model}`;
  return "";
}

export function droppedCitationsText(n: number): string | null {
  if (n <= 0) return null;
  return n === 1
    ? "Se descartó 1 referencia que no venía de sus documentos."
    : `Se descartaron ${n} referencias que no venían de sus documentos.`;
}

export function withheldText(n: number): string | null {
  if (n <= 0) return null;
  return n === 1
    ? "1 fragmento no se envió a la IA por contener datos sensibles."
    : `${n} fragmentos no se enviaron a la IA por contener datos sensibles.`;
}

export const CONSENT_REQUIRED_TEXT =
  "El asistente no puede usar el proveedor de IA externo porque usted aún no ha permitido que envíe datos fuera de este equipo. Puede permitirlo en Configuración, tras ver qué saldría. Hasta entonces no sale nada.";
export const NOT_CONFIGURED_TEXT =
  "Todavía no hay una IA preparada para el asistente. Elija un proveedor de IA o prepare la IA local en Configuración.";

const ERROR_TEXT: Record<string, string> = {
  unauthorized: "El proveedor de IA ha rechazado la clave. Corríjala en Configuración.",
  timeout: "El proveedor de IA tardó demasiado en responder. Puede volver a intentarlo.",
  rate_limited: "El proveedor de IA ha alcanzado su límite de uso. Espere un momento y vuelva a intentarlo.",
  no_credit: "La cuenta del proveedor de IA no tiene crédito.",
  model_unavailable: "El modelo elegido no está disponible en el proveedor. Elija otro en Configuración.",
  request_rejected: "El proveedor de IA ha rechazado la petición.",
  unavailable: "El proveedor de IA no está disponible ahora mismo. Vuelva a intentarlo más tarde.",
  invalid_response: "La IA no devolvió una respuesta que NestAdmin pueda usar.",
};

/** Mensaje de un error del asistente según su código, sin tecnicismos y diciendo que no se cambió nada. */
export function assistantErrorText(code: string | null | undefined): string {
  const base = (code && ERROR_TEXT[code]) || "El asistente no pudo contestar.";
  return `${base} ${NOTHING_CHANGED}`;
}

/** Si la llamada misma falló (sin turno), el mensaje del fondo; con vacío, uno genérico. */
export function assistantCallErrorText(error: unknown): string {
  const text = String(error ?? "").trim();
  return text ? `No se pudo preguntar al asistente: ${text} ${NOTHING_CHANGED}` : assistantErrorText(null);
}

export function loadErrorText(error: unknown): string {
  const text = String(error ?? "").trim();
  return text ? `No se pudo recuperar la conversación anterior: ${text}` : "No se pudo recuperar la conversación anterior.";
}

/** Texto de una respuesta o de un motivo, en párrafos: respeta los saltos de línea sin interpretar nada más. */
export function textParagraphs(text: string): string[] {
  return String(text ?? "")
    .split(/\r?\n/)
    .map((line) => line.trimEnd())
    .filter((line) => line.trim() !== "");
}

/** Vistas a las que el asistente puede llevar (lista cerrada, D184). */
export const NAVIGABLE: Record<string, string> = {
  review: "Pendientes",
  tasks: "Tareas",
  rules: "Reglas",
  history: "Historial",
  mail: "Cuentas de correo",
  settings: "Configuración",
};

/** La vista pedida si está en la lista cerrada; si no, `null`. */
export function navigateTarget(view: string | null | undefined): string | null {
  return view && Object.prototype.hasOwnProperty.call(NAVIGABLE, view) ? view : null;
}

export function navigatedText(view: string): string {
  return `Le he llevado a «${NAVIGABLE[view] ?? view}». El chat sigue abierto.`;
}

export function clearedText(n: number): string {
  if (n <= 0) return "No había conversaciones que borrar.";
  return n === 1 ? "Se borró 1 conversación." : `Se borraron ${n} conversaciones.`;
}

// ---- «Ver lo enviado» (D183) ----

export const SENT_BUTTON = "Ver lo enviado";
export const SENT_TITLE = "Lo enviado en esta respuesta";
export const SENT_BACK = "Volver a la conversación";
export const SENT_LOADING = "Cargando lo enviado…";
export const SENT_INTRO = "Este es el texto literal de cada llamada a la IA de esta respuesta. Se guarda solo en este equipo.";
export const SENT_NONE = "Esta respuesta no usó la IA, así que no se envió nada.";

export function sentError(error: unknown): string {
  const text = String(error ?? "").trim();
  return text ? `No se pudo cargar lo enviado: ${text}` : "No se pudo cargar lo enviado.";
}

export function sentCallTitle(call: SentLike): string {
  return `Llamada ${call.step}: ${providerName(call.provider)} · ${call.model}`;
}

export function sentCallWhere(call: SentLike): string {
  if (call.local) return "IA local de este equipo: no salió nada del equipo.";
  return call.host ? `Servidor: ${call.host}` : "Servicio externo";
}

// ---- Configuración: opción del asistente (D183) ----

export const ASSISTANT_TITLE = "Asistente";
export const ASSISTANT_OPTION = "Permitir que el asistente envíe datos fuera de este equipo";
export const ASSISTANT_INTRO =
  "El asistente del chat responde preguntas sobre sus documentos, correos y el estado de la aplicación, y cita de dónde sale cada dato. Con un proveedor de IA externo solo funciona si usted lo permite aquí, tras ver qué saldría. Con la IA local no sale nada.";
export const ASSISTANT_WITHOUT =
  "Sin esta autorización, el asistente usa solo la IA local si está preparada y, si no, responde con una búsqueda por palabras hecha en este equipo.";
export const ASSISTANT_GRANT_OPEN = "Ver qué saldría y autorizar…";
export const ASSISTANT_ACCEPT = "Aceptar y autorizar";
export const ASSISTANT_WITHDRAW = "Retirar autorización";
export const ASSISTANT_CANCEL = "Cancelar";
export const ASSISTANT_CLEAR_ALL = "Borrar todas las conversaciones";
export const ASSISTANT_CLEAR_ALL_CONFIRM =
  "Se borrarán todas las conversaciones con el asistente y sus propuestas sin confirmar. Solo existen en este equipo y no se pueden recuperar. ¿Borrar todas?";
export const ASSISTANT_PREVIEW_TITLE = "Qué saldría de este equipo";
export const ASSISTANT_SAMPLE_LABEL = "Texto literal de un envío de ejemplo";
export const ASSISTANT_MAY_HEADING = "Puede salir";
export const ASSISTANT_NEVER_HEADING = "Nunca sale";
export const ASSISTANT_LOADING = "Cargando…";

export function assistantLoadError(error: unknown): string {
  const text = String(error ?? "").trim();
  return text ? `No se pudo consultar el permiso del asistente: ${text}` : "No se pudo consultar el permiso del asistente.";
}

/** El rechazo del fondo (por ejemplo, un destino cambiado) tal cual, y se dice que se recargó la vista previa. */
export function grantErrorText(error: unknown): string {
  const text = String(error ?? "").trim();
  return text ? `${text} Se ha vuelto a cargar la vista previa.` : "No se pudo autorizar. Se ha vuelto a cargar la vista previa.";
}

/** Con punto de millar siempre, como en el resto de la interfaz. */
export function thousands(n: number): string {
  return String(n).replace(/\B(?=(\d{3})+(?!\d))/g, ".");
}

/** Fecha y hora local de un instante en segundos o milisegundos; `null` si no consta. */
export function formatStamp(at: number | null | undefined): string | null {
  if (at === null || at === undefined || !Number.isFinite(at)) return null;
  const date = new Date(at < 1e12 ? at * 1000 : at);
  return Number.isNaN(date.getTime()) ? null : date.toLocaleString("es-ES", { dateStyle: "medium", timeStyle: "short" });
}

export function consentStateText(view: ConsentLike): string {
  switch (view.state) {
    case "not_needed":
      return "La IA elegida es local: nada sale de este equipo, así que esta opción no hace falta.";
    case "granted": {
      const where = `${providerName(view.provider)} · ${view.model ?? "modelo sin indicar"}${view.host ? ` (servidor ${view.host})` : ""}`;
      const when = formatStamp(view.at);
      return `Activada para ${where}${when ? `, desde el ${when}` : ""}.`;
    }
    case "stale":
      return "Caducada: el proveedor, el modelo o el servidor han cambiado desde que la autorizó. Hasta que la vuelva a autorizar, el asistente no envía nada.";
    default:
      return "Desactivada: el asistente no envía nada fuera de este equipo.";
  }
}

export function canGrantConsent(view: ConsentLike): boolean {
  return view.state === "missing" || view.state === "stale";
}

export function canWithdrawConsent(view: ConsentLike): boolean {
  return view.state === "granted" || view.state === "stale";
}

export function assistantDestinationText(provider: string, model: string, host: string | null): string {
  return `Destino: ${providerName(provider)} · ${model}${host ? ` · servidor ${host}` : ""}.`;
}

export function limitLines(l: LimitsLike): string[] {
  return [
    `Hasta ${l.steps} pasos por pregunta.`,
    `Como mucho los ${l.history_turns} últimos turnos de la conversación, de ${thousands(l.turn_chars)} caracteres cada uno.`,
    `Hasta ${l.fragments} fragmentos de documentos de ${thousands(l.fragment_chars)} caracteres cada uno, y ${thousands(l.data_chars)} caracteres de datos en total por llamada.`,
  ];
}

export function localFallbackLine(on: boolean): string {
  return on
    ? "Si el proveedor externo no responde, la pregunta se repite una sola vez con la IA local, y la respuesta lo dice."
    : "El respaldo con la IA local está desactivado: si el proveedor no responde, el asistente lo dice.";
}

// ---- Historial (D188) ----

export const ASSISTANT_ACTION_LABEL: Record<string, string> = {
  assistant_turn: "Pregunta al asistente",
  assistant_cleared: "Conversaciones del asistente borradas",
};

const TOOL_LABEL: Record<string, string> = {
  search: "buscar documentos",
  get_document: "leer un documento",
  aggregate: "sumas y recuentos",
  list_tasks: "listar tareas",
  list_rules: "listar reglas",
  history: "historial de una tarea",
  mail_accounts: "cuentas de correo",
  ai_status: "estado de la IA",
  open_view: "abrir una pantalla",
};

/** Detalle del Historial de un evento del asistente, en palabras y sin texto de preguntas ni respuestas; `null` si no es suyo. */
export function assistantAuditText(action: string, detail: string | null, status?: string): string | null {
  if (!action.startsWith("assistant_") || !detail) return null;
  try {
    const d = JSON.parse(detail) as Record<string, unknown>;
    if (action === "assistant_cleared") {
      const n = typeof d.conversations === "number" ? d.conversations : 0;
      return `${d.scope === "all" ? "Todas las conversaciones" : "Una conversación"}. ${clearedText(n)}`;
    }
    if (action !== "assistant_turn") return detail;
    const parts: string[] = [];
    const steps = typeof d.steps === "number" ? d.steps : 0;
    parts.push(steps === 0 ? "Sin pasos con la IA" : `${steps} ${plural(steps, "paso", "pasos")}`);
    const tools = Array.isArray(d.tools) ? d.tools.map((t) => TOOL_LABEL[String(t)] ?? String(t)) : [];
    parts.push(tools.length ? `Herramientas: ${tools.join(", ")}` : "Sin herramientas");
    const who = whoAnswered({
      provider: typeof d.provider === "string" ? d.provider : null,
      model: typeof d.model === "string" ? d.model : null,
      local: d.local === true,
      fallback: typeof d.fallback === "string" ? { from: typeof d.provider === "string" ? d.provider : "", reason: d.fallback } : null,
    });
    // Quién contestó solo consta si hubo respuesta; en un error o una cancelación no se afirma.
    if (who && steps > 0 && (status === undefined || status === "answered" || status === "fallback")) parts.push(who.replace(/\.$/, ""));
    const cited = typeof d.citations === "number" ? d.citations : 0;
    parts.push(`${cited} ${plural(cited, "documento citado", "documentos citados")}`);
    const dropped = typeof d.dropped_citations === "number" ? d.dropped_citations : 0;
    if (dropped > 0) parts.push(`${dropped} ${plural(dropped, "referencia descartada", "referencias descartadas")}`);
    const withheld = typeof d.withheld === "number" ? d.withheld : 0;
    if (withheld > 0) parts.push(`${withheld} ${plural(withheld, "fragmento retenido", "fragmentos retenidos")}`);
    if (typeof d.error_code === "string" && d.error_code) parts.push(assistantErrorText(d.error_code).replace(` ${NOTHING_CHANGED}`, ""));
    return parts.join(" · ");
  } catch {
    return detail;
  }
}
