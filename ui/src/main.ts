import "./style.css";
import { api, isPreview, onAssistantProgress, onChanged, onMailSignIn, onSetup, onSetupDone, onStartup } from "./api";
import type {
  AiWait,
  AuditEntry,
  DocumentOrigin,
  FetchReport,
  FieldSource,
  FolderPreview,
  InitialReadOverview,
  InitialReadStatus,
  MailOrigin,
  MailAccount,
  MailProviderKind,
  MailView,
  Memory,
  MergePreview,
  BusinessProfile,
  ProfileFinding,
  ProfilePreview,
  ProfileView,
  ProviderChoice,
  ProviderInfo,
  SeenInvoice,
  LimitProposal,
  RefinePreview,
  RefinementView,
  RuleFinding,
  RuleView,
  SettingsView,
  Simulation,
  Suggestion,
  Suggestions,
  SetupCheck,
  SetupDone,
  SetupErrorCode,
  SetupEvent,
  SetupStep,
  Startup,
  PipelineRunView,
  Task,
  TaskTemplate,
  ThresholdSettings,
  SenderPreference,
  LearnedAcceptOutcome,
  LearnedBatch,
  LearnedProfileProposal,
  LearningPreview,
  ContinuousOverview,
  LearningStatus,
} from "./types";
import {
  BATCH_TITLE,
  DEACTIVATE_BUTTON,
  GO_PROFILE,
  GO_RULES,
  INTERRUPTED_LINE,
  LEARN_INTRO,
  LEARN_TITLE,
  LOCAL_BUTTON,
  LOCAL_BUTTON_HELP,
  NEVER_SENT_HEADING,
  NONE_MARKED,
  PREVIEW_BUTTON,
  PREVIEW_HIDE_BUTTON,
  PREVIEW_LABEL,
  PREVIEW_NOTE,
  PROPOSAL_AFTER,
  PROPOSAL_APPLY,
  PROPOSAL_BEFORE,
  PROPOSAL_DISMISS,
  PROPOSAL_EMPTY,
  PROPOSAL_INTRO,
  PROPOSAL_NOTE,
  PROPOSAL_TITLE,
  REPREVIEW_BUTTON,
  RETRY_BUTTON,
  SEE_FIRST,
  WITHDRAWN_LINE,
  acceptButton as learnedAcceptButton,
  acceptedLine as learnedAcceptedLine,
  batchEmpty as learnedBatchEmpty,
  batchFailureLine,
  batchIntro as learnedBatchIntro,
  batchStaleLine,
  consentLine as learnConsentLine,
  countLines as learnCountLines,
  deactivateConfirm,
  deactivatedLine,
  doneLine as learnDoneLine,
  evidenceText,
  failureLine as learnFailureLine,
  heldLine as learnHeldLine,
  isExternalDestination,
  learnDestinationLine,
  learnPhase,
  learnPhaseLabel,
  learnPhaseLine,
  learnPhaseTone,
  learnedChips,
  nothingText,
  proposalBlockedLine,
  rodajeNotice,
  sendButtonLabel,
  CONTINUOUS_INTRO,
  CONTINUOUS_OFF,
  CONTINUOUS_TITLE,
  CONTINUOUS_TOGGLE,
  continuousLimitNote,
  continuousMailboxLines,
} from "./learn-text";
import {
  READ_HELP,
  recentMailWarning,
  SUGGESTIONS_GLOBAL_NOTE,
  WAITING_UNKNOWN,
  byProviderText,
  canStartRead,
  canStopRead,
  confirmReadLines,
  connectDetails,
  connectNotice,
  continueButtonLabel,
  folderLabel,
  heldText,
  readButtonLabel,
  readDestination,
  readEffectsLine,
  readStateTone,
  waitingReadsNotice,
  readFailureLine,
  readNotes,
  readProblems,
  readRemaining,
  readStartedWith,
  readStateLabel,
  readSummary,
  readVolumeText,
  volumeProblem,
} from "./read-text";
import {
  ANY_SENDER_CONFIRMATION,
  APPROVED_BY_PERSON,
  AUTHORIZED_BY_RULE,
  CATEGORY_HELP,
  CATEGORY_LABEL,
  REVIEW_ONLY_CATEGORIES,
  REVIEW_ONLY_TEXT,
  FIELD_HELP,
  MERGE_NOTICE,
  NO_EVALUATION,
  NO_FINDINGS,
  SIMULATION_DISCLAIMER,
  SUGGESTION_DISCARDED,
  SUGGESTION_HELP,
  SUGGESTION_INACTIVE_NOTICE,
  SUGGESTION_INTRO,
  THRESHOLD_LABEL,
  actsOnNothing,
  anyAckReady,
  anySenderConsequences,
  activationGroups,
  anySenderEditNotice,
  auditMatches,
  auditStatusLabel,
  categoryLabel,
  conditionsSummary,
  draftConditions,
  draftFromSeed,
  draftOf,
  draftProblems,
  explainEvaluation,
  explainFinding,
  isKnownAction,
  mergeOptionLabel,
  mergeOptionText,
  mergeSimulationText,
  mergedDone,
  normalizeSearch,
  originText,
  publicDomainWarning,
  restoreHeading,
  ruleProblem,
  ruleRisk,
  ruleSentence,
  ruleStatus,
  searchText,
  seedNotice,
  similarClarification,
  simulationSummary,
  sortFindings,
  statsText,
  suggestionAccepted,
  acceptedWithModelLimit,
  aiCommentText,
  applyRefinement,
  droppedText,
  groupQuestion,
  hiddenBlockTitle,
  hiddenReasonText,
  modelLimitButtonLabel,
  modelLimitText,
  refineButton,
  refineModeText,
  refinePreviewLines,
  refinementStatus,
  AI_COMMENT_LABEL,
  AI_COMMENT_NOTE,
  consentLines,
  consentStatusText,
  refineLoadError,
  HIDDEN_BLOCK_NOTE,
  suggestionLimitText,
  suggestionOutlierWarning,
  suggestionSeenWarning,
  normalizeAmount,
  formatAmount as formatEuro,
  suggestionSupplierNote,
  suggestionsTabCount,
  suggestionReappeared,
  suggestionSentence,
  suggestionWhy,
  suggestionsEmpty,
  suggestionsHidden,
  thresholdProblem,
  thresholdSaved,
  type CandidateOutcome,
  type RuleDraft,
} from "./rules-text";

import { actionBar, dataTable, emptyState, h, helpTip, icon, sectionHeader, statusChip, tabs, type Child, type IconName, type Tone } from "./components";
import {
  BOARD_COLUMNS,
  BOARD_PAGE,
  CANCEL_ALSO_CONFIRM,
  CANCEL_ALSO_DONE,
  BLOCKED_FILTER,
  actionLabel,
  boardColumnOf,
  attemptsText,
  downloadFailureText,
  CANCEL_ALSO_LABEL,
  CONTINUE_LABEL,
  DISCARD_HELD_CONFIRM,
  TASKS_VIEW_KEY,
  TEMPLATES_EMPTY_TEXT,
  TEMPLATE_AUTHORITY_NOTE,
  TEMPLATE_NAME_MAX,
  TEMPLATE_SCOPE_NOTE,
  ACTIVATE_WITHOUT_CONDITION_CONFIRM,
  archiveConfirm,
  breakSegments,
  continueConfirm,
  describeCondition,
  filterTasks,
  groupForBoard,
  holdsReply,
  indexPipeline,
  limitColumn,
  parseTasksView,
  pipelineChip,
  pipelineContext,
  pipelineReason,
  reasonWithPipeline,
  runStageText,
  scopeOf,
  showStateChip,
  stepText,
  taskReason,
  taskTitle,
  validateTemplateForm,
  waivedNote,
  waivableBlocker,
  type BoardColumn,
  type PipelineEntry,
  type SenderScope,
  type TaskReason,
  type TasksView,
  REPLIED_OUTSIDE_LABEL,
  isRepliedOutside,
} from "./tasks-text";
import {
  PROFILE_FIELDS,
  PROFILE_INTRO,
  PROFILE_NOTICE,
  PROFILE_TITLE,
  applyPrefill,
  canSave as canSaveProfile,
  consentLines as profileConsentLines,
  counterText,
  emptyProfile,
  savedNotice,
  statusOf as profileStatus,
  warningTexts,
} from "./profile-text";
import { ageText, chooseTab, emailFlags, formatAmount, invoiceFlags, isKnownKind, nextSelection, sortForReview, splitByTab, tabOf, totalIsProposed, type Flag, type QueueTab, type ReviewTab } from "./pendientes-text";
import {
  ARCHIVED_NOTICE,
  ARCHIVE_LABEL,
  ARCHIVE_TITLE,
  INFORMATIVE_EMPTY,
  INFORMATIVE_INTRO,
  INFORMATIVE_TAB,
  REDRAFT_CONFIRM_NO,
  REDRAFT_CONFIRM_YES,
  REDRAFT_LABEL,
  REDRAFT_NOTICE,
  REDRAFT_TITLE,
  informativeReason,
  informativeTasks,
  isBounce,
  redraftConfirmText,
  redraftFor,
  redraftNeedsConfirm,
  reviewEmailActions,
  isInformative,
  triageNotices,
} from "./triage-text";
import type { AssistantConsentView, AssistantPreview, AssistantProgress, AssistantSentCall, AssistantStoredTurn, AssistantTurn, QuestionAnswer, SearchHit } from "./types";
import {
  ASSISTANT_ACCEPT,
  ASSISTANT_ACTION_LABEL,
  progressText,
  ASSISTANT_CANCEL,
  ASSISTANT_CLEAR_ALL,
  ASSISTANT_CLEAR_ALL_CONFIRM,
  ASSISTANT_GRANT_OPEN,
  ASSISTANT_INTRO,
  ASSISTANT_LOADING,
  ASSISTANT_OPTION,
  ASSISTANT_PREVIEW_TITLE,
  ASSISTANT_SAMPLE_LABEL,
  ASSISTANT_TITLE,
  ASSISTANT_WITHDRAW,
  ASSISTANT_WITHOUT,
  CANCELLED_TEXT,
  CHAT_CANCEL,
  CHAT_CANCELLING,
  CLEAR_CONVERSATION,
  CLEAR_CONVERSATION_CONFIRM,
  CONSENT_REQUIRED_TEXT,
  LOADING_PREVIOUS,
  ASSISTANT_MAY_HEADING,
  ASSISTANT_NEVER_HEADING,
  NOT_CONFIGURED_TEXT,
  SENT_BACK,
  SENT_BUTTON,
  SENT_INTRO,
  SENT_LOADING,
  SENT_NONE,
  SENT_TITLE,
  SETTINGS_BUTTON,
  WORDS_BUTTON,
  WORDS_INTRO,
  assistantAuditText,
  assistantCallErrorText,
  assistantErrorText,
  assistantLoadError,
  canGrantConsent,
  canWithdrawConsent,
  clearedText,
  consentStateText,
  assistantDestinationText,
  droppedCitationsText,
  grantErrorText,
  limitLines,
  loadErrorText,
  localFallbackLine,
  navigateTarget,
  navigatedText,
  sentCallTitle,
  sentCallWhere,
  sentError,
  textParagraphs,
  whoAnswered,
  withheldText,
  ASSISTANT_LABEL,
  CHAT_BUSY,
  CHAT_BUTTON,
  CHAT_BUTTON_TITLE,
  CHAT_CLOSE,
  CHAT_EMPTY_HINT,
  CHAT_EMPTY_TITLE,
  CHAT_EXAMPLES,
  CHAT_EXAMPLES_LABEL,
  CHAT_INPUT_LABEL,
  CHAT_INPUT_PLACEHOLDER,
  CHAT_LABEL,
  CHAT_SEND,
  CHAT_TITLE,
  NOT_FOUND_TASK,
  OPEN_ORIGINAL_LABEL,
  OPEN_ORIGINAL_PREVIEW,
  REINDEX_BUSY,
  REINDEX_BUTTON,
  SEARCH_NOTICE,
  YOU_LABEL,
  answerText,
  cardLabel,
  canOpenOriginal,
  originalLabel,
  originalErrorText,
  cardView,
  countLines,
  reindexErrorText,
  reindexLine,
  searchErrorText,
  shownLine,
} from "./assistant-text";
import {
  PREFERENCE_CANCEL,
  PREFERENCE_INFORMATIVE_TITLE,
  PREFERENCE_REMOVE,
  PREFERENCE_REVIEW_TITLE,
  PREFERENCES_EMPTY,
  PREFERENCES_INTRO,
  PREFERENCES_TITLE,
  PUBLIC_DOMAIN_NOTE,
  ROUTE_LABEL,
  canOfferPreference,
  isPublicDomain,
  preferenceAuditText,
  preferenceChoices,
  preferenceConfirmText,
  preferenceFor,
  preferenceLine,
  preferenceSavedText,
  preferenceWarningText,
  scopeText,
  senderParts,
  type PreferenceChoice,
  type PreferenceRoute,
} from "./preferences-text";

const STATE_LABEL: Record<string, string> = {
  queued: "En cola",
  running: "Procesando",
  awaiting_review: "Pendiente de revisión",
  completed: "Completada",
  failed: "Fallida",
  cancelled: "Descartada",
};

const ISSUE_LABEL: Record<string, string> = {
  missing_tax: "Falta el IVA",
  missing_total: "Falta el total",
  total_mismatch: "Base + IVA no coincide con el total",
  possible_duplicate: "Posible duplicado",
  amount_to_review: "Importe dudoso: confírmelo con el documento",
};

const RECOVERY_LABEL: Record<string, string> = {
  no_decimals: "sin decimales",
  one_decimal: "con un solo decimal",
  ambiguous_separator: "el punto o la coma podría ser de miles",
};

const AMOUNT_LABEL: Record<string, string> = { subtotal: "Base imponible", tax: "IVA", total: "Total" };

/** Facturas con importes dudosos ya avisadas en esta sesión, para no repetir el aviso. */
const announced = new Set<number>();

const FIELD_LABEL: Record<string, string> = {
  supplier: "Proveedor",
  invoice_number: "Número",
  date: "Fecha",
  subtotal: "Base imponible",
  tax: "IVA",
  total: "Total",
  currency: "Moneda",
};

const ACTION_LABEL: Record<string, string> = {
  register_invoice: "Registrar factura",
  send_draft: "Enviar respuesta",
  rule: "Regla",
  rule_any_sender: "Opción avanzada de una regla",
  rule_archive: "Archivado de una regla",
  rule_rename: "Nombre de una regla",
  rule_merge: "Unificación de reglas",
  sender_preference: "Preferencia de remitente",
  mail: "Correo",
  ...ASSISTANT_ACTION_LABEL,
};

type View = "review" | "tasks" | "rules" | "history" | "mail" | "settings" | "setup";

const PROVIDER_LABEL: Record<MailProviderKind, string> = {
  simulator: "Simulador",
  google: "Google (Gmail)",
  microsoft: "Microsoft 365 / Outlook",
};

const ACCOUNT_STATUS: Record<string, [string, string]> = {
  connected: ["Conectada", "state-completed"],
  signed_out: ["Sin sesión", "state-failed"],
  not_configured: ["Proveedor no configurado", "state-failed"],
  credentials_unavailable: ["Credenciales no disponibles", "state-failed"],
};

// Errores del contrato de correo, en palabras del usuario.
const MAIL_ERROR: Record<string, string> = {
  unauthorized: "Sesión no válida",
  unavailable: "Proveedor o red no disponibles",
  not_found: "No encontrado",
  conflict: "Conflicto",
  invalid_input: "Datos no válidos",
};

const state = {
  view: "review" as View,
  // Pendientes (D137): pestaña, seleccionado de cada una y orden mostrado, para pasar al siguiente.
  reviewTab: null as ReviewTab | null,
  selected: { invoices: null, emails: null, informative: null } as Record<ReviewTab, number | null>,
  reviewOrder: { invoices: [], emails: [], informative: [] } as Record<ReviewTab, number[]>,
  // Informativos (D157): cuántos se enseñan y el correo cuya redacción espera confirmación.
  informativeShown: BOARD_PAGE,
  confirmRedraft: null as number | null,
  // Preferencias por remitente (D161) y el correo cuya preferencia espera confirmación.
  preferences: [] as SenderPreference[],
  confirmPreference: null as null | { task: number; route: PreferenceRoute },
  // Clave del control que debe recibir el foco tras el repintado (flechas de la lista).
  focusAfterRender: null as string | null,
  // Chat (E1, D181): solo en memoria; las conversaciones no se guardan. Vive en su propio contenedor,
  // fuera del repintado de la aplicación, para que este no le quite lo escrito, el foco ni el desplazamiento.
  chat: {
    open: false,
    input: "",
    busy: false,
    /** `assistant`: el asistente piensa (se puede cancelar); `words`: búsqueda por palabras. */
    busyKind: "assistant" as "assistant" | "words",
    cancelling: false,
    /** Paso en curso del asistente (código cerrado del evento de progreso). */
    progress: null as null | AssistantProgress,
    reindexing: false,
    turns: [] as ChatTurn[],
    cardNote: null as null | { key: string; text: string },
    // E2 (D188): conversación guardada en el equipo; se recupera la última al abrir por primera vez.
    conversationId: null as number | null,
    loaded: false,
    loading: false,
    // «Ver lo enviado»: vista del propio panel con «Volver»; guarda el desplazamiento al que volver.
    sent: null as null | { turnId: number; calls: AssistantSentCall[] | null; error: string | null; scroll: number },
  },
  // Opción del asistente en Configuración (D183).
  assistantConsent: null as AssistantConsentView | null,
  assistantPreview: null as AssistantPreview | null,
  assistantConfirming: false,
  assistantBusy: false,
  assistantError: null as string | null,
  // Filtro de la lista de pendientes por origen: "" (todo), "file" o el ID de una cuenta.
  originFilter: "",
  tasks: [] as Task[],
  rules: [] as RuleView[],
  // Contradicciones entre reglas (R23); `findingsError` si no se pudieron calcular.
  findings: [] as RuleFinding[],
  findingsError: null as string | null,
  // Unificación en curso (D131): hallazgo, elección, nombre y plan que se está revisando.
  merge: null as null | { key: string; ids: string[]; chosen: string | null; name: string; status: "loading" | "done" | "error"; preview?: MergePreview; error?: string },
  // «¿Es el mismo proveedor?» desplegado por hallazgo.
  similarOpen: {} as Record<string, boolean>,
  audit: [] as AuditEntry[],
  settings: null as SettingsView | null,
  // Proveedor desplegado en Configuración (null: el elegido) y asistente de primer uso.
  pick: null as ProviderChoice["provider"] | null,
  wizard: false,
  wizardStep: "choose" as "choose" | "cloud" | "other",
  wizardOther: "openai_compatible" as "openai_compatible" | "anthropic",
  mail: null as MailView | null,
  // Primera lectura (E5, D133): lo que dice el backend, su error si no se pudo leer, el volumen
  // escrito por fuente, la lectura que se está confirmando y las que están en marcha.
  read: null as InitialReadOverview | null,
  readError: null as string | null,
  // Perfil del negocio (E6, D163): lo guardado, su vista previa, lo que se está escribiendo, los
  // avisos del último guardado y el recuadro de consentimiento.
  profile: null as ProfileView | null,
  profilePreview: null as ProfilePreview | null,
  profileError: null as string | null,
  profileDraft: null as BusinessProfile | null,
  profileWarnings: [] as ProfileFinding[],
  profileShowBlock: false,
  profileConfirming: false,
  profileBusy: false,
  readVolume: {} as Record<string, string>,
  readConfirm: null as null | { source: string; kind: "account" | "folder"; target: string },
  readBusy: {} as Record<string, boolean>,
  readFolder: null as FolderPreview | null,
  // Aprender a responder (E7, D167-D175): estado de cada cuenta, la vista previa vista, lo que está en curso
  // y la tanda de reglas aprendidas con sus casillas (por defecto marcadas: solo se guardan las desmarcadas).
  learning: [] as LearningStatus[],
  learningError: null as string | null,
  // Aprendizaje continuo (D178): resumen por buzón y error de lectura; `continuousBusy` evita un doble clic.
  continuous: null as ContinuousOverview | null,
  continuousError: null as string | null,
  continuousBusy: false,
  learnPreview: {} as Record<string, LearningPreview>,
  learnShow: {} as Record<string, boolean>,
  learnBusy: {} as Record<string, "collect" | "run" | "retry">,
  learnedBatch: null as LearnedBatch | null,
  learnedBatchError: null as string | null,
  learnedOff: {} as Record<string, boolean>,
  learnedBusy: false,
  learnedMessage: null as null | { kind: "ok" | "warn" | "error"; text: string },
  learnedFailedId: null as string | null,
  learnedDeactivating: false,
  learnedProposals: {} as Record<string, LearnedProfileProposal>,
  learnedProposalBusy: {} as Record<string, boolean>,
  learnedProposalError: {} as Record<string, string>,
  // Sugerencias de facturas solo vistas: formulario del límite (obligatorio, lo escribe la persona) y
  // lista de las facturas leídas de la serie.
  limitForm: {} as Record<string, { open: boolean; text: string }>,
  seenList: {} as Record<string, { status: "loading" | "done" | "error"; items?: SeenInvoice[]; error?: string }>,
  // Dirección del inicio de sesión en curso, por si el navegador no se abrió solo.
  signInUrl: null as string | null,
  notice: null as { kind: "ok" | "error" | "warn"; text: string; task?: number } | null,
  /** Tareas cuyos importes dudosos ha confirmado la persona antes de aprobar. */
  amountsChecked: {} as Record<number, boolean>,
  // Borradores editados y aún no guardados, para no perderlos al refrescar.
  // Incluye los formularios de reglas en curso, con clave «rule:new» o «rule:edit:<id>».
  drafts: {} as Record<string, string>,
  // Tareas (D147): vista elegida, filtro por estado de la lista, tarjetas mostradas en cada columna
  // del tablero y tarjetas con el motivo completo desplegado.
  tasksView: readTasksView(),
  // Plantillas (D149-D151): la pestaña «Plantillas» vive solo en memoria (nunca es preferencia).
  tasksPane: "view" as "view" | "templates",
  // Ejecuciones y plantillas, leídas solo con la vista Tareas abierta; `null` si no se pudieron leer.
  pipeline: [] as PipelineRunView[],
  pipelineIndex: new Map<number, PipelineEntry>(),
  pipelineError: null as string | null,
  templates: [] as TaskTemplate[],
  // «Ver pasos» desplegado por tarea y «Archivadas» desplegado.
  stepsOpen: new Set<number>(),
  archivedOpen: false,
  // Editor de plantilla: `id` nulo es una nueva; el borrador se conserva entre repintados.
  tplEditor: null as null | { id: string | null; name: string; scope: SenderScope; domain: string; sender: string; active: boolean; wasActive: boolean; wasAny: boolean; errors: { name?: string; domain?: string; sender?: string }; problem: string | null; busy: boolean },
  taskFilter: "",
  boardShown: {} as Partial<Record<BoardColumn, number>>,
  reasonsOpen: new Set<number>(),
  // «Cerradas» empieza plegada aunque tenga tarjetas; solo en memoria, no se guarda.
  closedOpen: false,
  // Tarea localizada desde el chat: la fila o tarjeta se resalta y recibe el foco (D181).
  taskLocate: null as number | null,
  // Panel de reglas: pestaña, filtros de «Mis reglas» y formulario abierto.
  rulesTab: "mine" as "mine" | "categories" | "suggested",
  // Sugerencias calculadas por código (D132), su error si no se pudieron calcular, las que se
  // están resolviendo (contra el doble clic) y los umbrales escritos y aún sin guardar.
  suggestions: null as Suggestions | null,
  suggestionsError: null as string | null,
  suggestionBusy: {} as Record<string, boolean>,
  // Afinado con IA (D134): su estado, lo que se enviaría, si se está confirmando el envío o
  // afinando, y las sugerencias que la persona devolvió a la lista principal.
  refinement: null as RefinementView | null,
  refinePreview: null as RefinePreview | null,
  refineConfirming: false,
  refineConsentConfirming: false,
  refineError: null as string | null,
  refineBusy: false,
  restoredFromAi: new Set<string>(),
  thresholdText: {} as Partial<Record<"invoice" | "reply", string>>,
  ruleFilter: { text: "", action: "", status: "", category: "" },
  ruleEditor: null as null | { mode: "new" } | { mode: "edit"; id: string },
  // Panel dentro de una fila (activar con evidencia, probar, opción avanzada, archivar) y su
  // simulación; la confirmación escrita de la opción avanzada; menú «Más» abierto.
  rulePanel: null as null | { id: string; kind: "test" | "activate" | "advanced" | "archive" | "restore" },
  // Avisos al restaurar una regla archivada (R23).
  restoreFindings: [] as RuleFinding[],
  ruleSim: null as null | { key: string; status: "loading" | "done" | "error"; kind: string; sig?: string; sim?: Simulation; error?: string },
  // `skip`: activar sin la comprobación cuando la prueba falló (confirmación expresa).
  ruleAdv: { ack: false, text: "", skip: false },
  ruleMore: null as string | null,
  // «Detalles» abiertos por regla.
  ruleDetails: {} as Record<string, boolean>,
  advancedOpen: false,
  // «Por qué está aquí» desplegado, por tarea; filtro del Historial (regla, aprobadas o todo).
  whyOpen: {} as Record<number, boolean>,
  auditFilter: "",
  // Preparación (R08): modo instalador, última comprobación y la ejecución en curso.
  prepareOnly: false,
  check: null as SetupCheck | null,
  memory: null as Memory | null,
  startup: null as Startup | null,
  preparing: false,
  steps: {} as Partial<Record<SetupStep, SetupEvent>>,
  setupDone: null as SetupDone | null,
};

const root = document.getElementById("app")!;

/**
 * Pide confirmación y espera la respuesta. En Tauri 2 el complemento de diálogos sustituye
 * `window.confirm` por una función asíncrona que devuelve una promesa, y una promesa siempre
 * cuenta como verdadera en un `if`: sin `await` la acción se ejecutaba antes de que la persona
 * contestara. En el navegador `confirm` sigue siendo síncrono; `await` sirve en los dos casos.
 */
async function ask(message: string): Promise<boolean> {
  return await (window.confirm(message) as boolean | Promise<boolean>);
}

function notify(kind: "ok" | "error", text: unknown) {
  state.notice = { kind, text: String(text) };
  render();
}

// R07, D137: resolver una tarea trae la siguiente al mismo sitio de la barra de acciones. Un
// doble clic no debe aprobar ni descartar esa siguiente, que la persona aún no ha visto.
const resolving = new Set<number>();
const ACTION_LOCK_MS = 600;
let actionLockUntil = 0;

const discarding = new Set<number>();

/**
 * Descartar en Pendientes. Una factura que retiene la respuesta a un correo (pipeline) avisa antes:
 * la respuesta quedará bloqueada hasta que la persona decida. El resto, sin cambios (D151).
 */
function discardTask(task: Task) {
  if (resolving.has(task.id) || discarding.has(task.id)) return;
  discarding.add(task.id);
  void (async () => {
    try {
      let holds = false;
      if (task.kind === "invoice") {
        try {
          holds = holdsReply((await api.pipelineOverview()).runs, task.id);
        } catch {
          // Sin poder leer las ejecuciones no se bloquea el descarte: es la acción de siempre.
        }
      }
      if (holds && !await ask(DISCARD_HELD_CONFIRM)) return;
      resolveTask(task, () => api.reject(task.id), "Tarea descartada");
    } finally {
      discarding.delete(task.id);
    }
  })();
}

function resolveTask(task: Task, work: () => Promise<unknown>, success?: string) {
  if (resolving.has(task.id)) return;
  resolving.add(task.id);
  void act(work, success).finally(() => resolving.delete(task.id));
}

/** Ignora los clics en la barra de acciones justo después de cambiar de tarea sola. */
function guardActions(detail: HTMLElement): HTMLElement {
  detail.addEventListener(
    "click",
    (event) => {
      if (Date.now() < actionLockUntil && (event.target as Element).closest(".action-bar")) {
        event.stopPropagation();
        event.preventDefault();
      }
    },
    true,
  );
  return detail;
}

async function act(work: () => Promise<unknown>, success?: string) {
  try {
    await work();
    if (success) state.notice = { kind: "ok", text: success };
    await refresh();
  } catch (error) {
    notify("error", error);
  }
}

/// Aviso según cómo terminó una aprobación: hecha, ya hecha en un intento anterior, de vuelta
/// en revisión (hay que confirmar algo) o fallida.
function afterApproval(task: Task, done: string) {
  const r = task.result ?? {};
  if (task.state === "completed") {
    state.notice = r.recovered
      ? { kind: "ok", text: `Ya constaba realizada la versión ${r.recovered.version}; no se ha repetido` }
      : { kind: "ok", text: done };
  } else if (task.state === "failed") {
    state.notice = { kind: "error", text: task.error?.message ?? "la acción falló" };
  } else {
    state.notice = { kind: "warn", text: `No se ha realizado: ${r.review_reason ?? "requiere revisión"}` };
  }
}

/// Explica un intento anterior sin confirmar (D50) y, si el borrador ya no existe, ofrece
/// confirmar que no se envió para preparar una versión nueva.
function recoveryBox(task: Task, verb: string) {
  const r = task.result ?? {};
  // Tras confirmar que no salió, la versión mostrada ya es otra y el aviso no aplica.
  if (!r.recovery && r.attempt?.version !== (r.version ?? 1)) return null;
  const version = r.recovery?.version ?? r.attempt?.version;
  const error = r.recovery?.error;
  const missing = r.recovery?.needs_confirmation === true;
  return h(
    "div",
    { class: "recovery" },
    h(
      "p",
      {},
      h("strong", {}, `Ya se intentó ${verb} la versión ${version} y no consta confirmado`),
      error ? ` (último error: ${error})` : "",
      ". Se conserva el contenido aprobado; la IA no ha propuesto otro.",
    ),
    missing
      ? h(
          "p",
          {},
          "El borrador de ese intento ya no existe en el buzón y no consta enviado. Si lo borró usted y está seguro de que no salió, confírmelo para preparar uno nuevo con el texto de abajo; después tendrá que aprobarlo.",
        )
      : h(
          "p",
          {},
          `Aprobar lo vuelve a intentar con la misma clave: si ya se hizo, solo se confirma y nunca se repite. Si cambia el contenido, antes de ${verb} la versión nueva se comprueba que la anterior no se realizó.`,
        ),
    missing
      ? h(
          "div",
          { class: "buttons" },
          h(
            "button",
            { onclick: () => void act(() => api.confirmNotSent(task.id).then((t) => afterConfirm(t))) },
            "Confirmo que no se envió: preparar uno nuevo",
          ),
        )
      : null,
  );
}

function afterConfirm(task: Task) {
  state.notice =
    task.state === "completed"
      ? { kind: "ok", text: "El proveedor lo ve realizado: la tarea se ha completado sin repetirlo" }
      : { kind: "ok", text: `Preparada la versión ${task.result?.version}; revísela y apruébela` };
}

async function refresh(background = false) {
  try {
    let findingsError: string | null = null;
    let suggestionsError: string | null = null;
    let refineError: string | null = null;
    let readError: string | null = null;
    let profileError: string | null = null;
    let learningError: string | null = null;
    let learnedBatchError: string | null = null;
    let continuousError: string | null = null;
    // Solo con la vista Tareas abierta: los demás refrescos no piden las ejecuciones (D151).
    const pipelineLoad = state.view === "tasks" ? loadPipeline() : null;
    const [tasks, preferences, rules, findings, suggestions, refinement, refinePreview, audit, settings, mail, read, profile, profilePreview, learningOverview, learnedBatch, continuous, assistantConsent] = await Promise.all([
      api.tasks(),
      api.senderPreferences(),
      api.ruleViews(),
      api.ruleFindings().catch((error) => {
        findingsError = String(error);
        return [] as RuleFinding[];
      }),
      api.ruleSuggestions().catch((error) => {
        suggestionsError = String(error);
        return null;
      }),
      api.suggestionRefinement().catch((error) => {
        refineError = String(error);
        return null;
      }),
      api.refinementPreview().catch((error) => {
        refineError = String(error);
        return null;
      }),
      api.auditLog(),
      api.settings(),
      api.mailAccounts(),
      api.initialReadOverview().catch((error) => {
        readError = String(error);
        return null;
      }),
      api.businessProfile().catch((error) => {
        profileError = String(error);
        return null;
      }),
      api.businessProfilePreview().catch((error) => {
        profileError = String(error);
        return null;
      }),
      api.learningOverview().catch((error) => {
        learningError = String(error);
        return null;
      }),
      api.learnedSuggestionsBatch().catch((error) => {
        learnedBatchError = String(error);
        return null;
      }),
      api.continuousLearningOverview().catch((error) => {
        continuousError = String(error);
        return null;
      }),
      // E2 (D183): un fallo no tumba el resto; se conserva el último estado conocido.
      api.assistantConsent().catch(() => state.assistantConsent),
    ]);
    await pipelineLoad;
    Object.assign(state, { assistantConsent, tasks, preferences, rules, findings, findingsError, suggestions, suggestionsError, refinement, refinePreview, refineError, audit, settings, mail, read, readError, profile, profilePreview, profileError, learningError, learnedBatch, learnedBatchError, continuous, continuousError });
    await syncLearning(learningOverview?.accounts ?? null);
    // La oferta de la IA local necesita saber si está preparada: se comprueba al aparecer la espera.
    if (isProviderWait(settings?.ai_wait)) {
      if (!checkedForWait) {
        checkedForWait = true;
        void refreshCheck();
      }
    } else checkedForWait = false;
    announceDoubtfulAmounts();
  } catch (error) {
    state.notice = { kind: "error", text: String(error) };
  }
  render(background);
}

/**
 * Lee las ejecuciones de pipeline y las plantillas. Nunca lanza: un fallo se guarda en
 * `pipelineError` y la vista lo dice, sin tumbar la lista de tareas.
 */
async function loadPipeline() {
  try {
    const [overview, templates] = await Promise.all([api.pipelineOverview(), api.taskTemplates()]);
    state.pipeline = overview.runs;
    state.pipelineIndex = indexPipeline(overview.runs);
    state.templates = templates;
    state.pipelineError = null;
  } catch (error) {
    state.pipeline = [];
    state.pipelineIndex = new Map();
    state.pipelineError = String(error);
  }
}

/** Título de una tarea (ver `tasks-text.ts`): el id de mensaje nunca lo es. */
function title(task: Task): string {
  return taskTitle(task);
}

function safeJson(text: string): Record<string, any> {
  try {
    return JSON.parse(text) ?? {};
  } catch {
    return {};
  }
}

/** Avisa una vez de cada factura que llega con importes dudosos (D120). */
function announceDoubtfulAmounts() {
  for (const t of state.tasks) {
    const message = t.result?.amount_review;
    if (t.state !== "awaiting_review" || !message || announced.has(t.id)) continue;
    announced.add(t.id);
    // El detalle da la ruta y los importes; el aviso, una línea con el documento y cómo llegar.
    state.notice = { kind: "warn", text: `Importes dudosos en «${title(t)}»: revíselos antes de aprobar. Solo se registra con su aprobación.`, task: t.id };
  }
}

/** Abre una tarea en su pestaña de Pendientes, quitando el filtro si la ocultara. */
function openInReview(task: Task, tab: ReviewTab = tabOf(task.kind)) {
  state.view = "review";
  state.reviewTab = tab;
  state.selected[tab] = task.id;
  state.originFilter = "";
  state.notice = null;
  render();
}

function missing(text = "No consta") {
  return h("span", { class: "missing" }, text);
}

// ---- Vistas ----

function header() {
  const working = state.tasks.filter((t) => t.state === "queued" || t.state === "running").length;
  const active = state.settings?.active;
  const fileInput = h("input", {
    type: "file",
    accept: ".txt,.pdf,.jpg,.jpeg,.png,.tif,.tiff,.bmp,.webp,text/plain,application/pdf,image/*",
    multiple: true,
    hidden: true,
    onchange: (event) => {
      const input = event.target as HTMLInputElement;
      const files = [...(input.files ?? [])];
      input.value = "";
      void act(async () => {
        for (const file of files) await api.importFile(file.name, new Uint8Array(await file.arrayBuffer()));
      }, files.length === 1 ? `«${files[0]?.name}» añadido a la cola` : `${files.length} documentos añadidos a la cola`);
    },
  }) as HTMLInputElement;
  return h(
    "header",
    { class: "topbar" },
    h("div", { class: "brand" }, h("img", { src: "/icon.svg", alt: "" }), h("span", {}, "NestAdmin")),
    h(
      "div",
      { class: "status" },
      working ? h("span", { class: "pill working" }, `${working} en proceso`) : null,
      state.settings?.ai_wait_message
        ? h("button", { class: "pill provider waiting", onclick: goToSettings }, "IA en espera")
        : null,
      // El proveedor elegido sigue a la vista aunque esté en espera; el motivo va en el aviso superior.
      active ? h("span", { class: `pill provider ${active.provider}` }, providerLabel(active.provider, active.model)) : null,
    ),
    h(
      "div",
      { class: "actions" },
      fileInput,
      h(
        "button",
        {
          class: "primary",
          onclick: () => {
            // En la aplicación, el selector del sistema da la ruta original (D49); en la vista
            // previa del navegador solo hay nombre y contenido.
            if (isPreview) return fileInput.click();
            void act(async () => {
              const report = await api.importDialog();
              if (report.errors.length) throw report.errors.join(" · ");
              if (report.imported.length)
                state.notice = {
                  kind: "ok",
                  text:
                    report.imported.length === 1
                      ? `«${title(report.imported[0]!)}» añadido a la cola`
                      : `${report.imported.length} documentos añadidos a la cola`,
                };
            });
          },
        },
        "Importar documento",
      ),
      h(
        "button",
        {
          onclick: () =>
            void act(async () => {
              state.notice = fetchNotice(await api.fetchMail());
            }),
        },
        "Consultar correo",
      ),
      h(
        "button",
        {
          "data-focus-key": "chat:button",
          "aria-expanded": String(state.chat.open),
          "aria-controls": "chat-panel",
          title: CHAT_BUTTON_TITLE,
          onclick: () => (state.chat.open ? closeChat() : openChat()),
        },
        CHAT_BUTTON,
      ),
    ),
  );
}

function nav() {
  // Entrada: vista, etiqueta, icono, contador y si el contador avisa de un problema.
  type Entry = [View, string, IconName, number?, boolean?];
  const groups: Array<[string, Entry[]]> = [
    [
      "Trabajo",
      [
        ["review", "Pendientes", "inbox", state.tasks.filter((t) => t.state === "awaiting_review").length],
        ["tasks", "Tareas", "tasks"],
        ["history", "Historial", "history"],
      ],
    ],
    ["Automatización", [["rules", "Reglas", "rules"]]],
    [
      "Sistema",
      [
        ["mail", "Cuentas de correo", "mail", state.mail?.accounts.some(needsAttention) ? 1 : undefined, true],
        ["settings", "Configuración", "settings"],
        ["setup", "Preparación", "setup", (state.check && state.check.status !== "ready") || state.memory?.level === "insufficient" ? 1 : undefined, true],
      ],
    ],
  ];
  return h(
    "nav",
    { class: "sidebar", "aria-label": "Secciones" },
    ...groups.flatMap(([group, entries]) => [
      h("div", { class: "nav-group" }, group),
      ...entries.map(([view, label, iconName, count, attention]) =>
        h(
          "button",
          {
            class: state.view === view ? "nav active" : "nav",
            title: label,
            "aria-current": state.view === view ? "page" : undefined,
            "data-focus-key": `nav:${view}`,
            onclick: () => {
              state.view = view;
              state.notice = null;
              state.taskLocate = null;
              if (view === "settings") resetBlocks();
              render();
              if (view === "tasks") void loadPipeline().then(() => render(true));
            },
          },
          icon(iconName),
          h("span", { class: "nav-label" }, label),
          count ? h("span", { class: attention ? "count attention" : "count" }, count) : null,
        ),
      ),
    ]),
    isPreview ? h("p", { class: "preview-note" }, "Vista previa en navegador con datos sintéticos.") : null,
  );
}

const TAB_NAME: Record<ReviewTab, string> = { invoices: "Facturas", emails: "Correos", informative: INFORMATIVE_TAB };

/** Fecha corta de una fila. Sin dato, «Sin fecha». */
function shortDate(value: unknown): string {
  if (typeof value !== "string" && typeof value !== "number") return "Sin fecha";
  const date = typeof value === "number" ? new Date(value * 1000) : new Date(value);
  if (Number.isNaN(date.getTime())) return String(value);
  return date.toLocaleDateString("es-ES", { day: "2-digit", month: "short", ...(typeof value === "string" && /^\d{4}-\d{2}-\d{2}$/.test(value) ? { timeZone: "UTC" } : {}) });
}

function flagChips(flags: Flag[]) {
  return flags.map((f) => statusChip(f.label, f.tone));
}

/** Fila de una factura: quién e importe, número y fecha, de dónde viene y qué falla. */
function invoiceRow(task: Task, ageLabel: string): Child[] {
  const fields = task.result?.fields ?? {};
  const flags = invoiceFlags(task.result);
  const input = safeJson(task.input_ref);
  const origin: DocumentOrigin | undefined = task.result?.origin ?? input.origin;
  // R13, D48: remitente y cuenta de cada adjunto; sin origen registrado, se dice.
  const source =
    origin?.kind === "email"
      ? `Adjunto de ${origin.from ?? "remitente no consta"} · ${accountShort(mailOrigin(origin.account, `${origin.account.account}:`), origin.account.account)}`
      : origin
        ? `Archivo importado: ${title(task)}`
        : `${title(task)} (origen no registrado)`;
  const proposed = totalIsProposed(task.result) ? " (propuesta)" : "";
  const part = task.result?.part?.of > 1 ? ` · parte ${task.result?.part.index} de ${task.result?.part.of}` : "";
  return [
    h(
      "span",
      { class: "item-line" },
      fields.supplier ? h("span", { class: "item-title" }, fields.supplier) : h("span", { class: "item-title missing" }, "Proveedor no consta"),
      h("span", { class: "item-amount" }, formatAmount(fields.total, fields.currency) ? `${formatAmount(fields.total, fields.currency)}${proposed}` : h("span", { class: "missing" }, "Total no consta")),
    ),
    h("span", { class: "item-sub" }, `${fields.invoice_number ?? "Sin número"} · ${shortDate(fields.date)}${part} · pendiente ${ageLabel}`),
    h("span", { class: "item-sub" }, source),
    flags.length ? h("span", { class: "item-flags" }, ...flagChips(flags)) : null,
  ];
}

/** Fila de un correo (R13, D48): remitente, asunto y cuenta, categoría y qué falla. */
function emailRow(task: Task): Child[] {
  const input = safeJson(task.input_ref);
  const message = task.result?.message ?? input.message ?? {};
  const account = isKnownKind(task.kind) ? accountShort(taskMailOrigin(task), accountId(message.id ?? input.message_id)) : "No consta";
  const category = task.result?.category;
  return [
    h(
      "span",
      { class: "item-line" },
      message.from ? h("span", { class: "item-title" }, message.from) : h("span", { class: "item-title missing" }, "Remitente no consta"),
      h("span", { class: "item-side" }, shortDate(message.date)),
    ),
    h("span", { class: "item-sub" }, message.subject ?? "(sin asunto)"),
    h("span", { class: "item-sub" }, `Cuenta: ${account}`),
    h(
      "span",
      { class: "item-flags" },
      isKnownKind(task.kind) ? null : statusChip("Tipo no reconocido", "error"),
      category ? statusChip(CATEGORY_LABEL[category] ?? category) : null,
      ...flagChips(emailFlags(task.result)),
    ),
  ];
}

/** Fila de un informativo (D157): remitente, asunto, cuenta y motivo legible. */
function informativeRow(task: Task): Child[] {
  const input = safeJson(task.input_ref);
  const message = task.result?.message ?? input.message ?? {};
  return [
    h(
      "span",
      { class: "item-line" },
      message.from ? h("span", { class: "item-title" }, message.from) : h("span", { class: "item-title missing" }, "Remitente no consta"),
      h("span", { class: "item-side" }, shortDate(message.date)),
    ),
    h("span", { class: "item-sub" }, message.subject ?? "(sin asunto)"),
    h("span", { class: "item-sub" }, `Cuenta: ${accountShort(taskMailOrigin(task), accountId(message.id ?? input.message_id))}`),
    h("span", { class: "item-sub" }, informativeReason(task.result)),
  ];
}

function reviewView() {
  const all = sortForReview(state.tasks.filter((t) => t.state === "awaiting_review"));
  const split = splitByTab(all);
  // Informativos (D157): correos ya cerrados; no cuentan como pendientes.
  const informative = informativeTasks(state.tasks);
  const tab = chooseTab(state.reviewTab, { invoices: split.invoices.length, emails: split.emails.length });
  state.reviewTab = tab;
  const isInfo = tab === "informative";
  const inTab: Task[] = isInfo ? informative : split[tab as QueueTab];
  // Orígenes de esta pestaña, para el filtro: archivos importados y cada cuenta.
  const origins = new Map<string, string>();
  for (const t of isInfo ? [] : inTab) {
    const key = originKey(t);
    if (!origins.has(key)) origins.set(key, key === "file" ? "Archivos importados" : accountShort(taskMailOrigin(t), key));
  }
  if (state.originFilter && !origins.has(state.originFilter)) state.originFilter = "";
  const pending = inTab.filter((t) => isInfo || !state.originFilter || originKey(t) === state.originFilter);
  // En Informativos se enseñan por tandas; la selección cuenta con todos.
  const rows = isInfo ? pending.slice(0, state.informativeShown) : pending;
  const hiddenInfo = pending.length - rows.length;
  const ids = pending.map((t) => t.id);
  const previous = state.selected[tab];
  state.selected[tab] = nextSelection(state.reviewOrder[tab], ids, previous);
  if (previous !== null && state.selected[tab] !== previous) actionLockUntil = Date.now() + ACTION_LOCK_MS;
  state.reviewOrder[tab] = ids;
  const selected = pending.find((t) => t.id === state.selected[tab]);
  const now = Date.now();
  const otherTab: QueueTab = tab === "invoices" ? "emails" : "invoices";
  const moveTo = (id: number | undefined) => {
    if (id === undefined) return;
    state.selected[tab] = id;
    render();
  };
  const filter = h(
    "select",
    {
      "aria-label": "Filtrar por origen",
      "data-focus-key": "review-filter",
      onchange: (event) => {
        state.originFilter = (event.target as HTMLSelectElement).value;
        render();
      },
    },
    h("option", { value: "" }, `Todos los orígenes (${inTab.length})`),
    ...[...origins].map(([key, label]) =>
      h("option", { value: key, selected: key === state.originFilter }, `${label} (${inTab.filter((t) => originKey(t) === key).length})`),
    ),
  );
  return h(
    "section",
    { class: "split" },
    h(
      "div",
      { class: "list" },
      h(
        "div",
        { class: "list-head" },
        h("h2", {}, "Pendientes de revisión"),
        tabs({
          label: "Tipo de pendiente",
          panelId: "review-list",
          active: tab,
          items: (["invoices", "emails", "informative"] as ReviewTab[]).map((key) => ({ key, label: TAB_NAME[key], count: key === "informative" ? informative.length : split[key as QueueTab].length })),
          onSelect: (key) => {
            state.reviewTab = key;
            state.originFilter = "";
            state.confirmRedraft = null;
            render();
          },
        }),
      ),
      isInfo ? h("p", { class: "meta list-intro" }, INFORMATIVE_INTRO) : null,
      !isInfo && origins.size > 1 ? h("label", { class: "filter" }, "Origen", filter) : null,
      h(
        "div",
        { class: "list-items", id: "review-list", role: "tabpanel", "aria-label": TAB_NAME[tab], "data-scroll-key": `list:${tab}` },
        isInfo
          ? inTab.length === 0
            ? emptyState(INFORMATIVE_EMPTY)
            : null
          : all.length === 0
          ? emptyState("No hay nada pendiente", "Las tareas nuevas aparecerán aquí.")
          : inTab.length === 0
            ? emptyState(
                `No hay ${TAB_NAME[tab].toLowerCase()} pendientes`,
                null,
                h(
                  "button",
                  {
                    onclick: () => {
                      state.reviewTab = otherTab;
                      render();
                    },
                  },
                  `Ver ${TAB_NAME[otherTab].toLowerCase()} (${split[otherTab].length})`,
                ),
              )
            : null,
        ...rows.map((t, index) =>
          h(
            "button",
            {
              class: t.id === state.selected[tab] ? "item selected" : "item",
              "aria-current": t.id === state.selected[tab] ? "true" : undefined,
              "data-focus-key": `item:${t.id}`,
              onclick: () => moveTo(t.id),
              // Las flechas solo mueven la selección: aprobar o descartar nunca tienen atajo (D138).
              onkeydown: (event) => {
                const key = (event as KeyboardEvent).key;
                const target = key === "ArrowDown" ? index + 1 : key === "ArrowUp" ? index - 1 : key === "Home" ? 0 : key === "End" ? rows.length - 1 : -1;
                if (target < 0 || target >= rows.length) return;
                event.preventDefault();
                state.focusAfterRender = `item:${rows[target]!.id}`;
                moveTo(rows[target]!.id);
              },
            },
            ...(isInfo ? informativeRow(t) : tabOf(t.kind) === "invoices" ? invoiceRow(t, ageText(t.created_at, now)) : emailRow(t)),
          ),
        ),
        hiddenInfo > 0
          ? h(
              "div",
              { class: "list-more" },
              h(
                "button",
                {
                  type: "button",
                  "data-focus-key": "informative:more",
                  onclick: () => {
                    state.informativeShown += BOARD_PAGE;
                    state.focusAfterRender = hiddenInfo > BOARD_PAGE ? "informative:more" : `item:${pending[rows.length]?.id}`;
                    render();
                  },
                },
                `Ver más (${hiddenInfo} sin mostrar)`,
              ),
            )
          : null,
      ),
    ),
    selected ? guardActions(isInfo ? informativeDetail(selected) : tabOf(selected.kind) === "invoices" ? invoiceDetail(selected) : emailDetail(selected)) : h("div", { class: "detail" }),
  );
}

/** Lista de incidencias con su porqué siempre a la vista (lo crítico nunca va en una ayuda). */
function flagList(flags: Flag[]) {
  const shown = flags.filter((f) => !f.recovery);
  return shown.length
    ? h("ul", { class: "flag-list" }, ...shown.map((f) => h("li", {}, statusChip(f.label, f.tone), h("span", {}, f.why))))
    : null;
}

function openOriginal(task: Task) {
  return h(
    "button",
    {
      onclick: () =>
        isPreview
          ? ((state.notice = { kind: "ok", text: "Vista previa: en la aplicación se abriría la copia guardada con el visor del sistema." }), render())
          : void act(() => api.openOriginal(task.id)),
    },
    "Abrir original",
  );
}

const EXTERNAL_NAME: Record<Exclude<ProviderInfo["provider"], "ollama">, string> = {
  // Resultados antiguos con proveedor Groq (retirado, D100): solo se muestra su procedencia.
  groq: "Groq (retirado)",
  ollama_cloud: "Ollama cloud",
  openai_compatible: "Compatible con OpenAI",
  anthropic: "Claude · Anthropic",
};

function providerLabel(provider: ProviderInfo["provider"], model: string): string {
  if (provider === "ollama") return `Local · ${model}`;
  return `${EXTERNAL_NAME[provider]} (externo) · ${model}`;
}

/// Host de una URL base para mostrarlo en el aviso; si no es una URL, el texto tal cual.
function hostOf(baseUrl: string): string {
  try {
    return new URL(baseUrl.trim()).hostname || baseUrl.trim();
  } catch {
    return baseUrl.trim() || "el servicio configurado";
  }
}

/// Texto del respaldo local (D101, D102): qué proveedor externo falló y por qué.
function fallbackText(fallback: { from: string; reason: string }): string {
  const name = EXTERNAL_NAME[fallback.from as keyof typeof EXTERNAL_NAME] ?? fallback.from;
  return fallback.reason === "rate_limited"
    ? `Procesado con la IA local porque ${name} alcanzó su límite`
    : `Procesado con la IA local porque ${name} no respondió`;
}

function fallbackNotice(result: Record<string, any>) {
  return result.fallback ? h("div", { class: "notice warn", role: "status" }, h("span", {}, fallbackText(result.fallback))) : null;
}

function provenance(result: Record<string, any>) {
  return h(
    "p",
    { class: "meta" },
    `Propuesta de ${result.provider in EXTERNAL_NAME ? `${EXTERNAL_NAME[result.provider as keyof typeof EXTERNAL_NAME]} (externo)` : "Ollama local"} · ${result.model}`,
    result.latency_ms ? ` · ${(result.latency_ms / 1000).toFixed(1)} s` : "",
    ` · versión ${result.version}`,
  );
}

function invoiceDetail(task: Task) {
  const r = task.result ?? {};
  const fields: Record<string, string | null> = r.fields ?? {};
  const issues: string[] = r.issues ?? [];
  const unreadable: Array<{ field: string }> = r.unreadable ?? [];
  const recovered: Array<{ field: string; literal: string; value: string; reason: string }> = r.recovered ?? [];
  const doubtful = Boolean(r.amount_review);
  const checked = !doubtful || Boolean(state.amountsChecked[task.id]);
  const pages: Array<{ number: number; method: string }> = r.pages ?? [];
  const ocrPages = pages.filter((p) => p.method === "ocr").map((p) => p.number);
  const sources: Record<string, FieldSource> = r.sources ?? {};
  const origin: DocumentOrigin | null = r.origin ?? safeJson(task.input_ref).origin ?? null;
  const needsConfirmation = r.recovery?.needs_confirmation === true;
  const article = h(
    "article",
    {},
    h(
      "div",
      { class: "detail-head" },
      h(
        "div",
        {},
        h("h2", {}, fields.supplier ?? h("span", { class: "missing" }, "Proveedor no consta")),
        h("p", { class: "detail-sub" }, `${fields.invoice_number ?? "Sin número"} · ${title(task)}`),
      ),
      h(
        "div",
        { class: "detail-amount" },
        formatAmount(fields.total, fields.currency) ?? h("span", { class: "missing" }, "Total no consta"),
        totalIsProposed(r) ? h("span", { class: "proposed" }, " (propuesta)") : null,
      ),
    ),
    provenance(r),
    fallbackNotice(r),
    r.part && r.part.of > 1
      ? h(
          "p",
          { class: "meta warn" },
          `Factura ${r.part.index} de ${r.part.of} de este documento: la IA separó las facturas que contiene. Compruebe con el original que los datos de cada una no se mezclan.`,
        )
      : null,
    pages.length
      ? h(
          "p",
          { class: "meta" },
          `${pages.length} ${pages.length === 1 ? "página" : "páginas"}`,
          ocrPages.length
            ? ` · texto obtenido por OCR local en ${ocrPages.length === pages.length ? "todas" : `la(s) página(s) ${ocrPages.join(", ")}`}; compruebe los importes con el original`
            : "",
        )
      : null,
    doubtful
      ? h(
          "div",
          { class: "amount-review", role: "alert" },
          h("strong", {}, "Revise los importes antes de aprobar"),
          r.document_path ? h("p", { class: "meta" }, "Archivo: ", h("code", {}, r.document_path)) : null,
          recovered.length
            ? h(
                "ul",
                {},
                ...recovered.map((a) =>
                  h(
                    "li",
                    {},
                    `${AMOUNT_LABEL[a.field] ?? a.field}: el documento dice «${a.literal}»; se propone ${a.value} (${RECOVERY_LABEL[a.reason] ?? a.reason}).`,
                  ),
                ),
              )
            : null,
          h(
            "label",
            { class: "confirm" },
            h("input", {
              type: "checkbox",
              ...(checked ? { checked: "" } : {}),
              onchange: (e: Event) => {
                state.amountsChecked[task.id] = (e.target as HTMLInputElement).checked;
                render();
              },
            }),
            " He comprobado los importes con el documento",
          ),
        )
      : h("p", { class: "reason" }, `Motivo de la revisión: ${r.review_reason ?? "requiere aprobación"}`),
    whyHere(task),
    recoveryBox(task, "registrar"),
    flagList(invoiceFlags(r)) ?? (issues.length ? null : h("p", { class: "ok-line" }, "Sin incidencias en los importes.")),
    h("h3", {}, "Datos extraídos"),
    h(
      "table",
      { class: "fields" },
      h("thead", {}, h("tr", {}, h("th", { scope: "col" }, "Campo"), h("th", { scope: "col" }, "Valor"), h("th", { scope: "col" }, "Dónde aparece en el original"))),
      h(
        "tbody",
        {},
        ...Object.keys(FIELD_LABEL).map((key) =>
          h(
            "tr",
            {},
            h("th", { scope: "row" }, FIELD_LABEL[key]),
            h(
              "td",
              {},
              fields[key] ?? missing(),
              unreadable.some((u) => u.field === key) ? h("span", { class: "missing" }, " (ilegible)") : null,
              recovered.some((a) => a.field === key) ? h("span", { class: "proposed" }, " (propuesta: revísela)") : null,
            ),
            h("td", { class: "source" }, fieldSource(fields[key], sources[key], pages.length)),
          ),
        ),
      ),
    ),
    h("h3", {}, "Origen del documento"),
    documentOrigin(origin),
  );
  return h(
    "div",
    { class: "detail" },
    h("div", { class: "detail-scroll", "data-scroll-key": `detail:${task.id}` }, article),
    actionBar(
      !checked ? "Para aprobar, confirme antes los importes con el documento." : needsConfirmation ? "Para aprobar, confirme primero que el intento anterior no se envió." : null,
      h(
        "button",
        {
          class: "primary",
          disabled: !checked || needsConfirmation,
          onclick: () => resolveTask(task, async () => afterApproval(await api.approve(task.id, r.version ?? 1), "Factura registrada en el simulador")),
        },
        "Aprobar y registrar",
      ),
      openOriginal(task),
      createRuleButton(task),
      h("button", { class: "danger", onclick: () => discardTask(task) }, "Descartar"),
    ),
  );
}

/** Tras archivar o redactar, el foco pasa al correo siguiente de la lista (o a la pestaña si no queda ninguno). */
function focusAfterLeaving(task: Task, tab: ReviewTab) {
  const order = state.reviewOrder[tab];
  const next = nextSelection(
    order,
    order.filter((id) => id !== task.id),
    task.id,
  );
  state.focusAfterRender = next !== null ? `item:${next}` : `tab:Tipo de pendiente:${tab}`;
}

function archiveEmail(task: Task) {
  focusAfterLeaving(task, "emails");
  resolveTask(task, () => api.archive(task.id), ARCHIVED_NOTICE);
}

function requestRedraft(task: Task, tab: ReviewTab) {
  state.confirmRedraft = null;
  if (tab === "informative") state.focusAfterRender = `redraft:${task.id}`;
  else focusAfterLeaving(task, tab);
  resolveTask(task, () => api.redraft(task.id), REDRAFT_NOTICE);
}

function askRedraft(task: Task, tab: ReviewTab) {
  if (!redraftNeedsConfirm(task.result)) return requestRedraft(task, tab);
  state.confirmRedraft = task.id;
  state.focusAfterRender = `redraft-no:${task.id}`;
  render();
}

function redraftButton(task: Task, tab: ReviewTab) {
  return h("button", { title: REDRAFT_TITLE, "data-focus-key": `redraft:${task.id}`, onclick: () => askRedraft(task, tab) }, REDRAFT_LABEL);
}

/** Confirmación al redactar a una dirección que no admite respuestas. */
function redraftConfirm(task: Task, tab: ReviewTab) {
  if (state.confirmRedraft !== task.id) return null;
  return h(
    "div",
    { class: "notice warn", role: "group", "aria-label": "Confirmar redacción" },
    h("span", { role: "status" }, redraftConfirmText(task.result?.message?.from)),
    h("button", { onclick: () => requestRedraft(task, tab) }, REDRAFT_CONFIRM_YES),
    h(
      "button",
      {
        class: "primary",
        "data-focus-key": `redraft-no:${task.id}`,
        onclick: () => {
          state.confirmRedraft = null;
          state.focusAfterRender = `redraft:${task.id}`;
          render();
        },
      },
      REDRAFT_CONFIRM_NO,
    ),
  );
}

// ---- Preferencias por remitente (D161) ----

function askPreference(task: Task, route: PreferenceRoute) {
  state.confirmPreference = { task: task.id, route };
  state.focusAfterRender = `pref-choice:${task.id}:address`;
  render();
}

function cancelPreference(task: Task, route: PreferenceRoute) {
  state.confirmPreference = null;
  state.focusAfterRender = `pref:${task.id}:${route}`;
  render();
}

async function setPreference(task: Task, choice: PreferenceChoice, route: PreferenceRoute) {
  state.confirmPreference = null;
  state.focusAfterRender = `item:${task.id}`;
  try {
    const change = await api.setSenderPreference(choice.scope, choice.key, route);
    const warning = preferenceWarningText(change);
    state.notice = warning ? { kind: "warn", text: `${preferenceSavedText(change)} ${warning}` } : { kind: "ok", text: preferenceSavedText(change) };
    await refresh();
  } catch (error) {
    notify("error", error);
  }
}

/** Botón de la barra de acciones: «Siempre a Informativos» o «Siempre a Pendientes» para el remitente. */
function preferenceButton(task: Task, from: unknown, route: PreferenceRoute) {
  if (!canOfferPreference(state.preferences, from, route)) return null;
  return h(
    "button",
    { title: route === "informative" ? PREFERENCE_INFORMATIVE_TITLE : PREFERENCE_REVIEW_TITLE, "data-focus-key": `pref:${task.id}:${route}`, onclick: () => askPreference(task, route) },
    ROUTE_LABEL[route],
  );
}

/** La preferencia vigente de este remitente, con la forma de quitarla. */
function preferenceNote(task: Task, from: unknown) {
  const current = preferenceFor(state.preferences, from);
  if (!current) return null;
  return h(
    "div",
    { class: "notice info", role: "status" },
    h("span", {}, `Su preferencia: ${preferenceLine(current)}.`),
    h("button", { "data-focus-key": `pref-remove:${task.id}`, onclick: () => void act(() => api.removeSenderPreference(current.scope, current.key), "Preferencia quitada") }, PREFERENCE_REMOVE),
  );
}

/** Confirmación con el efecto explicado y la elección entre la dirección y el dominio. */
function preferenceConfirm(task: Task, from: unknown) {
  const pending = state.confirmPreference;
  if (!pending || pending.task !== task.id) return null;
  const parts = senderParts(from);
  return h(
    "div",
    { class: "notice warn stacked", role: "group", "aria-label": "Confirmar preferencia" },
    h("span", { role: "status" }, preferenceConfirmText(pending.route, from)),
    parts && isPublicDomain(parts.domain) ? h("span", { class: "meta" }, PUBLIC_DOMAIN_NOTE) : null,
    h(
      "div",
      { class: "buttons" },
      ...preferenceChoices(from).map((choice) =>
        h("button", { class: choice.scope === "address" ? "primary" : undefined, "data-focus-key": `pref-choice:${task.id}:${choice.scope}`, onclick: () => void setPreference(task, choice, pending.route) }, choice.label),
      ),
      h("button", { "data-focus-key": `pref-cancel:${task.id}`, onclick: () => cancelPreference(task, pending.route) }, PREFERENCE_CANCEL),
    ),
  );
}

/** Lista de preferencias en Cuentas de correo: ver y quitar. */
function preferencesSection() {
  const list = state.preferences;
  return h(
    "section",
    { class: "section" },
    sectionHeader(PREFERENCES_TITLE),
    h("p", { class: "intro" }, PREFERENCES_INTRO),
    list.length
      ? dataTable(
          [{ label: "Remitente" }, { label: "Preferencia" }, { label: "Fijada" }, { label: "" }],
          list.map((p) => [
            scopeText(p),
            ROUTE_LABEL[p.route],
            dateText(p.updated_at / 1000),
            h("button", { "data-focus-key": `pref-list-remove:${p.scope}:${p.key}`, "aria-label": `Quitar la preferencia de ${scopeText(p)}`, onclick: () => void act(() => api.removeSenderPreference(p.scope, p.key), "Preferencia quitada") }, PREFERENCE_REMOVE),
          ]),
        )
      : emptyState("Sin preferencias", PREFERENCES_EMPTY),
  );
}

/** Detalle de solo lectura de un informativo (D157): no tiene borrador ni nada que aprobar. */
function informativeDetail(task: Task) {
  const r = task.result ?? {};
  const message = r.message ?? safeJson(task.input_ref).message ?? {};
  const requested = redraftFor(state.tasks, task.id);
  const article = h(
    "article",
    {},
    h("div", { class: "detail-head" }, h("h2", {}, message.subject ?? h("span", { class: "missing" }, "(sin asunto)")), statusChip("Informativo")),
    h("p", { class: "reason" }, `Motivo: ${informativeReason(r)}`),
    h("p", { class: "meta" }, "No se envió a la IA ni se ha borrado."),
    preferenceNote(task, message.from),
    preferenceConfirm(task, message.from),
    h(
      "dl",
      { class: "origin" },
      h("dt", {}, "Cuenta"),
      h("dd", {}, accountLine(r.origin ?? safeJson(task.input_ref).origin, message.id)),
      h("dt", {}, "De"),
      h("dd", {}, message.from ?? missing()),
      h("dt", {}, "Para"),
      h("dd", {}, Array.isArray(message.to) && message.to.length ? message.to.join(", ") : missing()),
      h("dt", {}, "Recibido"),
      h("dd", {}, message.date ? dateText(message.date) : missing()),
    ),
    h("div", { class: "message" }, h("p", {}, message.body ?? missing())),
    redraftConfirm(task, "informative"),
  );
  return h(
    "div",
    { class: "detail" },
    h("div", { class: "detail-scroll", "data-scroll-key": `detail:${task.id}` }, article),
    actionBar(
      requested ? "Ya se pidió una respuesta a este correo: aparece en Pendientes." : null,
      requested ? h("button", { "data-focus-key": `redraft:${task.id}`, onclick: () => openInReview(requested) }, "Ver en Pendientes") : redraftButton(task, "informative"),
      preferenceButton(task, message.from, "review"),
    ),
  );
}

function emailDetail(task: Task) {
  const r = task.result ?? {};
  const acts = reviewEmailActions(r);
  const notices = triageNotices(r);
  const message = r.message ?? {};
  const draft = r.draft ?? {};
  const body = h("textarea", {
    rows: "10",
    "aria-label": "Borrador de respuesta",
    "data-focus-key": `draft:${task.id}`,
    oninput: () => (state.drafts[task.id] = body.value),
  }) as HTMLTextAreaElement;
  body.value = state.drafts[task.id] ?? draft.body ?? "";
  const recipients: string[] = draft.to ?? [];
  const needsConfirmation = r.recovery?.needs_confirmation === true;
  const article = h(
    "article",
    {},
    h(
      "div",
      { class: "detail-head" },
      h("h2", {}, message.subject ?? h("span", { class: "missing" }, "(sin asunto)")),
      r.category ? statusChip(CATEGORY_LABEL[r.category] ?? r.category) : null,
    ),
    provenance(r),
    isKnownKind(task.kind) ? null : h("div", { class: "notice error", role: "alert" }, h("span", {}, `Tipo de tarea no reconocido: ${task.kind}. Se muestra como correo; compruébelo antes de aprobar.`)),
    fallbackNotice(r),
    h("p", { class: "reason" }, `Motivo de la revisión: ${r.review_reason ?? "requiere aprobación"}`),
    whyHere(task),
    recoveryBox(task, "enviar"),
    notices.length
      ? h("div", { class: "triage-notices" }, ...notices.map((n) => h("div", { class: `notice ${n.tone}`, role: "status" }, h("span", {}, n.text))))
      : null,
    redraftConfirm(task, "emails"),
    preferenceNote(task, message.from),
    preferenceConfirm(task, message.from),
    h(
      "dl",
      { class: "origin" },
      h("dt", {}, "Cuenta"),
      h("dd", {}, isKnownKind(task.kind) ? accountLine(r.origin ?? safeJson(task.input_ref).origin, message.id) : missing()),
      h("dt", {}, "De"),
      h("dd", {}, message.from ?? missing()),
      h("dt", {}, "Para"),
      h("dd", {}, Array.isArray(message.to) && message.to.length ? message.to.join(", ") : missing()),
      h("dt", {}, "Recibido"),
      h("dd", {}, message.date ? dateText(message.date) : missing()),
    ),
    h("div", { class: "message" }, h("p", {}, message.body ?? missing())),
    acts.editor ? h("h3", {}, "Borrador de respuesta") : null,
    acts.editor ? h("p", { class: "meta" }, "Para: ", recipients.length ? recipients.join(", ") : h("span", { class: "missing" }, "sin destinatario; no se puede enviar")) : null,
    acts.editor ? body : null,
  );
  return h(
    "div",
    { class: "detail" },
    h("div", { class: "detail-scroll", "data-scroll-key": `detail:${task.id}` }, article),
    actionBar(
      !isKnownKind(task.kind)
        ? "Tipo de tarea no reconocido: no se puede enviar desde aquí."
        : !acts.approve
          ? acts.approveHint
          : recipients.length === 0
            ? "Sin destinatario: no se puede enviar."
            : needsConfirmation
              ? "Para aprobar, confirme primero que el intento anterior no se envió."
              : null,
      acts.approve
        ? h(
            "button",
            {
              class: "primary",
              // Con el borrador anterior desaparecido, primero hay que confirmar que no se envió.
              disabled: !isKnownKind(task.kind) || recipients.length === 0 || needsConfirmation,
              onclick: () =>
                resolveTask(task, async () => {
                  let version = r.version ?? 1;
                  // Si se ha editado, se guarda primero como versión nueva y se aprueba esa.
                  if (body.value !== (draft.body ?? "")) version = (await api.editDraft(task.id, body.value)).result?.version;
                  afterApproval(await api.approve(task.id, version), `Enviada desde ${accountName(message.id, r.origin)}; la entrega al destinatario no se comprueba`);
                }),
            },
            "Aprobar y enviar",
          )
        : null,
      acts.editor ? h("button", { onclick: () => void act(() => api.editDraft(task.id, body.value), "Borrador guardado como versión nueva") }, "Guardar borrador") : null,
      acts.redraft ? redraftButton(task, "emails") : null,
      acts.archive ? h("button", { title: ARCHIVE_TITLE, "data-focus-key": `archive:${task.id}`, onclick: () => archiveEmail(task) }, ARCHIVE_LABEL) : null,
      // Un rebote no se manda a Informativos (D161) y una redacción pedida a mano tampoco lo necesita.
      isBounce(r) || r.redraft_of != null ? null : preferenceButton(task, message.from, "informative"),
      acts.editor ? createRuleButton(task) : null,
      h("button", { class: "danger", onclick: () => resolveTask(task, () => api.reject(task.id), "Tarea descartada") }, "Descartar"),
    ),
  );
}

const TASK_TONE: Record<string, Tone> = {
  queued: "info",
  running: "info",
  awaiting_review: "warn",
  completed: "ok",
  failed: "error",
  cancelled: "neutral",
};

/**
 * La vista de Tareas elegida la última vez. Es solo una comodidad de pantalla del navegador
 * integrado (no son datos de NestAdmin): si el almacenamiento falla o trae otra cosa, se usa la lista.
 */
function readTasksView(): TasksView {
  try {
    return parseTasksView(localStorage.getItem(TASKS_VIEW_KEY));
  } catch {
    return "list";
  }
}

function setTasksView(view: TasksView) {
  state.tasksView = view;
  try {
    localStorage.setItem(TASKS_VIEW_KEY, view);
  } catch {
    // Sin almacenamiento la elección vale solo para esta sesión.
  }
  render();
}

/** Texto que puede cortarse tras `@`, `.` y `-` sin partir las palabras letra a letra. */
function breakable(text: string): Child[] {
  const parts = breakSegments(text);
  const out: Child[] = [];
  parts.forEach((part, i) => {
    if (i > 0 && !/\s/.test(part) && !/\s/.test(parts[i - 1]!)) out.push(document.createElement("wbr"));
    out.push(part);
  });
  return out;
}

const kindLabel = (kind: string) => (isKnownKind(kind) ? (kind === "invoice" ? "Factura" : "Correo") : "Tipo no reconocido");

function reasonSpan(reason: TaskReason): HTMLElement {
  return reason.tone === "error"
    ? h("span", { class: "error-text" }, reason.text)
    : reason.tone === "muted"
      ? h("span", { class: "meta" }, reason.text)
      : reason.tone === "warn"
        ? h("span", { class: "warn-text" }, reason.text)
      : reason.tone === "missing"
        ? missing(reason.text)
        : h("span", {}, reason.text);
}

/** Motivo corto de una tarea, igual en la fila y en la tarjeta (el aviso largo de espera va solo arriba). */
function reasonOf(t: Task): TaskReason | null {
  // El motivo del pipeline (Esperando, Bloqueada) tiene prioridad sobre la espera de la IA (D151).
  return reasonWithPipeline(taskReason(t, Boolean(state.settings?.ai_wait_message), (i) => ISSUE_LABEL[i] ?? i), entryOf(t));
}

/** El paso de pipeline de una tarea; `undefined` si no tiene ejecución (no cambia nada). */
const entryOf = (t: Task): PipelineEntry | undefined => state.pipelineIndex.get(t.id);

/** «Paso 2 de 2 · Plantilla», para las tareas que forman parte de una ejecución. */
function contextLine(t: Task, className: string): HTMLElement | null {
  const text = pipelineContext(entryOf(t));
  if (!text) return null;
  const open = state.stepsOpen.has(t.id);
  return h(
    "p",
    { class: className },
    text,
    " ",
    h(
      "button",
      { class: "steps-toggle", type: "button", "aria-label": actionLabel(open ? "Ocultar pasos" : "Ver pasos", title(t)), "aria-expanded": open ? "true" : "false", "aria-controls": `steps-${t.id}`, "data-focus-key": `task:steps:${t.id}`, onclick: () => toggleSteps(t) },
      open ? "Ocultar pasos" : "Ver pasos",
    ),
  );
}

async function continueWithout(t: Task, step: { id: number; label: string }, code: string) {
  if (!await ask(continueConfirm(step.label, code))) return;
  // El botón desaparece al omitir: el foco pasa a «Ver pasos» de la misma tarea.
  state.focusAfterRender = `task:steps:${t.id}`;
  // Si el paso ya estaba omitido, la respuesta es la misma y solo se refresca (D150).
  void act(() => api.waivePipelineStep(step.id, ""), `Se continúa sin “${step.label}”. La respuesta irá siempre a revisión.`);
}

async function cancelAlso(t: Task) {
  if (!await ask(CANCEL_ALSO_CONFIRM)) return;
  state.focusAfterRender = `task:steps:${t.id}`;
  resolveTask(t, () => api.reject(t.id), CANCEL_ALSO_DONE);
}

function toggleSteps(t: Task) {
  if (state.stepsOpen.has(t.id)) state.stepsOpen.delete(t.id);
  else state.stepsOpen.add(t.id);
  state.focusAfterRender = `task:steps:${t.id}`;
  render();
}

/** Ejecución de la tarea: pasos por etapas con su estado (D148, D150). Solo texto. */
function stepsPanel(t: Task): HTMLElement | null {
  const entry = entryOf(t);
  if (!entry) return null;
  const { run } = entry;
  const stages = [...new Set(run.steps.map((s) => s.stage))].sort((a, b) => a - b);
  const taskOf = (id: number | null) => (id === null ? undefined : state.tasks.find((x) => x.id === id));
  return h(
    "div",
    { class: "run-steps", id: `steps-${t.id}` },
    h("p", { class: "meta" }, `Plantilla “${run.template_name ?? run.template_id}”, versión ${run.template_version}`),
    h(
      "ol",
      { class: "stage-list", "aria-label": "Etapas de la ejecución" },
      ...stages.map((stage) => {
        const steps = run.steps.filter((s) => s.stage === stage);
        const text = runStageText(steps[0]!.kind);
        return h(
          "li",
          { class: "stage" },
          h("p", { class: "stage-title" }, `Etapa ${stages.indexOf(stage) + 1}: ${text.title}`),
          text.phases ? h("p", { class: "meta" }, text.phases) : null,
          h(
            "ul",
            { class: "step-tasks" },
            ...steps.map((step) => {
              const task = taskOf(step.task_id);
              const why = pipelineReason({ run, step });
              const blocker = run.steps.flatMap((x) => x.blocked?.by ?? []).find((b) => b.step_id === step.id);
              return h(
                "li",
                { class: "step-task" },
                h(
                  "span",
                  { class: "step-line" },
                  h("span", { class: "step-label" }, ...breakable(step.label)),
                  step.task_id !== null
                    ? h("span", { class: "meta" }, `tarea ${step.task_id}`)
                    : statusChip(blocker?.code === "attachment_rejected" ? "Adjunto no importable" : "Adjunto sin importar", "warn"),
                  step.task_state !== null ? stateChip(step.task_state, pipelineChip({ run, step })) : null,
                  step.waived ? statusChip("Omitido", "neutral") : null,
                  task && task.state === "awaiting_review"
                    ? h("button", { class: "link", type: "button", "data-focus-key": `run:${t.id}:review:${task.id}`, onclick: () => openInReview(task) }, "Revisar")
                    : null,
                ),
                waivedNote(step) ? h("p", { class: "meta" }, waivedNote(step)) : null,
                why ? h("p", { class: "meta" }, reasonSpan(why)) : null,
                step.task_id === null && step.error_code ? h("p", { class: "meta" }, downloadFailureText(step.error_code)) : null,
              );
            }),
          ),
        );
      }),
    ),
  );
}

/** Acciones de una tarea: las mismas en la fila y en la tarjeta. Aprobar y enviar solo existen en Pendientes (R07). */
function taskActions(t: Task): HTMLElement {
  const entry = entryOf(t);
  const blocked = entry?.step.blocked?.reason === "blocked";
  const waive = waivableBlocker(entry);
  return h(
    "div",
    { class: "row-actions" },
    waive
      ? h(
          "button",
          { type: "button", "aria-label": actionLabel(CONTINUE_LABEL, title(t)), "data-focus-key": `task:waive:${t.id}`, onclick: () => void continueWithout(t, waive.step, waive.code) },
          CONTINUE_LABEL,
        )
      : null,
    blocked ? h("button", { type: "button", "aria-label": actionLabel(CANCEL_ALSO_LABEL, title(t)), "data-focus-key": `task:cancel-also:${t.id}`, onclick: () => void cancelAlso(t) }, CANCEL_ALSO_LABEL) : null,
    t.state === "failed"
      ? h("button", { "data-focus-key": `task:retry:${t.id}`, onclick: () => void act(() => api.retry(t.id), "Tarea reencolada") }, "Reintentar")
      : null,
    t.state === "awaiting_review" ? h("button", { "data-focus-key": `task:review:${t.id}`, onclick: () => openInReview(t) }, "Revisar") : null,
  );
}

/** Chip de estado; una tarea retenida por su pipeline dice «Bloqueada» o «Esperando» aunque siga en cola en los datos. */
function stateChip(state: string, held: { text: string; tone: "warn" | "neutral" } | null, label?: string) {
  return held ? statusChip(held.text, held.tone) : statusChip(label ?? STATE_LABEL[state] ?? state, TASK_TONE[state] ?? "neutral");
}

// Una tarea cerrada porque ya estaba respondida fuera no es «Completada» ni un rechazo (D178).
const taskChip = (t: Task) => stateChip(t.state, pipelineChip(entryOf(t)), isRepliedOutside(t) ? REPLIED_OUTSIDE_LABEL : undefined);

// Más de esto no cabe en las dos líneas de la tarjeta: se ofrece desplegarlo.
const CARD_REASON_CHARS = 60;

function taskCard(t: Task): HTMLElement {
  const reason = reasonOf(t);
  const open = state.reasonsOpen.has(t.id);
  const long = reason !== null && reason.text.length > CARD_REASON_CHARS;
  const fallback = t.result?.fallback;
  const card = h(
    "li",
    { class: "task-card" },
    h("p", { class: "task-card-title" }, ...breakable(title(t))),
    h("p", { class: "task-card-meta" }, h("span", {}, `${kindLabel(t.kind)} ${t.id}`), showStateChip(t) || pipelineChip(entryOf(t)) ? taskChip(t) : null),
    contextLine(t, "task-card-context"),
    reason
      ? h(
          "p",
          // El texto entero está siempre en el DOM; el recorte a dos líneas es solo visual.
          { class: long && !open ? "task-card-reason clamp" : "task-card-reason", title: long ? reason.text : undefined },
          reasonSpan(reason),
          fallback ? h("span", { class: "meta" }, ` ${fallbackText(fallback)}`) : null,
        )
      : null,
    long
      ? h(
          "button",
          {
            class: "task-more",
            type: "button",
            "aria-expanded": open ? "true" : "false",
            "data-focus-key": `task:reason:${t.id}`,
            onclick: () => {
              if (open) state.reasonsOpen.delete(t.id);
              else state.reasonsOpen.add(t.id);
              render();
            },
          },
          open ? "Ver menos" : "Ver el motivo completo",
        )
      : null,
    taskActions(t),
    state.stepsOpen.has(t.id) ? stepsPanel(t) : null,
  );
  if (state.taskLocate === t.id) markLocated(card, t.id);
  return card;
}

/** Resalta la fila o tarjeta de la tarea localizada y la hace enfocable (D138: `data-focus-key`). */
function markLocated(el: Element | undefined, id: number) {
  if (!el) return;
  el.classList.add("is-located");
  el.setAttribute("tabindex", "-1");
  el.setAttribute("data-focus-key", `task:row:${id}`);
}

function boardColumn(key: BoardColumn, label: string, items: Task[]): HTMLElement {
  const headingId = `board-heading-${key}`;
  const heading = h("h3", { id: headingId }, label, " ", h("span", { class: "board-count" }, items.length));
  // Una columna vacía se pliega en un carril estrecho para que las cinco quepan, sin botón.
  if (!items.length) return h("section", { class: "board-col is-empty", "aria-labelledby": headingId }, heading);
  // «Cerradas» es lo menos urgente: plegada por defecto, con un botón para desplegarla o plegarla.
  const foldable = key === "closed";
  const toggle = () => {
    state.closedOpen = !state.closedOpen;
    state.focusAfterRender = "board:fold:closed";
    render();
  };
  if (foldable && !state.closedOpen)
    return h(
      "section",
      { class: "board-col is-empty is-folded", "aria-labelledby": headingId },
      h("button", { class: "fold-button", type: "button", "aria-expanded": "false", "aria-label": `Mostrar ${items.length} tareas cerradas`, "data-focus-key": "board:fold:closed", onclick: toggle }, "+"),
      heading,
    );
  const shown = state.boardShown[key] ?? BOARD_PAGE;
  const { visible, hidden } = limitColumn(items, shown);
  return h(
    "section",
    { class: "board-col", "aria-labelledby": headingId },
    foldable
      ? h(
          "div",
          { class: "board-head" },
          heading,
          h("button", { class: "fold-button", type: "button", "aria-expanded": "true", "aria-label": "Plegar las tareas cerradas", "data-focus-key": "board:fold:closed", onclick: toggle }, "\u2212"),
        )
      : heading,
    h(
      "ul",
      { class: "board-cards", tabindex: "0", "aria-labelledby": headingId, "data-scroll-key": `tasks:col:${key}`, "data-focus-key": `board:col:${key}` },
      ...visible.map(taskCard),
      hidden > 0
        ? h(
            "li",
            { class: "board-more" },
            h(
              "button",
              {
                type: "button",
                "data-focus-key": `board:more:${key}`,
                onclick: () => {
                  state.boardShown[key] = shown + BOARD_PAGE;
                  state.focusAfterRender = hidden > BOARD_PAGE ? `board:more:${key}` : `board:col:${key}`;
                  render();
                },
              },
              `Ver más (${hidden} sin mostrar)`,
            ),
          )
        : null,
    ),
  );
}

function tasksBoard(): HTMLElement {
  const groups = groupForBoard(state.tasks);
  return h("div", { class: "board", role: "group", "aria-label": "Tareas por estado", tabindex: "0" }, ...BOARD_COLUMNS.map((c) => boardColumn(c.key, c.label, groups[c.key])));
}

function tasksList(): HTMLElement {
  const filter = state.taskFilter;
  const rows = filterTasks(state.tasks, filter, state.pipelineIndex);
  const select = h(
    "select",
    { "data-focus-key": "tasks:filter", onchange: () => ((state.taskFilter = select.value), render()) },
    h("option", { value: "" }, "Todos los estados"),
    ...Object.entries(STATE_LABEL).map(([value, label]) => h("option", { value }, label)),
    h("option", { value: BLOCKED_FILTER }, "Bloqueadas"),
  ) as HTMLSelectElement;
  select.value = filter;
  const table = dataTable(
    // Cuatro columnas: el tipo, el número y los intentos van bajo el elemento, para que el motivo quepa entero.
    [{ label: "Elemento" }, { label: "Estado" }, { label: "Motivo o detalle" }, { label: "Acciones" }],
    rows.map((t) => {
      const reason = reasonOf(t);
      return [
        h(
          "div",
          { class: "cell-title" },
          h("span", { class: "cell-name" }, ...breakable(title(t))),
          h("p", { class: "meta cell-meta" }, `${kindLabel(t.kind)} ${t.id} \u00b7 ${attemptsText(t.attempts)}`),
          contextLine(t, "meta cell-context"),
        ),
        taskChip(t),
        h("div", { class: "detail-cell" }, reason ? reasonSpan(reason) : null, t.result?.fallback ? h("span", { class: "meta" }, ` ${fallbackText(t.result.fallback)}`) : null),
        taskActions(t),
      ];
    }),
  );
  table.classList.add("tasks-grid");
  // «Ver pasos»: una fila más bajo la tarea, a todo el ancho de la tabla.
  const bodyRows = [...table.querySelectorAll("tbody > tr")];
  rows.forEach((t, i) => {
    if (state.taskLocate === t.id) markLocated(bodyRows[i], t.id);
    const panel = state.stepsOpen.has(t.id) ? stepsPanel(t) : null;
    if (panel) bodyRows[i]?.after(h("tr", { class: "run-row" }, h("td", { colspan: "4" }, panel)));
  });
  return h(
    "div",
    {},
    h(
      "div",
      { class: "filters", role: "search" },
      h("div", { class: "field" }, h("div", { class: "field-head" }, h("label", {}, "Mostrar", select))),
      filter ? h("div", { class: "field field-action" }, h("button", { "data-focus-key": "tasks:clear", onclick: () => ((state.taskFilter = ""), render()) }, "Quitar filtro")) : null,
    ),
    rows.length ? table : emptyState("Ninguna tarea está en ese estado", "Pruebe con otro estado o quite el filtro."),
  );
}

// ---- Plantillas de pipeline (D149-D151): lista, editor y estado vacío ----

function openTemplateEditor(t: TaskTemplate | null) {
  const scope = t ? scopeOf(t.conditions) : "any";
  state.tplEditor = {
    id: t?.id ?? null,
    name: t?.name ?? "",
    scope,
    domain: t?.conditions.sender_domain ?? "",
    sender: t?.conditions.sender ?? "",
    active: t?.active ?? false,
    wasActive: t?.active ?? false,
    wasAny: scope === "any",
    errors: {},
    problem: null,
    busy: false,
  };
  state.focusAfterRender = "tpl:editor:name";
  render();
}

function closeTemplateEditor(focus: string) {
  state.tplEditor = null;
  state.focusAfterRender = focus;
  render();
}

async function saveTemplate() {
  const ed = state.tplEditor;
  if (!ed || ed.busy) return;
  const { errors, conditions } = validateTemplateForm(ed);
  ed.errors = errors;
  ed.problem = null;
  if (errors.name || errors.domain || errors.sender) {
    state.focusAfterRender = errors.name ? "tpl:editor:name" : errors.domain ? "tpl:editor:domain" : "tpl:editor:sender";
    render();
    return;
  }
  // Activar sin condición, o ampliar a «cualquier remitente» una ya activa, abarca todos los correos con adjunto.
  if (ed.active && ed.scope === "any" && (!ed.wasActive || !ed.wasAny) && !await ask(ACTIVATE_WITHOUT_CONDITION_CONFIRM)) return;
  ed.busy = true;
  render();
  let saved = false;
  const name = ed.name.trim();
  try {
    const template = await api.saveTaskTemplate(ed.id, name, conditions, [{ kind: "attachment_invoices" }, { kind: "reply" }]);
    saved = true;
    ed.id = template.id;
    if (template.active !== ed.active) await api.setTaskTemplateActive(template.id, ed.active);
    state.tplEditor = null;
    state.notice = { kind: "ok", text: ed.active ? `Plantilla “${name}” guardada y activa.` : `Plantilla “${name}” guardada.` };
    state.focusAfterRender = `tpl:${template.id}:edit`;
    await refresh();
  } catch (error) {
    ed.busy = false;
    ed.problem = saved ? `La plantilla se guardó, pero no se pudo cambiar su estado: ${String(error)}` : String(error);
    state.focusAfterRender = "tpl:editor:problem";
    render();
  }
}

async function toggleTemplate(t: TaskTemplate) {
  if (!t.active && scopeOf(t.conditions) === "any" && !await ask(ACTIVATE_WITHOUT_CONDITION_CONFIRM)) return;
  void act(
    () => api.setTaskTemplateActive(t.id, !t.active),
    t.active ? `Plantilla “${t.name}” desactivada.` : `Plantilla “${t.name}” activa. ${TEMPLATE_SCOPE_NOTE}`,
  );
}

async function archiveTemplate(t: TaskTemplate) {
  if (!await ask(archiveConfirm(t.name))) return;
  void act(() => api.archiveTaskTemplate(t.id, true), `Plantilla “${t.name}” archivada.`);
}

function templateRow(t: TaskTemplate): HTMLElement {
  const label = (verb: string) => `${verb} la plantilla ${t.name}`;
  return h(
    "li",
    { class: "tpl-item" },
    h(
      "div",
      { class: "tpl-main" },
      h("p", { class: "tpl-name" }, t.name, " ", t.archived ? statusChip("Archivada", "neutral") : statusChip(t.active ? "Activa" : "Inactiva", t.active ? "ok" : "neutral")),
      h("p", { class: "meta" }, describeCondition(t.conditions)),
    ),
    h(
      "div",
      { class: "row-actions" },
      ...(t.archived
        ? [
            h(
              "button",
              { type: "button", "aria-label": label("Restaurar"), "data-focus-key": `tpl:${t.id}:restore`, onclick: () => void act(() => api.archiveTaskTemplate(t.id, false), `Plantilla “${t.name}” restaurada, inactiva.`) },
              "Restaurar",
            ),
          ]
        : [
            h("button", { type: "button", "aria-label": label("Editar"), "data-focus-key": `tpl:${t.id}:edit`, onclick: () => openTemplateEditor(t) }, "Editar"),
            h("button", { type: "button", "aria-label": label(t.active ? "Desactivar" : "Activar"), "data-focus-key": `tpl:${t.id}:toggle`, onclick: () => void toggleTemplate(t) }, t.active ? "Desactivar" : "Activar"),
            h("button", { type: "button", "aria-label": label("Archivar"), "data-focus-key": `tpl:${t.id}:archive`, onclick: () => void archiveTemplate(t) }, "Archivar"),
          ]),
    ),
  );
}

function templateEditor(): HTMLElement {
  const ed = state.tplEditor!;
  const radio = (scope: SenderScope, label: string) => {
    const input = h("input", { type: "radio", name: "tpl-scope", value: scope, "data-focus-key": `tpl:editor:scope:${scope}`, disabled: ed.busy }) as HTMLInputElement;
    input.checked = ed.scope === scope;
    input.addEventListener("change", () => {
      ed.scope = scope;
      ed.errors = {};
      state.focusAfterRender = `tpl:editor:scope:${scope}`;
      render();
    });
    return h("label", { class: "choice" }, input, ` ${label}`);
  };
  const text = (key: "name" | "domain" | "sender", label: string, placeholder: string, extra: Record<string, string> = {}) => {
    const error = ed.errors[key];
    const input = h("input", {
      id: `tpl-${key}`,
      name: key,
      "data-focus-key": `tpl:editor:${key}`,
      placeholder,
      autocomplete: "off",
      spellcheck: "false",
      disabled: ed.busy,
      "aria-invalid": error ? "true" : undefined,
      "aria-describedby": error ? `tpl-${key}-error` : undefined,
      ...extra,
    }) as HTMLInputElement;
    input.value = ed[key];
    input.addEventListener("input", () => {
      ed[key] = input.value;
    });
    return h(
      "div",
      { class: "field" },
      h("div", { class: "field-head" }, h("label", { for: `tpl-${key}` }, label)),
      input,
      error ? h("p", { class: "rule-problem", id: `tpl-${key}-error` }, error) : null,
    );
  };
  const active = h("input", { type: "checkbox", id: "tpl-active", "data-focus-key": "tpl:editor:active", disabled: ed.busy }) as HTMLInputElement;
  active.checked = ed.active;
  active.addEventListener("change", () => {
    ed.active = active.checked;
  });
  const form = h(
    "form",
    {
      class: "tpl-form",
      novalidate: true,
      onsubmit: (event) => {
        event.preventDefault();
        void saveTemplate();
      },
    },
    text("name", "Nombre de la plantilla", "p. ej. Facturas de Alfa", { maxlength: String(TEMPLATE_NAME_MAX) }),
    h(
      "fieldset",
      { class: "from-group" },
      h("legend", {}, "¿Para qué correos?"),
      radio("any", "Cualquier remitente"),
      radio("domain", "Este dominio"),
      ed.scope === "domain" ? text("domain", "Dominio del remitente", "p. ej. cliente.test") : null,
      radio("sender", "Esta dirección"),
      ed.scope === "sender" ? text("sender", "Dirección del remitente", "p. ej. compras@cliente.test") : null,
      h("p", { class: "hint" }, "Solo se aplica a correos con al menos un adjunto admitido. Si hay varias plantillas activas, gana la más concreta: dirección, luego dominio, luego cualquiera."),
    ),
    h("label", { class: "choice" }, active, " Activa"),
    h(
      "div",
      { class: "tpl-steps" },
      h("h4", {}, "Pasos"),
      h(
        "ol",
        { class: "tpl-step-list", "aria-label": "Pasos de la plantilla (solo lectura)" },
        h("li", {}, `1. ${stepText("attachment_invoices").title}`),
        h("li", {}, `2. ${stepText("reply").title}`),
      ),
      h("p", { class: "meta" }, TEMPLATE_AUTHORITY_NOTE),
      h("p", { class: "meta" }, TEMPLATE_SCOPE_NOTE),
    ),
    ed.problem ? h("p", { class: "rule-problem", role: "alert", tabindex: "-1", "data-focus-key": "tpl:editor:problem" }, ed.problem) : null,
    h(
      "div",
      { class: "buttons" },
      h("button", { class: "primary", type: "submit", disabled: ed.busy, "data-focus-key": "tpl:editor:save" }, ed.busy ? "Guardando…" : "Guardar plantilla"),
      h("button", { type: "button", disabled: ed.busy, "data-focus-key": "tpl:editor:cancel", onclick: () => closeTemplateEditor(ed.id ? `tpl:${ed.id}:edit` : "tpl:new") }, "Cancelar"),
    ),
  );
  return h("section", { class: "tpl-editor", "aria-labelledby": "tpl-editor-title" }, h("h3", { id: "tpl-editor-title" }, ed.id ? "Editar plantilla" : "Nueva plantilla"), form);
}

function templatesPane(): HTMLElement {
  if (state.tplEditor) return templateEditor();
  if (state.pipelineError)
    return h(
      "div",
      { class: "empty-state" },
      h("p", { class: "empty-title" }, "No se pudieron leer las plantillas"),
      h("p", { class: "empty-hint" }, state.pipelineError),
      h("button", { type: "button", "data-focus-key": "tpl:retry", onclick: () => void loadPipeline().then(() => render()) }, "Reintentar"),
    );
  const live = state.templates.filter((t) => !t.archived);
  const archived = state.templates.filter((t) => t.archived);
  const create = h("button", { class: "primary", type: "button", "data-focus-key": "tpl:new", onclick: () => openTemplateEditor(null) }, "Crear plantilla");
  if (!state.templates.length) return h("div", { class: "empty-state" }, h("p", { class: "empty-hint" }, TEMPLATES_EMPTY_TEXT), create);
  return h(
    "div",
    { class: "tpl-pane" },
    h("div", { class: "tpl-toolbar" }, create),
    live.length ? h("ul", { class: "tpl-list" }, ...live.map(templateRow)) : h("p", { class: "meta" }, "Todas las plantillas están archivadas."),
    archived.length
      ? h(
          "details",
          {
            class: "tpl-archived",
            open: state.archivedOpen,
            ontoggle: (event) => {
              state.archivedOpen = (event.target as HTMLDetailsElement).open;
            },
          },
          h("summary", { "data-focus-key": "tpl:archived" }, `Archivadas (${archived.length})`),
          h("ul", { class: "tpl-list" }, ...archived.map(templateRow)),
        )
      : null,
  );
}

function tasksView() {
  const view = state.tasksView;
  const templates = state.tasksPane === "templates";
  const board = !templates && view === "board" && state.tasks.length > 0;
  const shown: TasksView | "templates" = templates ? "templates" : view;
  return h(
    "section",
    { class: board ? "page tasks-page is-board" : "page tasks-page" },
    h("h2", {}, "Tareas"),
    tabs({
      label: "Vista de tareas",
      items: [
        { key: "list" as TasksView | "templates", label: "Lista" },
        { key: "board" as TasksView | "templates", label: "Tablero" },
        { key: "templates" as TasksView | "templates", label: "Plantillas" },
      ],
      active: shown,
      panelId: "tasks-panel",
      onSelect: (key) => {
        // «Plantillas» nunca se guarda como preferencia: solo lista y tablero (D147).
        if (key === "templates") {
          state.tasksPane = "templates";
          render();
        } else {
          state.tasksPane = "view";
          setTasksView(key);
        }
      },
    }),
    h(
      "div",
      { role: "tabpanel", id: "tasks-panel", class: "tabpanel", "aria-label": templates ? "Plantillas" : view === "board" ? "Tablero" : "Lista" },
      !templates && state.pipelineError
        ? h("p", { class: "meta warn", role: "status" }, `No se pudo leer el estado de las plantillas (${state.pipelineError}). Las tareas se muestran sin su indicador de pipeline.`)
        : null,
      templates
        ? templatesPane()
        : state.tasks.length === 0
          ? emptyState("Todavía no hay tareas", "Importe un documento o consulte el correo para crear la primera.")
          : view === "board"
            ? tasksBoard()
            : tasksList(),
    ),
  );
}

let helpSeq = 0;

/** Etiqueta con su ayuda al lado: la ayuda va fuera de `<label>` para no activar el campo. */
function field(labelText: string, control: HTMLElement, helpText: string, tag?: string): HTMLElement {
  const controlId = control.id || `field-${++helpSeq}`;
  control.id = controlId;
  return h(
    "div",
    { class: "field" },
    h(
      "div",
      { class: "field-head" },
      h("label", { for: controlId }, labelText),
      tag ? h("span", { class: "tag" }, tag) : null,
      helpTip(helpText, `Ayuda sobre ${labelText.toLowerCase()}`),
    ),
    control,
  );
}

// ---- Panel de reglas (R21, R22) ----

const newDraftKey = "rule:new";
const editDraftKey = (id: string) => `rule:edit:${id}`;

const noRuleFilter = () => ({ text: "", action: "", status: "", category: "" });

function setRulesTab(tab: "mine" | "categories" | "suggested") {
  state.rulesTab = tab;
  render();
}

/** Lleva a «Mis reglas» filtradas por una categoría. */
function showRulesOf(category: string) {
  state.ruleFilter = { ...noRuleFilter(), category };
  state.rulesTab = "mine";
  render();
}

/** Lleva a «Mis reglas» con esa regla buscada (también si está archivada). */
function showRule(id: string) {
  const rule = state.rules.find((r) => r.id === id);
  state.ruleFilter = { ...noRuleFilter(), text: id, status: rule?.archived_at ? "archived" : "" };
  state.view = "rules";
  state.rulesTab = "mine";
  state.notice = null;
  render();
}

/** Abre el Historial filtrado: por una regla (su identificador) o por quien aprobó. */
function showHistoryOf(filter: string) {
  state.auditFilter = filter;
  state.view = "history";
  state.notice = null;
  render();
}

function ruleName(id: string): string {
  return state.rules.find((r) => r.id === id)?.name ?? id;
}

function openRuleEditor(editor: NonNullable<typeof state.ruleEditor>, draft?: RuleDraft) {
  state.ruleEditor = editor;
  state.rulePanel = null;
  state.ruleSim = null;
  if (draft) state.drafts[editor.mode === "new" ? newDraftKey : editDraftKey(editor.id)] = JSON.stringify(draft);
  state.rulesTab = "mine";
  render();
}

function closeRuleEditor() {
  const editor = state.ruleEditor;
  if (editor) delete state.drafts[editor.mode === "new" ? newDraftKey : editDraftKey(editor.id)];
  state.ruleEditor = null;
  state.ruleSim = null;
}

// -- Simulación (solo lectura): nunca envía ni registra nada --

let simToken = 0;

/** Lanza la simulación y repinta al empezar y al terminar. Solo cuenta la última petición. */
function runSimulation(key: string, kind: string, conditions: Record<string, string>, assumeAnySender: boolean, excludeRuleId: string | null, sig?: string) {
  const token = ++simToken;
  state.ruleSim = { key, status: "loading", kind, sig };
  render();
  api.simulateRule(kind, conditions, null, assumeAnySender, excludeRuleId).then(
    (sim) => {
      if (token !== simToken) return;
      state.ruleSim = { key, status: "done", kind, sig, sim };
      render();
    },
    (error) => {
      if (token !== simToken) return;
      state.ruleSim = { key, status: "error", kind, sig, error: String(error) };
      render();
    },
  );
}

/** Estado de la simulación de una clave: `missing` si no hay ninguna (no hay evidencia). */
function simStatus(key: string): "loading" | "done" | "error" | "missing" {
  return state.ruleSim?.key === key ? state.ruleSim.status : "missing";
}

function exampleList(examples: Array<{ task_id: number; label: string }>): HTMLElement | null {
  if (!examples.length) return null;
  const shown = examples.slice(0, 10);
  return h(
    "details",
    { class: "sim-examples" },
    h("summary", { "data-focus-key": `sim:examples:${examples[0]!.task_id}:${examples.length}` }, `Ver ${examples.length === 1 ? "el ejemplo" : `los ejemplos (${examples.length})`}`),
    h(
      "ul",
      {},
      ...shown.map((e) => h("li", {}, `${e.label} (tarea ${e.task_id})`)),
      examples.length > shown.length ? h("li", { class: "meta" }, `y ${examples.length - shown.length} más`) : null,
    ),
  );
}

/** Un hallazgo con qué pasa, por qué y la consecuencia, para el aviso previo a activar o restaurar. */
function findingLines(f: RuleFinding): HTMLElement {
  const t = explainFinding(f, ruleName);
  return h(
    "div",
    { class: "finding-lines" },
    h("p", {}, statusChip(t.badge.text, t.badge.tone), " ", h("strong", {}, t.title)),
    h("p", {}, t.what),
    h("p", {}, h("span", { class: "finding-label" }, "Por qué: "), t.why),
    h("p", {}, h("span", { class: "finding-label" }, "Consecuencia: "), t.consequence),
    t.timing ? h("p", { class: "meta" }, t.timing) : null,
    t.tasks ? h("p", { class: "meta" }, t.tasks) : null,
  );
}

/** Avisos que traería activar la regla. «Chocaría» solo se dice de reglas activas; el resto se explica aparte. */
function activationFindings(findings: RuleFinding[]): HTMLElement | null {
  if (!findings.length) return null;
  return h(
    "div",
    { class: "activation-findings", role: "group", "aria-label": "Avisos sobre esta regla" },
    ...activationGroups(findings).flatMap((g) => [h("p", { class: "warn-text" }, h("strong", {}, g.heading)), ...g.findings.map(findingLines)]),
  );
}

/** Resultado de la simulación de la clave dada, o `null` si no hay ninguna en curso. */
function simulationBox(key: string, kind: string): HTMLElement | null {
  const s = state.ruleSim;
  if (!s || s.key !== key || s.kind !== kind) return null;
  if (s.status === "loading") return h("div", { class: "sim-box", role: "status" }, h("p", { class: "meta" }, "Comprobando la regla con sus tareas…"));
  if (s.status === "error" || !s.sim)
    return h(
      "div",
      { class: "sim-box", role: "alert" },
      h("p", { class: "error-text" }, `No se ha podido hacer la prueba: ${s.error ?? "error desconocido"}.`),
      h("p", {}, "No hay datos para decir qué haría esta regla con sus tareas."),
    );
  const summary = simulationSummary(s.sim, kind);
  return h(
    "div",
    { class: "sim-box", role: "status" },
    h("p", { class: "sim-headline" }, summary.headline),
    activationFindings(s.sim.findings ?? []),
    summary.empty
      ? null
      : h(
          "ul",
          { class: "sim-list" },
          h("li", {}, h("strong", {}, `Se automatizarían sin revisión: ${summary.automate}`), exampleList(summary.automateExamples)),
          summary.rodaje ? h("li", {}, summary.rodaje) : null,
          ...summary.groups.map((g) =>
            h("li", {}, h("strong", {}, `Irían a revisión: ${g.count}`), ` porque ${g.text}.`, exampleList(g.examples)),
          ),
          summary.noMatch ? h("li", {}, `No coinciden con la regla: ${summary.noMatch}.`) : null,
          ...summary.blocked.map((b) =>
            h("li", { class: "warn-text" }, `«${ruleName(b.rule_id)}» dejaría de actuar en ${b.count} ${b.count === 1 ? "tarea" : "tareas"}: coincidirían dos reglas activas y entonces no se automatiza.`),
          ),
        ),
    ...summary.notes.map((n) => h("p", { class: "meta" }, n)),
    h("p", { class: "sim-disclaimer" }, SIMULATION_DISCLAIMER),
  );
}

// -- Confirmación de la opción avanzada (D126): casilla y texto escrito --

function ackControls(
  category: () => string,
  value: () => { ack: boolean; text: string },
  set: (ack: boolean, text: string) => void,
  onChange: () => void,
  keyPrefix: string,
): HTMLElement & { refreshText: () => void } {
  const check = h("input", {
    type: "checkbox",
    "data-focus-key": `${keyPrefix}:ack`,
    onchange: () => {
      set(check.checked, value().text);
      onChange();
    },
  }) as HTMLInputElement;
  const typed = h("input", {
    type: "text",
    id: `${keyPrefix}-ack-text`,
    autocomplete: "off",
    spellcheck: "false",
    "data-focus-key": `${keyPrefix}:ack_text`,
    "aria-describedby": `${keyPrefix}-ack-hint`,
    oninput: () => {
      set(value().ack, typed.value);
      onChange();
    },
  }) as HTMLInputElement;
  check.checked = value().ack;
  typed.value = value().text;
  const warning = h("p", { class: "ack-warning", role: "note" }, anySenderConsequences(category()));
  const box = h(
    "div",
    { class: "ack-box" },
    warning,
    h("label", { class: "choice" }, check, " Entiendo las consecuencias"),
    h(
      "div",
      { class: "field" },
      h("div", { class: "field-head" }, h("label", { for: `${keyPrefix}-ack-text` }, `Para confirmar, escriba ${ANY_SENDER_CONFIRMATION}`)),
      typed,
      h("p", { class: "hint", id: `${keyPrefix}-ack-hint` }, "Escríbalo tal cual, en mayúsculas."),
    ),
  );
  return Object.assign(box, { refreshText: () => (warning.textContent = anySenderConsequences(category())) });
}

// -- Formulario --

function ruleForm(): HTMLElement {
  const editor = state.ruleEditor!;
  const editing = editor.mode === "edit" ? (state.rules.find((r) => r.id === editor.id) ?? null) : null;
  const key = editor.mode === "new" ? newDraftKey : editDraftKey(editor.id);
  let draft: RuleDraft;
  try {
    draft = state.drafts[key] ? { ...draftOf(editing), ...(JSON.parse(state.drafts[key]) as Partial<RuleDraft>) } : draftOf(editing);
  } catch {
    draft = draftOf(editing);
  }
  const name = h("input", { name: "name", "data-focus-key": "rule-form:name", required: true, maxlength: "80", placeholder: "p. ej. Facturas de Alfa hasta 500 €", autocomplete: "off" }) as HTMLInputElement;
  const kind = h(
    "select",
    { name: "kind", "data-focus-key": "rule-form:kind" },
    h("option", { value: "register_invoice" }, "Registrar factura"),
    h("option", { value: "send_draft" }, "Enviar respuesta"),
  ) as HTMLSelectElement;
  const supplier = h("input", { name: "supplier", "data-focus-key": "rule-form:supplier", placeholder: "Nombre exacto del proveedor", autocomplete: "off" }) as HTMLInputElement;
  const maxTotal = h("input", { name: "max_total", "data-focus-key": "rule-form:max_total", placeholder: "p. ej. 500.00", inputmode: "decimal", autocomplete: "off" }) as HTMLInputElement;
  const category = h(
    "select",
    { name: "category", "data-focus-key": "rule-form:category" },
    ...Object.entries(CATEGORY_LABEL)
      .filter(([value]) => !REVIEW_ONLY_CATEGORIES.includes(value))
      .map(([value, label]) => h("option", { value }, label)),
  ) as HTMLSelectElement;
  const scopeSender = h("input", { type: "radio", name: "scope", value: "sender", "data-focus-key": "rule-form:scope:sender" }) as HTMLInputElement;
  const scopeDomain = h("input", { type: "radio", name: "scope", value: "domain", "data-focus-key": "rule-form:scope:domain" }) as HTMLInputElement;
  const sender = h("input", { name: "sender", type: "email", "data-focus-key": "rule-form:sender", placeholder: "p. ej. compras@cliente.test", autocomplete: "off", spellcheck: "false" }) as HTMLInputElement;
  const domain = h("input", { name: "sender_domain", "data-focus-key": "rule-form:sender_domain", placeholder: "p. ej. cliente.test", autocomplete: "off", spellcheck: "false" }) as HTMLInputElement;
  const useAny = h("input", { type: "checkbox", "data-focus-key": "rule-form:any" }) as HTMLInputElement;

  name.value = draft.name;
  kind.value = draft.kind;
  supplier.value = draft.supplier;
  maxTotal.value = draft.max_total;
  // Una regla antigua con una categoría de solo revisión (D177) la conserva a la vista, sin poder
  // elegirla de nuevo: hay que cambiarla para guardar.
  if (REVIEW_ONLY_CATEGORIES.includes(draft.category)) {
    category.append(h("option", { value: draft.category, disabled: true, title: REVIEW_ONLY_TEXT }, `${CATEGORY_LABEL[draft.category] ?? draft.category} (siempre se revisa)`));
  }
  category.value = draft.category;
  scopeDomain.checked = draft.scope === "domain";
  scopeSender.checked = draft.scope !== "domain";
  sender.value = draft.sender;
  domain.value = draft.sender_domain;
  useAny.checked = draft.scope === "any";
  let ack = { ack: draft.ack, text: draft.ack_text };

  const invoiceFields = h(
    "div",
    { class: "field-group" },
    field("Proveedor", supplier, FIELD_HELP.supplier, "obligatorio"),
    h(
      "p",
      { class: "hint" },
      "Escríbalo tal como lo lee la aplicación en la factura: «Alfa SL» no coincide con «Alfa, S.L.». No distingue mayúsculas de minúsculas.",
    ),
    field("Total máximo (en euros)", maxTotal, FIELD_HELP.max_total, "opcional"),
    h("p", { class: "hint" }, "Hoy la aplicación solo registra facturas en euros."),
    draft.currency ? h("p", { class: "hint" }, `Moneda guardada en la regla: ${draft.currency}. Se conserva al guardar.`) : null,
  );

  const publicWarning = h("p", { class: "rule-risk", role: "status" });
  const fromGroup = h(
    "fieldset",
    { class: "from-group" },
    h("legend", {}, "¿De quién? ", h("span", { class: "tag" }, "obligatorio"), helpTip(FIELD_HELP.scope, "Ayuda sobre el remitente")),
    h("label", { class: "choice" }, scopeSender, " Una dirección exacta"),
    field("Dirección del remitente", sender, FIELD_HELP.sender),
    h("label", { class: "choice" }, scopeDomain, " Todo un dominio"),
    field("Dominio del remitente", domain, FIELD_HELP.sender_domain),
    publicWarning,
    h(
      "p",
      { class: "hint" },
      "El remitente de un correo se puede suplantar. Indicarlo reduce el riesgo, pero no lo elimina: revise siempre qué reglas activa.",
    ),
  );
  const emailFields = h("div", { class: "field-group" }, field("Categoría del correo", category, FIELD_HELP.category, "obligatorio"), fromGroup);

  // Opciones avanzadas: cerradas por defecto (D126).
  const anyBox = h("div", { class: "any-box" });
  const advanced = h(
    "details",
    {
      class: "advanced",
      ontoggle: () => (state.advancedOpen = advanced.open),
    },
    h("summary", { "data-focus-key": "rule-form:advanced" }, "Opciones avanzadas"),
    h("p", { class: "hint" }, FIELD_HELP.advanced),
    h("label", { class: "choice" }, useAny, " Quiero que la regla conteste a cualquier remitente"),
    anyBox,
  ) as HTMLDetailsElement;
  advanced.open = state.advancedOpen || draft.scope === "any";

  const previewSentence = h("p", { class: "rule-sentence" });
  const previewRisk = h("p", { class: "rule-risk" });
  const previewProblem = h("p", { class: "rule-problem" });
  const preview = h("div", { class: "rule-preview", "aria-live": "polite" }, h("p", { class: "meta" }, "Vista previa de la regla"), previewSentence, previewRisk, previewProblem);
  const editNotice = h("p", { class: "meta warn", role: "status" });
  const problems = h("ul", { class: "form-problems", id: "rule-form-problems", "aria-live": "polite" });
  const simSlot = h("div", { class: "sim-slot" });

  const read = (): RuleDraft => ({
    id: editing ? editing.id : "",
    name: name.value,
    kind: editing ? editing.action_kind : kind.value,
    supplier: supplier.value,
    max_total: maxTotal.value,
    currency: draft.currency,
    category: category.value,
    scope: useAny.checked ? "any" : scopeDomain.checked ? "domain" : "sender",
    sender: sender.value,
    sender_domain: domain.value,
    ack: ack.ack,
    ack_text: ack.text,
  });

  // Se asignan más abajo: `sync` los usa y se llama al final.
  let ackBox: (HTMLElement & { refreshText: () => void }) | null = null;
  let signature: (d: RuleDraft) => string = () => "";
  const submit = h("button", { class: "primary", type: "submit", "aria-describedby": "rule-form-problems" }, editing ? "Guardar cambios" : "Crear regla") as HTMLButtonElement;
  const testButton = h("button", { type: "button", "data-focus-key": "rule-form:test" }, "Probar con mis datos") as HTMLButtonElement;

  const sync = () => {
    const d = read();
    state.drafts[key] = JSON.stringify(d);
    const isInvoice = d.kind === "register_invoice";
    invoiceFields.hidden = !isInvoice;
    emailFields.hidden = isInvoice;
    advanced.hidden = isInvoice;
    // La dirección y el dominio solo se editan con su opción elegida; con «cualquier remitente»
    // quedan desactivados.
    sender.disabled = d.scope !== "sender";
    domain.disabled = d.scope !== "domain";
    scopeSender.disabled = scopeDomain.disabled = d.scope === "any";
    const warning = d.scope === "domain" ? publicDomainWarning(d.sender_domain) : null;
    publicWarning.textContent = warning ?? "";
    publicWarning.hidden = !warning;
    anyBox.hidden = d.scope !== "any";
    const rule = { id: d.id, version: 0, action_kind: d.kind, conditions: draftConditions(d), active: false, any_sender_ack_at: d.scope === "any" ? 1 : null };
    previewSentence.textContent = ruleSentence(rule);
    previewRisk.textContent = `Aviso: ${ruleRisk(rule)}`;
    const problem = ruleProblem(rule);
    previewProblem.textContent = problem ?? "";
    previewProblem.hidden = !problem;
    const notice = anySenderEditNotice(editing, d);
    editNotice.textContent = notice ?? "";
    editNotice.hidden = !notice;
    const issues = draftProblems(d, !editing);
    problems.replaceChildren(...issues.map((i) => h("li", {}, i)));
    problems.hidden = issues.length === 0;
    submit.disabled = issues.length > 0;
    testButton.disabled = draftProblems(d, false, { ignoreAck: true }).filter((p) => !p.startsWith("Escriba un nombre") && p !== REVIEW_ONLY_TEXT).length > 0;
    ackBox?.refreshText();
    // Si las condiciones cambian, el resultado de la prueba ya no es de lo que hay en pantalla.
    if (state.ruleSim?.key === "form" && state.ruleSim.sig !== signature(d)) {
      state.ruleSim = null;
      simToken++;
      simSlot.replaceChildren();
    }
    simSlot.hidden = state.ruleSim?.kind !== d.kind;
  };
  ackBox = ackControls(
    () => category.value,
    () => ack,
    (a, t) => (ack = { ack: a, text: t }),
    sync,
    "rule-form",
  );
  anyBox.append(ackBox);
  signature = (d: RuleDraft) => JSON.stringify([d.kind, draftConditions(d), d.scope === "any"]);
  testButton.addEventListener("click", () => {
    const d = read();
    // Al probar una regla guardada, se excluye a sí misma de la comparación.
    runSimulation("form", d.kind, draftConditions(d), d.scope === "any", editing?.id ?? null, signature(d));
  });
  const formSim = simulationBox("form", draft.kind);
  if (formSim) simSlot.append(formSim);

  const wasActive = editing?.active ?? false;
  const form = h(
    "form",
    {
      class: "card rule-form",
      novalidate: true,
      "aria-label": editing ? `Editar la regla ${editing.name}` : "Nueva regla",
      oninput: sync,
      onchange: sync,
      onsubmit: (event) => {
        event.preventDefault();
        const d = read();
        if (draftProblems(d, !editing).length) return;
        const conditions = draftConditions(d);
        if (editing) {
          void act(async () => {
            // Una sola llamada atómica: nombre, condiciones y validación avanzada, o nada. Guardar
            // borra la validación anterior; si se mantiene «cualquier remitente», va confirmada.
            await api.updateRuleWithAck(editing.id, d.name.trim(), conditions, editing.action_kind === "send_draft" && d.scope === "any");
            closeRuleEditor();
          }, wasActive ? "Regla guardada; sigue activa y se aplicará a las tareas que se procesen desde ahora" : "Regla guardada; sigue inactiva");
        } else {
          void act(async () => {
            await api.createRule(null, d.kind, conditions, { name: d.name.trim(), acknowledgeAnySender: d.kind === "send_draft" && d.scope === "any" });
            closeRuleEditor();
          }, "Regla creada inactiva; compruébela con «Probar» y actívela cuando quiera");
        }
      },
    },
    h("h3", {}, editing ? `Editar la regla ${editing.name}` : "Nueva regla"),
    h("div", { class: "field-group" }, field("Nombre", name, FIELD_HELP.name, "obligatorio")),
    editing
      ? h(
          "p",
          { class: "meta" },
          `Identificador: `,
          h("code", {}, editing.id),
          ` · Acción: ${ACTION_LABEL[editing.action_kind] ?? editing.action_kind}. Ni el identificador ni la acción se pueden cambiar; para otro tipo de regla, cree una nueva.`,
        )
      : h("div", { class: "field-group" }, field("Acción", kind, FIELD_HELP.action)),
    invoiceFields,
    emailFields,
    advanced,
    preview,
    editNotice,
    h(
      "div",
      { class: "sim-try" },
      h("p", { class: "hint" }, "Antes de guardar, puede probar la regla con sus tareas anteriores. Es solo una prueba: no se envía ni se registra nada."),
      testButton,
      simSlot,
    ),
    editing
      ? h(
          "p",
          { class: wasActive ? "hint warn-text" : "hint" },
          wasActive
            ? "Esta regla está activa y seguirá activa al guardar. Se aplicará a las tareas que se procesen desde ahora, no a las que ya están en Pendientes."
            : "Esta regla está inactiva y seguirá inactiva al guardar; actívela desde la lista cuando la haya comprobado.",
        )
      : h("p", { class: "hint" }, "La regla se crea inactiva: no hará nada hasta que usted la active desde la lista."),
    problems,
    h(
      "div",
      { class: "buttons" },
      submit,
      h("button", { type: "button", onclick: () => (closeRuleEditor(), render()) }, "Cancelar"),
    ),
  );
  if (editing) kind.disabled = true;
  sync();
  return form;
}

// -- Lista --

function filteredRules(): RuleView[] {
  const f = state.ruleFilter;
  const query = normalizeSearch(f.text.trim());
  return state.rules.filter(
    (r) =>
      (f.status === "archived" ? Boolean(r.archived_at) : !r.archived_at) &&
      (!f.action || r.action_kind === f.action) &&
      (!f.status || f.status === "archived" || (f.status === "active") === r.active) &&
      (!f.category || r.conditions.category === f.category) &&
      (!query || searchText(r).includes(query)),
  );
}

function closeRulePanel() {
  const id = state.rulePanel?.id;
  state.rulePanel = null;
  state.ruleSim = null;
  simToken++;
  if (id) state.focusAfterRender = `rule:${id}:more`;
  render();
}

function openRulePanel(r: RuleView, kind: NonNullable<typeof state.rulePanel>["kind"]) {
  state.rulePanel = { id: r.id, kind };
  state.ruleAdv = { ack: false, text: "", skip: false };
  state.ruleMore = null;
  state.ruleSim = null;
  state.focusAfterRender = `rule:${r.id}:panel`;
  const simulate = kind !== "archive" && isKnownAction(r.action_kind) && !(kind === "activate" && actsOnNothing(r));
  // La hipótesis de «cualquier remitente» vale si la regla ya lo tiene validado o se va a validar.
  // La regla se excluye de la comparación: no debe coincidir consigo misma.
  if (simulate) runSimulation(`${kind}:${r.id}`, r.action_kind, r.conditions, kind === "advanced" || r.any_sender_ack_at != null, r.id);
  else render();
}

/** Panel dentro de la tarjeta: activar con evidencia, probar, validar la opción avanzada o archivar. */
function rulePanel(r: RuleView): HTMLElement | null {
  const panel = state.rulePanel;
  if (!panel || panel.id !== r.id) return null;
  const key = `${panel.kind}:${r.id}`;
  const status = simStatus(key);
  const box = (label: string, ...children: Child[]) =>
    h(
      "div",
      {
        class: "row-panel",
        role: "group",
        "aria-label": label,
        tabindex: "-1",
        "data-focus-key": `rule:${r.id}:panel`,
        onkeydown: (event) => {
          if ((event as KeyboardEvent).key === "Escape") {
            event.stopPropagation();
            closeRulePanel();
          }
        },
      },
      ...children,
    );
  const cancel = (label = "Cancelar") => h("button", { type: "button", "data-focus-key": `rule:${r.id}:panel-cancel`, onclick: closeRulePanel }, label);

  if (panel.kind === "test") {
    return box(
      `Prueba de la regla ${r.name}`,
      h("h4", {}, "Probar con sus tareas"),
      h("p", { class: "rule-sentence" }, ruleSentence(r)),
      simulationBox(key, r.action_kind),
      h("div", { class: "buttons" }, cancel("Cerrar")),
    );
  }

  if (panel.kind === "activate") {
    const blocked = actsOnNothing(r) ? "Esta regla no indica remitente y no podría actuar. Añada un remitente o valide la opción avanzada antes de activarla." : ruleProblem(r);
    const failed = status === "error" || status === "missing";
    const go = h(
      "button",
      {
        class: "primary",
        type: "button",
        "data-focus-key": `rule:${r.id}:panel-go`,
        onclick: () =>
          void act(async () => {
            await api.setRuleActive(r.id, true);
            state.rulePanel = null;
            state.ruleSim = null;
          }, "Regla activada: actuará sin revisión en las tareas que se procesen desde ahora"),
      },
      "Activar regla",
    ) as HTMLButtonElement;
    const sync = () => (go.disabled = status === "loading" || (failed && !state.ruleAdv.skip));
    sync();
    const skip = h("input", {
      type: "checkbox",
      "data-focus-key": `rule:${r.id}:skip`,
      onchange: () => {
        state.ruleAdv = { ...state.ruleAdv, skip: skip.checked };
        sync();
      },
    }) as HTMLInputElement;
    skip.checked = state.ruleAdv.skip;
    return box(
      `Activar la regla ${r.name}`,
      h("h4", {}, "¿Activar esta regla?"),
      h("p", { class: "rule-sentence" }, ruleSentence(r)),
      h("p", { class: "rule-risk" }, "Aviso: ", ruleRisk(r)),
      blocked ? h("p", { class: "rule-problem" }, blocked) : h("p", { class: "meta" }, "Esto es lo que habría hecho con sus tareas anteriores:"),
      blocked ? null : simulationBox(key, r.action_kind),
      !blocked && failed
        ? h("label", { class: "choice" }, skip, " Quiero activarla sin esta comprobación")
        : null,
      blocked ? null : h("p", { class: "hint" }, "Desactivarla después es siempre un solo clic."),
      h(
        "div",
        { class: "buttons" },
        blocked
          ? actsOnNothing(r)
            ? h("button", { class: "primary", type: "button", "data-focus-key": `rule:${r.id}:panel-go`, onclick: () => addSender(r) }, "Añadir remitente")
            : null
          : go,
        cancel(),
      ),
    );
  }

  if (panel.kind === "advanced") {
    const go = h(
      "button",
      {
        class: "danger",
        type: "button",
        "data-focus-key": `rule:${r.id}:panel-go`,
        onclick: () =>
          void act(
            async () => {
              await api.setRuleAnySender(r.id, true);
              state.rulePanel = null;
              state.ruleSim = null;
            },
            r.active
              ? "Opción avanzada validada. La regla está activa: ya contesta a cualquier remitente sin revisión. Queda en el historial y puede retirarla cuando quiera"
              : "Opción avanzada validada. Cuando active la regla, contestará a cualquier remitente sin revisión. Queda en el historial y puede retirarla cuando quiera",
          ),
      },
      "Validar la opción avanzada",
    ) as HTMLButtonElement;
    const sync = () => (go.disabled = status === "loading" || !anyAckReady({ ack: state.ruleAdv.ack, ack_text: state.ruleAdv.text }));
    sync();
    return box(
      `Opción avanzada de la regla ${r.name}`,
      h("h4", {}, "Contestar a cualquier remitente"),
      h("p", { class: "rule-sentence" }, `Enviar sin revisión la respuesta redactada por la IA a cualquier remitente cuando se clasifique como ${categoryLabel(r.conditions.category ?? "")}`),
      r.active
        ? h("p", { class: "rule-problem", role: "alert" }, "Esta regla está activa: empezará a contestar en cuanto la valide.")
        : null,
      ackControls(
        () => r.conditions.category ?? "",
        () => ({ ack: state.ruleAdv.ack, text: state.ruleAdv.text }),
        (a, t) => (state.ruleAdv = { ...state.ruleAdv, ack: a, text: t }),
        sync,
        `rule:${r.id}`,
      ),
      h("p", { class: "meta" }, "Esto es lo que habría hecho con sus tareas anteriores:"),
      simulationBox(key, r.action_kind),
      h("div", { class: "buttons" }, go, cancel()),
    );
  }

  if (panel.kind === "restore") {
    return box(
      `Restaurar la regla ${r.name}`,
      h("h4", {}, "¿Restaurar esta regla?"),
      h("p", { class: "warn-text" }, h("strong", {}, restoreHeading(r.name, state.restoreFindings))),
      ...state.restoreFindings.map(findingLines),
      h("p", { class: "hint" }, "Restaurarla no activa nada: la regla vuelve inactiva y puede archivarla otra vez."),
      h(
        "div",
        { class: "buttons" },
        h(
          "button",
          {
            class: "primary",
            type: "button",
            "data-focus-key": `rule:${r.id}:panel-go`,
            onclick: () =>
              void act(async () => {
                await api.unarchiveRule(r.id);
                state.rulePanel = null;
              }, RESTORED),
          },
          "Restaurar igualmente",
        ),
        cancel(),
      ),
    );
  }

  return box(
    `Archivar la regla ${r.name}`,
    h("h4", {}, "¿Archivar esta regla?"),
    h(
      "p",
      {},
      `«${r.name}» quedará inactiva y fuera de la lista. No se podrá activar ni editar hasta que la restaure, y al restaurarla seguirá inactiva.${r.any_sender_ack_at ? " Se retirará también su opción avanzada." : ""} Su historial se conserva.`,
    ),
    h(
      "div",
      { class: "buttons" },
      h(
        "button",
        {
          class: "danger",
          type: "button",
          "data-focus-key": `rule:${r.id}:panel-go`,
          onclick: () =>
            void act(async () => {
              await api.archiveRule(r.id);
              state.rulePanel = null;
            }, "Regla archivada. Puede verla con el filtro «Archivadas» y restaurarla cuando quiera"),
        },
        "Archivar",
      ),
      cancel(),
    ),
  );
}

const RESTORED = "Regla restaurada. Queda inactiva: actívela cuando quiera";

/** Restaura una regla archivada; si chocaría con una regla actual, lo explica antes y pide confirmar. */
async function restoreRule(r: RuleView) {
  try {
    const findings = await api.previewRestore(r.id);
    if (!findings.length) {
      await act(() => api.unarchiveRule(r.id), RESTORED);
      return;
    }
    state.restoreFindings = findings;
    state.rulePanel = { id: r.id, kind: "restore" };
    state.ruleSim = null;
    state.ruleMore = null;
    state.focusAfterRender = `rule:${r.id}:panel`;
    render();
  } catch (error) {
    notify("error", error);
  }
}

/** Corrección en un clic de una regla antigua sin remitente: abre la edición con el foco ahí. */
function addSender(r: RuleView) {
  state.focusAfterRender = "rule-form:sender";
  openRuleEditor({ mode: "edit", id: r.id }, { ...draftOf(r), scope: "sender", ack: false, ack_text: "" });
}

function statsLine(r: RuleView): HTMLElement {
  const t = statsText(r.stats);
  return h("p", { class: "meta rule-stats" }, t.main, t.unclassified ? ` · ${t.unclassified}` : "");
}

function moreMenu(r: RuleView, items: Array<[string, string, () => void]>): HTMLElement {
  const menu = h(
    "details",
    {
      class: "more-menu",
      ontoggle: () => {
        if (menu.open) state.ruleMore = r.id;
        else if (state.ruleMore === r.id) state.ruleMore = null;
      },
      onkeydown: (event) => {
        if ((event as KeyboardEvent).key === "Escape" && menu.open) {
          event.stopPropagation();
          menu.open = false;
          (menu.querySelector("summary") as HTMLElement).focus();
        }
      },
    },
    h("summary", { "data-focus-key": `rule:${r.id}:more`, "aria-label": `Más acciones de ${r.name}` }, "Más"),
    h(
      "div",
      { class: "more-items" },
      ...items.map(([focus, label, run]) => h("button", { type: "button", "data-focus-key": `rule:${r.id}:${focus}`, onclick: run }, label)),
    ),
  ) as HTMLDetailsElement;
  menu.open = state.ruleMore === r.id;
  return menu;
}

/** Tarjeta de una regla: cabecera con nombre y estado, cuerpo con lo que hace y su riesgo, y pie
 * con las acciones siempre a la vista. Lo técnico va en «Detalles». */
function learnedRuleChips(r: RuleView): HTMLElement[] {
  const c = learnedChips(r.origin);
  return [c.learned ? statusChip("Aprendida", "info") : null, c.rodaje ? statusChip(c.rodaje, "warn") : null].filter((x): x is HTMLElement => x !== null);
}

function ruleCard(r: RuleView): HTMLElement {
  const problem = ruleProblem(r);
  const known = isKnownAction(r.action_kind);
  const status = ruleStatus(r);
  const archived = Boolean(r.archived_at);
  const missingSender = actsOnNothing(r) && !archived;
  const toggle = () => {
    if (r.active) void act(() => api.setRuleActive(r.id, false), "Regla desactivada");
    else if (known) openRulePanel(r, "activate");
    else void act(() => api.setRuleActive(r.id, true), "Regla activada");
  };
  const more: Array<[string, string, () => void]> = [];
  if (known && !archived) more.push(["dup", "Duplicar", () => openRuleEditor({ mode: "new" }, { ...draftOf(r), id: "", name: r.name === r.id ? "" : `${r.name} (copia)`, ack: false, ack_text: "" })]);
  more.push(["history", "Ver historial", () => showHistoryOf(r.id)]);
  if (!archived) more.push(["archive", "Archivar", () => openRulePanel(r, "archive")]);

  const details = h(
    "details",
    {
      class: "rule-details",
      ontoggle: () => (state.ruleDetails[r.id] = details.open),
    },
    h("summary", { "data-focus-key": `rule:${r.id}:details` }, "Detalles"),
    h(
      "dl",
      { class: "rule-facts" },
      h("dt", {}, "Identificador"),
      h("dd", {}, h("code", {}, r.id)),
      h("dt", {}, "Versión"),
      h("dd", {}, String(r.version)),
      h("dt", {}, "Acción"),
      h("dd", {}, ACTION_LABEL[r.action_kind] ?? r.action_kind),
      h("dt", {}, "Condiciones"),
      h("dd", {}, conditionsSummary(r)),
      ...(originText(r.origin, ruleName) ? [h("dt", {}, "Origen"), h("dd", {}, originText(r.origin, ruleName)!)] : []),
    ),
  ) as HTMLDetailsElement;
  details.open = Boolean(state.ruleDetails[r.id]);

  return h(
    "article",
    { class: `card rule-card${archived ? " is-archived" : ""}`, "aria-label": `Regla ${r.name}` },
    h(
      "header",
      { class: "rule-head" },
      h("h3", { class: "rule-title" }, r.name),
      h(
        "div",
        { class: "status-cell" },
        statusChip(status.primary.text, status.primary.tone),
        status.extra ? statusChip(status.extra.text, status.extra.tone) : null,
        ...learnedRuleChips(r),
        r.any_sender_ack_at && !archived ? statusChip("Contesta a cualquier remitente", "warn") : null,
      ),
    ),
    h(
      "div",
      { class: "rule-info" },
      h("p", { class: "rule-sentence" }, ruleSentence(r)),
      h("p", { class: missingSender || r.any_sender_ack_at ? "rule-problem" : "rule-risk" }, "Aviso: ", ruleRisk(r)),
      problem ? h("p", { class: "rule-problem" }, problem) : null,
      statsLine(r),
      missingSender
        ? h("p", { class: "meta" }, h("button", { class: "link", type: "button", "data-focus-key": `rule:${r.id}:advanced`, onclick: () => openRulePanel(r, "advanced") }, "Contestar a cualquier remitente (opción avanzada)"))
        : null,
      r.any_sender_ack_at && !archived
        ? h(
            "p",
            { class: "meta" },
            h("button", { class: "link", type: "button", "data-focus-key": `rule:${r.id}:retract`, onclick: () => void act(() => api.setRuleAnySender(r.id, false), "Opción avanzada retirada: la regla vuelve a exigir remitente y no actuará hasta que lo añada") }, "Retirar la opción avanzada"),
          )
        : null,
      rulePanel(r),
      details,
    ),
    h(
      "footer",
      { class: "rule-actions" },
      ...(archived
        ? [
            h("button", { "data-focus-key": `rule:${r.id}:restore`, onclick: () => void restoreRule(r) }, "Restaurar"),
            h("button", { "data-focus-key": `rule:${r.id}:history`, onclick: () => showHistoryOf(r.id) }, "Ver historial"),
          ]
        : [
            missingSender ? h("button", { class: "primary", "data-focus-key": `rule:${r.id}:add-sender`, onclick: () => addSender(r) }, "Añadir remitente") : null,
            h("button", { "data-focus-key": `rule:${r.id}:toggle`, onclick: toggle }, r.active ? "Desactivar" : "Activar"),
            known ? h("button", { "data-focus-key": `rule:${r.id}:edit`, onclick: () => openRuleEditor({ mode: "edit", id: r.id }, undefined) }, "Editar") : null,
            known ? h("button", { "data-focus-key": `rule:${r.id}:test`, onclick: () => openRulePanel(r, "test") }, "Probar") : null,
            moreMenu(r, more),
          ]),
    ),
  );
}

/** Lista con buscador y filtros; al escribir solo se rehace la lista, para conservar el foco. */
function rulesList(): HTMLElement {
  const box = h("div", { class: "rules-list" });
  const results = h("div", { "aria-live": "polite" });
  const fill = () => {
    const rows = filteredRules();
    const f = state.ruleFilter;
    const filtered = Boolean(f.text.trim() || f.action || f.status || f.category);
    const hiddenArchived = state.rules.filter((r) => r.archived_at).length;
    const live = state.rules.filter((r) => !r.archived_at).length;
    results.replaceChildren(
      !state.rules.length
        ? emptyState("Todavía no hay reglas", "Todas las acciones requieren su aprobación en Pendientes. Cree una con «Nueva regla».")
        : !rows.length
          ? emptyState(
              f.status === "archived" ? "No hay reglas archivadas con esa búsqueda." : "Ninguna regla coincide con la búsqueda y los filtros elegidos.",
              undefined,
              filtered ? h("button", { "data-focus-key": "rules:clear", onclick: clearRuleFilter }, "Quitar filtros") : null,
            )
          : h(
              "p",
              { class: "meta list-count" },
              f.status === "archived" ? `${rows.length} de ${hiddenArchived} reglas archivadas` : `${rows.length} de ${live} reglas`,
              ...(f.status !== "archived" && hiddenArchived
                ? [
                    ` · ${hiddenArchived} ${hiddenArchived === 1 ? "archivada oculta" : "archivadas ocultas"} `,
                    h("button", { class: "link", type: "button", "data-focus-key": "rules:show-archived", onclick: () => ((state.ruleFilter = { ...noRuleFilter(), status: "archived" }), render()) }, "Ver archivadas"),
                  ]
                : []),
              helpTip(FIELD_HELP.rules_list!, "Ayuda sobre las tarjetas de reglas"),
            ),
      ...(rows.length ? [h("div", { class: "rule-cards" }, ...rows.map(ruleCard))] : []),
    );
  };
  const search = h("input", {
    type: "search",
    id: "rule-search",
    "data-focus-key": "rules:search",
    placeholder: "Nombre, proveedor, remitente o categoría",
    autocomplete: "off",
    oninput: () => ((state.ruleFilter.text = search.value), fill()),
  }) as HTMLInputElement;
  search.value = state.ruleFilter.text;
  const select = (name: keyof typeof state.ruleFilter, options: Array<[string, string]>) => {
    const el = h(
      "select",
      { "data-focus-key": `rules:filter:${name}`, onchange: () => ((state.ruleFilter[name] = el.value), fill()) },
      ...options.map(([value, label]) => h("option", { value }, label)),
    ) as HTMLSelectElement;
    el.value = state.ruleFilter[name];
    return el;
  };
  const action = select("action", [["", "Todas las acciones"], ["register_invoice", "Registrar factura"], ["send_draft", "Enviar respuesta"]]);
  const status = select("status", [["", "Activas e inactivas"], ["active", "Solo activas"], ["inactive", "Solo inactivas"], ["archived", "Archivadas"]]);
  const category = select("category", [["", "Todas las categorías"], ...Object.entries(CATEGORY_LABEL)]);
  function clearRuleFilter() {
    state.ruleFilter = noRuleFilter();
    render();
  }
  box.append(
    h(
      "div",
      { class: "filters", role: "search" },
      h("div", { class: "field grow" }, h("div", { class: "field-head" }, h("label", { for: "rule-search" }, "Buscar"), helpTip(FIELD_HELP.search, "Ayuda sobre el buscador")), search),
      h("div", { class: "field" }, h("div", { class: "field-head" }, h("label", {}, "Acción", action), helpTip(FIELD_HELP.filter_action, "Ayuda sobre el filtro de acción"))),
      h("div", { class: "field" }, h("div", { class: "field-head" }, h("label", {}, "Estado", status), helpTip(FIELD_HELP.filter_status + " " + FIELD_HELP.filter_archived, "Ayuda sobre el filtro de estado"))),
      h("div", { class: "field" }, h("div", { class: "field-head" }, h("label", {}, "Categoría", category), helpTip(FIELD_HELP.filter_category, "Ayuda sobre el filtro de categoría"))),
    ),
    results,
  );
  fill();
  return box;
}

function categoriesPanel(): HTMLElement {
  const emails = state.tasks.filter((t) => t.kind === "email");
  const counts: Record<string, number> = {};
  let unclassified = 0;
  for (const t of emails) {
    const c = t.result?.category;
    if (typeof c === "string" && c) counts[c] = (counts[c] ?? 0) + 1;
    else unclassified += 1;
  }
  return h(
    "div",
    {},
    h(
      "p",
      { class: "intro" },
      "La IA clasifica cada correo recibido en una de estas once categorías; no se pueden añadir ni cambiar. Una regla de envío se aplica a los correos de la categoría que elija, de los remitentes que usted indique.",
    ),
    dataTable(
      [
        { label: h("span", {}, "Categoría ", helpTip(FIELD_HELP.category_name, "Ayuda sobre las categorías")) },
        { label: "Qué significa" },
        { label: "Ejemplo inventado" },
        { label: h("span", {}, "Correos ", helpTip(FIELD_HELP.category_count, "Ayuda sobre el recuento")) },
        { label: h("span", {}, "Reglas que la usan ", helpTip(FIELD_HELP.category_rules, "Ayuda sobre las reglas que la usan")) },
      ],
      Object.entries(CATEGORY_LABEL).map(([key, label]) => {
        const using = state.rules.filter((r) => r.action_kind === "send_draft" && !r.archived_at && r.conditions.category === key);
        const info = CATEGORY_HELP[key];
        return [
          h("strong", {}, label),
          info.description,
          h("em", {}, info.example),
          String(counts[key] ?? 0),
          using.length
            ? h(
                "span",
                {},
                `${using.length} ${using.length === 1 ? "regla" : "reglas"}: ${using.map((r) => r.name).join(", ")} `,
                h("button", { class: "link", onclick: () => showRulesOf(key) }, "Ver reglas"),
              )
            : "Ninguna",
        ];
      }),
    ),
    h(
      "p",
      { class: "meta" },
      state.tasks.length
        ? `Recuento sobre los ${emails.length} correos que constan en este equipo, incluidos los ya tratados${unclassified ? `; ${unclassified} sin categoría (aún sin clasificar o con error)` : ""}.`
        : "Todavía no consta ningún correo en este equipo: el recuento es cero.",
    ),
  );
}

// -- Revisión de reglas: contradicciones y unificación (R23, D130, D131) --

function closeMerge() {
  const key = state.merge?.key;
  state.merge = null;
  mergeToken++;
  if (key) state.focusAfterRender = `finding:${key}:merge`;
  render();
}

let mergeToken = 0;
let merging = false;

/** Pide el plan (solo lectura) y repinta. Solo cuenta la última petición. */
function loadMerge(key: string, ids: string[], chosen: string | null) {
  const token = ++mergeToken;
  const previous = state.merge?.key === key ? state.merge : null;
  state.merge = { key, ids, chosen, name: previous?.name ?? "", status: "loading", preview: previous?.preview };
  render();
  api.previewMerge(ids, chosen).then(
    (preview) => {
      if (token !== mergeToken || !state.merge) return;
      state.merge = { ...state.merge, status: "done", preview, name: state.merge.name || preview.suggested_name, chosen: preview.chosen ?? chosen };
      render();
    },
    (error) => {
      if (token !== mergeToken || !state.merge) return;
      state.merge = { ...state.merge, status: "error", error: String(error) };
      render();
    },
  );
}

function mergePanel(f: RuleFinding): HTMLElement | null {
  const m = state.merge;
  if (!m || m.key !== f.key) return null;
  const cancel = h("button", { type: "button", "data-focus-key": `finding:${f.key}:merge-cancel`, onclick: closeMerge }, "Cancelar");
  const box = (...children: Child[]) =>
    h(
      "div",
      {
        class: "row-panel merge-panel",
        role: "group",
        "aria-label": "Unificar reglas",
        tabindex: "-1",
        "data-focus-key": `finding:${f.key}:merge-panel`,
        onkeydown: (event) => {
          if ((event as KeyboardEvent).key === "Escape") {
            event.stopPropagation();
            closeMerge();
          }
        },
      },
      h("h4", {}, "Unificar en una sola regla"),
      ...children,
    );
  if (m.status === "loading" && !m.preview)
    return box(h("p", { class: "meta", role: "status" }, "Preparando la unificación…"), h("div", { class: "buttons" }, cancel));
  if (m.status === "error" || !m.preview)
    return box(
      h("p", { class: "error-text", role: "alert" }, `No se puede preparar la unificación: ${m.error ?? "error desconocido"}`),
      h("div", { class: "buttons" }, cancel),
    );
  const preview = m.preview;
  const plan = preview.plan;
  const sentenceOf = (conditions: Record<string, any>, anySender = false) =>
    ruleSentence({ id: "", version: 0, action_kind: preview.action_kind, conditions, active: false, any_sender_ack_at: anySender ? 1 : null });
  const subject =
    preview.action_kind === "register_invoice"
      ? String(preview.before[0]?.conditions.supplier ?? "")
      : categoryLabel(String(preview.before[0]?.conditions.category ?? ""));

  const choice = preview.needs_choice
    ? h(
        "fieldset",
        { class: "merge-choice" },
        h("legend", {}, preview.action_kind === "register_invoice" ? "¿Con qué límite debe quedar la regla nueva?" : "¿A quién debe contestar la regla nueva?"),
        ...preview.options.map((o) => {
          const radio = h("input", {
            type: "radio",
            name: `merge-${f.key}`,
            "data-focus-key": `finding:${f.key}:option:${o.key}`,
            onchange: () => loadMerge(f.key, m.ids, o.key),
          }) as HTMLInputElement;
          radio.checked = m.chosen === o.key;
          return h(
            "label",
            { class: "choice merge-option" },
            radio,
            h("span", {}, h("strong", {}, mergeOptionLabel(preview.action_kind, o)), h("br", {}), mergeOptionText(preview.action_kind, o, subject, ruleName)),
          );
        }),
      )
    : null;

  const before = h(
    "div",
    { class: "merge-column" },
    h("h5", {}, "Antes"),
    h("ul", {}, ...preview.before.map((r) => h("li", {}, h("strong", {}, ruleName(r.id)), `: ${sentenceOf(r.conditions, r.any_sender)}. `, statusChip(r.active ? "Activa" : "Inactiva", r.active ? "ok" : "neutral")))),
  );
  const nameInput = h("input", {
    type: "text",
    id: `merge-name-${f.key}`,
    maxlength: "80",
    autocomplete: "off",
    "data-focus-key": `finding:${f.key}:merge-name`,
    oninput: () => {
      if (state.merge) state.merge.name = nameInput.value;
    },
  }) as HTMLInputElement;
  nameInput.value = m.name;
  const after = h(
    "div",
    { class: "merge-column" },
    h("h5", {}, "Después"),
    plan
      ? h(
          "div",
          {},
          h("p", {}, h("strong", {}, "Regla nueva"), `: ${sentenceOf(plan.after.conditions)}. `, statusChip("Inactiva", "neutral")),
          h("div", { class: "field" }, h("div", { class: "field-head" }, h("label", { for: `merge-name-${f.key}` }, "Nombre de la regla nueva")), nameInput),
          h("p", { class: "meta" }, "Las reglas anteriores quedarán archivadas."),
        )
      : h("p", { class: "meta" }, "Elija una opción para ver cómo quedaría la regla nueva."),
  );

  let simulation: HTMLElement | null = null;
  if (plan) {
    const t = mergeSimulationText(plan, ruleName);
    simulation = h(
      "div",
      { class: "sim-box", role: "status", "aria-label": "Qué cambiaría" },
      h("p", { class: "sim-headline" }, "Qué cambiaría con sus tareas guardadas, si la regla nueva estuviera activa:"),
      t.heading ? h("p", { class: "meta" }, t.heading) : null,
      h("ul", { class: "sim-list" }, ...t.lines.map((line) => h("li", {}, line))),
      t.validated ? h("p", { class: "sim-headline" }, t.validated.heading) : null,
      t.validated ? h("ul", { class: "sim-list" }, ...t.validated.lines.map((line) => h("li", {}, line))) : null,
      ...t.notes.map((n) => h("p", { class: "warn-text" }, n)),
      h("p", { class: "sim-disclaimer" }, SIMULATION_DISCLAIMER),
    );
  }

  const go = h(
    "button",
    {
      class: "primary",
      type: "button",
      "data-focus-key": `finding:${f.key}:merge-go`,
      onclick: async () => {
        // Un segundo clic no debe repetir la unificación ni mostrar un error tras el éxito.
        if (!plan || go.disabled || merging) return;
        merging = true;
        go.disabled = true;
        const name = nameInput.value.trim();
        try {
          const view = await api.mergeRules(m.ids, plan.chosen, name || null, plan.fingerprint);
          state.merge = null;
          mergeToken++;
          state.ruleFilter = noRuleFilter();
          state.focusAfterRender = `rule:${view.id}:toggle`;
          state.notice = { kind: "ok", text: mergedDone(view.name) };
          await refresh();
        } catch (error) {
          notify("error", error);
          // Si las reglas cambiaron, se vuelve a calcular el plan para revisarlo de nuevo.
          loadMerge(m.key, m.ids, m.chosen);
        } finally {
          merging = false;
        }
      },
    },
    "Unificar",
  ) as HTMLButtonElement;
  go.disabled = !plan || m.status === "loading";

  return box(
    h("div", { class: "merge-columns" }, before, after),
    choice,
    simulation,
    h("p", { class: "rule-risk" }, "Aviso: ", MERGE_NOTICE),
    h("div", { class: "buttons" }, go, cancel),
  );
}

// -- Sugeridas (D132, R24): calculadas por código; crear una regla la deja inactiva --

function suggestionCard(c: Suggestion, extra: { proposal?: LimitProposal; hiddenReason?: string } = {}): HTMLElement {
  const busy = Boolean(state.suggestionBusy[c.fingerprint]);
  const proposal = extra.proposal;
  const limit = suggestionLimitText(c);
  const outlier = suggestionOutlierWarning(c);
  const supplierNote = suggestionSupplierNote(c);
  const risk = ruleRisk({ id: "", version: 0, action_kind: c.action_kind, conditions: c.conditions, active: false });
  const again = suggestionReappeared(c);
  const seenWarning = suggestionSeenWarning(c);
  const form = state.limitForm[c.fingerprint] ?? { open: false, text: "" };
  const typed = normalizeAmount(form.text);
  // Solo vistas: la regla no se crea con un clic ni sin límite; se abre el formulario con el proveedor y el límite vacío y obligatorio.
  const create = c.evidence.needs_limit
    ? h(
        "button",
        {
          class: "primary",
          type: "button",
          "data-focus-key": `sugg:${c.fingerprint}:open-limit`,
          disabled: busy,
          onclick: () => {
            state.limitForm[c.fingerprint] = { open: !form.open, text: form.text };
            render();
          },
        },
        "Crear regla con un límite…",
      )
    : h(
        "button",
        {
          class: "primary",
          type: "button",
          "data-focus-key": `sugg:${c.fingerprint}:create`,
          disabled: busy,
          onclick: () => resolveSuggestion(c.fingerprint, async () => suggestionAccepted((await api.acceptSuggestion(c.fingerprint, c.shown, null)).name)),
        },
        "Crear regla (inactiva)",
      );
  const limitInput = h("input", {
    type: "text",
    inputmode: "decimal",
    placeholder: "p. ej. 500,00",
    value: form.text,
    "aria-required": "true",
    oninput: (e: Event) => {
      state.limitForm[c.fingerprint] = { open: true, text: (e.target as HTMLInputElement).value };
    },
  }) as HTMLInputElement;
  const limitForm =
    c.evidence.needs_limit && form.open
      ? h(
          "div",
          { class: "confirm", role: "group", "aria-label": "Límite de la regla" },
          h("p", {}, h("strong", {}, "Proveedor: "), c.subject),
          field("Límite (obligatorio)", limitInput, "Importe máximo en euros de una factura de este proveedor que se registraría sin revisión. No sale de las facturas leídas: lo decide usted."),
          form.text.trim() && !typed.ok ? h("p", { class: "meta warn", role: "alert" }, typed.message) : null,
          h(
            "div",
            { class: "buttons" },
            h(
              "button",
              {
                type: "button",
                class: "primary",
                "data-focus-key": `sugg:${c.fingerprint}:create-limit`,
                disabled: busy || !typed.ok,
                onclick: () => {
                  if (!typed.ok) return;
                  const value = typed.value;
                  resolveSuggestion(c.fingerprint, async () => suggestionAccepted((await api.acceptSuggestion(c.fingerprint, c.shown, null, null, value)).name));
                },
              },
              "Crear regla (inactiva)",
            ),
          ),
        )
      : null;
  const seenCount = c.evidence.seen ?? 0;
  const seenState = state.seenList[c.fingerprint];
  const seenDetails =
    seenCount > 0
      ? h(
          "details",
          { class: "sim-examples" },
          h(
            "summary",
            {
              "data-focus-key": `sugg:${c.fingerprint}:seen`,
              onclick: () => {
                if (seenState) return;
                state.seenList[c.fingerprint] = { status: "loading" };
                void api.seenInvoices(c.fingerprint).then(
                  (items) => ((state.seenList[c.fingerprint] = { status: "done", items }), render()),
                  (error) => ((state.seenList[c.fingerprint] = { status: "error", error: String(error) }), render()),
                );
              },
            },
            `Ver las facturas leídas (${seenCount})`,
          ),
          !seenState || seenState.status === "loading"
            ? h("p", { class: "meta" }, "Cargando…")
            : seenState.status === "error"
              ? h("p", { class: "meta warn", role: "alert" }, `No se pudieron cargar: ${seenState.error}`)
              : h(
                  "ul",
                  {},
                  ...(seenState.items ?? []).map((i) => h("li", {}, `${i.document ?? "documento sin nombre"} · fecha de la factura: ${i.date ?? "no consta"} · importe: ${i.amount ? formatEuro(i.amount) : "no consta"}`)),
                ),
        )
      : null;
  const createWithModelLimit = proposal
    ? h(
        "button",
        {
          type: "button",
          "data-focus-key": `sugg:${c.fingerprint}:create-ai`,
          disabled: busy,
          onclick: () =>
            resolveSuggestion(c.fingerprint, async () => {
              const rule = await api.acceptSuggestion(c.fingerprint, proposal.shown, null, proposal.max_total);
              return acceptedWithModelLimit(rule.name, rule.conditions.max_total, proposal.max_total);
            }),
        },
        modelLimitButtonLabel(proposal.max_total),
      )
    : null;
  const restore = extra.hiddenReason
    ? h(
        "button",
        {
          type: "button",
          "data-focus-key": `sugg:${c.fingerprint}:restore`,
          onclick: () => {
            state.restoredFromAi.add(c.fingerprint);
            render();
          },
        },
        "Devolver a la lista principal",
      )
    : null;
  const discard = h(
    "button",
    {
      type: "button",
      "data-focus-key": `sugg:${c.fingerprint}:discard`,
      disabled: busy,
      onclick: () =>
        resolveSuggestion(c.fingerprint, async () => {
          await api.discardSuggestion(c.fingerprint);
          return SUGGESTION_DISCARDED;
        }),
    },
    "Descartar",
  );
  return h(
    "article",
    { class: "finding-card", "aria-label": suggestionSentence(c), "data-focus-key": `sugg:${c.fingerprint}` },
    h("header", { class: "finding-head" }, h("h4", { class: "finding-title" }, suggestionSentence(c)), statusChip(c.evidence.reappeared ? "Vuelve a aparecer" : "Sugerida", c.evidence.reappeared ? "warn" : "info")),
    h("p", {}, h("span", { class: "finding-label" }, "Por qué: "), suggestionWhy(c), " ", helpTip(SUGGESTION_HELP.evidence, "Ayuda sobre qué cuenta como evidencia")),
    seenWarning ? h("p", { class: "warn-text", role: "alert" }, seenWarning, " ", helpTip(SUGGESTION_HELP.seen, "Ayuda sobre las facturas solo vistas")) : null,
    seenDetails,
    limit ? h("p", {}, limit, " ", helpTip(SUGGESTION_HELP.limit, "Ayuda sobre el límite propuesto")) : null,
    outlier ? h("p", { class: "warn-text", role: "alert" }, outlier) : null,
    supplierNote ? h("p", { class: "meta" }, supplierNote) : null,
    again ? h("p", { class: "meta" }, again) : null,
    proposal ? h("p", {}, h("span", { class: "finding-label" }, "Propuesta de la IA: "), modelLimitText(proposal, c)) : null,
    extra.hiddenReason ? h("p", { class: "meta" }, hiddenReasonText(extra.hiddenReason)) : null,
    activationFindings(c.overlaps),
    h("p", { class: "rule-risk" }, "Aviso: ", risk),
    h("p", { class: "meta" }, SUGGESTION_INACTIVE_NOTICE),
    limitForm,
    h("div", { class: "buttons finding-actions" }, create, createWithModelLimit, helpTip(SUGGESTION_HELP.create, "Ayuda sobre crear la regla"), discard, helpTip(SUGGESTION_HELP.discard, "Ayuda sobre descartar"), restore),
  );
}

/** Pide el afinado una sola vez: un doble clic no repite la llamada (el backend tampoco la repetiría). */
function runRefinement() {
  const seen = state.refinePreview;
  if (state.refineBusy || !seen) return;
  state.refineBusy = true;
  state.refineConfirming = false;
  render();
  void api
    .refineSuggestions(seen)
    .then(
      (view) => {
        state.refinement = view;
        state.restoredFromAi.clear();
      },
      (error) => {
        state.notice = { kind: "error", text: String(error) };
      },
    )
    .finally(() => {
      state.refineBusy = false;
      void refresh();
    });
}

/** Lanza una acción del consentimiento una sola vez y refresca. */
function runConsent(work: () => Promise<unknown>) {
  if (state.refineBusy) return;
  state.refineBusy = true;
  state.refineConsentConfirming = false;
  render();
  void work()
    .then(
      () => undefined,
      (error) => {
        state.notice = { kind: "error", text: String(error) };
      },
    )
    .finally(() => {
      state.refineBusy = false;
      void refresh();
    });
}

/** Estado del afinado con IA, el modo del canal, el consentimiento y el botón con el aviso previo. */
function refinementBox(s: Suggestions): HTMLElement | null {
  if (state.refineError) {
    // Un fallo al leer el estado no se oculta: se dice y se puede reintentar o cerrar.
    return h(
      "section",
      { class: "finding-card ai-box", "aria-label": "Afinado con IA", "data-focus-key": "sugg:refine" },
      h("p", { role: "alert", class: "warn-text" }, refineLoadError(state.refineError)),
      h(
        "div",
        { class: "buttons" },
        h("button", { type: "button", "data-focus-key": "refine:retry-load", onclick: () => void refresh() }, "Reintentar"),
        h(
          "button",
          {
            type: "button",
            "data-focus-key": "refine:dismiss-load",
            onclick: () => {
              state.refineError = null;
              render();
            },
          },
          "Cerrar",
        ),
      ),
    );
  }
  const v = state.refinement;
  if (!v) return null;
  const status = refinementStatus(state.refineBusy ? { ...v, status: "running" } : v);
  const button = refineButton(v, state.refinePreview);
  const body: Child[] = [
    h("div", { class: "finding-head" }, h("h4", { class: "finding-title" }, "Afinado con IA"), statusChip(status.title, status.tone === "ok" ? "ok" : status.tone === "warn" ? "warn" : "info")),
    h("p", { class: "meta" }, refineModeText(v.mode, v.min_new_signals, v.max_attempts)),
    ...status.lines.map((line) => h("p", {}, line)),
  ];
  const preview = state.refinePreview;
  const consentText = consentStatusText(v.consent);
  if (consentText) body.push(h("p", { class: "meta" }, consentText));
  const buttons: Child[] = [];
  if (button.shown)
    buttons.push(
      h(
        "button",
        {
          type: "button",
          class: v.consent.required ? "" : "primary",
          "data-focus-key": "refine:open",
          disabled: button.disabled || state.refineBusy || state.refineConfirming,
          onclick: () => {
            state.refineConfirming = true;
            state.refineConsentConfirming = false;
            render();
          },
        },
        "Afinar con IA",
      ),
    );
  if (v.consent.required && v.consent.state === "granted")
    buttons.push(
      h(
        "button",
        { type: "button", "data-focus-key": "refine:withdraw", disabled: state.refineBusy, onclick: () => runConsent(() => api.withdrawRefineConsent()) },
        "Retirar consentimiento",
      ),
    );
  if (v.consent.required && v.consent.state !== "granted")
    buttons.push(
      h(
        "button",
        {
          type: "button",
          class: "primary",
          "data-focus-key": "refine:consent-open",
          disabled: state.refineBusy || !preview || Boolean(preview.blocked),
          onclick: () => {
            state.refineConsentConfirming = true;
            state.refineConfirming = false;
            render();
          },
        },
        "Activar afinado automático",
      ),
    );
  if (buttons.length) body.push(h("div", { class: "buttons" }, ...buttons));
  if (button.why) body.push(h("p", { class: "meta" }, button.why));
  if (state.refineConfirming && preview) {
    const lines = refinePreviewLines(preview);
    body.push(
      h(
        "div",
        { class: "confirm", role: "group", "aria-label": "Qué se enviará a la IA" },
        h("p", {}, h("strong", {}, "Antes de enviar. "), lines.destination),
        lines.fallback ? h("p", { class: "meta" }, lines.fallback) : null,
        h("p", { class: "meta" }, "Datos que se envían:"),
        h("ul", { class: "sim-list" }, ...lines.items.map((item) => h("li", {}, item))),
        ...lines.notes.map((note) => h("p", { class: "meta" }, note)),
        h("p", { class: "meta" }, lines.never),
        h(
          "div",
          { class: "buttons" },
          h("button", { type: "button", class: "primary", "data-focus-key": "refine:send", onclick: runRefinement }, "Enviar y afinar"),
          h(
            "button",
            {
              type: "button",
              "data-focus-key": "refine:cancel",
              onclick: () => {
                state.refineConfirming = false;
                render();
              },
            },
            "Cancelar",
          ),
        ),
      ),
    );
  }
  if (state.refineConsentConfirming && preview) {
    const c = consentLines(preview, v.min_new_signals, v.max_attempts);
    body.push(
      h(
        "div",
        { class: "confirm", role: "group", "aria-label": "Consentimiento del afinado automático" },
        h("p", {}, h("strong", {}, `${c.heading}. `), c.destination),
        ...c.lines.map((line) => h("p", { class: "meta" }, line)),
        h("p", { class: "meta" }, "Datos que se enviarían ahora:"),
        h("ul", { class: "sim-list" }, ...c.items.map((item) => h("li", {}, item))),
        h(
          "div",
          { class: "buttons" },
          h(
            "button",
            { type: "button", class: "primary", "data-focus-key": "refine:consent-accept", onclick: () => runConsent(() => api.grantRefineConsent(preview.provider.provider, preview.provider.model, preview.host)) },
            "Aceptar y activar",
          ),
          h(
            "button",
            {
              type: "button",
              "data-focus-key": "refine:consent-cancel",
              onclick: () => {
                state.refineConsentConfirming = false;
                render();
              },
            },
            "Cancelar",
          ),
        ),
      ),
    );
  }
  if (v.status === "refined") {
    const comment = v.comment ? aiCommentText(v.comment) : "";
    if (comment)
      body.push(
        h("p", { class: "ai-comment" }, h("span", { class: "finding-label" }, `${AI_COMMENT_LABEL}: `), comment),
        // Nota fija y siempre visible: ninguna validación del comentario la sustituye.
        h("p", { class: "meta" }, AI_COMMENT_NOTE),
      );
    const subject = (fp: string) => s.candidates.find((c) => c.fingerprint === fp)?.subject;
    for (const group of v.groups) {
      const names = group.map(subject);
      if (names.every((n): n is string => Boolean(n))) body.push(h("p", { class: "meta" }, h("span", { class: "finding-label" }, "Pregunta de la IA: "), groupQuestion(names)));
    }
    for (const dropped of v.dropped) body.push(h("p", { class: "meta" }, droppedText(dropped)));
  }
  return h("section", { class: "finding-card ai-box", "aria-label": "Afinado con IA", "data-focus-key": "sugg:refine" }, ...body);
}

/** Acepta o descarta una sugerencia una sola vez: un doble clic no repite nada. */
function resolveSuggestion(fingerprint: string, work: () => Promise<string>) {
  if (state.suggestionBusy[fingerprint]) return;
  state.suggestionBusy[fingerprint] = true;
  render();
  void work()
    .then(
      (text) => {
        state.notice = { kind: "ok", text };
      },
      (error) => {
        state.notice = { kind: "error", text: String(error) };
      },
    )
    .finally(() => {
      delete state.suggestionBusy[fingerprint];
      void refresh();
    });
}

function thresholdsBox(t: ThresholdSettings): HTMLElement {
  const row = (which: "invoice" | "reply") => {
    const current = which === "invoice" ? t.invoice : t.reply;
    const id = `threshold-${which}`;
    const input = h("input", {
      id,
      type: "number",
      min: String(t.min),
      max: String(t.max),
      step: "1",
      inputmode: "numeric",
      "data-focus-key": `threshold:${which}`,
      oninput: () => {
        state.thresholdText[which] = input.value;
      },
    }) as HTMLInputElement;
    input.value = state.thresholdText[which] ?? String(current);
    const problem = h("p", { class: "warn-text", role: "alert" });
    const save = h(
      "button",
      {
        type: "button",
        "data-focus-key": `threshold:${which}:save`,
        onclick: async () => {
          const bad = thresholdProblem(input.value, t);
          problem.textContent = bad ?? "";
          if (bad || save.disabled) return;
          save.disabled = true;
          try {
            const saved = await api.setSuggestionThreshold(which, Number(input.value.trim()));
            delete state.thresholdText[which];
            state.notice = { kind: "ok", text: thresholdSaved(which, which === "invoice" ? saved.invoice : saved.reply) };
          } catch (error) {
            state.notice = { kind: "error", text: String(error) };
          }
          await refresh();
        },
      },
      "Guardar",
    ) as HTMLButtonElement;
    return h(
      "div",
      { class: "field" },
      h("div", { class: "field-head" }, h("label", { for: id }, THRESHOLD_LABEL[which]), helpTip(SUGGESTION_HELP.thresholds, `Ayuda sobre el umbral: ${THRESHOLD_LABEL[which].toLowerCase()}`)),
      h("div", { class: "buttons" }, input, save),
      h("p", { class: "meta" }, `Ahora: ${current}. Mínimo ${t.min}, máximo ${t.max}.`),
      problem,
    );
  };
  return h(
    "details",
    { class: "sim-examples", "data-focus-key": "sugg:thresholds" },
    h("summary", {}, "Umbrales para sugerir"),
    row("invoice"),
    row("reply"),
  );
}

function suggestedPanel(): HTMLElement {
  if (state.suggestionsError)
    return h("p", { class: "meta findings-none", role: "alert" }, `No se han podido calcular las sugerencias: ${state.suggestionsError}`);
  const s = state.suggestions;
  if (!s) return h("p", { class: "meta" }, "Calculando sugerencias…");
  const hidden = suggestionsHidden(s.hidden);
  const body: Child[] = [];
  // La IA solo ordena y propone ocultar candidatas que calculó el programa (D134).
  const lists = applyRefinement(s.candidates, state.refinement, state.restoredFromAi);
  const proposalOf = (fingerprint: string) => (state.refinement?.status === "refined" ? state.refinement.limits.find((l) => l.fingerprint === fingerprint) : undefined);
  if (s.candidates.length) {
    if (lists.main.length) body.push(h("div", { class: "finding-cards" }, ...lists.main.map((c) => suggestionCard(c, { proposal: proposalOf(c.fingerprint) }))));
    if (lists.hidden.length)
      body.push(
        h(
          "section",
          { class: "ai-hidden", "aria-label": hiddenBlockTitle(lists.hidden.length), "data-focus-key": "sugg:ai-hidden" },
          h("h4", { class: "finding-title" }, hiddenBlockTitle(lists.hidden.length)),
          h("p", { class: "meta" }, HIDDEN_BLOCK_NOTE),
          h("div", { class: "finding-cards" }, ...lists.hidden.map((x) => suggestionCard(x.candidate, { proposal: proposalOf(x.candidate.fingerprint), hiddenReason: x.reason }))),
        ),
      );
  } else {
    const empty = suggestionsEmpty(s);
    body.push(emptyState(empty.title, empty.hint), empty.lines.length ? h("ul", { class: "sim-list" }, ...empty.lines.map((line) => h("li", {}, line))) : null);
  }
  return h(
    "div",
    {},
    h("p", { class: "intro" }, SUGGESTION_INTRO),
    learnedBatchBox(),
    refinementBox(s),
    ...body,
    hidden ? h("p", { class: "meta" }, hidden) : null,
    thresholdsBox(s.thresholds),
  );
}

/** «Crear regla» desde una revisión: abre el formulario precargado; la regla sigue naciendo inactiva. */
function createRuleButton(task: Task): HTMLElement {
  // Una respuesta de una categoría de solo revisión (D177) no admite regla: el botón se
  // desactiva y dice por qué, sin llegar a pedir nada al servidor.
  const category = task.kind === "email" ? (task.result as { category?: unknown } | null)?.category : null;
  const reviewOnly = typeof category === "string" && REVIEW_ONLY_CATEGORIES.includes(category);
  return h(
    "button",
    {
      type: "button",
      "data-focus-key": `task:${task.id}:create-rule`,
      disabled: reviewOnly,
      title: reviewOnly ? REVIEW_ONLY_TEXT : undefined,
      onclick: async () => {
        try {
          const seed = await api.ruleSeedForTask(task.id);
          state.view = "rules";
          state.notice = { kind: "ok", text: seedNotice(seed) };
          openRuleEditor({ mode: "new" }, draftFromSeed(seed));
        } catch (error) {
          notify("error", error);
        }
      },
    },
    "Crear regla",
  );
}

function findingCard(f: RuleFinding): HTMLElement {
  const t = explainFinding(f, ruleName);
  const rules = f.rules.map((r) => state.rules.find((v) => v.id === r.id)).filter((r): r is RuleView => Boolean(r));
  const actions: Child[] = [];
  if (f.kind === "similar") {
    actions.push(
      h(
        "button",
        {
          type: "button",
          "data-focus-key": `finding:${f.key}:similar`,
          "aria-expanded": String(Boolean(state.similarOpen[f.key])),
          onclick: () => {
            state.similarOpen[f.key] = !state.similarOpen[f.key];
            state.focusAfterRender = `finding:${f.key}:similar`;
            render();
          },
        },
        "¿Es el mismo proveedor?",
      ),
    );
  } else if (f.merge_candidate) {
    actions.push(
      h(
        "button",
        {
          class: "primary",
          type: "button",
          "data-focus-key": `finding:${f.key}:merge`,
          disabled: state.merge?.key === f.key,
          onclick: () => loadMerge(f.key, f.rules.map((r) => r.id), null),
        },
        "Revisar unificación",
      ),
    );
  } else if (f.kind === "defective" && f.effect.code === "sender_missing" && rules[0]) {
    actions.push(h("button", { class: "primary", type: "button", "data-focus-key": `finding:${f.key}:fix`, onclick: () => addSender(rules[0]!) }, "Añadir remitente"));
  } else if (f.kind === "defective" && rules[0] && isKnownAction(rules[0].action_kind)) {
    actions.push(h("button", { type: "button", "data-focus-key": `finding:${f.key}:fix`, onclick: () => openRuleEditor({ mode: "edit", id: rules[0]!.id }, undefined) }, "Editar la regla"));
  }
  for (const r of rules) {
    actions.push(h("button", { class: "link", type: "button", "data-focus-key": `finding:${f.key}:show:${r.id}`, onclick: () => showRule(r.id) }, `Ver «${r.name}»`));
  }
  const similar =
    f.kind === "similar" && state.similarOpen[f.key]
      ? h(
          "div",
          { class: "sim-box", role: "note" },
          h("p", {}, similarClarification()),
          h(
            "div",
            { class: "buttons" },
            ...rules.map((r) => h("button", { type: "button", "data-focus-key": `finding:${f.key}:edit:${r.id}`, onclick: () => openRuleEditor({ mode: "edit", id: r.id }, undefined) }, `Editar «${r.name}»`)),
          ),
        )
      : null;
  return h(
    "article",
    { class: "finding-card", "aria-label": t.title, "data-focus-key": `finding:${f.key}` },
    h("header", { class: "finding-head" }, h("h4", { class: "finding-title" }, t.title), statusChip(t.badge.text, t.badge.tone)),
    h("p", {}, t.what),
    h("p", {}, h("span", { class: "finding-label" }, "Por qué: "), t.why),
    h("p", {}, h("span", { class: "finding-label" }, "Consecuencia: "), t.consequence),
    t.timing ? h("p", { class: "meta" }, t.timing) : null,
    t.tasks ? h("p", { class: "meta" }, t.tasks) : null,
    similar,
    mergePanel(f),
    h("div", { class: "buttons finding-actions" }, ...actions),
  );
}

/** Bloque «Revisión de reglas» encima de la lista: una tarjeta por hallazgo o una línea discreta. */
function findingsBlock(): HTMLElement | null {
  if (state.findingsError)
    return h("p", { class: "meta findings-none", role: "alert" }, `No se ha podido revisar si hay reglas que se contradigan: ${state.findingsError}`);
  if (!state.rules.length) return null;
  const findings = sortFindings(state.findings);
  if (!findings.length) return h("p", { class: "meta findings-none" }, NO_FINDINGS);
  return h(
    "section",
    { class: "card findings", "aria-label": "Revisión de reglas" },
    h("h3", {}, "Revisión de reglas"),
    h(
      "p",
      { class: "meta" },
      `${findings.length === 1 ? "Hay 1 aviso" : `Hay ${findings.length} avisos`} sobre sus reglas. Las reglas solo autorizan; si dos reglas activas coinciden con una tarea, esa tarea va a revisión. Nada se cambia sin que usted lo acepte.`,
    ),
    h("div", { class: "finding-cards" }, ...findings.map(findingCard)),
  );
}

function rulesView() {
  const panel =
    state.rulesTab === "mine"
      ? h(
          "div",
          {},
          h(
            "p",
            { class: "intro" },
            "Por defecto todo se revisa. Una regla activa permite enviar o registrar sin revisión solo cuando es la única que coincide y los datos no tienen incidencias. Cada cambio crea una versión nueva y queda en el historial.",
          ),
          state.ruleEditor
            ? ruleForm()
            : sectionHeader(
                "Reglas guardadas",
                h("button", { class: "primary", "data-focus-key": "rules:new", onclick: () => openRuleEditor({ mode: "new" }, undefined) }, "Nueva regla"),
              ),
          learnedRulesBox(),
          findingsBlock(),
          rulesList(),
        )
      : state.rulesTab === "suggested"
        ? suggestedPanel()
        : categoriesPanel();
  return h(
    "section",
    { class: "page" },
    h("h2", {}, "Reglas"),
    tabs({
      label: "Secciones de reglas",
      items: [
        { key: "mine", label: "Mis reglas" },
        { key: "suggested", label: "Sugeridas", count: suggestionsTabCount(state.suggestions, Boolean(state.suggestionsError)) },
        { key: "categories", label: "Categorías" },
      ],
      active: state.rulesTab,
      panelId: "rules-panel",
      onSelect: setRulesTab,
    }),
    h("div", { role: "tabpanel", id: "rules-panel", class: "tabpanel", "aria-label": state.rulesTab === "mine" ? "Mis reglas" : state.rulesTab === "suggested" ? "Sugeridas" : "Categorías" }, panel),
  );
}

// -- Por qué está aquí (R22) --

const OUTCOME_CHIP: Record<CandidateOutcome, [string, Tone]> = {
  matched: ["Coincidía", "ok"],
  would_apply: ["Habría aplicado", "warn"],
  needs_sender: ["Falta el remitente", "warn"],
  no_match: ["No coincidía", "neutral"],
};

/** Bloque desplegable con las reglas que se comprobaron para esta tarea y por qué no se automatizó. */
function whyHere(task: Task): HTMLElement {
  const info = explainEvaluation(task.result?.evaluation, true);
  const details = h(
    "details",
    {
      class: "why",
      ontoggle: () => (state.whyOpen[task.id] = details.open),
    },
    h("summary", { "data-focus-key": `why:${task.id}` }, "Por qué está aquí"),
    !info
      ? h("p", { class: "meta" }, NO_EVALUATION)
      : h(
          "div",
          {},
          h("p", {}, "Resultado: ", info.headline),
          info.candidates.length
            ? h(
                "ul",
                { class: "why-list" },
                ...info.candidates.map((c) => {
                  const rule = state.rules.find((r) => r.id === c.id);
                  const [label, tone] = OUTCOME_CHIP[c.outcome];
                  return h(
                    "li",
                    {},
                    h("div", { class: "why-head" }, h("strong", {}, rule?.name ?? c.id), statusChip(label, tone), rule ? h("button", { class: "link", type: "button", "data-focus-key": `why:${task.id}:${c.id}`, onclick: () => showRule(c.id) }, "Ver regla") : h("span", { class: "missing" }, "(la regla ya no consta)")),
                    rule ? h("p", { class: "meta" }, ruleSentence(rule)) : null,
                    h("p", {}, c.text),
                    rule && rule.version !== c.version ? h("p", { class: "meta" }, `Se comprobó la versión ${c.version}; hoy la regla va por la ${rule.version}.`) : null,
                  );
                }),
              )
            : h("p", { class: "meta" }, "No había reglas de este tipo que comprobar."),
        ),
  ) as HTMLDetailsElement;
  details.open = Boolean(state.whyOpen[task.id]);
  return details;
}

function historyView() {
  const filter = state.auditFilter;
  const entries = [...state.audit].reverse().filter((e) => auditMatches(e, filter));
  const options: Array<[string, string]> = [
    ["", "Todo el historial"],
    [APPROVED_BY_PERSON, "Aprobadas por una persona"],
    [AUTHORIZED_BY_RULE, "Autorizadas por una regla"],
  ];
  const select = h(
    "select",
    {
      "data-focus-key": "history:filter",
      onchange: () => ((state.auditFilter = select.value), render()),
    },
    ...options.map(([value, label]) => h("option", { value }, label)),
    h(
      "optgroup",
      { label: "De una regla" },
      ...state.rules.map((r) => h("option", { value: r.id }, r.archived_at ? `${r.name} (archivada)` : r.name)),
      ...(filter && ![...options.map((o) => o[0]), ...state.rules.map((r) => r.id)].includes(filter) ? [h("option", { value: filter }, filter)] : []),
    ),
  ) as HTMLSelectElement;
  select.value = filter;
  return h(
    "section",
    { class: "page" },
    h("h2", {}, "Historial"),
    h("p", { class: "intro" }, "Acciones ejecutadas en las cuentas de correo y el simulador de administración, y cambios de reglas, con su autorización."),
    h(
      "div",
      { class: "filters", role: "search" },
      h("div", { class: "field" }, h("div", { class: "field-head" }, h("label", {}, "Mostrar", select), helpTip(FIELD_HELP.history_filter, "Ayuda sobre el filtro del historial"))),
      filter ? h("div", { class: "field" }, h("button", { "data-focus-key": "history:clear", onclick: () => ((state.auditFilter = ""), render()) }, "Quitar filtro")) : null,
    ),
    state.audit.length
      ? entries.length
        ? dataTable(
            [{ label: "Tarea", numeric: true }, { label: "Acción" }, { label: "Resultado" }, { label: "Autorización" }, { label: "Detalle" }],
            entries.map((e) => [
              e.task_id ?? missing("Sin tarea"),
              ACTION_LABEL[e.action] ?? e.action,
              auditStatusLabel(e.action, e.status),
              e.authorization ?? missing(),
              h("div", { class: "detail-cell" }, (e.action === "sender_preference" ? preferenceAuditText(e.detail) : e.action.startsWith("assistant_") ? assistantAuditText(e.action, e.detail, e.status) : e.detail) ?? missing()),
            ]),
          )
        : emptyState("Ninguna entrada coincide con el filtro", "Pruebe con otra regla o quite el filtro.")
      : emptyState("Aún no hay acciones registradas", "Aquí aparecerá cada acción ejecutada, con su autorización."),
  );
}

// ---- Cuentas de correo (R13) ----

function accountId(messageId: string | undefined): string {
  return messageId?.includes(":") ? messageId.split(":")[0]! : "simulador";
}

// Origen de un correo: el guardado en la tarea (D48) o, en tareas anteriores, el de la cuenta
// actual. El simulador es siempre una cuenta simulada, nunca una cuenta «desconectada».
function mailOrigin(saved: MailOrigin | null | undefined, messageId: string | undefined): MailOrigin | null {
  // El nombre actual de la cuenta prevalece (se puede renombrar); el guardado sirve si ya no está.
  const account = state.mail?.accounts.find((a) => a.id === (saved?.account ?? accountId(messageId)));
  if (account) return { account: account.id, provider: account.provider, label: account.label, address: account.address };
  if (saved) return saved;
  if (accountId(messageId) === "simulador")
    return { account: "simulador", provider: "simulator", label: "Correo simulado", address: "administracion@nestadmin.test" };
  return null;
}

const PROVIDER_SHORT: Record<MailProviderKind, string> = {
  simulator: "cuenta simulada",
  google: "Gmail",
  microsoft: "Outlook",
};

function accountShort(origin: MailOrigin | null, id: string): string {
  if (!origin) return `${id} (ya no está en NestAdmin)`;
  return origin.address ? `${origin.label} · ${origin.address}` : origin.label;
}

// «Oficina · admin@empresa.test (Gmail)»; la dirección que falta se dice, no se inventa.
function accountName(messageId: string | undefined, saved?: MailOrigin | null): string {
  const origin = mailOrigin(saved, messageId);
  if (!origin) return `${accountId(messageId)} (cuenta quitada de NestAdmin)`;
  const connected = state.mail?.accounts.some((a) => a.id === origin.account) ?? true;
  return `${origin.label} · ${origin.address ?? "dirección no consta"} (${PROVIDER_SHORT[origin.provider]}${connected ? "" : "; ya no está en NestAdmin"})`;
}

function accountLine(saved: MailOrigin | null | undefined, messageId: string | undefined) {
  const origin = mailOrigin(saved, messageId);
  if (!origin) return h("span", {}, `${accountId(messageId)} `, h("span", { class: "missing" }, "(cuenta quitada de NestAdmin)"));
  const connected = state.mail?.accounts.some((a) => a.id === origin.account) ?? true;
  return h(
    "span",
    {},
    h("strong", {}, origin.label),
    " · ",
    origin.address ?? missing(),
    ` (${PROVIDER_LABEL[origin.provider]}${origin.provider === "simulator" ? ", datos sintéticos" : ""})`,
    connected ? null : h("span", { class: "missing" }, " · ya no está en NestAdmin"),
  );
}

function taskMailOrigin(task: Task): MailOrigin | null {
  const input = safeJson(task.input_ref);
  if (task.kind === "invoice") {
    const origin: DocumentOrigin | undefined = task.result?.origin ?? input.origin;
    return origin?.kind === "email" ? mailOrigin(origin.account, `${origin.account.account}:`) : null;
  }
  const messageId = task.result?.message?.id ?? input.message_id;
  return mailOrigin(task.result?.origin ?? input.origin, messageId);
}

// Clave del filtro por origen: "file" para archivos importados o el ID de la cuenta.
function originKey(task: Task): string {
  const input = safeJson(task.input_ref);
  if (task.kind === "invoice") {
    const origin: DocumentOrigin | undefined = task.result?.origin ?? input.origin;
    return origin?.kind === "email" ? origin.account.account : "file";
  }
  return accountId(task.result?.message?.id ?? input.message_id);
}

function dateText(value: string | number): string {
  const date = typeof value === "number" ? new Date(value * 1000) : new Date(value);
  if (Number.isNaN(date.getTime())) return String(value);
  // Una fecha sin hora (AAAA-MM-DD) se muestra sin hora para no inventarla.
  if (typeof value === "string" && /^\d{4}-\d{2}-\d{2}$/.test(value)) return date.toLocaleDateString("es-ES", { timeZone: "UTC" });
  return date.toLocaleString("es-ES", { dateStyle: "medium", timeStyle: "short" });
}

function fileSize(bytes: number) {
  return bytes < 1024 ? `${bytes} B` : bytes < 1024 ** 2 ? `${(bytes / 1024).toFixed(1).replace(".", ",")} KB` : `${(bytes / 1024 ** 2).toFixed(1).replace(".", ",")} MB`;
}

// Origen de un documento (D49): archivo importado o adjunto de un correo, con su huella.
function documentOrigin(origin: DocumentOrigin | null) {
  const rows: Array<[string, Child]> = [];
  if (!origin) {
    rows.push(["Origen", h("span", { class: "missing" }, "No consta (documento importado antes de registrar el origen)")]);
  } else if (origin.kind === "file") {
    rows.push(["Origen", "Archivo importado"]);
    rows.push(["Archivo", origin.name]);
    rows.push(["Ruta original", origin.original_path ? h("span", { class: "mono" }, origin.original_path) : missing()]);
  } else {
    rows.push(["Origen", "Adjunto de un correo"]);
    rows.push(["Cuenta", accountLine(origin.account, `${origin.account.account}:`)]);
    rows.push(["Remitente", origin.from ?? missing()]);
    rows.push(["Asunto", origin.subject ?? missing()]);
    rows.push(["Adjunto", origin.attachment ?? missing()]);
  }
  if (origin) {
    rows.push(["Importado", origin.imported_at ? dateText(origin.imported_at) : missing()]);
    rows.push([
      "SHA-256",
      h("span", { class: "mono", title: origin.sha256 }, `${origin.sha256.slice(0, 12)}…`, ` · ${fileSize(origin.size)}`),
    ]);
  }
  return h(
    "div",
    { class: "origin-box" },
    h("dl", { class: "origin" }, ...rows.flatMap(([label, value]) => [h("dt", {}, label), h("dd", {}, value)])),
    h("p", { class: "meta" }, "«Abrir original» abre la copia guardada en NestAdmin con el visor del sistema."),
  );
}

// Procedencia de un campo: páginas y línea del texto en que aparece. Si el valor no aparece
// literalmente (el modelo lo reescribió o el OCR lo leyó distinto), se dice así.
function fieldSource(value: string | null | undefined, source: FieldSource | undefined, pageCount: number) {
  if (value == null) return "";
  if (!source) return h("span", { class: "missing" }, "No localizado en el texto; compruébelo en el original");
  const where = pageCount > 1 || source.pages.some((p) => p > 1) ? `pág. ${source.pages.join(", ")}` : "en el texto";
  return h("span", {}, h("strong", {}, where), " · ", h("q", {}, source.line));
}

function needsAttention(account: MailAccount) {
  return account.status !== "connected" || account.last_fetch?.error != null;
}

function issueText(error: { code: string; message: string }) {
  return `${MAIL_ERROR[error.code] ?? error.code}: ${error.message}`;
}

function fetchNotice(report: FetchReport): { kind: "ok" | "error"; text: string } {
  if (report.accounts.length === 0) return { kind: "error", text: "No hay cuentas de correo. Añada una en la sección Cuentas de correo." };
  const failed = report.accounts.filter((a) => a.error);
  const imported = report.accounts.reduce((sum, a) => sum + (a.attachments?.imported ?? 0), 0);
  const skipped = report.accounts.flatMap((a) => a.attachments?.skipped ?? []);
  const attachmentErrors = report.accounts.flatMap((a) => a.attachments?.failed ?? []);
  let added = report.added ? `${report.added} mensajes nuevos en cola` : "No hay mensajes nuevos";
  if (imported) added += `; ${imported} adjuntos importados como documentos`;
  if (skipped.length) added += `; adjuntos no procesables omitidos: ${skipped.join(", ")}`;
  const problems: string[] = failed.map(
    (a) => `no se pudo consultar ${state.mail?.accounts.find((m) => m.id === a.account)?.label ?? a.account} — ${issueText(a.error!)}`,
  );
  if (attachmentErrors.length) {
    problems.push(
      `adjuntos sin importar (se reintentarán en la próxima consulta): ${attachmentErrors.map((e) => `${e.name} — ${issueText(e)}`).join("; ")}`,
    );
  }
  if (!problems.length) return { kind: "ok", text: added };
  return { kind: "error", text: `${added}. ${problems.join(". ")}` };
}

function lastFetch(account: MailAccount) {
  const f = account.last_fetch;
  if (!f) return h("span", { class: "missing" }, "Sin consultar en esta sesión");
  if (f.error) return h("span", { class: "error-text" }, issueText(f.error));
  const a = f.attachments;
  const parts = [`${f.read} leídos, ${f.added} nuevos`];
  if (a?.imported) parts.push(`${a.imported} adjuntos importados`);
  if (a?.failed.length) return h("span", { class: "error-text" }, `${parts.join(", ")}; ${a.failed.length} adjuntos sin importar`);
  return parts.join(", ");
}

function signIn(provider: MailProviderKind, label: string, account?: MailAccount) {
  state.signInUrl = null;
  if (state.mail) state.mail.signing_in = true;
  state.notice = { kind: "ok", text: "Complete el inicio de sesión en el navegador que se ha abierto." };
  render();
  void act(async () => {
    try {
      // Lo mostrado antes de conectar: si falta o cambia, la lectura no arranca sola.
      const o = state.read;
      const shown = !account && o && o.mode === "automatic" && o.limits.fixed !== null ? { provider: o.provider.provider, model: o.provider.model, host: o.host ?? "", volume: o.limits.fixed } : null;
      const done = await api.mailSignIn(provider, label, account?.id, shown);
      state.notice = { kind: "ok", text: account ? `Sesión renovada en «${done.label}»` : `Cuenta «${done.label}» conectada` };
    } finally {
      state.signInUrl = null;
    }
  });
}

function accountRow(a: MailAccount, m: MailView, busy: boolean) {
  const [statusText, statusClass] = ACCOUNT_STATUS[a.status] ?? [a.status, "state-failed"];
  const real = a.provider !== "simulator";
  const configured = a.provider === "google" ? m.google_configured : m.microsoft_configured;
  const renew = real && configured && (a.status !== "connected" || a.last_fetch?.error?.code === "unauthorized");
  return h(
    "li",
    { class: "account" },
    h(
      "div",
      { class: "account-main" },
      h("div", { class: "account-name" }, h("strong", {}, a.label), statusChip(statusText, statusClass === "state-completed" ? "ok" : "error")),
      h("div", { class: "meta" }, PROVIDER_LABEL[a.provider], " · ", a.address ?? missing("dirección no consta")),
      h("div", { class: "meta" }, "Última consulta: ", lastFetch(a)),
      h("div", { class: "meta mono" }, a.id),
    ),
    h(
      "div",
      { class: "row-actions" },
      h(
        "button",
        {
          "data-focus-key": `mail:rename:${a.id}`,
          onclick: () => {
            const label = prompt(`Nombre de la cuenta ${a.address ?? a.id}.\nDéjelo vacío para volver al predeterminado.`, a.label);
            if (label !== null) void act(() => api.renameMailAccount(a.id, label), "Nombre de la cuenta guardado");
          },
        },
        "Renombrar",
      ),
      renew ? h("button", { "data-focus-key": `mail:renew:${a.id}`, disabled: busy, onclick: () => signIn(a.provider, a.label, a) }, "Volver a iniciar sesión") : null,
      h(
        "button",
        {
          class: "danger",
          "data-focus-key": `mail:remove:${a.id}`,
          onclick: async () => {
            const what = real
              ? `Se borrarán los tokens de «${a.label}» de este equipo y NestAdmin dejará de leer esa cuenta. Las tareas ya creadas seguirán visibles, pero no podrán enviar respuestas.`
              : "NestAdmin dejará de leer el correo simulado. Puede volver a añadirlo.";
            if (await ask(`${what} ¿Desconectar?`)) void act(() => api.removeMailAccount(a.id), `Cuenta «${a.label}» desconectada`);
          },
        },
        "Desconectar",
      ),
    ),
  );
}

// ---- Primera lectura (E5, D133) ----

/** Lanza una lectura una sola vez por fuente: un doble clic no repite la llamada (el backend tampoco la repetiría). */
function runRead(source: string, work: () => Promise<InitialReadStatus>) {
  if (state.readBusy[source]) return;
  state.readBusy[source] = true;
  state.readConfirm = null;
  render();
  void work()
    .then(
      (status) => {
        const kind = status.state === "waiting" || status.state === "listing_failed" ? "warn" : "ok";
        state.notice = { kind, text: status.state === "waiting" ? (status.waiting_message ?? "La lectura espera un proveedor de IA confirmado") : status.listing_error ? "No se pudo listar. Puede reanudar la lectura." : `Primera lectura: ${readStateLabel(status.state).toLowerCase()}` };
      },
      (error) => {
        state.notice = { kind: "error", text: String(error) };
      },
    )
    .finally(() => {
      delete state.readBusy[source];
      void refresh();
    });
}

/** Lo que la persona ve antes de pedir la lectura: se le devuelve al backend, que rechaza si algo cambió. */
function expectation(o: InitialReadOverview, volume: number) {
  return { provider: o.provider.provider, model: o.provider.model, host: o.host ?? "", volume };
}

function readStatusBlock(o: InitialReadOverview, status: InitialReadStatus): HTMLElement {
  const lines: HTMLElement[] = [];
  const effects = readEffectsLine(status);
  const remaining = readRemaining(status);
  const summary = readSummary(status, o.suggestions);
  if (status.tasks.total > 0 || status.emails_unclassified > 0 || status.state === "waiting" || status.documents_sent > 0)
    lines.push(h("p", {}, summary, " ", helpTip(SUGGESTIONS_GLOBAL_NOTE, "Ayuda sobre las sugerencias")));
  const held = heldText(status);
  if (held) lines.push(h("p", { class: "meta warn", role: "status" }, held));
  const providers = byProviderText(status);
  if (providers) lines.push(h("p", { class: "meta" }, providers));
  if (status.waiting_message) lines.push(h("p", { class: "meta warn", role: "status" }, status.waiting_message));
  if (effects) lines.push(h("p", { class: status.actions === 0 ? "meta" : "meta warn" }, effects));
  if (remaining) lines.push(h("p", { class: "meta" }, remaining));
  for (const problem of readProblems(status)) lines.push(h("p", { class: "meta warn", role: "alert" }, problem));
  if (status.failures.length)
    lines.push(h("ul", { class: "sim-list" }, ...status.failures.slice(0, 8).map((f) => h("li", {}, readFailureLine(f))), status.failures.length > 8 ? h("li", { class: "meta" }, `y ${status.failures.length - 8} más`) : null));
  for (const note of readNotes(status)) lines.push(h("p", { class: "meta" }, note));
  const started = readStartedWith(status);
  if (started) lines.push(h("p", { class: "meta" }, started));
  return h("div", {}, ...lines);
}

/** Una cuenta o carpeta: estado, resumen y, según el modo, el botón con su aviso previo. */
function readCard(o: InitialReadOverview, src: { source: string; label: string; address: string | null; status: InitialReadStatus }, kind: "account" | "folder", target: string): HTMLElement {
  const status = src.status;
  const busy = Boolean(state.readBusy[src.source]);
  const onDemand = o.mode === "on_demand";
  const volumeText = state.readVolume[src.source] ?? String(status.volume ?? o.limits.default);
  const problem = volumeProblem(volumeText, o.limits);
  const body: Array<HTMLElement | null> = [readStatusBlock(o, status)];
  const accountId = target;
  const startable = canStartRead(status) && (onDemand || status.state !== "not_started");
  if (startable) {
    const volume = h("input", {
      type: "number",
      min: String(o.limits.min),
      max: String(o.limits.max),
      step: "1",
      value: volumeText,
      disabled: busy || status.state !== "not_started",
      oninput: (e: Event) => {
        state.readVolume[src.source] = (e.target as HTMLInputElement).value;
      },
    }) as HTMLInputElement;
    const choosing = onDemand && status.state === "not_started";
    body.push(
      onDemand ? field(`Cuántos ${kind === "account" ? "mensajes" : "archivos"} leer`, volume, READ_HELP.volume) : null,
      problem && choosing ? h("p", { class: "meta warn", role: "alert" }, problem) : null,
      h(
        "div",
        { class: "buttons" },
        h(
          "button",
          {
            type: "button",
            class: "primary",
            "data-focus-key": `read:ask:${src.source}`,
            disabled: busy || Boolean(o.blocked) || (problem !== null && choosing),
            onclick: () => {
              state.readConfirm = { source: src.source, kind, target };
              render();
            },
          },
          readButtonLabel(status),
        ),
      ),
      o.blocked ? h("p", { class: "meta warn", role: "status" }, `${o.blocked_message ?? "Falta configurar la IA"}. No se puede leer hasta entonces.`) : null,
    );
    if (state.readConfirm?.source === src.source) {
      const volumeNumber = choosing ? Number(volumeText) : (status.volume ?? o.limits.fixed ?? Number(volumeText));
      const c = confirmReadLines(o, volumeNumber, kind === "account" ? `la cuenta ${src.label}` : `la carpeta ${folderLabel(src.label)}`);
      body.push(
        h(
          "div",
          { class: "confirm", role: "group", "aria-label": "Qué se enviará a la IA" },
          h("p", {}, h("strong", {}, `${c.heading}. `), c.lines[0]),
          h("p", { class: "meta" }, c.lines[1]),
          h("p", { class: "meta" }, "Qué se envía:"),
          h("ul", { class: "sim-list" }, ...c.sends.map((item) => h("li", {}, item))),
          h("p", { class: "meta" }, READ_HELP.sends),
          h("p", { class: "meta" }, c.never),
          h(
            "div",
            { class: "buttons" },
            h(
              "button",
              {
                type: "button",
                class: "primary",
                "data-focus-key": `read:send:${src.source}`,
                onclick: () =>
                  runRead(src.source, () =>
                    kind === "account"
                      ? api.startInitialRead(accountId, o.limits.fixed === null ? volumeNumber : null, expectation(o, volumeNumber))
                      : api.startInitialReadFolder(target, volumeNumber, expectation(o, volumeNumber)),
                  ),
              },
              "Enviar y leer",
            ),
            h(
              "button",
              {
                type: "button",
                "data-focus-key": `read:cancel:${src.source}`,
                onclick: () => {
                  state.readConfirm = null;
                  render();
                },
              },
              "Cancelar",
            ),
          ),
        ),
      );
    }
  } else if (!onDemand && status.state === "not_started") {
    body.push(h("p", { class: "meta" }, "No se ha leído: en esta versión la primera lectura se hace al conectar una cuenta nueva."));
  }
  if (canStopRead(status) && status.state !== "held" && status.state !== "interrupted") {
    body.push(
      h(
        "div",
        { class: "buttons" },
        h("button", { type: "button", "data-focus-key": `read:stop:${src.source}`, disabled: busy, onclick: () => runRead(src.source, () => api.stopInitialRead(src.source)) }, "Detener lectura"),
        helpTip(READ_HELP.stop, "Ayuda sobre detener la lectura"),
      ),
    );
  }
  if (status.held && status.held.reason !== "removed") {
    const volumeNow = status.volume ?? o.limits.fixed ?? o.limits.default;
    body.push(
      h(
        "div",
        { class: "buttons" },
        h(
          "button",
          { type: "button", class: "primary", "data-focus-key": `read:continue:${src.source}`, disabled: busy || Boolean(o.blocked), onclick: () => runRead(src.source, () => api.continueInitialRead(src.source, expectation(o, volumeNow))) },
          continueButtonLabel(status),
        ),
      ),
    );
  }
  if ((status.tasks.failed > 0 && status.failures.some((f) => f.recoverable)) || status.attachment_failures.length > 0) {
    body.push(
      h(
        "div",
        { class: "buttons" },
        h(
          "button",
          { type: "button", "data-focus-key": `read:retry:${src.source}`, disabled: busy, onclick: () => runRead(src.source, async () => (await api.retryInitialRead(src.source), readStatusNow(src.source))) },
          `Reintentar los errores (${status.failures.filter((f) => f.recoverable).length + status.attachment_failures.length})`,
        ),
        helpTip(READ_HELP.retry, "Ayuda sobre reintentar"),
      ),
    );
  }
  return h(
    "li",
    { class: "account read-card" },
    h("div", { class: "account-main" }, h("div", { class: "account-name" }, h("strong", {}, src.label), statusChip(readStateLabel(status.state), readStateTone(status.state))), src.address ? h("div", { class: "meta" }, src.address) : null, ...body),
  );
}

/** Estado actual de una fuente, para devolverlo tras un reintento. */
async function readStatusNow(source: string): Promise<InitialReadStatus> {
  const overview = await api.initialReadOverview();
  const found = [...overview.accounts, ...overview.folders].find((x) => x.source === source);
  if (!found) throw "La lectura ya no consta";
  return found.status;
}

// ---- Aprender a responder (E7, D167-D175) ----

/** Pone al día el estado de aprender: descarta una vista previa que ya no es la vigente y lee las propuestas de perfil. */
async function syncLearning(accounts: LearningStatus[] | null) {
  state.learning = accounts ?? [];
  for (const a of state.learning) {
    const seen = state.learnPreview[a.account];
    if (seen && (a.state !== "awaiting_consent" || a.input_fp !== seen.input_fp)) {
      delete state.learnPreview[a.account];
      delete state.learnShow[a.account];
    }
  }
  const next: Record<string, LearnedProfileProposal> = {};
  await Promise.all(
    state.learning
      .filter((a) => a.state === "done")
      .map(async (a) => {
        try {
          const p = await api.learnedProfileProposal(a.account);
          if (p) next[a.account] = p;
        } catch (error) {
          state.learnedProposalError[a.account] = String(error);
        }
      }),
  );
  state.learnedProposals = next;
}

/** Lanza una acción de aprender una sola vez por cuenta: un doble clic no repite la llamada. */
function runLearn(account: string, kind: "collect" | "run" | "retry", work: () => Promise<void>) {
  if (state.learnBusy[account]) return;
  state.learnBusy[account] = kind;
  render();
  void work()
    .then(
      () => undefined,
      (error) => {
        state.notice = { kind: "error", text: String(error) };
      },
    )
    .finally(() => {
      delete state.learnBusy[account];
      void refresh();
    });
}

/** Resultado de una ejecución, en una línea para el aviso. */
function learnResultNotice(status: LearningStatus) {
  const phase = learnPhase(status);
  if (phase === "done") state.notice = { kind: "ok", text: "Aprender a responder: terminado" };
  else if (phase === "nothing") state.notice = { kind: "ok", text: "Aprender a responder: nada que aprender" };
  else if (phase === "failed" || phase === "consent_withdrawn") state.notice = { kind: "warn", text: learnFailureLine(status) };
}

/** Recoge de nuevo (el backend lee el buzón por pasos, soltando la aplicación entre uno y otro) y guarda la vista previa vigente. */
function collectPreview(account: string) {
  runLearn(account, "collect", async () => {
    const view = await api.learningPreview(account);
    if (view.preview) {
      state.learnPreview[account] = view.preview;
      state.learnShow[account] = true;
    } else {
      delete state.learnPreview[account];
      delete state.learnShow[account];
      learnResultNotice(view.status);
    }
  });
}

function learnCountsList(a: LearningStatus): HTMLElement | null {
  return a.counts ? h("ul", { class: "sim-list", "aria-label": "Recuentos de la recogida" }, ...learnCountLines(a.counts).map((line) => h("li", {}, line))) : null;
}

/** La tarjeta «Aprender a responder» de una cuenta, con un estado por fase. */
function learnCard(a: LearningStatus): HTMLElement {
  const key = a.account;
  const busy = state.learnBusy[key] ?? null;
  const phase = learnPhase(a, busy);
  const body: Child[] = [];
  const line = learnPhaseLine(phase);
  if (line) body.push(h("p", { class: "meta", role: "status", "aria-live": "polite" }, line));
  if (phase === "collecting" || phase === "starting" || phase === "running") body.push(h("progress", { "aria-label": learnPhaseLabel(phase) }));
  if (phase === "collecting" && !busy && a.state === "collected")
    body.push(h("div", { class: "buttons" }, h("button", { type: "button", "data-focus-key": `learn:${key}:continue`, onclick: () => collectPreview(key) }, "Continuar ahora")));

  if (phase === "awaiting_consent") {
    const preview = state.learnPreview[key] ?? null;
    const shown = preview !== null && Boolean(state.learnShow[key]);
    const external = isExternalDestination(a.destination);
    body.push(learnCountsList(a), h("p", { class: "meta" }, learnDestinationLine(a.destination)));
    if (external) {
      body.push(
        h(
          "div",
          { class: "buttons" },
          h(
            "button",
            {
              type: "button",
              "data-focus-key": `learn:${key}:preview`,
              "aria-expanded": shown ? "true" : "false",
              onclick: () => {
                if (preview) {
                  state.learnShow[key] = !state.learnShow[key];
                  render();
                } else collectPreview(key);
              },
            },
            shown ? PREVIEW_HIDE_BUTTON : PREVIEW_BUTTON,
          ),
        ),
      );
    }
    if (external && shown && preview) {
      body.push(
        h(
          "div",
          { class: "sub-block" },
          h("pre", { class: "mono block learn-preview", tabindex: "0", role: "region", "aria-label": PREVIEW_LABEL }, preview.payload),
          h("p", { class: "meta" }, PREVIEW_NOTE),
          h("h4", {}, NEVER_SENT_HEADING),
          h("ul", { class: "sim-list" }, ...preview.never_sent.map((item) => h("li", {}, item))),
          preview.blocked ? h("p", { class: "meta warn", role: "alert" }, `El envío está bloqueado (${preview.blocked}). Puede usar la IA local.`) : h("p", { class: "meta" }, learnConsentLine(a.destination!)),
        ),
      );
    } else if (external) {
      body.push(h("p", { class: "meta" }, SEE_FIRST));
    }
    const actions: Child[] = [];
    if (external && a.destination) {
      actions.push(
        h(
          "button",
          {
            type: "button",
            class: "primary",
            "data-focus-key": `learn:${key}:send`,
            disabled: !shown || !preview || Boolean(preview.blocked),
            onclick: () => {
              const seen = state.learnPreview[key];
              if (!seen) return;
              runLearn(key, "run", async () => {
                // Consentir y enviar lo que vio, ligado a proveedor, modelo, servidor y huella.
                if (seen.consent.state !== "granted") await api.grantLearningConsent(key, seen);
                learnResultNotice(await api.runLearning(key, { kind: "send", expect: { provider: seen.provider.provider, model: seen.provider.model, host: seen.host ?? "", input_fp: seen.input_fp } }));
              });
            },
          },
          sendButtonLabel(a.destination.provider),
        ),
      );
    }
    actions.push(
      h(
        "button",
        {
          type: "button",
          "data-focus-key": `learn:${key}:local`,
          title: LOCAL_BUTTON_HELP,
          disabled: !a.input_fp,
          onclick: () => {
            const fp = a.input_fp;
            if (fp) runLearn(key, "run", async () => learnResultNotice(await api.runLearning(key, { kind: "use_local", input_fp: fp })));
          },
        },
        external ? LOCAL_BUTTON : "Aprender con la IA local",
      ),
    );
    body.push(h("div", { class: "buttons" }, ...actions), external ? h("p", { class: "meta" }, LOCAL_BUTTON_HELP) : null);
  }

  if (phase === "held") {
    body.push(
      h("p", { class: "meta warn", role: "status" }, learnHeldLine(a)),
      h("div", { class: "buttons" }, h("button", { type: "button", class: "primary", "data-focus-key": `learn:${key}:repreview`, onclick: () => collectPreview(key) }, REPREVIEW_BUTTON)),
    );
  }

  if (phase === "failed" || phase === "consent_withdrawn" || phase === "interrupted") {
    const text = phase === "consent_withdrawn" ? WITHDRAWN_LINE : phase === "interrupted" ? INTERRUPTED_LINE : learnFailureLine(a);
    body.push(
      h("p", { class: "meta warn", role: "alert" }, text),
      h("div", { class: "buttons" }, h("button", { type: "button", class: "primary", "data-focus-key": `learn:${key}:retry`, onclick: () => runLearn(key, "retry", async () => learnResultNotice(await api.retryLearning(key))) }, RETRY_BUTTON)),
    );
  }

  if (phase === "done") {
    const hasProposal = Boolean(state.learnedProposals[key]);
    const rules = state.learnedBatch?.items.length ?? 0;
    body.push(
      h("p", {}, learnDoneLine(a)),
      learnCountsList(a),
      h(
        "div",
        { class: "buttons" },
        hasProposal ? h("button", { type: "button", "data-focus-key": `learn:${key}:go-profile`, onclick: () => ((state.view = "settings"), render()) }, GO_PROFILE) : null,
        rules
          ? h(
              "button",
              {
                type: "button",
                "data-focus-key": `learn:${key}:go-rules`,
                onclick: () => {
                  state.view = "rules";
                  state.rulesTab = "suggested";
                  render();
                },
              },
              GO_RULES,
            )
          : null,
      ),
      hasProposal ? null : h("p", { class: "meta" }, "No hay una propuesta de tono pendiente."),
      rules ? null : h("p", { class: "meta" }, "No hay reglas propuestas."),
    );
  }

  if (phase === "nothing") {
    const n = nothingText(a.counts);
    body.push(h("p", {}, h("strong", {}, n.title)), ...n.lines.map((l, i) => h("p", { class: i ? "meta" : "" }, l)));
  }

  return h(
    "li",
    { class: "account read-card learn-card", "aria-label": `${LEARN_TITLE}: ${a.label}` },
    h("div", { class: "account-main" }, h("div", { class: "account-name" }, h("strong", {}, `${LEARN_TITLE}: ${a.label}`), statusChip(learnPhaseLabel(phase), learnPhaseTone(phase))), a.address ? h("div", { class: "meta" }, a.address) : null, ...body),
  );
}

/** Bloque «Aprendizaje continuo» (D178): interruptor global y, por buzón, solo recuentos y el motivo de un fallo. */
function continuousBlock(): HTMLElement | null {
  const c = state.continuous;
  if (!c) return state.continuousError ? h("p", { class: "meta warn", role: "alert" }, `No se pudo leer el aprendizaje continuo: ${state.continuousError}`) : null;
  const now = Date.now();
  const toggle = h("input", {
    type: "checkbox",
    id: "continuous-toggle",
    "data-focus-key": "learn:continuous:toggle",
    checked: c.enabled,
    disabled: state.continuousBusy,
    onchange: (event: Event) => {
      const input = event.target as HTMLInputElement;
      if (state.continuousBusy) return;
      state.continuousBusy = true;
      const wanted = input.checked;
      void api
        .setContinuousLearning(wanted)
        .then(
          (next) => {
            state.continuous = next;
          },
          (error) => {
            // Falla: la casilla vuelve a como estaba.
            input.checked = !wanted;
            state.notice = { kind: "error", text: String(error) };
          },
        )
        .finally(() => {
          state.continuousBusy = false;
          render();
        });
    },
  }) as HTMLInputElement;
  return h(
    "div",
    { class: "sub-block continuous-block", "aria-label": CONTINUOUS_TITLE },
    h("h3", {}, CONTINUOUS_TITLE),
    h("p", { class: "intro" }, CONTINUOUS_INTRO),
    h("p", {}, h("label", { for: "continuous-toggle" }, toggle, " ", CONTINUOUS_TOGGLE)),
    c.enabled
      ? h(
          "ul",
          { class: "accounts" },
          ...c.mailboxes.map((m) =>
            h(
              "li",
              { class: "account", "aria-label": `${CONTINUOUS_TITLE}: ${m.label}` },
              h("div", { class: "account-main" }, h("strong", {}, m.label), m.address ? h("div", { class: "meta" }, m.address) : null, ...continuousMailboxLines(m, c.every_minutes, now).map((line) => h("p", { class: "meta", role: "status" }, line))),
            ),
          ),
        )
      : h("p", { class: "meta", role: "status" }, CONTINUOUS_OFF),
    h("p", { class: "meta" }, continuousLimitNote(c.window_days)),
  );
}

/** Bloque de aprender de la sección de lectura; sin cuentas con datos, no se pinta. */
function learnSection(): HTMLElement | null {
  if (state.learningError)
    return h("div", {}, sectionHeader(LEARN_TITLE), h("p", { class: "meta warn", role: "alert" }, `No se pudo leer el estado de aprender a responder: ${state.learningError}`));
  const anyMailbox = state.learning.length > 0 || (state.continuous?.mailboxes.length ?? 0) > 0;
  if (!anyMailbox) return null;
  const continuous = continuousBlock();
  return h("div", { "aria-label": LEARN_TITLE }, sectionHeader(LEARN_TITLE), h("p", { class: "intro" }, LEARN_INTRO), h("ul", { class: "accounts" }, ...state.learning.map(learnCard)), continuous);
}

// -- Propuesta de perfil (D171) --

function runProposal(account: string, work: () => Promise<void>) {
  if (state.learnedProposalBusy[account]) return;
  state.learnedProposalBusy[account] = true;
  delete state.learnedProposalError[account];
  render();
  void work()
    .then(
      () => undefined,
      (error) => {
        state.learnedProposalError[account] = String(error);
        state.notice = { kind: "error", text: String(error) };
      },
    )
    .finally(() => {
      delete state.learnedProposalBusy[account];
      void refresh();
    });
}

/** Antes y después del tono propuesto, con «Aplicar al perfil» y «Descartar». El motivo de un rechazo se muestra. */
function learnedProposalBox(p: LearnedProfileProposal): HTMLElement {
  const busy = Boolean(state.learnedProposalBusy[p.account]);
  const error = state.learnedProposalError[p.account];
  const side = (title: string, text: string) => h("div", { class: "learn-side" }, h("h5", {}, title), h("p", {}, text.trim() ? text : PROPOSAL_EMPTY));
  return h(
    "div",
    { class: "confirm learn-proposal", role: "group", "aria-label": PROPOSAL_TITLE },
    h("p", {}, h("strong", {}, PROPOSAL_TITLE)),
    h("p", { class: "meta" }, PROPOSAL_INTRO),
    h("div", { class: "learn-diff" }, side(PROPOSAL_BEFORE, p.before.tone), side(PROPOSAL_AFTER, p.after.tone)),
    p.blocking.length ? h("p", { class: "meta warn", role: "alert" }, proposalBlockedLine(p.blocking)) : null,
    error ? h("p", { class: "meta warn", role: "alert" }, error) : null,
    h("p", { class: "meta" }, PROPOSAL_NOTE),
    h(
      "div",
      { class: "buttons" },
      h(
        "button",
        {
          type: "button",
          class: "primary",
          "data-focus-key": `learn:${p.account}:proposal-apply`,
          disabled: busy || p.blocking.length > 0,
          onclick: () =>
            runProposal(p.account, async () => {
              const saved = await api.acceptLearnedProfile(p.account, p.fp);
              state.profileDraft = null;
              state.profileWarnings = saved.warnings;
              state.notice = { kind: "ok", text: savedNotice(saved) };
            }),
        },
        PROPOSAL_APPLY,
      ),
      h("button", { type: "button", "data-focus-key": `learn:${p.account}:proposal-dismiss`, disabled: busy, onclick: () => runProposal(p.account, () => api.dismissLearnedProfile(p.account)) }, PROPOSAL_DISMISS),
    ),
  );
}

// -- Reglas aprendidas (D172-D174) --

/** La tanda de reglas aprendidas: una casilla por regla (marcadas) y un único botón para aceptar y activar. */
function learnedBatchBox(): HTMLElement | null {
  if (state.learnedBatchError) return h("p", { class: "meta warn", role: "alert" }, `No se pudieron leer las reglas aprendidas: ${state.learnedBatchError}`);
  const b = state.learnedBatch;
  if (!b) return null;
  if (!b.items.length) return state.learning.some((a) => a.state === "done") ? h("p", { class: "meta" }, learnedBatchEmpty()) : null;
  const marked = () => b.items.filter((i) => !state.learnedOff[i.id]).length;
  const accept = h(
    "button",
    { type: "button", class: "primary", "data-focus-key": "learn:batch-accept", disabled: state.learnedBusy || marked() === 0, onclick: () => void acceptLearned(b) },
    learnedAcceptButton(marked()),
  ) as HTMLButtonElement;
  const none = h("p", { class: "meta", role: "status", "aria-live": "polite" }, marked() === 0 ? NONE_MARKED : "");
  const rows = b.items.map((item) => {
    const id = `learned-${item.id}`;
    const failed = state.learnedFailedId === item.id;
    const box = h("input", {
      type: "checkbox",
      id,
      "data-focus-key": `learn:batch:${item.id}`,
      checked: !state.learnedOff[item.id],
      onchange: (e: Event) => {
        state.learnedOff[item.id] = !(e.target as HTMLInputElement).checked;
        if (state.learnedFailedId === item.id) state.learnedFailedId = null;
        accept.textContent = learnedAcceptButton(marked());
        accept.disabled = state.learnedBusy || marked() === 0;
        none.textContent = marked() === 0 ? NONE_MARKED : "";
      },
    });
    const sentence = ruleSentence({ id: "", version: 0, action_kind: "send_draft", conditions: item.conditions, active: false, any_sender_ack_at: null });
    return h(
      "li",
      { class: `learn-rule${failed ? " is-failed" : ""}` },
      box,
      h("label", { for: id }, h("span", { class: "rule-sentence" }, sentence), h("span", { class: "meta" }, evidenceText(item, categoryLabel))),
    );
  });
  const m = state.learnedMessage;
  return h(
    "section",
    { class: "learn-batch", "aria-label": BATCH_TITLE },
    h("h3", {}, BATCH_TITLE),
    h("p", { class: "intro" }, learnedBatchIntro(b.items.length)),
    h("ul", { class: "learn-rules" }, ...rows),
    h("div", { class: "notice info", role: "note" }, h("span", {}, rodajeNotice(b.rodaje_sends))),
    m ? h("div", { class: `notice ${m.kind}`, role: m.kind === "error" ? "alert" : "status" }, h("span", {}, m.text)) : null,
    h("div", { class: "buttons" }, accept),
    none,
  );
}

async function acceptLearned(batch: LearnedBatch) {
  const ids = batch.items.filter((i) => !state.learnedOff[i.id]).map((i) => i.id);
  if (!ids.length || state.learnedBusy) return;
  state.learnedBusy = true;
  state.learnedMessage = null;
  state.learnedFailedId = null;
  render();
  try {
    const out: LearnedAcceptOutcome = await api.acceptAndActivateLearned(batch.batch_fp, ids);
    if (out.failed) {
      state.learnedFailedId = out.failed.id;
      state.learnedMessage = { kind: "error", text: batchFailureLine(out.failed) };
    } else {
      state.learnedMessage = null;
      state.notice = { kind: "ok", text: learnedAcceptedLine(out, batch.rodaje_sends) };
    }
  } catch (error) {
    // Huella caducada o sugerencia que ya no está vigente: se recarga la tanda y se avisa.
    state.learnedMessage = { kind: "warn", text: batchStaleLine(String(error)) };
  } finally {
    state.learnedBusy = false;
    await refresh();
  }
}

/** «Desactivar las aprendidas» con su confirmación; solo aparece si hay reglas aprendidas activas. */
function learnedRulesBox(): HTMLElement | null {
  const n = state.rules.filter((r) => r.origin?.evidence_origin === "learned" && r.active && !r.archived_at).length;
  if (!n) return null;
  if (!state.learnedDeactivating)
    return h("div", { class: "buttons" }, h("button", { type: "button", "data-focus-key": "learn:deactivate", onclick: () => ((state.learnedDeactivating = true), render()) }, DEACTIVATE_BUTTON));
  return h(
    "div",
    { class: "confirm", role: "group", "aria-label": DEACTIVATE_BUTTON },
    h("p", {}, deactivateConfirm(n)),
    h(
      "div",
      { class: "buttons" },
      h(
        "button",
        {
          type: "button",
          class: "primary",
          "data-focus-key": "learn:deactivate-confirm",
          onclick: () => {
            state.learnedDeactivating = false;
            void act(async () => {
              const out = await api.deactivateLearnedRules();
              state.notice = { kind: "ok", text: deactivatedLine(out.deactivated.length) };
            });
          },
        },
        "Desactivar",
      ),
      h("button", { type: "button", "data-focus-key": "learn:deactivate-cancel", onclick: () => ((state.learnedDeactivating = false), render()) }, "Cancelar"),
    ),
  );
}

async function chooseReadFolder() {
  try {
    const path = await api.pickReadFolder();
    if (!path) return;
    state.readFolder = await api.initialReadFolderPreview(path);
    render();
  } catch (error) {
    state.notice = { kind: "error", text: String(error) };
    render();
  }
}

/** Sección «Primera lectura» de Cuentas de correo: qué hace, a quién se envía, y el estado de cada fuente. */
function readSection(): HTMLElement {
  if (state.readError)
    return h("section", { class: "section" }, sectionHeader("Primera lectura"), h("p", { class: "meta warn", role: "alert" }, `No se pudo leer el estado de la primera lectura: ${state.readError}`));
  const o = state.read;
  if (!o) return h("section", { class: "section" }, sectionHeader("Primera lectura"), h("p", { class: "meta" }, "Cargando…"));
  const accounts = o.accounts.map((a) => readCard(o, a, "account", a.target));
  const removedCards = o.removed.map((a) => readCard(o, a, "account", a.target));
  const folderPreview = state.readFolder;
  const folders = o.folders.map((f) => readCard(o, f, "folder", f.label));
  const previewSource = folderPreview && !o.folders.some((f) => f.source === folderPreview.source) ? folderPreview : null;
  return h(
    "section",
    { class: "section", "aria-label": "Primera lectura" },
    sectionHeader("Primera lectura", helpTip(READ_HELP.apart, "Ayuda sobre la primera lectura")),
    h("p", { class: "intro" }, `Se lee ${readVolumeText(o)}. Solo salen los documentos que parecen una factura, completos; en esta lectura no sale ningún correo.`),
    h("p", { class: "meta" }, o.blocked ? `No hay proveedor de IA confirmado (${o.blocked_message ?? "falta configurar la IA"}): no se enviará nada hasta que lo haya.` : readDestination(o)),
    h("p", { class: "meta warn" }, recentMailWarning(o)),
    h("details", { class: "sim-examples" }, h("summary", { "data-focus-key": "read:detail" }, "Ver el detalle de lo que sale y lo que no"), ...connectDetails(o).map((line) => h("p", { class: "meta" }, line)), h("p", { class: "meta" }, READ_HELP.sends)),
    o.accounts.length ? h("ul", { class: "accounts" }, ...accounts) : h("p", { class: "meta" }, "No hay cuentas conectadas: no hay nada que leer."),
    learnSection(),
    removedCards.length ? h("div", {}, sectionHeader("Cuentas desconectadas"), h("p", { class: "meta" }, "Su lectura se canceló al desconectarlas; lo ya leído sigue constando."), h("ul", { class: "accounts" }, ...removedCards)) : null,
    o.mode === "on_demand"
      ? h(
          "div",
          {},
          sectionHeader("Carpeta de archivos"),
          h("p", { class: "meta" }, `Lee ${readVolumeText(o, "folder")}. Una carpeta se lee una sola vez; los archivos que ya conoce NestAdmin (por su contenido) no se vuelven a leer.`),
          h("div", { class: "buttons" }, h("button", { type: "button", "data-focus-key": "read:folder", onclick: () => void chooseReadFolder() }, "Elegir carpeta…")),
          previewSource
            ? h(
                "ul",
                { class: "accounts" },
                readCard(o, { source: previewSource.source, label: previewSource.path, address: `${previewSource.files} ${previewSource.files === 1 ? "archivo admitido" : "archivos admitidos"} en la carpeta`, status: previewSource.status }, "folder", previewSource.path),
              )
            : null,
          folders.length ? h("ul", { class: "accounts" }, ...folders) : null,
        )
      : null,
  );
}

function mailView() {
  const m = state.mail;
  if (!m) return h("section", { class: "page" }, h("p", {}, "Cargando…"));
  const label = h("input", { name: "label", maxlength: "60", placeholder: "p. ej. Facturación" }) as HTMLInputElement;
  const limit = h("input", { type: "number", min: "1", max: "500", step: "1", value: String(m.limit) }) as HTMLInputElement;
  const busy = m.signing_in;
  const providerButton = (provider: "google" | "microsoft", configured: boolean) =>
    h(
      "button",
      {
        class: "primary",
        "data-focus-key": `mail:connect:${provider}`,
        disabled: busy || !configured,
        title: configured ? undefined : "El editor no ha configurado este proveedor en esta instalación",
        onclick: () => signIn(provider, label.value),
      },
      `Conectar ${PROVIDER_LABEL[provider]}`,
    );
  const unconfigured = [
    !m.google_configured ? "Google" : null,
    !m.microsoft_configured ? "Microsoft" : null,
  ].filter(Boolean);
  return h(
    "section",
    { class: "page" },
    h("h2", {}, "Cuentas de correo"),
    h(
      "p",
      { class: "intro" },
      "Cada cuenta inicia sesión en el navegador del sistema y sus tokens se guardan en el Administrador de credenciales de Windows. El correo solo viaja entre este equipo y el proveedor de la cuenta. Ninguna respuesta se envía sin su aprobación o una regla activa.",
    ),
    m.clients_error ? h("div", { class: "notice error", role: "alert" }, h("span", {}, m.clients_error)) : null,
    m.accounts.length
      ? h("ul", { class: "accounts" }, ...m.accounts.map((a) => accountRow(a, m, busy)))
      : emptyState("No hay cuentas de correo", "Conecte una cuenta de Google o Microsoft para consultar su bandeja de entrada."),
    h(
      "section",
      { class: "section" },
      sectionHeader("Añadir cuenta"),
      h(
        "p",
        { class: "intro" },
        "Se abrirá el navegador para que inicie sesión y conceda permiso de lectura, borradores y envío. NestAdmin espera la respuesta en este equipo (dirección de bucle local: localhost o 127.0.0.1) durante 5 minutos.",
      ),
      h(
        "div",
        { class: state.read ? "notice info" : "notice warn", role: "note", "aria-label": "Qué pasará al conectar" },
        h(
          "span",
          {},
          ...connectNotice(state.read, state.readError).flatMap((line, i) => [i ? h("br", {}) : null, line]),
          state.read ? h("details", {}, h("summary", { "data-focus-key": "mail:notice-detail" }, "Ver el detalle"), ...connectDetails(state.read).map((line) => h("p", { class: "meta" }, line))) : null,
        ),
      ),
      h("div", { class: "row" }, h("label", {}, "Nombre de la cuenta (opcional)", label)),
      h(
        "div",
        { class: "buttons" },
        providerButton("google", m.google_configured),
        providerButton("microsoft", m.microsoft_configured),
        m.accounts.some((a) => a.provider === "simulator")
          ? null
          : h("button", { "data-focus-key": "mail:add-simulated", disabled: busy, onclick: () => void act(() => api.addSimulatedMailAccount(), "Correo simulado añadido") }, "Añadir correo simulado"),
      ),
      busy
        ? h(
            "div",
            { class: "notice ok", role: "status" },
            h(
              "span",
              {},
              "Esperando al navegador… ",
              state.signInUrl ? h("span", {}, "Si no se abrió, copie esta dirección en su navegador: ", h("span", { class: "mono" }, state.signInUrl)) : null,
            ),
            h("button", { "data-focus-key": "mail:cancel", onclick: () => void act(() => api.mailSignInCancel()) }, "Cancelar"),
          )
        : null,
      unconfigured.length
        ? h(
            "p",
            { class: "meta" },
            `${unconfigured.join(" y ")}: esta instalación no incluye el cliente OAuth del editor (archivo correo-oauth.json), así que no se puede iniciar sesión.`,
          )
        : null,
    ),
    preferencesSection(),
    readSection(),
    h(
      "section",
      { class: "section" },
      sectionHeader("Mensajes por consulta"),
      h("p", { class: "intro" }, "Cada vez que pulse «Consultar correo», NestAdmin lee como mucho este número de mensajes recientes de cada cuenta (de 1 a 500)."),
      h("div", { class: "row" }, h("label", {}, "Límite por cuenta", limit)),
      h(
        "button",
        {
          "data-focus-key": "mail:save-limit",
          onclick: () => void act(() => api.setMailLimit(Number(limit.value)), `Límite guardado: ${limit.value} mensajes por cuenta`),
        },
        "Guardar límite",
      ),
    ),
  );
}

// ---- Proveedor de IA (D100–D102, D122) ----

type Kind = ProviderChoice["provider"];

/// Orden de la lista: el externo por defecto primero y la IA local la última.
const KIND_ORDER: Kind[] = ["ollama_cloud", "openai_compatible", "anthropic", "ollama"];
const KIND_NAME: Record<Kind, string> = {
  ollama_cloud: "Ollama cloud",
  openai_compatible: "Compatible con OpenAI",
  anthropic: "Claude · Anthropic",
  ollama: "IA local (Ollama)",
};
const KIND_NOTE: Record<Kind, string> = {
  ollama_cloud: "externo, por defecto",
  openai_compatible: "externo; solo https",
  anthropic: "externo",
  ollama: "no sale nada del equipo",
};
const LOCAL_URL = "http://127.0.0.1:11434";

/// Estado de cada bloque de proveedor. Vive fuera de la vista para que un repintado no borre lo
/// escrito; la clave tecleada solo está aquí hasta que el proveedor responde y se guarda.
interface Block {
  key: string;
  url: string;
  /// Modelo elegido en la lista ("" si no hay ninguno elegido).
  model: string;
  models: string[] | null;
  loading: boolean;
  listError: string | null;
  /// Hay clave guardada para este proveedor (y host); `null` si aún no se ha consultado.
  saved: boolean | null;
  consent: boolean;
  busy: boolean;
  outcome: { kind: "ok" | "error"; text: string } | null;
  opened: boolean;
}

interface BlockOptions {
  /// Carga la lista al abrir si ya hay clave guardada (por defecto, solo el proveedor activo).
  auto?: boolean;
  /// Se llama cuando el proveedor quedó elegido y la prueba respondió bien.
  onDone?: () => void;
}

const blocks: Partial<Record<Kind, Block>> = {};

function resetBlocks() {
  for (const kind of KIND_ORDER) delete blocks[kind];
}

function block(kind: Kind): Block {
  let b = blocks[kind];
  if (!b) {
    const c = state.settings?.choice;
    const same = c && c.provider === kind ? c : null;
    b = {
      key: "",
      url: same && "base_url" in same ? same.base_url : kind === "ollama" ? LOCAL_URL : "",
      model: "",
      models: null,
      loading: false,
      listError: null,
      saved: null,
      consent: false,
      busy: false,
      outcome: null,
      opened: false,
    };
    blocks[kind] = b;
  }
  return b;
}

/// Modelo guardado de este proveedor, si es el elegido actualmente.
function savedModel(kind: Kind): string | null {
  const c = state.settings?.choice;
  return c && c.provider === kind ? c.model : null;
}

function choiceFor(kind: Kind, model: string): ProviderChoice {
  const b = block(kind);
  switch (kind) {
    case "ollama_cloud":
      return { provider: kind, model };
    case "anthropic":
      return { provider: kind, model };
    case "openai_compatible":
      return { provider: kind, base_url: b.url.trim(), model };
    case "ollama":
      return { provider: kind, base_url: b.url.trim() || LOCAL_URL, model };
  }
}

/// Problema visible de la URL base del proveedor compatible; el adaptador valida el resto.
function compatProblem(b: Block): string | null {
  const url = b.url.trim();
  if (!url) return "Indique la URL base del servicio.";
  if (!/^https:\/\//i.test(url)) return "Solo se admite una URL https://.";
  return null;
}

const LEAVES =
  "El contenido que la tarea necesita (texto extraído de documentos; asunto, remitente, destinatarios, fecha y cuerpo de correos) sale de este equipo hacia";
const STAYS = "No salen la base de datos, otras claves ni el archivo original.";

function destinationName(kind: Kind, b: Block): string {
  switch (kind) {
    case "ollama_cloud":
      return "Ollama Inc. (ollama.com, principalmente en EE. UU.)";
    case "anthropic":
      return "Anthropic (api.anthropic.com)";
    case "openai_compatible":
      return `${hostOf(b.url)} (el servicio que usted ha indicado)`;
    case "ollama":
      return "este equipo";
  }
}

function destinationText(kind: Kind, b: Block): string {
  return `${LEAVES} ${destinationName(kind, b)}. ${STAYS}`;
}

/// El destino de esta elección ya está confirmado (solo se sabe del proveedor guardado).
function alreadyConsented(kind: Kind, b: Block): boolean {
  const s = state.settings;
  const c = s?.choice;
  if (!s || !c || c.provider !== kind || !s.consented) return false;
  return c.provider !== "openai_compatible" || hostOf(c.base_url) === hostOf(b.url);
}

/** Esperas que nacen de la respuesta del proveedor (D146), no de la configuración. */
function isProviderWait(wait: AiWait | null | undefined): boolean {
  return wait === "model_unavailable" || wait === "no_credit";
}

let probingProvider = false;
let checkedForWait = false;

/**
 * «Probar de nuevo» (D146, D122): la prueba sintética con el proveedor elegido. Con éxito el
 * backend levanta la espera y la cola sigue; si no, el error se muestra y la espera continúa.
 */
async function retryProvider() {
  const choice = state.settings?.choice;
  if (!choice || probingProvider) return;
  probingProvider = true;
  render(true);
  try {
    await api.testProvider(choice);
    state.notice = { kind: "ok", text: "El proveedor responde: las tareas esperando se retoman." };
  } catch (error) {
    state.notice = { kind: "error", text: `La prueba sigue fallando: ${probeFailureText(error)}.` };
  }
  probingProvider = false;
  await refresh();
}

/** Sin códigos técnicos: lo que el error de la prueba dice, en palabras. */
function probeFailureText(error: unknown): string {
  const text = String(error);
  if (text.startsWith("model_unavailable")) return "el proveedor sigue sin encontrar el modelo o la dirección";
  if (text.startsWith("no_credit")) return "la cuenta sigue sin crédito";
  if (text.startsWith("unauthorized")) return "el proveedor rechazó la clave";
  if (text.startsWith("timeout")) return "el proveedor no respondió a tiempo";
  if (text.startsWith("invalid_response")) return "el proveedor responde, pero no con el formato pedido";
  if (text.startsWith("request_rejected")) return "el proveedor rechazó la petición de prueba";
  return text.replace(/^[a-z_]+: /, "");
}

/** La IA local está preparada o solo detenida (se arranca al usarla), como en D101. */
const localCheckUsable = () => state.check?.status === "ready" || state.check?.status === "stopped";

/** El botón explícito «Usar la IA local» (D146) solo se ofrece con la IA local preparada. */
function canUseLocalAi(): boolean {
  return localCheckUsable() && state.settings?.active.provider !== "ollama";
}

/**
 * Cambio explícito a la IA local, nunca automático. Comprueba la preparación justo antes y reutiliza
 * el flujo de «Usar este proveedor» (`useProvider`, con su prueba posterior) para que un fallo se vea.
 */
async function useLocalAi() {
  const s = state.settings;
  if (!s) return;
  await refreshCheck();
  if (!localCheckUsable()) {
    state.notice = { kind: "warn", text: "La IA local ya no está preparada: prepárela en la sección Preparación antes de usarla." };
    render();
    return;
  }
  // D134: el consentimiento del afinado automático es de un destino concreto y se retira al cambiarlo.
  if (state.refinement?.consent.state === "granted" && !await ask("Cambiar a la IA local retira el consentimiento del afinado automático de sugerencias (cambia el destino de los datos); podrá volver a activarlo después. ¿Continuar?")) return;
  const b = block("ollama");
  b.model = s.default_local_model;
  b.url = s.default_local_url || LOCAL_URL;
  await useProvider("ollama", {});
  // El resultado de la prueba se ve aquí, no solo en Configuración.
  state.notice = b.outcome ? { kind: b.outcome.kind, text: b.outcome.text } : null;
  render();
}

function goToSettings() {
  state.view = "settings";
  state.notice = null;
  resetBlocks();
  render();
}

function goInstallLocal() {
  state.wizard = false;
  state.view = "setup";
  state.notice = null;
  render();
}

async function openBlock(kind: Kind, opts: BlockOptions) {
  const b = block(kind);
  if (kind === "ollama") return void (await loadModels(kind, null));
  try {
    b.saved = await api.providerKeySaved(choiceFor(kind, ""));
  } catch {
    b.saved = false;
  }
  const auto = opts.auto ?? state.settings?.choice?.provider === kind;
  if (b.saved && auto && !(kind === "openai_compatible" && compatProblem(b))) await loadModels(kind, null);
  else render();
}

/// Pide la lista de modelos (D122). Con una clave tecleada, el backend la guarda solo si la
/// lista responde; si falla, la clave sigue en el campo y no se guarda nada.
async function loadModels(kind: Kind, typed: string | null) {
  const b = block(kind);
  if (b.loading) return;
  b.loading = true;
  b.listError = null;
  b.outcome = null;
  render();
  try {
    const models = await api.listModels(choiceFor(kind, b.model || savedModel(kind) || ""), typed);
    b.models = models;
    if (typed) {
      b.saved = true;
      b.key = "";
    }
    const saved = savedModel(kind);
    if (!b.model || !models.includes(b.model)) {
      if (saved) b.model = models.includes(saved) ? saved : "";
      else b.model = models[0] ?? "";
    }
    if (typed) await refresh(true);
  } catch (error) {
    b.models = null;
    b.model = "";
    b.listError = String(error);
  }
  b.loading = false;
  render();
}

async function useProvider(kind: Kind, opts: BlockOptions) {
  const b = block(kind);
  const choice = choiceFor(kind, b.model);
  b.busy = true;
  b.outcome = null;
  render();
  try {
    await api.selectProvider(choice, alreadyConsented(kind, b) || b.consent);
  } catch (error) {
    b.busy = false;
    b.outcome = { kind: "error", text: `No se ha cambiado de proveedor: ${error}` };
    render();
    return;
  }
  b.consent = false;
  await refresh(true);
  let done = false;
  try {
    await api.testProvider(choice);
    b.outcome = { kind: "ok", text: "Proveedor listo: respondió correctamente" };
    done = true;
  } catch (error) {
    b.outcome = {
      kind: "error",
      text: `La prueba del proveedor falló: ${error}. La selección se ha mantenido; compruebe la clave y el modelo, o elija otro proveedor.`,
    };
  }
  b.busy = false;
  render();
  if (done) opts.onDone?.();
}

async function deleteKey(kind: Kind) {
  const b = block(kind);
  try {
    await api.deleteProviderKey(choiceFor(kind, b.model || savedModel(kind) || ""));
    b.saved = false;
    b.models = null;
    b.model = "";
    b.key = "";
    b.outcome = { kind: "ok", text: "Clave eliminada" };
  } catch (error) {
    b.outcome = { kind: "error", text: String(error) };
  }
  await refresh(true);
}

/// Cambió la URL base: la lista y la clave dependen del host, así que se vuelven a consultar.
async function urlChanged(kind: Kind, opts: BlockOptions) {
  const b = block(kind);
  b.models = null;
  b.model = "";
  b.listError = null;
  b.outcome = null;
  b.saved = null;
  b.consent = false;
  if (kind === "openai_compatible" && compatProblem(b)) {
    b.saved = false;
    return render();
  }
  render();
  await openBlock(kind, opts);
}

/// Bloque de un proveedor: clave, lista de modelos, destino de los datos y «Usar este proveedor».
/// Lo usan Configuración y el asistente de primer uso.
function providerBlock(kind: Kind, opts: BlockOptions = {}): HTMLElement {
  const b = block(kind);
  const external = kind !== "ollama";
  if (!b.opened) {
    b.opened = true;
    queueMicrotask(() => void openBlock(kind, opts));
  }
  const problem = kind === "openai_compatible" ? compatProblem(b) : null;
  const consented = !external || alreadyConsented(kind, b);
  const hasList = !!b.models && b.models.length > 0;
  const saved = savedModel(kind);
  const missing = hasList && saved !== null && !b.models!.includes(saved) ? saved : null;
  const ready = hasList && b.models!.includes(b.model) && (consented || b.consent) && !b.busy && !b.loading && !problem;

  const urlField =
    kind === "openai_compatible" || kind === "ollama"
      ? h(
          "label",
          {},
          kind === "ollama" ? "Dirección local" : "URL base",
          h("input", {
            value: b.url,
            placeholder: kind === "ollama" ? LOCAL_URL : "https://api.ejemplo.com/v1",
            autocomplete: "off",
            oninput: (event) => (b.url = (event.target as HTMLInputElement).value),
            onchange: () => void urlChanged(kind, opts),
          }),
        )
      : null;
  const hostLine =
    kind === "openai_compatible"
      ? h(
          "p",
          { class: problem ? "error-text" : "intro" },
          problem ??
            `La clave queda ligada al host ${hostOf(b.url)}: con otro host no se envía. ${b.saved ? "Hay una clave guardada para este host." : "No hay clave guardada para este host."}`,
        )
      : null;
  const checkDisabled = () => b.loading || b.busy || !!problem || (!b.key.trim() && !b.saved);
  const check = h(
    "button",
    {
      disabled: checkDisabled(),
      "data-focus-key": `provider:check:${kind}`,
      "aria-busy": b.loading ? "true" : undefined,
      onclick: () => void loadModels(kind, b.key.trim() || null),
    },
    b.loading ? "Comprobando…" : "Comprobar y ver modelos",
  ) as HTMLButtonElement;
  const keyField = external
    ? h(
        "div",
        { class: "row" },
        h(
          "label",
          {},
          "Clave",
          h("input", {
            type: "password",
            autocomplete: "off",
            value: b.key,
            placeholder: b.saved ? "Clave guardada" : "Pegue aquí la clave",
            // Sin repintar: el botón se habilita al teclear, sin perder el foco.
            oninput: (event) => {
              b.key = (event.target as HTMLInputElement).value;
              check.disabled = checkDisabled();
            },
          }),
        ),
        check,
      )
    : null;
  const keyNote = external
    ? h("p", { class: "intro" }, "La clave se guarda en el Administrador de credenciales de Windows, solo si el proveedor responde. NestAdmin no la muestra ni la escribe en registros.")
    : null;

  const model = h(
    "label",
    {},
    "Modelo",
    h(
      "select",
      {
        "aria-label": "Modelo",
        disabled: !hasList || b.loading || b.busy,
        onchange: (event) => {
          b.model = (event.target as HTMLSelectElement).value;
          render();
        },
      },
      hasList ? (b.model ? null : h("option", { value: "" }, "Elija un modelo")) : h("option", { value: "" }, b.loading ? "Cargando la lista…" : "Sin lista de modelos"),
      ...(b.models ?? []).map((name) => h("option", { value: name, selected: name === b.model }, name)),
    ),
  );
  const modelNotes: Child[] = [];
  if (missing) {
    modelNotes.push(
      h("p", { class: "meta warn", role: "status" }, `El modelo guardado ${missing} ya no aparece en la lista. No se ha cambiado: elija otro de la lista.`),
    );
  } else if (!hasList && saved) {
    modelNotes.push(h("p", { class: "meta" }, `Modelo guardado: ${saved}. No se puede cambiar hasta que la lista responda.`));
  }
  if (b.listError) modelNotes.push(h("p", { class: "error-text", role: "alert" }, `No se pudo obtener la lista de modelos: ${b.listError}`));
  if (kind === "ollama" && !b.loading && (b.listError || (b.models && b.models.length === 0))) {
    modelNotes.push(
      h(
        "p",
        {},
        b.listError ? "Ollama no está disponible en esta dirección. " : "Ollama no tiene ningún modelo instalado. ",
        h("button", { class: "primary", "data-focus-key": "provider:install-local", onclick: goInstallLocal }, "Instalar IA local"),
      ),
    );
  }

  let consent: Child = null;
  if (external) {
    consent = consented
      ? h("p", { class: "meta" }, `Destino de los datos confirmado: ${destinationName(kind, b)}.`)
      : h(
          "label",
          { class: "choice consent" },
          h("input", {
            type: "checkbox",
            checked: b.consent,
            onchange: (event) => {
              b.consent = (event.target as HTMLInputElement).checked;
              render();
            },
          }),
          h("span", {}, destinationText(kind, b)),
        );
  }

  return h(
    "div",
    { class: "provider-block" },
    urlField ? h("div", { class: "row" }, urlField) : null,
    hostLine,
    keyField,
    keyNote,
    h("div", { class: "row" }, model),
    ...modelNotes,
    consent,
    h(
      "div",
      { class: "buttons" },
      h(
        "button",
        { class: "primary", "data-focus-key": `provider:use:${kind}`, disabled: !ready, "aria-busy": b.busy ? "true" : undefined, onclick: () => void useProvider(kind, opts) },
        b.busy ? "Probando el proveedor…" : "Usar este proveedor",
      ),
      external && b.saved ? h("button", { class: "danger", "data-focus-key": `provider:delete-key:${kind}`, disabled: b.busy, onclick: () => void deleteKey(kind) }, "Eliminar clave") : null,
    ),
    b.outcome ? h("div", { class: `notice ${b.outcome.kind}`, role: b.outcome.kind === "error" ? "alert" : "status" }, h("span", {}, b.outcome.text)) : null,
  );
}

function fallbackStateText(s: SettingsView): string {
  if (!s.choice || s.choice.provider === "ollama") return "El respaldo solo se aplica con un proveedor externo.";
  if (s.fallback_period_active) return "Activo durante 10 minutos porque el proveedor no responde: las tareas van directamente a la IA local.";
  return s.local_fallback ? "Activado." : "Desactivado.";
}

/** Aviso a quien confirma un proveedor mientras hay lecturas en espera: qué arrancará, con qué volumen y datos. */
function waitingReadsBox(): HTMLElement | null {
  const o = state.read;
  // Sin el resumen tampoco se sabe si hay lecturas en espera: se dice, no se calla.
  if (!o) return state.readError ? h("div", { class: "notice warn", role: "note" }, h("span", {}, WAITING_UNKNOWN)) : null;
  const text = waitingReadsNotice(o, o.accounts.filter((a) => a.status.state === "waiting").length);
  return text ? h("div", { class: "notice info", role: "note" }, h("span", {}, text)) : null;
}

function settingsView() {
  const s = state.settings;
  if (!s) return h("section", { class: "page" }, h("p", {}, "Cargando…"));
  const pick: Kind = state.pick ?? s.choice?.provider ?? "ollama_cloud";
  const externalChosen = !!s.choice && s.choice.provider !== "ollama";
  return h(
    "section",
    { class: "page" },
    h("h2", {}, "Configuración"),
    waitingReadsBox(),
    h(
      "section",
      { class: "section" },
      sectionHeader("Proveedor activo"),
      h("p", {}, h("strong", {}, s.choice ? providerLabel(s.active.provider, s.active.model) : "Ninguno elegido")),
      s.ai_wait_message ? h("div", { class: "notice warn", role: "status" }, h("span", {}, s.ai_wait_message)) : null,
      h(
        "label",
        { class: "choice" },
        h("input", {
          type: "checkbox",
          "data-focus-key": "settings:local-fallback",
          checked: s.local_fallback,
          disabled: !externalChosen,
          onchange: (event) => {
            const enabled = (event.target as HTMLInputElement).checked;
            void act(() => api.setLocalFallback(enabled), enabled ? "Respaldo con la IA local activado" : "Respaldo con la IA local desactivado");
          },
        }),
        h("span", {}, h("strong", {}, "Respaldo con la IA local"), " — ", fallbackStateText(s)),
      ),
      h(
        "p",
        { class: "intro" },
        "Si el proveedor externo elegido no responde, agota el tiempo o alcanza su límite, la tarea se repite una sola vez con la IA local, siempre que esté preparada. Nunca se pasa a otro servicio externo ni se descarga nada para ello.",
      ),
    ),
    s.offer_external ? externalOffer() : null,
    h(
      "section",
      { class: "section" },
      sectionHeader("Proveedor de IA"),
      h("p", { class: "intro" }, "Elija el proveedor. Solo se envía contenido a un proveedor externo si usted confirma su destino. OCR y extracción de texto se hacen siempre en este equipo."),
      ...KIND_ORDER.map((kind) =>
        h(
          "div",
          { class: pick === kind ? "provider-choice open" : "provider-choice" },
          h(
            "label",
            { class: "choice" },
            h("input", {
              type: "radio",
              name: "provider",
              value: kind,
              "data-focus-key": `provider:pick:${kind}`,
              checked: pick === kind,
              onchange: () => {
                state.pick = kind;
                render();
              },
            }),
            h("span", {}, h("strong", {}, KIND_NAME[kind]), ` (${KIND_NOTE[kind]})`),
          ),
          pick === kind ? h("div", { class: "indent" }, providerBlock(kind)) : null,
        ),
      ),
    ),
    assistantSection(),
    profileSection(),
    h("section", { class: "section" }, sectionHeader("Datos locales"), h("p", { class: "mono" }, s.data_dir || missing())),
  );
}

// ---- Asistente del chat en Configuración (E2, D183) ----
// El backend decide si el permiso vale; aquí solo se ve qué saldría, se autoriza con lo que se mostró y se retira.

async function loadAssistantConsent() {
  try {
    state.assistantConsent = await api.assistantConsent();
    state.assistantError = null;
  } catch (error) {
    state.assistantError = assistantLoadError(error);
  }
  render(true);
}

/** Carga la vista previa literal y abre el bloque de confirmación; no autoriza nada. */
async function openAssistantPreview() {
  if (state.assistantBusy) return;
  state.assistantBusy = true;
  state.assistantError = null;
  render();
  try {
    const preview = await api.assistantPreview();
    state.assistantPreview = preview;
    state.assistantConsent = preview.consent;
    state.assistantConfirming = true;
    state.focusAfterRender = "assistant:confirm";
  } catch (error) {
    state.assistantError = String(error);
  } finally {
    state.assistantBusy = false;
    render();
  }
}

/** Autoriza con el proveedor, el modelo y el servidor que se mostraron; si el fondo lo rechaza, recarga la vista previa. */
async function grantAssistant() {
  const preview = state.assistantPreview;
  if (!preview || state.assistantBusy) return;
  state.assistantBusy = true;
  state.assistantError = null;
  render();
  let ok = false;
  try {
    state.assistantConsent = await api.grantAssistantConsent(preview.provider.provider, preview.provider.model, preview.host ?? "");
    state.assistantConfirming = false;
    state.assistantPreview = null;
    state.notice = { kind: "ok", text: "Envío del asistente autorizado" };
    state.focusAfterRender = "assistant:withdraw";
    ok = true;
  } catch (error) {
    state.assistantError = grantErrorText(error);
    try {
      const fresh = await api.assistantPreview();
      state.assistantPreview = fresh;
      state.assistantConsent = fresh.consent;
    } catch {
      state.assistantConfirming = false;
      state.assistantPreview = null;
    }
    state.focusAfterRender = "assistant:confirm";
  } finally {
    state.assistantBusy = false;
    if (ok) await refresh();
    else render();
  }
}

async function withdrawAssistant() {
  if (state.assistantBusy) return;
  state.assistantBusy = true;
  state.assistantError = null;
  render();
  try {
    state.assistantConsent = await api.withdrawAssistantConsent();
    state.assistantConfirming = false;
    state.assistantPreview = null;
    state.notice = { kind: "ok", text: "Autorización del asistente retirada" };
    state.focusAfterRender = "assistant:grant";
  } catch (error) {
    state.assistantError = String(error);
  } finally {
    state.assistantBusy = false;
    await refresh();
  }
}

/** «Borrar todas las conversaciones», con la confirmación en pantalla de las acciones destructivas. */
async function clearAllConversations() {
  if (state.assistantBusy) return;
  if (!(await ask(ASSISTANT_CLEAR_ALL_CONFIRM))) return;
  state.assistantBusy = true;
  state.assistantError = null;
  try {
    const n = await api.assistantClear(null);
    state.chat.conversationId = null;
    state.chat.turns = [];
    state.chat.sent = null;
    state.chat.cardNote = null;
    state.notice = { kind: "ok", text: clearedText(n) };
    renderChat();
  } catch (error) {
    state.assistantError = String(error);
  } finally {
    state.assistantBusy = false;
    await refresh();
  }
}

function bulletList(label: string, items: string[]): HTMLElement {
  return h("div", {}, h("p", {}, h("strong", {}, label)), h("ul", { "aria-label": label }, ...items.map((item) => h("li", {}, item))));
}

function assistantSection(): HTMLElement {
  const c = state.assistantConsent;
  const preview = state.assistantPreview;
  const body: Child[] = [h("p", { class: "intro" }, ASSISTANT_INTRO)];
  if (!c) {
    body.push(state.assistantError ? h("div", { class: "notice error", role: "alert" }, h("span", {}, state.assistantError)) : h("p", {}, ASSISTANT_LOADING));
  } else {
    body.push(h("p", {}, h("strong", {}, ASSISTANT_OPTION)), h("p", { role: "status" }, consentStateText(c)));
    if (c.state !== "not_needed") body.push(h("p", { class: "meta" }, ASSISTANT_WITHOUT));
    if (state.assistantError) body.push(h("div", { class: "notice error", role: "alert" }, h("span", {}, state.assistantError)));
    const buttons: Child[] = [];
    if (canGrantConsent(c) && !state.assistantConfirming)
      buttons.push(h("button", { type: "button", class: "primary", "data-focus-key": "assistant:grant", disabled: state.assistantBusy, onclick: () => void openAssistantPreview() }, ASSISTANT_GRANT_OPEN));
    if (canWithdrawConsent(c))
      buttons.push(h("button", { type: "button", "data-focus-key": "assistant:withdraw", disabled: state.assistantBusy, onclick: () => void withdrawAssistant() }, ASSISTANT_WITHDRAW));
    if (buttons.length) body.push(h("div", { class: "buttons" }, ...buttons));
    if (state.assistantConfirming && preview && canGrantConsent(c)) {
      body.push(
        h(
          "div",
          { class: "confirm", role: "group", "aria-label": ASSISTANT_PREVIEW_TITLE },
          h("h4", { tabindex: "-1", "data-focus-key": "assistant:confirm" }, ASSISTANT_PREVIEW_TITLE),
          h("p", {}, assistantDestinationText(preview.provider.provider, preview.provider.model, preview.host)),
          h("p", { class: "meta" }, localFallbackLine(preview.local_fallback)),
          h("p", {}, h("strong", {}, ASSISTANT_SAMPLE_LABEL)),
          h("pre", { class: "mono block", tabindex: "0", "aria-label": ASSISTANT_SAMPLE_LABEL }, preview.sample),
          bulletList(ASSISTANT_MAY_HEADING, preview.may_travel),
          bulletList(ASSISTANT_NEVER_HEADING, preview.never_sent),
          h("ul", { class: "meta", "aria-label": "Límites de cada envío" }, ...limitLines(preview.limits).map((line) => h("li", {}, line))),
          h("p", {}, preview.notice),
          h(
            "div",
            { class: "buttons" },
            h("button", { type: "button", class: "primary", "data-focus-key": "assistant:accept", disabled: state.assistantBusy, onclick: () => void grantAssistant() }, ASSISTANT_ACCEPT),
            h(
              "button",
              {
                type: "button",
                "data-focus-key": "assistant:cancel",
                onclick: () => {
                  state.assistantConfirming = false;
                  state.assistantPreview = null;
                  state.focusAfterRender = "assistant:grant";
                  render();
                },
              },
              ASSISTANT_CANCEL,
            ),
          ),
        ),
      );
    }
  }
  body.push(h("div", { class: "buttons" }, h("button", { type: "button", class: "danger", "data-focus-key": "assistant:clear-all", disabled: state.assistantBusy, onclick: () => void clearAllConversations() }, ASSISTANT_CLEAR_ALL)));
  // El botón del chat lleva aquí el foco y el desplazamiento mediante esta clave estable.
  return h("section", { class: "section", tabindex: "-1", "data-focus-key": "settings:assistant", "aria-label": ASSISTANT_TITLE }, sectionHeader(ASSISTANT_TITLE), ...body);
}

/// Perfil del negocio (E6, D163): editor, vista previa literal y consentimiento. El backend valida
/// y decide qué sale; aquí solo se escribe, se ve y se autoriza.
function profileSection() {
  if (state.profileError && !state.profile)
    return h("section", { class: "section" }, sectionHeader(PROFILE_TITLE), h("div", { class: "notice error", role: "alert" }, h("span", {}, state.profileError)));
  const saved = state.profile;
  if (!saved) return h("section", { class: "section" }, sectionHeader(PROFILE_TITLE), h("p", {}, "Cargando…"));
  const draft: BusinessProfile = (state.profileDraft ??= { ...saved.profile });
  const counter = h("span", { class: "meta", role: "status" });
  const saveButton = h("button", { type: "button", class: "primary", "data-focus-key": "profile:save", onclick: () => void saveProfile() }, "Guardar perfil") as HTMLButtonElement;
  const refreshControls = () => {
    const c = counterText(draft);
    counter.textContent = c.text;
    counter.className = c.over ? "meta error-text" : "meta";
    saveButton.disabled = state.profileBusy || !canSaveProfile(draft, saved.profile);
  };
  const fields = PROFILE_FIELDS.map((f) => {
    const input = h(f.rows === 1 ? "input" : "textarea", {
      ...(f.rows === 1 ? { type: "text" } : { rows: String(f.rows) }),
      id: `profile-${f.key}`,
      "data-focus-key": `profile:${f.key}`,
      placeholder: f.hint,
      oninput: () => {
        draft[f.key] = (input as HTMLInputElement | HTMLTextAreaElement).value;
        refreshControls();
      },
    }) as HTMLInputElement | HTMLTextAreaElement;
    input.value = draft[f.key];
    return h("div", { class: "field" }, h("label", { for: `profile-${f.key}` }, f.label), input);
  });
  // Propuestas de tono aprendidas (D171): van justo debajo del campo Tono, con su antes y después.
  const proposals = Object.values(state.learnedProposals).map(learnedProposalBox);
  const toneAt = PROFILE_FIELDS.findIndex((f) => f.key === "tone");
  const fieldsShown: HTMLElement[] = toneAt < 0 ? [...fields, ...proposals] : [...fields.slice(0, toneAt + 1), ...proposals, ...fields.slice(toneAt + 1)];
  refreshControls();
  const preview = state.profilePreview;
  const status = preview ? profileStatus(preview) : null;
  const dirty = canSaveProfile(draft, saved.profile);
  const buttons: Child[] = [
    saveButton,
    h(
      "button",
      {
        type: "button",
        "data-focus-key": "profile:prefill",
        disabled: state.profileBusy,
        title: "Rellena los campos vacíos con el nombre, la dirección y el correo de su empresa. No incluye NIF ni IBAN.",
        onclick: () => void prefillProfile(),
      },
      "Precargar desde los datos de la empresa",
    ),
  ];
  if (saved.saved)
    buttons.push(
      h("button", { type: "button", "data-focus-key": "profile:clear", disabled: state.profileBusy, onclick: () => void runProfile(() => api.clearBusinessProfile(), true, "Perfil borrado") }, "Borrar perfil"),
    );
  const body: Child[] = [
    h("p", { class: "intro" }, PROFILE_INTRO),
    h("div", { class: "notice warn", role: "note" }, h("span", {}, PROFILE_NOTICE)),
    ...fieldsShown,
    h("div", { class: "buttons" }, ...buttons, counter),
    ...warningTexts(state.profileWarnings).map((text) => h("div", { class: "notice warn", role: "status" }, h("span", {}, text))),
  ];
  if (preview) {
    body.push(
      h(
        "div",
        { class: "sub-block" },
        h("h4", {}, "Qué se enviaría"),
        h("p", {}, status?.line ?? ""),
        dirty ? h("p", { class: "meta" }, "Guarde los cambios para que esto los refleje: la vista previa muestra el perfil guardado.") : null,
        h(
          "div",
          { class: "buttons" },
          h(
            "button",
            {
              type: "button",
              "data-focus-key": "profile:preview",
              disabled: !preview.block,
              onclick: () => {
                state.profileShowBlock = !state.profileShowBlock;
                render();
              },
            },
            state.profileShowBlock ? "Ocultar el bloque" : "Ver el bloque que se enviaría",
          ),
          status?.canGrant
            ? h(
                "button",
                {
                  type: "button",
                  class: "primary",
                  "data-focus-key": "profile:consent-open",
                  disabled: state.profileBusy || dirty,
                  title: dirty ? "Guarde primero los cambios" : undefined,
                  onclick: () => {
                    state.profileConfirming = true;
                    render();
                  },
                },
                "Autorizar el envío…",
              )
            : null,
          status?.canWithdraw
            ? h("button", { type: "button", "data-focus-key": "profile:withdraw", disabled: state.profileBusy, onclick: () => void runProfile(() => api.withdrawProfileConsent(), false, "Autorización retirada") }, "Retirar autorización")
            : null,
        ),
        state.profileShowBlock && preview.block ? h("pre", { class: "mono block", "aria-label": "Bloque del perfil que se enviaría" }, preview.block) : null,
      ),
    );
    if (state.profileConfirming && preview.block && status?.canGrant) {
      const c = profileConsentLines(preview);
      body.push(
        h(
          "div",
          { class: "confirm", role: "group", "aria-label": "Autorizar el envío del perfil" },
          h("p", {}, h("strong", {}, `${c.heading}. `)),
          ...c.lines.map((line) => h("p", { class: "meta" }, line)),
          h("pre", { class: "mono block" }, preview.block),
          h(
            "div",
            { class: "buttons" },
            h("button", { type: "button", class: "primary", "data-focus-key": "profile:consent-accept", onclick: () => void runProfile(() => api.grantProfileConsent(preview), false, "Envío del perfil autorizado") }, "Aceptar y autorizar"),
            h(
              "button",
              {
                type: "button",
                "data-focus-key": "profile:consent-cancel",
                onclick: () => {
                  state.profileConfirming = false;
                  render();
                },
              },
              "Cancelar",
            ),
          ),
        ),
      );
    }
  } else if (state.profileError) body.push(h("div", { class: "notice error", role: "alert" }, h("span", {}, state.profileError)));
  return h("section", { class: "section" }, sectionHeader(PROFILE_TITLE), ...body);
}

/// Ejecuta una acción del perfil sin repetirla si se pulsa dos veces; `reset` vuelve a cargar lo
/// escrito desde lo guardado.
async function runProfile(work: () => Promise<unknown>, reset: boolean, success?: string) {
  if (state.profileBusy) return;
  state.profileBusy = true;
  state.profileConfirming = false;
  render();
  try {
    await work();
    if (reset) {
      state.profileDraft = null;
      state.profileWarnings = [];
    }
    if (success) state.notice = { kind: "ok", text: success };
  } catch (error) {
    state.notice = { kind: "error", text: String(error) };
  } finally {
    state.profileBusy = false;
    await refresh();
  }
}

async function saveProfile() {
  const draft = state.profileDraft;
  if (!draft || state.profileBusy) return;
  state.profileBusy = true;
  render();
  try {
    const saved = await api.saveBusinessProfile(draft);
    state.profileDraft = { ...saved.view.profile };
    state.profileWarnings = saved.warnings;
    state.profileShowBlock = false;
    state.notice = { kind: "ok", text: savedNotice(saved) };
  } catch (error) {
    // Un bloqueo (IBAN, tarjeta, clave, tamaño) deja lo escrito tal cual para corregirlo.
    state.notice = { kind: "error", text: String(error) };
  } finally {
    state.profileBusy = false;
    await refresh();
  }
}

async function prefillProfile() {
  try {
    const prefill = await api.businessProfilePrefill();
    state.profileDraft = applyPrefill(state.profileDraft ?? state.profile?.profile ?? emptyProfile(), prefill);
    state.notice = { kind: "ok", text: "Datos de la empresa precargados en los campos vacíos. Revíselos y guarde." };
    render();
  } catch (error) {
    notify("error", error);
  }
}

/// Oferta única (D100) a quien tenía la IA local: pasar a Ollama cloud. No cambia nada hasta que
/// la persona termina el bloque (clave, modelo, destino y prueba).
function externalOffer() {
  return h(
    "section",
    { class: "section", role: "status" },
    sectionHeader("Ollama cloud"),
    h(
      "p",
      {},
      "NestAdmin ahora puede trabajar con Ollama cloud, un proveedor externo que no necesita preparar nada en este equipo. Seguirá usando la IA local y no cambiará nada hasta que usted termine de configurarlo y confirme el destino de los datos.",
    ),
    h(
      "div",
      { class: "buttons" },
      h(
        "button",
        {
          class: "primary",
          "data-focus-key": "offer:accept",
          onclick: () => {
            state.pick = "ollama_cloud";
            void act(() => api.dismissExternalOffer());
          },
        },
        "Pasar a Ollama cloud",
      ),
      h("button", { "data-focus-key": "offer:dismiss", onclick: () => void act(() => api.dismissExternalOffer()) }, "No, gracias"),
    ),
  );
}

// ---- Asistente de primer uso (D100, D101, D103) ----

function closeWizard(notice?: string) {
  state.wizard = false;
  state.wizardStep = "choose";
  state.view = "review";
  state.notice = notice ? { kind: "ok", text: notice } : null;
  resetBlocks();
  void refresh();
}

function wizardView() {
  const later = h("button", { "data-focus-key": "wizard:later", onclick: () => closeWizard() }, "Ahora no");
  const done = () => closeWizard("Proveedor listo: la IA responde y las tareas en espera se reanudan");
  const head = h(
    "div",
    {},
    h("h2", {}, "Configure la IA de NestAdmin"),
    waitingReadsBox(),
    h(
      "p",
      { class: "intro" },
      "NestAdmin necesita un proveedor de IA para leer facturas y preparar borradores de correo. Hasta que lo configure, las tareas esperan en la cola con el motivo a la vista; puede importar documentos y consultar el correo con normalidad. El OCR y la extracción de texto se hacen siempre en este equipo.",
    ),
  );
  if (state.wizardStep === "cloud") {
    return h(
      "section",
      { class: "page wizard" },
      head,
      sectionHeader("Ollama cloud"),
      providerBlock("ollama_cloud", { auto: true, onDone: done }),
      h("div", { class: "buttons" }, h("button", { "data-focus-key": "wizard:back", onclick: () => ((state.wizardStep = "choose"), render()) }, "Volver"), later),
    );
  }
  if (state.wizardStep === "other") {
    const kinds: Array<"openai_compatible" | "anthropic"> = ["openai_compatible", "anthropic"];
    return h(
      "section",
      { class: "page wizard" },
      head,
      sectionHeader("Otro proveedor externo"),
      ...kinds.map((kind) =>
        h(
          "div",
          { class: "provider-choice" },
          h(
            "label",
            { class: "choice" },
            h("input", {
              type: "radio",
              name: "wizard-other",
              checked: state.wizardOther === kind,
              onchange: () => {
                state.wizardOther = kind;
                render();
              },
            }),
            h("span", {}, h("strong", {}, KIND_NAME[kind]), ` (${KIND_NOTE[kind]})`),
          ),
          state.wizardOther === kind ? h("div", { class: "indent" }, providerBlock(kind, { auto: true, onDone: done })) : null,
        ),
      ),
      h("div", { class: "buttons" }, h("button", { "data-focus-key": "wizard:back", onclick: () => ((state.wizardStep = "choose"), render()) }, "Volver"), later),
    );
  }
  return h(
    "section",
    { class: "page wizard" },
    head,
    h(
      "section",
      { class: "section" },
      sectionHeader("Ollama cloud (recomendado)"),
      h("p", { class: "intro" }, "Proveedor externo por defecto: no hay que instalar nada. Necesita su clave de Ollama y que usted confirme qué contenido sale del equipo."),
      h("button", { class: "primary", "data-focus-key": "wizard:cloud", onclick: () => ((state.wizardStep = "cloud"), render()) }, "Configurar Ollama cloud"),
    ),
    h(
      "section",
      { class: "section" },
      sectionHeader("Otro proveedor externo"),
      h("p", { class: "intro" }, "Un servicio compatible con OpenAI o Claude de Anthropic, con su propia clave."),
      h("button", { "data-focus-key": "wizard:other", onclick: () => ((state.wizardStep = "other"), render()) }, "Elegir otro proveedor externo"),
    ),
    h(
      "section",
      { class: "section" },
      sectionHeader("Usar la IA local"),
      h("p", { class: "intro" }, "Ollama en este equipo: el contenido no sale de él. Hay que instalar Ollama y descargar un modelo de varios GB (Preparación)."),
      h("button", { "data-focus-key": "wizard:local", onclick: goInstallLocal }, "Usar la IA local"),
    ),
    h("div", { class: "buttons" }, later),
  );
}

// ---- Preparación del equipo (R08) ----

const STEP_LABEL: Record<SetupStep, string> = {
  ocr: "OCR incluido (Tesseract)",
  ollama: "Ollama (IA local)",
  service: "Servicio de Ollama",
  model: "Modelo de IA local",
  verify: "Respuesta de la IA local",
};

const STEP_STATE: Record<SetupEvent["state"], string> = {
  running: "En curso",
  done: "Hecho",
  skipped: "Ya estaba listo",
  failed: "Falló",
};

const MISSING_LABEL: Record<string, string> = {
  ollama: "Falta Ollama, el servicio de IA local.",
  ollama_outdated: "La versión de Ollama instalada no es compatible; hay que actualizarla.",
  model: "Falta el modelo de IA local (unos 5,2 GB).",
  ocr: "Faltan componentes de OCR del paquete; reinstale NestAdmin.",
};

// Qué puede hacer el usuario ante cada error del contrato.
const ERROR_HINT: Record<SetupErrorCode, string> = {
  network: "Compruebe la conexión a Internet y pulse Reintentar: la descarga continúa desde donde se cortó.",
  signature: "El instalador descargado no está firmado por Ollama Inc. y no se ha ejecutado. Pulse Reintentar para descargarlo de nuevo.",
  install_failed: "El instalador de Ollama terminó con error. Pulse Reintentar o instale Ollama desde ollama.com.",
  unavailable: "Ollama no responde tras arrancarlo. Pulse Reintentar o reinicie el equipo.",
  model_failed: "Ollama rechazó la descarga del modelo (por ejemplo, por falta de espacio en disco). Libere espacio y pulse Reintentar.",
  verify_failed: "El modelo no dio una respuesta válida. Pulse Reintentar.",
  ocr_missing: "Reinstale NestAdmin para recuperar los componentes de OCR.",
  cancelled: "La preparación se canceló. Puede reanudarla cuando quiera; lo ya hecho no se repite.",
};

// Un tiempo agotado no es una respuesta inválida: cargar el modelo es lo más lento.
const TIMEOUT_HINT = "Pulse Reintentar: con el modelo ya cargado en memoria suele responder antes.";

function size(bytes: number) {
  return bytes >= 1e9 ? `${(bytes / 1e9).toFixed(1).replace(".", ",")} GB` : `${Math.round(bytes / 1e6)} MB`;
}

function seconds(ms: number) {
  return `${(ms / 1000).toFixed(1).replace(".", ",")} s`;
}

async function refreshCheck() {
  try {
    const status = await api.setupCheck();
    state.check = status;
    state.memory = status.memory;
  } catch (error) {
    state.notice = { kind: "error", text: String(error) };
  }
  render(true);
}

async function startPreparation() {
  // Se marca antes de llamar: los eventos pueden llegar antes que la respuesta.
  state.preparing = true;
  state.steps = {};
  state.setupDone = null;
  render();
  try {
    await api.setupPrepare();
  } catch (error) {
    state.preparing = false;
    state.setupDone = { report: null, error: { code: "cancelled", message: String(error) } };
  }
  render();
}

let progressRender: number | null = null;

onSetup((event) => {
  state.steps[event.step] = event;
  // Ollama informa muchas veces por segundo: se agrupan los repintados.
  progressRender ??= window.setTimeout(() => {
    progressRender = null;
    render(true);
  }, 250);
});

onSetupDone((done) => {
  state.preparing = false;
  state.setupDone = done;
  if (done.report) {
    state.check = { status: "ready" };
    state.memory = done.report.memory;
  }
  else void refreshCheck();
  render();
});

onStartup((startup) => {
  state.startup = startup;
  applyStartup();
});

// Comprobación al abrir (D39): si falta algo, la ventana muestra el asistente.
function applyStartup() {
  const s = state.startup;
  if (!s) return;
  if (s.outcome === "needs_preparation") state.check = s.check;
  if ((s.outcome === "needs_preparation" || s.outcome === "failed") && !state.preparing) {
    state.view = "setup";
    state.notice = null;
  }
  render(true);
}

const STEP_ORDER: SetupStep[] = ["ocr", "ollama", "service", "model", "verify"];

const STEP_TONE: Record<SetupEvent["state"], Tone> = { running: "info", done: "ok", skipped: "ok", failed: "error" };

function stepRow(step: SetupStep) {
  const event = state.steps[step];
  const running = event?.state === "running";
  return h(
    "li",
    { class: "component" },
    h("span", { class: "name" }, `Paso ${STEP_ORDER.indexOf(step) + 1}. ${STEP_LABEL[step]}`),
    event ? statusChip(STEP_STATE[event.state], STEP_TONE[event.state]) : statusChip("Pendiente", "neutral"),
    event
      ? h(
          "span",
          { class: event.state === "failed" ? "error-text" : "detail-line" },
          event.done != null && event.total ? `${event.message}: ${size(event.done)} de ${size(event.total)}` : event.message,
        )
      : null,
    running ? (event.total ? h("progress", { max: String(event.total), value: String(event.done ?? 0) }) : h("progress", {})) : null,
  );
}

// Aviso de memoria (D46): informa y deja seguir; los proveedores externos solo se eligen a mano.
function memoryNotice() {
  const m = state.memory;
  if (!m?.warning) return null;
  const strong = m.level === "insufficient";
  return h(
    "div",
    { class: strong ? "notice error" : "notice warn", role: strong ? "alert" : "status" },
    h("span", {}, m.warning),
    state.prepareOnly || state.preparing || m.level === "recommended"
      ? null
      : h("button", { "data-focus-key": "setup:to-settings", onclick: () => ((state.view = "settings"), render()) }, "Ir a Configuración"),
  );
}

function setupView() {
  const done = state.setupDone;
  const error = done?.error;
  const report = done?.report;
  const check = state.check;
  const startupFailed = state.startup?.outcome === "failed" ? state.startup.error : null;
  const finished = STEP_ORDER.filter((s) => state.steps[s]?.state === "done" || state.steps[s]?.state === "skipped").length;
  const buttons: Child[] = [];
  if (state.preparing) {
    buttons.push(h("button", { "data-focus-key": "setup:cancel", onclick: () => void api.setupCancel() }, "Cancelar"));
  } else if (!report || !state.prepareOnly) {
    buttons.push(
      h(
        "button",
        { class: "primary", "data-focus-key": "setup:start", onclick: () => void startPreparation() },
        error ? "Reintentar" : report ? "Comprobar de nuevo" : "Preparar ahora",
      ),
    );
  }
  if (state.prepareOnly && !report) {
    buttons.push(h("button", { "data-focus-key": "setup:abandon", onclick: () => void api.setupAbandon() }, "Cerrar sin terminar"));
  }
  return h(
    "section",
    { class: state.prepareOnly ? "page wizard" : "page" },
    h("h2", {}, "Preparación de la IA local"),
    h(
      "p",
      { class: "intro" },
      "NestAdmin trabaja con componentes locales. Lo que falta se descarga de su fuente oficial y se comprueba antes de usarlo (huella SHA-256 y firma de Ollama Inc.); lo ya instalado se reutiliza. Si se corta la conexión, se reintenta hasta 3 veces y la descarga continúa. Ningún documento sale del equipo y nunca se pasa a un servicio externo.",
    ),
    !state.preparing && !done && check && check.status !== "ready"
      ? h("p", {}, check.status === "stopped" ? "Ollama está instalado pero detenido; se arrancará." : MISSING_LABEL[check.component])
      : null,
    !state.preparing && !done && check?.status === "ready" ? h("p", { class: "ok-line" }, "La IA local está lista.") : null,
    startupFailed && !done ? h("p", { class: "error-text", role: "alert" }, `Ollama no arrancó: ${startupFailed.message}`) : null,
    memoryNotice(),
    h("p", { class: "meta", role: "status" }, `${finished} de ${STEP_ORDER.length} pasos completados`),
    h("ol", { class: "components" }, ...STEP_ORDER.map(stepRow)),
    report
      ? h(
          "p",
          { class: "ok-line", role: "status" },
          `Listo: el modelo ${report.model} respondió «${report.answer}» en ${seconds(report.latency_ms)}.`,
          state.prepareOnly ? " El instalador terminará en unos segundos." : "",
        )
      : null,
    error
      ? h(
          "div",
          { class: "notice error", role: "alert" },
          h("span", {}, error.code === "cancelled" ? ERROR_HINT.cancelled : `${error.message}. ${error.message.startsWith("timeout:") ? TIMEOUT_HINT : (ERROR_HINT[error.code] ?? "")}`),
        )
      : null,
    h("div", { class: "buttons" }, ...buttons),
  );
}

// ---- Chat (E1, D181) y asistente con IA (E2, D182-D184, D188) ----
// Cada pregunta va al asistente (`assistant_turn`). Sin asistente utilizable, o si el asistente falla, el
// chat contesta con una búsqueda por palabras hecha en este equipo. Nada se busca ni se envía mientras se
// escribe: solo al enviar. Todo texto del modelo se pinta como texto plano. El panel no es modal y vive en
// su propio contenedor (`#chat-root`), así que el repintado de la aplicación no lo toca.

/** Una respuesta del asistente lista para pintar, venga de un turno recién hecho o de uno guardado. */
interface AiView {
  text: string;
  outcome: AssistantTurn["outcome"];
  citations: SearchHit[];
  dropped: number;
  withheld: number;
  /** Vista a la que se ha llevado a la persona; solo en el turno recién hecho. */
  navigated: string | null;
  provider: string | null;
  model: string | null;
  local: boolean;
  fallback: { from: string; reason: string } | null;
  turnId: number | null;
  /** Búsqueda por palabras de `consent_required` y `not_configured`. */
  words: QuestionAnswer | null;
  /** La pregunta, para ofrecer «Buscar por palabras» tras un error. */
  question: string;
}

type ChatTurn =
  | { role: "user"; text: string }
  | { role: "assistant"; text: string; answer: QuestionAnswer }
  | { role: "ai"; view: AiView }
  | { role: "info"; text: string; error?: boolean };

/** El texto de una respuesta: la del modelo si hubo, y si no el motivo en palabras llanas. */
function aiText(outcome: AssistantTurn["outcome"], answer: string, errorCode: string | null): string {
  switch (outcome) {
    case "answered":
      return answer;
    case "consent_required":
      return CONSENT_REQUIRED_TEXT;
    case "not_configured":
      return NOT_CONFIGURED_TEXT;
    case "cancelled":
      return CANCELLED_TEXT;
    default:
      return assistantErrorText(errorCode);
  }
}

function aiFromTurn(t: AssistantTurn, question: string, navigated: string | null): AiView {
  return {
    text: aiText(t.outcome, t.answer, t.error_code),
    outcome: t.outcome,
    citations: t.citations,
    dropped: t.dropped_citations,
    withheld: t.withheld,
    navigated,
    provider: t.provider,
    model: t.model,
    local: t.local,
    fallback: t.fallback,
    turnId: t.turn_id,
    words: t.words,
    question,
  };
}

function aiFromStored(t: AssistantStoredTurn, question: string): AiView {
  const outcome = t.outcome ?? "answered";
  return {
    text: aiText(outcome, t.text, null),
    outcome,
    citations: t.citations,
    dropped: t.dropped_citations,
    withheld: t.withheld,
    navigated: null,
    provider: t.provider,
    model: t.model,
    local: t.local,
    fallback: t.fallback,
    turnId: t.id,
    words: null,
    question,
  };
}

const chatHost = (() => {
  const el = document.createElement("div");
  el.id = "chat-root";
  document.body.append(el);
  return el;
})();
/** Quién tenía el foco al abrir: se le devuelve al cerrar. */
let chatOpener: { el: HTMLElement | null; key: string | null } = { el: null, key: null };

/** El chat se puede abrir salvo en el asistente de primer uso y en la preparación de la IA local. */
function chatAllowed(): boolean {
  return !state.prepareOnly && !state.wizard && state.view !== "setup";
}

/** Con una confirmación en pantalla abierta el chat no se abre; la del sistema es modal y no deja pasar el teclado. */
function confirmationOpen(): boolean {
  return state.confirmRedraft !== null || state.confirmPreference !== null || state.readConfirm !== null || state.learnedDeactivating || state.profileConfirming;
}

function appField(key: string): HTMLElement | null {
  return [...root.querySelectorAll<HTMLElement>("[data-focus-key]")].find((el) => el.dataset.focusKey === key) ?? null;
}

function setChatExpanded() {
  appField("chat:button")?.setAttribute("aria-expanded", String(state.chat.open));
}

function chatField(key: string): HTMLElement | null {
  return [...chatHost.querySelectorAll<HTMLElement>("[data-focus-key]")].find((el) => el.dataset.focusKey === key) ?? null;
}

function focusChatInput() {
  const input = chatField("chat:input") as HTMLInputElement | null;
  input?.focus();
  input?.setSelectionRange(input.value.length, input.value.length);
}

function openChat() {
  if (!chatAllowed() || confirmationOpen()) return;
  if (state.chat.open) {
    if (state.chat.sent) closeSent();
    return focusChatInput();
  }
  const active = document.activeElement as HTMLElement | null;
  const usable = active !== null && active !== document.body && root.contains(active);
  chatOpener = { el: usable ? active : null, key: usable ? (active.dataset.focusKey ?? null) : "chat:button" };
  state.chat.open = true;
  setChatExpanded();
  renderChat({ focus: "chat:input", end: true });
  void loadLastConversation();
}

/** Primera apertura de la sesión: recupera la última conversación guardada en el equipo (D188). */
async function loadLastConversation() {
  const chat = state.chat;
  if (chat.loaded) return;
  chat.loaded = true;
  chat.loading = true;
  renderChat();
  try {
    const conversation = await api.assistantConversation(null);
    // Si la persona ya ha empezado otra conversación mientras tanto, no se pisa.
    if (conversation && chat.turns.length === 0) {
      chat.conversationId = conversation.id;
      let question = "";
      for (const turn of conversation.turns) {
        if (turn.role === "user") {
          question = turn.text;
          chat.turns.push({ role: "user", text: turn.text });
        } else chat.turns.push({ role: "ai", view: aiFromStored(turn, question) });
      }
    }
  } catch (error) {
    chat.turns.push({ role: "info", text: loadErrorText(error), error: true });
  } finally {
    chat.loading = false;
    renderChat({ end: true });
  }
}

/** `restoreFocus` es falso cuando el cierre viene de un cambio de vista que ya reparte el foco. */
function closeChat(restoreFocus = true) {
  if (!state.chat.open) return;
  state.chat.open = false;
  chatHost.replaceChildren();
  setChatExpanded();
  if (!restoreFocus) return;
  const { el, key } = chatOpener;
  const target = el?.isConnected ? el : key ? appField(key) : null;
  (target ?? appField("chat:button"))?.focus();
}

/** Repinta solo el panel y conserva lo escrito, el foco y el desplazamiento (D138); `end` baja al último turno. */
function renderChat(options: { focus?: string; end?: boolean; scroll?: number } = {}) {
  if (!state.chat.open) return;
  const scroll = chatHost.querySelector<HTMLElement>(".chat-turns")?.scrollTop ?? 0;
  const active = document.activeElement as HTMLElement | null;
  const inside = active !== null && chatHost.contains(active);
  const focusKey = options.focus ?? (inside ? (active.dataset.focusKey ?? null) : null);
  const selection = inside && active instanceof HTMLInputElement ? ([active.selectionStart, active.selectionEnd] as const) : null;
  chatHost.replaceChildren(chatPanel());
  const list = chatHost.querySelector<HTMLElement>(".chat-turns");
  if (list) list.scrollTop = options.end ? list.scrollHeight : (options.scroll ?? scroll);
  if (!focusKey) return;
  const target = chatField(focusKey);
  if (!target) return;
  target.focus();
  if (target instanceof HTMLInputElement) {
    if (selection && !options.focus) target.setSelectionRange(selection[0], selection[1]);
    else target.setSelectionRange(target.value.length, target.value.length);
  }
}

/** Lleva la ventana a una vista de la lista cerrada del asistente; el chat sigue abierto y el foco no cambia. */
function navigateFromChat(view: string) {
  state.view = view as View;
  state.notice = null;
  state.taskLocate = null;
  if (view === "settings") resetBlocks();
  render();
  if (view === "tasks") void loadPipeline().then(() => render(true));
}

async function sendChat() {
  const chat = state.chat;
  const text = chat.input.trim();
  if (!text || chat.busy || chat.loading) return;
  chat.turns.push({ role: "user", text });
  chat.input = "";
  chat.busy = true;
  chat.busyKind = "assistant";
  chat.cancelling = false;
  chat.progress = null;
  chat.cardNote = null;
  chat.sent = null;
  renderChat({ focus: "chat:input", end: true });
  try {
    const turn = await api.assistantTurn(chat.conversationId, text, state.view);
    if (turn.conversation_id !== null) chat.conversationId = turn.conversation_id;
    // Solo las vistas de la lista cerrada, y solo con una respuesta: nunca por un error o una cancelación.
    const target = turn.outcome === "answered" ? navigateTarget(turn.navigate) : null;
    chat.turns.push({ role: "ai", view: aiFromTurn(turn, text, target) });
    if (target) navigateFromChat(target);
  } catch (error) {
    chat.turns.push({ role: "info", text: assistantCallErrorText(error), error: true });
  } finally {
    chat.busy = false;
    chat.cancelling = false;
    chat.progress = null;
    renderChat({ focus: "chat:input", end: true });
  }
}

/** Pide parar el turno en curso; el turno termina con `cancelled` y no deja efectos. */
async function cancelChat() {
  const chat = state.chat;
  if (!chat.busy || chat.busyKind !== "assistant" || chat.cancelling) return;
  chat.cancelling = true;
  renderChat();
  try {
    await api.assistantCancel();
  } catch (error) {
    chat.cancelling = false;
    chat.turns.push({ role: "info", text: String(error), error: true });
    renderChat({ end: true });
  }
}

/** Búsqueda por palabras de la pregunta (D181): la oferta tras un error del asistente. */
async function searchByWords(question: string) {
  const chat = state.chat;
  if (!question.trim() || chat.busy) return;
  chat.busy = true;
  chat.busyKind = "words";
  chat.cardNote = null;
  renderChat({ focus: "chat:input", end: true });
  try {
    const answer = await api.searchQuestion(question);
    chat.turns.push({ role: "assistant", text: answerText(answer.matched, answer.page.total, answer.terms, answer.kind), answer });
  } catch (error) {
    chat.turns.push({ role: "info", text: searchErrorText(error), error: true });
  } finally {
    chat.busy = false;
    renderChat({ focus: "chat:input", end: true });
  }
}

/** Borra la conversación abierta, con confirmación en pantalla (D188), y deja el panel vacío. */
async function clearConversation() {
  const chat = state.chat;
  if (chat.busy || chat.conversationId === null) return;
  if (!(await ask(CLEAR_CONVERSATION_CONFIRM))) return;
  try {
    await api.assistantClear(chat.conversationId);
    chat.conversationId = null;
    chat.turns = [];
    chat.sent = null;
    chat.cardNote = null;
    renderChat({ focus: "chat:input" });
  } catch (error) {
    chat.turns.push({ role: "info", text: String(error), error: true });
    renderChat({ end: true });
  }
}

/** Abre «Ver lo enviado» de un turno dentro del panel. */
async function openSent(turnId: number) {
  const chat = state.chat;
  const scroll = chatHost.querySelector<HTMLElement>(".chat-turns")?.scrollTop ?? 0;
  chat.sent = { turnId, calls: null, error: null, scroll };
  renderChat({ focus: "chat:sent:title", scroll: 0 });
  try {
    const calls = await api.assistantSent(turnId);
    if (chat.sent?.turnId === turnId) chat.sent.calls = calls;
  } catch (error) {
    if (chat.sent?.turnId === turnId) chat.sent.error = sentError(error);
  }
  renderChat();
}

function closeSent() {
  const sent = state.chat.sent;
  if (!sent) return;
  state.chat.sent = null;
  renderChat({ focus: `chat:sent-open:${sent.turnId}`, scroll: sent.scroll });
}

/** De la respuesta de un turno sin permiso o sin IA a Configuración, con el foco y el desplazamiento en el bloque del asistente. */
function goToAssistantSettings() {
  state.view = "settings";
  state.notice = null;
  state.taskLocate = null;
  resetBlocks();
  state.focusAfterRender = "settings:assistant";
  render();
  root.querySelector('[data-focus-key="settings:assistant"]')?.scrollIntoView({ block: "start" });
  if (!state.assistantConsent) void loadAssistantConsent();
}

async function retryFailedIndex() {
  const chat = state.chat;
  if (chat.reindexing) return;
  chat.reindexing = true;
  renderChat();
  try {
    chat.turns.push({ role: "info", text: reindexLine(await api.reindexFailed()) });
  } catch (error) {
    chat.turns.push({ role: "info", text: reindexErrorText(error), error: true });
  } finally {
    chat.reindexing = false;
    renderChat({ end: true });
  }
}

/** Pestaña de Pendientes que muestra la tarea, o `null` si no tiene vista de revisión. */
function reviewTabOf(task: Task): ReviewTab | null {
  if (isInformative(task)) return "informative";
  return task.state === "awaiting_review" && isKnownKind(task.kind) ? tabOf(task.kind) : null;
}

/**
 * Abre la tarea de una tarjeta del chat: en su pestaña de Pendientes si está en revisión o es un
 * Informativo; en cualquier otro estado, en la vista Tareas, con su fila resaltada.
 */
async function openHit(hit: SearchHit, key: string) {
  let task = state.tasks.find((t) => t.id === hit.task_id);
  if (!task) {
    // La lista puede estar desactualizada: se refresca una vez antes de darla por perdida.
    await refresh(true);
    task = state.tasks.find((t) => t.id === hit.task_id);
  }
  if (!task) {
    state.chat.cardNote = { key, text: NOT_FOUND_TASK };
    renderChat();
    return;
  }
  state.chat.cardNote = null;
  const tab = reviewTabOf(task);
  if (tab) {
    openInReview(task, tab);
    return;
  }
  locateTask(task);
}

/** Lleva a la vista Tareas con la tarea visible: quita el filtro que la oculte y despliega su columna. */
function locateTask(task: Task) {
  state.view = "tasks";
  state.tasksPane = "view";
  state.notice = null;
  state.taskLocate = task.id;
  if (state.tasksView === "list") {
    if (!filterTasks([task], state.taskFilter, state.pipelineIndex).length) state.taskFilter = "";
  } else {
    const column = boardColumnOf(task);
    if (column === "closed") state.closedOpen = true;
    const at = groupForBoard(state.tasks)[column].findIndex((t) => t.id === task.id);
    if (at >= (state.boardShown[column] ?? BOARD_PAGE)) state.boardShown[column] = Math.ceil((at + 1) / BOARD_PAGE) * BOARD_PAGE;
  }
  state.focusAfterRender = `task:row:${task.id}`;
  render();
  root.querySelector(`[data-focus-key="task:row:${task.id}"]`)?.scrollIntoView({ block: "center" });
  void loadPipeline().then(() => render(true));
}

async function openOriginalHit(hit: SearchHit, key: string) {
  if (isPreview) {
    state.chat.cardNote = { key, text: OPEN_ORIGINAL_PREVIEW };
    renderChat();
    return;
  }
  try {
    await api.openOriginal(hit.task_id);
    state.chat.cardNote = null;
  } catch (error) {
    state.chat.cardNote = { key, text: originalErrorText(error) };
  }
  renderChat({ focus: `${key}:original` });
}

function chatCard(hit: SearchHit, key: string): HTMLElement {
  const v = cardView(hit);
  const note = state.chat.cardNote?.key === key ? state.chat.cardNote.text : null;
  return h(
    "li",
    {},
    h(
      "button",
      { type: "button", class: "chat-card", "data-focus-key": key, "aria-label": cardLabel(hit), onclick: () => void openHit(hit, key) },
      h("span", { class: "chat-card-head" }, statusChip(v.kind, "info"), h("span", { class: "chat-card-title" }, v.title)),
      v.meta.length ? h("span", { class: "chat-card-meta" }, v.meta.join(" · ")) : null,
      v.note ? h("span", { class: "chat-card-note" }, statusChip(v.note, "warn")) : null,
      v.snippet ? h("span", { class: "chat-card-snippet" }, v.snippet) : null,
    ),
    canOpenOriginal(hit)
      ? h("button", { type: "button", class: "chat-card-original", "data-focus-key": `${key}:original`, "aria-label": originalLabel(hit), onclick: () => void openOriginalHit(hit, key) }, OPEN_ORIGINAL_LABEL)
      : null,
    note ? h("p", { class: "chat-card-error", role: "status" }, note) : null,
  );
}

/** Texto plano en párrafos: respeta los saltos de línea y no interpreta nada (sin HTML, imágenes ni enlaces activos). */
function plainText(text: string, role?: string): HTMLElement {
  return h("div", { class: "chat-text", role }, ...textParagraphs(text).map((line) => h("p", {}, line)));
}

/** Lo que cuelga de una búsqueda por palabras: aviso, tarjetas, recuentos y el reintento de lo que falló. */
function wordsBlock(text: string, answer: QuestionAnswer, index: number, last: boolean): Array<HTMLElement | null> {
  const { page } = answer;
  const counts = countLines(page);
  const shown = shownLine(page.results.length, page.total);
  return [
    h("p", {}, text),
    h("p", { class: "chat-notice" }, SEARCH_NOTICE),
    page.results.length ? h("ul", { class: "chat-cards" }, ...page.results.map((hit) => chatCard(hit, `chat:card:${index}:${hit.task_id}`))) : null,
    shown ? h("p", { class: "meta" }, shown) : null,
    counts.length ? h("p", { class: "meta" }, counts.join(" ")) : null,
    page.failed > 0 && last
      ? h("button", { type: "button", "data-focus-key": "chat:reindex", disabled: state.chat.reindexing, onclick: () => void retryFailedIndex() }, state.chat.reindexing ? REINDEX_BUSY : REINDEX_BUTTON)
      : null,
  ];
}

/** Respuesta del asistente (E2): texto plano, tarjetas de lo citado, quién contestó y notas de lo descartado o retenido. */
function aiTurn(v: AiView, index: number, last: boolean): HTMLElement {
  const answered = v.outcome === "answered";
  const who = answered ? whoAnswered(v) : "";
  const notes = answered ? [droppedCitationsText(v.dropped), withheldText(v.withheld)].filter((n): n is string => n !== null) : [];
  const sendHint = v.outcome === "consent_required" || v.outcome === "not_configured";
  return h(
    "div",
    { class: `chat-turn assistant${v.outcome === "error" ? " error" : ""}` },
    h("span", { class: "chat-who" }, ASSISTANT_LABEL),
    plainText(v.text, v.outcome === "error" ? "alert" : undefined),
    v.navigated ? h("p", { class: "chat-notice", role: "status" }, navigatedText(v.navigated)) : null,
    v.citations.length ? h("ul", { class: "chat-cards" }, ...v.citations.map((hit) => chatCard(hit, `chat:card:${index}:${hit.task_id}`))) : null,
    who ? h("p", { class: "chat-notice chat-source" }, who) : null,
    ...notes.map((note) => h("p", { class: "chat-notice" }, note)),
    sendHint
      ? h("div", { class: "buttons" }, h("button", { type: "button", "data-focus-key": `chat:settings:${index}`, onclick: goToAssistantSettings }, SETTINGS_BUTTON))
      : null,
    sendHint && v.words ? h("p", { class: "chat-notice" }, WORDS_INTRO) : null,
    ...(sendHint && v.words ? wordsBlock(answerText(v.words.matched, v.words.page.total, v.words.terms, v.words.kind), v.words, index, last) : []),
    v.outcome === "error" && last && v.question
      ? h("div", { class: "buttons" }, h("button", { type: "button", "data-focus-key": "chat:words", disabled: state.chat.busy, onclick: () => void searchByWords(v.question) }, WORDS_BUTTON))
      : null,
    v.turnId !== null
      ? h("div", { class: "buttons" }, h("button", { type: "button", class: "link", "data-focus-key": `chat:sent-open:${v.turnId}`, onclick: () => void openSent(v.turnId!) }, SENT_BUTTON))
      : null,
  );
}

function chatTurn(turn: ChatTurn, index: number, last: boolean): HTMLElement {
  if (turn.role === "user") return h("div", { class: "chat-turn user" }, h("span", { class: "chat-who" }, YOU_LABEL), h("p", {}, turn.text));
  if (turn.role === "info")
    return h("div", { class: `chat-turn assistant${turn.error ? " error" : ""}` }, h("span", { class: "chat-who" }, ASSISTANT_LABEL), h("p", { role: turn.error ? "alert" : undefined }, turn.text));
  if (turn.role === "ai") return aiTurn(turn.view, index, last);
  return h("div", { class: "chat-turn assistant" }, h("span", { class: "chat-who" }, ASSISTANT_LABEL), ...wordsBlock(turn.text, turn.answer, index, last));
}

/** Vista del panel «Ver lo enviado» (D183): el texto literal de cada llamada, en un bloque seleccionable. */
function sentView(): HTMLElement {
  const sent = state.chat.sent!;
  return h(
    "div",
    { class: "chat-turns chat-sent" },
    h("div", { class: "buttons" }, h("button", { type: "button", "data-focus-key": "chat:sent:back", onclick: closeSent }, SENT_BACK)),
    h("h3", { tabindex: "-1", "data-focus-key": "chat:sent:title" }, SENT_TITLE),
    h("p", { class: "meta" }, SENT_INTRO),
    sent.error
      ? h("p", { class: "error-text", role: "alert" }, sent.error)
      : sent.calls === null
        ? h("p", { class: "meta", role: "status" }, SENT_LOADING)
        : sent.calls.length === 0
          ? h("p", { role: "status" }, SENT_NONE)
          : h(
              "div",
              { class: "chat-sent-calls" },
              ...sent.calls.map((call) =>
                h(
                  "section",
                  { class: "chat-sent-call", "aria-label": sentCallTitle(call) },
                  h("h4", {}, sentCallTitle(call)),
                  h("p", { class: "meta" }, sentCallWhere(call)),
                  h("pre", { class: "mono block chat-sent-text", tabindex: "0", "aria-label": `Texto enviado en la llamada ${call.step}` }, call.sent_text),
                ),
              ),
            ),
  );
}

function chatPanel(): HTMLElement {
  const chat = state.chat;
  const input = h("input", {
    type: "text",
    id: "chat-input",
    "data-focus-key": "chat:input",
    placeholder: CHAT_INPUT_PLACEHOLDER,
    autocomplete: "off",
    spellcheck: "false",
    oninput: (event) => {
      chat.input = (event.target as HTMLInputElement).value;
    },
  }) as HTMLInputElement;
  input.value = chat.input;
  const lastAnswer = chat.turns.map((t) => (t.role === "assistant" || t.role === "ai" ? "a" : t.role)).lastIndexOf("a");
  const list = chat.turns.length
    ? chat.turns.map((turn, i) => chatTurn(turn, i, i === lastAnswer))
    : [
        h(
          "div",
          { class: "chat-empty" },
          h("p", { class: "chat-empty-title" }, CHAT_EMPTY_TITLE),
          h("p", { class: "meta" }, CHAT_EMPTY_HINT),
          h(
            "ul",
            { class: "chat-examples", "aria-label": CHAT_EXAMPLES_LABEL },
            ...CHAT_EXAMPLES.map((text, i) =>
              h(
                "li",
                {},
                h(
                  "button",
                  {
                    type: "button",
                    "data-focus-key": `chat:example:${i}`,
                    // Solo rellena el cuadro: no se envía nada hasta pulsar Intro o «Preguntar».
                    onclick: () => {
                      chat.input = text;
                      renderChat({ focus: "chat:input" });
                    },
                  },
                  text,
                ),
              ),
            ),
          ),
        ),
      ];
  return h(
    "section",
    { id: "chat-panel", class: "chat-panel", role: "dialog", "aria-label": CHAT_LABEL },
    h(
      "div",
      { class: "chat-head" },
      h("h2", {}, CHAT_TITLE),
      h(
        "div",
        { class: "chat-head-actions" },
        chat.conversationId !== null && !chat.sent
          ? h("button", { type: "button", "data-focus-key": "chat:clear", disabled: chat.busy, onclick: () => void clearConversation() }, CLEAR_CONVERSATION)
          : null,
        h("button", { type: "button", class: "close", "data-focus-key": "chat:close", "aria-label": CHAT_CLOSE, title: CHAT_CLOSE, onclick: () => closeChat() }, "×"),
      ),
    ),
    chat.sent
      ? sentView()
      : h(
          "div",
          { class: "chat-turns", "data-scroll-key": "chat:turns", "aria-live": "polite", "aria-busy": String(chat.busy || chat.loading) },
          ...list,
          chat.loading ? h("p", { class: "meta", role: "status" }, LOADING_PREVIOUS) : null,
          chat.busy
            ? h(
                "div",
                { class: "chat-busy" },
                h("span", { class: "meta", role: "status" }, chat.busyKind === "assistant" ? progressText(chat.progress) : CHAT_BUSY),
                chat.busyKind === "assistant"
                  ? h("button", { type: "button", "data-focus-key": "chat:cancel", disabled: chat.cancelling, onclick: () => void cancelChat() }, chat.cancelling ? CHAT_CANCELLING : CHAT_CANCEL)
                  : null,
              )
            : null,
        ),
    // Un formulario: Intro en el cuadro y el botón «Preguntar» envían igual (una sola vía).
    chat.sent
      ? null
      : h(
      "form",
      {
        class: "chat-form",
        onsubmit: (event) => {
          event.preventDefault();
          void sendChat();
        },
      },
      h("label", { for: "chat-input", class: "chat-label" }, CHAT_INPUT_LABEL),
      h("div", { class: "chat-row" }, input, h("button", { type: "submit", class: "primary", "data-focus-key": "chat:send", disabled: chat.busy || chat.loading }, CHAT_SEND)),
    ),
  );
}

// Un único `keydown` global: Ctrl+K abre el chat (o lleva el foco a su cuadro) y Esc lo cierra.
document.addEventListener("keydown", (event) => {
  if ((event.ctrlKey || event.metaKey) && !event.altKey && !event.shiftKey && event.key.toLowerCase() === "k") {
    event.preventDefault();
    openChat();
    return;
  }
  if (event.key === "Escape" && state.chat.open && !event.defaultPrevented) {
    const target = event.target as Node | null;
    if (!target || target === document.body || target === document.documentElement || chatHost.contains(target)) {
      event.preventDefault();
      closeChat();
    }
  }
});

function render(background = false) {
  // Modo instalador (D36): solo el asistente, sin secciones ni datos.
  if (state.prepareOnly) {
    root.replaceChildren(h("main", { class: "wizard-only" }, setupView()));
    return;
  }
  // El chat no acompaña al asistente de primer uso ni a la preparación de la IA local.
  if (state.chat.open && !chatAllowed()) closeChat(false);
  const views: Record<View, () => HTMLElement> = {
    review: reviewView,
    tasks: tasksView,
    rules: rulesView,
    history: historyView,
    mail: mailView,
    settings: settingsView,
    setup: setupView,
  };
  const reviewable = state.notice?.task !== undefined ? state.tasks.find((t) => t.id === state.notice?.task && t.state === "awaiting_review") : undefined;
  const notice = state.notice
    ? h(
        "div",
        { class: `notice ${state.notice.kind}`, role: state.notice.kind === "error" ? "alert" : "status" },
        h("span", {}, state.notice.text),
        reviewable ? h("button", { onclick: () => openInReview(reviewable) }, "Ver") : null,
        h("button", { class: "close", "aria-label": "Cerrar aviso", onclick: () => ((state.notice = null), render()) }, "×"),
      )
    : null;
  // Una actualización de fondo no borra lo que el usuario está escribiendo.
  const focused = document.activeElement;
  // Una casilla o un selector con el foco no congelan los refrescos: no hay texto que perder.
  if (background && (focused instanceof HTMLTextAreaElement || (focused instanceof HTMLInputElement && !["file", "checkbox", "radio"].includes(focused.type)))) {
    if (root.contains(focused)) return;
  }
  // D103: con un proveedor externo, al abrir solo se comprueba el OCR del paquete.
  const ocrBanner =
    state.startup?.outcome === "ocr_missing"
      ? h("div", { class: "notice error", role: "alert" }, h("span", {}, "Faltan componentes de OCR del paquete. Reinstale NestAdmin para recuperarlos; mientras tanto no se podrán leer documentos escaneados."))
      : null;
  const wait = state.settings?.ai_wait_message;
  const providerWait = isProviderWait(state.settings?.ai_wait);
  const waitBanner =
    wait && state.view !== "settings"
      ? providerWait
        ? h(
            "div",
            { class: "banner", role: "status" },
            // D146: el texto ya dice que las tareas esperan sin gastar intentos.
            h("span", {}, wait),
            h(
              "div",
              { class: "banner-actions" },
              h("button", { disabled: probingProvider, "data-focus-key": "wait:retry", onclick: () => void retryProvider() }, probingProvider ? "Probando…" : "Probar de nuevo"),
              // Sin crédito, otro modelo del mismo proveedor no ayuda: se ofrece elegir otro proveedor.
              h("button", { "data-focus-key": "wait:change", onclick: goToSettings }, state.settings?.ai_wait === "no_credit" ? "Elegir otro proveedor" : "Cambiar modelo"),
              canUseLocalAi() ? h("button", { "data-focus-key": "wait:local", onclick: () => void useLocalAi() }, "Usar la IA local") : null,
            ),
          )
        : h("div", { class: "banner", role: "status" }, h("span", {}, `${wait}. Las tareas esperan en la cola.`), h("button", { onclick: goToSettings }, "Ir a Configuración"))
      : null;
  if (state.wizard) {
    root.replaceChildren(h("main", { class: "wizard-only" }, wizardView()));
    return;
  }
  const restore = keepPosition();
  root.replaceChildren(
    header(),
    h(
      "div",
      { class: "body" },
      nav(),
      h("main", { class: state.view === "review" || (state.view === "tasks" && state.tasksPane === "view" && state.tasksView === "board" && state.tasks.length > 0) ? "fill" : undefined, "data-scroll-key": `main:${state.view}` }, ocrBanner, notice, waitBanner, views[state.view]()),
    ),
  );
  restore();
}

/**
 * El repintado sustituye el DOM (D138): guarda el desplazamiento de cada panel y el foco, y
 * devuelve la función que los repone. Un panel de otro elemento tiene otra clave y empieza arriba.
 */
function keepPosition(): () => void {
  const scroll = new Map<string, number>();
  for (const el of root.querySelectorAll<HTMLElement>("[data-scroll-key]")) scroll.set(el.dataset.scrollKey!, el.scrollTop);
  const active = document.activeElement as HTMLElement | null;
  const focusKey = state.focusAfterRender ?? active?.dataset?.focusKey ?? null;
  state.focusAfterRender = null;
  const selection = active instanceof HTMLTextAreaElement ? ([active.selectionStart, active.selectionEnd] as const) : null;
  return () => {
    for (const el of root.querySelectorAll<HTMLElement>("[data-scroll-key]")) {
      const top = scroll.get(el.dataset.scrollKey!);
      if (top) el.scrollTop = top;
    }
    if (!focusKey) return;
    const target = [...root.querySelectorAll<HTMLElement>("[data-focus-key]")].find((el) => el.dataset.focusKey === focusKey);
    if (!target) return;
    target.focus();
    if (selection && target instanceof HTMLTextAreaElement) target.setSelectionRange(selection[0], selection[1]);
  };
}

void (async () => {
  try {
    const launch = await api.launchMode();
    state.prepareOnly = launch.prepare_only;
    state.preparing = launch.preparing;
    state.startup ??= launch.startup;
  } catch (error) {
    state.notice = { kind: "error", text: String(error) };
  }
  if (state.prepareOnly) {
    // El instalador espera: la preparación empieza sola.
    state.view = "setup";
    if (state.preparing) render();
    else void startPreparation();
    // La comprobación solo lee: aporta el aviso de memoria mientras se prepara.
    void refreshCheck();
    return;
  }
  onChanged(() => void refresh(true));
  onAssistantProgress((step) => {
    if (!state.chat.busy || state.chat.busyKind !== "assistant") return;
    state.chat.progress = step;
    renderChat();
  });
  onMailSignIn((url) => {
    state.signInUrl = url;
    render(true);
  });
  await refresh();
  // Primer uso (D101): una vez por arranque, y solo si falta elegir, la clave o el destino;
  // con una clave rechazada basta el aviso. Las esperas de D146 (404, 402) tampoco lo abren: la
  // configuración ya existe y el aviso ofrece probar de nuevo, cambiar de modelo o la IA local.
  const wait = state.settings?.ai_wait;
  if (wait === "needs_choice" || wait === "needs_key" || wait === "needs_consent") {
    state.wizard = true;
    render();
  }
  applyStartup();
  void refreshCheck();
})();
