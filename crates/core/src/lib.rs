//! Núcleo local de NestAdmin (decisiones D01 y D03): almacenamiento SQLite,
//! cola durable con recuperación y acciones idempotentes con historial.

pub mod actions;
pub mod error;
pub mod queue;
pub mod store;

pub use actions::{
    ActionOutcome, ActionRequest, AuditEntry, Authorization, Connector, ConnectorError,
};
pub use error::{CoreError, Result};
pub use queue::{Task, TaskError, TaskState};
pub use store::Store;
