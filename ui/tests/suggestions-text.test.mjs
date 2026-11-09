// D132, R24: los textos de las sugerencias solo afirman lo que los datos del backend sostienen.
// Todos los ejemplos son inventados.
import assert from "node:assert/strict";
import { test } from "node:test";
import {
  SUGGESTED_RULE,
  SUGGESTION_HELP,
  SUGGESTION_INACTIVE_NOTICE,
  draftFromSeed,
  explainFinding,
  originText,
  seedNotice,
  suggestionAccepted,
  suggestionLimitText,
  suggestionOutlierWarning,
  suggestionReappeared,
  simulationSummary,
  suggestionSentence,
  suggestionSeenWarning,
  suggestionSupplierNote,
  suggestionWhy,
  suggestionsEmpty,
  suggestionsHidden,
  suggestionsTabCount,
  thresholdProblem,
  thresholdSaved,
} from "../src/rules-text.ts";

const at = (month, day) => new Date(2026, month - 1, day, 12).getTime();

const reply = (over = {}, evidence = {}) => ({
  fingerprint: "f1",
  action_kind: "send_draft",
  name: "Respuestas a pedidos@ejemplo.test",
  conditions: { category: "solicitud_factura", sender: "pedidos@ejemplo.test" },
  subject: "pedidos@ejemplo.test",
  category: "solicitud_factura",
  evidence: { count: 6, threshold: 5, earlier_breaks: 0, last_at: at(11, 3), amounts: [], limit: null, reappeared: null, ...evidence },
  ...over,
});

const invoice = (evidence = {}) => ({
  fingerprint: "f2",
  action_kind: "register_invoice",
  name: "Facturas de Alfa SL",
  conditions: { supplier: "Alfa SL", max_total: "1210.00" },
  subject: "Alfa SL",
  category: null,
  evidence: {
    count: 3,
    threshold: 3,
    earlier_breaks: 0,
    last_at: at(10, 28),
    amounts: ["968.00", "1210.00", "605.00"],
    limit: "1210.00",
    amount_min: "605.00",
    amount_second: "968.00",
    limit_outlier: false,
    reappeared: null,
    ...evidence,
  },
});

const hidden0 = { covered: 0, discarded: 0, accepted: 0, invalid: 0, not_suggested: 0 };
const empty = (over = {}) => ({
  candidates: [],
  progress: [],
  observed: 0,
  counted: 0,
  hidden: { ...hidden0 },
  thresholds: { invoice: 3, reply: 5, min: 2, max: 50 },
  ...over,
});

test("la evidencia de una respuesta dice lo que consta, sin «N de N», «seguidas» ni «las últimas»", () => {
  assert.equal(
    suggestionWhy(reply()),
    "Constan 6 respuestas aprobadas sin cambios a pedidos@ejemplo.test clasificadas como Solicitud de factura; la última, el 03/11.",
  );
  const restarted = suggestionWhy(reply({}, { count: 5, earlier_breaks: 2 }));
  assert.match(restarted, /Constan 5 respuestas aprobadas sin cambios/);
  assert.match(restarted, /Antes constan 2 cortes \(con cambios, rechazo o sin clasificar\) que reiniciaron el recuento\./);
  for (const text of [suggestionWhy(reply()), restarted, suggestionWhy(invoice()), suggestionWhy(invoice({ earlier_breaks: 1 }))]) {
    assert.doesNotMatch(text, /\d+ de \d+ (respuestas|facturas)|seguidas|las últimas|últimas \d/i, text);
  }
});

test("la evidencia de una factura habla de incidencias y nombra todas las causas de reinicio", () => {
  assert.equal(suggestionWhy(invoice()), "Constan 3 facturas aprobadas de Alfa SL sin incidencias; la última, el 28/10.");
  const cut = suggestionWhy(invoice({ earlier_breaks: 1 }));
  assert.match(cut, /Antes consta 1 corte \(una factura con incidencias, rechazada o sin clasificar\)/);
  for (const cause of ["edición", "rechazo", "sin clasificar", "incidencias", "posible duplicada", "factura separada"]) {
    assert.ok(SUGGESTION_HELP.evidence.includes(cause), `la ayuda cita «${cause}»`);
  }
});

test("el límite muestra el rango y el segundo mayor, avisa de un mayor muy alto y recuerda que se reconoce por nombre", () => {
  assert.equal(
    suggestionLimitText(invoice()),
    "Límite propuesto: 1.210,00 €, el mayor de los 3 importes aprobados (de 605,00 € a 1.210,00 €; el segundo mayor es 968,00 €). Importes: 968,00 €, 1.210,00 €, 605,00 €.",
  );
  const many = invoice({ count: 8, amounts: Array.from({ length: 8 }, (_, i) => `${100 + i}.00`), limit: "107.00", amount_min: "100.00", amount_second: "106.00" });
  assert.equal(
    suggestionLimitText(many),
    "Límite propuesto: 107,00 €, el mayor de los 8 importes aprobados (de 100,00 € a 107,00 €; el segundo mayor es 106,00 €).",
  );
  assert.equal(suggestionLimitText(reply()), null, "una respuesta no tiene límite");

  assert.equal(suggestionOutlierWarning(invoice()), null);
  const big = invoice({ limit: "9680.00", amount_second: "242.00", amount_min: "121.00", limit_outlier: true });
  assert.equal(
    suggestionOutlierWarning(big),
    "Revise el límite: el mayor importe (9.680,00 €) es al menos el triple del segundo mayor (242,00 €) y podría ser demasiado alto.",
  );
  assert.equal(suggestionOutlierWarning(reply()), null);
  assert.equal(suggestionOutlierWarning(invoice({ limit_outlier: true, amount_second: "0.00" })), null, "con un segundo mayor no positivo no se avisa");

  assert.match(suggestionSupplierNote(invoice()), /solo por su nombre, «Alfa SL», tal como aparece en la factura; no comprueba que sea la misma empresa/);
  assert.equal(suggestionSupplierNote(reply()), null);
  assert.match(SUGGESTION_HELP.limit, /solo por su nombre/);
});

test("la frase de la regla describe lo que cubriría por sí sola", () => {
  assert.equal(suggestionSentence(invoice()), "Registrar sin revisión las facturas de Alfa SL de hasta 1.210,00 €");
  assert.equal(
    suggestionSentence(reply()),
    "Enviar sin revisión la respuesta redactada por la IA a pedidos@ejemplo.test cuando se clasifique como Solicitud de factura",
  );
});

test("ningún texto promete que se registrará o enviará sola, ni dice siempre", () => {
  const texts = [
    suggestionWhy(invoice()),
    suggestionWhy(reply()),
    suggestionLimitText(invoice()) ?? "",
    suggestionOutlierWarning(invoice({ limit_outlier: true })) ?? "",
    suggestionSupplierNote(invoice()) ?? "",
    SUGGESTION_INACTIVE_NOTICE,
    suggestionAccepted("Facturas de Alfa SL"),
    ...["foreign_currency", "currency_not_shown", "currency_not_checked", "amount_review", "no_total"].map((code) =>
      seedNotice({ action_kind: "register_invoice", conditions: {}, name: "", limit_from_this_invoice: false, limit_skipped: { code, currencies: ["USD"] } }),
    ),
    seedNotice({ action_kind: "register_invoice", conditions: {}, name: "", limit_from_this_invoice: true }),
    seedNotice({ action_kind: "send_draft", conditions: {}, name: "", limit_from_this_invoice: false }),
    ...Object.values(SUGGESTION_HELP),
    suggestionsEmpty(empty()).hint,
    thresholdSaved("invoice", 2),
  ].join("\n");
  assert.doesNotMatch(texts, /se registrará sola|se enviará sola|siempre|nunca fall|garantiz/i);
  assert.match(SUGGESTION_INACTIVE_NOTICE, /nacerá inactiva/);
  assert.match(suggestionAccepted("X"), /inactiva/);
  assert.match(SUGGESTION_HELP.thresholds, /ninguna se crea ni se activa sin su aceptación/);
});

test("el estado vacío no afirma «sin datos» si solo se aprobaron partes separadas o acciones fallidas", () => {
  const none = suggestionsEmpty(empty());
  assert.equal(none.title, "Aún no hay datos para sugerir reglas");
  assert.match(none.hint, /No hay observaciones que cuenten/);
  assert.doesNotMatch(none.hint, /ninguna decisión suya/, "podría haber aprobado partes de una factura separada");
  assert.match(none.hint, /3 facturas sin incidencias/);
  assert.match(none.hint, /5 respuestas aprobadas sin cambios/);

  const decided = suggestionsEmpty(empty({ observed: 3 }));
  assert.equal(decided.title, "Aún no hay observaciones que cuenten");
  assert.match(decided.hint, /Constan 3 decisiones, pero ninguna cuenta como evidencia ahora/);

  const some = suggestionsEmpty(
    empty({
      observed: 6,
      counted: 5,
      progress: [
        { action_kind: "register_invoice", subject: "Beta SA", category: null, count: 2, needed: 3, missing: 1 },
        { action_kind: "send_draft", subject: "ventas@ejemplo.test", category: "consulta_pago", count: 3, needed: 5, missing: 2 },
      ],
    }),
  );
  assert.equal(some.title, "Aún no hay sugerencias, pero hay datos");
  assert.deepEqual(some.lines, [
    "Beta SA: 2 de 3 facturas sin incidencias (falta 1)",
    "ventas@ejemplo.test (Consulta de pago): 3 de 5 respuestas aprobadas sin cambios (faltan 2)",
  ]);
});

test("el estado vacío cuenta con lo oculto: nunca dice que ninguna serie llega al mínimo si hay una descartada, cubierta o aceptada", () => {
  for (const key of ["discarded", "covered", "accepted", "invalid", "not_suggested"]) {
    const s = suggestionsEmpty(empty({ observed: 3, counted: 3, hidden: { ...hidden0, [key]: 1 } }));
    assert.equal(s.title, "No hay sugerencias que mostrar", key);
    assert.match(s.hint, /3 observaciones cuentan, pero lo que podría sugerirse está oculto/, key);
    assert.doesNotMatch(s.hint, /ninguna serie llega|no llega/i, key);
  }
  const plain = suggestionsEmpty(empty({ observed: 3, counted: 3 }));
  assert.doesNotMatch(plain.hint, /ninguna serie llega/);
  assert.match(plain.hint, /3 observaciones cuentan, pero no hay ninguna sugerencia que mostrar/);
  assert.match(suggestionsEmpty(empty({ observed: 1, counted: 1 })).hint, /1 observación cuenta, pero/);
});

test("lo que no se muestra se dice, y nada oculto se calla", () => {
  assert.equal(suggestionsHidden({ ...hidden0 }), null);
  assert.equal(
    suggestionsHidden({ covered: 1, discarded: 2, accepted: 1, invalid: 3, not_suggested: 1 }),
    "No se muestran: 1 ya cubierta por una regla existente (la misma o una activa más amplia); 2 descartadas; 1 ya aceptada; 3 sin datos válidos para una regla; 1 de la categoría «Otros», que no se sugiere (puede crear la regla a mano).",
  );
});

test("sin cálculo no hay recuento en la pestaña, ni un cero", () => {
  assert.equal(suggestionsTabCount(null, true), undefined);
  assert.equal(suggestionsTabCount({ candidates: [1, 2] }, true), undefined, "un cálculo anterior no vale si el último falló");
  assert.equal(suggestionsTabCount(null, false), undefined);
  assert.equal(suggestionsTabCount({ candidates: [] }, false), 0);
  assert.equal(suggestionsTabCount({ candidates: [1, 2] }, false), 2);
});

test("una sugerencia descartada que vuelve explica por qué", () => {
  assert.equal(suggestionReappeared(reply()), null);
  assert.equal(
    suggestionReappeared(reply({}, { reappeared: { at_discard: 3, new_since: 6, needed_new: 6 } })),
    "La descartó cuando había 3 observaciones. Vuelve a aparecer porque desde entonces hay 6 nuevas (hacían falta 6).",
  );
});

test("los umbrales: el mínimo impide sugerir con una sola observación", () => {
  const t = { min: 2, max: 50 };
  assert.match(thresholdProblem("1", t), /El mínimo es 2: con una sola observación no se sugiere nada/);
  assert.match(thresholdProblem("0", t), /El mínimo es 2/);
  assert.match(thresholdProblem("51", t), /El máximo es 50/);
  for (const bad of ["", " ", "-3", "2,5", "abc", "1e2", "2 3"]) assert.match(thresholdProblem(bad, t), /número entero/, bad);
  for (const ok of ["2", " 5 ", "50"]) assert.equal(thresholdProblem(ok, t), null, ok);
  assert.match(thresholdSaved("reply", 4), /Umbral de respuestas guardado: 4\. Queda en el historial\. No crea ni activa ninguna regla\./);
});

test("el borrador precargado desde una revisión conserva lo que calculó el backend", () => {
  const draft = draftFromSeed({
    action_kind: "register_invoice",
    conditions: { supplier: "Alfa SL", max_total: "1210.00" },
    name: "Facturas de Alfa SL",
    limit_from_this_invoice: true,
  });
  assert.equal(draft.kind, "register_invoice");
  assert.equal(draft.supplier, "Alfa SL");
  assert.equal(draft.max_total, "1210.00");
  assert.equal(draft.ack, false, "nunca llega con la opción avanzada marcada");

  const noLimit = draftFromSeed({ action_kind: "register_invoice", conditions: { supplier: "Alfa SL" }, name: "Facturas de Alfa SL", limit_from_this_invoice: false, limit_skipped: { code: "foreign_currency", currencies: ["USD"] } });
  assert.equal(noLimit.max_total, "", "sin euro comprobado no hay límite precargado");

  const mail = draftFromSeed({
    action_kind: "send_draft",
    conditions: { category: "consulta_pago", sender: "ventas@ejemplo.test" },
    name: "Respuestas a ventas@ejemplo.test",
    limit_from_this_invoice: false,
  });
  assert.deepEqual([mail.kind, mail.category, mail.scope, mail.sender], ["send_draft", "consulta_pago", "sender", "ventas@ejemplo.test"]);
});

test("el aviso de la precarga dice por qué no hay límite, con la moneda concreta", () => {
  const notice = (code, currencies = []) =>
    seedNotice({ action_kind: "register_invoice", conditions: {}, name: "", limit_from_this_invoice: false, limit_skipped: { code, currencies } });
  assert.equal(
    notice("foreign_currency", ["USD"]),
    "Precargado desde esta factura: solo el proveedor, porque el documento menciona USD y el límite de una regla está en euros, así que no se precarga ningún límite. La regla nacerá inactiva.",
  );
  assert.match(notice("foreign_currency", ["CAD", "USD"]), /menciona CAD, USD/);
  assert.match(notice("currency_not_shown"), /no muestra la moneda y no se puede comprobar que el total esté en euros/);
  assert.match(notice("currency_not_checked"), /no consta la comprobación de moneda/);
  assert.match(notice("amount_review"), /el importe de esta factura está pendiente de comprobar/);
  assert.match(notice("no_total"), /no consta un total legible/);
  assert.match(
    seedNotice({ action_kind: "register_invoice", conditions: {}, name: "", limit_from_this_invoice: true, limit_skipped: null }),
    /el total de esta factura\. Es un solo dato, no una estadística; ajústelo\. La regla nacerá inactiva\./,
  );
  assert.match(seedNotice({ action_kind: "send_draft", conditions: {}, name: "", limit_from_this_invoice: false }), /la categoría y la dirección del remitente\. La regla nacerá inactiva/);
});

test("el origen de una regla creada desde una sugerencia se muestra sin inventar", () => {
  const nameOf = (id) => id;
  assert.equal(originText({ kind: "suggestion", count: 6, threshold: 5 }, nameOf), "Creada desde una sugerencia, con 6 observaciones (el mínimo era 5).");
  assert.equal(originText({ kind: "suggestion", count: 3 }, nameOf), "Creada desde una sugerencia, con 3 observaciones.");
  assert.equal(originText({ kind: "suggestion" }, nameOf), null, "sin cifras no se afirma nada");
});

test("el aviso de solapamiento de una sugerencia habla de «esta regla», no de un identificador interno", () => {
  const finding = {
    key: "overlap:alfa-libre+(sugerida)",
    kind: "overlap",
    action_kind: "register_invoice",
    rules: [
      { id: "alfa-libre", version: 1, active: false },
      { id: SUGGESTED_RULE, version: 1, active: true },
    ],
    active_conflict: false,
    effect: {
      code: "narrow_blocks_wide",
      data: { supplier: "Alfa SL", wider: { id: "alfa-libre", limit: null }, narrower: { id: SUGGESTED_RULE, limit: "1210.00" } },
    },
    merge_candidate: true,
    affected_tasks: null,
  };
  const t = explainFinding(finding, (id) => (id === "alfa-libre" ? "Alfa libre" : id));
  const all = [t.title, t.what, t.why, t.consequence, t.timing ?? ""].join("\n");
  assert.ok(!all.includes("(sugerida)"), all);
  assert.match(all, /esta regla/);
  assert.match(all, /«Alfa libre»/);
  assert.match(t.timing, /Ahora no ocurre: «Alfa libre» está inactiva/);
  assert.equal(t.tasks, null, "sin recuento no se afirma ninguna tarea afectada");
});

// ---- E5: la primera lectura no es una aprobación ----

const seenInvoice = (evidence = {}) =>
  invoice({ count: 5, threshold: 5, approved: 0, seen: 5, amounts: ["121.00", "121.00", "242.00", "363.00", "484.00"], limit: "484.00", amount_min: "121.00", amount_second: "363.00", ...evidence });

test("una sugerencia solo de facturas vistas no afirma aprobaciones ni validación, y no propone límite", () => {
  const e = { needs_limit: true, limit: null, amounts: [], amount_min: null, amount_second: null };
  const text = suggestionWhy(seenInvoice(e));
  assert.match(text, /^Se observaron 5 facturas de Alfa SL en la primera lectura; usted no ha aprobado ninguna de ellas y nadie ha revisado su contenido\. La última se leyó el /);
  assert.doesNotMatch(text, /aprobadas de|Constan|Ninguna tenía incidencias|sin incidencias/, "no afirma aprobaciones ni validación");
  assert.match(suggestionLimitText(seenInvoice(e)), /No hay límite propuesto.*un límite no sale de lo que solo se ha visto.*Escriba usted el límite/);
  assert.equal(suggestionOutlierWarning(seenInvoice(e)), null);
  assert.equal(suggestionSeenWarning(seenInvoice(e)), "Estas facturas solo se han leído; nadie las ha revisado y un tercero puede haberlas enviado.");
  assert.equal(suggestionSeenWarning(invoice()), null);
  assert.match(SUGGESTION_HELP.evidence, /Cuentan sus aprobaciones ya ejecutadas y, solo en facturas, las facturas vistas/);
  assert.doesNotMatch(SUGGESTION_HELP.evidence, /Solo cuentan aprobaciones suyas/);
});

test("con aprobaciones y vistas mezcladas el texto separa unas de otras y el límite sale de lo aprobado", () => {
  const text = suggestionWhy(seenInvoice({ count: 5, approved: 2, seen: 3 }));
  assert.match(text, /2 facturas aprobadas por usted y 3 vistas en la primera lectura sin aprobar/);
  assert.match(text, /fecha de lectura o de aprobación, no de la factura/);
  assert.match(suggestionLimitText(seenInvoice({ approved: 2, seen: 3 })), /importes aprobados/);
  assert.match(suggestionWhy(invoice()), /Constan 3 facturas aprobadas de Alfa SL/);
});

test("el origen de la regla dice que las facturas eran solo vistas", () => {
  const nameOf = () => "";
  assert.match(originText({ kind: "suggestion", count: 5, threshold: 5, approved: 0, seen: 5 }, nameOf), /5 facturas vistas en la primera lectura, ninguna aprobada por usted/);
  assert.match(originText({ kind: "suggestion", count: 5, threshold: 3, approved: 3, seen: 2 }, nameOf), /3 aprobadas por usted y 2 vistas en la primera lectura sin aprobar/);
  assert.doesNotMatch(originText({ kind: "suggestion", count: 3, threshold: 3 }, nameOf), /primera lectura/);
});

test("el estado vacío cuenta las facturas vistas aparte de las decisiones y pide más repetición", () => {
  const only = suggestionsEmpty(empty({ observed: 4, seen: 4, decisions: 0, counted: 0 }));
  assert.match(only.hint, /Constan 4 facturas vistas en la primera lectura, pero ninguna cuenta/);
  assert.doesNotMatch(only.hint, /decisiones/);
  assert.match(only.hint, /5 si solo se vieron en la primera lectura/);
  const raised = suggestionsEmpty(empty({ thresholds: { invoice: 8, reply: 5, min: 2, max: 50 } }));
  assert.match(raised.hint, /8 si solo se vieron/, "subir el umbral sube también el mínimo de vistas");
  assert.match(SUGGESTION_HELP.seen, /no las ha aprobado/);
  assert.match(SUGGESTION_HELP.seen, /Nunca sirven para sugerir respuestas por correo/);
});

test("la simulación cuenta aparte lo que solo se leyó y no lo presenta como tareas", () => {
  const sim = { considered: 3, truncated: false, limit: 200, skipped_without_result: 0, without_currency_check: 0, would_automate: 2, would_review: {}, overlaps: 0, no_match: 1, would_block_other: [], items: [], from_read: { considered: 5, would_automate: 4, truncated: false, limit: 200 } };
  const summary = simulationSummary(sim, "register_invoice");
  assert.match(summary.notes.join(" "), /Aparte: de 5 facturas que solo se leyeron en la primera lectura.*habría registrado 4\. No son tareas suyas: no están en Tareas ni se registraron/);
  assert.match(summary.headline, /De las últimas 3 /);
  assert.doesNotMatch(simulationSummary({ ...sim, from_read: null }, "register_invoice").notes.join(" "), /Aparte/);
});

test("lo ocultado por una factura vista con incidencias se dice, sin silencio", () => {
  const h = suggestionsHidden({ covered: 0, discarded: 0, accepted: 0, invalid: 0, seen_invalidated: 2 });
  assert.match(h, /2 proveedores con facturas solo leídas no se sugieren: basta una factura con incidencias, posiblemente enviada por un tercero, para que ninguna de las leídas cuente; sus aprobaciones, si las hay, se conservan/);
  assert.match(suggestionsHidden({ covered: 0, discarded: 0, accepted: 0, invalid: 0, seen_invalidated: 1 }), /1 proveedor con facturas solo leídas no se sugiere/);
  assert.equal(suggestionsHidden({ covered: 0, discarded: 0, accepted: 0, invalid: 0, seen_invalidated: 0 }), null);
});
