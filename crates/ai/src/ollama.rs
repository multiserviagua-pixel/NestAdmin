use std::time::{Duration, Instant};

use serde_json::{json, Value};
use ureq::Agent;

use crate::http;
use crate::provider::{
    parse_output, AiError, GenerateRequest, GenerateResult, Provider, ProviderKind, Usage,
};

pub const DEFAULT_URL: &str = "http://127.0.0.1:11434";
pub const DEFAULT_MODEL: &str = "qwen3:8b";
/// Digest del manifiesto de `qwen3:8b` evaluado (T10, D62): el SHA-256 que Ollama muestra en
/// `/api/tags` y que coincide con `registry.ollama.ai/v2/library/qwen3/manifests/8b`. Ollama
/// comprueba al descargar la huella de cada capa del manifiesto.
pub const DEFAULT_MODEL_DIGEST: &str =
    "500a1f067a9f782620b40bee6f7b0c89e17ae61f686b92c24933e4ca4b2b8b41";

/// Adaptador local (D06). Solo admite direcciones de bucle local, de modo que el
/// contenido nunca sale del equipo por este camino.
pub struct OllamaProvider {
    base_url: String,
    model: String,
    agent: Agent,
}

impl OllamaProvider {
    pub fn new(base_url: &str, model: &str, timeout: Duration) -> Result<Self, AiError> {
        if !is_loopback_http(base_url) {
            return Err(AiError::Unavailable(
                "Ollama solo se admite en una dirección de bucle local".into(),
            ));
        }
        Ok(Self {
            base_url: base_url.trim_end_matches('/').to_owned(),
            model: model.to_owned(),
            agent: http::agent(timeout),
        })
    }
}

/// `http://` con host 127.0.0.1, localhost o [::1] y, opcionalmente, puerto.
pub fn is_loopback_http(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("http://") else {
        return false;
    };
    let authority = rest.split('/').next().unwrap_or("");
    if authority.contains('@') {
        return false;
    }
    let host = if let Some(v6) = authority.strip_prefix('[') {
        match v6.split_once(']') {
            Some((host, port)) if port.is_empty() || port.starts_with(':') => host,
            _ => return false,
        }
    } else {
        authority.split(':').next().unwrap_or("")
    };
    matches!(host, "127.0.0.1" | "localhost" | "::1")
}

impl Provider for OllamaProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Ollama
    }

    fn model(&self) -> &str {
        &self.model
    }

    fn generate(&self, request: &GenerateRequest) -> Result<GenerateResult, AiError> {
        let mut body = json!({
            "model": self.model,
            "messages": [
                { "role": "system", "content": request.instructions },
                { "role": "user", "content": request.content },
            ],
            "stream": false,
            // Sin razonamiento visible: la salida debe ser solo el JSON pedido.
            "think": false,
            "options": { "temperature": 0 },
        });
        if let Some(schema) = &request.schema {
            body["format"] = schema.clone();
        }
        let started = Instant::now();
        let response = http::post_json(
            &self.agent,
            &format!("{}/api/chat", self.base_url),
            None,
            &body,
        )?;
        let latency_ms = started.elapsed().as_millis() as u64;
        let text = response
            .pointer("/message/content")
            .and_then(Value::as_str)
            .ok_or_else(|| AiError::InvalidResponse("falta message.content".into()))?;
        Ok(GenerateResult {
            request_id: request.request_id.clone(),
            provider: ProviderKind::Ollama,
            model: response
                .get("model")
                .and_then(Value::as_str)
                .unwrap_or(&self.model)
                .to_owned(),
            output: parse_output(request, text)?,
            latency_ms,
            usage: Usage {
                input_tokens: response.get("prompt_eval_count").and_then(Value::as_u64),
                output_tokens: response.get("eval_count").and_then(Value::as_u64),
            },
        })
    }
}
