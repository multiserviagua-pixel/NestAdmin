// Textos del panel de reglas (R21). Solo funciones y datos puros, sin DOM, para poder
// probarlos con `node --test`. Solo tipos borrables (sin `enum`): Node los ejecuta tal cual.
// Todos los ejemplos son inventados.

export interface RuleLike {
  id: string;
  version: number;
  action_kind: string;
  conditions: Record<string, unknown>;
  active: boolean;
  /** Marca de la opción avanzada (D126): la regla de envío contesta a cualquier remitente. */
  any_sender_ack_at?: number | null;
  archived_at?: number | null;
  name?: string;
}

export const CATEGORY_LABEL: Record<string, string> = {
  solicitud_factura: "Solicitud de factura",
  consulta_pago: "Consulta de pago",
  incidencia_factura: "Incidencia de factura",
  documentacion: "Documentación",
  otros: "Otros",
};

export const ACTION_TEXT: Record<string, string> = {
  register_invoice: "Registrar factura",
  send_draft: "Enviar respuesta",
};

export interface CategoryHelp {
  description: string;
  example: string;
}

export const CATEGORY_HELP: Record<string, CategoryHelp> = {
  solicitud_factura: {
    description: "Quien escribe pide una factura o una copia de una factura.",
    example: "«Buenos días: ¿pueden enviarme la factura del pedido 2231, de 1.250,00 €?»",
  },
  consulta_pago: {
    description: "Quien escribe pregunta por el estado de un pago, si se ha recibido o cuándo se hará.",
    example: "«¿Han recibido la transferencia de 480,00 € que hicimos el día 3?»",
  },
  incidencia_factura: {
    description: "Quien escribe señala un error o un problema en una factura ya emitida o recibida.",
    example: "«La factura F-2026-014 tiene mal el IVA: debería ser de 210,00 €, no de 120,00 €.»",
  },
  documentacion: {
    description: "Quien escribe envía o pide documentos o certificados que no son una factura.",
    example: "«Le adjunto el certificado de estar al corriente de pago; ¿podrían confirmarnos la recepción?»",
  },
  otros: {
    description: "Cualquier correo que no encaja en las categorías anteriores.",
    example: "«Les informamos de que el horario de recogida cambia a partir del lunes.»",
  },
};

/** Ayuda de cada campo del formulario y de cada columna de la lista. */
export const FIELD_HELP: Record<string, string> = {
  name: "Obligatorio. Nombre claro para reconocer la regla en la lista y en el historial, por ejemplo «Facturas de Alfa hasta 500 €». Puede cambiarlo cuando quiera. El identificador interno se genera solo a partir del nombre.",
  id: "Nombre corto interno de la regla. Se genera solo a partir del nombre y no se puede cambiar. Aparece en el historial.",
  action:
    "Qué hará la regla cuando coincida: registrar una factura en el sistema administrativo o enviar la respuesta que redactó la IA. No se puede cambiar después.",
  supplier:
    "Obligatorio. Nombre del proveedor tal como lo lee la aplicación en la factura. Debe coincidir exactamente, sin importar mayúsculas: «Alfa SL» no coincide con «Alfa, S.L.». Si el nombre no coincide, la factura se queda en Pendientes para que usted la revise.",
  max_total:
    "Opcional. Importe máximo, en euros, hasta el cual se registra sin revisión; escriba, por ejemplo, 500.00. Las facturas con un total mayor se quedan en Pendientes. Hoy la aplicación solo registra facturas en euros: si el documento menciona otra moneda, se revisa siempre. Sin máximo, se registrarán sin revisión facturas de cualquier importe de ese proveedor.",
  category:
    "Categoría en la que la IA clasifica el correo recibido. La regla se aplica a los correos que la IA clasifique así y que vengan del remitente que usted indique.",
  scope:
    "Obligatorio. Con quién se usa la regla. «Una dirección exacta» solo vale para los correos de esa dirección. «Todo un dominio» vale para cualquier dirección que acabe en @ese-dominio (sin subdominios). El remitente de un correo se puede suplantar: indicarlo reduce el riesgo, no lo elimina.",
  sender: "Dirección completa del remitente, por ejemplo compras@cliente.test. No distingue mayúsculas de minúsculas.",
  sender_domain:
    "Parte de la dirección que va después de la @, por ejemplo cliente.test. Vale para cualquier dirección de ese dominio, pero no para sus subdominios.",
  advanced:
    "Para casos excepcionales: permite que una regla de envío conteste a cualquier remitente. Exige una confirmación expresa y se puede retirar en cualquier momento.",
  search: "Busca por el nombre, el texto de la regla, su identificador, el remitente, el proveedor o la categoría.",
  filter_action: "Muestra solo las reglas que registran facturas o solo las que envían respuestas.",
  filter_status: "Una regla inactiva no hace nada: solo una regla activa puede actuar sin revisión.",
  filter_category: "Muestra solo las reglas de envío que se aplican a los correos de esta categoría.",
  filter_archived:
    "Las reglas archivadas no actúan ni se pueden editar, pero su historial se conserva. Por defecto no se muestran.",
  rules_list:
    "Cada tarjeta es una regla. El nombre y la frase dicen lo que hará; el aviso recuerda su riesgo. «Activa» puede actuar sin revisión, «Inactiva» no hace nada y «No actúa» indica que la regla está incompleta. «Detalles» muestra el identificador y la versión (un número interno que también sube al activar o desactivar). «Más» reúne las acciones menos habituales. Una regla nueva nace inactiva hasta que usted la active.",
  stats:
    "«Aplicada» cuenta las veces que la regla ha registrado o enviado algo sin revisión. «Pasó a revisión» cuenta las tareas que la regla habría automatizado pero que se quedaron en Pendientes por otro motivo, por ejemplo una incidencia. «Sin clasificar» son acciones de versiones anteriores de la aplicación que citan la regla de una forma que no se puede leer.",
  category_name: "Clasificación que la IA asigna a cada correo recibido. Son cinco y no se pueden cambiar.",
  category_count: "Correos que constan en este equipo con esa categoría, incluidos los ya tratados.",
  category_rules: "Reglas de envío que se aplican a los correos de esta categoría.",
  history_filter: "Muestra solo las acciones de una regla, o las que aprobó una persona. Incluye también los cambios de esa regla.",
};

const FALLBACK_UNKNOWN = "Tipo de regla no reconocido";

/** «500.00» -> «500,00 €»; «1234.5» -> «1.234,50 €». Lo que no es un importe se deja tal cual. */
export function formatAmount(raw: string): string {
  const text = raw.trim();
  const dot = /^(\d{1,12})(?:\.(\d{1,2}))?$/.exec(text);
  const comma = /^(\d{1,3}(?:\.\d{3})*|\d+),(\d{1,2})$/.exec(text);
  let whole: string;
  let cents: string;
  if (dot) {
    whole = dot[1];
    cents = dot[2] ?? "";
    whole = whole.replace(/\B(?=(\d{3})+(?!\d))/g, ".");
  } else if (comma) {
    whole = comma[1];
    cents = comma[2];
    if (!whole.includes(".")) whole = whole.replace(/\B(?=(\d{3})+(?!\d))/g, ".");
  } else {
    return text;
  }
  return `${whole},${cents.padEnd(2, "0")} €`;
}

function text(value: unknown): string {
  return typeof value === "string" ? value.trim() : "";
}

export function categoryLabel(category: string): string {
  return CATEGORY_LABEL[category] ?? category;
}

export function isKnownAction(kind: string): boolean {
  return kind === "register_invoice" || kind === "send_draft";
}

// ---- Remitente (D126) ----

/** Dominios de correo público, igual que `is_public_mail_domain` en `crates/app/src/rules.rs`
 * (una prueba comprueba que las dos listas coinciden). */
export const PUBLIC_MAIL_DOMAINS: readonly string[] = [
  "gmail.com",
  "googlemail.com",
  "outlook.com",
  "outlook.es",
  "hotmail.com",
  "hotmail.es",
  "live.com",
  "msn.com",
  "yahoo.com",
  "yahoo.es",
  "ymail.com",
  "icloud.com",
  "me.com",
  "mac.com",
  "aol.com",
  "proton.me",
  "protonmail.com",
  "gmx.com",
  "gmx.es",
  "gmx.net",
  "mail.com",
  "zoho.com",
  "yandex.com",
  "telefonica.net",
  "movistar.es",
  "orange.es",
];

/** Dominio sin la `@` inicial ni espacios. */
export function cleanDomain(value: string): string {
  return value.trim().replace(/^@+/, "").trim();
}

export function isPublicMailDomain(domain: string): boolean {
  return PUBLIC_MAIL_DOMAINS.includes(cleanDomain(domain).toLowerCase());
}

/** Aviso de un dominio de correo público, o `null`. */
export function publicDomainWarning(domain: string): string | null {
  const d = cleanDomain(domain).toLowerCase();
  if (!isPublicMailDomain(d)) return null;
  const who = d === "gmail.com" ? "cualquier usuario de Gmail" : `cualquier usuario de ${d}`;
  return `${d} abarca a ${who}, no solo a una empresa. Si quiere acotar más, indique una dirección exacta.`;
}

export type AmountResult = { ok: true; value: string } | { ok: false; message: string };

/** Importe como lo guarda la regla: `500` -> `500.00`, `500,5` -> `500.50`, `1.234,50` ->
 * `1234.50`. Lo ambiguo (`1.234`: ¿mil doscientos treinta y cuatro o 1,234?) se rechaza. */
export function normalizeAmount(raw: string): AmountResult {
  const t = raw.trim().replace(/\s*(€|EUR)$/i, "");
  const bad = (message: string): AmountResult => ({ ok: false, message });
  if (/^\d{1,12}$/.test(t)) return { ok: true, value: `${t}.00` };
  let m = /^(\d{1,12})[.,](\d{1,2})$/.exec(t);
  if (m) return { ok: true, value: `${m[1]}.${m[2].padEnd(2, "0")}` };
  m = /^(\d{1,3}(?:\.\d{3})+),(\d{1,2})$/.exec(t);
  if (m) return { ok: true, value: `${m[1].replace(/\./g, "")}.${m[2].padEnd(2, "0")}` };
  m = /^(\d{1,3}(?:,\d{3})+)\.(\d{1,2})$/.exec(t);
  if (m) return { ok: true, value: `${m[1].replace(/,/g, "")}.${m[2].padEnd(2, "0")}` };
  if (/^\d{1,3}([.,]\d{3})+$/.test(t) || /^\d+[.,]\d{3,}$/.test(t))
    return bad(`«${t}» es ambiguo: no queda claro si son miles o decimales. Escriba, por ejemplo, 1234.00 o 1.234,00.`);
  return bad("El total máximo debe ser un importe en euros, por ejemplo 500.00.");
}

/** Importe tal como lo exige el backend (`parse_amount`): dos decimales y, si hay miles, un solo
 * separador distinto del decimal. Una regla guardada con otra forma no coincide nunca. */
export function isStoredAmount(value: string): boolean {
  const t = value.trim().replace(/\s*(€|EUR)$/, "");
  return (
    /^\d+[.,]\d{2}$/.test(t) ||
    /^\d{1,3}(\.\d{3})+,\d{2}$/.test(t) ||
    /^\d{1,3}(,\d{3})+\.\d{2}$/.test(t) ||
    /^\d{1,3}([\s\u00a0]\d{3})+[.,]\d{2}$/.test(t)
  );
}

export function isEmailAddress(value: string): boolean {
  return /^[^\s@,<>;()"]{1,64}@[A-Za-z0-9-]+(\.[A-Za-z0-9-]+)+$/.test(value.trim());
}

export function isDomainName(value: string): boolean {
  return /^[A-Za-z0-9-]+(\.[A-Za-z0-9-]+)+$/.test(cleanDomain(value));
}

export type SenderKind = "sender" | "domain" | "any" | "missing";

/** A quién contesta una regla de envío: dirección, dominio, cualquiera (opción avanzada) o nadie. */
export function senderKind(rule: RuleLike): SenderKind {
  const c = rule.conditions ?? {};
  if (text(c.sender)) return "sender";
  if (text(c.sender_domain)) return "domain";
  return rule.any_sender_ack_at ? "any" : "missing";
}

/** Una regla de envío antigua sin remitente ni marca avanzada no actúa (D126). */
export function actsOnNothing(rule: RuleLike): boolean {
  return rule.action_kind === "send_draft" && senderKind(rule) === "missing";
}

export const NOT_ACTING = "No actúa: falta el remitente";

/** Frase que describe lo que hace la regla. No inventa datos que la regla no tiene. */
export function ruleSentence(rule: RuleLike): string {
  const c = rule.conditions ?? {};
  if (rule.action_kind === "register_invoice") {
    const supplier = text(c.supplier);
    const max = text(c.max_total);
    const who = supplier ? `de ${supplier}` : "(la regla no indica proveedor)";
    return `Registrar sin revisión las facturas ${who}${max ? ` de hasta ${formatAmount(max)}` : ""}`;
  }
  if (rule.action_kind === "send_draft") {
    const category = text(c.category);
    const when = category ? `cuando se clasifique como ${categoryLabel(category)}` : "(la regla no indica categoría)";
    const base = "Enviar sin revisión la respuesta redactada por la IA";
    switch (senderKind(rule)) {
      case "sender":
        return `${base} a ${text(c.sender)} ${when}`;
      case "domain":
        return `${base} a cualquier dirección de @${cleanDomain(text(c.sender_domain))} ${when}`;
      case "any":
        return `${base} a cualquier remitente ${when}`;
      default:
        return `${base} ${when}, sin remitente indicado`;
    }
  }
  return FALLBACK_UNKNOWN;
}

/** Aviso que debe verse siempre junto a la regla. Dice la verdad sobre a quién alcanza. */
export function ruleRisk(rule: RuleLike): string {
  if (rule.action_kind === "send_draft") {
    const c = rule.conditions ?? {};
    const spoof = "El remitente de un correo se puede suplantar: indicarlo reduce el riesgo, no lo elimina.";
    switch (senderKind(rule)) {
      case "sender":
        return `Enviará sin preguntarle la respuesta de la IA a los correos de ${text(c.sender)} que la IA clasifique en esta categoría. ${spoof}`;
      case "domain": {
        const domain = cleanDomain(text(c.sender_domain));
        const warning = publicDomainWarning(domain);
        return `Enviará sin preguntarle la respuesta de la IA a los correos de cualquier dirección de @${domain} que la IA clasifique en esta categoría. ${warning ? `${warning} ` : ""}${spoof}`;
      }
      case "any":
        return "Contesta a cualquier remitente: cualquier persona, también un desconocido, que escriba un correo que la IA clasifique en esta categoría recibe sin revisión la respuesta redactada por la IA.";
      default:
        return `${NOT_ACTING}. Añada una dirección o un dominio para que pueda actuar.`;
    }
  }
  if (rule.action_kind === "register_invoice")
    return "Registrará la factura sin preguntarle si coincide y no tiene incidencias. Hoy solo se registran facturas en euros.";
  return "La aplicación no reconoce este tipo de regla, así que no la aplicará.";
}

export interface StatusChip {
  text: string;
  tone: "neutral" | "ok" | "warn";
}

/** Estado de la regla para la lista: una regla de envío sin remitente ni marca no actúa. */
export function ruleStatus(rule: RuleLike): { primary: StatusChip; extra: StatusChip | null } {
  if (rule.archived_at) return { primary: { text: "Archivada", tone: "neutral" }, extra: null };
  if (actsOnNothing(rule)) {
    return {
      primary: { text: NOT_ACTING, tone: "warn" },
      extra: { text: rule.active ? "Activa" : "Inactiva", tone: "neutral" },
    };
  }
  if (isKnownAction(rule.action_kind) && ruleProblem(rule)) {
    return {
      primary: { text: "No actúa: la regla tiene un problema", tone: "warn" },
      extra: { text: rule.active ? "Activa" : "Inactiva", tone: "neutral" },
    };
  }
  return { primary: { text: rule.active ? "Activa" : "Inactiva", tone: rule.active ? "ok" : "neutral" }, extra: null };
}

/** Problema de la regla que impide que actúe (fallo cerrado, D123), o `null`. */
export function ruleProblem(rule: RuleLike): string | null {
  const c = rule.conditions;
  if (!c || typeof c !== "object" || Array.isArray(c)) return "Las condiciones de la regla no son válidas: no actuará.";
  if (!isKnownAction(rule.action_kind)) return null;
  const allowed = rule.action_kind === "register_invoice" ? ["supplier", "max_total", "currency"] : ["category", "sender", "sender_domain"];
  const unknown = Object.keys(c).find((k) => !allowed.includes(k));
  if (unknown) return `La regla tiene una condición desconocida («${unknown}»): no actuará.`;
  if (rule.action_kind === "register_invoice" && !text(c.supplier)) return "La regla no indica proveedor: no actuará.";
  if (rule.action_kind === "register_invoice" && "max_total" in c && (typeof c.max_total !== "string" || !isStoredAmount(c.max_total)))
    return `El máximo de la regla${typeof c.max_total === "string" ? ` («${c.max_total}»)` : ""} no es un importe válido: no actuará.`;
  if (rule.action_kind === "register_invoice" && "currency" in c && c.currency !== "EUR")
    return "Hoy la aplicación solo registra facturas en euros: la regla no actuará.";
  if (rule.action_kind === "send_draft" && !text(c.category)) return "La regla no indica categoría: no actuará.";
  if (rule.action_kind === "send_draft" && text(c.sender) && text(c.sender_domain))
    return "La regla indica a la vez una dirección y un dominio: no actuará.";
  return null;
}

/** Condiciones en una línea corta, para la columna «Condiciones». */
export function conditionsSummary(rule: RuleLike): string {
  const c = rule.conditions ?? {};
  if (rule.action_kind === "register_invoice") {
    const supplier = text(c.supplier);
    const max = text(c.max_total);
    return `Proveedor ${supplier ? `«${supplier}»` : "sin indicar"}${max ? `, total ≤ ${formatAmount(max)}` : ", sin máximo"}, en euros, sin incidencias`;
  }
  if (rule.action_kind === "send_draft") {
    const category = text(c.category);
    const from = {
      sender: `remitente ${text(c.sender)}`,
      domain: `dominio @${cleanDomain(text(c.sender_domain))}`,
      any: "cualquier remitente (opción avanzada)",
      missing: "sin remitente indicado",
    }[senderKind(rule)];
    return `Categoría ${category ? `«${categoryLabel(category)}»` : "sin indicar"}, ${from}, con destinatario`;
  }
  return "Sin datos comparables";
}

/** Texto en minúsculas y sin acentos sobre el que busca el buscador. */
export function searchText(rule: RuleLike): string {
  const c = rule.conditions ?? {};
  const category = text(c.category);
  return normalizeSearch(
    [ruleSentence(rule), rule.name ?? "", rule.id, text(c.supplier), text(c.sender), text(c.sender_domain), category, category ? categoryLabel(category) : ""].join(" "),
  );
}

export function normalizeSearch(value: string): string {
  return value.normalize("NFD").replace(/[̀-ͯ]/g, "").toLowerCase();
}

// ---- Borrador del formulario (D124, D126, D127) ----

export type SenderScope = "sender" | "domain" | "any";

export interface RuleDraft {
  id: string;
  name: string;
  kind: string;
  supplier: string;
  max_total: string;
  /** Moneda guardada en la regla; el formulario no la edita, pero no la pierde. */
  currency: string;
  category: string;
  scope: SenderScope;
  sender: string;
  sender_domain: string;
  /** Confirmación de la opción avanzada, en dos pasos: casilla y texto. */
  ack: boolean;
  ack_text: string;
}

export const ANY_SENDER_CONFIRMATION = "CUALQUIER REMITENTE";

/** Consecuencias de contestar a cualquier remitente, con la categoría de la regla. */
export function anySenderConsequences(category: string): string {
  const which = category ? categoryLabel(category) : "la categoría elegida";
  return `Cualquiera, también un desconocido, que escriba un correo que la IA clasifique como ${which} recibirá sin revisión la respuesta redactada por la IA. Esa persona puede influir en la clasificación con lo que escribe. Puede desactivarla en cualquier momento, pero lo enviado no se puede deshacer.`;
}

export function anyAckReady(d: Pick<RuleDraft, "ack" | "ack_text">): boolean {
  return d.ack && d.ack_text.trim() === ANY_SENDER_CONFIRMATION;
}

/** Borrador para editar o duplicar una regla. Conserva remitente, dominio y moneda. */
export function draftOf(rule: RuleLike | null): RuleDraft {
  const c = rule?.conditions ?? {};
  const kind = rule?.action_kind ?? "register_invoice";
  const scope: SenderScope = text(c.sender)
    ? "sender"
    : text(c.sender_domain)
      ? "domain"
      : rule && kind === "send_draft" && rule.any_sender_ack_at
        ? "any"
        : "sender";
  return {
    id: rule?.id ?? "",
    name: rule?.name && rule.name !== rule.id ? rule.name : "",
    kind,
    supplier: text(c.supplier),
    max_total: text(c.max_total),
    currency: text(c.currency),
    category: text(c.category) || Object.keys(CATEGORY_LABEL)[0],
    scope,
    sender: text(c.sender),
    sender_domain: text(c.sender_domain),
    ack: false,
    ack_text: "",
  };
}

/** Condiciones que se envían al guardar. Solo las de su tipo de regla. */
export function draftConditions(d: RuleDraft): Record<string, string> {
  if (d.kind === "register_invoice") {
    const amount = normalizeAmount(d.max_total);
    return Object.fromEntries(
      Object.entries({
        supplier: d.supplier.trim(),
        max_total: d.max_total.trim() ? (amount.ok ? amount.value : d.max_total.trim()) : "",
        currency: d.currency.trim(),
      }).filter(([, v]) => v),
    );
  }
  const out: Record<string, string> = { category: d.category };
  if (d.scope === "sender" && d.sender.trim()) out.sender = d.sender.trim();
  if (d.scope === "domain" && cleanDomain(d.sender_domain)) out.sender_domain = cleanDomain(d.sender_domain);
  return out;
}

/** Lo que falta o está mal en el borrador, en palabras claras. Vacío: se puede guardar. */
export function draftProblems(d: RuleDraft, creating: boolean, options: { ignoreAck?: boolean } = {}): string[] {
  const out: string[] = [];
  if (creating && !d.name.trim()) out.push("Escriba un nombre para la regla.");
  if (d.name.trim().length > 80) out.push("El nombre admite hasta 80 caracteres.");
  if (d.kind === "register_invoice") {
    if (!d.supplier.trim()) out.push("Indique el proveedor.");
    if (d.max_total.trim()) {
      const amount = normalizeAmount(d.max_total);
      if (!amount.ok) out.push(amount.message);
    }
  } else {
    if (d.scope === "sender" && !isEmailAddress(d.sender)) out.push("Indique la dirección completa del remitente, por ejemplo compras@cliente.test.");
    if (d.scope === "domain" && !isDomainName(d.sender_domain)) out.push("Indique el dominio, por ejemplo cliente.test (sin comodines ni @ en medio).");
    if (d.scope === "any" && !options.ignoreAck && !anyAckReady(d)) out.push(`Para contestar a cualquier remitente, marque «Entiendo las consecuencias» y escriba ${ANY_SENDER_CONFIRMATION}.`);
  }
  return out;
}

/** Aviso al editar una regla de envío con la opción avanzada o que pasa a tenerla: guardar borra
 * la validación (D126) y, si la regla está activa, empieza a contestar a cualquiera. */
export function anySenderEditNotice(rule: RuleLike | null, d: RuleDraft): string | null {
  if (!rule || rule.action_kind !== "send_draft") return null;
  const parts: string[] = [];
  if (rule.any_sender_ack_at) {
    parts.push(
      d.scope === "any"
        ? "Esta regla tiene validada la opción avanzada. Guardar cualquier cambio borra esa validación: dejará de enviar hasta que vuelva a confirmarla (más abajo, en «Opciones avanzadas»)."
        : "Esta regla tenía validada la opción avanzada. Al indicar un remitente ya no la necesita y la validación se borra al guardar.",
    );
  }
  if (d.scope === "any" && rule.active) parts.push("Esta regla está activa: empezará a contestar a cualquier remitente en cuanto la guarde.");
  return parts.length ? parts.join(" ") : null;
}

// ---- Contadores (D129) ----

export interface StatsLike {
  applied: number;
  last_applied_at: number | null;
  reviewed: number;
  unclassified: number;
}

function times(n: number): string {
  return n === 1 ? "1 vez" : `${n} veces`;
}

/** «03/11/2026» a partir de milisegundos desde 1970 (como los guarda el backend), en hora local. */
export function formatDate(millis: number): string {
  const d = new Date(millis);
  const two = (n: number) => String(n).padStart(2, "0");
  return `${two(d.getDate())}/${two(d.getMonth() + 1)}/${d.getFullYear()}`;
}

/** Los contadores de una regla en una línea, y aparte lo que no se pudo clasificar. */
export function statsText(stats: StatsLike | null | undefined): { main: string; unclassified: string | null } {
  const s = stats ?? { applied: 0, last_applied_at: null, reviewed: 0, unclassified: 0 };
  const unclassified = s.unclassified > 0 ? `${s.unclassified} sin clasificar (versión anterior)` : null;
  if (s.applied === 0 && s.reviewed === 0) return { main: "Aún no se ha aplicado", unclassified };
  const parts: string[] = [];
  if (s.applied === 0) parts.push("Aún no se ha aplicado");
  else {
    parts.push(`Aplicada ${times(s.applied)}`);
    if (s.last_applied_at) parts.push(`última el ${formatDate(s.last_applied_at)}`);
  }
  parts.push(`pasó a revisión ${times(s.reviewed)}`);
  return { main: parts.join(" · "), unclassified };
}

// ---- Motivos de la evaluación (D125, R22) ----

/** Códigos de motivo de `Explanation` (`ReasonCode` en `crates/app/src/rules.rs`). */
export const REASON_CODES = [
  "automated",
  "no_active_rule_matches",
  "invoice_issues",
  "unreadable_fields",
  "no_recipient",
  "several_rules_match",
  "split_invoice",
  "amount_review",
  "sender_required",
  "foreign_currency",
  "currency_not_checked",
  "currency_not_shown",
] as const;

export type ReasonCode = (typeof REASON_CODES)[number];

function idList(value: unknown): string {
  return Array.isArray(value) ? value.filter((v): v is string => typeof v === "string" && v !== "").join(", ") : "";
}

/** Motivo en una cláusula en minúsculas («la regla no indica remitente»), con los datos del hecho
 * cuando constan. Un código desconocido se dice como tal, sin inventar. */
export function reasonText(code: string, facts?: Record<string, unknown> | null): string {
  const f = facts ?? {};
  switch (code as ReasonCode) {
    case "automated":
      return "una regla activa coincide y no hay nada que revisar";
    case "no_active_rule_matches":
      return "ninguna regla activa coincide";
    case "invoice_issues":
      return "la factura tiene incidencias";
    case "unreadable_fields":
      return "hay campos que no se han podido leer";
    case "no_recipient":
      return "el borrador no tiene destinatario";
    case "several_rules_match": {
      const n = typeof f.matching === "number" ? f.matching : null;
      return `${n ? `${n} reglas activas coinciden` : "coinciden varias reglas activas"} y la decisión no es inequívoca`;
    }
    case "split_invoice":
      return "la factura procede de un documento con varias facturas separadas por la IA";
    case "amount_review":
      return typeof f.message === "string" && f.message ? f.message : "algún importe es dudoso y debe confirmarlo una persona";
    case "sender_required": {
      const ids = idList(f.rule_ids);
      return `la regla no indica remitente; añádalo o valide la opción avanzada${ids ? ` (regla: ${ids})` : ""}`;
    }
    case "foreign_currency": {
      const list = idList(f.currencies);
      return `el documento menciona ${list || "otra moneda"}; hoy la aplicación solo registra facturas en euros`;
    }
    case "currency_not_checked":
      return "no consta la comprobación de moneda; reprocese o revise";
    case "currency_not_shown":
      return "el documento no muestra la moneda; no se puede comprobar que sea en euros";
    default:
      return `motivo no reconocido (${code})`;
  }
}

export function capitalize(value: string): string {
  return value ? value[0].toUpperCase() + value.slice(1) : value;
}

// ---- Simulación (D129) ----

export interface SimulationLike {
  considered: number;
  truncated: boolean;
  limit: number;
  skipped_without_result: number;
  without_currency_check: number;
  would_automate: number;
  would_review: Record<string, number>;
  overlaps: number;
  no_match: number;
  would_block_other: Array<{ rule_id: string; task_ids: number[] }>;
  items: Array<{
    task_id: number;
    label: string | null;
    outcome: string;
    reason_code: string | null;
    explanation: string;
    overlapping_rules: string[];
  }>;
}

export interface ReviewGroup {
  code: string;
  count: number;
  text: string;
  /** Tareas de esta causa, con la etiqueta mínima que consta. */
  examples: Array<{ task_id: number; label: string }>;
}

export interface SimulationSummary {
  empty: boolean;
  headline: string;
  automate: number;
  groups: ReviewGroup[];
  noMatch: number;
  blocked: Array<{ rule_id: string; count: number }>;
  notes: string[];
  automateExamples: Array<{ task_id: number; label: string }>;
}

function taskWord(kind: string, n: number): string {
  const one = kind === "register_invoice" ? "factura" : "correo";
  return n === 1 ? one : `${one}s`;
}

function exampleLabel(item: SimulationLike["items"][number]): { task_id: number; label: string } {
  return { task_id: item.task_id, label: item.label ?? "(sin nombre)" };
}

/** Resumen en lenguaje normal de una simulación, agrupado por motivo. */
export function simulationSummary(sim: SimulationLike, actionKind: string): SimulationSummary {
  const word = (n: number) => taskWord(actionKind, n);
  const notes: string[] = [];
  const groups: ReviewGroup[] = [];
  const reviewCodes = Object.keys(sim.would_review).sort((a, b) => sim.would_review[b] - sim.would_review[a]);
  for (const code of reviewCodes) {
    const count = sim.would_review[code];
    if (!count) continue;
    const items = sim.items.filter((i) => i.outcome === "review" && i.reason_code === code);
    groups.push({ code, count, text: reasonText(code), examples: items.map(exampleLabel) });
  }
  if (sim.overlaps > 0) {
    groups.push({
      code: "several_rules_match",
      count: sim.overlaps,
      text: reasonText("several_rules_match"),
      examples: sim.items.filter((i) => i.outcome === "overlap").map(exampleLabel),
    });
  }
  const blocked = sim.would_block_other.map((b) => ({ rule_id: b.rule_id, count: b.task_ids.length }));
  if (sim.without_currency_check > 0)
    notes.push(
      sim.without_currency_check === 1
        ? `1 ${word(1)} no se pudo comprobar en cuanto a la moneda (es anterior a esa comprobación): iría a revisión.`
        : `${sim.without_currency_check} ${word(sim.without_currency_check)} no se pudieron comprobar en cuanto a la moneda (son anteriores a esa comprobación): irían a revisión.`,
    );
  if (sim.truncated) notes.push(`Solo se han mirado las últimas ${sim.limit} tareas: hay más antiguas que no se han comprobado.`);
  if (sim.skipped_without_result > 0)
    notes.push(
      `${sim.skipped_without_result} ${sim.skipped_without_result === 1 ? "tarea no tiene" : "tareas no tienen"} resultado todavía (fallidas o sin procesar) y no se han contado.`,
    );
  const empty = sim.considered === 0;
  const reviewCount = groups.reduce((n, g) => n + g.count, 0);
  const headline = empty
    ? "No hay tareas con resultado para comprobar la regla: no se puede decir qué haría. Pruébela de nuevo cuando haya tareas."
    : `De las últimas ${sim.considered} ${word(sim.considered)} con resultado, la regla automatizaría ${sim.would_automate} y mandaría ${reviewCount} a revisión; ${sim.no_match} no coinciden con la regla.`;
  return {
    empty,
    headline,
    automate: sim.would_automate,
    groups,
    noMatch: sim.no_match,
    blocked,
    notes,
    automateExamples: sim.items.filter((i) => i.outcome === "automate").map(exampleLabel),
  };
}

export const SIMULATION_DISCLAIMER = "Es una prueba: no se ha enviado ni registrado nada.";

// ---- Por qué está aquí (R22) ----

export interface CandidateLike {
  id: string;
  version: number;
  active: boolean;
  matches: boolean;
  mismatch?: string | null;
  sender_required?: boolean;
}

export interface EvaluationLike {
  code: string;
  facts?: Record<string, unknown> | null;
  candidates: CandidateLike[];
}

export function isEvaluation(value: unknown): value is EvaluationLike {
  if (!value || typeof value !== "object") return false;
  const v = value as Record<string, unknown>;
  return typeof v.code === "string" && Array.isArray(v.candidates);
}

export type CandidateOutcome = "matched" | "would_apply" | "needs_sender" | "no_match";

export interface CandidateExplanation {
  id: string;
  version: number;
  outcome: CandidateOutcome;
  text: string;
}

export function sentenceEnd(value: string): string {
  const t = capitalize(value.trim());
  return /[.!?]$/.test(t) ? t : `${t}.`;
}

/** Cada regla comprobada, con si coincidía y el motivo concreto. Solo usa lo que guardó la evaluación. */
export const AUTOMATED_NOT_CONFIRMED =
  "una regla activa coincidía y se intentó la acción, pero no consta que se completara; por eso queda en revisión. Compruebe el aviso de recuperación de esta tarea antes de aprobarla";

export function explainEvaluation(
  evaluation: unknown,
  awaitingReview = false,
): { headline: string; candidates: CandidateExplanation[] } | null {
  if (!isEvaluation(evaluation)) return null;
  const several = evaluation.code === "several_rules_match";
  const rank: Record<CandidateOutcome, number> = { matched: 0, would_apply: 1, needs_sender: 2, no_match: 3 };
  const candidates = evaluation.candidates
    .map((c): CandidateExplanation => {
      let outcome: CandidateOutcome;
      let detail: string;
      if (c.matches && c.active) {
        outcome = "matched";
        detail = several ? "Coincidía, pero coincidían varias reglas activas y entonces no se automatiza." : "Coincidía.";
      } else if (c.matches) {
        outcome = "would_apply";
        detail = "Habría aplicado, pero está inactiva.";
      } else if (c.sender_required) {
        outcome = "needs_sender";
        detail = sentenceEnd(c.mismatch ?? reasonText("sender_required"));
      } else {
        outcome = "no_match";
        detail = `No coincidía: ${(c.mismatch ?? "sin motivo guardado").trim().replace(/\.$/, "")}.`;
      }
      return { id: c.id, version: c.version, outcome, text: detail };
    })
    .sort((a, b) => rank[a.outcome] - rank[b.outcome]);
  // Una tarea en revisión cuya evaluación dice «automatizada» es un intento sin confirmar.
  const reason = awaitingReview && evaluation.code === "automated" ? AUTOMATED_NOT_CONFIRMED : reasonText(evaluation.code, evaluation.facts);
  return { headline: sentenceEnd(reason), candidates };
}

export const NO_EVALUATION = "Esta tarea es anterior a este registro: no consta qué reglas se comprobaron.";

// ---- Historial por regla ----

export interface AuditLike {
  task_id: number | null;
  action: string;
  status: string;
  authorization: string | null;
  detail: string | null;
}

export const APPROVED_BY_PERSON = "__aprobacion__";
export const AUTHORIZED_BY_RULE = "__regla__";

/** ¿La entrada del historial es de esta regla? Acciones que autorizó y cambios de la propia regla. */
export function auditMentionsRule(entry: AuditLike, ruleId: string): boolean {
  if (entry.authorization && /^regla (.+) v\d+$/.exec(entry.authorization)?.[1] === ruleId) return true;
  if (entry.action.startsWith("rule")) {
    const detail = entry.detail ?? "";
    return detail.startsWith(`${ruleId} v`) || detail.startsWith(`${ruleId}:`);
  }
  return false;
}

export function auditMatches(entry: AuditLike, filter: string): boolean {
  if (!filter) return true;
  if (filter === APPROVED_BY_PERSON) return entry.authorization === "aprobación";
  if (filter === AUTHORIZED_BY_RULE) return /^regla .+ v\d+$/.test(entry.authorization ?? "");
  return auditMentionsRule(entry, filter);
}

// ---- Contradicciones y unificación (R23, D130, D131) ----
// Todo aviso explica qué pasa, por qué y qué consecuencia tiene, con datos concretos. La
// decisión de si dos reglas se pisan la toma el backend (`compare`/`analyze`); aquí solo se
// redacta lo que ese código ya decidió.

export interface FindingRuleLike {
  id: string;
  version: number;
  active: boolean;
}

export interface FindingLike {
  key: string;
  kind: "duplicate" | "overlap" | "similar" | "defective";
  action_kind: string;
  rules: FindingRuleLike[];
  /** Las dos reglas están activas: el choque ocurre ya. */
  active_conflict: boolean;
  effect: { code: string; data: Record<string, any> };
  merge_candidate: boolean;
  /** Tareas que ya coincidieron con las dos reglas; `null` si no aplica. */
  affected_tasks: number | null;
}

export const NO_FINDINGS = "No hay reglas que se contradigan";
export const NO_TASKS_AFFECTED = "Aún no hay tareas afectadas.";
/** Id de la regla hipotética del backend (`(simulada)`): en la interfaz es «esta regla». */
export const SIMULATED_RULE = "(simulada)";
export const MERGE_NOTICE = "La regla nueva quedará inactiva y las anteriores se archivarán; puede restaurarlas.";
export const MERGE_NEXT_STEP = "Pruébela y actívela cuando quiera";

export type NameOf = (id: string) => string;

function ref(id: string, nameOf: NameOf): string {
  return id === SIMULATED_RULE ? "esta regla" : `«${nameOf(id)}»`;
}

function plural(n: number, one: string, many: string): string {
  return n === 1 ? one : many;
}

function noun(actionKind: string, n: number): string {
  return actionKind === "register_invoice" ? plural(n, "factura", "facturas") : plural(n, "correo", "correos");
}

function upTo(limit: string): string {
  return formatAmount(limit);
}

/** Quién está cubierto por una condición de remitente. */
function who(scope: string, value: string | null): string {
  if (scope === "sender") return String(value ?? "");
  if (scope === "domain") return `cualquier dirección de @${cleanDomain(String(value ?? ""))}`;
  return "cualquier remitente";
}

export interface FindingText {
  title: string;
  badge: { text: string; tone: "warn" | "info" | "neutral" };
  /** Qué pasa. */
  what: string;
  /** Por qué pasa. */
  why: string;
  /** Qué consecuencia tiene. */
  consequence: string;
  /** Solo si el choque aún no ocurre porque alguna regla está inactiva. */
  timing: string | null;
  /** Recuento real de tareas afectadas, o la ausencia explícita; `null` si no aplica. */
  tasks: string | null;
}

function inactiveNote(f: FindingLike, nameOf: NameOf): string | null {
  if (f.active_conflict || f.kind === "similar" || f.kind === "defective") return null;
  const inactive = f.rules.filter((r) => !r.active);
  if (inactive.length >= f.rules.length) return "Todavía no ocurre: las dos reglas están inactivas. Ocurrirá en cuanto active las dos.";
  const names = inactive.map((r) => ref(r.id, nameOf)).join(" y ");
  return `Todavía no ocurre: ${names} ${inactive.length === 1 ? "está inactiva" : "están inactivas"}. Ocurrirá en cuanto ${inactive.length === 1 ? "la active" : "las active"}.`;
}

function tasksNote(f: FindingLike): string | null {
  if (f.kind !== "duplicate" && f.kind !== "overlap") return null;
  const n = f.affected_tasks ?? 0;
  if (n <= 0) return NO_TASKS_AFFECTED;
  const w = noun(f.action_kind, n);
  return `${n} ${w} ${plural(n, "ya coincidió", "ya coincidieron")} con las dos reglas, según el registro de evaluaciones.`;
}

/** Texto completo de un hallazgo: qué pasa, por qué y qué consecuencia tiene. */
export function explainFinding(f: FindingLike, nameOf: NameOf): FindingText {
  const d = f.effect.data;
  const [a, b] = f.rules.map((r) => ref(r.id, nameOf));
  const timing = inactiveNote(f, nameOf);
  const tasks = tasksNote(f);
  const badge: FindingText["badge"] =
    f.kind === "defective"
      ? f.effect.code === "not_comparable"
        ? { text: "No comparable", tone: "neutral" }
        : { text: "Pendiente de corregir", tone: "warn" }
      : f.kind === "similar"
        ? { text: "Pregunta", tone: "info" }
        : f.active_conflict
          ? { text: "Choque activo", tone: "warn" }
          : { text: "Solo si se activan", tone: "info" };

  if (f.kind === "similar") {
    const first = String(d.first?.supplier ?? "");
    const second = String(d.second?.supplier ?? "");
    return {
      title: `¿«${first}» y «${second}» son el mismo proveedor?`,
      badge,
      what: `${a} (proveedor «${first}») y ${b} (proveedor «${second}») usan nombres de proveedor que solo se diferencian en mayúsculas, puntuación o espacios.`,
      why: "La aplicación compara el nombre tal como aparece en la factura, letra a letra y sin distinguir mayúsculas; por eso los trata como dos proveedores distintos y no puede asegurar que sean el mismo (todavía no lee el NIF).",
      consequence: "No chocan ahora. Si son el mismo proveedor, una factura solo coincide con la regla cuyo nombre se escriba igual que en ella y la otra puede no aplicarse nunca. Si son proveedores distintos, no hay nada que hacer.",
      timing: null,
      tasks: null,
    };
  }

  if (f.kind === "defective") {
    const rule = ref(f.rules[0]!.id, nameOf);
    const reason = String(d.reason ?? "");
    if (f.effect.code === "sender_missing") {
      return {
        title: `${rule} no tiene remitente`,
        badge,
        what: `${rule} no indica a quién contesta y no tiene validada la opción avanzada.`,
        why: "Una regla de envío necesita una dirección o un dominio de remitente, o la opción avanzada «cualquier remitente», para poder actuar.",
        consequence: "No actúa: los correos que cubriría van a revisión. No es una contradicción, es una regla pendiente de corregir.",
        timing: null,
        tasks: null,
      };
    }
    if (f.effect.code === "not_comparable") {
      return {
        title: `${rule} no se puede comparar`,
        badge,
        what: `Regla ${rule}: ${reason}.`,
        why: "Solo se pueden comparar reglas de los tipos que la aplicación conoce.",
        consequence: "No se puede decir si choca con otras reglas; no se compara con ninguna.",
        timing: null,
        tasks: null,
      };
    }
    return {
      title: `${rule} no puede actuar`,
      badge,
      what: `Hay un problema en ${rule}: ${reason}.`,
      why: "Una regla con condiciones no válidas nunca coincide con nada, para no automatizar por error.",
      consequence: "No actúa y no se compara con las demás. Corríjala o archívela.",
      timing: null,
      tasks: null,
    };
  }

  if (f.action_kind === "register_invoice") {
    const supplier = String(d.supplier ?? "");
    if (f.kind === "duplicate") {
      const limit: string | null = d.limit ?? null;
      const covers = limit === null ? "sin límite de importe" : `de hasta ${upTo(limit)}`;
      return {
        title: `Reglas iguales: ${a} y ${b}`,
        badge,
        what: `Estas dos reglas son iguales: las dos cubren las facturas de ${supplier} ${covers}.`,
        why: "Cada una de esas facturas coincide con las dos reglas a la vez, y con dos reglas activas coincidentes la aplicación no sabe cuál manda.",
        consequence: `Ninguna factura de ${supplier} ${covers} se registra sola: todas van a revisión. Sobra una de las dos.`,
        timing,
        tasks,
      };
    }
    const wider = d.wider as { id: string; limit: string | null };
    const narrower = d.narrower as { id: string; limit: string | null };
    const W = ref(wider.id, nameOf);
    const N = ref(narrower.id, nameOf);
    const nl = upTo(narrower.limit ?? "");
    return {
      title: `Se pisan: ${a} y ${b}`,
      badge,
      what: `Estas dos reglas se pisan porque ambas cubren las facturas de ${supplier} de hasta ${nl}.`,
      why:
        wider.limit === null
          ? `${W} no tiene límite de importe y ${N} llega hasta ${nl}: todo lo que cubre ${N} ya lo cubre ${W}. Esas facturas coinciden con las dos reglas y, con dos reglas activas coincidentes, la aplicación no automatiza.`
          : `${W} llega hasta ${upTo(wider.limit)} y ${N} hasta ${nl}: todo lo que cubre ${N} ya lo cubre ${W}. Esas facturas coinciden con las dos reglas y, con dos reglas activas coincidentes, la aplicación no automatiza.`,
      consequence:
        wider.limit === null
          ? `Las facturas de ${supplier} de hasta ${nl} van a revisión aunque las dos reglas las permitan. Solo las de más de ${nl} se registran solas.`
          : `Las facturas de ${supplier} de hasta ${nl} van a revisión aunque las dos reglas las permitan. Solo las de más de ${nl} y hasta ${upTo(wider.limit)} se registran solas; las de más de ${upTo(wider.limit)} siempre se revisan.`,
      timing,
      tasks,
    };
  }

  // Reglas de envío.
  const category = categoryLabel(String(d.category ?? ""));
  if (f.kind === "duplicate") {
    const target = who(String(d.scope), d.value ?? null);
    return {
      title: `Reglas iguales: ${a} y ${b}`,
      badge,
      what: `Estas dos reglas son iguales: las dos contestan a ${target} cuando el correo se clasifica como «${category}».`,
      why: "Cada uno de esos correos coincide con las dos reglas a la vez, y con dos reglas activas coincidentes la aplicación no sabe cuál manda.",
      consequence: "Ninguno de esos correos se contesta solo: todos van a revisión. Sobra una de las dos.",
      timing,
      tasks,
    };
  }
  const wider = d.wider as { id: string; scope: string; value: string | null };
  const narrower = d.narrower as { id: string; scope: string; value: string | null };
  const W = ref(wider.id, nameOf);
  const N = ref(narrower.id, nameOf);
  const ww = who(wider.scope, wider.value);
  const nw = who(narrower.scope, narrower.value);
  const rest =
    wider.scope === "domain"
      ? `los de otras direcciones de @${cleanDomain(String(wider.value ?? ""))}`
      : "los de los demás remitentes";
  return {
    title: `Se pisan: ${a} y ${b}`,
    badge,
    what: `Estas dos reglas se pisan porque ambas contestan a los correos de ${nw} clasificados como «${category}».`,
    why: `${W} contesta a ${ww} y ${N} solo a ${nw}, que ya está dentro; por eso esos correos coinciden con las dos reglas y, con dos reglas activas coincidentes, la aplicación no automatiza.`,
    consequence: `Los correos de ${nw} van a revisión aunque las dos reglas los permitan. Solo se contestan solos ${rest}.`,
    timing,
    tasks,
  };
}

/** Aclaración de «¿Es el mismo proveedor?»: no decide nada, solo explica qué hacer. */
export function similarClarification(): string {
  return "Solo usted puede saberlo: la aplicación no lo decide por usted. Si es el mismo proveedor, edite una de las dos reglas para que el nombre sea idéntico al de sus facturas; después podrá unificarlas. Si son proveedores distintos, no haga nada.";
}

function severityRank(f: FindingLike): number {
  if ((f.kind === "duplicate" || f.kind === "overlap") && f.active_conflict) return 0;
  if (f.kind === "duplicate" || f.kind === "overlap") return 1;
  if (f.kind === "similar") return 2;
  return 3;
}

/** De más a menos grave: primero los choques con las dos reglas activas. */
export function sortFindings<T extends FindingLike>(findings: T[]): T[] {
  return [...findings].sort((x, y) => severityRank(x) - severityRank(y) || (x.key < y.key ? -1 : x.key > y.key ? 1 : 0));
}

// ---- Unificación ----

export interface MergeOptionLike {
  key: string;
  scope: Record<string, any>;
  rule_ids: string[];
}

export interface MergePlanLike {
  action_kind: string;
  before: Array<{ id: string; version: number; active: boolean; conditions: Record<string, any>; any_sender: boolean }>;
  options: MergeOptionLike[];
  chosen: string;
  after: {
    conditions: Record<string, any>;
    active: boolean;
    option_key: string;
    loses_any_sender: string[];
    needs_sender_validation: boolean;
  };
  simulation: {
    considered: number;
    truncated: boolean;
    skipped_without_result: number;
    touched: number;
    start_automating: Array<{ task_id: number; label: string | null; total: string | null }>;
    stop_automating: Array<{ task_id: number; label: string | null; total: string | null }>;
    unchanged_automated: number;
    unchanged_review: number;
  };
  fingerprint: string;
}

function mergeSubject(actionKind: string, conditions: Record<string, any>): string {
  if (actionKind === "register_invoice") return String(conditions.supplier ?? "");
  return categoryLabel(String(conditions.category ?? ""));
}

/** Qué significa elegir esta opción para la regla nueva. */
export function mergeOptionText(actionKind: string, option: MergeOptionLike, subject: string, nameOf: NameOf): string {
  const has = `La tienen ${option.rule_ids.map((id) => ref(id, nameOf)).join(" y ")}.`;
  if (actionKind === "register_invoice") {
    const limit = option.scope.limit as string | null;
    return limit === null
      ? `Sin límite de importe: la regla nueva registrará sola cualquier factura limpia de ${subject}, sea del importe que sea. ${has}`
      : `Hasta ${upTo(limit)}: la regla nueva registrará sola las facturas limpias de ${subject} de hasta ${upTo(limit)}; las de más importe irán a revisión. ${has}`;
  }
  const scope = String(option.scope.scope);
  const value = (option.scope.value as string | null) ?? null;
  if (scope === "sender") return `Solo a ${value}: la regla nueva contestará sola solo a esa dirección; los demás remitentes irán a revisión. ${has}`;
  if (scope === "domain") return `A cualquier dirección de @${cleanDomain(String(value))}: la regla nueva contestará sola a todo ese dominio. ${has}`;
  return `A cualquier remitente: la regla nueva contestará sola a cualquiera. La regla nueva no hereda la opción avanzada, así que tendrá que validarla de nuevo antes de que actúe. ${has}`;
}

/** Título corto de una opción, para el selector. */
export function mergeOptionLabel(actionKind: string, option: MergeOptionLike): string {
  if (actionKind === "register_invoice") return option.scope.limit === null ? "Sin límite" : `Hasta ${upTo(String(option.scope.limit))}`;
  const scope = String(option.scope.scope);
  if (scope === "sender") return `Solo ${option.scope.value}`;
  if (scope === "domain") return `Todo @${cleanDomain(String(option.scope.value))}`;
  return "Cualquier remitente";
}

export interface MergeSimulationText {
  empty: boolean;
  lines: string[];
  notes: string[];
}

/** Simulación de la unificación en frases con recuento real. Supone la regla nueva ya activa. */
export function mergeSimulationText(plan: MergePlanLike, nameOf: NameOf): MergeSimulationText {
  const sim = plan.simulation;
  const invoice = plan.action_kind === "register_invoice";
  const notes: string[] = [];
  const lines: string[] = [];
  const chosenLimit = invoice ? ((plan.after.conditions.max_total as string | undefined) ?? null) : null;
  const subject = mergeSubject(plan.action_kind, plan.after.conditions);
  const sender = plan.after.conditions;
  const target = invoice
    ? `de ${subject}`
    : sender.sender
      ? `de ${sender.sender}`
      : sender.sender_domain
        ? `de cualquier dirección de @${cleanDomain(String(sender.sender_domain))}`
        : "de cualquier remitente";
  const n = sim.start_automating.length;
  const m = sim.stop_automating.length;
  const startRange = chosenLimit ? ` de hasta ${upTo(chosenLimit)}` : "";
  const stopRange = chosenLimit ? ` de más de ${upTo(chosenLimit)}` : "";
  const w = (k: number) => noun(plan.action_kind, k);
  if (n > 0)
    lines.push(
      invoice
        ? `${n} ${w(n)} ${target}${startRange} ${plural(n, "dejaría", "dejarían")} de revisarse: ${plural(n, "se registraría sola", "se registrarían solas")}.`
        : `${n} ${w(n)} ${target} ${plural(n, "dejaría", "dejarían")} de revisarse: ${plural(n, "se contestaría solo", "se contestarían solos")}.`,
    );
  if (m > 0)
    lines.push(
      `${m} ${w(m)} ${target}${stopRange} ${plural(m, "dejaría", "dejarían")} de automatizarse: hoy ${plural(m, invoice ? "se registra sola" : "se contesta solo", invoice ? "se registran solas" : "se contestan solos")} y con la regla nueva ${plural(m, "iría", "irían")} a revisión.`,
    );
  if (sim.unchanged_automated > 0)
    lines.push(
      `${sim.unchanged_automated} ${w(sim.unchanged_automated)} ${plural(sim.unchanged_automated, "seguiría", "seguirían")} ${invoice ? "registrándose" : "contestándose"} ${invoice ? plural(sim.unchanged_automated, "sola", "solas") : plural(sim.unchanged_automated, "solo", "solos")}.`,
    );
  if (sim.unchanged_review > 0)
    lines.push(`${sim.unchanged_review} ${w(sim.unchanged_review)} ${plural(sim.unchanged_review, "seguiría", "seguirían")} en revisión.`);
  const empty = sim.touched === 0;
  if (empty) {
    lines.push(
      sim.considered === 0
        ? "No hay tareas guardadas con resultado: no se puede decir qué cambiaría."
        : `Ninguna de las ${sim.considered} ${w(sim.considered)} guardadas está cubierta por estas reglas: no cambiaría nada.`,
    );
  }
  if (plan.after.loses_any_sender.length)
    notes.push(
      `${plan.after.loses_any_sender.map((id) => ref(id, nameOf)).join(" y ")} ${plan.after.loses_any_sender.length === 1 ? "tenía" : "tenían"} la opción avanzada «cualquier remitente»: la regla nueva no la hereda.`,
    );
  if (plan.after.needs_sender_validation)
    notes.push("La regla nueva no indica remitente: no contestará a nadie hasta que valide la opción avanzada, por eso en esta simulación no se automatiza ningún correo.");
  if (sim.truncated) notes.push(`Solo se han mirado las últimas ${200} tareas: hay más antiguas que no se han comprobado.`);
  if (sim.skipped_without_result > 0)
    notes.push(`${sim.skipped_without_result} ${plural(sim.skipped_without_result, "tarea no tiene", "tareas no tienen")} resultado todavía y no se ${plural(sim.skipped_without_result, "ha contado", "han contado")}.`);
  return { empty, lines, notes };
}

/** Mensaje tras unificar, con el siguiente paso. */
export function mergedDone(name: string): string {
  return `Reglas unificadas en «${name}». La regla nueva está inactiva y las anteriores, archivadas (puede restaurarlas). ${MERGE_NEXT_STEP}.`;
}

export interface OriginLike {
  kind?: string;
  sources?: Array<{ id: string; version: number }>;
}

/** De dónde sale una regla creada al unificar; `null` si no tiene origen. */
export function originText(origin: OriginLike | null | undefined, nameOf: NameOf): string | null {
  if (!origin || origin.kind !== "merge" || !origin.sources?.length) return null;
  const list = origin.sources.map((s) => `«${nameOf(s.id)}» (versión ${s.version})`);
  return `Creada al unificar ${list.length === 1 ? list[0] : `${list.slice(0, -1).join(", ")} y ${list[list.length - 1]}`}.`;
}
