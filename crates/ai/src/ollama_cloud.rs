use std::time::{Duration, Instant};

use ureq::Agent;

use crate::credentials::Secret;
use crate::http;
use crate::ollama::{chat_body, chat_result};
use crate::provider::{AiError, GenerateRequest, GenerateResult, Provider, ProviderKind};

pub const ENDPOINT: &str = "https://ollama.com/api";

/// Adaptador externo (D106). Llama directamente a la API de Ollama en la nube, sin pasar por
/// el daemon local. Solo se construye tras la selección explícita del usuario y con una
/// credencial presente; nunca se usa como respaldo de otro proveedor.
pub struct OllamaCloudProvider {
    endpoint: String,
    model: String,
    api_key: Secret,
    agent: Agent,
}

impl OllamaCloudProvider {
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
            agent: http::agent_no_redirects(timeout),
        }
    }
}

impl std::fmt::Debug for OllamaCloudProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OllamaCloudProvider")
            .field("endpoint", &self.endpoint)
            .field("model", &self.model)
            .field("api_key", &self.api_key)
            .finish()
    }
}

impl Provider for OllamaCloudProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::OllamaCloud
    }

    fn model(&self) -> &str {
        &self.model
    }

    fn destination(&self) -> Option<String> {
        Some("ollama.com".to_owned())
    }

    fn generate(&self, request: &GenerateRequest) -> Result<GenerateResult, AiError> {
        let body = chat_body(&self.model, request);
        let started = Instant::now();
        let response = http::post_json(
            &self.agent,
            &format!("{}/chat", self.endpoint),
            Some(self.api_key.expose()),
            &body,
        )?;
        let latency_ms = started.elapsed().as_millis() as u64;
        chat_result(
            ProviderKind::OllamaCloud,
            &self.model,
            request,
            &response,
            latency_ms,
        )
    }
}
