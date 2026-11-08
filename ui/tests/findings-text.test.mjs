// R23: cada hallazgo se explica con qué pasa, por qué y qué consecuencia tiene, con datos concretos.
import assert from "node:assert/strict";
import { test } from "node:test";
import {
  MERGE_NEXT_STEP,
  MERGE_NOTICE,
  NO_FINDINGS,
  NO_TASKS_AFFECTED,
  SIMULATED_RULE,
  explainFinding,
  mergeOptionLabel,
  mergeOptionText,
  mergeSimulationText,
  mergedDone,
  originText,
  similarClarification,
  sortFindings,
} from "../src/rules-text.ts";

const names = { "alfa-libre": "Alfa libre", "alfa-500": "Alfa hasta 500", "alfa-sa": "Alfa SA", "alfa-sa2": "Alfa con puntos", dominio: "Dominio Alfa", direccion: "Ana Alfa", sin: "Sin remitente" };
const nameOf = (id) => names[id] ?? id;
const rules = (...list) => list.map(([id, active]) => ({ id, version: 1, active }));

const overlap = (over = {}) => ({
  key: "overlap:alfa-500+alfa-libre",
  kind: "overlap",
  action_kind: "register_invoice",
  rules: rules(["alfa-500", true], ["alfa-libre", true]),
  active_conflict: true,
  effect: {
    code: "narrow_blocks_wide",
    data: { supplier: "Alfa SL", wider: { id: "alfa-libre", limit: null }, narrower: { id: "alfa-500", limit: "500.00" } },
  },
  merge_candidate: true,
  affected_tasks: 0,
  ...over,
});

test("un solape de facturas explica qué pasa, por qué y la consecuencia con los importes", () => {
  const t = explainFinding(overlap(), nameOf);
  assert.equal(t.title, "Se pisan: «Alfa hasta 500» y «Alfa libre»");
  assert.equal(t.badge.text, "Choque activo");
  assert.equal(t.what, "Estas dos reglas se pisan porque ambas cubren las facturas de Alfa SL de hasta 500,00 €.");
  assert.match(t.why, /«Alfa libre» no tiene límite de importe y «Alfa hasta 500» llega hasta 500,00 €/);
  assert.match(t.why, /coinciden con las dos reglas/);
  assert.equal(
    t.consequence,
    "Las facturas de Alfa SL de hasta 500,00 € van a revisión aunque las dos reglas las permitan. Solo las de más de 500,00 € se registran solas.",
  );
  assert.equal(t.timing, null);
});

test("con dos límites, la consecuencia nombra el tramo que sí se registra solo", () => {
  const t = explainFinding(
    overlap({ effect: { code: "narrow_blocks_wide", data: { supplier: "Alfa SL", wider: { id: "alfa-libre", limit: "1000.00" }, narrower: { id: "alfa-500", limit: "500.00" } } } }),
    nameOf,
  );
  assert.match(t.consequence, /Solo las de más de 500,00 € y hasta 1\.000,00 € se registran solas; las de más de 1\.000,00 € siempre se revisan\./);
});

test("el recuento de tareas afectadas es real o dice que aún no hay", () => {
  assert.equal(explainFinding(overlap({ affected_tasks: 0 }), nameOf).tasks, NO_TASKS_AFFECTED);
  assert.equal(NO_TASKS_AFFECTED, "Aún no hay tareas afectadas.");
  assert.equal(
    explainFinding(overlap({ affected_tasks: 3 }), nameOf).tasks,
    "3 facturas ya coincidieron con las dos reglas, según el registro de evaluaciones.",
  );
  assert.match(explainFinding(overlap({ affected_tasks: 1 }), nameOf).tasks, /^1 factura ya coincidió/);
  const mail = explainFinding(
    { ...sender(), affected_tasks: 2 },
    nameOf,
  );
  assert.match(mail.tasks, /^2 correos ya coincidieron/);
});

test("un solape con una regla inactiva dice que todavía no ocurre y cuándo ocurrirá", () => {
  const t = explainFinding(
    overlap({ active_conflict: false, rules: rules(["alfa-500", false], ["alfa-libre", true]) }),
    nameOf,
  );
  assert.equal(t.badge.text, "Solo si se activan");
  assert.equal(t.timing, "Todavía no ocurre: «Alfa hasta 500» está inactiva. Ocurrirá en cuanto la active.");
  const both = explainFinding(overlap({ active_conflict: false, rules: rules(["alfa-500", false], ["alfa-libre", false]) }), nameOf);
  assert.match(both.timing, /las dos reglas están inactivas/);
});

test("reglas iguales: sobra una y ninguna factura se registra sola", () => {
  const t = explainFinding(
    {
      key: "duplicate:alfa-500+alfa-libre",
      kind: "duplicate",
      action_kind: "register_invoice",
      rules: rules(["alfa-500", true], ["alfa-libre", true]),
      active_conflict: true,
      effect: { code: "same_conditions", data: { supplier: "Alfa SL", limit: "500.00" } },
      merge_candidate: true,
      affected_tasks: 2,
    },
    nameOf,
  );
  assert.equal(t.title, "Reglas iguales: «Alfa hasta 500» y «Alfa libre»");
  assert.equal(t.what, "Estas dos reglas son iguales: las dos cubren las facturas de Alfa SL de hasta 500,00 €.");
  assert.match(t.consequence, /Ninguna factura de Alfa SL de hasta 500,00 € se registra sola: todas van a revisión\. Sobra una de las dos\./);
});

function sender(over = {}) {
  return {
    key: "overlap:dominio+direccion",
    kind: "overlap",
    action_kind: "send_draft",
    rules: rules(["dominio", true], ["direccion", true]),
    active_conflict: true,
    effect: {
      code: "narrow_blocks_wide",
      data: {
        category: "solicitud_factura",
        wider: { id: "dominio", scope: "domain", value: "alfa.test" },
        narrower: { id: "direccion", scope: "sender", value: "ana@alfa.test" },
      },
    },
    merge_candidate: true,
    affected_tasks: 0,
    ...over,
  };
}

test("un solape de envío nombra la dirección, el dominio y la categoría", () => {
  const t = explainFinding(sender(), nameOf);
  assert.equal(
    t.what,
    "Estas dos reglas se pisan porque ambas contestan a los correos de ana@alfa.test clasificados como «Solicitud de factura».",
  );
  assert.match(t.why, /«Dominio Alfa» contesta a cualquier dirección de @alfa\.test y «Ana Alfa» solo a ana@alfa\.test, que ya está dentro/);
  assert.equal(
    t.consequence,
    "Los correos de ana@alfa.test van a revisión aunque las dos reglas los permitan. Solo se contestan solos los de otras direcciones de @alfa.test.",
  );
  const any = explainFinding(
    sender({
      effect: {
        code: "narrow_blocks_wide",
        data: { category: "consulta_pago", wider: { id: "dominio", scope: "any", value: null }, narrower: { id: "direccion", scope: "domain", value: "alfa.test" } },
      },
    }),
    nameOf,
  );
  assert.match(any.what, /correos de cualquier dirección de @alfa\.test clasificados como «Consulta de pago»/);
  assert.match(any.why, /contesta a cualquier remitente/);
  assert.match(any.consequence, /Solo se contestan solos los de los demás remitentes\./);
});

test("un nombre de proveedor parecido es una pregunta, no una contradicción", () => {
  const t = explainFinding(
    {
      key: "similar:alfa-sa+alfa-sa2",
      kind: "similar",
      action_kind: "register_invoice",
      rules: rules(["alfa-sa", true], ["alfa-sa2", true]),
      active_conflict: false,
      effect: { code: "similar_supplier", data: { first: { id: "alfa-sa", supplier: "Alfa SA" }, second: { id: "alfa-sa2", supplier: "ALFA, S.A." } } },
      merge_candidate: false,
      affected_tasks: null,
    },
    nameOf,
  );
  assert.equal(t.title, "¿«Alfa SA» y «ALFA, S.A.» son el mismo proveedor?");
  assert.equal(t.badge.text, "Pregunta");
  assert.match(t.what, /«Alfa SA» \(proveedor «Alfa SA»\) y «Alfa con puntos» \(proveedor «ALFA, S\.A\.»\)/);
  assert.match(t.why, /no puede asegurar que sean el mismo/);
  assert.match(t.consequence, /No chocan ahora/);
  assert.equal(t.tasks, null);
  assert.match(similarClarification(), /la aplicación no lo decide por usted/);
  assert.match(similarClarification(), /edite una de las dos reglas/);
});

test("una regla sin remitente se muestra como pendiente de corregir, no como contradicción", () => {
  const t = explainFinding(
    {
      key: "defective:sin",
      kind: "defective",
      action_kind: "send_draft",
      rules: rules(["sin", true]),
      active_conflict: false,
      effect: { code: "sender_missing", data: { reason: "no indica remitente" } },
      merge_candidate: false,
      affected_tasks: null,
    },
    nameOf,
  );
  assert.equal(t.badge.text, "Pendiente de corregir");
  assert.equal(t.title, "«Sin remitente» no tiene remitente");
  assert.match(t.consequence, /No es una contradicción, es una regla pendiente de corregir\./);
});

test("condiciones inválidas y tipos desconocidos citan el motivo", () => {
  const base = { kind: "defective", action_kind: "register_invoice", rules: rules(["alfa-libre", true]), active_conflict: false, merge_candidate: false, affected_tasks: null };
  const bad = explainFinding({ ...base, key: "defective:alfa-libre", effect: { code: "invalid_conditions", data: { reason: "su máximo «mucho» no es un importe válido" } } }, nameOf);
  assert.equal(bad.what, "Hay un problema en «Alfa libre»: su máximo «mucho» no es un importe válido.");
  assert.match(bad.consequence, /No actúa y no se compara con las demás/);
  const unknown = explainFinding({ ...base, key: "defective:alfa-libre", effect: { code: "not_comparable", data: { reason: "la aplicación no reconoce el tipo de regla «emitir_recurrente»" } } }, nameOf);
  assert.equal(unknown.badge.text, "No comparable");
  assert.match(unknown.what, /emitir_recurrente/);
});

test("la regla hipotética se llama «esta regla» en el aviso previo a activar", () => {
  const t = explainFinding(
    overlap({
      key: "overlap:(simulada)+alfa-libre",
      rules: rules([SIMULATED_RULE, true], ["alfa-libre", true]),
      effect: { code: "narrow_blocks_wide", data: { supplier: "Alfa SL", wider: { id: "alfa-libre", limit: null }, narrower: { id: SIMULATED_RULE, limit: "500.00" } } },
    }),
    nameOf,
  );
  assert.equal(t.title, "Se pisan: esta regla y «Alfa libre»");
  assert.match(t.why, /«Alfa libre» no tiene límite de importe y esta regla llega hasta 500,00 €/);
  assert.doesNotMatch(JSON.stringify(t), /simulada/);
});

test("los hallazgos se ordenan por gravedad: choque activo primero", () => {
  const f = (key, kind, active_conflict) => ({ key, kind, active_conflict, action_kind: "register_invoice", rules: [], effect: { code: "x", data: {} }, merge_candidate: false, affected_tasks: null });
  const sorted = sortFindings([
    f("defective:a", "defective", false),
    f("similar:a+b", "similar", false),
    f("overlap:c+d", "overlap", false),
    f("overlap:a+b", "overlap", true),
    f("duplicate:a+b", "duplicate", true),
  ]);
  assert.deepEqual(
    sorted.map((x) => x.key),
    ["duplicate:a+b", "overlap:a+b", "overlap:c+d", "similar:a+b", "defective:a"],
  );
});

test("cada opción de unificación explica qué significa elegirla", () => {
  const free = { key: "none", scope: { limit: null }, rule_ids: ["alfa-libre"] };
  const capped = { key: "500.00", scope: { limit: "500.00" }, rule_ids: ["alfa-500"] };
  assert.equal(mergeOptionLabel("register_invoice", free), "Sin límite");
  assert.equal(mergeOptionLabel("register_invoice", capped), "Hasta 500,00 €");
  assert.match(mergeOptionText("register_invoice", free, "Alfa SL", nameOf), /cualquier factura limpia de Alfa SL, sea del importe que sea\. La tienen «Alfa libre»\./);
  assert.match(mergeOptionText("register_invoice", capped, "Alfa SL", nameOf), /de hasta 500,00 €; las de más importe irán a revisión\. La tienen «Alfa hasta 500»\./);
  const any = { key: "any", scope: { scope: "any", value: null }, rule_ids: ["sin"] };
  assert.match(mergeOptionText("send_draft", any, "Solicitud de factura", nameOf), /no hereda la opción avanzada/);
  assert.match(mergeOptionText("send_draft", { key: "sender:ana@alfa.test", scope: { scope: "sender", value: "ana@alfa.test" }, rule_ids: ["direccion"] }, "x", nameOf), /Solo a ana@alfa\.test/);
  assert.equal(mergeOptionLabel("send_draft", { key: "domain:alfa.test", scope: { scope: "domain", value: "alfa.test" }, rule_ids: [] }), "Todo @alfa.test");
});

const plan = (over = {}) => ({
  action_kind: "register_invoice",
  before: [],
  options: [],
  chosen: "500.00",
  after: { conditions: { supplier: "Alfa SL", max_total: "500.00" }, active: false, option_key: "500.00", loses_any_sender: [], needs_sender_validation: false },
  simulation: { considered: 8, truncated: false, skipped_without_result: 0, touched: 7, start_automating: new Array(5).fill({}), stop_automating: new Array(2).fill({}), unchanged_automated: 0, unchanged_review: 0 },
  fingerprint: "abc",
  ...over,
});

test("la simulación de unificar dice cuántas dejan de revisarse y cuántas dejan de automatizarse", () => {
  const t = mergeSimulationText(plan(), nameOf);
  assert.equal(t.empty, false);
  assert.equal(t.lines[0], "5 facturas de Alfa SL de hasta 500,00 € dejarían de revisarse: se registrarían solas.");
  assert.equal(
    t.lines[1],
    "2 facturas de Alfa SL de más de 500,00 € dejarían de automatizarse: hoy se registran solas y con la regla nueva irían a revisión.",
  );
  // Sin límite elegido no se inventan tramos de importe.
  const free = mergeSimulationText(
    plan({ after: { conditions: { supplier: "Alfa SL" }, active: false, option_key: "none", loses_any_sender: [], needs_sender_validation: false }, simulation: { ...plan().simulation, stop_automating: [], unchanged_automated: 2 } }),
    nameOf,
  );
  assert.equal(free.lines[0], "5 facturas de Alfa SL dejarían de revisarse: se registrarían solas.");
  assert.equal(free.lines[1], "2 facturas seguirían registrándose solas.");
});

test("sin tareas cubiertas la ausencia es explícita", () => {
  const none = mergeSimulationText(plan({ simulation: { ...plan().simulation, considered: 0, touched: 0, start_automating: [], stop_automating: [] } }), nameOf);
  assert.equal(none.empty, true);
  assert.match(none.lines[0], /No hay tareas guardadas con resultado/);
  const uncovered = mergeSimulationText(plan({ simulation: { ...plan().simulation, considered: 4, touched: 0, start_automating: [], stop_automating: [] } }), nameOf);
  assert.match(uncovered.lines[0], /Ninguna de las 4 facturas guardadas está cubierta/);
});

test("la unificación avisa de la opción avanzada que no hereda y de la regla sin remitente", () => {
  const t = mergeSimulationText(
    plan({
      action_kind: "send_draft",
      after: { conditions: { category: "solicitud_factura" }, active: false, option_key: "any", loses_any_sender: ["sin"], needs_sender_validation: true },
      simulation: { ...plan().simulation, start_automating: [], stop_automating: [{}], unchanged_review: 1, truncated: true, skipped_without_result: 2 },
    }),
    nameOf,
  );
  assert.match(t.notes[0], /«Sin remitente» tenía la opción avanzada «cualquier remitente»: la regla nueva no la hereda\./);
  assert.match(t.notes[1], /no contestará a nadie hasta que valide la opción avanzada/);
  assert.match(t.notes.join(" "), /hay más antiguas/);
  assert.match(t.notes.join(" "), /2 tareas no tienen resultado/);
  assert.match(t.lines[0], /1 correo de cualquier remitente dejaría de automatizarse: hoy se contesta solo/);
});

test("avisos fijos de la unificación y origen de la regla nueva", () => {
  assert.equal(MERGE_NOTICE, "La regla nueva quedará inactiva y las anteriores se archivarán; puede restaurarlas.");
  assert.equal(MERGE_NEXT_STEP, "Pruébela y actívela cuando quiera");
  assert.equal(NO_FINDINGS, "No hay reglas que se contradigan");
  assert.match(mergedDone("Alfa hasta 500"), /^Reglas unificadas en «Alfa hasta 500»\..*inactiva.*archivadas.*Pruébela y actívela cuando quiera\.$/);
  assert.equal(
    originText({ kind: "merge", sources: [{ id: "alfa-500", version: 2 }, { id: "alfa-libre", version: 3 }] }, nameOf),
    "Creada al unificar «Alfa hasta 500» (versión 2) y «Alfa libre» (versión 3).",
  );
  assert.equal(originText(null, nameOf), null);
  assert.equal(originText({ kind: "otro" }, nameOf), null);
});
