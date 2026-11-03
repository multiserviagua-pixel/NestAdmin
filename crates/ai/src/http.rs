use std::time::Duration;

use serde_json::Value;
use ureq::Agent;

use crate::provider::AiError;

/// Agente sin proxy: ni las variables `HTTP_PROXY` del sistema desvían las peticiones (D12).
/// La conexión tiene un plazo propio más corto: en Windows un puerto local cerrado no se
/// rechaza al instante, y ese caso debe informarse como proveedor no disponible, no como
/// un modelo lento.
pub(crate) fn agent(timeout: Duration) -> Agent {
    Agent::config_builder()
        .timeout_global(Some(timeout))
        .timeout_connect(Some((timeout / 2).min(Duration::from_secs(2))))
        .http_status_as_error(false)
        .proxy(None)
        .build()
        .into()
}

/// POST JSON y respuesta JSON, con los errores del contrato. `bearer` nunca aparece en
/// los errores devueltos.
pub(crate) fn post_json(
    agent: &Agent,
    url: &str,
    bearer: Option<&str>,
    body: &Value,
) -> Result<Value, AiError> {
    let mut request = agent.post(url);
    if let Some(token) = bearer {
        request = request.header("Authorization", &format!("Bearer {token}"));
    }
    let mut response = request.send_json(body).map_err(map_error)?;
    let status = response.status().as_u16();
    match status {
        200..=299 => response
            .body_mut()
            .read_json::<Value>()
            .map_err(|_| AiError::InvalidResponse("cuerpo de respuesta no es JSON".into())),
        401 | 403 => Err(AiError::Unauthorized),
        408 | 504 => Err(AiError::Timeout),
        _ => Err(AiError::Unavailable(format!("estado HTTP {status}"))),
    }
}

fn map_error(error: ureq::Error) -> AiError {
    match error {
        ureq::Error::Timeout(ureq::Timeout::Connect | ureq::Timeout::Resolve) => {
            AiError::Unavailable("no se pudo conectar".into())
        }
        ureq::Error::Timeout(_) => AiError::Timeout,
        ureq::Error::Json(_) => AiError::InvalidResponse("JSON inválido".into()),
        ureq::Error::Io(e) if e.kind() == std::io::ErrorKind::TimedOut => AiError::Timeout,
        ureq::Error::ConnectionFailed | ureq::Error::HostNotFound | ureq::Error::Io(_) => {
            AiError::Unavailable("no se pudo conectar".into())
        }
        other => AiError::Unavailable(format!("error de transporte: {}", kind_only(&other))),
    }
}

/// Nombre de la variante, sin detalles que pudieran incluir URL o cabeceras.
fn kind_only(error: &ureq::Error) -> String {
    let debug = format!("{error:?}");
    debug
        .split(|c: char| !c.is_alphanumeric())
        .next()
        .unwrap_or("desconocido")
        .to_owned()
}
