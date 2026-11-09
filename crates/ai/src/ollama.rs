use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use ureq::Agent;

use crate::http;
use crate::provider::{
    parse_output, AiError, GenerateRequest, GenerateResult, Provider, ProviderKind, Usage,
};

pub const DEFAULT_URL: &str = "http://127.0.0.1:11434";
pub const DEFAULT_MODEL: &str = "qwen3:8b";
/// Modelo prefijado en la interfaz para Ollama cloud: elección del usuario (2026-11-06,
/// D108), pendiente de medir en T12. Es el nombre para la API directa de ollama.com
/// (`https://ollama.com/api/tags`).
pub const DEFAULT_CLOUD_MODEL: &str = "gemma4:31b";
/// Digest del manifiesto de `qwen3:8b` evaluado (T10, D62): el SHA-256 que Ollama muestra en
/// `/api/tags` y que coincide con `registry.ollama.ai/v2/library/qwen3/manifests/8b`. Ollama
/// comprueba al descargar la huella de cada capa del manifiesto.
pub const DEFAULT_MODEL_DIGEST: &str =
    "500a1f067a9f782620b40bee6f7b0c89e17ae61f686b92c24933e4ca4b2b8b41";

/// Adaptador local (D06). Solo admite direcciones de bucle local, de modo que el
/// contenido nunca sale del equipo por este camino.
pub struct OllamaProvider {
    base_url: String,
    model: String,
    agent: Agent,
    /// El modelo ya se comprobó como local con `/api/show` (D107); se cachea por proveedor.
    verified_local: AtomicBool,
}

impl OllamaProvider {
    pub fn new(base_url: &str, model: &str, timeout: Duration) -> Result<Self, AiError> {
        if !is_loopback_http(base_url) {
            return Err(AiError::Unavailable(
                "Ollama solo se admite en una dirección de bucle local".into(),
            ));
        }
        // D107: el daemon local reenviaría a la nube los modelos `-cloud` / `:cloud`.
        if is_cloud_model(model) {
            return Err(AiError::Unavailable(
                "el modelo es de la nube de Ollama y el proveedor local lo reenviaría fuera del \
                 equipo; use el proveedor externo \"Ollama cloud\""
                    .into(),
            ));
        }
        Ok(Self {
            base_url: base_url.trim_end_matches('/').to_owned(),
            model: model.to_owned(),
            agent: http::agent(timeout),
            verified_local: AtomicBool::new(false),
        })
    }

    /// D107, segunda guarda: un alias local (`ollama cp x-cloud alias`, `FROM x-cloud`) no
    /// delata su destino por el nombre; el daemon lo declara en `/api/show` con
    /// `remote_host` / `remote_model`. Falla cerrado: sin respuesta válida no se genera.
    fn ensure_local_model(&self) -> Result<(), AiError> {
        if self.verified_local.load(Ordering::Acquire) {
            return Ok(());
        }
        let info = http::post_json_local(
            &self.agent,
            &format!("{}/api/show", self.base_url),
            &json!({ "model": self.model }),
        )?;
        if !info.is_object() {
            return Err(AiError::InvalidResponse(
                "respuesta de /api/show inválida".into(),
            ));
        }
        let remote = |key: &str| info.get(key).is_some_and(|v| !is_blank(v));
        if remote("remote_host") || remote("remote_model") {
            return Err(cloud_model_error());
        }
        self.verified_local.store(true, Ordering::Release);
        Ok(())
    }
}

fn is_blank(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::String(s) => s.trim().is_empty(),
        _ => false,
    }
}

fn cloud_model_error() -> AiError {
    AiError::Unavailable(
        "el modelo es de la nube de Ollama y el proveedor local lo reenviaría fuera del          equipo; use el proveedor externo \"Ollama cloud\""
            .into(),
    )
}

/// Modelos que el daemon local de Ollama reenvía a la nube: etiqueta `:cloud` o nombre que
/// termina en `-cloud` (por ejemplo `gpt-oss:120b-cloud`, `gemma4:cloud`).
pub fn is_cloud_model(model: &str) -> bool {
    let model = model.trim().to_ascii_lowercase();
    let tag = model.rsplit_once(':').map(|(_, tag)| tag);
    tag == Some("cloud") || model.ends_with("-cloud")
}

/// `http://` con host 127.0.0.1, localhost o [::1] y, opcionalmente, puerto.
pub fn is_loopback_http(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("http://") else {
        return false;
    };
    let authority = rest.split('/').next().unwrap_or("");
    if authority.contains('@') {
        return false;
    }
    let host = if let Some(v6) = authority.strip_prefix('[') {
        match v6.split_once(']') {
            Some((host, port)) if port.is_empty() || port.starts_with(':') => host,
            _ => return false,
        }
    } else {
        authority.split(':').next().unwrap_or("")
    };
    matches!(host, "127.0.0.1" | "localhost" | "::1")
}

impl Provider for OllamaProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Ollama
    }

    fn model(&self) -> &str {
        &self.model
    }

    fn generate(&self, request: &GenerateRequest) -> Result<GenerateResult, AiError> {
        self.ensure_local_model()?;
        let body = chat_body(&self.model, request);
        let started = Instant::now();
        let response =
            http::post_json_local(&self.agent, &format!("{}/api/chat", self.base_url), &body)?;
        let latency_ms = started.elapsed().as_millis() as u64;
        chat_result(
            ProviderKind::Ollama,
            &self.model,
            request,
            &response,
            latency_ms,
        )
    }
}

/// Cuerpo de `POST /chat` de la API nativa de Ollama; lo comparten el adaptador local y el
/// de la nube.
pub(crate) fn chat_body(model: &str, request: &GenerateRequest) -> Value {
    let mut body = json!({
        "model": model,
        "messages": [
            { "role": "system", "content": request.instructions },
            { "role": "user", "content": request.content },
        ],
        "stream": false,
        // Sin razonamiento visible: la salida debe ser solo el JSON pedido.
        "think": false,
        "options": { "temperature": 0 },
    });
    if let Some(schema) = &request.schema {
        body["format"] = schema.clone();
    }
    body
}

/// Interpreta la respuesta de `/chat` según el contrato.
pub(crate) fn chat_result(
    provider: ProviderKind,
    model: &str,
    request: &GenerateRequest,
    response: &Value,
    latency_ms: u64,
) -> Result<GenerateResult, AiError> {
    let text = response
        .pointer("/message/content")
        .and_then(Value::as_str)
        .ok_or_else(|| AiError::InvalidResponse("falta message.content".into()))?;
    Ok(GenerateResult {
        request_id: request.request_id.clone(),
        provider,
        model: response
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or(model)
            .to_owned(),
        output: parse_output(request, text)?,
        latency_ms,
        usage: Usage {
            input_tokens: response.get("prompt_eval_count").and_then(Value::as_u64),
            output_tokens: response.get("eval_count").and_then(Value::as_u64),
        },
    })
}
