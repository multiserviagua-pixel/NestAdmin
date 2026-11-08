// Lógica y textos de Pendientes (D137), sin DOM: se prueban con `node --test`.

export type ReviewTab = "invoices" | "emails";
export type FlagTone = "warn" | "error";

export interface Flag {
  label: string;
  tone: FlagTone;
  /** Por qué, con datos concretos. */
  why: string;
  /** Intento anterior sin confirmar: el detalle lo explica en su propio recuadro. */
  recovery?: boolean;
}

/** Lo mínimo de una tarea que necesita esta lógica. */
export interface ReviewTask {
  id: number;
  kind: string;
  created_at: number;
  result: Record<string, any> | null;
}

export function tabOf(kind: string): ReviewTab {
  return kind === "invoice" ? "invoices" : "emails";
}

export function isKnownKind(kind: string): boolean {
  return kind === "invoice" || kind === "email";
}

export function splitByTab<T extends { kind: string }>(tasks: T[]): Record<ReviewTab, T[]> {
  return {
    invoices: tasks.filter((t) => tabOf(t.kind) === "invoices"),
    emails: tasks.filter((t) => tabOf(t.kind) === "emails"),
  };
}

/** La pestaña inicial es la primera con elementos; una ya elegida no cambia sola. */
export function chooseTab(current: ReviewTab | null, counts: Record<ReviewTab, number>): ReviewTab {
  if (current) return current;
  return counts.invoices === 0 && counts.emails > 0 ? "emails" : "invoices";
}

/** Importe en formato español. Sin dato, null; un valor que no es un número se deja literal. */
export function formatAmount(value: unknown, currency: unknown): string | null {
  if (value === null || value === undefined || value === "") return null;
  const text = String(value);
  if (!/^-?\d+(\.\d+)?$/.test(text)) return text;
  const code = typeof currency === "string" && /^[A-Z]{3}$/.test(currency) ? currency : null;
  const decimals = Math.max(2, text.split(".")[1]?.length ?? 0);
  return new Intl.NumberFormat("es-ES", {
    minimumFractionDigits: decimals,
    maximumFractionDigits: decimals,
    ...(code ? { style: "currency", currency: code } : {}),
  }).format(Number(text));
}

/** D120: el total es una propuesta si se recuperó de un literal dudoso. */
export function totalIsProposed(result: Record<string, any> | null): boolean {
  return ((result?.recovered ?? []) as Array<{ field: string }>).some((a) => a.field === "total");
}

/** Antigüedad en la bandeja. `created` y `now` en milisegundos, como `Task.created_at`. */
export function ageText(created: number, now: number): string {
  const days = Math.floor((now - created) / 86_400_000);
  if (days <= 0) return "hoy";
  return days === 1 ? "ayer" : `hace ${days} días`;
}

function recoveryFlag(r: Record<string, any>): Flag | null {
  if (r.recovery?.needs_confirmation === true) {
    return {
      label: "Requiere confirmación",
      tone: "error",
      recovery: true,
      why: `El borrador de la versión ${r.recovery.version ?? "anterior"} ya no existe y no consta enviado: confirme que no salió antes de preparar otro.`,
    };
  }
  if (r.recovery || (r.attempt && r.attempt.version === (r.version ?? 1))) {
    return {
      label: "Intento sin confirmar",
      tone: "warn",
      recovery: true,
      why: `Ya se intentó la versión ${r.recovery?.version ?? r.attempt?.version} y no consta confirmado; aprobar reintenta sin repetirlo.`,
    };
  }
  return null;
}

/** Incidencias de una factura, sin repetir etiqueta. */
export function invoiceFlags(result: Record<string, any> | null): Flag[] {
  const r = result ?? {};
  const fields = r.fields ?? {};
  const flags: Flag[] = [];
  const add = (flag: Flag) => {
    if (!flags.some((f) => f.label === flag.label)) flags.push(flag);
  };
  const recovery = recoveryFlag(r);
  if (recovery) add(recovery);
  // D120: el mensaje nombra el archivo, la copia guardada y los importes; va antes que la marca genérica.
  if (r.amount_review) add({ label: "Importe dudoso", tone: "warn", why: String(r.amount_review) });
  for (const issue of (r.issues ?? []) as string[]) {
    if (issue === "total_mismatch") {
      add({
        label: "Total no cuadra",
        tone: "warn",
        why: `Base ${fields.subtotal ?? "?"} + IVA ${fields.tax ?? "?"} no coincide con el total ${fields.total ?? "?"}.`,
      });
    } else if (issue === "missing_total") add({ label: "Falta el total", tone: "warn", why: "No se encontró el total en el documento." });
    else if (issue === "missing_tax") add({ label: "Falta el IVA", tone: "warn", why: "No se encontró el IVA en el documento." });
    else if (issue === "possible_duplicate") add({ label: "Posible duplicado", tone: "warn", why: "Se parece a otra factura ya procesada." });
    else if (issue === "amount_to_review") add({ label: "Importe dudoso", tone: "warn", why: "El importe se leyó con dudas; compruébelo con el documento." });
    else add({ label: `Incidencia: ${issue}`, tone: "warn", why: `Código de incidencia no reconocido: ${issue}.` });
  }
  const duplicates = (r.duplicates ?? []) as Array<{ existing_id: string }>;
  if (duplicates.length) {
    const why = `Se parece a: ${duplicates.map((d) => `tarea ${d.existing_id}`).join(", ")}. No se ha descartado; decida usted.`;
    const existing = flags.find((f) => f.label === "Posible duplicado");
    if (existing) existing.why = why;
    else add({ label: "Posible duplicado", tone: "warn", why });
  }
  const lowPages = ((r.pages ?? []) as Array<{ number: number; low_confidence?: boolean }>).filter((p) => p.low_confidence).map((p) => p.number);
  if (lowPages.length) {
    add({
      label: "OCR de baja calidad",
      tone: "warn",
      why: `El OCR leyó con poca confianza ${lowPages.length === 1 ? `la página ${lowPages[0]}` : `las páginas ${lowPages.join(", ")}`}; compruebe los datos con el original.`,
    });
  }
  const unreadable = (r.unreadable ?? []) as Array<{ field: string }>;
  if (unreadable.length) add({ label: "Campo ilegible", tone: "warn", why: `No se pudo leer: ${unreadable.map((u) => u.field).join(", ")}.` });
  return flags;
}

/** Incidencias de un correo. */
export function emailFlags(result: Record<string, any> | null): Flag[] {
  const r = result ?? {};
  const flags: Flag[] = [];
  const recovery = recoveryFlag(r);
  if (recovery) flags.push(recovery);
  if (!(r.draft?.to ?? []).length) flags.push({ label: "Sin destinatario", tone: "warn", why: "El borrador no tiene destinatario y no se puede enviar." });
  return flags;
}

export function flagsOf(task: ReviewTask): Flag[] {
  return tabOf(task.kind) === "invoices" ? invoiceFlags(task.result) : emailFlags(task.result);
}

/** Primero lo que tiene incidencias y, dentro de cada grupo, lo más antiguo. */
export function sortForReview<T extends ReviewTask>(tasks: T[]): T[] {
  const rank = (t: T) => (flagsOf(t).length ? 0 : 1);
  return [...tasks].sort((a, b) => rank(a) - rank(b) || a.created_at - b.created_at || a.id - b.id);
}

/**
 * Elemento seleccionado tras un cambio de la lista: se conserva si sigue y, si se ha resuelto,
 * pasa al que venía detrás en el orden anterior (o al último que quede).
 */
export function nextSelection(previousOrder: number[], current: number[], selected: number | null): number | null {
  if (selected !== null && current.includes(selected)) return selected;
  const at = selected === null ? -1 : previousOrder.indexOf(selected);
  if (at >= 0) {
    for (const id of previousOrder.slice(at + 1)) if (current.includes(id)) return id;
    return current[current.length - 1] ?? null;
  }
  return current[0] ?? null;
}
