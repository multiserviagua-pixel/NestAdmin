// Comandos de la capa de aplicación. Dentro de Tauri se llama al proceso Rust; en un
// navegador normal (vista previa de desarrollo) se usa un servidor simulado en memoria con
// datos sintéticos, que nunca se incluye en el instalador.

import type { AuditEntry, ProviderChoice, ProviderInfo, Rule, SettingsView, Task } from "./types";

type Invoke = <T>(command: string, args?: Record<string, unknown>) => Promise<T>;
type Listen = (handler: () => void) => void;

const inTauri = "__TAURI_INTERNALS__" in window;

let invoke: Invoke;
let onChanged: Listen;

if (inTauri) {
  const core = await import("@tauri-apps/api/core");
  const event = await import("@tauri-apps/api/event");
  invoke = core.invoke;
  onChanged = (handler) => void event.listen("nestadmin://changed", handler);
} else if (import.meta.env.DEV) {
  // Solo en desarrollo: la compilación de producción elimina esta rama y el simulado.
  const mock = await import("./mock");
  invoke = mock.invoke;
  onChanged = mock.onChanged;
} else {
  invoke = () => Promise.reject("NestAdmin debe abrirse como aplicación de escritorio");
  onChanged = () => {};
}

export const isPreview = !inTauri;
export { onChanged };

export const api = {
  tasks: () => invoke<Task[]>("tasks"),
  importDocument: (name: string, textContent: string) => invoke<Task>("import_document", { name, textContent }),
  fetchMail: () => invoke<number>("fetch_mail"),
  approve: (id: number, version: number) => invoke<Task>("approve", { id, version }),
  editDraft: (id: number, body: string) => invoke<Task>("edit_draft", { id, body }),
  reject: (id: number) => invoke<Task>("reject", { id }),
  retry: (id: number) => invoke<Task>("retry", { id }),
  rules: () => invoke<Rule[]>("rules"),
  createRule: (id: string, actionKind: string, conditions: Record<string, string>) =>
    invoke<Rule>("create_rule", { id, actionKind, conditions }),
  updateRule: (id: string, conditions: Record<string, string>) => invoke<Rule>("update_rule", { id, conditions }),
  setRuleActive: (id: string, active: boolean) => invoke<Rule>("set_rule_active", { id, active }),
  auditLog: () => invoke<AuditEntry[]>("audit_log"),
  settings: () => invoke<SettingsView>("settings"),
  selectProvider: (choice: ProviderChoice) => invoke<ProviderInfo>("select_provider", { choice }),
  saveGroqKey: (key: string) => invoke<void>("save_groq_key", { key }),
  deleteGroqKey: () => invoke<void>("delete_groq_key"),
};
