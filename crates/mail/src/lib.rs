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

/// Código de error que devuelve el proveedor, apto para un mensaje: solo un identificador
/// corto (`invalid_grant`, `InvalidAuthenticationToken`…). Un texto libre, o uno que cite
/// alguna de las credenciales de la petición, se sustituye entero: filtrar caracteres
/// dejaría fragmentos del secreto (R09, D35).
pub(crate) fn provider_code(raw: Option<&str>, secrets: &[&str]) -> String {
    match raw {
        Some(code)
            if (1..=40).contains(&code.len())
                && code.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                && !secrets
                    .iter()
                    .any(|s| !s.is_empty() && (code.contains(s) || s.contains(code))) =>
        {
            code.to_owned()
        }
        _ => "sin detalle".to_owned(),
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
    use super::{checked_url, provider_code};

    #[test]
    fn provider_codes_never_carry_secrets_or_free_text() {
        let secret = "ya29_PRUEBA_SINTETICA";
        assert_eq!(
            provider_code(Some("invalid_grant"), &[secret]),
            "invalid_grant"
        );
        assert_eq!(provider_code(None, &[]), "sin detalle");
        for raw in [
            "ya29_PRUEBA_SINTETICA",
            "token_ya29_PRUEBA_SINTETICA_caducado",
            "PRUEBA",
            "Bearer ya29.PRUEBA",
            "texto libre con espacios",
            "",
        ] {
            assert_eq!(provider_code(Some(raw), &[secret]), "sin detalle", "{raw}");
        }
    }

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
