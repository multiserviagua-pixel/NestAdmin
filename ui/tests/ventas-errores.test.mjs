import assert from "node:assert/strict";
import { test } from "node:test";
import { isBlockingCode, parseSalesError, salesErrorText, setupNotices, chainText } from "../src/ventas-text.ts";

test("el código estable se separa del mensaje", () => {
  assert.deepEqual(parseSalesError("[verifactu_not_configured] falta el productor"), { code: "verifactu_not_configured", detail: "falta el productor" });
  assert.deepEqual(parseSalesError(new Error("[conflict] otro\ndocumento")), { code: "conflict", detail: "otro\ndocumento" });
  assert.deepEqual(parseSalesError("sin código"), { code: null, detail: "sin código" });
  assert.deepEqual(parseSalesError("texto [no_es_codigo] después"), { code: null, detail: "texto [no_es_codigo] después" });
  assert.deepEqual(parseSalesError(undefined), { code: null, detail: "" });
});

test("el texto para el usuario añade el encabezado del código y conserva el detalle", () => {
  assert.equal(salesErrorText("[invalid_input] falta el cliente"), "Datos no válidos: falta el cliente");
  assert.equal(
    salesErrorText("[invalid_input] datos no válidos: letterhead: el PDF está protegido con contraseña"),
    "Datos no válidos: el PDF está protegido con contraseña",
  );
  assert.equal(
    salesErrorText("[invalid_input] datos no válidos: letterhead.margins: el margen izquierdo es excesivo"),
    "Datos no válidos: el margen izquierdo es excesivo",
  );
  assert.equal(
    salesErrorText("[invalid_input] datos no válidos: lines[0].quantity: debe ser positiva"),
    "Datos no válidos: lines[0].quantity: debe ser positiva",
  );
  assert.equal(salesErrorText("[codigo_nuevo] algo"), "algo");
  assert.equal(salesErrorText("[not_found]"), "No encontrado");
  assert.equal(salesErrorText(""), "Error desconocido");
});

test("solo bloquean los códigos de Verifactu que impiden emitir", () => {
  for (const c of ["verifactu_not_configured", "verifactu_chain_broken", "verifactu_clock_behind"]) assert.equal(isBlockingCode(c), true);
  for (const c of ["conflict", "invalid_input", null]) assert.equal(isBlockingCode(c), false);
});

test("avisos de estado: productor sin configurar con motivo y cadena rota bloqueante", () => {
  const ok = { producer_configured: true, producer_problem: null, chain: { state: "ok", records: 2 } };
  assert.deepEqual(setupNotices(ok), []);
  assert.deepEqual(setupNotices(null), []);
  const missing = setupNotices({ ...ok, producer_configured: false, producer_problem: "el NIF no es válido" });
  assert.equal(missing.length, 1);
  assert.equal(missing[0].canReload, true);
  assert.equal(missing[0].blocksInvoices, true);
  assert.match(missing[0].detail, /^El NIF no es válido. /);
  assert.match(missing[0].detail, /Volver a leer/);
  const broken = setupNotices({ ...ok, chain: { state: "broken", at_id: 7, reason: "huella distinta" } });
  assert.equal(broken[0].tone, "error");
  assert.equal(broken[0].canReload, false);
  assert.equal(broken[0].blocksInvoices, true);
  assert.match(broken[0].detail, /huella distinta/);
});

test("texto de la cadena", () => {
  assert.match(chainText({ state: "empty" }), /vacía/);
  assert.match(chainText({ state: "ok", records: 1 }), /1 registro verificado/);
  assert.match(chainText({ state: "ok", records: 3 }), /3 registros verificados/);
  assert.match(chainText({ state: "broken", at_id: 4, reason: "x" }), /rota en el registro 4/);
});
