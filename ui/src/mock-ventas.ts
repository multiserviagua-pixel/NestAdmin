// Servidor simulado de la facturación de venta para la vista previa en navegador (E3b). Imita
// los comandos de `crates/desktop/src/ventas.rs` con datos sintéticos (dominio .test, NIF
// inventados) y sus errores con código `[codigo] mensaje` (D204). No decide nada que el backend
// real no decida: los cálculos son una aproximación para ver la interfaz.
//
// Parámetros de la URL para ver los avisos: `?sin-productor` (productor sin configurar hasta
// pulsar «Volver a leer» una vez), `?cadena-rota` (cadena de registros rota). Para la remisión de
// Verifactu (E5): `?verifactu-rechazo` (el siguiente envío rechaza el primer registro),
// `?verifactu-sin-red` (el siguiente envío no obtiene respuesta) y `?verifactu-fault` (el
// siguiente envío recibe un `Fault` de esquema).

import type {
  LetterheadMargins,
  MailAccount,
  SalesSend,
  Task,
  SalesCollection,
  SalesCompany,
  SalesCustomer,
  SalesDocument,
  SalesKind,
  SalesLine,
  SalesLink,
  SalesPayment,
  SalesProduct,
  SalesReceivedNote,
  SalesRecurring,
  SalesStatus,
  SalesTotals,
  VerifactuBatchView,
  VerifactuEnvironment,
  VerifactuPumpReport,
  VerifactuRecord,
  VerifactuRecordView,
  VerifactuStatus,
  VerifactuStatusView,
} from "./types";

import { parseMargins } from "./ventas-text";

const params = new URLSearchParams(typeof location === "undefined" ? "" : location.search);
const TODAY = "2026-11-10";
const fail = (code: string, message: string): never => {
  throw `[${code}] ${message}`;
};

let producerOk = !params.has("sin-productor");
const chainBroken = params.has("cadena-rota");

// ---- Cálculo aproximado en céntimos (el backend real usa decimales exactos) ----

const cents = (value: string): number => Math.round(Number(value) * 100);
const money = (c: number): string => (c / 100).toFixed(2);

function totalsOf(lines: SalesLine[], withholding: string | null): SalesTotals | null {
  if (lines.length === 0) return null;
  const groups = new Map<string, { percent: string; qualification: SalesLine["exemption"]; descriptions: string[]; base: number }>();
  const amounts: string[] = [];
  for (const l of lines) {
    const gross = Number(l.quantity) * Number(l.unit_price) * (1 - Number(l.discount));
    const amount = Math.round(gross * 100);
    amounts.push(money(amount));
    const key = `${l.tax_percent}|${l.exemption?.qualification ?? ""}`;
    const g = groups.get(key) ?? { percent: l.tax_percent, qualification: l.exemption, descriptions: [], base: 0 };
    g.base += amount;
    if (!g.descriptions.includes(l.description)) g.descriptions.push(l.description);
    groups.set(key, g);
  }
  const breakdown = [...groups.values()].map((g) => ({
    tax_percent: g.percent,
    qualification: g.qualification?.qualification ?? null,
    descriptions: g.descriptions,
    base: money(g.base),
    tax: money(Math.round((g.base * Number(g.percent)) / 100)),
  }));
  const base = breakdown.reduce((s, b) => s + cents(b.base), 0);
  const tax = breakdown.reduce((s, b) => s + cents(b.tax), 0);
  const held = Math.round((base * Number(withholding ?? "0")) / 100);
  return { currency: "EUR", line_amounts: amounts, breakdown, base_total: money(base), tax_total: money(tax), total: money(base + tax), withholding: money(held), amount_due: money(base + tax - held) };
}

// ---- Estado en memoria ----

const DEFAULT_MARGINS: LetterheadMargins = { top_mm: 35, right_mm: 20, bottom_mm: 25, left_mm: 20 };
let company: SalesCompany | null = { name: "Taller Ejemplo, S.L.", tax_id: "B12345674", address: "Calle Falsa 1, 28000 Madrid", email: "facturas@taller-ejemplo.test", iban: "ES7921000813610123456789", has_logo: false, logo_mime: null, has_letterhead: false, letterhead_margins: DEFAULT_MARGINS };
let letterhead = false;
let logo: Uint8Array | null = null;
let nextId = 100;
const id = () => ++nextId;

const customers: SalesCustomer[] = [
  { id: 1, data: { name: "Alfa Suministros S.A.", tax_id: "A58818501", address: "Avenida Prueba 5, 08001 Barcelona", email: "compras@alfa.test" }, retired: false },
  { id: 2, data: { name: "Beta Servicios S.L.", tax_id: "B84532127", address: null, email: "admin@beta.test" }, retired: false },
];
const products: SalesProduct[] = [
  { id: 1, description: "Hora de mantenimiento", unit_price: "45", tax_percent: "21", exemption: null, retired: false },
  { id: 2, description: "Formación (exenta)", unit_price: "300", tax_percent: "0", exemption: { qualification: "E1", description: "Exento art. 20" }, retired: false },
];
const received: SalesReceivedNote[] = [];
const documents = new Map<number, SalesDocument>();
const links: SalesLink[] = [];
const payments = new Map<number, SalesPayment & { document_id: number }>();
const templates: SalesRecurring[] = [];
const records: VerifactuRecord[] = [];
const generatedKeys = new Set<string>();
const sequences = new Map<string, number>();

function newDraft(over: Partial<SalesDocument> & { kind: SalesKind }): SalesDocument {
  const doc: SalesDocument = {
    id: id(), status: "draft", series_year: null, sequence: null, revision: 1, number: null, superseded_by: null, customer_id: null, issue_date: null, operation_date: null, due_date: null,
    withholding_percent: null, currency: "EUR", issuer: null, recipient: null, totals: null, lines: [], rectifies_id: null, rectification: null, recipient_requested_tax_id: false,
    art_4_2_declared: false, recurring_template_id: null, recurring_period: null, created_at: Date.now(), updated_at: Date.now(), ...over,
  };
  doc.totals = totalsOf(doc.lines, doc.withholding_percent);
  documents.set(doc.id, doc);
  return doc;
}

const get = (docId: number): SalesDocument => documents.get(docId) ?? fail("not_found", `documento ${docId} inexistente`);
const lineFrom = (l: any): SalesLine => {
  if (!String(l.description ?? "").trim()) fail("invalid_input", "datos no válidos: description: falta");
  if (Number.isNaN(Number(l.quantity)) || Number.isNaN(Number(l.unit_price))) fail("invalid_input", "datos no válidos: quantity: importe o porcentaje no válido");
  if (!["0", "4", "10", "21"].includes(String(l.tax_percent))) fail("invalid_input", "datos no válidos: tax_percent: tipo de IVA no admitido");
  if (l.tax_percent === "0" && !l.exemption) fail("invalid_input", "datos no válidos: exemption: falta la causa con IVA 0 %");
  return { description: l.description, quantity: String(l.quantity), unit_price: String(l.unit_price), discount: String(l.discount ?? "0"), tax_percent: String(l.tax_percent), exemption: l.exemption ?? null, amount: null, product_id: l.product_id ?? null };
};
const requireDraft = (d: SalesDocument) => {
  if (d.status !== "draft") fail("invalid_transition", `un documento en estado ${d.status} no se puede editar`);
};
const isInvoice = (k: SalesKind) => k.endsWith("invoice");

function fillDraft(d: SalesDocument, input: any) {
  d.customer_id = input.customer_id ?? null;
  d.lines = (input.lines ?? []).map(lineFrom);
  d.issue_date = input.issue_date || null;
  d.operation_date = input.operation_date || null;
  d.due_date = input.due_date || null;
  d.withholding_percent = input.withholding_percent || null;
  d.recipient_requested_tax_id = Boolean(input.recipient_requested_tax_id);
  d.art_4_2_declared = Boolean(input.art_4_2_declared);
  if (input.rectification) d.rectification = { kind: input.rectification.kind, method: input.rectification.method, rectified_base: input.rectification.rectified_base ?? null, rectified_tax: input.rectification.rectified_tax ?? null };
  d.totals = totalsOf(d.lines, d.withholding_percent);
  d.updated_at = Date.now();
}

function guardInvoicing() {
  if (!producerOk) fail("verifactu_not_configured", "falta configurar el productor del software (nombre y NIF); no se puede emitir ni anular facturas");
  if (chainBroken) fail("verifactu_chain_broken", "cadena de registros Verifactu: el registro 2 no coincide con su huella");
}

function addRecord(kind: "alta" | "anulacion", d: SalesDocument) {
  const previous = records[records.length - 1] ?? null;
  const hash = (0x9e3779b1 * (records.length + 7)).toString(16).toUpperCase().padStart(8, "0").repeat(8).slice(0, 64);
  const [y, m, day] = (d.issue_date ?? TODAY).split("-");
  records.push({
    id: records.length + 1, kind, document_id: d.id, issuer_tax_id: company?.tax_id ?? "", invoice_number: d.number ?? "", issue_date: `${day}-${m}-${y}`,
    invoice_type: kind === "alta" ? (d.rectification ? "R1" : d.kind === "simplified_invoice" ? "F2" : "F1") : null, total: kind === "alta" ? d.totals?.total ?? null : null,
    tax_total: kind === "alta" ? d.totals?.tax_total ?? null : null, previous_id: previous?.id ?? null, previous_hash: previous?.hash ?? null, generated_at: `${TODAY}T10:00:00+01:00`, hash,
    record: { TipoRegistro: kind, NumFactura: d.number, Huella: hash }, created_at: Date.now(),
  });
}

function issue(d: SalesDocument, date: string | null): SalesDocument {
  requireDraft(d);
  if (d.lines.length === 0) fail("invalid_input", "datos no válidos: lines: falta al menos una línea");
  if (isInvoice(d.kind)) guardInvoicing();
  const day = date || d.issue_date || TODAY;
  const year = Number(day.slice(0, 4));
  const prefix = { quote: "P", delivery_note: "A", invoice: "F", simplified_invoice: "S", corrective_invoice: "R" }[d.kind];
  const key = `${prefix}-${year}`;
  const customer = customers.find((c) => c.id === d.customer_id);
  if (isInvoice(d.kind) && d.kind !== "simplified_invoice" && !customer?.data.tax_id) fail("invalid_input", "datos no válidos: recipient: una factura completa necesita el NIF del cliente");
  if (d.number === null) {
    const seq = (sequences.get(key) ?? 0) + 1;
    sequences.set(key, seq);
    d.series_year = year;
    d.sequence = seq;
    d.number = `${prefix}-${year}-${String(seq).padStart(4, "0")}`;
  }
  d.status = "issued";
  d.issue_date = day;
  d.issuer = { party: { name: company?.name ?? null, tax_id: company?.tax_id ?? null, address: company?.address ?? null }, email: company?.email ?? null, iban: company?.iban ?? null, logo_sha256: logo ? "a".repeat(64) : null, letterhead_sha256: letterhead ? "b".repeat(64) : null, letterhead_margins: letterhead ? (company?.letterhead_margins ?? DEFAULT_MARGINS) : null };
  d.recipient = customer ? { name: customer.data.name, tax_id: customer.data.tax_id, address: customer.data.address } : null;
  d.totals = totalsOf(d.lines, d.withholding_percent);
  d.lines = d.lines.map((l, i) => ({ ...l, amount: d.totals?.line_amounts[i] ?? null }));
  if (isInvoice(d.kind)) addRecord("alta", d);
  return d;
}

const NEXT: Record<string, SalesStatus[]> = {
  "quote:draft": ["issued"], "quote:issued": ["sent", "accepted", "rejected", "expired"], "quote:sent": ["accepted", "rejected", "expired"], "quote:accepted": ["converted"],
  "delivery_note:draft": ["issued"], "delivery_note:issued": ["sent", "delivered"], "delivery_note:sent": ["delivered"], "delivery_note:delivered": ["invoiced"],
};

function collection(d: SalesDocument): SalesCollection {
  const payable = d.totals?.amount_due ?? "0.00";
  const paid = [...payments.values()].filter((p) => p.document_id === d.id).reduce((s, p) => s + cents(p.amount), 0);
  const out = cents(payable) - paid;
  const state: SalesCollection["state"] = d.status === "voided" ? (paid ? "voided_with_payments" : "voided") : out < 0 ? "to_refund" : out === 0 ? "paid" : paid > 0 ? "partial" : d.due_date && d.due_date < TODAY ? "overdue" : "pending";
  return { state, payable, paid: money(paid), outstanding: money(out) };
}

const byId = <T extends { id: number }>(list: T[], wanted: number, what: string): T => list.find((x) => x.id === wanted) ?? fail("not_found", `${what} ${wanted} inexistente`);

const mockLogo = (): ArrayBuffer => {
  if (!logo) fail("not_found", "la empresa no tiene logotipo");
  return logo!.slice().buffer;
};

// ---- Envío de documentos por correo (E4b): tareas `send_sales_document` en la lista de tareas del
// simulado (la comparte `mock.ts`) y un historial de envíos. Aproxima las reglas del backend con
// los mismos códigos de error; `?envio-falla` hace que el primer envío falle para ver la
// recuperación (D50).

const SEND = "send_sales_document";
let sendTasks: Task[] = [];
let sendAccounts: () => MailAccount[] = () => [];
const sends: SalesSend[] = [];
let nextSendTask = 9000;
let nextSend = 0;
let failNextSend = params.has("envio-falla");

/** `mock.ts` entrega su lista de tareas y sus cuentas de correo. */
export function bindSendMock(tasks: Task[], accounts: () => MailAccount[]) {
  sendTasks = tasks;
  sendAccounts = accounts;
}

const SIMPLE_ADDRESS = /^[A-Za-z0-9._%+-]+@[A-Za-z0-9-]+(\.[A-Za-z0-9-]+)+$/;

const sendable = (d: SalesDocument) =>
  d.kind === "delivery_note" ? ["issued", "sent", "delivered"].includes(d.status) : ["issued", "sent"].includes(d.status);

/** Cuentas elegibles: las conectadas, sin «Correo simulado» si hay alguna real. */
function sendPool(): MailAccount[] {
  const connected = sendAccounts().filter((a) => a.status === "connected");
  const real = connected.filter((a) => a.provider !== "simulator");
  return real.length ? real : connected;
}

const accountOf = (a: MailAccount) => ({ id: a.id, provider: a.provider, label: a.label, address: a.address });

function openSend(docId: number): Task | undefined {
  return sendTasks.find((t) => t.kind === SEND && t.result?.document?.id === docId && t.state !== "completed" && t.state !== "cancelled");
}

function sendTaskOf(id: number): Task {
  const t = sendTasks.find((x) => x.id === id && x.kind === SEND);
  if (!t) fail("not_found", `tarea ${id} inexistente`);
  return t!;
}

function reviewableSend(id: number): Task {
  const t = sendTaskOf(id);
  if (t.state !== "awaiting_review") fail("conflict", `la tarea ${id} no está pendiente de revisión`);
  return t;
}

function pdfName(d: SalesDocument): string {
  return `${(d.number ?? "documento").replace(/[^A-Za-z0-9_-]/g, "_")}${d.revision > 1 ? `-r${d.revision}` : ""}.pdf`;
}

function recipientState(to: string): "ok" | "missing" | "invalid" {
  return !to ? "missing" : SIMPLE_ADDRESS.test(to) && to.length <= 254 ? "ok" : "invalid";
}

function checkSendContent(to: string, subject: string, body: string, account: string) {
  const state = recipientState(to);
  if (state === "missing") fail("recipient_missing", "el cliente no tiene correo: falta el destinatario");
  if (state === "invalid") fail("recipient_invalid", "el destinatario no es una dirección válida");
  if (!subject || subject.length > 200 || /[\u0000-\u001f\u007f]/.test(subject)) fail("subject_invalid", "el asunto no es válido");
  if (!body.trim()) fail("body_empty", "el cuerpo está vacío");
  if (!account) fail("account_required", "elija una cuenta de correo conectada");
  if (!sendAccounts().some((a) => a.id === account && a.status === "connected")) fail("account_unavailable", "la cuenta elegida no está conectada");
}

function prepareSend(docId: number): Task {
  const d = get(docId);
  if (d.status === "voided") fail("document_voided", "el documento está anulado y no se envía");
  if (d.superseded_by !== null) fail("document_revised", "el documento tiene una revisión posterior");
  if (!d.number || !sendable(d)) fail("status_not_sendable", `un documento en estado «${d.status}» no se puede enviar por correo`);
  if (openSend(docId)) fail("send_in_progress", "este documento ya tiene un envío por correo abierto");
  const pool = sendPool();
  if (pool.length === 0) fail("no_mail_account", "no hay ninguna cuenta de correo conectada para enviar");
  const mine = company?.email?.toLowerCase();
  const chosen = pool.find((a) => a.address?.toLowerCase() === mine) ?? (pool.length === 1 ? pool[0] : undefined);
  const customer = customers.find((c) => c.id === d.customer_id);
  const email = customer?.data.email?.trim() || null;
  const subject = `${KIND_TITLE[d.kind]} ${d.number}${d.revision > 1 ? ` (revisión ${d.revision})` : ""}${company?.name ? ` — ${company.name}` : ""}`;
  const body = [`Hola${customer ? `, ${customer.data.name}` : ""}:`, "", `Le adjuntamos ${KIND_WITH_ARTICLE[d.kind]} ${d.number}${d.issue_date ? ` del ${d.issue_date.split("-").reverse().join("/")}` : ""}.`, "", "Un saludo,", company?.name ?? ""].join("\n");
  const task: Task = {
    id: ++nextSendTask,
    kind: SEND,
    input_ref: `sales_document:${docId}`,
    state: "awaiting_review",
    attempts: 0,
    error: null,
    created_at: Date.now(),
    updated_at: Date.now(),
    result: {
      type: SEND,
      version: 1,
      document: { id: d.id, kind: d.kind, number: d.number, revision: d.revision, status: d.status },
      customer_id: d.customer_id,
      customer_email_at_prepare: email,
      draft: { to: email ? [email] : [], subject, body, attachments: [{ name: pdfName(d), mime: "application/pdf", size: 48_000 + d.id, sha256: String(d.id).padStart(64, "c") }] },
      account: chosen ? accountOf(chosen) : null,
      checks: { sendable: true, pdf: "ok", account: chosen ? "ok" : "missing", recipient: recipientState(email ?? "") },
      warnings: { recipient_differs: false },
      review_reason: "todo envío por correo requiere su aprobación",
      action: { kind: SEND },
    },
  };
  sendTasks.push(task);
  return task;
}

const KIND_TITLE: Record<SalesKind, string> = { quote: "Presupuesto", delivery_note: "Albarán", invoice: "Factura", simplified_invoice: "Factura simplificada", corrective_invoice: "Factura rectificativa" };
const KIND_WITH_ARTICLE: Record<SalesKind, string> = { quote: "el presupuesto", delivery_note: "el albarán", invoice: "la factura", simplified_invoice: "la factura simplificada", corrective_invoice: "la factura rectificativa" };

function editSend(id: number, to: string, subject: string, body: string, account: string): Task {
  const t = reviewableSend(id);
  const r = t.result!;
  to = String(to).trim();
  subject = String(subject).trim();
  checkSendContent(to, subject, String(body), String(account).trim());
  const chosen = sendAccounts().find((a) => a.id === String(account).trim())!;
  const email = r.customer_email_at_prepare as string | null;
  t.result = {
    ...r,
    version: (r.version ?? 1) + 1,
    edited: true,
    draft: { ...r.draft, to: [to], subject, body },
    account: accountOf(chosen),
    checks: { ...r.checks, account: "ok", recipient: "ok" },
    warnings: { recipient_differs: email !== null && email.toLowerCase() !== to.toLowerCase() },
  };
  t.updated_at = Date.now();
  return t;
}

function approveSend(id: number, version: number): Task {
  const t = reviewableSend(id);
  const r = t.result!;
  if ((r.version ?? 1) !== version) fail("stale_version", `el contenido cambió (versión ${r.version}, se aprobó la ${version})`);
  checkSendContent(r.draft.to?.[0] ?? "", r.draft.subject, r.draft.body, r.account?.id ?? "");
  if (r.recovery?.needs_confirmation) return t;
  if (failNextSend) {
    failNextSend = false;
    Object.assign(t, { state: "failed", attempts: t.attempts + 1, error: { code: "unavailable", message: "el servicio de correo no respondió" } });
    t.result = { ...r, attempt: { version } };
    return t;
  }
  const d = get(r.document.id);
  const account = r.account;
  sends.push({
    id: ++nextSend,
    document_id: d.id,
    task_id: t.id,
    version,
    idempotency_key: `task-${t.id}/send_sales_document/v${version}`,
    account: account.id,
    account_address: account.address ?? null,
    recipient: r.draft.to[0],
    subject: r.draft.subject,
    attachment_name: r.draft.attachments[0].name,
    attachment_size: r.draft.attachments[0].size,
    attachment_sha256: r.draft.attachments[0].sha256,
    authorization: "aprobación",
    sent_message_id: `simulado-${nextSend}`,
    sent_at: Date.now(),
  });
  if (d.status === "issued") d.status = "sent";
  Object.assign(t, { state: "completed", error: null });
  t.result = { ...r, receipt: { simulated: true } };
  return t;
}

/** Reintentar un envío fallido: vuelve a revisión con el aviso de intento sin confirmar (D50). */
export function recoverSend(t: Task): Task {
  const version = t.result?.attempt?.version ?? t.result?.version ?? 1;
  t.state = "awaiting_review";
  t.result = { ...t.result, recovery: { version, error: t.error?.message ?? null } };
  t.error = null;
  return t;
}

function confirmSendNotSent(id: number): Task {
  const t = reviewableSend(id);
  const r = t.result!;
  if (!r.attempt) fail("conflict", `la tarea ${id} no tiene ningún intento anterior`);
  const { recovery: _recovery, ...rest } = r;
  t.result = { ...rest, version: (r.version ?? 1) + 1, review_reason: "versión nueva tras confirmar que el intento anterior no se realizó" };
  return t;
}

function rejectSend(id: number): Task {
  const t = sendTaskOf(id);
  if (t.state === "completed" || t.state === "cancelled") fail("conflict", `la tarea ${id} ya no se puede descartar`);
  if (t.result?.attempt && t.result?.recovery) fail("send_in_progress", "el envío tiene un intento sin cerrar: confirme que no se envió antes de descartarlo");
  t.state = "cancelled";
  return t;
}

export const salesHandlers: Record<string, (a: any, options?: any) => unknown> = {
  sales_prepare_send: ({ id: d }) => prepareSend(d),
  sales_edit_send: ({ id: t, to, subject, body, account }) => editSend(t, to, subject, body, account),
  sales_approve_send: ({ id: t, version }) => approveSend(t, version),
  sales_confirm_send_not_sent: ({ id: t }) => confirmSendNotSent(t),
  sales_confirm_send_not_sent_unverified: ({ id: t }) => confirmSendNotSent(t),
  sales_reject_send: ({ id: t }) => rejectSend(t),
  sales_send_task: ({ id: d }) => openSend(d) ?? null,
  sales_document_sends: ({ id: d }) => sends.filter((s) => s.document_id === d),
  sales_send_accounts: () => sendPool(),
  reload_verifactu_producer: () => {
    // Simula corregir el archivo: la primera recarga lo deja válido.
    producerOk = true;
    return { configured: true, problem: null };
  },
  sales_setup_status: () => ({
    producer_configured: producerOk,
    producer_problem: producerOk ? null : "el archivo verifactu-productor.json conserva los marcadores PONER-AQUI",
    chain: chainBroken ? { state: "broken", at_id: 2, reason: "la huella del registro no coincide con su contenido" } : records.length ? { state: "ok", records: records.length } : { state: "empty" },
    today: TODAY,
  }),
  sales_company: () => company,
  sales_save_company: ({ company: data }) => {
    company = { ...(company ?? { has_logo: false, logo_mime: null, has_letterhead: false, letterhead_margins: DEFAULT_MARGINS }), ...data, has_logo: company?.has_logo ?? false, logo_mime: company?.logo_mime ?? null } as SalesCompany;
  },
  sales_set_company_logo: (bytes: Uint8Array, options?: { headers: Record<string, string> }) => {
    const mime = options?.headers?.["x-nestadmin-mime"];
    if (mime !== "image/png" && mime !== "image/jpeg") fail("invalid_input", "datos no válidos: logo_mime: use PNG o JPEG");
    if (bytes.length > 2 * 1024 * 1024) fail("invalid_input", "datos no válidos: logo: supera los 2 MB");
    logo = bytes;
    company = { ...(company as SalesCompany), has_logo: true, logo_mime: mime ?? null };
  },
  sales_remove_company_logo: () => {
    logo = null;
    if (company) company = { ...company, has_logo: false, logo_mime: null };
  },
  // Membrete (D222): el simulado solo conoce la cabecera `%PDF-`; PDFium decide páginas, tamaño y
  // contraseña en el backend real.
  sales_set_letterhead: (bytes: Uint8Array) => {
    if (!(bytes instanceof Uint8Array) || bytes.length === 0) fail("invalid_input", "datos no válidos: letterhead: el archivo está vacío");
    if (bytes.length > 5 * 1024 * 1024) fail("invalid_input", "datos no válidos: letterhead: el PDF supera los 5 MB");
    if (String.fromCharCode(...bytes.slice(0, 5)) !== "%PDF-") fail("invalid_input", "datos no válidos: letterhead: el archivo no es un PDF válido o está dañado");
    letterhead = true;
    company = { ...(company ?? ({ name: null, tax_id: null, address: null, email: null, iban: null, has_logo: false, logo_mime: null, letterhead_margins: DEFAULT_MARGINS } as SalesCompany)), has_letterhead: true };
  },
  sales_remove_letterhead: () => {
    letterhead = false;
    if (company) company = { ...company, has_letterhead: false };
  },
  sales_set_letterhead_margins: ({ margins }) => {
    const parsed = parseMargins({ top_mm: String(margins.top_mm), right_mm: String(margins.right_mm), bottom_mm: String(margins.bottom_mm), left_mm: String(margins.left_mm) });
    if (!parsed.ok) fail("invalid_input", `datos no válidos: letterhead.margins: ${parsed.problems[0]}`);
    else company = { ...(company as SalesCompany), letterhead_margins: parsed.margins };
  },
  sales_document_pdf: ({ id: d }) => {
    const doc = get(d);
    if (doc.status === "draft" || !doc.number) fail("conflict", "solo se genera el PDF de un documento emitido");
    return `documentos\\ventas\\${doc.number}.pdf`;
  },
  sales_open_document_pdf: ({ id: d }) => {
    const doc = get(d);
    if (doc.status === "draft" || !doc.number) fail("conflict", "solo se genera el PDF de un documento emitido");
  },
  sales_company_logo: () => mockLogo(),
  sales_logo_by_hash: () => mockLogo(),
  sales_customers: ({ retired }) => customers.filter((c) => retired || !c.retired),
  sales_customer: ({ id: c }) => byId(customers, c, "cliente"),
  sales_create_customer: ({ data }) => {
    if (!String(data.name ?? "").trim()) fail("invalid_input", "datos no válidos: name: falta el nombre");
    const c = { id: id(), data, retired: false };
    customers.push(c);
    return c.id;
  },
  sales_update_customer: ({ id: c, data }) => {
    if (!String(data.name ?? "").trim()) fail("invalid_input", "datos no válidos: name: falta el nombre");
    byId(customers, c, "cliente").data = data;
  },
  sales_retire_customer: ({ id: c }) => {
    byId(customers, c, "cliente").retired = true;
  },
  sales_products: ({ retired }) => products.filter((p) => retired || !p.retired),
  sales_product: ({ id: p }) => byId(products, p, "producto"),
  sales_create_product: ({ product }) => {
    lineFrom({ description: product.description, quantity: "1", unit_price: product.unit_price, tax_percent: product.tax_percent, exemption: product.exemption });
    const p: SalesProduct = { id: id(), description: product.description, unit_price: product.unit_price, tax_percent: product.tax_percent, exemption: product.exemption ?? null, retired: false };
    products.push(p);
    return p.id;
  },
  sales_update_product: ({ id: p, product }) => {
    lineFrom({ description: product.description, quantity: "1", unit_price: product.unit_price, tax_percent: product.tax_percent, exemption: product.exemption });
    Object.assign(byId(products, p, "producto"), { ...product, exemption: product.exemption ?? null, retired: Boolean(product.retired) });
  },
  sales_retire_product: ({ id: p }) => {
    byId(products, p, "producto").retired = true;
  },
  sales_received_notes: () => received,
  sales_received_note: ({ id: n }) => byId(received, n, "albarán recibido"),
  sales_create_received_note: ({ note }) => {
    if (!String(note.supplier_name ?? "").trim() || !String(note.supplier_number ?? "").trim()) fail("invalid_input", "datos no válidos: proveedor y número son obligatorios");
    const n: SalesReceivedNote = { id: id(), supplier_name: note.supplier_name, supplier_tax_id: note.supplier_tax_id ?? null, supplier_number: note.supplier_number, date: note.date, status: "received", document_ref: null, invoice_ref: null };
    received.push(n);
    return n.id;
  },
  sales_review_received_note: ({ id: n, document, invoice }) => {
    const note = byId(received, n, "albarán recibido");
    if (note.status === "reviewed") fail("invalid_transition", "el albarán recibido ya está revisado");
    Object.assign(note, { status: "reviewed", document_ref: document ?? null, invoice_ref: invoice ?? null });
  },
  sales_create_draft: ({ draft }) => {
    const d = newDraft({ kind: draft.kind });
    fillDraft(d, draft);
    return d.id;
  },
  sales_update_draft: ({ id: d, draft }) => {
    const doc = get(d);
    requireDraft(doc);
    fillDraft(doc, draft);
  },
  sales_delete_draft: ({ id: d }) => {
    requireDraft(get(d));
    documents.delete(d);
  },
  sales_add_line_from_product: ({ id: d, product, quantity, discount }) => {
    const doc = get(d);
    requireDraft(doc);
    const p = byId(products, product, "producto");
    doc.lines.push(lineFrom({ description: p.description, quantity, unit_price: p.unit_price, discount, tax_percent: p.tax_percent, exemption: p.exemption, product_id: p.id }));
    doc.totals = totalsOf(doc.lines, doc.withholding_percent);
  },
  sales_documents: ({ filter }) =>
    [...documents.values()]
      .filter((d) => (!filter?.kind || d.kind === filter.kind) && (!filter?.status || d.status === filter.status) && (filter?.include_superseded || d.superseded_by === null))
      .sort((a, b) => b.id - a.id),
  sales_document: ({ id: d }) => get(d),
  sales_links: ({ id: d }) => links.filter((l) => l.from_id === d || l.to_id === d),
  sales_issue: ({ id: d, date }) => issue(get(d), date),
  sales_void: ({ id: d }) => {
    const doc = get(d);
    if (!isInvoice(doc.kind)) fail("invalid_transition", "solo se anula una factura");
    if (doc.status === "sent") fail("invalid_transition", "una factura enviada no se anula: use una rectificativa (D81)");
    if (doc.status !== "issued") fail("invalid_transition", `${doc.kind} en estado ${doc.status} no se puede anular`);
    guardInvoicing();
    doc.status = "voided";
    addRecord("anulacion", doc);
    return doc;
  },
  sales_transition: ({ id: d, to }) => {
    const doc = get(d);
    if (!(NEXT[`${doc.kind}:${doc.status}`] ?? (isInvoice(doc.kind) ? (doc.status === "issued" ? ["sent", "voided"] : doc.status === "draft" ? ["issued"] : []) : [])).includes(to))
      fail("invalid_transition", `transición no permitida para ${doc.kind}: ${doc.status} a ${to}`);
    if (to === "issued") return issue(doc, null);
    doc.status = to;
    return doc;
  },
  sales_revise: ({ id: d }) => {
    const doc = get(d);
    if (isInvoice(doc.kind) || !["issued", "sent"].includes(doc.status)) fail("invalid_transition", `${doc.kind} en estado ${doc.status} no admite una revisión nueva`);
    if (doc.superseded_by !== null) fail("conflict", "el documento ya tiene una revisión posterior");
    const next = newDraft({ ...doc, id: undefined, status: "draft", revision: doc.revision + 1, lines: doc.lines.map((l) => ({ ...l, amount: null })), issuer: null, recipient: null, created_at: Date.now() } as any);
    doc.superseded_by = next.id;
    links.push({ from_id: doc.id, to_id: next.id, relation: "revision" });
    return next.id;
  },
  sales_convert: ({ ids, to }) => {
    const sources = (ids as number[]).map(get);
    if (sources.length === 0) fail("invalid_input", "datos no válidos: ids: no hay documentos");
    for (const s of sources) {
      const ok = (s.kind === "quote" && s.status === "accepted" && (to === "delivery_note" || to === "invoice")) || (s.kind === "delivery_note" && s.status === "delivered" && to === "invoice");
      if (!ok) fail("invalid_transition", "conversión no permitida: solo presupuestos aceptados o albaranes entregados");
    }
    if (sources.length > 1 && !sources.every((s) => s.kind === "delivery_note")) fail("invalid_transition", "conversión no permitida: solo se agrupan albaranes");
    const taxIds = sources.map((s) => customers.find((c) => c.id === s.customer_id)?.data.tax_id ?? null);
    if (sources.length > 1 && (taxIds.some((t) => !t) || new Set(taxIds).size > 1)) fail("invalid_transition", "conversión no permitida: los albaranes deben ser del mismo cliente, con NIF");
    const d = newDraft({ kind: to, customer_id: sources[0]!.customer_id, withholding_percent: sources[0]!.withholding_percent, lines: sources.flatMap((s) => s.lines.map((l) => ({ ...l, amount: null }))) });
    for (const s of sources) {
      links.push({ from_id: s.id, to_id: d.id, relation: "converted" });
      s.status = s.kind === "quote" ? "converted" : "invoiced";
    }
    return d.id;
  },
  sales_create_corrective: ({ id: d, kind, method }) => {
    const doc = get(d);
    if (!isInvoice(doc.kind) || doc.kind === "corrective_invoice" || !["issued", "sent"].includes(doc.status)) fail("invalid_transition", `${doc.kind} en estado ${doc.status} no se puede rectificar`);
    const c = newDraft({ kind: "corrective_invoice", customer_id: doc.customer_id, rectifies_id: doc.id, rectification: { kind, method, rectified_base: null, rectified_tax: null }, lines: method === "substitution" ? doc.lines.map((l) => ({ ...l, amount: null })) : [] });
    links.push({ from_id: c.id, to_id: doc.id, relation: "rectifies" });
    return c.id;
  },
  sales_add_payment: ({ id: d, date, amount, method }) => {
    const doc = get(d);
    if (!isInvoice(doc.kind) || !["issued", "sent"].includes(doc.status)) fail("invalid_input", "datos no válidos: solo se cobra una factura emitida");
    if (Number.isNaN(Number(amount)) || Number(amount) === 0) fail("invalid_input", "datos no válidos: amount: importe no válido");
    const p = { id: id(), document_id: d, date, amount, method };
    payments.set(p.id, p);
    return { payment_id: p.id, collection: collection(doc) };
  },
  sales_remove_payment: ({ id: p }) => {
    const pay = payments.get(p) ?? fail("not_found", `cobro ${p} inexistente`);
    payments.delete(p);
    return collection(get(pay.document_id));
  },
  sales_payments: ({ id: d }) => [...payments.values()].filter((p) => p.document_id === d).map(({ document_id: _doc, ...p }) => p),
  sales_collection: ({ id: d }) => collection(get(d)),
  sales_recurring_templates: () => templates,
  sales_recurring_template: ({ id: t }) => byId(templates, t, "plantilla"),
  sales_create_recurring: ({ template }) => {
    byId(customers, template.customer_id, "cliente");
    const t: SalesRecurring = { id: id(), customer_id: template.customer_id, lines: template.lines.map(lineFrom), frequency: template.frequency, first_day: template.first_day, end: template.end ?? null, active: true };
    templates.push(t);
    return t.id;
  },
  sales_update_recurring: ({ id: t, template }) => {
    Object.assign(byId(templates, t, "plantilla"), { customer_id: template.customer_id, lines: template.lines.map(lineFrom), frequency: template.frequency, first_day: template.first_day, end: template.end ?? null });
  },
  sales_set_recurring_active: ({ id: t, active }) => {
    byId(templates, t, "plantilla").active = Boolean(active);
  },
  sales_generate_recurring: () => {
    const run: { created: any[]; failed: any[] } = { created: [], failed: [] };
    for (const t of templates.filter((x) => x.active)) {
      if (t.first_day > TODAY) continue;
      const key = `recurring:${t.id}:1`;
      if (generatedKeys.has(key)) continue;
      generatedKeys.add(key);
      const d = newDraft({ kind: "invoice", customer_id: t.customer_id, lines: t.lines.map((l) => ({ ...l })), recurring_template_id: t.id, recurring_period: 1 });
      run.created.push({ document_id: d.id, template_id: t.id, period: 1, key, mode: "requires_review" });
    }
    return run;
  },
  verifactu_records: () => records,
  verifactu_status: () => verifactuStatus(),
  verifactu_submit_now: () => verifactuPump(),
  verifactu_subsanate: ({ recordId }) => verifactuSubsanate(recordId),
  verifactu_environment: () => verifactuEnvironment,
  verifactu_set_environment: ({ environment }) => verifactuSetEnvironment(environment),
};

// ---- Remisión de Verifactu (E5): estado derivado de cada registro, último lote, espera y entorno.
// Aproxima el backend con datos sintéticos; las marcas de tiempo son milisegundos, como las reales.

interface MockRecordState {
  status: VerifactuStatus;
  code: number | null;
  description: string | null;
  duplicate: boolean;
  batch: number | null;
  subsanation: number | null;
}

const recordStates = new Map<number, MockRecordState>();
let verifactuEnvironment: VerifactuEnvironment = "simulador";
let lastBatch: VerifactuBatchView | null = null;
let nextBatch = 1;
let nextSendAt: number | null = null;
let consecutiveFaults = 0;
let forceReject = params.has("verifactu-rechazo");
let forceNoNetwork = params.has("verifactu-sin-red");
let forceFault = params.has("verifactu-fault");
const WAIT_SECONDS = 60;

const stateOf = (recordId: number): MockRecordState => recordStates.get(recordId) ?? { status: "pending", code: null, description: null, duplicate: false, batch: null, subsanation: null };
const countStatus = (status: VerifactuStatus) => records.filter((r) => stateOf(r.id).status === status).length;

function verifactuStatus(): VerifactuStatusView {
  const views: VerifactuRecordView[] = records.map((r) => {
    const st = stateOf(r.id);
    return { record_id: r.id, document_id: r.document_id, kind: r.kind, invoice_number: r.invoice_number, issue_date: r.issue_date, status: st.status, aeat_code: st.code, aeat_description: st.description, duplicate: st.duplicate, batch_id: st.batch, subsanation_record_id: st.subsanation, environment: "simulador" };
  });
  const remaining = nextSendAt === null ? null : Math.max(0, Math.ceil((nextSendAt - Date.now()) / 1000));
  return {
    environment: verifactuEnvironment,
    selectable_environments: ["simulador"],
    environment_notice: verifactuEnvironment === "simulador" ? "Entorno de pruebas: nada de esto llega a la AEAT y las facturas emitidas aquí llevan para siempre el QR de pruebas." : null,
    records: views,
    last_batch: lastBatch,
    next_send_at: nextSendAt,
    wait_remaining_seconds: remaining,
    halted: consecutiveFaults >= 3,
    consecutive_faults: consecutiveFaults,
    pending: countStatus("pending"),
    sending: countStatus("sending"),
    rejected: countStatus("rejected"),
    unexpected_duplicates: countStatus("unexpected_duplicate"),
  };
}

function verifactuPump(): VerifactuPumpReport {
  const open = records.filter((r) => ["pending", "sending"].includes(stateOf(r.id).status));
  if (open.length === 0) return { state: "idle" };
  const now = Date.now();
  if (nextSendAt !== null && now < nextSendAt) return { state: "waiting", seconds: Math.ceil((nextSendAt - now) / 1000), until: nextSendAt };
  const batch = nextBatch++;
  nextSendAt = now + WAIT_SECONDS * 1000;
  const base = { id: batch, environment: "simulador", record_count: open.length, started_at: now, attempts: 1 };
  if (forceNoNetwork) {
    forceNoNetwork = false;
    for (const r of open) recordStates.set(r.id, { ...stateOf(r.id), status: "sending", batch });
    lastBatch = { ...base, outcome: null, send_state: null, error_code: null, error_description: null, wait_seconds: null, csv: null, finished_at: null };
    return { state: "no_response", batch_id: batch, code: "unavailable", message: "sin conexión (simulado)" };
  }
  if (forceFault) {
    forceFault = false;
    consecutiveFaults += 1;
    const description = "Codigo[4104].Error en la cabecera del envío (simulado)";
    lastBatch = { ...base, outcome: "fault", send_state: null, error_code: 4104, error_description: description, wait_seconds: WAIT_SECONDS, csv: null, finished_at: now };
    return { state: "fault", batch_id: batch, code: 4104, description };
  }
  consecutiveFaults = 0;
  let rejected = 0;
  open.forEach((r, i) => {
    if (forceReject && i === 0) {
      rejected += 1;
      recordStates.set(r.id, { status: "rejected", code: 1100, description: "Rechazo simulado: valor o tipo incorrecto en un campo", duplicate: false, batch, subsanation: null });
    } else recordStates.set(r.id, { status: "accepted", code: null, description: null, duplicate: false, batch, subsanation: null });
  });
  forceReject = false;
  lastBatch = { ...base, outcome: "responded", send_state: rejected ? "ParcialmenteCorrecto" : "Correcto", error_code: null, error_description: null, wait_seconds: WAIT_SECONDS, csv: `CSV-DEMO-${String(batch).padStart(4, "0")}`, finished_at: now };
  return { state: "sent", batch_id: batch, accepted: open.length - rejected, accepted_with_errors: 0, rejected, unexpected_duplicates: 0, missing: 0, wait_seconds: WAIT_SECONDS };
}

function verifactuSubsanate(recordId: number) {
  const rejected = byId(records, recordId, "registro Verifactu");
  const st = stateOf(recordId);
  if (st.subsanation !== null) return { rejected_record_id: recordId, record_id: st.subsanation, created: false };
  if (st.status !== "rejected") fail("conflict", `el registro ${recordId} está «${st.status}»: solo se subsana uno rechazado`);
  const previous = records[records.length - 1] ?? null;
  const hash = (0x7f4a7c15 * (records.length + 3)).toString(16).toUpperCase().padStart(8, "0").repeat(8).slice(0, 64);
  const fresh: VerifactuRecord = { ...rejected, id: records.length + 1, previous_id: previous?.id ?? null, previous_hash: previous?.hash ?? null, hash, created_at: Date.now() };
  records.push(fresh);
  recordStates.set(recordId, { ...st, status: "subsanated", subsanation: fresh.id });
  return { rejected_record_id: recordId, record_id: fresh.id, created: true };
}

function verifactuSetEnvironment(environment: string) {
  if (!["simulador", "preproduccion", "produccion"].includes(environment)) fail("invalid_input", "datos no válidos: environment: entorno no admitido");
  if (environment === verifactuEnvironment) return { environment, changed: false };
  if (environment !== "simulador") {
    const busy = countStatus("pending") + countStatus("sending") + countStatus("rejected");
    if (busy > 0) fail("verifactu_environment_busy", `no se cambia de entorno con registros por remitir (${busy})`);
    fail("environment_unavailable", `el entorno «${environment}» no está disponible todavía`);
  }
  verifactuEnvironment = "simulador";
  return { environment, changed: true };
}

/** Comandos que cambian datos de venta (para el aviso de cambios del simulado). */
export const salesMutating = new Set<string>();

// Datos de partida: un presupuesto aceptado y una factura emitida, para ver la lista llena.
{
  const quote = newDraft({ kind: "quote", customer_id: 1, lines: [lineFrom({ description: "Revisión anual", quantity: "2", unit_price: "45", tax_percent: "21", product_id: 1 })] });
  issue(quote, "2026-10-20");
  quote.status = "accepted";
  if (producerOk && !chainBroken) {
    const invoice = newDraft({ kind: "invoice", customer_id: 2, due_date: "2026-11-30", lines: [lineFrom({ description: "Soporte mensual", quantity: "1", unit_price: "120", tax_percent: "21" })] });
    issue(invoice, "2026-11-02");
    // Registros de remisión de todos los estados, para ver la pestaña llena (datos inventados).
    const now = Date.now();
    const more = (customer: number, description: string, price: string, date: string) => issue(newDraft({ kind: "invoice", customer_id: customer, lines: [lineFrom({ description, quantity: "1", unit_price: price, tax_percent: "21" })] }), date);
    more(1, "Revisión de instalación", "80", "2026-11-03");
    more(2, "Cambio de piezas", "210", "2026-11-04");
    more(1, "Soporte urgente", "95", "2026-11-05");
    more(2, "Soporte urgente (subsanación)", "95", "2026-11-05");
    more(1, "Formación básica", "150", "2026-11-06");
    more(2, "Mantenimiento trimestral", "300", "2026-11-09");
    const at = (recordId: number, state: Partial<MockRecordState> & { status: VerifactuStatus }) => recordStates.set(recordId, { code: null, description: null, duplicate: false, batch: 1, subsanation: null, ...state });
    at(1, { status: "accepted" });
    at(2, { status: "accepted_with_errors", code: 2000, description: "Aceptado con errores (simulado): falta un dato opcional" });
    at(3, { status: "rejected", code: 1100, description: "El NIF del destinatario no consta en el censo (simulado)" });
    at(4, { status: "subsanated", code: 1100, description: "Valor incorrecto en el importe total (simulado)", subsanation: 5 });
    at(5, { status: "pending", batch: null });
    at(6, { status: "unexpected_duplicate", code: 3000, description: "Registro duplicado" });
    at(7, { status: "sending", batch: 2 });
    nextBatch = 3;
    lastBatch = { id: 1, environment: "simulador", record_count: 4, started_at: now - 120_000, attempts: 1, outcome: "responded", send_state: "ParcialmenteCorrecto", error_code: null, error_description: null, wait_seconds: WAIT_SECONDS, csv: "CSV-DEMO-0001", finished_at: now - 118_000 };
    nextSendAt = now + 45_000;
  }
}
