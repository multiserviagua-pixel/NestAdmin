// Textos de Tareas, sin DOM: se prueban con `node --test`.

import { SEND_KIND, sendTaskTitle } from "./ventas-envio-text.ts";

/** Lo mínimo de una tarea que necesita esta lógica. */
export interface TitledTask {
  id: number;
  kind: string;
  input_ref: string;
  result: Record<string, any> | null;
}

const cache = new Map<number, { ref: string; input: Record<string, any> }>();

/**
 * La entrada de una tarea, ya leída. `input_ref` incluye el cuerpo del correo, así que se lee una
 * vez por tarea y se vuelve a leer solo si el texto cambió. Un texto que no es JSON da `{}`.
 */
export function parseTaskInput(task: Pick<TitledTask, "id" | "input_ref">): Record<string, any> {
  const hit = cache.get(task.id);
  if (hit && hit.ref === task.input_ref) return hit.input;
  let input: Record<string, any> = {};
  try {
    const parsed = JSON.parse(task.input_ref);
    if (parsed && typeof parsed === "object" && !Array.isArray(parsed)) input = parsed;
  } catch {
    // Una entrada ilegible no tiene título propio: se usa el de reserva.
  }
  cache.set(task.id, { ref: task.input_ref, input });
  return input;
}

/** Olvida las entradas leídas (las pruebas lo usan para empezar de cero). */
export function clearTaskInputCache() {
  cache.clear();
}

function text(value: unknown): string | null {
  return typeof value === "string" && value.trim() !== "" ? value.trim() : null;
}

/**
 * Título de una tarea. Factura: el documento. Correo: el asunto del resultado, luego el de la
 * entrada, luego «Correo de {remitente}» y por último «Correo sin asunto». El identificador del
 * mensaje nunca es título.
 */
export function taskTitle(task: TitledTask): string {
  const input = parseTaskInput(task);
  if (task.kind === "invoice") return task.result?.document ?? input.document ?? `Documento ${task.id}`;
  if (task.kind === SEND_KIND) return sendTaskTitle(task.result);
  const subject = text(task.result?.message?.subject) ?? text(input.message?.subject);
  if (subject) return subject;
  const from = text(task.result?.message?.from) ?? text(input.message?.from);
  return from ? `Correo de ${from}` : "Correo sin asunto";
}

// ---- Vistas de Tareas (D147): lista y tablero sobre los mismos datos ----

/** Lo que necesitan el tablero y la lista de una tarea (un `Task` completo lo cumple). */
export interface BoardTask extends TitledTask {
  state: string;
  attempts: number;
  error: { message: string } | null;
  created_at: number;
  updated_at: number;
}

export type TasksView = "list" | "board";

/**
 * Clave de la preferencia de vista. Es solo una comodidad de pantalla del navegador integrado
 * (la última vista elegida), no datos de NestAdmin: no se guarda en la base ni cuenta como estado.
 */
export const TASKS_VIEW_KEY = "nestadmin.tasks.view";

/** Valida la preferencia guardada: cualquier cosa que no sea «board» o «list» da «list». */
export function parseTasksView(value: unknown): TasksView {
  return value === "board" ? "board" : "list";
}

export type BoardColumn = "queued" | "running" | "review" | "error" | "closed";

export const BOARD_COLUMNS: ReadonlyArray<{ key: BoardColumn; label: string }> = [
  { key: "queued", label: "En cola" },
  { key: "running", label: "En curso" },
  { key: "review", label: "En revisión" },
  { key: "error", label: "Con error" },
  { key: "closed", label: "Cerradas" },
];

const KNOWN_STATES = new Set(["queued", "running", "awaiting_review", "failed", "completed", "cancelled"]);

/** Columna de una tarea. Un estado desconocido no se pierde: va a «Con error» (ver `isKnownState`). */
export function boardColumnOf(task: Pick<BoardTask, "state">): BoardColumn {
  switch (task.state) {
    case "queued":
      return "queued";
    case "running":
      return "running";
    case "awaiting_review":
      return "review";
    case "completed":
    case "cancelled":
      return "closed";
    default:
      return "error";
  }
}

export function isKnownState(state: string): boolean {
  return KNOWN_STATES.has(state);
}

/**
 * La tarjeta muestra el estado solo si aporta: en «Cerradas» distingue completada de descartada y
 * un estado desconocido se enseña tal cual; en el resto la columna ya lo dice.
 */
export function showStateChip(task: Pick<BoardTask, "state">): boolean {
  return task.state === "completed" || task.state === "cancelled" || !isKnownState(task.state);
}

/**
 * Reparte las tareas en columnas. En cola, en curso y en revisión, las más antiguas primero
 * (`created_at`, luego `id`; D137); con error y cerradas, las más recientes primero
 * (`updated_at`, luego `id`). No modifica la entrada.
 */
export function groupForBoard<T extends BoardTask>(tasks: readonly T[]): Record<BoardColumn, T[]> {
  const groups: Record<BoardColumn, T[]> = { queued: [], running: [], review: [], error: [], closed: [] };
  for (const task of tasks) groups[boardColumnOf(task)].push(task);
  for (const key of ["queued", "running", "review"] as const) groups[key].sort((a, b) => a.created_at - b.created_at || a.id - b.id);
  for (const key of ["error", "closed"] as const) groups[key].sort((a, b) => b.updated_at - a.updated_at || b.id - a.id);
  return groups;
}

/** Tarjetas que se enseñan al principio en cada columna, y las que añade «Ver más». */
export const BOARD_PAGE = 50;

/** Las primeras `shown` tarjetas y cuántas quedan ocultas. Un límite no válido vale el inicial. */
export function limitColumn<T>(items: readonly T[], shown: number = BOARD_PAGE): { visible: T[]; hidden: number } {
  const limit = Number.isInteger(shown) && shown > 0 ? shown : BOARD_PAGE;
  return { visible: items.slice(0, limit), hidden: Math.max(0, items.length - limit) };
}

/** Filtro de la lista por estado: «» deja todas. */
export function filterByState<T extends Pick<BoardTask, "state">>(tasks: readonly T[], filter: string): T[] {
  return filter ? tasks.filter((t) => t.state === filter) : [...tasks];
}

/** Aviso de espera global (D146): una tarea en cola solo dice que espera; el detalle va en el aviso superior. */
export const WAITING_FOR_PROVIDER = "Esperando al proveedor de IA";

export interface TaskReason {
  /** `muted`: informativo y sin alarma (el aviso de espera ya advierte arriba). */
  tone: "error" | "warn" | "missing" | "plain" | "muted";
  text: string;
}

/** Códigos de error con que el backend empieza el mensaje de una tarea (`AiError::code`). */
const ERROR_CODES = ["unavailable", "unauthorized", "timeout", "invalid_response", "rate_limited", "model_unavailable", "no_credit", "request_rejected"];

/**
 * Texto de un error guardado, listo para leer. Solo cambia cómo se muestra: el dato no se toca.
 * Quita un prefijo de código conocido y pone mayúscula inicial; «estado HTTP N», con o sin
 * «(tipo)», pasa a una frase sin afirmar la causa. Un texto no reconocido se conserva literal.
 * Devuelve «» si no queda texto (quien llama dirá «Motivo no consta»).
 */
export function humanizeTaskError(text: string): string {
  let body = text.trim();
  const prefix = /^([a-z][a-z_]*):\s*/.exec(body);
  const stripped = Boolean(prefix && ERROR_CODES.includes(prefix[1]!));
  if (stripped) body = body.slice(prefix![0].length).trim();
  const http = /^estado HTTP (\d{3})(?:\s*\([^)]*\))?\.?$/i.exec(body);
  if (http) return `El proveedor de IA respondió con un error ${http[1]}. Puede reintentar la tarea.`;
  if (!body) return "";
  // Un texto con un código desconocido («otro_codigo: …») se conserva tal cual; el resto empieza con mayúscula.
  if (!stripped && /^[a-z][a-z_]*:\s/.test(body)) return body;
  return body[0]!.toUpperCase() + body.slice(1);
}

/** Motivo de una tarea que volvió a la cola tras un fallo, sin espera global. */
export const RETRY_QUEUED = "Reintento en cola";

/**
 * Motivo corto de una tarea, el mismo en la fila y en la tarjeta. Orden: el error propio, un
 * fallo sin error guardado («Motivo no consta»), la espera global, las incidencias de la factura.
 * `null` si no hay nada que decir. El texto de los fallos es el completo; recortarlo es cosa de la
 * presentación. `issueLabel` traduce los códigos de incidencia.
 */
export function taskReason(
  task: Pick<BoardTask, "state" | "error" | "attempts"> & { result: Record<string, any> | null },
  waitActive: boolean,
  issueLabel: (code: string) => string = (code) => code,
): TaskReason | null {
  // Una tarea en cola no enseña su error anterior como motivo actual (el dato se conserva hasta que se completa).
  if (task.state === "queued") {
    if (waitActive) return { tone: "muted", text: WAITING_FOR_PROVIDER };
    if (task.error || task.attempts > 0) return { tone: "muted", text: RETRY_QUEUED };
  } else if (task.error) {
    const text = humanizeTaskError(task.error.message);
    return text ? { tone: "error", text } : { tone: "missing", text: "Motivo no consta" };
  }
  const outside = repliedOutsideDetail(task);
  if (outside) return { tone: "plain", text: outside };
  if (task.state === "failed") return { tone: "missing", text: "Motivo no consta" };
  const issues: unknown = task.result?.issues;
  if (Array.isArray(issues) && issues.length) return { tone: "plain", text: issues.map((i) => issueLabel(String(i))).join(", ") };
  return null;
}

/**
 * Parte un texto largo en trozos tras `@`, `.`, `-`, `_` y `/`, para que la presentación pueda
 * cortarlo en esos puntos y no letra a letra. Las palabras cortas quedan enteras.
 */
export function breakSegments(text: string, minLength = 20): string[] {
  return text
    .split(/(\s+)/)
    .flatMap((word) => (word.length >= minLength ? (word.match(/[^@._/-]*[@._/-]?/g) ?? [word]).filter(Boolean) : [word]))
    .filter((part) => part !== "");
}

// ---- Plantillas de pipeline (D149-D151): indicador, pasos y editor, sin DOM ----

/** Pestañas de Tareas. «templates» nunca se guarda como preferencia (`parseTasksView` no la admite). */
export type TasksTab = TasksView | "templates";

/** Lo mínimo de una ejecución que usa esta lógica (`PipelineRunView` la cumple). */
export interface RunLike {
  id: number;
  template_id: string;
  template_version: number;
  template_name: string | null;
  steps: StepLike[];
}

export interface StepLike {
  id: number;
  stage: number;
  kind: string;
  label: string;
  task_id: number | null;
  task_state: string | null;
  waived: boolean;
  waived_note: string | null;
  error_code: string | null;
  waivable: boolean;
  linked_after_waive: boolean;
  blocked: { reason: "waiting" | "blocked"; by: Array<{ step_id: number; code: string }> } | null;
}

/** El paso de una tarea y la ejecución a la que pertenece. */
export interface PipelineEntry {
  run: RunLike;
  step: StepLike;
}

/** Indexa por id de tarea los pasos de las ejecuciones. Una tarea sin paso no aparece. */
export function indexPipeline(runs: readonly RunLike[]): Map<number, PipelineEntry> {
  const index = new Map<number, PipelineEntry>();
  for (const run of runs) for (const step of run.steps) if (step.task_id !== null) index.set(step.task_id, { run, step });
  return index;
}

/** El paso que se enseña como causa: el primero que no avanza; si todos avanzan, el primero. */
export function mainBlocker(entry: PipelineEntry): { step: StepLike; code: string } | null {
  const by = entry.step.blocked?.by ?? [];
  const chosen = entry.step.blocked?.reason === "blocked" ? (by.find((b) => b.code !== "waiting") ?? by[0]) : by[0];
  const step = chosen ? entry.run.steps.find((s) => s.id === chosen.step_id) : undefined;
  return chosen && step ? { step, code: chosen.code } : null;
}

/**
 * Indicador de una tarea retenida: «Esperando a “paso”» (tono secundario) o «Bloqueada: …» (aviso).
 * `null` si la tarea no tiene ejecución o nada la retiene. Tiene prioridad sobre el motivo de
 * espera de la IA: quien llama lo usa en lugar de `taskReason`.
 */
export function pipelineReason(entry: PipelineEntry | undefined): TaskReason | null {
  if (!entry?.step.blocked) return null;
  const blocker = mainBlocker(entry);
  const name = blocker?.step.label ?? "paso";
  if (entry.step.blocked.reason === "waiting") return { tone: "muted", text: `Esperando a “${name}”` };
  switch (blocker?.code) {
    case "failed":
      return { tone: "warn", text: `Bloqueada: “${name}” falló` };
    case "cancelled":
      return { tone: "warn", text: `Bloqueada: “${name}” se canceló` };
    case "attachment_missing":
      return { tone: "warn", text: `Bloqueada: falta el adjunto “${name}”` };
    case "attachment_rejected":
      return { tone: "warn", text: `Bloqueada: “${name}” no se puede importar y no se resolverá solo` };
    default:
      return { tone: "warn", text: `Bloqueada por “${name}”` };
  }
}

/** «Paso {n} de {m} · {plantilla}»: n es la posición de la etapa de la tarea. Sin nombre, el identificador de la plantilla. */
export function pipelineContext(entry: PipelineEntry | undefined): string | null {
  if (!entry) return null;
  const stages = [...new Set(entry.run.steps.map((s) => s.stage))].sort((a, b) => a - b);
  const n = stages.indexOf(entry.step.stage) + 1;
  return `Paso ${n} de ${stages.length} · ${entry.run.template_name ?? entry.run.template_id}`;
}

/** Motivo que se enseña: el del pipeline, si lo hay, y si no el de siempre. */
export function reasonWithPipeline(base: TaskReason | null, entry: PipelineEntry | undefined): TaskReason | null {
  return pipelineReason(entry) ?? base;
}

/** El paso que «Continuar sin ese paso» omitiría para una tarea bloqueada y por qué retiene; `null` si no procede. */
export function waivableBlocker(entry: PipelineEntry | undefined): { step: StepLike; code: string } | null {
  if (!entry || entry.step.blocked?.reason !== "blocked") return null;
  const blocker = mainBlocker(entry);
  return blocker?.step.waivable ? blocker : null;
}

export const CONTINUE_LABEL = "Continuar sin ese paso";
export const CANCEL_ALSO_LABEL = "Cancelar respuesta";
export const CANCEL_ALSO_CONFIRM = "Se cancelará la respuesta a este correo y no se enviará. Las facturas de este correo siguen su curso. ¿Cancelar?";
export const CANCEL_ALSO_DONE = "Respuesta cancelada. No se ha enviado.";

/**
 * Confirmación de «Continuar sin ese paso». La frase sobre un adjunto que llega después solo
 * cabe si el motivo es un adjunto pendiente (`attachment_missing`): una factura fallida o
 * cancelada ya tiene tarea y un adjunto rechazado no se vuelve a descargar. Ante la duda, se omite.
 */
export function continueConfirm(stepName: string, code: string): string {
  const base = `La respuesta seguirá sin \u201c${stepName}\u201d e irá siempre a revisión, aunque coincida una regla.`;
  return code === "attachment_missing" ? `${base} Si ese adjunto llega después, también se procesará. ¿Continuar?` : `${base} ¿Continuar?`;
}

export const DISCARD_HELD_CONFIRM =
  "Esta factura retiene la respuesta a un correo. Si la descarta, esa respuesta quedará bloqueada hasta que usted decida. ¿Descartar?";

/**
 * ¿Retiene esta tarea a una respuesta? Sí si es el paso, sin omitir, de una etapa anterior a una
 * tarea que sigue en cola. Descartarla deja a esa respuesta bloqueada.
 */
export function holdsReply(runs: readonly RunLike[], taskId: number): boolean {
  for (const run of runs) {
    const step = run.steps.find((s) => s.task_id === taskId);
    if (!step || step.waived) continue;
    if (run.steps.some((s) => s.stage > step.stage && s.task_state === "queued")) return true;
  }
  return false;
}

export const TEMPLATE_STEP_TEXT: Record<string, { title: string; phases: string }> = {
  attachment_invoices: {
    title: "Facturas adjuntas: extraer, revisar y registrar",
    phases: "Fases internas: extraer, revisar y registrar. Se hacen dentro de cada factura, no son tareas aparte.",
  },
  reply: {
    title: "Respuesta al correo: clasificar y redactar, revisar y enviar",
    phases: "Fases internas: clasificar y redactar (una sola llamada de IA), revisar y enviar.",
  },
};

export const LINKED_AFTER_WAIVE_TEXT = "Este adjunto llegó después de continuar sin él.";

/** Etapas de «Ver pasos»: título corto y debajo qué hace, sin repetir el título (el editor usa `TEMPLATE_STEP_TEXT`). */
export const RUN_STAGE_TEXT: Record<string, { title: string; phases: string }> = {
  attachment_invoices: { title: "Facturas adjuntas", phases: "Extraer, revisar y registrar, dentro de cada factura." },
  reply: { title: "Respuesta al correo", phases: "Clasificar y redactar en un solo paso de IA; después, revisar y enviar." },
};

/** Título y descripción de una etapa de «Ver pasos». Un tipo que esta versión no conoce no enseña su código. */
export function runStageText(kind: string): { title: string; phases: string | null } {
  return RUN_STAGE_TEXT[kind] ?? { title: "Paso no reconocido", phases: null };
}

/**
 * Qué decir de un adjunto que no se pudo descargar, sin enseñar nunca el código. `attachment_rejected`
 * no se vuelve a intentar; un código conocido del proveedor pasa por `humanizeTaskError`; el resto,
 * incluido un código desconocido, da una frase general.
 */
export function downloadFailureText(code: string | null | undefined): string {
  if (code === "attachment_rejected") return "No se puede importar y no se volverá a descargar.";
  const known = code && ERROR_CODES.includes(code) ? humanizeTaskError(`${code}: no se pudo descargar`) : "";
  return known ? `${known}.` : "No se pudo descargar.";
}

/** Nombre accesible de un botón repetido por tarea: el texto visible y después el título (texto, no HTML). */
export function actionLabel(visible: string, taskTitle: string): string {
  return `${visible}: ${taskTitle}`;
}

/** «1 intento» / «3 intentos». */
export function attemptsText(attempts: number): string {
  return `${attempts} ${attempts === 1 ? "intento" : "intentos"}`;
}

/** Qué se dice de un paso omitido: si su adjunto llegó después, solo eso; si no, que se continuó sin él. */
export function waivedNote(step: Pick<StepLike, "waived" | "waived_note" | "linked_after_waive">): string | null {
  if (step.linked_after_waive) return LINKED_AFTER_WAIVE_TEXT;
  if (!step.waived) return null;
  return step.waived_note ? `Se continuó sin este paso: ${step.waived_note}` : "Se continuó sin este paso.";
}

/**
 * Chip de estado de una tarea retenida por su pipeline: «Bloqueada» (aviso) o «Esperando»
 * (secundario). La tarea sigue en cola en los datos; esto solo cambia la etiqueta. `null` si
 * no está retenida.
 */
export function pipelineChip(entry: PipelineEntry | undefined): { text: string; tone: "warn" | "neutral" } | null {
  const reason = entry?.step.blocked?.reason;
  if (reason === "blocked") return { text: "Bloqueada", tone: "warn" };
  if (reason === "waiting") return { text: "Esperando", tone: "neutral" };
  return null;
}

/** Filtro «Mostrar» de la lista: un estado, «blocked» (tareas bloqueadas por su pipeline) o «» (todas). */
export const BLOCKED_FILTER = "blocked";

export function filterTasks<T extends Pick<BoardTask, "state" | "id">>(tasks: readonly T[], filter: string, index: ReadonlyMap<number, PipelineEntry>): T[] {
  if (filter === BLOCKED_FILTER) return tasks.filter((t) => index.get(t.id)?.step.blocked?.reason === "blocked");
  return filterByState(tasks, filter);
}

/** Título y fases de un paso; un tipo que esta versión no conoce se muestra tal cual. */
export function stepText(kind: string): { title: string; phases: string | null } {
  return TEMPLATE_STEP_TEXT[kind] ?? { title: "Paso no reconocido", phases: null };
}

export type SenderScope = "any" | "domain" | "sender";

type Conditions = { sender?: string; sender_domain?: string };

/** Condición de una plantilla en palabras. */
export function describeCondition(conditions: Conditions | null | undefined): string {
  if (conditions?.sender) return `Correos de ${conditions.sender}`;
  if (conditions?.sender_domain) return `Correos del dominio ${conditions.sender_domain}`;
  return "Correos de cualquier remitente";
}

export function scopeOf(conditions: Conditions | null | undefined): SenderScope {
  return conditions?.sender ? "sender" : conditions?.sender_domain ? "domain" : "any";
}

export const TEMPLATE_NAME_MAX = 80;

export interface TemplateForm {
  name: string;
  scope: SenderScope;
  domain: string;
  sender: string;
}

export interface TemplateErrors {
  name?: string;
  domain?: string;
  sender?: string;
}

// Mismas reglas que `is_domain` e `is_email_address` de la aplicación (D126); el backend decide.
const isDomain = (d: string) =>
  d.length <= 253 && d.includes(".") && d.split(".").every((l) => l.length > 0 && l.length <= 63 && !l.startsWith("-") && !l.endsWith("-") && /^[A-Za-z0-9-]+$/.test(l));
const isAddress = (a: string) => {
  const at = a.indexOf("@");
  if (at < 1 || at !== a.lastIndexOf("@")) return false;
  const local = a.slice(0, at);
  return local.length <= 64 && /^[A-Za-z0-9._%+\-'=!#$&*/?^`{|}~]+$/.test(local) && isDomain(a.slice(at + 1));
};

/** Valida el formulario con mensajes concretos. `conditions` solo es fiable sin errores. */
export function validateTemplateForm(form: TemplateForm): { errors: TemplateErrors; conditions: Conditions } {
  const errors: TemplateErrors = {};
  const name = form.name.trim();
  if (!name) errors.name = "Escriba un nombre para la plantilla.";
  else if ([...name].length > TEMPLATE_NAME_MAX) errors.name = `El nombre admite como mucho ${TEMPLATE_NAME_MAX} caracteres.`;
  let conditions: Conditions = {};
  if (form.scope === "domain") {
    const domain = form.domain.trim().toLowerCase();
    if (!domain) errors.domain = "Escriba el dominio, por ejemplo cliente.test.";
    else if (domain.includes("@")) errors.domain = "Un dominio no lleva «@». Para una dirección completa, elija «Esta dirección».";
    else if (!isDomain(domain)) errors.domain = `“${domain}” no es un dominio válido. Use un formato como cliente.test, sin espacios.`;
    else conditions = { sender_domain: domain };
  } else if (form.scope === "sender") {
    const sender = form.sender.trim().toLowerCase();
    if (!sender) errors.sender = "Escriba la dirección completa, por ejemplo compras@cliente.test.";
    else if (!isAddress(sender)) errors.sender = `“${sender}” no es una dirección de correo simple. Use un formato como compras@cliente.test.`;
    else conditions = { sender };
  }
  return { errors, conditions };
}

export const ACTIVATE_WITHOUT_CONDITION_CONFIRM =
  "Esta plantilla se aplicará a todos los correos con un adjunto admitido, aunque el adjunto no sea una factura, y sus respuestas esperarán. ¿Activarla?";

export function archiveConfirm(name: string): string {
  return `Se archivará la plantilla “${name}”: deja de aplicarse a correos nuevos. Las ejecuciones en curso y su historial se conservan. ¿Archivar?`;
}

export const TEMPLATES_EMPTY_TEXT = "Aún no hay plantillas. Una plantilla hace que la respuesta a un correo espere hasta terminar sus facturas adjuntas.";
export const TEMPLATE_AUTHORITY_NOTE = "La plantilla solo decide cuándo puede empezar cada tarea. Registrar y enviar siguen necesitando su revisión o una regla activa.";
export const TEMPLATE_SCOPE_NOTE = "Se aplica a correos con fecha desde el día en que la activa. No cambia los correos anteriores.";

// ---- Tarea cerrada porque ya estaba respondida fuera de NestAdmin (D178) ----

export const REPLIED_OUTSIDE_LABEL = "Respondido fuera de NestAdmin";

/** Texto del Historial para la entrada `mail` / `replied_outside`. */
export const REPLIED_OUTSIDE_EVENT = "Tarea cerrada: ya estaba respondido fuera de NestAdmin";

/** `true` si la tarea se cerró sin enviar porque la persona ya había respondido desde su programa de correo. */
export function isRepliedOutside(task: { result: Record<string, any> | null }): boolean {
  return task.result?.closed?.reason === "replied_outside";
}

/** Fecha corta «dd/mm/aaaa» de la respuesta, o `null` si falta o no se entiende. */
export function repliedDate(value: unknown): string | null {
  if (typeof value !== "string" || !value.trim()) return null;
  const iso = /^(\d{4})-(\d{2})-(\d{2})/.exec(value.trim());
  if (iso) return `${iso[3]}/${iso[2]}/${iso[1]}`;
  const d = new Date(value);
  if (Number.isNaN(d.getTime())) return null;
  return `${String(d.getUTCDate()).padStart(2, "0")}/${String(d.getUTCMonth() + 1).padStart(2, "0")}/${d.getUTCFullYear()}`;
}

/** Detalle de una tarea cerrada como respondida fuera; `null` si no lo es. */
export function repliedOutsideDetail(task: { result: Record<string, any> | null }): string | null {
  if (!isRepliedOutside(task)) return null;
  const date = repliedDate(task.result?.closed?.replied_at);
  return date
    ? `Usted ya respondió a este correo desde su programa de correo el ${date}. NestAdmin no envió nada.`
    : "Usted ya respondió a este correo desde su programa de correo. NestAdmin no envió nada.";
}
