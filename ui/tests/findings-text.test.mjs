// R23: cada hallazgo se explica con qué pasa, por qué y qué consecuencia tiene, con datos concretos.
import assert from "node:assert/strict";
import { test } from "node:test";
import {
  MERGE_NEXT_STEP,
  MERGE_NOTICE,
  NO_FINDINGS,
  NO_TASKS_AFFECTED,
  SIMULATED_RULE,
  activationGroups,
  auditStatusLabel,
  explainFinding,
  mergeOptionLabel,
  mergeOptionText,
  mergeSimulationText,
  mergedDone,
  originText,
  restoreHeading,
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
    "Las facturas de Alfa SL de hasta 500,00 € coinciden con las dos reglas y, con dos reglas activas coincidentes, van a revisión aunque las dos las permitan. Las de más de 500,00 € las cubre «Alfa libre», no «Alfa hasta 500».",
  );
  assert.doesNotMatch(t.consequence, /siempre|Solo/);
  assert.equal(t.timing, null);
});

test("con dos límites, la consecuencia nombra qué tramo cubre cada regla", () => {
  const t = explainFinding(
    overlap({ effect: { code: "narrow_blocks_wide", data: { supplier: "Alfa SL", wider: { id: "alfa-libre", limit: "1000.00" }, narrower: { id: "alfa-500", limit: "500.00" } } } }),
    nameOf,
  );
  assert.match(t.consequence, /Las de más de 500,00 € y hasta 1\.000,00 € las cubre «Alfa libre», no «Alfa hasta 500»; las de más de 1\.000,00 € no las cubre ninguna de las dos\./);
  assert.doesNotMatch(t.consequence, /siempre|se registran solas/);
});

test("el recuento de tareas afectadas es real o dice que aún no hay", () => {
  assert.equal(explainFinding(overlap({ affected_tasks: 0 }), nameOf).tasks, NO_TASKS_AFFECTED);
  assert.equal(NO_TASKS_AFFECTED, "Aún no hay tareas afectadas.");
  assert.equal(
    explainFinding(overlap({ affected_tasks: 3 }), nameOf).tasks,
    "3 facturas ya coincidieron con las dos reglas, según el registro de evaluaciones y con las condiciones que tienen hoy (no cuentan las evaluadas con condiciones ya editadas).",
  );
  // Sin recuento no se afirma nada, ni siquiera que no haya tareas.
  assert.equal(explainFinding(overlap({ affected_tasks: null }), nameOf).tasks, null);
  assert.match(explainFinding(overlap({ affected_tasks: 1 }), nameOf).tasks, /^1 factura ya coincidió/);
  const mail = explainFinding(
    { ...sender(), affected_tasks: 2 },
    nameOf,
  );
  assert.match(mail.tasks, /^2 correos ya coincidieron/);
});

test("un solape con una regla inactiva dice que ahora no ocurre y cuándo ocurrirá", () => {
  const t = explainFinding(
    overlap({ active_conflict: false, rules: rules(["alfa-500", false], ["alfa-libre", true]) }),
    nameOf,
  );
  assert.equal(t.badge.text, "Solo si se activan");
  assert.equal(t.timing, "Ahora no ocurre: «Alfa hasta 500» está inactiva. Ocurrirá en cuanto la active.");
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
    "Los correos de ana@alfa.test coinciden con las dos reglas y, con dos reglas activas coincidentes, van a revisión aunque las dos los permitan. Los de otras direcciones de @alfa.test los cubre «Dominio Alfa», no «Ana Alfa».",
  );
  assert.doesNotMatch(t.consequence, /Solo|siempre/);
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
  assert.match(any.consequence, /Los de los demás remitentes los cubre «Dominio Alfa», no «Ana Alfa»\./);
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
  const freeText = mergeOptionText("register_invoice", free, "Alfa SL", nameOf);
  assert.match(freeText, /^Sin límite de importe: por sí sola, la regla nueva cubriría todas las facturas de Alfa SL, de cualquier importe\. Qué haría con sus tareas guardadas, junto a sus demás reglas, lo muestra la simulación\. La tienen «Alfa libre»\./);
  const cappedText = mergeOptionText("register_invoice", capped, "Alfa SL", nameOf);
  assert.match(cappedText, /por sí sola, la regla nueva cubriría las facturas de Alfa SL de hasta 500,00 € y no las de más importe\. Qué haría con sus tareas guardadas, junto a sus demás reglas, lo muestra la simulación\./);
  // Nada que otra regla activa pueda desmentir: ni «irán a revisión» ni «sea del importe que sea».
  for (const text of [freeText, cappedText]) assert.doesNotMatch(text, /irán a revisión|sea del importe|limpia/);
  const any = { key: "any", scope: { scope: "any", value: null }, rule_ids: ["sin"] };
  assert.match(mergeOptionText("send_draft", any, "Solicitud de factura", nameOf), /no hereda la opción avanzada/);
  const senderText = mergeOptionText("send_draft", { key: "sender:ana@alfa.test", scope: { scope: "sender", value: "ana@alfa.test" }, rule_ids: ["direccion"] }, "x", nameOf);
  assert.match(senderText, /Solo a ana@alfa\.test: por sí sola, la regla nueva cubriría únicamente esa dirección\./);
  assert.doesNotMatch(senderText, /irán a revisión/);
  assert.equal(mergeOptionLabel("send_draft", { key: "domain:alfa.test", scope: { scope: "domain", value: "alfa.test" }, rule_ids: [] }), "Todo @alfa.test");
});

const task = (id, before, after) => ({ task_id: id, label: `${id}.txt`, total: null, automated_before_by: before, automated_after_by: after });
const simulation = (over = {}) => ({
  considered: 8,
  truncated: false,
  skipped_without_result: 0,
  touched: 7,
  start_automating: [],
  stop_automating: [],
  unchanged_automated: 0,
  unchanged_review: 0,
  without_currency_check: 0,
  ...over,
});
const plan = (over = {}) => ({
  action_kind: "register_invoice",
  before: [],
  options: [],
  chosen: "500.00",
  after: { conditions: { supplier: "Alfa SL", max_total: "500.00" }, active: false, option_key: "500.00", loses_any_sender: [], needs_sender_validation: false },
  simulation: simulation({
    start_automating: [1, 2, 3, 4, 5].map((i) => task(i, null, "(unificada)")),
    stop_automating: [6, 7].map((i) => task(i, "alfa-libre", null)),
  }),
  simulation_if_validated: null,
  fingerprint: "abc",
  ...over,
});

test("la simulación de unificar da recuentos ciertos y nombra la regla que automatiza cada grupo, sin tramos de importe", () => {
  const t = mergeSimulationText(plan(), nameOf);
  assert.equal(t.empty, false);
  assert.equal(t.lines[0], "5 facturas dejarían de revisarse: hoy van a revisión y con la regla nueva se registrarían solas.");
  assert.equal(t.lines[1], "2 facturas dejarían de automatizarse: hoy las automatiza «Alfa libre» y con la regla nueva irían a revisión.");
  // El texto no deduce ningún tramo del límite elegido.
  assert.doesNotMatch(t.lines.join(" "), /hasta|más de/);
});

test("si otra regla ajena a la unificación es la que automatiza, el texto lo dice y no atribuye el cambio a la nueva", () => {
  // A ≤500 y B ≤1000 se unifican en 500; C ≤600 sigue activa: una factura de 550 € pasa a automatizarla C.
  const t = mergeSimulationText(
    plan({ simulation: simulation({ start_automating: [task(2, null, "alfa-sin-limite")], stop_automating: [task(3, "alfa-500", null)], unchanged_review: 1 }) }),
    nameOf,
  );
  assert.equal(
    t.lines[0],
    "1 factura dejaría de revisarse, pero no por la regla nueva: la automatizaría «alfa-sin-limite», una regla ajena a la unificación que hoy coincide a la vez con alguna de las reglas que se unifican.",
  );
  assert.doesNotMatch(t.lines[0], /hasta 500/);
  assert.match(t.lines[1], /hoy la automatiza «Alfa hasta 500»/);
  assert.equal(t.lines[2], "1 factura seguiría en revisión.");
});

test("sin tareas cubiertas la ausencia es explícita", () => {
  const none = mergeSimulationText(plan({ simulation: simulation({ considered: 0, touched: 0 }) }), nameOf);
  assert.equal(none.empty, true);
  assert.match(none.lines[0], /No hay tareas guardadas con resultado/);
  const uncovered = mergeSimulationText(plan({ simulation: simulation({ considered: 4, touched: 0 }) }), nameOf);
  assert.match(uncovered.lines[0], /no cubren lo guardado \(4 facturas con resultado\)/);
});

test("con «cualquier remitente» se muestran las dos hipótesis, con datos reales y sin afirmar que no se automatiza nada", () => {
  const t = mergeSimulationText(
    plan({
      action_kind: "send_draft",
      after: { conditions: { category: "solicitud_factura" }, active: false, option_key: "any", loses_any_sender: ["sin"], needs_sender_validation: true },
      simulation: simulation({ start_automating: [task(2, null, "otra")], stop_automating: [task(3, "sin", null)], unchanged_review: 1, truncated: true, skipped_without_result: 2, without_currency_check: 0 }),
      simulation_if_validated: simulation({ start_automating: [task(1, null, "(unificada)")], unchanged_review: 1 }),
    }),
    nameOf,
  );
  assert.match(t.notes[0], /«Sin remitente» tenía la opción avanzada «cualquier remitente»: la regla nueva no la hereda\./);
  // La nota va en el encabezado del primer bloque, sin referencias de posición ni «tal como se crea».
  assert.equal(t.heading, "Sin validar la opción avanzada, la regla nueva no contesta a nadie, y esa validación no se da al unificar. Con esa condición:");
  assert.doesNotMatch(`${t.heading} ${t.notes.join(" ")}`, /ningún correo|de arriba|tal como se crea/);
  assert.match(t.notes.join(" "), /hay más antiguas/);
  assert.match(t.notes.join(" "), /2 tareas no tienen resultado/);
  assert.equal(mergeSimulationText(plan(), nameOf).heading, null);
  assert.match(t.lines[0], /1 correo dejaría de revisarse, pero no por la regla nueva: la automatizaría «otra»/);
  assert.match(t.lines[1], /1 correo dejaría de automatizarse: hoy la automatiza «Sin remitente»/);
  assert.match(t.validated.heading, /Si después valida la opción avanzada/);
  assert.match(t.validated.heading, /no se valida al unificar/);
  assert.equal(t.validated.lines[0], "1 correo dejaría de revisarse: hoy va a revisión y con la regla nueva se contestaría solo.");
});

test("las facturas sin comprobación de moneda se anotan", () => {
  const t = mergeSimulationText(plan({ simulation: simulation({ without_currency_check: 2, start_automating: [], stop_automating: [] }) }), nameOf);
  assert.match(t.notes[0], /^2 facturas no se pudieron comprobar en cuanto a la moneda: con cualquier regla irían a revisión\.$/);
  assert.doesNotMatch(t.notes[0], /anteriores/);
});

test("la opción de dominio avisa de un dominio de correo público", () => {
  const gmail = { key: "domain:gmail.com", scope: { scope: "domain", value: "gmail.com" }, rule_ids: ["dominio"] };
  assert.match(mergeOptionText("send_draft", gmail, "Solicitud de factura", nameOf), /gmail\.com abarca a cualquier usuario de Gmail/);
  const own = { key: "domain:alfa.test", scope: { scope: "domain", value: "alfa.test" }, rule_ids: ["dominio"] };
  assert.doesNotMatch(mergeOptionText("send_draft", own, "x", nameOf), /abarca a/);
});

test("la consecuencia nombra a las terceras reglas activas: A sin límite, B hasta 500 y C hasta 1000", () => {
  // Con 700 € coinciden A y C: por eso «solo las de más de 500 se registran solas» no es toda la verdad.
  const finding = overlap({ others: [{ id: "alfa-1000", version: 1, active: true }] });
  const t = explainFinding(finding, (id) => (id === "alfa-1000" ? "Alfa hasta 1000" : nameOf(id)));
  assert.equal(
    t.consequence,
    "Las facturas de Alfa SL de hasta 500,00 € coinciden con las dos reglas y, con dos reglas activas coincidentes, van a revisión aunque las dos las permitan. Las de más de 500,00 € las cubre «Alfa libre», no «Alfa hasta 500». Si además las cubre «Alfa hasta 1000», también coinciden con esa regla y van a revisión.",
  );
  // Sin terceras reglas se mantiene la frase.
  assert.doesNotMatch(explainFinding(overlap({ others: [] }), nameOf).consequence, /Si además/);
  // Varias: coincide con alguna de ellas, no con todas (con 900 € solo coincide una).
  const two = explainFinding(overlap({ others: [{ id: "a", version: 1, active: true }, { id: "b", version: 1, active: true }] }), nameOf);
  assert.match(two.consequence, /Si además las cubre alguna de estas reglas \(«a» o «b»\), también coinciden con ella y van a revisión/);
  assert.doesNotMatch(two.consequence, /«a» y «b»/);
  // Envío: lo mismo con los correos.
  const mail = explainFinding(sender({ others: [{ id: "otra", version: 1, active: true }] }), nameOf);
  assert.match(mail.consequence, /Los de otras direcciones de @alfa\.test los cubre «Dominio Alfa», no «Ana Alfa»\. Si además los cubre «otra»/);
});

test("B hasta 500 y C hasta 1000 con A sin límite: no se afirma nada de lo que hay más allá de 1000 ni que solo ciertos correos se contesten", () => {
  // Hallazgo B+C: la regla A (sin límite) también está activa y cubre más allá de C.
  const bc = explainFinding(
    overlap({
      key: "overlap:alfa-1000+alfa-500",
      rules: rules(["alfa-500", true], ["alfa-1000", true]),
      effect: { code: "narrow_blocks_wide", data: { supplier: "Alfa SL", wider: { id: "alfa-1000", limit: "1000.00" }, narrower: { id: "alfa-500", limit: "500.00" } } },
      others: [{ id: "alfa-libre", version: 1, active: true }],
    }),
    (id) => (id === "alfa-1000" ? "Alfa hasta 1000" : nameOf(id)),
  );
  assert.match(bc.consequence, /las de más de 1\.000,00 € no las cubre ninguna de las dos\./);
  assert.match(bc.consequence, /Si además las cubre «Alfa libre»/);
  assert.doesNotMatch(bc.consequence, /siempre|se registran solas|se revisan/);
  // Envío: dominio, dirección y una tercera «cualquier remitente»: lo de @beta.test no se afirma.
  const mail = explainFinding(sender({ others: [{ id: "cualquiera", version: 1, active: true }] }), nameOf);
  assert.doesNotMatch(mail.consequence, /Solo|solos|siempre/);
  assert.equal(
    mail.consequence,
    "Los correos de ana@alfa.test coinciden con las dos reglas y, con dos reglas activas coincidentes, van a revisión aunque las dos los permitan. Los de otras direcciones de @alfa.test los cubre «Dominio Alfa», no «Ana Alfa». Si además los cubre «cualquiera», también coinciden con esa regla y van a revisión.",
  );
});

test("al activar, «chocaría» solo se dice de reglas activas; lo inactivo y lo parecido se explican aparte", () => {
  const f = (key, kind, active_conflict) => ({ key, kind, active_conflict, action_kind: "register_invoice", rules: [], effect: { code: "x", data: {} }, merge_candidate: false, affected_tasks: null });
  const groups = activationGroups([f("similar:a+b", "similar", false), f("overlap:a+b", "overlap", false), f("overlap:c+d", "overlap", true)]);
  assert.equal(groups.length, 3);
  assert.equal(groups[0].heading, "Al activarla chocaría con otra regla activa:");
  assert.match(groups[1].heading, /hoy está inactiva; solo chocarían si activa también esa/);
  assert.doesNotMatch(groups[1].heading, /^Al activarla chocaría/);
  assert.match(groups[2].heading, /no es un choque, es una pregunta/);
  assert.doesNotMatch(groups[2].heading, /chocaría/);
  assert.deepEqual(activationGroups([]), []);
});

test("al restaurar una regla archivada se avisa con el mismo texto de hallazgo", () => {
  assert.equal(restoreHeading("Alfa hasta 500", [overlap()]), "«Alfa hasta 500» volvería inactiva, pero tiene 1 aviso con las reglas actuales:");
  assert.match(restoreHeading("X", [overlap(), overlap()]), /tiene 2 avisos/);
  const t = explainFinding(overlap({ active_conflict: false, affected_tasks: null, rules: rules(["alfa-500", false], ["alfa-libre", true]) }), nameOf);
  assert.match(t.timing, /Ahora no ocurre: «Alfa hasta 500» está inactiva/);
  assert.equal(t.tasks, null);
  // Al restaurar sí puede haber recuento: el choque ocurrió antes y la nota no lo niega.
  const past = explainFinding(overlap({ active_conflict: false, affected_tasks: 1, rules: rules(["alfa-500", false], ["alfa-libre", true]) }), nameOf);
  assert.match(past.timing, /^Ahora no ocurre:/);
  assert.match(past.tasks, /^1 factura ya coincidió/);
});

test("el historial dice en palabras qué pasó con una regla", () => {
  assert.equal(auditStatusLabel("rule_merge", "merged"), "Unificada");
  assert.equal(auditStatusLabel("mail", "replied_outside"), "Tarea cerrada: ya estaba respondido fuera de NestAdmin");
  assert.equal(auditStatusLabel("rule_archive", "unarchived"), "Restaurada");
  assert.equal(auditStatusLabel("rule_any_sender", "revoked"), "Opción avanzada retirada");
  // Es el estado de la versión guardada, también al crear o editar: no se dice «activada».
  assert.equal(auditStatusLabel("rule", "active"), "Versión activa");
  assert.equal(auditStatusLabel("rule", "inactive"), "Versión inactiva");
  // Lo que no es de una regla se deja tal cual.
  assert.equal(auditStatusLabel("register_invoice", "confirmed"), "confirmed");
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
