import assert from "node:assert/strict";
import { beforeEach, test } from "node:test";
import {
  BOARD_COLUMNS,
  BOARD_PAGE,
  WAITING_FOR_PROVIDER,
  ACTIVATE_WITHOUT_CONDITION_CONFIRM,
  CANCEL_ALSO_CONFIRM,
  CANCEL_ALSO_DONE,
  CANCEL_ALSO_LABEL,
  actionLabel,
  attemptsText,
  downloadFailureText,
  filterTasks,
  pipelineChip,
  runStageText,
  waivedNote,
  DISCARD_HELD_CONFIRM,
  LINKED_AFTER_WAIVE_TEXT,
  archiveConfirm,
  continueConfirm,
  describeCondition,
  holdsReply,
  indexPipeline,
  pipelineContext,
  pipelineReason,
  reasonWithPipeline,
  scopeOf,
  stepText,
  validateTemplateForm,
  waivableBlocker,
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

// ---- Plantillas de pipeline (D149-D151) ----

const step = (id, stage, kind, label, over = {}) => ({
  id,
  stage,
  kind,
  label,
  task_id: null,
  task_state: null,
  waived: false,
  waived_note: null,
  error_code: null,
  waivable: false,
  linked_after_waive: false,
  blocked: null,
  ...over,
});

/** Ejecución con una factura (paso 1) y la respuesta (paso 2) retenida por ella. */
function runWith(invoice, code, reason = "blocked", over = {}) {
  const reply = step(2, 2, "reply", "Responder al correo", { task_id: 20, task_state: "queued", blocked: { reason, by: [{ step_id: 1, code }] } });
  return { id: 1, template_id: "facturas-zeta", template_version: 2, template_name: "Facturas de Zeta", steps: [step(1, 1, "attachment_invoices", "factura-0910.pdf", invoice), reply], ...over };
}

const entryOfReply = (run) => indexPipeline([run]).get(20);

test("esperando: «Esperando a “paso”» en tono secundario", () => {
  const run = runWith({ task_id: 10, task_state: "awaiting_review" }, "waiting", "waiting");
  assert.deepEqual(pipelineReason(entryOfReply(run)), { tone: "muted", text: "Esperando a “factura-0910.pdf”" });
});

test("bloqueada: cada motivo tiene su texto, en tono de aviso", () => {
  const casos = [
    [{ task_id: 10, task_state: "failed" }, "failed", "Bloqueada: “factura-0910.pdf” falló"],
    [{ task_id: 10, task_state: "cancelled" }, "cancelled", "Bloqueada: “factura-0910.pdf” se canceló"],
    [{}, "attachment_missing", "Bloqueada: falta el adjunto “factura-0910.pdf”"],
    [{}, "attachment_rejected", "Bloqueada: “factura-0910.pdf” no se puede importar y no se resolverá solo"],
  ];
  for (const [invoice, code, text] of casos) {
    assert.deepEqual(pipelineReason(entryOfReply(runWith(invoice, code))), { tone: "warn", text }, code);
  }
});

test("un código de bloqueo desconocido no se pierde", () => {
  const run = runWith({}, "otro_codigo");
  assert.deepEqual(pipelineReason(entryOfReply(run)), { tone: "warn", text: "Bloqueada por “factura-0910.pdf”" });
});

test("con varios pasos, la causa es el primero que no avanza", () => {
  const reply = step(3, 2, "reply", "Responder al correo", {
    task_id: 20,
    task_state: "queued",
    blocked: { reason: "blocked", by: [{ step_id: 1, code: "waiting" }, { step_id: 2, code: "failed" }] },
  });
  const run = {
    id: 1,
    template_id: "x",
    template_version: 1,
    template_name: null,
    steps: [step(1, 1, "attachment_invoices", "a.pdf", { task_id: 10, task_state: "awaiting_review" }), step(2, 1, "attachment_invoices", "b.pdf", { task_id: 11, task_state: "failed" }), reply],
  };
  assert.equal(pipelineReason(indexPipeline([run]).get(20)).text, "Bloqueada: “b.pdf” falló");
});

test("línea de contexto: «Paso n de m · plantilla»; sin nombre, el identificador", () => {
  const run = runWith({ task_id: 10, task_state: "failed" }, "failed");
  const index = indexPipeline([run]);
  assert.equal(pipelineContext(index.get(20)), "Paso 2 de 2 · Facturas de Zeta");
  assert.equal(pipelineContext(index.get(10)), "Paso 1 de 2 · Facturas de Zeta");
  assert.equal(pipelineContext(entryOfReply({ ...run, template_name: null })), "Paso 2 de 2 · facturas-zeta");
});

test("el indicador del pipeline tiene prioridad sobre la espera de la IA", () => {
  const entry = entryOfReply(runWith({ task_id: 10, task_state: "failed" }, "failed"));
  const espera = taskReason(t(20, "queued"), true);
  assert.deepEqual(espera, { tone: "muted", text: WAITING_FOR_PROVIDER });
  assert.equal(reasonWithPipeline(espera, entry).text, "Bloqueada: “factura-0910.pdf” falló");
  assert.equal(reasonWithPipeline(taskReason(t(20, "queued"), false), entry).text, "Bloqueada: “factura-0910.pdf” falló");
});

test("una tarea sin ejecución, o sin bloqueo, no cambia nada", () => {
  const espera = taskReason(t(5, "queued"), true);
  assert.equal(reasonWithPipeline(espera, undefined), espera);
  assert.equal(pipelineReason(undefined), null);
  assert.equal(pipelineContext(undefined), null);
  const index = indexPipeline([runWith({ task_id: 10, task_state: "failed" }, "failed")]);
  assert.equal(index.get(5), undefined);
  // La factura del paso 1 no está retenida: su motivo es el de siempre, con la línea de contexto.
  const factura = index.get(10);
  assert.equal(pipelineReason(factura), null);
  assert.equal(reasonWithPipeline(null, factura), null);
});

test("«Continuar sin ese paso» solo se ofrece si hay bloqueo y el paso es omitible", () => {
  const omitible = runWith({ task_id: 10, task_state: "failed", waivable: true }, "failed");
  assert.deepEqual({ id: waivableBlocker(entryOfReply(omitible)).step.id, code: waivableBlocker(entryOfReply(omitible)).code }, { id: 1, code: "failed" });
  assert.equal(waivableBlocker(entryOfReply(runWith({ task_id: 10, task_state: "failed", waivable: false }, "failed"))), null);
  assert.equal(waivableBlocker(entryOfReply(runWith({ task_id: 10, task_state: "awaiting_review", waivable: false }, "waiting", "waiting"))), null);
  assert.equal(waivableBlocker(undefined), null);
});

test("los textos de confirmación son los acordados", () => {
  const base = "La respuesta seguirá sin \u201cfactura-0910.pdf\u201d e irá siempre a revisión, aunque coincida una regla.";
  assert.equal(continueConfirm("factura-0910.pdf", "attachment_missing"), `${base} Si ese adjunto llega después, también se procesará. ¿Continuar?`);
  assert.equal(CANCEL_ALSO_CONFIRM, "Se cancelará la respuesta a este correo y no se enviará. Las facturas de este correo siguen su curso. ¿Cancelar?");
  assert.equal(CANCEL_ALSO_DONE, "Respuesta cancelada. No se ha enviado.");
  assert.equal(DISCARD_HELD_CONFIRM, "Esta factura retiene la respuesta a un correo. Si la descarta, esa respuesta quedará bloqueada hasta que usted decida. ¿Descartar?");
  assert.equal(
    ACTIVATE_WITHOUT_CONDITION_CONFIRM,
    "Esta plantilla se aplicará a todos los correos con un adjunto admitido, aunque el adjunto no sea una factura, y sus respuestas esperarán. ¿Activarla?",
  );
  assert.match(archiveConfirm("Zeta"), /“Zeta”.*¿Archivar\?$/);
  assert.equal(LINKED_AFTER_WAIVE_TEXT, "Este adjunto llegó después de continuar sin él.");
  assert.equal(CANCEL_ALSO_LABEL, "Cancelar respuesta");
});

test("una factura retiene la respuesta si es un paso sin omitir y la respuesta sigue en cola", () => {
  const run = runWith({ task_id: 10, task_state: "failed" }, "failed");
  assert.equal(holdsReply([run], 10), true);
  assert.equal(holdsReply([run], 20), false, "la respuesta no retiene a nadie");
  assert.equal(holdsReply([run], 99), false, "una tarea sin ejecución");
  const omitida = { ...run, steps: run.steps.map((s) => (s.id === 1 ? { ...s, waived: true } : s)) };
  assert.equal(holdsReply([omitida], 10), false);
  const avanzada = { ...run, steps: run.steps.map((s) => (s.id === 2 ? { ...s, task_state: "awaiting_review" } : s)) };
  assert.equal(holdsReply([avanzada], 10), false);
});

test("los pasos tienen su título y sus fases; un tipo desconocido no enseña su código", () => {
  assert.equal(stepText("attachment_invoices").title, "Facturas adjuntas: extraer, revisar y registrar");
  assert.equal(stepText("reply").title, "Respuesta al correo: clasificar y redactar, revisar y enviar");
  assert.match(stepText("reply").phases, /una sola llamada de IA/);
  assert.deepEqual(stepText("nuevo"), { title: "Paso no reconocido", phases: null });
});

test("la condición se describe en palabras", () => {
  assert.equal(describeCondition({}), "Correos de cualquier remitente");
  assert.equal(describeCondition({ sender_domain: "cliente.test" }), "Correos del dominio cliente.test");
  assert.equal(describeCondition({ sender: "compras@cliente.test" }), "Correos de compras@cliente.test");
  assert.equal(scopeOf({}), "any");
  assert.equal(scopeOf({ sender_domain: "a.test" }), "domain");
  assert.equal(scopeOf({ sender: "a@b.test" }), "sender");
});

test("validación del editor: mensajes concretos y condición normalizada", () => {
  const ok = validateTemplateForm({ name: " Zeta ", scope: "domain", domain: " Proveedor-Zeta.TEST ", sender: "" });
  assert.deepEqual(ok, { errors: {}, conditions: { sender_domain: "proveedor-zeta.test" } });
  assert.deepEqual(validateTemplateForm({ name: "A", scope: "sender", domain: "", sender: "Ventas@Zeta.test" }).conditions, { sender: "ventas@zeta.test" });
  assert.deepEqual(validateTemplateForm({ name: "A", scope: "any", domain: "ignorado", sender: "ignorado" }), { errors: {}, conditions: {} });
  assert.equal(validateTemplateForm({ name: "  ", scope: "any", domain: "", sender: "" }).errors.name, "Escriba un nombre para la plantilla.");
  assert.match(validateTemplateForm({ name: "x".repeat(81), scope: "any", domain: "", sender: "" }).errors.name, /80 caracteres/);
  assert.match(validateTemplateForm({ name: "A", scope: "domain", domain: "", sender: "" }).errors.domain, /Escriba el dominio/);
  assert.match(validateTemplateForm({ name: "A", scope: "domain", domain: "ventas@zeta.test", sender: "" }).errors.domain, /no lleva «@»/);
  assert.match(validateTemplateForm({ name: "A", scope: "domain", domain: "sin punto", sender: "" }).errors.domain, /no es un dominio válido/);
  assert.match(validateTemplateForm({ name: "A", scope: "sender", domain: "", sender: "" }).errors.sender, /dirección completa/);
  assert.match(validateTemplateForm({ name: "A", scope: "sender", domain: "", sender: "zeta.test" }).errors.sender, /no es una dirección de correo simple/);
  assert.match(validateTemplateForm({ name: "A", scope: "sender", domain: "", sender: "a@@b.test" }).errors.sender, /no es una dirección/);
});

test("«Plantillas» nunca es preferencia: la vista guardada sigue siendo solo lista o tablero", () => {
  assert.equal(parseTasksView("templates"), "list");
});

test("el chip de una tarea retenida dice Bloqueada o Esperando; sin bloqueo no hay chip", () => {
  assert.deepEqual(pipelineChip(entryOfReply(runWith({ task_id: 10, task_state: "failed" }, "failed"))), { text: "Bloqueada", tone: "warn" });
  assert.deepEqual(pipelineChip(entryOfReply(runWith({ task_id: 10, task_state: "awaiting_review" }, "waiting", "waiting"))), { text: "Esperando", tone: "neutral" });
  assert.equal(pipelineChip(indexPipeline([runWith({ task_id: 10, task_state: "failed" }, "failed")]).get(10)), null);
  assert.equal(pipelineChip(undefined), null);
});

test("el filtro «blocked» deja solo las tareas bloqueadas; los demás valores siguen igual", () => {
  const index = indexPipeline([runWith({ task_id: 10, task_state: "failed" }, "failed")]);
  const tareas = [t(20, "queued"), t(10, "failed"), t(5, "queued")];
  assert.deepEqual(filterTasks(tareas, "blocked", index).map((x) => x.id), [20]);
  const espera = indexPipeline([runWith({ task_id: 10, task_state: "awaiting_review" }, "waiting", "waiting")]);
  assert.deepEqual(filterTasks(tareas, "blocked", espera), []);
  assert.deepEqual(filterTasks(tareas, "queued", index).map((x) => x.id), [20, 5]);
  assert.equal(filterTasks(tareas, "", index).length, 3);
});

test("las etapas de «Ver pasos» no repiten el título en la descripción", () => {
  assert.deepEqual(runStageText("attachment_invoices"), { title: "Facturas adjuntas", phases: "Extraer, revisar y registrar, dentro de cada factura." });
  assert.deepEqual(runStageText("reply"), { title: "Respuesta al correo", phases: "Clasificar y redactar en un solo paso de IA; después, revisar y enviar." });
  assert.deepEqual(runStageText("nuevo"), { title: "Paso no reconocido", phases: null });
});

test("un paso con adjunto posterior muestra solo ese texto; uno omitido, que se continuó sin él", () => {
  assert.equal(waivedNote({ waived: true, waived_note: null, linked_after_waive: true }), "Este adjunto llegó después de continuar sin él.");
  assert.equal(waivedNote({ waived: true, waived_note: null, linked_after_waive: false }), "Se continuó sin este paso.");
  assert.equal(waivedNote({ waived: true, waived_note: "sin copia", linked_after_waive: false }), "Se continuó sin este paso: sin copia");
  assert.equal(waivedNote({ waived: false, waived_note: null, linked_after_waive: false }), null);
});

test("el motivo de error empieza con mayúscula sin tocar el dato", () => {
  assert.equal(humanizeTaskError("el servicio no respondió"), "El servicio no respondió");
  const tarea = t(1, "failed", { error: { message: "el servicio no respondió" } });
  assert.equal(taskReason(tarea, false).text, "El servicio no respondió");
  assert.equal(tarea.error.message, "el servicio no respondió");
});

test("un adjunto que no se descargó se explica sin enseñar el código", () => {
  assert.equal(downloadFailureText("attachment_rejected"), "No se puede importar y no se volverá a descargar.");
  assert.equal(downloadFailureText("unavailable"), "No se pudo descargar.");
  assert.equal(downloadFailureText("codigo_raro"), "No se pudo descargar.");
  assert.equal(downloadFailureText(null), "No se pudo descargar.");
  for (const code of ["attachment_rejected", "unavailable", "timeout", "codigo_raro", "attachment_too_large"]) {
    assert.ok(!downloadFailureText(code).includes(code), code);
    assert.ok(!/_/.test(downloadFailureText(code)), code);
  }
});

test("los botones repetidos llevan el título de la tarea tras el texto visible", () => {
  assert.equal(actionLabel("Ver pasos", "Pedido 2235"), "Ver pasos: Pedido 2235");
  assert.equal(actionLabel("Cancelar respuesta", "<b>x</b>"), "Cancelar respuesta: <b>x</b>");
});

test("los intentos se cuentan en singular y plural", () => {
  assert.equal(attemptsText(0), "0 intentos");
  assert.equal(attemptsText(1), "1 intento");
  assert.equal(attemptsText(3), "3 intentos");
});

test("la frase del adjunto que llega después solo aparece si el motivo es un adjunto pendiente", () => {
  const frase = "Si ese adjunto llega después";
  const sin = "La respuesta seguirá sin \u201cfactura-0910.pdf\u201d e irá siempre a revisión, aunque coincida una regla. ¿Continuar?";
  assert.ok(continueConfirm("factura-0910.pdf", "attachment_missing").includes(frase));
  for (const code of ["failed", "cancelled", "attachment_rejected", "waiting", "otro_codigo"]) {
    assert.equal(continueConfirm("factura-0910.pdf", code), sin, code);
    assert.ok(!continueConfirm("factura-0910.pdf", code).includes(frase), code);
  }
});
