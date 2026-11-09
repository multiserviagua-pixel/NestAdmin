// Textos de la primera lectura (E5, D133). Todo es puro: recibe los datos del backend y devuelve
// texto. Un texto solo afirma lo que esos datos sostienen: «no se ha enviado ningún correo ni se ha
// registrado nada» solo se dice si el backend cuenta cero acciones, y siempre aclara que los
// documentos que parecían una factura sí se enviaron completos al proveedor.

export type ReadModeLike = "on_demand" | "automatic";

export interface DestinationLike {
  provider: string;
  model: string;
  host: string | null;
}

export interface ReadStatusLike {
  source?: string;
  kind?: string;
  state: string;
  volume: number | null;
  provider?: string | null;
  model?: string | null;
  host?: string | null;
  listed: number;
  already_known: number;
  recent_left_out: number;
  emails_unclassified: number;
  documents_sent: number;
  documents_skipped: number;
  over_cap: number;
  tasks: { total: number; queued: number; running: number; completed: number; failed: number; cancelled: number };
  by_provider: Array<{ provider: string; model: string; host: string | null; documents: number }>;
  failures: Array<{ task_id: number; kind: string; name: string | null; code: string; message: string; recoverable: boolean }>;
  attachment_failures: Array<{ name: string; code: string; message: string }>;
  skipped_attachments: number;
  listing_error: { code: string; message: string } | null;
  scan_capped: boolean;
  remaining: number;
  fallback_used: number;
  actions: number;
  waiting_message: string | null;
  held: { reason: string; authorized: DestinationLike | null; now: DestinationLike } | null;
  removed: boolean;
}

export interface ReadOverviewLike {
  mode: ReadModeLike;
  limits: { min: number; max: number; fixed: number | null; default: number };
  recent_days: number;
  documents_cap: number;
  provider: { provider: string; model: string };
  host: string | null;
  external: boolean;
  local_fallback: boolean;
  blocked: string | null;
  blocked_message: string | null;
  sends: string[];
  never_sent: string[];
}

export interface ReadSuggestionsLike {
  seen_documents: number;
  ready: number;
  closest: number;
  needed: number;
  error: string | null;
}

const PROVIDER_NAME: Record<string, string> = {
  ollama_cloud: "Ollama cloud",
  openai_compatible: "un servicio compatible con OpenAI",
  anthropic: "Claude (Anthropic)",
  ollama: "la IA local (Ollama)",
};

export function readProviderName(code: string): string {
  return PROVIDER_NAME[code] ?? code;
}

function plural(n: number, one: string, many: string): string {
  return n === 1 ? one : many;
}

/** Nombre, modelo y servidor de un destino, como `Provider::destination()`. */
export function destinationLabel(d: DestinationLike): string {
  const where = d.host ? `, servidor ${d.host}` : ", en este equipo";
  return `${readProviderName(d.provider)}, modelo ${d.model}${where}`;
}

/** A quién se enviaría el texto y con qué modelo y servidor. */
export function readDestination(o: Pick<ReadOverviewLike, "provider" | "host" | "external" | "local_fallback">): string {
  const name = readProviderName(o.provider.provider);
  if (!o.external) return `Se hará con ${name}, modelo ${o.provider.model}. El texto no sale de este equipo.`;
  const where = o.host ? `, servidor ${o.host}` : "";
  const fallback = o.local_fallback
    ? " Si ese proveedor falla, solo se repetirá con la IA local de este equipo si está preparada; nunca con otro proveedor externo."
    : " El respaldo con la IA local está desactivado: si el proveedor falla, esa lectura quedará con error y podrá reintentarla.";
  return `Se enviará a ${name}, modelo ${o.provider.model}${where}.${fallback}`;
}

/** Cuánto se lee, según el modo: mensajes anteriores a los últimos días, y el tope de documentos. */
export function readVolumeText(o: Pick<ReadOverviewLike, "limits" | "recent_days" | "documents_cap">, kind: "account" | "folder" = "account"): string {
  if (kind === "folder") {
    const n = o.limits.fixed !== null ? `hasta ${o.limits.fixed}` : `entre ${o.limits.min} y ${o.limits.max}, el volumen que usted elija,`;
    return `${n} archivos admitidos de la carpeta (PDF, imágenes y texto; sin subcarpetas); el mismo número es el tope de documentos enviados`;
  }
  const n = o.limits.fixed !== null ? `hasta ${o.limits.fixed} mensajes` : `entre ${o.limits.min} y ${o.limits.max} mensajes, el volumen que usted elija,`;
  const cap = o.limits.fixed !== null ? o.documents_cap : null;
  return `${n} anteriores a los últimos ${o.recent_days} días de la cuenta, con un tope total de ${cap ?? "tantos documentos como mensajes"} documentos enviados (PDF, imágenes y texto)`;
}

/** Lo que pasa con los correos de los últimos días: el flujo normal, con IA y reglas. Visible siempre, nunca solo en una ayuda. */
export function recentMailWarning(o: Pick<ReadOverviewLike, "recent_days">): string {
  return `Los correos de los últimos ${o.recent_days} días no se leen: siguen el flujo normal y, al consultar el correo, salen completos al proveedor para clasificarlos y redactar un borrador; una regla activa puede enviar la respuesta sin revisión.`;
}

/** Qué hace la primera lectura y qué no. Lo garantiza el código y lo comprueban sus pruebas. */
export function readPromise(o: Pick<ReadOverviewLike, "recent_days">): string {
  return `En esta lectura no se envía ningún correo ni se registra nada, no se crean pendientes y una regla activa no actúa sobre lo leído. Los correos leídos no salen a ningún proveedor (se cuentan sin clasificar): salen solo los documentos adjuntos o de la carpeta que parecen una factura según una comprobación local, y salen completos. ${recentMailWarning(o)} Se hace una sola vez por buzón y lo leído no vuelve a procesarse.`;
}

/** Lo que aporta a las sugerencias: repetición observada, nunca una aprobación. */
export const READ_EVIDENCE_NOTE =
  "Lo leído solo puede sugerir reglas de facturas (nunca de respuestas por correo) y cuenta como «vista», no como aprobada: se exige más repetición que con sus aprobaciones y el límite lo escribe usted, porque no sale de lo visto.";

export const READ_HELP = {
  volume:
    "El volumen es el número de mensajes anteriores a los últimos días (o de archivos, en una carpeta) que se recorren, y también el tope de documentos enviados al proveedor. Cada documento es una llamada al proveedor de IA: un volumen mayor cuesta más y tarda más.",
  sends:
    "El OCR y la extracción de texto de los documentos se hacen siempre en este equipo; al proveedor solo sale el texto ya extraído, nunca el archivo original, y completo. Si el texto menciona direcciones o nombres, salen con él.",
  apart:
    "Las tareas de la primera lectura no aparecen en Pendientes ni en Tareas: se ven aquí, con su recuento y su estado. En el Historial queda una sola entrada por lectura, al terminar o detenerse.",
  retry: "Reintentar vuelve a poner en cola las tareas que fallaron por un error recuperable y vuelve a pedir los adjuntos que no se pudieron descargar. No duplica nada.",
  stop: "Detener la lectura impide que salga nada más hacia el proveedor. Puede continuarla después, viendo de nuevo a quién se envía.",
} as const;

/**
 * Aviso de la pantalla de conexión, ANTES de conectar una cuenta: lo visible es corto (cuánto, a
 * quién, qué sale y qué pasa con los correos de los últimos días); el detalle va aparte
 * (`connectDetails`). Si el resumen no se pudo calcular (`error`), lo dice y la lectura no
 * arranca sola: queda pendiente con un botón.
 */
const READ_NO_PENDING = "Los correos leídos no crean pendientes ni se responden, y no vuelven a procesarse.";

export function connectNotice(o: ReadOverviewLike | null, error: string | null = null): string[] {
  if (!o) {
    return [
      `No se ha podido comprobar qué se leería ni a quién se enviaría${error ? ` (${error})` : ""}.`,
      `Puede conectar la cuenta, pero la primera lectura no arrancará sola: quedará pendiente, con un botón para empezarla cuando vea lo que se envía. ${READ_NO_PENDING}`,
      "Los correos recientes siguen el flujo normal: al consultar el correo salen completos al proveedor y una regla activa puede actuar sobre ellos.",
    ];
  }
  if (o.mode === "on_demand") {
    return ["En esta versión de prueba no se lee al conectar: usted la pide más abajo, eligiendo el volumen y viendo antes a qué proveedor se envía.", `${READ_NO_PENDING} ${recentMailWarning(o)}`];
  }
  const lines = [`Al conectar una cuenta se leerá una sola vez: ${readVolumeText(o)}. ${READ_NO_PENDING}`];
  if (o.blocked) {
    lines.push(
      `Ahora mismo no hay proveedor de IA confirmado (${o.blocked_message ?? "falta configurar la IA"}): no se enviará nada, la lectura quedará en espera y, cuando confirme un proveedor, la configuración le dirá a quién se enviará y empezará sola.`,
    );
  } else {
    lines.push(`${readDestination(o).split(". ")[0]!.replace(/\.$/, "")}: solo los documentos que parecen una factura, completos. En esta lectura no sale ningún correo.`);
  }
  lines.push(recentMailWarning(o));
  if (!o.blocked) lines.push("El proveedor ya está confirmado: la lectura empezará al terminar de conectar, si lo que ve aquí sigue siendo cierto.");
  return lines;
}

/** El detalle del aviso (plegado): qué sale, qué no, respaldo y cómo se usa lo leído. */
export function connectDetails(o: ReadOverviewLike): string[] {
  return [readPromise(o), `Qué se envía: ${o.sends.join("; ")}.`, `No se envía nunca: ${o.never_sent.join("; ")}.`, o.external ? readDestination(o) : "", READ_EVIDENCE_NOTE].filter(Boolean);
}

/** Lo que debe leer quien confirma un proveedor mientras hay lecturas en espera (asistente y configuración). */
/** Sin el resumen (falló) tampoco se sabe si hay lecturas en espera: se dice. */
export const WAITING_UNKNOWN =
  "No se ha podido comprobar si hay lecturas de correo en espera. Si las hay, al confirmar el proveedor empezarán solas y enviarán al proveedor documentos que parecen una factura, completos; abra Cuentas de correo para verlo.";

export function waitingReadsNotice(o: ReadOverviewLike, waiting: number): string | null {
  if (waiting <= 0 || o.mode !== "automatic") return null;
  return `Hay ${waiting} ${plural(waiting, "lectura en espera", "lecturas en espera")}: al confirmar el proveedor, empezarán solas. Leerán ${readVolumeText(o)} y enviarán ${o.sends.join("; ")}. No se envía nunca: ${o.never_sent.join("; ")}. ${recentMailWarning(o)}`;
}

/** Lo que se muestra justo antes del botón de leer. */
export function confirmReadLines(o: ReadOverviewLike, volume: number, what: string): { heading: string; lines: string[]; sends: string[]; never: string } {
  return {
    heading: `Va a leer ${what}: ${volume} ${plural(volume, "elemento", "elementos")} como mucho, y no más de ${volume} documentos enviados`,
    lines: [readDestination(o), readPromise(o)],
    sends: o.sends,
    never: `No se envía nunca: ${o.never_sent.join("; ")}.`,
  };
}

export function volumeProblem(raw: string, limits: { min: number; max: number }): string | null {
  const text = raw.trim();
  if (!/^\d+$/.test(text)) return `Escriba un número entero entre ${limits.min} y ${limits.max}.`;
  const n = Number(text);
  if (n < limits.min || n > limits.max) return `El volumen debe estar entre ${limits.min} y ${limits.max}.`;
  return null;
}

const STATE_TEXT: Record<string, string> = {
  not_started: "Aún no se ha leído",
  waiting: "En espera de un proveedor de IA confirmado",
  pending: "Pendiente: falta que la empiece usted",
  interrupted: "Interrumpida: se puede reanudar sin duplicar nada",
  listing_failed: "No se pudo listar: se puede reanudar",
  stopped: "Detenida",
  held: "Retenida: cambió el destino de los datos",
  reading: "Leyendo",
  done_with_errors: "Terminada, con errores",
  done: "Terminada",
};

export function readStateLabel(state: string): string {
  return STATE_TEXT[state] ?? state;
}

/** Si la lectura se puede (re)lanzar con el botón de empezar. */
export function canStartRead(status: ReadStatusLike): boolean {
  return ["not_started", "pending", "interrupted", "listing_failed"].includes(status.state);
}

/** Se puede detener mientras algo sale o puede salir. */
export function canStopRead(status: ReadStatusLike): boolean {
  return ["reading", "waiting", "held", "interrupted"].includes(status.state) && !status.removed;
}

/** Texto del botón según el estado: reanudar solo si ya había empezado. */
export function readButtonLabel(status: ReadStatusLike): string {
  if (status.state === "pending") return "Empezar la lectura";
  return status.state === "interrupted" || status.state === "listing_failed" ? "Reanudar la lectura" : "Leer ahora";
}

/** Por qué lo pendiente no sale y qué hace falta, mostrando el destino nuevo si cambió. */
export function heldText(status: ReadStatusLike): string | null {
  const h = status.held;
  if (!h) return null;
  if (h.reason === "stopped") return "Detenida: no sale nada más hacia el proveedor. Puede continuarla viendo antes a quién se enviaría ahora.";
  if (h.reason === "removed") return "La cuenta ya no está conectada: lo pendiente se canceló y no sale nada más. El resultado sigue aquí.";
  const before = h.authorized ? destinationLabel(h.authorized) : "el que vio al empezar";
  return `Retenida: ${before} era el destino que vio al empezar, y ahora el destino es ${destinationLabel(h.now)}. Lo que queda en cola no sale hasta que confirme continuar con el destino nuevo.`;
}

export function continueButtonLabel(status: ReadStatusLike): string {
  const h = status.held;
  if (!h || h.reason === "stopped") return "Continuar la lectura";
  return `Continuar con ${destinationLabel(h.now)}`;
}

/** Recuento por proveedor, para el final de la lectura. */
export function byProviderText(status: ReadStatusLike): string | null {
  if (!status.by_provider.length) return null;
  const parts = status.by_provider.map((p) => `${p.documents} a ${destinationLabel({ provider: p.provider, model: p.model, host: p.host })}`);
  return `Documentos enviados: ${parts.join("; ")}.`;
}

/** Lo que sugiere lo leído, solo con las cifras del backend. Son globales: de todas las lecturas y decisiones. */
export function suggestionsLine(s: ReadSuggestionsLike): string {
  if (s.error) return `no se han podido calcular (${s.error})`;
  if (s.seen_documents === 0) return "ninguna: no se ha visto ninguna factura legible";
  const parts: string[] = [];
  if (s.ready) parts.push(`${s.ready} ${plural(s.ready, "lista para revisar", "listas para revisar")} en Mis reglas, pestaña Sugeridas`);
  if (s.closest) parts.push(`${s.closest} ${plural(s.closest, "proveedor aún no llega", "proveedores aún no llegan")} al mínimo de ${s.needed} facturas vistas`);
  if (!parts.length) return `ninguna todavía (${s.seen_documents} ${plural(s.seen_documents, "documento visto", "documentos vistos")} sin repetirse lo bastante ni cumplir las condiciones)`;
  return parts.join("; ");
}

/** Nota que acompaña al resumen: las sugerencias no son de esta fuente sola. */
export const SUGGESTIONS_GLOBAL_NOTE = "Las sugerencias cuentan todas las lecturas y sus decisiones, no solo esta fuente.";

/** «Leídos N documentos; … sugerencias: …». Mientras queda trabajo, dice cuánto. */
export function readSummary(status: ReadStatusLike, suggestions: ReadSuggestionsLike): string {
  const t = status.tasks;
  if (t.total === 0) {
    if (status.state === "waiting") return "Todavía no se ha leído nada: está en espera.";
    if (status.emails_unclassified === 0 && status.documents_sent === 0) return "Todavía no se ha leído nada.";
  }
  const sent = `Enviados ${status.documents_sent} ${plural(status.documents_sent, "documento", "documentos")}`;
  const skipped = `${status.documents_skipped} ${plural(status.documents_skipped, "omitido", "omitidos")} por no parecer ${plural(status.documents_skipped, "una factura", "facturas")}`;
  const emails = `${status.emails_unclassified} ${plural(status.emails_unclassified, "correo", "correos")} sin clasificar (no se ${plural(status.emails_unclassified, "envía", "envían")})`;
  const over = status.over_cap ? `; ${status.over_cap} sin leer por el tope` : "";
  return `${sent}; ${skipped}; ${emails}${over}; sugerencias: ${suggestionsLine(suggestions)}.`;
}

/** Solo si el backend cuenta cero acciones. Aclara siempre que los documentos sí se enviaron. */
export function readEffectsLine(status: ReadStatusLike): string | null {
  if (status.tasks.total === 0 && status.emails_unclassified === 0) return null;
  if (status.actions !== 0)
    return `Atención: constan ${status.actions} ${plural(status.actions, "acción", "acciones")} de envío o registro en esta lectura. Esto no debería pasar; avise al soporte.`;
  const docs =
    status.documents_sent > 0
      ? ` Al proveedor sí se enviaron ${status.documents_sent} ${plural(status.documents_sent, "documento completo", "documentos completos")}, con todo lo que contienen.`
      : " Al proveedor no se ha enviado ningún documento.";
  return `No se ha enviado ningún correo ni se ha registrado nada.${docs}`;
}

/** Lo que falta: en cola, en curso y con error. Vacío si no falta nada. */
export function readRemaining(status: ReadStatusLike): string | null {
  const t = status.tasks;
  const parts: string[] = [];
  if (t.queued) parts.push(`${t.queued} en cola`);
  if (t.running) parts.push(`${t.running} en curso`);
  if (t.failed) parts.push(`${t.failed} con error`);
  return parts.length ? `Falta: ${parts.join(", ")}.` : null;
}

const ERROR_WORDS: Record<string, string> = {
  unavailable: "el servicio no respondió (red o servidor)",
  timeout: "el servicio tardó demasiado",
  rate_limited: "el proveedor limita las peticiones",
  unauthorized: "las credenciales no son válidas o caducaron",
  invalid_response: "la respuesta del modelo no era válida",
  ocr_unavailable: "el OCR local no está disponible",
  pdf_unavailable: "el lector de PDF local no está disponible",
  document_unreadable: "el documento no se pudo leer",
  invalid_input: "el archivo no es válido",
  not_found: "ya no existe",
  internal: "falló algo interno",
};

export function readErrorWords(code: string): string {
  return ERROR_WORDS[code] ?? `error «${code}»`;
}

export function readFailureLine(f: ReadStatusLike["failures"][number]): string {
  const what = f.name ?? (f.kind === "email" ? "un correo" : "un documento");
  return `${what}: ${readErrorWords(f.code)}${f.recoverable ? "; se puede reintentar" : "; no se puede reintentar"}.`;
}

/** Lo que no se pudo listar o descargar, explícito. En una carpeta no se habla de «adjuntos». */
export function readProblems(status: ReadStatusLike): string[] {
  const out: string[] = [];
  if (status.listing_error) out.push(`No se pudo listar: ${readErrorWords(status.listing_error.code)}. Puede reanudar la lectura.`);
  const word = status.kind === "folder" ? "Archivo" : "Adjunto";
  for (const a of status.attachment_failures) out.push(`${word} «${a.name}»: ${readErrorWords(a.code)}.`);
  return out;
}

/** Con qué proveedor se empezó, tal como quedó anotado. */
export function readStartedWith(status: ReadStatusLike): string | null {
  if (!status.provider) return null;
  const host = status.host ? `, servidor ${status.host}` : "";
  return `Se empezó con ${readProviderName(status.provider)}, modelo ${status.model ?? "?"}${host}.`;
}

/** Líneas de contexto del recuento. */
export function readNotes(status: ReadStatusLike): string[] {
  const out: string[] = [];
  if (status.recent_left_out)
    out.push(`${status.recent_left_out} ${plural(status.recent_left_out, "correo reciente o sin fecha legible sigue", "correos recientes o sin fecha legible siguen")} el flujo normal: no se leyeron.`);
  if (status.already_known) out.push(`${status.already_known} ${plural(status.already_known, "elemento ya tenía", "elementos ya tenían")} tarea o se había importado antes: no se ha tocado.`);
  if (status.skipped_attachments) out.push(`${status.skipped_attachments} ${plural(status.skipped_attachments, "adjunto", "adjuntos")} de un tipo que NestAdmin no procesa (por ejemplo, .docx o .zip).`);
  if (status.over_cap) out.push(`${status.over_cap} ${plural(status.over_cap, "documento no se envió", "documentos no se enviaron")} por el tope.`);
  if (status.scan_capped) out.push("Se recorrió el máximo de mensajes sin reunir el volumen pedido.");
  if (status.fallback_used) out.push(`${status.fallback_used} ${plural(status.fallback_used, "tarea la atendió", "tareas las atendió")} la IA local de respaldo porque el proveedor no respondió.`);
  return out;
}

/** Nombre corto de una carpeta para mostrarla. */
export function folderLabel(path: string): string {
  const parts = path.split(/[\\/]/).filter(Boolean);
  return parts.length ? parts[parts.length - 1]! : path;
}

/** Tono del distintivo de estado: verde solo si terminó sin ningún fallo. */
export function readStateTone(state: string): "ok" | "neutral" | "info" | "warn" {
  if (state === "done") return "ok";
  if (state === "not_started") return "neutral";
  if (state === "reading") return "info";
  return "warn";
}
