// Servidor simulado para ver la interfaz en un navegador sin Tauri. Imita los comandos
// reales con datos sintéticos (dominio .test); no decide nada que la capa Rust no decida.

import type { AuditEntry, ComponentStatus, ProviderChoice, Rule, SettingsView, SetupComponent, SetupEvent, Task } from "./types";

const now = () => Date.now();
const listeners: Array<() => void> = [];
const changed = () => setTimeout(() => listeners.forEach((l) => l()), 0);

let nextId = 1;
const tasks: Task[] = [];
const rules: Rule[] = [];
const audit: AuditEntry[] = [];
let choice: ProviderChoice = { provider: "ollama", base_url: "http://127.0.0.1:11434", model: "qwen3:8b" };
let groqKey = false;

// Preparación simulada: falta Tesseract y el modelo; la instalación avanza por pasos.
const setupListeners: Array<(event: SetupEvent) => void> = [];
let installing: SetupComponent | null = null;
const components: ComponentStatus[] = [
  { component: "webview2", state: "present", version: "130.0.2849.80", installable: false, detail: "Motor de la ventana de Windows." },
  { component: "pdfium", state: "present", version: null, installable: false, detail: "Incluido con NestAdmin." },
  { component: "tessdata", state: "present", version: "spa, eng", installable: false, detail: "Datos de idioma del OCR, incluidos con NestAdmin." },
  { component: "tesseract", state: "missing", installable: true, detail: "Necesario para PDF escaneados (unos 50 MB; pedirá permiso de administrador)." },
  { component: "ollama", state: "present", version: "0.40.14", installable: true, detail: "Servicio de IA local en 127.0.0.1." },
  { component: "model", state: "missing", installable: true, detail: "Modelo qwen3:8b (unos 5,2 GB)." },
];

function simulateInstall(component: SetupComponent) {
  const total = component === "model" ? 5_200_000_000 : 50_000_000;
  let done = 0;
  const emit = (event: SetupEvent) => setupListeners.forEach((l) => l(event));
  const tick = () => {
    done = Math.min(total, done + total / 8);
    emit({ component, progress: { component, step: "Descargando", done, total }, finished: false, error: null });
    if (done < total) return void setTimeout(tick, 300);
    const status = components.find((c) => c.component === component)!;
    Object.assign(status, { state: "present", version: component === "model" ? "500a1f067a9f" : "5.4.0.20240606" });
    installing = null;
    emit({ component, progress: null, finished: true, error: null });
  };
  setTimeout(tick, 300);
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
  setup_status: () => ({ components: structuredClone(components), installing }),
  setup_install: ({ component }) => {
    if (installing) throw `ya se está preparando ${installing}; espere a que termine`;
    const status = components.find((c) => c.component === component);
    if (!status?.installable) throw "este componente se incluye con el instalador de NestAdmin; reinstálelo";
    if (status.state === "present") {
      // Como en Rust: lo ya presente termina enseguida sin tocarlo.
      setTimeout(() => setupListeners.forEach((l) => l({ component, progress: null, finished: true, error: null })), 0);
      return;
    }
    installing = component;
    simulateInstall(component);
  },
  setup_verify: () => {
    if (components.find((c) => c.component === "model")?.state !== "present") throw "IA local: model 'qwen3:8b' not found";
    return { model: "qwen3:8b", answer: "listo", latency_ms: 2400 };
  },
};

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
