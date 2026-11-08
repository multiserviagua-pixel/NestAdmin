import assert from "node:assert/strict";
import { test } from "node:test";
import {
  CATEGORY_HELP,
  CATEGORY_LABEL,
  conditionsSummary,
  formatAmount,
  ruleProblem,
  ruleRisk,
  ruleSentence,
  searchText,
} from "../src/rules-text.ts";

const invoice = (conditions) => ({
  id: "r",
  version: 1,
  action_kind: "register_invoice",
  conditions,
  active: false,
});
const send = (conditions) => ({ ...invoice(conditions), action_kind: "send_draft" });

test("importes en formato español", () => {
  assert.equal(formatAmount("500.00"), "500,00 €");
  assert.equal(formatAmount("1234.5"), "1.234,50 €");
  assert.equal(formatAmount("1250"), "1.250,00 €");
  assert.equal(formatAmount("1.234,56"), "1.234,56 €");
  assert.equal(formatAmount("mucho"), "mucho");
});

test("frase de una regla de registro", () => {
  assert.equal(
    ruleSentence(invoice({ supplier: "Alfa SL", max_total: "500.00" })),
    "Registrar sin revisión las facturas de Alfa SL de hasta 500,00 €",
  );
});

test("frase de una regla de registro sin máximo", () => {
  assert.equal(ruleSentence(invoice({ supplier: "Alfa SL" })), "Registrar sin revisión las facturas de Alfa SL");
  assert.match(conditionsSummary(invoice({ supplier: "Alfa SL" })), /sin máximo/);
});

test("frase de una regla de envío", () => {
  assert.equal(
    ruleSentence(send({ category: "consulta_pago" })),
    "Enviar sin revisión la respuesta redactada por la IA a los correos clasificados como Consulta de pago",
  );
});

test("no inventa datos que faltan", () => {
  assert.match(ruleSentence(invoice({})), /la regla no indica proveedor/);
  assert.match(ruleSentence(send({})), /la regla no indica categoría/);
  assert.equal(ruleSentence(send({ category: "nueva_categoria" })).endsWith("como nueva_categoria"), true);
});

test("tipo desconocido", () => {
  const unknown = { ...invoice({}), action_kind: "issue_recurring" };
  assert.equal(ruleSentence(unknown), "Tipo de regla no reconocido");
  assert.match(ruleRisk(unknown), /no la aplicará/);
  assert.equal(conditionsSummary(unknown), "Sin datos comparables");
  assert.equal(ruleProblem(unknown), null);
});

test("avisos de riesgo", () => {
  assert.equal(
    ruleRisk(send({ category: "consulta_pago" })),
    "Enviará correos sin preguntarle a cualquier remitente que la IA clasifique en esta categoría.",
  );
  assert.equal(
    ruleRisk(invoice({ supplier: "Alfa SL" })),
    "Registrará la factura sin preguntarle si coincide y no tiene incidencias.",
  );
});

test("problemas que impiden que la regla actúe", () => {
  assert.equal(ruleProblem(invoice({ supplier: "Alfa SL", max_total: "500.00" })), null);
  assert.match(ruleProblem(invoice({ supplier: "Alfa SL", max_total: 500 })) ?? "", /no es un importe válido/);
  assert.match(ruleProblem(invoice({ supplier: "Alfa SL", color: "rojo" })) ?? "", /«color»/);
  // La moneda (D127) y el remitente (D126) son condiciones conocidas.
  assert.equal(ruleProblem(invoice({ supplier: "Alfa SL", max_total: "500.00", currency: "EUR" })), null);
  assert.equal(ruleProblem(send({ category: "consulta_pago", sender: "pedidos@alfa.test" })), null);
  assert.equal(ruleProblem(send({ category: "consulta_pago", sender_domain: "alfa.test" })), null);
  assert.match(ruleProblem(invoice({})) ?? "", /no indica proveedor/);
  assert.match(ruleProblem(send({})) ?? "", /no indica categoría/);
});

test("las cinco categorías tienen etiqueta, descripción y ejemplo", () => {
  const keys = ["solicitud_factura", "consulta_pago", "incidencia_factura", "documentacion", "otros"];
  assert.deepEqual(Object.keys(CATEGORY_LABEL).sort(), [...keys].sort());
  assert.deepEqual(Object.keys(CATEGORY_HELP).sort(), [...keys].sort());
  for (const key of keys) {
    assert.ok(CATEGORY_HELP[key].description.length > 10, key);
    assert.ok(CATEGORY_HELP[key].example.length > 10, key);
  }
});

test("el buscador ignora mayúsculas y acentos", () => {
  const text = searchText(send({ category: "documentacion" }));
  assert.ok(text.includes("documentacion"));
  assert.ok(searchText(invoice({ supplier: "Suministros Álvarez SL" })).includes("alvarez"));
});
