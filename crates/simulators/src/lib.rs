//! Simuladores locales de correo y administración (R06). Implementan los contratos de
//! contratos.md (traits `MailConnector` y `AdminConnector`, que extienden el `Connector` del
//! núcleo) sin ningún cliente de red: el estado vive en memoria y, opcionalmente, en un
//! archivo JSON local para sobrevivir a un reinicio. Todos los datos de ejemplo son sintéticos.

pub mod admin;
pub mod contracts;
pub mod mail;
mod state;

pub use admin::{AdminSimulator, Invoice, InvoiceCriteria, InvoiceMatch, InvoiceReceipt};
pub use contracts::{AdminConnector, MailConnector, MAX_ATTACHMENT_BYTES};
pub use mail::{Attachment, Draft, MailSimulator, Message, MessagePage, NewDraft, SendReceipt};
pub use nestadmin_core::ConnectorError;

/// Resultado de una operación de simulador; los errores usan los códigos de contratos.md.
pub type SimResult<T> = std::result::Result<T, ConnectorError>;

/// Códigos de error de los contratos de correo y administración.
pub mod codes {
    pub const NOT_FOUND: &str = "not_found";
    pub const CONFLICT: &str = "conflict";
    pub const UNAVAILABLE: &str = "unavailable";
    pub const UNAUTHORIZED: &str = "unauthorized";
    pub const INVALID_INPUT: &str = "invalid_input";
}

/// Fallo forzado para pruebas: mientras está activo, toda operación lo devuelve sin efectos.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureMode {
    Unavailable,
    Unauthorized,
}

impl FailureMode {
    pub fn code(self) -> &'static str {
        match self {
            Self::Unavailable => codes::UNAVAILABLE,
            Self::Unauthorized => codes::UNAUTHORIZED,
        }
    }
}

pub(crate) fn error(code: &str, message: impl Into<String>) -> ConnectorError {
    ConnectorError {
        code: code.to_owned(),
        message: message.into(),
    }
}

/// Exige una clave de idempotencia no vacía.
pub(crate) fn check_key(key: &str) -> SimResult<()> {
    if key.trim().is_empty() {
        return Err(error(
            codes::INVALID_INPUT,
            "falta la clave de idempotencia",
        ));
    }
    Ok(())
}
