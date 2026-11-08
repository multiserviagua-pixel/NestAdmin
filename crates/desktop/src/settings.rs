//! Configuración local: la selección de proveedor, la confirmación del destino de los datos
//! de cada proveedor externo (D100), el respaldo local (D101) y el límite de mensajes leídos
//! por cuenta en cada consulta. Las claves de los proveedores externos y los tokens de correo
//! nunca se guardan aquí, sino en el almacén de credenciales del sistema (R09, D13, D31).

use std::fs;
use std::io;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use nestadmin_ai::{
    build_provider, AiError, CredentialStore, GenerateRequest, GenerateResult, Provider,
    ProviderChoice, ProviderKind,
};
use nestadmin_app::AiWait;
use nestadmin_setup::{explain_timeouts, Memory, System, WindowsSystem};
use serde::{Deserialize, Serialize};
use serde_json::Value;

const FILE: &str = "configuracion.json";
/// Plazo por solicitud; una extracción local tarda del orden de decenas de segundos.
const TIMEOUT: Duration = Duration::from_secs(180);

/// Confirmación del usuario de que sus datos salen hacia un destino (D100). No lleva secretos.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Consent {
    pub provider: String,
    pub destination: String,
}

impl Consent {
    /// `None` para la IA local o si no hay destino válido.
    pub fn for_choice(choice: &ProviderChoice) -> Option<Self> {
        Some(Self {
            provider: choice.id().to_owned(),
            destination: choice.destination()?,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    /// `None`: el usuario debe elegir proveedor (por ejemplo, tras una configuración con
    /// Groq); mientras tanto no se envía nada.
    pub provider: Option<ProviderChoice>,
    /// Mensajes leídos como mucho por cuenta en cada consulta (D42).
    #[serde(default = "default_mail_limit")]
    pub mail_limit: usize,
    /// Destinos externos confirmados, uno por proveedor y destino.
    #[serde(default)]
    pub external_consent: Vec<Consent>,
    /// Respaldo automático del proveedor externo elegido a la IA local (D101).
    #[serde(default = "default_true")]
    pub local_fallback: bool,
    /// La oferta única de pasar al proveedor externo ya se mostró (D100).
    #[serde(default)]
    pub external_offer_shown: bool,
}

fn default_mail_limit() -> usize {
    nestadmin_app::accounts::DEFAULT_MAIL_LIMIT
}

fn default_true() -> bool {
    true
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            provider: Some(ProviderChoice::default()),
            mail_limit: default_mail_limit(),
            external_consent: Vec::new(),
            local_fallback: true,
            external_offer_shown: false,
        }
    }
}

impl Settings {
    /// Sin archivo, o con uno ilegible, se usa la selección inicial (Ollama cloud, D100) sin
    /// confirmación: nada se envía hasta que el usuario guarde la clave y confirme el destino.
    /// Se lee como JSON y no como la estructura para decidir sobre la etiqueta en bruto:
    /// - `provider` nulo, de Groq (retirado) o ilegible deja `None`, conservando el resto, y
    ///   nunca se sustituye por otro proveedor externo;
    /// - un archivo anterior con proveedor externo y sin `external_consent` lo da por
    ///   confirmado para ese destino (las versiones anteriores lo confirmaban en un diálogo).
    pub fn load(data_dir: &Path) -> Self {
        let value = fs::read_to_string(data_dir.join(FILE))
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .filter(Value::is_object);
        let Some(value) = value else {
            return Self::default();
        };
        let saved = value.get("provider");
        let provider = match saved {
            None => Some(ProviderChoice::default()),
            Some(Value::Null) => None,
            // Groq se retiró (D100). Se reconoce la etiqueta en bruto, aunque el tipo ya no
            // exista, para dejar claro que una configuración guardada con Groq pide elegir
            // proveedor y no se sustituye por otro externo (enviaría el contenido a un tercero
            // no elegido). La clave de Groq que quede en el almacén no se lee ni se borra.
            Some(raw) if raw.get("provider").and_then(Value::as_str) == Some("groq") => None,
            Some(raw) => serde_json::from_value(raw.clone()).ok(),
        };
        let mail_limit = value
            .get("mail_limit")
            .and_then(Value::as_u64)
            .map_or_else(default_mail_limit, |n| n as usize);
        let local_fallback = value
            .get("local_fallback")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let external_consent = match value.get("external_consent") {
            Some(raw) => serde_json::from_value(raw.clone()).unwrap_or_default(),
            None if saved.is_some() => provider
                .as_ref()
                .and_then(Consent::for_choice)
                .into_iter()
                .collect(),
            None => Vec::new(),
        };
        let external_offer_shown = value
            .get("external_offer_shown")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        Self {
            provider,
            mail_limit,
            external_consent,
            local_fallback,
            external_offer_shown,
        }
    }

    pub fn has_consent(&self, choice: &ProviderChoice) -> bool {
        Consent::for_choice(choice).is_some_and(|c| self.external_consent.contains(&c))
    }

    /// Registra la confirmación del destino de `choice` (sin efecto con la IA local).
    pub fn record_consent(&mut self, choice: &ProviderChoice) {
        if let Some(consent) = Consent::for_choice(choice) {
            if !self.external_consent.contains(&consent) {
                self.external_consent.push(consent);
            }
        }
    }

    /// Si la elección actual puede enviar datos: `Ok` con la elección o el motivo de espera
    /// (D101). El destino confirmado se comprueba antes de leer ninguna clave. Un error del
    /// almacén de credenciales no es «falta la clave»: se devuelve `Ok` y el error explícito
    /// sale al construir el proveedor.
    pub fn readiness(&self, credentials: &dyn CredentialStore) -> Result<&ProviderChoice, AiWait> {
        let choice = self.provider.as_ref().ok_or(AiWait::NeedsChoice)?;
        if choice.is_local() {
            return Ok(choice);
        }
        if !self.has_consent(choice) {
            return Err(AiWait::NeedsConsent);
        }
        let name = choice.credential_name().ok_or(AiWait::NeedsConsent)?;
        match credentials.get(&name) {
            Ok(None) => Err(AiWait::NeedsKey),
            Ok(Some(_)) | Err(_) => Ok(choice),
        }
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
    build(choice, credentials).unwrap_or_else(|error| unprepared(choice, error))
}

/// Proveedor que no puede enviar: cada solicitud devuelve `error`. Conserva el tipo y el
/// modelo de la elección para mostrarlos.
pub fn unprepared(choice: &ProviderChoice, error: AiError) -> Arc<dyn Provider> {
    let (kind, model) = match choice {
        ProviderChoice::Ollama { model, .. } => (ProviderKind::Ollama, model.clone()),
        ProviderChoice::OllamaCloud { model } => (ProviderKind::OllamaCloud, model.clone()),
        ProviderChoice::OpenAiCompatible { model, .. } => {
            (ProviderKind::OpenAiCompatible, model.clone())
        }
        ProviderChoice::Anthropic { model } => (ProviderKind::Anthropic, model.clone()),
    };
    Arc::new(Unprepared { kind, model, error })
}

/// Proveedor de espera: sin elección (o sin permiso para enviar) devuelve `unauthorized` y no
/// construye ningún cliente.
pub fn waiting_provider(choice: Option<&ProviderChoice>) -> Arc<dyn Provider> {
    match choice {
        Some(choice) => unprepared(choice, AiError::Unauthorized),
        None => unprepared(&ProviderChoice::default(), AiError::Unauthorized),
    }
}

/// Proveedor que corresponde a la configuración y, si no puede enviar todavía, el motivo de
/// la espera (D101). Sin permiso o sin elección no se lee ni se construye nada para enviar.
pub fn resolve(
    settings: &Settings,
    credentials: &dyn CredentialStore,
) -> (Arc<dyn Provider>, Option<AiWait>) {
    match settings.readiness(credentials) {
        Ok(choice) => (provider_for(choice, credentials), None),
        Err(wait) => (waiting_provider(settings.provider.as_ref()), Some(wait)),
    }
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
    use nestadmin_ai::credentials::OLLAMA_CLOUD_API_KEY;
    use nestadmin_ai::{MemoryCredentialStore, Secret, Task};

    const KEY: &str = "gsk_PRUEBA_SINTETICA_T09_ajustes";
    /// Clave que pudo dejar Groq (retirado, D100) en el almacén: nunca se lee ni se usa.
    const LEFTOVER_GROQ_KEY: &str = "groq_api_key";

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
        store.set(OLLAMA_CLOUD_API_KEY, &Secret::new(KEY)).unwrap();
        let settings = Settings {
            provider: Some(ProviderChoice::OllamaCloud {
                model: "modelo-t09".into(),
            }),
            ..Settings::default()
        };
        let mut settings = settings;
        settings.record_consent(settings.provider.clone().as_ref().unwrap());
        settings.save(&dir).unwrap();
        let text = fs::read_to_string(dir.join(FILE)).unwrap();
        assert!(!text.contains("PRUEBA_SINTETICA"), "{text}");
        assert!(text.contains("ollama.com"), "el destino sí consta: {text}");
        assert_eq!(Settings::load(&dir).provider, settings.provider);
        assert_eq!(
            Settings::load(&dir).external_consent,
            settings.external_consent
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn missing_key_at_startup_fails_explicitly_and_keeps_the_selection() {
        let store = MemoryCredentialStore::default();
        let choice = ProviderChoice::Anthropic {
            model: "modelo-t09".into(),
        };
        let provider = provider_for(&choice, &store);
        assert_eq!(
            provider.kind(),
            ProviderKind::Anthropic,
            "sin respaldo a Ollama"
        );
        assert_eq!(provider.generate(&request()), Err(AiError::Unauthorized));
        assert!(build(&choice, &store).is_err());
    }

    #[test]
    fn missing_ollama_cloud_key_at_startup_fails_explicitly_and_keeps_the_selection() {
        let store = MemoryCredentialStore::default();
        // Con una clave de Groq olvidada en el almacén, la de Ollama cloud sigue ausente y
        // aquella no se usa.
        store.set(LEFTOVER_GROQ_KEY, &Secret::new(KEY)).unwrap();
        let choice = ProviderChoice::OllamaCloud {
            model: "modelo-t09".into(),
        };
        let provider = provider_for(&choice, &store);
        assert_eq!(
            provider.kind(),
            ProviderKind::OllamaCloud,
            "sin respaldo a Ollama local ni a otro externo"
        );
        assert_eq!(provider.model(), "modelo-t09");
        assert_eq!(provider.generate(&request()), Err(AiError::Unauthorized));
        assert!(build(&choice, &store).is_err());
    }

    #[test]
    fn missing_key_for_new_external_providers_fails_explicitly_and_keeps_the_selection() {
        let store = MemoryCredentialStore::default();
        // Con todas las demás claves presentes (y la de otro host), la elegida sigue ausente.
        store.set(LEFTOVER_GROQ_KEY, &Secret::new(KEY)).unwrap();
        store
            .set(
                nestadmin_ai::credentials::OLLAMA_CLOUD_API_KEY,
                &Secret::new(KEY),
            )
            .unwrap();
        store
            .set(
                "openai_compatible_api_key:otro-host.example.org",
                &Secret::new(KEY),
            )
            .unwrap();
        let compat = ProviderChoice::OpenAiCompatible {
            base_url: "https://api.example.com/v1".into(),
            model: "modelo-t09".into(),
        };
        let provider = provider_for(&compat, &store);
        assert_eq!(provider.kind(), ProviderKind::OpenAiCompatible);
        assert_eq!(provider.model(), "modelo-t09");
        assert_eq!(provider.generate(&request()), Err(AiError::Unauthorized));
        assert!(build(&compat, &store).is_err());

        store
            .set(
                nestadmin_ai::credentials::ANTHROPIC_API_KEY,
                &Secret::new(KEY),
            )
            .unwrap();
        store
            .delete(nestadmin_ai::credentials::ANTHROPIC_API_KEY)
            .unwrap();
        let claude = ProviderChoice::Anthropic {
            model: "claude-opus-5-5".into(),
        };
        let provider = provider_for(&claude, &store);
        assert_eq!(provider.kind(), ProviderKind::Anthropic, "sin respaldo");
        assert_eq!(provider.generate(&request()), Err(AiError::Unauthorized));
        assert!(build(&claude, &store).is_err());

        // Una URL no válida también falla explícito, sin leer claves ni caer a otro proveedor.
        let local = ProviderChoice::OpenAiCompatible {
            base_url: "https://localhost/v1".into(),
            model: "modelo-t09".into(),
        };
        let provider = provider_for(&local, &store);
        assert_eq!(provider.kind(), ProviderKind::OpenAiCompatible);
        assert!(matches!(
            provider.generate(&request()),
            Err(AiError::Unavailable(_))
        ));
    }

    #[test]
    fn saved_settings_with_new_external_providers_never_contain_the_key() {
        let dir = std::env::temp_dir().join(format!("nestadmin-t09-ext-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        for provider in [
            ProviderChoice::OpenAiCompatible {
                base_url: "https://api.example.com/v1".into(),
                model: "modelo-t09".into(),
            },
            ProviderChoice::Anthropic {
                model: "claude-opus-5-5".into(),
            },
        ] {
            let settings = Settings {
                provider: Some(provider),
                ..Settings::default()
            };
            settings.save(&dir).unwrap();
            let text = fs::read_to_string(dir.join(FILE)).unwrap();
            assert!(
                !text.contains(KEY) && !text.contains("PRUEBA_SINTETICA"),
                "{text}"
            );
            assert_eq!(Settings::load(&dir).provider, settings.provider);
        }
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn saved_settings_with_ollama_cloud_never_contain_the_key() {
        let dir = std::env::temp_dir().join(format!("nestadmin-t09-cloud-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let store = MemoryCredentialStore::default();
        store
            .set(
                nestadmin_ai::credentials::OLLAMA_CLOUD_API_KEY,
                &Secret::new(KEY),
            )
            .unwrap();
        let settings = Settings {
            provider: Some(ProviderChoice::OllamaCloud {
                model: "modelo-t09".into(),
            }),
            ..Settings::default()
        };
        settings.save(&dir).unwrap();
        let text = fs::read_to_string(dir.join(FILE)).unwrap();
        assert!(!text.contains("PRUEBA_SINTETICA"), "{text}");
        assert_eq!(Settings::load(&dir).provider, settings.provider);
        fs::remove_dir_all(&dir).unwrap();
    }

    // ---- D100, D101: elección, confirmación del destino y espera de la IA ----

    fn dir_with(name: &str, json: Option<&str>) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("nestadmin-t03g-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        if let Some(json) = json {
            fs::write(dir.join(FILE), json).unwrap();
        }
        dir
    }

    #[test]
    fn t03g_no_file_selects_ollama_cloud_without_consent_and_waits() {
        let dir = dir_with("sin-archivo", None);
        let settings = Settings::load(&dir);
        assert_eq!(settings.provider, Some(ProviderChoice::default()));
        assert!(matches!(
            settings.provider,
            Some(ProviderChoice::OllamaCloud { .. })
        ));
        assert!(settings.external_consent.is_empty());
        assert!(settings.local_fallback);
        // Sin confirmación no se mira ni la clave: aunque esté guardada, se espera el permiso.
        let store = MemoryCredentialStore::default();
        store
            .set(
                nestadmin_ai::credentials::OLLAMA_CLOUD_API_KEY,
                &Secret::new(KEY),
            )
            .unwrap();
        assert_eq!(
            settings.readiness(&store).unwrap_err(),
            AiWait::NeedsConsent
        );
        let (provider, wait) = resolve(&settings, &store);
        assert_eq!(wait, Some(AiWait::NeedsConsent));
        assert_eq!(provider.generate(&request()), Err(AiError::Unauthorized));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn t03g_readiness_needs_key_after_consent_and_then_is_ready() {
        let mut settings = Settings::default();
        let choice = settings.provider.clone().unwrap();
        settings.record_consent(&choice);
        let store = MemoryCredentialStore::default();
        assert_eq!(settings.readiness(&store).unwrap_err(), AiWait::NeedsKey);
        store
            .set(
                nestadmin_ai::credentials::OLLAMA_CLOUD_API_KEY,
                &Secret::new(KEY),
            )
            .unwrap();
        assert_eq!(settings.readiness(&store).unwrap(), &choice);
        let (_, wait) = resolve(&settings, &store);
        assert_eq!(wait, None);
    }

    #[test]
    fn t03g_consent_for_another_destination_does_not_count() {
        let mut settings = Settings::default();
        settings.record_consent(&ProviderChoice::Anthropic { model: "m".into() });
        settings.record_consent(&ProviderChoice::OpenAiCompatible {
            base_url: "https://api.uno.example/v1".into(),
            model: "m".into(),
        });
        let store = MemoryCredentialStore::default();
        // Ollama cloud sigue sin confirmar.
        assert_eq!(
            settings.readiness(&store).unwrap_err(),
            AiWait::NeedsConsent
        );
        // La URL compatible confirmada no vale para otro host.
        settings.provider = Some(ProviderChoice::OpenAiCompatible {
            base_url: "https://api.otro.example/v1".into(),
            model: "m".into(),
        });
        assert_eq!(
            settings.readiness(&store).unwrap_err(),
            AiWait::NeedsConsent
        );
        settings.provider = Some(ProviderChoice::OpenAiCompatible {
            base_url: "https://api.uno.example/v1".into(),
            model: "otro-modelo".into(),
        });
        assert_eq!(settings.readiness(&store).unwrap_err(), AiWait::NeedsKey);
        // Una URL no válida no tiene destino: nunca hay permiso.
        settings.provider = Some(ProviderChoice::OpenAiCompatible {
            base_url: "https://localhost/v1".into(),
            model: "m".into(),
        });
        assert_eq!(
            settings.readiness(&store).unwrap_err(),
            AiWait::NeedsConsent
        );
    }

    #[test]
    fn t03g_local_choice_needs_neither_consent_nor_key() {
        let settings = Settings {
            provider: Some(ProviderChoice::Ollama {
                base_url: "http://127.0.0.1:11434".into(),
                model: "m".into(),
            }),
            ..Settings::default()
        };
        assert!(settings
            .readiness(&MemoryCredentialStore::default())
            .is_ok());
    }

    #[test]
    fn t03g_saved_groq_needs_a_choice_keeps_the_rest_and_picks_no_other_external() {
        let dir = dir_with(
            "groq",
            Some(
                r#"{"provider":{"provider":"groq","model":"m"},"mail_limit":7,"local_fallback":false}"#,
            ),
        );
        let settings = Settings::load(&dir);
        assert_eq!(settings.provider, None);
        assert_eq!(settings.mail_limit, 7);
        assert!(!settings.local_fallback);
        assert!(settings.external_consent.is_empty(), "nada que migrar");
        let store = MemoryCredentialStore::default();
        store
            .set(
                nestadmin_ai::credentials::OLLAMA_CLOUD_API_KEY,
                &Secret::new(KEY),
            )
            .unwrap();
        assert_eq!(settings.readiness(&store).unwrap_err(), AiWait::NeedsChoice);
        let (provider, wait) = resolve(&settings, &store);
        assert_eq!(wait, Some(AiWait::NeedsChoice));
        assert_eq!(provider.generate(&request()), Err(AiError::Unauthorized));
        // Un valor nulo o ilegible también deja la elección pendiente.
        for raw in [
            r#"{"provider":null,"mail_limit":9}"#,
            r#"{"provider":{"provider":"inventado"}}"#,
        ] {
            fs::write(dir.join(FILE), raw).unwrap();
            assert_eq!(Settings::load(&dir).provider, None, "{raw}");
        }
        // Guardar de nuevo conserva «sin proveedor».
        let mut saved = Settings::load(&dir);
        saved.mail_limit = 11;
        saved.save(&dir).unwrap();
        let reloaded = Settings::load(&dir);
        assert_eq!(reloaded.provider, None);
        assert_eq!(reloaded.mail_limit, 11);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn t03g_old_file_with_an_external_provider_and_no_consent_key_migrates_the_consent() {
        let dir = dir_with(
            "migracion",
            Some(r#"{"provider":{"provider":"anthropic","model":"m"},"mail_limit":30}"#),
        );
        let settings = Settings::load(&dir);
        assert_eq!(
            settings.external_consent,
            vec![Consent {
                provider: "anthropic".into(),
                destination: "api.anthropic.com".into()
            }]
        );
        assert!(settings.local_fallback);
        let store = MemoryCredentialStore::default();
        assert_eq!(settings.readiness(&store).unwrap_err(), AiWait::NeedsKey);
        // Con la clave `external_consent` presente (aunque vacía) no se migra nada.
        fs::write(
            dir.join(FILE),
            r#"{"provider":{"provider":"anthropic","model":"m"},"external_consent":[]}"#,
        )
        .unwrap();
        let settings = Settings::load(&dir);
        assert!(settings.external_consent.is_empty());
        assert_eq!(
            settings.readiness(&store).unwrap_err(),
            AiWait::NeedsConsent
        );
        // Un archivo antiguo con IA local no genera ninguna confirmación.
        fs::write(
            dir.join(FILE),
            r#"{"provider":{"provider":"ollama","base_url":"http://127.0.0.1:11434","model":"m"}}"#,
        )
        .unwrap();
        let settings = Settings::load(&dir);
        assert!(settings.external_consent.is_empty());
        assert!(settings.readiness(&store).is_ok());
        // Un archivo ilegible vuelve a la selección inicial, sin confirmación.
        fs::write(dir.join(FILE), "{no es json").unwrap();
        let settings = Settings::load(&dir);
        assert_eq!(settings.provider, Some(ProviderChoice::default()));
        assert!(settings.external_consent.is_empty());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn t03g_a_key_store_error_is_not_a_missing_key() {
        struct Broken;
        impl CredentialStore for Broken {
            fn get(&self, _: &str) -> Result<Option<Secret>, nestadmin_ai::CredentialError> {
                Err(nestadmin_ai::CredentialError("almacén roto".into()))
            }
            fn set(&self, _: &str, _: &Secret) -> Result<(), nestadmin_ai::CredentialError> {
                unreachable!()
            }
            fn delete(&self, _: &str) -> Result<(), nestadmin_ai::CredentialError> {
                unreachable!()
            }
        }
        let mut settings = Settings::default();
        let choice = settings.provider.clone().unwrap();
        settings.record_consent(&choice);
        let (provider, wait) = resolve(&settings, &Broken);
        assert_eq!(wait, None, "no se confunde con una clave ausente");
        assert!(matches!(
            provider.generate(&request()),
            Err(AiError::Unavailable(_))
        ));
    }

    #[test]
    fn t03g_saved_consent_never_contains_a_key() {
        let dir = dir_with("sin-clave", None);
        let mut settings = Settings::default();
        let choice = ProviderChoice::Anthropic { model: "m".into() };
        settings.record_consent(&choice);
        settings.record_consent(&choice);
        assert_eq!(settings.external_consent.len(), 1, "sin duplicados");
        settings.save(&dir).unwrap();
        let text = fs::read_to_string(dir.join(FILE)).unwrap();
        assert!(
            !text.contains(KEY) && !text.contains("PRUEBA_SINTETICA"),
            "{text}"
        );
        assert!(text.contains("api.anthropic.com"), "{text}");
        fs::remove_dir_all(&dir).unwrap();
    }
}
