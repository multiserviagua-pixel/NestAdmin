use std::time::{Duration, Instant};

use serde_json::{json, Value};
use ureq::Agent;

use crate::credentials::Secret;
use crate::http;
use crate::provider::{
    parse_output, AiError, GenerateRequest, GenerateResult, Provider, ProviderKind, Usage,
};

pub const ENDPOINT: &str = "https://api.groq.com/openai/v1";

/// Reintentos ante un 429 dentro del plazo de la petición. La espera es la que pide el
/// proveedor (`Retry-After`), o `DEFAULT_WAIT` si no la da, con un máximo de `MAX_WAIT`.
const MAX_RATE_LIMIT_RETRIES: u32 = 10;
const DEFAULT_WAIT: Duration = Duration::from_secs(2);
const MAX_WAIT: Duration = Duration::from_secs(30);

/// Adaptador externo (D07). Solo se construye tras la selección explícita del usuario y
/// con una credencial presente; nunca se usa como respaldo del proveedor local.
pub struct GroqProvider {
    endpoint: String,
    model: String,
    api_key: Secret,
    agent: Agent,
    timeout: Duration,
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
            timeout,
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
        let url = format!("{}/chat/completions", self.endpoint);
        let mut retries = 0;
        let response = loop {
            match http::post_json(&self.agent, &url, Some(self.api_key.expose()), &body) {
                Err(AiError::RateLimited { retry_after_ms }) => {
                    let wait = retry_after_ms
                        .map(Duration::from_millis)
                        .unwrap_or(DEFAULT_WAIT)
                        .min(MAX_WAIT);
                    // Solo se espera si cabe en el plazo; si no, se informa sin esperar.
                    if retries >= MAX_RATE_LIMIT_RETRIES || started.elapsed() + wait >= self.timeout
                    {
                        return Err(AiError::RateLimited { retry_after_ms });
                    }
                    retries += 1;
                    std::thread::sleep(wait);
                }
                other => break other?,
            }
        };
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
