// Preferencias por remitente (D161) en la interfaz, sin DOM: se prueban con `node --test`.
// El backend normaliza, rechaza los dominios públicos y decide; aquí solo se ofrece la acción y
// se redactan los textos, sin prometer más de lo que hace: solo cambia los correos nuevos.

import type { PreferenceChange, SenderPreference } from "./types";

export type PreferenceRoute = SenderPreference["route"];
export type PreferenceScope = SenderPreference["scope"];

export const ROUTE_LABEL: Record<PreferenceRoute, string> = {
  informative: "Siempre a Informativos",
  review: "Siempre a Pendientes",
};

export const PREFERENCES_TITLE = "Remitentes con preferencia";
export const PREFERENCES_INTRO =
  "Usted decide qué remitentes van siempre a Informativos o a Pendientes. Solo afecta a los correos nuevos y NestAdmin no aprende ninguna por su cuenta.";
export const PREFERENCES_EMPTY = "No hay preferencias. Puede fijarlas desde el detalle de un correo, en Pendientes o en Informativos.";
export const PREFERENCE_REMOVE = "Quitar";
export const PREFERENCE_CANCEL = "Cancelar";
export const PUBLIC_DOMAIN_NOTE = "Es un servicio de correo público: solo se puede fijar para esta dirección, no para todo el dominio.";
export const PREFERENCE_INFORMATIVE_TITLE = "Los correos nuevos de este remitente irán a Informativos sin pasar por la IA";
export const PREFERENCE_REVIEW_TITLE = "Los correos nuevos de este remitente nunca se guardarán solos en Informativos";

/** Dominios de correo público: espejo de `is_public_mail_domain` (backend), solo para no ofrecer lo que rechazaría. */
const PUBLIC_DOMAINS = new Set([
  "gmail.com", "googlemail.com", "outlook.com", "outlook.es", "hotmail.com", "hotmail.es", "live.com", "msn.com",
  "yahoo.com", "yahoo.es", "ymail.com", "icloud.com", "me.com", "mac.com", "aol.com", "proton.me", "protonmail.com",
  "gmx.com", "gmx.es", "gmx.net", "mail.com", "zoho.com", "yandex.com", "telefonica.net", "movistar.es", "orange.es",
]);

export function isPublicDomain(domain: string): boolean {
  return PUBLIC_DOMAINS.has(domain.trim().toLowerCase());
}

/** Dirección completa y dominio en minúsculas de `Nombre <a@b>` o `a@b`; `null` si no hay dirección válida. */
export function senderParts(from: unknown): { address: string; domain: string } | null {
  if (typeof from !== "string") return null;
  const open = from.lastIndexOf("<");
  const close = from.lastIndexOf(">");
  const inner = open >= 0 && close > open ? from.slice(open + 1, close) : from;
  const text = inner.trim().replace(/^"+|"+$/g, "").trim();
  const at = text.lastIndexOf("@");
  if (at < 1) return null;
  const local = text.slice(0, at).trim().toLowerCase();
  const domain = text.slice(at + 1).trim().toLowerCase();
  const validDomain = /^[\p{L}\p{N}-]+(\.[\p{L}\p{N}-]+)+$/u.test(domain);
  if (!validDomain || !local || /[\s@]/.test(local)) return null;
  return { address: `${local}@${domain}`, domain };
}

/** Preferencia vigente de un remitente: la de su dirección y, si no hay, la de su dominio. */
export function preferenceFor(prefs: readonly SenderPreference[], from: unknown): SenderPreference | null {
  const parts = senderParts(from);
  if (!parts) return null;
  return (
    prefs.find((p) => p.scope === "address" && p.key === parts.address) ??
    prefs.find((p) => p.scope === "domain" && p.key === parts.domain) ??
    null
  );
}

export interface PreferenceChoice {
  scope: PreferenceScope;
  key: string;
  label: string;
}

/** Opciones de la confirmación: solo esta dirección y, si no es un servicio público, todo el dominio. */
export function preferenceChoices(from: unknown): PreferenceChoice[] {
  const parts = senderParts(from);
  if (!parts) return [];
  const choices: PreferenceChoice[] = [{ scope: "address", key: parts.address, label: `Solo ${parts.address}` }];
  if (!isPublicDomain(parts.domain)) choices.push({ scope: "domain", key: parts.domain, label: `Todo @${parts.domain}` });
  return choices;
}

/** ¿Se ofrece fijar esta ruta para el remitente? No si no hay dirección o si ya manda esa ruta. */
export function canOfferPreference(prefs: readonly SenderPreference[], from: unknown, route: PreferenceRoute): boolean {
  if (!senderParts(from)) return false;
  return preferenceFor(prefs, from)?.route !== route;
}

/** Explicación del efecto, que se muestra antes de confirmar. */
export function preferenceConfirmText(route: PreferenceRoute, from: unknown): string {
  const parts = senderParts(from);
  const who = parts ? parts.address : "este remitente";
  if (route === "informative")
    return `A partir de ahora, los correos nuevos de ${who} se guardarán en Informativos sin pasar por la IA y sin borrador. Los que ya tiene no cambian y nada se borra. Puede quitarlo en Cuentas de correo.`;
  return `A partir de ahora, los correos nuevos de ${who} nunca se guardarán solos en Informativos: pasarán por la IA y quedarán en Pendientes aunque parezcan automáticos. Una regla no los enviará sin su revisión si llevan señales de envío automático. Los que ya tiene no cambian.`;
}

export function scopeText(p: Pick<SenderPreference, "scope" | "key">): string {
  return p.scope === "domain" ? `Todo @${p.key}` : p.key;
}

/** Línea de una preferencia en la lista y en el detalle de un correo. */
export function preferenceLine(p: Pick<SenderPreference, "scope" | "key" | "route">): string {
  return `${scopeText(p)}: ${ROUTE_LABEL[p.route].toLowerCase()}`;
}

/** Aviso (no bloqueo) de una preferencia que contradice una regla activa de envío. */
export function preferenceWarningText(change: Pick<PreferenceChange, "warnings">): string | null {
  const names = change.warnings.map((w) => `«${w.rule_name}»`);
  if (!names.length) return null;
  const list = names.length === 1 ? names[0]! : `${names.slice(0, -1).join(", ")} y ${names[names.length - 1]}`;
  return names.length === 1
    ? `Contradice la regla activa ${list}: los correos de este remitente ya no pasarán por la IA, así que esa regla no podrá responderlos.`
    : `Contradice las reglas activas ${list}: los correos de este remitente ya no pasarán por la IA, así que esas reglas no podrán responderlos.`;
}

export function preferenceSavedText(change: PreferenceChange): string {
  const p = change.preference;
  return change.changed ? `Preferencia guardada. ${preferenceLine(p)}` : `Ya estaba fijada. ${preferenceLine(p)}`;
}

/** Detalle del Historial de una preferencia, en palabras; el valor en bruto solo si no se entiende. */
export function preferenceAuditText(detail: string | null): string | null {
  if (!detail) return null;
  try {
    const d = JSON.parse(detail) as { scope?: string; key?: string; route?: string; previous?: string | null };
    if ((d.scope !== "address" && d.scope !== "domain") || typeof d.key !== "string") return detail;
    const who = scopeText({ scope: d.scope, key: d.key });
    const route = d.route === "informative" || d.route === "review" ? ROUTE_LABEL[d.route].toLowerCase() : null;
    const before = d.previous === "informative" || d.previous === "review" ? ROUTE_LABEL[d.previous].toLowerCase() : "sin preferencia";
    return route ? `${who}: ${route} (antes: ${before})` : `${who}: preferencia quitada (era: ${before})`;
  } catch {
    return detail;
  }
}
