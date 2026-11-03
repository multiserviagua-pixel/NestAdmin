// Comandos de la capa de aplicación. Dentro de Tauri se llama al proceso Rust; en un
// navegador normal (vista previa de desarrollo) se usa un servidor simulado en memoria con
// datos sintéticos, que nunca se incluye en el instalador.

import type {
  AuditEntry,
  Preparation,
  ProviderChoice,
  ProviderInfo,
  Rule,
  SettingsView,
  SetupComponent,
  SetupEvent,
  Task,
  Verification,
} from "./types";

type InvokeOptions = { headers: Record<string, string> };
type Invoke = <T>(command: string, args?: Record<string, unknown> | Uint8Array, options?: InvokeOptions) => Promise<T>;
type Listen = (handler: () => void) => void;

const inTauri = "__TAURI_INTERNALS__" in window;

let invoke: Invoke;
let onChanged: Listen;
let onSetup: (handler: (event: SetupEvent) => void) => void;

if (inTauri) {
  const core = await import("@tauri-apps/api/core");
  const event = await import("@tauri-apps/api/event");
  invoke = core.invoke;
  onChanged = (handler) => void event.listen("nestadmin://changed", handler);
  onSetup = (handler) => void event.listen<SetupEvent>("nestadmin://setup", (e) => handler(e.payload));
} else if (import.meta.env.DEV) {
  // Solo en desarrollo: la compilación de producción elimina esta rama y el simulado.
  const mock = await import("./mock");
  invoke = mock.invoke;
  onChanged = mock.onChanged;
  onSetup = mock.onSetup;
} else {
  invoke = () => Promise.reject("NestAdmin debe abrirse como aplicación de escritorio");
  onChanged = () => {};
  onSetup = () => {};
}

export const isPreview = !inTauri;
export { onChanged, onSetup };

export const api = {
  tasks: () => invoke<Task[]>("tasks"),
  // El archivo viaja como datos binarios; el nombre, en una cabecera.
  importFile: (name: string, bytes: Uint8Array) =>
    invoke<Task>("import_file", bytes, { headers: { "x-nestadmin-name": encodeURIComponent(name) } }),
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
  setupStatus: () => invoke<Preparation>("setup_status"),
  setupInstall: (component: SetupComponent) => invoke<void>("setup_install", { component }),
  setupVerify: () => invoke<Verification>("setup_verify"),
};
