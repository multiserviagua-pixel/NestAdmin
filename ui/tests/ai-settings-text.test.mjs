// D186b: los textos del Historial para los cambios de configuración de la IA. Ejemplos inventados.
import assert from "node:assert/strict";
import { test } from "node:test";
import { AI_SETTING_ACTION, AI_SETTING_ACTION_LABEL, aiSettingAuditText, aiSettingStatusLabel } from "../src/ai-settings-text.ts";

test("cada estado cerrado tiene su etiqueta y uno desconocido se deja tal cual", () => {
  assert.equal(AI_SETTING_ACTION_LABEL[AI_SETTING_ACTION], "Configuración de la IA");
  const labels = {
    provider_selected: "Proveedor elegido",
    local_fallback_on: "Respaldo con la IA local activado",
    local_fallback_off: "Respaldo con la IA local desactivado",
    key_saved: "Clave guardada",
    key_deleted: "Clave borrada",
  };
  for (const [status, label] of Object.entries(labels)) assert.equal(aiSettingStatusLabel(status), label);
  assert.equal(aiSettingStatusLabel("otro"), "otro");
});

test("elegir proveedor dice proveedor, modelo, servidor y la confirmación", () => {
  const external = JSON.stringify({ provider: "ollama_cloud", model: "modelo-x", host: "ollama.com", consent: true });
  assert.equal(
    aiSettingAuditText("ai_setting", "provider_selected", external),
    "Ollama cloud · modelo modelo-x · servidor ollama.com · con la confirmación del destino de los datos",
  );
  const earlier = JSON.stringify({ provider: "anthropic", model: "m", host: "api.anthropic.com", consent: false });
  assert.match(aiSettingAuditText("ai_setting", "provider_selected", earlier), /con una confirmación anterior/);
  const local = JSON.stringify({ provider: "ollama", model: "qwen", host: null, consent: false });
  const text = aiSettingAuditText("ai_setting", "provider_selected", local);
  assert.equal(text, "Ollama (local) · modelo qwen · en este equipo");
});

test("el respaldo y las claves se explican sin mostrar ninguna clave", () => {
  assert.match(aiSettingAuditText("ai_setting", "local_fallback_on", "{}"), /se usará una sola vez la IA local/);
  assert.match(aiSettingAuditText("ai_setting", "local_fallback_off", "{}"), /no se usará la IA local/);
  const saved = aiSettingAuditText("ai_setting", "key_saved", JSON.stringify({ provider: "anthropic" }));
  assert.match(saved, /Claude · Anthropic: se guardó una clave/);
  assert.match(saved, /La clave no consta aquí/);
  assert.match(aiSettingAuditText("ai_setting", "key_deleted", JSON.stringify({ provider: "ollama_cloud" })), /Ollama cloud: se borró la clave/);
});

test("no es suyo lo que no es ai_setting, y un detalle ilegible se muestra tal cual", () => {
  assert.equal(aiSettingAuditText("rule", "active", "alfa v1"), null);
  assert.equal(aiSettingAuditText("ai_setting", "key_saved", "no es json"), "no es json");
  assert.equal(aiSettingAuditText("ai_setting", "key_saved", null), "Proveedor: se guardó una clave en el almacén protegido del sistema. La clave no consta aquí.");
});
