import assert from "node:assert/strict";
import { beforeEach, test } from "node:test";
import {
  BOARD_COLUMNS,
  BOARD_PAGE,
  WAITING_FOR_PROVIDER,
  boardColumnOf,
  breakSegments,
  clearTaskInputCache,
  filterByState,
  groupForBoard,
  humanizeTaskError,
  RETRY_QUEUED,
  isKnownState,
  limitColumn,
  parseTaskInput,
  parseTasksView,
  showStateChip,
  taskReason,
  taskTitle,
} from "../src/tasks-text.ts";

beforeEach(() => clearTaskInputCache());

const email = (over = {}) => ({
  id: 7,
  kind: "email",
  input_ref: JSON.stringify({ message_id: "google-1:1a18c0ffee", message: { id: "google-1:1a18c0ffee", subject: "Asunto de la entrada", from: "ana@ejemplo.invalid" } }),
  result: null,
  ...over,
});

test("el asunto del resultado gana al de la entrada", () => {
  const t = email({ result: { message: { subject: "Asunto del resultado", from: "otra@ejemplo.invalid" } } });
  assert.equal(taskTitle(t), "Asunto del resultado");
});

test("sin resultado se usa el asunto de la entrada", () => {
  assert.equal(taskTitle(email()), "Asunto de la entrada");
});

test("sin asunto se usa «Correo de» el remitente, primero del resultado y luego de la entrada", () => {
  const sinAsunto = { message_id: "m-1", message: { subject: null, from: "ana@ejemplo.invalid" } };
  assert.equal(taskTitle(email({ input_ref: JSON.stringify(sinAsunto) })), "Correo de ana@ejemplo.invalid");
  assert.equal(
    taskTitle(email({ input_ref: JSON.stringify(sinAsunto), result: { message: { subject: "", from: "bea@ejemplo.invalid" } } })),
    "Correo de bea@ejemplo.invalid",
  );
});

test("un asunto vacío o en blanco no cuenta", () => {
  const t = email({ input_ref: JSON.stringify({ message: { subject: "   ", from: "ana@ejemplo.invalid" } }) });
  assert.equal(taskTitle(t), "Correo de ana@ejemplo.invalid");
});

test("sin asunto ni remitente: «Correo sin asunto»; el id del mensaje nunca es título", () => {
  const t = email({ input_ref: JSON.stringify({ message_id: "google-1:1a18c0ffee" }) });
  assert.equal(taskTitle(t), "Correo sin asunto");
  assert.ok(!taskTitle(t).includes("google-1"));
  assert.equal(taskTitle(email({ input_ref: "esto no es JSON" })), "Correo sin asunto");
  assert.equal(taskTitle(email({ input_ref: "[1,2]" })), "Correo sin asunto");
});

test("la factura conserva su título: documento del resultado, de la entrada o «Documento N»", () => {
  const base = { id: 3, kind: "invoice", input_ref: JSON.stringify({ document: "entrada.pdf" }), result: null };
  assert.equal(taskTitle(base), "entrada.pdf");
  assert.equal(taskTitle({ ...base, result: { document: "resultado.pdf" } }), "resultado.pdf");
  assert.equal(taskTitle({ ...base, input_ref: "{}" }), "Documento 3");
});

test("la entrada se lee una vez por tarea y se vuelve a leer si cambia", () => {
  const t = email();
  const first = parseTaskInput(t);
  assert.equal(parseTaskInput(t), first, "mismo objeto: sin nuevo parseo");
  assert.equal(parseTaskInput({ ...t }), first, "otro objeto con el mismo texto, también");
  const changed = parseTaskInput({ id: t.id, input_ref: JSON.stringify({ message: { subject: "Nuevo" } }) });
  assert.notEqual(changed, first);
  assert.equal(changed.message.subject, "Nuevo");
});

// ---- Vistas de Tareas (D147) ----

const t = (id, state, over = {}) => ({ id, kind: "email", input_ref: "{}", result: null, state, attempts: 0, error: null, created_at: id, updated_at: id, ...over });

test("la preferencia de vista solo admite «list» y «board»; lo demás da «list»", () => {
  assert.equal(parseTasksView("board"), "board");
  assert.equal(parseTasksView("list"), "list");
  for (const bad of [null, undefined, "", "Board", "kanban", 1, {}, ["board"]]) assert.equal(parseTasksView(bad), "list");
});

test("cada estado va a su columna y el orden de columnas es el pedido", () => {
  assert.deepEqual(BOARD_COLUMNS.map((c) => c.label), ["En cola", "En curso", "En revisión", "Con error", "Cerradas"]);
  const column = (state) => boardColumnOf({ state });
  assert.equal(column("queued"), "queued");
  assert.equal(column("running"), "running");
  assert.equal(column("awaiting_review"), "review");
  assert.equal(column("failed"), "error");
  assert.equal(column("completed"), "closed");
  assert.equal(column("cancelled"), "closed");
});

test("un estado desconocido no se pierde: va a «Con error» y se muestra tal cual", () => {
  assert.equal(boardColumnOf({ state: "paused" }), "error");
  assert.equal(isKnownState("paused"), false);
  assert.equal(showStateChip({ state: "paused" }), true);
  const groups = groupForBoard([t(1, "paused"), t(2, "failed")]);
  assert.deepEqual(groups.error.map((x) => x.id).sort(), [1, 2]);
});

test("el estado solo se muestra donde aporta: completada o descartada, no en la columna que ya lo dice", () => {
  for (const state of ["completed", "cancelled"]) assert.equal(showStateChip({ state }), true);
  for (const state of ["queued", "running", "awaiting_review", "failed"]) assert.equal(showStateChip({ state }), false);
});

test("en cola, en curso y en revisión van las más antiguas primero, con el id como desempate", () => {
  const tasks = [t(5, "queued", { created_at: 30 }), t(2, "queued", { created_at: 10 }), t(3, "queued", { created_at: 10 }), t(9, "running", { created_at: 9 }), t(8, "running", { created_at: 1 }), t(7, "awaiting_review", { created_at: 4 }), t(6, "awaiting_review", { created_at: 2 })];
  const g = groupForBoard(tasks);
  assert.deepEqual(g.queued.map((x) => x.id), [2, 3, 5]);
  assert.deepEqual(g.running.map((x) => x.id), [8, 9]);
  assert.deepEqual(g.review.map((x) => x.id), [6, 7]);
});

test("con error y cerradas van las más recientes primero (updated_at, luego id) y la entrada no cambia", () => {
  const tasks = [t(1, "failed", { updated_at: 5 }), t(2, "failed", { updated_at: 9 }), t(3, "failed", { updated_at: 9 }), t(4, "completed", { updated_at: 1 }), t(5, "cancelled", { updated_at: 8 })];
  const copy = tasks.map((x) => x.id);
  const g = groupForBoard(tasks);
  assert.deepEqual(g.error.map((x) => x.id), [3, 2, 1]);
  assert.deepEqual(g.closed.map((x) => x.id), [5, 4]);
  assert.deepEqual(tasks.map((x) => x.id), copy);
});

test("una columna vacía queda como lista vacía, no ausente", () => {
  const g = groupForBoard([]);
  for (const { key } of BOARD_COLUMNS) assert.deepEqual(g[key], []);
});

test("el límite inicial es 50 y «Ver más» añade 50", () => {
  const items = Array.from({ length: 120 }, (_, i) => i);
  assert.equal(BOARD_PAGE, 50);
  let r = limitColumn(items);
  assert.equal(r.visible.length, 50);
  assert.equal(r.hidden, 70);
  r = limitColumn(items, 50 + BOARD_PAGE);
  assert.equal(r.visible.length, 100);
  assert.equal(r.hidden, 20);
  r = limitColumn(items, 150);
  assert.equal(r.visible.length, 120);
  assert.equal(r.hidden, 0);
  assert.equal(limitColumn(items, -3).visible.length, 50);
  assert.equal(limitColumn(items, Number.NaN).visible.length, 50);
  assert.deepEqual(limitColumn([], 50), { visible: [], hidden: 0 });
});

test("el filtro de la lista deja todas con «» y solo el estado pedido con otro valor", () => {
  const tasks = [t(1, "queued"), t(2, "failed"), t(3, "failed"), t(4, "completed")];
  assert.equal(filterByState(tasks, "").length, 4);
  assert.deepEqual(filterByState(tasks, "failed").map((x) => x.id), [2, 3]);
  assert.deepEqual(filterByState(tasks, "running"), []);
  assert.notEqual(filterByState(tasks, ""), tasks);
});

test("el motivo de una tarea en cola con la espera global activa es corto; sin espera no hay motivo", () => {
  assert.deepEqual(taskReason(t(1, "queued"), true), { tone: "muted", text: WAITING_FOR_PROVIDER });
  assert.equal(WAITING_FOR_PROVIDER, "Esperando al proveedor de IA");
  assert.equal(taskReason(t(1, "queued"), false), null);
  assert.equal(taskReason(t(1, "running"), true), null);
});

test("un fallo conserva su texto completo y, sin error guardado, dice «Motivo no consta»", () => {
  const largo = "Ollama cloud rechazó esta tarea (error 422). Las demás siguen en marcha. Puede reintentarla o elegir otro modelo.";
  assert.deepEqual(taskReason(t(1, "failed", { error: { message: largo } }), true), { tone: "error", text: largo });
  assert.deepEqual(taskReason(t(1, "failed"), false), { tone: "missing", text: "Motivo no consta" });
});

test("las incidencias de una factura se traducen y se unen; un código sin traducción se muestra tal cual", () => {
  const factura = t(1, "awaiting_review", { kind: "invoice", result: { issues: ["missing_tax", "otra"] } });
  assert.deepEqual(taskReason(factura, false, (c) => (c === "missing_tax" ? "Falta el IVA" : c)), { tone: "plain", text: "Falta el IVA, otra" });
  assert.deepEqual(taskReason(factura, false), { tone: "plain", text: "missing_tax, otra" });
  assert.equal(taskReason(t(1, "awaiting_review", { result: { issues: [] } }), false), null);
});

test("las direcciones largas se parten en sus separadores y las palabras cortas quedan enteras", () => {
  const direccion = "contabilidad@suministros-omega.test";
  const trozos = breakSegments(`Correo de ${direccion}`);
  assert.equal(trozos.join(""), `Correo de ${direccion}`);
  assert.ok(trozos.includes("Correo"));
  assert.ok(trozos.includes("contabilidad@"));
  assert.ok(trozos.every((x) => x.length < 20 || /\s/.test(x)), "ningún trozo largo");
  assert.deepEqual(breakSegments("Cambio de horario"), ["Cambio", " ", "de", " ", "horario"]);
  assert.deepEqual(breakSegments(""), []);
});

test("humanizeTaskError quita el código conocido y pone la mayúscula", () => {
  assert.equal(humanizeTaskError("unavailable: no se pudo conectar con Ollama"), "No se pudo conectar con Ollama");
  assert.equal(humanizeTaskError("timeout: tiempo agotado"), "Tiempo agotado");
  assert.equal(humanizeTaskError("invalid_response: JSON no válido"), "JSON no válido");
});

test("«estado HTTP N», con o sin tipo, no afirma que falte el modelo", () => {
  const frase = "El proveedor de IA respondió con un error 404. Puede reintentar la tarea.";
  assert.equal(humanizeTaskError("unavailable: estado HTTP 404"), frase);
  assert.equal(humanizeTaskError("unavailable: estado HTTP 404 (not_found)"), frase);
  assert.equal(humanizeTaskError("estado HTTP 404"), frase);
  assert.ok(!frase.includes("modelo"));
});

test("un texto no reconocido se conserva literal y uno vacío da «»", () => {
  assert.equal(humanizeTaskError("Ollama cloud rechazó esta tarea (error 422)."), "Ollama cloud rechazó esta tarea (error 422).");
  assert.equal(humanizeTaskError("otro_codigo: algo"), "otro_codigo: algo");
  assert.equal(humanizeTaskError("unavailable:"), "");
  assert.equal(humanizeTaskError("   "), "");
  assert.deepEqual(taskReason(t(1, "failed", { error: { message: "unavailable: " } }), false), { tone: "missing", text: "Motivo no consta" });
});

test("el motivo de un fallo usa el texto legible", () => {
  assert.deepEqual(taskReason(t(1, "failed", { error: { message: "unavailable: estado HTTP 404" } }), false), {
    tone: "error",
    text: "El proveedor de IA respondió con un error 404. Puede reintentar la tarea.",
  });
});

test("una tarea reencolada no muestra su error anterior", () => {
  const vieja = t(1, "queued", { attempts: 1, error: { message: "Ollama cloud rechazó esta tarea (error 422)." } });
  assert.deepEqual(taskReason(vieja, true), { tone: "muted", text: WAITING_FOR_PROVIDER });
  assert.deepEqual(taskReason(vieja, false), { tone: "muted", text: RETRY_QUEUED });
  assert.equal(RETRY_QUEUED, "Reintento en cola");
  assert.equal(taskReason(t(2, "queued"), false), null);
});
