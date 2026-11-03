/// Errores de documentos. No contienen rutas ni el texto del documento: solo el campo
/// afectado y, para un valor ilegible, su literal, que el usuario necesita para revisarlo.
#[derive(Debug, thiserror::Error)]
pub enum DocumentError {
    #[error("formato {format} no soportado todavía")]
    UnsupportedFormat { format: String },
    #[error("el documento no es texto UTF-8 válido")]
    InvalidUtf8,
    #[error("el documento no contiene texto")]
    EmptyText,
    #[error("lectura del documento: {0}")]
    Io(#[from] std::io::Error),
    #[error("importe ilegible en {field}: {literal:?}")]
    InvalidAmount {
        field: &'static str,
        literal: String,
    },
    #[error("fecha ilegible en {field}: {literal:?}")]
    InvalidDate {
        field: &'static str,
        literal: String,
    },
}

impl DocumentError {
    /// Código estable para errores estructurados y para la interfaz.
    pub fn code(&self) -> &'static str {
        match self {
            Self::UnsupportedFormat { .. } => "unsupported_format",
            Self::InvalidUtf8 => "invalid_utf8",
            Self::EmptyText => "empty_text",
            Self::Io(_) => "io",
            Self::InvalidAmount { .. } => "invalid_amount",
            Self::InvalidDate { .. } => "invalid_date",
        }
    }
}

pub type Result<T> = std::result::Result<T, DocumentError>;
