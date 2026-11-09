export type TaskState = "queued" | "running" | "awaiting_review" | "completed" | "failed" | "cancelled";

export interface TaskError {
  code: string;
  message: string;
  recoverable: boolean;
}

export interface Task {
  id: number;
  kind: "invoice" | "email" | string;
  input_ref: string;
  state: TaskState;
  attempts: number;
  result: Record<string, any> | null;
  error: TaskError | null;
  created_at: number;
  updated_at: number;
}

export interface Rule {
  id: string;
  version: number;
  action_kind: "register_invoice" | "send_draft" | string;
  conditions: Record<string, string>;
  active: boolean;
  created_at: number;
}

/** Contadores de una regla (D129). */
export interface RuleStats {
  rule_id: string;
  /** Acciones confirmadas que autorizó la regla. */
  applied: number;
  last_applied_at: number | null;
  /** Tareas que la regla habría automatizado y fueron a revisión. */
  reviewed: number;
  /** Acciones antiguas que citan la regla en un formato ilegible. */
  unclassified: number;
}

/** Una regla con lo que el panel muestra de ella (D124, D126, D128). */
export interface RuleView extends Rule {
  /** Nombre visible; si la regla no tiene, su identificador. */
  name: string;
  archived_at: number | null;
  /** Marca de la opción avanzada: la regla de envío contesta a cualquier remitente. */
  any_sender_ack_at: number | null;
  /** De dónde sale la regla: al unificar, las reglas de origen y la opción elegida (D131). */
  origin: { kind: string; sources?: Array<{ id: string; version: number }>; option?: string; count?: number; threshold?: number } | null;
  stats: RuleStats;
}

export type SimulationOutcome = "automate" | "review" | "overlap" | "no_match";

export interface SimulationItem {
  task_id: number;
  label: string | null;
  outcome: SimulationOutcome;
  reason_code: string | null;
  explanation: string;
  overlapping_rules: string[];
}

/** Resultado de simular una regla hipotética contra las últimas tareas (solo lectura). */
export interface Simulation {
  action_kind: string;
  limit: number;
  considered: number;
  truncated: boolean;
  skipped_without_result: number;
  /** Facturas sin la comprobación local de moneda (tareas antiguas). */
  without_currency_check: number;
  /** Correos sin la comprobación de correo automático (tareas antiguas): irían a revisión. */
  without_signal_check?: number;
  would_automate: number;
  would_review: Record<string, number>;
  overlaps: number;
  no_match: number;
  would_block_other: Array<{ rule_id: string; task_ids: number[] }>;
  /** Contradicciones que la regla crearía al activarse, con su explicación (D130). */
  findings: RuleFinding[];
  items: SimulationItem[];
  /** Lo mismo para las facturas solo leídas en la primera lectura, aparte y sin listarlas por tarea. */
  from_read?: Simulation | null;
}

/** Una contradicción entre reglas (D130). El texto lo redacta `rules-text.ts`. */
export interface RuleFinding {
  key: string;
  kind: "duplicate" | "overlap" | "similar" | "defective";
  action_kind: string;
  rules: Array<{ id: string; version: number; active: boolean }>;
  /** Las dos reglas están activas: el choque ocurre ya. */
  active_conflict: boolean;
  effect: { code: string; data: Record<string, any> };
  merge_candidate: boolean;
  /** Otras reglas activas, ajenas al par, que también cubren lo que solo cubre la más amplia. */
  others: Array<{ id: string; version: number; active: boolean }>;
  /** Tareas que ya coincidieron con las dos reglas con sus condiciones actuales; `null` si no aplica. */
  affected_tasks: number | null;
}

export interface MergeOption {
  /** `none` o el límite en facturas; `any`, `domain:x` o `sender:x` en envíos. */
  key: string;
  scope: Record<string, any>;
  rule_ids: string[];
}

export interface PlanRule {
  id: string;
  version: number;
  active: boolean;
  conditions: Record<string, any>;
  any_sender: boolean;
}

export interface SimTask {
  task_id: number;
  label: string | null;
  total: string | null;
  /** Regla que la automatiza hoy (resultado real), si alguna. */
  automated_before_by: string | null;
  /** Regla que la automatizaría con la unificación: `(unificada)` es la regla nueva. */
  automated_after_by: string | null;
}

export interface MergeSimulation {
  considered: number;
  truncated: boolean;
  skipped_without_result: number;
  touched: number;
  start_automating: SimTask[];
  stop_automating: SimTask[];
  unchanged_automated: number;
  unchanged_review: number;
  without_currency_check: number;
}

export interface MergePlan {
  action_kind: string;
  before: PlanRule[];
  options: MergeOption[];
  chosen: string;
  after: {
    conditions: Record<string, any>;
    active: boolean;
    option_key: string;
    loses_any_sender: string[];
    needs_sender_validation: boolean;
  };
  simulation: MergeSimulation;
  /** Solo con «cualquier remitente»: qué pasaría después de validar la opción avanzada. */
  simulation_if_validated: MergeSimulation | null;
  fingerprint: string;
}

/** Lo que se ve al revisar una unificación (D131). Sin elección, `plan` es `null`. */
export interface MergePreview {
  action_kind: string;
  before: PlanRule[];
  options: MergeOption[];
  needs_choice: boolean;
  chosen: string | null;
  plan: MergePlan | null;
  suggested_name: string;
}

export interface AuditEntry {
  task_id: number | null;
  action: string;
  status: string;
  authorization: string | null;
  detail: string | null;
}

export type ProviderChoice =
  | { provider: "ollama"; base_url: string; model: string }
  | { provider: "ollama_cloud"; model: string }
  | { provider: "openai_compatible"; base_url: string; model: string }
  | { provider: "anthropic"; model: string };

export type ProviderId = ProviderChoice["provider"];

export interface ProviderInfo {
  /** `"groq"` solo en resultados antiguos: el proveedor se retiró (D100) y ya no se puede elegir. */
  provider: ProviderId | "groq";
  model: string;
}

/**
 * Por qué las tareas con IA esperan en la cola (D101, D146). `model_unavailable` (404 o 410 al
 * generar) y `no_credit` (402) nacen de la respuesta del proveedor, no de la configuración.
 */
export type AiWait = "needs_choice" | "needs_key" | "needs_consent" | "invalid_key" | "model_unavailable" | "no_credit";

/** Qué indicó el proveedor con una espera `model_unavailable`; `other`: no consta la causa exacta. */
export type AiWaitCause = "model_not_found" | "route_not_found" | "other";

export interface SettingsView {
  /** `null`: hay que elegir proveedor (D100). */
  choice: ProviderChoice | null;
  active: ProviderInfo;
  ai_wait: AiWait | null;
  ai_wait_message: string | null;
  /** Con `model_unavailable`, la causa enumerada; con `no_credit`, `null`. */
  ai_wait_cause: AiWaitCause | null;
  /** Estado HTTP que originó una espera de D146. */
  ai_wait_status: number | null;
  /** Respaldo del proveedor externo elegido a la IA local (D101). */
  local_fallback: boolean;
  /** Las tareas van directas a la IA local durante 10 minutos por un fallo del externo. */
  fallback_period_active: boolean;
  /** Destino de la elección actual ya confirmado (siempre `true` con la IA local). */
  consented: boolean;
  /** Oferta única de pasar al proveedor externo a quien tenía la IA local (D100). */
  offer_external: boolean;
  /** Modelo recomendado de Ollama cloud: `DEFAULT_CLOUD_MODEL` del backend (D108). */
  default_cloud_model: string;
  /** Modelo y dirección de la IA local, para el botón explícito «Usar la IA local» (D146). */
  default_local_model: string;
  default_local_url: string;
  data_dir: string;
}

// Preparación de componentes locales (R08, contrato «Preparación de componentes locales»).

export type SetupStep = "ocr" | "ollama" | "service" | "model" | "verify";
export type StepState = "running" | "done" | "skipped" | "failed";
export type SetupErrorCode =
  | "network"
  | "signature"
  | "install_failed"
  | "unavailable"
  | "model_failed"
  | "verify_failed"
  | "ocr_missing"
  | "cancelled";

export interface SetupError {
  code: SetupErrorCode;
  message: string;
}

export type SetupCheck =
  | { status: "ready" }
  | { status: "stopped" }
  | { status: "missing"; component: "ollama" | "ollama_outdated" | "model" | "ocr" };

// Memoria del equipo frente a la IA local (D46): solo avisa, nunca bloquea.
export type MemoryLevel = "recommended" | "below_recommended" | "insufficient" | "unknown";

export interface Memory {
  total_bytes: number | null;
  available_bytes: number | null;
  level: MemoryLevel;
  warning: string | null;
}

export type SetupStatus = SetupCheck & { memory: Memory };

export interface SetupEvent {
  step: SetupStep;
  state: StepState;
  done: number | null;
  total: number | null;
  message: string;
}

export interface SetupReport {
  ollama_version: string;
  ollama_installed: boolean;
  model: string;
  model_digest: string | null;
  model_pinned: boolean | null;
  answer: string;
  latency_ms: number;
  memory: Memory;
}

export interface SetupDone {
  report: SetupReport | null;
  error: SetupError | null;
}

export type Startup =
  | { outcome: "not_local" }
  | { outcome: "ocr_missing" }
  | { outcome: "ready" }
  | { outcome: "started" }
  | { outcome: "needs_preparation"; check: SetupCheck }
  | { outcome: "failed"; error: SetupError };

export interface LaunchView {
  prepare_only: boolean;
  preparing: boolean;
  startup: Startup | null;
}


// Cuentas de correo (R13, D41–D44).

export type MailProviderKind = "simulator" | "google" | "microsoft";
export type AccountStatus = "connected" | "signed_out" | "not_configured" | "credentials_unavailable";

export interface MailIssue {
  code: "unauthorized" | "unavailable" | "not_found" | "conflict" | "invalid_input" | string;
  message: string;
}

export interface AccountFetch {
  account: string;
  read: number;
  added: number;
  /** Mensajes nuevos que la ruta local cerró como informativos, sin IA (D157). */
  informative?: number;
  error: MailIssue | null;
  attachments: AttachmentFetch;
}

/** Adjuntos tratados en una consulta; los fallidos se reintentan en la siguiente. */
export interface AttachmentFetch {
  imported: number;
  skipped: string[];
  failed: { name: string; code: string; message: string }[];
}

export interface FetchReport {
  added: number;
  limit: number;
  accounts: AccountFetch[];
}

export interface MailAccount {
  id: string;
  provider: MailProviderKind;
  label: string;
  /** Dirección del buzón (D48); null si no consta. */
  address: string | null;
  status: AccountStatus;
  last_fetch: AccountFetch | null;
}

export interface MailView {
  accounts: MailAccount[];
  limit: number;
  google_configured: boolean;
  microsoft_configured: boolean;
  clients_error: string | null;
  signing_in: boolean;
}

// Origen de correos y documentos (D48, D49). Lo que falta llega como null, nunca inventado.

/** Cuenta de la que vino un correo, copiada en la tarea al encolarlo. */
export interface MailOrigin {
  account: string;
  provider: MailProviderKind;
  label: string;
  address: string | null;
}

export type DocumentOrigin =
  | {
      kind: "file";
      name: string;
      original_path: string | null;
      /** Segundos desde 1970. */
      imported_at: number | null;
      sha256: string;
      size: number;
    }
  | {
      /** Adjunto de un correo ya recibido (`import_attachment`). */
      kind: "email";
      account: MailOrigin;
      message_id: string | null;
      from: string | null;
      subject: string | null;
      attachment: string | null;
      imported_at: number | null;
      sha256: string;
      size: number;
    };

/** Dónde aparece un campo extraído en el texto del documento. */
export interface FieldSource {
  pages: number[];
  text: string;
  line: string;
}

// ---- Sugerencias de reglas (D132, R24) ----

/** Datos de la serie que sostiene una sugerencia. Todas las cifras las calcula el backend. */
export interface SuggestionEvidence {
  count: number;
  threshold: number;
  /** Veces que la serie se cortó antes (edición, rechazo, sin clasificar o factura con incidencias). */
  earlier_breaks: number;
  /** De `count`: aprobaciones de la persona y facturas que solo se vieron en la primera lectura (E5, D133). */
  approved: number;
  seen: number;
  /** Cuándo se guardó la última observación (ms). */
  last_at: number;
  task_ids: number[];
  /** Facturas: los importes observados, en orden. */
  amounts: string[];
  /** Facturas: límite propuesto, el mayor de los importes observados. */
  limit: string | null;
  /** Facturas sin ninguna aprobada: la persona debe escribir el límite (D133). */
  needs_limit: boolean;
  /** Facturas: el menor y el segundo mayor de los importes observados. */
  amount_min: string | null;
  amount_second: string | null;
  /** Facturas: el mayor es al menos el triple del segundo mayor. */
  limit_outlier: boolean;
  reappeared: { at_discard: number; new_since: number; needed_new: number } | null;
}

export interface Suggestion {
  fingerprint: string;
  /** Huella de las condiciones mostradas: aceptar exige la misma. */
  shown: string;
  action_kind: string;
  name: string;
  conditions: Record<string, string>;
  subject: string;
  category: string | null;
  evidence: SuggestionEvidence;
  /** Contradicciones que crearía la regla si se activara (la candidata va como `(sugerida)`). */
  overlaps: RuleFinding[];
}

export interface SuggestionProgress {
  action_kind: string;
  subject: string;
  category: string | null;
  count: number;
  approved: number;
  seen: number;
  needed: number;
  missing: number;
}

export interface ThresholdSettings {
  invoice: number;
  reply: number;
  min: number;
  max: number;
  default_invoice: number;
  default_reply: number;
}

export interface Suggestions {
  candidates: Suggestion[];
  progress: SuggestionProgress[];
  /** Señales válidas que se miraron. Cero: aún no hay datos. */
  observed: number;
  /** De ellas, decisiones de la persona (aprobar, editar o rechazar) y facturas vistas en la primera lectura. */
  decisions: number;
  seen: number;
  /** Observaciones que cuentan como evidencia en las series actuales, se muestren o no. */
  counted: number;
  hidden: { covered: number; discarded: number; accepted: number; invalid: number; not_suggested: number; seen_invalidated: number };
  thresholds: ThresholdSettings;
  watermark: number;
}

// ---- Capa con modelo de las sugerencias (D134) ----

export type RefineMode = "on_demand" | "automatic";

/** Consentimiento del afinado automático (solo la versión final con un proveedor externo). */
export interface ConsentView {
  required: boolean;
  /** `not_needed`, `missing`, `granted` o `stale` (el proveedor o el modelo cambiaron). */
  state: "not_needed" | "missing" | "granted" | "stale";
  provider: string | null;
  model: string | null;
  /** Servidor (host) al que se consintió: el consentimiento está ligado a proveedor, modelo y servidor. */
  host: string | null;
  at: number | null;
}

/** Candidata que la IA propone ocultar: no se borra, sigue visible y recuperable. */
export interface HiddenByModel {
  fingerprint: string;
  /** `low_value`, `likely_one_off` o `unclear_name`. */
  reason: string;
}

/** Límite propuesto por la IA que pasó `check_rule` y la simulación. */
export interface LimitProposal {
  fingerprint: string;
  max_total: string;
  /** Huella de las condiciones con este límite: es la que exige aceptar la propuesta. */
  shown: string;
  considered: number;
  would_automate: number;
}

export interface DroppedProposal {
  kind: string;
  fingerprint: string;
  code: string;
}

/** Estado del afinado frente a las sugerencias de ahora. Las cifras las pone el backend. */
export interface RefinementView {
  mode: RefineMode;
  status: "refined" | "stale" | "running" | "not_refined";
  reason: string | null;
  detail: string | null;
  ran_at: number | null;
  /** Cuándo se obtuvo el último resultado válido, aunque ya no esté vigente. */
  last_ok_at: number | null;
  provider: string | null;
  model: string | null;
  fallback: { from: string; reason: string } | null;
  /** Huellas en orden de utilidad. */
  order: string[];
  hide: HiddenByModel[];
  /** Grupos de huellas cuyo proveedor quizá es el mismo: solo una pregunta. */
  groups: string[][];
  limits: LimitProposal[];
  dropped: DroppedProposal[];
  /** Texto plano de la IA. */
  comment: string | null;
  /** Solo se descartó el comentario (cifras en letra o afirmaciones); el resto del afinado se conserva. */
  comment_dropped: string | null;
  /** Candidatas actuales, cuántas se envían (tope de 30) y a cuántas se aplica el resultado. */
  candidates: number;
  sent: number;
  covered: number;
  new_signals: number;
  min_new_signals: number;
  retry_at: number | null;
  gave_up: boolean;
  failures: number;
  max_attempts: number;
  consent: ConsentView;
}

/** Lo que se enviaría al afinar, candidata a candidata. */
export interface RefineSentCandidate {
  id: string;
  kind: "invoice" | "reply";
  supplier: string | null;
  category: string | null;
  sender_domain: string | null;
  count: number;
  /** De `count`, las que solo se vieron en la primera lectura y la persona nunca aprobó. */
  seen: number;
  earlier_breaks: number;
  amount_min: string | null;
  amount_max: string | null;
  discarded_before: boolean;
  /** Por qué no se envía el nombre o el dominio: `email`, `id_number`, `iban`, `phone`, `address`, `public_mail_domain`. */
  withheld: string | null;
}

export interface RefinePreview {
  mode: RefineMode;
  provider: { provider: string; model: string };
  /** Servidor real al que saldrían los datos. */
  host: string | null;
  /** Huella de lo que se enviaría: `refine_suggestions` rechaza si ya no coincide. */
  input_fp: string;
  external: boolean;
  via_local_first: boolean;
  local_fallback: boolean;
  blocked: string | null;
  blocked_message: string | null;
  consent: ConsentView;
  candidates: RefineSentCandidate[];
  candidates_total: number;
  discarded_by_user: number;
  never_sent: string[];
}

/** Datos para precargar una regla desde una revisión. */
export interface RuleSeed {
  action_kind: string;
  conditions: Record<string, string>;
  name: string;
  /** Facturas: el límite es el total de esa factura, no una estadística. */
  limit_from_this_invoice: boolean;
  /** Facturas: por qué no se precargó el límite (solo el proveedor). */
  limit_skipped: { code: string; currencies: string[] } | null;
}

// ---- Primera lectura (E5, D133) ----

/** Estado de la lectura de una fuente (`account:<proveedor>:<dirección>` o `folder:<huella>`), calculado con los datos. */
export type ReadState = "not_started" | "waiting" | "pending" | "interrupted" | "listing_failed" | "stopped" | "held" | "reading" | "done_with_errors" | "done";

export interface ReadFailure {
  task_id: number;
  kind: string;
  /** Nombre del documento. */
  name: string | null;
  code: string;
  message: string;
  recoverable: boolean;
}

export interface ReadIssue {
  code: string;
  message: string;
}

export interface ReadDestination {
  provider: string;
  model: string;
  host: string | null;
}

export interface InitialReadStatus {
  source: string;
  kind: "account" | "folder";
  state: ReadState;
  volume: number | null;
  started_at: number | null;
  provider: string | null;
  model: string | null;
  host: string | null;
  listed: number;
  already_known: number;
  /** Correos de los últimos días o sin fecha legible: siguen el flujo normal. */
  recent_left_out: number;
  /** Correos que quedaron sin clasificar: no se envían. */
  emails_unclassified: number;
  /** Documentos enviados al proveedor, omitidos por no parecer factura y no leídos por el tope. */
  documents_sent: number;
  documents_skipped: number;
  over_cap: number;
  tasks: { total: number; queued: number; running: number; completed: number; failed: number; cancelled: number };
  by_provider: Array<{ provider: string; model: string; host: string | null; documents: number }>;
  failures: ReadFailure[];
  attachment_failures: Array<{ name: string; code: string; message: string }>;
  skipped_attachments: number;
  listing_error: ReadIssue | null;
  scan_capped: boolean;
  remaining: number;
  fallback_used: number;
  /** Envíos o registros hechos por las tareas de esta lectura: debe ser 0. */
  actions: number;
  waiting_message: string | null;
  /** Lo pendiente no sale: detenida, destino cambiado o cuenta quitada. */
  held: { reason: "stopped" | "destination_changed" | "removed"; authorized: ReadDestination | null; now: ReadDestination } | null;
  removed: boolean;
}

export interface ReadSource {
  source: string;
  /** Identificador de la cuenta o ruta de la carpeta. */
  target: string;
  label: string;
  address: string | null;
  status: InitialReadStatus;
}

export interface ReadSuggestions {
  seen_documents: number;
  ready: number;
  closest: number;
  needed: number;
  error: string | null;
}

export interface InitialReadOverview {
  mode: RefineMode;
  limits: { min: number; max: number; fixed: number | null; default: number };
  recent_days: number;
  documents_cap: number;
  provider: { provider: string; model: string };
  host: string | null;
  external: boolean;
  local_fallback: boolean;
  blocked: string | null;
  blocked_message: string | null;
  sends: string[];
  never_sent: string[];
  accounts: ReadSource[];
  removed: ReadSource[];
  folders: ReadSource[];
  suggestions: ReadSuggestions;
}

/** Una factura que solo se leyó en la primera lectura. */
export interface SeenInvoice {
  task_id: number;
  document: string | null;
  date: string | null;
  amount: string | null;
  read_at: number;
}

export interface FolderPreview {
  source: string;
  path: string;
  files: number;
  status: InitialReadStatus;
}

/** Lo que la persona vio antes de pedir la lectura: si algo cambia, el backend no lee. */
export interface ReadExpectation {
  provider: string;
  model: string;
  host: string;
  volume: number;
}
