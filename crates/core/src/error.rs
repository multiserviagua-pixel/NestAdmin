use crate::queue::TaskState;

/// Errores del núcleo. Nunca contienen secretos: solo identificadores y estados.
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("almacenamiento: {0}")]
    Storage(#[from] rusqlite::Error),
    #[error("serialización: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("tarea {0} inexistente")]
    TaskNotFound(i64),
    #[error("transición no permitida de {from} a {to} en la tarea {task}")]
    InvalidTransition {
        task: i64,
        from: TaskState,
        to: TaskState,
    },
    #[error("esquema de base de datos {found} más reciente que el soportado ({supported})")]
    UnsupportedSchema { found: i64, supported: i64 },
    #[error("la clave {0} ya existe con otro contenido")]
    IdempotencyConflict(String),
    #[error("conector: {0}")]
    Connector(#[from] crate::actions::ConnectorError),
}

pub type Result<T> = std::result::Result<T, CoreError>;
