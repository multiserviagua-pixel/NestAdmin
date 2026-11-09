// Perfil del negocio (E6, D163) en la interfaz, sin DOM: se prueba con `node --test`.
// El backend valida, decide qué sale y guarda el consentimiento; aquí solo se cuenta, se redacta
// y se combina la precarga, sin prometer más de lo que hace.

import type { BusinessProfile, ProfileConsentView, ProfileFinding, ProfilePreview, ProfileSaved } from "./types";

export const PROFILE_MAX_CHARS = 2000;

export const PROFILE_NOTICE = "Escriba solo lo que pueda leer cualquier remitente: la IA puede citarlo en una respuesta.";
export const PROFILE_TITLE = "Perfil del negocio";
export const PROFILE_INTRO =
  "Horario, tono, firma y políticas para que las respuestas redactadas por la IA se parezcan a las suyas. Solo se usa al redactar una respuesta, nunca al clasificar ni al leer facturas. Si no lo rellena o no lo autoriza, la redacción sigue funcionando sin él.";

export const PROFILE_FIELDS: ReadonlyArray<{ key: keyof BusinessProfile; label: string; rows: number; hint: string }> = [
  { key: "display_name", label: "Nombre del negocio", rows: 1, hint: "Opcional" },
  { key: "hours", label: "Horario", rows: 2, hint: "Por ejemplo: lunes a viernes, de 9:00 a 14:00" },
  { key: "tone", label: "Tono", rows: 2, hint: "Por ejemplo: cercano y breve, tratando de usted" },
  { key: "signature", label: "Firma", rows: 3, hint: "Cómo termina sus respuestas" },
  { key: "policies", label: "Políticas", rows: 5, hint: "Devoluciones, plazos de pago, condiciones…" },
];

const EMPTY: BusinessProfile = { display_name: "", hours: "", tone: "", signature: "", policies: "" };

export function emptyProfile(): BusinessProfile {
  return { ...EMPTY };
}

/** Caracteres de los cinco campos, contados como el backend (por carácter, no por byte). */
export function profileChars(p: BusinessProfile): number {
  return PROFILE_FIELDS.reduce((sum, f) => sum + [...(p[f.key] ?? "")].length, 0);
}

/** Con punto de millar siempre (el español no lo pone en las cifras de cuatro dígitos). */
function thousands(n: number): string {
  return String(n).replace(/\B(?=(\d{3})+(?!\d))/g, ".");
}

/** «1.234 de 2.000 caracteres»; `over` si se pasa del tope. */
export function counterText(p: BusinessProfile): { text: string; over: boolean } {
  const n = profileChars(p);
  return { text: `${thousands(n)} de ${thousands(PROFILE_MAX_CHARS)} caracteres`, over: n > PROFILE_MAX_CHARS };
}

const trimmed = (p: BusinessProfile): BusinessProfile => ({
  display_name: p.display_name.trim(),
  hours: p.hours.trim(),
  tone: p.tone.trim(),
  signature: p.signature.trim(),
  policies: p.policies.trim(),
});

/** Si lo escrito difiere de lo guardado (sin contar los espacios de los extremos). */
export function isDirty(draft: BusinessProfile, saved: BusinessProfile): boolean {
  const a = trimmed(draft);
  const b = trimmed(saved);
  return PROFILE_FIELDS.some((f) => a[f.key] !== b[f.key]);
}

/** Se puede guardar: hay cambios y no se pasa del tope (lo demás lo decide el backend). */
export function canSave(draft: BusinessProfile, saved: BusinessProfile): boolean {
  return isDirty(draft, saved) && profileChars(draft) <= PROFILE_MAX_CHARS;
}

/** La precarga solo rellena los campos vacíos: nunca pisa lo que la persona ya escribió. */
export function applyPrefill(draft: BusinessProfile, prefill: BusinessProfile): BusinessProfile {
  const out = { ...draft };
  for (const f of PROFILE_FIELDS) if (!draft[f.key].trim() && prefill[f.key].trim()) out[f.key] = prefill[f.key];
  return out;
}

const FIELD_LABEL = new Map<string, string>([...PROFILE_FIELDS.map((f) => [f.key, f.label] as const), ["total", "El perfil"]]);

/** Un hallazgo del examen, dicho en una frase con su campo. */
export function findingText(f: ProfileFinding): string {
  const where = FIELD_LABEL.get(f.field) ?? f.field;
  switch (f.code) {
    case "iban":
      return `${where}: hay un IBAN. Quítelo, no puede ir en el perfil.`;
    case "card":
      return `${where}: hay un número de tarjeta. Quítelo, no puede ir en el perfil.`;
    case "secret":
      return `${where}: hay algo que parece una clave o un token. Quítelo.`;
    case "too_long":
      return `El perfil supera los ${thousands(PROFILE_MAX_CHARS)} caracteres: acórtelo.`;
    case "phone":
      return `${where}: incluye un teléfono. Se enviará tal cual a la IA y cualquier remitente podrá leerlo en una respuesta.`;
    case "email":
      return `${where}: incluye una dirección de correo. Se enviará tal cual a la IA y cualquier remitente podrá leerla en una respuesta.`;
  }
}

/** Avisos (teléfono, correo) del último guardado, sin repetidos. */
export function warningTexts(warnings: readonly ProfileFinding[]): string[] {
  return [...new Set(warnings.map(findingText))];
}

/** Aviso tras guardar. */
export function savedNotice(saved: ProfileSaved): string {
  if (!saved.view.saved) return "Perfil borrado";
  return saved.consent_withdrawn ? "Perfil guardado. Cambia lo que se enviaría: vuelva a autorizarlo" : "Perfil guardado";
}

const PROVIDER_NAMES: Record<string, string> = {
  ollama: "la IA local (Ollama)",
  ollama_cloud: "Ollama cloud",
  openai_compatible: "Compatible con OpenAI",
  anthropic: "Claude (Anthropic)",
};

const providerName = (code: string): string => PROVIDER_NAMES[code] ?? code;

/** El destino con su nombre: «Ollama cloud (gemma4:31b)». */
export function destinationText(p: Pick<ProfilePreview, "provider">): string {
  return `${providerName(p.provider.provider)} (${p.provider.model})`;
}

/** La frase del consentimiento, con el destino nombrado (D163). */
export function consentSentence(p: Pick<ProfilePreview, "provider">): string {
  return `Se enviará a ${destinationText(p)} al redactar respuestas`;
}

export interface ProfileStatus {
  /** Una línea con el estado actual. */
  line: string;
  /** Si se ofrece autorizar. */
  canGrant: boolean;
  /** Si se ofrece retirar. */
  canWithdraw: boolean;
}

/** Estado del envío del perfil y qué acciones se ofrecen. */
export function statusOf(p: ProfilePreview): ProfileStatus {
  if (!p.has_profile) return { line: "No hay perfil: la redacción no lleva nada.", canGrant: false, canWithdraw: false };
  if (p.blocked) return { line: "El perfil guardado no puede enviarse: " + findingText({ field: "total", code: p.blocked }).replace(/^El perfil: /, ""), canGrant: false, canWithdraw: false };
  if (!p.external) return { line: "Con la IA local el perfil no sale de este equipo: se usa sin más autorización.", canGrant: false, canWithdraw: false };
  const c: ProfileConsentView = p.consent;
  const where = destinationText(p) + (p.host ? `, servidor ${p.host}` : "");
  switch (c.state) {
    case "granted":
      return { line: `Autorizado: se enviará a ${where} al redactar respuestas.`, canGrant: false, canWithdraw: true };
    case "stale":
      return { line: "La autorización ya no vale (cambió el proveedor, el modelo, el servidor o el perfil). Revise lo que se enviaría y vuelva a autorizar.", canGrant: !!p.block, canWithdraw: true };
    default:
      return { line: "Sin autorización: la redacción no lleva el perfil.", canGrant: !!p.block, canWithdraw: false };
  }
}

/** Texto del recuadro de confirmación: el destino nombrado y lo que no ocurre. */
export function consentLines(p: ProfilePreview): { heading: string; lines: string[] } {
  const where = p.host ? ` (servidor ${p.host})` : "";
  return {
    heading: consentSentence(p),
    lines: [
      `Sale el bloque que ve abajo, tal cual, solo en la llamada que redacta la respuesta${where}. No sale al clasificar ni al leer facturas.`,
      p.local_fallback
        ? "Si ese proveedor no responde, la redacción se repite con la IA local de este equipo, que también usa el perfil sin que salga de aquí."
        : "No hay respaldo con la IA local activado.",
      "Vale solo para este proveedor, este modelo, este servidor y este contenido: si cambia cualquiera, se retira y tendrá que aceptarlo otra vez. Puede retirarlo cuando quiera; la redacción seguirá funcionando sin el perfil.",
      PROFILE_NOTICE,
    ],
  };
}
