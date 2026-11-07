use nestadmin_sales::{
    CalcError, CatalogError, FieldIssue, PaymentError, Problem, RecurringError, SeriesError,
    StatusError,
};

/// Errores de la persistencia de venta. Nunca contienen secretos ni datos personales completos:
/// solo identificadores, nombres de campo y estados.
#[derive(Debug, thiserror::Error)]
pub enum SalesStoreError {
    /// Falta un dato o es inválido (contratos.md `invalid_input`).
    #[error("datos no válidos: {message}")]
    InvalidInput {
        message: String,
        issues: Vec<FieldIssue>,
    },
    #[error("{what} {id} inexistente")]
    NotFound { what: &'static str, id: i64 },
    /// El estado del documento o de la serie no permite la operación.
    #[error("{0}")]
    Conflict(String),
    #[error("{0}")]
    Status(#[from] StatusError),
    #[error("{0}")]
    Series(#[from] SeriesError),
    #[error("{0}")]
    Payment(#[from] PaymentError),
    /// Fallo devuelto por un gancho de la capa de aplicación (p. ej. el registro Verifactu).
    #[error("{code}: {message}")]
    External { code: String, message: String },
    #[error("almacenamiento: {0}")]
    Storage(#[from] rusqlite::Error),
    /// Un dato guardado no se puede interpretar (base alterada o de otra versión).
    #[error("dato guardado ilegible: {0}")]
    Corrupt(String),
}

impl SalesStoreError {
    /// Código estable de contratos.md.
    pub fn code(&self) -> &str {
        match self {
            Self::InvalidInput { .. } => "invalid_input",
            Self::NotFound { .. } => "not_found",
            Self::Conflict(_) | Self::Series(_) => "conflict",
            Self::Status(e) => e.code(),
            Self::Payment(e) => e.code(),
            Self::External { code, .. } => code,
            Self::Storage(_) | Self::Corrupt(_) => "storage",
        }
    }

    pub fn invalid(field: &str, why: impl Into<String>) -> Self {
        let why = why.into();
        Self::InvalidInput {
            message: format!("{field}: {why}"),
            issues: vec![FieldIssue {
                field: field.to_owned(),
                problem: Problem::Invalid(why),
            }],
        }
    }

    pub fn from_issues(issues: Vec<FieldIssue>) -> Self {
        let message = issues
            .iter()
            .map(|i| match &i.problem {
                Problem::Missing => format!("falta {}", i.field),
                Problem::Invalid(why) => format!("{}: {why}", i.field),
            })
            .collect::<Vec<_>>()
            .join("; ");
        Self::InvalidInput { message, issues }
    }

    pub(crate) fn not_found(what: &'static str, id: i64) -> Self {
        Self::NotFound { what, id }
    }
}

impl From<CalcError> for SalesStoreError {
    fn from(e: CalcError) -> Self {
        Self::invalid("lines", e.to_string())
    }
}

impl From<CatalogError> for SalesStoreError {
    fn from(e: CatalogError) -> Self {
        Self::invalid("product", e.to_string())
    }
}

impl From<RecurringError> for SalesStoreError {
    fn from(e: RecurringError) -> Self {
        Self::invalid("template", e.to_string())
    }
}

impl From<serde_json::Error> for SalesStoreError {
    fn from(e: serde_json::Error) -> Self {
        Self::Corrupt(e.to_string())
    }
}

pub type SalesResult<T> = std::result::Result<T, SalesStoreError>;
