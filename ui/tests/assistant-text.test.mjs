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

test("el indicador de progreso muestra un texto por herramienta y nunca contenido", () => {
  assert.equal(T.progressText(null), T.ASSISTANT_BUSY);
  assert.equal(T.progressText({ kind: "thinking" }), "Pensando…");
  assert.equal(T.progressText({ kind: "tool", name: "search" }), "Buscando en sus documentos…");
  assert.equal(T.progressText({ kind: "tool", name: "aggregate" }), "Calculando…");
  assert.equal(T.progressText({ kind: "tool", name: "history" }), "Consultando el historial…");
  // Una herramienta que no se conoce (p. ej. un nombre con marcas) no se muestra tal cual.
  assert.equal(T.progressText({ kind: "tool", name: "<b>x</b>" }), T.ASSISTANT_BUSY);
  assert.equal(T.progressText({ kind: "tool", name: "toString" }), T.ASSISTANT_BUSY);
  for (const text of Object.values(T.TOOL_PROGRESS)) assert.ok(!/[<>]|&\w+;/.test(text), text);
});

// ---- Propuestas del asistente (E3, D185-D187) ----

const card = (kind, over = {}) => ({
  kind,
  status: "pending",
  effect: "none",
  summary: { task: { title: "Cambio de horario" } },
  receipt: null,
  ...over,
});

/** Una tarjeta de cada tipo, con datos inventados. */
const CARDS = [
  card("approve", { effect: "send_mail", summary: { type: "email", to: ["cliente.beta@ejemplo.test"] } }),
  card("approve", { effect: "register_invoice", summary: { type: "invoice", supplier: "Alfa SL", invoice_number: "F-2" } }),
  card("reject"),
  card("retry"),
  card("archive"),
  card("confirm_not_sent"),
  card("edit_draft"),
  card("fetch_mail", { summary: { type: "fetch_mail", accounts: 2, limit: 50 } }),
  card("mail_limit", { summary: { type: "mail_limit", current: 50, new: 100 } }),
  card("rename_account", { summary: { type: "rename_account", current_label: "Cuenta A", new_label: "Cuenta B" } }),
  card("sender_preference", { summary: { type: "sender_preference", key: "ventas@ejemplo.test" } }),
  card("accept_suggestion", { effect: "rule_inactive", summary: { type: "accept_suggestion", name: "Facturas de Alfa SL" } }),
  card("discard_suggestion", { summary: { type: "discard_suggestion", name: "Facturas de Alfa SL" } }),
  card("create_rule", { effect: "rule_inactive", summary: { type: "create_rule", name: "Facturas de Beta SL" } }),
  card("edit_rule", { effect: "rule_edited", summary: { type: "edit_rule", name: "Facturas de Alfa SL", was_active: true, stays_active: true } }),
  card("import", { effect: "import", summary: { type: "import", active_register_rules: 0 } }),
];

test("cada tipo de propuesta tiene encabezado, efecto, verbo y nombre de botón propios", () => {
  for (const kind of T.PROPOSAL_KINDS) assert.ok(CARDS.some((c) => c.kind === kind), kind);
  const headings = new Set();
  const verbs = new Set();
  const effects = new Set();
  for (const c of CARDS) {
    const heading = T.proposalHeading(c.kind, c.effect);
    assert.ok(heading.startsWith("Propuesta: "), heading);
    headings.add(heading);
    verbs.add(T.proposalVerb(c.kind, c.effect));
    effects.add(T.proposalEffectText(c.kind, c.effect, c.summary));
    assert.ok(T.proposalButtonName(c).length > 0);
  }
  assert.equal(headings.size, CARDS.length);
  assert.equal(verbs.size, CARDS.length);
  assert.equal(effects.size, CARDS.length);
});

test("ningún verbo de botón es genérico y el nombre accesible empieza por el verbo visible", () => {
  const generic = /^(aceptar|ok|sí|si|confirmar|continuar|aprobar)$/i;
  for (const c of CARDS) {
    const verb = T.proposalVerb(c.kind, c.effect);
    assert.ok(!generic.test(verb), verb);
    assert.ok(T.proposalButtonName(c).startsWith(verb), `${verb} / ${T.proposalButtonName(c)}`);
  }
  assert.equal(T.proposalVerb("approve", "send_mail"), "Enviar");
  assert.equal(T.proposalVerb("approve", "register_invoice"), "Registrar");
  assert.equal(T.proposalVerb("edit_draft", "none"), "Guardar borrador");
  assert.equal(T.proposalVerb("reject", "none"), "Descartar tarea");
});

test("el nombre del botón incluye la acción y su destino", () => {
  assert.equal(T.proposalButtonName(CARDS[0]), "Enviar la respuesta a cliente.beta@ejemplo.test");
  assert.equal(T.proposalButtonName(CARDS[1]), "Registrar la factura Alfa SL F-2");
  assert.ok(T.proposalButtonName(card("reject")).includes("«Cambio de horario»"));
  assert.ok(T.proposalDismissName(CARDS[0]).startsWith("No hacerlo: "));
  assert.ok(T.proposalDismissName(CARDS[0]).includes("enviar esta respuesta"));
});

test("las acciones con efecto externo nombran el efecto", () => {
  assert.ok(T.proposalEffectText("approve", "send_mail").includes("enviará"));
  assert.ok(T.proposalEffectText("approve", "register_invoice").includes("registrará"));
  // Guardar un borrador no envía nada, y lo dice.
  const edit = T.proposalEffectText("edit_draft", "none");
  assert.ok(edit.includes("solo guardará") && edit.includes("No enviará nada"));
  assert.equal(T.proposalTone("approve"), "primary");
  assert.equal(T.proposalTone("reject"), "danger");
  assert.equal(T.proposalTone("mail_limit"), "plain");
});

test("la línea fija dice que nada se ha hecho y que solo cuenta el botón", () => {
  assert.equal(T.PROPOSAL_NOTHING_DONE, "Nada se ha hecho todavía. Solo se ejecuta si usted pulsa el botón.");
});

test("cada estado final tiene su texto", () => {
  assert.equal(T.proposalFinalText(card("approve", { status: "rejected" })), "No se hizo: usted la descartó.");
  assert.equal(T.proposalFinalText(card("approve", { status: "expired" })), "Caducó sin confirmarse. Pídala de nuevo si la necesita.");
  assert.equal(
    T.proposalFinalText(card("approve", { status: "stale" })),
    "La tarea cambió después de preparar esta propuesta. Pídala de nuevo para ver los datos actuales.",
  );
  assert.ok(T.proposalFinalText(card("approve", { status: "failed" })).startsWith("No se hizo nada"));
  assert.equal(T.proposalFinalText(card("approve", { status: "pending" })), null);
  assert.equal(T.proposalFinalText(card("approve", { status: "confirmed" })), null);
  for (const status of ["pending", "executing", "confirmed", "rejected", "expired", "stale", "failed"]) assert.notEqual(T.proposalStatusLabel(status), status);
  assert.equal(T.proposalIsOpen("pending"), true);
  assert.equal(T.proposalIsOpen("executing"), true);
  assert.equal(T.proposalIsOpen("confirmed"), false);
});

test("el recibo dice qué se hizo, cuándo y si ya constaba hecho", () => {
  const confirmed = (kind, effect, result = {}, reconciled = false) =>
    card(kind, { status: "confirmed", effect, receipt: { confirmed_at: Date.UTC(2026, 10, 10, 9, 30), reconciled, result } });
  const sent = T.proposalReceiptLines(confirmed("approve", "send_mail"));
  assert.ok(sent[0].startsWith("Enviado el "), sent[0]);
  assert.ok(sent.some((l) => l.includes("entrega al destinatario no se comprueba")));
  assert.ok(T.proposalReceiptLines(confirmed("approve", "register_invoice"))[0].startsWith("Registrada el "));
  assert.ok(T.proposalReceiptLines(confirmed("edit_draft", "none"))[0].startsWith("Borrador guardado el "));
  assert.ok(T.proposalReceiptLines(confirmed("edit_draft", "none")).some((l) => l.includes("No se ha enviado nada")));
  assert.ok(T.proposalReceiptLines(confirmed("mail_limit", "none", { limit: 100 }))[0].includes("a 100"));
  const again = T.proposalReceiptLines(confirmed("archive", "none", {}, true));
  assert.ok(again.includes("La acción ya constaba como hecha y no se repitió."));
  assert.ok(!T.proposalReceiptLines(confirmed("archive", "none")).some((l) => l.includes("no se repitió")));
  // Sin confirmar no hay recibo.
  assert.deepEqual(T.proposalReceiptLines(card("approve")), []);
});

test("una aprobación que no envía ni registra dice el resultado real y no es un recibo de éxito", () => {
  const notDone = (effect, reason) => card("approve", { status: "failed", effect, receipt: { confirmed_at: Date.UTC(2026, 10, 10, 9, 30), reconciled: false, result: {}, outcome: "not_done", reason } });
  assert.equal(T.proposalFinalText(notDone("send_mail", "returned_to_review")), "No se pudo enviar; la tarea sigue pendiente de revisión.");
  assert.equal(T.proposalFinalText(notDone("send_mail", "action_failed")), "No se pudo enviar; la tarea quedó fallida y se puede reintentar.");
  assert.equal(T.proposalFinalText(notDone("register_invoice", "action_failed")), "No se pudo registrar la factura; la tarea quedó fallida y se puede reintentar.");
  assert.equal(T.proposalFinalText(notDone("register_invoice", "returned_to_review")), "No se pudo registrar la factura; la tarea sigue pendiente de revisión.");
  assert.ok(T.proposalFinalText(notDone("send_mail", "not_completed")).startsWith("No se pudo enviar"));
  // Una fallida sin recibo conserva el texto de siempre, y ninguna de las dos pinta «Enviado».
  assert.ok(T.proposalFinalText(card("approve", { status: "failed" })).startsWith("No se hizo nada"));
  assert.deepEqual(T.proposalReceiptLines(notDone("send_mail", "action_failed")), []);
  const done = card("approve", { status: "confirmed", effect: "send_mail", receipt: { confirmed_at: Date.UTC(2026, 10, 10, 9, 30), reconciled: false, result: {}, outcome: "done", reason: null } });
  assert.ok(T.proposalReceiptLines(done)[0].startsWith("Enviado el "));
});

test("consultar el correo y reintentar avisan de las reglas activas, en singular y plural", () => {
  const one = T.proposalRulesText("fetch_mail", "may_trigger_rules", { active_send_rules: 1, active_register_rules: 0 });
  assert.equal(one, "Tiene 1 regla activa que puede enviar respuestas o registrar facturas sin revisión con el correo nuevo.");
  const many = T.proposalRulesText("fetch_mail", "may_trigger_rules", { active_send_rules: 2, active_register_rules: 1 });
  assert.equal(many, "Tiene 3 reglas activas que pueden enviar respuestas o registrar facturas sin revisión con el correo nuevo.");
  assert.ok(T.proposalRulesText("retry", "may_trigger_rules", { active_send_rules: 0, active_register_rules: 2 }).endsWith("con la tarea reintentada."));
  assert.equal(T.proposalRulesText("fetch_mail", "none", { active_send_rules: 0, active_register_rules: 0 }), null);
  assert.equal(T.proposalRulesText("approve", "send_mail", {}), null);
  const audit = T.assistantAuditText("assistant_proposal", JSON.stringify({ kind: "fetch_mail", task_id: null, effect: "may_trigger_rules", code: null }), "created");
  assert.ok(audit.includes("puede activar reglas"));
  const failed = T.assistantAuditText("assistant_proposal", JSON.stringify({ kind: "approve", task_id: 4, effect: "send_mail", code: "returned_to_review" }), "failed");
  assert.ok(failed.includes("no se hizo: la tarea volvió a revisión"));
});

test("el mensaje de error dice qué pasó con la propuesta", () => {
  assert.equal(
    T.proposalErrorText("hay importes que no se leen sin duda.", "pending"),
    "Hay importes que no se leen sin duda. La propuesta sigue pendiente: no se ha hecho nada.",
  );
  assert.ok(T.proposalErrorText("sin conexión", "executing").includes("lo comprobará antes de repetirlo"));
  assert.equal(T.proposalErrorText("la propuesta caducó; pídala de nuevo", "expired"), "La propuesta caducó; pídala de nuevo.");
  assert.equal(T.proposalErrorText("", null), "No se pudo confirmar.");
  assert.ok(T.proposalDismissErrorText("x").startsWith("No se pudo descartar"));
});

test("avisos del correo: destinatario automático e intento sin confirmar", () => {
  assert.equal(T.recipientSignalsText(null), null);
  assert.equal(T.recipientSignalsText([]), null);
  assert.ok(T.recipientSignalsText(["noreply_address"]).includes("no admite respuestas"));
  assert.ok(T.recipientSignalsText(["algo_nuevo"]).includes("dirección automática"));
  assert.ok(T.emailIssueText("unconfirmed_attempt", 2).includes("versión 2"));
  assert.ok(T.emailIssueText("no_recipient").includes("no tiene destinatario"));
  assert.equal(T.emailIssueText("otra"), null);
});

test("detecta las marcas de datos protegidos sin repetirlas y en orden", () => {
  assert.deepEqual(T.maskedMarksIn("El IBAN es [IBAN] y el teléfono [TELÉFONO]; otra vez [IBAN]."), ["[IBAN]", "[TELÉFONO]"]);
  assert.deepEqual(T.maskedMarksIn("Escriba a [CORREO] o vea [URL]."), ["[CORREO]", "[URL]"]);
  assert.deepEqual(T.maskedMarksIn("Sin datos protegidos. [otra cosa] [iban]"), []);
  assert.deepEqual(T.maskedMarksIn(""), []);
  assert.deepEqual(T.maskedMarksIn(undefined), []);
  for (const mark of ["[ID]", "[CLAVE]", "[TARJETA]"]) assert.deepEqual(T.maskedMarksIn(`dato ${mark}`), [mark]);
  assert.equal(T.maskedNote([]), null);
  const note = T.maskedNote(["[IBAN]", "[TELÉFONO]"]);
  assert.ok(note.includes("[IBAN] y [TELÉFONO]") && note.includes("no se enviaron a la IA") && note.includes("abriendo el documento"));
});

test("historial: etiquetas de las propuestas y marca «Desde el chat»", () => {
  assert.equal(T.ASSISTANT_ACTION_LABEL.assistant_proposal, "Propuesta del asistente");
  for (const status of ["created", "confirmed", "rejected", "expired", "stale", "failed"]) {
    const label = auditStatusLabel("assistant_proposal", status);
    assert.notEqual(label, status);
    assert.ok(!/[<>]/.test(label));
  }
  assert.equal(T.CHAT_ORIGIN_LABEL, "Desde el chat");
  assert.equal(T.isFromChat("chat"), true);
  assert.equal(T.isFromChat(null), false);
  assert.equal(T.isFromChat(undefined), false);
  const detail = T.assistantAuditText("assistant_proposal", JSON.stringify({ kind: "approve", task_id: 4, effect: "send_mail", code: null }), "confirmed");
  assert.equal(detail, "Enviar esta respuesta · envía un correo");
  const reconciled = T.assistantAuditText("assistant_proposal", JSON.stringify({ kind: "archive", task_id: 4, effect: "none", code: "reconciled" }), "confirmed");
  assert.ok(reconciled.includes("ya constaba hecha"));
});

test("los textos de las propuestas no contienen HTML", () => {
  const texts = [
    T.PROPOSAL_NOTHING_DONE,
    T.CARD_DISMISS,
    T.PROPOSAL_EXECUTING,
    T.PROPOSAL_OPEN_TASK,
    T.PROPOSAL_STUCK_TEXT,
    T.AMOUNTS_CHECKED_LABEL,
    T.AMOUNTS_REQUIRED_HINT,
    T.BODY_LABEL,
    T.CURRENT_TEXT_LABEL,
    T.NEW_TEXT_LABEL,
    T.PROPOSAL_LIVE_LABEL,
    T.CHAT_ORIGIN_LABEL,
    T.maskedNote(T.MASK_MARKS.slice()),
    ...CARDS.flatMap((c) => [T.proposalHeading(c.kind, c.effect), T.proposalVerb(c.kind, c.effect), T.proposalEffectText(c.kind, c.effect, c.summary), T.proposalButtonName(c), T.proposalDismissName(c)]),
    ...["rejected", "expired", "stale", "failed"].map((status) => T.proposalFinalText(card("approve", { status }))),
    ...["pending", "executing", "confirmed", "rejected", "expired", "stale", "failed"].map(T.proposalStatusLabel),
    T.proposalErrorText("x", "pending"),
    T.proposalRulesText("fetch_mail", "may_trigger_rules", { active_send_rules: 2, active_register_rules: 0 }),
    ...["action_failed", "returned_to_review", "not_completed"].map((reason) => T.proposalFinalText(card("approve", { status: "failed", effect: "send_mail", receipt: { confirmed_at: 0, reconciled: false, result: {}, outcome: "not_done", reason } }))),
    T.recipientSignalsText(["noreply_address"]),
    T.emailIssueText("unconfirmed_attempt", 1),
  ];
  for (const text of texts) assert.ok(!/[<>]|&\w+;/.test(text), text);
});


// ---- Reglas, sugerencias e importación (E4, D191) ----

test("los textos de las cinco propuestas nuevas son los acordados y nunca prometen activar la regla", () => {
  assert.equal(T.proposalHeading("create_rule", "rule_inactive"), "Propuesta: crear una regla (quedará inactiva)");
  assert.equal(T.proposalVerb("create_rule", "rule_inactive"), "Crear regla");
  assert.equal(T.proposalVerb("accept_suggestion", "rule_inactive"), "Aceptar sugerencia");
  assert.equal(T.proposalVerb("discard_suggestion", "none"), "No volver a proponer");
  assert.equal(T.proposalVerb("edit_rule", "rule_edited"), "Guardar cambios");
  assert.equal(T.proposalVerb("import", "import"), "Elegir archivos…");
  for (const kind of ["accept_suggestion", "create_rule"]) {
    const text = T.proposalEffectText(kind, "rule_inactive", {});
    assert.ok(text.includes("INACTIVA") && text.includes("Reglas"), text);
  }
  assert.ok(T.proposalEffectText("discard_suggestion", "none", {}).includes("No se crea ninguna regla"));
  for (const c of CARDS.filter((c) => ["accept_suggestion", "create_rule", "edit_rule"].includes(c.kind))) {
    assert.ok(!/activará|activa la regla/i.test(T.proposalEffectText(c.kind, c.effect, c.summary)));
  }
  assert.equal(T.proposalTone("create_rule"), "plain");
  assert.equal(T.INACTIVE_NOTICE, "La regla quedará inactiva: no actuará hasta que usted la active en Reglas.");
});

test("editar dice si la regla sigue activa solo cuando el cambio la estrecha", () => {
  assert.ok(T.proposalEffectText("edit_rule", "rule_edited", { was_active: true, stays_active: true }).includes("sigue activa porque el cambio la estrecha"));
  assert.ok(T.proposalEffectText("edit_rule", "rule_edited", { was_active: true, stays_active: false }).includes("quedará INACTIVA"));
  assert.ok(T.proposalEffectText("edit_rule", "rule_edited", { was_active: false, stays_active: false }).includes("sigue inactiva"));
  assert.ok(T.editedRuleStateText({ was_active: true, stays_active: false }).includes("quedará inactiva"));
  assert.ok(T.editedRuleStateText({ was_active: true, stays_active: true }).includes("seguirá activa"));
  assert.ok(T.editedRuleStateText({ was_active: false }).includes("seguirá así"));
});

test("las condiciones exactas, los casos cubiertos y los solapamientos se dicen con las cifras de la tarjeta", () => {
  assert.deepEqual(T.ruleConditionLines({ supplier: "Alfa SL", max_total: "500.00" }), [
    ["Proveedor", "Alfa SL"],
    ["Importe máximo (EUR)", "500.00"],
  ]);
  assert.deepEqual(T.ruleConditionLines({ category: "pedido", sender_domain: "gamma.test" }, (c) => c.toUpperCase()), [
    ["Categoría", "PEDIDO"],
    ["Dominio del remitente", "gamma.test"],
  ]);
  assert.deepEqual(T.ruleConditionLines(null), []);
  assert.equal(T.proposalCoverageText({ covered: 4, would_automate: 3 }), "Habría cubierto 4 casos anteriores (3 se habrían automatizado).");
  assert.equal(T.proposalCoverageText({ covered: 1, would_automate: 1 }), "Habría cubierto 1 caso anterior (1 se habría automatizado).");
  assert.equal(T.proposalCoverageText({ covered: 0, would_automate: 0 }), "No habría cubierto ningún caso anterior.");
  assert.equal(T.proposalCoverageText(undefined), null);
  assert.deepEqual(T.proposalOverlapLines([{ kind: "overlap", rules: [{ id: "a", name: "Pedidos de Ana" }] }]), ["Se solapa con «Pedidos de Ana»: al activarla, los casos que cubran las dos irían a revisión."]);
  assert.deepEqual(T.proposalOverlapLines([]), []);
  assert.deepEqual(T.proposalOverlapLines(null), []);
  assert.equal(T.suggestionEvidenceText({ count: 3, threshold: 3 }), "Sale de 3 observaciones (umbral 3).");
  assert.equal(T.suggestionEvidenceText(undefined), null);
  assert.equal(T.ruleActionText("register_invoice"), "Registrar facturas sin revisión");
  assert.ok(T.ruleActionText("send_draft").startsWith("Enviar"));
});

test("tras confirmar, el recibo dice que la regla está inactiva y la importación da sus recuentos", () => {
  const done = (kind, effect, result, over = {}) => ({
    ...card(kind, { effect, status: "confirmed" }),
    receipt: { confirmed_at: 1_760_000_000_000, reconciled: false, result, outcome: "done", reason: null, rule_id: "chat-7", ...over },
  });
  const created = T.proposalReceiptLines(done("create_rule", "rule_inactive", { rule_id: "chat-7", active: false }));
  assert.ok(created[0].startsWith("Regla creada (inactiva) el "), created[0]);
  assert.ok(created.some((l) => l.includes("Está inactiva")));
  assert.ok(T.proposalReceiptLines(done("accept_suggestion", "rule_inactive", { active: false }))[0].startsWith("Regla creada (inactiva)"));
  assert.ok(T.proposalReceiptLines(done("discard_suggestion", "none", {}))[0].startsWith("Sugerencia descartada"));
  assert.ok(T.proposalReceiptLines(done("edit_rule", "rule_edited", { deactivated_by_edit: true, active: false }))[0].startsWith("Regla editada: ha quedado inactiva"));
  assert.ok(T.proposalReceiptLines(done("edit_rule", "rule_edited", { active: true }))[0].startsWith("Regla editada el "));
  const imported = T.proposalReceiptLines(done("import", "import", { imported: 2, errors: 1 }));
  assert.ok(imported[0].startsWith("2 importados, 1 con error el "), imported[0]);
  assert.equal(T.importCountsText({ imported: 1, errors: 0 }), "1 importado, 0 con error");
  assert.equal(T.PROPOSAL_OPEN_RULE, "Abrir en Reglas");
});

test("cerrar el diálogo de importar es un aviso discreto y la tarjeta sigue pendiente", () => {
  assert.equal(T.IMPORT_CANCELLED_CODE, "import_cancelled");
  assert.equal(T.isImportCancelled("import_cancelled"), true);
  assert.equal(T.isImportCancelled(" import_cancelled "), true);
  assert.equal(T.isImportCancelled("la propuesta caducó; pídala de nuevo"), false);
  assert.equal(T.isImportCancelled(null), false);
  assert.ok(T.IMPORT_CANCELLED_NOTE.includes("sigue pendiente"));
  assert.ok(T.proposalFinalText(card("import", { status: "failed", effect: "import", receipt: { outcome: "not_done", reason: "nothing_imported" } })).startsWith("No se importó ningún archivo"));
  assert.ok(T.proposalFinalText(card("create_rule", { status: "stale" })).includes("regla o la sugerencia cambió"));
  assert.ok(T.proposalFinalText(card("approve", { status: "stale" })).includes("La tarea cambió"));
});

test("importar avisa de las reglas activas que registran y el historial describe las propuestas nuevas", () => {
  assert.equal(T.proposalRulesText("import", "import", { active_register_rules: 0 }), null);
  assert.ok(T.proposalRulesText("import", "import", { active_register_rules: 2 }).includes("2 reglas activas que pueden registrar facturas sin revisión"));
  assert.ok(T.proposalRulesText("import", "import", { active_register_rules: 1 }).includes("1 regla activa que puede registrar"));
  const audit = (kind, effect) => T.assistantAuditText("assistant_proposal", JSON.stringify({ kind, task_id: null, effect, code: null }), "created");
  assert.ok(audit("create_rule", "rule_inactive").includes("crea una regla inactiva"));
  assert.ok(audit("edit_rule", "rule_edited").includes("cambia las condiciones de una regla"));
  assert.ok(audit("import", "import").includes("importa documentos"));
  assert.ok(T.assistantAuditText("assistant_proposal", JSON.stringify({ kind: "import", effect: "import", code: "import_interrupted" }), "failed").includes("no se repite sola"));
  assert.equal(T.PROPOSAL_KINDS.length, 15);
});

test("la tarjeta de una regla muestra también las condiciones que no son de las cinco conocidas", () => {
  // `currency` (y cualquier otra clave) no se esconde: ni al crear, ni al aceptar, ni en el antes y el después.
  const lines = T.ruleConditionLines({ supplier: "Alfa SL", max_total: "500.00", currency: "EUR", zeta: 3, alfa: { a: 1 } });
  assert.deepEqual(lines, [
    ["Proveedor", "Alfa SL"],
    ["Importe máximo (EUR)", "500.00"],
    ["alfa", '{"a":1}'],
    ["Moneda", "EUR"],
    ["zeta", "3"],
  ]);
  const before = T.ruleConditionLines({ supplier: "Alfa SL", currency: "EUR" });
  const after = T.ruleConditionLines({ supplier: "Alfa SL", currency: "USD" });
  assert.deepEqual(before.map((l) => l[0]), after.map((l) => l[0]));
  assert.notDeepEqual(before, after);
  assert.deepEqual(T.ruleConditionLines({ supplier: "Alfa SL", currency: null }), [["Proveedor", "Alfa SL"]]);
  assert.deepEqual(T.ruleConditionLines({ max_total: 5 }), [["Importe máximo (EUR)", "5"]]);
});

test("D264: la nota de enlaces retirados distingue singular y plural y no sale con cero", () => {
  assert.equal(T.linksRemovedText(0), null);
  assert.equal(T.linksRemovedText(undefined), null);
  assert.equal(T.linksRemovedText(null), null);
  assert.equal(T.linksRemovedText(-2), null);
  assert.equal(T.linksRemovedText(1), "Se retiró 1 enlace de la respuesta.");
  assert.equal(T.linksRemovedText(3), "Se retiraron 3 enlaces de la respuesta.");
});

test("D266: el aviso de datos de otros documentos lee lo que la aplicación halló, sin fiarse de la forma", () => {
  const summary = {
    other_documents: [
      { kind: "invoice_number", value: "F-2026-101" },
      { kind: "file_name", value: "factura-101.txt" },
      { kind: "party", value: "Suministros Ficticios SL" },
      { kind: "party", value: "   " },
      null,
      { kind: "party" },
    ],
    other_documents_total: 7,
  };
  const found = T.otherDocumentsOf(summary);
  assert.equal(found.items.length, 3);
  assert.equal(found.total, 7);
  assert.deepEqual(T.otherDocumentsOf({}), { items: [], total: 0 });
  assert.deepEqual(T.otherDocumentsOf(null), { items: [], total: 0 });
  assert.deepEqual(T.otherDocumentsOf({ other_documents: [], other_documents_total: 0 }), { items: [], total: 0 });
  // Sin total, cuenta los que enseña; el total nunca es menor que lo enseñado.
  assert.equal(T.otherDocumentsOf({ other_documents: [{ kind: "party", value: "Alfa SL" }] }).total, 1);
  assert.equal(T.otherDocumentsOf({ other_documents: [{ kind: "party", value: "Alfa SL" }], other_documents_total: 0 }).total, 1);
  assert.equal(T.otherDocumentKindText("invoice_number"), "Número de factura");
  assert.equal(T.otherDocumentKindText("file_name"), "Nombre de archivo");
  assert.equal(T.otherDocumentKindText("party"), "Proveedor o remitente");
  assert.equal(T.otherDocumentKindText("otra"), "Dato");
});

test("D266: la frase del aviso dice cuántos hay y cuántos se enseñan, y la casilla pide revisarlos", () => {
  assert.ok(T.otherDocumentsIntro(1, 1).startsWith("La aplicación halló 1 dato de otro documento que no está en este correo"));
  const many = T.otherDocumentsIntro(7, 5);
  assert.ok(many.includes("7 datos de otros documentos") && many.includes("(se enseñan 5)"));
  assert.ok(!T.otherDocumentsIntro(3, 3).includes("se enseñan"));
  assert.equal(T.OTHER_DOCUMENTS_CHECKED_LABEL, "He revisado que estos datos deben ir en la respuesta");
  assert.ok(T.OTHER_DOCUMENTS_REQUIRED_HINT.includes("marque antes"));
  assert.ok(T.OTHER_DOCUMENTS_TITLE.includes("otros documentos"));
});

test("D267: el aviso de documento no nombrado solo sale si la aplicación lo dice expresamente", () => {
  assert.equal(T.proposalIsUnnamed({ named_by_person: false }), true);
  assert.equal(T.proposalIsUnnamed({ named_by_person: true }), false);
  // Una propuesta guardada antes de D267 no lleva el dato: no avisa de lo que no sabe.
  assert.equal(T.proposalIsUnnamed({}), false);
  assert.equal(T.proposalIsUnnamed(null), false);
  assert.equal(T.UNNAMED_TITLE, "Usted no nombró este documento");
  assert.ok(T.UNNAMED_REQUIRED_HINT.includes("marque antes"));
});

test("D268: los valores de regla que no aparecen en sus documentos se rotulan y la lista vacía no avisa", () => {
  assert.deepEqual(T.unknownRuleValues({ unknown_values: ["supplier", "sender_domain"] }), ["Proveedor", "Dominio del remitente"]);
  assert.deepEqual(T.unknownRuleValues({ unknown_values: [] }), []);
  assert.deepEqual(T.unknownRuleValues({}), []);
  assert.deepEqual(T.unknownRuleValues({ unknown_values: [3, "sender", null] }), ["Remitente"]);
  assert.equal(T.UNKNOWN_VALUES_TITLE, "No aparece en sus documentos");
  assert.ok(T.UNKNOWN_VALUES_REQUIRED_HINT.includes("marque antes"));
});

test("D269: solo uncited y none llevan aviso de respaldo, con los textos fijos", () => {
  assert.equal(T.groundingNotice("uncited"), "Esta respuesta no cita ninguno de sus documentos");
  assert.equal(T.groundingNotice("none"), "Para esta respuesta no se consultó ningún documento ni dato de NestAdmin");
  assert.equal(T.UNCITED_NOTICE, T.groundingNotice("uncited"));
  assert.equal(T.NO_CONSULT_NOTICE, T.groundingNotice("none"));
  for (const quiet of ["documents", "data", "no_results", null, undefined, "", "otro", 3, {}]) {
    assert.equal(T.groundingNotice(quiet), null, String(quiet));
    assert.equal(T.groundingOffersWords(quiet), false, String(quiet));
  }
  assert.equal(T.groundingOffersWords("uncited"), true);
  assert.equal(T.groundingOffersWords("none"), true);
});

test("D269: un estado desconocido o un turno anterior a D269 no inventa un aviso", () => {
  for (const g of ["documents", "data", "no_results", "uncited", "none"]) assert.equal(T.knownGrounding(g), g);
  for (const g of ["", "Documents", "sin_respaldo", null, undefined, 0, [], {}]) assert.equal(T.knownGrounding(g), null);
});

test("D269: las cifras sin origen dicen cuántas son y las enumeran, hasta cinco", () => {
  assert.equal(T.unsourcedNotice(0, []), null);
  assert.equal(T.unsourcedNotice(undefined, undefined), null);
  assert.equal(T.unsourcedNotice(null, null), null);
  assert.equal(T.unsourcedNotice(1, ["9:00"]), "Contiene 1 dato que no sale de sus documentos: 9:00.");
  assert.equal(T.unsourcedNotice(2, ["9:00", "14:00"]), "Contiene 2 datos que no salen de sus documentos: 9:00, 14:00.");
  assert.equal(
    T.unsourcedNotice(7, ["9:00", "14:00", "350,00 EUR", "F-2026-999", "15/04/2026"]),
    "Contiene 7 datos que no salen de sus documentos: 9:00, 14:00, 350,00 EUR, F-2026-999, 15/04/2026 y 2 más.",
  );
  // Nunca más de cinco en la lista, aunque lleguen más.
  const six = ["1", "2", "3", "4", "5", "6"].map((n) => `${n}0:00`);
  assert.equal(T.unsourcedNotice(6, six), "Contiene 6 datos que no salen de sus documentos: 10:00, 20:00, 30:00, 40:00, 50:00 y 1 más.");
  // El recuento sin lista (un turno guardado con el total y sin los textos) sigue avisando.
  assert.equal(T.unsourcedNotice(3, []), "Contiene 3 datos que no salen de sus documentos.");
  // Valores raros: se ignoran los que no son texto y se recortan los largos.
  assert.equal(T.unsourcedNotice(2, ["9:00", 4, null, "  "]), "Contiene 2 datos que no salen de sus documentos: 9:00 y 1 más.");
  assert.ok(T.unsourcedNotice(1, ["x".repeat(200)]).length < 130);
  assert.equal(T.unsourcedNotice(Number.NaN, []), null);
  assert.equal(T.unsourcedNotice(-4, []), null);
});

test("D269: el aviso y la lista de cifras no usan el mismo texto que las demás notas de la respuesta", () => {
  const notes = [T.linksRemovedText(1), T.droppedCitationsText(1), T.withheldText(1)];
  for (const notice of [T.groundingNotice("uncited"), T.groundingNotice("none"), T.unsourcedNotice(1, ["9:00"])]) {
    assert.ok(notice && !notes.includes(notice));
    // Texto llano en español de usted, sin marcado ni enlaces.
    assert.ok(!/[<>`*_]|https?:/.test(notice), notice);
  }
});
