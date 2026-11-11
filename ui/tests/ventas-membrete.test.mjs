// Textos y validaciones del membrete PDF y del botón «Ver PDF» (E4a, D222, D223; D197). Reglas
// puras, sin DOM. Todos los ejemplos son inventados.
import assert from "node:assert/strict";
import { test } from "node:test";
import {
  LETTERHEAD_HELP, LETTERHEAD_MARGINS_HELP, LETTERHEAD_MAX_BYTES, LETTERHEAD_REMOVE_CONFIRM, LETTERHEAD_SCOPE, LETTERHEAD_WARNING, MARGIN_FIELDS,
  VIEW_PDF_LABEL, canViewPdf, letterheadFileProblem, marginsToText, parseMargins, salesErrorText,
} from "../src/ventas-text.ts";

const ok = { top_mm: "35", right_mm: "20", bottom_mm: "25", left_mm: "20" };

test("el archivo del membrete se comprueba antes de enviarlo", () => {
  const pdf = { name: "papel-carta.pdf", size: 120_000, type: "application/pdf" };
  assert.equal(letterheadFileProblem(pdf), null);
  assert.equal(letterheadFileProblem({ ...pdf, name: "PAPEL.PDF", type: "" }), null, "por la extensión si el sistema no da el tipo");
  assert.equal(letterheadFileProblem(undefined), "Elija primero un archivo PDF.");
  assert.equal(letterheadFileProblem({ name: "logo.png", size: 10, type: "image/png" }), "El membrete debe ser un archivo PDF.");
  assert.equal(letterheadFileProblem({ ...pdf, size: 0 }), "El archivo está vacío.");
  assert.equal(letterheadFileProblem({ ...pdf, size: LETTERHEAD_MAX_BYTES }), null);
  assert.equal(letterheadFileProblem({ ...pdf, size: LETTERHEAD_MAX_BYTES + 1 }), "El PDF supera los 5 MB.");
});

test("los cuatro márgenes son milímetros enteros de 0 a 100", () => {
  const parsed = parseMargins(ok);
  assert.deepEqual(parsed, { ok: true, margins: { top_mm: 35, right_mm: 20, bottom_mm: 25, left_mm: 20 } });
  assert.deepEqual(parseMargins({ top_mm: " 0 ", right_mm: "100", bottom_mm: "0", left_mm: "0" }), { ok: true, margins: { top_mm: 0, right_mm: 100, bottom_mm: 0, left_mm: 0 } });
  for (const bad of ["", "abc", "12,5", "12.5", "-1", "1e2", "1000", "  "]) {
    const result = parseMargins({ ...ok, top_mm: bad });
    assert.equal(result.ok, false, bad);
    assert.match(result.problems[0], /^Margen superior \(mm\): escriba milímetros enteros, de 0 a 100\.$/, bad);
  }
  const over = parseMargins({ ...ok, left_mm: "101" });
  assert.deepEqual(over, { ok: false, problems: ["Margen izquierdo (mm): el máximo es 100 mm."] });
  // Un problema por campo, en el orden de la pantalla.
  const many = parseMargins({ top_mm: "x", right_mm: "y", bottom_mm: "25", left_mm: "z" });
  assert.equal(many.ok, false);
  assert.deepEqual(many.problems.map((p) => p.split(":")[0]), ["Margen superior (mm)", "Margen derecho (mm)", "Margen izquierdo (mm)"]);
});

test("el área útil no baja de 100 x 150 mm, como en el backend", () => {
  assert.deepEqual(parseMargins({ ...ok, left_mm: "55", right_mm: "55" }), { ok: true, margins: { top_mm: 35, right_mm: 55, bottom_mm: 25, left_mm: 55 } });
  const narrow = parseMargins({ ...ok, left_mm: "56", right_mm: "55" });
  assert.deepEqual(narrow, { ok: false, problems: ["Los márgenes izquierdo y derecho dejan menos de 100 mm de ancho útil."] });
  assert.equal(parseMargins({ ...ok, top_mm: "100", bottom_mm: "47" }).ok, true);
  const short = parseMargins({ ...ok, top_mm: "100", bottom_mm: "48" });
  assert.deepEqual(short, { ok: false, problems: ["Los márgenes superior e inferior dejan menos de 150 mm de alto útil."] });
  const both = parseMargins({ top_mm: "100", right_mm: "100", bottom_mm: "100", left_mm: "100" });
  assert.equal(both.ok, false);
  assert.equal(both.problems.length, 2);
});

test("los márgenes guardados se muestran como texto y vuelven iguales", () => {
  const saved = { top_mm: 40, right_mm: 15, bottom_mm: 30, left_mm: 18 };
  const text = marginsToText(saved);
  assert.deepEqual(text, { top_mm: "40", right_mm: "15", bottom_mm: "30", left_mm: "18" });
  assert.deepEqual(parseMargins(text), { ok: true, margins: saved });
  assert.deepEqual(MARGIN_FIELDS.map(([key]) => key), ["top_mm", "right_mm", "bottom_mm", "left_mm"]);
  assert.deepEqual(MARGIN_FIELDS.map(([, label]) => label), ["Margen superior (mm)", "Margen derecho (mm)", "Margen inferior (mm)", "Margen izquierdo (mm)"]);
});

test("«Ver PDF» solo existe para un documento emitido", () => {
  assert.equal(VIEW_PDF_LABEL, "Ver PDF");
  assert.equal(canViewPdf({ status: "draft", number: null }), false);
  assert.equal(canViewPdf({ status: "draft", number: "P-2026-0001" }), false, "una revisión en borrador tampoco");
  for (const status of ["issued", "sent", "accepted", "rejected", "expired", "converted", "delivered", "invoiced", "voided"]) {
    assert.equal(canViewPdf({ status, number: "F-2026-0001" }), true, status);
  }
});

test("la explicación del membrete dice lo que no hace y a quién afecta", () => {
  assert.match(LETTERHEAD_HELP, /una o dos páginas A4 verticales, sin contraseña y de hasta 5 MB/);
  assert.match(LETTERHEAD_HELP, /no se dibuja el logotipo/);
  assert.match(LETTERHEAD_HELP, /debe ser clara/);
  assert.match(LETTERHEAD_WARNING, /no oculta ni sustituye ningún dato obligatorio/);
  assert.match(LETTERHEAD_WARNING, /VERI\*FACTU/);
  assert.match(LETTERHEAD_MARGINS_HELP, /área útil/);
  assert.match(LETTERHEAD_SCOPE, /solo afecta a los documentos que se emitan después/);
  assert.match(LETTERHEAD_REMOVE_CONFIRM, /Los documentos ya emitidos conservan el suyo/);
  assert.match(LETTERHEAD_REMOVE_CONFIRM, /¿Quitarlo\?$/);
});

test("los errores del PDF y del membrete llegan con su encabezado y el motivo del backend", () => {
  assert.equal(
    salesErrorText("[invalid_input] datos no válidos: letterhead: el archivo no es un PDF válido o está dañado"),
    "Datos no válidos: el archivo no es un PDF válido o está dañado",
  );
  assert.equal(salesErrorText("[pdf_unavailable] PDFium no disponible: no se pudo cargar pdfium.dll"), "PDFium no está disponible: PDFium no disponible: no se pudo cargar pdfium.dll");
  assert.equal(salesErrorText("[pdf_failed] no se pudo guardar el PDF"), "No se pudo generar el PDF: no se pudo guardar el PDF");
  assert.equal(salesErrorText("[conflict] solo se genera el PDF de un documento emitido"), "Conflicto con otro documento: solo se genera el PDF de un documento emitido");
});
