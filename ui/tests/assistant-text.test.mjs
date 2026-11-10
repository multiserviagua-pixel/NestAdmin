// E1, D181: los textos del chat describen una búsqueda por palabras hecha en el equipo, no una respuesta de la IA.
// Todos los ejemplos son inventados.
import assert from "node:assert/strict";
import { test } from "node:test";
import * as T from "../src/assistant-text.ts";

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

test("el aviso fijo dice que no es la IA y no promete fecha ni activación", () => {
  assert.ok(T.SEARCH_NOTICE.includes("búsqueda por palabras hecha en este equipo"));
  assert.ok(T.SEARCH_NOTICE.includes("no una respuesta de la IA"));
  assert.ok(T.SEARCH_NOTICE.includes("llegará en una versión posterior"));
  assert.ok(!/activ|Configuración|\d{4}/.test(T.SEARCH_NOTICE));
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
