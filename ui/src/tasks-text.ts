// Textos de Tareas, sin DOM: se prueban con `node --test`.

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
  return stripped ? body[0]!.toUpperCase() + body.slice(1) : body;
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
