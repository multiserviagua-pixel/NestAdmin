// Recorrido T18 (remisión Verifactu, E5) sobre `tauri dev` con el simulador AEAT en proceso.
//
// Requisitos: ventana de `tauri dev` abierta con WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9333,
// NESTADMIN_PDFIUM_DIR y una carpeta de datos con un verifactu-productor.json sintético. Tres fases, cada una
// con su arranque de la ventana:
//   rechazo  — carpeta NUEVA y NESTADMIN_VERIFACTU_SIM=rechazar-siguiente: rechazo, subsanación, espera,
//              aceptado; alta y anulación de otra factura; Configuración > Verifactu.
//   sin-red  — misma carpeta, NESTADMIN_VERIFACTU_SIM=sin-red: emitir sin red queda pendiente y «Remitir ahora».
//   reanudar — misma carpeta, sin la variable: lo pendiente se remite solo al reabrir.
//
// Uso: node recorrido-remision.mjs <fase> <carpeta de salida>
import { mkdirSync, readFileSync, writeFileSync, existsSync } from "node:fs";
import { join } from "node:path";

const [fase, salida] = process.argv.slice(2);
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
  writeFileSync(join(salida, `remision-${nombre}.jpg`), Buffer.from(shot.result.data, "base64"));
}
const linea = (d, q, p, iva = "21") => ({ description: d, quantity: q, unit_price: p, tax_percent: iva, discount: null });
async function emitir(lines) {
  const id = await call("sales_create_draft", { draft: { kind: "invoice", customer_id: 1, lines, due_date: "2026-12-10" } });
  if (typeof id !== "number") return { error: id };
  const t0 = Date.now();
  const doc = await call("sales_issue", { id, date: null });
  return { id, numero: doc?.number ?? doc, msEmitir: Date.now() - t0 };
}
/** Resumen del estado: entorno, contadores y estado por registro. */
async function estado() {
  const s = await call("verifactu_status");
  if (typeof s === "string") return { error: s };
  return {
    entorno: s.environment, pendientes: s.pending, enviando: s.sending, rechazados: s.rejected, parado: s.halted,
    espera_s: s.wait_remaining_seconds, ultimoLote: s.last_batch,
    registros: s.records.map((r) => ({ id: r.id ?? r.record_id, ...r })),
  };
}
/** Espera hasta que `pred(estado)` se cumpla (el worker remite cada ~5 s y respeta la espera de la AEAT). */
async function esperar(pred, maxMs, etiqueta) {
  const t0 = Date.now();
  for (;;) {
    const e = await estado();
    if (pred(e)) return { ...e, esperadoMs: Date.now() - t0 };
    if (Date.now() - t0 > maxMs) return { ...e, timeout: etiqueta, esperadoMs: Date.now() - t0 };
    await sleep(3000);
  }
}
async function verRegistros(nombre) {
  await pulsar("Ventas");
  await pulsar("Registros Verifactu");
  await pulsar("Actualizar estado");
  await sleep(500);
  await captura(nombre);
  return await ev(`return main().innerText.slice(0, 4000);`);
}

const ficheroR = join(salida, "remision-resultados.json");
const r = existsSync(ficheroR) ? JSON.parse(readFileSync(ficheroR, "utf8")) : {};
const f = (r[fase] = { fecha: new Date().toISOString() });
process.on("exit", () => writeFileSync(ficheroR, JSON.stringify(r, null, 2) + "\n"));
await ev(`window.__confirmaciones = []; window.confirm = (m) => { window.__confirmaciones.push(m); return true; };`);
await pulsar("Ahora no");

if (fase === "rechazo") {
  f.entorno = await call("verifactu_environment");
  f.empresa = await call("sales_save_company", { company: { name: "Taller Sintético de Pruebas, S.L.", tax_id: "B12345674", address: "Calle Ficticia 1, 28000 Madrid", email: "facturas@ejemplo.test", iban: "ES9121000418450200051332" } });
  f.cliente = await call("sales_create_customer", { data: { name: "Alfa Suministros S.A.", tax_id: "A58818501", address: "Avenida Inventada 2, Sevilla", email: "compras@alfa.test" } });
  // 1. Primera factura: el simulador rechaza el primer lote (modo de depuración).
  f.factura1 = await emitir([linea("Revisión anual de equipos", "2", "45.00")]);
  f.trasRechazo = await esperar((e) => e.rechazados >= 1, 30000, "rechazo");
  f.textoRechazo = await verRegistros("01-rechazado");
  // 2. Subsanar desde la pestaña: confirmación y registro nuevo de subsanación.
  f.subsanar = await pulsar("Subsanar");
  await sleep(1500);
  f.confirmaciones = await ev(`return window.__confirmaciones.slice();`);
  f.avisosSubsanar = await avisos();
  // 3. «Remitir ahora» dentro de la espera de la AEAT: no envía y dice cuánto falta.
  f.remitirEnEspera = { boton: await pulsar("Remitir ahora") };
  await sleep(1500);
  f.remitirEnEspera.avisos = await avisos();
  f.remitirEnEspera.estado = await estado();
  await captura("02-espera");
  // 4. Pasada la espera, el worker remite la subsanación y queda aceptada.
  f.trasSubsanar = await esperar((e) => e.pendientes === 0 && e.enviando === 0 && e.rechazados === 0, 120000, "subsanación aceptada");
  f.textoAceptado = await verRegistros("03-subsanado-aceptado");
  // 5. Segunda factura y su anulación: alta y anulación van en lotes distintos (D257).
  f.factura2 = await emitir([linea("Instalación sintética", "1", "300.00")]);
  f.anular = await call("sales_void", { id: f.factura2.id });
  f.trasAnular = await esperar((e) => e.pendientes === 0 && e.enviando === 0, 200000, "alta y anulación aceptadas");
  f.textoFinal = await verRegistros("04-todo-aceptado");
  // 6. Configuración > Verifactu: solo el simulador se puede elegir.
  await pulsar("Configuración");
  await ev(`[...main().querySelectorAll('h2,h3')].find(h => h.textContent.trim() === 'Verifactu')?.scrollIntoView({ block: 'start' });`);
  await sleep(500);
  f.configuracion = await ev(`const h = [...main().querySelectorAll('h2,h3')].find(h => h.textContent.trim() === 'Verifactu'); const s = h?.closest('section') ?? h?.parentElement;
    return { texto: s?.innerText ?? null, opciones: [...(s?.querySelectorAll('input[type=radio], option') ?? [])].map(o => ({ valor: o.value, desactivada: o.disabled, elegida: o.checked ?? o.selected })) };`);
  await captura("05-configuracion");
  f.cambiarAPreproduccion = await call("verifactu_set_environment", { environment: "preproduccion" });
  f.cambiarAProduccion = await call("verifactu_set_environment", { environment: "produccion" });
  f.entornoDespues = await call("verifactu_environment");
} else if (fase === "sin-red") {
  // 7. Sin red: la emisión vuelve enseguida y el registro queda pendiente, visible.
  f.factura3 = await emitir([linea("Mantenimiento sintético", "1", "80.00")]);
  await sleep(12000); // más de dos ticks del worker
  f.estado = await estado();
  f.remitir = { boton: await pulsar("Ventas") && await pulsar("Registros Verifactu") && await pulsar("Remitir ahora") };
  await sleep(4000);
  f.remitir.avisos = await avisos();
  f.texto = await verRegistros("06-sin-red-pendiente");
} else if (fase === "reanudar") {
  // 8. Al reabrir con red, lo pendiente se remite solo; el simulador se sembró con lo aceptado.
  f.estado = await esperar((e) => e.pendientes === 0 && e.enviando === 0, 120000, "pendiente remitido al reabrir");
  f.texto = await verRegistros("07-reanudado");
  await pulsar("Tareas");
  await sleep(800);
  f.tareas = await ev(`return main().innerText.slice(0, 2500);`);
  await captura("08-tareas");
} else {
  throw new Error("fase desconocida: " + fase);
}
ws.close();
console.log(JSON.stringify(f, null, 2));
