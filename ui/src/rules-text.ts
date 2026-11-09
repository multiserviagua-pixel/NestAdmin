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
  "pipeline_step_waived",
  "automatic_sender",
  "signals_not_checked",
  "recipient_automatic",
  "no_reply_needed",
  "redraft",
  "empty_draft",
  "draft_not_checked",
  "draft_flagged",
  "rodaje",
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
    case "pipeline_step_waived":
      return "se continuó sin un paso anterior de la plantilla; la respuesta se revisa antes de enviarla";
    case "automatic_sender":
      return "el correo parece automático; ninguna regla responde a un remitente automático";
    case "signals_not_checked":
      return "no consta la comprobación de correo automático; reprocese o revise";
    case "recipient_automatic":
      return "el destinatario del borrador parece una dirección automática";
    case "no_reply_needed":
      return "la IA considera que el correo no requiere respuesta";
    case "redraft":
      return "la respuesta se pidió a mano; una persona debe revisarla siempre";
    case "empty_draft":
      return "el borrador está vacío";
    case "draft_not_checked":
      return "no consta la comprobación del borrador; reprocese o revise";
    case "draft_flagged":
      return "el borrador contiene enlaces, direcciones, teléfonos o IBAN que no se pueden dar por buenos; compruébelos";
    case "rodaje":
      // Regla aprendida aceptada en lote (D174): sus 3 primeros envíos se revisan. El mismo texto que el backend.
      return "en rodaje: esta regla aprendida enviará sola a partir del envío n.º 4";
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
  without_signal_check?: number;
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
  /** Lo mismo para las facturas solo leídas en la primera lectura (aparte, sin listarlas). */
  from_read?: { considered: number; would_automate: number; truncated: boolean; limit: number } | null;
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
        ? `1 ${word(1)} no se pudo comprobar en cuanto a la moneda: iría a revisión.`
        : `${sim.without_currency_check} ${word(sim.without_currency_check)} no se pudieron comprobar en cuanto a la moneda: irían a revisión.`,
    );
  const unchecked = sim.without_signal_check ?? 0;
  if (unchecked > 0)
    notes.push(
      unchecked === 1
        ? "1 correo no se pudo comprobar en cuanto a si es automático: iría a revisión."
        : `${unchecked} correos no se pudieron comprobar en cuanto a si son automáticos: irían a revisión.`,
    );
  if (sim.truncated) notes.push(`Solo se han mirado las últimas ${sim.limit} tareas: hay más antiguas que no se han comprobado.`);
  if (sim.skipped_without_result > 0)
    notes.push(
      `${sim.skipped_without_result} ${sim.skipped_without_result === 1 ? "tarea no tiene" : "tareas no tienen"} resultado todavía (fallidas o sin procesar) y no se han contado.`,
    );
  if (sim.from_read && sim.from_read.considered > 0) {
    const r = sim.from_read;
    notes.push(
      `Aparte: de ${r.considered} ${plural(r.considered, "factura que solo se leyó", "facturas que solo se leyeron")} en la primera lectura${r.truncated ? ` (las últimas ${r.limit})` : ""}, la regla habría registrado ${r.would_automate}. No son tareas suyas: no están en Tareas ni se registraron.`,
    );
  }
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
  /** Otras reglas activas, ajenas al par, que también cubren lo que solo cubre la más amplia. */
  others?: FindingRuleLike[];
  /** Tareas evaluadas con las dos reglas activas y coincidentes, con sus condiciones actuales; `null` si no aplica. */
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
  return id === SIMULATED_RULE || id === SUGGESTED_RULE ? "esta regla" : `«${nameOf(id)}»`;
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
  if (inactive.length >= f.rules.length) return "Ahora no ocurre: las dos reglas están inactivas. Ocurrirá en cuanto active las dos.";
  const names = inactive.map((r) => ref(r.id, nameOf)).join(" y ");
  return `Ahora no ocurre: ${names} ${inactive.length === 1 ? "está inactiva" : "están inactivas"}. Ocurrirá en cuanto ${inactive.length === 1 ? "la active" : "las active"}.`;
}

function tasksNote(f: FindingLike): string | null {
  if (f.kind !== "duplicate" && f.kind !== "overlap") return null;
  // Sin recuento (por ejemplo, una regla que aún no existe) no se afirma nada.
  if (f.affected_tasks === null || f.affected_tasks === undefined) return null;
  const n = f.affected_tasks;
  if (n <= 0) return NO_TASKS_AFFECTED;
  const w = noun(f.action_kind, n);
  return `${n} ${w} ${plural(n, "ya coincidió", "ya coincidieron")} con las dos reglas, según el registro de evaluaciones y con las condiciones que tienen hoy (no cuentan las evaluadas con condiciones ya editadas).`;
}

/** Terceras reglas activas que cubren parte de lo que el par deja a la más amplia. Solo habla de coincidencias, no del resultado global. */
function exceptOthers(f: FindingLike, nameOf: NameOf, what: "las" | "los"): string {
  const others = f.others ?? [];
  if (!others.length) return "";
  const names = others.map((r) => ref(r.id, nameOf));
  if (names.length === 1) return `. Si además ${what} cubre ${names[0]}, también coinciden con esa regla y van a revisión`;
  const list = `${names.slice(0, -1).join(", ")} o ${names[names.length - 1]}`;
  return `. Si además ${what} cubre alguna de estas reglas (${list}), también coinciden con ella y van a revisión`;
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
          ? `Las facturas de ${supplier} de hasta ${nl} coinciden con las dos reglas y, con dos reglas activas coincidentes, van a revisión aunque las dos las permitan. Las de más de ${nl} las cubre ${W}, no ${N}${exceptOthers(f, nameOf, "las")}.`
          : `Las facturas de ${supplier} de hasta ${nl} coinciden con las dos reglas y, con dos reglas activas coincidentes, van a revisión aunque las dos las permitan. Las de más de ${nl} y hasta ${upTo(wider.limit)} las cubre ${W}, no ${N}${exceptOthers(f, nameOf, "las")}; las de más de ${upTo(wider.limit)} no las cubre ninguna de las dos.`,
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
      ? `Los de otras direcciones de @${cleanDomain(String(wider.value ?? ""))}`
      : "Los de los demás remitentes";
  return {
    title: `Se pisan: ${a} y ${b}`,
    badge,
    what: `Estas dos reglas se pisan porque ambas contestan a los correos de ${nw} clasificados como «${category}».`,
    why: `${W} contesta a ${ww} y ${N} solo a ${nw}, que ya está dentro; por eso esos correos coinciden con las dos reglas y, con dos reglas activas coincidentes, la aplicación no automatiza.`,
    consequence: `Los correos de ${nw} coinciden con las dos reglas y, con dos reglas activas coincidentes, van a revisión aunque las dos los permitan. ${rest} los cubre ${W}, no ${N}${exceptOthers(f, nameOf, "los")}.`,
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

export interface SimTaskLike {
  task_id: number;
  label: string | null;
  total: string | null;
  automated_before_by: string | null;
  automated_after_by: string | null;
}

export interface MergeSimulationLike {
  considered: number;
  truncated: boolean;
  skipped_without_result: number;
  touched: number;
  start_automating: SimTaskLike[];
  stop_automating: SimTaskLike[];
  unchanged_automated: number;
  unchanged_review: number;
  without_currency_check: number;
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
  simulation: MergeSimulationLike;
  simulation_if_validated?: MergeSimulationLike | null;
  fingerprint: string;
}

/** Qué cubriría la regla nueva por sí sola con esta opción. Lo que haría con las tareas guardadas lo muestra la simulación. */
export function mergeOptionText(actionKind: string, option: MergeOptionLike, subject: string, nameOf: NameOf): string {
  const has = `La tienen ${option.rule_ids.map((id) => ref(id, nameOf)).join(" y ")}.`;
  const sim = "Qué haría con sus tareas guardadas, junto a sus demás reglas, lo muestra la simulación.";
  if (actionKind === "register_invoice") {
    const limit = option.scope.limit as string | null;
    return limit === null
      ? `Sin límite de importe: por sí sola, la regla nueva cubriría todas las facturas de ${subject}, de cualquier importe. ${sim} ${has}`
      : `Hasta ${upTo(limit)}: por sí sola, la regla nueva cubriría las facturas de ${subject} de hasta ${upTo(limit)} y no las de más importe. ${sim} ${has}`;
  }
  const scope = String(option.scope.scope);
  const value = (option.scope.value as string | null) ?? null;
  if (scope === "sender") return `Solo a ${value}: por sí sola, la regla nueva cubriría únicamente esa dirección. ${sim} ${has}`;
  if (scope === "domain") {
    const warning = publicDomainWarning(cleanDomain(String(value)));
    return `A cualquier dirección de @${cleanDomain(String(value))}: por sí sola, la regla nueva cubriría todo ese dominio. ${warning ? `${warning} ` : ""}${sim} ${has}`;
  }
  return `A cualquier remitente: por sí sola, la regla nueva cubriría a cualquier remitente. La regla nueva no hereda la opción avanzada, así que tendrá que validarla de nuevo antes de que actúe. ${sim} ${has}`;
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
  /** Solo con «cualquier remitente»: encabezado del primer bloque, que es el escenario sin validar. */
  heading: string | null;
  lines: string[];
  notes: string[];
  /** Solo con «cualquier remitente»: encabezado y frases de lo que pasaría tras validar. */
  validated: { heading: string; lines: string[] } | null;
}

/** Id de la regla nueva en `automated_after_by` (`MERGED_RULE` del backend). */
const MERGED_RULE_ID = "(unificada)";

function groupBy(tasks: SimTaskLike[], key: (t: SimTaskLike) => string | null): Array<[string, number]> {
  const counts = new Map<string, number>();
  for (const t of tasks) {
    const k = key(t) ?? "";
    counts.set(k, (counts.get(k) ?? 0) + 1);
  }
  return [...counts];
}

/** Frases de una simulación con recuentos ciertos: cada grupo dice qué regla automatiza, sin tramos deducidos. */
function simulationLines(plan: MergePlanLike, sim: MergeSimulationLike, nameOf: NameOf): string[] {
  const invoice = plan.action_kind === "register_invoice";
  const w = (k: number) => noun(plan.action_kind, k);
  const auto = (k: number) => (invoice ? plural(k, "se registraría sola", "se registrarían solas") : plural(k, "se contestaría solo", "se contestarían solos"));
  const lines: string[] = [];
  for (const [rule, n] of groupBy(sim.start_automating, (t) => t.automated_after_by)) {
    lines.push(
      rule === MERGED_RULE_ID
        ? `${n} ${w(n)} ${plural(n, "dejaría", "dejarían")} de revisarse: hoy ${plural(n, "va", "van")} a revisión y con la regla nueva ${auto(n)}.`
        : `${n} ${w(n)} ${plural(n, "dejaría", "dejarían")} de revisarse, pero no por la regla nueva: ${plural(n, "la automatizaría", "las automatizaría")} ${ref(rule, nameOf)}, una regla ajena a la unificación que hoy ${plural(n, "coincide", "coinciden")} a la vez con alguna de las reglas que se unifican.`,
    );
  }
  for (const [rule, n] of groupBy(sim.stop_automating, (t) => t.automated_before_by)) {
    lines.push(
      `${n} ${w(n)} ${plural(n, "dejaría", "dejarían")} de automatizarse: hoy ${plural(n, "la automatiza", "las automatiza")} ${ref(rule, nameOf)} y con la regla nueva ${plural(n, "iría", "irían")} a revisión.`,
    );
  }
  if (sim.unchanged_automated > 0)
    lines.push(
      `${sim.unchanged_automated} ${w(sim.unchanged_automated)} ${plural(sim.unchanged_automated, "seguiría", "seguirían")} ${invoice ? "registrándose" : "contestándose"} ${invoice ? plural(sim.unchanged_automated, "sola", "solas") : plural(sim.unchanged_automated, "solo", "solos")}.`,
    );
  if (sim.unchanged_review > 0) lines.push(`${sim.unchanged_review} ${w(sim.unchanged_review)} ${plural(sim.unchanged_review, "seguiría", "seguirían")} en revisión.`);
  if (sim.touched === 0)
    lines.push(
      sim.considered === 0
        ? "No hay tareas guardadas con resultado: no se puede decir qué cambiaría."
        : `Estas reglas no cubren lo guardado (${sim.considered} ${w(sim.considered)} con resultado): no cambiaría nada.`,
    );
  return lines;
}

/** Simulación de la unificación en frases con recuento real. Supone la regla nueva ya activa. */
export function mergeSimulationText(plan: MergePlanLike, nameOf: NameOf): MergeSimulationText {
  const sim = plan.simulation;
  const notes: string[] = [];
  const w = (k: number) => noun(plan.action_kind, k);
  if (plan.after.loses_any_sender.length)
    notes.push(
      `${plan.after.loses_any_sender.map((id) => ref(id, nameOf)).join(" y ")} ${plan.after.loses_any_sender.length === 1 ? "tenía" : "tenían"} la opción avanzada «cualquier remitente»: la regla nueva no la hereda.`,
    );
  if (sim.without_currency_check > 0)
    notes.push(
      `${sim.without_currency_check} ${w(sim.without_currency_check)} ${plural(sim.without_currency_check, "no se pudo comprobar", "no se pudieron comprobar")} en cuanto a la moneda: con cualquier regla ${plural(sim.without_currency_check, "iría", "irían")} a revisión.`,
    );
  if (sim.truncated) notes.push(`Solo se han mirado las últimas ${200} tareas: hay más antiguas que no se han comprobado.`);
  if (sim.skipped_without_result > 0)
    notes.push(`${sim.skipped_without_result} ${plural(sim.skipped_without_result, "tarea no tiene", "tareas no tienen")} resultado todavía y no se ${plural(sim.skipped_without_result, "ha contado", "han contado")}.`);
  const validated = plan.simulation_if_validated
    ? {
        heading: "Si después valida la opción avanzada de la regla nueva (es solo una hipótesis: no se valida al unificar):",
        lines: simulationLines(plan, plan.simulation_if_validated, nameOf),
      }
    : null;
  const heading = plan.after.needs_sender_validation
    ? "Sin validar la opción avanzada, la regla nueva no contesta a nadie, y esa validación no se da al unificar. Con esa condición:"
    : null;
  return { empty: sim.touched === 0, heading, lines: simulationLines(plan, sim, nameOf), notes, validated };
}

/** Mensaje tras unificar, con el siguiente paso. */
export function mergedDone(name: string): string {
  return `Reglas unificadas en «${name}». La regla nueva está inactiva y las anteriores, archivadas (puede restaurarlas). ${MERGE_NEXT_STEP}.`;
}

export interface OriginLike {
  kind?: string;
  sources?: Array<{ id: string; version: number }>;
  count?: number;
  threshold?: number;
  limit_source?: string;
  /** De `count`, aprobaciones de la persona y facturas solo vistas en la primera lectura (E5). */
  approved?: number;
  seen?: number;
}

/** De dónde sale una regla creada al unificar; `null` si no tiene origen. */
export function originText(origin: OriginLike | null | undefined, nameOf: NameOf): string | null {
  if (origin?.kind === "suggestion" && typeof origin.count === "number") {
    const min = typeof origin.threshold === "number" ? ` (el mínimo era ${origin.threshold})` : "";
    const proposedByAi = origin.limit_source === "model" ? " El límite lo propuso la IA y usted lo eligió." : "";
    // Con facturas solo vistas, el origen lo dice: no hubo aprobación que lo sostuviera.
    const seen = typeof origin.seen === "number" ? origin.seen : 0;
    const approved = typeof origin.approved === "number" ? origin.approved : origin.count - seen;
    const split =
      seen > 0
        ? approved === 0
          ? `: ${seen} ${plural(seen, "factura vista", "facturas vistas")} en la primera lectura, ninguna aprobada por usted`
          : `: ${approved} ${plural(approved, "aprobada", "aprobadas")} por usted y ${seen} ${plural(seen, "vista", "vistas")} en la primera lectura sin aprobar`
        : "";
    return `Creada desde una sugerencia, con ${origin.count} ${plural(origin.count, "observación", "observaciones")}${split}${min}.${proposedByAi}`;
  }
  if (!origin || origin.kind !== "merge" || !origin.sources?.length) return null;
  const list = origin.sources.map((s) => `«${nameOf(s.id)}» (versión ${s.version})`);
  return `Creada al unificar ${list.length === 1 ? list[0] : `${list.slice(0, -1).join(", ")} y ${list[list.length - 1]}`}.`;
}

// ---- Avisos al activar y al restaurar (R23) ----

export interface FindingGroup<T extends FindingLike> {
  /** Encabezado que dice solo lo que los datos sostienen: «chocaría» únicamente con reglas activas. */
  heading: string;
  findings: T[];
}

/** Separa lo que chocaría de verdad (las dos activas) de lo que solo chocará si se activan y de las preguntas de nombre parecido. */
export function activationGroups<T extends FindingLike>(findings: T[]): Array<FindingGroup<T>> {
  const sorted = sortFindings(findings);
  const real = sorted.filter((f) => (f.kind === "duplicate" || f.kind === "overlap") && f.active_conflict);
  const pending = sorted.filter((f) => (f.kind === "duplicate" || f.kind === "overlap") && !f.active_conflict);
  const similar = sorted.filter((f) => f.kind === "similar");
  const groups: Array<FindingGroup<T>> = [];
  if (real.length)
    groups.push({ heading: real.length === 1 ? "Al activarla chocaría con otra regla activa:" : `Al activarla chocaría con otras ${real.length} reglas activas:`, findings: real });
  if (pending.length)
    groups.push({
      heading: pending.length === 1 ? "Se pisa con una regla que hoy está inactiva; solo chocarían si activa también esa:" : `Se pisa con ${pending.length} reglas que hoy están inactivas; solo chocarían si activa también esas:`,
      findings: pending,
    });
  if (similar.length)
    groups.push({ heading: "Su proveedor se escribe casi igual que el de otra regla (no es un choque, es una pregunta):", findings: similar });
  return groups;
}

/** Encabezado del aviso al restaurar una regla archivada. */
export function restoreHeading(name: string, findings: FindingLike[]): string {
  const n = findings.length;
  return `«${name}» volvería inactiva, pero ${n === 1 ? "tiene 1 aviso" : `tiene ${n} avisos`} con las reglas actuales:`;
}

// ---- Historial de reglas ----

const RULE_AUDIT_STATUS: Record<string, string> = {
  // El estado es el de la versión que se guardó (también al crear o editar), no un cambio.
  active: "Versión activa",
  inactive: "Versión inactiva",
  acknowledged: "Opción avanzada validada",
  revoked: "Opción avanzada retirada",
  archived: "Archivada",
  unarchived: "Restaurada",
  renamed: "Renombrada",
  merged: "Unificada",
  accepted: "Sugerencia aceptada",
  discarded: "Sugerencia descartada",
  changed: "Ajuste cambiado",
};

/** Estado en palabras de una entrada del historial que viene de una regla; el resto se deja tal cual. */
export function auditStatusLabel(action: string, status: string): string {
  if (action === "sender_preference") return status === "set" ? "Fijada" : status === "removed" ? "Quitada" : status;
  return action.startsWith("rule") ? (RULE_AUDIT_STATUS[status] ?? status) : status;
}

// ---- Sugerencias de reglas (D132, R24) ----
//
// Las candidatas, su evidencia y las cifras las calcula el backend (`suggest`). Aquí solo se
// redactan, y un texto solo afirma lo que esos datos sostienen: la candidata describe lo que
// cubriría por sí sola; otra regla activa o una incidencia pueden cambiar el resultado, y los
// avisos de solapamiento (D130) lo dicen aparte.

/** Id de la regla hipotética de una candidata en los hallazgos del backend. */
export const SUGGESTED_RULE = "(sugerida)";

export interface SuggestionEvidenceLike {
  count: number;
  threshold: number;
  earlier_breaks: number;
  /** De `count`: aprobaciones de la persona y facturas solo vistas en la primera lectura (E5). */
  approved?: number;
  seen?: number;
  last_at: number;
  amounts: string[];
  limit: string | null;
  amount_min?: string | null;
  amount_second?: string | null;
  limit_outlier?: boolean;
  needs_limit?: boolean;
  reappeared: { at_discard: number; new_since: number; needed_new: number } | null;
}

export interface SuggestionLike {
  fingerprint: string;
  action_kind: string;
  name: string;
  conditions: Record<string, unknown>;
  subject: string;
  category: string | null;
  evidence: SuggestionEvidenceLike;
}

export interface ProgressLike {
  action_kind: string;
  subject: string;
  category: string | null;
  count: number;
  approved?: number;
  seen?: number;
  needed: number;
  missing: number;
}

export interface ThresholdsLike {
  invoice: number;
  reply: number;
  min: number;
  max: number;
}

export interface SuggestionsLike {
  candidates: SuggestionLike[];
  progress: ProgressLike[];
  observed: number;
  /** Decisiones de la persona y facturas vistas en la primera lectura, entre las señales válidas. */
  decisions?: number;
  seen?: number;
  counted: number;
  hidden: { covered: number; discarded: number; accepted: number; invalid: number; not_suggested?: number; seen_invalidated?: number };
  thresholds: ThresholdsLike;
}

/** Mínimo de facturas del mismo proveedor solo vistas en la primera lectura (el mayor de esto y del umbral de facturas). Igual que `SEEN_INVOICE_MIN` del backend. */
export const SEEN_INVOICE_MIN = 5;

export function seenNeeded(invoiceThreshold: number): number {
  return Math.max(invoiceThreshold, SEEN_INVOICE_MIN);
}

export const SUGGESTION_HELP = {
  seen:
    "«Vistas» son facturas que NestAdmin leyó en la primera lectura de su correo o de una carpeta: muestran que un proveedor se repite, pero usted no las ha aprobado ni nadie comprobó que las cifras estén bien leídas. Por eso se exigen más (como mínimo " +
    SEEN_INVOICE_MIN +
    ") que con sus aprobaciones, y la sugerencia lo indica. Nunca sirven para sugerir respuestas por correo.",
  evidence:
    "Cuentan sus aprobaciones ya ejecutadas y, solo en facturas, las facturas vistas en la primera lectura (que no son aprobaciones suyas). Reinician el recuento: una edición del borrador, un rechazo, una aprobación sin clasificar (por ejemplo, tras confirmar que un envío no se hizo) y, en facturas, una aprobación con incidencias. No cuentan ni reinician: las partes de una factura separada, lo que ya hizo una regla, los documentos repetidos (mismo contenido), los reintentos de una misma tarea y las facturas marcadas solo como posible duplicada. Lo que no conste en este equipo (por ejemplo, lo que hizo fuera de la aplicación) no se puede contar.",
  limit:
    "El límite sale de los importes de las facturas observadas (aprobadas por usted o solo vistas en la primera lectura): es el mayor de ellos. Es una propuesta, no una garantía: puede cambiarlo al revisar la regla. Por sí sola, la regla no cubriría las facturas de más importe. La regla reconoce al proveedor solo por su nombre tal como aparece en la factura.",
  create: "Crea la regla inactiva: no actúa hasta que usted la pruebe y la active. La encontrará en Mis reglas.",
  discard: "Oculta la sugerencia. Solo vuelve a aparecer si, tras descartarla, se acumula el doble de observaciones nuevas que las que había.",
  thresholds:
    "Observaciones mínimas para sugerir una regla. El mínimo posible es 2: con una sola observación no se sugiere nada. Bajar un umbral solo hace que aparezcan más sugerencias; ninguna se crea ni se activa sin su aceptación.",
} as const;

export const SUGGESTION_INTRO =
  "Estas sugerencias salen de lo que usted aprueba en Pendientes y, solo para facturas, de la repetición observada en la primera lectura (que no es una aprobación). Se calculan en este equipo, sin IA, y ninguna se crea ni se activa sin su aceptación. Una IA puede afinarlas (ordenarlas, proponer ocultar las poco útiles, preguntar por proveedores parecidos o proponer otro límite), pero nunca crea ni activa reglas ni pone las cifras.";

export const SUGGESTION_INACTIVE_NOTICE = "Si la crea, nacerá inactiva: no actuará hasta que usted la pruebe y la active.";

/** Cuando el cálculo falló no hay recuento que mostrar: se dice, no se enseña un 0. */
export const SUGGESTIONS_UNAVAILABLE = "No se han podido calcular las sugerencias";

/** Etiqueta de la pestaña: sin recuento si no hay cálculo. */
export function suggestionsTabCount(s: { candidates: unknown[] } | null | undefined, failed: boolean): number | undefined {
  return failed || !s ? undefined : s.candidates.length;
}

function shortDate(millis: number): string {
  const d = new Date(millis);
  const two = (n: number) => String(n).padStart(2, "0");
  return `${two(d.getDate())}/${two(d.getMonth() + 1)}`;
}

/** Frase de la regla que se crearía: lo que cubriría por sí sola. */
export function suggestionSentence(c: SuggestionLike): string {
  return ruleSentence({ id: "", version: 0, action_kind: c.action_kind, conditions: c.conditions, active: false });
}

/**
 * Qué observaciones constan, con cifras del backend. No dice «seguidas» ni «las últimas»: lo no
 * registrado (lo que se hizo fuera de la aplicación) podría desmentirlo; solo dice lo que consta.
 */
export function suggestionWhy(c: SuggestionLike): string {
  const e = c.evidence;
  const n = e.count;
  const last = shortDate(e.last_at);
  const cuts = e.earlier_breaks;
  const before = (what: string) => (cuts === 1 ? ` Antes consta 1 corte (${what}) que reinició el recuento.` : cuts > 1 ? ` Antes constan ${cuts} cortes (${what}) que reiniciaron el recuento.` : "");
  if (c.action_kind === "register_invoice") {
    const seen = e.seen ?? 0;
    const approved = e.approved ?? n - seen;
    const cutsText = before("una factura con incidencias, rechazada o sin clasificar");
    // La primera lectura no es una aprobación: el texto separa lo que usted aprobó de lo que solo se vio.
    // «La última» de lo visto es la fecha de lectura, no la de la factura.
    if (seen > 0 && approved === 0)
      return `Se observaron ${n} ${plural(n, "factura", "facturas")} de ${c.subject} en la primera lectura; usted no ha aprobado ninguna de ${n === 1 ? "ella" : "ellas"} y nadie ha revisado su contenido. La última se leyó el ${last}.${cutsText}`;
    if (seen > 0)
      return `Constan ${approved} ${plural(approved, "factura aprobada", "facturas aprobadas")} por usted y ${seen} ${plural(seen, "vista", "vistas")} en la primera lectura sin aprobar, de ${c.subject}. La última observación es del ${last} (fecha de lectura o de aprobación, no de la factura).${cutsText}`;
    return `Constan ${n} ${plural(n, "factura aprobada", "facturas aprobadas")} de ${c.subject} sin incidencias; la última, el ${last}.${cutsText}`;
  }
  const cat = c.category ? categoryLabel(c.category) : "sin categoría";
  return `Constan ${n} ${plural(n, "respuesta aprobada", "respuestas aprobadas")} sin cambios a ${c.subject} clasificadas como ${cat}; la última, el ${last}.${before("con cambios, rechazo o sin clasificar")}`;
}

/** De dónde sale el límite propuesto (solo facturas): el mayor, el rango y el segundo mayor. */
export function suggestionLimitText(c: SuggestionLike): string | null {
  const e = c.evidence;
  if (c.action_kind === "register_invoice" && e.needs_limit)
    return "No hay límite propuesto: estas facturas no las ha aprobado usted y un límite no sale de lo que solo se ha visto. Escriba usted el límite al crear la regla.";
  if (c.action_kind !== "register_invoice" || !e.limit) return null;
  const parts = [`el mayor de los ${e.amounts.length} importes aprobados`];
  if (e.amount_min && e.amount_second) parts.push(`de ${formatAmount(e.amount_min)} a ${formatAmount(e.limit)}; el segundo mayor es ${formatAmount(e.amount_second)}`);
  const list = e.amounts.length <= 5 ? ` Importes: ${e.amounts.map(formatAmount).join(", ")}.` : "";
  return `Límite propuesto: ${formatAmount(e.limit)}, ${parts[0]}${parts[1] ? ` (${parts[1]})` : ""}.${list}`;
}

/** Aviso visible de lo que solo se leyó: nadie lo ha revisado. */
export const SEEN_WARNING = "Estas facturas solo se han leído; nadie las ha revisado y un tercero puede haberlas enviado.";

export function suggestionSeenWarning(c: SuggestionLike): string | null {
  return c.action_kind === "register_invoice" && (c.evidence.seen ?? 0) > 0 ? SEEN_WARNING : null;
}

/** Aviso visible si el mayor importe se aleja mucho del resto, o `null`. */
export function suggestionOutlierWarning(c: SuggestionLike): string | null {
  const e = c.evidence;
  if (c.action_kind !== "register_invoice" || !e.limit_outlier || !e.amount_second || !(Number(e.amount_second) > 0)) return null;
  return `Revise el límite: el mayor importe (${formatAmount(e.limit ?? "")}) es al menos el triple del segundo mayor (${formatAmount(e.amount_second)}) y podría ser demasiado alto.`;
}

/** La coincidencia es solo por el nombre del proveedor; no se sabe si son el mismo proveedor. */
export function suggestionSupplierNote(c: SuggestionLike): string | null {
  return c.action_kind === "register_invoice"
    ? `La regla reconoce al proveedor solo por su nombre, «${c.subject}», tal como aparece en la factura; no comprueba que sea la misma empresa.`
    : null;
}

/** Por qué vuelve a aparecer una sugerencia descartada, o `null`. */
export function suggestionReappeared(c: SuggestionLike): string | null {
  const r = c.evidence.reappeared;
  if (!r) return null;
  return `La descartó cuando había ${r.at_discard} observaciones. Vuelve a aparecer porque desde entonces hay ${r.new_since} nuevas (hacían falta ${r.needed_new}).`;
}

function progressLine(p: ProgressLike): string {
  const missing = p.missing === 1 ? "falta 1" : `faltan ${p.missing}`;
  if (p.action_kind === "register_invoice") {
    const seen = p.seen ?? 0;
    if (seen > 0) {
      const approved = p.approved ?? p.count - seen;
      const mine = approved === 0 ? "ninguna aprobada por usted" : `${approved} ${plural(approved, "aprobada", "aprobadas")} por usted`;
      return `${p.subject}: ${p.count} de ${p.needed} facturas sin incidencias, ${seen} ${plural(seen, "vista", "vistas")} en la primera lectura y ${mine} (${missing})`;
    }
    return `${p.subject}: ${p.count} de ${p.needed} facturas sin incidencias (${missing})`;
  }
  const cat = p.category ? ` (${categoryLabel(p.category)})` : "";
  return `${p.subject}${cat}: ${p.count} de ${p.needed} respuestas aprobadas sin cambios (${missing})`;
}

export interface SuggestionsEmpty {
  title: string;
  hint: string;
  /** Lo más cerca de sugerirse, con cuánto falta. */
  lines: string[];
}

/** Lo que no se muestra y por qué, o `null` si no se ocultó nada. */
export function suggestionsHidden(h: SuggestionsLike["hidden"]): string | null {
  const parts: string[] = [];
  if (h.covered) parts.push(`${h.covered} ya ${plural(h.covered, "cubierta", "cubiertas")} por una regla existente (la misma o una activa más amplia)`);
  if (h.discarded) parts.push(`${h.discarded} ${plural(h.discarded, "descartada", "descartadas")}`);
  if (h.accepted) parts.push(`${h.accepted} ya ${plural(h.accepted, "aceptada", "aceptadas")}`);
  if (h.invalid) parts.push(`${h.invalid} sin datos válidos para una regla`);
  if (h.seen_invalidated)
    parts.push(
      `${h.seen_invalidated} ${plural(h.seen_invalidated, "proveedor con facturas solo leídas no se sugiere", "proveedores con facturas solo leídas no se sugieren")}: basta una factura con incidencias, posiblemente enviada por un tercero, para que ninguna de las leídas cuente; sus aprobaciones, si las hay, se conservan`,
    );
  if (h.not_suggested) parts.push(`${h.not_suggested} de la categoría «Otros», que no se sugiere (puede crear la regla a mano)`);
  return parts.length ? `No se muestran: ${parts.join("; ")}.` : null;
}

/** Estado vacío explícito, contando con lo que se oculta: nunca afirma que no hay nada si hay algo oculto. */
export function suggestionsEmpty(s: SuggestionsLike): SuggestionsEmpty {
  const t = s.thresholds;
  const needs = `Se necesitan ${t.invoice} facturas sin incidencias del mismo proveedor aprobadas por usted (o ${seenNeeded(t.invoice)} si solo se vieron en la primera lectura), o ${t.reply} respuestas aprobadas sin cambios al mismo remitente.`;
  const seen = s.seen ?? 0;
  const decisions = s.decisions ?? s.observed - seen;
  if (s.counted === 0) {
    if (s.observed === 0) return { title: "Aún no hay datos para sugerir reglas", hint: `No hay observaciones que cuenten. ${needs}`, lines: [] };
    const what = [
      decisions ? `${decisions} ${plural(decisions, "decisión", "decisiones")}` : null,
      seen ? `${seen} ${plural(seen, "factura vista", "facturas vistas")} en la primera lectura` : null,
    ].filter(Boolean);
    return {
      title: "Aún no hay observaciones que cuenten",
      hint: `Constan ${what.join(" y ")}, pero ninguna cuenta como evidencia ahora (por ejemplo, una edición, un rechazo, una factura con incidencias o una aprobación sin clasificar reinician el recuento). ${needs}`,
      lines: [],
    };
  }
  const lines = s.progress.map(progressLine);
  if (lines.length) return { title: "Aún no hay sugerencias, pero hay datos", hint: "Lo más cerca de sugerirse:", lines };
  const counted = `${s.counted} ${plural(s.counted, "observación cuenta", "observaciones cuentan")}`;
  if (suggestionsHidden(s.hidden))
    return { title: "No hay sugerencias que mostrar", hint: `${capitalize(counted)}, pero lo que podría sugerirse está oculto; abajo se dice qué y por qué.`, lines: [] };
  return { title: "No hay sugerencias que mostrar", hint: `${capitalize(counted)}, pero no hay ninguna sugerencia que mostrar. ${needs}`, lines: [] };
}

export const THRESHOLD_LABEL = {
  invoice: "Facturas sin incidencias del mismo proveedor",
  reply: "Respuestas aprobadas sin cambios al mismo remitente",
} as const;

/** Problema de un umbral escrito, o `null`. El backend lo valida de nuevo. */
export function thresholdProblem(raw: string, t: Pick<ThresholdsLike, "min" | "max">): string | null {
  const text = raw.trim();
  if (!/^\d{1,6}$/.test(text)) return "Escriba un número entero.";
  const n = Number(text);
  if (n < t.min) return `El mínimo es ${t.min}: con una sola observación no se sugiere nada.`;
  if (n > t.max) return `El máximo es ${t.max}.`;
  return null;
}

export function thresholdSaved(which: "invoice" | "reply", value: number): string {
  return `Umbral de ${which === "invoice" ? "facturas" : "respuestas"} guardado: ${value}. Queda en el historial. No crea ni activa ninguna regla.`;
}

export function suggestionAccepted(name: string): string {
  return `Regla «${name}» creada, inactiva. ${MERGE_NEXT_STEP}.`;
}

export const SUGGESTION_DISCARDED = "Sugerencia descartada. Solo volverá a aparecer con el doble de observaciones nuevas.";

// ---- Capa con modelo (D134): la IA afina, el código garantiza ----

export type RefineModeLike = "on_demand" | "automatic";

export interface ConsentLike {
  required: boolean;
  /** `not_needed`, `missing`, `granted` o `stale`. */
  state: string;
  provider: string | null;
  model: string | null;
  host: string | null;
  at: number | null;
}

export interface RefinementLike {
  mode: RefineModeLike;
  status: "refined" | "stale" | "running" | "not_refined";
  reason: string | null;
  detail: string | null;
  ran_at: number | null;
  last_ok_at: number | null;
  provider: string | null;
  model: string | null;
  fallback: { from: string; reason: string } | null;
  order: string[];
  hide: { fingerprint: string; reason: string }[];
  groups: string[][];
  limits: { fingerprint: string; max_total: string; shown: string; considered: number; would_automate: number }[];
  dropped: { kind: string; fingerprint: string; code: string }[];
  comment: string | null;
  comment_dropped: string | null;
  candidates: number;
  sent: number;
  covered: number;
  new_signals: number;
  min_new_signals: number;
  retry_at: number | null;
  gave_up: boolean;
  failures: number;
  max_attempts: number;
  consent: ConsentLike;
}

export interface RefinePreviewLike {
  provider: { provider: string; model: string };
  host: string | null;
  input_fp: string;
  external: boolean;
  via_local_first: boolean;
  local_fallback: boolean;
  blocked: string | null;
  blocked_message: string | null;
  consent: ConsentLike;
  candidates: {
    id: string;
    kind: string;
    supplier: string | null;
    category: string | null;
    sender_domain: string | null;
    count: number;
    seen?: number;
    earlier_breaks: number;
    discarded_before: boolean;
    amount_min: string | null;
    amount_max: string | null;
    withheld: string | null;
  }[];
  candidates_total: number;
  discarded_by_user: number;
  never_sent: string[];
}

/** Rótulo fijo del texto libre del modelo: se muestra siempre como texto plano. */
export const AI_COMMENT_LABEL = "Comentario de la IA";

/** Nota fija, siempre visible junto al comentario: ninguna validación del comentario la sustituye. */
export const AI_COMMENT_NOTE = "Es una opinión del modelo, no un dato; no se ha verificado.";

export const AI_REFINE_NEVER_ACTS =
  "La IA no crea ni activa reglas, no decide qué se automatiza y no cambia las cifras. Si no está disponible, las sugerencias del programa siguen igual.";

const PROVIDER_NAMES: Record<string, string> = {
  ollama: "la IA local (Ollama, en este equipo)",
  ollama_cloud: "Ollama cloud (ollama.com)",
  openai_compatible: "un servicio compatible con OpenAI",
  anthropic: "Claude (Anthropic)",
};

/** Nombre legible del proveedor, o el código si no se conoce. */
export function providerName(code: string): string {
  return PROVIDER_NAMES[code] ?? code;
}

/** Modo del canal, dicho en una frase: el panel siempre dice en cuál está. */
export function refineModeText(mode: RefineModeLike, minNewSignals: number, maxAttempts = 3): string {
  return mode === "on_demand"
    ? "Versión de prueba: la IA solo afina cuando usted pulsa «Afinar con IA», para reducir gasto. Antes verá a qué proveedor se envía y qué datos."
    : `Versión final: el afinado automático está desactivado hasta que usted lo active. Activado, afina solo, por lotes, cuando se acumulan al menos ${minNewSignals} decisiones válidas nuevas, una vez por conjunto de datos y, si falla por un motivo pasajero, hasta ${maxAttempts} intentos en total. También puede afinar a mano con el botón.`;
}

const REJECTION_TEXT: Record<string, string> = {
  not_an_object: "no era un objeto",
  unknown_field: "traía campos que no existen en el formato",
  missing_field: "faltaban campos del formato",
  wrong_type: "algún campo no tenía el tipo esperado",
  unknown_id: "se refería a sugerencias que no se enviaron",
  duplicate_id: "repetía una sugerencia donde no debía",
  too_many_items: "traía más elementos de los permitidos",
  unknown_reason: "daba un motivo fuera de la lista",
  bad_group: "un grupo no era válido",
  bad_amount: "un importe no era texto plano con dos decimales como mucho",
  comment_too_long: "el comentario era demasiado largo",
  comment_has_figures: "el comentario llevaba cifras, también escritas en letra, que solo pone el programa",
  comment_has_markup: "el comentario llevaba marcado, enlaces, saltos de línea o caracteres invisibles",
  comment_makes_claims: "el comentario afirmaba seguridad o verificación, o invitaba a activar algo",
  group_not_invoice: "agrupaba algo que no es una factura",
};

const FAILURE_TEXT: Record<string, string> = {
  unavailable: "el proveedor no estaba disponible (red o servicio)",
  timeout: "el proveedor no respondió a tiempo",
  rate_limited: "el proveedor limitó las peticiones",
  unauthorized: "el proveedor rechazó la clave; corríjala en Configuración",
  invalid_response: "la respuesta del proveedor no tenía el formato pedido",
  model_unavailable: "el proveedor no encuentra el modelo o la dirección elegidos",
  no_credit: "la cuenta del proveedor no tiene crédito",
  request_rejected: "el proveedor rechazó la petición",
};

/** Un motivo de respaldo, en palabras. */
export function failureWords(code: string): string {
  return FAILURE_TEXT[code] ?? "el proveedor falló";
}

const WAIT_TEXT: Record<string, string> = {
  needs_choice: "falta elegir un proveedor de IA en Configuración",
  needs_key: "falta guardar la clave del proveedor en Configuración",
  needs_consent: "falta confirmar en Configuración el destino de los datos",
  invalid_key: "la clave de IA no es válida o fue revocada",
  model_unavailable: "el proveedor no encuentra el modelo o la dirección elegidos",
  no_credit: "la cuenta del proveedor no tiene crédito",
};

export interface RefinementStatus {
  /** `ok`, `warn` o `info`: el color nunca es la única señal, el título lo dice. */
  tone: "ok" | "warn" | "info";
  title: string;
  /** Líneas explicativas, en orden. */
  lines: string[];
}

function when(millis: number): string {
  const d = new Date(millis);
  const two = (n: number) => String(n).padStart(2, "0");
  return `${two(d.getDate())}/${two(d.getMonth() + 1)} ${two(d.getHours())}:${two(d.getMinutes())}`;
}

/** Cuándo volverá a intentarlo sola la IA tras un fallo, o por qué ya no lo hará. */
export function retryText(v: Pick<RefinementLike, "retry_at" | "gave_up" | "failures" | "max_attempts" | "mode" | "reason" | "consent">): string | null {
  if (v.mode !== "automatic" || !v.reason || !["unavailable", "timeout", "rate_limited"].includes(v.reason)) return null;
  if (v.gave_up) return `Tras ${v.failures} intentos no volverá a intentarlo sola con estos datos; pulse «Afinar con IA» cuando quiera.`;
  if (v.consent.required && v.consent.state !== "granted") return "No lo intentará sola mientras el afinado automático no esté activado.";
  return v.retry_at ? `Volverá a intentarlo sola hacia las ${when(v.retry_at)} (intento ${v.failures + 1} de ${v.max_attempts}).` : null;
}

/** Por qué no hay afinado ahora, según el código del backend. */
function pendingReason(v: RefinementLike): string {
  switch (v.reason) {
    case "no_candidates":
      return "No hay sugerencias que afinar.";
    case "on_demand":
      return "En esta versión de prueba la IA solo afina cuando usted lo pide.";
    case "auto_consent_missing":
      return "El afinado automático está desactivado: no se envía nada a la IA hasta que usted lo active (botón «Activar afinado automático»). Mientras tanto puede afinar a mano.";
    case "auto_consent_stale":
      return `El afinado automático se activó para ${v.consent.provider ? providerName(v.consent.provider) : "otro proveedor"}${v.consent.model ? ` (modelo ${v.consent.model})` : ""}${v.consent.host ? ` en ${v.consent.host}` : ""} y ya no vale: el proveedor, el modelo o el servidor han cambiado. Vuelva a activarlo si quiere seguir.`;
    case "waiting_signals":
      return v.last_ok_at !== null
        ? `La IA afinará sola cuando haya ${v.min_new_signals} decisiones válidas nuevas; desde el último intento hay ${v.new_signals}.`
        : `La IA afinará sola cuando haya ${v.min_new_signals} decisiones válidas; hay ${v.new_signals}.`;
    case "pending_auto":
      return "Hay decisiones nuevas suficientes: se afinará en breve.";
    case "interrupted":
      return "El envío anterior no llegó a terminar (por ejemplo, se cerró la aplicación). No se repite solo para no gastar de más; puede pedirlo con el botón.";
    case "rejected":
      return `La respuesta de la IA se descartó entera porque ${REJECTION_TEXT[v.detail ?? ""] ?? "no cumplía el formato"}. No se aplicó nada de ella.`;
    default: {
      const code = v.reason ?? "";
      // Un afinado que se envió y falló (failures > 0) no es una espera: algo salió al proveedor,
      // así que no se dice «No se envió nada», aunque el código coincida con el de una espera.
      if (FAILURE_TEXT[code] && v.failures > 0) return `El último intento falló: ${FAILURE_TEXT[code]}.`;
      if (WAIT_TEXT[code]) return `No se envió nada: ${WAIT_TEXT[code]}.`;
      if (FAILURE_TEXT[code]) return `El último intento falló: ${FAILURE_TEXT[code]}.`;
      return "No se pudo afinar.";
    }
  }
}

/** Estado del afinado para la pestaña Sugeridas: afinado, no afinado y por qué, o afinando. */
export function refinementStatus(v: RefinementLike): RefinementStatus {
  const retry = retryText(v);
  switch (v.status) {
    case "running":
      return { tone: "info", title: "Afinando con IA…", lines: ["Mientras tanto, las sugerencias que ve son las calculadas por el programa."] };
    case "refined": {
      const lines = [
        `Afinado${v.ran_at ? ` el ${when(v.ran_at)}` : ""} con ${v.provider ? providerName(v.provider) : "la IA"}${v.model ? ` (modelo ${v.model})` : ""}.`,
      ];
      if (v.covered < v.candidates)
        lines.push(`Se aplica a ${v.covered} de ${v.candidates} sugerencias: las demás cambiaron desde entonces y se muestran como las calculó el programa.`);
      if (v.sent < v.candidates) lines.push(`Solo se envían las primeras ${v.sent} de ${v.candidates} sugerencias; el resto no se afina.`);
      if (v.comment_dropped) lines.push(commentDroppedText(v.comment_dropped));
      if (v.fallback) lines.push(`Respondió la IA local porque ${providerName(v.fallback.from)} no estaba disponible (${failureWords(v.fallback.reason)}).`);
      if (v.reason) {
        lines.push(`${pendingReason(v)} Se muestra el último afinado válido.`);
        if (retry) lines.push(retry);
      }
      lines.push(AI_REFINE_NEVER_ACTS);
      return { tone: "ok", title: "Afinado con IA", lines };
    }
    case "stale":
      return {
        tone: "warn",
        title: "Afinado anterior no vigente",
        lines: [
          `Las sugerencias cambiaron desde el último afinado${v.last_ok_at ? ` (${when(v.last_ok_at)})` : ""}, así que no se aplica ese resultado a ninguna. Las sugerencias que ve son las calculadas por el programa.`,
          pendingReason(v),
          ...(retry ? [retry] : []),
        ],
      };
    default: {
      const calm = ["on_demand", "waiting_signals", "no_candidates", "pending_auto", "auto_consent_missing"].includes(v.reason ?? "");
      return {
        tone: calm ? "info" : "warn",
        title: "No afinado",
        lines: [pendingReason(v), ...(retry ? [retry] : []), ...(v.sent < v.candidates ? [`Solo se enviarían las primeras ${v.sent} de ${v.candidates} sugerencias.`] : []), "Las sugerencias que ve son las calculadas por el programa, sin IA."],
      };
    }
  }
}

/** El botón «Afinar con IA»: existe en los dos modos (en la versión final también sirve para reintentar). */
export function refineButton(
  v: RefinementLike,
  preview: Pick<RefinePreviewLike, "blocked" | "blocked_message"> | null,
): { shown: boolean; disabled: boolean; why: string | null } {
  if (v.status === "running") return { shown: true, disabled: true, why: "Ya se está afinando." };
  if (v.candidates === 0) return { shown: true, disabled: true, why: "No hay sugerencias que afinar." };
  if (preview?.blocked) return { shown: true, disabled: true, why: preview.blocked_message ?? "Falta configurar la IA." };
  if (v.status === "refined" && v.covered === v.candidates && !v.reason) return { shown: true, disabled: true, why: "Estas sugerencias ya están afinadas. Si cambian, podrá volver a afinar." };
  return { shown: true, disabled: false, why: null };
}

function sentSubject(c: RefinePreviewLike["candidates"][number]): string {
  const cat = c.category ? ` (${categoryLabel(c.category)})` : "";
  if (c.kind === "invoice") return c.supplier ? `facturas de «${c.supplier}»` : "facturas de un proveedor cuyo nombre no se envía";
  return c.sender_domain ? `respuestas al dominio ${c.sender_domain}${cat}` : `respuestas a un remitente cuyo dominio no se envía${cat}`;
}

const WITHHELD_TEXT: Record<string, string> = {
  email: "su nombre lleva una dirección de correo",
  id_number: "su nombre lleva un número de identificación fiscal",
  iban: "su nombre lleva una cuenta bancaria",
  phone: "su nombre lleva un teléfono",
  public_mail_domain: "es un dominio de correo público (gmail.com y similares)",
  address: "su nombre lleva una dirección postal",
};

/** Aviso sobre datos personales, el mismo en la vista previa y en el consentimiento. */
export const PERSONAL_DATA_NOTE =
  "El nombre de un proveedor que es autónomo o un negocio pequeño identifica a una persona: se envía tal como figura en el documento, salvo si lleva un correo, un NIF o CIF, una cuenta bancaria o un teléfono (entonces no se envía), ni si lleva una dirección postal evidente (una vía con su número); otras formas de escribir una dirección pueden no detectarse. Un dominio propio de una persona o de un negocio pequeño también puede identificarla; los dominios de correo públicos no se envían.";

/** Con pocas facturas, el menor y el mayor importe son totales exactos de facturas. */
export const EXACT_AMOUNTS_NOTE = "Con pocas facturas, el menor y el mayor importe son los totales exactos de esas facturas.";

/** Lo que se va a enviar y a quién, antes de pedir el afinado. */
export function refinePreviewLines(p: RefinePreviewLike): { destination: string; fallback: string | null; items: string[]; never: string; notes: string[] } {
  const name = providerName(p.provider.provider);
  let destination: string;
  const where = p.host ? `, servidor ${p.host}` : "";
  if (!p.external) destination = `Se hará con ${name}, modelo ${p.provider.model}. Los datos no salen de este equipo.`;
  else if (p.via_local_first)
    destination = `Ahora mismo la IA local de este equipo atiende primero las peticiones (el proveedor no respondió hace poco). Si falla, se enviará a ${name}, modelo ${p.provider.model}${where}, y los datos de abajo saldrán de este equipo.`;
  else destination = `Se enviará a ${name}, modelo ${p.provider.model}${where}. Los datos de abajo salen de este equipo hacia ese proveedor.`;
  const fallback = p.external
    ? p.local_fallback
      ? "El respaldo con la IA local está activado: si ese proveedor falla y la IA local está preparada en este equipo, se repetirá con ella; si no, no se afinará. Nunca se usa otro proveedor externo."
      : "El respaldo con la IA local está desactivado: si el proveedor falla, no se afinará."
    : null;
  const items = p.candidates.map((c) => {
    const range = c.amount_min && c.amount_max ? `, importes de ${formatAmount(c.amount_min)} a ${formatAmount(c.amount_max)}` : "";
    const why = c.withheld ? ` (no se envía: ${WITHHELD_TEXT[c.withheld] ?? "dato personal"})` : "";
    const breaks = c.earlier_breaks === 0 ? "sin cortes anteriores de la serie" : `${c.earlier_breaks} ${plural(c.earlier_breaks, "corte anterior", "cortes anteriores")} de la serie (ediciones, rechazos o incidencias)`;
    const back = c.discarded_before ? "la descartó antes y vuelve a aparecer" : "no descartada antes";
    const seen = c.seen ? ` (${c.seen === c.count ? "todas" : c.seen} solo ${plural(c.seen, "vista", "vistas")} en la primera lectura, sin aprobar)` : "";
    return `${c.id}: ${sentSubject(c)}${why}, ${c.count} ${plural(c.count, "observación", "observaciones")}${seen}${range}, ${breaks}, ${back}`;
  });
  if (p.discarded_by_user) items.push(`Sugerencias que usted descartó: ${p.discarded_by_user}`);
  const notes = [PERSONAL_DATA_NOTE, EXACT_AMOUNTS_NOTE];
  if (p.candidates_total > p.candidates.length) notes.push(`Se envían como mucho ${p.candidates.length} sugerencias de las ${p.candidates_total} calculadas; el resto no se afina.`);
  return { destination, fallback, items, never: `No se envía nunca: ${p.never_sent.join(", ")}.`, notes };
}

/** Estado del consentimiento del afinado automático, en una frase. */
export function consentStatusText(c: ConsentLike): string | null {
  switch (c.state) {
    case "granted":
      return `Afinado automático activado para ${c.provider ? providerName(c.provider) : "este proveedor"}${c.model ? ` (modelo ${c.model})` : ""}${c.host ? `, servidor ${c.host}` : ""}${c.at ? `, desde el ${when(c.at)}` : ""}. Puede retirarlo cuando quiera.`;
    case "stale":
      return "Afinado automático desactivado: el proveedor, el modelo o el servidor han cambiado desde que lo activó.";
    case "missing":
      return "Afinado automático desactivado: no se envía nada hasta que lo active.";
    default:
      return null;
  }
}

/** Texto que se muestra antes de aceptar el afinado automático: lo que se envía, a quién y cuándo. */
export function consentLines(p: RefinePreviewLike, minNewSignals: number, maxAttempts = 3): { heading: string; lines: string[]; items: string[]; destination: string } {
  const preview = refinePreviewLines(p);
  const name = providerName(p.provider.provider);
  return {
    heading: `Activar el afinado automático con ${name}, modelo ${p.provider.model}${p.host ? `, servidor ${p.host}` : ""}`,
    destination: preview.destination,
    lines: [
      `Cada vez que se acumulen ${minNewSignals} decisiones válidas nuevas, la aplicación enviará sola a ${name} el resumen de sus sugerencias que ve abajo, sin preguntarle antes. Como mucho una vez por conjunto de datos y, si falla por un motivo pasajero, hasta ${maxAttempts} intentos.`,
      "El resumen sale de sus decisiones en este equipo, también de las que tomó antes de elegir este proveedor o mientras la IA era solo local: esas decisiones no se enviaron entonces, pero su resumen sí saldrá ahora.",
      ...preview.notes,
      preview.never,
      `El consentimiento vale solo para este proveedor, este modelo y este servidor${p.host ? ` (${p.host})` : ""}: si cambia cualquiera de los tres, se retira y tendrá que aceptarlo otra vez; volver al anterior no lo recupera. Puede retirarlo en cualquier momento; las sugerencias del programa no cambian.`,
    ],
    items: preview.items,
  };
}

export interface RefinedLists<T> {
  /** Las que se muestran en la lista principal, en orden de utilidad. */
  main: T[];
  /** Las que la IA propone ocultar y la persona no ha recuperado: visibles en su bloque. */
  hidden: { candidate: T; reason: string }[];
}

/**
 * Orden y bloque de ocultas. Solo trabaja con las candidatas que calculó el programa: una huella
 * del resultado que no esté entre ellas se ignora, y sin un afinado vigente no cambia nada.
 */
export function applyRefinement<T extends { fingerprint: string }>(candidates: T[], v: RefinementLike | null, restored: ReadonlySet<string>): RefinedLists<T> {
  if (!v || v.status !== "refined") return { main: candidates, hidden: [] };
  const rank = new Map(v.order.map((fp, i) => [fp, i] as const));
  const hideReason = new Map(v.hide.map((h) => [h.fingerprint, h.reason] as const));
  const sorted = candidates
    .map((c, i) => ({ c, i }))
    .sort((a, b) => (rank.get(a.c.fingerprint) ?? Infinity) - (rank.get(b.c.fingerprint) ?? Infinity) || a.i - b.i)
    .map((x) => x.c);
  const isHidden = (c: T) => hideReason.has(c.fingerprint) && !restored.has(c.fingerprint);
  return {
    main: sorted.filter((c) => !isHidden(c)),
    hidden: sorted.filter(isHidden).map((candidate) => ({ candidate, reason: hideReason.get(candidate.fingerprint)! })),
  };
}

const HIDE_TEXT: Record<string, string> = {
  low_value: "le parece poco útil",
  likely_one_off: "le parece un caso puntual",
  unclear_name: "el nombre no es claro",
};

export function hiddenBlockTitle(n: number): string {
  return `La IA propone ocultar ${n} ${plural(n, "sugerencia", "sugerencias")}`;
}

export const HIDDEN_BLOCK_NOTE =
  "No se han borrado ni descartado: siguen aquí y puede crearlas, descartarlas o devolverlas a la lista principal. Es solo una opinión de la IA.";

export function hiddenReasonText(reason: string): string {
  return `La IA propone ocultarla porque ${HIDE_TEXT[reason] ?? "la considera poco útil"}.`;
}

/** Pregunta sobre proveedores que quizá son el mismo. Nunca une nada: solo pregunta. */
export function groupQuestion(names: string[]): string {
  const quoted = names.map((n) => `«${n}»`);
  const list = quoted.length <= 2 ? quoted.join(" y ") : `${quoted.slice(0, -1).join(", ")} y ${quoted[quoted.length - 1]}`;
  return `¿${list} son el mismo proveedor? La IA cree que podrían serlo porque los nombres se parecen. El programa no puede comprobarlo sin el NIF y no los une por su cuenta: compruébelo con sus facturas.`;
}

/** Límite propuesto por la IA junto al del programa; la persona elige. Las cifras son del backend. */
export function modelLimitText(proposal: { max_total: string; considered: number; would_automate: number }, c: SuggestionLike): string {
  const mine = c.evidence.limit ? `, frente a ${formatAmount(c.evidence.limit)} del programa` : "";
  const range = c.evidence.amount_min && c.evidence.limit ? ` Está dentro del rango observado (de ${formatAmount(c.evidence.amount_min)} a ${formatAmount(c.evidence.limit)}).` : "";
  const sim = `En la simulación de ahora, con ${proposal.considered} ${plural(proposal.considered, "factura guardada", "facturas guardadas")}, esta regla habría registrado ${proposal.would_automate}.`;
  return `La IA propone un límite de ${formatAmount(proposal.max_total)}${mine}.${range} ${sim} Usted elige cuál usar; con cualquiera, la regla nace inactiva.`;
}

const DROPPED_TEXT: Record<string, string> = {
  out_of_range: "estaba fuera del rango de importes observado",
  same_as_code: "coincidía con el que ya propone el programa",
  not_positive: "no era un importe positivo",
  bad_amount: "no era un importe legible",
  no_range: "no había un rango de importes con el que compararlo",
  not_invoice: "se refería a algo que no es una factura",
  invalid_rule: "no producía una regla válida",
  simulation_failed: "no se pudo simular",
  simulation_no_effect: "en la simulación no habría registrado ninguna factura",
  simulation_blocks_other: "en la simulación dejaría sin actuar a otra regla activa",
  no_approved_amounts: "no hay importes aprobados por usted de los que partir: un límite no sale de lo que solo se ha visto",
};

/** Qué propuesta de la IA se descartó y por qué (sin contenido del modelo). */
export function droppedText(d: { kind?: string; code: string }): string {
  if (d.kind === "group") return "Se descartó una pregunta de agrupación de la IA porque incluía una sugerencia cuyo nombre no se envía.";
  return `Se descartó un límite propuesto por la IA porque ${DROPPED_TEXT[d.code] ?? "no pasó las comprobaciones"}.`;
}

const COMMENT_DROPPED_TEXT: Record<string, string> = {
  comment_number_words: "llevaba cifras escritas en letra, que solo pone el programa",
  comment_makes_claims: "afirmaba seguridad o verificación, o invitaba a activar algo",
};

/** Solo se descartó el comentario: el resto del afinado se conserva. */
export function commentDroppedText(code: string): string {
  return `El comentario de la IA se descartó porque ${COMMENT_DROPPED_TEXT[code] ?? "no cumplía las reglas"}; el resto del afinado se conserva.`;
}

/** Texto plano del comentario: sin caracteres de control ni de formato, ni marcado, y con longitud acotada. */
export function aiCommentText(raw: string): string {
  const flat = raw
    .replace(/[\u0000-\u001f\u007f-\u009f­؜᠎​-‏‪-‮⁠-⁯﻿<>`]/g, " ")
    .replace(/\s+/g, " ")
    .trim();
  return flat.length > 240 ? `${flat.slice(0, 237)}…` : flat;
}

export function modelLimitButtonLabel(limit: string): string {
  return `Crear regla con el límite de la IA (${formatAmount(limit)}, inactiva)`;
}

/** Aviso al crear la regla: dice «límite de la IA» solo si la regla creada lo lleva de verdad. */
export function acceptedWithModelLimit(name: string, created: string | undefined, proposed: string): string {
  if (created !== undefined && Number(created) === Number(proposed))
    return `Regla «${name}» creada, inactiva, con el límite de ${formatAmount(proposed)} que propuso la IA y que usted eligió. ${MERGE_NEXT_STEP}.`;
  return `Ya existía la regla «${name}», creada antes desde esta sugerencia${created ? ` con otro límite (${formatAmount(created)})` : ""}; no se ha creado otra. ${MERGE_NEXT_STEP}.`;
}

/** Fallo al leer el estado del afinado o la vista previa: se dice, no se oculta. */
export function refineLoadError(error: string): string {
  return `No se pudo leer el estado del afinado con IA: ${error}. Las sugerencias del programa no cambian.`;
}

// ---- «Crear regla» desde una revisión ----

export interface RuleSeedLike {
  action_kind: string;
  conditions: Record<string, unknown>;
  name: string;
  limit_from_this_invoice: boolean;
  limit_skipped?: { code: string; currencies: string[] } | null;
}

/** Borrador del formulario precargado con lo que calculó el backend. */
export function draftFromSeed(seed: RuleSeedLike): RuleDraft {
  return draftOf({ id: "", version: 0, action_kind: seed.action_kind, conditions: seed.conditions, active: false, name: seed.name });
}

/** Por qué no se precargó el límite de una factura, según el código del backend. */
function limitSkippedText(skipped: { code: string; currencies: string[] }): string {
  switch (skipped.code) {
    case "foreign_currency":
      return `el documento menciona ${skipped.currencies.join(", ")} y el límite de una regla está en euros`;
    case "currency_not_shown":
      return "el documento no muestra la moneda y no se puede comprobar que el total esté en euros";
    case "currency_not_checked":
      return "no consta la comprobación de moneda de esta factura";
    case "amount_review":
      return "el importe de esta factura está pendiente de comprobar";
    case "no_total":
      return "no consta un total legible";
    default:
      return "no se pudo comprobar el total";
  }
}

/** Qué se precargó y de dónde sale, sin prometer más de lo que hay. */
export function seedNotice(seed: RuleSeedLike): string {
  let base: string;
  if (seed.action_kind !== "register_invoice") {
    base = "Precargado desde este correo: la categoría y la dirección del remitente.";
  } else if (seed.limit_skipped) {
    base = `Precargado desde esta factura: solo el proveedor, porque ${limitSkippedText(seed.limit_skipped)}, así que no se precarga ningún límite.`;
  } else if (seed.limit_from_this_invoice) {
    base = "Precargado desde esta factura: el proveedor y, como límite, el total de esta factura. Es un solo dato, no una estadística; ajústelo.";
  } else {
    base = "Precargado desde esta factura: solo el proveedor.";
  }
  return `${base} La regla nacerá inactiva.`;
}
