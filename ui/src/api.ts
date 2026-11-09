// Comandos de la capa de aplicación. Dentro de Tauri se llama al proceso Rust; en un
// navegador normal (vista previa de desarrollo) se usa un servidor simulado en memoria con
// datos sintéticos, que nunca se incluye en el instalador.

import type {
  MailAccount,
  AuditEntry,
  FetchReport,
  FolderPreview,
  InitialReadOverview,
  InitialReadStatus,
  MailProviderKind,
  MailView,
  LaunchView,
  ProviderChoice,
  ProviderInfo,
  ReadExpectation,
  ConsentView,
  MergePreview,
  RefinePreview,
  RefinementView,
  SeenInvoice,
  PipelineOverview,
  PipelineRunView,
  Rule,
  RuleFinding,
  RuleSeed,
  RuleStats,
  RuleView,
  TaskTemplate,
  SettingsView,
  Simulation,
  Suggestions,
  ThresholdSettings,
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
  // Selector de archivos del sistema: conserva la ruta original como origen (D49).
  importDialog: () => invoke<{ imported: Task[]; errors: string[] }>("import_dialog"),
  openOriginal: (id: number) => invoke<void>("open_original", { id }),
  fetchMail: () => invoke<FetchReport>("fetch_mail"),
  mailAccounts: () => invoke<MailView>("mail_accounts"),
  setMailLimit: (limit: number) => invoke<void>("set_mail_limit", { limit }),
  // Sin `account` añade una cuenta; con ella, renueva su sesión. Termina al volver del navegador.
  // `expect` es lo que se mostró antes de conectar: si ya no es cierto, la lectura no arranca sola (D133).
  mailSignIn: (provider: MailProviderKind, label: string, account?: string, expect: ReadExpectation | null = null) =>
    invoke<MailAccount>("mail_sign_in", { provider, label, account: account ?? null, expect }),
  mailSignInCancel: () => invoke<void>("mail_sign_in_cancel"),
  renameMailAccount: (id: string, label: string) => invoke<MailAccount>("rename_mail_account", { id, label }),
  removeMailAccount: (id: string) => invoke<void>("remove_mail_account", { id }),
  addSimulatedMailAccount: () => invoke<MailAccount>("add_simulated_mail_account"),
  approve: (id: number, version: number) => invoke<Task>("approve", { id, version }),
  confirmNotSent: (id: number) => invoke<Task>("confirm_not_sent", { id }),
  editDraft: (id: number, body: string) => invoke<Task>("edit_draft", { id, body }),
  reject: (id: number) => invoke<Task>("reject", { id }),
  retry: (id: number) => invoke<Task>("retry", { id }),
  /** Plantillas de pipeline (D149): versión vigente de cada una, archivadas incluidas. */
  taskTemplates: () => invoke<TaskTemplate[]>("task_templates"),
  /** Una plantilla nueva nace inactiva; guardar una existente crea una versión nueva y conserva
   * su estado. Los pasos son del catálogo cerrado. Una plantilla nunca autoriza nada. */
  saveTaskTemplate: (
    id: string | null,
    name: string,
    conditions: { sender?: string; sender_domain?: string },
    steps: Array<{ kind: string }>,
  ) => invoke<TaskTemplate>("save_task_template", { id, name, conditions, steps }),
  setTaskTemplateActive: (id: string, active: boolean) =>
    invoke<TaskTemplate>("set_task_template_active", { id, active }),
  /** Archivar no borra: la plantilla queda inactiva y se puede restaurar (D151). */
  archiveTaskTemplate: (id: string, archived: boolean) => invoke<TaskTemplate>("archive_task_template", { id, archived }),
  /** Ejecuciones de pipeline con sus pasos y, por tarea retenida, qué la retiene (D148). */
  pipelineOverview: () => invoke<PipelineOverview>("pipeline_overview"),
  /** «Continuar sin ese paso» (D150): la respuesta irá siempre a revisión. */
  waivePipelineStep: (stepId: number, note: string) =>
    invoke<PipelineRunView>("waive_pipeline_step", { stepId, note }),
  rules: () => invoke<Rule[]>("rules"),
  ruleViews: () => invoke<RuleView[]>("rule_views"),
  /** `id` puede ser `null` (se genera del nombre). `acknowledgeAnySender` no tiene valor por
   * defecto: solo `true` crea una regla de envío sin remitente (D126). */
  createRule: (
    id: string | null,
    actionKind: string,
    conditions: Record<string, string>,
    options: { name: string | null; acknowledgeAnySender: boolean },
  ) =>
    invoke<RuleView>("create_rule", {
      id,
      name: options.name,
      actionKind,
      conditions,
      acknowledgeAnySender: options.acknowledgeAnySender,
    }),
  renameRule: (id: string, name: string | null) => invoke<RuleView>("rename_rule", { id, name }),
  setRuleAnySender: (id: string, acknowledge: boolean) =>
    invoke<RuleView>("set_rule_any_sender", { id, acknowledge }),
  archiveRule: (id: string) => invoke<RuleView>("archive_rule", { id }),
  unarchiveRule: (id: string) => invoke<RuleView>("unarchive_rule", { id }),
  ruleStats: () => invoke<RuleStats[]>("rule_stats"),
  /** Solo lectura: no escribe nada ni llama a la IA. `assumeAnySenderAck` es solo una hipótesis
   * (D126): ve qué haría una regla de envío sin remitente si tuviera la opción avanzada; nunca
   * guarda la marca. `excludeRuleId` quita esa regla de la comparación (probar una regla
   * guardada sin que coincida consigo misma). */
  simulateRule: (
    actionKind: string,
    conditions: Record<string, string>,
    limit: number | null,
    assumeAnySenderAck = false,
    excludeRuleId: string | null = null,
  ) => invoke<Simulation>("simulate_rule", { actionKind, conditions, limit, assumeAnySenderAck, excludeRuleId }),
  /** Contradicciones entre las reglas, de más a menos graves (D130). Solo lectura. */
  ruleFindings: () => invoke<RuleFinding[]>("rule_findings"),
  /** Sugerencias de reglas calculadas por código (D132). Solo lectura: no llama a la IA. */
  ruleSuggestions: () => invoke<Suggestions>("rule_suggestions"),
  /** Crea la regla de una sugerencia, siempre inactiva. Repetirlo no duplica. `shown` es la huella
   * de lo que se mostró: si los datos cambiaron, el backend rechaza y hay que revisar de nuevo. */
  acceptSuggestion: (fingerprint: string, shown: string, name: string | null, modelLimit: string | null = null, userLimit: string | null = null) =>
    invoke<RuleView>("accept_suggestion", { fingerprint, shown, name, modelLimit, userLimit }),
  /** Las facturas solo leídas de la serie de una sugerencia, con documento, fecha e importe. Solo lectura. */
  seenInvoices: (fingerprint: string) => invoke<SeenInvoice[]>("seen_invoices", { fingerprint }),
  /** Estado del afinado con IA (D134). Solo lectura: no llama al proveedor. */
  suggestionRefinement: () => invoke<RefinementView>("suggestion_refinement"),
  /** A qué proveedor y qué datos se enviarían al afinar. Solo lectura. */
  refinementPreview: () => invoke<RefinePreview>("refinement_preview"),
  /** Afina las sugerencias con la IA cuando la persona lo pide, tras ver la vista previa. Nunca crea ni activa reglas. */
  refineSuggestions: (seen: RefinePreview) =>
    invoke<RefinementView>("refine_suggestions", { expect: { provider: seen.provider.provider, model: seen.provider.model, host: seen.host ?? "", input_fp: seen.input_fp } }),
  /** Activa el afinado automático para el proveedor y modelo que la persona vio (versión final). Queda en el historial. */
  grantRefineConsent: (provider: string, model: string, host: string | null) => invoke<ConsentView>("grant_refine_consent", { provider, model, host: host ?? "" }),
  /** Retira el consentimiento del afinado automático. */
  withdrawRefineConsent: () => invoke<ConsentView>("withdraw_refine_consent"),
  /** Primera lectura (E5, D133): modo, volumen, a qué proveedor se envía y qué, y el estado de cada fuente. Solo lectura. */
  initialReadOverview: () => invoke<InitialReadOverview>("initial_read_overview"),
  /** Una carpeta antes de leerla: archivos admitidos y si ya se leyó. No lee ni envía nada. */
  initialReadFolderPreview: (path: string) => invoke<FolderPreview>("initial_read_folder_preview", { path }),
  /** Selector de carpetas del sistema; `null` si se cancela. */
  pickReadFolder: () => invoke<string | null>("pick_read_folder"),
  /** Lectura bajo demanda de una cuenta, con lo que la persona vio. Solo clasifica: no envía ni registra nada. */
  startInitialRead: (account: string, volume: number | null, expect: ReadExpectation | null) =>
    invoke<InitialReadStatus>("start_initial_read", { account, volume, expect }),
  startInitialReadFolder: (path: string, volume: number | null, expect: ReadExpectation | null) =>
    invoke<InitialReadStatus>("start_initial_read_folder", { path, volume, expect }),
  /** Detiene una lectura: no sale nada más y se puede continuar. */
  stopInitialRead: (source: string) => invoke<InitialReadStatus>("stop_initial_read", { source }),
  /** Continúa una lectura detenida o retenida por un cambio de destino, con el destino que se vio ahora. */
  continueInitialRead: (source: string, expect: ReadExpectation) => invoke<InitialReadStatus>("continue_initial_read", { source, expect }),
  /** Reintenta las tareas fallidas de una lectura (la misma tarea vuelve a la cola). */
  retryInitialRead: (source: string) => invoke<number>("retry_initial_read", { source }),
  /** Descarta una sugerencia: solo reaparece con el doble de evidencia nueva. */
  discardSuggestion: (fingerprint: string) => invoke<void>("discard_suggestion", { fingerprint }),
  suggestionThresholds: () => invoke<ThresholdSettings>("suggestion_thresholds"),
  /** `which`: `invoice` o `reply`. El backend rechaza lo que no esté entre el mínimo y el máximo. */
  setSuggestionThreshold: (which: "invoice" | "reply", value: number) => invoke<ThresholdSettings>("set_suggestion_threshold", { which, value }),
  /** Datos para precargar una regla desde una revisión (solo lectura). */
  ruleSeedForTask: (taskId: number) => invoke<RuleSeed>("rule_seed_for_task", { taskId }),
  /** Revisa una unificación sin cambiar nada: antes, opciones y, con la elección hecha, el plan. */
  previewMerge: (ids: string[], chosen: string | null) => invoke<MergePreview>("preview_merge", { ids, chosen }),
  /** Contradicciones que tendría una regla archivada al restaurarla (solo lectura). */
  previewRestore: (id: string) => invoke<RuleFinding[]>("preview_restore", { id }),
  /** Unifica en una sola transacción (D131): regla nueva inactiva, originales archivadas.
   * `planFingerprint` es la del plan que se revisó; si las reglas cambiaron, se rechaza. */
  mergeRules: (ids: string[], chosen: string | null, name: string | null, planFingerprint: string) =>
    invoke<RuleView>("merge_rules", { ids, chosen, name, planFingerprint }),
  updateRule: (id: string, conditions: Record<string, string>) => invoke<Rule>("update_rule", { id, conditions }),
  /** Guarda de una vez condiciones, nombre y validación avanzada (D126): sin guardados
   * parciales. `name`: `null` lo deja como está y `""` lo borra. */
  updateRuleWithAck: (id: string, name: string | null, conditions: Record<string, string>, acknowledgeAnySender: boolean) =>
    invoke<RuleView>("update_rule_with_ack", { id, name, conditions, acknowledgeAnySender }),
  setRuleActive: (id: string, active: boolean) => invoke<Rule>("set_rule_active", { id, active }),
  auditLog: () => invoke<AuditEntry[]>("audit_log"),
  settings: () => invoke<SettingsView>("settings"),
  selectProvider: (choice: ProviderChoice, consent: boolean) => invoke<ProviderInfo>("select_provider", { choice, consent }),
  // Una clave nueva solo se guarda si la lista responde (D122); nunca se devuelve.
  listModels: (choice: ProviderChoice, key: string | null) => invoke<string[]>("list_models", { choice, key }),
  testProvider: (choice: ProviderChoice) => invoke<void>("test_provider", { choice }),
  setLocalFallback: (enabled: boolean) => invoke<void>("set_local_fallback", { enabled }),
  providerKeySaved: (choice: ProviderChoice) => invoke<boolean>("provider_key_saved", { choice }),
  deleteProviderKey: (choice: ProviderChoice) => invoke<void>("delete_provider_key", { choice }),
  dismissExternalOffer: () => invoke<void>("dismiss_external_offer"),
  launchMode: () => invoke<LaunchView>("launch_mode"),
  setupCheck: () => invoke<SetupStatus>("setup_check"),
  setupPrepare: () => invoke<void>("setup_prepare"),
  setupCancel: () => invoke<void>("setup_cancel"),
  setupAbandon: () => invoke<void>("setup_abandon"),
};
