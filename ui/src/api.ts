// Comandos de la capa de aplicación. Dentro de Tauri se llama al proceso Rust; en un
// navegador normal (vista previa de desarrollo) se usa un servidor simulado en memoria con
// datos sintéticos, que nunca se incluye en el instalador.

import type {
  MailAccount,
  AuditEntry,
  FetchReport,
  MailProviderKind,
  MailView,
  LaunchView,
  ProviderChoice,
  ProviderInfo,
  Rule,
  SettingsView,
  SetupDone,
  SetupEvent,
  SetupStatus,
  Startup,
  Task,
} from "./types";

type InvokeOptions = { headers: Record<string, string> };
type Invoke = <T>(command: string, args?: Record<string, unknown> | Uint8Array, options?: InvokeOptions) => Promise<T>;
type Listen = (handler: () => void) => void;
type ListenTo<T> = (handler: (payload: T) => void) => void;

const inTauri = "__TAURI_INTERNALS__" in window;

let invoke: Invoke;
let onChanged: Listen;
let onSetup: ListenTo<SetupEvent>;
let onSetupDone: ListenTo<SetupDone>;
let onStartup: ListenTo<Startup>;
let onMailSignIn: ListenTo<string>;

if (inTauri) {
  const core = await import("@tauri-apps/api/core");
  const event = await import("@tauri-apps/api/event");
  const listenTo =
    <T>(name: string): ListenTo<T> =>
    (handler) =>
      void event.listen<T>(name, (e) => handler(e.payload));
  invoke = core.invoke;
  onChanged = (handler) => void event.listen("nestadmin://changed", handler);
  onSetup = listenTo("nestadmin://setup");
  onSetupDone = listenTo("nestadmin://setup-done");
  onStartup = listenTo("nestadmin://startup");
  onMailSignIn = listenTo("nestadmin://mail-sign-in");
} else if (import.meta.env.DEV) {
  // Solo en desarrollo: la compilación de producción elimina esta rama y el simulado.
  const mock = await import("./mock");
  invoke = mock.invoke;
  onChanged = mock.onChanged;
  onSetup = mock.onSetup;
  onSetupDone = mock.onSetupDone;
  onStartup = mock.onStartup;
  onMailSignIn = mock.onMailSignIn;
} else {
  invoke = () => Promise.reject("NestAdmin debe abrirse como aplicación de escritorio");
  onChanged = () => {};
  onSetup = () => {};
  onSetupDone = () => {};
  onStartup = () => {};
  onMailSignIn = () => {};
}

export const isPreview = !inTauri;
export { onChanged, onMailSignIn, onSetup, onSetupDone, onStartup };

export const api = {
  tasks: () => invoke<Task[]>("tasks"),
  // El archivo viaja como datos binarios; el nombre, en una cabecera.
  importFile: (name: string, bytes: Uint8Array) =>
    invoke<Task>("import_file", bytes, { headers: { "x-nestadmin-name": encodeURIComponent(name) } }),
  fetchMail: () => invoke<FetchReport>("fetch_mail"),
  mailAccounts: () => invoke<MailView>("mail_accounts"),
  setMailLimit: (limit: number) => invoke<void>("set_mail_limit", { limit }),
  // Sin `account` añade una cuenta; con ella, renueva su sesión. Termina al volver del navegador.
  mailSignIn: (provider: MailProviderKind, label: string, account?: string) =>
    invoke<MailAccount>("mail_sign_in", { provider, label, account: account ?? null }),
  mailSignInCancel: () => invoke<void>("mail_sign_in_cancel"),
  removeMailAccount: (id: string) => invoke<void>("remove_mail_account", { id }),
  addSimulatedMailAccount: () => invoke<MailAccount>("add_simulated_mail_account"),
  approve: (id: number, version: number) => invoke<Task>("approve", { id, version }),
  confirmNotSent: (id: number) => invoke<Task>("confirm_not_sent", { id }),
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
  launchMode: () => invoke<LaunchView>("launch_mode"),
  setupCheck: () => invoke<SetupStatus>("setup_check"),
  setupPrepare: () => invoke<void>("setup_prepare"),
  setupCancel: () => invoke<void>("setup_cancel"),
  setupAbandon: () => invoke<void>("setup_abandon"),
};
