//! Adaptador nativo de la API de Anthropic (D110). Proveedor externo con el mismo estatus que
//! los demás externos: solo por selección explícita y con la credencial presente; sin respaldo hacia otro
//! modelo ni proveedor (no se usa el parámetro `fallbacks`).
//!
//! Rust no tiene SDK oficial, así que se llama a `POST /v1/messages` directamente.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use ureq::Agent;

use crate::credentials::Secret;
use crate::http;
use crate::provider::{
    parse_output, AiError, GenerateRequest, GenerateResult, Provider, ProviderKind, Usage,
};

pub const ENDPOINT: &str = "https://api.anthropic.com/v1/messages";
/// Versión de la API (cabecera `anthropic-version`). Con esta versión la API rechaza
/// `temperature`, `top_p` y `top_k`, que por eso no se envían.
pub const API_VERSION: &str = "2026-10-21";
/// Modelo sugerido en la interfaz; el modelo es configurable.
pub const DEFAULT_MODEL: &str = "claude-opus-5-5";
const MAX_TOKENS: u32 = 16000;

pub struct AnthropicProvider {
    endpoint: String,
    model: String,
    api_key: Secret,
    agent: Agent,
}

impl AnthropicProvider {
    pub fn new(model: &str, api_key: Secret, timeout: Duration) -> Self {
        // Sin redirecciones y con el DNS filtrado: la clave solo viaja a api.anthropic.com.
        Self::build(ENDPOINT, model, api_key, http::agent_external(timeout))
    }

    /// Para pruebas contra un servidor simulado en 127.0.0.1 (agente sin el filtro de DNS).
    #[doc(hidden)]
    pub fn with_endpoint(endpoint: &str, model: &str, api_key: Secret, timeout: Duration) -> Self {
        Self::build(endpoint, model, api_key, http::agent_no_redirects(timeout))
    }

    fn build(endpoint: &str, model: &str, api_key: Secret, agent: Agent) -> Self {
        Self {
            endpoint: endpoint.to_owned(),
            model: model.to_owned(),
            api_key,
            agent,
        }
    }

    fn body(&self, request: &GenerateRequest) -> Value {
        // Sin `temperature`/`top_p`/`top_k`, sin prefill y sin configurar `thinking`. El
        // esquema se describe en las instrucciones y la salida se comprueba igual que con el
        // resto de proveedores.
        let system = match &request.schema {
            Some(schema) => format!(
                "{}\n\nResponde únicamente con un objeto JSON que cumpla este esquema:\n{schema}",
                request.instructions
            ),
            None => request.instructions.clone(),
        };
        json!({
            "model": self.model,
            "max_tokens": MAX_TOKENS,
            "system": system,
            "messages": [{ "role": "user", "content": request.content }],
            "output_config": { "effort": "low" },
        })
    }
}

impl std::fmt::Debug for AnthropicProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AnthropicProvider")
            .field("endpoint", &self.endpoint)
            .field("model", &self.model)
            .field("api_key", &self.api_key)
            .finish()
    }
}

impl Provider for AnthropicProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Anthropic
    }

    fn model(&self) -> &str {
        &self.model
    }

    fn generate(&self, request: &GenerateRequest) -> Result<GenerateResult, AiError> {
        let started = Instant::now();
        let response = http::post_json_with_headers(
            &self.agent,
            &self.endpoint,
            &[
                ("x-api-key", self.api_key.expose()),
                ("anthropic-version", API_VERSION),
                ("content-type", "application/json"),
            ],
            &self.body(request),
        )?;
        let latency_ms = started.elapsed().as_millis() as u64;
        let text = message_text(&response)?;
        Ok(GenerateResult {
            request_id: request.request_id.clone(),
            provider: ProviderKind::Anthropic,
            model: response
                .get("model")
                .and_then(Value::as_str)
                .unwrap_or(&self.model)
                .to_owned(),
            output: parse_output(request, &text)?,
            latency_ms,
            usage: Usage {
                input_tokens: response
                    .pointer("/usage/input_tokens")
                    .and_then(Value::as_u64),
                output_tokens: response
                    .pointer("/usage/output_tokens")
                    .and_then(Value::as_u64),
            },
        })
    }
}

/// Texto de una respuesta de Messages. Comprueba `stop_reason` antes de leer el contenido
/// y concatena solo los bloques `text`, ignorando `thinking` y cualquier otro.
fn message_text(response: &Value) -> Result<String, AiError> {
    match response.get("stop_reason").and_then(Value::as_str) {
        Some("refusal") => {
            return Err(AiError::InvalidResponse(
                "rechazo del modelo (stop_reason refusal)".into(),
            ))
        }
        Some("max_tokens") => {
            return Err(AiError::InvalidResponse(
                "respuesta truncada por max_tokens".into(),
            ))
        }
        Some("model_context_window_exceeded") => {
            return Err(AiError::InvalidResponse(
                "el contenido supera la ventana de contexto del modelo".into(),
            ))
        }
        _ => {}
    }
    let blocks = response
        .get("content")
        .and_then(Value::as_array)
        .ok_or_else(|| AiError::InvalidResponse("falta content".into()))?;
    let text: String = blocks
        .iter()
        .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|b| b.get("text").and_then(Value::as_str))
        .collect();
    if text.is_empty() {
        return Err(AiError::InvalidResponse(
            "la respuesta no contiene bloques de texto".into(),
        ));
    }
    Ok(text)
}
