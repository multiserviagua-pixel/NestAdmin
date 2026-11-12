// Comandos de la capa de aplicación. Dentro de Tauri se llama al proceso Rust; en un
// navegador normal (vista previa de desarrollo) se usa un servidor simulado en memoria con
// datos sintéticos, que nunca se incluye en el instalador.

import type {
  MailAccount,
  AuditEntry,
  FetchReport,
  AccountLearningPreview,
  FolderPreview,
  InitialReadOverview,
  InitialReadStatus,
  LearnChoice,
  LearningOverview,
  LearningStatus,
  MailProviderKind,
  MailView,
  LaunchView,
  ProviderChoice,
  PreferenceChange,
  ProviderInfo,
  ReadExpectation,
  BusinessProfile,
  ConsentView,
  MergePreview,
  ProfileConsentView,
  ProfilePreview,
  LearnedAcceptOutcome,
  LearnedBatch,
  LearnedDeactivateOutcome,
  ContinuousOverview,
  LearnedProfileProposal,
  ProfileSaved,
  ProfileView,
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
  SenderPreference,
  AssistantConsentView,
  AssistantConversation,
  ProposalCard,
  ProposalReceipt,
  AssistantPreview,
  AssistantProgress,
  AssistantSentCall,
  AssistantTurn,
  QuestionAnswer,
  SearchFilters,
  SearchPage,
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
let onAssistantProgress: ListenTo<AssistantProgress>;

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
  onAssistantProgress = listenTo("nestadmin://assistant-progress");
} else if (import.meta.env.DEV) {
  // Solo en desarrollo: la compilación de producción elimina esta rama y el simulado.
  const mock = await import("./mock");
  invoke = mock.invoke;
  onChanged = mock.onChanged;
  onSetup = mock.onSetup;
  onSetupDone = mock.onSetupDone;
  onStartup = mock.onStartup;
  onMailSignIn = mock.onMailSignIn;
  onAssistantProgress = mock.onAssistantProgress;
} else {
  invoke = () => Promise.reject("NestAdmin debe abrirse como aplicación de escritorio");
  onChanged = () => {};
  onSetup = () => {};
  onSetupDone = () => {};
  onStartup = () => {};
  onMailSignIn = () => {};
  onAssistantProgress = () => {};
}

export const isPreview = !inTauri;
export { onAssistantProgress, onChanged, onMailSignIn, onSetup, onSetupDone, onStartup };

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
  /** `amountsChecked`: la persona confirmó los importes con el documento; sin ello el fondo rechaza una factura con importe dudoso (D186). */
  approve: (id: number, version: number, amountsChecked = false) => invoke<Task>("approve", { id, version, amountsChecked }),
  confirmNotSent: (id: number) => invoke<Task>("confirm_not_sent", { id }),
  editDraft: (id: number, body: string) => invoke<Task>("edit_draft", { id, body }),
  reject: (id: number) => invoke<Task>("reject", { id }),
  /** Cierra un correo en revisión sin enviar nada ni contar como rechazo (D159). */
  archive: (id: number) => invoke<Task>("archive", { id }),
  /** Crea la tarea que solo redacta la respuesta a un informativo, un rebote o un correo sin respuesta necesaria (D159). */
  redraft: (id: number) => invoke<Task>("redraft", { id }),
  retry: (id: number) => invoke<Task>("retry", { id }),
  /** Chat sin IA (E1, D181): busca en el equipo por las palabras de la pregunta; no llama a la red. */
  searchQuestion: (text: string) => invoke<QuestionAnswer>("search_question", { text }),
  /** Búsqueda con filtros (D180); la usará el asistente con IA (E2), no la ventana todavía. */
  search: (query: string, filters: SearchFilters, offset: number) => invoke<SearchPage>("search", { query, filters, offset }),
  /** Vuelve a poner en el índice los documentos que no se pudieron leer; devuelve cuántos. */
  reindexFailed: () => invoke<number>("reindex_failed"),
  /** Un turno del asistente (E2, D182): la pregunta, la vista abierta y, opcionalmente, la conversación. */
  assistantTurn: (conversationId: number | null, text: string, view: string) =>
    invoke<AssistantTurn>("assistant_turn", { conversationId, text, view }),
  /** Pide cancelar el turno en curso; se comprueba entre pasos. */
  assistantCancel: () => invoke<void>("assistant_cancel"),
  /** Una conversación guardada en el equipo (la indicada o, sin indicar, la última). */
  assistantConversation: (id: number | null) => invoke<AssistantConversation | null>("assistant_conversation", { id }),
  /** Lo que se envió en las llamadas de un turno («Ver lo enviado»). */
  assistantSent: (turnId: number) => invoke<AssistantSentCall[]>("assistant_sent", { turnId }),
  /** Borra una conversación o, sin indicar, todas; devuelve cuántas. */
  assistantClear: (id: number | null) => invoke<number>("assistant_clear", { id }),
  /** Tarjetas de propuestas de una conversación con su estado actual (E3, D185); recalcula `stale` y `expired`. No ejecuta nada. */
  assistantProposals: (conversationId: number) => invoke<ProposalCard[]>("assistant_proposals", { conversationId }),
  /** Confirma una propuesta: SOLO lo llama el botón de la tarjeta. `amountsChecked` es la casilla de importes dudosos (D186) y `otherDocumentsChecked` la de datos de otros documentos en un borrador (D266); confirmar otra vez devuelve el mismo recibo. */
  proposalConfirm: (id: number, amountsChecked?: boolean, otherDocumentsChecked?: boolean) => invoke<ProposalReceipt>("proposal_confirm", { id, amountsChecked, otherDocumentsChecked }),
  /** La persona descarta la tarjeta: ya no se puede confirmar. */
  proposalDismiss: (id: number) => invoke<ProposalCard>("proposal_dismiss", { id }),
  /** Estado del permiso para enviar datos fuera del equipo (D183). */
  assistantConsent: () => invoke<AssistantConsentView>("assistant_consent"),
  /** Texto literal de un envío de ejemplo y su destino; no llama al proveedor. */
  assistantPreview: () => invoke<AssistantPreview>("assistant_preview"),
  /** Solo desde Configuración: el asistente no tiene vía para concederlo. */
  grantAssistantConsent: (provider: string, model: string, host: string) =>
    invoke<AssistantConsentView>("grant_assistant_consent", { provider, model, host }),
  withdrawAssistantConsent: () => invoke<AssistantConsentView>("withdraw_assistant_consent"),
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
  /** Preferencias por remitente (D161). Solo lectura. */
  senderPreferences: () => invoke<SenderPreference[]>("sender_preferences"),
  /** «Siempre a Informativos» (`informative`) o «Siempre a Pendientes» (`review`) para una dirección o un dominio. El backend normaliza la clave, rechaza los dominios de correo público y avisa si contradice una regla activa. Solo afecta a los correos nuevos. */
  setSenderPreference: (scope: SenderPreference["scope"], key: string, route: SenderPreference["route"]) =>
    invoke<PreferenceChange>("set_sender_preference", { scope, key, route }),
  removeSenderPreference: (scope: SenderPreference["scope"], key: string) => invoke<boolean>("remove_sender_preference", { scope, key }),
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
  /** Las sugerencias aprendidas pendientes con su evidencia y la huella de la tanda (D173). Solo lectura. */
  learnedSuggestionsBatch: () => invoke<LearnedBatch>("learned_suggestions_batch"),
  /** Acepta y activa de una vez las reglas aprendidas marcadas (subconjunto de la tanda de `batchFp`). Todo o nada: rechaza una
   * huella caducada y, si una regla falla, `failed` dice cuál y por qué. Repetirlo no duplica. Cada regla nace con un rodaje de 3 envíos (D174). */
  acceptAndActivateLearned: (batchFp: string, ids: string[]) => invoke<LearnedAcceptOutcome>("accept_and_activate_learned", { batchFp, ids }),
  /** «Desactivar las aprendidas»: desactiva en una transacción todas las reglas de origen aprendido. Idempotente (D174). */
  deactivateLearnedRules: () => invoke<LearnedDeactivateOutcome>("deactivate_learned_rules"),
  /** Resumen del aprendizaje continuo por buzón: solo recuentos y códigos (D178). Solo lectura. */
  continuousLearningOverview: () => invoke<ContinuousOverview>("continuous_learning_overview"),
  /** Activa o desactiva el aprendizaje continuo de las respuestas hechas fuera (D178); el cambio queda en el Historial. */
  setContinuousLearning: (enabled: boolean) => invoke<ContinuousOverview>("set_continuous_learning", { enabled }),
  /** Las facturas solo leídas de la serie de una sugerencia, con documento, fecha e importe. Solo lectura. */
  seenInvoices: (fingerprint: string) => invoke<SeenInvoice[]>("seen_invoices", { fingerprint }),
  /** Perfil del negocio (D163). Solo lectura. */
  businessProfile: () => invoke<ProfileView>("business_profile"),
  /** Nombre y firma (dirección y correo) desde los datos de la empresa, sin NIF ni IBAN. Solo lectura: nada se guarda ni se envía. */
  businessProfilePrefill: () => invoke<BusinessProfile>("business_profile_prefill"),
  /** Guarda el perfil. Bloquea IBAN, tarjetas y claves; avisa de teléfono y correo; retira el consentimiento si cambia lo que saldría. */
  saveBusinessProfile: (profile: BusinessProfile) => invoke<ProfileSaved>("save_business_profile", { profile }),
  clearBusinessProfile: () => invoke<ProfileSaved>("clear_business_profile"),
  /** Propuesta de perfil con el estilo aprendido (D171), con su antes y después. Solo lectura; `null` si no hay. */
  learnedProfileProposal: (account: string) => invoke<LearnedProfileProposal | null>("learned_profile_proposal", { account }),
  /** Acepta la propuesta que la persona vio (`fp`) por el guardado normal del perfil: mismos bloqueos y tope; retira el consentimiento del perfil si cambia lo que saldría. */
  acceptLearnedProfile: (account: string, fp: string) => invoke<ProfileSaved>("accept_learned_profile", { account, fp }),
  /** Descarta la propuesta: queda registrado sin contenido y no vuelve a ofrecerse. */
  dismissLearnedProfile: (account: string) => invoke<void>("dismiss_learned_profile", { account }),
  /** El bloque literal que saldría en la redacción y a qué destino. Solo lectura. */
  businessProfilePreview: () => invoke<ProfilePreview>("business_profile_preview"),
  /** Consiente a lo que la persona vio: proveedor, modelo, servidor y huella del bloque. Se rechaza si algo cambió. */
  grantProfileConsent: (seen: ProfilePreview) =>
    invoke<ProfileConsentView>("grant_profile_consent", { provider: seen.provider.provider, model: seen.provider.model, host: seen.host ?? "", fp: seen.fp ?? "" }),
  withdrawProfileConsent: () => invoke<ProfileConsentView>("withdraw_profile_consent"),
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
  /** Aprender a responder (E4, D167): estado de cada buzón. Solo lectura: no toca el buzón ni la IA. */
  learningOverview: () => invoke<LearningOverview>("learning_overview"),
  /** Vuelve a recoger y devuelve la vista previa vigente (texto literal, destino, huella). No llama a la IA. */
  learningPreview: (account: string) => invoke<AccountLearningPreview>("learning_preview", { account }),
  /** Consiente enviar lo que la persona vio, a ese proveedor, modelo y servidor. Se rechaza si algo cambió. */
  grantLearningConsent: (account: string, seen: AccountLearningPreview["preview"] & object) =>
    invoke<ConsentView>("grant_learning_consent", { account, expect: { provider: seen.provider.provider, model: seen.provider.model, host: seen.host ?? "", input_fp: seen.input_fp } }),
  /** Ejecuta el aprendizaje: «send» con lo que vio o «use_local» (nada sale del equipo). */
  runLearning: (account: string, choice: LearnChoice) => invoke<LearningStatus>("run_learning", { account, choice }),
  /** Tras un fallo, una lectura fallida o un corte: recoge de nuevo y deja una vista previa nueva. No llama a la IA. */
  retryLearning: (account: string) => invoke<LearningStatus>("retry_learning", { account }),
  /** Descarta una sugerencia: solo reaparece con el doble de evidencia nueva. */
  discardSuggestion: (fingerprint: string) => invoke<void>("discard_suggestion", { fingerprint }),
  suggestionThresholds: () => invoke<ThresholdSettings>("suggestion_thresholds"),
  /** `which`: `invoice` o `reply`. El backend rechaza lo que no esté entre el mínimo y el máximo. */
  setSuggestionThreshold: (which: "invoice" | "reply" | "learned_reply", value: number) => invoke<ThresholdSettings>("set_suggestion_threshold", { which, value }),
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
