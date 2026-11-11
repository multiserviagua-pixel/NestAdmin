// Recorrido T15b (envío, E4b) sobre `tauri dev`: enviar por correo un documento de venta con revisión.
//
// Requisitos: la ventana de `tauri dev` abierta con una carpeta de datos NUEVA (sin cuentas reales:
// la única cuenta es el buzón simulado), un verifactu-productor.json sintético en ella,
// NESTADMIN_PDFIUM_DIR apuntando a PDFium y WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9333.
//
// Uso: node recorrido-envio.mjs <carpeta de datos> <carpeta de salida>
// Escribe <salida>/envio-resultados.json y capturas <salida>/envio-*.jpg.
import { readFileSync, mkdirSync, writeFileSync } from "node:fs";
import { createHash } from "node:crypto";
import { join } from "node:path";

const [datos, salida] = process.argv.slice(2);
mkdirSync(salida, { recursive: true });
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
  const btn = (t) => [...document.querySelectorAll('button, a')].find(b => b.textContent.trim() === t && !b.disabled);`;
async function ev(expr) {
  const m = await send("Runtime.evaluate", { expression: `(async () => { ${PRE} ${expr} })()`, awaitPromise: true, returnByValue: true });
  if (m.result?.exceptionDetails) throw new Error(m.result.exceptionDetails.exception?.description ?? m.result.exceptionDetails.text);
  return m.result?.result?.value;
}
const call = (cmd, args) => ev(`return await inv(${JSON.stringify(cmd)}, ${JSON.stringify(args ?? {})}).catch(e => 'ERROR ' + (typeof e === 'string' ? e : JSON.stringify(e)));`);
const avisos = () => ev(`return [...document.querySelectorAll('[role=alert],[role=status],.notice,.toast,.error')]
  .map(x => x.textContent.trim().replace(/×$/, '')).filter(t => t && !/componentes de OCR|configurar la IA/.test(t));`);
const pulsar = async (texto) => { const ok = await ev(`const b = btn(${JSON.stringify(texto)}); if (!b) return false; b.click(); return true;`); await sleep(1000); return ok; };
async function captura(nombre) {
  const shot = await send("Page.captureScreenshot", { format: "jpeg", quality: 70 });
  writeFileSync(join(salida, `envio-${nombre}.jpg`), Buffer.from(shot.result.data, "base64"));
}
/** Valores visibles de la tarjeta de envío. */
const tarjeta = () => ev(`const q = (l) => document.querySelector('[aria-label="' + l + '"]');
  return { para: q('Destinatario')?.value ?? null, cuenta: q('Cuenta de correo')?.selectedOptions?.[0]?.textContent ?? null,
    asunto: q('Asunto')?.value ?? null, cuerpo: q('Cuerpo del correo')?.value ?? null,
    titulo: document.querySelector('.detail-head h2')?.textContent ?? null,
    texto: document.querySelector('.detail article')?.innerText ?? null };`);
const escribir = (label, valor) => ev(`const el = document.querySelector('[aria-label="${label}"]'); if (!el) return false; el.value = ${JSON.stringify(valor)};
  el.dispatchEvent(new Event('input', { bubbles: true })); el.dispatchEvent(new Event('change', { bubbles: true })); return true;`);
async function abrirDocumento(numero) {
  await pulsar("Ventas");
  await pulsar("Documentos");
  const ok = await ev(`const b = [...main().querySelectorAll('button')].find(b => b.textContent.trim().startsWith('Abrir ' + ${JSON.stringify(numero)})); if (!b) return false; b.click(); return true;`);
  await sleep(1000);
  return ok;
}
const historial = () => ev(`return main().innerText.match(/Envíos por correo\\n[\\s\\S]{0,600}/)?.[0] ?? null;`);
const simulador = () => JSON.parse(readFileSync(join(datos, "simulador-correo.json"), "utf8")).state;
const sha = (ruta) => createHash("sha256").update(readFileSync(ruta)).digest("hex");
const linea = (d, q, p, iva = "21") => ({ description: d, quantity: q, unit_price: p, tax_percent: iva, discount: null });
async function emitir(customer_id, lines) {
  const id = await call("sales_create_draft", { draft: { kind: "invoice", customer_id, lines, due_date: "2026-12-10" } });
  if (typeof id !== "number") return { error: id };
  const doc = await call("sales_issue", { id, date: null });
  return { id, numero: doc?.number ?? doc };
}

const r = { fecha: new Date().toISOString() };
// Los resultados se guardan también si un paso falla, con el error.
process.on("uncaughtException", (e) => { r.errorDelRecorrido = String(e?.stack ?? e); process.exitCode = 1; });
process.on("exit", () => writeFileSync(join(salida, "envio-resultados.json"), JSON.stringify(r, null, 2) + "\n"));
// Se guardan los textos de confirmación que muestra la ventana y se aceptan.
await ev(`window.__confirmaciones = []; window.confirm = (m) => { window.__confirmaciones.push(m); return true; };`);
await pulsar("Ahora no");

// 1. Datos sintéticos.
r.empresa = await call("sales_save_company", { company: { name: "Taller Sintético de Pruebas, S.L.", tax_id: "B12345674", address: "Calle Ficticia 1, 28000 Madrid", email: "facturas@ejemplo.test", iban: "ES9121000418450200051332" } });
const conCorreo = await call("sales_create_customer", { data: { name: "Alfa Suministros S.A.", tax_id: "A58818501", address: "Avenida Inventada 2, Sevilla", email: "compras@alfa.test" } });
const sinCorreo = await call("sales_create_customer", { data: { name: "Beta Sin Correo S.L.", tax_id: "B87654323", address: "Plaza Imaginaria 3, Valencia", email: null } });
r.clientes = { conCorreo, sinCorreo };
r.cuentas = await call("sales_send_accounts");
r.factura = await emitir(conCorreo, [linea("Revisión anual de equipos", "2", "45.00"), linea("Material de sustitución", "3", "12.50", "10")]);
r.facturaSinCorreo = await emitir(sinCorreo, [linea("Instalación sintética", "1", "300.00")]);
r.factura.pdf = await call("sales_document_pdf", { id: r.factura.id });

// 2. Desde el detalle de la factura: «Enviar por correo» abre la tarjeta en Pendientes.
r.abrirDetalle = await abrirDocumento(`Factura ${r.factura.numero}`);
r.historialAntes = await historial();
await captura("01-detalle-antes");
r.enviar = await pulsar("Enviar por correo");
await sleep(800);
r.tarjetaInicial = await tarjeta();
r.tareaInicial = await call("sales_send_task", { id: r.factura.id });
await captura("02-tarjeta-revision");
r.enviadosAntes = (simulador().drafts ?? []).filter((d) => d.standalone && d.sent_receipt).length;

// 3. Editar el asunto y guardar: versión nueva, nada enviado.
await escribir("Asunto", `Factura ${r.factura.numero} - revisada`);
r.guardar = await pulsar("Guardar cambios");
await sleep(600);
r.avisosGuardar = await avisos();
r.tareaTrasGuardar = await call("sales_send_task", { id: r.factura.id });
r.enviadosTrasGuardar = (simulador().drafts ?? []).filter((d) => d.standalone && d.sent_receipt).length;

// 4. Aprobar y enviar, con la confirmación de la ventana.
r.aprobar = await pulsar("Aprobar y enviar");
await sleep(1500);
r.confirmaciones = await ev(`return window.__confirmaciones.slice();`);
r.avisosAprobar = await avisos();
await captura("03-tras-aprobar");
r.tareaTrasAprobar = await call("sales_send_task", { id: r.factura.id });
r.envios = await call("sales_document_sends", { id: r.factura.id });

// 5. Lo que salió del buzón simulado: destinatario, asunto y el adjunto frente al PDF emitido.
const sim = simulador();
const salidos = (sim.drafts ?? []).filter((d) => d.standalone && d.sent_receipt);
r.simulador = {
  enviados: salidos.length,
  mensajes: salidos.map((d) => ({ to: d.content?.to, subject: d.content?.subject, adjuntos: d.content?.attachments, recibo: d.sent_receipt, clave: d.idempotency_key })),
  borradores: (sim.drafts ?? []).length,
  shaPdfEmitido: typeof r.factura.pdf === "string" && !r.factura.pdf.startsWith("ERROR") ? sha(r.factura.pdf) : null,
  claves: Object.keys(sim),
};

// 6. Historial en el detalle.
await abrirDocumento(`Factura ${r.factura.numero}`);
r.historialDespues = await historial();
await ev(`[...main().querySelectorAll('h2,h3')].find(h => h.textContent.includes('Envíos por correo'))?.scrollIntoView({ block: 'start' });`);
await sleep(300);
await captura("04-historial");

// 7. Segundo envío del mismo documento y descartarlo: no sale nada y el historial no cambia.
r.segundo = { boton: await pulsar("Enviar por correo") };
await sleep(800);
r.segundo.tarjeta = await tarjeta();
r.segundo.descartar = await pulsar("Descartar");
await sleep(800);
r.segundo.avisos = await avisos();
r.segundo.envios = await call("sales_document_sends", { id: r.factura.id });
r.segundo.enviadosSimulador = (simulador().drafts ?? []).filter((d) => d.standalone && d.sent_receipt).length;

// 8. Cliente sin correo: la tarjeta abre sin destinatario y no se puede aprobar.
await abrirDocumento(`Factura ${r.facturaSinCorreo.numero}`);
r.sinCorreo = { boton: await pulsar("Enviar por correo") };
await sleep(800);
r.sinCorreo.tarjeta = await tarjeta();
r.sinCorreo.avisos = await avisos();
await captura("05-sin-correo");
const antesConf = (await ev(`return window.__confirmaciones.length;`));
r.sinCorreo.aprobar = await pulsar("Aprobar y enviar");
await sleep(800);
r.sinCorreo.avisosAprobar = await avisos();
r.sinCorreo.confirmacionPedida = (await ev(`return window.__confirmaciones.length;`)) > antesConf;
r.sinCorreo.enviadosSimulador = (simulador().drafts ?? []).filter((d) => d.standalone && d.sent_receipt).length;
// Destinatario no válido escrito a mano.
await escribir("Destinatario", "no-es-un-correo");
r.sinCorreo.invalido = { guardar: await pulsar("Guardar cambios") };
await sleep(600);
r.sinCorreo.invalido.avisos = await avisos();
await captura("06-destinatario-invalido");

ws.close();
writeFileSync(join(salida, "envio-resultados.json"), JSON.stringify(r, null, 2) + "\n");
console.log(JSON.stringify(r, null, 2));
