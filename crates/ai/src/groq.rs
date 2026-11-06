use std::time::Duration;

use crate::credentials::Secret;
use crate::openai_chat::ChatClient;
use crate::provider::{AiError, GenerateRequest, GenerateResult, Provider, ProviderKind};

pub const ENDPOINT: &str = "https://api.groq.com/openai/v1";

/// Adaptador externo (D07). Solo se construye tras la selección explícita del usuario y
/// con una credencial presente; nunca se usa como respaldo del proveedor local. Es un caso
/// de la API compatible con OpenAI con endpoint fijo (D95).
pub struct GroqProvider {
    client: ChatClient,
}

impl GroqProvider {
    pub fn new(model: &str, api_key: Secret, timeout: Duration) -> Self {
        Self::with_endpoint(ENDPOINT, model, api_key, timeout)
    }

    /// Para pruebas contra un servidor simulado.
    #[doc(hidden)]
    pub fn with_endpoint(endpoint: &str, model: &str, api_key: Secret, timeout: Duration) -> Self {
        Self {
            client: ChatClient::with_timeout(endpoint, model, api_key, timeout)
                .retrying_rate_limits(timeout),
        }
    }
}

impl std::fmt::Debug for GroqProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GroqProvider")
            .field("endpoint", &self.client.endpoint)
            .field("model", &self.client.model)
            .field("api_key", &self.client.api_key)
            .finish()
    }
}

impl Provider for GroqProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Groq
    }

    fn model(&self) -> &str {
        &self.client.model
    }

    fn generate(&self, request: &GenerateRequest) -> Result<GenerateResult, AiError> {
        self.client.generate(ProviderKind::Groq, request)
    }
}
