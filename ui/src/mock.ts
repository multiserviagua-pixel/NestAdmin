// Servidor simulado para ver la interfaz en un navegador sin Tauri. Imita los comandos
// reales con datos sintéticos (dominio .test); no decide nada que la capa Rust no decida.

import type {
  AccountFetch,
  AiWait,
  AuditEntry,
  DocumentOrigin,
  MailOrigin,
  FetchReport,
  MailAccount,
  MailProviderKind,
  MailView,
  Memory,
  MergeOption,
  MergePlan,
  MergePreview,
  PlanRule,
  ProviderChoice,
  Rule,
  RuleFinding,
  RuleSeed,
  RuleStats,
  RuleView,
  SettingsView,
  Simulation,
  Suggestion,
  Suggestions,
  ThresholdSettings,
  SimulationItem,
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
// Escenarios de IA (D100–D102, D122) por la URL, como `?sin-cuentas` en el correo:
// - sin parámetros: Ollama cloud con clave y destino confirmados;
// - `?sin-ia`: instalación nueva (sin clave ni confirmación): abre el asistente de primer uso;
// - `?local`: quien tenía la IA local (con la oferta única de pasar al externo);
// - `?clave-revocada`: la clave guardada fue rechazada (aviso sin asistente);
// - `?modelo-retirado`: el modelo guardado ya no aparece en la lista;
// - `?sin-ollama`: la IA local no responde; `?sin-respaldo`: respaldo desactivado;
// - `?periodo-respaldo`: periodo de 10 minutos tras un fallo del externo;
// - `?prueba-falla`: la prueba sintética devuelve un JSON no válido (D105).
// Claves sintéticas: una que empiece por «gsk-invalida» o «invalida» da `unauthorized`, y una
// que empiece por «sin-red» da `unavailable`.
const aiParams = new URLSearchParams(location.search);
const CLOUD_DEFAULT = "gemma4:31b"; // espejo de DEFAULT_CLOUD_MODEL (solo vista previa)
const freshInstall = aiParams.has("sin-ia") || aiParams.has("local");
let choice: ProviderChoice | null = aiParams.has("local")
  ? { provider: "ollama", base_url: "http://127.0.0.1:11434", model: "qwen3:8b" }
  : { provider: "ollama_cloud", model: aiParams.has("modelo-retirado") ? "gemma3:12b-retirado" : CLOUD_DEFAULT };
// Nombres de credencial guardados: «ollama_cloud», «anthropic» o «compat:<host>».
const savedKeys = new Set<string>(freshInstall ? [] : ["ollama_cloud"]);
// Destinos confirmados: «<proveedor>|<destino>».
const consents = new Set<string>(freshInstall ? [] : ["ollama_cloud|ollama.com"]);
let localFallback = !aiParams.has("sin-respaldo");
let offerShown = false;
let keyRevoked = aiParams.has("clave-revocada");

function credentialName(c: ProviderChoice): string | null {
  switch (c.provider) {
    case "ollama":
      return null;
    case "ollama_cloud":
      return "ollama_cloud";
    case "anthropic":
      return "anthropic";
    case "openai_compatible":
      return `compat:${compatHost(c.base_url)}`;
  }
}

function destination(c: ProviderChoice): string | null {
  switch (c.provider) {
    case "ollama":
      return null;
    case "ollama_cloud":
      return "ollama.com";
    case "anthropic":
      return "api.anthropic.com";
    case "openai_compatible":
      return compatHost(c.base_url);
  }
}

function consentKey(c: ProviderChoice): string | null {
  const d = destination(c);
  return d ? `${c.provider}|${d}` : null;
}

function aiWait(): AiWait | null {
  if (!choice) return "needs_choice";
  const key = consentKey(choice);
  if (key === null) return null;
  if (!consents.has(key)) return "needs_consent";
  if (!savedKeys.has(credentialName(choice) ?? "")) return "needs_key";
  return keyRevoked ? "invalid_key" : null;
}

const AI_WAIT_MESSAGE: Record<AiWait, string> = {
  needs_choice: "Falta configurar la IA: elija un proveedor en Configuración",
  needs_key: "Falta configurar la IA: guarde la clave del proveedor en Configuración",
  needs_consent: "Falta configurar la IA: confirme el destino de los datos en Configuración",
  invalid_key: "La clave de IA no es válida o fue revocada: corríjala en Configuración",
};

const MODEL_LISTS: Record<string, string[]> = {
  ollama_cloud: [CLOUD_DEFAULT, "gpt-oss:120b", "mistral-large-4", "deepseek-v4.1-flash"],
  anthropic: ["claude-opus-5-5", "claude-sonnet-5-5", "claude-haiku-5-5"],
  openai_compatible: ["modelo-sintetico-grande", "modelo-sintetico-pequeno"],
};

/// Réplica reducida de la validación de URL base del adaptador (D109).
function compatHost(baseUrl: string): string {
  const raw = String(baseUrl).trim();
  if (!/^https:\/\//i.test(raw)) throw "unavailable: URL base no admitida: solo se admite https://";
  let url: URL;
  try {
    url = new URL(raw);
  } catch {
    throw "unavailable: URL base no admitida: host no válido";
  }
  if (url.username || url.password) throw "unavailable: URL base no admitida: no puede llevar usuario ni contraseña";
  if (url.search || url.hash) throw "unavailable: URL base no admitida: no puede llevar query ni fragmento";
  const host = url.hostname.toLowerCase().replace(/\.$/, "");
  const local =
    !host.includes(".") ||
    /(^|\.)(localhost|local|internal|lan|localdomain)$/.test(host) ||
    /^(127\.|10\.|0\.|192\.168\.|169\.254\.|172\.(1[6-9]|2\d|3[01])\.)/.test(host) ||
    host.startsWith("[");
  if (local) throw "unavailable: URL base no admitida: apunta a este equipo o a la red local";
  return host;
}

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
        last_fetch: { account: "microsoft-1", read: 25, added: 3, error: null, attachments: { imported: 2, skipped: [], failed: [] } },
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
          attachments: { imported: 0, skipped: [], failed: [] },
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
        ? {
            account: a.id,
            read: Math.min(mailLimit, a.provider === "simulator" ? 5 : 2),
            added: 0,
            error: null,
            attachments: { imported: 0, skipped: [], failed: [] },
          }
        : {
            account: a.id,
            read: 0,
            added: 0,
            attachments: { imported: 0, skipped: [], failed: [] },
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
      ? `Este equipo tiene ${gb(total)} GB de memoria. Con 8 GB o menos, la IA local (Qwen3 8B) probablemente no responderá a tiempo. Puede intentarlo igualmente o elegir un proveedor externo en Configuración; NestAdmin no lo cambiará por su cuenta.`
      : level === "below_recommended"
        ? `Este equipo tiene ${gb(total)} GB de memoria; se recomiendan 16 GB para la IA local. Puede seguir, pero las respuestas serán más lentas y alguna podría no llegar a tiempo; si ocurre, puede elegir un proveedor externo en Configuración.`
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
      const message = `timeout: el proveedor no respondió a tiempo, probablemente por falta de memoria: el equipo tiene ${ramGb.toFixed(1).replace(".", ",")} GB y la IA local necesita 16 GB; puede elegir un proveedor externo en Configuración`;
      emit("verify", "failed", message);
      return finish({ report: null, error: { code: "verify_failed", message } });
    }
    finish({
      report: {
        ollama_version: "0.40.14",
        ollama_installed: false,
        model: "qwen3:8b",
        model_digest: "500a1f067a9f",
        model_pinned: true,
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
  // Comprobación local de moneda (D127): solo códigos y booleanos.
  currency_evidence: { foreign: [] as string[], euro: !issues.length, ocr: issues.length > 0 },
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
  (() => {
    const result = invoiceResult("factura-beta-escaneada.pdf", "125.00", ["total_mismatch"], {
      ...fileOrigin("factura-beta-escaneada.pdf", null, 0),
      kind: "email",
      account: origin("google-1"),
      message_id: "google-1:18c2-sintetico",
      from: "facturas@proveedor-alfa.test",
      subject: "Factura FS-2026-015",
      attachment: "factura-beta-escaneada.pdf",
    } as DocumentOrigin);
    // Nombre largo y OCR de baja calidad en una página (D93).
    result.fields.supplier = "Distribuciones y Suministros Industriales Beta del Norte, Sociedad Limitada";
    result.fields.invoice_number = "FS-2026-015";
    (result.pages[1] as Record<string, unknown>).low_confidence = true;
    return result;
  })(),
);
// Importado antes de que se registrara el origen y con datos sin leer: se dice que no consta.
task(
  "invoice",
  { document: "factura-antigua.txt" },
  "awaiting_review",
  (() => {
    const result = invoiceResult("factura-antigua.txt", "121.00", [], null) as { fields: Record<string, string | null> };
    Object.assign(result.fields, { supplier: null, invoice_number: null, date: null, total: null, currency: null });
    return result;
  })(),
);
// `?muchos`: lista grande, para comprobar el desplazamiento de la lista y del detalle.
if (new URLSearchParams(location.search).has("muchos")) {
  for (let i = 1; i <= 24; i++) {
    const name = `factura-lote-${i}.pdf`;
    task("invoice", { document: name }, "awaiting_review", invoiceResult(name, `${100 + i}.00`, [], fileOrigin(name, null, i)));
  }
}
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
// Importe dudoso (D120): se pide confirmarlo con el documento antes de aprobar.
task("invoice", { document: "factura-dudosa.txt" }, "awaiting_review", {
  ...invoiceResult("factura-dudosa.txt", "1210.00", ["amount_to_review"], fileOrigin("factura-dudosa.txt", "C:\\Users\\Usuario\\Documents\\Facturas\\factura-dudosa.txt", 2)),
  fallback: { from: "ollama_cloud", reason: "rate_limited" },
  fields: { supplier: "Suministros Alfa SL", invoice_number: "FS-2026-015", date: "2026-10-02", subtotal: "1000.00", tax: "210.00", total: "1210.00", currency: "EUR" },
  document_path: "C:\\Users\\demo\\AppData\\Local\\NestAdmin\\documentos\\1793816133.txt",
  recovered: [{ field: "subtotal", literal: "1.000", evidence: "1.000", value: "1000.00", reason: "ambiguous_separator" }],
  amount_review:
    "Revise los importes en «factura-dudosa.txt», copia en «C:\\Users\\demo\\AppData\\Local\\NestAdmin\\documentos\\1793816133.txt»: Base imponible leído como «1.000», se propone 1000.00 (el punto o la coma podría ser de miles). Compruébelos con el documento; solo se registra con su aprobación.",
  review_reason:
    "Revise los importes en «factura-dudosa.txt», copia en «C:\\Users\\demo\\AppData\\Local\\NestAdmin\\documentos\\1793816133.txt»: Base imponible leído como «1.000», se propone 1000.00 (el punto o la coma podría ser de miles). Compruébelos con el documento; solo se registra con su aprobación.",
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
  fallback: { from: "ollama_cloud", reason: "unavailable" },
  review_reason: "ninguna regla activa coincide",
  action: { kind: "send_draft" },
});
// Envío reintentado cuyo borrador borró el usuario (D50): se conserva el texto aprobado y hay
// que confirmar que no se envió antes de preparar uno nuevo.
task("email", { message_id: "google-1:18c2-sintetico-7", account: "google-1" }, "awaiting_review", {
  type: "email",
  version: 1,
  message: {
    id: "google-1:18c2-sintetico-7",
    subject: "Solicitud de duplicado de factura",
    from: "compras@cliente-delta.test",
    body: "¿Pueden reenviarnos el duplicado de la factura FS-2026-011?",
  },
  category: "solicitud_factura",
  draft: {
    to: ["compras@cliente-delta.test"],
    subject: "Re: Solicitud de duplicado de factura",
    body: "Buenos días:\n\nAdjuntamos el duplicado de la factura FS-2026-011.\n\nUn saludo.",
  },
  provider: "ollama",
  model: "qwen3:8b",
  review_reason:
    "el borrador del intento anterior ya no existe y no consta enviado; confirme que no se envió antes de preparar otro",
  attempt: { version: 1 },
  recovery: { version: 1, error: "not_found", needs_confirmation: true },
  action: { kind: "send_draft" },
});
task("invoice", { document: "escaneada.txt" }, "failed", null, {
  code: "unavailable",
  message: "unavailable: no se pudo conectar con Ollama",
  recoverable: true,
});
task("email", { message_id: "simulador:M-0001", account: "simulador" }, "queued", null);

// Correo de un remitente de correo público: una regla inactiva habría aplicado (R22).
task("email", { message_id: "google-1:18c2-sintetico-9", account: "google-1" }, "awaiting_review", {
  type: "email",
  version: 1,
  origin: origin("google-1"),
  message: {
    id: "google-1:18c2-sintetico-9",
    subject: "Duplicado de la factura FS-2026-020",
    from: "lucia.perez@gmail.com",
    to: ["oficina@empresa.test"],
    body: "Buenos días: ¿pueden enviarme otra copia de la factura FS-2026-020?",
    date: "2026-11-05T10:30:00Z",
  },
  category: "solicitud_factura",
  draft: {
    to: ["lucia.perez@gmail.com"],
    subject: "Re: Duplicado de la factura FS-2026-020",
    body: "Buenos días:\n\nLe enviamos una copia de la factura FS-2026-020.\n\nUn saludo.",
  },
  provider: "ollama",
  model: "qwen3:8b",
  review_reason: "ninguna regla activa coincide",
  action: { kind: "send_draft" },
});

// Evaluación guardada con cada tarea (D125): reglas comprobadas, si coincidían y por qué. Las
// tareas sin ella son anteriores al registro y el panel lo dice.
const check = (id: string, active: boolean, matches: boolean, mismatch: string | null = null, sender_required = false) => ({
  id,
  version: 1,
  active,
  matches,
  mismatch,
  ...(sender_required ? { sender_required } : {}),
});
function setEvaluation(documentOrSubject: string, reviewReason: string, evaluation: Record<string, unknown>) {
  const t = tasks.find((t) => t.result && (t.result.document === documentOrSubject || t.result.message?.subject === documentOrSubject));
  if (!t) return;
  t.result = { ...t.result, review_reason: reviewReason, evaluation };
}
setEvaluation("factura-alfa.pdf", "no consta la comprobación de moneda; reprocese o revise", {
  code: "currency_not_checked",
  facts: {},
  candidates: [
    check("alfa-hasta-500", true, true),
    check("beta-sin-limite", false, false, "el proveedor es «Suministros Alfa SL» y la regla pide «Beta Servicios SL»"),
  ],
});
// Tarea anterior a la comprobación de moneda: sin `currency_evidence`.
{
  const alfa = tasks.find((t) => t.result?.document === "factura-alfa.pdf");
  if (alfa?.result) delete alfa.result.currency_evidence;
}
setEvaluation("factura-beta-escaneada.pdf", "la factura tiene incidencias", {
  code: "invoice_issues",
  facts: { issues: ["total_mismatch"] },
  candidates: [
    check("alfa-hasta-500", true, false, "el proveedor es «Distribuciones y Suministros Industriales Beta del Norte, Sociedad Limitada» y la regla pide «Suministros Alfa SL»"),
    check("beta-sin-limite", false, false, "el proveedor es «Distribuciones y Suministros Industriales Beta del Norte, Sociedad Limitada» y la regla pide «Beta Servicios SL»"),
  ],
});
setEvaluation("Cambio de horario", "ninguna regla activa coincide", {
  code: "no_active_rule_matches",
  facts: {},
  candidates: [
    check("consulta-pago-cliente", true, false, "la categoría es «otros» y la regla pide «consulta_pago»"),
    check("consulta-pago-dominio", true, false, "la categoría es «otros» y la regla pide «consulta_pago»"),
    check("consulta-pago-auto", true, false, "la categoría es «otros» y la regla pide «consulta_pago»"),
    check("contesta-gmail", false, false, "la categoría es «otros» y la regla pide «solicitud_factura»"),
  ],
});
setEvaluation("Consulta sobre el pedido 2231", "2 reglas coinciden; la decisión no es inequívoca", {
  code: "several_rules_match",
  facts: { matching: 2, rule_ids: ["consulta-pago-cliente", "consulta-pago-dominio"] },
  candidates: [
    check("consulta-pago-cliente", true, true),
    check("consulta-pago-dominio", true, true),
    check("consulta-pago-auto", true, false, "la regla no indica remitente; añádalo o valide la opción avanzada", true),
    check("contesta-gmail", false, false, "la categoría es «consulta_pago» y la regla pide «solicitud_factura»"),
  ],
});
setEvaluation("Duplicado de la factura FS-2026-020", "ninguna regla activa coincide", {
  code: "no_active_rule_matches",
  facts: {},
  candidates: [
    check("contesta-gmail", false, true),
    check("consulta-pago-cliente", true, false, "la categoría es «solicitud_factura» y la regla pide «consulta_pago»"),
    check("consulta-pago-dominio", true, false, "la categoría es «solicitud_factura» y la regla pide «consulta_pago»"),
    check("consulta-pago-auto", true, false, "la categoría es «solicitud_factura» y la regla pide «consulta_pago»"),
  ],
});
{
  const dudosa = tasks.find((t) => t.result?.document === "factura-dudosa.txt");
  if (dudosa?.result)
    dudosa.result = {
      ...dudosa.result,
      evaluation: {
        code: "amount_review",
        facts: { message: dudosa.result.amount_review },
        candidates: [
          check("alfa-hasta-500", true, false, "el total 1210.00 supera el máximo 500.00"),
          check("beta-sin-limite", false, false, "el proveedor es «Suministros Alfa SL» y la regla pide «Beta Servicios SL»"),
        ],
      },
    };
}

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

// Nombre, archivado y opción avanzada de cada regla (D124, D126, D128).
const ruleMeta = new Map<string, { name: string | null; archived_at: number | null; any_sender_ack_at: number | null; origin: RuleView["origin"] }>();
const metaOf = (id: string) => ruleMeta.get(id) ?? { name: null, archived_at: null, any_sender_ack_at: null, origin: null };
const setMeta = (id: string, patch: Partial<ReturnType<typeof metaOf>>) => ruleMeta.set(id, { ...metaOf(id), ...patch });
const hasSender = (c: Record<string, unknown>) => "sender" in c || "sender_domain" in c;
const slug = (name: string) =>
  name
    .toLowerCase()
    .normalize("NFD")
    .replace(/[\u0300-\u036f]/g, "")
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 40)
    .replace(/-+$/g, "") || "regla";

function ruleView(rule: Rule): RuleView {
  const meta = metaOf(rule.id);
  const applied = tasks.filter((t) => t.result?.authorization?.rule_id === rule.id && t.state === "completed");
  const seed = seedStats.get(rule.id) ?? {};
  const stats: RuleStats = {
    rule_id: rule.id,
    applied: (seed.applied ?? 0) + applied.length,
    last_applied_at: applied.length ? Math.max(...applied.map((t) => t.updated_at)) : (seed.last_applied_at ?? null),
    reviewed: seed.reviewed ?? 0,
    unclassified: seed.unclassified ?? 0,
  };
  return { ...rule, name: meta.name ?? rule.id, archived_at: meta.archived_at, any_sender_ack_at: meta.any_sender_ack_at, origin: meta.origin, stats };
}

// Condiciones cerradas por acción (D123, D126, D127), como en el backend. `allowNoSender`: la
// opción avanzada permite una regla de envío sin remitente.
function checkConditions(kind: string, conditions: Record<string, unknown>, allowNoSender = false) {
  const allowed =
    kind === "register_invoice" ? ["supplier", "max_total", "currency"] : kind === "send_draft" ? ["category", "sender", "sender_domain"] : null;
  if (!allowed) throw `tipo de regla no admitido: ${kind}`;
  const unknown = Object.keys(conditions).find((k) => !allowed.includes(k));
  if (unknown) throw `condición no admitida: ${unknown}`;
  if (kind === "register_invoice" && !String(conditions.supplier ?? "").trim()) throw "la regla de registro necesita un proveedor";
  if (kind === "send_draft" && !String(conditions.category ?? "").trim()) throw "la regla de envío necesita una categoría";
  if ("currency" in conditions && conditions.currency !== "EUR")
    throw "Hoy la aplicación solo registra facturas en euros: la moneda de la regla solo puede ser EUR";
  if (kind === "send_draft") {
    if ("sender" in conditions && "sender_domain" in conditions) throw "indique la dirección del remitente o su dominio, no las dos cosas";
    if ("sender" in conditions && !/^[^@\s,<>]+@[A-Za-z0-9-]+(\.[A-Za-z0-9-]+)+$/.test(String(conditions.sender).trim()))
      throw "el remitente debe ser una dirección de correo";
    if ("sender_domain" in conditions && !/^[A-Za-z0-9-]+(\.[A-Za-z0-9-]+)+$/.test(String(conditions.sender_domain).trim()))
      throw "el dominio del remitente debe ser como proveedor.example";
    if (!hasSender(conditions) && !allowNoSender)
      throw "la regla de envío necesita un remitente (dirección o dominio); sin él solo puede guardarse con la validación avanzada";
  }
  if ("max_total" in conditions && (typeof conditions.max_total !== "string" || !/^\d+([.,]\d{1,2})?$/.test(conditions.max_total)))
    throw "el total máximo debe ser un importe, por ejemplo 500.00";
}

// Reglas sintéticas para la vista previa (D124-D129): con dirección, con dominio, con dominio
// público, antigua sin remitente (no actúa), con la opción avanzada, archivada, con contadores
// y sin ellos, y un tipo que la aplicación no reconoce.
interface SeedRule extends Omit<Rule, "version" | "created_at"> {
  name?: string;
  archived?: boolean;
  anySender?: boolean;
  stats?: Partial<RuleStats>;
}
const DAY = 86_400_000; // milisegundos, como los guarda el backend
const seedRules: SeedRule[] = [
  {
    id: "alfa-hasta-500",
    name: "Facturas de Alfa hasta 500 €",
    action_kind: "register_invoice",
    conditions: { supplier: "Suministros Alfa SL", max_total: "500.00", currency: "EUR" },
    active: true,
    stats: { applied: 3, last_applied_at: now() - 5 * DAY, reviewed: 1 },
  },
  { id: "beta-sin-limite", action_kind: "register_invoice", conditions: { supplier: "Beta Servicios SL" }, active: false },
  {
    id: "consulta-pago-cliente",
    name: "Consultas de pago de Beta",
    action_kind: "send_draft",
    conditions: { category: "consulta_pago", sender: "cliente.beta@ejemplo.test" },
    active: true,
  },
  {
    id: "consulta-pago-dominio",
    name: "Consultas de pago de ejemplo.test",
    action_kind: "send_draft",
    conditions: { category: "consulta_pago", sender_domain: "ejemplo.test" },
    active: true,
    stats: { applied: 0, reviewed: 2 },
  },
  {
    id: "contesta-gmail",
    name: "Solicitudes desde Gmail",
    action_kind: "send_draft",
    conditions: { category: "solicitud_factura", sender_domain: "gmail.com" },
    active: false,
  },
  // Regla antigua: sin remitente ni marca, no actúa aunque esté activa.
  {
    id: "consulta-pago-auto",
    action_kind: "send_draft",
    conditions: { category: "consulta_pago" },
    active: true,
    stats: { applied: 2, last_applied_at: now() - 40 * DAY, reviewed: 0, unclassified: 2 },
  },
  {
    id: "documentacion-cualquiera",
    name: "Documentación a cualquier remitente",
    action_kind: "send_draft",
    conditions: { category: "documentacion" },
    active: true,
    anySender: true,
    stats: { applied: 1, last_applied_at: now() - 2 * DAY, reviewed: 0 },
  },
  // Pares para la revisión de reglas (D130): solape de facturas con choque activo, reglas
  // iguales, solape que solo chocará al activarse, proveedor de nombre parecido y «cualquier
  // remitente» frente a un dominio.
  { id: "alfa-sin-limite", name: "Facturas de Alfa sin límite", action_kind: "register_invoice", conditions: { supplier: "Suministros Alfa SL" }, active: true },
  { id: "gama-300-a", name: "Facturas de Gama hasta 300 €", action_kind: "register_invoice", conditions: { supplier: "Gama Obras SL", max_total: "300.00" }, active: true },
  { id: "gama-300-b", name: "Gama hasta 300 € (copia)", action_kind: "register_invoice", conditions: { supplier: "gama obras sl", max_total: "300.00" }, active: true },
  { id: "beta-hasta-100", name: "Facturas de Beta hasta 100 €", action_kind: "register_invoice", conditions: { supplier: "Beta Servicios SL", max_total: "100.00" }, active: true },
  { id: "delta-sa", name: "Facturas de Delta SA", action_kind: "register_invoice", conditions: { supplier: "Delta SA" }, active: true },
  { id: "delta-sa-puntos", name: "Facturas de Delta (con puntos)", action_kind: "register_invoice", conditions: { supplier: "DELTA, S.A." }, active: true },
  {
    id: "documentacion-dominio",
    name: "Documentación de ejemplo.test",
    action_kind: "send_draft",
    conditions: { category: "documentacion", sender_domain: "ejemplo.test" },
    active: false,
  },
  { id: "alfa-antigua", name: "Alfa (regla antigua)", action_kind: "register_invoice", conditions: { supplier: "Suministros Alfa SL" }, active: false, archived: true },
  { id: "emitir-recurrente", action_kind: "issue_recurring", conditions: {}, active: false },
];
const seedStats = new Map<string, Partial<RuleStats>>();
for (const { name, archived, anySender, stats, ...rule } of seedRules) {
  addRule({ ...rule, version: 1, created_at: now() });
  setMeta(rule.id, { name: name ?? null, archived_at: archived ? now() : null, any_sender_ack_at: anySender ? now() : null });
  if (stats) seedStats.set(rule.id, stats);
}
// Historial de ejemplo: acciones autorizadas por reglas y aprobadas por una persona.
for (const [task_id, id] of [
  [101, "alfa-hasta-500"],
  [102, "alfa-hasta-500"],
  [103, "alfa-hasta-500"],
  [104, "consulta-pago-auto"],
] as const) {
  audit.push({
    task_id,
    action: id === "alfa-hasta-500" ? "register_invoice" : "send_draft",
    status: "confirmed",
    authorization: `regla ${id} v1`,
    detail: null,
  });
}
audit.push({ task_id: 105, action: "register_invoice", status: "confirmed", authorization: "aprobación", detail: null });

/** Monedas extranjeras de la evidencia local (D127); `null` si no consta o es defectuosa. */
function foreignOf(result: any): string[] | null {
  const foreign = result?.currency_evidence?.foreign;
  if (!Array.isArray(foreign) || foreign.some((c) => typeof c !== "string")) return null;
  const explicit = String(result.fields?.currency ?? "").trim();
  return [...new Set([...foreign, ...(explicit && explicit.toUpperCase() !== "EUR" ? [explicit.toUpperCase()] : [])])].sort();
}

/** Coincidencia de unas condiciones con el resultado de una tarea, como `evaluate` (D125, D126). */
function matchRule(
  kind: string,
  rule: { conditions: Record<string, string>; ack: boolean },
  result: any,
): { matches: boolean; mismatch?: string; senderRequired?: boolean } {
  const c = rule.conditions;
  const no = (mismatch: string, senderRequired = false) => ({ matches: false, mismatch, senderRequired });
  if (kind === "invoice") {
    const actual = String(result.fields?.supplier ?? "").trim();
    if (actual.toLowerCase() !== String(c.supplier ?? "").trim().toLowerCase())
      return no(actual ? `el proveedor es «${actual}» y la regla pide «${c.supplier}»` : `la factura no indica proveedor y la regla pide «${c.supplier}»`);
    const foreign = foreignOf(result) ?? [];
    if (c.max_total && foreign.length) return no(`el documento menciona ${foreign.join(", ")} y el límite está en EUR`);
    if (c.max_total && Number(result.fields?.total) > Number(String(c.max_total).replace(",", ".")))
      return no(`el total ${result.fields?.total} supera el máximo ${c.max_total}`);
    return { matches: true };
  }
  if (result.category !== c.category) return no(`la categoría es «${result.category ?? "sin categoría"}» y la regla pide «${c.category}»`);
  const from = String(result.message?.from ?? "").trim();
  if (c.sender) {
    return from.toLowerCase() === c.sender.toLowerCase() ? { matches: true } : no(`el remitente es «${from}» y la regla pide «${c.sender}»`);
  }
  if (c.sender_domain) {
    const domain = from.split("@")[1] ?? "";
    return domain.toLowerCase() === c.sender_domain.toLowerCase() ? { matches: true } : no(`el remitente es «${from}», de otro dominio que «${c.sender_domain}»`);
  }
  return rule.ack ? { matches: true } : no("la regla no indica remitente; añádalo o valide la opción avanzada", true);
}

// ---- Contradicciones y unificación (D130, D131) ----
// Réplica mínima de `compare`, `analyze` y `merge_plan` del backend, solo para ver la interfaz
// sin Tauri. La decisión real la toma siempre `crates/app/src/rules.rs`.

type MCond =
  | { kind: "invoice"; supplier: string; key: string; limit: string | null }
  | { kind: "send"; category: string; scope: "sender" | "domain" | "any"; value: string | null };
type MDefect = { code: "invalid_conditions" | "sender_missing" | "not_comparable"; reason: string };

const canonLimit = (raw: string) => {
  const t = raw.trim();
  const n = Number(t.includes(",") ? t.replace(/\./g, "").replace(",", ".") : t);
  return Number.isFinite(n) ? n.toFixed(2) : null;
};

function mockCond(r: Rule): MCond | MDefect {
  const c = r.conditions as Record<string, unknown>;
  if (r.action_kind === "register_invoice") {
    const supplier = String(c.supplier ?? "").trim();
    if (!supplier) return { code: "invalid_conditions", reason: "no indica proveedor" };
    let limit: string | null = null;
    if ("max_total" in c) {
      limit = typeof c.max_total === "string" ? canonLimit(c.max_total) : null;
      if (limit === null) return { code: "invalid_conditions", reason: "su máximo no es un importe válido" };
    }
    return { kind: "invoice", supplier, key: supplier.toLowerCase(), limit };
  }
  if (r.action_kind === "send_draft") {
    const category = String(c.category ?? "").trim();
    if (!category) return { code: "invalid_conditions", reason: "no indica categoría" };
    if (c.sender) return { kind: "send", category, scope: "sender", value: String(c.sender).trim().toLowerCase() };
    if (c.sender_domain) return { kind: "send", category, scope: "domain", value: String(c.sender_domain).trim().toLowerCase() };
    if (metaOf(r.id).any_sender_ack_at !== null) return { kind: "send", category, scope: "any", value: null };
    return { code: "sender_missing", reason: "no indica remitente ni tiene validada la opción avanzada: no actúa" };
  }
  return { code: "not_comparable", reason: `la aplicación no reconoce el tipo de regla «${r.action_kind}», así que no puede compararla ni aplicarla` };
}

type MRel = "equal" | "first" | "second" | "similar" | "disjoint";
const foldName = (n: string) => n.toLowerCase().replace(/[^\p{L}\p{N}]/gu, "");

/** `first`/`second`: cuál de las dos cubre más. */
function mockRelation(a: MCond, b: MCond): MRel {
  if (a.kind === "invoice" && b.kind === "invoice") {
    if (a.key === b.key) {
      if (a.limit === b.limit) return "equal";
      if (a.limit === null) return "first";
      if (b.limit === null) return "second";
      return Number(a.limit) > Number(b.limit) ? "first" : "second";
    }
    return foldName(a.supplier) && foldName(a.supplier) === foldName(b.supplier) ? "similar" : "disjoint";
  }
  if (a.kind === "send" && b.kind === "send") {
    if (a.category !== b.category) return "disjoint";
    if (a.scope === "any" && b.scope === "any") return "equal";
    if (a.scope === "any") return "first";
    if (b.scope === "any") return "second";
    if (a.scope === b.scope) return a.value === b.value ? "equal" : "disjoint";
    const [dom, adr, wider] = a.scope === "domain" ? [a, b, "first"] : [b, a, "second"];
    return (adr.value ?? "").split("@")[1] === dom.value ? (wider as MRel) : "disjoint";
  }
  return "disjoint";
}

function mockSide(r: Rule, c: MCond) {
  return c.kind === "invoice" ? { id: r.id, limit: c.limit } : { id: r.id, scope: c.scope, value: c.value };
}

function mockFindings(list: Rule[]): RuleFinding[] {
  const live = list.filter((r) => !metaOf(r.id).archived_at).sort((a, b) => (a.id < b.id ? -1 : 1));
  const conds = live.map(mockCond);
  const out: RuleFinding[] = [];
  const fr = (r: Rule) => ({ id: r.id, version: r.version, active: r.active });
  live.forEach((r, i) => {
    const c = conds[i]!;
    if ("code" in c) out.push({ key: `defective:${r.id}`, kind: "defective", action_kind: r.action_kind, rules: [fr(r)], active_conflict: false, effect: { code: c.code, data: { reason: c.reason } }, merge_candidate: false, others: [], affected_tasks: null });
  });
  for (let i = 0; i < live.length; i++) {
    for (let j = i + 1; j < live.length; j++) {
      const [a, b, ca, cb] = [live[i]!, live[j]!, conds[i]!, conds[j]!];
      if (a.action_kind !== b.action_kind || "code" in ca || "code" in cb) continue;
      const rel = mockRelation(ca, cb);
      if (rel === "disjoint") continue;
      const real = rel !== "similar";
      const taskKind = a.action_kind === "register_invoice" ? "invoice" : "email";
      // Solo cuentan las tareas que coincidieron con las dos reglas estando las dos activas.
      const both = !(a.active && b.active) ? 0 : tasks.filter((t) => t.kind === taskKind && t.result && [a, b].every((x) => matchRule(taskKind, { conditions: x.conditions, ack: metaOf(x.id).any_sender_ack_at !== null }, t.result).matches)).length;
      let effect: RuleFinding["effect"];
      if (rel === "similar") effect = { code: "similar_supplier", data: { first: { id: a.id, supplier: (ca as any).supplier }, second: { id: b.id, supplier: (cb as any).supplier } } };
      else if (rel === "equal") effect = { code: "same_conditions", data: ca.kind === "invoice" ? { supplier: ca.supplier, limit: ca.limit } : { category: ca.category, scope: ca.scope, value: ca.value } };
      else {
        const [w, wc, n, nc] = rel === "first" ? [a, ca, b, cb] : [b, cb, a, ca];
        effect = { code: "narrow_blocks_wide", data: { ...(ca.kind === "invoice" ? { supplier: ca.supplier } : { category: ca.category }), wider: mockSide(w, wc), narrower: mockSide(n, nc) } };
      }
      // Terceras reglas activas que cubren lo que solo cubre la más amplia.
      let others: RuleFinding["rules"] = [];
      if (rel === "first" || rel === "second") {
        const [wc, nc] = rel === "first" ? [ca, cb] : [cb, ca];
        others = live
          .filter((k, idx) => {
            const kc = conds[idx]!;
            if (k === a || k === b || !k.active || k.action_kind !== a.action_kind || "code" in kc) return false;
            const wide = mockRelation(kc, wc);
            const narrow = mockRelation(kc, nc);
            return wide !== "disjoint" && wide !== "similar" && narrow !== "equal" && narrow !== "second";
          })
          .map(fr);
      }
      out.push({
        key: `${rel === "similar" ? "similar" : rel === "equal" ? "duplicate" : "overlap"}:${a.id}+${b.id}`,
        kind: rel === "similar" ? "similar" : rel === "equal" ? "duplicate" : "overlap",
        action_kind: a.action_kind,
        rules: [fr(a), fr(b)],
        active_conflict: real && a.active && b.active,
        effect,
        merge_candidate: real,
        others,
        affected_tasks: real ? both : null,
      });
    }
  }
  const rank = (f: RuleFinding) => (f.kind === "defective" ? 3 : f.kind === "similar" ? 2 : f.active_conflict ? 0 : 1);
  return out.sort((x, y) => rank(x) - rank(y) || (x.key < y.key ? -1 : 1));
}

/** Regla única que coincide y automatiza, con los bloqueos de la tarea (D125); `null` si va a revisión. */
function mockAutomatedBy(list: Rule[], kind: string, result: any, validated = false): string | null {
  const matching = list.filter((r) => r.active && matchRule(kind, { conditions: r.conditions, ack: r.id === "(unificada)" ? validated : metaOf(r.id).any_sender_ack_at !== null }, result).matches);
  if (matching.length !== 1) return null;
  const blocked =
    kind === "invoice"
      ? Boolean(result.issues?.length || result.unreadable?.length || result.amount_review || (result.part && result.part.of > 1) || result.currency_evidence?.euro !== true || (foreignOf(result) ?? ["?"]).length)
      : !(result.draft?.to ?? []).length;
  return blocked ? null : matching[0]!.id;
}

function mockOptions(members: Rule[]): MergeOption[] {
  const options: MergeOption[] = [];
  for (const r of members) {
    const c = mockCond(r) as MCond;
    const key = c.kind === "invoice" ? (c.limit ?? "none") : c.scope === "any" ? "any" : `${c.scope}:${c.value}`;
    const found = options.find((o) => o.key === key);
    if (found) found.rule_ids.push(r.id);
    else options.push({ key, scope: c.kind === "invoice" ? { limit: c.limit } : { scope: c.scope, value: c.value }, rule_ids: [r.id] });
  }
  return options;
}

function mockMembers(ids: string[]): Rule[] {
  if (ids.length < 2) throw "seleccione al menos dos reglas para unificar";
  const members = ids.map((id) => latest().find((r) => r.id === id) ?? (() => { throw `la regla ${id} no existe`; })()).sort((a, b) => (a.id < b.id ? -1 : 1));
  const archived = members.find((r) => metaOf(r.id).archived_at);
  if (archived) throw `la regla ${archived.id} está archivada; restáurela antes de unificarla`;
  if (members.some((r) => r.action_kind !== members[0]!.action_kind))
    throw "solo se pueden unificar reglas del mismo tipo: no mezcle reglas de registrar facturas con reglas de enviar respuestas";
  const conds = members.map(mockCond);
  const bad = conds.findIndex((c) => "code" in c);
  if (bad >= 0) throw `la regla ${members[bad]!.id} no actúa (${(conds[bad] as MDefect).reason}); corríjala antes de unificarla`;
  for (let i = 0; i < members.length; i++)
    for (let j = i + 1; j < members.length; j++) {
      const rel = mockRelation(conds[i] as MCond, conds[j] as MCond);
      if (rel === "similar") throw `«${(conds[i] as any).supplier}» y «${(conds[j] as any).supplier}» tienen un nombre de proveedor parecido, pero la aplicación no puede asegurar que sean el mismo; si lo son, edite una de las dos para dejar el nombre igual y después unifíquelas`;
      if (rel === "disjoint") throw `las reglas ${members[i]!.id} y ${members[j]!.id} cubren casos distintos: no se contradicen y no hay nada que unificar`;
    }
  return members;
}

function mockFingerprint(members: Rule[], chosen: string) {
  const parts = latest().filter((r) => r.action_kind === members[0]!.action_kind && !metaOf(r.id).archived_at).map((r) => `${r.id}:${r.version}:${r.active ? 1 : 0}:${metaOf(r.id).any_sender_ack_at !== null ? 1 : 0}`);
  return `${chosen}|${parts.sort().join("|")}`;
}

function mockPlanRule(r: Rule): PlanRule {
  return { id: r.id, version: r.version, active: r.active, conditions: r.conditions, any_sender: metaOf(r.id).any_sender_ack_at !== null };
}

function mockPlan(members: Rule[], chosen: string): MergePlan {
  const options = mockOptions(members);
  const model = members.find((r) => mockOptions([r])[0]!.key === chosen);
  if (!model) throw `la opción «${chosen}» no corresponde a ninguna de las reglas elegidas`;
  const kind = members[0]!.action_kind === "register_invoice" ? "invoice" : "email";
  const originals = new Set(members.map((r) => r.id));
  const current = latest().filter((r) => !metaOf(r.id).archived_at && r.action_kind === members[0]!.action_kind);
  const merged: Rule = { id: "(unificada)", version: 1, action_kind: model.action_kind, conditions: model.conditions, active: true, created_at: 0 };
  const future = [...current.filter((r) => !originals.has(r.id)), merged];
  const ofKind = tasks.filter((t) => t.kind === kind).sort((a, b) => b.id - a.id);
  const simulate = (validated: boolean): MergePlan["simulation"] => {
    const sim: MergePlan["simulation"] = { considered: 0, truncated: ofKind.length > 200, skipped_without_result: 0, touched: 0, start_automating: [], stop_automating: [], unchanged_automated: 0, unchanged_review: 0, without_currency_check: 0 };
    for (const t of ofKind.slice(0, 200)) {
      if (!t.result) {
        sim.skipped_without_result++;
        continue;
      }
      sim.considered++;
      const covered = [...members, merged].some((r) => matchRule(kind, { conditions: r.conditions, ack: r.id === "(unificada)" ? validated : metaOf(r.id).any_sender_ack_at !== null }, t.result!).matches);
      if (!covered) continue;
      sim.touched++;
      if (kind === "invoice" && foreignOf(t.result) === null) sim.without_currency_check++;
      const was = mockAutomatedBy(current, kind, t.result);
      const will = mockAutomatedBy(future, kind, t.result, validated);
      const item = { task_id: t.id, label: kind === "invoice" ? (t.result.document ?? null) : (t.result.message?.subject ?? null), total: kind === "invoice" ? (t.result.fields?.total ?? null) : null, automated_before_by: was, automated_after_by: will };
      if (!was && will) sim.start_automating.push(item);
      else if (was && !will) sim.stop_automating.push(item);
      else if (was) sim.unchanged_automated++;
      else sim.unchanged_review++;
    }
    return sim;
  };
  const simulation = simulate(false);
  const needsValidation = model.action_kind === "send_draft" && chosen === "any";
  return {
    action_kind: model.action_kind,
    before: members.map(mockPlanRule),
    options,
    chosen,
    after: {
      conditions: model.conditions,
      active: false,
      option_key: chosen,
      loses_any_sender: members.filter((r) => metaOf(r.id).any_sender_ack_at !== null).map((r) => r.id),
      needs_sender_validation: needsValidation,
    },
    simulation,
    simulation_if_validated: needsValidation ? simulate(true) : null,
    fingerprint: mockFingerprint(members, chosen),
  };
}

// ---- Sugerencias (D132): datos sintéticos fijos ----

const mockThresholds = { invoice: 3, reply: 5 };
const mockDiscarded = new Set<string>();
const mockAccepted = new Map<string, string>();
const mockThresholdSettings = (): ThresholdSettings => ({
  invoice: mockThresholds.invoice,
  reply: mockThresholds.reply,
  min: 2,
  max: 50,
  default_invoice: 3,
  default_reply: 5,
});

const mockEvidence = (count: number, threshold: number, daysAgo: number, amounts: string[] = []): Suggestion["evidence"] => {
  const sorted = amounts.map(Number).sort((x, y) => y - x);
  return {
    count,
    threshold,
    earlier_breaks: 0,
    last_at: now() - daysAgo * 24 * 3600 * 1000,
    task_ids: [],
    amounts,
    limit: amounts.length ? sorted[0]!.toFixed(2) : null,
    amount_min: amounts.length ? sorted[sorted.length - 1]!.toFixed(2) : null,
    amount_second: amounts.length > 1 ? sorted[1]!.toFixed(2) : null,
    limit_outlier: amounts.length > 1 && sorted[0]! >= 3 * sorted[1]!,
    reappeared: null,
  };
};

function mockSuggestions(): Suggestions {
  const base: Array<Omit<Suggestion, "shown" | "overlaps">> = [
    {
      fingerprint: "mock-factura-alfa",
      action_kind: "register_invoice",
      name: "Facturas de Alfa SL",
      conditions: { supplier: "Alfa SL", max_total: "1210.00" },
      subject: "Alfa SL",
      category: null,
      evidence: mockEvidence(3, mockThresholds.invoice, 5, ["968.00", "1210.00", "605.00"]),
    },
    {
      // Un importe muy por encima del resto: el aviso de límite alto.
      fingerprint: "mock-factura-omega",
      action_kind: "register_invoice",
      name: "Facturas de Omega SL",
      conditions: { supplier: "Omega SL", max_total: "9680.00" },
      subject: "Omega SL",
      category: null,
      evidence: mockEvidence(3, mockThresholds.invoice, 9, ["121.00", "242.00", "9680.00"]),
    },
    {
      // Nombre casi igual al de una regla existente («DELTA, S.A.»): lleva su aviso real.
      fingerprint: "mock-factura-delta",
      action_kind: "register_invoice",
      name: "Facturas de Delta SA",
      conditions: { supplier: "Delta SA", max_total: "605.00" },
      subject: "Delta SA",
      category: null,
      evidence: mockEvidence(4, mockThresholds.invoice, 3, ["121.00", "242.00", "363.00", "605.00"]),
    },
    {
      fingerprint: "mock-respuesta-pedidos",
      action_kind: "send_draft",
      name: "Respuestas a pedidos@ejemplo.test (solicitud_factura)",
      conditions: { category: "solicitud_factura", sender: "pedidos@ejemplo.test" },
      subject: "pedidos@ejemplo.test",
      category: "solicitud_factura",
      evidence: mockEvidence(6, mockThresholds.reply, 2),
    },
    {
      // Dentro de una regla inactiva del dominio: se muestra con su aviso.
      fingerprint: "mock-respuesta-documentacion",
      action_kind: "send_draft",
      name: "Respuestas a ana@ejemplo.test (documentacion)",
      conditions: { category: "documentacion", sender: "ana@ejemplo.test" },
      subject: "ana@ejemplo.test",
      category: "documentacion",
      evidence: mockEvidence(5, mockThresholds.reply, 6),
    },
  ];
  const hidden = { covered: 0, discarded: 0, accepted: 0, invalid: 0, not_suggested: 0 };
  const candidates: Suggestion[] = [];
  for (const c of base) {
    if (c.evidence.count < c.evidence.threshold) continue;
    if (mockAccepted.has(c.fingerprint)) {
      hidden.accepted++;
      continue;
    }
    // Hallazgos de la candidata como si estuviera activa, con las reglas no archivadas.
    const hypothetical: Rule = { id: "(sugerida)", version: 1, action_kind: c.action_kind, conditions: c.conditions, active: true, created_at: 0 };
    const findings = mockFindings([...latest(), hypothetical]).filter((f) => f.kind !== "defective" && f.rules.some((r) => r.id === "(sugerida)"));
    // Igual, o más amplia y activa: ya está cubierta (lo mismo que hace el backend).
    const covered = findings.some(
      (f) =>
        f.kind === "duplicate" ||
        (f.kind === "overlap" && f.effect.data.wider?.id !== "(sugerida)" && f.rules.some((r) => r.id === f.effect.data.wider?.id && r.active)),
    );
    if (covered) {
      hidden.covered++;
      continue;
    }
    if (mockDiscarded.has(c.fingerprint)) {
      hidden.discarded++;
      continue;
    }
    candidates.push({ ...c, shown: `${c.fingerprint}|${JSON.stringify(c.conditions)}`, overlaps: findings });
  }
  return {
    candidates,
    progress: [{ action_kind: "register_invoice", subject: "Beta SA", category: null, count: 2, needed: mockThresholds.invoice, missing: Math.max(0, mockThresholds.invoice - 2) }].filter((p) => p.missing > 0),
    observed: 24,
    counted: 24,
    hidden,
    thresholds: mockThresholdSettings(),
    watermark: 24,
  };
}

const handlers: Record<string, (a: any, options?: any) => unknown> = {
  tasks: () => structuredClone(tasks),
  import_file: (bytes: Uint8Array, options?: { headers: Record<string, string> }) => {
    const name = decodeURIComponent(options?.headers["x-nestadmin-name"] ?? "");
    if (!/\.(txt|pdf|jpe?g|png|tiff?|bmp|webp)$/i.test(name)) throw `«${name}»: solo se admiten texto (.txt), PDF e imágenes (.jpg, .png, .tif, .bmp, .webp)`;
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
    // Como la aplicación: sin confirmar que el intento anterior no salió, no se envía otro.
    if (t.result?.recovery?.needs_confirmation) return t;
    audit.push({ task_id: id, action, status: "confirmed", authorization: "aprobación", detail: null });
    t.state = "completed";
    t.result = { ...t.result, receipt: { simulated: true } };
    return t;
  },
  confirm_not_sent: ({ id }) => {
    const t = reviewable(id);
    if (!t.result?.attempt) throw `la tarea ${id} no tiene ningún intento anterior`;
    const action = t.kind === "invoice" ? "register_invoice" : "send_draft";
    const version = t.result.version ?? 1;
    audit.push({
      task_id: id,
      action,
      status: "superseded",
      authorization: "aprobación",
      detail: `task-${id}/${action}/v${version}: el usuario confirmó que no se realizó`,
    });
    const { recovery: _recovery, ...rest } = t.result;
    t.result = {
      ...rest,
      version: version + 1,
      review_reason: "versión nueva tras confirmar que el intento anterior no se realizó",
    };
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
  rule_views: () => latest().map(ruleView),
  create_rule: ({ id, name, actionKind, conditions, acknowledgeAnySender }) => {
    if (typeof acknowledgeAnySender !== "boolean") throw "missing required key acknowledgeAnySender";
    const cleanName = String(name ?? "").trim() || null;
    let ruleId = String(id ?? "").trim();
    if (!ruleId) {
      if (!cleanName) throw "indique un nombre o un identificador para la regla";
      const base = slug(cleanName);
      ruleId = base;
      for (let n = 2; latest().some((r) => r.id === ruleId); n++) ruleId = `${base}-${n}`;
    }
    if (latest().some((r) => r.id === ruleId)) throw `la regla ${ruleId} ya existe`;
    if (!/^[a-z0-9-]+$/.test(ruleId)) throw "el identificador solo admite minúsculas, números y guiones";
    if (acknowledgeAnySender && (actionKind !== "send_draft" || hasSender(conditions)))
      throw "la validación avanzada solo se aplica a reglas de envío sin remitente";
    checkConditions(actionKind, conditions, acknowledgeAnySender);
    const rule = addRule({ id: ruleId, version: 1, action_kind: actionKind, conditions, active: false, created_at: now() });
    setMeta(ruleId, { name: cleanName, any_sender_ack_at: acknowledgeAnySender ? now() : null });
    return ruleView(rule);
  },
  rename_rule: ({ id, name }) => {
    const r = latest().find((r) => r.id === id);
    if (!r) throw `regla ${id} inexistente`;
    setMeta(id, { name: String(name ?? "").trim() || null });
    return ruleView(r);
  },
  update_rule: ({ id, conditions }) => {
    const r = latest().find((r) => r.id === id);
    if (!r) throw `regla ${id} inexistente`;
    if (metaOf(id).archived_at) throw `la regla ${id} está archivada; desarchívela para volver a usarla`;
    checkConditions(r.action_kind, conditions, metaOf(id).any_sender_ack_at !== null);
    setMeta(id, { any_sender_ack_at: null }); // editar borra la validación avanzada
    return addRule({ ...r, version: r.version + 1, conditions, created_at: now() });
  },
  // Atómico: se valida todo antes de tocar nada (nombre, condiciones y validación avanzada).
  update_rule_with_ack: ({ id, name, conditions, acknowledgeAnySender }) => {
    const r = latest().find((r) => r.id === id);
    if (!r) throw `regla ${id} inexistente`;
    if (metaOf(id).archived_at) throw `la regla ${id} está archivada; desarchívela para volver a usarla`;
    if (acknowledgeAnySender && (r.action_kind !== "send_draft" || hasSender(conditions)))
      throw "la validación avanzada solo se aplica a reglas de envío sin remitente";
    checkConditions(r.action_kind, conditions, Boolean(acknowledgeAnySender));
    const clean = name === null || name === undefined ? undefined : String(name).trim() || null;
    if (clean !== undefined && clean !== null && clean.length > 80) throw "el nombre de la regla admite como mucho 80 caracteres";
    const hadAck = metaOf(id).any_sender_ack_at !== null;
    const saved = addRule({ ...r, version: r.version + 1, conditions, created_at: now() });
    if (hadAck) audit.push({ task_id: null, action: "rule_any_sender", status: "revoked", authorization: null, detail: `${id} v${saved.version}: retirada: las condiciones cambiaron` });
    setMeta(id, { any_sender_ack_at: null });
    if (clean !== undefined && clean !== metaOf(id).name) {
      setMeta(id, { name: clean });
      audit.push({ task_id: null, action: "rule_rename", status: "renamed", authorization: null, detail: `${id}: ${clean ?? "(sin nombre)"}` });
    }
    if (acknowledgeAnySender) {
      setMeta(id, { any_sender_ack_at: now() });
      audit.push({ task_id: null, action: "rule_any_sender", status: "acknowledged", authorization: null, detail: `${id} v${saved.version}: validada al guardar la regla` });
    }
    return ruleView(saved);
  },
  set_rule_active: ({ id, active }) => {
    const r = latest().find((r) => r.id === id);
    if (!r) throw `regla ${id} inexistente`;
    if (active) {
      if (metaOf(id).archived_at) throw `la regla ${id} está archivada; desarchívela para volver a usarla`;
      try {
        checkConditions(r.action_kind, r.conditions, metaOf(id).any_sender_ack_at !== null);
      } catch (why) {
        throw `la regla ${id} no se puede activar: ${why}`;
      }
    }
    return r.active === active ? r : addRule({ ...r, version: r.version + 1, active, created_at: now() });
  },
  set_rule_any_sender: ({ id, acknowledge }) => {
    const r = latest().find((r) => r.id === id);
    if (!r) throw `regla ${id} inexistente`;
    if (metaOf(id).archived_at) throw `la regla ${id} está archivada; desarchívela para volver a usarla`;
    if (acknowledge && (r.action_kind !== "send_draft" || hasSender(r.conditions)))
      throw "la opción avanzada solo existe en reglas de envío sin remitente";
    setMeta(id, { any_sender_ack_at: acknowledge ? now() : null });
    audit.push({
      task_id: null,
      action: "rule_any_sender",
      status: acknowledge ? "acknowledged" : "revoked",
      authorization: null,
      detail: `${id} v${r.version}: ${acknowledge ? "validada la opción avanzada" : "retirada la opción avanzada"}`,
    });
    return ruleView(r);
  },
  archive_rule: ({ id }) => {
    let r = latest().find((r) => r.id === id);
    if (!r) throw `regla ${id} inexistente`;
    if (metaOf(id).archived_at) return ruleView(r);
    if (r.active) r = addRule({ ...r, version: r.version + 1, active: false, created_at: now() });
    // Archivar retira la opción avanzada; desarchivar no la devuelve (D126).
    if (metaOf(id).any_sender_ack_at !== null)
      audit.push({ task_id: null, action: "rule_any_sender", status: "revoked", authorization: null, detail: `${id} v${r.version}: retirada: la regla se archivó` });
    setMeta(id, { archived_at: now(), any_sender_ack_at: null });
    audit.push({ task_id: null, action: "rule_archive", status: "archived", authorization: null, detail: `${id} v${r.version}` });
    return ruleView(r);
  },
  unarchive_rule: ({ id }) => {
    const r = latest().find((r) => r.id === id);
    if (!r) throw `regla ${id} inexistente`;
    setMeta(id, { archived_at: null });
    return ruleView(r);
  },
  rule_stats: (): RuleStats[] => latest().map((r) => ruleView(r).stats),
  // Simulación con las tareas sintéticas: replica el orden de la capa Rust (D125, D126, D129)
  // sin llamar a nada: la regla no coincide, atajos (importe dudoso, factura separada), bloqueos
  // de la tarea (incidencias, campos ilegibles, moneda, destinatario) y por último solapes.
  simulate_rule: ({ actionKind, conditions, limit, assumeAnySenderAck, excludeRuleId }): Simulation => {
    checkConditions(actionKind, conditions, true);
    const max = limit ?? 200;
    const kind = actionKind === "register_invoice" ? "invoice" : "email";
    const ofKind = tasks.filter((t) => t.kind === kind).sort((a, b) => b.id - a.id);
    const items: SimulationItem[] = [];
    const sim: Simulation = {
      action_kind: actionKind,
      limit: max,
      considered: 0,
      truncated: ofKind.length > max,
      skipped_without_result: 0,
      without_currency_check: 0,
      would_automate: 0,
      would_review: {},
      overlaps: 0,
      no_match: 0,
      would_block_other: [],
      findings: [],
      items,
    };
    const blocked = new Map<string, number[]>();
    const live = latest().filter((r) => r.active && r.action_kind === actionKind && !metaOf(r.id).archived_at && r.id !== excludeRuleId);
    for (const t of ofKind.slice(0, max)) {
      const r = t.result;
      if (!r) {
        sim.skipped_without_result++;
        continue;
      }
      sim.considered++;
      const label = kind === "invoice" ? (r.document ?? null) : (r.message?.subject ?? null);
      if (kind === "invoice" && foreignOf(r) === null) sim.without_currency_check++;
      const push = (outcome: SimulationItem["outcome"], reason_code: string | null, explanation: string, overlapping_rules: string[] = []) =>
        items.push({ task_id: t.id, label, outcome, reason_code, explanation, overlapping_rules });
      const review = (code: string, text: string) => {
        sim.would_review[code] = (sim.would_review[code] ?? 0) + 1;
        push("review", code, text);
      };
      const mine = matchRule(kind, { conditions, ack: Boolean(assumeAnySenderAck) }, r);
      if (!mine.matches) {
        if (mine.senderRequired) review("sender_required", "la regla no indica remitente; añádalo o valide la opción avanzada");
        else {
          sim.no_match++;
          push("no_match", null, mine.mismatch ?? "la regla no coincide");
        }
        continue;
      }
      // Atajos previos a las reglas.
      if (kind === "invoice" && r.amount_review) {
        review("amount_review", String(r.amount_review));
        continue;
      }
      if (kind === "invoice" && r.part && r.part.of > 1) {
        review("split_invoice", "el documento contiene varias facturas; la IA las separó y cada una se revisa por separado");
        continue;
      }
      // Bloqueos de la tarea, en el orden del backend.
      const foreign = kind === "invoice" ? foreignOf(r) : [];
      if (kind === "invoice" && r.issues?.length) review("invoice_issues", "la factura tiene incidencias");
      else if (kind === "invoice" && r.unreadable?.length) review("unreadable_fields", "hay campos ilegibles");
      else if (kind === "invoice" && foreign === null) review("currency_not_checked", "no consta la comprobación de moneda; reprocese o revise");
      else if (kind === "invoice" && foreign && foreign.length) review("foreign_currency", `el documento menciona ${foreign.join(", ")}; hoy la aplicación solo registra facturas en euros`);
      else if (kind === "invoice" && r.currency_evidence?.euro !== true) review("currency_not_shown", "el documento no muestra la moneda; no se puede comprobar que sea en euros");
      else if (kind === "email" && !(r.draft?.to ?? []).length) review("no_recipient", "el borrador no tiene destinatario");
      else {
        const others = live
          .filter((o) => matchRule(kind, { conditions: o.conditions, ack: metaOf(o.id).any_sender_ack_at !== null }, r).matches)
          .map((o) => o.id);
        if (others.length) {
          sim.overlaps++;
          push("overlap", "several_rules_match", `también coincide ${others.join(", ")}; con dos reglas la decisión no es inequívoca y iría a revisión`, others);
          // Solo deja de actuar la regla que sola habría automatizado esta tarea.
          if (others.length === 1) blocked.set(others[0]!, [...(blocked.get(others[0]!) ?? []), t.id]);
        } else {
          sim.would_automate++;
          push("automate", "automated", "la regla la automatizaría");
        }
      }
    }
    sim.would_block_other = [...blocked].map(([rule_id, task_ids]) => ({ rule_id, task_ids }));
    // Contradicciones que crearía al activarse: la hipotética frente a las demás reglas.
    const own = excludeRuleId ? latest().find((r) => r.id === excludeRuleId) : undefined;
    const hypothetical: Rule = { id: excludeRuleId ?? "(simulada)", version: own?.version ?? 1, action_kind: actionKind, conditions, active: true, created_at: 0 };
    const saved = ruleMeta.get(hypothetical.id);
    if (assumeAnySenderAck) setMeta(hypothetical.id, { any_sender_ack_at: now() });
    sim.findings = mockFindings([...latest().filter((r) => r.id !== excludeRuleId), hypothetical]).filter(
      (f) => f.kind !== "defective" && f.rules.some((r) => r.id === hypothetical.id),
    );
    if (assumeAnySenderAck) {
      if (saved) ruleMeta.set(hypothetical.id, saved);
      else ruleMeta.delete(hypothetical.id);
    }
    return sim;
  },
  // Sugerencias (D132): datos sintéticos fijos; el cálculo real vive en la capa Rust.
  rule_suggestions: (): Suggestions => mockSuggestions(),
  accept_suggestion: ({ fingerprint, shown, name }) => {
    const known = mockAccepted.get(String(fingerprint));
    if (known) return ruleView(latest().find((r) => r.id === known)!);
    const c = mockSuggestions().candidates.find((x) => x.fingerprint === fingerprint);
    if (!c) throw "La sugerencia ya no está vigente: los datos cambiaron o ya se resolvió. Actualice la lista.";
    if (c.shown !== shown) throw "La sugerencia ha cambiado desde que la vio (otro límite u otros datos): revise la tarjeta de nuevo antes de crear la regla.";
    const view = handlers.create_rule!({
      id: null,
      name: String(name ?? "").trim() || c.name,
      actionKind: c.action_kind,
      conditions: c.conditions,
      acknowledgeAnySender: false,
    }) as RuleView;
    setMeta(view.id, { origin: { kind: "suggestion", count: c.evidence.count, threshold: c.evidence.threshold } });
    mockAccepted.set(String(fingerprint), view.id);
    audit.push({ task_id: null, action: "rule_suggestion", status: "accepted", authorization: null, detail: `${view.id} v1 (inactiva) creada desde la sugerencia ${fingerprint}` });
    return ruleView(latest().find((r) => r.id === view.id)!);
  },
  discard_suggestion: ({ fingerprint }) => {
    if (!mockSuggestions().candidates.some((x) => x.fingerprint === fingerprint) && !mockDiscarded.has(String(fingerprint)))
      throw "La sugerencia ya no está vigente: los datos cambiaron o ya se resolvió. Actualice la lista.";
    if (!mockDiscarded.has(String(fingerprint))) {
      mockDiscarded.add(String(fingerprint));
      audit.push({ task_id: null, action: "rule_suggestion", status: "discarded", authorization: null, detail: `sugerencia ${fingerprint} descartada` });
    }
  },
  suggestion_thresholds: (): ThresholdSettings => mockThresholdSettings(),
  set_suggestion_threshold: ({ which, value }): ThresholdSettings => {
    if (which !== "invoice" && which !== "reply") throw `umbral desconocido: ${which}; use «invoice» o «reply»`;
    if (!Number.isInteger(value) || value < 2 || value > 50)
      throw "el umbral debe estar entre 2 y 50: con una sola observación no se sugiere nada";
    if (mockThresholds[which as "invoice" | "reply"] !== value) {
      audit.push({ task_id: null, action: "rule_setting", status: "changed", authorization: null, detail: `umbral para sugerir reglas (${which}): ${mockThresholds[which as "invoice" | "reply"]} -> ${value}` });
      mockThresholds[which as "invoice" | "reply"] = value;
    }
    return mockThresholdSettings();
  },
  rule_seed_for_task: ({ taskId }): RuleSeed => {
    const task = tasks.find((t) => t.id === taskId);
    const r = task?.result;
    if (!task || !r) throw "la tarea no tiene resultado";
    if (task.kind === "invoice") {
      const supplier = String(r.fields?.supplier ?? "").trim();
      if (!supplier || /[\u0000-\u001f\u007f]/.test(supplier)) throw "la factura no indica proveedor: no hay qué precargar";
      const total = /^\d+\.\d{2}$/.test(String(r.fields?.total ?? "")) ? String(r.fields.total) : null;
      // Mismo orden que el backend: el límite solo con el euro comprobado y sin importe dudoso.
      const foreign = foreignOf(r);
      let skipped: RuleSeed["limit_skipped"] = null;
      if (foreign === null) skipped = { code: "currency_not_checked", currencies: [] };
      else if (foreign.length) skipped = { code: "foreign_currency", currencies: foreign };
      else if (r.currency_evidence?.euro !== true) skipped = { code: "currency_not_shown", currencies: [] };
      else if (typeof r.amount_review === "string") skipped = { code: "amount_review", currencies: [] };
      else if (!total) skipped = { code: "no_total", currencies: [] };
      return {
        action_kind: "register_invoice",
        conditions: skipped ? { supplier } : { supplier, max_total: total! },
        name: `Facturas de ${supplier}`.slice(0, 80),
        limit_from_this_invoice: !skipped,
        limit_skipped: skipped,
      };
    }
    const sender = String(r.message?.from ?? "").trim().toLowerCase();
    if (!/^[^@\s,<>;:"]+@[a-z0-9-]+(\.[a-z0-9-]+)+$/.test(sender)) throw "el correo no indica un remitente que se pueda usar en una regla";
    if (!["solicitud_factura", "consulta_pago", "incidencia_factura", "documentacion", "otros"].includes(String(r.category ?? ""))) throw "la regla de envío necesita una categoría válida";
    return { action_kind: "send_draft", conditions: { category: String(r.category), sender }, name: `Respuestas a ${sender} (${r.category})`.slice(0, 80), limit_from_this_invoice: false, limit_skipped: null };
  },
  rule_findings: (): RuleFinding[] => mockFindings(latest()),
  preview_restore: ({ id }): RuleFinding[] => {
    const rule = latest().find((r) => r.id === id);
    if (!rule || !metaOf(id).archived_at) throw `la regla ${id} no está archivada`;
    const saved = metaOf(id);
    setMeta(id, { archived_at: null, any_sender_ack_at: null });
    try {
      return mockFindings(latest().map((r) => (r.id === id ? { ...r, active: false } : r))).filter((f) => f.rules.some((r) => r.id === id));
    } finally {
      ruleMeta.set(id, saved);
    }
  },
  preview_merge: ({ ids, chosen }): MergePreview => {
    const members = mockMembers(ids);
    const options = mockOptions(members);
    const pick = chosen ? String(chosen) : options.length === 1 ? options[0]!.key : null;
    if (chosen && !options.some((o) => o.key === chosen)) throw `la opción «${chosen}» no corresponde a ninguna de las reglas elegidas`;
    return {
      action_kind: members[0]!.action_kind,
      before: members.map(mockPlanRule),
      options,
      needs_choice: options.length > 1,
      chosen: pick,
      plan: pick ? mockPlan(members, pick) : null,
      suggested_name: `Unificada: ${members.map((r) => ruleView(r).name).join(" + ")}`.slice(0, 80),
    };
  },
  merge_rules: ({ ids, chosen, name, planFingerprint }): RuleView => {
    const members = mockMembers(ids);
    const options = mockOptions(members);
    const key = chosen ? String(chosen) : options.length === 1 ? options[0]!.key : "";
    if (!options.some((o) => o.key === key) || mockFingerprint(members, key) !== planFingerprint) throw "las reglas han cambiado; revise de nuevo";
    const plan = mockPlan(members, key);
    const title = String(name ?? "").trim() || `Unificada: ${members.map((r) => ruleView(r).name).join(" + ")}`.slice(0, 80);
    let id = slug(title);
    for (let n = 2; latest().some((r) => r.id === id); n++) id = `${slug(title)}-${n}`;
    for (const r of members) handlers.archive_rule!({ id: r.id });
    const rule = addRule({ id, version: 1, action_kind: plan.action_kind, conditions: plan.after.conditions as Record<string, string>, active: false, created_at: now() });
    setMeta(id, { name: title, origin: { kind: "merge", sources: members.map((r) => ({ id: r.id, version: r.version })), option: key } });
    audit.push({ task_id: null, action: "rule_merge", status: "merged", authorization: null, detail: `${id} v1 (inactiva) sustituye a ${members.map((r) => `${r.id} v${r.version}`).join(", ")}; originales archivadas` });
    return ruleView(rule);
  },
  audit_log: () => structuredClone(audit),
  settings: (): SettingsView => {
    const wait = aiWait();
    const consented = !choice || choice.provider === "ollama" || consents.has(consentKey(choice) ?? "");
    return {
      choice,
      active: choice ? { provider: choice.provider, model: choice.model } : { provider: "ollama_cloud", model: CLOUD_DEFAULT },
      ai_wait: wait,
      ai_wait_message: wait ? AI_WAIT_MESSAGE[wait] : null,
      local_fallback: localFallback,
      fallback_period_active: aiParams.has("periodo-respaldo"),
      consented,
      offer_external: !offerShown && choice?.provider === "ollama",
      default_cloud_model: CLOUD_DEFAULT,
      data_dir: "%LOCALAPPDATA%\\NestAdmin (vista previa: datos en memoria)",
    };
  },
  select_provider: ({ choice: next, consent }) => {
    const dest = consentKey(next);
    if (dest) {
      if (consent) consents.add(dest);
      else if (!consents.has(dest)) throw "Confirme antes el destino de los datos";
      if (!savedKeys.has(credentialName(next) ?? "")) throw "unauthorized: credencial ausente o rechazada";
    }
    choice = next;
    keyRevoked = false;
    return { provider: next.provider, model: next.model };
  },
  list_models: async ({ choice: target, key }) => {
    await new Promise((resolve) => setTimeout(resolve, 600));
    if (target.provider === "ollama") {
      if (aiParams.has("sin-ollama")) throw "unavailable: Ollama no está instalado o no responde en esta dirección";
      return modelPresent ? ["qwen3:8b"] : [];
    }
    const name = credentialName(target)!;
    const typed = String(key ?? "").trim();
    if (!typed && !savedKeys.has(name)) throw "unauthorized: falta la clave del proveedor";
    if (/^(gsk-invalida|invalida)/.test(typed)) throw "unauthorized: el proveedor rechazó la clave";
    if (/^sin-red/.test(typed)) throw "unavailable: no se pudo conectar con el proveedor";
    if (typed) {
      savedKeys.add(name);
      keyRevoked = false;
    }
    return structuredClone(MODEL_LISTS[target.provider]);
  },
  test_provider: async ({ choice: target }) => {
    const dest = consentKey(target);
    if (dest && !consents.has(dest)) throw "Confirme antes el destino de los datos";
    await new Promise((resolve) => setTimeout(resolve, 900));
    if (aiParams.has("prueba-falla"))
      throw "invalid_response: la respuesta del modelo no es el JSON esperado (se recibió texto libre)";
  },
  set_local_fallback: ({ enabled }) => {
    localFallback = Boolean(enabled);
  },
  provider_key_saved: ({ choice: target }) => savedKeys.has(credentialName(target) ?? ""),
  delete_provider_key: ({ choice: target }) => {
    savedKeys.delete(credentialName(target) ?? "");
  },
  dismiss_external_offer: () => {
    offerShown = true;
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
  // `?sinocr` simula un paquete sin componentes de OCR (D103).
  if (choice?.provider !== "ollama") return new URLSearchParams(location.search).has("sinocr") ? { outcome: "ocr_missing" } : { outcome: "not_local" };
  return modelPresent ? { outcome: "ready" } : { outcome: "needs_preparation", check: { status: "missing", component: "model" } };
}

const mutating = new Set([
  "import_file",
  "fetch_mail",
  "approve",
  "confirm_not_sent",
  "edit_draft",
  "reject",
  "retry",
  "create_rule",
  "rename_rule",
  "update_rule",
  "update_rule_with_ack",
  "set_rule_active",
  "set_rule_any_sender",
  "archive_rule",
  "unarchive_rule",
  "merge_rules",
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
