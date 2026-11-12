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

/** Lo que hace cada herramienta del asistente, para el indicador mientras trabaja (D182). */
export const TOOL_PROGRESS: Record<string, string> = {
  search: "Buscando en sus documentos…",
  get_document: "Leyendo un documento…",
  aggregate: "Calculando…",
  list_tasks: "Repasando las tareas…",
  list_rules: "Consultando las reglas…",
  history: "Consultando el historial…",
  mail_accounts: "Consultando las cuentas de correo…",
  ai_status: "Comprobando la IA…",
  open_view: "Preparando la vista…",
};

/** Texto del indicador según el paso en curso; sin paso o con una herramienta desconocida, «Pensando…». */
export function progressText(step: { kind: string; name?: string } | null | undefined): string {
  if (step?.kind === "tool" && step.name && Object.hasOwn(TOOL_PROGRESS, step.name)) return TOOL_PROGRESS[step.name];
  return ASSISTANT_BUSY;
}
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

/** Nota discreta cuando la aplicación quitó enlaces o marcas de la respuesta (D264); `null` si no quitó ninguno. */
export function linksRemovedText(n: number | null | undefined): string | null {
  const count = typeof n === "number" && Number.isFinite(n) ? Math.trunc(n) : 0;
  if (count <= 0) return null;
  return count === 1 ? "Se retiró 1 enlace de la respuesta." : `Se retiraron ${count} enlaces de la respuesta.`;
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
  assistant_proposal: "Propuesta del asistente",
};

/** Marca junto a una acción del Historial ejecutada desde una tarjeta del chat (`origin` = `chat`, D185). */
export const CHAT_ORIGIN_LABEL = "Desde el chat";

/** ¿La entrada del Historial viene de una tarjeta del chat? */
export function isFromChat(origin: string | null | undefined): boolean {
  return origin === "chat";
}

const TOOL_LABEL: Record<string, string> = {
  search: "buscar documentos",
  get_document: "leer un documento",
  aggregate: "sumas y recuentos",
  list_tasks: "listar tareas",
  list_rules: "listar reglas",
  list_suggestions: "listar sugerencias",
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
    if (action === "assistant_proposal") return proposalAuditText(d);
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

// ---- Propuestas del asistente (E3, D185-D187): tarjetas con botón ----
// La aplicación compone la tarjeta con los datos reales del equipo; el modelo no la redacta ni la confirma.
// Nada se ejecuta hasta que la persona pulsa el botón de la tarjeta: un «sí» escrito en el chat no confirma nada.

export interface ProposalLike {
  kind: string;
  status: string;
  effect: string;
  summary: Record<string, any>;
  receipt?: { confirmed_at: number; reconciled: boolean; result: Record<string, any>; outcome?: string; reason?: string | null } | null;
}

export const PROPOSAL_KINDS = [
  "approve",
  "reject",
  "retry",
  "archive",
  "confirm_not_sent",
  "edit_draft",
  "fetch_mail",
  "mail_limit",
  "rename_account",
  "sender_preference",
  "accept_suggestion",
  "discard_suggestion",
  "create_rule",
  "edit_rule",
  "import",
] as const;

export const PROPOSAL_NOTHING_DONE = "Nada se ha hecho todavía. Solo se ejecuta si usted pulsa el botón.";
export const CARD_DISMISS = "No hacerlo";
export const PROPOSAL_EXECUTING = "Ejecutando…";
export const PROPOSAL_OPEN_TASK = "Abrir la tarea";
export const PROPOSAL_OPEN_RULE = "Abrir en Reglas";
/** Código cerrado con el que la ventana dice que la persona cerró el diálogo sin elegir archivos. */
export const IMPORT_CANCELLED_CODE = "import_cancelled";
export const IMPORT_CANCELLED_NOTE = "No se eligió ningún archivo: la propuesta sigue pendiente y no se ha hecho nada.";
export const INACTIVE_NOTICE = "La regla quedará inactiva: no actuará hasta que usted la active en Reglas.";
export const CONDITIONS_LABEL = "Condiciones";
export const PROPOSAL_LIVE_LABEL = "Estado de la propuesta";
export const AMOUNTS_CHECKED_LABEL = "He comprobado los importes con el documento";
export const AMOUNTS_REQUIRED_HINT = "Para registrar la factura, marque antes que ha comprobado los importes con el documento.";
export const OTHER_DOCUMENTS_TITLE = "El texto propuesto incluye datos de otros documentos";
export const OTHER_DOCUMENTS_CHECKED_LABEL = "He revisado que estos datos deben ir en la respuesta";
export const OTHER_DOCUMENTS_REQUIRED_HINT = "Para continuar, marque antes que ha revisado que esos datos de otros documentos deben ir en la respuesta.";
export const BODY_LABEL = "Texto completo del correo";
export const CURRENT_TEXT_LABEL = "Texto actual";
export const NEW_TEXT_LABEL = "Texto propuesto por la IA";
export const PROPOSAL_STUCK_TEXT =
  "Un intento anterior no terminó y no consta si se hizo. Si pulsa el botón, NestAdmin comprobará primero si ya se hizo y no lo repetirá.";

/** Qué es la propuesta, dicho con claridad y sin dar nada por hecho. */
export function proposalHeading(kind: string, effect?: string): string {
  switch (kind) {
    case "approve":
      return effect === "register_invoice" ? "Propuesta: registrar esta factura" : "Propuesta: enviar esta respuesta";
    case "reject":
      return "Propuesta: descartar esta tarea";
    case "retry":
      return "Propuesta: reintentar esta tarea";
    case "archive":
      return "Propuesta: archivar este correo";
    case "confirm_not_sent":
      return "Propuesta: confirmar que el correo no se envió";
    case "edit_draft":
      return "Propuesta: cambiar el borrador";
    case "fetch_mail":
      return "Propuesta: consultar el correo ahora";
    case "mail_limit":
      return "Propuesta: cambiar el límite de lectura";
    case "rename_account":
      return "Propuesta: cambiar el nombre de una cuenta";
    case "sender_preference":
      return "Propuesta: guardar una preferencia de remitente";
    case "accept_suggestion":
      return "Propuesta: aceptar una sugerencia (la regla quedará inactiva)";
    case "discard_suggestion":
      return "Propuesta: no volver a proponer una sugerencia";
    case "create_rule":
      return "Propuesta: crear una regla (quedará inactiva)";
    case "edit_rule":
      return "Propuesta: cambiar las condiciones de una regla";
    case "import":
      return "Propuesta: importar documentos";
    default:
      return "Propuesta del asistente";
  }
}

/** Verbo exacto del botón principal; nunca un genérico. */
export function proposalVerb(kind: string, effect?: string): string {
  switch (kind) {
    case "approve":
      return effect === "register_invoice" ? "Registrar" : "Enviar";
    case "reject":
      return "Descartar tarea";
    case "retry":
      return "Reintentar";
    case "archive":
      return "Archivar";
    case "confirm_not_sent":
      return "Confirmar que no se envió";
    case "edit_draft":
      return "Guardar borrador";
    case "fetch_mail":
      return "Consultar correo";
    case "mail_limit":
      return "Cambiar límite";
    case "rename_account":
      return "Cambiar nombre";
    case "sender_preference":
      return "Guardar preferencia";
    case "accept_suggestion":
      return "Aceptar sugerencia";
    case "discard_suggestion":
      return "No volver a proponer";
    case "create_rule":
      return "Crear regla";
    case "edit_rule":
      return "Guardar cambios";
    case "import":
      return "Elegir archivos…";
    default:
      return "Confirmar";
  }
}

/** Estilo del botón principal: enviar y registrar son `primary`, descartar es `danger`, el resto es corriente. */
export function proposalTone(kind: string): "primary" | "danger" | "plain" {
  if (kind === "approve") return "primary";
  if (kind === "reject") return "danger";
  return "plain";
}

const numberText = (n: unknown): string => (typeof n === "number" && Number.isFinite(n) ? String(n) : "?");

/** Lo que ocurrirá al confirmar; nombra el efecto externo cuando lo hay. */
export function proposalEffectText(kind: string, effect?: string, summary: Record<string, any> = {}): string {
  switch (kind) {
    case "approve":
      return effect === "register_invoice" ? "Al confirmar, NestAdmin registrará esta factura." : "Al confirmar, NestAdmin enviará este correo.";
    case "reject":
      return "Al confirmar, la tarea se descartará y dejará de estar pendiente. No se envía ni se registra nada.";
    case "retry":
      return "Al confirmar, NestAdmin volverá a procesar esta tarea desde el principio.";
    case "archive":
      return "Al confirmar, NestAdmin archivará el correo sin responderlo y sin enviar nada.";
    case "confirm_not_sent":
      return "Al confirmar, NestAdmin dará por no enviado el intento anterior y se podrá preparar otra versión. Confírmelo solo si ha comprobado que el correo no salió.";
    case "edit_draft":
      return "Al confirmar, NestAdmin solo guardará el texto propuesto como borrador nuevo. No enviará nada: enviar es otro paso, con su propia revisión.";
    case "fetch_mail":
      return `Al confirmar, NestAdmin consultará ahora el correo de ${numberText(summary.accounts)} ${plural(Number(summary.accounts), "cuenta", "cuentas")} y traerá los mensajes nuevos. Límite de lectura actual: ${numberText(summary.limit)}.`;
    case "mail_limit":
      return `Al confirmar, el límite de lectura pasará de ${numberText(summary.current)} a ${numberText(summary.new)}.`;
    case "rename_account":
      return `Al confirmar, la cuenta pasará a llamarse «${String(summary.new_label ?? "")}»${summary.current_label ? ` en lugar de «${String(summary.current_label)}»` : ""}.`;
    case "sender_preference":
      return "Al confirmar, NestAdmin guardará esta preferencia para los correos nuevos de ese remitente. No cambia los que ya están en la lista.";
    case "accept_suggestion":
      return "Al confirmar, NestAdmin creará esta regla INACTIVA, con el origen de la sugerencia. No actuará hasta que usted la active en Reglas.";
    case "discard_suggestion":
      return "Al confirmar, NestAdmin no volverá a proponer esta sugerencia salvo que aparezca evidencia nueva. No se crea ninguna regla.";
    case "create_rule":
      return "Al confirmar, NestAdmin creará esta regla INACTIVA. No actuará hasta que usted la active en Reglas.";
    case "edit_rule":
      return summary.stays_active
        ? "Al confirmar, NestAdmin guardará las condiciones nuevas. La regla sigue activa porque el cambio la estrecha."
        : summary.was_active
          ? "Al confirmar, NestAdmin guardará las condiciones nuevas y la regla quedará INACTIVA: el cambio no la estrecha. La activa usted de nuevo en Reglas."
          : "Al confirmar, NestAdmin guardará las condiciones nuevas. La regla sigue inactiva: solo la activa usted en Reglas.";
    case "import":
      return "Al confirmar, se abrirá el selector de archivos del sistema para que usted elija qué importar. Si lo cierra sin elegir, no se hace nada.";
    default:
      return "Al confirmar, NestAdmin hará lo que indica esta tarjeta.";
  }
}

const quoted = (text: unknown): string => `«${String(text ?? "").trim()}»`;

/** Título de la tarea de la propuesta, si la hay. */
export function proposalTaskTitle(summary: Record<string, any>): string {
  const title = String(summary?.task?.title ?? "").trim();
  return title || "sin título";
}

/** Nombre accesible del botón principal: la acción y su destino. */
export function proposalButtonName(card: ProposalLike): string {
  const s = card.summary ?? {};
  switch (card.kind) {
    case "approve":
      if (card.effect === "register_invoice") {
        const what = [s.supplier, s.invoice_number]
          .map((x) => String(x ?? "").trim())
          .filter(Boolean)
          .join(" ");
        return what ? `Registrar la factura ${what}` : "Registrar la factura";
      }
      return Array.isArray(s.to) && s.to.length ? `Enviar la respuesta a ${s.to.join(", ")}` : "Enviar la respuesta";
    case "reject":
      return `Descartar tarea: ${quoted(proposalTaskTitle(s))}`;
    case "retry":
      return `Reintentar la tarea ${quoted(proposalTaskTitle(s))}`;
    case "archive":
      return `Archivar el correo ${quoted(proposalTaskTitle(s))}`;
    case "confirm_not_sent":
      return `Confirmar que no se envió el correo ${quoted(proposalTaskTitle(s))}`;
    case "edit_draft":
      return `Guardar borrador nuevo de ${quoted(proposalTaskTitle(s))}`;
    case "fetch_mail":
      return "Consultar correo ahora";
    case "mail_limit":
      return `Cambiar límite de lectura de ${numberText(s.current)} a ${numberText(s.new)}`;
    case "rename_account":
      return `Cambiar nombre de la cuenta ${quoted(s.current_label)} a ${quoted(s.new_label)}`;
    case "sender_preference":
      return `Guardar preferencia del remitente ${String(s.key ?? "")}`;
    case "accept_suggestion":
      return `Aceptar sugerencia ${quoted(s.name)} y crear la regla inactiva`;
    case "discard_suggestion":
      return `No volver a proponer la sugerencia ${quoted(s.name)}`;
    case "create_rule":
      return `Crear regla inactiva ${quoted(s.name)}`;
    case "edit_rule":
      return `Guardar cambios en la regla ${quoted(s.name)}`;
    case "import":
      return "Elegir archivos… para importar documentos";
    default:
      return proposalVerb(card.kind, card.effect);
  }
}

/** Nombre accesible de «No hacerlo»: dice qué propuesta se descarta. */
export function proposalDismissName(card: ProposalLike): string {
  return `${CARD_DISMISS}: ${proposalHeading(card.kind, card.effect).replace(/^Propuesta: /, "")}`;
}

/** La propuesta sigue esperando a la persona (o a que termine la ejecución). */
export function proposalIsOpen(status: string): boolean {
  return status === "pending" || status === "executing";
}

const RULE_KINDS = new Set(["accept_suggestion", "discard_suggestion", "create_rule", "edit_rule"]);

/** Texto de los estados finales que no son un recibo; `null` si no aplica. */
export function proposalFinalText(card: ProposalLike): string | null {
  switch (card.status) {
    case "rejected":
      return "No se hizo: usted la descartó.";
    case "expired":
      return "Caducó sin confirmarse. Pídala de nuevo si la necesita.";
    case "stale":
      return RULE_KINDS.has(card.kind)
        ? "La regla o la sugerencia cambió después de preparar esta propuesta. Pídala de nuevo para ver los datos actuales."
        : "La tarea cambió después de preparar esta propuesta. Pídala de nuevo para ver los datos actuales.";
    case "failed":
      return notDoneText(card) ?? "No se hizo nada: la acción ya no era posible con el estado actual de la tarea. Pídala de nuevo si la necesita.";
    default:
      return null;
  }
}

/** Lo que pasó cuando se pulsó el botón y la acción no llegó a hacerse (recibo `not_done`); `null` si no hay ese recibo. */
function notDoneText(card: ProposalLike): string | null {
  if (card.receipt?.outcome !== "not_done") return null;
  if (card.kind === "import") return "No se importó ningún archivo: revise los avisos en Tareas y vuelva a pedirlo si hace falta.";
  const what = card.effect === "register_invoice" ? "registrar la factura" : "enviar";
  switch (card.receipt.reason) {
    case "action_failed":
      return `No se pudo ${what}; la tarea quedó fallida y se puede reintentar.`;
    case "returned_to_review":
      return `No se pudo ${what}; la tarea sigue pendiente de revisión.`;
    default:
      return `No se pudo ${what}: la tarea no llegó a completarse.`;
  }
}

/** Aviso de que confirmar puede activar reglas que envían o registran sin revisión; `null` si no es el caso. */
export function proposalRulesText(kind: string, effect: string, summary: Record<string, any> = {}): string | null {
  if (effect === "import") {
    const n = Number(summary.active_register_rules) || 0;
    return n > 0 ? `Tiene ${n} ${plural(n, "regla activa que puede", "reglas activas que pueden")} registrar facturas sin revisión con lo que importe.` : null;
  }
  if (effect !== "may_trigger_rules") return null;
  const n = (Number(summary.active_send_rules) || 0) + (Number(summary.active_register_rules) || 0);
  const what = `${n} ${plural(n, "regla activa que puede", "reglas activas que pueden")} enviar respuestas o registrar facturas sin revisión`;
  return `Tiene ${what} con ${kind === "retry" ? "la tarea reintentada" : "el correo nuevo"}.`;
}

/** Etiqueta corta del estado, para el chip de la tarjeta. */
export function proposalStatusLabel(status: string): string {
  switch (status) {
    case "pending":
      return "Pendiente de su decisión";
    case "executing":
      return "Ejecutándose";
    case "confirmed":
      return "Hecho";
    case "rejected":
      return "Descartada";
    case "expired":
      return "Caducada";
    case "stale":
      return "Obsoleta";
    case "failed":
      return "No se pudo hacer";
    default:
      return status;
  }
}

function receiptHeadline(card: ProposalLike): string {
  const result = card.receipt?.result ?? {};
  switch (card.kind) {
    case "approve":
      return card.effect === "register_invoice" ? "Registrada" : "Enviado";
    case "edit_draft":
      return "Borrador guardado";
    case "reject":
      return "Tarea descartada";
    case "retry":
      return "Tarea puesta a reintentar";
    case "archive":
      return "Correo archivado";
    case "confirm_not_sent":
      return "Confirmado que el correo no se envió";
    case "fetch_mail":
      return typeof result.added === "number" ? `Correo consultado: ${result.added} ${plural(result.added, "mensaje nuevo", "mensajes nuevos")}` : "Correo consultado";
    case "mail_limit":
      return typeof result.limit === "number" ? `Límite de lectura cambiado a ${result.limit}` : "Límite de lectura cambiado";
    case "rename_account":
      return result.label ? `Cuenta renombrada a «${String(result.label)}»` : "Cuenta renombrada";
    case "sender_preference":
      return result.changed === false ? "La preferencia ya estaba así" : "Preferencia guardada";
    case "accept_suggestion":
    case "create_rule":
      return "Regla creada (inactiva)";
    case "discard_suggestion":
      return "Sugerencia descartada";
    case "edit_rule":
      return result.deactivated_by_edit ? "Regla editada: ha quedado inactiva" : "Regla editada";
    case "import":
      return importCountsText(result);
    default:
      return "Hecho";
  }
}

/** «N importados, M con error», a partir del recibo de una importación. */
export function importCountsText(result: Record<string, any> = {}): string {
  const imported = Number(result.imported) || 0;
  const errors = Number(result.errors) || 0;
  return `${imported} ${plural(imported, "importado", "importados")}, ${errors} con error`;
}

/** Qué se hizo y cuándo, tomado del recibo; vacío si la propuesta no está confirmada. */
export function proposalReceiptLines(card: ProposalLike): string[] {
  if (card.status !== "confirmed" || !card.receipt || card.receipt.outcome === "not_done") return [];
  const when = formatStamp(card.receipt.confirmed_at);
  const lines = [`${receiptHeadline(card)}${when ? ` el ${when}` : ""}.`];
  if (card.kind === "approve" && card.effect === "send_mail") lines.push("La entrega al destinatario no se comprueba.");
  if (card.kind === "edit_draft") lines.push("No se ha enviado nada: enviar es otro paso.");
  if (card.kind === "accept_suggestion" || card.kind === "create_rule") lines.push("Está inactiva: actívela en Reglas cuando quiera que actúe.");
  if (card.kind === "edit_rule" && !card.receipt.result?.active) lines.push("La regla no está activa: solo se activa en Reglas.");
  if (card.kind === "import" && Number(card.receipt.result?.imported) > 0) lines.push("Lo importado se procesa como cualquier documento y pasa por sus reglas.");
  if (card.receipt.reconciled) lines.push("La acción ya constaba como hecha y no se repitió.");
  return lines;
}

// ---- Reglas y sugerencias (E4, D191) ----

/** ¿Es el error de la ventana por cerrar el diálogo de importar sin elegir nada? Entonces la tarjeta sigue pendiente. */
export function isImportCancelled(error: unknown): boolean {
  return String(error ?? "").trim() === IMPORT_CANCELLED_CODE;
}

const RULE_ACTION_TEXT: Record<string, string> = {
  register_invoice: "Registrar facturas sin revisión",
  send_draft: "Enviar la respuesta redactada por la IA sin revisión",
};

/** Tipo de acción de una regla, dicho con claridad. */
export function ruleActionText(action: unknown): string {
  return RULE_ACTION_TEXT[String(action ?? "")] ?? "Acción desconocida";
}

const EXTRA_CONDITION_LABEL: Record<string, string> = { currency: "Moneda" };

const CONDITION_LABEL: Record<string, string> = {
  supplier: "Proveedor",
  max_total: "Importe máximo (EUR)",
  category: "Categoría",
  sender: "Remitente",
  sender_domain: "Dominio del remitente",
};

/** Texto de un valor de condición cualquiera: nunca se omite uno que la regla lleve. */
function conditionValueText(value: unknown): string {
  if (typeof value === "string") return value;
  if (typeof value === "number" || typeof value === "boolean") return String(value);
  return JSON.stringify(value) ?? "";
}

/**
 * Las condiciones exactas de una regla como pares «rótulo, valor»; la categoría se traduce con `categoryLabel`.
 * Primero las cinco conocidas, en su orden, y después, de forma genérica y por orden alfabético, cualquier otra
 * clave que la regla lleve (p. ej. `currency`): la tarjeta no esconde ninguna condición.
 */
export function ruleConditionLines(conditions: Record<string, any> | null | undefined, categoryLabel: (code: string) => string = (c) => c): Array<[string, string]> {
  const out: Array<[string, string]> = [];
  const known = ["supplier", "max_total", "category", "sender", "sender_domain"];
  for (const key of known) {
    const value = conditions?.[key];
    if (typeof value === "string" && value.trim()) out.push([CONDITION_LABEL[key]!, key === "category" ? categoryLabel(value) : value]);
    else if (value !== undefined && value !== null && typeof value !== "string") out.push([CONDITION_LABEL[key]!, conditionValueText(value)]);
  }
  for (const key of Object.keys(conditions ?? {}).filter((k) => !known.includes(k)).sort()) {
    const value = conditions![key];
    if (value === undefined || value === null) continue;
    out.push([EXTRA_CONDITION_LABEL[key] ?? key, conditionValueText(value)]);
  }
  return out;
}

/** Cuántos casos anteriores habría cubierto la regla, de la simulación; `null` si la tarjeta no lo trae. */
export function proposalCoverageText(coverage: Record<string, any> | null | undefined): string | null {
  if (!coverage || typeof coverage.covered !== "number") return null;
  const n = coverage.covered;
  const base = n === 0 ? "No habría cubierto ningún caso anterior" : `Habría cubierto ${n} ${plural(n, "caso anterior", "casos anteriores")}`;
  const auto = typeof coverage.would_automate === "number" && n > 0 ? ` (${coverage.would_automate} ${plural(coverage.would_automate, "se habría automatizado", "se habrían automatizado")})` : "";
  return `${base}${auto}.`;
}

/** De cuántas observaciones sale una sugerencia; `null` si la tarjeta no lo trae (reglas creadas a petición). */
export function suggestionEvidenceText(evidence: Record<string, any> | null | undefined): string | null {
  if (!evidence || typeof evidence.count !== "number") return null;
  return `Sale de ${evidence.count} ${plural(evidence.count, "observación", "observaciones")} (umbral ${numberText(evidence.threshold)}).`;
}

/** Una frase por solapamiento de la regla con otras; vacío si no hay. */
export function proposalOverlapLines(overlaps: unknown): string[] {
  if (!Array.isArray(overlaps)) return [];
  return overlaps.flatMap((o: any) => {
    const names = Array.isArray(o?.rules) ? o.rules.map((r: any) => `«${String(r?.name ?? r?.id ?? "")}»`).filter((n: string) => n !== "«»") : [];
    if (!names.length) return [];
    return [`Se solapa con ${names.join(", ")}: al activarla, los casos que cubran las dos irían a revisión.`];
  });
}

/** Qué pasa con la regla editada: antes y después de su estado de activación. */
export function editedRuleStateText(summary: Record<string, any> = {}): string {
  if (!summary.was_active) return "La regla está inactiva y seguirá así: solo la activa usted en Reglas.";
  return summary.stays_active ? "La regla está activa y seguirá activa: el cambio la estrecha." : "La regla está activa y quedará inactiva: el cambio no la estrecha.";
}

/** Qué clase de dato de otro documento es, para la lista del aviso de un borrador (D266). */
export function otherDocumentKindText(kind: unknown): string {
  if (kind === "invoice_number") return "Número de factura";
  if (kind === "file_name") return "Nombre de archivo";
  if (kind === "party") return "Proveedor o remitente";
  return "Dato";
}

/** Los datos de otros documentos que la aplicación halló en el borrador propuesto: hasta cinco y el total. */
export function otherDocumentsOf(summary: Record<string, any> | null | undefined): { items: Array<{ kind: string; value: string }>; total: number } {
  const raw = Array.isArray(summary?.other_documents) ? summary!.other_documents : [];
  const items = raw
    .filter((i: any) => i && typeof i.value === "string" && i.value.trim() !== "")
    .map((i: any) => ({ kind: String(i.kind ?? ""), value: String(i.value) }));
  const total = typeof summary?.other_documents_total === "number" ? Math.max(summary.other_documents_total, items.length) : items.length;
  return { items, total };
}

/** Frase del aviso: cuántos datos hay y, si no caben todos, cuántos se enseñan. */
export function otherDocumentsIntro(total: number, shown: number): string {
  const base = total === 1 ? "La aplicación halló 1 dato de otro documento" : `La aplicación halló ${total} datos de otros documentos`;
  const rest = total > shown ? ` (se enseñan ${shown})` : "";
  const where = total === 1 ? "que no está en este correo ni en su hilo" : "que no están en este correo ni en su hilo";
  return `${base}${rest} ${where}. Compruebe que deben ir en la respuesta.`;
}

const trimmedReason = (error: unknown): string => String(error ?? "").trim().replace(/[.\s]+$/, "");

/** Mensaje de un fallo al confirmar: el motivo del fondo y qué pasó con la propuesta. */
export function proposalErrorText(error: unknown, status: string | null): string {
  const raw = trimmedReason(error);
  const reason = raw ? `${raw.charAt(0).toUpperCase()}${raw.slice(1)}.` : "No se pudo confirmar.";
  if (status === "executing") return `${reason} No consta si llegó a hacerse: al volver a pulsar el botón, NestAdmin lo comprobará antes de repetirlo.`;
  if (status === "pending") return `${reason} La propuesta sigue pendiente: no se ha hecho nada.`;
  return reason;
}

export function proposalDismissErrorText(error: unknown): string {
  const raw = trimmedReason(error);
  return raw ? `No se pudo descartar: ${raw}.` : "No se pudo descartar la propuesta.";
}

/** Aviso sobre los destinatarios de un borrador (señales de dirección automática); `null` si no hay. */
export function recipientSignalsText(signals: unknown): string | null {
  const codes = Array.isArray(signals) ? signals.filter((c): c is string => typeof c === "string") : [];
  if (!codes.length) return null;
  const reasons: Record<string, string> = {
    noreply_address: "una dirección que no admite respuestas",
    bounce_address: "una dirección de rebotes",
    null_return_path: "una dirección de rebotes",
    auto_submitted: "un remitente automático",
    precedence_junk: "un remitente de correo no solicitado",
    list_id: "una lista de distribución",
    list_unsubscribe: "una lista de distribución",
    precedence_bulk: "un envío masivo",
    precedence_list: "una lista de distribución",
  };
  const known = [...new Set(codes.map((c) => reasons[c]).filter((r): r is string => Boolean(r)))];
  const what = known.length ? known.join(", ") : "una dirección automática";
  return `Algún destinatario parece ${what}: es probable que nadie lea la respuesta. Compruébelo antes de enviar.`;
}

/** Texto de las incidencias propias de una propuesta de correo, por su código. */
export function emailIssueText(code: string, version?: unknown): string | null {
  if (code === "unconfirmed_attempt") return `Ya se intentó la versión ${numberText(version)} y no consta confirmado: confirmar reintenta sin repetirlo.`;
  if (code === "no_recipient") return "El borrador no tiene destinatario y no se puede enviar.";
  return null;
}

const AUDIT_REASONS: Record<string, string> = {
  action_failed: "la tarea quedó fallida",
  returned_to_review: "la tarea volvió a revisión",
  nothing_imported: "no se importó ningún archivo",
  import_interrupted: "la importación quedó a medias y no se repite sola",
  not_completed: "la tarea no se completó",
};

/** Detalle del Historial de un evento `assistant_proposal`, en palabras; sin texto de correos ni de documentos. */
export function proposalAuditText(d: Record<string, unknown>): string {
  const what = typeof d.kind === "string" ? proposalHeading(d.kind, typeof d.effect === "string" ? d.effect : undefined).replace(/^Propuesta: /, "") : "propuesta";
  const effect = d.effect === "send_mail" ? "envía un correo" : d.effect === "register_invoice" ? "registra una factura" : d.effect === "may_trigger_rules" ? "puede activar reglas" : d.effect === "rule_inactive" ? "crea una regla inactiva" : d.effect === "rule_edited" ? "cambia las condiciones de una regla" : d.effect === "import" ? "importa documentos" : "sin envío ni registro";
  const code = typeof d.code === "string" && d.code ? (d.code === "reconciled" ? " · ya constaba hecha y no se repitió" : d.code in AUDIT_REASONS ? ` · no se hizo: ${AUDIT_REASONS[String(d.code)]}` : ` · motivo: ${d.code}`) : "";
  return `${what.charAt(0).toUpperCase()}${what.slice(1)} · ${effect}${code}`;
}

// ---- Datos protegidos en las respuestas (D183) ----

/** Marcas con las que el enmascarado local sustituye los datos protegidos antes de enviarlos a la IA. */
export const MASK_MARKS = ["[IBAN]", "[TELÉFONO]", "[CORREO]", "[ID]", "[CLAVE]", "[TARJETA]", "[URL]"] as const;

/** Las marcas de enmascarado que contiene el texto, sin repetir y en el orden en que aparecen. */
export function maskedMarksIn(text: string): string[] {
  const body = String(text ?? "");
  const found: Array<{ mark: string; at: number }> = [];
  for (const mark of MASK_MARKS) {
    const at = body.indexOf(mark);
    if (at >= 0) found.push({ mark, at });
  }
  return found.sort((a, b) => a.at - b.at).map((f) => f.mark);
}

/** Nota bajo una respuesta que contiene marcas: el dato no se envió a la IA y se ve en el documento. */
export function maskedNote(marks: readonly string[]): string | null {
  if (!marks.length) return null;
  const list = marks.length > 1 ? `${marks.slice(0, -1).join(", ")} y ${marks[marks.length - 1]}` : marks[0];
  return `Los datos que aparecen como ${list} no se enviaron a la IA, para protegerlos. Puede verlos abriendo el documento en su tarjeta.`;
}
