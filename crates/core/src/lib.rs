//! Núcleo local de NestAdmin (decisiones D01 y D03): almacenamiento SQLite,
//! cola durable con recuperación y acciones idempotentes con historial.

pub mod actions;
pub mod error;
pub mod pipeline;
pub mod queue;
pub mod rule_panel;
pub mod rules;
pub mod sales;
pub mod store;

pub use actions::{
    ActionOutcome, ActionRecord, ActionRequest, AuditEntry, Authorization, Connector,
    ConnectorError,
};
pub use error::{CoreError, Result};
pub use pipeline::{BlockingStep, NewStep, PipelineStep};
pub use queue::{Task, TaskError, TaskState};
pub use rule_panel::{
    EvaluationRecord, EvaluationRow, RuleMeta, RuleStats, SignalRecord, SignalRow, SuggestionRow,
    READ_MARK_PREFIX, REFINEMENT_KEY,
};
pub use rules::Rule;
pub use store::Store;
