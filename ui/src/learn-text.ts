// Textos de «Aprender a responder» (E7, D167-D175). Todo es puro: recibe los datos del backend y
// devuelve texto. Un texto solo afirma lo que esos datos sostienen. La ausencia de datos es un
// resultado normal y se dice con recuentos, no como un error. Los ejemplos de las pruebas son inventados.

// Los nombres de proveedor y de error repiten los de `read-text.ts`: un módulo de textos no importa a otro
// para que las pruebas de Node carguen cada uno por separado.

const PROVIDER_NAME: Record<string, string> = {
  ollama_cloud: "Ollama cloud",
  openai_compatible: "un servicio compatible con OpenAI",
  anthropic: "Claude (Anthropic)",
  ollama: "la IA local (Ollama)",
};

export function readProviderName(code: string): string {
  return PROVIDER_NAME[code] ?? code;
}

const ERROR_WORDS: Record<string, string> = {
  unavailable: "el servicio no respondió (red o servidor)",
  timeout: "el servicio tardó demasiado",
  rate_limited: "el proveedor limita las peticiones",
  unauthorized: "las credenciales no son válidas o caducaron",
  invalid_response: "la respuesta del modelo no era válida",
  not_found: "ya no existe",
  internal: "falló algo interno",
};

export function readErrorWords(code: string): string {
  return ERROR_WORDS[code] ?? `error «${code}»`;
}

/** Nombre, modelo y servidor de un destino. */
export function destinationLabel(d: { provider: string; model: string; host: string | null }): string {
  const where = d.host ? `, servidor ${d.host}` : ", en este equipo";
  return `${readProviderName(d.provider)}, modelo ${d.model}${where}`;
}

export interface CountsLike {
  scanned: number;
  automatic: number;
  no_own_reply: number;
  no_sent_folder: number;
  quote_dropped?: number;
  fetch_errors?: number;
  over_limit?: number;
  pairs: number;
}

export interface LearnStatusLike {
  state: string;
  interrupted: boolean;
  read_done: boolean;
  counts: CountsLike | null;
  pairs: number;
  destination: { provider: string; model: string; host: string | null } | null;
  held: { reason: string; authorized: { provider: string; model: string; host: string | null } | null; now: { provider: string; model: string; host: string | null } } | null;
  ai: "external" | "local" | null;
  fallback: string | null;
  failure: string | null;
  error: { code: string; message: string } | null;
}

export type LearnPhase =
  | "waiting_read"
  | "starting"
  | "collecting"
  | "awaiting_consent"
  | "running"
  | "done"
  | "nothing"
  | "held"
  | "failed"
  | "consent_withdrawn"
  | "interrupted";

function plural(n: number, one: string, many: string): string {
  return n === 1 ? one : many;
}

export const LEARN_TITLE = "Aprender a responder";

export const LEARN_INTRO =
  "NestAdmin mira cómo respondió usted a sus correos recientes para proponerle un tono y reglas de respuesta. No envía ni registra nada: usted decide después.";

/** Códigos de fallo que puede traer el backend (separados por comas) y su explicación. */
const FAILURE_WORDS: Record<string, string> = {
  no_consent: "falta su consentimiento para enviar",
  use_local: "se eligió la IA local",
  no_local_ai: "no hay una IA local preparada en este equipo",
};

export function learnFailureWords(code: string): string {
  return FAILURE_WORDS[code] ?? readErrorWords(code);
}

/** Fase de la tarjeta, deducida del estado que devuelve el backend. `busy` es la acción en curso de la ventana. */
export function learnPhase(status: LearnStatusLike, busy: "collect" | "run" | "retry" | null = null): LearnPhase {
  if (busy === "collect" || busy === "retry") return "collecting";
  if (busy === "run") return "running";
  if (status.error) return "failed";
  switch (status.state) {
    case "held":
      return "held";
    case "done":
      return "done";
    case "nothing":
      return "nothing";
    case "sent":
      return status.interrupted ? "interrupted" : "running";
    case "awaiting_consent": {
      const codes = (status.failure ?? "").split(",").map((c) => c.trim()).filter(Boolean);
      if (codes.includes("consent_withdrawn")) return "consent_withdrawn";
      return codes.length ? "failed" : "awaiting_consent";
    }
    case "collected":
      return status.failure ? "failed" : "collecting";
    default:
      return status.read_done ? "starting" : "waiting_read";
  }
}

export function learnPhaseLabel(phase: LearnPhase): string {
  switch (phase) {
    case "waiting_read":
      return "Espera a la primera lectura";
    case "starting":
    case "collecting":
      return "Recogiendo";
    case "awaiting_consent":
      return "Espera su decisión";
    case "running":
      return "Aprendiendo";
    case "done":
      return "Terminado";
    case "nothing":
      return "Nada que aprender";
    case "held":
      return "Retenido";
    case "failed":
    case "interrupted":
      return "Sin terminar";
    case "consent_withdrawn":
      return "Consentimiento retirado";
  }
}

export function learnPhaseTone(phase: LearnPhase): "ok" | "neutral" | "info" | "warn" {
  switch (phase) {
    case "done":
    case "nothing":
      return "ok";
    case "waiting_read":
      return "neutral";
    case "starting":
    case "collecting":
    case "running":
    case "awaiting_consent":
      return "info";
    default:
      return "warn";
  }
}

/** Frase de la fase, para un `aria-live`. */
export function learnPhaseLine(phase: LearnPhase): string {
  switch (phase) {
    case "waiting_read":
      return "Empezará cuando termine la primera lectura de esta cuenta.";
    case "starting":
      return "Se está preparando. En cuanto empiece, verá aquí el progreso.";
    case "collecting":
      return "Recogiendo sus correos y sus respuestas. Puede seguir usando NestAdmin: no cambia nada en su buzón.";
    case "running":
      return "Analizando las respuestas. Puede seguir usando NestAdmin; esto puede tardar un poco.";
    default:
      return "";
  }
}

/** Recuentos de la recogida, uno por línea. Siempre dice cuántos pares se enviarían, aunque sean cero. */
export function countLines(c: CountsLike): string[] {
  const out = [
    `${c.scanned} ${plural(c.scanned, "correo revisado", "correos revisados")}.`,
    `${c.automatic} ${plural(c.automatic, "automático excluido", "automáticos excluidos")} (avisos, boletines y notificaciones): no se usan.`,
    `${c.no_own_reply} ${plural(c.no_own_reply, "sin respuesta suya", "sin respuesta suya")}.`,
  ];
  if (c.no_sent_folder) out.push(`${c.no_sent_folder} no se pudieron emparejar porque su cuenta no permite leer Enviados.`);
  if (c.quote_dropped) out.push(`${c.quote_dropped} ${plural(c.quote_dropped, "descartado", "descartados")} por no poder separar su texto del citado.`);
  if (c.fetch_errors) out.push(`${c.fetch_errors} ${plural(c.fetch_errors, "no se pudo leer", "no se pudieron leer")}.`);
  if (c.over_limit) out.push(`${c.over_limit} ${plural(c.over_limit, "quedó fuera", "quedaron fuera")} por el tope de tamaño.`);
  out.push(`${c.pairs} ${plural(c.pairs, "par (correo recibido y su respuesta)", "pares (correo recibido y su respuesta)")} para aprender.`);
  return out;
}

export function pairsText(n: number): string {
  return `${n} ${plural(n, "par", "pares")}`;
}

/** Resumen de una línea del resultado «nada que aprender»: siempre con los tres recuentos. */
export function nothingText(c: CountsLike | null): { title: string; lines: string[] } {
  const title = "Nada que aprender";
  if (!c) return { title, lines: ["No hay ninguna respuesta suya de la que aprender. No es un error: NestAdmin sigue funcionando con las reglas que cree usted."] };
  return {
    title,
    lines: [
      `De ${c.scanned} ${plural(c.scanned, "correo revisado", "correos revisados")} no salió ningún par: ${c.automatic} ${plural(c.automatic, "automático", "automáticos")}, ${c.no_own_reply} ${plural(c.no_own_reply, "sin respuesta suya", "sin respuesta suya")} y ${c.no_sent_folder} porque su cuenta no permite leer Enviados.`,
      "No es un error: en este buzón no hay respuestas suyas de las que aprender. No se ha enviado nada fuera ni se ha creado ninguna regla.",
    ],
  };
}

/** A quién se enviaría, con proveedor, modelo y servidor. */
export function learnDestinationLine(d: { provider: string; model: string; host: string | null } | null): string {
  if (!d) return "No hay un proveedor de IA elegido: configúrelo antes de aprender.";
  if (!d.host) return `Se analizará con ${destinationLabel(d)}. Nada sale de este equipo.`;
  return `Se enviaría a ${destinationLabel(d)}.`;
}

export function isExternalDestination(d: { host: string | null } | null): boolean {
  return Boolean(d?.host);
}

export function sendButtonLabel(provider: string): string {
  return `Enviar a ${readProviderName(provider)}`;
}

export const LOCAL_BUTTON = "Usar IA local";
export const LOCAL_BUTTON_HELP = "Se analiza en este equipo con la IA local; nada sale de él.";
export const PREVIEW_BUTTON = "Ver lo que se enviaría";
export const PREVIEW_HIDE_BUTTON = "Ocultar lo que se enviaría";
export const PREVIEW_LABEL = "Texto que se enviaría, tal cual (solo lectura)";
export const PREVIEW_NOTE =
  "Es exactamente lo que saldría: las direcciones se sustituyen por alias y los datos sensibles se enmascaran. Si algo cambia antes del envío, NestAdmin lo rechaza.";
export const NEVER_SENT_HEADING = "Qué no sale nunca";
export const RETRY_BUTTON = "Reintentar";
export const REPREVIEW_BUTTON = "Volver a generar la vista previa";

/** El envío no se puede lanzar sin haber visto la vista previa. */
export const SEE_FIRST = "Para enviar, primero vea lo que se enviaría.";

export function consentLine(d: { provider: string; model: string; host: string | null }): string {
  return `Al pulsar «${sendButtonLabel(d.provider)}» da su consentimiento para enviar ese texto, a ese proveedor, modelo y servidor, una sola vez. Si algo cambia, tendrá que verlo de nuevo.`;
}

export function heldLine(status: LearnStatusLike): string {
  const h = status.held;
  if (!h) return "El destino de la IA cambió. Vuelva a generar la vista previa.";
  const was = h.authorized ? destinationLabel(h.authorized) : "ninguno confirmado";
  return `El destino de la IA cambió (antes: ${was}; ahora: ${destinationLabel(h.now)}). No ha salido nada: vuelva a generar la vista previa y decida de nuevo.`;
}

/** Qué falló y qué puede hacer. El texto del error del servicio nunca se muestra: solo su código traducido. */
export function failureLine(status: LearnStatusLike): string {
  if (status.error) return `No se pudo leer su bandeja: ${readErrorWords(status.error.code)}. Puede reintentar: se volverá a recoger y verá la vista previa de nuevo.`;
  const codes = [...new Set((status.failure ?? "").split(",").map((c) => c.trim()).filter(Boolean))];
  const words = codes.map(learnFailureWords);
  return `No se pudo aprender: ${words.length ? words.join("; ") : "sin respuesta válida"}. No se ha creado ninguna regla. Puede reintentar.`;
}

export const WITHDRAWN_LINE =
  "Retiró el consentimiento antes de que saliera el texto: no ha salido nada y tampoco se usó la IA local. Si quiere, vea de nuevo lo que se enviaría y decida.";

export const INTERRUPTED_LINE = "El aprendizaje se cortó antes de terminar. No se repite solo: pulse «Reintentar» para volver a recoger y decidir.";

/** Resultado terminado: con qué IA y cuántos pares, sin prometer reglas que no existan. */
export function doneLine(status: LearnStatusLike): string {
  const who = status.ai === "local" ? "la IA local" : "la IA externa elegida";
  const fallback = status.ai === "local" && status.fallback ? ` (el proveedor externo no respondió: ${learnFailureWords(status.fallback.split(",")[0] ?? "")})` : "";
  const pairs = status.counts?.pairs ?? status.pairs;
  return `Terminado con ${who}${fallback}: ${pairsText(pairs)} analizados. No se envió ningún correo ni se activó ninguna regla.`;
}

export const GO_PROFILE = "Ver la propuesta de perfil";
export const GO_RULES = "Ver las reglas propuestas";

// ---- Propuesta de perfil (D171) ----

export const PROPOSAL_TITLE = "Tono aprendido de sus respuestas";
export const PROPOSAL_INTRO =
  "A partir de cómo responde usted, NestAdmin propone este tono para su perfil. Solo cambia el campo Tono; no se aplica hasta que usted lo pulse.";
export const PROPOSAL_BEFORE = "Antes";
export const PROPOSAL_AFTER = "Después";
export const PROPOSAL_EMPTY = "(vacío)";
export const PROPOSAL_APPLY = "Aplicar al perfil";
export const PROPOSAL_DISMISS = "Descartar";
export const PROPOSAL_NOTE = "Si cambia lo que se envía a la IA, tendrá que volver a autorizarlo.";

export function proposalBlockedLine(codes: ReadonlyArray<{ code: string }>): string {
  const words: Record<string, string> = {
    iban: "contiene un IBAN",
    card: "contiene un número de tarjeta",
    secret: "contiene algo que parece una clave",
    too_long: "supera el tope de longitud del perfil",
  };
  const parts = [...new Set(codes.map((c) => words[c.code] ?? c.code))];
  return `No se puede aplicar: el perfil propuesto ${parts.join(" y ")}. Descártelo o edite el perfil a mano.`;
}

// ---- Reglas aprendidas (D172-D174) ----

export const BATCH_TITLE = "Reglas aprendidas de sus respuestas";

export function batchIntro(n: number): string {
  return `${n} ${plural(n, "regla propuesta", "reglas propuestas")} a partir de cómo respondió usted. Revise cada una y desmarque las que no quiera: no se crea ni se activa nada hasta que pulse el botón.`;
}

export function batchEmpty(): string {
  return "Todavía no hay reglas aprendidas que proponer. Aparecen cuando usted respondió varias veces a la misma dirección sobre el mismo tema (y ese tema es una de las categorías de facturación).";
}

export function rodajeNotice(sends: number): string {
  return `Las ${sends} primeras respuestas de cada regla irán a revisión (rodaje). Después se enviarán solas si pasan las comprobaciones.`;
}

/** Evidencia de una regla: solo recuentos y la dirección o el dominio de la propia regla. */
export function evidenceText(item: { learned: number; count: number; subject: string; category: string | null; addresses: number }, categoryName: (c: string) => string): string {
  const n = item.learned || item.count;
  const about = item.category ? ` sobre ${categoryName(item.category)}` : "";
  const where = item.addresses > 1 ? `${item.addresses} direcciones de ${item.subject}` : item.subject;
  return `Usted respondió ${n} ${plural(n, "vez", "veces")} a ${where}${about} (primera carga).`;
}

export function acceptButton(marked: number): string {
  return marked ? `Aceptar y activar las marcadas (${marked})` : "Aceptar y activar las marcadas";
}

export const NONE_MARKED = "Marque al menos una regla.";

const BATCH_FAILURE_WORDS: Record<string, string> = {
  invalid_rule: "no es una regla válida",
  not_valid: "la sugerencia ya no es válida",
  rule_exists: "ya existe una regla igual",
  rule_archived: "existe una regla archivada igual",
  write_failed: "no se pudo guardar",
  batch_write_failed: "no se pudo guardar la tanda",
};

/** Qué regla hizo fallar el lote: no se activó ninguna y se puede desmarcar. */
export function batchFailureLine(f: { name: string | null; code: string }): string {
  // Sin regla asociada (falló la escritura de la tanda): no hay nada que desmarcar.
  if (f.name === null) return `No se activó ninguna regla: ${BATCH_FAILURE_WORDS[f.code] ?? `falló (${f.code})`}. Vuelva a pulsar el botón; si se repite, avise.`;
  return `No se activó ninguna regla: «${f.name}» ${BATCH_FAILURE_WORDS[f.code] ?? `falló (${f.code})`}. Desmárquela y vuelva a pulsar el botón.`;
}

/** La huella de la tanda caducó (o una sugerencia ya no vale): se dice lo que respondió el backend y que se recargó la lista. */
export function batchStaleLine(reason: string): string {
  const why = reason.trim();
  return `${why ? `${why.endsWith(".") ? why : `${why}.`} ` : ""}Se ha recargado la lista de reglas propuestas: revísela y vuelva a marcar.`;
}

export function acceptedLine(outcome: { activated: number; already_active: number }, sends: number): string {
  const parts: string[] = [];
  if (outcome.activated) parts.push(`${outcome.activated} ${plural(outcome.activated, "regla activada", "reglas activadas")}`);
  if (outcome.already_active) parts.push(`${outcome.already_active} ${plural(outcome.already_active, "ya estaba activa", "ya estaban activas")}`);
  return `${parts.join(" y ")}. Las ${sends} primeras respuestas de cada una irán a revisión.`;
}

export const DEACTIVATE_BUTTON = "Desactivar las aprendidas";

export function deactivateConfirm(n: number): string {
  return `Desactivar ${n} ${plural(n, "regla aprendida", "reglas aprendidas")}. Las respuestas volverán a revisión.`;
}

export function deactivatedLine(n: number): string {
  return n ? `${n} ${plural(n, "regla aprendida desactivada", "reglas aprendidas desactivadas")}.` : "No había reglas aprendidas activas: no se cambió nada.";
}

// ---- Etiquetas de las tarjetas de regla ----

export interface OriginLike {
  evidence_origin?: string;
  rodaje?: { left: number; of: number };
}

/** «Aprendida» y, mientras queden envíos de rodaje, «En rodaje: n de N». Sin datos del backend, nada. */
export function learnedChips(origin: OriginLike | null | undefined): { learned: boolean; rodaje: string | null } {
  const learned = origin?.evidence_origin === "learned";
  const r = origin?.rodaje;
  const rodaje = learned && r && r.left > 0 ? `En rodaje: ${Math.max(0, r.of - r.left)} de ${r.of}` : null;
  return { learned, rodaje };
}
