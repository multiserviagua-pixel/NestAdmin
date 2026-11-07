//! Cliente común de la API de chat compatible con OpenAI (`POST {base}/chat/completions`).
//! Lo usan Groq (endpoint fijo) y el adaptador genérico (D109, URL validada).

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use ureq::Agent;

use crate::credentials::Secret;
use crate::http;
use crate::provider::{
    parse_output, AiError, GenerateRequest, GenerateResult, ProviderKind, Usage,
};

/// Reintentos ante un 429 dentro del plazo de la petición. La espera es la que pide el
/// proveedor (`Retry-After`), o `DEFAULT_WAIT` si no la da, con un máximo de `MAX_WAIT`.
const MAX_RATE_LIMIT_RETRIES: u32 = 10;
const DEFAULT_WAIT: Duration = Duration::from_secs(2);
const MAX_WAIT: Duration = Duration::from_secs(30);

pub(crate) struct ChatClient {
    pub(crate) endpoint: String,
    pub(crate) model: String,
    pub(crate) api_key: Secret,
    agent: Agent,
    /// Plazo total de la petición si se reintenta ante un 429 (`rate_limited`); `None` informa
    /// el 429 sin esperar.
    rate_limit_deadline: Option<Duration>,
}

impl ChatClient {
    pub(crate) fn new(endpoint: &str, model: &str, api_key: Secret, agent: Agent) -> Self {
        Self {
            endpoint: endpoint.trim_end_matches('/').to_owned(),
            model: model.to_owned(),
            api_key,
            agent,
            rate_limit_deadline: None,
        }
    }

    /// Reintenta con el mismo proveedor ante un 429 mientras quepa en `deadline` (D65).
    pub(crate) fn retrying_rate_limits(mut self, deadline: Duration) -> Self {
        self.rate_limit_deadline = Some(deadline);
        self
    }

    pub(crate) fn with_timeout(
        endpoint: &str,
        model: &str,
        api_key: Secret,
        timeout: Duration,
    ) -> Self {
        Self::new(endpoint, model, api_key, http::agent(timeout))
    }

    pub(crate) fn generate(
        &self,
        kind: ProviderKind,
        request: &GenerateRequest,
    ) -> Result<GenerateResult, AiError> {
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
                    let fits = self
                        .rate_limit_deadline
                        .is_some_and(|deadline| started.elapsed() + wait < deadline);
                    if retries >= MAX_RATE_LIMIT_RETRIES || !fits {
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
            provider: kind,
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
