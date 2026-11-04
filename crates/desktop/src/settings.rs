//! Configuración local: solo la selección de proveedor. La clave de Groq nunca se guarda
//! aquí, sino en el almacén de credenciales del sistema (R09, D13).

use std::fs;
use std::io;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use nestadmin_ai::{
    build_provider, AiError, CredentialStore, GenerateRequest, GenerateResult, Provider,
    ProviderChoice, ProviderKind,
};
use nestadmin_setup::{explain_timeouts, Memory, System, WindowsSystem};
use serde::{Deserialize, Serialize};

const FILE: &str = "configuracion.json";
/// Plazo por solicitud; una extracción local tarda del orden de decenas de segundos.
const TIMEOUT: Duration = Duration::from_secs(180);

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Settings {
    pub provider: ProviderChoice,
}

impl Settings {
    /// Sin archivo, o con uno ilegible, se usa la selección inicial: Ollama local.
    pub fn load(data_dir: &Path) -> Self {
        fs::read_to_string(data_dir.join(FILE))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, data_dir: &Path) -> io::Result<()> {
        let text = serde_json::to_string_pretty(self).map_err(io::Error::other)?;
        let path = data_dir.join(FILE);
        let temporary = path.with_extension("json.tmp");
        fs::write(&temporary, text)?;
        fs::rename(temporary, path)
    }
}

/// Con Ollama en un equipo con poca memoria, un tiempo agotado indica esa causa probable
/// (D41); el código sigue siendo `timeout` y no se cambia de proveedor.
pub fn build(
    choice: &ProviderChoice,
    credentials: &dyn CredentialStore,
) -> Result<Arc<dyn Provider>, AiError> {
    let provider: Arc<dyn Provider> = build_provider(choice, credentials, TIMEOUT)?.into();
    Ok(explain_timeouts(
        provider,
        &Memory::assess(WindowsSystem.memory()),
    ))
}

/// Al arrancar se respeta la selección guardada aunque no se pueda preparar: las tareas
/// fallan con el motivo y el usuario decide si cambia de proveedor. Nunca se pasa a otro.
pub fn provider_for(
    choice: &ProviderChoice,
    credentials: &dyn CredentialStore,
) -> Arc<dyn Provider> {
    build(choice, credentials).unwrap_or_else(|error| {
        let (kind, model) = match choice {
            ProviderChoice::Ollama { model, .. } => (ProviderKind::Ollama, model.clone()),
            ProviderChoice::Groq { model } => (ProviderKind::Groq, model.clone()),
        };
        Arc::new(Unprepared { kind, model, error })
    })
}

/// Proveedor elegido que no se pudo preparar; cada solicitud devuelve el mismo error.
struct Unprepared {
    kind: ProviderKind,
    model: String,
    error: AiError,
}

impl Provider for Unprepared {
    fn kind(&self) -> ProviderKind {
        self.kind
    }
    fn model(&self) -> &str {
        &self.model
    }
    fn generate(&self, _request: &GenerateRequest) -> Result<GenerateResult, AiError> {
        Err(self.error.clone())
    }
}

#[cfg(windows)]
pub fn credential_store() -> Box<dyn CredentialStore> {
    match nestadmin_ai::credentials::WindowsCredentialStore::new() {
        Ok(store) => Box::new(store),
        Err(error) => Box::new(Failing(error)),
    }
}

#[cfg(not(windows))]
pub fn credential_store() -> Box<dyn CredentialStore> {
    Box::new(nestadmin_ai::MemoryCredentialStore::default())
}

/// Almacén del sistema no disponible: se informa en cada uso en lugar de guardar en claro.
#[cfg(windows)]
struct Failing(nestadmin_ai::CredentialError);

#[cfg(windows)]
impl CredentialStore for Failing {
    fn get(
        &self,
        _name: &str,
    ) -> Result<Option<nestadmin_ai::Secret>, nestadmin_ai::CredentialError> {
        Err(self.0.clone())
    }
    fn set(
        &self,
        _name: &str,
        _secret: &nestadmin_ai::Secret,
    ) -> Result<(), nestadmin_ai::CredentialError> {
        Err(self.0.clone())
    }
    fn delete(&self, _name: &str) -> Result<(), nestadmin_ai::CredentialError> {
        Err(self.0.clone())
    }
}
