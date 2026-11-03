use std::time::{Duration, Instant};

use serde_json::{json, Value};
use ureq::Agent;

use crate::credentials::Secret;
use crate::http;
use crate::provider::{
    parse_output, AiError, GenerateRequest, GenerateResult, Provider, ProviderKind, Usage,
};

pub const ENDPOINT: &str = "https://api.groq.com/openai/v1";

/// Adaptador externo (D07). Solo se construye tras la selección explícita del usuario y
/// con una credencial presente; nunca se usa como respaldo del proveedor local.
pub struct GroqProvider {
    endpoint: String,
    model: String,
    api_key: Secret,
    agent: Agent,
}

impl GroqProvider {
    pub fn new(model: &str, api_key: Secret, timeout: Duration) -> Self {
        Self::with_endpoint(ENDPOINT, model, api_key, timeout)
    }

    /// Para pruebas contra un servidor simulado.
    #[doc(hidden)]
    pub fn with_endpoint(endpoint: &str, model: &str, api_key: Secret, timeout: Duration) -> Self {
        Self {
            endpoint: endpoint.trim_end_matches('/').to_owned(),
            model: model.to_owned(),
            api_key,
            agent: http::agent(timeout),
        }
    }
}

impl std::fmt::Debug for GroqProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GroqProvider")
            .field("endpoint", &self.endpoint)
            .field("model", &self.model)
            .field("api_key", &self.api_key)
            .finish()
    }
}

impl Provider for GroqProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Groq
    }

    fn model(&self) -> &str {
        &self.model
    }

    fn generate(&self, request: &GenerateRequest) -> Result<GenerateResult, AiError> {
        // El modo JSON de la API compatible con OpenAI no valida un esquema: se describe en
        // las instrucciones y la salida se comprueba igual que con Ollama.
        let instructions = match &request.schema {
            Some(schema) => format!(
                "{}\n\nResponde únicamente con un objeto JSON que cumpla este esquema:\n{schema}",
                request.instructions
            ),
            None => request.instructions.clone(),
        };
        let mut body = json!({
            "model": self.model,
            "messages": [
                { "role": "system", "content": instructions },
                { "role": "user", "content": request.content },
            ],
            "temperature": 0,
        });
        if request.schema.is_some() {
            body["response_format"] = json!({ "type": "json_object" });
        }
        let started = Instant::now();
        let response = http::post_json(
            &self.agent,
            &format!("{}/chat/completions", self.endpoint),
            Some(self.api_key.expose()),
            &body,
        )?;
        let latency_ms = started.elapsed().as_millis() as u64;
        let text = response
            .pointer("/choices/0/message/content")
            .and_then(Value::as_str)
            .ok_or_else(|| AiError::InvalidResponse("falta choices[0].message.content".into()))?;
        Ok(GenerateResult {
            request_id: request.request_id.clone(),
            provider: ProviderKind::Groq,
            model: response
                .get("model")
                .and_then(Value::as_str)
                .unwrap_or(&self.model)
                .to_owned(),
            output: parse_output(request, text)?,
            latency_ms,
            usage: Usage {
                input_tokens: response
                    .pointer("/usage/prompt_tokens")
                    .and_then(Value::as_u64),
                output_tokens: response
                    .pointer("/usage/completion_tokens")
                    .and_then(Value::as_u64),
            },
        })
    }
}
