// Servidor simulado para ver la interfaz en un navegador sin Tauri. Imita los comandos
// reales con datos sintéticos (dominio .test); no decide nada que la capa Rust no decida.

import type {
  AccountFetch,
  AuditEntry,
  DocumentOrigin,
  MailOrigin,
  FetchReport,
  MailAccount,
  MailProviderKind,
  MailView,
  Memory,
  ProviderChoice,
  Rule,
  SettingsView,
  SetupDone,
  SetupEvent,
  SetupStep,
  Startup,
  StepState,
  Task,
} from "./types";

const now = () => Date.now();
const listeners: Array<() => void> = [];
const changed = () => setTimeout(() => listeners.forEach((l) => l()), 0);

let nextId = 1;
const tasks: Task[] = [];
const rules: Rule[] = [];
const audit: AuditEntry[] = [];
let choice: ProviderChoice = { provider: "ollama", base_url: "http://127.0.0.1:11434", model: "qwen3:8b" };
let groqKey = false;

// Cuentas de correo simuladas. `?sin-cuentas` en la URL abre la pantalla sin ninguna cuenta y
// `?sin-cliente` imita una instalación sin clientes OAuth del editor. Iniciar sesión espera
// unos segundos como si el usuario estuviera en el navegador; no se abre ninguna página.
const params = new URLSearchParams(location.search);
const clientsConfigured = !params.has("sin-cliente");
let mailLimit = 25;
let signingIn: { cancelled: boolean } | null = null;
const signInListeners: Array<(url: string) => void> = [];
const mailAccounts: MailAccount[] = params.has("sin-cuentas")
  ? []
  : [
      {
        id: "simulador",
        provider: "simulator",
        label: "Correo simulado",
        address: "administracion@nestadmin.test",
        status: "connected",
        last_fetch: null,
      },
      {
        id: "microsoft-1",
        provider: "microsoft",
        label: "Oficina",
        address: "oficina@empresa.test",
        status: clientsConfigured ? "connected" : "not_configured",
        last_fetch: { account: "microsoft-1", read: 25, added: 3, error: null },
      },
      {
        id: "google-1",
        provider: "google",
        label: "Facturación",
        address: "facturas@empresa.test",
        status: clientsConfigured ? "signed_out" : "not_configured",
        last_fetch: {
          account: "google-1",
          read: 0,
          added: 0,
          error: { code: "unauthorized", message: "la sesión caducó; vuelva a iniciar sesión" },
        },
      },
    ];

function nextAccountId(provider: MailProviderKind) {
  const used = mailAccounts
    .map((a) => a.id.match(new RegExp(`^${provider}-(\\d+)$`))?.[1])
    .filter(Boolean)
    .map(Number);
  return `${provider}-${Math.max(0, ...used) + 1}`;
}

function signIn(provider: MailProviderKind, label: string, account: string | null): Promise<MailAccount> {
  if (signingIn) return Promise.reject("ya hay un inicio de sesión en curso");
  if (provider === "simulator" || !clientsConfigured)
    return Promise.reject(`unavailable: esta instalación no tiene configurado el cliente OAuth de ${provider === "google" ? "Google" : "Microsoft"}`);
  const run = { cancelled: false };
  signingIn = run;
  const host = provider === "google" ? "accounts.google.com" : "login.microsoftonline.com";
  setTimeout(() => signInListeners.forEach((l) => l(`https://${host}/… (vista previa: no se abre ninguna página)`)), 0);
  return new Promise((resolve, reject) => {
    setTimeout(() => {
      signingIn = null;
      if (run.cancelled) return reject("inicio de sesión cancelado");
      const existing = account ? mailAccounts.find((a) => a.id === account) : undefined;
      if (existing) {
        Object.assign(existing, { status: "connected", last_fetch: null });
        changed();
        return resolve(structuredClone(existing));
      }
      const id = nextAccountId(provider);
      const created: MailAccount = {
        id,
        provider,
        label: label.trim() || `${provider === "google" ? "Google" : "Microsoft"} ${id.split("-")[1]}`,
        // Como `users.getProfile` o `/me` en la aplicación (D48).
        address: `buzon-${id}@empresa.test`,
        status: "connected",
        last_fetch: null,
      };
      mailAccounts.push(created);
      changed();
      resolve(structuredClone(created));
    }, 2500);
  });
}

function fetchMail(): FetchReport {
  const accounts: AccountFetch[] = mailAccounts.map((a) => {
    const fetch: AccountFetch =
      a.status === "connected"
        ? { account: a.id, read: Math.min(mailLimit, a.provider === "simulator" ? 5 : 2), added: 0, error: null }
        : {
            account: a.id,
            read: 0,
            added: 0,
            error:
              a.status === "not_configured"
                ? { code: "unavailable", message: "esta instalación no tiene configurado el cliente OAuth" }
                : { code: "unauthorized", message: "la sesión caducó; vuelva a iniciar sesión" },
          };
    if (a.status === "connected" && a.provider !== "simulator") {
      const n = tasks.filter((t) => t.input_ref.includes(`"${a.id}:`)).length + 1;
      task("email", { message_id: `${a.id}:sintetico-${n}`, account: a.id, origin: origin(a.id) }, "queued", null);
      fetch.added = 1;
    }
    a.last_fetch = fetch;
    return fetch;
  });
  return { added: accounts.reduce((sum, a) => sum + a.added, 0), limit: mailLimit, accounts };
}

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

const origin = (account: string): MailOrigin => {
  const a = mailAccounts.find((m) => m.id === account);
  return a
    ? { account, provider: a.provider, label: a.label, address: a.address }
    : { account, provider: "google", label: "Antigua cuenta", address: null };
};

// Huella sintética estable para la vista previa (no es la de ningún archivo real).
const fakeSha = (seed: string) =>
  Array.from({ length: 64 }, (_, i) => "0123456789abcdef"[(seed.charCodeAt(i % seed.length) * (i + 7)) % 16]).join("");

const fileOrigin = (name: string, path: string | null, daysAgo: number): DocumentOrigin => ({
  kind: "file",
  name,
  original_path: path,
  imported_at: Math.floor(now() / 1000) - daysAgo * 86400,
  sha256: fakeSha(name),
  size: 48_213,
});

const invoiceResult = (document: string, total: string, issues: string[], source: DocumentOrigin | null) => ({
  type: "invoice",
  version: 1,
  document,
  origin: source,
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
  pages: issues.length
    ? [
        { number: 1, method: "ocr", chars: 120, offset: 0 },
        { number: 2, method: "ocr", chars: 80, offset: 140 },
      ]
    : [{ number: 1, method: "pdf_text", chars: 120, offset: 0 }],
  // Dónde aparece cada campo en el texto; la moneda no aparece literalmente.
  sources: {
    supplier: { pages: [1], text: "Suministros Alfa SL", line: "Suministros Alfa SL · CIF B00000000" },
    invoice_number: { pages: [1], text: "FS-2026-014", line: "Factura n.º FS-2026-014" },
    date: { pages: [1], text: "01/10/2026", line: "Fecha: 01/10/2026" },
    subtotal: { pages: [issues.length ? 2 : 1], text: "100,00", line: "Base imponible 100,00" },
    tax: { pages: [issues.length ? 2 : 1], text: "21,00", line: "IVA 21 % 21,00" },
    total: { pages: [issues.length ? 2 : 1], text: total.replace(".", ","), line: `TOTAL ${total.replace(".", ",")} €` },
  },
  provider: "ollama",
  model: "qwen3:8b",
  latency_ms: 15200,
  review_reason: issues.length ? "la factura tiene incidencias" : "ninguna regla activa coincide",
  action: { kind: "register_invoice" },
});

task(
  "invoice",
  { document: "factura-alfa.pdf" },
  "awaiting_review",
  invoiceResult("factura-alfa.pdf", "121.00", [], fileOrigin("factura-alfa.pdf", "C:\\Users\\Usuario\\Documents\\Facturas\\factura-alfa.pdf", 1)),
);
task(
  "invoice",
  { document: "factura-beta-escaneada.pdf" },
  "awaiting_review",
  invoiceResult("factura-beta-escaneada.pdf", "125.00", ["total_mismatch"], {
    ...fileOrigin("factura-beta-escaneada.pdf", null, 0),
    kind: "email",
    account: origin("google-1"),
    message_id: "google-1:18c2-sintetico",
    from: "facturas@proveedor-alfa.test",
    subject: "Factura FS-2026-015",
    attachment: "factura-beta-escaneada.pdf",
  } as DocumentOrigin),
);
// Importado antes de que se registrara el origen: se dice que no consta.
task("invoice", { document: "factura-antigua.txt" }, "awaiting_review", invoiceResult("factura-antigua.txt", "121.00", [], null));
task("email", { message_id: "simulador:M-0005", account: "simulador" }, "awaiting_review", {
  type: "email",
  version: 1,
  origin: origin("simulador"),
  message: {
    id: "simulador:M-0005",
    subject: "Cambio de horario",
    from: "logistica@transportes-delta.test",
    to: ["administracion@nestadmin.test"],
    body: "El horario de recogida cambia a partir del lunes.",
    date: "2026-10-06",
  },
  category: "otros",
  draft: {
    to: ["logistica@transportes-delta.test"],
    subject: "Re: Cambio de horario",
    body: "Gracias por el aviso; tomamos nota del nuevo horario de recogida.\n\nUn saludo.",
  },
  provider: "ollama",
  model: "qwen3:8b",
  review_reason: "ninguna regla activa coincide",
  action: { kind: "send_draft" },
});
task("email", { message_id: "microsoft-1:AAMk-sintetico-2", account: "microsoft-1" }, "awaiting_review", {
  type: "email",
  version: 1,
  origin: origin("microsoft-1"),
  message: {
    id: "microsoft-1:AAMk-sintetico-2",
    subject: "Consulta sobre el pedido 2231",
    from: "cliente.beta@ejemplo.test",
    to: ["oficina@empresa.test", "compras@empresa.test"],
    body: "¿Podrían confirmar la fecha de entrega del pedido 2231?",
    date: "2026-11-04T09:12:00Z",
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
task("email", { message_id: "simulador:M-0001", account: "simulador" }, "queued", null);

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
    if (!/\.(txt|pdf|jpe?g|png)$/i.test(name)) throw `«${name}»: solo se admiten archivos .txt, .pdf y fotos .jpg o .png`;
    if (bytes.length === 0) throw `«${name}» está vacío`;
    // El navegador no da la ruta original: queda como «no consta», igual que en la aplicación.
    return task("invoice", { document: name, origin: { ...fileOrigin(name, null, 0), size: bytes.length } }, "queued", null);
  },
  fetch_mail: () => fetchMail(),
  open_original: ({ id }) => {
    if (find(id).kind !== "invoice") throw `la tarea ${id} no tiene documento`;
  },
  mail_accounts: (): MailView => ({
    accounts: mailAccounts,
    limit: mailLimit,
    google_configured: clientsConfigured,
    microsoft_configured: clientsConfigured,
    clients_error: null,
    signing_in: signingIn !== null,
  }),
  set_mail_limit: ({ limit }) => {
    if (!Number.isInteger(limit) || limit < 1 || limit > 500) throw "el límite de mensajes debe estar entre 1 y 500";
    mailLimit = limit;
  },
  mail_sign_in: ({ provider, label, account }) => signIn(provider, String(label ?? ""), account ?? null),
  mail_sign_in_cancel: () => {
    if (signingIn) signingIn.cancelled = true;
  },
  remove_mail_account: ({ id }) => {
    const index = mailAccounts.findIndex((a) => a.id === id);
    if (index < 0) throw `not_found: la cuenta ${id} no está conectada en NestAdmin`;
    mailAccounts.splice(index, 1);
  },
  rename_mail_account: ({ id, label }) => {
    const account = mailAccounts.find((a) => a.id === id);
    if (!account) throw `not_found: la cuenta ${id} no está conectada en NestAdmin`;
    const text = String(label ?? "").trim().slice(0, 60);
    // Mismo nombre predeterminado que `account_label` en Rust.
    account.label =
      text || (account.provider === "simulator" ? "Correo simulado" : `${account.provider === "google" ? "Google" : "Microsoft"} ${id.split("-")[1]}`);
    return account;
  },
  add_simulated_mail_account: () => {
    let account = mailAccounts.find((a) => a.id === "simulador");
    if (!account) {
      account = {
        id: "simulador",
        provider: "simulator",
        label: "Correo simulado",
        address: "administracion@nestadmin.test",
        status: "connected",
        last_fetch: null,
      };
      mailAccounts.unshift(account);
    }
    return account;
  },
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

const mutating = new Set([
  "import_file",
  "fetch_mail",
  "approve",
  "edit_draft",
  "reject",
  "retry",
  "create_rule",
  "update_rule",
  "set_rule_active",
  "remove_mail_account",
  "rename_mail_account",
  "add_simulated_mail_account",
]);

export async function invoke<T>(
  command: string,
  args: Record<string, unknown> | Uint8Array = {},
  options?: { headers: Record<string, string> },
): Promise<T> {
  const handler = handlers[command];
  if (!handler) throw `comando desconocido: ${command}`;
  const value = (await handler(args, options)) as T;
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

export function onMailSignIn(handler: (url: string) => void) {
  signInListeners.push(handler);
}
