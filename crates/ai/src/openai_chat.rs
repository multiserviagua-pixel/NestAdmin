//! Cliente común de la API de chat compatible con OpenAI (`POST {base}/chat/completions`).
//! Lo usan Groq (endpoint fijo) y el adaptador genérico (D94, URL validada).

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use ureq::Agent;

use crate::credentials::Secret;
use crate::http;
use crate::provider::{
    parse_output, AiError, GenerateRequest, GenerateResult, ProviderKind, Usage,
};

pub(crate) struct ChatClient {
    pub(crate) endpoint: String,
    pub(crate) model: String,
    pub(crate) api_key: Secret,
    agent: Agent,
}

impl ChatClient {
    pub(crate) fn new(endpoint: &str, model: &str, api_key: Secret, agent: Agent) -> Self {
        Self {
            endpoint: endpoint.trim_end_matches('/').to_owned(),
            model: model.to_owned(),
            api_key,
            agent,
        }
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
