// E1, D181: los textos del chat describen una búsqueda por palabras hecha en el equipo, no una respuesta de la IA.
// Todos los ejemplos son inventados.
import assert from "node:assert/strict";
import { test } from "node:test";
import * as T from "../src/assistant-text.ts";
import { auditStatusLabel } from "../src/rules-text.ts";

const hit = (over = {}) => ({
  kind: "invoice",
  title: "Factura F-2026-014",
  party: "Suministros Ejemplo S.L.",
  number: "F-2026-014",
  doc_date: "2026-03-05",
  total_minor: 123456,
  currency: "EUR",
  index_status: "indexed",
  snippet: "Material de oficina.",
  ...over,
});

test("respuesta con todas las palabras: singular y plural", () => {
  assert.equal(T.answerText("all", 1, ["factura"]), "Encontré 1 documento con todas las palabras.");
  assert.equal(T.answerText("all", 3, ["factura"]), "Encontré 3 documentos con todas las palabras.");
});

test("respuesta con alguna palabra lo dice expresamente", () => {
  const one = T.answerText("any", 1, ["a", "b"]);
  assert.ok(one.startsWith("No encontré nada con todas las palabras; esto coincide con alguna."));
  assert.ok(one.includes("1 documento."));
  assert.ok(T.answerText("any", 4, ["a", "b"]).includes("4 documentos."));
});

test("sin resultados dice qué se buscó y sugiere proveedor, número o mes", () => {
  const text = T.answerText("none", 0, ["taller", "demo"]);
  assert.ok(text.includes("«taller», «demo»"));
  assert.ok(text.includes("proveedor") && text.includes("número") && text.includes("mes"));
  // Aunque matched diga otra cosa, total 0 es «sin resultados».
  assert.ok(T.answerText("all", 0, ["x"]).startsWith("No encontré nada con «x»"));
});

test("sin palabras útiles se pide el proveedor, el número o el mes", () => {
  const text = T.answerText("none", 0, []);
  assert.ok(text.includes("No encontré palabras útiles"));
  assert.ok(text.includes("proveedor") && text.includes("número") && text.includes("mes"));
});

test("el tipo se nombra de forma natural, con singular y plural", () => {
  assert.equal(T.answerText("all", 1, ["luz"], "invoice"), "Encontré 1 factura con todas las palabras.");
  assert.equal(T.answerText("all", 2, ["luz"], "invoice"), "Encontré 2 facturas con todas las palabras.");
  assert.equal(T.answerText("all", 3, ["luz"], "email"), "Encontré 3 correos con todas las palabras.");
  assert.equal(T.answerText("all", 2, ["luz"], null), "Encontré 2 documentos con todas las palabras.");
  assert.ok(T.answerText("any", 1, ["a", "b"], "email").startsWith("No encontré ningún correo con todas las palabras;"));
  assert.ok(T.answerText("any", 1, ["a", "b"], "email").endsWith("Hay 1 correo."));
  assert.ok(T.answerText("any", 2, ["a", "b"], "invoice").startsWith("No encontré ninguna factura con todas las palabras;"));
});

test("sin resultados con tipo dice qué tipo y qué palabras", () => {
  assert.ok(T.answerText("none", 0, ["luz"], "invoice").startsWith("No encontré ninguna factura con «luz»."));
  assert.ok(T.answerText("none", 0, ["luz"], "email").startsWith("No encontré ningún correo con «luz»."));
  assert.equal(T.answerText("none", 0, [], "invoice"), "No encontré ninguna factura en este equipo.");
});

test("solo el tipo, sin palabras: dice que son las más recientes", () => {
  assert.equal(T.answerText("all", 4, [], "invoice"), "Estas son las 4 facturas más recientes.");
  assert.equal(T.answerText("all", 1, [], "invoice"), "Esta es la factura más reciente.");
  assert.equal(T.answerText("all", 2, [], "email"), "Estas son las 2 correos más recientes.");
  assert.equal(T.answerText("all", 1, [], "email"), "Este es el correo más reciente.");
});

test("el aviso fijo dice que no es la IA y ya no promete un asistente futuro", () => {
  assert.ok(T.SEARCH_NOTICE.includes("búsqueda por palabras hecha en este equipo"));
  assert.ok(T.SEARCH_NOTICE.includes("no una respuesta de la IA"));
  assert.ok(!/versión posterior|llegará|próxim/i.test(T.SEARCH_NOTICE));
  for (const text of Object.values(T).filter((v) => typeof v === "string")) assert.ok(!/versión posterior/i.test(text), text);
});

test("recuentos con singular y plural, y vacío si no hay nada", () => {
  assert.deepEqual(T.countLines({ pending_index: 0, no_text: 0, failed: 0 }), []);
  assert.deepEqual(T.countLines({ pending_index: 1, no_text: 0, failed: 0 }), ["Aún se está preparando 1 documento."]);
  assert.deepEqual(T.countLines({ pending_index: 4, no_text: 2, failed: 1 }), [
    "Aún se están preparando 4 documentos.",
    "2 sin texto legible.",
    "1 no se pudo leer.",
  ]);
  assert.deepEqual(T.countLines({ pending_index: 0, no_text: 0, failed: 3 }), ["3 no se pudieron leer."]);
});

test("«Mostrando N de M» solo si hay más de los devueltos", () => {
  assert.equal(T.shownLine(50, 120), "Mostrando 50 de 120");
  assert.equal(T.shownLine(50, 50), null);
  assert.equal(T.shownLine(0, 0), null);
});

test("importe en céntimos sin coma flotante", () => {
  assert.equal(T.formatMinor(123456, "EUR"), "1.234,56 €");
  assert.equal(T.formatMinor(0, "EUR"), "0,00 €");
  assert.equal(T.formatMinor(5, "EUR"), "0,05 €");
  assert.equal(T.formatMinor(-2550, "EUR"), "-25,50 €");
  assert.equal(T.formatMinor(100000000, "EUR"), "1.000.000,00 €");
  assert.equal(T.formatMinor(1999, "USD"), "19,99 $");
  assert.equal(T.formatMinor(1999, "CHF"), "19,99 CHF");
  assert.equal(T.formatMinor(null, "EUR"), null);
  assert.equal(T.formatMinor(undefined, "EUR"), null);
});

test("moneda desconocida o vacía: solo el número", () => {
  assert.equal(T.formatMinor(123456, ""), "1.234,56");
  assert.equal(T.formatMinor(123456, "euros"), "1.234,56");
  assert.equal(T.formatMinor(123456, null), "1.234,56");
});

test("fecha en formato español", () => {
  assert.equal(T.formatDate("2026-03-05"), "05/03/2026");
  assert.equal(T.formatDate("2026-03-05T10:00:00Z"), "05/03/2026");
  assert.equal(T.formatDate(null), null);
  assert.equal(T.formatDate("  "), null);
  assert.equal(T.formatDate("marzo"), "marzo");
});

test("tarjeta completa", () => {
  const v = T.cardView(hit());
  assert.equal(v.kind, "Factura");
  assert.equal(v.title, "Factura F-2026-014");
  assert.deepEqual(v.meta, ["Suministros Ejemplo S.L.", "N.º F-2026-014", "05/03/2026", "1.234,56 €"]);
  assert.equal(v.note, null);
  assert.equal(v.snippet, "Material de oficina.");
});

test("tarjeta de correo sin datos: solo lo que consta", () => {
  const v = T.cardView(hit({ kind: "email", number: "", party: "", doc_date: null, total_minor: null, currency: "", title: "  " }));
  assert.equal(v.kind, "Correo");
  assert.equal(v.title, "Sin título");
  assert.deepEqual(v.meta, []);
});

test("resultado sin texto o que falló lleva su estado", () => {
  assert.equal(T.cardView(hit({ index_status: "no_text", snippet: "" })).note, "Sin texto legible");
  assert.equal(T.cardView(hit({ index_status: "failed" })).note, "No se pudo leer");
  assert.equal(T.cardView(hit({ index_status: "indexed" })).note, null);
  // D179: una tarea que espera a la IA avisa de que aún no se ha leído; lo que ya terminó, no.
  assert.equal(T.cardView(hit({ task_state: "queued" })).note, "En espera de la IA: aún no se ha leído");
  assert.equal(T.cardView(hit({ task_state: "completed" })).note, null);
  assert.equal(T.cardView(hit({ task_state: "queued", index_status: "failed" })).note, "No se pudo leer");
  assert.equal(T.cardView(hit({ index_status: "no_text", snippet: "" })).snippet, "");
});

test("etiqueta accesible de la tarjeta", () => {
  assert.equal(T.cardLabel(hit({ total_minor: null })), "Abrir factura: Factura F-2026-014, Suministros Ejemplo S.L., N.º F-2026-014, 05/03/2026");
});

test("errores y reintento con singular y plural", () => {
  assert.equal(T.searchErrorText("sin acceso"), "No se pudo buscar: sin acceso");
  assert.equal(T.searchErrorText(""), "No se pudo buscar. Vuelva a intentarlo.");
  assert.ok(T.reindexLine(0).startsWith("No había documentos"));
  assert.ok(T.reindexLine(1).startsWith("Se volverá a intentar 1 documento."));
  assert.ok(T.reindexLine(2).startsWith("Se volverán a intentar 2 documentos."));
  assert.ok(T.reindexErrorText("x").includes("No se pudo reintentar: x"));
});

test("ningún texto fijo ni generado contiene HTML", () => {
  const texts = [
    ...Object.values(T).filter((v) => typeof v === "string"),
    ...T.CHAT_EXAMPLES,
    T.answerText("all", 2, ["a"]),
    T.answerText("any", 2, ["a", "b"]),
    T.answerText("none", 0, ["a"]),
    T.answerText("none", 0, []),
    ...T.countLines({ pending_index: 2, no_text: 1, failed: 2 }),
    T.shownLine(50, 90),
    T.reindexLine(3),
    T.OPEN_ORIGINAL_LABEL,
    T.OPEN_ORIGINAL_PREVIEW,
    T.originalErrorText(""),
    T.originalErrorText("la copia ya no está"),
    T.NOT_FOUND_TASK,
  ];
  for (const text of texts) assert.ok(!/[<>]|&\w+;/.test(text), text);
});

test("solo la tarjeta de una factura ofrece abrir el documento, y el error del fondo se muestra tal cual", () => {
  const invoice = { kind: "invoice", title: "Factura F-1", party: "", number: "", doc_date: null, total_minor: null, currency: "", index_status: "indexed", snippet: "" };
  assert.equal(T.canOpenOriginal(invoice), true);
  assert.equal(T.canOpenOriginal({ ...invoice, kind: "email" }), false);
  assert.equal(T.originalLabel(invoice), "Abrir el documento: Factura F-1");
  assert.equal(T.originalErrorText("la copia ya no está"), "la copia ya no está");
  assert.equal(T.originalErrorText(new Error("x")), "Error: x");
  assert.equal(T.originalErrorText(""), "No se pudo abrir el documento.");
  assert.equal(T.originalErrorText(null), "No se pudo abrir el documento.");
});

// ---- E2: asistente con IA (D182-D184, D188). Todo inventado. ----

const external = { provider: "ollama_cloud", model: "modelo-x", local: false, fallback: null };

test("quién contestó: externo, local y respaldo", () => {
  assert.equal(T.whoAnswered(external), "Respondió Ollama cloud · modelo-x");
  assert.equal(T.whoAnswered({ ...external, provider: "ollama", local: true }), "Respondió la IA local de este equipo");
  const fb = T.whoAnswered({ provider: "ollama", model: "m", local: true, fallback: { from: "ollama_cloud", reason: "timeout" } });
  assert.ok(fb.includes("El proveedor externo (Ollama cloud) no respondió a tiempo"));
  assert.ok(fb.includes("contestó la IA local de este equipo"));
  assert.ok(T.whoAnswered({ provider: "ollama", model: "m", local: true, fallback: { from: "anthropic", reason: "otro" } }).includes("no respondió y contestó"));
  assert.equal(T.whoAnswered({ provider: null, model: null, local: false, fallback: null }), "");
});

test("citas descartadas y fragmentos retenidos, en singular y plural", () => {
  assert.equal(T.droppedCitationsText(0), null);
  assert.equal(T.droppedCitationsText(1), "Se descartó 1 referencia que no venía de sus documentos.");
  assert.equal(T.droppedCitationsText(3), "Se descartaron 3 referencias que no venían de sus documentos.");
  assert.equal(T.withheldText(0), null);
  assert.equal(T.withheldText(1), "1 fragmento no se envió a la IA por contener datos sensibles.");
  assert.equal(T.withheldText(2), "2 fragmentos no se enviaron a la IA por contener datos sensibles.");
});

test("cada código de error tiene su mensaje y todos dicen que no se cambió nada", () => {
  const codes = ["unauthorized", "timeout", "rate_limited", "no_credit", "model_unavailable", "request_rejected", "unavailable", "invalid_response"];
  const seen = new Set();
  for (const code of codes) {
    const text = T.assistantErrorText(code);
    assert.ok(text.endsWith(T.NOTHING_CHANGED), code);
    seen.add(text);
  }
  assert.equal(seen.size, codes.length);
  assert.ok(T.assistantErrorText("unauthorized").includes("clave"));
  assert.ok(T.assistantErrorText("algo_raro").startsWith("El asistente no pudo contestar."));
  assert.ok(T.assistantErrorText(null).endsWith(T.NOTHING_CHANGED));
  assert.ok(T.assistantCallErrorText("sin red").includes("sin red"));
  assert.ok(T.assistantCallErrorText("sin red").endsWith(T.NOTHING_CHANGED));
  assert.ok(T.assistantCallErrorText("").endsWith(T.NOTHING_CHANGED));
});

test("sin permiso o sin IA el motivo dice cómo resolverlo", () => {
  assert.ok(T.CONSENT_REQUIRED_TEXT.includes("Configuración"));
  assert.ok(T.CONSENT_REQUIRED_TEXT.includes("no sale nada"));
  assert.ok(T.NOT_CONFIGURED_TEXT.includes("Configuración"));
  assert.equal(T.CANCELLED_TEXT, "Pregunta cancelada.");
  assert.ok(T.WORDS_INTRO.includes("búsqueda por palabras"));
});

test("las vistas a las que se navega son solo las de la lista cerrada", () => {
  for (const view of ["review", "tasks", "rules", "history", "mail", "settings"]) assert.equal(T.navigateTarget(view), view);
  for (const view of ["setup", "wizard", "", null, undefined, "constructor", "__proto__", "toString"]) assert.equal(T.navigateTarget(view), null, String(view));
  assert.equal(T.navigatedText("rules"), "Le he llevado a «Reglas». El chat sigue abierto.");
});

test("la respuesta se divide en párrafos sin interpretar nada", () => {
  assert.deepEqual(T.textParagraphs("Uno\n\nDos  \r\nTres"), ["Uno", "Dos", "Tres"]);
  assert.deepEqual(T.textParagraphs(""), []);
  // Una URL, HTML o Markdown quedan tal cual: el pintado usa solo texto.
  assert.deepEqual(T.textParagraphs("Vea https://ejemplo.test/x y <b>negrita</b> **no**"), ["Vea https://ejemplo.test/x y <b>negrita</b> **no**"]);
});

test("borrado de conversaciones: singular y plural", () => {
  assert.equal(T.clearedText(0), "No había conversaciones que borrar.");
  assert.equal(T.clearedText(1), "Se borró 1 conversación.");
  assert.equal(T.clearedText(4), "Se borraron 4 conversaciones.");
});

test("«Ver lo enviado» dice dónde se guarda y qué ocurre sin llamadas", () => {
  assert.ok(T.SENT_INTRO.includes("solo en este equipo"));
  assert.ok(T.SENT_NONE.includes("no se envió nada"));
  const call = { step: 2, provider: "ollama_cloud", model: "modelo-x", host: "ollama.test", local: false };
  assert.equal(T.sentCallTitle(call), "Llamada 2: Ollama cloud · modelo-x");
  assert.equal(T.sentCallWhere(call), "Servidor: ollama.test");
  assert.ok(T.sentCallWhere({ ...call, local: true, host: "127.0.0.1" }).includes("no salió nada"));
  assert.equal(T.sentCallWhere({ ...call, host: "" }), "Servicio externo");
  assert.ok(T.sentError("x").includes("x"));
  assert.equal(T.sentError(""), "No se pudo cargar lo enviado.");
});

test("estados del permiso del asistente y qué botones corresponden", () => {
  const view = (state, over = {}) => ({ state, provider: "ollama_cloud", model: "modelo-x", host: "ollama.test", at: 1_700_000_000, ...over });
  assert.ok(T.consentStateText(view("not_needed")).includes("local"));
  assert.ok(T.consentStateText(view("missing")).startsWith("Desactivada"));
  const granted = T.consentStateText(view("granted"));
  assert.ok(granted.startsWith("Activada para Ollama cloud · modelo-x (servidor ollama.test), desde el "));
  assert.ok(T.consentStateText(view("granted", { at: null, host: null })).endsWith("modelo-x."));
  assert.ok(T.consentStateText(view("stale")).includes("han cambiado"));
  assert.equal(T.canGrantConsent(view("missing")), true);
  assert.equal(T.canGrantConsent(view("stale")), true);
  assert.equal(T.canGrantConsent(view("granted")), false);
  assert.equal(T.canGrantConsent(view("not_needed")), false);
  assert.equal(T.canWithdrawConsent(view("granted")), true);
  assert.equal(T.canWithdrawConsent(view("stale")), true);
  assert.equal(T.canWithdrawConsent(view("missing")), false);
  assert.equal(T.canWithdrawConsent(view("not_needed")), false);
  assert.ok(T.ASSISTANT_WITHOUT.includes("IA local") && T.ASSISTANT_WITHOUT.includes("búsqueda por palabras"));
  assert.equal(T.ASSISTANT_OPTION, "Permitir que el asistente envíe datos fuera de este equipo");
});

test("destino y límites de la vista previa, en frases", () => {
  assert.equal(T.assistantDestinationText("anthropic", "modelo-y", "api.ejemplo.test"), "Destino: Claude · Anthropic · modelo-y · servidor api.ejemplo.test.");
  assert.equal(T.assistantDestinationText("ollama_cloud", "m", null), "Destino: Ollama cloud · m.");
  const lines = T.limitLines({ steps: 6, history_turns: 6, turn_chars: 2000, fragments: 8, fragment_chars: 600, data_chars: 12000 });
  assert.deepEqual(lines, [
    "Hasta 6 pasos por pregunta.",
    "Como mucho los 6 últimos turnos de la conversación, de 2.000 caracteres cada uno.",
    "Hasta 8 fragmentos de documentos de 600 caracteres cada uno, y 12.000 caracteres de datos en total por llamada.",
  ]);
  assert.ok(T.localFallbackLine(true).includes("una sola vez"));
  assert.ok(T.localFallbackLine(false).includes("desactivado"));
  assert.ok(T.grantErrorText("El destino cambió.").includes("vista previa"));
  assert.ok(T.grantErrorText("").includes("vista previa"));
});

test("historial: etiquetas legibles y detalle sin texto de preguntas ni respuestas", () => {
  assert.equal(T.ASSISTANT_ACTION_LABEL.assistant_turn, "Pregunta al asistente");
  assert.equal(T.ASSISTANT_ACTION_LABEL.assistant_cleared, "Conversaciones del asistente borradas");
  const states = { answered: "Respondida", fallback: "Respondida por la IA local (respaldo)", error: "Con error", cancelled: "Cancelada", consent_required: "Sin permiso para enviar datos", not_configured: "Sin IA preparada" };
  for (const [status, label] of Object.entries(states)) assert.equal(auditStatusLabel("assistant_turn", status), label);
  assert.equal(auditStatusLabel("assistant_cleared", "cleared"), "Borradas");
  assert.equal(auditStatusLabel("assistant_turn", "otro"), "otro");
  const detail = JSON.stringify({ steps: 2, tools: ["search", "aggregate"], provider: "ollama_cloud", model: "modelo-x", local: false, fallback: null, citations: 1, dropped_citations: 2, withheld: 1, error_code: null });
  const text = T.assistantAuditText("assistant_turn", detail);
  assert.ok(text.includes("2 pasos") && text.includes("buscar documentos, sumas y recuentos") && text.includes("Respondió Ollama cloud · modelo-x"));
  assert.ok(text.includes("1 documento citado") && text.includes("2 referencias descartadas") && text.includes("1 fragmento retenido"));
  const fb = T.assistantAuditText("assistant_turn", JSON.stringify({ steps: 1, tools: [], provider: "ollama", model: "m", local: true, fallback: "timeout", citations: 0, dropped_citations: 0, withheld: 0, error_code: null }));
  assert.ok(fb.startsWith("1 paso ·"));
  assert.ok(fb.includes("Sin herramientas") && fb.includes("contestó la IA local"));
  const failed = T.assistantAuditText("assistant_turn", JSON.stringify({ steps: 0, tools: [], provider: "ollama_cloud", model: "m", local: false, fallback: null, citations: 0, dropped_citations: 0, withheld: 0, error_code: "no_credit" }));
  assert.ok(failed.includes("Sin pasos con la IA") && failed.includes("no tiene crédito") && !failed.includes(T.NOTHING_CHANGED));
  assert.ok(!T.assistantAuditText("assistant_turn", detail, "cancelled").includes("Respondió"));
  assert.ok(T.assistantAuditText("assistant_turn", detail, "answered").includes("Respondió"));
  assert.equal(T.assistantAuditText("assistant_cleared", JSON.stringify({ scope: "all", conversations: 3 })), "Todas las conversaciones. Se borraron 3 conversaciones.");
  assert.equal(T.assistantAuditText("assistant_cleared", JSON.stringify({ scope: "one", conversations: 1 })), "Una conversación. Se borró 1 conversación.");
  // Un detalle que no es JSON se ve tal cual; uno de otra acción no es suyo.
  assert.equal(T.assistantAuditText("assistant_turn", "texto raro"), "texto raro");
  assert.equal(T.assistantAuditText("rule", "{}"), null);
  assert.equal(T.assistantAuditText("assistant_turn", null), null);
});

test("ningún texto nuevo contiene HTML", () => {
  const view = { state: "granted", provider: "ollama_cloud", model: "m", host: "h.test", at: 1_700_000_000 };
  const texts = [
    T.whoAnswered(external),
    T.whoAnswered({ provider: "ollama", model: "m", local: true, fallback: { from: "ollama_cloud", reason: "rate_limited" } }),
    T.droppedCitationsText(2),
    T.withheldText(2),
    ...["unauthorized", "timeout", "rate_limited", "no_credit", "model_unavailable", "request_rejected", "unavailable", "invalid_response", "x"].map(T.assistantErrorText),
    T.assistantCallErrorText("x"),
    T.loadErrorText("x"),
    T.navigatedText("settings"),
    T.clearedText(2),
    T.sentCallTitle({ step: 1, provider: "anthropic", model: "m", host: "h", local: false }),
    T.sentCallWhere({ step: 1, provider: "ollama", model: "m", host: "", local: true }),
    T.sentError("x"),
    T.consentStateText(view),
    T.consentStateText({ ...view, state: "stale" }),
    T.consentStateText({ ...view, state: "missing" }),
    T.assistantDestinationText("ollama_cloud", "m", "h.test"),
    ...T.limitLines({ steps: 6, history_turns: 6, turn_chars: 2000, fragments: 8, fragment_chars: 600, data_chars: 12000 }),
    T.localFallbackLine(true),
    T.assistantLoadError("x"),
    T.grantErrorText("x"),
    ...Object.values(T.NAVIGABLE),
    ...Object.values(T.ASSISTANT_ACTION_LABEL),
    T.assistantAuditText("assistant_turn", JSON.stringify({ steps: 1, tools: ["search"], provider: "ollama_cloud", model: "m", local: false, fallback: null, citations: 1, dropped_citations: 1, withheld: 1, error_code: "timeout" })),
  ];
  for (const text of texts) assert.ok(!/[<>]|&\w+;/.test(text), text);
});
