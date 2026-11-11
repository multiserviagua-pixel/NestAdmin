// Recorrido T15b sobre `tauri dev`: PDF de venta, membrete (D222-D228) y regeneración.
//
// Requisitos: la ventana de `tauri dev` abierta con una carpeta de datos NUEVA, un
// verifactu-productor.json sintético en ella, NESTADMIN_PDFIUM_DIR apuntando a PDFium y
// WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9333.
//
// Uso: node recorrido.mjs <carpeta de membretes> <carpeta de salida>
// Escribe <salida>/resultados.json, capturas <salida>/pantalla-*.jpg y copia los PDF a <salida>/pdf.
import { copyFileSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { createHash } from "node:crypto";
import { basename, join } from "node:path";

const [membretes, salida] = process.argv.slice(2);
mkdirSync(join(salida, "pdf"), { recursive: true });
const targets = await (await fetch("http://127.0.0.1:9333/json")).json();
const page = targets.find((t) => t.type === "page" && t.url.startsWith("http://localhost:5173"));
if (!page) throw new Error("no está abierta la ventana de tauri dev");
const ws = new WebSocket(page.webSocketDebuggerUrl);
await new Promise((r) => ws.addEventListener("open", r, { once: true }));
let n = 0;
const waiting = new Map();
ws.addEventListener("message", (e) => {
  const m = JSON.parse(e.data);
  if (m.id && waiting.has(m.id)) (waiting.get(m.id)(m), waiting.delete(m.id));
});
const send = (method, params = {}) => new Promise((r) => { const id = ++n; waiting.set(id, r); ws.send(JSON.stringify({ id, method, params })); });
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const PRE = `const inv = window.__TAURI_INTERNALS__.invoke; const main = () => document.querySelector('main');
  const btn = (t) => [...document.querySelectorAll('button, a')].find(b => b.textContent.trim() === t);`;
async function ev(expr) {
  const m = await send("Runtime.evaluate", { expression: `(async () => { ${PRE} ${expr} })()`, awaitPromise: true, returnByValue: true });
  if (m.result?.exceptionDetails) throw new Error(m.result.exceptionDetails.exception?.description ?? m.result.exceptionDetails.text);
  return m.result?.result?.value;
}
// Invoca un comando; devuelve el valor o «ERROR <texto>».
const call = (cmd, args) => ev(`return await inv(${JSON.stringify(cmd)}, ${JSON.stringify(args ?? {})}).catch(e => 'ERROR ' + e);`);
// Avisos visibles de la ventana, sin los dos fijos de este entorno (OCR sin empaquetar e IA sin configurar).
const avisos = () => ev(`return [...document.querySelectorAll('[role=alert],[role=status],.notice,.toast,.error')]
  .map(x => x.textContent.trim().replace(/×$/, '')).filter(t => t && !/componentes de OCR|configurar la IA/.test(t));`);
const pulsar = async (texto) => { const ok = await ev(`const b = btn(${JSON.stringify(texto)}); if (!b) return false; b.click(); return true;`); await sleep(900); return ok; };
const empresa = () => ev(`const c = await inv('sales_company'); return { membrete: c.has_letterhead, margenes: c.letterhead_margins };`);
async function captura(nombre) {
  await ev(`window.scrollTo(0, 0); document.querySelector('main')?.scrollTo?.(0, 0);`);
  const shot = await send("Page.captureScreenshot", { format: "jpeg", quality: 70 });
  writeFileSync(join(salida, `pantalla-${nombre}.jpg`), Buffer.from(shot.result.data, "base64"));
}
async function verSeccionMembrete(nombre) {
  await ev(`document.getElementById('ventas-letterhead-file')?.closest('div')?.parentElement?.parentElement?.scrollIntoView({ block: 'start' });`);
  await sleep(300);
  const shot = await send("Page.captureScreenshot", { format: "jpeg", quality: 70 });
  writeFileSync(join(salida, `pantalla-${nombre}.jpg`), Buffer.from(shot.result.data, "base64"));
}
async function subir(archivo) {
  const obj = await send("Runtime.evaluate", { expression: "document.getElementById('ventas-letterhead-file')" });
  await send("DOM.setFileInputFiles", { files: [join(membretes, archivo)], objectId: obj.result.result.objectId });
  await sleep(200);
  await pulsar("Guardar membrete");
  await sleep(700);
  return { avisos: await avisos(), empresa: await empresa() };
}
async function margenes(v) {
  await ev(`const v = ${JSON.stringify(v)}; for (const k of Object.keys(v)) {
    const el = document.querySelector('[data-focus-key*="margin:' + k + '"]'); el.value = v[k]; el.dispatchEvent(new Event('input', { bubbles: true })); }`);
  await sleep(300);
  await pulsar("Guardar márgenes");
  await sleep(700);
  return { avisos: await avisos(), empresa: await empresa() };
}
const M = (top_mm, right_mm, bottom_mm, left_mm) => ({ top_mm: String(top_mm), right_mm: String(right_mm), bottom_mm: String(bottom_mm), left_mm: String(left_mm) });
// El descuento viaja como fracción (0,10 = 10 %); la ventana lo convierte desde el porcentaje.
const linea = (d, q, p, iva = "21", dto = null) => ({ description: d, quantity: q, unit_price: p, tax_percent: iva, discount: dto });
async function emitir(kind, lines, extra = {}) {
  const id = await call("sales_create_draft", { draft: { kind, customer_id: 1, lines, due_date: "2026-12-10", ...extra } });
  if (typeof id !== "number") return { error: id };
  const doc = await call("sales_issue", { id, date: null });
  return { id, numero: doc?.number ?? doc };
}
const pdfs = {};
async function pdf(id) {
  const ruta = await call("sales_document_pdf", { id });
  if (String(ruta).startsWith("ERROR")) return ruta;
  pdfs[id] = ruta;
  return basename(ruta);
}
const sha = (ruta) => createHash("sha256").update(readFileSync(ruta)).digest("hex");

const r = { fecha: new Date().toISOString() };
await ev(`window.confirm = () => true;`);
await pulsar("Ahora no");

// 1. Datos sintéticos de partida.
r.productor = await call("sales_setup_status");
r.empresa = await call("sales_save_company", { company: { name: "Taller Sintético de Pruebas, S.L.", tax_id: "B12345674", address: "Calle Ficticia 1, 28000 Madrid", email: "facturas@ejemplo.test", iban: "ES9121000418450200051332" } });
r.cliente = await call("sales_create_customer", { data: { name: "Alfa Suministros S.A.", tax_id: "A58818501", address: "Avenida Inventada 2, Sevilla", email: "compras@alfa.test" } });

// 2. Sin membrete: factura, presupuesto y rectificativa por diferencias.
r.facturaSinMembrete = await emitir("invoice", [linea("Revisión anual de equipos", "2", "45.00"), linea("Material de sustitución", "3", "12.50", "10"), linea("Desplazamiento", "1", "20.00", "21", "0.10")]);
r.presupuesto = await emitir("quote", [linea("Instalación sintética", "1", "300.00")]);
const rect = await call("sales_create_corrective", { id: r.facturaSinMembrete.id, kind: "R4", method: "differences" });
r.rectificativa = typeof rect === "number"
  ? { id: rect, editar: await call("sales_update_draft", { id: rect, draft: { kind: "corrective_invoice", customer_id: 1, lines: [linea("Ajuste de precio de la revisión", "-1", "45.00")], rectification: { kind: "R4", method: "differences" } } }), numero: (await call("sales_issue", { id: rect, date: null }))?.number }
  : { error: rect };
for (const d of [r.facturaSinMembrete, r.presupuesto, r.rectificativa]) if (d.id) d.pdf = await pdf(d.id);

// 3. Pantalla Empresa > Membrete.
await pulsar("Ventas");
await pulsar("Empresa");
r.textoAyuda = await ev(`return main().innerText.match(/Membrete\\n[\\s\\S]{0,900}/)?.[0]`);
await verSeccionMembrete("01-membrete-vacio");

// 4. Archivos que deben rechazarse.
r.rechazos = {};
for (const f of ["tres-paginas.pdf", "apaisado.pdf", "a5.pdf", "con-clave.pdf", "grande.pdf", "vacio.pdf", "no-es-pdf.txt", "oscuro.pdf", "banda-en-texto.pdf"]) {
  r.rechazos[f] = await subir(f);
  if (f === "oscuro.pdf") await captura("02-rechazo-oscuro"); // el aviso sale arriba
}
// 5. Membrete válido de dos páginas y PDF con restricciones pero sin clave de apertura.
r.restringido = await subir("restringido.pdf");
r.dosPaginas = await subir("dos-paginas.pdf");
await verSeccionMembrete("03-membrete-guardado");

// 6. Márgenes.
r.margenes = {
  letras: await margenes(M("abc", 20, 25, 20)),
  negativo: await margenes(M(-5, 20, 25, 20)),
  masDe100: await margenes(M(200, 20, 25, 20)),
  anchoUtilInsuficiente: await margenes(M(35, 60, 25, 60)),
  bandaEnZonaDeTexto: await margenes(M(5, 20, 5, 20)),
};
await captura("04-margen-rechazado-banda");
r.margenes.validos = await margenes(M(40, 20, 25, 20));

// 7. Con membrete: factura de 40 líneas y factura con márgenes mínimos e importes grandes.
r.facturaLarga = await emitir("invoice", Array.from({ length: 40 }, (_, i) => linea(`Servicio sintético ${i + 1}`, "1", "10.00")));
r.facturaLarga.pdf = await pdf(r.facturaLarga.id);
r.margenes.estrechos = await margenes(M(100, 55, 47, 55));
r.facturaEstrecha = await emitir("invoice", Array.from({ length: 12 }, (_, i) => linea(`Servicio sintético con una descripción larga para forzar el ajuste, número ${i + 1}`, "1234.56", "98765.43", i % 2 ? "10" : "21", "0.125")));
r.facturaEstrecha.pdf = await pdf(r.facturaEstrecha.id);
r.margenes.restaurados = await margenes(M(40, 20, 25, 20));

// 8. «Ver PDF» desde el detalle de la factura larga.
await pulsar("Documentos");
r.abrirDetalle = await ev(`const b = [...main().querySelectorAll('button')].find(b => b.textContent.trim().startsWith('Abrir Factura ${r.facturaLarga.numero}')); if (!b) return false; b.click(); return true;`);
await sleep(900);
await captura("05-detalle-factura");
r.verPdf = { boton: await pulsar("Ver PDF") };
await sleep(1500);
r.verPdf.avisos = await avisos();

// 9. Quitar el membrete.
await pulsar("Empresa");
await pulsar("Quitar membrete");
await sleep(600);
r.quitar = { avisos: await avisos(), empresa: await empresa() };
await verSeccionMembrete("06-membrete-quitado");

// 10. Regenerar: se borran los PDF y se generan de nuevo; deben salir los mismos bytes.
r.regeneracion = {};
const antes = Object.fromEntries(Object.entries(pdfs).map(([id, ruta]) => [id, sha(ruta)]));
for (const ruta of Object.values(pdfs)) rmSync(ruta);
for (const id of Object.keys(pdfs)) {
  await call("sales_document_pdf", { id: Number(id) });
  r.regeneracion[basename(pdfs[id])] = { sha256: antes[id], igual: sha(pdfs[id]) === antes[id] };
}
for (const ruta of Object.values(pdfs)) copyFileSync(ruta, join(salida, "pdf", basename(ruta)));

ws.close();
writeFileSync(join(salida, "resultados.json"), JSON.stringify(r, null, 2) + "\n");
console.log(JSON.stringify(r, null, 2));
