// Textos del panel de reglas (R21). Solo funciones y datos puros, sin DOM, para poder
// probarlos con `node --test`. Solo tipos borrables (sin `enum`): Node los ejecuta tal cual.
// Todos los ejemplos son inventados.

export interface RuleLike {
  id: string;
  version: number;
  action_kind: string;
  conditions: Record<string, unknown>;
  active: boolean;
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
  id: "Nombre corto para reconocer la regla en el historial. Solo admite minúsculas, números y guiones, y no se puede cambiar después.",
  action:
    "Qué hará la regla cuando coincida: registrar una factura en el sistema administrativo o enviar la respuesta que redactó la IA. No se puede cambiar después.",
  supplier:
    "Obligatorio. Nombre del proveedor tal como lo lee la aplicación en la factura. Debe coincidir exactamente, sin importar mayúsculas: «Alfa SL» no coincide con «Alfa, S.L.». Si el nombre no coincide, la factura se queda en Pendientes para que usted la revise.",
  max_total:
    "Opcional. Importe máximo, en euros, hasta el cual se registra sin revisión; escriba, por ejemplo, 500.00. Las facturas con un total mayor se quedan en Pendientes. Sin máximo, se registrarán sin revisión facturas de cualquier importe de ese proveedor.",
  category:
    "Categoría en la que la IA clasifica el correo recibido. La regla se aplica a todos los correos que la IA clasifique así, sea quien sea el remitente.",
  search: "Busca por el texto de la regla, su identificador, el proveedor o la categoría.",
  filter_action: "Muestra solo las reglas que registran facturas o solo las que envían respuestas.",
  filter_status: "Una regla inactiva no hace nada: solo una regla activa puede actuar sin revisión.",
  filter_category: "Muestra solo las reglas de envío que se aplican a los correos de esta categoría.",
  column_rule:
    "La frase describe lo que hará la regla. El aviso de debajo recuerda su riesgo y el identificador sirve para encontrarla en el historial.",
  column_action: "Lo que la regla hace sin preguntarle cuando coincide.",
  column_conditions: "Datos que deben coincidir para que la regla actúe. Si alguno no coincide, la tarea queda en Pendientes.",
  column_status:
    "Activa: puede actuar sin revisión. Inactiva: no hace nada. Una regla nueva nace inactiva hasta que usted la active.",
  column_version:
    "Número interno para el historial: sube también al activar o desactivar la regla, aunque no cambie lo que hace.",
  column_actions: "Activar o desactivar, editar las condiciones o crear una regla nueva a partir de esta.",
  category_name: "Clasificación que la IA asigna a cada correo recibido. Son cinco y no se pueden cambiar.",
  category_count: "Correos que constan en este equipo con esa categoría, incluidos los ya tratados.",
  category_rules: "Reglas de envío que se aplican a los correos de esta categoría.",
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
    const which = category ? `como ${categoryLabel(category)}` : "(la regla no indica categoría)";
    return `Enviar sin revisión la respuesta redactada por la IA a los correos clasificados ${which}`;
  }
  return FALLBACK_UNKNOWN;
}

/** Aviso que debe verse siempre junto a la regla. */
export function ruleRisk(rule: RuleLike): string {
  if (rule.action_kind === "send_draft")
    return "Enviará correos sin preguntarle a cualquier remitente que la IA clasifique en esta categoría.";
  if (rule.action_kind === "register_invoice")
    return "Registrará la factura sin preguntarle si coincide y no tiene incidencias.";
  return "La aplicación no reconoce este tipo de regla, así que no la aplicará.";
}

/** Problema de la regla que impide que actúe (fallo cerrado, D123), o `null`. */
export function ruleProblem(rule: RuleLike): string | null {
  const c = rule.conditions;
  if (!c || typeof c !== "object" || Array.isArray(c)) return "Las condiciones de la regla no son válidas: no actuará.";
  if (!isKnownAction(rule.action_kind)) return null;
  const allowed = rule.action_kind === "register_invoice" ? ["supplier", "max_total"] : ["category"];
  const unknown = Object.keys(c).find((k) => !allowed.includes(k));
  if (unknown) return `La regla tiene una condición desconocida («${unknown}»): no actuará.`;
  if (rule.action_kind === "register_invoice" && !text(c.supplier)) return "La regla no indica proveedor: no actuará.";
  if (rule.action_kind === "register_invoice" && "max_total" in c && typeof c.max_total !== "string")
    return "El máximo de la regla no es un importe válido: no actuará.";
  if (rule.action_kind === "send_draft" && !text(c.category)) return "La regla no indica categoría: no actuará.";
  return null;
}

/** Condiciones en una línea corta, para la columna «Condiciones». */
export function conditionsSummary(rule: RuleLike): string {
  const c = rule.conditions ?? {};
  if (rule.action_kind === "register_invoice") {
    const supplier = text(c.supplier);
    const max = text(c.max_total);
    return `Proveedor ${supplier ? `«${supplier}»` : "sin indicar"}${max ? `, total ≤ ${formatAmount(max)}` : ", sin máximo"}, sin incidencias`;
  }
  if (rule.action_kind === "send_draft") {
    const category = text(c.category);
    return `Categoría ${category ? `«${categoryLabel(category)}»` : "sin indicar"}, con destinatario`;
  }
  return "Sin datos comparables";
}

/** Texto en minúsculas y sin acentos sobre el que busca el buscador. */
export function searchText(rule: RuleLike): string {
  const c = rule.conditions ?? {};
  const category = text(c.category);
  return normalizeSearch([ruleSentence(rule), rule.id, text(c.supplier), category, category ? categoryLabel(category) : ""].join(" "));
}

export function normalizeSearch(value: string): string {
  return value.normalize("NFD").replace(/[̀-ͯ]/g, "").toLowerCase();
}
