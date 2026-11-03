//! Conectores de correo reales (R13, D30–D35). Microsoft Graph y Gmail implementan el
//! contrato `MailConnector` igual que el simulador, de modo que la capa de aplicación no
//! distingue entre ellos. Cada cuenta es un `Mailbox` independiente; la lógica de
//! idempotencia y reconciliación es común y cada proveedor solo aporta sus llamadas.

pub mod gmail;
pub mod graph;
mod http;
mod mailbox;
pub mod oauth;
mod state;
pub mod text;

use nestadmin_core::ConnectorError;
use nestadmin_simulators::codes;
use url::Url;

pub use gmail::Gmail;
pub use graph::Graph;
pub use mailbox::{MailProvider, Mailbox};
pub use oauth::{OAuthConfig, PendingAuthorization, TokenSource, Tokens};

pub(crate) type MailResult<T> = Result<T, ConnectorError>;

pub(crate) fn error(code: &str, message: impl Into<String>) -> ConnectorError {
    ConnectorError {
        code: code.to_owned(),
        message: message.into(),
    }
}

/// `http://` solo se admite en bucle local (servidores simulados de las pruebas); todo lo
/// demás debe ser HTTPS para que tokens y correo viajen cifrados (D32).
pub(crate) fn checked_url(text: &str) -> MailResult<Url> {
    let url = Url::parse(text)
        .map_err(|_| error(codes::INVALID_INPUT, "dirección de servicio no válida"))?;
    let loopback = matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"));
    match url.scheme() {
        "https" => Ok(url),
        "http" if loopback => Ok(url),
        _ => Err(error(
            codes::INVALID_INPUT,
            "solo se admiten direcciones HTTPS",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::checked_url;

    #[test]
    fn only_https_or_loopback_http_is_accepted() {
        assert!(checked_url("https://graph.microsoft.com/v1.0").is_ok());
        assert!(checked_url("http://127.0.0.1:8080/api").is_ok());
        assert!(checked_url("http://graph.microsoft.com/v1.0").is_err());
        assert!(checked_url("http://127.0.0.1.example.test/").is_err());
        assert!(checked_url("ftp://127.0.0.1/").is_err());
        assert!(checked_url("no es una url").is_err());
    }
}
