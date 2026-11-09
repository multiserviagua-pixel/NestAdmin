import assert from "node:assert/strict";
import { test } from "node:test";
import { readFileSync } from "node:fs";
import {
  ANY_SENDER_CONFIRMATION,
  APPROVED_BY_PERSON,
  AUTHORIZED_BY_RULE,
  CATEGORY_HELP,
  CATEGORY_LABEL,
  AUTOMATED_NOT_CONFIRMED,
  NOT_ACTING,
  NO_EVALUATION,
  PUBLIC_MAIL_DOMAINS,
  REASON_CODES,
  actsOnNothing,
  anyAckReady,
  anySenderConsequences,
  anySenderEditNotice,
  auditMatches,
  auditMentionsRule,
  capitalize,
  conditionsSummary,
  draftConditions,
  draftOf,
  draftProblems,
  explainEvaluation,
  formatAmount,
  formatDate,
  isPublicMailDomain,
  isStoredAmount,
  normalizeAmount,
  publicDomainWarning,
  reasonText,
  ruleProblem,
  ruleRisk,
  ruleSentence,
  ruleStatus,
  searchText,
  simulationSummary,
  statsText,
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

test("frase de una regla de envío con dirección, dominio y opción avanzada", () => {
  assert.equal(
    ruleSentence(send({ category: "consulta_pago", sender: "compras@cliente.test" })),
    "Enviar sin revisión la respuesta redactada por la IA a compras@cliente.test cuando se clasifique como Consulta de pago",
  );
  assert.equal(
    ruleSentence(send({ category: "consulta_pago", sender_domain: "cliente.test" })),
    "Enviar sin revisión la respuesta redactada por la IA a cualquier dirección de @cliente.test cuando se clasifique como Consulta de pago",
  );
  assert.equal(
    ruleSentence({ ...send({ category: "consulta_pago" }), any_sender_ack_at: 1 }),
    "Enviar sin revisión la respuesta redactada por la IA a cualquier remitente cuando se clasifique como Consulta de pago",
  );
});

test("una regla antigua sin remitente ni marca no actúa y lo dice", () => {
  const old = send({ category: "consulta_pago" });
  assert.equal(actsOnNothing(old), true);
  assert.match(ruleSentence(old), /sin remitente indicado/);
  assert.match(ruleRisk(old), /^No actúa: falta el remitente/);
  assert.doesNotMatch(ruleRisk(old), /cualquier remitente/);
  assert.equal(ruleStatus({ ...old, active: true }).primary.text, NOT_ACTING);
  assert.equal(ruleStatus({ ...old, active: true }).primary.tone, "warn");
  assert.equal(ruleStatus({ ...old, active: true }).extra.text, "Activa");
  assert.equal(ruleStatus(old).extra.text, "Inactiva");
  assert.match(conditionsSummary(old), /sin remitente indicado/);
  // Con remitente, dominio o marca, sí actúa.
  for (const rule of [send({ category: "otros", sender: "a@b.test" }), send({ category: "otros", sender_domain: "b.test" }), { ...old, any_sender_ack_at: 5 }]) {
    assert.equal(actsOnNothing(rule), false);
  }
  assert.equal(actsOnNothing(invoice({ supplier: "Alfa SL" })), false);
});

test("la opción avanzada avisa fuerte y una archivada se marca", () => {
  const advanced = { ...send({ category: "documentacion" }), any_sender_ack_at: 9 };
  assert.match(ruleRisk(advanced), /^Contesta a cualquier remitente/);
  assert.match(ruleRisk(advanced), /también un desconocido/);
  assert.match(conditionsSummary(advanced), /cualquier remitente \(opción avanzada\)/);
  assert.equal(ruleStatus({ ...advanced, archived_at: 3 }).primary.text, "Archivada");
});

test("el aviso de una regla con dirección o dominio recuerda la suplantación", () => {
  assert.match(ruleRisk(send({ category: "otros", sender: "compras@cliente.test" })), /compras@cliente\.test/);
  assert.match(ruleRisk(send({ category: "otros", sender: "compras@cliente.test" })), /se puede suplantar/);
  assert.match(ruleRisk(send({ category: "otros", sender_domain: "cliente.test" })), /@cliente\.test/);
});

test("un dominio de correo público se avisa con claridad", () => {
  assert.equal(isPublicMailDomain("Gmail.com"), true);
  assert.equal(isPublicMailDomain("@gmail.com"), true);
  assert.equal(isPublicMailDomain("cliente.test"), false);
  assert.match(publicDomainWarning("gmail.com"), /gmail\.com abarca a cualquier usuario de Gmail/);
  assert.match(publicDomainWarning("outlook.com"), /abarca a cualquier usuario de outlook\.com/);
  assert.equal(publicDomainWarning("cliente.test"), null);
  assert.match(ruleRisk(send({ category: "otros", sender_domain: "gmail.com" })), /cualquier usuario de Gmail/);
});

test("la lista de dominios públicos coincide con la del backend", () => {
  const rust = readFileSync(new URL("../../crates/app/src/rules.rs", import.meta.url), "utf8");
  const fn = rust.slice(rust.indexOf("pub fn is_public_mail_domain"));
  const block = fn.slice(fn.indexOf("const PUBLIC"), fn.indexOf("];"));
  const backend = [...block.matchAll(/"([a-z0-9.-]+)"/g)].map((m) => m[1]);
  assert.ok(backend.length > 10, "no se pudo leer la lista del backend");
  assert.deepEqual([...PUBLIC_MAIL_DOMAINS].sort(), [...backend].sort());
});

test("no inventa datos que faltan", () => {
  assert.match(ruleSentence(invoice({})), /la regla no indica proveedor/);
  assert.match(ruleSentence(send({})), /la regla no indica categoría/);
  assert.equal(ruleSentence(send({ category: "nueva_categoria", sender: "a@b.test" })).endsWith("como nueva_categoria"), true);
});

test("tipo desconocido", () => {
  const unknown = { ...invoice({}), action_kind: "issue_recurring" };
  assert.equal(ruleSentence(unknown), "Tipo de regla no reconocido");
  assert.match(ruleRisk(unknown), /no la aplicará/);
  assert.equal(conditionsSummary(unknown), "Sin datos comparables");
  assert.equal(ruleProblem(unknown), null);
});

test("avisos de riesgo", () => {
  assert.match(ruleRisk(invoice({ supplier: "Alfa SL" })), /^Registrará la factura sin preguntarle si coincide y no tiene incidencias/);
  assert.match(ruleRisk(invoice({ supplier: "Alfa SL" })), /solo se registran facturas en euros/);
});

test("problemas que impiden que la regla actúe", () => {
  assert.equal(ruleProblem(invoice({ supplier: "Alfa SL", max_total: "500.00" })), null);
  assert.match(ruleProblem(invoice({ supplier: "Alfa SL", max_total: 500 })) ?? "", /no es un importe válido/);
  assert.match(ruleProblem(invoice({ supplier: "Alfa SL", color: "rojo" })) ?? "", /«color»/);
  // La moneda (D127) y el remitente (D126) son condiciones conocidas.
  assert.equal(ruleProblem(invoice({ supplier: "Alfa SL", max_total: "500.00", currency: "EUR" })), null);
  assert.equal(ruleProblem(send({ category: "consulta_pago", sender: "pedidos@alfa.test" })), null);
  assert.equal(ruleProblem(send({ category: "consulta_pago", sender_domain: "alfa.test" })), null);
  assert.match(ruleProblem(send({ category: "consulta_pago", sender: "a@alfa.test", sender_domain: "alfa.test" })) ?? "", /a la vez/);
  assert.match(ruleProblem(invoice({ supplier: "Alfa SL", currency: "USD" })) ?? "", /solo registra facturas en euros/);
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

test("el buscador encuentra por nombre y remitente", () => {
  const rule = { ...send({ category: "consulta_pago", sender: "Compras@Cliente.test" }), name: "Consultas del cliente" };
  assert.ok(searchText(rule).includes("consultas del cliente"));
  assert.ok(searchText(rule).includes("compras@cliente.test"));
});

test("el buscador ignora mayúsculas y acentos", () => {
  const text = searchText(send({ category: "documentacion" }));
  assert.ok(text.includes("documentacion"));
  assert.ok(searchText(invoice({ supplier: "Suministros Álvarez SL" })).includes("alvarez"));
});

// ---- Borrador del formulario ----

const ruleWith = (action_kind, conditions, extra = {}) => ({ id: "r-1", version: 3, action_kind, conditions, active: false, ...extra });

test("draftOf y draftConditions conservan remitente, dominio y moneda al editar y duplicar", () => {
  const withSender = ruleWith("send_draft", { category: "consulta_pago", sender: "compras@cliente.test" }, { name: "Pagos del cliente" });
  const d1 = draftOf(withSender);
  assert.equal(d1.scope, "sender");
  assert.equal(d1.sender, "compras@cliente.test");
  assert.equal(d1.name, "Pagos del cliente");
  assert.deepEqual(draftConditions(d1), { category: "consulta_pago", sender: "compras@cliente.test" });
  // Duplicar: mismas condiciones, sin identificador.
  assert.deepEqual(draftConditions({ ...d1, id: "" }), withSender.conditions);

  const withDomain = ruleWith("send_draft", { category: "otros", sender_domain: "cliente.test" });
  const d2 = draftOf(withDomain);
  assert.equal(d2.scope, "domain");
  assert.deepEqual(draftConditions(d2), { category: "otros", sender_domain: "cliente.test" });

  const register = ruleWith("register_invoice", { supplier: "Alfa SL", max_total: "500.00", currency: "EUR" });
  const d3 = draftOf(register);
  assert.equal(d3.currency, "EUR");
  assert.deepEqual(draftConditions(d3), register.conditions);
  // Sin moneda guardada no se inventa una.
  assert.deepEqual(draftConditions(draftOf(ruleWith("register_invoice", { supplier: "Alfa SL" }))), { supplier: "Alfa SL" });
});

test("el nombre solo se copia al borrador si es distinto del identificador", () => {
  assert.equal(draftOf(ruleWith("register_invoice", { supplier: "Alfa SL" }, { name: "r-1" })).name, "");
  assert.equal(draftOf(null).scope, "sender");
  assert.equal(draftOf(null).kind, "register_invoice");
});

test("una regla con opción avanzada se edita como «cualquier remitente» sin confirmación previa", () => {
  const advanced = ruleWith("send_draft", { category: "documentacion" }, { any_sender_ack_at: 7 });
  const d = draftOf(advanced);
  assert.equal(d.scope, "any");
  assert.equal(d.ack, false);
  assert.deepEqual(draftConditions(d), { category: "documentacion" });
  assert.match(anySenderEditNotice(advanced, d), /borra esa validación/);
  assert.match(anySenderEditNotice(advanced, { ...d, scope: "sender" }), /ya no la necesita/);
  assert.equal(anySenderEditNotice(ruleWith("send_draft", { category: "otros", sender: "a@b.test" }), d), null);
  assert.equal(anySenderEditNotice(null, d), null);
  // Una regla activa que pasa a «cualquier remitente» empieza a contestar al guardar.
  const active = ruleWith("send_draft", { category: "documentacion" }, { active: true, any_sender_ack_at: 7 });
  assert.match(anySenderEditNotice(active, draftOf(active)), /está activa: empezará a contestar a cualquier remitente/);
  const old = ruleWith("send_draft", { category: "documentacion" }, { active: true });
  assert.match(anySenderEditNotice(old, { ...draftOf(old), scope: "any" }), /empezará a contestar/);
  assert.equal(anySenderEditNotice(old, draftOf(old)), null);
  assert.equal(anySenderEditNotice(ruleWith("send_draft", { category: "x" }, { active: false }), { ...d, scope: "any" }), null);
});

test("la confirmación de la opción avanzada exige casilla y texto exacto", () => {
  assert.equal(ANY_SENDER_CONFIRMATION, "CUALQUIER REMITENTE");
  assert.equal(anyAckReady({ ack: false, ack_text: "CUALQUIER REMITENTE" }), false);
  assert.equal(anyAckReady({ ack: true, ack_text: "cualquier remitente" }), false);
  assert.equal(anyAckReady({ ack: true, ack_text: "CUALQUIER" }), false);
  assert.equal(anyAckReady({ ack: true, ack_text: "  CUALQUIER REMITENTE " }), true);
  assert.match(
    anySenderConsequences("consulta_pago"),
    /Cualquiera, también un desconocido, que escriba un correo que la IA clasifique como Consulta de pago recibirá sin revisión la respuesta redactada por la IA/,
  );
  assert.match(anySenderConsequences("consulta_pago"), /lo enviado no se puede deshacer/);
});

test("el borrador dice qué falta", () => {
  const base = draftOf(null);
  assert.deepEqual(draftProblems({ ...base, name: "Pagos", supplier: "Alfa SL" }, true), []);
  assert.deepEqual(draftProblems({ ...base, name: " ", supplier: "Alfa SL" }, true), ["Escriba un nombre para la regla."]);
  assert.deepEqual(draftProblems({ ...base, name: "", supplier: "Alfa SL" }, false), []);
  assert.match(draftProblems({ ...base, name: "x", supplier: "" }, true)[0], /proveedor/);
  assert.match(draftProblems({ ...base, name: "x", supplier: "Alfa SL", max_total: "mucho" }, true)[0], /importe/);
  const mail = { ...base, name: "x", kind: "send_draft" };
  assert.match(draftProblems({ ...mail, scope: "sender", sender: "sin-arroba" }, true)[0], /dirección completa/);
  assert.deepEqual(draftProblems({ ...mail, scope: "sender", sender: "compras@cliente.test" }, true), []);
  assert.match(draftProblems({ ...mail, scope: "domain", sender_domain: "*.cliente.test" }, true)[0], /dominio/);
  assert.deepEqual(draftProblems({ ...mail, scope: "domain", sender_domain: "@cliente.test" }, true), []);
  assert.match(draftProblems({ ...mail, scope: "any" }, true)[0], /CUALQUIER REMITENTE/);
  assert.deepEqual(draftProblems({ ...mail, scope: "any", ack: true, ack_text: "CUALQUIER REMITENTE" }, true), []);
  assert.deepEqual(draftProblems({ ...mail, scope: "any" }, true, { ignoreAck: true }), []);
});

// ---- Contadores ----

test("contadores de una regla", () => {
  // El backend entrega milisegundos desde 1970 (`now_ms`).
  const day = new Date(2026, 10, 3, 12).getTime();
  assert.ok(day > 1_000_000_000_000, "un valor realista en milisegundos");
  assert.equal(formatDate(day), "03/11/2026");
  assert.equal(formatDate(1_794_149_612_941), formatDate(new Date(1_794_149_612_941).getTime()));
  assert.match(formatDate(1_794_149_612_941), /^\d{2}\/\d{2}\/2026$/, "no cae en 1970 ni en un año lejano");
  assert.deepEqual(statsText({ applied: 3, last_applied_at: day, reviewed: 1, unclassified: 0 }), {
    main: "Aplicada 3 veces · última el 03/11/2026 · pasó a revisión 1 vez",
    unclassified: null,
  });
  assert.equal(
    statsText({ applied: 1, last_applied_at: day, reviewed: 0, unclassified: 0 }).main,
    "Aplicada 1 vez · última el 03/11/2026 · pasó a revisión 0 veces",
  );
  assert.deepEqual(statsText({ applied: 0, last_applied_at: null, reviewed: 0, unclassified: 0 }), { main: "Aún no se ha aplicado", unclassified: null });
  assert.equal(statsText(undefined).main, "Aún no se ha aplicado");
  assert.equal(statsText({ applied: 0, last_applied_at: null, reviewed: 2, unclassified: 0 }).main, "Aún no se ha aplicado · pasó a revisión 2 veces");
  assert.equal(statsText({ applied: 2, last_applied_at: null, reviewed: 0, unclassified: 2 }).unclassified, "2 sin clasificar (versión anterior)");
  assert.equal(statsText({ applied: 0, last_applied_at: null, reviewed: 0, unclassified: 1 }).main, "Aún no se ha aplicado");
});

// ---- Motivos ----

test("todos los códigos de motivo tienen texto y ninguno se muestra como código", () => {
  for (const code of REASON_CODES) {
    const text = reasonText(code);
    assert.ok(text.length > 15, code);
    // «rodaje» es además una palabra del texto («en rodaje»): para ese código solo se comprueba que no salga con guion bajo.
    if (code !== "rodaje") assert.ok(!text.includes(code), `${code} se muestra en crudo: ${text}`);
    assert.ok(!text.includes("_"), `${code}: ${text}`);
  }
});

test("los códigos del frontend son los del backend", () => {
  const rust = readFileSync(new URL("../../crates/app/src/rules.rs", import.meta.url), "utf8");
  const start = rust.indexOf("pub enum ReasonCode");
  const body = rust.slice(start, rust.indexOf("\n}", start));
  const variants = [...body.matchAll(/^ {4}([A-Z][A-Za-z]+),$/gm)].map((m) => m[1].replace(/([a-z])([A-Z])/g, "$1_$2").toLowerCase());
  assert.ok(variants.length >= 12, "no se pudo leer el enum del backend");
  assert.deepEqual([...REASON_CODES].sort(), variants.sort());
});

test("el motivo del rodaje dice lo mismo que el backend y no lleva contenido del correo", () => {
  const rust = readFileSync(new URL("../../crates/app/src/rules.rs", import.meta.url), "utf8").replace(/\r\n/g, "\n");
  const backend = /RODAJE_REASON: &str =\s*"([^"]+)"/.exec(rust)?.[1];
  assert.equal(backend, "En rodaje: esta regla aprendida enviará sola a partir del envío n.º 4");
  assert.equal(capitalize(reasonText("rodaje")), backend);
  // Con o sin hechos, el texto es el mismo: los hechos (regla, versión, envíos que faltan) no se mezclan.
  assert.equal(reasonText("rodaje", { rule_id: "regla-ana", rule_version: 2, left: 2, of: 3 }), reasonText("rodaje"));
  assert.ok(!reasonText("rodaje", { rule_id: "regla-ana" }).includes("regla-ana"));
});

test("los motivos usan los datos de la evaluación y no inventan", () => {
  assert.equal(reasonText("several_rules_match", { matching: 2 }), "2 reglas activas coinciden y la decisión no es inequívoca");
  assert.match(reasonText("foreign_currency", { currencies: ["USD", "GBP"] }), /menciona USD, GBP/);
  assert.match(reasonText("sender_required", { rule_ids: ["a", "b"] }), /\(regla: a, b\)/);
  assert.equal(reasonText("amount_review", { message: "Revise el total" }), "Revise el total");
  assert.match(reasonText("codigo_futuro"), /motivo no reconocido \(codigo_futuro\)/);
});

test("por qué está aquí: reglas comprobadas con su motivo concreto", () => {
  const info = explainEvaluation({
    code: "several_rules_match",
    facts: { matching: 2 },
    candidates: [
      { id: "b", version: 1, active: true, matches: false, mismatch: "el proveedor es «Alfa SL» y la regla pide «Beta SL»" },
      { id: "a", version: 2, active: true, matches: true },
      { id: "c", version: 1, active: true, matches: true },
      { id: "d", version: 1, active: false, matches: true },
      { id: "e", version: 1, active: true, matches: false, mismatch: "la regla no indica remitente; añádalo o valide la opción avanzada", sender_required: true },
    ],
  });
  assert.equal(info.headline, "2 reglas activas coinciden y la decisión no es inequívoca.");
  assert.deepEqual(
    info.candidates.map((c) => [c.id, c.outcome]),
    [
      ["a", "matched"],
      ["c", "matched"],
      ["d", "would_apply"],
      ["e", "needs_sender"],
      ["b", "no_match"],
    ],
  );
  assert.match(info.candidates[0].text, /varias reglas activas/);
  assert.equal(info.candidates[2].text, "Habría aplicado, pero está inactiva.");
  assert.equal(info.candidates[3].text, "La regla no indica remitente; añádalo o valide la opción avanzada.");
  assert.equal(info.candidates[4].text, "No coincidía: el proveedor es «Alfa SL» y la regla pide «Beta SL».");
});

test("por qué está aquí: una tarea anterior al registro lo dice", () => {
  for (const value of [undefined, null, {}, "texto", { code: 3, candidates: [] }]) assert.equal(explainEvaluation(value), null);
  assert.match(NO_EVALUATION, /anterior a este registro/);
  assert.deepEqual(explainEvaluation({ code: "no_active_rule_matches", candidates: [] }).candidates, []);
});

// ---- Simulación ----

const sim = (extra = {}) => ({
  considered: 6,
  truncated: false,
  limit: 200,
  skipped_without_result: 0,
  without_currency_check: 0,
  would_automate: 2,
  would_review: {},
  overlaps: 0,
  no_match: 4,
  would_block_other: [],
  items: [],
  ...extra,
});

test("la simulación se resume en lenguaje normal", () => {
  const s = simulationSummary(
    sim({
      would_review: { invoice_issues: 1, currency_not_checked: 2 },
      overlaps: 1,
      no_match: 0,
      without_currency_check: 2,
      truncated: true,
      limit: 6,
      skipped_without_result: 1,
      would_block_other: [{ rule_id: "otra", task_ids: [4, 5] }],
      items: [
        { task_id: 1, label: "factura-a.pdf", outcome: "automate", reason_code: "automated", explanation: "", overlapping_rules: [] },
        { task_id: 2, label: "factura-b.pdf", outcome: "review", reason_code: "currency_not_checked", explanation: "", overlapping_rules: [] },
        { task_id: 3, label: null, outcome: "overlap", reason_code: "several_rules_match", explanation: "", overlapping_rules: ["otra"] },
      ],
    }),
    "register_invoice",
  );
  assert.equal(s.empty, false);
  assert.equal(s.headline, "De las últimas 6 facturas con resultado, la regla automatizaría 2 y mandaría 4 a revisión; 0 no coinciden con la regla.");
  assert.deepEqual(
    s.groups.map((g) => [g.code, g.count]),
    [
      ["currency_not_checked", 2],
      ["invoice_issues", 1],
      ["several_rules_match", 1],
    ],
  );
  assert.deepEqual(s.groups[0].examples, [{ task_id: 2, label: "factura-b.pdf" }]);
  assert.deepEqual(s.groups[2].examples, [{ task_id: 3, label: "(sin nombre)" }]);
  assert.deepEqual(s.blocked, [{ rule_id: "otra", count: 2 }]);
  assert.equal(s.notes.length, 3);
  assert.match(s.notes[0], /2 facturas no se pudieron comprobar en cuanto a la moneda/);
  assert.match(s.notes[1], /últimas 6 tareas/);
  assert.match(s.notes[2], /1 tarea no tiene resultado/);
  assert.deepEqual(s.automateExamples, [{ task_id: 1, label: "factura-a.pdf" }]);
});

test("la simulación sin tareas lo dice", () => {
  const s = simulationSummary(sim({ considered: 0, would_automate: 0, no_match: 0 }), "send_draft");
  assert.equal(s.empty, true);
  assert.match(s.headline, /No hay tareas con resultado/);
});

// ---- Historial ----

test("el historial se filtra por regla y por aprobación de una persona", () => {
  const entries = [
    { task_id: 1, action: "register_invoice", status: "confirmed", authorization: "regla alfa v2", detail: null },
    { task_id: 2, action: "register_invoice", status: "confirmed", authorization: "aprobación", detail: null },
    { task_id: null, action: "rule", status: "active", authorization: null, detail: "alfa v3 (register_invoice)" },
    { task_id: null, action: "rule_any_sender", status: "acknowledged", authorization: null, detail: "alfa v3: validada" },
    { task_id: null, action: "rule_rename", status: "renamed", authorization: null, detail: "alfa: Facturas de Alfa" },
    { task_id: 3, action: "send_draft", status: "confirmed", authorization: "regla alfa-2 v1", detail: null },
    { task_id: null, action: "rule", status: "active", authorization: null, detail: "alfa-2 v1 (send_draft)" },
  ];
  assert.equal(entries.filter((e) => auditMentionsRule(e, "alfa")).length, 4);
  assert.equal(entries.filter((e) => auditMentionsRule(e, "alfa-2")).length, 2);
  assert.equal(entries.filter((e) => auditMatches(e, "")).length, 7);
  assert.deepEqual(entries.filter((e) => auditMatches(e, APPROVED_BY_PERSON)).map((e) => e.task_id), [2]);
  assert.deepEqual(entries.filter((e) => auditMatches(e, AUTHORIZED_BY_RULE)).map((e) => e.task_id), [1, 3]);
  assert.equal(auditMentionsRule({ ...entries[1], detail: "alfa v1" }, "alfa"), false, "solo cuentan los cambios de reglas");
});

// ---- Importes ----

test("el total máximo se normaliza a dos decimales antes de enviarlo", () => {
  const ok = (raw, value) => assert.deepEqual(normalizeAmount(raw), { ok: true, value }, raw);
  ok("500", "500.00");
  ok(" 500 ", "500.00");
  ok("500,5", "500.50");
  ok("500.5", "500.50");
  ok("500,50", "500.50");
  ok("500.00", "500.00");
  ok("1.234,5", "1234.50");
  ok("1,234.50", "1234.50");
  ok("1.234.567,00", "1234567.00");
  ok("500 €", "500.00");
  ok("500.00 EUR", "500.00");
  const draft = { ...draftOf(null), supplier: "Alfa SL" };
  assert.deepEqual(draftConditions({ ...draft, max_total: "500" }), { supplier: "Alfa SL", max_total: "500.00" });
  assert.deepEqual(draftConditions({ ...draft, max_total: "500,5" }), { supplier: "Alfa SL", max_total: "500.50" });
  assert.deepEqual(draftConditions({ ...draft, max_total: "" }), { supplier: "Alfa SL" });
});

test("lo ambiguo o inválido se rechaza con un mensaje claro", () => {
  for (const raw of ["1.234", "1,234", "1.234.567", "500,555", "mucho", "-5", "5 000", "5,", ",5"]) {
    const r = normalizeAmount(raw);
    assert.equal(r.ok, false, raw);
    assert.ok(r.message.length > 20, raw);
  }
  assert.match(normalizeAmount("1.234").message, /ambiguo/);
  const base = { ...draftOf(null), name: "x", supplier: "Alfa SL" };
  assert.match(draftProblems({ ...base, max_total: "1.234" }, true)[0], /ambiguo/);
  assert.deepEqual(draftProblems({ ...base, max_total: "500" }, true), []);
});

test("un máximo guardado que el backend no entiende se muestra como «No actúa»", () => {
  for (const good of ["500.00", "500,00", "1.234,56", "1,234.56", "1 234,56", "500.00 EUR"]) assert.equal(isStoredAmount(good), true, good);
  for (const bad of ["500", "500,5", "1.234", "mucho", "1.234,5", ""]) assert.equal(isStoredAmount(bad), false, bad);
  const broken = { ...invoice({ supplier: "Alfa SL", max_total: "500" }), active: true };
  assert.match(ruleProblem(broken), /«500»\) no es un importe válido: no actuará/);
  assert.equal(ruleStatus(broken).primary.text, "No actúa: la regla tiene un problema");
  assert.equal(ruleStatus(broken).primary.tone, "warn");
  assert.equal(ruleStatus(broken).extra.text, "Activa");
  assert.equal(ruleStatus({ ...invoice({ supplier: "Alfa SL", max_total: "500.00" }), active: true }).primary.text, "Activa");
  // Un tipo desconocido no se marca como problema: su aviso ya lo dice.
  assert.equal(ruleStatus({ ...invoice({}), action_kind: "issue_recurring" }).primary.text, "Inactiva");
});

test("una tarea en revisión cuya evaluación dice «automatizada» explica el intento sin confirmar", () => {
  const evaluation = { code: "automated", facts: {}, candidates: [{ id: "a", version: 1, active: true, matches: true }] };
  const info = explainEvaluation(evaluation, true);
  assert.ok(info.headline.startsWith("Una regla activa coincidía y se intentó la acción"));
  assert.match(info.headline, /no consta que se completara/);
  assert.doesNotMatch(info.headline, /nada que revisar/);
  // Sin el contexto de revisión (simulación), sigue siendo el texto general.
  assert.match(explainEvaluation(evaluation).headline, /nada que revisar/);
  assert.ok(AUTOMATED_NOT_CONFIRMED.length > 40);
});

test("la moneda no mostrada tiene su propio motivo", () => {
  assert.equal(reasonText("currency_not_shown"), "el documento no muestra la moneda; no se puede comprobar que sea en euros");
  assert.match(reasonText("codigo_antiguo_scanned_currency"), /motivo no reconocido/);
});
