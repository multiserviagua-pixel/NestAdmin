import "./style.css";
import { api, isPreview, onChanged, onMailSignIn, onSetup, onSetupDone, onStartup } from "./api";
import type {
  AuditEntry,
  DocumentOrigin,
  FetchReport,
  FieldSource,
  MailOrigin,
  MailAccount,
  MailProviderKind,
  MailView,
  Memory,
  ProviderChoice,
  ProviderInfo,
  Rule,
  SettingsView,
  SetupCheck,
  SetupDone,
  SetupErrorCode,
  SetupEvent,
  SetupStep,
  Startup,
  Task,
} from "./types";
import {
  CATEGORY_HELP,
  CATEGORY_LABEL,
  FIELD_HELP,
  conditionsSummary,
  isKnownAction,
  normalizeSearch,
  ruleProblem,
  ruleRisk,
  ruleSentence,
  searchText,
} from "./rules-text";

import { actionBar, emptyState, h, helpTip, icon, sectionHeader, statusChip, tabs, type Child, type IconName } from "./components";
import { ageText, chooseTab, emailFlags, formatAmount, invoiceFlags, isKnownKind, nextSelection, sortForReview, splitByTab, tabOf, type Flag, type ReviewTab } from "./pendientes-text";

const STATE_LABEL: Record<string, string> = {
  queued: "En cola",
  running: "Procesando",
  awaiting_review: "Pendiente de revisión",
  completed: "Completada",
  failed: "Fallida",
  cancelled: "Descartada",
};

const ISSUE_LABEL: Record<string, string> = {
  missing_tax: "Falta el IVA",
  missing_total: "Falta el total",
  total_mismatch: "Base + IVA no coincide con el total",
  possible_duplicate: "Posible duplicado",
  amount_to_review: "Importe dudoso: confírmelo con el documento",
};

const RECOVERY_LABEL: Record<string, string> = {
  no_decimals: "sin decimales",
  one_decimal: "con un solo decimal",
  ambiguous_separator: "el punto o la coma podría ser de miles",
};

const AMOUNT_LABEL: Record<string, string> = { subtotal: "Base imponible", tax: "IVA", total: "Total" };

/** Facturas con importes dudosos ya avisadas en esta sesión, para no repetir el aviso. */
const announced = new Set<number>();

const FIELD_LABEL: Record<string, string> = {
  supplier: "Proveedor",
  invoice_number: "Número",
  date: "Fecha",
  subtotal: "Base imponible",
  tax: "IVA",
  total: "Total",
  currency: "Moneda",
};

const ACTION_LABEL: Record<string, string> = {
  register_invoice: "Registrar factura",
  send_draft: "Enviar respuesta",
  rule: "Regla",
};

type View = "review" | "tasks" | "rules" | "history" | "mail" | "settings" | "setup";

const PROVIDER_LABEL: Record<MailProviderKind, string> = {
  simulator: "Simulador",
  google: "Google (Gmail)",
  microsoft: "Microsoft 365 / Outlook",
};

const ACCOUNT_STATUS: Record<string, [string, string]> = {
  connected: ["Conectada", "state-completed"],
  signed_out: ["Sin sesión", "state-failed"],
  not_configured: ["Proveedor no configurado", "state-failed"],
  credentials_unavailable: ["Credenciales no disponibles", "state-failed"],
};

// Errores del contrato de correo, en palabras del usuario.
const MAIL_ERROR: Record<string, string> = {
  unauthorized: "Sesión no válida",
  unavailable: "Proveedor o red no disponibles",
  not_found: "No encontrado",
  conflict: "Conflicto",
  invalid_input: "Datos no válidos",
};

const state = {
  view: "review" as View,
  // Pendientes (D137): pestaña, seleccionado de cada una y orden mostrado, para pasar al siguiente.
  reviewTab: null as ReviewTab | null,
  selected: { invoices: null, emails: null } as Record<ReviewTab, number | null>,
  reviewOrder: { invoices: [], emails: [] } as Record<ReviewTab, number[]>,
  // Clave del control que debe recibir el foco tras el repintado (flechas de la lista).
  focusAfterRender: null as string | null,
  // Filtro de la lista de pendientes por origen: "" (todo), "file" o el ID de una cuenta.
  originFilter: "",
  tasks: [] as Task[],
  rules: [] as Rule[],
  audit: [] as AuditEntry[],
  settings: null as SettingsView | null,
  // Proveedor desplegado en Configuración (null: el elegido) y asistente de primer uso.
  pick: null as ProviderChoice["provider"] | null,
  wizard: false,
  wizardStep: "choose" as "choose" | "cloud" | "other",
  wizardOther: "openai_compatible" as "openai_compatible" | "anthropic",
  mail: null as MailView | null,
  // Dirección del inicio de sesión en curso, por si el navegador no se abrió solo.
  signInUrl: null as string | null,
  notice: null as { kind: "ok" | "error" | "warn"; text: string; task?: number } | null,
  /** Tareas cuyos importes dudosos ha confirmado la persona antes de aprobar. */
  amountsChecked: {} as Record<number, boolean>,
  // Borradores editados y aún no guardados, para no perderlos al refrescar.
  // Incluye los formularios de reglas en curso, con clave «rule:new» o «rule:edit:<id>».
  drafts: {} as Record<string, string>,
  // Panel de reglas: pestaña, filtros de «Mis reglas» y formulario abierto.
  rulesTab: "mine" as "mine" | "categories",
  ruleFilter: { text: "", action: "", status: "", category: "" },
  ruleEditor: null as null | { mode: "new" } | { mode: "edit"; id: string },
  // Preparación (R08): modo instalador, última comprobación y la ejecución en curso.
  prepareOnly: false,
  check: null as SetupCheck | null,
  memory: null as Memory | null,
  startup: null as Startup | null,
  preparing: false,
  steps: {} as Partial<Record<SetupStep, SetupEvent>>,
  setupDone: null as SetupDone | null,
};

const root = document.getElementById("app")!;

function notify(kind: "ok" | "error", text: unknown) {
  state.notice = { kind, text: String(text) };
  render();
}

async function act(work: () => Promise<unknown>, success?: string) {
  try {
    await work();
    if (success) state.notice = { kind: "ok", text: success };
    await refresh();
  } catch (error) {
    notify("error", error);
  }
}

/// Aviso según cómo terminó una aprobación: hecha, ya hecha en un intento anterior, de vuelta
/// en revisión (hay que confirmar algo) o fallida.
function afterApproval(task: Task, done: string) {
  const r = task.result ?? {};
  if (task.state === "completed") {
    state.notice = r.recovered
      ? { kind: "ok", text: `Ya constaba realizada la versión ${r.recovered.version}; no se ha repetido` }
      : { kind: "ok", text: done };
  } else if (task.state === "failed") {
    state.notice = { kind: "error", text: task.error?.message ?? "la acción falló" };
  } else {
    state.notice = { kind: "warn", text: `No se ha realizado: ${r.review_reason ?? "requiere revisión"}` };
  }
}

/// Explica un intento anterior sin confirmar (D50) y, si el borrador ya no existe, ofrece
/// confirmar que no se envió para preparar una versión nueva.
function recoveryBox(task: Task, verb: string) {
  const r = task.result ?? {};
  // Tras confirmar que no salió, la versión mostrada ya es otra y el aviso no aplica.
  if (!r.recovery && r.attempt?.version !== (r.version ?? 1)) return null;
  const version = r.recovery?.version ?? r.attempt?.version;
  const error = r.recovery?.error;
  const missing = r.recovery?.needs_confirmation === true;
  return h(
    "div",
    { class: "recovery" },
    h(
      "p",
      {},
      h("strong", {}, `Ya se intentó ${verb} la versión ${version} y no consta confirmado`),
      error ? ` (último error: ${error})` : "",
      ". Se conserva el contenido aprobado; la IA no ha propuesto otro.",
    ),
    missing
      ? h(
          "p",
          {},
          "El borrador de ese intento ya no existe en el buzón y no consta enviado. Si lo borró usted y está seguro de que no salió, confírmelo para preparar uno nuevo con el texto de abajo; después tendrá que aprobarlo.",
        )
      : h(
          "p",
          {},
          `Aprobar lo vuelve a intentar con la misma clave: si ya se hizo, solo se confirma y nunca se repite. Si cambia el contenido, antes de ${verb} la versión nueva se comprueba que la anterior no se realizó.`,
        ),
    missing
      ? h(
          "div",
          { class: "buttons" },
          h(
            "button",
            { onclick: () => void act(() => api.confirmNotSent(task.id).then((t) => afterConfirm(t))) },
            "Confirmo que no se envió: preparar uno nuevo",
          ),
        )
      : null,
  );
}

function afterConfirm(task: Task) {
  state.notice =
    task.state === "completed"
      ? { kind: "ok", text: "El proveedor lo ve realizado: la tarea se ha completado sin repetirlo" }
      : { kind: "ok", text: `Preparada la versión ${task.result?.version}; revísela y apruébela` };
}

async function refresh(background = false) {
  try {
    const [tasks, rules, audit, settings, mail] = await Promise.all([
      api.tasks(),
      api.rules(),
      api.auditLog(),
      api.settings(),
      api.mailAccounts(),
    ]);
    Object.assign(state, { tasks, rules, audit, settings, mail });
    announceDoubtfulAmounts();
  } catch (error) {
    state.notice = { kind: "error", text: String(error) };
  }
  render(background);
}

function title(task: Task): string {
  const input = safeJson(task.input_ref);
  if (task.kind === "invoice") return task.result?.document ?? input.document ?? `Documento ${task.id}`;
  return task.result?.message?.subject ?? (input.message_id ? `Mensaje ${input.message_id}` : `Correo ${task.id}`);
}

function safeJson(text: string): Record<string, any> {
  try {
    return JSON.parse(text) ?? {};
  } catch {
    return {};
  }
}

function badge(taskState: string) {
  return h("span", { class: `badge state-${taskState}` }, STATE_LABEL[taskState] ?? taskState);
}

/** Avisa una vez de cada factura que llega con importes dudosos (D120). */
function announceDoubtfulAmounts() {
  for (const t of state.tasks) {
    const message = t.result?.amount_review;
    if (t.state !== "awaiting_review" || !message || announced.has(t.id)) continue;
    announced.add(t.id);
    // El detalle da la ruta y los importes; el aviso, una línea con el documento y cómo llegar.
    state.notice = { kind: "warn", text: `Importes dudosos en «${title(t)}»: revíselos antes de aprobar. Solo se registra con su aprobación.`, task: t.id };
  }
}

/** Abre una tarea en su pestaña de Pendientes, quitando el filtro si la ocultara. */
function openInReview(task: Task) {
  const tab = tabOf(task.kind);
  state.view = "review";
  state.reviewTab = tab;
  state.selected[tab] = task.id;
  state.originFilter = "";
  state.notice = null;
  render();
}

function missing() {
  return h("span", { class: "missing" }, "No consta");
}

// ---- Vistas ----

function header() {
  const working = state.tasks.filter((t) => t.state === "queued" || t.state === "running").length;
  const active = state.settings?.active;
  const fileInput = h("input", {
    type: "file",
    accept: ".txt,.pdf,.jpg,.jpeg,.png,.tif,.tiff,.bmp,.webp,text/plain,application/pdf,image/*",
    multiple: true,
    hidden: true,
    onchange: (event) => {
      const input = event.target as HTMLInputElement;
      const files = [...(input.files ?? [])];
      input.value = "";
      void act(async () => {
        for (const file of files) await api.importFile(file.name, new Uint8Array(await file.arrayBuffer()));
      }, files.length === 1 ? `«${files[0]?.name}» añadido a la cola` : `${files.length} documentos añadidos a la cola`);
    },
  }) as HTMLInputElement;
  return h(
    "header",
    { class: "topbar" },
    h("div", { class: "brand" }, h("img", { src: "/icon.svg", alt: "" }), h("span", {}, "NestAdmin")),
    h(
      "div",
      { class: "status" },
      working ? h("span", { class: "pill working" }, `${working} en proceso`) : null,
      state.settings?.ai_wait_message
        ? h("button", { class: "pill provider waiting", title: state.settings.ai_wait_message, onclick: goToSettings }, "IA sin configurar")
        : active
          ? h(
              "span",
              { class: `pill provider ${active.provider}`, title: "Proveedor de IA activo" },
              providerLabel(active.provider, active.model),
            )
          : null,
    ),
    h(
      "div",
      { class: "actions" },
      fileInput,
      h(
        "button",
        {
          class: "primary",
          onclick: () => {
            // En la aplicación, el selector del sistema da la ruta original (D49); en la vista
            // previa del navegador solo hay nombre y contenido.
            if (isPreview) return fileInput.click();
            void act(async () => {
              const report = await api.importDialog();
              if (report.errors.length) throw report.errors.join(" · ");
              if (report.imported.length)
                state.notice = {
                  kind: "ok",
                  text:
                    report.imported.length === 1
                      ? `«${title(report.imported[0]!)}» añadido a la cola`
                      : `${report.imported.length} documentos añadidos a la cola`,
                };
            });
          },
        },
        "Importar documento",
      ),
      h(
        "button",
        {
          onclick: () =>
            void act(async () => {
              state.notice = fetchNotice(await api.fetchMail());
            }),
        },
        "Consultar correo",
      ),
    ),
  );
}

function nav() {
  // Entrada: vista, etiqueta, icono, contador y si el contador avisa de un problema.
  type Entry = [View, string, IconName, number?, boolean?];
  const groups: Array<[string, Entry[]]> = [
    [
      "Trabajo",
      [
        ["review", "Pendientes", "inbox", state.tasks.filter((t) => t.state === "awaiting_review").length],
        ["tasks", "Tareas", "tasks"],
        ["history", "Historial", "history"],
      ],
    ],
    ["Automatización", [["rules", "Reglas", "rules"]]],
    [
      "Sistema",
      [
        ["mail", "Cuentas de correo", "mail", state.mail?.accounts.some(needsAttention) ? 1 : undefined, true],
        ["settings", "Configuración", "settings"],
        ["setup", "Preparación", "setup", (state.check && state.check.status !== "ready") || state.memory?.level === "insufficient" ? 1 : undefined, true],
      ],
    ],
  ];
  return h(
    "nav",
    { class: "sidebar", "aria-label": "Secciones" },
    ...groups.flatMap(([group, entries]) => [
      h("div", { class: "nav-group" }, group),
      ...entries.map(([view, label, iconName, count, attention]) =>
        h(
          "button",
          {
            class: state.view === view ? "nav active" : "nav",
            title: label,
            "aria-current": state.view === view ? "page" : undefined,
            "data-focus-key": `nav:${view}`,
            onclick: () => {
              state.view = view;
              state.notice = null;
              if (view === "settings") resetBlocks();
              render();
            },
          },
          icon(iconName),
          h("span", { class: "nav-label" }, label),
          count ? h("span", { class: attention ? "count attention" : "count" }, count) : null,
        ),
      ),
    ]),
    isPreview ? h("p", { class: "preview-note" }, "Vista previa en navegador con datos sintéticos.") : null,
  );
}

const TAB_NAME: Record<ReviewTab, string> = { invoices: "Facturas", emails: "Correos" };

/** Fecha corta de una fila. Sin dato, «Sin fecha». */
function shortDate(value: unknown): string {
  if (typeof value !== "string" && typeof value !== "number") return "Sin fecha";
  const date = typeof value === "number" ? new Date(value * 1000) : new Date(value);
  if (Number.isNaN(date.getTime())) return String(value);
  return date.toLocaleDateString("es-ES", { day: "2-digit", month: "short", ...(typeof value === "string" && /^\d{4}-\d{2}-\d{2}$/.test(value) ? { timeZone: "UTC" } : {}) });
}

function flagChips(flags: Flag[]) {
  return flags.map((f) => statusChip(f.label, f.tone));
}

/** Fila de una factura: quién e importe, número y fecha, de dónde viene y qué falla. */
function invoiceRow(task: Task, ageLabel: string): Child[] {
  const fields = task.result?.fields ?? {};
  const flags = invoiceFlags(task.result);
  const input = safeJson(task.input_ref);
  const origin: DocumentOrigin | undefined = task.result?.origin ?? input.origin;
  const source = origin?.kind === "email" ? `Adjunto de ${origin.from ?? "remitente no consta"}` : title(task);
  const part = task.result?.part?.of > 1 ? ` · parte ${task.result?.part.index} de ${task.result?.part.of}` : "";
  return [
    h(
      "span",
      { class: "item-line" },
      fields.supplier ? h("span", { class: "item-title" }, fields.supplier) : h("span", { class: "item-title missing" }, "Proveedor no consta"),
      h("span", { class: "item-amount" }, formatAmount(fields.total, fields.currency) ?? h("span", { class: "missing" }, "Total no consta")),
    ),
    h("span", { class: "item-sub" }, `${fields.invoice_number ?? "Sin número"} · ${shortDate(fields.date)}${part}`),
    h("span", { class: "item-sub" }, `${source} · ${ageLabel}`),
    flags.length ? h("span", { class: "item-flags" }, ...flagChips(flags)) : null,
  ];
}

/** Fila de un correo (R13, D48): remitente, asunto y cuenta, categoría y qué falla. */
function emailRow(task: Task): Child[] {
  const input = safeJson(task.input_ref);
  const message = task.result?.message ?? input.message ?? {};
  const account = accountShort(taskMailOrigin(task), accountId(message.id ?? input.message_id));
  const category = task.result?.category;
  return [
    h(
      "span",
      { class: "item-line" },
      message.from ? h("span", { class: "item-title" }, message.from) : h("span", { class: "item-title missing" }, "Remitente no consta"),
      h("span", { class: "item-side" }, shortDate(message.date)),
    ),
    h("span", { class: "item-sub" }, message.subject ?? "(sin asunto)"),
    h("span", { class: "item-sub" }, `Cuenta: ${account}`),
    h(
      "span",
      { class: "item-flags" },
      isKnownKind(task.kind) ? null : statusChip("Tipo no reconocido", "error"),
      category ? statusChip(CATEGORY_LABEL[category] ?? category) : null,
      ...flagChips(emailFlags(task.result)),
    ),
  ];
}

function reviewView() {
  const all = sortForReview(state.tasks.filter((t) => t.state === "awaiting_review"));
  const split = splitByTab(all);
  const tab = chooseTab(state.reviewTab, { invoices: split.invoices.length, emails: split.emails.length });
  state.reviewTab = tab;
  const inTab = split[tab];
  // Orígenes de esta pestaña, para el filtro: archivos importados y cada cuenta.
  const origins = new Map<string, string>();
  for (const t of inTab) {
    const key = originKey(t);
    if (!origins.has(key)) origins.set(key, key === "file" ? "Archivos importados" : accountShort(taskMailOrigin(t), key));
  }
  if (state.originFilter && !origins.has(state.originFilter)) state.originFilter = "";
  const pending = inTab.filter((t) => !state.originFilter || originKey(t) === state.originFilter);
  const ids = pending.map((t) => t.id);
  state.selected[tab] = nextSelection(state.reviewOrder[tab], ids, state.selected[tab]);
  state.reviewOrder[tab] = ids;
  const selected = pending.find((t) => t.id === state.selected[tab]);
  const now = Date.now() / 1000;
  const otherTab: ReviewTab = tab === "invoices" ? "emails" : "invoices";
  const moveTo = (id: number | undefined) => {
    if (id === undefined) return;
    state.selected[tab] = id;
    render();
  };
  const filter = h(
    "select",
    {
      "aria-label": "Filtrar por origen",
      "data-focus-key": "review-filter",
      onchange: (event) => {
        state.originFilter = (event.target as HTMLSelectElement).value;
        render();
      },
    },
    h("option", { value: "" }, `Todos los orígenes (${inTab.length})`),
    ...[...origins].map(([key, label]) =>
      h("option", { value: key, selected: key === state.originFilter }, `${label} (${inTab.filter((t) => originKey(t) === key).length})`),
    ),
  );
  return h(
    "section",
    { class: "split" },
    h(
      "div",
      { class: "list" },
      h(
        "div",
        { class: "list-head" },
        h("h2", {}, "Pendientes de revisión"),
        tabs({
          label: "Tipo de pendiente",
          panelId: "review-list",
          active: tab,
          items: (["invoices", "emails"] as ReviewTab[]).map((key) => ({ key, label: TAB_NAME[key], count: split[key].length })),
          onSelect: (key) => {
            state.reviewTab = key;
            state.originFilter = "";
            render();
          },
        }),
      ),
      origins.size > 1 ? h("label", { class: "filter" }, "Origen", filter) : null,
      h(
        "div",
        { class: "list-items", id: "review-list", role: "tabpanel", "aria-label": TAB_NAME[tab], "data-scroll-key": `list:${tab}` },
        all.length === 0
          ? emptyState("No hay nada pendiente", "Las tareas nuevas aparecerán aquí.")
          : inTab.length === 0
            ? emptyState(
                `No hay ${TAB_NAME[tab].toLowerCase()} pendientes`,
                null,
                h(
                  "button",
                  {
                    onclick: () => {
                      state.reviewTab = otherTab;
                      render();
                    },
                  },
                  `Ver ${TAB_NAME[otherTab].toLowerCase()} (${split[otherTab].length})`,
                ),
              )
            : null,
        ...pending.map((t, index) =>
          h(
            "button",
            {
              class: t.id === state.selected[tab] ? "item selected" : "item",
              "aria-current": t.id === state.selected[tab] ? "true" : undefined,
              "data-focus-key": `item:${t.id}`,
              onclick: () => moveTo(t.id),
              // Las flechas solo mueven la selección: aprobar o descartar nunca tienen atajo (D138).
              onkeydown: (event) => {
                const key = (event as KeyboardEvent).key;
                const target = key === "ArrowDown" ? index + 1 : key === "ArrowUp" ? index - 1 : key === "Home" ? 0 : key === "End" ? pending.length - 1 : -1;
                if (target < 0 || target >= pending.length) return;
                event.preventDefault();
                state.focusAfterRender = `item:${pending[target]!.id}`;
                moveTo(pending[target]!.id);
              },
            },
            ...(tabOf(t.kind) === "invoices" ? invoiceRow(t, ageText(t.created_at, now)) : emailRow(t)),
          ),
        ),
      ),
    ),
    selected ? (tabOf(selected.kind) === "invoices" ? invoiceDetail(selected) : emailDetail(selected)) : h("div", { class: "detail" }),
  );
}

/** Lista de incidencias con su porqué siempre a la vista (lo crítico nunca va en una ayuda). */
function flagList(flags: Flag[]) {
  const shown = flags.filter((f) => !f.recovery);
  return shown.length
    ? h("ul", { class: "flag-list" }, ...shown.map((f) => h("li", {}, statusChip(f.label, f.tone), h("span", {}, f.why))))
    : null;
}

function openOriginal(task: Task) {
  return h(
    "button",
    {
      onclick: () =>
        isPreview
          ? ((state.notice = { kind: "ok", text: "Vista previa: en la aplicación se abriría la copia guardada con el visor del sistema." }), render())
          : void act(() => api.openOriginal(task.id)),
    },
    "Abrir original",
  );
}

const EXTERNAL_NAME: Record<Exclude<ProviderInfo["provider"], "ollama">, string> = {
  // Resultados antiguos con proveedor Groq (retirado, D100): solo se muestra su procedencia.
  groq: "Groq (retirado)",
  ollama_cloud: "Ollama cloud",
  openai_compatible: "Compatible con OpenAI",
  anthropic: "Claude · Anthropic",
};

function providerLabel(provider: ProviderInfo["provider"], model: string): string {
  if (provider === "ollama") return `Local · ${model}`;
  return `${EXTERNAL_NAME[provider]} (externo) · ${model}`;
}

/// Host de una URL base para mostrarlo en el aviso; si no es una URL, el texto tal cual.
function hostOf(baseUrl: string): string {
  try {
    return new URL(baseUrl.trim()).hostname || baseUrl.trim();
  } catch {
    return baseUrl.trim() || "el servicio configurado";
  }
}

/// Texto del respaldo local (D101, D102): qué proveedor externo falló y por qué.
function fallbackText(fallback: { from: string; reason: string }): string {
  const name = EXTERNAL_NAME[fallback.from as keyof typeof EXTERNAL_NAME] ?? fallback.from;
  return fallback.reason === "rate_limited"
    ? `Procesado con la IA local porque ${name} alcanzó su límite`
    : `Procesado con la IA local porque ${name} no respondió`;
}

function fallbackNotice(result: Record<string, any>) {
  return result.fallback ? h("div", { class: "notice warn", role: "status" }, h("span", {}, fallbackText(result.fallback))) : null;
}

function provenance(result: Record<string, any>) {
  return h(
    "p",
    { class: "meta" },
    `Propuesta de ${result.provider in EXTERNAL_NAME ? `${EXTERNAL_NAME[result.provider as keyof typeof EXTERNAL_NAME]} (externo)` : "Ollama local"} · ${result.model}`,
    result.latency_ms ? ` · ${(result.latency_ms / 1000).toFixed(1)} s` : "",
    ` · versión ${result.version}`,
  );
}

function invoiceDetail(task: Task) {
  const r = task.result ?? {};
  const fields: Record<string, string | null> = r.fields ?? {};
  const issues: string[] = r.issues ?? [];
  const unreadable: Array<{ field: string }> = r.unreadable ?? [];
  const recovered: Array<{ field: string; literal: string; value: string; reason: string }> = r.recovered ?? [];
  const doubtful = Boolean(r.amount_review);
  const checked = !doubtful || Boolean(state.amountsChecked[task.id]);
  const pages: Array<{ number: number; method: string }> = r.pages ?? [];
  const ocrPages = pages.filter((p) => p.method === "ocr").map((p) => p.number);
  const sources: Record<string, FieldSource> = r.sources ?? {};
  const origin: DocumentOrigin | null = r.origin ?? safeJson(task.input_ref).origin ?? null;
  const needsConfirmation = r.recovery?.needs_confirmation === true;
  const article = h(
    "article",
    {},
    h(
      "div",
      { class: "detail-head" },
      h(
        "div",
        {},
        h("h2", {}, fields.supplier ?? h("span", { class: "missing" }, "Proveedor no consta")),
        h("p", { class: "detail-sub" }, `${fields.invoice_number ?? "Sin número"} · ${title(task)}`),
      ),
      h("div", { class: "detail-amount" }, formatAmount(fields.total, fields.currency) ?? h("span", { class: "missing" }, "Total no consta")),
    ),
    provenance(r),
    fallbackNotice(r),
    r.part && r.part.of > 1
      ? h(
          "p",
          { class: "meta warn" },
          `Factura ${r.part.index} de ${r.part.of} de este documento: la IA separó las facturas que contiene. Compruebe con el original que los datos de cada una no se mezclan.`,
        )
      : null,
    pages.length
      ? h(
          "p",
          { class: "meta" },
          `${pages.length} ${pages.length === 1 ? "página" : "páginas"}`,
          ocrPages.length
            ? ` · texto obtenido por OCR local en ${ocrPages.length === pages.length ? "todas" : `la(s) página(s) ${ocrPages.join(", ")}`}; compruebe los importes con el original`
            : "",
        )
      : null,
    doubtful
      ? h(
          "div",
          { class: "amount-review", role: "alert" },
          h("strong", {}, "Revise los importes antes de aprobar"),
          r.document_path ? h("p", { class: "meta" }, "Archivo: ", h("code", {}, r.document_path)) : null,
          recovered.length
            ? h(
                "ul",
                {},
                ...recovered.map((a) =>
                  h(
                    "li",
                    {},
                    `${AMOUNT_LABEL[a.field] ?? a.field}: el documento dice «${a.literal}»; se propone ${a.value} (${RECOVERY_LABEL[a.reason] ?? a.reason}).`,
                  ),
                ),
              )
            : null,
          h(
            "label",
            { class: "confirm" },
            h("input", {
              type: "checkbox",
              ...(checked ? { checked: "" } : {}),
              onchange: (e: Event) => {
                state.amountsChecked[task.id] = (e.target as HTMLInputElement).checked;
                render();
              },
            }),
            " He comprobado los importes con el documento",
          ),
        )
      : h("p", { class: "reason" }, `Motivo de la revisión: ${r.review_reason ?? "requiere aprobación"}`),
    recoveryBox(task, "registrar"),
    flagList(invoiceFlags(r)) ?? (issues.length ? null : h("p", { class: "ok-line" }, "Sin incidencias en los importes.")),
    h("h3", {}, "Datos extraídos"),
    h(
      "table",
      { class: "fields" },
      h("thead", {}, h("tr", {}, h("th", { scope: "col" }, "Campo"), h("th", { scope: "col" }, "Valor"), h("th", { scope: "col" }, "Dónde aparece en el original"))),
      h(
        "tbody",
        {},
        ...Object.keys(FIELD_LABEL).map((key) =>
          h(
            "tr",
            {},
            h("th", { scope: "row" }, FIELD_LABEL[key]),
            h(
              "td",
              {},
              fields[key] ?? missing(),
              unreadable.some((u) => u.field === key) ? h("span", { class: "missing" }, " (ilegible)") : null,
              recovered.some((a) => a.field === key) ? h("span", { class: "proposed" }, " (propuesta: revísela)") : null,
            ),
            h("td", { class: "source" }, fieldSource(fields[key], sources[key], pages.length)),
          ),
        ),
      ),
    ),
    h("h3", {}, "Origen del documento"),
    documentOrigin(origin),
  );
  return h(
    "div",
    { class: "detail" },
    h("div", { class: "detail-scroll", "data-scroll-key": `detail:${task.id}` }, article),
    actionBar(
      !checked ? "Para aprobar, confirme antes los importes con el documento." : needsConfirmation ? "Para aprobar, confirme primero que el intento anterior no se envió." : null,
      h(
        "button",
        {
          class: "primary",
          disabled: !checked || needsConfirmation,
          onclick: () => void act(async () => afterApproval(await api.approve(task.id, r.version ?? 1), "Factura registrada en el simulador")),
        },
        "Aprobar y registrar",
      ),
      openOriginal(task),
      h("button", { class: "danger", onclick: () => void act(() => api.reject(task.id), "Tarea descartada") }, "Descartar"),
    ),
  );
}

function emailDetail(task: Task) {
  const r = task.result ?? {};
  const message = r.message ?? {};
  const draft = r.draft ?? {};
  const body = h("textarea", {
    rows: "10",
    "aria-label": "Borrador de respuesta",
    "data-focus-key": `draft:${task.id}`,
    oninput: () => (state.drafts[task.id] = body.value),
  }) as HTMLTextAreaElement;
  body.value = state.drafts[task.id] ?? draft.body ?? "";
  const recipients: string[] = draft.to ?? [];
  const needsConfirmation = r.recovery?.needs_confirmation === true;
  const article = h(
    "article",
    {},
    h(
      "div",
      { class: "detail-head" },
      h("h2", {}, message.subject ?? h("span", { class: "missing" }, "(sin asunto)")),
      r.category ? statusChip(CATEGORY_LABEL[r.category] ?? r.category) : null,
    ),
    provenance(r),
    isKnownKind(task.kind) ? null : h("div", { class: "notice error", role: "alert" }, h("span", {}, `Tipo de tarea no reconocido: ${task.kind}. Se muestra como correo; compruébelo antes de aprobar.`)),
    fallbackNotice(r),
    h("p", { class: "reason" }, `Motivo de la revisión: ${r.review_reason ?? "requiere aprobación"}`),
    recoveryBox(task, "enviar"),
    h(
      "dl",
      { class: "origin" },
      h("dt", {}, "Cuenta"),
      h("dd", {}, accountLine(r.origin ?? safeJson(task.input_ref).origin, message.id)),
      h("dt", {}, "De"),
      h("dd", {}, message.from ?? missing()),
      h("dt", {}, "Para"),
      h("dd", {}, Array.isArray(message.to) && message.to.length ? message.to.join(", ") : missing()),
      h("dt", {}, "Recibido"),
      h("dd", {}, message.date ? dateText(message.date) : missing()),
    ),
    h("div", { class: "message" }, h("p", {}, message.body ?? missing())),
    h("h3", {}, "Borrador de respuesta"),
    h("p", { class: "meta" }, "Para: ", recipients.length ? recipients.join(", ") : h("span", { class: "missing" }, "sin destinatario; no se puede enviar")),
    body,
  );
  return h(
    "div",
    { class: "detail" },
    h("div", { class: "detail-scroll", "data-scroll-key": `detail:${task.id}` }, article),
    actionBar(
      recipients.length === 0 ? "Sin destinatario: no se puede enviar." : needsConfirmation ? "Para aprobar, confirme primero que el intento anterior no se envió." : null,
      h(
        "button",
        {
          class: "primary",
          // Con el borrador anterior desaparecido, primero hay que confirmar que no se envió.
          disabled: recipients.length === 0 || needsConfirmation,
          onclick: () =>
            void act(async () => {
              let version = r.version ?? 1;
              // Si se ha editado, se guarda primero como versión nueva y se aprueba esa.
              if (body.value !== (draft.body ?? "")) version = (await api.editDraft(task.id, body.value)).result?.version;
              afterApproval(await api.approve(task.id, version), `Respuesta enviada desde ${accountName(message.id, r.origin)}`);
            }),
        },
        "Aprobar y enviar",
      ),
      h("button", { onclick: () => void act(() => api.editDraft(task.id, body.value), "Borrador guardado como versión nueva") }, "Guardar borrador"),
      h("button", { class: "danger", onclick: () => void act(() => api.reject(task.id), "Tarea descartada") }, "Descartar"),
    ),
  );
}

function tasksView() {
  return h(
    "section",
    { class: "page" },
    h("h2", {}, "Tareas"),
    state.tasks.length === 0 ? h("p", { class: "empty" }, "Todavía no hay tareas. Importe un documento o consulte el correo.") : null,
    state.tasks.length
      ? h(
          "table",
          { class: "grid" },
          h("thead", {}, h("tr", {}, ...["#", "Tipo", "Elemento", "Estado", "Intentos", "Detalle", ""].map((c) => h("th", {}, c)))),
          h(
            "tbody",
            {},
            ...state.tasks.map((t) =>
              h(
                "tr",
                {},
                h("td", {}, t.id),
                h("td", {}, t.kind === "invoice" ? "Factura" : "Correo"),
                h("td", {}, title(t)),
                h("td", {}, badge(t.state)),
                h("td", {}, t.attempts),
                h(
                  "td",
                  { class: "detail-cell" },
                  t.error
                    ? h("span", { class: "error-text" }, t.error.message)
                    : t.state === "queued" && state.settings?.ai_wait_message
                      ? h("span", { class: "warn-text" }, state.settings.ai_wait_message)
                      : t.result?.issues?.length
                        ? t.result.issues.map((i: string) => ISSUE_LABEL[i] ?? i).join(", ")
                        : "",
                  t.result?.fallback ? h("span", { class: "meta" }, ` ${fallbackText(t.result.fallback)}`) : null,
                ),
                h(
                  "td",
                  {},
                  t.state === "failed" ? h("button", { onclick: () => void act(() => api.retry(t.id), "Tarea reencolada") }, "Reintentar") : null,
                  t.state === "awaiting_review"
                    ? h(
                        "button",
                        {
                          onclick: () => openInReview(t),
                        },
                        "Revisar",
                      )
                    : null,
                ),
              ),
            ),
          ),
        )
      : null,
  );
}

let helpSeq = 0;

/** Etiqueta con su ayuda al lado: la ayuda va fuera de `<label>` para no activar el campo. */
function field(labelText: string, control: HTMLElement, helpText: string, tag?: string): HTMLElement {
  const controlId = control.id || `field-${++helpSeq}`;
  control.id = controlId;
  return h(
    "div",
    { class: "field" },
    h(
      "div",
      { class: "field-head" },
      h("label", { for: controlId }, labelText),
      tag ? h("span", { class: "tag" }, tag) : null,
      helpTip(helpText, `Ayuda sobre ${labelText.toLowerCase()}`),
    ),
    control,
  );
}

// ---- Panel de reglas (R21, R22) ----

interface RuleDraft {
  id: string;
  kind: string;
  supplier: string;
  max_total: string;
  category: string;
}

const newDraftKey = "rule:new";
const editDraftKey = (id: string) => `rule:edit:${id}`;

function draftOf(rule: Rule | null): RuleDraft {
  return {
    id: rule?.id ?? "",
    kind: rule?.action_kind ?? "register_invoice",
    supplier: rule?.conditions.supplier ?? "",
    max_total: rule?.conditions.max_total ?? "",
    category: rule?.conditions.category ?? Object.keys(CATEGORY_LABEL)[0],
  };
}

function draftConditions(d: RuleDraft): Record<string, string> {
  if (d.kind === "register_invoice") {
    return Object.fromEntries(Object.entries({ supplier: d.supplier.trim(), max_total: d.max_total.trim() }).filter(([, v]) => v));
  }
  return { category: d.category };
}

function setRulesTab(tab: "mine" | "categories") {
  state.rulesTab = tab;
  render();
}

/** Lleva a «Mis reglas» filtradas por una categoría. */
function showRulesOf(category: string) {
  state.ruleFilter = { text: "", action: "", status: "", category };
  state.rulesTab = "mine";
  render();
}

function openRuleEditor(editor: NonNullable<typeof state.ruleEditor>, draft?: RuleDraft) {
  state.ruleEditor = editor;
  if (draft) state.drafts[editor.mode === "new" ? newDraftKey : editDraftKey(editor.id)] = JSON.stringify(draft);
  state.rulesTab = "mine";
  render();
}

function closeRuleEditor() {
  const editor = state.ruleEditor;
  if (editor) delete state.drafts[editor.mode === "new" ? newDraftKey : editDraftKey(editor.id)];
  state.ruleEditor = null;
}

function ruleForm(): HTMLElement {
  const editor = state.ruleEditor!;
  const editing = editor.mode === "edit" ? (state.rules.find((r) => r.id === editor.id) ?? null) : null;
  const key = editor.mode === "new" ? newDraftKey : editDraftKey(editor.id);
  let draft: RuleDraft;
  try {
    draft = state.drafts[key] ? (JSON.parse(state.drafts[key]) as RuleDraft) : draftOf(editing);
  } catch {
    draft = draftOf(editing);
  }
  const id = h("input", { name: "id", "data-focus-key": "rule-form:id", required: true, placeholder: "p. ej. alfa-mensual", pattern: "[a-z0-9-]+", autocomplete: "off" }) as HTMLInputElement;
  const kind = h(
    "select",
    { name: "kind", "data-focus-key": "rule-form:kind" },
    h("option", { value: "register_invoice" }, "Registrar factura"),
    h("option", { value: "send_draft" }, "Enviar respuesta"),
  ) as HTMLSelectElement;
  const supplier = h("input", { name: "supplier", "data-focus-key": "rule-form:supplier", required: true, placeholder: "Nombre exacto del proveedor", autocomplete: "off" }) as HTMLInputElement;
  const maxTotal = h("input", { name: "max_total", "data-focus-key": "rule-form:max_total", placeholder: "p. ej. 500.00", inputmode: "decimal", autocomplete: "off" }) as HTMLInputElement;
  const category = h(
    "select",
    { name: "category", "data-focus-key": "rule-form:category" },
    ...Object.entries(CATEGORY_LABEL).map(([value, label]) => h("option", { value }, label)),
  ) as HTMLSelectElement;
  id.value = draft.id;
  kind.value = draft.kind;
  supplier.value = draft.supplier;
  maxTotal.value = draft.max_total;
  category.value = draft.category;

  const invoiceFields = h(
    "div",
    { class: "field-group" },
    field("Proveedor", supplier, FIELD_HELP.supplier, "obligatorio"),
    h(
      "p",
      { class: "hint" },
      "Escríbalo tal como lo lee la aplicación en la factura: «Alfa SL» no coincide con «Alfa, S.L.». No distingue mayúsculas de minúsculas.",
    ),
    field("Total máximo", maxTotal, FIELD_HELP.max_total, "opcional"),
  );
  const emailFields = h("div", { class: "field-group" }, field("Categoría del correo", category, FIELD_HELP.category, "obligatorio"));
  const previewSentence = h("p", { class: "rule-sentence" });
  const previewRisk = h("p", { class: "rule-risk" });
  const previewProblem = h("p", { class: "rule-problem" });
  const preview = h("div", { class: "rule-preview", "aria-live": "polite" }, h("p", { class: "meta" }, "Vista previa de la regla"), previewSentence, previewRisk, previewProblem);

  const read = (): RuleDraft => ({
    id: editing ? editing.id : id.value.trim(),
    kind: editing ? editing.action_kind : kind.value,
    supplier: supplier.value,
    max_total: maxTotal.value,
    category: category.value,
  });
  const sync = () => {
    const d = read();
    state.drafts[key] = JSON.stringify(d);
    invoiceFields.hidden = d.kind !== "register_invoice";
    emailFields.hidden = d.kind === "register_invoice";
    supplier.required = d.kind === "register_invoice";
    const rule = { id: d.id, version: 0, action_kind: d.kind, conditions: draftConditions(d), active: false };
    previewSentence.textContent = ruleSentence(rule);
    previewRisk.textContent = `Aviso: ${ruleRisk(rule)}`;
    const problem = ruleProblem(rule);
    previewProblem.textContent = problem ?? "";
    previewProblem.hidden = !problem;
  };

  const wasActive = editing?.active ?? false;
  const form = h(
    "form",
    {
      class: "card rule-form",
      "aria-label": editing ? `Editar la regla ${editing.id}` : "Nueva regla",
      oninput: sync,
      onchange: sync,
      onsubmit: (event) => {
        event.preventDefault();
        const d = read();
        const conditions = draftConditions(d);
        if (editing) {
          void act(async () => {
            await api.updateRule(editing.id, conditions);
            closeRuleEditor();
          }, wasActive ? "Regla guardada; sigue activa y se aplicará a las tareas que se procesen desde ahora" : "Regla guardada; sigue inactiva");
        } else {
          void act(async () => {
            await api.createRule(d.id, d.kind, conditions);
            closeRuleEditor();
          }, "Regla creada inactiva; actívela cuando la haya comprobado");
        }
      },
    },
    h("h3", {}, editing ? `Editar la regla ${editing.id}` : "Nueva regla"),
    editing
      ? h(
          "p",
          { class: "meta" },
          `Identificador: ${editing.id} · Acción: ${ACTION_LABEL[editing.action_kind] ?? editing.action_kind}. Ninguno de los dos se puede cambiar; para otro tipo de regla, cree una nueva.`,
        )
      : h("div", { class: "field-group" }, field("Identificador", id, FIELD_HELP.id, "obligatorio"), field("Acción", kind, FIELD_HELP.action)),
    invoiceFields,
    emailFields,
    preview,
    editing
      ? h(
          "p",
          { class: wasActive ? "hint warn-text" : "hint" },
          wasActive
            ? "Esta regla está activa y seguirá activa al guardar. Se aplicará a las tareas que se procesen desde ahora, no a las que ya están en Pendientes."
            : "Esta regla está inactiva y seguirá inactiva al guardar; actívela desde la lista cuando la haya comprobado.",
        )
      : h("p", { class: "hint" }, "La regla se crea inactiva: no hará nada hasta que usted la active desde la lista."),
    h(
      "div",
      { class: "buttons" },
      h("button", { class: "primary", type: "submit" }, editing ? "Guardar cambios" : "Crear regla"),
      h("button", { type: "button", onclick: () => (closeRuleEditor(), render()) }, "Cancelar"),
    ),
  );
  if (editing) {
    // Los datos fijos no se editan.
    id.disabled = true;
    kind.disabled = true;
  }
  sync();
  return form;
}

function filteredRules(): Rule[] {
  const f = state.ruleFilter;
  const query = normalizeSearch(f.text.trim());
  return state.rules.filter(
    (r) =>
      (!f.action || r.action_kind === f.action) &&
      (!f.status || (f.status === "active") === r.active) &&
      (!f.category || r.conditions.category === f.category) &&
      (!query || searchText(r).includes(query)),
  );
}

function ruleRow(r: Rule): HTMLElement {
  const problem = ruleProblem(r);
  return h(
    "tr",
    {},
    h(
      "td",
      {},
      h("p", { class: "rule-sentence" }, ruleSentence(r)),
      h("p", { class: "rule-risk" }, "Aviso: ", ruleRisk(r)),
      problem ? h("p", { class: "rule-problem" }, problem) : null,
      h("p", { class: "meta" }, "Identificador: ", h("code", {}, r.id), ` · Versión ${r.version}`, helpTip(FIELD_HELP.column_version, "Ayuda sobre la versión")),
    ),
    h("td", {}, ACTION_LABEL[r.action_kind] ?? r.action_kind),
    h("td", {}, conditionsSummary(r)),
    h("td", {}, statusChip(r.active ? "Activa" : "Inactiva", r.active ? "ok" : "neutral")),
    h(
      "td",
      {},
      h(
        "div",
        { class: "row-actions" },
        h(
          "button",
          { "data-focus-key": `rule:${r.id}:toggle`, onclick: () => void act(() => api.setRuleActive(r.id, !r.active), r.active ? "Regla desactivada" : "Regla activada") },
          r.active ? "Desactivar" : "Activar",
        ),
        isKnownAction(r.action_kind) ? h("button", { "data-focus-key": `rule:${r.id}:edit`, onclick: () => openRuleEditor({ mode: "edit", id: r.id }, undefined) }, "Editar") : null,
        isKnownAction(r.action_kind)
          ? h("button", { "data-focus-key": `rule:${r.id}:dup`, onclick: () => openRuleEditor({ mode: "new" }, { ...draftOf(r), id: "" }) }, "Duplicar")
          : null,
      ),
    ),
  );
}

/** Lista con buscador y filtros; al escribir solo se rehace la lista, para conservar el foco. */
function rulesList(): HTMLElement {
  const box = h("div", { class: "rules-list" });
  const results = h("div", { "aria-live": "polite" });
  const fill = () => {
    const rows = filteredRules();
    const f = state.ruleFilter;
    const filtered = Boolean(f.text.trim() || f.action || f.status || f.category);
    results.replaceChildren(
      !state.rules.length
        ? emptyState("Todavía no hay reglas", "Todas las acciones requieren su aprobación en Pendientes. Cree una con «Nueva regla».")
        : !rows.length
          ? emptyState(
              "Ninguna regla coincide con la búsqueda y los filtros elegidos.",
              undefined,
              filtered ? h("button", { "data-focus-key": "rules:clear", onclick: clearRuleFilter }, "Quitar filtros") : null,
            )
          : h("p", { class: "meta" }, `${rows.length} de ${state.rules.length} reglas`),
      ...(rows.length
        ? [h("div", { class: "table-wrap" }, h(
            "table",
            { class: "grid rules-grid" },
            h(
              "thead",
              {},
              h(
                "tr",
                {},
                h("th", {}, "Regla ", helpTip(FIELD_HELP.column_rule, "Ayuda sobre la columna Regla")),
                h("th", {}, "Acción ", helpTip(FIELD_HELP.column_action, "Ayuda sobre la columna Acción")),
                h("th", {}, "Condiciones ", helpTip(FIELD_HELP.column_conditions, "Ayuda sobre la columna Condiciones")),
                h("th", {}, "Estado ", helpTip(FIELD_HELP.column_status, "Ayuda sobre la columna Estado")),
                h("th", {}, "Acciones ", helpTip(FIELD_HELP.column_actions, "Ayuda sobre la columna Acciones")),
              ),
            ),
            h("tbody", {}, ...rows.map(ruleRow)),
          ))]
        : []),
    );
  };
  const search = h("input", {
    type: "search",
    id: "rule-search",
    "data-focus-key": "rules:search",
    placeholder: "Proveedor, categoría o identificador",
    autocomplete: "off",
    oninput: () => ((state.ruleFilter.text = search.value), fill()),
  }) as HTMLInputElement;
  search.value = state.ruleFilter.text;
  const select = (name: keyof typeof state.ruleFilter, options: Array<[string, string]>) => {
    const el = h(
      "select",
      { "data-focus-key": `rules:filter:${name}`, onchange: () => ((state.ruleFilter[name] = el.value), fill()) },
      ...options.map(([value, label]) => h("option", { value }, label)),
    ) as HTMLSelectElement;
    el.value = state.ruleFilter[name];
    return el;
  };
  const action = select("action", [["", "Todas las acciones"], ["register_invoice", "Registrar factura"], ["send_draft", "Enviar respuesta"]]);
  const status = select("status", [["", "Activas e inactivas"], ["active", "Solo activas"], ["inactive", "Solo inactivas"]]);
  const category = select("category", [["", "Todas las categorías"], ...Object.entries(CATEGORY_LABEL)]);
  function clearRuleFilter() {
    state.ruleFilter = { text: "", action: "", status: "", category: "" };
    render();
  }
  box.append(
    h(
      "div",
      { class: "filters", role: "search" },
      h("div", { class: "field grow" }, h("div", { class: "field-head" }, h("label", { for: "rule-search" }, "Buscar"), helpTip(FIELD_HELP.search, "Ayuda sobre el buscador")), search),
      h("div", { class: "field" }, h("div", { class: "field-head" }, h("label", {}, "Acción", action), helpTip(FIELD_HELP.filter_action, "Ayuda sobre el filtro de acción"))),
      h("div", { class: "field" }, h("div", { class: "field-head" }, h("label", {}, "Estado", status), helpTip(FIELD_HELP.filter_status, "Ayuda sobre el filtro de estado"))),
      h("div", { class: "field" }, h("div", { class: "field-head" }, h("label", {}, "Categoría", category), helpTip(FIELD_HELP.filter_category, "Ayuda sobre el filtro de categoría"))),
    ),
    results,
  );
  fill();
  return box;
}

function categoriesPanel(): HTMLElement {
  const emails = state.tasks.filter((t) => t.kind === "email");
  const counts: Record<string, number> = {};
  let unclassified = 0;
  for (const t of emails) {
    const c = t.result?.category;
    if (typeof c === "string" && c) counts[c] = (counts[c] ?? 0) + 1;
    else unclassified += 1;
  }
  return h(
    "div",
    {},
    h(
      "p",
      { class: "intro" },
      "La IA clasifica cada correo recibido en una de estas cinco categorías; no se pueden añadir ni cambiar. Una regla de envío se aplica a los correos de la categoría que elija.",
    ),
    h("div", { class: "table-wrap" }, h(
      "table",
      { class: "grid" },
      h(
        "thead",
        {},
        h(
          "tr",
          {},
          h("th", {}, "Categoría ", helpTip(FIELD_HELP.category_name, "Ayuda sobre las categorías")),
          h("th", {}, "Qué significa"),
          h("th", {}, "Ejemplo inventado"),
          h("th", {}, "Correos ", helpTip(FIELD_HELP.category_count, "Ayuda sobre el recuento")),
          h("th", {}, "Reglas que la usan ", helpTip(FIELD_HELP.category_rules, "Ayuda sobre las reglas que la usan")),
        ),
      ),
      h(
        "tbody",
        {},
        ...Object.entries(CATEGORY_LABEL).map(([key, label]) => {
          const using = state.rules.filter((r) => r.action_kind === "send_draft" && r.conditions.category === key);
          const info = CATEGORY_HELP[key];
          return h(
            "tr",
            {},
            h("td", {}, h("strong", {}, label)),
            h("td", {}, info.description),
            h("td", {}, h("em", {}, info.example)),
            h("td", {}, String(counts[key] ?? 0)),
            h(
              "td",
              {},
              using.length
                ? h(
                    "span",
                    {},
                    `${using.length} ${using.length === 1 ? "regla" : "reglas"}: ${using.map((r) => r.id).join(", ")} `,
                    h("button", { class: "link", onclick: () => showRulesOf(key) }, "Ver reglas"),
                  )
                : "Ninguna",
            ),
          );
        }),
      ),
    )),
    h(
      "p",
      { class: "meta" },
      state.tasks.length
        ? `Recuento sobre los ${emails.length} correos que constan en este equipo, incluidos los ya tratados${unclassified ? `; ${unclassified} sin categoría (aún sin clasificar o con error)` : ""}.`
        : "Todavía no consta ningún correo en este equipo: el recuento es cero.",
    ),
  );
}

function rulesView() {
  const panel =
    state.rulesTab === "mine"
      ? h(
          "div",
          {},
          h(
            "p",
            { class: "intro" },
            "Por defecto todo se revisa. Una regla activa permite enviar o registrar sin revisión solo cuando es la única que coincide y los datos no tienen incidencias. Cada cambio crea una versión nueva y queda en el historial.",
          ),
          state.ruleEditor
            ? ruleForm()
            : sectionHeader(
                "Reglas guardadas",
                h("button", { class: "primary", "data-focus-key": "rules:new", onclick: () => openRuleEditor({ mode: "new" }, undefined) }, "Nueva regla"),
              ),
          rulesList(),
        )
      : categoriesPanel();
  return h(
    "section",
    { class: "page" },
    h("h2", {}, "Reglas"),
    tabs({
      label: "Secciones de reglas",
      items: [
        { key: "mine", label: "Mis reglas" },
        { key: "categories", label: "Categorías" },
      ],
      active: state.rulesTab,
      panelId: "rules-panel",
      onSelect: setRulesTab,
    }),
    h("div", { role: "tabpanel", id: "rules-panel", class: "tabpanel", "aria-label": state.rulesTab === "mine" ? "Mis reglas" : "Categorías" }, panel),
  );
}

function historyView() {
  return h(
    "section",
    { class: "page" },
    h("h2", {}, "Historial"),
    h("p", { class: "intro" }, "Acciones ejecutadas en las cuentas de correo y el simulador de administración, y cambios de reglas, con su autorización."),
    state.audit.length
      ? h(
          "table",
          { class: "grid" },
          h("thead", {}, h("tr", {}, ...["Tarea", "Acción", "Resultado", "Autorización", "Detalle"].map((c) => h("th", {}, c)))),
          h(
            "tbody",
            {},
            ...[...state.audit].reverse().map((e) =>
              h(
                "tr",
                {},
                h("td", {}, e.task_id ?? "—"),
                h("td", {}, ACTION_LABEL[e.action] ?? e.action),
                h("td", {}, e.status),
                h("td", {}, e.authorization ?? "—"),
                h("td", {}, e.detail ?? ""),
              ),
            ),
          ),
        )
      : h("p", { class: "empty" }, "Aún no hay acciones registradas."),
  );
}

// ---- Cuentas de correo (R13) ----

function accountId(messageId: string | undefined): string {
  return messageId?.includes(":") ? messageId.split(":")[0]! : "simulador";
}

// Origen de un correo: el guardado en la tarea (D48) o, en tareas anteriores, el de la cuenta
// actual. El simulador es siempre una cuenta simulada, nunca una cuenta «desconectada».
function mailOrigin(saved: MailOrigin | null | undefined, messageId: string | undefined): MailOrigin | null {
  // El nombre actual de la cuenta prevalece (se puede renombrar); el guardado sirve si ya no está.
  const account = state.mail?.accounts.find((a) => a.id === (saved?.account ?? accountId(messageId)));
  if (account) return { account: account.id, provider: account.provider, label: account.label, address: account.address };
  if (saved) return saved;
  if (accountId(messageId) === "simulador")
    return { account: "simulador", provider: "simulator", label: "Correo simulado", address: "administracion@nestadmin.test" };
  return null;
}

const PROVIDER_SHORT: Record<MailProviderKind, string> = {
  simulator: "cuenta simulada",
  google: "Gmail",
  microsoft: "Outlook",
};

function accountShort(origin: MailOrigin | null, id: string): string {
  if (!origin) return `${id} (ya no está en NestAdmin)`;
  return origin.address ? `${origin.label} · ${origin.address}` : origin.label;
}

// «Oficina · admin@empresa.test (Gmail)»; la dirección que falta se dice, no se inventa.
function accountName(messageId: string | undefined, saved?: MailOrigin | null): string {
  const origin = mailOrigin(saved, messageId);
  if (!origin) return `${accountId(messageId)} (cuenta quitada de NestAdmin)`;
  const connected = state.mail?.accounts.some((a) => a.id === origin.account) ?? true;
  return `${origin.label} · ${origin.address ?? "dirección no consta"} (${PROVIDER_SHORT[origin.provider]}${connected ? "" : "; ya no está en NestAdmin"})`;
}

function accountLine(saved: MailOrigin | null | undefined, messageId: string | undefined) {
  const origin = mailOrigin(saved, messageId);
  if (!origin) return h("span", {}, `${accountId(messageId)} `, h("span", { class: "missing" }, "(cuenta quitada de NestAdmin)"));
  const connected = state.mail?.accounts.some((a) => a.id === origin.account) ?? true;
  return h(
    "span",
    {},
    h("strong", {}, origin.label),
    " · ",
    origin.address ?? missing(),
    ` (${PROVIDER_LABEL[origin.provider]}${origin.provider === "simulator" ? ", datos sintéticos" : ""})`,
    connected ? null : h("span", { class: "missing" }, " · ya no está en NestAdmin"),
  );
}

function taskMailOrigin(task: Task): MailOrigin | null {
  const input = safeJson(task.input_ref);
  if (task.kind === "invoice") {
    const origin: DocumentOrigin | undefined = task.result?.origin ?? input.origin;
    return origin?.kind === "email" ? mailOrigin(origin.account, `${origin.account.account}:`) : null;
  }
  const messageId = task.result?.message?.id ?? input.message_id;
  return mailOrigin(task.result?.origin ?? input.origin, messageId);
}

// Clave del filtro por origen: "file" para archivos importados o el ID de la cuenta.
function originKey(task: Task): string {
  const input = safeJson(task.input_ref);
  if (task.kind === "invoice") {
    const origin: DocumentOrigin | undefined = task.result?.origin ?? input.origin;
    return origin?.kind === "email" ? origin.account.account : "file";
  }
  return accountId(task.result?.message?.id ?? input.message_id);
}

function dateText(value: string | number): string {
  const date = typeof value === "number" ? new Date(value * 1000) : new Date(value);
  if (Number.isNaN(date.getTime())) return String(value);
  // Una fecha sin hora (AAAA-MM-DD) se muestra sin hora para no inventarla.
  if (typeof value === "string" && /^\d{4}-\d{2}-\d{2}$/.test(value)) return date.toLocaleDateString("es-ES", { timeZone: "UTC" });
  return date.toLocaleString("es-ES", { dateStyle: "medium", timeStyle: "short" });
}

function fileSize(bytes: number) {
  return bytes < 1024 ? `${bytes} B` : bytes < 1024 ** 2 ? `${(bytes / 1024).toFixed(1).replace(".", ",")} KB` : `${(bytes / 1024 ** 2).toFixed(1).replace(".", ",")} MB`;
}

// Origen de un documento (D49): archivo importado o adjunto de un correo, con su huella.
function documentOrigin(origin: DocumentOrigin | null) {
  const rows: Array<[string, Child]> = [];
  if (!origin) {
    rows.push(["Origen", h("span", { class: "missing" }, "No consta (documento importado antes de registrar el origen)")]);
  } else if (origin.kind === "file") {
    rows.push(["Origen", "Archivo importado"]);
    rows.push(["Archivo", origin.name]);
    rows.push(["Ruta original", origin.original_path ? h("span", { class: "mono" }, origin.original_path) : missing()]);
  } else {
    rows.push(["Origen", "Adjunto de un correo"]);
    rows.push(["Cuenta", accountLine(origin.account, `${origin.account.account}:`)]);
    rows.push(["Remitente", origin.from ?? missing()]);
    rows.push(["Asunto", origin.subject ?? missing()]);
    rows.push(["Adjunto", origin.attachment ?? missing()]);
  }
  if (origin) {
    rows.push(["Importado", origin.imported_at ? dateText(origin.imported_at) : missing()]);
    rows.push([
      "SHA-256",
      h("span", { class: "mono", title: origin.sha256 }, `${origin.sha256.slice(0, 12)}…`, ` · ${fileSize(origin.size)}`),
    ]);
  }
  return h(
    "div",
    { class: "origin-box" },
    h("dl", { class: "origin" }, ...rows.flatMap(([label, value]) => [h("dt", {}, label), h("dd", {}, value)])),
    h("p", { class: "meta" }, "«Abrir original» abre la copia guardada en NestAdmin con el visor del sistema."),
  );
}

// Procedencia de un campo: páginas y línea del texto en que aparece. Si el valor no aparece
// literalmente (el modelo lo reescribió o el OCR lo leyó distinto), se dice así.
function fieldSource(value: string | null | undefined, source: FieldSource | undefined, pageCount: number) {
  if (value == null) return "";
  if (!source) return h("span", { class: "missing" }, "No localizado en el texto; compruébelo en el original");
  const where = pageCount > 1 || source.pages.some((p) => p > 1) ? `pág. ${source.pages.join(", ")}` : "en el texto";
  return h("span", {}, h("strong", {}, where), " · ", h("q", {}, source.line));
}

function needsAttention(account: MailAccount) {
  return account.status !== "connected" || account.last_fetch?.error != null;
}

function issueText(error: { code: string; message: string }) {
  return `${MAIL_ERROR[error.code] ?? error.code}: ${error.message}`;
}

function fetchNotice(report: FetchReport): { kind: "ok" | "error"; text: string } {
  if (report.accounts.length === 0) return { kind: "error", text: "No hay cuentas de correo. Añada una en la sección Correo." };
  const failed = report.accounts.filter((a) => a.error);
  const imported = report.accounts.reduce((sum, a) => sum + (a.attachments?.imported ?? 0), 0);
  const skipped = report.accounts.flatMap((a) => a.attachments?.skipped ?? []);
  const attachmentErrors = report.accounts.flatMap((a) => a.attachments?.failed ?? []);
  let added = report.added ? `${report.added} mensajes nuevos en cola` : "No hay mensajes nuevos";
  if (imported) added += `; ${imported} adjuntos importados como documentos`;
  if (skipped.length) added += `; adjuntos no procesables omitidos: ${skipped.join(", ")}`;
  const problems: string[] = failed.map(
    (a) => `no se pudo consultar ${state.mail?.accounts.find((m) => m.id === a.account)?.label ?? a.account} — ${issueText(a.error!)}`,
  );
  if (attachmentErrors.length) {
    problems.push(
      `adjuntos sin importar (se reintentarán en la próxima consulta): ${attachmentErrors.map((e) => `${e.name} — ${issueText(e)}`).join("; ")}`,
    );
  }
  if (!problems.length) return { kind: "ok", text: added };
  return { kind: "error", text: `${added}. ${problems.join(". ")}` };
}

function lastFetch(account: MailAccount) {
  const f = account.last_fetch;
  if (!f) return h("span", { class: "missing" }, "Sin consultar en esta sesión");
  if (f.error) return h("span", { class: "error-text" }, issueText(f.error));
  const a = f.attachments;
  const parts = [`${f.read} leídos, ${f.added} nuevos`];
  if (a?.imported) parts.push(`${a.imported} adjuntos importados`);
  if (a?.failed.length) return h("span", { class: "error-text" }, `${parts.join(", ")}; ${a.failed.length} adjuntos sin importar`);
  return parts.join(", ");
}

function signIn(provider: MailProviderKind, label: string, account?: MailAccount) {
  state.signInUrl = null;
  if (state.mail) state.mail.signing_in = true;
  state.notice = { kind: "ok", text: "Complete el inicio de sesión en el navegador que se ha abierto." };
  render();
  void act(async () => {
    try {
      const done = await api.mailSignIn(provider, label, account?.id);
      state.notice = { kind: "ok", text: account ? `Sesión renovada en «${done.label}»` : `Cuenta «${done.label}» conectada` };
    } finally {
      state.signInUrl = null;
    }
  });
}

function mailView() {
  const m = state.mail;
  if (!m) return h("section", { class: "page" }, h("p", {}, "Cargando…"));
  const label = h("input", { name: "label", maxlength: "60", placeholder: "p. ej. Facturación" }) as HTMLInputElement;
  const limit = h("input", { type: "number", min: "1", max: "500", step: "1", value: String(m.limit) }) as HTMLInputElement;
  const busy = m.signing_in;
  const providerButton = (provider: "google" | "microsoft", configured: boolean) =>
    h(
      "button",
      {
        class: "primary",
        disabled: busy || !configured,
        title: configured ? undefined : "El editor no ha configurado este proveedor en esta instalación",
        onclick: () => signIn(provider, label.value),
      },
      `Conectar ${PROVIDER_LABEL[provider]}`,
    );
  const unconfigured = [
    !m.google_configured ? "Google" : null,
    !m.microsoft_configured ? "Microsoft" : null,
  ].filter(Boolean);
  return h(
    "section",
    { class: "page" },
    h("h2", {}, "Cuentas de correo"),
    h(
      "p",
      { class: "intro" },
      "Cada cuenta inicia sesión en el navegador del sistema y sus tokens se guardan en el Administrador de credenciales de Windows. El correo solo viaja entre este equipo y el proveedor de la cuenta. Ninguna respuesta se envía sin su aprobación o una regla activa.",
    ),
    m.clients_error ? h("div", { class: "notice error", role: "alert" }, h("span", {}, m.clients_error)) : null,
    m.accounts.length
      ? h(
          "table",
          { class: "grid" },
          h("thead", {}, h("tr", {}, ...["Cuenta", "Proveedor", "Estado", "Última consulta", ""].map((c) => h("th", {}, c)))),
          h(
            "tbody",
            {},
            ...m.accounts.map((a) => {
              const [statusText, statusClass] = ACCOUNT_STATUS[a.status] ?? [a.status, "state-failed"];
              const real = a.provider !== "simulator";
              const configured = a.provider === "google" ? m.google_configured : m.microsoft_configured;
              const renew = real && configured && (a.status !== "connected" || a.last_fetch?.error?.code === "unauthorized");
              return h(
                "tr",
                {},
                h(
                  "td",
                  {},
                  h("strong", {}, a.label),
                  " ",
                  h(
                    "button",
                    {
                      class: "link",
                      title: "Cambiar el nombre con que se muestra esta cuenta",
                      onclick: () => {
                        const label = prompt(
                          `Nombre de la cuenta ${a.address ?? a.id}.
Déjelo vacío para volver al predeterminado.`,
                          a.label,
                        );
                        if (label !== null) void act(() => api.renameMailAccount(a.id, label), "Nombre de la cuenta guardado");
                      },
                    },
                    "Renombrar",
                  ),
                  h("div", { class: "meta" }, a.address ?? h("span", { class: "missing" }, "dirección no consta")),
                  h("div", { class: "meta mono" }, a.id),
                ),
                h("td", {}, PROVIDER_LABEL[a.provider]),
                h("td", {}, h("span", { class: `badge ${statusClass}` }, statusText)),
                h("td", { class: "detail-cell" }, lastFetch(a)),
                h(
                  "td",
                  {},
                  renew
                    ? h("button", { disabled: busy, onclick: () => signIn(a.provider, a.label, a) }, "Volver a iniciar sesión")
                    : null,
                  h(
                    "button",
                    {
                      class: "danger",
                      onclick: () => {
                        const what = real
                          ? `Se borrarán los tokens de «${a.label}» de este equipo y NestAdmin dejará de leer esa cuenta. Las tareas ya creadas seguirán visibles, pero no podrán enviar respuestas.`
                          : "NestAdmin dejará de leer el correo simulado. Puede volver a añadirlo.";
                        if (confirm(`${what} ¿Desconectar?`)) void act(() => api.removeMailAccount(a.id), `Cuenta «${a.label}» desconectada`);
                      },
                    },
                    "Desconectar",
                  ),
                ),
              );
            }),
          ),
        )
      : h("p", { class: "empty" }, "No hay cuentas de correo. Conecte una cuenta de Google o Microsoft para consultar su bandeja de entrada."),
    h(
      "div",
      { class: "card" },
      h("h3", {}, "Añadir cuenta"),
      h(
        "p",
        { class: "intro" },
        "Se abrirá el navegador para que inicie sesión y conceda permiso de lectura, borradores y envío. NestAdmin espera la respuesta en este equipo (dirección de bucle local: localhost o 127.0.0.1) durante 5 minutos.",
      ),
      h("div", { class: "row" }, h("label", {}, "Nombre de la cuenta (opcional)", label)),
      h(
        "div",
        { class: "buttons" },
        providerButton("google", m.google_configured),
        providerButton("microsoft", m.microsoft_configured),
        m.accounts.some((a) => a.provider === "simulator")
          ? null
          : h("button", { disabled: busy, onclick: () => void act(() => api.addSimulatedMailAccount(), "Correo simulado añadido") }, "Añadir correo simulado"),
      ),
      busy
        ? h(
            "div",
            { class: "notice ok", role: "status" },
            h(
              "span",
              {},
              "Esperando al navegador… ",
              state.signInUrl ? h("span", {}, "Si no se abrió, copie esta dirección en su navegador: ", h("span", { class: "mono" }, state.signInUrl)) : null,
            ),
            h("button", { onclick: () => void act(() => api.mailSignInCancel()) }, "Cancelar"),
          )
        : null,
      unconfigured.length
        ? h(
            "p",
            { class: "meta" },
            `${unconfigured.join(" y ")}: esta instalación no incluye el cliente OAuth del editor (archivo correo-oauth.json), así que no se puede iniciar sesión.`,
          )
        : null,
    ),
    h(
      "div",
      { class: "card" },
      h("h3", {}, "Mensajes por consulta"),
      h("p", { class: "intro" }, "Cada vez que pulse «Consultar correo», NestAdmin lee como mucho este número de mensajes recientes de cada cuenta (de 1 a 500)."),
      h("div", { class: "row" }, h("label", {}, "Límite por cuenta", limit)),
      h(
        "button",
        {
          onclick: () => void act(() => api.setMailLimit(Number(limit.value)), `Límite guardado: ${limit.value} mensajes por cuenta`),
        },
        "Guardar límite",
      ),
    ),
  );
}

// ---- Proveedor de IA (D100–D102, D122) ----

type Kind = ProviderChoice["provider"];

/// Orden de la lista: el externo por defecto primero y la IA local la última.
const KIND_ORDER: Kind[] = ["ollama_cloud", "openai_compatible", "anthropic", "ollama"];
const KIND_NAME: Record<Kind, string> = {
  ollama_cloud: "Ollama cloud",
  openai_compatible: "Compatible con OpenAI",
  anthropic: "Claude · Anthropic",
  ollama: "IA local (Ollama)",
};
const KIND_NOTE: Record<Kind, string> = {
  ollama_cloud: "externo, por defecto",
  openai_compatible: "externo; solo https",
  anthropic: "externo",
  ollama: "no sale nada del equipo",
};
const LOCAL_URL = "http://127.0.0.1:11434";

/// Estado de cada bloque de proveedor. Vive fuera de la vista para que un repintado no borre lo
/// escrito; la clave tecleada solo está aquí hasta que el proveedor responde y se guarda.
interface Block {
  key: string;
  url: string;
  /// Modelo elegido en la lista ("" si no hay ninguno elegido).
  model: string;
  models: string[] | null;
  loading: boolean;
  listError: string | null;
  /// Hay clave guardada para este proveedor (y host); `null` si aún no se ha consultado.
  saved: boolean | null;
  consent: boolean;
  busy: boolean;
  outcome: { kind: "ok" | "error"; text: string } | null;
  opened: boolean;
}

interface BlockOptions {
  /// Carga la lista al abrir si ya hay clave guardada (por defecto, solo el proveedor activo).
  auto?: boolean;
  /// Se llama cuando el proveedor quedó elegido y la prueba respondió bien.
  onDone?: () => void;
}

const blocks: Partial<Record<Kind, Block>> = {};

function resetBlocks() {
  for (const kind of KIND_ORDER) delete blocks[kind];
}

function block(kind: Kind): Block {
  let b = blocks[kind];
  if (!b) {
    const c = state.settings?.choice;
    const same = c && c.provider === kind ? c : null;
    b = {
      key: "",
      url: same && "base_url" in same ? same.base_url : kind === "ollama" ? LOCAL_URL : "",
      model: "",
      models: null,
      loading: false,
      listError: null,
      saved: null,
      consent: false,
      busy: false,
      outcome: null,
      opened: false,
    };
    blocks[kind] = b;
  }
  return b;
}

/// Modelo guardado de este proveedor, si es el elegido actualmente.
function savedModel(kind: Kind): string | null {
  const c = state.settings?.choice;
  return c && c.provider === kind ? c.model : null;
}

function choiceFor(kind: Kind, model: string): ProviderChoice {
  const b = block(kind);
  switch (kind) {
    case "ollama_cloud":
      return { provider: kind, model };
    case "anthropic":
      return { provider: kind, model };
    case "openai_compatible":
      return { provider: kind, base_url: b.url.trim(), model };
    case "ollama":
      return { provider: kind, base_url: b.url.trim() || LOCAL_URL, model };
  }
}

/// Problema visible de la URL base del proveedor compatible; el adaptador valida el resto.
function compatProblem(b: Block): string | null {
  const url = b.url.trim();
  if (!url) return "Indique la URL base del servicio.";
  if (!/^https:\/\//i.test(url)) return "Solo se admite una URL https://.";
  return null;
}

const LEAVES =
  "El contenido que la tarea necesita (texto extraído de documentos; asunto, remitente, destinatarios, fecha y cuerpo de correos) sale de este equipo hacia";
const STAYS = "No salen la base de datos, otras claves ni el archivo original.";

function destinationName(kind: Kind, b: Block): string {
  switch (kind) {
    case "ollama_cloud":
      return "Ollama Inc. (ollama.com, principalmente en EE. UU.)";
    case "anthropic":
      return "Anthropic (api.anthropic.com)";
    case "openai_compatible":
      return `${hostOf(b.url)} (el servicio que usted ha indicado)`;
    case "ollama":
      return "este equipo";
  }
}

function destinationText(kind: Kind, b: Block): string {
  return `${LEAVES} ${destinationName(kind, b)}. ${STAYS}`;
}

/// El destino de esta elección ya está confirmado (solo se sabe del proveedor guardado).
function alreadyConsented(kind: Kind, b: Block): boolean {
  const s = state.settings;
  const c = s?.choice;
  if (!s || !c || c.provider !== kind || !s.consented) return false;
  return c.provider !== "openai_compatible" || hostOf(c.base_url) === hostOf(b.url);
}

function goToSettings() {
  state.view = "settings";
  state.notice = null;
  resetBlocks();
  render();
}

function goInstallLocal() {
  state.wizard = false;
  state.view = "setup";
  state.notice = null;
  render();
}

async function openBlock(kind: Kind, opts: BlockOptions) {
  const b = block(kind);
  if (kind === "ollama") return void (await loadModels(kind, null));
  try {
    b.saved = await api.providerKeySaved(choiceFor(kind, ""));
  } catch {
    b.saved = false;
  }
  const auto = opts.auto ?? state.settings?.choice?.provider === kind;
  if (b.saved && auto && !(kind === "openai_compatible" && compatProblem(b))) await loadModels(kind, null);
  else render();
}

/// Pide la lista de modelos (D122). Con una clave tecleada, el backend la guarda solo si la
/// lista responde; si falla, la clave sigue en el campo y no se guarda nada.
async function loadModels(kind: Kind, typed: string | null) {
  const b = block(kind);
  if (b.loading) return;
  b.loading = true;
  b.listError = null;
  b.outcome = null;
  render();
  try {
    const models = await api.listModels(choiceFor(kind, b.model || savedModel(kind) || ""), typed);
    b.models = models;
    if (typed) {
      b.saved = true;
      b.key = "";
    }
    const saved = savedModel(kind);
    if (!b.model || !models.includes(b.model)) {
      if (saved) b.model = models.includes(saved) ? saved : "";
      else b.model = models[0] ?? "";
    }
    if (typed) await refresh(true);
  } catch (error) {
    b.models = null;
    b.model = "";
    b.listError = String(error);
  }
  b.loading = false;
  render();
}

async function useProvider(kind: Kind, opts: BlockOptions) {
  const b = block(kind);
  const choice = choiceFor(kind, b.model);
  b.busy = true;
  b.outcome = null;
  render();
  try {
    await api.selectProvider(choice, alreadyConsented(kind, b) || b.consent);
  } catch (error) {
    b.busy = false;
    b.outcome = { kind: "error", text: `No se ha cambiado de proveedor: ${error}` };
    render();
    return;
  }
  b.consent = false;
  await refresh(true);
  let done = false;
  try {
    await api.testProvider(choice);
    b.outcome = { kind: "ok", text: "Proveedor listo: respondió correctamente" };
    done = true;
  } catch (error) {
    b.outcome = {
      kind: "error",
      text: `La prueba del proveedor falló: ${error}. La selección se ha mantenido; compruebe la clave y el modelo, o elija otro proveedor.`,
    };
  }
  b.busy = false;
  render();
  if (done) opts.onDone?.();
}

async function deleteKey(kind: Kind) {
  const b = block(kind);
  try {
    await api.deleteProviderKey(choiceFor(kind, b.model || savedModel(kind) || ""));
    b.saved = false;
    b.models = null;
    b.model = "";
    b.key = "";
    b.outcome = { kind: "ok", text: "Clave eliminada" };
  } catch (error) {
    b.outcome = { kind: "error", text: String(error) };
  }
  await refresh(true);
}

/// Cambió la URL base: la lista y la clave dependen del host, así que se vuelven a consultar.
async function urlChanged(kind: Kind, opts: BlockOptions) {
  const b = block(kind);
  b.models = null;
  b.model = "";
  b.listError = null;
  b.outcome = null;
  b.saved = null;
  b.consent = false;
  if (kind === "openai_compatible" && compatProblem(b)) {
    b.saved = false;
    return render();
  }
  render();
  await openBlock(kind, opts);
}

/// Bloque de un proveedor: clave, lista de modelos, destino de los datos y «Usar este proveedor».
/// Lo usan Configuración y el asistente de primer uso.
function providerBlock(kind: Kind, opts: BlockOptions = {}): HTMLElement {
  const b = block(kind);
  const external = kind !== "ollama";
  if (!b.opened) {
    b.opened = true;
    queueMicrotask(() => void openBlock(kind, opts));
  }
  const problem = kind === "openai_compatible" ? compatProblem(b) : null;
  const consented = !external || alreadyConsented(kind, b);
  const hasList = !!b.models && b.models.length > 0;
  const saved = savedModel(kind);
  const missing = hasList && saved !== null && !b.models!.includes(saved) ? saved : null;
  const ready = hasList && b.models!.includes(b.model) && (consented || b.consent) && !b.busy && !b.loading && !problem;

  const urlField =
    kind === "openai_compatible" || kind === "ollama"
      ? h(
          "label",
          {},
          kind === "ollama" ? "Dirección local" : "URL base",
          h("input", {
            value: b.url,
            placeholder: kind === "ollama" ? LOCAL_URL : "https://api.ejemplo.com/v1",
            autocomplete: "off",
            oninput: (event) => (b.url = (event.target as HTMLInputElement).value),
            onchange: () => void urlChanged(kind, opts),
          }),
        )
      : null;
  const hostLine =
    kind === "openai_compatible"
      ? h(
          "p",
          { class: problem ? "error-text" : "intro" },
          problem ??
            `La clave queda ligada al host ${hostOf(b.url)}: con otro host no se envía. ${b.saved ? "Hay una clave guardada para este host." : "No hay clave guardada para este host."}`,
        )
      : null;
  const checkDisabled = () => b.loading || b.busy || !!problem || (!b.key.trim() && !b.saved);
  const check = h(
    "button",
    {
      disabled: checkDisabled(),
      "aria-busy": b.loading ? "true" : undefined,
      onclick: () => void loadModels(kind, b.key.trim() || null),
    },
    b.loading ? "Comprobando…" : "Comprobar y ver modelos",
  ) as HTMLButtonElement;
  const keyField = external
    ? h(
        "div",
        { class: "row" },
        h(
          "label",
          {},
          "Clave",
          h("input", {
            type: "password",
            autocomplete: "off",
            value: b.key,
            placeholder: b.saved ? "Clave guardada" : "Pegue aquí la clave",
            // Sin repintar: el botón se habilita al teclear, sin perder el foco.
            oninput: (event) => {
              b.key = (event.target as HTMLInputElement).value;
              check.disabled = checkDisabled();
            },
          }),
        ),
        check,
      )
    : null;
  const keyNote = external
    ? h("p", { class: "intro" }, "La clave se guarda en el Administrador de credenciales de Windows, solo si el proveedor responde. NestAdmin no la muestra ni la escribe en registros.")
    : null;

  const model = h(
    "label",
    {},
    "Modelo",
    h(
      "select",
      {
        "aria-label": "Modelo",
        disabled: !hasList || b.loading || b.busy,
        onchange: (event) => {
          b.model = (event.target as HTMLSelectElement).value;
          render();
        },
      },
      hasList ? (b.model ? null : h("option", { value: "" }, "Elija un modelo")) : h("option", { value: "" }, b.loading ? "Cargando la lista…" : "Sin lista de modelos"),
      ...(b.models ?? []).map((name) => h("option", { value: name, selected: name === b.model }, name)),
    ),
  );
  const modelNotes: Child[] = [];
  if (missing) {
    modelNotes.push(
      h("p", { class: "meta warn", role: "status" }, `El modelo guardado ${missing} ya no aparece en la lista. No se ha cambiado: elija otro de la lista.`),
    );
  } else if (!hasList && saved) {
    modelNotes.push(h("p", { class: "meta" }, `Modelo guardado: ${saved}. No se puede cambiar hasta que la lista responda.`));
  }
  if (b.listError) modelNotes.push(h("p", { class: "error-text", role: "alert" }, `No se pudo obtener la lista de modelos: ${b.listError}`));
  if (kind === "ollama" && !b.loading && (b.listError || (b.models && b.models.length === 0))) {
    modelNotes.push(
      h(
        "p",
        {},
        b.listError ? "Ollama no está disponible en esta dirección. " : "Ollama no tiene ningún modelo instalado. ",
        h("button", { class: "primary", onclick: goInstallLocal }, "Instalar IA local"),
      ),
    );
  }

  let consent: Child = null;
  if (external) {
    consent = consented
      ? h("p", { class: "meta" }, `Destino de los datos confirmado: ${destinationName(kind, b)}.`)
      : h(
          "label",
          { class: "choice consent" },
          h("input", {
            type: "checkbox",
            checked: b.consent,
            onchange: (event) => {
              b.consent = (event.target as HTMLInputElement).checked;
              render();
            },
          }),
          h("span", {}, destinationText(kind, b)),
        );
  }

  return h(
    "div",
    { class: "provider-block" },
    urlField ? h("div", { class: "row" }, urlField) : null,
    hostLine,
    keyField,
    keyNote,
    h("div", { class: "row" }, model),
    ...modelNotes,
    consent,
    h(
      "div",
      { class: "buttons" },
      h(
        "button",
        { class: "primary", disabled: !ready, "aria-busy": b.busy ? "true" : undefined, onclick: () => void useProvider(kind, opts) },
        b.busy ? "Probando el proveedor…" : "Usar este proveedor",
      ),
      external && b.saved ? h("button", { class: "danger", disabled: b.busy, onclick: () => void deleteKey(kind) }, "Eliminar clave") : null,
    ),
    b.outcome ? h("div", { class: `notice ${b.outcome.kind}`, role: b.outcome.kind === "error" ? "alert" : "status" }, h("span", {}, b.outcome.text)) : null,
  );
}

function fallbackStateText(s: SettingsView): string {
  if (!s.choice || s.choice.provider === "ollama") return "El respaldo solo se aplica con un proveedor externo.";
  if (s.fallback_period_active) return "Activo durante 10 minutos porque el proveedor no responde: las tareas van directamente a la IA local.";
  return s.local_fallback ? "Activado." : "Desactivado.";
}

function settingsView() {
  const s = state.settings;
  if (!s) return h("section", { class: "page" }, h("p", {}, "Cargando…"));
  const pick: Kind = state.pick ?? s.choice?.provider ?? "ollama_cloud";
  const externalChosen = !!s.choice && s.choice.provider !== "ollama";
  return h(
    "section",
    { class: "page" },
    h("h2", {}, "Configuración"),
    h(
      "div",
      { class: "card" },
      h("h3", {}, "Proveedor activo"),
      h("p", {}, h("strong", {}, s.choice ? providerLabel(s.active.provider, s.active.model) : "Ninguno elegido")),
      s.ai_wait_message ? h("div", { class: "notice warn", role: "status" }, h("span", {}, s.ai_wait_message)) : null,
      h(
        "label",
        { class: "choice" },
        h("input", {
          type: "checkbox",
          checked: s.local_fallback,
          disabled: !externalChosen,
          onchange: (event) => {
            const enabled = (event.target as HTMLInputElement).checked;
            void act(() => api.setLocalFallback(enabled), enabled ? "Respaldo con la IA local activado" : "Respaldo con la IA local desactivado");
          },
        }),
        h("span", {}, h("strong", {}, "Respaldo con la IA local"), " — ", fallbackStateText(s)),
      ),
      h(
        "p",
        { class: "intro" },
        "Si el proveedor externo elegido no responde, agota el tiempo o alcanza su límite, la tarea se repite una sola vez con la IA local, siempre que esté preparada. Nunca se pasa a otro servicio externo ni se descarga nada para ello.",
      ),
    ),
    s.offer_external ? externalOffer() : null,
    h(
      "div",
      { class: "card" },
      h("h3", {}, "Proveedor de IA"),
      h("p", { class: "intro" }, "Elija el proveedor. Solo se envía contenido a un proveedor externo si usted confirma su destino. OCR y extracción de texto se hacen siempre en este equipo."),
      ...KIND_ORDER.map((kind) =>
        h(
          "div",
          { class: pick === kind ? "provider-choice open" : "provider-choice" },
          h(
            "label",
            { class: "choice" },
            h("input", {
              type: "radio",
              name: "provider",
              value: kind,
              checked: pick === kind,
              onchange: () => {
                state.pick = kind;
                render();
              },
            }),
            h("span", {}, h("strong", {}, KIND_NAME[kind]), ` (${KIND_NOTE[kind]})`),
          ),
          pick === kind ? h("div", { class: "indent" }, providerBlock(kind)) : null,
        ),
      ),
    ),
    h("div", { class: "card" }, h("h3", {}, "Datos locales"), h("p", { class: "mono" }, s.data_dir)),
  );
}

/// Oferta única (D100) a quien tenía la IA local: pasar a Ollama cloud. No cambia nada hasta que
/// la persona termina el bloque (clave, modelo, destino y prueba).
function externalOffer() {
  return h(
    "div",
    { class: "banner", role: "status" },
    h(
      "span",
      {},
      "NestAdmin ahora puede trabajar con Ollama cloud, un proveedor externo que no necesita preparar nada en este equipo. Seguirá usando la IA local y no cambiará nada hasta que usted termine de configurarlo y confirme el destino de los datos.",
    ),
    h(
      "div",
      { class: "buttons" },
      h(
        "button",
        {
          class: "primary",
          onclick: () => {
            state.pick = "ollama_cloud";
            void act(() => api.dismissExternalOffer());
          },
        },
        "Pasar a Ollama cloud",
      ),
      h("button", { onclick: () => void act(() => api.dismissExternalOffer()) }, "No, gracias"),
    ),
  );
}

// ---- Asistente de primer uso (D100, D101, D103) ----

function closeWizard(notice?: string) {
  state.wizard = false;
  state.wizardStep = "choose";
  state.view = "review";
  state.notice = notice ? { kind: "ok", text: notice } : null;
  resetBlocks();
  void refresh();
}

function wizardView() {
  const later = h("button", { onclick: () => closeWizard() }, "Ahora no");
  const done = () => closeWizard("Proveedor listo: la IA responde y las tareas en espera se reanudan");
  const head = h(
    "div",
    {},
    h("h2", {}, "Configure la IA de NestAdmin"),
    h(
      "p",
      { class: "intro" },
      "NestAdmin necesita un proveedor de IA para leer facturas y preparar borradores de correo. Hasta que lo configure, las tareas esperan en la cola con el motivo a la vista; puede importar documentos y consultar el correo con normalidad. El OCR y la extracción de texto se hacen siempre en este equipo.",
    ),
  );
  if (state.wizardStep === "cloud") {
    return h(
      "section",
      { class: "page wizard" },
      head,
      h("h3", {}, "Ollama cloud"),
      providerBlock("ollama_cloud", { auto: true, onDone: done }),
      h("div", { class: "buttons" }, h("button", { onclick: () => ((state.wizardStep = "choose"), render()) }, "Volver"), later),
    );
  }
  if (state.wizardStep === "other") {
    const kinds: Array<"openai_compatible" | "anthropic"> = ["openai_compatible", "anthropic"];
    return h(
      "section",
      { class: "page wizard" },
      head,
      h("h3", {}, "Otro proveedor externo"),
      ...kinds.map((kind) =>
        h(
          "div",
          { class: "provider-choice" },
          h(
            "label",
            { class: "choice" },
            h("input", {
              type: "radio",
              name: "wizard-other",
              checked: state.wizardOther === kind,
              onchange: () => {
                state.wizardOther = kind;
                render();
              },
            }),
            h("span", {}, h("strong", {}, KIND_NAME[kind]), ` (${KIND_NOTE[kind]})`),
          ),
          state.wizardOther === kind ? h("div", { class: "indent" }, providerBlock(kind, { auto: true, onDone: done })) : null,
        ),
      ),
      h("div", { class: "buttons" }, h("button", { onclick: () => ((state.wizardStep = "choose"), render()) }, "Volver"), later),
    );
  }
  return h(
    "section",
    { class: "page wizard" },
    head,
    h(
      "div",
      { class: "card" },
      h("h3", {}, "Ollama cloud (recomendado)"),
      h("p", { class: "intro" }, "Proveedor externo por defecto: no hay que instalar nada. Necesita su clave de Ollama y que usted confirme qué contenido sale del equipo."),
      h("button", { class: "primary", onclick: () => ((state.wizardStep = "cloud"), render()) }, "Configurar Ollama cloud"),
    ),
    h(
      "div",
      { class: "card" },
      h("h3", {}, "Otro proveedor externo"),
      h("p", { class: "intro" }, "Un servicio compatible con OpenAI o Claude de Anthropic, con su propia clave."),
      h("button", { onclick: () => ((state.wizardStep = "other"), render()) }, "Elegir otro proveedor externo"),
    ),
    h(
      "div",
      { class: "card" },
      h("h3", {}, "Usar la IA local"),
      h("p", { class: "intro" }, "Ollama en este equipo: el contenido no sale de él. Hay que instalar Ollama y descargar un modelo de varios GB (Preparación)."),
      h("button", { onclick: goInstallLocal }, "Usar la IA local"),
    ),
    h("div", { class: "buttons" }, later),
  );
}

// ---- Preparación del equipo (R08) ----

const STEP_LABEL: Record<SetupStep, string> = {
  ocr: "OCR incluido (Tesseract)",
  ollama: "Ollama (IA local)",
  service: "Servicio de Ollama",
  model: "Modelo de IA local",
  verify: "Respuesta de la IA local",
};

const STEP_STATE: Record<SetupEvent["state"], string> = {
  running: "En curso",
  done: "Hecho",
  skipped: "Ya estaba listo",
  failed: "Falló",
};

const MISSING_LABEL: Record<string, string> = {
  ollama: "Falta Ollama, el servicio de IA local.",
  ollama_outdated: "La versión de Ollama instalada no es compatible; hay que actualizarla.",
  model: "Falta el modelo de IA local (unos 5,2 GB).",
  ocr: "Faltan componentes de OCR del paquete; reinstale NestAdmin.",
};

// Qué puede hacer el usuario ante cada error del contrato.
const ERROR_HINT: Record<SetupErrorCode, string> = {
  network: "Compruebe la conexión a Internet y pulse Reintentar: la descarga continúa desde donde se cortó.",
  signature: "El instalador descargado no está firmado por Ollama Inc. y no se ha ejecutado. Pulse Reintentar para descargarlo de nuevo.",
  install_failed: "El instalador de Ollama terminó con error. Pulse Reintentar o instale Ollama desde ollama.com.",
  unavailable: "Ollama no responde tras arrancarlo. Pulse Reintentar o reinicie el equipo.",
  model_failed: "Ollama rechazó la descarga del modelo (por ejemplo, por falta de espacio en disco). Libere espacio y pulse Reintentar.",
  verify_failed: "El modelo no dio una respuesta válida. Pulse Reintentar.",
  ocr_missing: "Reinstale NestAdmin para recuperar los componentes de OCR.",
  cancelled: "La preparación se canceló. Puede reanudarla cuando quiera; lo ya hecho no se repite.",
};

// Un tiempo agotado no es una respuesta inválida: cargar el modelo es lo más lento.
const TIMEOUT_HINT = "Pulse Reintentar: con el modelo ya cargado en memoria suele responder antes.";

function size(bytes: number) {
  return bytes >= 1e9 ? `${(bytes / 1e9).toFixed(1).replace(".", ",")} GB` : `${Math.round(bytes / 1e6)} MB`;
}

function seconds(ms: number) {
  return `${(ms / 1000).toFixed(1).replace(".", ",")} s`;
}

async function refreshCheck() {
  try {
    const status = await api.setupCheck();
    state.check = status;
    state.memory = status.memory;
  } catch (error) {
    state.notice = { kind: "error", text: String(error) };
  }
  render(true);
}

async function startPreparation() {
  // Se marca antes de llamar: los eventos pueden llegar antes que la respuesta.
  state.preparing = true;
  state.steps = {};
  state.setupDone = null;
  render();
  try {
    await api.setupPrepare();
  } catch (error) {
    state.preparing = false;
    state.setupDone = { report: null, error: { code: "cancelled", message: String(error) } };
  }
  render();
}

let progressRender: number | null = null;

onSetup((event) => {
  state.steps[event.step] = event;
  // Ollama informa muchas veces por segundo: se agrupan los repintados.
  progressRender ??= window.setTimeout(() => {
    progressRender = null;
    render(true);
  }, 250);
});

onSetupDone((done) => {
  state.preparing = false;
  state.setupDone = done;
  if (done.report) {
    state.check = { status: "ready" };
    state.memory = done.report.memory;
  }
  else void refreshCheck();
  render();
});

onStartup((startup) => {
  state.startup = startup;
  applyStartup();
});

// Comprobación al abrir (D39): si falta algo, la ventana muestra el asistente.
function applyStartup() {
  const s = state.startup;
  if (!s) return;
  if (s.outcome === "needs_preparation") state.check = s.check;
  if ((s.outcome === "needs_preparation" || s.outcome === "failed") && !state.preparing) {
    state.view = "setup";
    state.notice = null;
  }
  render(true);
}

function stepRow(step: SetupStep) {
  const event = state.steps[step];
  const running = event?.state === "running";
  return h(
    "div",
    { class: "component" },
    h(
      "span",
      { class: "name" },
      STEP_LABEL[step],
      event ? h("span", { class: `badge step-${event.state}` }, STEP_STATE[event.state]) : h("span", { class: "badge" }, "Pendiente"),
    ),
    h("div", { class: "side" }),
    event
      ? h(
          "span",
          { class: event.state === "failed" ? "error-text" : "detail-line" },
          event.done != null && event.total ? `${event.message}: ${size(event.done)} de ${size(event.total)}` : event.message,
        )
      : null,
    running ? (event.total ? h("progress", { max: String(event.total), value: String(event.done ?? 0) }) : h("progress", {})) : null,
  );
}

// Aviso de memoria (D46): informa y deja seguir; los proveedores externos solo se eligen a mano.
function memoryNotice() {
  const m = state.memory;
  if (!m?.warning) return null;
  const strong = m.level === "insufficient";
  return h(
    "div",
    { class: strong ? "notice error" : "notice warn", role: strong ? "alert" : "status" },
    h("span", {}, m.warning),
    state.prepareOnly || state.preparing || m.level === "recommended"
      ? null
      : h("button", { onclick: () => ((state.view = "settings"), render()) }, "Ir a Configuración"),
  );
}

function setupView() {
  const done = state.setupDone;
  const error = done?.error;
  const report = done?.report;
  const check = state.check;
  const startupFailed = state.startup?.outcome === "failed" ? state.startup.error : null;
  const buttons: Child[] = [];
  if (state.preparing) {
    buttons.push(h("button", { onclick: () => void api.setupCancel() }, "Cancelar"));
  } else if (!report || !state.prepareOnly) {
    buttons.push(
      h(
        "button",
        { class: "primary", onclick: () => void startPreparation() },
        error ? "Reintentar" : report ? "Comprobar de nuevo" : "Preparar ahora",
      ),
    );
  }
  if (state.prepareOnly && !report) {
    buttons.push(h("button", { onclick: () => void api.setupAbandon() }, "Cerrar sin terminar"));
  }
  return h(
    "section",
    { class: state.prepareOnly ? "page wizard" : "page" },
    h("h2", {}, "Preparación de la IA local"),
    h(
      "p",
      { class: "intro" },
      "NestAdmin trabaja con componentes locales. Lo que falta se descarga de su fuente oficial y se comprueba antes de usarlo (huella SHA-256 y firma de Ollama Inc.); lo ya instalado se reutiliza. Si se corta la conexión, se reintenta hasta 3 veces y la descarga continúa. Ningún documento sale del equipo y nunca se pasa a un servicio externo.",
    ),
    !state.preparing && !done && check && check.status !== "ready"
      ? h("p", {}, check.status === "stopped" ? "Ollama está instalado pero detenido; se arrancará." : MISSING_LABEL[check.component])
      : null,
    !state.preparing && !done && check?.status === "ready" ? h("p", { class: "ok-line" }, "La IA local está lista.") : null,
    startupFailed && !done ? h("p", { class: "error-text", role: "alert" }, `Ollama no arrancó: ${startupFailed.message}`) : null,
    memoryNotice(),
    h("div", { class: "components" }, ...(["ocr", "ollama", "service", "model", "verify"] as SetupStep[]).map(stepRow)),
    report
      ? h(
          "p",
          { class: "ok-line", role: "status" },
          `Listo: el modelo ${report.model} respondió «${report.answer}» en ${seconds(report.latency_ms)}.`,
          state.prepareOnly ? " El instalador terminará en unos segundos." : "",
        )
      : null,
    error
      ? h(
          "div",
          { class: "notice error", role: "alert" },
          h("span", {}, error.code === "cancelled" ? ERROR_HINT.cancelled : `${error.message}. ${error.message.startsWith("timeout:") ? TIMEOUT_HINT : (ERROR_HINT[error.code] ?? "")}`),
        )
      : null,
    h("div", { class: "buttons" }, ...buttons),
  );
}

function render(background = false) {
  // Modo instalador (D36): solo el asistente, sin secciones ni datos.
  if (state.prepareOnly) {
    root.replaceChildren(h("main", { class: "wizard-only" }, setupView()));
    return;
  }
  const views: Record<View, () => HTMLElement> = {
    review: reviewView,
    tasks: tasksView,
    rules: rulesView,
    history: historyView,
    mail: mailView,
    settings: settingsView,
    setup: setupView,
  };
  const reviewable = state.notice?.task !== undefined ? state.tasks.find((t) => t.id === state.notice?.task && t.state === "awaiting_review") : undefined;
  const notice = state.notice
    ? h(
        "div",
        { class: `notice ${state.notice.kind}`, role: state.notice.kind === "error" ? "alert" : "status" },
        h("span", {}, state.notice.text),
        reviewable ? h("button", { onclick: () => openInReview(reviewable) }, "Ver") : null,
        h("button", { class: "close", "aria-label": "Cerrar aviso", onclick: () => ((state.notice = null), render()) }, "×"),
      )
    : null;
  // Una actualización de fondo no borra lo que el usuario está escribiendo.
  const focused = document.activeElement;
  if (background && (focused instanceof HTMLTextAreaElement || focused instanceof HTMLSelectElement || (focused instanceof HTMLInputElement && focused.type !== "file"))) {
    if (root.contains(focused)) return;
  }
  // D103: con un proveedor externo, al abrir solo se comprueba el OCR del paquete.
  const ocrBanner =
    state.startup?.outcome === "ocr_missing"
      ? h("div", { class: "notice error", role: "alert" }, h("span", {}, "Faltan componentes de OCR del paquete. Reinstale NestAdmin para recuperarlos; mientras tanto no se podrán leer documentos escaneados."))
      : null;
  const wait = state.settings?.ai_wait_message;
  const waitBanner =
    wait && (state.view === "review" || state.view === "tasks")
      ? h("div", { class: "banner", role: "status" }, h("span", {}, `${wait}. Las tareas esperan en la cola.`), h("button", { onclick: goToSettings }, "Ir a Configuración"))
      : null;
  if (state.wizard) {
    root.replaceChildren(h("main", { class: "wizard-only" }, wizardView()));
    return;
  }
  const restore = keepPosition();
  root.replaceChildren(
    header(),
    h(
      "div",
      { class: "body" },
      nav(),
      h("main", { class: state.view === "review" ? "fill" : undefined, "data-scroll-key": `main:${state.view}` }, ocrBanner, notice, waitBanner, views[state.view]()),
    ),
  );
  restore();
}

/**
 * El repintado sustituye el DOM (D138): guarda el desplazamiento de cada panel y el foco, y
 * devuelve la función que los repone. Un panel de otro elemento tiene otra clave y empieza arriba.
 */
function keepPosition(): () => void {
  const scroll = new Map<string, number>();
  for (const el of root.querySelectorAll<HTMLElement>("[data-scroll-key]")) scroll.set(el.dataset.scrollKey!, el.scrollTop);
  const active = document.activeElement as HTMLElement | null;
  const focusKey = state.focusAfterRender ?? active?.dataset?.focusKey ?? null;
  state.focusAfterRender = null;
  const selection = active instanceof HTMLTextAreaElement ? ([active.selectionStart, active.selectionEnd] as const) : null;
  return () => {
    for (const el of root.querySelectorAll<HTMLElement>("[data-scroll-key]")) {
      const top = scroll.get(el.dataset.scrollKey!);
      if (top) el.scrollTop = top;
    }
    if (!focusKey) return;
    const target = [...root.querySelectorAll<HTMLElement>("[data-focus-key]")].find((el) => el.dataset.focusKey === focusKey);
    if (!target) return;
    target.focus();
    if (selection && target instanceof HTMLTextAreaElement) target.setSelectionRange(selection[0], selection[1]);
  };
}

void (async () => {
  try {
    const launch = await api.launchMode();
    state.prepareOnly = launch.prepare_only;
    state.preparing = launch.preparing;
    state.startup ??= launch.startup;
  } catch (error) {
    state.notice = { kind: "error", text: String(error) };
  }
  if (state.prepareOnly) {
    // El instalador espera: la preparación empieza sola.
    state.view = "setup";
    if (state.preparing) render();
    else void startPreparation();
    // La comprobación solo lee: aporta el aviso de memoria mientras se prepara.
    void refreshCheck();
    return;
  }
  onChanged(() => void refresh(true));
  onMailSignIn((url) => {
    state.signInUrl = url;
    render(true);
  });
  await refresh();
  // Primer uso (D101): una vez por arranque, y solo si falta elegir, la clave o el destino;
  // con una clave rechazada basta el aviso.
  const wait = state.settings?.ai_wait;
  if (wait === "needs_choice" || wait === "needs_key" || wait === "needs_consent") {
    state.wizard = true;
    render();
  }
  applyStartup();
  void refreshCheck();
})();
