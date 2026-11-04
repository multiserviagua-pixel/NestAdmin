//! Configuración local: la selección de proveedor y el límite de mensajes leídos por cuenta
//! en cada consulta. La clave de Groq y los tokens de correo nunca se guardan aquí, sino en el
//! almacén de credenciales del sistema (R09, D13, D31).

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub provider: ProviderChoice,
    /// Mensajes leídos como mucho por cuenta en cada consulta (D42).
    #[serde(default = "default_mail_limit")]
    pub mail_limit: usize,
}

fn default_mail_limit() -> usize {
    nestadmin_app::accounts::DEFAULT_MAIL_LIMIT
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            provider: ProviderChoice::default(),
            mail_limit: default_mail_limit(),
        }
    }
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
/// (D46); el código sigue siendo `timeout` y no se cambia de proveedor.
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
pub fn credential_store() -> Arc<dyn CredentialStore> {
    match nestadmin_ai::credentials::WindowsCredentialStore::new() {
        Ok(store) => Arc::new(store),
        Err(error) => Arc::new(Failing(error)),
    }
}

#[cfg(not(windows))]
pub fn credential_store() -> Arc<dyn CredentialStore> {
    Arc::new(nestadmin_ai::MemoryCredentialStore::default())
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

#[cfg(test)]
mod tests {
    //! T09: la configuración guardada no contiene la clave, y una clave ausente al arrancar
    //! da un error explícito sin pasar a Ollama.

    use super::*;
    use nestadmin_ai::credentials::GROQ_API_KEY;
    use nestadmin_ai::{MemoryCredentialStore, Secret, Task};

    const KEY: &str = "gsk_PRUEBA_SINTETICA_T09_ajustes";

    fn request() -> GenerateRequest {
        GenerateRequest {
            request_id: "t09".into(),
            task: Task::Classification,
            instructions: "clasifica".into(),
            content: "texto sintético".into(),
            schema: None,
        }
    }

    #[test]
    fn saved_settings_never_contain_the_key() {
        let dir = std::env::temp_dir().join(format!("nestadmin-t09-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let store = MemoryCredentialStore::default();
        store.set(GROQ_API_KEY, &Secret::new(KEY)).unwrap();
        let settings = Settings {
            provider: ProviderChoice::Groq {
                model: "modelo-t09".into(),
            },
            ..Settings::default()
        };
        settings.save(&dir).unwrap();
        let text = fs::read_to_string(dir.join(FILE)).unwrap();
        assert!(!text.contains("PRUEBA_SINTETICA"), "{text}");
        assert_eq!(Settings::load(&dir).provider, settings.provider);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn missing_key_at_startup_fails_explicitly_and_keeps_groq() {
        let store = MemoryCredentialStore::default();
        let choice = ProviderChoice::Groq {
            model: "modelo-t09".into(),
        };
        let provider = provider_for(&choice, &store);
        assert_eq!(provider.kind(), ProviderKind::Groq, "sin respaldo a Ollama");
        assert_eq!(provider.generate(&request()), Err(AiError::Unauthorized));
        assert!(build(&choice, &store).is_err());
    }
}
