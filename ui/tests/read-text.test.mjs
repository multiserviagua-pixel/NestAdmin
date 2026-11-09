// E5, D133: los textos de la primera lectura solo afirman lo que los datos del backend sostienen.
// Todos los ejemplos son inventados.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import {
  READ_EVIDENCE_NOTE,
  READ_HELP,
  byProviderText,
  canStartRead,
  canStopRead,
  confirmReadLines,
  connectDetails,
  connectNotice,
  continueButtonLabel,
  folderLabel,
  heldText,
  readButtonLabel,
  readDestination,
  readEffectsLine,
  readErrorWords,
  readFailureLine,
  readNotes,
  readProblems,
  readPromise,
  recentMailWarning,
  WAITING_UNKNOWN,
  readRemaining,
  readStartedWith,
  readStateLabel,
  readStateTone,
  readSummary,
  readVolumeText,
  suggestionsLine,
  volumeProblem,
  waitingReadsNotice,
} from "../src/read-text.ts";

const tasks = (over = {}) => ({ total: 23, queued: 0, running: 0, completed: 23, failed: 0, cancelled: 0, ...over });

const status = (over = {}) => ({
  source: "account:google:ana@ejemplo.test",
  kind: "account",
  state: "done",
  volume: 100,
  provider: "ollama_cloud",
  model: "modelo-sintetico",
  host: "ollama.test",
  listed: 30,
  already_known: 0,
  recent_left_out: 0,
  emails_unclassified: 20,
  documents_sent: 3,
  documents_skipped: 0,
  over_cap: 0,
  tasks: tasks(),
  by_provider: [],
  failures: [],
  attachment_failures: [],
  skipped_attachments: 0,
  listing_error: null,
  scan_capped: false,
  remaining: 0,
  fallback_used: 0,
  actions: 0,
  waiting_message: null,
  held: null,
  removed: false,
  ...over,
});

const overview = (over = {}) => ({
  mode: "automatic",
  limits: { min: 25, max: 500, fixed: 100, default: 100 },
  recent_days: 7,
  documents_cap: 100,
  provider: { provider: "ollama_cloud", model: "modelo-sintetico" },
  host: "ollama.test",
  external: true,
  local_fallback: true,
  blocked: null,
  blocked_message: null,
  sends: ["El texto completo de cada documento que parece una factura, ya extraído en este equipo"],
  never_sent: ["el cuerpo ni el asunto de ningún correo", "claves"],
  ...over,
});

const sugg = (over = {}) => ({ seen_documents: 12, ready: 1, closest: 2, needed: 5, error: null, ...over });

test("el aviso visible dice cuánto, a quién, qué sale y qué pasa con los últimos 7 días; el detalle va aparte", () => {
  const visible = connectNotice(overview()).join(" ");
  assert.match(visible, /se leerá una sola vez: hasta 100 mensajes anteriores a los últimos 7 días de la cuenta, con un tope total de 100 documentos enviados/);
  assert.match(visible, /Se enviará a Ollama cloud, modelo modelo-sintetico, servidor ollama\.test: solo los documentos que parecen una factura, completos\. En esta lectura no sale ningún correo/);
  assert.match(visible, /Los correos de los últimos 7 días no se leen: siguen el flujo normal/);
  assert.match(visible, /salen completos al proveedor.*una regla activa puede enviar la respuesta sin revisión/);
  assert.match(visible, /El proveedor ya está confirmado/);
  const details = connectDetails(overview()).join(" ");
  assert.match(details, /Qué se envía: El texto completo/);
  assert.match(details, /No se envía nunca: el cuerpo ni el asunto de ningún correo; claves\./);
  assert.match(details, /lo leído no vuelve a procesarse/);
  assert.ok(connectNotice(overview()).length < 6, "lo visible es corto");
});

test("la promesa sobre los correos se limita a esta lectura y avisa de que los recientes salen completos al consultar", () => {
  const promise = readPromise({ recent_days: 7 });
  assert.match(promise, /^En esta lectura no se envía ningún correo ni se registra nada/);
  assert.doesNotMatch(promise, /Los correos no se envían a ningún proveedor\./, "no es una promesa general");
  assert.match(promise, /Los correos leídos no salen a ningún proveedor/);
  assert.match(promise, /Los correos de los últimos 7 días no se leen: siguen el flujo normal.*salen completos al proveedor.*una regla activa puede enviar la respuesta sin revisión/);
  assert.match(recentMailWarning({ recent_days: 3 }), /últimos 3 días/);
  // También sin resumen y en las versiones de prueba el aviso dice lo de los recientes.
  assert.match(connectNotice(null, "falló").join(" "), /Los correos recientes siguen el flujo normal: al consultar el correo salen completos al proveedor y una regla activa puede actuar/);
  assert.match(connectNotice(overview({ mode: "on_demand", limits: { min: 25, max: 500, fixed: null, default: 100 } })).join(" "), /siguen el flujo normal/);
  assert.match(connectNotice(overview({ blocked: "needs_key", blocked_message: "x" })).join(" "), /siguen el flujo normal/);
});

test("sin proveedor confirmado no dice «se enviará a X» y avisa de que la configuración dirá a quién", () => {
  const text = connectNotice(overview({ blocked: "needs_key", blocked_message: "Falta configurar la IA: guarde la clave" })).join(" ");
  assert.match(text, /no hay proveedor de IA confirmado \(Falta configurar la IA: guarde la clave\): no se enviará nada.*en espera/);
  assert.doesNotMatch(text, /Se enviará a Ollama/);
  assert.doesNotMatch(text, /ya está confirmado/);
});

test("si falla el resumen lo dice y la lectura no arranca sola", () => {
  const text = connectNotice(null, "no se pudo abrir la base").join(" ");
  assert.match(text, /No se ha podido comprobar qué se leería ni a quién se enviaría \(no se pudo abrir la base\)/);
  assert.match(text, /no arrancará sola: quedará pendiente, con un botón/);
});

test("al confirmar un proveedor con lecturas en espera, el aviso dice que arrancarán con volumen y datos", () => {
  const text = waitingReadsNotice(overview(), 2);
  assert.match(text, /Hay 2 lecturas en espera: al confirmar el proveedor, empezarán solas/);
  assert.match(text, /hasta 100 mensajes/);
  assert.match(text, /enviarán El texto completo/);
  assert.equal(waitingReadsNotice(overview(), 0), null);
  assert.equal(waitingReadsNotice(overview({ mode: "on_demand" }), 3), null);
});

test("en las versiones de prueba el aviso dice que no se lee al conectar y que se pide con volumen", () => {
  const o = overview({ mode: "on_demand", limits: { min: 25, max: 500, fixed: null, default: 100 } });
  assert.match(connectNotice(o).join(" "), /no se lee al conectar: usted la pide más abajo, eligiendo el volumen y viendo antes a qué proveedor/);
  assert.match(readVolumeText(o), /entre 25 y 500 mensajes, el volumen que usted elija, anteriores a los últimos 7 días/);
  assert.match(readVolumeText(o, "folder"), /archivos admitidos de la carpeta.*sin subcarpetas.*tope de documentos/);
});

test("el destino nombra proveedor, modelo y servidor, y el respaldo solo es local", () => {
  const d = readDestination(overview());
  assert.match(d, /Ollama cloud, modelo modelo-sintetico, servidor ollama\.test/);
  assert.match(d, /solo se repetirá con la IA local de este equipo si está preparada; nunca con otro proveedor externo/);
  assert.match(readDestination(overview({ local_fallback: false })), /respaldo con la IA local está desactivado/);
  assert.match(readDestination(overview({ external: false, host: null, provider: { provider: "ollama", model: "qwen" } })), /no sale de este equipo/);
});

test("la promesa dice que los documentos de factura salen completos y el límite lo escribe usted", () => {
  const p = readPromise({ recent_days: 7 });
  assert.match(p, /salen solo los documentos.*parecen una factura.*salen completos/);
  assert.match(p, /una sola vez por buzón/);
  assert.match(READ_EVIDENCE_NOTE, /nunca de respuestas por correo/);
  assert.match(READ_EVIDENCE_NOTE, /el límite lo escribe usted/);
});

test("sin el resumen tampoco se sabe si hay lecturas en espera, y se dice", () => {
  assert.match(WAITING_UNKNOWN, /No se ha podido comprobar si hay lecturas de correo en espera.*empezarán solas.*completos/);
  assert.match(waitingReadsNotice(overview(), 1), /siguen el flujo normal/);
});

test("lo que se confirma antes de leer incluye destino, qué sale y qué no", () => {
  const c = confirmReadLines(overview({ mode: "on_demand" }), 50, "la cuenta Oficina");
  assert.match(c.heading, /la cuenta Oficina: 50 elementos como mucho, y no más de 50 documentos enviados/);
  assert.match(c.lines[0], /servidor ollama\.test/);
  assert.equal(c.sends.length, 1);
  assert.match(c.never, /No se envía nunca: el cuerpo ni el asunto de ningún correo; claves\./);
});

test("el volumen se valida contra el rango del backend", () => {
  const limits = { min: 25, max: 500 };
  assert.equal(volumeProblem("25", limits), null);
  assert.equal(volumeProblem("500", limits), null);
  assert.match(volumeProblem("24", limits), /entre 25 y 500/);
  assert.match(volumeProblem("501", limits), /entre 25 y 500/);
  assert.match(volumeProblem("", limits), /número entero/);
  assert.match(volumeProblem("1,5", limits), /número entero/);
});

test("el resumen separa documentos enviados, omitidos y correos sin clasificar, y dice que las sugerencias son globales", () => {
  const text = readSummary(status({ documents_skipped: 2, over_cap: 4 }), sugg());
  assert.equal(text, "Enviados 3 documentos; 2 omitidos por no parecer facturas; 20 correos sin clasificar (no se envían); 4 sin leer por el tope; sugerencias: 1 lista para revisar en Mis reglas, pestaña Sugeridas; 2 proveedores aún no llegan al mínimo de 5 facturas vistas.");
  assert.match(readSummary(status({ documents_sent: 1, documents_skipped: 1, emails_unclassified: 1 }), sugg()), /Enviados 1 documento; 1 omitido por no parecer una factura; 1 correo sin clasificar \(no se envía\)/);
});

test("sin facturas vistas dice que no hay sugerencias porque no se vio ninguna, no que todo va bien", () => {
  assert.match(suggestionsLine(sugg({ seen_documents: 0, ready: 0, closest: 0 })), /ninguna: no se ha visto ninguna factura legible/);
  assert.match(suggestionsLine(sugg({ ready: 0, closest: 0 })), /ninguna todavía \(12 documentos vistos sin repetirse lo bastante/);
  assert.match(suggestionsLine(sugg({ error: "falló" })), /no se han podido calcular \(falló\)/);
  const none = status({ tasks: tasks({ total: 0, completed: 0 }), emails_unclassified: 0, documents_sent: 0, state: "not_started" });
  assert.equal(readSummary(none, sugg()), "Todavía no se ha leído nada.");
  assert.match(readSummary({ ...none, state: "waiting" }, sugg()), /en espera/);
});

test("«no se ha enviado ningún correo ni registrado nada» solo con cero acciones, y aclara que los documentos sí se enviaron", () => {
  const sent = readEffectsLine(status());
  assert.match(sent, /^No se ha enviado ningún correo ni se ha registrado nada\./);
  assert.match(sent, /Al proveedor sí se enviaron 3 documentos completos, con todo lo que contienen/);
  assert.match(readEffectsLine(status({ documents_sent: 0 })), /Al proveedor no se ha enviado ningún documento/);
  assert.match(readEffectsLine(status({ actions: 2 })), /^Atención: constan 2 acciones/);
  assert.doesNotMatch(readEffectsLine(status({ actions: 2 })), /No se ha enviado ningún correo/);
  assert.equal(readEffectsLine(status({ tasks: tasks({ total: 0, completed: 0 }), emails_unclassified: 0 })), null);
});

test("lo que falta, los errores y los adjuntos que fallaron son explícitos, y en una carpeta no se dice «adjunto»", () => {
  assert.equal(readRemaining(status()), null);
  assert.equal(readRemaining(status({ tasks: tasks({ queued: 3, running: 1, failed: 2 }) })), "Falta: 3 en cola, 1 en curso, 2 con error.");
  assert.match(readFailureLine({ task_id: 1, kind: "invoice", name: "a.pdf", code: "ocr_unavailable", message: "x", recoverable: true }), /a\.pdf: el OCR local no está disponible; se puede reintentar/);
  assert.match(readErrorWords("rara"), /error «rara»/);
  const issues = { listing_error: { code: "unavailable", message: "x" }, attachment_failures: [{ name: "f.pdf", code: "unavailable", message: "x" }] };
  const problems = readProblems(status(issues));
  assert.match(problems[0], /No se pudo listar: el servicio no respondió.*reanudar/);
  assert.match(problems[1], /Adjunto «f\.pdf»/);
  assert.match(readProblems(status({ ...issues, kind: "folder" }))[1], /Archivo «f\.pdf»/);
  assert.deepEqual(readProblems(status()), []);
});

test("las notas dicen qué no se tocó, qué quedó para el flujo normal y qué atendió el respaldo", () => {
  const notes = readNotes(status({ recent_left_out: 4, already_known: 3, skipped_attachments: 2, over_cap: 6, scan_capped: true, fallback_used: 1 }));
  assert.match(notes[0], /4 correos recientes o sin fecha legible siguen el flujo normal: no se leyeron/);
  assert.match(notes[1], /3 elementos ya tenían tarea o se había importado antes: no se ha tocado/);
  assert.match(notes[2], /2 adjuntos de un tipo que NestAdmin no procesa/);
  assert.match(notes[3], /6 documentos no se enviaron por el tope/);
  assert.match(notes[4], /máximo de mensajes/);
  assert.match(notes[5], /1 tarea la atendió la IA local de respaldo/);
  assert.deepEqual(readNotes(status()), []);
});

test("detener, retener y continuar: textos y botones según el estado", () => {
  const stopped = status({ state: "stopped", held: { reason: "stopped", authorized: null, now: { provider: "ollama_cloud", model: "m", host: "a.test" } } });
  assert.match(heldText(stopped), /Detenida: no sale nada más/);
  assert.equal(continueButtonLabel(stopped), "Continuar la lectura");
  const changed = status({ state: "held", held: { reason: "destination_changed", authorized: { provider: "ollama_cloud", model: "a", host: "a.test" }, now: { provider: "ollama", model: "qwen", host: null } } });
  const text = heldText(changed);
  assert.match(text, /Ollama cloud, modelo a, servidor a\.test era el destino que vio al empezar/);
  assert.match(text, /ahora el destino es la IA local \(Ollama\), modelo qwen, en este equipo/);
  assert.match(text, /no sale hasta que confirme continuar con el destino nuevo/);
  assert.equal(continueButtonLabel(changed), "Continuar con la IA local (Ollama), modelo qwen, en este equipo");
  assert.match(heldText(status({ state: "done", held: { reason: "removed", authorized: null, now: changed.held.now } })), /ya no está conectada.*se canceló/);
  assert.equal(heldText(status()), null);
  assert.ok(canStopRead(status({ state: "reading" })) && !canStopRead(status({ state: "done" })) && !canStopRead(status({ state: "reading", removed: true })));
});

test("el recuento por proveedor dice cuántos documentos fue a cada destino", () => {
  assert.equal(byProviderText(status()), null);
  const text = byProviderText(status({ by_provider: [{ provider: "ollama_cloud", model: "b", host: "b.test", documents: 2 }, { provider: "ollama", model: "q", host: null, documents: 1 }] }));
  assert.equal(text, "Documentos enviados: 2 a Ollama cloud, modelo b, servidor b.test; 1 a la IA local (Ollama), modelo q, en este equipo.");
});

test("el botón solo reanuda si ya había empezado; «terminada» solo en verde sin fallos", () => {
  assert.equal(readButtonLabel(status({ state: "not_started" })), "Leer ahora");
  assert.equal(readButtonLabel(status({ state: "pending" })), "Empezar la lectura");
  assert.equal(readButtonLabel(status({ state: "interrupted" })), "Reanudar la lectura");
  assert.ok(["not_started", "pending", "interrupted", "listing_failed"].every((state) => canStartRead(status({ state }))));
  assert.ok(!canStartRead(status({ state: "reading" })) && !canStartRead(status({ state: "done" })) && !canStartRead(status({ state: "stopped" })));
  for (const state of ["not_started", "waiting", "pending", "interrupted", "listing_failed", "stopped", "held", "reading", "done_with_errors", "done"]) assert.notEqual(readStateLabel(state), state);
  assert.equal(readStateTone("done"), "ok");
  for (const state of ["done_with_errors", "stopped", "held", "listing_failed", "waiting"]) assert.equal(readStateTone(state), "warn", state);
  assert.match(readStartedWith(status()), /Se empezó con Ollama cloud, modelo modelo-sintetico, servidor ollama\.test/);
  assert.equal(readStartedWith(status({ provider: null })), null);
  assert.equal(folderLabel("C:\\Facturas\\Octubre"), "Octubre");
});

test("las ayudas dicen que el texto sale completo, qué retiene detener y que el Historial lleva una entrada", () => {
  assert.match(READ_HELP.sends, /siempre en este equipo.*completo.*direcciones o nombres, salen con él/);
  assert.match(READ_HELP.apart, /Una sola entrada por lectura|una sola entrada por lectura/);
  assert.match(READ_HELP.stop, /impide que salga nada más/);
  assert.match(READ_HELP.volume, /también el tope de documentos/);
});

test("ningún texto de la primera lectura está en mayúsculas ni usa tamaños menores de 12 px", () => {
  const source = readFileSync(new URL("../src/read-text.ts", import.meta.url), "utf8");
  assert.doesNotMatch(source, /toUpperCase|text-transform/);
  for (const m of source.matchAll(/"([^"\n]{12,})"/g)) assert.ok(!/^[A-ZÁÉÍÓÚÑ\s.,:;!¡¿?]+$/.test(m[1]), `en mayúsculas: ${m[1]}`);
});

test("todas las variantes del aviso visible dicen que lo leído no crea pendientes ni se responde", () => {
  const frase = /Los correos leídos no crean pendientes ni se responden, y no vuelven a procesarse\./;
  assert.match(connectNotice(overview()).join(" "), frase, "final");
  assert.match(connectNotice(overview({ blocked: "needs_key", blocked_message: "x" })).join(" "), frase, "en espera");
  assert.match(connectNotice(null, "falló").join(" "), frase, "sin resumen");
  assert.match(connectNotice(overview({ mode: "on_demand", limits: { min: 25, max: 500, fixed: null, default: 100 } })).join(" "), frase, "bajo demanda");
});
