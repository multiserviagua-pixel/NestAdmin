//! Lista de modelos de un proveedor (D122). Solo `GET` con la clave de ese proveedor, sin
//! redirecciones y sin enviar contenido del usuario. Listar no prueba que la clave sea válida
//! (`/api/tags` puede ser público): eso lo comprueba la prueba sintética de la ventana.
//!
//! La lista se devuelve tal cual la da el proveedor, con el modelo recomendado primero si
//! está, sin duplicados y con un máximo de [`MAX_MODELS`] nombres.

use std::time::Duration;

use serde_json::Value;
use ureq::Agent;

use crate::credentials::Secret;
use crate::http;
use crate::ollama::{self, is_loopback_http};
use crate::openai_compatible::parse_base_url;
use crate::provider::AiError;
use crate::selection::ProviderChoice;
use crate::{anthropic, ollama_cloud};

/// Máximo de nombres que se devuelven.
pub const MAX_MODELS: usize = 500;
/// Páginas como mucho que se piden a Anthropic (1000 modelos por página).
const MAX_PAGES: usize = 5;
const PAGE_SIZE: usize = 1000;
/// Un nombre más largo o con caracteres de control no es un identificador de modelo.
const MAX_NAME_LEN: usize = 200;
const ANTHROPIC_MODELS: &str = "https://api.anthropic.com/v1/models";

fn key_required(key: Option<&Secret>) -> Result<&Secret, AiError> {
    key.ok_or(AiError::Unauthorized)
}

/// Modelos del proveedor elegido. Un proveedor externo exige su clave (`None` da
/// `unauthorized` sin hacer ninguna petición); la IA local no usa clave. Una URL compatible
/// con OpenAI no válida falla antes de cualquier petición.
pub fn list_models(
    choice: &ProviderChoice,
    key: Option<&Secret>,
    timeout: Duration,
) -> Result<Vec<String>, AiError> {
    match choice {
        ProviderChoice::Ollama { base_url, .. } => {
            if !is_loopback_http(base_url) {
                return Err(AiError::Unavailable(
                    "Ollama solo se admite en una dirección de bucle local".into(),
                ));
            }
            ollama_tags(&http::agent(timeout), base_url, None, true)
        }
        ProviderChoice::OllamaCloud { .. } => ollama_tags(
            &http::agent_no_redirects(timeout),
            ollama_cloud::ENDPOINT,
            Some(key_required(key)?),
            false,
        ),
        ProviderChoice::OpenAiCompatible { base_url, .. } => {
            let base = parse_base_url(base_url)?;
            openai_models(
                &http::agent_external(timeout),
                &base.url,
                key_required(key)?,
            )
        }
        ProviderChoice::Anthropic { .. } => anthropic_models(
            &http::agent_external(timeout),
            ANTHROPIC_MODELS,
            key_required(key)?,
        ),
        ProviderChoice::Groq { .. } => Err(AiError::Unavailable(
            "Groq se retira: no admite listar modelos".into(),
        )),
    }
}

/// Para pruebas contra un servidor simulado en 127.0.0.1: `endpoint` sustituye al destino del
/// proveedor (la base de `/api/tags` o `/models`, o la URL completa de Claude) y el agente no
/// filtra el DNS ni valida la URL. No se expone a la configuración de usuario.
#[doc(hidden)]
pub fn list_models_with_endpoint(
    choice: &ProviderChoice,
    endpoint: &str,
    key: Option<&Secret>,
    timeout: Duration,
) -> Result<Vec<String>, AiError> {
    let agent = http::agent_no_redirects(timeout);
    match choice {
        ProviderChoice::Ollama { .. } => ollama_tags(&agent, endpoint, None, true),
        ProviderChoice::OllamaCloud { .. } => {
            ollama_tags(&agent, endpoint, Some(key_required(key)?), false)
        }
        ProviderChoice::OpenAiCompatible { .. } => {
            openai_models(&agent, endpoint, key_required(key)?)
        }
        ProviderChoice::Anthropic { .. } => anthropic_models(&agent, endpoint, key_required(key)?),
        ProviderChoice::Groq { .. } => Err(AiError::Unavailable(
            "Groq se retira: no admite listar modelos".into(),
        )),
    }
}

/// Ollama (local o nube): `GET {base}/api/tags` o `{endpoint}/tags` para la nube, cuyo
/// `ENDPOINT` ya termina en `/api`. En el local se excluyen los modelos que el daemon reenvía
/// a la nube (`remote_model` / `remote_host`, D107).
fn ollama_tags(
    agent: &Agent,
    base: &str,
    key: Option<&Secret>,
    local: bool,
) -> Result<Vec<String>, AiError> {
    let base = base.trim_end_matches('/');
    let url = if local && !base.ends_with("/api") {
        format!("{base}/api/tags")
    } else {
        format!("{base}/tags")
    };
    let bearer = key.map(|k| format!("Bearer {}", k.expose()));
    let headers: Vec<(&str, &str)> = bearer
        .as_deref()
        .map(|b| vec![("Authorization", b)])
        .unwrap_or_default();
    let body = http::get_json_with_headers(agent, &url, &headers)?;
    let entries = body
        .get("models")
        .and_then(Value::as_array)
        .ok_or_else(|| AiError::InvalidResponse("la lista de modelos no tiene `models`".into()))?;
    let names = entries
        .iter()
        .filter(|entry| !(local && is_remote(entry)))
        .filter_map(|entry| {
            entry
                .get("name")
                .or_else(|| entry.get("model"))
                .and_then(Value::as_str)
        });
    let recommended = if local {
        ollama::DEFAULT_MODEL
    } else {
        ollama::DEFAULT_CLOUD_MODEL
    };
    Ok(finish(names, Some(recommended)))
}

fn is_remote(entry: &Value) -> bool {
    ["remote_model", "remote_host"].iter().any(|key| {
        entry.get(key).is_some_and(|v| match v {
            Value::Null => false,
            Value::String(s) => !s.trim().is_empty(),
            _ => true,
        })
    })
}

/// Compatible con OpenAI: `GET {base}/models` con `Authorization: Bearer`.
fn openai_models(agent: &Agent, base: &str, key: &Secret) -> Result<Vec<String>, AiError> {
    let bearer = format!("Bearer {}", key.expose());
    let body = http::get_json_with_headers(
        agent,
        &format!("{}/models", base.trim_end_matches('/')),
        &[("Authorization", &bearer)],
    )?;
    let entries = body
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| AiError::InvalidResponse("la lista de modelos no tiene `data`".into()))?;
    let names = entries
        .iter()
        .filter_map(|entry| entry.get("id").and_then(Value::as_str));
    Ok(finish(names, None))
}

/// Claude: `GET /v1/models` con `x-api-key` y `anthropic-version`, siguiendo la paginación
/// (`has_more` / `last_id` → `after_id`) hasta tener los nombres necesarios.
fn anthropic_models(agent: &Agent, endpoint: &str, key: &Secret) -> Result<Vec<String>, AiError> {
    let mut all: Vec<String> = Vec::new();
    let mut after: Option<String> = None;
    for _ in 0..MAX_PAGES {
        let mut url = format!("{endpoint}?limit={PAGE_SIZE}");
        if let Some(id) = &after {
            url.push_str("&after_id=");
            url.push_str(id);
        }
        let body = http::get_json_with_headers(
            agent,
            &url,
            &[
                ("x-api-key", key.expose()),
                ("anthropic-version", anthropic::API_VERSION),
            ],
        )?;
        let entries = body.get("data").and_then(Value::as_array).ok_or_else(|| {
            AiError::InvalidResponse("la lista de modelos no tiene `data`".into())
        })?;
        all.extend(
            entries
                .iter()
                .filter_map(|entry| entry.get("id").and_then(Value::as_str))
                .map(str::to_owned),
        );
        let more = body.get("has_more").and_then(Value::as_bool) == Some(true);
        if !more || all.len() >= MAX_MODELS {
            break;
        }
        // El identificador va en la URL: solo caracteres de identificador, nunca otros.
        let last = body
            .get("last_id")
            .and_then(Value::as_str)
            .filter(|id| {
                !id.is_empty()
                    && id.len() <= MAX_NAME_LEN
                    && id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
            })
            .ok_or_else(|| AiError::InvalidResponse("paginación de modelos inválida".into()))?;
        if after.as_deref() == Some(last) {
            return Err(AiError::InvalidResponse(
                "paginación de modelos inválida".into(),
            ));
        }
        after = Some(last.to_owned());
    }
    Ok(finish(all.iter().map(String::as_str), None))
}

/// Descarta nombres inválidos y repetidos, pone el recomendado primero si está y limita a
/// [`MAX_MODELS`]. El resto, en el orden recibido.
fn finish<'a>(names: impl Iterator<Item = &'a str>, recommended: Option<&str>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out: Vec<String> = names
        .map(str::trim)
        .filter(|n| !n.is_empty() && n.len() <= MAX_NAME_LEN && !n.chars().any(char::is_control))
        .filter(|n| seen.insert(*n))
        .map(str::to_owned)
        .collect();
    if let Some(recommended) = recommended {
        if let Some(position) = out.iter().position(|n| n == recommended) {
            let name = out.remove(position);
            out.insert(0, name);
        }
    }
    out.truncate(MAX_MODELS);
    out
}
