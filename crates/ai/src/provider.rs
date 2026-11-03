use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Task {
    Extraction,
    Classification,
    Draft,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    Ollama,
    Groq,
}

impl fmt::Display for ProviderKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Ollama => "ollama",
            Self::Groq => "groq",
        })
    }
}

/// Solicitud del contrato de proveedor (contratos.md). `content` es el mínimo necesario;
/// con `schema`, la salida debe ser un objeto JSON que lo cumpla.
#[derive(Debug, Clone)]
pub struct GenerateRequest {
    pub request_id: String,
    pub task: Task,
    pub instructions: String,
    pub content: String,
    pub schema: Option<Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
}

/// Resultado con el proveedor y el modelo que realmente respondieron.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GenerateResult {
    pub request_id: String,
    pub provider: ProviderKind,
    pub model: String,
    /// Objeto JSON si la solicitud tenía esquema; si no, una cadena.
    pub output: Value,
    pub latency_ms: u64,
    pub usage: Usage,
}

/// Errores del contrato. Ninguno autoriza cambiar de proveedor, y ninguno contiene
/// credenciales ni el contenido enviado.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AiError {
    #[error("unavailable: {0}")]
    Unavailable(String),
    #[error("unauthorized: credencial ausente o rechazada")]
    Unauthorized,
    #[error("timeout: el proveedor no respondió a tiempo")]
    Timeout,
    #[error("invalid_response: {0}")]
    InvalidResponse(String),
}

impl AiError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Unavailable(_) => "unavailable",
            Self::Unauthorized => "unauthorized",
            Self::Timeout => "timeout",
            Self::InvalidResponse(_) => "invalid_response",
        }
    }
}

pub trait Provider: Send + Sync {
    fn kind(&self) -> ProviderKind;
    fn model(&self) -> &str;
    fn generate(&self, request: &GenerateRequest) -> Result<GenerateResult, AiError>;
}

/// Interpreta la respuesta textual del modelo según la solicitud.
pub(crate) fn parse_output(request: &GenerateRequest, text: &str) -> Result<Value, AiError> {
    if request.schema.is_none() {
        return Ok(Value::String(text.to_owned()));
    }
    match serde_json::from_str::<Value>(text.trim()) {
        Ok(value @ Value::Object(_)) => Ok(value),
        Ok(_) => Err(AiError::InvalidResponse(
            "la salida no es un objeto JSON".into(),
        )),
        Err(_) => Err(AiError::InvalidResponse(
            "la salida no es JSON válido".into(),
        )),
    }
}
