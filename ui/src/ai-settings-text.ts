// Historial de los cambios de configuración de la IA (D186b). Texto puro, sin DOM: el backend
// anota `ai_setting` con un estado cerrado y un detalle JSON que nunca lleva la clave, ni su
// longitud, ni un fragmento, ni una URL con credenciales. Aquí solo se redacta lo que consta.

const PROVIDER_NAME: Record<string, string> = {
  ollama: "Ollama (local)",
  ollama_cloud: "Ollama cloud",
  openai_compatible: "Compatible con OpenAI",
  anthropic: "Claude · Anthropic",
};

function providerName(provider: string): string {
  return PROVIDER_NAME[provider] ?? provider;
}

export const AI_SETTING_ACTION = "ai_setting";

/** Etiqueta de la acción en la tabla del Historial. */
export const AI_SETTING_ACTION_LABEL: Record<string, string> = {
  [AI_SETTING_ACTION]: "Configuración de la IA",
};

const STATUS_LABEL: Record<string, string> = {
  provider_selected: "Proveedor elegido",
  local_fallback_on: "Respaldo con la IA local activado",
  local_fallback_off: "Respaldo con la IA local desactivado",
  key_saved: "Clave guardada",
  key_deleted: "Clave borrada",
};

/** Resultado en palabras; un estado desconocido se muestra tal cual, sin inventar. */
export function aiSettingStatusLabel(status: string): string {
  return STATUS_LABEL[status] ?? status;
}

/** Detalle en palabras de un evento `ai_setting`; `null` si no es de configuración de la IA. */
export function aiSettingAuditText(action: string, status: string, detail: string | null): string | null {
  if (action !== AI_SETTING_ACTION) return null;
  let d: Record<string, unknown> = {};
  try {
    const parsed: unknown = detail ? JSON.parse(detail) : {};
    if (parsed && typeof parsed === "object") d = parsed as Record<string, unknown>;
  } catch {
    return detail;
  }
  const provider = typeof d.provider === "string" ? providerName(d.provider) : null;
  switch (status) {
    case "provider_selected": {
      const model = typeof d.model === "string" ? d.model : null;
      const host = typeof d.host === "string" ? d.host : null;
      const parts = [provider ?? "Proveedor desconocido"];
      if (model) parts.push(`modelo ${model}`);
      parts.push(host ? `servidor ${host}` : "en este equipo");
      if (host) parts.push(d.consent === true ? "con la confirmación del destino de los datos" : "con una confirmación anterior del destino");
      return parts.join(" · ");
    }
    case "local_fallback_on":
      return "Si el proveedor externo falla, se usará una sola vez la IA local.";
    case "local_fallback_off":
      return "Si el proveedor externo falla, no se usará la IA local.";
    case "key_saved":
      return `${provider ?? "Proveedor"}: se guardó una clave en el almacén protegido del sistema. La clave no consta aquí.`;
    case "key_deleted":
      return `${provider ?? "Proveedor"}: se borró la clave del almacén protegido del sistema.`;
    default:
      return detail;
  }
}
