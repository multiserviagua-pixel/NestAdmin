import "./style.css";
import { api, isPreview, onChanged } from "./api";
import type { AuditEntry, ProviderChoice, Rule, SettingsView, Task } from "./types";

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

type View = "review" | "tasks" | "rules" | "history" | "settings";

const state = {
  view: "review" as View,
  selected: null as number | null,
  tasks: [] as Task[],
  rules: [] as Rule[],
  audit: [] as AuditEntry[],
  settings: null as SettingsView | null,
  notice: null as { kind: "ok" | "error"; text: string } | null,
  // Borradores editados y aún no guardados, para no perderlos al refrescar.
  drafts: {} as Record<number, string>,
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

async function refresh(background = false) {
  try {
    const [tasks, rules, audit, settings] = await Promise.all([
      api.tasks(),
      api.rules(),
      api.auditLog(),
      api.settings(),
    ]);
    Object.assign(state, { tasks, rules, audit, settings });
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
    accept: ".txt,text/plain",
    multiple: true,
    hidden: true,
    onchange: (event) => {
      const files = [...((event.target as HTMLInputElement).files ?? [])];
      void act(async () => {
        for (const file of files) await api.importDocument(file.name, await file.text());
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
      h("button", { class: "primary", onclick: () => fileInput.click() }, "Importar documento"),
      h(
        "button",
        {
          onclick: () =>
            void act(async () => {
              const added = await api.fetchMail();
              state.notice = { kind: "ok", text: added ? `${added} mensajes nuevos en cola` : "No hay mensajes nuevos" };
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
    ["settings", "Configuración"],
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
  const pending = state.tasks.filter((t) => t.state === "awaiting_review");
  if (state.selected === null || !pending.some((t) => t.id === state.selected)) state.selected = pending[0]?.id ?? null;
  const selected = pending.find((t) => t.id === state.selected);
  return h(
    "section",
    { class: "split" },
    h(
      "div",
      { class: "list" },
      h("h2", {}, "Pendientes de revisión"),
      pending.length === 0 ? h("p", { class: "empty" }, "No hay nada pendiente. Las tareas nuevas aparecerán aquí.") : null,
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
  return h(
    "article",
    {},
    h("h2", {}, title(task)),
    provenance(r),
    h("p", { class: "reason" }, `Motivo de la revisión: ${r.review_reason ?? "requiere aprobación"}`),
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
          ),
        ),
      ),
    ),
    h(
      "div",
      { class: "buttons" },
      h(
        "button",
        { class: "primary", onclick: () => void act(() => api.approve(task.id, r.version ?? 1), "Factura registrada en el simulador") },
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
    h(
      "div",
      { class: "message" },
      h("p", { class: "meta" }, "De: ", message.from ?? missing()),
      h("p", {}, message.body ?? missing()),
    ),
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
          disabled: recipients.length === 0,
          onclick: () =>
            void act(async () => {
              let version = r.version ?? 1;
              // Si se ha editado, se guarda primero como versión nueva y se aprueba esa.
              if (body.value !== (draft.body ?? "")) version = (await api.editDraft(task.id, body.value)).result?.version;
              await api.approve(task.id, version);
            }, "Respuesta enviada mediante el simulador"),
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
    h("p", { class: "intro" }, "Acciones ejecutadas en los simuladores y cambios de reglas, con su autorización."),
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

function render(background = false) {
  const views: Record<View, () => HTMLElement> = {
    review: reviewView,
    tasks: tasksView,
    rules: rulesView,
    history: historyView,
    settings: settingsView,
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

onChanged(() => void refresh(true));
void refresh();
