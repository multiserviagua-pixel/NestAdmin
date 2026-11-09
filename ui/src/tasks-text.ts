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
