//! Selección de proveedor (R03, flujo 4). Local por defecto; el externo solo con una
//! elección explícita y credencial guardada. Un fallo se devuelve tal cual: no existe ningún
//! camino de código que pase a otro proveedor.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::credentials::{CredentialStore, GROQ_API_KEY};
use crate::groq::GroqProvider;
use crate::ollama::{self, OllamaProvider};
use crate::provider::{AiError, GenerateRequest, GenerateResult, Provider, ProviderKind};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "provider", rename_all = "snake_case")]
pub enum ProviderChoice {
    Ollama { base_url: String, model: String },
    Groq { model: String },
}

impl Default for ProviderChoice {
    fn default() -> Self {
        Self::Ollama {
            base_url: ollama::DEFAULT_URL.to_owned(),
            model: ollama::DEFAULT_MODEL.to_owned(),
        }
    }
}

/// Construye únicamente el proveedor elegido. Sin credencial, Groq falla con
/// `unauthorized` y no se crea ningún cliente externo.
pub fn build_provider(
    choice: &ProviderChoice,
    credentials: &dyn CredentialStore,
    timeout: Duration,
) -> Result<Box<dyn Provider>, AiError> {
    match choice {
        ProviderChoice::Ollama { base_url, model } => {
            Ok(Box::new(OllamaProvider::new(base_url, model, timeout)?))
        }
        ProviderChoice::Groq { model } => {
            let key = credentials
                .get(GROQ_API_KEY)
                .map_err(|e| AiError::Unavailable(e.to_string()))?
                .ok_or(AiError::Unauthorized)?;
            Ok(Box::new(GroqProvider::new(model, key, timeout)))
        }
    }
}

/// Servicio con un único proveedor activo. Cambiarlo exige `select`, una acción explícita.
pub struct AiService {
    provider: Box<dyn Provider>,
}

impl AiService {
    pub fn new(provider: Box<dyn Provider>) -> Self {
        Self { provider }
    }

    pub fn select(&mut self, provider: Box<dyn Provider>) {
        self.provider = provider;
    }

    pub fn active(&self) -> (ProviderKind, &str) {
        (self.provider.kind(), self.provider.model())
    }

    pub fn generate(&self, request: &GenerateRequest) -> Result<GenerateResult, AiError> {
        self.provider.generate(request)
    }
}
