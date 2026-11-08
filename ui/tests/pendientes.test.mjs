import assert from "node:assert/strict";
import { test } from "node:test";
import {
  ageText,
  chooseTab,
  emailFlags,
  formatAmount,
  invoiceFlags,
  isKnownKind,
  nextSelection,
  sortForReview,
  splitByTab,
  tabOf,
  totalIsProposed,
} from "../src/pendientes-text.ts";

const labels = (flags) => flags.map((f) => f.label);

test("cada tipo va a su pestaña; uno desconocido no se pierde", () => {
  assert.equal(tabOf("invoice"), "invoices");
  assert.equal(tabOf("email"), "emails");
  assert.equal(tabOf("otro"), "emails");
  assert.equal(isKnownKind("otro"), false);
  const split = splitByTab([{ kind: "invoice" }, { kind: "email" }, { kind: "otro" }]);
  assert.equal(split.invoices.length, 1);
  assert.equal(split.emails.length, 2);
});

test("la pestaña inicial es la primera con elementos y una elegida no cambia", () => {
  assert.equal(chooseTab(null, { invoices: 2, emails: 1 }), "invoices");
  assert.equal(chooseTab(null, { invoices: 0, emails: 1 }), "emails");
  assert.equal(chooseTab(null, { invoices: 0, emails: 0 }), "invoices");
  assert.equal(chooseTab("emails", { invoices: 2, emails: 0 }), "emails");
});

test("importes en formato español, ausencia explícita y literal si no es número", () => {
  const space = (s) => s.replace(/\s/g, " ");
  assert.equal(space(formatAmount("121.00", "EUR")), "121,00 €");
  assert.equal(formatAmount("121.5", null), "121,50");
  assert.equal(formatAmount(null, "EUR"), null);
  assert.equal(formatAmount("", "EUR"), null);
  assert.equal(formatAmount("ciento veinte", "EUR"), "ciento veinte");
  assert.equal(formatAmount("10.00", "euros"), "10,00");
});

test("antigüedad con marcas en milisegundos, como Task.created_at", () => {
  const day = 86_400_000;
  const now = 1_794_000_000_000;
  assert.equal(ageText(now, now), "hoy");
  assert.equal(ageText(now + day, now), "hoy");
  assert.equal(ageText(now - day, now), "ayer");
  assert.equal(ageText(now - 30 * day, now), "hace 30 días");
});

test("el total recuperado se reconoce como propuesta", () => {
  assert.equal(totalIsProposed({ recovered: [{ field: "total" }] }), true);
  assert.equal(totalIsProposed({ recovered: [{ field: "tax" }] }), false);
  assert.equal(totalIsProposed(null), false);
});

test("una factura sin problemas no tiene marcas; sin resultado tampoco falla", () => {
  assert.deepEqual(invoiceFlags({ issues: [], duplicates: [], pages: [{ number: 1 }] }), []);
  assert.deepEqual(invoiceFlags(null), []);
});

test("cada marca de factura explica el porqué con datos", () => {
  const flags = invoiceFlags({
    fields: { subtotal: "100.00", tax: "21.00", total: "125.00" },
    issues: ["total_mismatch", "raro"],
    pages: [{ number: 1, low_confidence: true }, { number: 2 }],
    unreadable: [{ field: "total" }],
  });
  assert.deepEqual(labels(flags), ["Total no cuadra", "Incidencia: raro", "OCR de baja calidad", "Campo ilegible"]);
  assert.match(flags[0].why, /100\.00.*21\.00.*125\.00/);
  assert.match(flags[2].why, /página 1/);
});

test("las marcas no se repiten y los duplicados citan la tarea", () => {
  const flags = invoiceFlags({
    issues: ["possible_duplicate", "amount_to_review"],
    amount_review: "Revise el importe",
    duplicates: [{ existing_id: "7" }],
  });
  assert.deepEqual(labels(flags).sort(), ["Importe dudoso", "Posible duplicado"]);
  assert.match(flags.find((f) => f.label === "Posible duplicado").why, /tarea 7/);
});

test("duplicados sin incidencia también se marcan", () => {
  assert.deepEqual(labels(invoiceFlags({ duplicates: [{ existing_id: "7" }] })), ["Posible duplicado"]);
});

test("recuperación: borrador perdido bloquea; intento sin confirmar avisa", () => {
  assert.equal(emailFlags({ draft: { to: ["a@x.test"] }, recovery: { version: 1, needs_confirmation: true } })[0].label, "Requiere confirmación");
  assert.equal(emailFlags({ draft: { to: ["a@x.test"] }, recovery: { version: 1 } })[0].label, "Intento sin confirmar");
  assert.equal(emailFlags({ draft: { to: ["a@x.test"] }, attempt: { version: 2 }, version: 2 })[0].label, "Intento sin confirmar");
  assert.deepEqual(emailFlags({ draft: { to: ["a@x.test"] }, attempt: { version: 1 }, version: 2 }), []);
});

test("un correo sin destinatario se marca", () => {
  assert.deepEqual(labels(emailFlags({ draft: { to: [] } })), ["Sin destinatario"]);
  assert.deepEqual(labels(emailFlags(null)), ["Sin destinatario"]);
});

test("orden: primero con incidencias y luego lo más antiguo", () => {
  const task = (id, created_at, issues = []) => ({ id, kind: "invoice", created_at, result: { issues } });
  const sorted = sortForReview([task(1, 30), task(2, 10), task(3, 20, ["missing_tax"]), task(4, 40, ["missing_tax"])]);
  assert.deepEqual(sorted.map((t) => t.id), [3, 4, 2, 1]);
});

test("tras resolver se pasa al siguiente", () => {
  assert.equal(nextSelection([1, 2, 3], [1, 2, 3], 2), 2);
  assert.equal(nextSelection([1, 2, 3], [1, 3], 2), 3);
  assert.equal(nextSelection([1, 2, 3], [1, 2], 3), 2);
  assert.equal(nextSelection([1, 2, 3], [], 2), null);
  assert.equal(nextSelection([], [5, 6], null), 5);
  assert.equal(nextSelection([], [5, 6], 9), 5);
});
