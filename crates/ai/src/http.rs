use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use serde_json::Value;
use ureq::config::Config;
use ureq::unversioned::resolver::{DefaultResolver, ResolvedSocketAddrs, Resolver};
use ureq::unversioned::transport::{DefaultConnector, NextTimeout};
use ureq::{http::Uri, Agent};

use crate::openai_compatible::is_blocked_ip;
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

/// Igual que `agent`, pero sin seguir redirecciones (D95, D96): una credencial externa solo
/// viaja al host validado, nunca a otro al que apunte un `Location`. Una respuesta 3xx se
/// informa como proveedor no disponible.
pub(crate) fn agent_no_redirects(timeout: Duration) -> Agent {
    Agent::config_builder()
        .timeout_global(Some(timeout))
        .timeout_connect(Some((timeout / 2).min(Duration::from_secs(2))))
        .http_status_as_error(false)
        .max_redirects(0)
        .proxy(None)
        .build()
        .into()
}

/// Error del resolvedor: el host solo resuelve a direcciones no públicas (D95).
#[derive(Debug)]
struct NonPublicHost;

impl std::fmt::Display for NonPublicHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("el host resuelve solo a direcciones no públicas")
    }
}

impl std::error::Error for NonPublicHost {}

/// Descarta todas las direcciones locales, privadas o reservadas. Sin ninguna pública,
/// error: no se conecta a nada (D95).
pub(crate) fn public_addrs_only(addrs: Vec<SocketAddr>) -> Result<Vec<SocketAddr>, AiError> {
    let public: Vec<SocketAddr> = addrs
        .into_iter()
        .filter(|a| !is_blocked_ip(&normalize_ip(a.ip())))
        .collect();
    if public.is_empty() {
        Err(AiError::Unavailable(NonPublicHost.to_string()))
    } else {
        Ok(public)
    }
}

fn normalize_ip(ip: IpAddr) -> IpAddr {
    ip.to_canonical()
}

/// Resolvedor que filtra el DNS en el momento de conectar, de modo que cubre también el
/// rebinding (D95).
#[derive(Debug, Default)]
struct PublicOnlyResolver {
    inner: DefaultResolver,
}

impl Resolver for PublicOnlyResolver {
    fn resolve(
        &self,
        uri: &Uri,
        config: &Config,
        timeout: NextTimeout,
    ) -> Result<ResolvedSocketAddrs, ureq::Error> {
        let resolved = self.inner.resolve(uri, config, timeout)?;
        let public = public_addrs_only(resolved.iter().copied().collect())
            .map_err(|_| ureq::Error::Other(Box::new(NonPublicHost)))?;
        let mut out = self.empty();
        for addr in public {
            out.push(addr);
        }
        Ok(out)
    }
}

/// Agente de los proveedores externos de destino configurable o público: sin redirecciones y
/// con el resolvedor que descarta direcciones no públicas (D95).
pub(crate) fn agent_external(timeout: Duration) -> Agent {
    let config = Agent::config_builder()
        .timeout_global(Some(timeout))
        .timeout_connect(Some((timeout / 2).min(Duration::from_secs(2))))
        .http_status_as_error(false)
        .max_redirects(0)
        .proxy(None)
        .build();
    Agent::with_parts(
        config,
        DefaultConnector::default(),
        PublicOnlyResolver::default(),
    )
}

/// POST JSON y respuesta JSON, con los errores del contrato. `bearer` nunca aparece en
/// los errores devueltos.
pub(crate) fn post_json(
    agent: &Agent,
    url: &str,
    bearer: Option<&str>,
    body: &Value,
) -> Result<Value, AiError> {
    match bearer {
        Some(token) => post_json_with_headers(
            agent,
            url,
            &[("Authorization", &format!("Bearer {token}"))],
            body,
        ),
        None => post_json_with_headers(agent, url, &[], body),
    }
}

/// POST JSON con cabeceras adicionales (por ejemplo `x-api-key`). Ningún valor de cabecera
/// aparece en los errores devueltos.
pub(crate) fn post_json_with_headers(
    agent: &Agent,
    url: &str,
    headers: &[(&str, &str)],
    body: &Value,
) -> Result<Value, AiError> {
    let mut request = agent.post(url);
    for (name, value) in headers {
        request = request.header(*name, *value);
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
        400..=499 => {
            let kind = response
                .body_mut()
                .with_config()
                .limit(64 * 1024)
                .read_to_string()
                .ok()
                .and_then(|body| error_type(&body));
            Err(AiError::Unavailable(match kind {
                Some(kind) => format!("estado HTTP {status} ({kind})"),
                None => format!("estado HTTP {status}"),
            }))
        }
        _ => Err(AiError::Unavailable(format!("estado HTTP {status}"))),
    }
}

/// Solo `error.type` de un cuerpo de error, si es un identificador corto `[a-z_]{1,64}`
/// (por ejemplo `invalid_request_error`). Nunca `error.message` ni el cuerpo completo: podrían
/// repetir contenido enviado.
pub(crate) fn error_type(body: &str) -> Option<String> {
    let value: Value = serde_json::from_str(body).ok()?;
    let kind = value.pointer("/error/type")?.as_str()?;
    let valid =
        (1..=64).contains(&kind.len()) && kind.bytes().all(|b| b.is_ascii_lowercase() || b == b'_');
    valid.then(|| kind.to_owned())
}

fn map_error(error: ureq::Error) -> AiError {
    match error {
        ureq::Error::Other(inner) if inner.is::<NonPublicHost>() => {
            AiError::Unavailable(NonPublicHost.to_string())
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn sa(ip: &str) -> SocketAddr {
        SocketAddr::new(ip.parse().unwrap(), 443)
    }

    #[test]
    fn resolver_filter_keeps_only_public_addresses() {
        let mixed = vec![
            sa("127.0.0.1"),
            sa("93.184.216.34"),
            sa("10.0.0.5"),
            sa("::1"),
            sa("2606:2800:220:1::1"),
            sa("::ffff:192.168.1.1"),
            sa("169.254.169.254"),
        ];
        let kept = public_addrs_only(mixed).unwrap();
        assert_eq!(kept, vec![sa("93.184.216.34"), sa("2606:2800:220:1::1")]);
    }

    #[test]
    fn resolver_filter_fails_when_all_are_non_public() {
        let all_private = vec![
            sa("127.0.0.1"),
            sa("192.168.0.1"),
            sa("fe80::1"),
            sa("::ffff:127.0.0.1"),
        ];
        let error = public_addrs_only(all_private).unwrap_err();
        assert_eq!(error.code(), "unavailable");
        assert_eq!(public_addrs_only(vec![]).unwrap_err().code(), "unavailable");
    }

    #[test]
    fn external_agent_refuses_to_connect_to_a_loopback_address() {
        // El resolvedor se aplica al conectar: una IP local (como la que devolvería un DNS
        // malicioso o un rebinding) falla sin abrir ninguna conexión.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("https://{}/x", listener.local_addr().unwrap());
        let agent = agent_external(Duration::from_secs(2));
        let error = post_json(&agent, &url, None, &serde_json::json!({})).unwrap_err();
        assert_eq!(error.code(), "unavailable");
        assert!(error.to_string().contains("no públicas"), "{error}");
        let accepted = listener.accept();
        assert_eq!(
            accepted.unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock,
            "no debe haber conexión"
        );
    }

    #[test]
    fn error_type_accepts_only_short_identifiers() {
        assert_eq!(
            error_type(r#"{"error":{"type":"invalid_request_error","message":"x"}}"#).as_deref(),
            Some("invalid_request_error")
        );
        assert_eq!(error_type(r#"{"error":{"type":"Bad Type!"}}"#), None);
        assert_eq!(
            error_type(&format!(r#"{{"error":{{"type":"{}"}}}}"#, "a".repeat(65))),
            None
        );
        assert_eq!(error_type("no json"), None);
    }
}
