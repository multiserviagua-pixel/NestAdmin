// Textos y reglas puras de la sección Ventas (fase 11, E3b), sin DOM: se prueban con
// `node --test`. Solo tipos borrables (sin `enum`): Node los ejecuta tal cual. Todos los
// ejemplos son inventados.
//
// Los importes viajan como cadenas decimales con punto (D204) y nunca se pasan por `Number`:
// aquí solo se formatean con operaciones sobre el texto. Los totales los calcula el backend.

import type { SalesCollectionState, SalesDocument, SalesFrequency, SalesKind, SalesPaymentMethod, SalesQualification, SalesRectificationKind, SalesRectificationMethod, SalesStatus } from "./types";

// ---- Errores con código (D204) ----

export interface SalesError {
  /** Código estable entre corchetes; `null` si el error no lo trae. */
  code: string | null;
  /** El texto del backend sin el código, tal cual. */
  detail: string;
}

/** Separa `[codigo] mensaje`. Cualquier otro texto se muestra sin interpretar. */
export function parseSalesError(error: unknown): SalesError {
  const text = String(error instanceof Error ? error.message : (error ?? "")).trim();
  const match = /^\[([a-z][a-z0-9_]*)\]\s*([\s\S]*)$/.exec(text);
  return match ? { code: match[1]!, detail: match[2]!.trim() } : { code: null, detail: text };
}

const ERROR_HEADLINE: Record<string, string> = {
  verifactu_not_configured: "Falta configurar el productor de Verifactu",
  verifactu_chain_broken: "La cadena de registros Verifactu está rota",
  verifactu_clock_behind: "El reloj del equipo está atrasado respecto al último registro",
  verifactu_record_invalid: "No se pudo generar el registro Verifactu",
  invalid_input: "Datos no válidos",
  invalid_transition: "Esa operación no está permitida en el estado actual",
  conflict: "Conflicto con otro documento",
  not_found: "No encontrado",
  storage: "Error al leer o guardar los datos",
  pdf_unavailable: "PDFium no está disponible",
  pdf_failed: "No se pudo generar el PDF",
};

/** `invalid_input` llega como «datos no válidos: campo: motivo»: el encabezado ya dice lo primero y
 * el campo del membrete (`letterhead`, `letterhead.margins`…) no le dice nada al usuario. */
function cleanInvalidInput(detail: string): string {
  let text = detail.replace(/^datos no válidos:\s*/i, "");
  text = text.replace(/^letterhead(?:\.[A-Za-z0-9_.]+)?:\s*/, "");
  return text.trim();
}

/** Texto para el usuario: el encabezado del código y, detrás, el detalle del backend sin tocar. */
export function salesErrorText(error: unknown): string {
  const { code, detail } = parseSalesError(error);
  const headline = code ? ERROR_HEADLINE[code] : undefined;
  if (!headline) return detail || "Error desconocido";
  const body = code === "invalid_input" ? cleanInvalidInput(detail) : detail;
  return body ? `${headline}: ${body}` : headline;
}

/** Códigos que impiden emitir y anular hasta que se arregle algo fuera del documento. */
export function isBlockingCode(code: string | null): boolean {
  return code === "verifactu_not_configured" || code === "verifactu_chain_broken" || code === "verifactu_clock_behind";
}

// ---- Estado de configuración (D199, D203, D195) ----

export interface SetupLike {
  producer_configured: boolean;
  producer_problem: string | null;
  chain: { state: "empty" } | { state: "ok"; records: number } | { state: "broken"; at_id: number; reason: string };
}

export interface SetupNotice {
  tone: "warn" | "error";
  title: string;
  detail: string;
  /** Ofrece «Volver a leer» el archivo del productor. */
  canReload: boolean;
  /** Emitir y anular facturas queda bloqueado. */
  blocksInvoices: boolean;
}

export const SETUP_RELOAD = "Volver a leer";

/** Avisos de la cabecera de Ventas. Vacío si todo está en orden. */
/** Un motivo del backend como frase: mayúscula inicial y punto final. */
export function asSentence(text: string): string {
  const t = text.trim();
  if (!t) return t;
  const first = t.charAt(0).toUpperCase() + t.slice(1);
  return /[.!?…»)]$/.test(first) ? first : `${first}.`;
}

export function setupNotices(status: SetupLike | null): SetupNotice[] {
  if (!status) return [];
  const notices: SetupNotice[] = [];
  if (!status.producer_configured) {
    notices.push({
      tone: "warn",
      title: "Productor de Verifactu sin configurar",
      detail: `${status.producer_problem ? asSentence(status.producer_problem) : "No se encontró el archivo verifactu-productor.json en la carpeta de datos."} Los presupuestos y los albaranes funcionan; emitir o anular una factura queda bloqueado hasta que se configure. Corrija el archivo y pulse «${SETUP_RELOAD}».`,
      canReload: true,
      blocksInvoices: true,
    });
  }
  if (status.chain.state === "broken") {
    notices.push({
      tone: "error",
      title: "Cadena de registros Verifactu rota",
      detail: `${status.chain.reason} (registro ${status.chain.at_id}). No se puede emitir ni anular facturas hasta resolverlo; los registros existentes no se modifican.`,
      canReload: false,
      blocksInvoices: true,
    });
  }
  return notices;
}

/** Línea de estado de la cadena para la vista de registros. */
export function chainText(chain: SetupLike["chain"] | null): string {
  if (!chain) return "Estado de la cadena sin consultar";
  if (chain.state === "empty") return "Cadena vacía: todavía no se ha emitido ninguna factura";
  if (chain.state === "ok") return `Cadena íntegra: ${chain.records} ${chain.records === 1 ? "registro verificado" : "registros verificados"}`;
  return `Cadena rota en el registro ${chain.at_id}: ${chain.reason}`;
}

// ---- Etiquetas ----

export const KIND_LABEL: Record<SalesKind, string> = {
  quote: "Presupuesto",
  delivery_note: "Albarán",
  invoice: "Factura",
  simplified_invoice: "Factura simplificada",
  corrective_invoice: "Factura rectificativa",
};

export const KIND_ORDER: SalesKind[] = ["quote", "delivery_note", "invoice", "simplified_invoice", "corrective_invoice"];

export function isInvoiceKind(kind: SalesKind): boolean {
  return kind === "invoice" || kind === "simplified_invoice" || kind === "corrective_invoice";
}

export const STATUS_LABEL: Record<SalesStatus, string> = {
  draft: "Borrador",
  issued: "Emitido",
  sent: "Enviado",
  accepted: "Aceptado",
  rejected: "Rechazado",
  expired: "Caducado",
  converted: "Convertido",
  delivered: "Entregado",
  invoiced: "Facturado",
  voided: "Anulado",
};

export const STATUS_ORDER: SalesStatus[] = ["draft", "issued", "sent", "accepted", "rejected", "expired", "converted", "delivered", "invoiced", "voided"];

export type StatusTone = "neutral" | "info" | "ok" | "warn" | "error";

export function statusTone(status: SalesStatus): StatusTone {
  switch (status) {
    case "draft":
      return "neutral";
    case "voided":
    case "rejected":
      return "error";
    case "expired":
      return "warn";
    case "accepted":
    case "delivered":
    case "invoiced":
    case "converted":
      return "ok";
    default:
      return "info";
  }
}

export const QUALIFICATION_LABEL: Record<SalesQualification, string> = {
  S1: "S1 · Sujeta y no exenta",
  S2: "S2 · Inversión del sujeto pasivo",
  N1: "N1 · No sujeta (arts. 7 y 14 y otros)",
  N2: "N2 · No sujeta por localización",
  E1: "E1 · Exenta, art. 20",
  E2: "E2 · Exenta, art. 21",
  E3: "E3 · Exenta, art. 22",
  E4: "E4 · Exenta, arts. 23 y 24",
  E5: "E5 · Exenta, art. 25",
  E6: "E6 · Exenta por otras causas",
};

/** Calificaciones que se ofrecen en una línea al 0 %: S1 solo existe con tipos gravados. */
export const ZERO_RATE_QUALIFICATIONS: SalesQualification[] = ["S2", "N1", "N2", "E1", "E2", "E3", "E4", "E5", "E6"];

export const TAX_PERCENTS = ["21", "10", "4", "0"] as const;

export const RECTIFICATION_KIND_LABEL: Record<SalesRectificationKind, string> = {
  R1: "R1 · Error fundado en derecho y otros (art. 80.uno, dos y seis)",
  R2: "R2 · Concurso de acreedores (art. 80.tres)",
  R3: "R3 · Créditos incobrables (art. 80.cuatro)",
  R4: "R4 · Resto de causas",
  R5: "R5 · Facturas simplificadas",
};

export const RECTIFICATION_METHOD_LABEL: Record<SalesRectificationMethod, string> = {
  substitution: "Por sustitución: importes completos nuevos",
  differences: "Por diferencias: solo la variación (puede ser negativa)",
};

export const PAYMENT_METHOD_LABEL: Record<SalesPaymentMethod, string> = {
  transfer: "Transferencia",
  cash: "Efectivo",
  card: "Tarjeta",
  other: "Otro",
};

export const FREQUENCY_LABEL: Record<SalesFrequency, string> = {
  monthly: "Mensual",
  quarterly: "Trimestral",
  annual: "Anual",
};

export const COLLECTION_LABEL: Record<SalesCollectionState, string> = {
  pending: "Pendiente de cobro",
  partial: "Cobrada en parte",
  paid: "Cobrada",
  overdue: "Vencida",
  to_refund: "Cobrado de más: hay que devolver la diferencia",
  substituted: "Sustituida por una rectificativa: cobra la sustituta",
  applied_to_original: "Su importe se suma al de la factura original",
  voided: "Anulada sin cobros",
  voided_with_payments: "Anulada con cobros por regularizar",
};

export function collectionTone(state: SalesCollectionState): StatusTone {
  if (state === "paid") return "ok";
  if (state === "overdue" || state === "to_refund" || state === "voided_with_payments") return "error";
  if (state === "partial") return "warn";
  return "neutral";
}

/** `Factura F-2026-0001`; un borrador sin número se identifica por su id. */
export function documentTitle(doc: Pick<SalesDocument, "kind" | "number" | "revision" | "id" | "status">): string {
  const base = doc.number ?? (doc.status === "draft" ? `borrador n.º ${doc.id}` : `n.º ${doc.id}`);
  const revision = doc.revision > 1 && doc.number ? ` (revisión ${doc.revision})` : "";
  return `${KIND_LABEL[doc.kind]} ${base}${revision}`;
}

// ---- Acciones permitidas según el estado ----
// Reflejo de `crates/sales/src/status.rs`: el backend decide siempre y rechaza lo no permitido;
// aquí solo se ofrece lo que tiene sentido, para no invitar a acciones que fallarían.

export type SalesAction =
  | "edit"
  | "delete"
  | "issue"
  | "void"
  | "mark_sent"
  | "accept"
  | "reject"
  | "expire"
  | "deliver"
  | "revise"
  | "convert_invoice"
  | "convert_delivery_note"
  | "corrective"
  | "payments";

export function allowedActions(doc: Pick<SalesDocument, "kind" | "status" | "superseded_by">): SalesAction[] {
  if (doc.superseded_by !== null) return [];
  const { kind, status } = doc;
  if (status === "draft") return ["edit", "issue", "delete"];
  if (isInvoiceKind(kind)) {
    const list: SalesAction[] = [];
    if (status === "issued") list.push("mark_sent", "void");
    if (status === "issued" || status === "sent") list.push("payments");
    if (kind !== "corrective_invoice" && (status === "issued" || status === "sent")) list.push("corrective");
    return list;
  }
  if (kind === "quote") {
    if (status === "issued") return ["mark_sent", "accept", "reject", "expire", "revise"];
    if (status === "sent") return ["accept", "reject", "expire", "revise"];
    if (status === "accepted") return ["convert_delivery_note", "convert_invoice"];
    return [];
  }
  // Albarán.
  if (status === "issued") return ["mark_sent", "deliver", "revise"];
  if (status === "sent") return ["deliver", "revise"];
  if (status === "delivered") return ["convert_invoice"];
  return [];
}

/** Estado al que lleva cada transición directa. */
export const ACTION_TARGET: Partial<Record<SalesAction, SalesStatus>> = {
  mark_sent: "sent",
  accept: "accepted",
  reject: "rejected",
  expire: "expired",
  deliver: "delivered",
};

export const ACTION_LABEL: Record<SalesAction, string> = {
  edit: "Editar borrador",
  delete: "Eliminar borrador",
  issue: "Emitir",
  void: "Anular",
  mark_sent: "Marcar como enviado",
  accept: "Marcar como aceptado",
  reject: "Marcar como rechazado",
  expire: "Marcar como caducado",
  deliver: "Marcar como entregado",
  revise: "Crear revisión",
  convert_invoice: "Convertir en factura",
  convert_delivery_note: "Convertir en albarán",
  corrective: "Crear rectificativa",
  payments: "Cobros",
};

// ---- Confirmaciones explícitas ----

export function issueConfirmText(doc: Pick<SalesDocument, "kind">, today: string | null): string {
  const what = KIND_LABEL[doc.kind].toLowerCase();
  const when = today ? ` con fecha ${formatDate(today)}` : "";
  if (isInvoiceKind(doc.kind)) {
    return `Va a emitir una ${what}${when}.\n\nSe le asignará el siguiente número de la serie, se generará su registro Verifactu encadenado y dejará de poder editarse. Un error posterior solo se corrige con una factura rectificativa.\n\n¿Emitir ahora?`;
  }
  return `Va a emitir este ${what}${when}. Se le asignará número y dejará de poder editarse (después solo se admiten revisiones).\n\n¿Emitir ahora?`;
}

export function voidConfirmText(doc: Pick<SalesDocument, "number">): string {
  return `Va a anular ${doc.number ? `la factura ${doc.number}` : "la factura"}.\n\nSe genera un registro de anulación encadenado en Verifactu; la factura y su número se conservan y no se pueden reutilizar. Si ya se envió al cliente, use una rectificativa en su lugar. Esta acción no se puede deshacer.\n\n¿Anular la factura?`;
}

export function deleteDraftConfirmText(doc: Pick<SalesDocument, "id">): string {
  return `Se eliminará el borrador n.º ${doc.id}. No ha consumido número y no se puede recuperar.\n\n¿Eliminarlo?`;
}

export function convertConfirmText(to: SalesKind, count: number): string {
  const what = KIND_LABEL[to].toLowerCase();
  return count > 1
    ? `Se creará un borrador de ${what} con las líneas de ${count} documentos del mismo cliente. Quedan vinculados y los originales cambian de estado. Podrá revisarlo antes de emitir.\n\n¿Continuar?`
    : `Se creará un borrador de ${what} con las líneas del documento. Queda vinculado y el original cambia de estado. Podrá revisarlo antes de emitir.\n\n¿Continuar?`;
}

export function generateRecurringConfirmText(): string {
  return "Se crearán borradores de las facturas recurrentes que toquen. No se emite nada: cada borrador queda para que usted lo revise y lo emita. Un periodo ya generado no se repite.\n\n¿Generar borradores?";
}

export function recurringResultText(run: { created: unknown[]; failed: Array<{ template_id: number; reason: string }> }): string {
  const created = run.created.length;
  const base = created === 0 ? "No había periodos pendientes: no se creó ningún borrador." : `Se ${created === 1 ? "creó 1 borrador" : `crearon ${created} borradores`} para revisar. No se ha emitido nada.`;
  if (run.failed.length === 0) return base;
  const failed = run.failed.map((f) => `plantilla ${f.template_id}: ${f.reason}`).join("; ");
  return `${base} No se pudo generar: ${failed}.`;
}

// ---- Fechas e importes ----

/** `2026-11-03` -> `03/11/2026`. Lo que no es una fecha AAAA-MM-DD se devuelve igual. */
export function formatDate(value: string | null | undefined): string {
  if (!value) return "Sin fecha";
  const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(value);
  return m ? `${m[3]}/${m[2]}/${m[1]}` : value;
}

/** Fecha AAAA-MM-DD válida del calendario, sin pasar por `Date` ni por el huso horario. */
export function isIsoDate(value: string): boolean {
  const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(value);
  if (!m) return false;
  const year = Number(m[1]);
  const month = Number(m[2]);
  const day = Number(m[3]);
  if (month < 1 || month > 12 || day < 1) return false;
  const leap = (year % 4 === 0 && year % 100 !== 0) || year % 400 === 0;
  const days = [31, leap ? 29 : 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31][month - 1]!;
  return day <= days;
}

const CURRENCY_SYMBOL: Record<string, string> = { EUR: "€" };

function groupThousands(digits: string): string {
  return digits.replace(/\B(?=(\d{3})+(?!\d))/g, ".");
}

/**
 * Importe en formato español a partir de una cadena decimal con punto, sin pasar por `Number`:
 * `1234.5` -> `1.234,50 €`. Respeta los decimales que traiga (mínimo 2) para no redondear lo que
 * calcula el backend (p. ej. un precio unitario de 4 decimales).
 */
export function formatMoney(value: string | null | undefined, currency = "EUR"): string {
  if (value === null || value === undefined || value === "") return "—";
  const m = /^(-?)(\d+)(?:\.(\d+))?$/.exec(value.trim());
  if (!m) return value;
  const decimals = (m[3] ?? "").replace(/0+$/, "").padEnd(2, "0");
  const negative = m[1] === "-" && /[1-9]/.test(m[2]! + decimals);
  const symbol = CURRENCY_SYMBOL[currency] ?? currency;
  return `${negative ? "-" : ""}${groupThousands(m[2]!)},${decimals} ${symbol}`;
}

/** Cantidad o porcentaje en formato español, sin ceros sobrantes: `2.50` -> `2,5`. */
export function formatNumber(value: string | null | undefined): string {
  if (value === null || value === undefined || value === "") return "—";
  const m = /^(-?)(\d+)(?:\.(\d+))?$/.exec(value.trim());
  if (!m) return value;
  const decimals = (m[3] ?? "").replace(/0+$/, "");
  return `${m[1]}${groupThousands(m[2]!)}${decimals ? `,${decimals}` : ""}`;
}

/**
 * Lo que escribe la persona -> cadena decimal con punto para el backend, o `null` si no es un
 * número. Acepta `1.234,50`, `1234,5`, `1234.50` y `-12,5`. Con un solo punto y sin coma, el
 * punto es decimal (`12.5`); con varios puntos y sin coma, son miles (`1.234.567`).
 */
export function parseDecimalInput(text: string, options: { negative?: boolean } = {}): string | null {
  let t = text.trim().replace(/\s|€/g, "");
  if (!t) return null;
  let sign = "";
  if (t.startsWith("-")) {
    if (!options.negative) return null;
    sign = "-";
    t = t.slice(1);
  }
  const comma = t.indexOf(",");
  let integer: string;
  let fraction = "";
  if (comma >= 0) {
    if (t.lastIndexOf(",") !== comma) return null;
    integer = t.slice(0, comma);
    fraction = t.slice(comma + 1);
    if (integer.includes(".")) {
      if (!/^\d{1,3}(\.\d{3})+$/.test(integer)) return null;
      integer = integer.replace(/\./g, "");
    }
  } else {
    const dots = (t.match(/\./g) ?? []).length;
    if (dots > 1) {
      if (!/^\d{1,3}(\.\d{3})+$/.test(t)) return null;
      integer = t.replace(/\./g, "");
    } else if (dots === 1) {
      [integer, fraction] = t.split(".") as [string, string];
    } else {
      integer = t;
    }
  }
  if (!/^\d*$/.test(integer) || !/^\d*$/.test(fraction)) return null;
  if (integer === "" && fraction === "") return null;
  integer = integer.replace(/^0+(?=\d)/, "") || "0";
  fraction = fraction.replace(/0+$/, "");
  const out = fraction ? `${integer}.${fraction}` : integer;
  return sign && /[1-9]/.test(out) ? `-${out}` : out;
}

/** Desplaza la coma decimal sin pasar por `Number`; elimina ceros sobrantes. */
export function shiftDecimal(value: string, places: number): string {
  const m = /^(\d+)(?:\.(\d+))?$/.exec(value);
  if (!m) return value;
  let digits = m[1]! + (m[2] ?? "");
  let point = m[1]!.length + places;
  if (point < 0) {
    digits = "0".repeat(-point) + digits;
    point = 0;
  } else if (point > digits.length) {
    digits += "0".repeat(point - digits.length);
  }
  const integer = digits.slice(0, point).replace(/^0+(?=\d)/, "") || "0";
  const fraction = digits.slice(point).replace(/0+$/, "");
  return fraction ? `${integer}.${fraction}` : integer;
}

/** Descuento: la persona escribe un porcentaje (`10`) y el backend espera una fracción (`0.1`). */
export function percentToFraction(percent: string): string | null {
  const value = parseDecimalInput(percent);
  return value === null ? null : shiftDecimal(value, -2);
}

export function fractionToPercent(fraction: string | null | undefined): string {
  if (!fraction) return "";
  const shifted = shiftDecimal(fraction, 2);
  return shifted === "0" ? "" : shifted.replace(".", ",");
}

// ---- Validaciones previas a guardar (el backend valida siempre; esto evita viajes inútiles) ----

export interface LineDraftText {
  description: string;
  quantity: string;
  unit_price: string;
  discount_percent: string;
  tax_percent: string;
  qualification: SalesQualification | "";
  exemption_text: string;
}

export interface LineValidation {
  ok: boolean;
  problem?: string;
  line?: {
    description: string;
    quantity: string;
    unit_price: string;
    discount: string | null;
    tax_percent: string;
    exemption: { qualification: SalesQualification; description: string | null } | null;
  };
}

/**
 * Línea que la persona no ha empezado a rellenar (sin descripción, precio ni descuento): se
 * ignora al guardar, para que la línea vacía con que abre un documento nuevo no impida añadir
 * un producto del catálogo (visto en tauri dev).
 */
export function isBlankLine(text: LineDraftText): boolean {
  return !text.description.trim() && !text.unit_price.trim() && !text.discount_percent.trim();
}

/** Traduce los textos de una línea al formato del backend o dice qué falta. */
export function validateLine(text: LineDraftText): LineValidation {
  if (!text.description.trim()) return { ok: false, problem: "Falta la descripción" };
  // Signo admitido: una rectificativa por diferencias necesita cantidades negativas; en los
  // demás documentos el backend rechaza la cantidad no positiva con su propio mensaje.
  const quantity = parseDecimalInput(text.quantity, { negative: true });
  if (quantity === null) return { ok: false, problem: "La cantidad no es un número" };
  const unit = parseDecimalInput(text.unit_price, { negative: true });
  if (unit === null) return { ok: false, problem: "El precio no es un número" };
  let discount: string | null = null;
  if (text.discount_percent.trim()) {
    const percent = parseDecimalInput(text.discount_percent);
    if (percent === null) return { ok: false, problem: "El descuento no es un porcentaje" };
    const fraction = shiftDecimal(percent, -2);
    if (fraction !== "1" && !fraction.startsWith("0")) return { ok: false, problem: "El descuento debe estar entre 0 y 100 %" };
    discount = fraction;
  }
  const zero = text.tax_percent === "0";
  if (zero && !text.qualification) return { ok: false, problem: "Con IVA 0 % indique la causa: exención, no sujeción o inversión" };
  return {
    ok: true,
    line: {
      description: text.description.trim(),
      quantity,
      unit_price: unit,
      discount,
      tax_percent: text.tax_percent,
      exemption: zero && text.qualification ? { qualification: text.qualification, description: text.exemption_text.trim() || null } : null,
    },
  };
}

/** Texto de la causa de una línea exenta para las tablas. */
export function exemptionText(exemption: { qualification: SalesQualification; description: string | null } | null): string {
  if (!exemption) return "";
  return exemption.description ? `${exemption.qualification} · ${exemption.description}` : QUALIFICATION_LABEL[exemption.qualification];
}

export const EMPTY_LIST = "Todavía no hay documentos. Cree un borrador con «Nuevo documento».";
export const NO_TOTALS = "Los totales aparecerán cuando el borrador tenga al menos una línea válida y se guarde.";
export const VERIFACTU_READONLY = "Vista de solo lectura: los registros no se pueden editar ni borrar. Se crean al emitir o anular una factura.";

// ---- Membrete PDF de fondo (D222, D223) ----

/** Tope del PDF de membrete: el mismo que comprueba el backend. */
export const LETTERHEAD_MAX_BYTES = 5 * 1024 * 1024;

export const LETTERHEAD_HELP =
  "El membrete es un PDF propio (su papel de carta o diseño) de una o dos páginas A4 verticales, sin contraseña y de hasta 5 MB. La primera página es el fondo de la primera hoja de cada presupuesto, albarán y factura; la segunda, si existe, el de las siguientes, y si no, se repite la primera. Con membrete no se dibuja el logotipo.";

export const LETTERHEAD_MARGINS_HELP =
  "Los márgenes marcan el área útil, en milímetros: NestAdmin escribe los datos, las líneas y los totales dentro de ella para no pisar el diseño. Elija márgenes que dejen libre lo que su membrete ya imprime.";

export const LETTERHEAD_WARNING =
  "El membrete no oculta ni sustituye ningún dato obligatorio: el emisor, el cliente, los importes, el código QR y la leyenda VERI*FACTU se dibujan siempre encima.";

export const LETTERHEAD_SCOPE = "Un cambio de membrete o de márgenes solo afecta a los documentos que se emitan después: los ya emitidos conservan el suyo y su PDF no cambia.";

export const LETTERHEAD_REMOVE_CONFIRM = "Se quitará el membrete de la empresa. Los documentos ya emitidos conservan el suyo y los nuevos se dibujarán con el diseño normal y el logotipo.\n\n¿Quitarlo?";

export const MARGIN_FIELDS = [
  ["top_mm", "Margen superior (mm)"],
  ["right_mm", "Margen derecho (mm)"],
  ["bottom_mm", "Margen inferior (mm)"],
  ["left_mm", "Margen izquierdo (mm)"],
] as const;

export type MarginKey = (typeof MARGIN_FIELDS)[number][0];
export type MarginsText = Record<MarginKey, string>;
export type MarginsNumbers = Record<MarginKey, number>;

/** Lo que comprueba la ventana antes de enviar el archivo; PDFium decide el resto en el backend. */
export function letterheadFileProblem(file: { name: string; size: number; type: string } | null | undefined): string | null {
  if (!file) return "Elija primero un archivo PDF.";
  if (file.type !== "application/pdf" && !/\.pdf$/i.test(file.name)) return "El membrete debe ser un archivo PDF.";
  if (file.size === 0) return "El archivo está vacío.";
  if (file.size > LETTERHEAD_MAX_BYTES) return "El PDF supera los 5 MB.";
  return null;
}

export function marginsToText(margins: MarginsNumbers): MarginsText {
  return { top_mm: String(margins.top_mm), right_mm: String(margins.right_mm), bottom_mm: String(margins.bottom_mm), left_mm: String(margins.left_mm) };
}

/**
 * Lee los cuatro márgenes (milímetros enteros de 0 a 100) y comprueba que el área útil no baja de
 * 100 x 150 mm en una hoja de 210 x 297 mm, como el backend. Devuelve los números o los problemas,
 * uno por campo o general.
 */
export function parseMargins(text: MarginsText): { ok: true; margins: MarginsNumbers } | { ok: false; problems: string[] } {
  const problems: string[] = [];
  const margins = {} as MarginsNumbers;
  for (const [key, label] of MARGIN_FIELDS) {
    const raw = text[key].trim();
    if (!/^\d{1,3}$/.test(raw)) {
      problems.push(`${label}: escriba milímetros enteros, de 0 a 100.`);
      continue;
    }
    const value = Number(raw);
    if (value > 100) {
      problems.push(`${label}: el máximo es 100 mm.`);
      continue;
    }
    margins[key] = value;
  }
  if (problems.length) return { ok: false, problems };
  if (210 - margins.left_mm - margins.right_mm < 100) problems.push("Los márgenes izquierdo y derecho dejan menos de 100 mm de ancho útil.");
  if (297 - margins.top_mm - margins.bottom_mm < 150) problems.push("Los márgenes superior e inferior dejan menos de 150 mm de alto útil.");
  return problems.length ? { ok: false, problems } : { ok: true, margins };
}

/** «Ver PDF» solo existe para un documento emitido: un borrador no tiene PDF. */
export function canViewPdf(doc: Pick<SalesDocument, "status" | "number">): boolean {
  return doc.status !== "draft" && doc.number !== null;
}

export const VIEW_PDF_LABEL = "Ver PDF";
