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

// ---- Utilidades de DOM: todo el texto se inserta como texto, nunca como HTML. ----

type Child = Node | string | number | null | undefined | false;
type Attrs = Record<string, string | boolean | ((event: Event) => void) | undefined>;

function h(tag: string, attrs: Attrs = {}, ...children: Child[]): HTMLElement {
  const el = document.createElement(tag);
  for (const [key, value] of Object.entries(attrs)) {
    if (value === undefined || value === false) continue;
    if (typeof value === "function") el.addEventListener(key.replace(/^on/, ""), value);
    else if (value === true) el.setAttribute(key, "");
    else el.setAttribute(key, value);
  }
  for (const child of children) {
    if (child === null || child === undefined || child === false) continue;
    el.append(child instanceof Node ? child : String(child));
  }
  return el;
}

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
};

const CATEGORY_LABEL: Record<string, string> = {
  solicitud_factura: "Solicitud de factura",
  consulta_pago: "Consulta de pago",
  incidencia_factura: "Incidencia de factura",
  documentacion: "Documentación",
  otros: "Otros",
};

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
  selected: null as number | null,
  // Filtro de la lista de pendientes por origen: "" (todo), "file" o el ID de una cuenta.
  originFilter: "",
  tasks: [] as Task[],
  rules: [] as Rule[],
  audit: [] as AuditEntry[],
  settings: null as SettingsView | null,
  mail: null as MailView | null,
  // Dirección del inicio de sesión en curso, por si el navegador no se abrió solo.
  signInUrl: null as string | null,
  notice: null as { kind: "ok" | "error" | "warn"; text: string } | null,
  // Borradores editados y aún no guardados, para no perderlos al refrescar.
  drafts: {} as Record<number, string>,
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

function missing() {
  return h("span", { class: "missing" }, "No consta");
}

// ---- Vistas ----

function header() {
  const pending = state.tasks.filter((t) => t.state === "awaiting_review").length;
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
      h("span", { class: "pill" }, `${pending} pendientes`),
      working ? h("span", { class: "pill working" }, `${working} en proceso`) : null,
      active
        ? h(
            "span",
            { class: `pill provider ${active.provider}`, title: "Proveedor de IA activo" },
            active.provider === "ollama" ? `Local · ${active.model}` : `Groq (externo) · ${active.model}`,
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
  const items: Array<[View, string, number?]> = [
    ["review", "Pendientes", state.tasks.filter((t) => t.state === "awaiting_review").length],
    ["tasks", "Tareas"],
    ["rules", "Reglas"],
    ["history", "Historial"],
    ["mail", "Correo", state.mail?.accounts.some(needsAttention) ? 1 : undefined],
    ["settings", "Configuración"],
    ["setup", "Preparación", (state.check && state.check.status !== "ready") || state.memory?.level === "insufficient" ? 1 : undefined],
  ];
  return h(
    "nav",
    { class: "sidebar", "aria-label": "Secciones" },
    ...items.map(([view, label, count]) =>
      h(
        "button",
        {
          class: state.view === view ? "nav active" : "nav",
          "aria-current": state.view === view ? "page" : undefined,
          onclick: () => {
            state.view = view;
            state.notice = null;
            render();
          },
        },
        label,
        count ? h("span", { class: "count" }, count) : null,
      ),
    ),
    isPreview ? h("p", { class: "preview-note" }, "Vista previa en navegador con datos sintéticos.") : null,
  );
}

function reviewView() {
  const all = state.tasks.filter((t) => t.state === "awaiting_review");
  // Orígenes presentes en la lista, para el filtro: archivos importados y cada cuenta.
  const origins = new Map<string, string>();
  for (const t of all) {
    const key = originKey(t);
    if (!origins.has(key)) origins.set(key, key === "file" ? "Archivos importados" : accountShort(taskMailOrigin(t), key));
  }
  if (state.originFilter && !origins.has(state.originFilter)) state.originFilter = "";
  const pending = all.filter((t) => !state.originFilter || originKey(t) === state.originFilter);
  if (state.selected === null || !pending.some((t) => t.id === state.selected)) state.selected = pending[0]?.id ?? null;
  const selected = pending.find((t) => t.id === state.selected);
  const filter = h(
    "select",
    {
      "aria-label": "Filtrar por origen",
      onchange: (event) => {
        state.originFilter = (event.target as HTMLSelectElement).value;
        render();
      },
    },
    h("option", { value: "" }, `Todos los orígenes (${all.length})`),
    ...[...origins].map(([key, label]) =>
      h("option", { value: key, selected: key === state.originFilter }, `${label} (${all.filter((t) => originKey(t) === key).length})`),
    ),
  );
  return h(
    "section",
    { class: "split" },
    h(
      "div",
      { class: "list" },
      h("h2", {}, "Pendientes de revisión"),
      origins.size > 1 ? h("label", { class: "filter" }, "Origen ", filter) : null,
      all.length === 0 ? h("p", { class: "empty" }, "No hay nada pendiente. Las tareas nuevas aparecerán aquí.") : null,
      ...pending.map((t) =>
        h(
          "button",
          {
            class: t.id === state.selected ? "item selected" : "item",
            onclick: () => {
              state.selected = t.id;
              render();
            },
          },
          h("span", { class: "kind" }, t.kind === "invoice" ? "Factura" : "Correo"),
          h("span", { class: "item-title" }, title(t)),
          (t.result?.issues?.length ?? 0) > 0 ? h("span", { class: "warn-dot", title: "Con incidencias" }, "!") : null,
          h("span", { class: "item-origin" }, itemOrigin(t)),
        ),
      ),
    ),
    h("div", { class: "detail" }, selected ? (selected.kind === "invoice" ? invoiceDetail(selected) : emailDetail(selected)) : null),
  );
}

function provenance(result: Record<string, any>) {
  return h(
    "p",
    { class: "meta" },
    `Propuesta de ${result.provider === "groq" ? "Groq (externo)" : "Ollama local"} · ${result.model}`,
    result.latency_ms ? ` · ${(result.latency_ms / 1000).toFixed(1)} s` : "",
    ` · versión ${result.version}`,
  );
}

function invoiceDetail(task: Task) {
  const r = task.result ?? {};
  const fields: Record<string, string | null> = r.fields ?? {};
  const issues: string[] = r.issues ?? [];
  const unreadable: Array<{ field: string }> = r.unreadable ?? [];
  const pages: Array<{ number: number; method: string }> = r.pages ?? [];
  const ocrPages = pages.filter((p) => p.method === "ocr").map((p) => p.number);
  const sources: Record<string, FieldSource> = r.sources ?? {};
  const origin: DocumentOrigin | null = r.origin ?? safeJson(task.input_ref).origin ?? null;
  return h(
    "article",
    {},
    h("h2", {}, title(task)),
    provenance(r),
    documentOrigin(task, origin),
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
    h("p", { class: "reason" }, `Motivo de la revisión: ${r.review_reason ?? "requiere aprobación"}`),
    recoveryBox(task, "registrar"),
    issues.length
      ? h("ul", { class: "issues" }, ...issues.map((i) => h("li", {}, ISSUE_LABEL[i] ?? i)))
      : h("p", { class: "ok-line" }, "Sin incidencias en los importes."),
    (r.duplicates ?? []).length
      ? h(
          "p",
          { class: "dup" },
          "Se parece a: ",
          (r.duplicates as Array<{ existing_id: string }>).map((d) => `tarea ${d.existing_id}`).join(", "),
          ". No se ha descartado; decida usted.",
        )
      : null,
    h(
      "table",
      { class: "fields" },
      h(
        "tbody",
        {},
        h("tr", {}, h("th", {}, "Campo"), h("th", {}, "Valor extraído"), h("th", {}, "Dónde aparece en el original")),
        ...Object.keys(FIELD_LABEL).map((key) =>
          h(
            "tr",
            {},
            h("th", {}, FIELD_LABEL[key]),
            h(
              "td",
              {},
              fields[key] ?? missing(),
              unreadable.some((u) => u.field === key) ? h("span", { class: "missing" }, " (ilegible)") : null,
            ),
            h("td", { class: "source" }, fieldSource(fields[key], sources[key], pages.length)),
          ),
        ),
      ),
    ),
    h(
      "div",
      { class: "buttons" },
      h(
        "button",
        {
          class: "primary",
          disabled: r.recovery?.needs_confirmation === true,
          onclick: () => void act(async () => afterApproval(await api.approve(task.id, r.version ?? 1), "Factura registrada en el simulador")),
        },
        "Aprobar y registrar",
      ),
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
    oninput: () => (state.drafts[task.id] = body.value),
  }) as HTMLTextAreaElement;
  body.value = state.drafts[task.id] ?? draft.body ?? "";
  const recipients: string[] = draft.to ?? [];
  return h(
    "article",
    {},
    h("h2", {}, message.subject ?? h("span", { class: "missing" }, "(sin asunto)")),
    provenance(r),
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
    h("p", {}, "Categoría: ", h("strong", {}, CATEGORY_LABEL[r.category] ?? r.category)),
    h("h3", {}, "Borrador"),
    h("p", { class: "meta" }, "Para: ", recipients.length ? recipients.join(", ") : h("span", { class: "missing" }, "sin destinatario; no se puede enviar")),
    body,
    h(
      "div",
      { class: "buttons" },
      h(
        "button",
        {
          class: "primary",
          // Con el borrador anterior desaparecido, primero hay que confirmar que no se envió.
          disabled: recipients.length === 0 || r.recovery?.needs_confirmation === true,
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
                  t.error ? h("span", { class: "error-text" }, t.error.message) : t.result?.issues?.length ? t.result.issues.map((i: string) => ISSUE_LABEL[i] ?? i).join(", ") : "",
                ),
                h(
                  "td",
                  {},
                  t.state === "failed" ? h("button", { onclick: () => void act(() => api.retry(t.id), "Tarea reencolada") }, "Reintentar") : null,
                  t.state === "awaiting_review"
                    ? h(
                        "button",
                        {
                          onclick: () => {
                            state.view = "review";
                            state.selected = t.id;
                            render();
                          },
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

function conditionsText(rule: Rule) {
  const c = rule.conditions;
  if (rule.action_kind === "register_invoice")
    return `Proveedor «${c.supplier}»${c.max_total ? `, total ≤ ${c.max_total}` : ""}, sin incidencias`;
  return `Categoría «${CATEGORY_LABEL[c.category] ?? c.category}», con destinatario`;
}

function rulesView() {
  const kind = h(
    "select",
    { name: "kind", onchange: () => toggleFields() },
    h("option", { value: "register_invoice" }, "Registrar factura"),
    h("option", { value: "send_draft" }, "Enviar respuesta"),
  ) as HTMLSelectElement;
  const id = h("input", { name: "id", required: true, placeholder: "p. ej. alfa-mensual", pattern: "[a-z0-9-]+" }) as HTMLInputElement;
  const supplier = h("input", { name: "supplier", placeholder: "Nombre exacto del proveedor" }) as HTMLInputElement;
  const maxTotal = h("input", { name: "max_total", placeholder: "500.00", inputmode: "decimal" }) as HTMLInputElement;
  const category = h(
    "select",
    { name: "category" },
    ...Object.entries(CATEGORY_LABEL).map(([value, label]) => h("option", { value }, label)),
  ) as HTMLSelectElement;
  const invoiceFields = h("div", { class: "row" }, h("label", {}, "Proveedor", supplier), h("label", {}, "Total máximo (opcional)", maxTotal));
  const emailFields = h("div", { class: "row", hidden: true }, h("label", {}, "Categoría", category));
  function toggleFields() {
    invoiceFields.hidden = kind.value !== "register_invoice";
    emailFields.hidden = kind.value === "register_invoice";
  }
  const form = h(
    "form",
    {
      class: "card",
      onsubmit: (event) => {
        event.preventDefault();
        const conditions: Record<string, string> =
          kind.value === "register_invoice"
            ? Object.fromEntries(Object.entries({ supplier: supplier.value.trim(), max_total: maxTotal.value.trim() }).filter(([, v]) => v))
            : { category: category.value };
        void act(() => api.createRule(id.value.trim(), kind.value, conditions), "Regla creada inactiva; actívela cuando la haya comprobado");
      },
    },
    h("h3", {}, "Nueva regla"),
    h("div", { class: "row" }, h("label", {}, "Identificador", id), h("label", {}, "Acción", kind)),
    invoiceFields,
    emailFields,
    h("button", { class: "primary", type: "submit" }, "Crear regla"),
  );
  return h(
    "section",
    { class: "page" },
    h("h2", {}, "Reglas"),
    h(
      "p",
      { class: "intro" },
      "Por defecto todo se revisa. Una regla activa permite enviar o registrar sin revisión solo cuando es la única que coincide y los datos no tienen incidencias. Cada cambio crea una versión nueva y queda en el historial.",
    ),
    state.rules.length
      ? h(
          "table",
          { class: "grid" },
          h("thead", {}, h("tr", {}, ...["Regla", "Acción", "Condiciones", "Versión", "Estado", ""].map((c) => h("th", {}, c)))),
          h(
            "tbody",
            {},
            ...state.rules.map((r) =>
              h(
                "tr",
                {},
                h("td", {}, r.id),
                h("td", {}, ACTION_LABEL[r.action_kind] ?? r.action_kind),
                h("td", {}, conditionsText(r)),
                h("td", {}, `v${r.version}`),
                h("td", {}, h("span", { class: r.active ? "badge state-completed" : "badge state-cancelled" }, r.active ? "Activa" : "Inactiva")),
                h(
                  "td",
                  {},
                  h(
                    "button",
                    { onclick: () => void act(() => api.setRuleActive(r.id, !r.active), r.active ? "Regla desactivada" : "Regla activada") },
                    r.active ? "Desactivar" : "Activar",
                  ),
                ),
              ),
            ),
          ),
        )
      : h("p", { class: "empty" }, "No hay reglas: todas las acciones requieren su aprobación."),
    form,
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

// Segunda línea de cada pendiente: de qué buzón y de quién viene, o de qué archivo.
function itemOrigin(task: Task): string {
  const input = safeJson(task.input_ref);
  if (task.kind === "invoice") {
    const origin: DocumentOrigin | undefined = task.result?.origin ?? input.origin;
    if (origin?.kind === "email")
      return `Adjunto de ${origin.from ?? "remitente no consta"} · ${accountShort(mailOrigin(origin.account, `${origin.account.account}:`), origin.account.account)}`;
    return origin ? `Archivo importado${origin.imported_at ? ` el ${dateText(origin.imported_at)}` : ""}` : "Archivo importado (origen no registrado)";
  }
  const message = task.result?.message ?? input.message ?? {};
  return `${message.from ?? "Remitente no consta"} → ${accountShort(taskMailOrigin(task), accountId(message.id ?? input.message_id))}`;
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
function documentOrigin(task: Task, origin: DocumentOrigin | null) {
  const open = h(
    "button",
    {
      onclick: () =>
        isPreview
          ? ((state.notice = { kind: "ok", text: "Vista previa: en la aplicación se abriría la copia guardada con el visor del sistema." }), render())
          : void act(() => api.openOriginal(task.id)),
    },
    "Abrir original",
  );
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
    h("div", { class: "buttons" }, open, h("span", { class: "meta" }, "Abre la copia guardada en NestAdmin con el visor del sistema.")),
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

function settingsView() {
  const s = state.settings;
  if (!s) return h("section", { class: "page" }, h("p", {}, "Cargando…"));
  const ollamaModel = s.choice.provider === "ollama" ? s.choice.model : "qwen3:8b";
  const ollamaUrl = s.choice.provider === "ollama" ? s.choice.base_url : "http://127.0.0.1:11434";
  const groqModel = s.choice.provider === "groq" ? s.choice.model : "qwen/qwen3-32b";
  const ollamaRadio = h("input", { type: "radio", name: "provider", value: "ollama", checked: s.choice.provider === "ollama" }) as HTMLInputElement;
  const groqRadio = h("input", { type: "radio", name: "provider", value: "groq", checked: s.choice.provider === "groq" }) as HTMLInputElement;
  const model = h("input", { value: ollamaModel }) as HTMLInputElement;
  const url = h("input", { value: ollamaUrl }) as HTMLInputElement;
  const gModel = h("input", { value: groqModel }) as HTMLInputElement;
  const key = h("input", { type: "password", autocomplete: "off", placeholder: s.groq_key_saved ? "Clave guardada" : "gsk_…" }) as HTMLInputElement;
  const apply = () => {
    const choice: ProviderChoice = groqRadio.checked
      ? { provider: "groq", model: gModel.value.trim() }
      : { provider: "ollama", base_url: url.value.trim(), model: model.value.trim() };
    if (choice.provider === "groq" && !confirm("Con Groq, el contenido de documentos y correos se envía a un servicio externo. ¿Continuar?")) return;
    void act(() => api.selectProvider(choice), `Proveedor activo: ${choice.provider === "groq" ? "Groq" : "Ollama local"}`);
  };
  return h(
    "section",
    { class: "page" },
    h("h2", {}, "Configuración"),
    h(
      "div",
      { class: "card" },
      h("h3", {}, "Proveedor de IA"),
      h("p", { class: "intro" }, "Si el proveedor elegido falla, la tarea queda fallida con el motivo. NestAdmin nunca cambia de proveedor por su cuenta."),
      h("label", { class: "choice" }, ollamaRadio, h("span", {}, h("strong", {}, "Ollama (local, recomendado)"), " — el contenido no sale del equipo.")),
      h("div", { class: "row indent" }, h("label", {}, "Modelo", model), h("label", {}, "Dirección local", url)),
      h("label", { class: "choice" }, groqRadio, h("span", {}, h("strong", {}, "Groq (externo)"), " — envía el contenido a la API de Groq. Requiere clave.")),
      h("div", { class: "row indent" }, h("label", {}, "Modelo", gModel)),
      h("button", { class: "primary", onclick: apply }, "Usar este proveedor"),
    ),
    h(
      "div",
      { class: "card" },
      h("h3", {}, "Clave de Groq"),
      h("p", { class: "intro" }, "Se guarda en el Administrador de credenciales de Windows. NestAdmin no la muestra ni la escribe en registros."),
      h("div", { class: "row" }, h("label", {}, s.groq_key_saved ? "Sustituir clave" : "Clave", key)),
      h(
        "div",
        { class: "buttons" },
        h("button", { onclick: () => void act(() => api.saveGroqKey(key.value), "Clave guardada") }, "Guardar clave"),
        s.groq_key_saved ? h("button", { class: "danger", onclick: () => void act(() => api.deleteGroqKey(), "Clave eliminada") }, "Eliminar clave") : null,
      ),
    ),
    h("div", { class: "card" }, h("h3", {}, "Datos locales"), h("p", { class: "mono" }, s.data_dir)),
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

// Aviso de memoria (D46): informa y deja seguir; Groq solo se elige a mano.
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
  const notice = state.notice
    ? h(
        "div",
        { class: `notice ${state.notice.kind}`, role: state.notice.kind === "error" ? "alert" : "status" },
        h("span", {}, state.notice.text),
        h("button", { class: "close", "aria-label": "Cerrar aviso", onclick: () => ((state.notice = null), render()) }, "×"),
      )
    : null;
  // Una actualización de fondo no borra lo que el usuario está escribiendo.
  const focused = document.activeElement;
  if (background && (focused instanceof HTMLTextAreaElement || (focused instanceof HTMLInputElement && focused.type !== "file"))) {
    if (root.contains(focused)) return;
  }
  root.replaceChildren(header(), h("div", { class: "body" }, nav(), h("main", {}, notice, views[state.view]())));
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
  applyStartup();
  void refreshCheck();
})();
