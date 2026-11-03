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

export type SetupComponent = "webview2" | "pdfium" | "tessdata" | "tesseract" | "ollama" | "model";

export interface ComponentStatus {
  component: SetupComponent;
  state: "present" | "stopped" | "missing" | "incompatible";
  version?: string | null;
  found?: string;
  required?: string;
  installable: boolean;
  detail: string;
}

export interface Preparation {
  components: ComponentStatus[];
  installing: SetupComponent | null;
}

export interface SetupProgress {
  component: string;
  step: string;
  done: number | null;
  total: number | null;
}

export interface SetupEvent {
  component: SetupComponent;
  progress: SetupProgress | null;
  finished: boolean;
  error: string | null;
}

export interface Verification {
  model: string;
  answer: string;
  latency_ms: number;
}
