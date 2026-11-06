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

export interface AuditEntry {
  task_id: number | null;
  action: string;
  status: string;
  authorization: string | null;
  detail: string | null;
}

export type ProviderChoice =
  | { provider: "ollama"; base_url: string; model: string }
  | { provider: "groq"; model: string };

export interface ProviderInfo {
  provider: "ollama" | "groq";
  model: string;
}

export interface SettingsView {
  choice: ProviderChoice;
  active: ProviderInfo;
  groq_key_saved: boolean;
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
