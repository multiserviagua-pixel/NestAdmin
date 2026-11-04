// Servidor simulado para ver la interfaz en un navegador sin Tauri. Imita los comandos
// reales con datos sintéticos (dominio .test); no decide nada que la capa Rust no decida.

import type { AuditEntry, Memory, ProviderChoice, Rule, SettingsView, SetupDone, SetupEvent, SetupStep, Startup, StepState, Task } from "./types";

const now = () => Date.now();
const listeners: Array<() => void> = [];
const changed = () => setTimeout(() => listeners.forEach((l) => l()), 0);

let nextId = 1;
const tasks: Task[] = [];
const rules: Rule[] = [];
const audit: AuditEntry[] = [];
let choice: ProviderChoice = { provider: "ollama", base_url: "http://127.0.0.1:11434", model: "qwen3:8b" };
let groqKey = false;

// Preparación simulada: Ollama está instalado y en marcha, falta el modelo. La primera
// descarga del modelo se corta una vez para mostrar el reintento. `?preparar` en la URL
// imita el modo del instalador y `?ram=12` u `?ram=8` un equipo con poca memoria (D46): con
// 8 GB la primera verificación agota el tiempo y la siguiente responde despacio.
const setupListeners: Array<(event: SetupEvent) => void> = [];
const doneListeners: Array<(done: SetupDone) => void> = [];
const startupListeners: Array<(startup: Startup) => void> = [];
const prepareOnly = new URLSearchParams(location.search).has("preparar");
let modelPresent = false;
let preparing: { cancelled: boolean } | null = null;
const GIB = 2 ** 30;
const ramGb = Number(new URLSearchParams(location.search).get("ram") ?? "16") || 16;
let verifyAttempts = 0;

// Mismos umbrales y textos que `crates/setup/src/memory.rs`; aquí solo se imitan.
function memory(): Memory {
  const total = ramGb * GIB;
  const available = Math.min(total * 0.4, 10 * GIB);
  const gb = (bytes: number) => (bytes / GIB).toFixed(1).replace(".", ",");
  const level = total + GIB >= 16 * GIB ? "recommended" : total < 9 * GIB ? "insufficient" : "below_recommended";
  let warning =
    level === "insufficient"
      ? `Este equipo tiene ${gb(total)} GB de memoria. Con 8 GB o menos, la IA local (Qwen3 8B) probablemente no responderá a tiempo. Puede intentarlo igualmente o elegir Groq en Configuración; NestAdmin no lo cambiará por su cuenta.`
      : level === "below_recommended"
        ? `Este equipo tiene ${gb(total)} GB de memoria; se recomiendan 16 GB para la IA local. Puede seguir, pero las respuestas serán más lentas y alguna podría no llegar a tiempo; si ocurre, puede elegir Groq en Configuración.`
        : null;
  if (available < 6 * GIB) {
    const free = `Ahora hay ${gb(available)} GB libres: cierre otros programas antes de usar la IA local.`;
    warning = warning ? `${warning} ${free}` : free;
  }
  return { total_bytes: total, available_bytes: available, level, warning };
}

function simulatePreparation(run: { cancelled: boolean }) {
  let last: SetupStep = "ocr";
  const emit = (step: SetupStep, state: StepState, message: string, done: number | null = null, total: number | null = null) => {
    last = step;
    setupListeners.forEach((l) => l({ step, state, message, done, total }));
  };
  const finish = (result: SetupDone) => {
    preparing = null;
    doneListeners.forEach((l) => l(result));
  };
  const plan: Array<() => void> = [
    () => emit("ocr", "running", "Comprobando el OCR incluido"),
    () => emit("ocr", "done", "Tesseract 5.4.0.20240606 con spa y eng"),
    () => emit("ollama", "skipped", "Ollama 0.40.14 ya instalado"),
    () => emit("service", "skipped", "Ollama ya está en marcha"),
  ];
  const total = 5_200_000_000;
  if (!modelPresent) {
    for (let i = 1; i <= 4; i++) plan.push(() => emit("model", "running", "Descargando qwen3:8b", (total / 8) * i, total));
    plan.push(() => emit("model", "running", "Reintento 2 de 3: la conexión se cortó"));
    for (let i = 5; i <= 8; i++) plan.push(() => emit("model", "running", "Descargando qwen3:8b", (total / 8) * i, total));
    plan.push(() => ((modelPresent = true), emit("model", "done", "Modelo qwen3:8b descargado")));
  } else {
    plan.push(() => emit("model", "skipped", "El modelo qwen3:8b ya está descargado"));
  }
  plan.push(() => emit("verify", "running", "Pidiendo una respuesta al modelo local"));
  const timesOut = memory().level === "insufficient" && verifyAttempts++ === 0;
  if (!timesOut) plan.push(() => emit("verify", "done", "El modelo respondió «listo»"));
  let step = 0;
  const tick = () => {
    if (run.cancelled) {
      emit(last, "failed", "Preparación cancelada");
      return finish({ report: null, error: { code: "cancelled", message: "Preparación cancelada" } });
    }
    const next = plan[step++];
    if (next) {
      next();
      return void setTimeout(tick, 350);
    }
    if (timesOut) {
      const message = `timeout: el proveedor no respondió a tiempo, probablemente por falta de memoria: el equipo tiene ${ramGb.toFixed(1).replace(".", ",")} GB y la IA local necesita 16 GB; puede elegir Groq en Configuración`;
      emit("verify", "failed", message);
      return finish({ report: null, error: { code: "verify_failed", message } });
    }
    finish({
      report: {
        ollama_version: "0.40.14",
        ollama_installed: false,
        model: "qwen3:8b",
        model_digest: "500a1f067a9f",
        answer: "listo",
        latency_ms: memory().level === "recommended" ? 2400 : 94_000,
        memory: memory(),
      },
      error: null,
    });
  };
  setTimeout(tick, 200);
}

function task(kind: string, input: object, state: Task["state"], result: Task["result"], error: Task["error"] = null) {
  const t: Task = {
    id: nextId++,
    kind,
    input_ref: JSON.stringify(input),
    state,
    attempts: state === "queued" ? 0 : 1,
    result,
    error,
    created_at: now(),
    updated_at: now(),
  };
  tasks.unshift(t);
  return t;
}

const invoiceResult = (document: string, total: string, issues: string[]) => ({
  type: "invoice",
  version: 1,
  document,
  fields: {
    supplier: "Suministros Alfa SL",
    invoice_number: "FS-2026-014",
    date: "2026-10-01",
    subtotal: "100.00",
    tax: "21.00",
    total,
    currency: "EUR",
  },
  issues,
  duplicates: [],
  unreadable: [],
  pages: [{ number: 1, method: issues.length ? "ocr" : "pdf_text", chars: 120 }],
  provider: "ollama",
  model: "qwen3:8b",
  latency_ms: 15200,
  review_reason: issues.length ? "la factura tiene incidencias" : "ninguna regla activa coincide",
  action: { kind: "register_invoice" },
});

task("invoice", { document: "factura-alfa.txt" }, "awaiting_review", invoiceResult("factura-alfa.txt", "121.00", []));
task("invoice", { document: "factura-beta.txt" }, "awaiting_review", invoiceResult("factura-beta.txt", "125.00", ["total_mismatch"]));
task("email", { message_id: "M-0002" }, "awaiting_review", {
  type: "email",
  version: 1,
  message: {
    id: "M-0002",
    subject: "Consulta sobre el pedido 2231",
    from: "cliente.beta@ejemplo.test",
    body: "¿Podrían confirmar la fecha de entrega del pedido 2231?",
  },
  category: "consulta_pago",
  draft: {
    to: ["cliente.beta@ejemplo.test"],
    subject: "Re: Consulta sobre el pedido 2231",
    body: "Buenos días:\n\nRevisamos el pedido 2231 y le confirmaremos la fecha de entrega en cuanto la tengamos.\n\nUn saludo.",
  },
  provider: "ollama",
  model: "qwen3:8b",
  review_reason: "ninguna regla activa coincide",
  action: { kind: "send_draft" },
});
task("invoice", { document: "escaneada.txt" }, "failed", null, {
  code: "unavailable",
  message: "unavailable: no se pudo conectar con Ollama",
  recoverable: true,
});
task("email", { message_id: "M-0001" }, "queued", null);

function find(id: number) {
  const t = tasks.find((t) => t.id === id);
  if (!t) throw `tarea ${id} inexistente`;
  return t;
}

function reviewable(id: number) {
  const t = find(id);
  if (t.state !== "awaiting_review") throw `la tarea ${id} no está pendiente de revisión`;
  return t;
}

function latest(): Rule[] {
  const map = new Map<string, Rule>();
  for (const r of rules) map.set(r.id, r);
  return [...map.values()];
}

function addRule(rule: Rule) {
  rules.push(rule);
  audit.push({
    task_id: null,
    action: "rule",
    status: rule.active ? "active" : "inactive",
    authorization: null,
    detail: `${rule.id} v${rule.version} (${rule.action_kind})`,
  });
  return rule;
}

const handlers: Record<string, (a: any, options?: any) => unknown> = {
  tasks: () => structuredClone(tasks),
  import_file: (bytes: Uint8Array, options?: { headers: Record<string, string> }) => {
    const name = decodeURIComponent(options?.headers["x-nestadmin-name"] ?? "");
    if (!/\.(txt|pdf)$/i.test(name)) throw `«${name}»: solo se admiten archivos .txt y .pdf`;
    if (bytes.length === 0) throw `«${name}» está vacío`;
    return task("invoice", { document: name }, "queued", null);
  },
  fetch_mail: () => 0,
  approve: ({ id, version }) => {
    const t = reviewable(id);
    const current = t.result?.version ?? 1;
    if (current !== version) throw `el contenido cambió (versión ${current}, se aprobó la ${version}); revíselo de nuevo`;
    const action = t.kind === "invoice" ? "register_invoice" : "send_draft";
    audit.push({ task_id: id, action, status: "confirmed", authorization: "aprobación", detail: null });
    t.state = "completed";
    t.result = { ...t.result, receipt: { simulated: true } };
    return t;
  },
  edit_draft: ({ id, body }) => {
    const t = reviewable(id);
    if (!String(body).trim()) throw "el borrador está vacío";
    t.result = { ...t.result, version: (t.result?.version ?? 1) + 1, draft: { ...t.result?.draft, body } };
    return t;
  },
  reject: ({ id }) => Object.assign(reviewable(id), { state: "cancelled" }),
  retry: ({ id }) => {
    const t = find(id);
    if (t.state !== "failed") throw "solo se reintentan tareas fallidas";
    return Object.assign(t, { state: "queued" });
  },
  rules: () => latest(),
  create_rule: ({ id, actionKind, conditions }) => {
    if (latest().some((r) => r.id === id)) throw `la regla ${id} ya existe`;
    return addRule({ id, version: 1, action_kind: actionKind, conditions, active: false, created_at: now() });
  },
  update_rule: ({ id, conditions }) => {
    const r = latest().find((r) => r.id === id);
    if (!r) throw `regla ${id} inexistente`;
    return addRule({ ...r, version: r.version + 1, conditions, created_at: now() });
  },
  set_rule_active: ({ id, active }) => {
    const r = latest().find((r) => r.id === id);
    if (!r) throw `regla ${id} inexistente`;
    return r.active === active ? r : addRule({ ...r, version: r.version + 1, active, created_at: now() });
  },
  audit_log: () => structuredClone(audit),
  settings: (): SettingsView => ({
    choice,
    active: { provider: choice.provider, model: choice.model },
    groq_key_saved: groqKey,
    data_dir: "%LOCALAPPDATA%\\NestAdmin (vista previa: datos en memoria)",
  }),
  select_provider: ({ choice: next }) => {
    if (next.provider === "groq" && !groqKey) throw "unauthorized: credencial ausente o rechazada";
    choice = next;
    return { provider: next.provider, model: next.model };
  },
  save_groq_key: ({ key }) => {
    if (!String(key).trim()) throw "la clave está vacía";
    groqKey = true;
  },
  delete_groq_key: () => {
    groqKey = false;
  },
  launch_mode: () => ({
    prepare_only: prepareOnly,
    preparing: preparing !== null,
    startup: prepareOnly ? null : startup(),
  }),
  setup_check: () => ({ ...(modelPresent ? { status: "ready" } : { status: "missing", component: "model" }), memory: memory() }),
  setup_prepare: () => {
    if (preparing) throw "la preparación ya está en curso";
    preparing = { cancelled: false };
    simulatePreparation(preparing);
  },
  setup_cancel: () => {
    if (preparing) preparing.cancelled = true;
  },
  setup_abandon: () => {
    if (preparing) preparing.cancelled = true;
  },
};

function startup(): Startup {
  if (choice.provider !== "ollama") return { outcome: "not_local" };
  return modelPresent ? { outcome: "ready" } : { outcome: "needs_preparation", check: { status: "missing", component: "model" } };
}

const mutating = new Set(["import_file", "approve", "edit_draft", "reject", "retry", "create_rule", "update_rule", "set_rule_active"]);

export async function invoke<T>(
  command: string,
  args: Record<string, unknown> | Uint8Array = {},
  options?: { headers: Record<string, string> },
): Promise<T> {
  const handler = handlers[command];
  if (!handler) throw `comando desconocido: ${command}`;
  const value = handler(args, options) as T;
  if (mutating.has(command)) changed();
  return structuredClone(value);
}

export function onChanged(handler: () => void) {
  listeners.push(handler);
}

export function onSetup(handler: (event: SetupEvent) => void) {
  setupListeners.push(handler);
}

export function onSetupDone(handler: (done: SetupDone) => void) {
  doneListeners.push(handler);
}

export function onStartup(handler: (startup: Startup) => void) {
  startupListeners.push(handler);
}
