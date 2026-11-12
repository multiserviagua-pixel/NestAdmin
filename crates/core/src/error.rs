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
    #[error("la tarea {0} espera a que terminen los pasos anteriores de su pipeline")]
    TaskBlocked(i64),
    #[error("el paso {0} del pipeline no existe")]
    PipelineStepNotFound(i64),
    #[error("la ejecución {0} del pipeline no existe")]
    PipelineRunNotFound(i64),
    #[error(
        "una ejecución de pipeline necesita exactamente un paso cuya tarea sea la tarea sujeto"
    )]
    PipelineSubjectNotAStep,
    #[error("añadir un paso en la etapa {0} retendría una tarea de una etapa posterior que ya fue tomada")]
    PipelineStageTooLate(i64),
    #[error("el paso {0} no retiene ninguna tarea que pueda empezar, o ya se omitió")]
    PipelineStepNotRetaining(i64),
    #[error("la plantilla {0} no existe")]
    TemplateNotFound(String),
    #[error("la plantilla {0} está archivada; desarchívela para volver a usarla")]
    TemplateArchived(String),
    #[error(
        "esquema de base de datos {found} más reciente que el soportado ({supported}); \n         instale la versión de NestAdmin que lo creó o restaure la copia nestadmin.esquema-{supported}.db"
    )]
    UnsupportedSchema { found: i64, supported: i64 },
    #[error("no se pudo guardar la copia previa a la migración: {0}")]
    Backup(std::io::Error),
    #[error("la clave {0} ya existe con otro contenido")]
    IdempotencyConflict(String),
    #[error("regla {0} inexistente")]
    RuleNotFound(String),
    #[error("la regla {0} está archivada; desarchívela para volver a usarla")]
    RuleArchived(String),
    #[error("la regla {0} ha cambiado; revise de nuevo")]
    RuleChanged(String),
    #[error("filtro de búsqueda no válido: {0}")]
    InvalidFilter(String),
    #[error("la conversación {0} no existe")]
    ConversationNotFound(i64),
    #[error("wrong_database: se esperaba la base {expected} y esta es {found}")]
    WrongDatabase { expected: String, found: String },
    #[error("{code}: {message}")]
    Profile { code: &'static str, message: String },
    #[error("archivo: {0}")]
    Io(#[from] std::io::Error),
    #[error("el sistema no da bytes aleatorios: {0}")]
    Entropy(String),
    #[error("conector: {0}")]
    Connector(#[from] crate::actions::ConnectorError),
}

pub type Result<T> = std::result::Result<T, CoreError>;
