//! Núcleo local de NestAdmin (decisiones D01 y D03): almacenamiento SQLite,
//! cola durable con recuperación y acciones idempotentes con historial.

pub mod actions;
pub mod business_profile;
pub mod continuous;
pub mod error;
pub mod mail_signals;
pub mod pipeline;
pub mod preferences;
pub mod queue;
pub mod rule_panel;
pub mod rules;
pub mod sales;
pub mod store;
pub mod templates;

pub use actions::{
    ActionOutcome, ActionRecord, ActionRequest, AuditEntry, Authorization, Connector,
    ConnectorError,
};
pub use business_profile::{BusinessProfile, MAX_PROFILE_CHARS};
pub use error::{CoreError, Result};
pub use pipeline::{BlockingStep, NewStep, PipelineRun, PipelineStep, ATTACHMENT_REJECTED};
pub use preferences::{normalize_key, PreferenceScope, SenderPreference};
pub use queue::{Task, TaskError, TaskState};
pub use rule_panel::{
    is_learned_origin, BatchApplied, BatchRule, EvaluationRecord, EvaluationRow, LearnedEvidence,
    LearnedSignal, RodajeState, RuleMeta, RuleStats, SignalRecord, SignalRow, SuggestionRow,
    LEARN_MARK_PREFIX, LEARN_RESULT_PREFIX, READ_MARK_PREFIX, REFINEMENT_KEY, RODAJE_SENDS,
};
pub use rules::Rule;
pub use store::Store;
pub use templates::{active_since, TaskTemplate};
