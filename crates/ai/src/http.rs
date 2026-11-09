use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use serde_json::Value;
use ureq::config::Config;
use ureq::unversioned::resolver::{DefaultResolver, ResolvedSocketAddrs, Resolver};
use ureq::unversioned::transport::{DefaultConnector, NextTimeout};
use ureq::{http::Uri, Agent};

use crate::openai_compatible::is_blocked_ip;
use crate::provider::{AiError, ModelUnavailableCause};

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

/// Igual que `agent`, pero sin seguir redirecciones (D109, D110): una credencial externa solo
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

/// Error del resolvedor: el host solo resuelve a direcciones no públicas (D109).
#[derive(Debug)]
struct NonPublicHost;

impl std::fmt::Display for NonPublicHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("el host resuelve solo a direcciones no públicas")
    }
}

impl std::error::Error for NonPublicHost {}

/// Descarta todas las direcciones locales, privadas o reservadas. Sin ninguna pública,
/// error: no se conecta a nada (D109).
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
/// rebinding (D109).
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
/// con el resolvedor que descarta direcciones no públicas (D109).
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
    post_json_as(agent, url, headers, body, Endpoint::Generate)
}

/// POST JSON de la IA local (`/api/show`, `/api/chat`): sin el mapeo de D146, un 4xx sigue
/// siendo `unavailable` (D101, R03).
pub(crate) fn post_json_local(agent: &Agent, url: &str, body: &Value) -> Result<Value, AiError> {
    post_json_as(agent, url, &[], body, Endpoint::Plain)
}

fn post_json_as(
    agent: &Agent,
    url: &str,
    headers: &[(&str, &str)],
    body: &Value,
    endpoint: Endpoint,
) -> Result<Value, AiError> {
    let mut request = agent.post(url);
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let response = request.send_json(body).map_err(map_error)?;
    response_json(response, endpoint)
}

/// GET con respuesta JSON y cabeceras adicionales; mismos errores del contrato que el POST.
/// Ningún valor de cabecera aparece en los errores devueltos.
pub(crate) fn get_json_with_headers(
    agent: &Agent,
    url: &str,
    headers: &[(&str, &str)],
) -> Result<Value, AiError> {
    let mut request = agent.get(url);
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let response = request.call().map_err(map_error)?;
    response_json(response, Endpoint::Plain)
}

/// Qué se pidió: del mapeo depende el significado de un 4xx. Al generar con un proveedor externo
/// (POST de chat), un 404 es una configuración que no sirve (D146). Al listar modelos (GET) y con
/// la IA local, incluidos `/api/show` y la verificación del asistente, un 4xx sigue siendo
/// `unavailable` (D101, R03): el mapeo nuevo solo es de proveedores externos.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Endpoint {
    Generate,
    Plain,
}

/// Estado HTTP de una respuesta a los errores del contrato (compartido por GET y POST).
fn response_json(
    mut response: ureq::http::Response<ureq::Body>,
    endpoint: Endpoint,
) -> Result<Value, AiError> {
    let status = response.status().as_u16();
    match status {
        200..=299 => response
            .body_mut()
            .read_json::<Value>()
            .map_err(|_| AiError::InvalidResponse("cuerpo de respuesta no es JSON".into())),
        401 | 403 => Err(AiError::Unauthorized),
        429 => Err(AiError::RateLimited {
            retry_after_ms: response
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(retry_after_ms),
        }),
        408 | 504 => Err(AiError::Timeout),
        400..=499 => {
            // El cuerpo se lee con límite, se interpreta y se descarta: nunca se guarda ni se
            // registra, porque podría repetir contenido enviado.
            let body = response
                .body_mut()
                .with_config()
                .limit(64 * 1024)
                .read_to_string()
                .ok();
            Err(client_error(endpoint, status, body.as_deref()))
        }
        _ => Err(AiError::Unavailable(format!("estado HTTP {status}"))),
    }
}

/// Error de un 4xx que no es 401, 403, 408 ni 429 (D146). `body` solo se interpreta aquí.
fn client_error(endpoint: Endpoint, status: u16, body: Option<&str>) -> AiError {
    if endpoint == Endpoint::Generate {
        match status {
            404 | 410 => {
                return AiError::ModelUnavailable {
                    cause: body.map_or(ModelUnavailableCause::RouteNotFound, not_found_cause),
                    status,
                }
            }
            402 => return AiError::NoCredit,
            400 if body.is_some_and(says_model_not_found) => {
                return AiError::ModelUnavailable {
                    cause: ModelUnavailableCause::ModelNotFound,
                    status,
                }
            }
            400 | 413 | 422 => {
                return AiError::RequestRejected {
                    status,
                    kind: body.and_then(error_type),
                }
            }
            _ => {}
        }
    }
    let kind = body.and_then(error_type);
    AiError::Unavailable(match kind {
        Some(kind) => format!("estado HTTP {status} ({kind})"),
        None => format!("estado HTTP {status}"),
    })
}

/// Textos del cuerpo de error que hablan de la causa: `error` si es una cadena (formato de
/// Ollama) o `error.code`, `error.type` y `error.message`, y `detail` o `message` de primer nivel. En minúsculas, solo para clasificar.
fn error_texts(body: &str) -> Option<Vec<String>> {
    let value: Value = serde_json::from_str(body).ok()?;
    let mut texts = Vec::new();
    match value.get("error") {
        Some(Value::String(text)) => texts.push(text.to_ascii_lowercase()),
        Some(error @ Value::Object(_)) => {
            for key in ["code", "type", "message"] {
                if let Some(text) = error.get(key).and_then(Value::as_str) {
                    texts.push(text.to_ascii_lowercase());
                }
            }
        }
        _ => {}
    }
    // Servicios web genéricos: `{"detail": "Not Found"}` o `{"message": "Not Found"}`.
    for key in ["detail", "message"] {
        if let Some(text) = value.get(key).and_then(Value::as_str) {
            texts.push(text.to_ascii_lowercase());
        }
    }
    Some(texts)
}

/// Un 400 es «modelo no disponible» si `error.code` o `error.type` es `model_not_found`, o si
/// `error.message` (o `error` como cadena) combina «model» con una mención de inexistencia o
/// «invalid model» (por ejemplo «Model does not exist»). Solo clasifica: el mensaje no se guarda.
fn says_model_not_found(body: &str) -> bool {
    let Ok(value) = serde_json::from_str::<Value>(body) else {
        return false;
    };
    let coded = ["/error/code", "/error/type"]
        .iter()
        .filter_map(|p| value.pointer(p).and_then(Value::as_str))
        .any(|text| text.eq_ignore_ascii_case("model_not_found"));
    let message = value
        .pointer("/error/message")
        .or_else(|| value.get("error"))
        .and_then(Value::as_str)
        .map(str::to_ascii_lowercase);
    coded
        || message.is_some_and(|m| {
            m.contains("invalid model")
                || (m.contains("model")
                    && ["does not exist", "doesn't exist", "not found", "no such"]
                        .iter()
                        .any(|needle| m.contains(needle)))
        })
}

fn mentions_missing(text: &str) -> bool {
    [
        "not found",
        "not_found",
        "does not exist",
        "doesn't exist",
        "no such",
        "unknown",
        "not available",
        "unavailable",
    ]
    .iter()
    .any(|needle| text.contains(needle))
}

/// Causa de un 404 o 410 a partir del cuerpo. Si no consta, `Other`: es la salida segura.
pub(crate) fn not_found_cause(body: &str) -> ModelUnavailableCause {
    let Some(texts) = error_texts(body) else {
        // Cuerpo no JSON (o vacío): la ruta no es la del servicio.
        return ModelUnavailableCause::RouteNotFound;
    };
    let joined = texts.join(" ");
    let model = joined.contains("model");
    if model && mentions_missing(&joined) {
        ModelUnavailableCause::ModelNotFound
    } else if !model && (joined.contains("not found") || joined.contains("not_found")) {
        ModelUnavailableCause::RouteNotFound
    } else {
        ModelUnavailableCause::Other
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

/// `Retry-After` en segundos (entero o decimal). La forma de fecha HTTP no se interpreta:
/// sin espera conocida, quien reintenta usa la suya.
pub(crate) fn retry_after_ms(value: &str) -> Option<u64> {
    let seconds: f64 = value.trim().parse().ok()?;
    (seconds.is_finite() && seconds >= 0.0).then(|| (seconds * 1000.0).ceil() as u64)
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
    fn not_found_cause_is_only_an_enumeration_of_what_the_body_supports() {
        use ModelUnavailableCause::*;
        let cause = |body: &str| not_found_cause(body);
        // Formato de Ollama: `error` es una cadena.
        assert_eq!(
            cause(r#"{"error":"model \"x:1b\" not found, try pulling it first"}"#),
            ModelNotFound
        );
        // Formato OpenAI y Anthropic.
        assert_eq!(
            cause(r#"{"error":{"code":"model_not_found","message":"x"}}"#),
            ModelNotFound
        );
        assert_eq!(
            cause(r#"{"type":"error","error":{"type":"not_found_error","message":"model: x"}}"#),
            ModelNotFound
        );
        // Sin JSON o «not found» genérico sin modelo: la dirección.
        assert_eq!(cause("404 page not found"), RouteNotFound);
        assert_eq!(cause(""), RouteNotFound);
        assert_eq!(cause(r#"{"error":"Not Found"}"#), RouteNotFound);
        assert_eq!(cause(r#"{"detail":"Not Found"}"#), RouteNotFound);
        // Lo demás: no consta.
        assert_eq!(cause(r#"{"error":"cuenta suspendida"}"#), Other);
        assert_eq!(cause("{}"), Other);
        assert_eq!(cause("[1]"), Other);
    }

    #[test]
    fn status_mapping_differs_between_generating_and_listing() {
        use Endpoint::*;
        let kind = |e, status, body| client_error(e, status, body).code();
        // Un 400 con «Model does not exist» en el mensaje también es modelo no disponible.
        assert_eq!(
            kind(
                Generate,
                400,
                Some(r#"{"error":{"message":"Model does not exist"}}"#)
            ),
            "model_unavailable"
        );
        assert_eq!(
            kind(
                Generate,
                400,
                Some(r#"{"error":{"message":"Invalid model: x"}}"#)
            ),
            "model_unavailable"
        );
        assert_eq!(
            kind(
                Generate,
                400,
                Some(r#"{"error":{"message":"unknown parameter: model"}}"#)
            ),
            "request_rejected"
        );
        for status in [404, 410] {
            assert_eq!(kind(Generate, status, Some("{}")), "model_unavailable");
            assert_eq!(kind(Plain, status, Some("{}")), "unavailable");
        }
        assert_eq!(kind(Generate, 402, None), "no_credit");
        assert_eq!(kind(Plain, 402, None), "unavailable");
        for status in [400, 413, 422] {
            assert_eq!(kind(Generate, status, None), "request_rejected");
            assert_eq!(kind(Plain, status, None), "unavailable");
        }
        assert_eq!(
            kind(
                Generate,
                400,
                Some(r#"{"error":{"code":"model_not_found"}}"#)
            ),
            "model_unavailable"
        );
        assert_eq!(kind(Generate, 418, None), "unavailable");
        assert_eq!(kind(Generate, 409, None), "unavailable");
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
