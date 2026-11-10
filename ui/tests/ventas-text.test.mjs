import assert from "node:assert/strict";
import { test } from "node:test";
import {
  allowedActions, asSentence, setupNotices, documentTitle, formatDate, formatMoney, formatNumber, fractionToPercent, isIsoDate, issueConfirmText,
  isBlankLine, parseDecimalInput, percentToFraction, recurringResultText, shiftDecimal, validateLine, voidConfirmText,
} from "../src/ventas-text.ts";

test("importes en formato español sin pasar por Number", () => {
  assert.equal(formatMoney("1234.5"), "1.234,50 €");
  assert.equal(formatMoney("0"), "0,00 €");
  assert.equal(formatMoney("-12.345"), "-12,345 €");
  assert.equal(formatMoney("12345678901234567890.10"), "12.345.678.901.234.567.890,10 €");
  assert.equal(formatMoney("-0.00"), "0,00 €");
  assert.equal(formatMoney(null), "—");
  assert.equal(formatMoney("abc"), "abc");
  assert.equal(formatNumber("2.50"), "2,5");
  assert.equal(formatNumber("1000"), "1.000");
});

test("lo escrito se convierte a decimal con punto o se rechaza", () => {
  const cases = [["1.234,50", "1234.5"], ["1234,5", "1234.5"], ["1234.50", "1234.5"], ["12.5", "12.5"], ["1.234.567", "1234567"], ["007", "7"], ["0,5", "0.5"], [",5", "0.5"], [" 10 € ", "10"]];
  for (const [input, expected] of cases) assert.equal(parseDecimalInput(input), expected, input);
  for (const bad of ["", "abc", "1,2,3", "12.34.5", "1.23,4", "-5", "1e3", "."]) assert.equal(parseDecimalInput(bad), null, bad);
  assert.equal(parseDecimalInput("-12,5", { negative: true }), "-12.5");
  assert.equal(parseDecimalInput("-0", { negative: true }), "0");
});

test("el descuento pasa de porcentaje a fracción y vuelta", () => {
  assert.equal(percentToFraction("10"), "0.1");
  assert.equal(percentToFraction("12,5"), "0.125");
  assert.equal(percentToFraction("100"), "1");
  assert.equal(percentToFraction("0"), "0");
  assert.equal(fractionToPercent("0.125"), "12,5");
  assert.equal(fractionToPercent("0"), "");
  assert.equal(shiftDecimal("5", -2), "0.05");
});

test("fechas", () => {
  assert.equal(formatDate("2026-11-03"), "03/11/2026");
  assert.equal(formatDate(null), "Sin fecha");
  assert.equal(isIsoDate("2028-02-29"), true);
  assert.equal(isIsoDate("2026-02-29"), false);
  assert.equal(isIsoDate("2026-13-01"), false);
  assert.equal(isIsoDate("03/11/2026"), false);
});

const base = { description: "Servicio de prueba", quantity: "2", unit_price: "10,50", discount_percent: "", tax_percent: "21", qualification: "", exemption_text: "" };

test("una línea válida se traduce al formato del backend", () => {
  const v = validateLine({ ...base, discount_percent: "10" });
  assert.equal(v.ok, true);
  assert.deepEqual(v.line, { description: "Servicio de prueba", quantity: "2", unit_price: "10.5", discount: "0.1", tax_percent: "21", exemption: null });
});

test("el motivo del productor se muestra como frase (visto en el simulado)", () => {
  assert.equal(asSentence("el archivo conserva los marcadores PONER-AQUI"), "El archivo conserva los marcadores PONER-AQUI.");
  assert.equal(asSentence("Ya es frase."), "Ya es frase.");
  const [n] = setupNotices({ producer_configured: false, producer_problem: "verifactu-productor.json: tiene marcadores sin rellenar", chain: { state: "empty" }, today: "2026-11-10" });
  assert.match(n.detail, /^Verifactu-productor\.json: tiene marcadores sin rellenar\. Los presupuestos/);
});

test("la línea sin empezar no bloquea el guardado; una a medias sí cuenta (visto en tauri dev)", () => {
  const blank = { description: "", quantity: "1", unit_price: "", discount_percent: "", tax_percent: "21", qualification: "", exemption_text: "" };
  assert.equal(isBlankLine(blank), true);
  assert.equal(isBlankLine({ ...blank, unit_price: "10" }), false);
  assert.equal(isBlankLine({ ...blank, description: "Algo" }), false);
});

test("una rectificativa por diferencias admite cantidad negativa (visto en tauri dev)", () => {
  const v = validateLine({ ...base, quantity: "-1", unit_price: "45" });
  assert.equal(v.ok, true);
  assert.equal(v.line.quantity, "-1");
});

test("las líneas no válidas dicen qué falta; al 0 % exigen la causa", () => {
  assert.match(validateLine({ ...base, description: " " }).problem, /descripción/);
  assert.match(validateLine({ ...base, quantity: "x" }).problem, /cantidad/);
  assert.match(validateLine({ ...base, unit_price: "" }).problem, /precio/);
  assert.match(validateLine({ ...base, discount_percent: "150" }).problem, /entre 0 y 100/);
  assert.match(validateLine({ ...base, tax_percent: "0" }).problem, /causa/);
  const e = validateLine({ ...base, tax_percent: "0", qualification: "E1", exemption_text: "Art. 20" });
  assert.deepEqual(e.line.exemption, { qualification: "E1", description: "Art. 20" });
  assert.equal(validateLine({ ...base, qualification: "E1" }).line.exemption, null, "con IVA gravado no se envía exención");
});

test("las acciones ofrecidas siguen las transiciones del backend", () => {
  const a = (kind, status, superseded_by = null) => allowedActions({ kind, status, superseded_by });
  assert.deepEqual(a("invoice", "draft"), ["edit", "issue", "delete"]);
  assert.ok(a("invoice", "issued").includes("void"));
  assert.ok(!a("invoice", "sent").includes("void"), "una factura enviada no se anula (D81)");
  assert.ok(a("invoice", "sent").includes("corrective"));
  assert.ok(!a("corrective_invoice", "issued").includes("corrective"));
  assert.deepEqual(a("invoice", "voided"), []);
  assert.ok(a("quote", "accepted").includes("convert_invoice"));
  assert.ok(a("quote", "issued").includes("revise"));
  assert.deepEqual(a("quote", "converted"), []);
  assert.deepEqual(a("delivery_note", "delivered"), ["convert_invoice"]);
  assert.deepEqual(a("quote", "issued", 9), [], "una revisión sustituida no se toca");
});

test("las confirmaciones dicen lo que hace la acción", () => {
  assert.match(issueConfirmText({ kind: "invoice" }, "2026-11-03"), /03\/11\/2026.*registro Verifactu.*rectificativa/s);
  assert.match(issueConfirmText({ kind: "quote" }, null), /revisiones/);
  assert.match(voidConfirmText({ number: "F-2026-0001" }), /F-2026-0001.*no se puede deshacer/s);
  assert.match(recurringResultText({ created: [1, 2], failed: [] }), /2 borradores.*No se ha emitido nada/);
  assert.match(recurringResultText({ created: [], failed: [{ template_id: 3, reason: "sin cliente" }] }), /plantilla 3: sin cliente/);
  assert.equal(documentTitle({ kind: "invoice", number: "F-2026-0001", revision: 1, id: 1, status: "issued" }), "Factura F-2026-0001");
  assert.equal(documentTitle({ kind: "quote", number: null, revision: 1, id: 4, status: "draft" }), "Presupuesto borrador n.º 4");
});
