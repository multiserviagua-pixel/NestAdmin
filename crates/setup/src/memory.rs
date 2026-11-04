//! Memoria del equipo frente a la IA local (D41). Qwen3 8B ocupa unos 5,2 GB y, con
//! Windows y la aplicación, necesita 16 GB para responder dentro del plazo. Con menos se
//! avisa sin bloquear; con 8 GB o menos, el aviso dice que probablemente no responderá a
//! tiempo. Nunca se elige Groq por el usuario: solo se le indica que puede elegirlo.

use std::sync::Arc;

use nestadmin_ai::{AiError, GenerateRequest, GenerateResult, Provider, ProviderKind};
use serde::Serialize;

const GIB: u64 = 1 << 30;
/// Memoria recomendada para la IA local (equipo objetivo de PLAN.md).
pub const RECOMMENDED_RAM_GB: u64 = 16;
/// Por debajo de este valor nominal (incluido), la IA local probablemente no responde a
/// tiempo.
pub const LOW_RAM_GB: u64 = 8;
/// Windows no cuenta la memoria que reservan el firmware y los gráficos integrados: un
/// equipo de 16 GB informa de unos 15,3–15,9 GiB y uno de 8 GB, de unos 7,6–7,9 GiB.
const RESERVED: u64 = GIB;
/// Memoria libre por debajo de la cual se sugiere cerrar programas antes de usar el modelo.
const FREE_FOR_MODEL: u64 = 6 * GIB;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryLevel {
    /// 16 GB o más.
    Recommended,
    /// Menos de 16 GB y más de 8 GB: funciona más despacio.
    BelowRecommended,
    /// 8 GB o menos: la IA local probablemente no responderá a tiempo.
    Insufficient,
    /// No se pudo leer: no se avisa ni se supone nada.
    Unknown,
}

/// Memoria física del equipo y el aviso que corresponde. No bloquea nada.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Memory {
    pub total_bytes: Option<u64>,
    pub available_bytes: Option<u64>,
    pub level: MemoryLevel,
    /// Aviso legible para el asistente; `None` si no hay nada que advertir.
    pub warning: Option<String>,
}

/// GB con una cifra decimal y coma, como los muestra Windows.
fn gb(bytes: u64) -> String {
    format!("{:.1}", bytes as f64 / GIB as f64).replace('.', ",")
}

impl Memory {
    /// Clasifica la memoria leída por `System::memory`.
    pub fn assess(reading: Option<(u64, u64)>) -> Self {
        let Some((total, available)) = reading else {
            return Self {
                total_bytes: None,
                available_bytes: None,
                level: MemoryLevel::Unknown,
                warning: None,
            };
        };
        let level = if total + RESERVED >= RECOMMENDED_RAM_GB * GIB {
            MemoryLevel::Recommended
        } else if total < (LOW_RAM_GB * GIB) + RESERVED {
            MemoryLevel::Insufficient
        } else {
            MemoryLevel::BelowRecommended
        };
        let mut warning = match level {
            MemoryLevel::Insufficient => Some(format!(
                "Este equipo tiene {} GB de memoria. Con {LOW_RAM_GB} GB o menos, la IA local \
                 (Qwen3 8B) probablemente no responderá a tiempo. Puede intentarlo igualmente o \
                 elegir Groq en Configuración; NestAdmin no lo cambiará por su cuenta.",
                gb(total)
            )),
            MemoryLevel::BelowRecommended => Some(format!(
                "Este equipo tiene {} GB de memoria; se recomiendan {RECOMMENDED_RAM_GB} GB para \
                 la IA local. Puede seguir, pero las respuestas serán más lentas y alguna podría \
                 no llegar a tiempo; si ocurre, puede elegir Groq en Configuración.",
                gb(total)
            )),
            _ => None,
        };
        if available < FREE_FOR_MODEL && level != MemoryLevel::Unknown {
            let free = format!(
                "Ahora hay {} GB libres: cierre otros programas antes de usar la IA local.",
                gb(available)
            );
            warning = Some(match warning {
                Some(text) => format!("{text} {free}"),
                None => free,
            });
        }
        Self {
            total_bytes: Some(total),
            available_bytes: Some(available),
            level,
            warning,
        }
    }

    /// Causa probable que añadir a un tiempo agotado de la IA local, si la memoria es baja.
    pub fn timeout_cause(&self) -> Option<String> {
        let total = self.total_bytes?;
        match self.level {
            MemoryLevel::Insufficient | MemoryLevel::BelowRecommended => Some(format!(
                "probablemente por falta de memoria: el equipo tiene {} GB y la IA local necesita \
                 {RECOMMENDED_RAM_GB} GB; puede elegir Groq en Configuración",
                gb(total)
            )),
            _ => None,
        }
    }

    /// Añade la causa a un tiempo agotado; los demás errores no cambian.
    pub fn explain(&self, error: AiError) -> AiError {
        match (error, self.timeout_cause()) {
            (AiError::Timeout, Some(cause)) => AiError::TimeoutCause(cause),
            (error, _) => error,
        }
    }
}

/// Proveedor local que explica los tiempos agotados por poca memoria. Con memoria
/// suficiente, desconocida o con Groq, devuelve el proveedor tal cual.
pub fn explain_timeouts(provider: Arc<dyn Provider>, memory: &Memory) -> Arc<dyn Provider> {
    if provider.kind() != ProviderKind::Ollama || memory.timeout_cause().is_none() {
        return provider;
    }
    Arc::new(MemoryAware {
        inner: provider,
        memory: memory.clone(),
    })
}

struct MemoryAware {
    inner: Arc<dyn Provider>,
    memory: Memory,
}

impl Provider for MemoryAware {
    fn kind(&self) -> ProviderKind {
        self.inner.kind()
    }
    fn model(&self) -> &str {
        self.inner.model()
    }
    fn generate(&self, request: &GenerateRequest) -> Result<GenerateResult, AiError> {
        self.inner
            .generate(request)
            .map_err(|error| self.memory.explain(error))
    }
}
