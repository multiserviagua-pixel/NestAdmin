//! Selección de proveedor (R03, flujo 4). Por defecto Ollama cloud (D100), que no envía nada
//! sin clave guardada y destino confirmado; la IA local y los demás externos, por elección
//! explícita. Un fallo se devuelve tal cual: este módulo no tiene ningún camino de código que
//! pase a otro proveedor (el único respaldo, del externo elegido al local, vive en la capa de
//! aplicación, D101).

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::anthropic::AnthropicProvider;
use crate::credentials::{CredentialStore, ANTHROPIC_API_KEY, OLLAMA_CLOUD_API_KEY};
use crate::ollama::{self, OllamaProvider};
use crate::ollama_cloud::OllamaCloudProvider;
use crate::openai_compatible::{parse_base_url, OpenAiCompatibleProvider};
use crate::provider::{AiError, GenerateRequest, GenerateResult, Provider, ProviderKind};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "provider", rename_all = "snake_case")]
pub enum ProviderChoice {
    Ollama {
        base_url: String,
        model: String,
    },
    OllamaCloud {
        model: String,
    },
    #[serde(rename = "openai_compatible")]
    OpenAiCompatible {
        base_url: String,
        model: String,
    },
    Anthropic {
        model: String,
    },
}

impl ProviderChoice {
    /// Solo Ollama en bucle local procesa en el equipo; el resto son proveedores externos
    /// (R03, D06): no hay nada local que comprobar, arrancar ni descargar.
    pub fn is_local(&self) -> bool {
        matches!(self, Self::Ollama { .. })
    }

    /// Identificador estable del proveedor (la etiqueta de `configuracion.json`).
    pub fn id(&self) -> &'static str {
        match self {
            Self::Ollama { .. } => "ollama",
            Self::OllamaCloud { .. } => "ollama_cloud",
            Self::OpenAiCompatible { .. } => "openai_compatible",
            Self::Anthropic { .. } => "anthropic",
        }
    }

    /// Nombre de la credencial en el almacén del sistema; `None` para la IA local o si la URL
    /// del proveedor compatible no es válida.
    pub fn credential_name(&self) -> Option<String> {
        match self {
            Self::Ollama { .. } => None,
            Self::OllamaCloud { .. } => Some(OLLAMA_CLOUD_API_KEY.to_owned()),
            Self::Anthropic { .. } => Some(ANTHROPIC_API_KEY.to_owned()),
            Self::OpenAiCompatible { base_url, .. } => {
                parse_base_url(base_url).ok().map(|b| b.credential_name())
            }
        }
    }

    /// Destino al que salen los datos con esta elección (D100): `None` para la IA local o si
    /// la URL del proveedor compatible no es válida (entonces nada se envía). Con un puerto
    /// distinto del 443 se añade, igual que en el nombre de la credencial.
    pub fn destination(&self) -> Option<String> {
        match self {
            Self::Ollama { .. } => None,
            Self::OllamaCloud { .. } => Some("ollama.com".to_owned()),
            Self::Anthropic { .. } => Some("api.anthropic.com".to_owned()),
            Self::OpenAiCompatible { base_url, .. } => parse_base_url(base_url).ok().map(|base| {
                if base.port == 443 {
                    base.host
                } else {
                    format!("{}:{}", base.host, base.port)
                }
            }),
        }
    }
}

impl Default for ProviderChoice {
    fn default() -> Self {
        Self::OllamaCloud {
            model: ollama::DEFAULT_CLOUD_MODEL.to_owned(),
        }
    }
}

/// Construye únicamente el proveedor elegido. Sin credencial, los externos fallan con
/// `unauthorized` y no se crea ningún cliente. La clave del proveedor compatible con OpenAI
/// es la del host de su URL (D109): la de otro host no se pide ni se envía.
pub fn build_provider(
    choice: &ProviderChoice,
    credentials: &dyn CredentialStore,
    timeout: Duration,
) -> Result<Box<dyn Provider>, AiError> {
    match choice {
        ProviderChoice::Ollama { base_url, model } => {
            Ok(Box::new(OllamaProvider::new(base_url, model, timeout)?))
        }
        ProviderChoice::OllamaCloud { model } => {
            let key = credentials
                .get(OLLAMA_CLOUD_API_KEY)
                .map_err(|e| AiError::Unavailable(e.to_string()))?
                .ok_or(AiError::Unauthorized)?;
            Ok(Box::new(OllamaCloudProvider::new(model, key, timeout)))
        }
        ProviderChoice::OpenAiCompatible { base_url, model } => {
            let base = parse_base_url(base_url)?;
            let key = credentials
                .get(&base.credential_name())
                .map_err(|e| AiError::Unavailable(e.to_string()))?
                .ok_or(AiError::Unauthorized)?;
            Ok(Box::new(OpenAiCompatibleProvider::new(
                &base.url, model, key, timeout,
            )?))
        }
        ProviderChoice::Anthropic { model } => {
            let key = credentials
                .get(ANTHROPIC_API_KEY)
                .map_err(|e| AiError::Unavailable(e.to_string()))?
                .ok_or(AiError::Unauthorized)?;
            Ok(Box::new(AnthropicProvider::new(model, key, timeout)))
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
