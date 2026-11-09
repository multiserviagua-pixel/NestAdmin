import assert from "node:assert/strict";
import { beforeEach, test } from "node:test";
import { clearTaskInputCache, parseTaskInput, taskTitle } from "../src/tasks-text.ts";

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
