// Sección «Ventas» (fase 11, E3b): facturación de venta y registros Verifactu. Las reglas puras
// (textos, importes, acciones permitidas) están en `ventas-text.ts`; aquí solo se pinta y se
// llama a la API. Todo el texto se inserta como texto, nunca como HTML (D16). El backend decide
// siempre: la interfaz no calcula totales ni impone estados, solo ofrece lo que tiene sentido.
// Emitir y anular son acciones explícitas con confirmación; los recurrentes solo crean
// borradores (D87, D204).

import { api } from "./api";
import { actionBar, dataTable, emptyState, h, sectionHeader, statusChip, tabs, type Child } from "./components";
import type {
  SalesCompany,
  SalesCustomer,
  SalesDocument,
  SalesDraftIn,
  SalesKind,
  SalesLink,
  SalesPayment,
  SalesCollection,
  SalesProduct,
  SalesReceivedNote,
  SalesRecurring,
  SalesSetupStatus,
  SalesStatus,
  SalesFrequency,
  SalesPaymentMethod,
  SalesQualification,
  SalesRectificationKind,
  SalesRectificationMethod,
  VerifactuRecord,
} from "./types";
import {
  ACTION_LABEL,
  ACTION_TARGET,
  COLLECTION_LABEL,
  EMPTY_LIST,
  FREQUENCY_LABEL,
  KIND_LABEL,
  KIND_ORDER,
  NO_TOTALS,
  PAYMENT_METHOD_LABEL,
  QUALIFICATION_LABEL,
  RECTIFICATION_KIND_LABEL,
  RECTIFICATION_METHOD_LABEL,
  SETUP_RELOAD,
  STATUS_LABEL,
  STATUS_ORDER,
  TAX_PERCENTS,
  VERIFACTU_READONLY,
  ZERO_RATE_QUALIFICATIONS,
  allowedActions,
  chainText,
  collectionTone,
  convertConfirmText,
  deleteDraftConfirmText,
  documentTitle,
  exemptionText,
  formatDate,
  formatMoney,
  formatNumber,
  fractionToPercent,
  generateRecurringConfirmText,
  isBlockingCode,
  isInvoiceKind,
  isIsoDate,
  issueConfirmText,
  parseDecimalInput,
  parseSalesError,
  recurringResultText,
  salesErrorText,
  setupNotices,
  statusTone,
  validateLine,
  voidConfirmText,
  type LineDraftText,
  type SalesAction,
} from "./ventas-text";

type Tab = "documents" | "customers" | "products" | "company" | "recurring" | "received" | "records";

interface Context {
  render: () => void;
  ask: (message: string) => Promise<boolean>;
}

interface Editor {
  id: number | null;
  kind: SalesKind;
  status: SalesStatus;
  customer_id: string;
  issue_date: string;
  operation_date: string;
  due_date: string;
  withholding: string;
  requested: boolean;
  art42: boolean;
  lines: LineDraftText[];
  rect_kind: SalesRectificationKind;
  rect_method: SalesRectificationMethod;
  rect_base: string;
  rect_tax: string;
  problems: string[];
  doc: SalesDocument | null;
  dirty: boolean;
  busy: boolean;
}

interface Detail {
  doc: SalesDocument;
  links: SalesLink[];
  payments: SalesPayment[];
  collection: SalesCollection | null;
  logoUrl: string | null;
  pay: { date: string; amount: string; method: SalesPaymentMethod };
}

const emptyLine = (): LineDraftText => ({ description: "", quantity: "1", unit_price: "", discount_percent: "", tax_percent: "21", qualification: "", exemption_text: "" });
const comma = (value: string | null | undefined) => (value ?? "").replace(".", ",");

export function createSalesView(ctx: Context) {
  const s = {
    tab: "documents" as Tab,
    loaded: false,
    loading: false,
    message: null as null | { kind: "ok" | "error"; text: string },
    setup: null as SalesSetupStatus | null,
    company: null as SalesCompany | null,
    companyLogoUrl: null as string | null,
    companyForm: { name: "", tax_id: "", address: "", email: "", iban: "" },
    customers: [] as SalesCustomer[],
    showRetiredCustomers: false,
    customerForm: null as null | { id: number | null; name: string; tax_id: string; address: string; email: string },
    products: [] as SalesProduct[],
    showRetiredProducts: false,
    productForm: null as null | { id: number | null; description: string; unit_price: string; tax_percent: string; qualification: SalesQualification | ""; exemption_text: string },
    docs: [] as SalesDocument[],
    filter: { kind: "" as SalesKind | "", status: "" as SalesStatus | "", superseded: false },
    selected: new Set<number>(),
    newKind: "invoice" as SalesKind,
    editor: null as Editor | null,
    detail: null as Detail | null,
    templates: [] as SalesRecurring[],
    recurringForm: null as null | { id: number | null; customer_id: string; frequency: SalesFrequency; first_day: string; end: string; lines: LineDraftText[]; problems: string[] },
    received: [] as SalesReceivedNote[],
    receivedForm: null as null | { supplier_name: string; supplier_tax_id: string; supplier_number: string; date: string },
    review: null as null | { id: number; document: string; invoice: string },
    records: [] as VerifactuRecord[],
  };

  const blocksInvoices = () => setupNotices(s.setup).some((n) => n.blocksInvoices);
  const customerName = (id: number | null) => (id === null ? "Sin cliente" : (s.customers.find((c) => c.id === id)?.data.name ?? `Cliente ${id}`));
  const today = () => s.setup?.today ?? "";

  function say(kind: "ok" | "error", text: string) {
    s.message = { kind, text };
    ctx.render();
  }

  /** Ejecuta una acción de la API: muestra el error con su código y vuelve a leer el estado. */
  async function run<T>(work: () => Promise<T>, success?: string): Promise<T | undefined> {
    try {
      const value = await work();
      if (success) s.message = { kind: "ok", text: success };
      return value;
    } catch (error) {
      s.message = { kind: "error", text: salesErrorText(error) };
      // Un error de Verifactu puede deberse a un cambio de estado: se vuelve a leer el aviso.
      if (isBlockingCode(parseSalesError(error).code)) await refreshSetup();
      ctx.render();
      return undefined;
    }
  }

  /** Como `run`, para acciones sin valor de retorno: `true` si se hizo. */
  async function attempt(work: () => Promise<unknown>, success?: string): Promise<boolean> {
    return (await run(async () => (await work(), true), success)) === true;
  }

  async function refreshSetup() {
    try {
      s.setup = await api.salesSetupStatus();
    } catch (error) {
      s.message = { kind: "error", text: salesErrorText(error) };
    }
  }

  async function loadCore() {
    const [setup, company, customers, products, docs] = await Promise.all([
      api.salesSetupStatus(),
      api.salesCompany(),
      api.salesCustomers(true),
      api.salesProducts(true),
      api.salesDocuments({ kind: s.filter.kind || null, status: s.filter.status || null, include_superseded: s.filter.superseded }),
    ]);
    s.setup = setup;
    s.company = company;
    s.customers = customers;
    s.products = products;
    s.docs = docs;
    s.companyForm = { name: company?.name ?? "", tax_id: company?.tax_id ?? "", address: company?.address ?? "", email: company?.email ?? "", iban: company?.iban ?? "" };
  }

  async function loadTab() {
    if (s.tab === "recurring") s.templates = await api.salesRecurringTemplates();
    else if (s.tab === "received") s.received = await api.salesReceivedNotes();
    else if (s.tab === "records") {
      s.records = await api.verifactuRecords();
      s.setup = await api.salesSetupStatus();
    } else if (s.tab === "company") await loadLogo();
  }

  async function loadLogo() {
    if (s.companyLogoUrl) URL.revokeObjectURL(s.companyLogoUrl);
    s.companyLogoUrl = null;
    if (!s.company?.has_logo) return;
    try {
      s.companyLogoUrl = URL.createObjectURL(new Blob([await api.salesCompanyLogo()], { type: s.company.logo_mime ?? "image/png" }));
    } catch {
      s.companyLogoUrl = null;
    }
  }

  /** Lee de nuevo lo que muestra la pestaña actual (tras una acción o al entrar). */
  async function reload() {
    try {
      await loadCore();
      await loadTab();
      if (s.detail) await openDetail(s.detail.doc.id, false);
    } catch (error) {
      s.message = { kind: "error", text: salesErrorText(error) };
    }
    s.loaded = true;
    s.loading = false;
    ctx.render();
  }

  /** Se llama al abrir la sección. */
  function enter() {
    s.loading = true;
    void reload();
  }

  // ---- Componentes de formulario ----

  let uid = 0;
  function textField(label: string, get: () => string, set: (value: string) => void, options: { type?: string; hint?: string; key: string; placeholder?: string; inputmode?: string } = { key: "" }): HTMLElement {
    const id = `ventas-${options.key}-${++uid}`;
    const input = h("input", { id, type: options.type ?? "text", "data-focus-key": `ventas:${options.key}`, placeholder: options.placeholder, inputmode: options.inputmode, "aria-describedby": options.hint ? `${id}-hint` : undefined }) as HTMLInputElement;
    input.value = get();
    input.addEventListener("input", () => set(input.value));
    return h("div", { class: "field" }, h("div", { class: "field-head" }, h("label", { for: id }, label)), input, options.hint ? h("p", { class: "meta", id: `${id}-hint` }, options.hint) : null);
  }

  function selectField(label: string, options: Array<[string, string]>, get: () => string, set: (value: string) => void, key: string, onChange?: () => void): HTMLElement {
    const id = `ventas-${key}-${++uid}`;
    const select = h("select", { id, "data-focus-key": `ventas:${key}` }, ...options.map(([value, text]) => h("option", { value }, text))) as HTMLSelectElement;
    select.value = get();
    select.addEventListener("change", () => {
      set(select.value);
      onChange?.();
    });
    return h("div", { class: "field" }, h("div", { class: "field-head" }, h("label", { for: id }, label)), select);
  }

  function checkField(label: string, get: () => boolean, set: (value: boolean) => void, key: string): HTMLElement {
    const input = h("input", { type: "checkbox", "data-focus-key": `ventas:${key}` }) as HTMLInputElement;
    input.checked = get();
    input.addEventListener("change", () => set(input.checked));
    return h("label", { class: "choice" }, input, ` ${label}`);
  }

  const customerOptions = (includeEmpty = true): Array<[string, string]> => [
    ...(includeEmpty ? ([["", "Sin cliente"]] as Array<[string, string]>) : []),
    ...s.customers.filter((c) => !c.retired).map((c): [string, string] => [String(c.id), c.data.name]),
  ];

  /** Líneas editables (documentos y recurrentes). Cambia el arreglo recibido. */
  function linesEditor(lines: LineDraftText[], prefix: string, onChange: () => void): HTMLElement {
    return h(
      "div",
      { class: "sales-lines" },
      ...lines.map((line, index) => {
        const zero = line.tax_percent === "0";
        return h(
          "fieldset",
          { class: "row-panel sales-line" },
          h("legend", {}, `Línea ${index + 1}`),
          h(
            "div",
            { class: "row" },
            textField("Descripción", () => line.description, (v) => ((line.description = v), onChange()), { key: `${prefix}:${index}:description` }),
            textField("Cantidad", () => line.quantity, (v) => ((line.quantity = v), onChange()), { key: `${prefix}:${index}:quantity`, inputmode: "decimal" }),
            textField("Precio unitario (€)", () => line.unit_price, (v) => ((line.unit_price = v), onChange()), { key: `${prefix}:${index}:price`, inputmode: "decimal" }),
            textField("Descuento (%)", () => line.discount_percent, (v) => ((line.discount_percent = v), onChange()), { key: `${prefix}:${index}:discount`, inputmode: "decimal" }),
            selectField(
              "IVA (%)",
              TAX_PERCENTS.map((p): [string, string] => [p, p]),
              () => line.tax_percent,
              (v) => {
                line.tax_percent = v;
                if (v !== "0") line.qualification = "";
              },
              `${prefix}:${index}:tax`,
              () => (onChange(), ctx.render()),
            ),
          ),
          zero
            ? h(
                "div",
                { class: "row" },
                selectField(
                  "Causa del IVA 0 %",
                  [["", "Elija una causa"], ...ZERO_RATE_QUALIFICATIONS.map((q): [string, string] => [q, QUALIFICATION_LABEL[q]])],
                  () => line.qualification,
                  (v) => ((line.qualification = v as SalesQualification | ""), onChange()),
                  `${prefix}:${index}:qualification`,
                ),
                textField("Texto en la factura (opcional)", () => line.exemption_text, (v) => ((line.exemption_text = v), onChange()), { key: `${prefix}:${index}:exemption` }),
              )
            : null,
          h("div", { class: "row-actions" }, h("button", { type: "button", onclick: () => (lines.splice(index, 1), onChange(), ctx.render()) }, `Quitar línea ${index + 1}`)),
        );
      }),
      h("div", { class: "row-actions" }, h("button", { type: "button", "data-focus-key": `ventas:${prefix}:add-line`, onclick: () => (lines.push(emptyLine()), onChange(), ctx.render()) }, "Añadir línea")),
    );
  }

  function lineProblems(lines: LineDraftText[]) {
    const problems: string[] = [];
    const out: NonNullable<ReturnType<typeof validateLine>["line"]>[] = [];
    lines.forEach((line, index) => {
      const result = validateLine(line);
      if (!result.ok) problems.push(`Línea ${index + 1}: ${result.problem}`);
      else out.push(result.line!);
    });
    return { problems, out };
  }

  const problemList = (problems: string[]) =>
    problems.length ? h("div", { class: "notice error", role: "alert" }, h("ul", {}, ...problems.map((p) => h("li", {}, p)))) : null;

  // ---- Avisos de estado ----

  function setupBanner(): Child {
    const notices = setupNotices(s.setup);
    if (notices.length === 0) return null;
    return h(
      "div",
      {},
      ...notices.map((n) =>
        h(
          "div",
          { class: n.tone === "error" ? "notice error" : "banner", role: n.tone === "error" ? "alert" : "status" },
          h("span", {}, h("strong", {}, n.title), ". ", n.detail),
          n.canReload
            ? h(
                "button",
                {
                  "data-focus-key": "ventas:reload-producer",
                  onclick: () =>
                    void (async () => {
                      const state = await run(() => api.reloadVerifactuProducer());
                      await refreshSetup();
                      if (state?.configured) s.message = { kind: "ok", text: "Productor de Verifactu leído correctamente." };
                      else if (state) s.message = { kind: "error", text: state.problem ?? "El archivo del productor sigue sin estar disponible." };
                      ctx.render();
                    })(),
                },
                SETUP_RELOAD,
              )
            : null,
        ),
      ),
    );
  }

  // ---- Documentos: lista ----

  function listView(): HTMLElement {
    const groupable = s.docs.filter((d) => s.selected.has(d.id));
    const openNew = () => {
      s.editor = newEditor(s.newKind);
      ctx.render();
    };
    return h(
      "div",
      {},
      sectionHeader(
        "Documentos",
        selectField("Tipo", KIND_ORDER.filter((k) => k !== "corrective_invoice").map((k): [string, string] => [k, KIND_LABEL[k]]), () => s.newKind, (v) => (s.newKind = v as SalesKind), "new-kind"),
        h("button", { class: "primary", "data-focus-key": "ventas:new-doc", onclick: openNew }, "Nuevo documento"),
      ),
      h(
        "div",
        { class: "row" },
        selectField("Filtrar por tipo", [["", "Todos"], ...KIND_ORDER.map((k): [string, string] => [k, KIND_LABEL[k]])], () => s.filter.kind, (v) => (s.filter.kind = v as SalesKind | ""), "filter-kind", () => enter()),
        selectField("Filtrar por estado", [["", "Todos"], ...STATUS_ORDER.map((st): [string, string] => [st, STATUS_LABEL[st]])], () => s.filter.status, (v) => (s.filter.status = v as SalesStatus | ""), "filter-status", () => enter()),
        checkField("Incluir revisiones sustituidas", () => s.filter.superseded, (v) => ((s.filter.superseded = v), enter()), "filter-superseded"),
      ),
      s.docs.length === 0
        ? emptyState(s.loading ? "Cargando…" : s.filter.kind || s.filter.status ? "Ningún documento coincide con el filtro." : EMPTY_LIST)
        : dataTable(
            [{ label: "Agrupar" }, { label: "Documento" }, { label: "Cliente" }, { label: "Fecha" }, { label: "Total", numeric: true }, { label: "Estado" }, { label: "Acciones" }],
            s.docs.map((d) => [
              d.kind === "delivery_note" && d.status === "delivered"
                ? h("input", {
                    type: "checkbox",
                    "aria-label": `Agrupar ${documentTitle(d)}`,
                    checked: s.selected.has(d.id),
                    onchange: (event) => {
                      if ((event.target as HTMLInputElement).checked) s.selected.add(d.id);
                      else s.selected.delete(d.id);
                      ctx.render();
                    },
                  })
                : null,
              documentTitle(d),
              customerName(d.customer_id),
              formatDate(d.issue_date),
              d.totals ? formatMoney(d.totals.total, d.currency) : "—",
              statusChip(STATUS_LABEL[d.status], statusTone(d.status)),
              h("button", { "data-focus-key": `ventas:open:${d.id}`, onclick: () => void openDetail(d.id) }, `Abrir ${documentTitle(d)}`),
            ]),
          ),
      groupable.length
        ? actionBar(
            `${groupable.length} albarán(es) seleccionado(s): deben ser del mismo cliente, con NIF.`,
            h(
              "button",
              {
                class: "primary",
                onclick: () =>
                  void (async () => {
                    if (!(await ctx.ask(convertConfirmText("invoice", groupable.length)))) return;
                    const id = await run(() => api.salesConvert(groupable.map((d) => d.id), "invoice"), "Borrador de factura creado. Revíselo antes de emitir.");
                    s.selected.clear();
                    if (id !== undefined) await editDraft(id);
                    else ctx.render();
                  })(),
              },
              "Crear factura con los seleccionados",
            ),
          )
        : null,
    );
  }

  // ---- Documentos: editor de borrador ----

  function newEditor(kind: SalesKind): Editor {
    return {
      id: null, kind, status: "draft", customer_id: "", issue_date: "", operation_date: "", due_date: "", withholding: "", requested: false, art42: false, lines: [emptyLine()],
      rect_kind: "R1", rect_method: "substitution", rect_base: "", rect_tax: "", problems: [], doc: null, dirty: true, busy: false,
    };
  }

  function editorFrom(doc: SalesDocument): Editor {
    return {
      id: doc.id, kind: doc.kind, status: doc.status, customer_id: doc.customer_id === null ? "" : String(doc.customer_id), issue_date: doc.issue_date ?? "", operation_date: doc.operation_date ?? "", due_date: doc.due_date ?? "",
      withholding: comma(doc.withholding_percent), requested: doc.recipient_requested_tax_id, art42: doc.art_4_2_declared,
      lines: doc.lines.map((l) => ({ description: l.description, quantity: comma(l.quantity), unit_price: comma(l.unit_price), discount_percent: fractionToPercent(l.discount), tax_percent: l.tax_percent, qualification: l.exemption?.qualification ?? "", exemption_text: l.exemption?.description ?? "" })),
      rect_kind: doc.rectification?.kind ?? "R1", rect_method: doc.rectification?.method ?? "substitution", rect_base: comma(doc.rectification?.rectified_base), rect_tax: comma(doc.rectification?.rectified_tax),
      problems: [], doc, dirty: false, busy: false,
    };
  }

  async function editDraft(id: number) {
    const doc = await run(() => api.salesDocument(id));
    if (doc) {
      s.editor = editorFrom(doc);
      s.detail = null;
    }
    await reloadList();
    ctx.render();
  }

  async function reloadList() {
    try {
      s.docs = await api.salesDocuments({ kind: s.filter.kind || null, status: s.filter.status || null, include_superseded: s.filter.superseded });
    } catch (error) {
      s.message = { kind: "error", text: salesErrorText(error) };
    }
  }

  /** Traduce el editor al formato del backend; `null` si hay problemas (quedan anotados). */
  function draftInput(e: Editor): SalesDraftIn | null {
    const { problems, out } = lineProblems(e.lines);
    const dates: Array<[string, string]> = [["fecha de expedición", e.issue_date], ["fecha de la operación", e.operation_date], ["vencimiento", e.due_date]];
    for (const [name, value] of dates) if (value.trim() && !isIsoDate(value.trim())) problems.push(`La ${name} debe tener el formato AAAA-MM-DD`);
    let withholding: string | null = null;
    if (e.withholding.trim()) {
      withholding = parseDecimalInput(e.withholding);
      if (withholding === null) problems.push("La retención no es un porcentaje");
    }
    let base: string | null = null;
    let tax: string | null = null;
    if (e.kind === "corrective_invoice" && e.rect_method === "substitution") {
      if (e.rect_base.trim()) base = parseDecimalInput(e.rect_base, { negative: true });
      if (e.rect_tax.trim()) tax = parseDecimalInput(e.rect_tax, { negative: true });
      if ((e.rect_base.trim() && base === null) || (e.rect_tax.trim() && tax === null)) problems.push("La base o la cuota rectificadas no son importes");
    }
    e.problems = problems;
    if (problems.length) return null;
    return {
      kind: e.kind,
      customer_id: e.customer_id ? Number(e.customer_id) : null,
      lines: out,
      issue_date: e.issue_date.trim() || null,
      operation_date: e.operation_date.trim() || null,
      due_date: e.due_date.trim() || null,
      withholding_percent: withholding,
      recipient_requested_tax_id: e.requested,
      art_4_2_declared: e.art42,
      rectification: e.kind === "corrective_invoice" ? { kind: e.rect_kind, method: e.rect_method, rectified_base: base, rectified_tax: tax } : null,
    };
  }

  /** Guarda el borrador y vuelve a leerlo: los totales que se muestran son los del backend. */
  async function saveEditor(e: Editor): Promise<boolean> {
    const input = draftInput(e);
    if (!input) {
      ctx.render();
      return false;
    }
    e.busy = true;
    const id = await run(async () => {
      if (e.id === null) e.id = await api.salesCreateDraft(input);
      else await api.salesUpdateDraft(e.id, input);
      return e.id;
    });
    e.busy = false;
    if (id === undefined) return false;
    const doc = await run(() => api.salesDocument(id));
    if (doc) {
      const keepMessage = s.message;
      s.editor = editorFrom(doc);
      s.message = keepMessage;
    }
    await reloadList();
    return doc !== undefined;
  }

  function editorView(e: Editor): HTMLElement {
    const mark = () => (e.dirty = true);
    const corrective = e.kind === "corrective_invoice";
    const totals = e.doc?.totals ?? null;
    const issueBlocked = isInvoiceKind(e.kind) && blocksInvoices();
    return h(
      "div",
      {},
      sectionHeader(e.id === null ? `Nuevo ${KIND_LABEL[e.kind].toLowerCase()}` : `Borrador: ${documentTitle(e.doc ?? { kind: e.kind, number: null, revision: 1, id: e.id, status: "draft" })}`),
      problemList(e.problems),
      h(
        "div",
        { class: "row" },
        selectField("Cliente", customerOptions(), () => e.customer_id, (v) => ((e.customer_id = v), mark()), "ed:customer"),
        textField("Fecha de expedición (AAAA-MM-DD)", () => e.issue_date, (v) => ((e.issue_date = v), mark()), { key: "ed:issue", hint: "Vacía: se usa la de hoy al emitir.", placeholder: today() }),
        textField("Fecha de la operación", () => e.operation_date, (v) => ((e.operation_date = v), mark()), { key: "ed:operation" }),
        textField("Vencimiento", () => e.due_date, (v) => ((e.due_date = v), mark()), { key: "ed:due" }),
        textField("Retención (%)", () => e.withholding, (v) => ((e.withholding = v), mark()), { key: "ed:withholding", inputmode: "decimal" }),
      ),
      isInvoiceKind(e.kind)
        ? h(
            "div",
            { class: "field-group" },
            checkField("El destinatario ha pedido que conste su NIF", () => e.requested, (v) => ((e.requested = v), mark()), "ed:requested"),
            checkField("Factura simplificada con declaración del art. 4.2", () => e.art42, (v) => ((e.art42 = v), mark()), "ed:art42"),
          )
        : null,
      corrective
        ? h(
            "div",
            { class: "row" },
            selectField("Causa de la rectificación", (Object.keys(RECTIFICATION_KIND_LABEL) as SalesRectificationKind[]).map((k): [string, string] => [k, RECTIFICATION_KIND_LABEL[k]]), () => e.rect_kind, (v) => ((e.rect_kind = v as SalesRectificationKind), mark()), "ed:rect-kind"),
            selectField("Método", (Object.keys(RECTIFICATION_METHOD_LABEL) as SalesRectificationMethod[]).map((k): [string, string] => [k, RECTIFICATION_METHOD_LABEL[k]]), () => e.rect_method, (v) => ((e.rect_method = v as SalesRectificationMethod), mark()), "ed:rect-method", () => ctx.render()),
            e.rect_method === "substitution" ? textField("Base rectificada (€)", () => e.rect_base, (v) => ((e.rect_base = v), mark()), { key: "ed:rect-base", inputmode: "decimal" }) : null,
            e.rect_method === "substitution" ? textField("Cuota rectificada (€)", () => e.rect_tax, (v) => ((e.rect_tax = v), mark()), { key: "ed:rect-tax", inputmode: "decimal" }) : null,
          )
        : null,
      sectionHeader("Líneas"),
      linesEditor(e.lines, "ed", mark),
      h(
        "div",
        { class: "row" },
        selectField("Añadir del catálogo", [["", "Elija un producto"], ...s.products.filter((p) => !p.retired).map((p): [string, string] => [String(p.id), `${p.description} (${formatMoney(p.unit_price)})`])], () => "", () => {}, "ed:product"),
        h(
          "div",
          { class: "field" },
          h(
            "button",
            {
              type: "button",
              onclick: () =>
                void (async () => {
                  const select = document.querySelector<HTMLSelectElement>('[data-focus-key="ventas:ed:product"]');
                  const product = Number(select?.value);
                  if (!product) return say("error", "Elija primero un producto del catálogo.");
                  if (!(await saveEditor(e)) || s.editor?.id == null) return;
                  const saved = s.editor;
                  await attempt(() => api.salesAddLineFromProduct(saved.id!, product, "1", null));
                  await editDraft(saved.id!);
                })(),
            },
            "Añadir producto (guarda el borrador)",
          ),
        ),
      ),
      sectionHeader("Totales"),
      totals ? totalsView(totals, e.dirty) : h("p", { class: "meta" }, NO_TOTALS),
      actionBar(
        issueBlocked ? "Emitir está bloqueado hasta resolver el aviso de Verifactu." : e.dirty ? "Hay cambios sin guardar: los totales mostrados son los del último guardado." : null,
        h("button", { class: "primary", disabled: e.busy, "data-focus-key": "ventas:ed:save", onclick: () => void saveEditor(e).then((ok) => ok && say("ok", "Borrador guardado.")) }, "Guardar borrador"),
        h(
          "button",
          {
            disabled: e.busy || issueBlocked,
            "data-focus-key": "ventas:ed:issue",
            onclick: () =>
              void (async () => {
                if (!(await saveEditor(e)) || s.editor?.id == null) return;
                await issueDocument(s.editor.id, s.editor.kind);
              })(),
          },
          "Guardar y emitir…",
        ),
        h("button", { onclick: () => ((s.editor = null), ctx.render()) }, "Cerrar editor"),
      ),
    );
  }

  function totalsView(totals: NonNullable<SalesDocument["totals"]>, stale = false): HTMLElement {
    const money = (v: string) => formatMoney(v, totals.currency);
    return h(
      "div",
      { class: "sales-totals" },
      stale ? h("p", { class: "meta", role: "status" }, "Calculados por NestAdmin al guardar por última vez.") : null,
      dataTable(
        [{ label: "Tipo de IVA" }, { label: "Concepto" }, { label: "Base", numeric: true }, { label: "Cuota", numeric: true }],
        totals.breakdown.map((b) => [`${formatNumber(b.tax_percent)} %${b.qualification ? ` · ${b.qualification}` : ""}`, b.descriptions.join(", "), money(b.base), money(b.tax)]),
      ),
      h(
        "dl",
        { class: "sales-sum" },
        h("dt", {}, "Base imponible"), h("dd", {}, money(totals.base_total)),
        h("dt", {}, "Cuota de IVA"), h("dd", {}, money(totals.tax_total)),
        h("dt", {}, "Total"), h("dd", {}, h("strong", {}, money(totals.total))),
        totals.withholding !== "0" && totals.withholding !== "0.00" ? h("dt", {}, "Retención") : null,
        totals.withholding !== "0" && totals.withholding !== "0.00" ? h("dd", {}, `-${money(totals.withholding)}`) : null,
        h("dt", {}, "A pagar"), h("dd", {}, h("strong", {}, money(totals.amount_due))),
      ),
    );
  }

  // ---- Documentos: detalle y acciones explícitas ----

  async function openDetail(id: number, render = true) {
    try {
      const doc = await api.salesDocument(id);
      const [links, payments, collection] = await Promise.all([
        api.salesLinks(id),
        isInvoiceKind(doc.kind) && doc.status !== "draft" ? api.salesPayments(id) : Promise.resolve([] as SalesPayment[]),
        isInvoiceKind(doc.kind) && doc.status !== "draft" ? api.salesCollection(id) : Promise.resolve(null),
      ]);
      let logoUrl: string | null = null;
      if (doc.issuer?.logo_sha256) {
        try {
          logoUrl = URL.createObjectURL(new Blob([await api.salesLogoByHash(doc.issuer.logo_sha256)]));
        } catch {
          logoUrl = null;
        }
      }
      if (s.detail?.logoUrl) URL.revokeObjectURL(s.detail.logoUrl);
      s.detail = { doc, links, payments, collection, logoUrl, pay: s.detail?.doc.id === id ? s.detail.pay : { date: today(), amount: "", method: "transfer" } };
      s.editor = null;
    } catch (error) {
      s.message = { kind: "error", text: salesErrorText(error) };
    }
    if (render) ctx.render();
  }

  async function issueDocument(id: number, kind: SalesKind) {
    if (!(await ctx.ask(issueConfirmText({ kind }, today())))) return;
    const doc = await run(() => api.salesIssue(id, null), "Documento emitido.");
    await refreshSetup();
    await reloadList();
    if (doc) {
      s.editor = null;
      await openDetail(doc.id);
    } else ctx.render();
  }

  async function perform(doc: SalesDocument, action: SalesAction) {
    const reopen = async (message: string, work: () => Promise<unknown>) => {
      const ok = await run(work, message);
      await refreshSetup();
      await reloadList();
      await openDetail(doc.id);
      return ok;
    };
    switch (action) {
      case "edit":
        return editDraft(doc.id);
      case "delete":
        if (!(await ctx.ask(deleteDraftConfirmText(doc)))) return;
        if (await attempt(() => api.salesDeleteDraft(doc.id), "Borrador eliminado.")) s.detail = null;
        await reloadList();
        ctx.render();
        return;
      case "issue":
        return issueDocument(doc.id, doc.kind);
      case "void":
        if (!(await ctx.ask(voidConfirmText(doc)))) return;
        await reopen("Factura anulada. Se ha generado su registro de anulación.", () => api.salesVoid(doc.id));
        return;
      case "revise": {
        const id = await run(() => api.salesRevise(doc.id), "Borrador de revisión creado.");
        await reloadList();
        if (id !== undefined) await editDraft(id);
        else ctx.render();
        return;
      }
      case "convert_invoice":
      case "convert_delivery_note": {
        const to: SalesKind = action === "convert_invoice" ? "invoice" : "delivery_note";
        if (!(await ctx.ask(convertConfirmText(to, 1)))) return;
        const id = await run(() => api.salesConvert([doc.id], to), "Borrador creado. Revíselo antes de emitir.");
        await reloadList();
        if (id !== undefined) await editDraft(id);
        else ctx.render();
        return;
      }
      case "corrective":
        return; // Se gestiona con el formulario de la ficha.
      case "payments":
        return;
      default: {
        const to = ACTION_TARGET[action];
        if (!to) return;
        if (!(await ctx.ask(`${ACTION_LABEL[action]}: ${documentTitle(doc)}.\n\n¿Continuar?`))) return;
        await reopen(`${documentTitle(doc)}: ${STATUS_LABEL[to].toLowerCase()}.`, () => api.salesTransition(doc.id, to));
      }
    }
  }

  const correctiveForm = { kind: "R1" as SalesRectificationKind, method: "substitution" as SalesRectificationMethod };

  function detailView(d: Detail): HTMLElement {
    const doc = d.doc;
    const actions = allowedActions(doc);
    const invoicing = isInvoiceKind(doc.kind);
    const blocked = invoicing && blocksInvoices();
    const direct = actions.filter((a) => a !== "payments" && a !== "corrective");
    return h(
      "div",
      {},
      h("p", {}, h("button", { class: "link", "data-focus-key": "ventas:back", onclick: () => ((s.detail = null), ctx.render()) }, "← Volver a la lista")),
      sectionHeader(documentTitle(doc), statusChip(STATUS_LABEL[doc.status], statusTone(doc.status))),
      doc.superseded_by !== null ? h("p", { class: "meta" }, `Sustituido por la revisión del documento ${doc.superseded_by}. Se conserva para consulta.`) : null,
      h(
        "div",
        { class: "card" },
        d.logoUrl ? h("img", { src: d.logoUrl, alt: `Logotipo de ${doc.issuer?.party.name ?? "la empresa"}`, class: "sales-logo" }) : null,
        h("p", {}, h("strong", {}, "Emisor: "), doc.issuer ? `${doc.issuer.party.name ?? "Sin nombre"} · ${doc.issuer.party.tax_id ?? "Sin NIF"}` : "Se fijará al emitir"),
        h("p", {}, h("strong", {}, "Cliente: "), doc.recipient ? `${doc.recipient.name ?? ""} · ${doc.recipient.tax_id ?? "Sin NIF"}` : customerName(doc.customer_id)),
        h("p", { class: "meta" }, `Expedición: ${formatDate(doc.issue_date)} · Operación: ${formatDate(doc.operation_date)} · Vencimiento: ${formatDate(doc.due_date)}`),
        doc.rectification ? h("p", { class: "meta" }, `${RECTIFICATION_KIND_LABEL[doc.rectification.kind]} · ${RECTIFICATION_METHOD_LABEL[doc.rectification.method]}${doc.rectifies_id ? ` · rectifica el documento ${doc.rectifies_id}` : ""}`) : null,
        doc.recurring_template_id ? h("p", { class: "meta" }, `Generado por la plantilla recurrente ${doc.recurring_template_id} (periodo ${doc.recurring_period}).`) : null,
      ),
      dataTable(
        [{ label: "Descripción" }, { label: "Cantidad", numeric: true }, { label: "Precio", numeric: true }, { label: "Dto.", numeric: true }, { label: "IVA", numeric: true }, { label: "Importe", numeric: true }],
        doc.lines.map((l) => [
          h("span", {}, l.description, l.exemption ? h("span", { class: "meta" }, ` — ${exemptionText(l.exemption)}`) : null),
          formatNumber(l.quantity),
          formatMoney(l.unit_price, doc.currency),
          l.discount === "0" ? "—" : `${fractionToPercent(l.discount)} %`,
          `${formatNumber(l.tax_percent)} %`,
          l.amount ? formatMoney(l.amount, doc.currency) : "—",
        ]),
      ),
      doc.totals ? totalsView(doc.totals) : h("p", { class: "meta" }, NO_TOTALS),
      d.links.length
        ? h("div", {}, sectionHeader("Documentos vinculados"), h("ul", {}, ...d.links.map((l) => h("li", {}, h("button", { class: "link", onclick: () => void openDetail(l.from_id === doc.id ? l.to_id : l.from_id) }, `Documento ${l.from_id === doc.id ? l.to_id : l.from_id}`), ` (${l.relation === "converted" ? "conversión" : l.relation === "rectifies" ? "rectificación" : l.relation === "revision" ? "revisión" : l.relation}, ${l.from_id === doc.id ? "destino" : "origen"})`))))
        : null,
      invoicing && d.collection ? collectionView(d) : null,
      actions.includes("corrective") ? correctiveView(doc) : null,
      actionBar(
        blocked && (actions.includes("issue") || actions.includes("void")) ? "Emitir y anular facturas están bloqueados hasta resolver el aviso de Verifactu." : null,
        ...direct.map((a) =>
          h(
            "button",
            { class: a === "issue" ? "primary" : a === "void" || a === "delete" ? "danger" : undefined, disabled: blocked && (a === "issue" || a === "void"), "data-focus-key": `ventas:action:${a}`, onclick: () => void perform(doc, a) },
            a === "edit" || a === "revise" ? ACTION_LABEL[a] : `${ACTION_LABEL[a]}…`,
          ),
        ),
      ),
    );
  }

  function collectionView(d: Detail): HTMLElement {
    const c = d.collection!;
    const canPay = allowedActions(d.doc).includes("payments");
    return h(
      "div",
      {},
      sectionHeader("Cobros", statusChip(COLLECTION_LABEL[c.state], collectionTone(c.state))),
      h("p", {}, `A cobrar ${formatMoney(c.payable)} · cobrado ${formatMoney(c.paid)} · pendiente ${formatMoney(c.outstanding)}`),
      d.payments.length
        ? dataTable(
            [{ label: "Fecha" }, { label: "Medio" }, { label: "Importe", numeric: true }, { label: "Acciones" }],
            d.payments.map((p) => [
              formatDate(p.date),
              PAYMENT_METHOD_LABEL[p.method],
              formatMoney(p.amount),
              h(
                "button",
                {
                  onclick: () =>
                    void (async () => {
                      if (!(await ctx.ask(`Se quitará el cobro de ${formatMoney(p.amount)} del ${formatDate(p.date)}.\n\n¿Quitarlo?`))) return;
                      await run(() => api.salesRemovePayment(p.id), "Cobro eliminado.");
                      await openDetail(d.doc.id);
                    })(),
                },
                `Quitar el cobro del ${formatDate(p.date)}`,
              ),
            ]),
          )
        : h("p", { class: "meta" }, "Todavía no hay cobros registrados."),
      canPay
        ? h(
            "div",
            { class: "row" },
            textField("Fecha del cobro (AAAA-MM-DD)", () => d.pay.date, (v) => (d.pay.date = v), { key: "pay:date" }),
            textField("Importe (€)", () => d.pay.amount, (v) => (d.pay.amount = v), { key: "pay:amount", inputmode: "decimal" }),
            selectField("Medio", (Object.keys(PAYMENT_METHOD_LABEL) as SalesPaymentMethod[]).map((m): [string, string] => [m, PAYMENT_METHOD_LABEL[m]]), () => d.pay.method, (v) => (d.pay.method = v as SalesPaymentMethod), "pay:method"),
            h(
              "div",
              { class: "field" },
              h(
                "button",
                {
                  "data-focus-key": "ventas:pay:add",
                  onclick: () =>
                    void (async () => {
                      const amount = parseDecimalInput(d.pay.amount);
                      if (amount === null || !isIsoDate(d.pay.date.trim())) return say("error", "Indique una fecha AAAA-MM-DD y un importe válido.");
                      await run(() => api.salesAddPayment(d.doc.id, d.pay.date.trim(), amount, d.pay.method), "Cobro registrado.");
                      d.pay.amount = "";
                      await openDetail(d.doc.id);
                    })(),
                },
                "Registrar cobro",
              ),
            ),
          )
        : null,
    );
  }

  function correctiveView(doc: SalesDocument): HTMLElement {
    return h(
      "div",
      {},
      sectionHeader("Rectificar esta factura"),
      h("p", { class: "meta" }, "Una factura emitida no se edita: se corrige con una rectificativa, que se crea como borrador para que la revise antes de emitirla."),
      h(
        "div",
        { class: "row" },
        selectField("Causa", (Object.keys(RECTIFICATION_KIND_LABEL) as SalesRectificationKind[]).map((k): [string, string] => [k, RECTIFICATION_KIND_LABEL[k]]), () => correctiveForm.kind, (v) => (correctiveForm.kind = v as SalesRectificationKind), "corr:kind"),
        selectField("Método", (Object.keys(RECTIFICATION_METHOD_LABEL) as SalesRectificationMethod[]).map((k): [string, string] => [k, RECTIFICATION_METHOD_LABEL[k]]), () => correctiveForm.method, (v) => (correctiveForm.method = v as SalesRectificationMethod), "corr:method"),
        h(
          "div",
          { class: "field" },
          h(
            "button",
            {
              "data-focus-key": "ventas:corr:create",
              onclick: () =>
                void (async () => {
                  const id = await run(() => api.salesCreateCorrective(doc.id, correctiveForm.kind, correctiveForm.method), "Borrador de rectificativa creado. Revíselo antes de emitir.");
                  await reloadList();
                  if (id !== undefined) await editDraft(id);
                  else ctx.render();
                })(),
            },
            ACTION_LABEL.corrective,
          ),
        ),
      ),
    );
  }

  function documentsTab(): HTMLElement {
    if (s.editor) return editorView(s.editor);
    if (s.detail) return detailView(s.detail);
    return listView();
  }

  // ---- Clientes ----

  function customersTab(): HTMLElement {
    const f = s.customerForm;
    const list = s.customers.filter((c) => s.showRetiredCustomers || !c.retired);
    return h(
      "div",
      {},
      sectionHeader("Clientes", h("button", { class: "primary", "data-focus-key": "ventas:customer:new", onclick: () => ((s.customerForm = { id: null, name: "", tax_id: "", address: "", email: "" }), ctx.render()) }, "Nuevo cliente")),
      checkField("Mostrar clientes retirados", () => s.showRetiredCustomers, (v) => ((s.showRetiredCustomers = v), ctx.render()), "customers:retired"),
      f
        ? h(
            "div",
            { class: "card" },
            h("h4", {}, f.id === null ? "Nuevo cliente" : "Editar cliente"),
            h(
              "div",
              { class: "row" },
              textField("Nombre", () => f.name, (v) => (f.name = v), { key: "customer:name" }),
              textField("NIF", () => f.tax_id, (v) => (f.tax_id = v), { key: "customer:tax" }),
            ),
            h(
              "div",
              { class: "row" },
              textField("Dirección", () => f.address, (v) => (f.address = v), { key: "customer:address" }),
              textField("Correo", () => f.email, (v) => (f.email = v), { key: "customer:email", type: "email" }),
            ),
            h(
              "div",
              { class: "row-actions" },
              h(
                "button",
                {
                  class: "primary",
                  onclick: () =>
                    void (async () => {
                      const data = { name: f.name.trim(), tax_id: f.tax_id.trim() || null, address: f.address.trim() || null, email: f.email.trim() || null };
                      if (!data.name) return say("error", "Falta el nombre del cliente.");
                      const ok = await attempt(() => (f.id === null ? api.salesCreateCustomer(data) : api.salesUpdateCustomer(f.id, data)), "Cliente guardado.");
                      if (ok) s.customerForm = null;
                      await reload();
                    })(),
                },
                "Guardar cliente",
              ),
              h("button", { onclick: () => ((s.customerForm = null), ctx.render()) }, "Cancelar"),
            ),
          )
        : null,
      list.length === 0
        ? emptyState("Todavía no hay clientes.")
        : dataTable(
            [{ label: "Nombre" }, { label: "NIF" }, { label: "Correo" }, { label: "Estado" }, { label: "Acciones" }],
            list.map((c) => [
              c.data.name,
              c.data.tax_id ?? "Sin NIF",
              c.data.email ?? "",
              c.retired ? statusChip("Retirado", "neutral") : statusChip("Activo", "ok"),
              c.retired
                ? null
                : h(
                    "div",
                    { class: "row-actions" },
                    h("button", { onclick: () => ((s.customerForm = { id: c.id, name: c.data.name, tax_id: c.data.tax_id ?? "", address: c.data.address ?? "", email: c.data.email ?? "" }), ctx.render()) }, `Editar ${c.data.name}`),
                    h(
                      "button",
                      {
                        onclick: () =>
                          void (async () => {
                            if (!(await ctx.ask(`Se retirará a ${c.data.name} de la lista. Sus documentos no cambian.\n\n¿Retirar?`))) return;
                            await run(() => api.salesRetireCustomer(c.id), "Cliente retirado.");
                            await reload();
                          })(),
                      },
                      `Retirar ${c.data.name}`,
                    ),
                  ),
            ]),
          ),
    );
  }

  // ---- Productos ----

  function productsTab(): HTMLElement {
    const f = s.productForm;
    const list = s.products.filter((p) => s.showRetiredProducts || !p.retired);
    return h(
      "div",
      {},
      sectionHeader("Productos y servicios", h("button", { class: "primary", "data-focus-key": "ventas:product:new", onclick: () => ((s.productForm = { id: null, description: "", unit_price: "", tax_percent: "21", qualification: "", exemption_text: "" }), ctx.render()) }, "Nuevo producto")),
      checkField("Mostrar productos retirados", () => s.showRetiredProducts, (v) => ((s.showRetiredProducts = v), ctx.render()), "products:retired"),
      f
        ? h(
            "div",
            { class: "card" },
            h("h4", {}, f.id === null ? "Nuevo producto" : "Editar producto"),
            h(
              "div",
              { class: "row" },
              textField("Descripción", () => f.description, (v) => (f.description = v), { key: "product:description" }),
              textField("Precio unitario (€)", () => f.unit_price, (v) => (f.unit_price = v), { key: "product:price", inputmode: "decimal" }),
              selectField("IVA (%)", TAX_PERCENTS.map((p): [string, string] => [p, p]), () => f.tax_percent, (v) => (f.tax_percent = v), "product:tax", () => ctx.render()),
            ),
            f.tax_percent === "0"
              ? h(
                  "div",
                  { class: "row" },
                  selectField("Causa del IVA 0 %", [["", "Elija una causa"], ...ZERO_RATE_QUALIFICATIONS.map((q): [string, string] => [q, QUALIFICATION_LABEL[q]])], () => f.qualification, (v) => (f.qualification = v as SalesQualification | ""), "product:qualification"),
                  textField("Texto en la factura (opcional)", () => f.exemption_text, (v) => (f.exemption_text = v), { key: "product:exemption" }),
                )
              : null,
            h(
              "div",
              { class: "row-actions" },
              h(
                "button",
                {
                  class: "primary",
                  onclick: () =>
                    void (async () => {
                      const price = parseDecimalInput(f.unit_price, { negative: true });
                      if (!f.description.trim() || price === null) return say("error", "Indique la descripción y un precio válido.");
                      if (f.tax_percent === "0" && !f.qualification) return say("error", "Con IVA 0 % indique la causa.");
                      const product = { description: f.description.trim(), unit_price: price, tax_percent: f.tax_percent, exemption: f.tax_percent === "0" && f.qualification ? { qualification: f.qualification, description: f.exemption_text.trim() || null } : null };
                      const ok = await attempt(() => (f.id === null ? api.salesCreateProduct(product) : api.salesUpdateProduct(f.id, product)), "Producto guardado.");
                      if (ok) s.productForm = null;
                      await reload();
                    })(),
                },
                "Guardar producto",
              ),
              h("button", { onclick: () => ((s.productForm = null), ctx.render()) }, "Cancelar"),
            ),
          )
        : null,
      list.length === 0
        ? emptyState("Todavía no hay productos.")
        : dataTable(
            [{ label: "Descripción" }, { label: "Precio", numeric: true }, { label: "IVA", numeric: true }, { label: "Estado" }, { label: "Acciones" }],
            list.map((p) => [
              h("span", {}, p.description, p.exemption ? h("span", { class: "meta" }, ` — ${exemptionText(p.exemption)}`) : null),
              formatMoney(p.unit_price),
              `${formatNumber(p.tax_percent)} %`,
              p.retired ? statusChip("Retirado", "neutral") : statusChip("Activo", "ok"),
              p.retired
                ? null
                : h(
                    "div",
                    { class: "row-actions" },
                    h("button", { onclick: () => ((s.productForm = { id: p.id, description: p.description, unit_price: comma(p.unit_price), tax_percent: p.tax_percent, qualification: p.exemption?.qualification ?? "", exemption_text: p.exemption?.description ?? "" }), ctx.render()) }, `Editar ${p.description}`),
                    h(
                      "button",
                      {
                        onclick: () =>
                          void (async () => {
                            if (!(await ctx.ask(`Se retirará «${p.description}» del catálogo. Los documentos ya creados no cambian.\n\n¿Retirar?`))) return;
                            await run(() => api.salesRetireProduct(p.id), "Producto retirado.");
                            await reload();
                          })(),
                      },
                      `Retirar ${p.description}`,
                    ),
                  ),
            ]),
          ),
    );
  }

  // ---- Empresa y logotipo ----

  function companyTab(): HTMLElement {
    const f = s.companyForm;
    const fileInput = h("input", { type: "file", id: "ventas-logo-file", accept: "image/png,image/jpeg", "data-focus-key": "ventas:logo:file" }) as HTMLInputElement;
    return h(
      "div",
      {},
      sectionHeader("Datos de la empresa"),
      h("p", { class: "meta" }, "Estos datos se copian en cada factura al emitirla: un cambio posterior no altera las facturas ya emitidas. El nombre y el NIF son obligatorios para emitir facturas."),
      h(
        "div",
        { class: "card" },
        h(
          "div",
          { class: "row" },
          textField("Nombre o razón social", () => f.name, (v) => (f.name = v), { key: "company:name" }),
          textField("NIF", () => f.tax_id, (v) => (f.tax_id = v), { key: "company:tax" }),
        ),
        h(
          "div",
          { class: "row" },
          textField("Dirección", () => f.address, (v) => (f.address = v), { key: "company:address" }),
          textField("Correo", () => f.email, (v) => (f.email = v), { key: "company:email", type: "email" }),
          textField("IBAN", () => f.iban, (v) => (f.iban = v), { key: "company:iban" }),
        ),
        h(
          "div",
          { class: "row-actions" },
          h(
            "button",
            {
              class: "primary",
              "data-focus-key": "ventas:company:save",
              onclick: () =>
                void (async () => {
                  const clean = (v: string) => v.trim() || null;
                  await run(() => api.salesSaveCompany({ name: clean(f.name), tax_id: clean(f.tax_id), address: clean(f.address), email: clean(f.email), iban: clean(f.iban) }), "Datos de la empresa guardados.");
                  await reload();
                })(),
            },
            "Guardar datos",
          ),
        ),
      ),
      sectionHeader("Logotipo"),
      s.companyLogoUrl ? h("img", { src: s.companyLogoUrl, alt: "Logotipo de la empresa", class: "sales-logo" }) : h("p", { class: "meta" }, "No hay logotipo. Admite PNG o JPEG de hasta 2 MB."),
      h(
        "div",
        { class: "row" },
        h("div", { class: "field" }, h("div", { class: "field-head" }, h("label", { for: "ventas-logo-file" }, "Elegir archivo de logotipo")), fileInput),
        h(
          "div",
          { class: "field" },
          h(
            "button",
            {
              onclick: () =>
                void (async () => {
                  const file = fileInput.files?.[0];
                  if (!file) return say("error", "Elija primero un archivo PNG o JPEG.");
                  if (file.size > 2 * 1024 * 1024) return say("error", "El logotipo supera los 2 MB.");
                  await run(async () => api.salesSetCompanyLogo(new Uint8Array(await file.arrayBuffer()), file.type), "Logotipo guardado.");
                  await reload();
                })(),
            },
            "Guardar logotipo",
          ),
          s.company?.has_logo
            ? h(
                "button",
                {
                  onclick: () =>
                    void (async () => {
                      if (!(await ctx.ask("Se quitará el logotipo de la empresa. Las facturas ya emitidas conservan el suyo.\n\n¿Quitarlo?"))) return;
                      await run(() => api.salesRemoveCompanyLogo(), "Logotipo quitado.");
                      await reload();
                    })(),
                },
                "Quitar logotipo",
              )
            : null,
        ),
      ),
    );
  }

  // ---- Recurrentes ----

  function recurringTab(): HTMLElement {
    const f = s.recurringForm;
    return h(
      "div",
      {},
      sectionHeader(
        "Facturas recurrentes",
        h("button", { onclick: () => ((s.recurringForm = { id: null, customer_id: "", frequency: "monthly", first_day: today(), end: "", lines: [emptyLine()], problems: [] }), ctx.render()) }, "Nueva plantilla"),
        h(
          "button",
          {
            class: "primary",
            "data-focus-key": "ventas:recurring:generate",
            onclick: () =>
              void (async () => {
                if (!(await ctx.ask(generateRecurringConfirmText()))) return;
                const run_ = await run(() => api.salesGenerateRecurring());
                if (run_) s.message = { kind: "ok", text: recurringResultText(run_) };
                await reload();
              })(),
          },
          "Generar borradores",
        ),
      ),
      h("p", { class: "meta" }, "Las plantillas solo generan borradores: nunca emiten por su cuenta. Cada borrador aparece en Documentos para que lo revise y lo emita."),
      f
        ? h(
            "div",
            { class: "card" },
            h("h4", {}, f.id === null ? "Nueva plantilla" : "Editar plantilla"),
            problemList(f.problems),
            h(
              "div",
              { class: "row" },
              selectField("Cliente", customerOptions(false), () => f.customer_id, (v) => (f.customer_id = v), "rec:customer"),
              selectField("Periodicidad", (Object.keys(FREQUENCY_LABEL) as SalesFrequency[]).map((k): [string, string] => [k, FREQUENCY_LABEL[k]]), () => f.frequency, (v) => (f.frequency = v as SalesFrequency), "rec:frequency"),
              textField("Primer día (AAAA-MM-DD)", () => f.first_day, (v) => (f.first_day = v), { key: "rec:first" }),
              textField("Último día (opcional)", () => f.end, (v) => (f.end = v), { key: "rec:end" }),
            ),
            linesEditor(f.lines, "rec", () => {}),
            h(
              "div",
              { class: "row-actions" },
              h(
                "button",
                {
                  class: "primary",
                  onclick: () =>
                    void (async () => {
                      const { problems, out } = lineProblems(f.lines);
                      if (!f.customer_id) problems.push("Elija un cliente");
                      if (!isIsoDate(f.first_day.trim())) problems.push("El primer día debe tener el formato AAAA-MM-DD");
                      if (f.end.trim() && !isIsoDate(f.end.trim())) problems.push("El último día debe tener el formato AAAA-MM-DD");
                      f.problems = problems;
                      if (problems.length) return ctx.render();
                      const template = { customer_id: Number(f.customer_id), lines: out, frequency: f.frequency, first_day: f.first_day.trim(), end: f.end.trim() || null };
                      const ok = await attempt(() => (f.id === null ? api.salesCreateRecurring(template) : api.salesUpdateRecurring(f.id, template)), "Plantilla guardada.");
                      if (ok) s.recurringForm = null;
                      await reload();
                    })(),
                },
                "Guardar plantilla",
              ),
              h("button", { onclick: () => ((s.recurringForm = null), ctx.render()) }, "Cancelar"),
            ),
          )
        : null,
      s.templates.length === 0
        ? emptyState("Todavía no hay plantillas recurrentes.")
        : dataTable(
            [{ label: "Cliente" }, { label: "Periodicidad" }, { label: "Desde" }, { label: "Hasta" }, { label: "Líneas" }, { label: "Estado" }, { label: "Acciones" }],
            s.templates.map((t) => [
              customerName(t.customer_id),
              FREQUENCY_LABEL[t.frequency],
              formatDate(t.first_day),
              t.end ? formatDate(t.end) : "Sin fin",
              t.lines.map((l) => l.description).join(", "),
              t.active ? statusChip("Activa", "ok") : statusChip("Pausada", "neutral"),
              h(
                "div",
                { class: "row-actions" },
                h(
                  "button",
                  {
                    onclick: () =>
                      (s.recurringForm = {
                        id: t.id, customer_id: String(t.customer_id), frequency: t.frequency, first_day: t.first_day, end: t.end ?? "", problems: [],
                        lines: t.lines.map((l) => ({ description: l.description, quantity: comma(l.quantity), unit_price: comma(l.unit_price), discount_percent: fractionToPercent(l.discount), tax_percent: l.tax_percent, qualification: l.exemption?.qualification ?? "", exemption_text: l.exemption?.description ?? "" })),
                      }) && ctx.render(),
                  },
                  `Editar la plantilla de ${customerName(t.customer_id)}`,
                ),
                h(
                  "button",
                  {
                    onclick: () =>
                      void (async () => {
                        await run(() => api.salesSetRecurringActive(t.id, !t.active), t.active ? "Plantilla pausada." : "Plantilla activada.");
                        await reload();
                      })(),
                  },
                  t.active ? `Pausar la plantilla de ${customerName(t.customer_id)}` : `Activar la plantilla de ${customerName(t.customer_id)}`,
                ),
              ),
            ]),
          ),
    );
  }

  // ---- Albaranes recibidos ----

  function receivedTab(): HTMLElement {
    const f = s.receivedForm;
    const r = s.review;
    return h(
      "div",
      {},
      sectionHeader("Albaranes recibidos de proveedores", h("button", { onclick: () => ((s.receivedForm = { supplier_name: "", supplier_tax_id: "", supplier_number: "", date: today() }), ctx.render()) }, "Registrar albarán")),
      f
        ? h(
            "div",
            { class: "card" },
            h(
              "div",
              { class: "row" },
              textField("Proveedor", () => f.supplier_name, (v) => (f.supplier_name = v), { key: "recv:name" }),
              textField("NIF del proveedor", () => f.supplier_tax_id, (v) => (f.supplier_tax_id = v), { key: "recv:tax" }),
              textField("Número del albarán", () => f.supplier_number, (v) => (f.supplier_number = v), { key: "recv:number" }),
              textField("Fecha (AAAA-MM-DD)", () => f.date, (v) => (f.date = v), { key: "recv:date" }),
            ),
            h(
              "div",
              { class: "row-actions" },
              h(
                "button",
                {
                  class: "primary",
                  onclick: () =>
                    void (async () => {
                      if (!f.supplier_name.trim() || !f.supplier_number.trim() || !isIsoDate(f.date.trim())) return say("error", "Indique proveedor, número y una fecha AAAA-MM-DD.");
                      const ok = await attempt(() => api.salesCreateReceivedNote({ supplier_name: f.supplier_name.trim(), supplier_tax_id: f.supplier_tax_id.trim() || null, supplier_number: f.supplier_number.trim(), date: f.date.trim() }), "Albarán registrado.");
                      if (ok) s.receivedForm = null;
                      await reload();
                    })(),
                },
                "Guardar albarán",
              ),
              h("button", { onclick: () => ((s.receivedForm = null), ctx.render()) }, "Cancelar"),
            ),
          )
        : null,
      r
        ? h(
            "div",
            { class: "card" },
            h("h4", {}, "Marcar como revisado"),
            h(
              "div",
              { class: "row" },
              textField("Documento asociado (opcional)", () => r.document, (v) => (r.document = v), { key: "recv:doc" }),
              textField("Factura asociada (opcional)", () => r.invoice, (v) => (r.invoice = v), { key: "recv:inv" }),
            ),
            h(
              "div",
              { class: "row-actions" },
              h(
                "button",
                {
                  class: "primary",
                  onclick: () =>
                    void (async () => {
                      const ok = await attempt(() => api.salesReviewReceivedNote(r.id, r.document.trim() || null, r.invoice.trim() || null), "Albarán revisado.");
                      if (ok) s.review = null;
                      await reload();
                    })(),
                },
                "Confirmar revisión",
              ),
              h("button", { onclick: () => ((s.review = null), ctx.render()) }, "Cancelar"),
            ),
          )
        : null,
      s.received.length === 0
        ? emptyState("No hay albaranes recibidos.")
        : dataTable(
            [{ label: "Proveedor" }, { label: "Número" }, { label: "Fecha" }, { label: "Estado" }, { label: "Referencias" }, { label: "Acciones" }],
            s.received.map((n) => [
              `${n.supplier_name}${n.supplier_tax_id ? ` · ${n.supplier_tax_id}` : ""}`,
              n.supplier_number,
              formatDate(n.date),
              n.status === "reviewed" ? statusChip("Revisado", "ok") : statusChip("Recibido", "info"),
              [n.document_ref, n.invoice_ref].filter(Boolean).join(" · "),
              n.status === "received" ? h("button", { onclick: () => ((s.review = { id: n.id, document: "", invoice: "" }), ctx.render()) }, `Revisar el albarán ${n.supplier_number}`) : null,
            ]),
          ),
    );
  }

  // ---- Registros Verifactu (solo lectura) ----

  function recordsTab(): HTMLElement {
    const chain = s.setup?.chain ?? null;
    return h(
      "div",
      {},
      sectionHeader("Registros Verifactu"),
      h("p", { class: "meta" }, VERIFACTU_READONLY),
      h("div", { class: chain?.state === "broken" ? "notice error" : "notice info", role: chain?.state === "broken" ? "alert" : "status" }, h("span", {}, chainText(chain))),
      s.records.length === 0
        ? emptyState("Todavía no hay registros.", "Se crean al emitir o anular una factura.")
        : dataTable(
            [{ label: "N.º" }, { label: "Tipo" }, { label: "Factura" }, { label: "Fecha" }, { label: "Total", numeric: true }, { label: "Huella" }, { label: "Detalle" }],
            s.records.map((r) => [
              r.id,
              r.kind === "alta" ? "Alta" : "Anulación",
              r.invoice_number,
              r.issue_date,
              r.total ? formatMoney(r.total) : "—",
              h("code", {}, `${r.hash.slice(0, 16)}…`),
              h("details", {}, h("summary", {}, `Ver registro ${r.id}`), h("pre", { class: "sales-json" }, JSON.stringify({ ...(r.record as object), huella_completa: r.hash, huella_anterior: r.previous_hash, generado: r.generated_at }, null, 2))),
            ]),
          ),
    );
  }

  // ---- Vista principal ----

  function view(): HTMLElement {
    const items: Array<{ key: Tab; label: string }> = [
      { key: "documents", label: "Documentos" },
      { key: "customers", label: "Clientes" },
      { key: "products", label: "Productos" },
      { key: "company", label: "Empresa" },
      { key: "recurring", label: "Recurrentes" },
      { key: "received", label: "Albaranes recibidos" },
      { key: "records", label: "Registros Verifactu" },
    ];
    const panels: Record<Tab, () => HTMLElement> = { documents: documentsTab, customers: customersTab, products: productsTab, company: companyTab, recurring: recurringTab, received: receivedTab, records: recordsTab };
    return h(
      "section",
      { class: "page" },
      h("h2", {}, "Ventas"),
      s.message
        ? h("div", { class: `notice ${s.message.kind}`, role: s.message.kind === "error" ? "alert" : "status" }, h("span", {}, s.message.text), h("button", { class: "close", "aria-label": "Cerrar aviso", onclick: () => ((s.message = null), ctx.render()) }, "×"))
        : null,
      setupBanner(),
      tabs({
        label: "Secciones de ventas",
        items,
        active: s.tab,
        panelId: "ventas-panel",
        onSelect: (key) => {
          s.tab = key;
          s.message = null;
          s.editor = null;
          s.detail = null;
          s.customerForm = s.productForm = null;
          s.recurringForm = null;
          s.receivedForm = null;
          s.review = null;
          ctx.render();
          void loadTab().then(() => ctx.render());
        },
      }),
      h("div", { id: "ventas-panel", role: "tabpanel", class: "sales-panel" }, s.loaded || !s.loading ? panels[s.tab]() : h("p", {}, "Cargando…")),
    );
  }

  return { view, enter };
}
