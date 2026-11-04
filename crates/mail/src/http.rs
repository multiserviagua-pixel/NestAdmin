//! Llamadas autenticadas a la API del proveedor con la traducción de errores de D35.

use std::time::Duration;

use nestadmin_simulators::codes;
use serde_json::Value;
use ureq::Agent;
use url::Url;

use crate::oauth::TokenSource;
use crate::{checked_url, error, provider_code, MailResult};

pub(crate) enum Method {
    Get,
    Post,
}

pub(crate) struct Api {
    base: Url,
    agent: Agent,
    tokens: TokenSource,
    /// Cabecera `Prefer` fija (Graph); `None` para proveedores que no la usan.
    prefer: Option<String>,
}

impl Api {
    pub(crate) fn new(
        base_url: &str,
        tokens: TokenSource,
        prefer: Option<&str>,
    ) -> MailResult<Self> {
        let mut base = checked_url(base_url)?;
        // Sin barra final, `join` sustituiría el último segmento de la ruta base.
        if !base.path().ends_with('/') {
            base.set_path(&format!("{}/", base.path()));
        }
        Ok(Self {
            base,
            agent: Agent::config_builder()
                .timeout_global(Some(Duration::from_secs(60)))
                .timeout_connect(Some(Duration::from_secs(10)))
                .http_status_as_error(false)
                .proxy(None)
                .build()
                .into(),
            tokens,
            prefer: prefer.map(str::to_owned),
        })
    }

    pub(crate) fn tokens(&self) -> &TokenSource {
        &self.tokens
    }

    /// URL bajo la base con cada segmento codificado (los IDs del proveedor pueden llevar
    /// `=`, `/` o `+`) y los parámetros de consulta indicados.
    pub(crate) fn url(&self, segments: &[&str], query: &[(&str, &str)]) -> Url {
        let mut url = self.base.clone();
        {
            let mut path = url
                .path_segments_mut()
                .expect("una URL HTTP admite segmentos");
            path.pop_if_empty();
            for segment in segments {
                path.push(segment);
            }
        }
        if !query.is_empty() {
            url.query_pairs_mut().extend_pairs(query);
        }
        url
    }

    /// Un cursor del proveedor (por ejemplo `@odata.nextLink`) solo se sigue si apunta al
    /// mismo origen y bajo la misma ruta base: el token nunca viaja a otro servidor (D32).
    pub(crate) fn same_origin(&self, link: &str) -> MailResult<Url> {
        let url = checked_url(link)?;
        if url.origin() == self.base.origin() && url.path().starts_with(self.base.path()) {
            Ok(url)
        } else {
            Err(error(
                codes::INVALID_INPUT,
                "el cursor apunta fuera del servicio de la cuenta",
            ))
        }
    }

    /// Envía la petición con el token vigente; ante un 401 renueva el token una vez y repite.
    pub(crate) fn call(
        &self,
        method: Method,
        url: &Url,
        body: Option<&Value>,
    ) -> MailResult<Value> {
        match self.attempt(&method, url, body, false) {
            Err(e) if e.code == "retry_unauthorized" => self.attempt(&method, url, body, true),
            other => other,
        }
    }

    fn attempt(
        &self,
        method: &Method,
        url: &Url,
        body: Option<&Value>,
        refreshed: bool,
    ) -> MailResult<Value> {
        let token = self.tokens.access_token(refreshed)?;
        let bearer = format!("Bearer {}", token.expose());
        let result = match method {
            Method::Get => {
                let mut request = self
                    .agent
                    .get(url.as_str())
                    .header("Authorization", &bearer);
                if let Some(prefer) = &self.prefer {
                    request = request.header("Prefer", prefer);
                }
                request.call()
            }
            Method::Post => {
                let mut request = self
                    .agent
                    .post(url.as_str())
                    .header("Authorization", &bearer);
                if let Some(prefer) = &self.prefer {
                    request = request.header("Prefer", prefer);
                }
                match body {
                    Some(body) => request.send_json(body),
                    None => request.header("Content-Length", "0").send_empty(),
                }
            }
        };
        let mut response = result.map_err(|_| {
            error(
                codes::UNAVAILABLE,
                "no se pudo conectar con el proveedor de correo",
            )
        })?;
        let status = response.status().as_u16();
        let text = response.body_mut().read_to_string().unwrap_or_default();
        let json = if text.trim().is_empty() {
            Value::Null
        } else {
            serde_json::from_str(&text).unwrap_or(Value::Null)
        };
        match status {
            200..=299 => Ok(json),
            401 if !refreshed => Err(error("retry_unauthorized", "")),
            401 | 403 => Err(error(
                codes::UNAUTHORIZED,
                format!(
                    "el proveedor rechazó el acceso ({}); vuelva a iniciar sesión",
                    reason(&json, token.expose())
                ),
            )),
            404 => Err(error(
                codes::NOT_FOUND,
                "el proveedor no encuentra el elemento",
            )),
            400 | 422 => Err(error(
                codes::INVALID_INPUT,
                format!(
                    "el proveedor rechazó la petición ({})",
                    reason(&json, token.expose())
                ),
            )),
            409 => Err(error(
                codes::CONFLICT,
                "el proveedor informa de un conflicto",
            )),
            429 => Err(error(
                codes::UNAVAILABLE,
                "el proveedor limita las peticiones; reintente más tarde",
            )),
            _ => Err(error(
                codes::UNAVAILABLE,
                format!("el proveedor devolvió el estado HTTP {status}"),
            )),
        }
    }
}

/// Código de error del proveedor, sin el texto completo, que podría citar contenido o el
/// propio token.
fn reason(json: &Value, token: &str) -> String {
    let code = json
        .pointer("/error/code")
        .or_else(|| json.pointer("/error/status"))
        .and_then(Value::as_str);
    provider_code(code, &[token])
}
