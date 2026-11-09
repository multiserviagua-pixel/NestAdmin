use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Task {
    Extraction,
    Classification,
    Draft,
    /// Afinado de sugerencias de reglas con datos agregados (D134).
    RefineSuggestions,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    Ollama,
    OllamaCloud,
    #[serde(rename = "openai_compatible")]
    OpenAiCompatible,
    Anthropic,
}

impl fmt::Display for ProviderKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Ollama => "ollama",
            Self::OllamaCloud => "ollama_cloud",
            Self::OpenAiCompatible => "openai_compatible",
            Self::Anthropic => "anthropic",
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
    /// Mismo código que `Timeout`, con la causa probable que conoce quien llama (por
    /// ejemplo, poca memoria para el modelo local, D46). No cambia de proveedor.
    #[error("timeout: el proveedor no respondió a tiempo, {0}")]
    TimeoutCause(String),
    #[error("invalid_response: {0}")]
    InvalidResponse(String),
    /// El proveedor externo limita las peticiones (HTTP 429). Lleva la espera que pidió, si
    /// la dio. Se reintenta con el mismo proveedor dentro del plazo; nunca cambia de proveedor.
    #[error("rate_limited: el proveedor limita las peticiones; vuelva a intentarlo más tarde")]
    RateLimited { retry_after_ms: Option<u64> },
    /// Al generar, el proveedor respondió 404 o 410 (o 400 con `model_not_found`): la
    /// configuración (modelo o dirección) no sirve, no el servicio. Es una espera de configuración
    /// (D146), no activa el respaldo. La causa es un valor enumerado: nunca se guarda el cuerpo.
    #[error("model_unavailable: estado HTTP {status}")]
    ModelUnavailable {
        cause: ModelUnavailableCause,
        status: u16,
    },
    /// El proveedor respondió 402: la cuenta no tiene crédito (D146). Espera global, sin respaldo.
    #[error("no_credit: la cuenta del proveedor no tiene crédito")]
    NoCredit,
    /// El proveedor rechazó esta petición (400, 413 o 422 al generar): falla solo esa tarea
    /// (D146), sin espera global ni respaldo. `kind` es el identificador corto `error.type`, si lo
    /// hay.
    #[error("request_rejected: estado HTTP {status}{}", kind_suffix(.kind))]
    RequestRejected { status: u16, kind: Option<String> },
}

/// Qué indica el cuerpo de un 404 o 410 al generar. Solo se conserva esta enumeración, nunca el
/// cuerpo, porque podría repetir contenido enviado (D146).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelUnavailableCause {
    /// El cuerpo dice que el modelo no existe o no está disponible.
    ModelNotFound,
    /// Cuerpo no JSON o «not found» genérico, sin mención de modelo: la dirección no es la del servicio.
    RouteNotFound,
    /// Cualquier otro cuerpo: no consta la causa exacta.
    Other,
}

fn kind_suffix(kind: &Option<String>) -> String {
    kind.as_ref().map(|k| format!(" ({k})")).unwrap_or_default()
}

impl AiError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Unavailable(_) => "unavailable",
            Self::Unauthorized => "unauthorized",
            Self::Timeout | Self::TimeoutCause(_) => "timeout",
            Self::InvalidResponse(_) => "invalid_response",
            Self::RateLimited { .. } => "rate_limited",
            Self::ModelUnavailable { .. } => "model_unavailable",
            Self::NoCredit => "no_credit",
            Self::RequestRejected { .. } => "request_rejected",
        }
    }
}

pub trait Provider: Send + Sync {
    fn kind(&self) -> ProviderKind;
    fn model(&self) -> &str;
    /// Destino real al que salen los datos (host y puerto si no es el 443): `None` con la IA local.
    /// El consentimiento del afinado automático (D134) se liga a él.
    fn destination(&self) -> Option<String> {
        None
    }
    /// Dirección completa configurada por el usuario (solo el compatible con OpenAI). Distingue
    /// dos rutas del mismo host: corregir la ruta cambia la identidad del proveedor (D146).
    fn configured_url(&self) -> Option<String> {
        None
    }
    fn generate(&self, request: &GenerateRequest) -> Result<GenerateResult, AiError>;
}

/// Interpreta la respuesta textual del modelo según la solicitud.
pub(crate) fn parse_output(request: &GenerateRequest, text: &str) -> Result<Value, AiError> {
    if request.schema.is_none() {
        return Ok(Value::String(text.to_owned()));
    }
    match serde_json::from_str::<Value>(strip_code_fence(text)) {
        Ok(value @ Value::Object(_)) => Ok(value),
        Ok(_) => Err(AiError::InvalidResponse(
            "la salida no es un objeto JSON".into(),
        )),
        Err(_) => Err(AiError::InvalidResponse(
            "la salida no es JSON válido".into(),
        )),
    }
}

/// Quita UN solo bloque de código ```` ``` ```` o ```` ```json ```` que envuelva todo el
/// texto (con espacios alrededor), sin tocar el contenido interior. Algunos modelos
/// envuelven así el JSON aunque se pida solo el objeto. Si el texto no es exactamente un
/// bloque, se devuelve recortado y sin más cambios.
pub(crate) fn strip_code_fence(text: &str) -> &str {
    let trimmed = text.trim();
    let Some(rest) = trimmed.strip_prefix("```") else {
        return trimmed;
    };
    let Some(inner) = rest.strip_suffix("```") else {
        return trimmed;
    };
    let inner = match inner.strip_prefix("json") {
        Some(after) if after.starts_with(char::is_whitespace) => after,
        _ => inner,
    };
    // Un cierre ``` dentro significa más de un bloque: no se toca.
    if inner.contains("```") {
        return trimmed;
    }
    inner.trim()
}
