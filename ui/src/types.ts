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

