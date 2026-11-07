//! Credenciales de proveedores externos (D08, R09). En Windows se guardan en el
//! administrador de credenciales; nunca en la base local, registros ni evaluaciones.

use std::collections::HashMap;
use std::fmt;
use std::sync::Mutex;

pub const SERVICE: &str = "NestAdmin";
pub const GROQ_API_KEY: &str = "groq_api_key";
pub const OLLAMA_CLOUD_API_KEY: &str = "ollama_cloud_api_key";
pub const ANTHROPIC_API_KEY: &str = "anthropic_api_key";
/// Prefijo de la clave del proveedor compatible con OpenAI (D109): el nombre completo es
/// `openai_compatible_api_key:<host en minúsculas>`, de modo que una clave nunca se envía a
/// otro host.
pub const OPENAI_COMPATIBLE_API_KEY_PREFIX: &str = "openai_compatible_api_key:";

/// Secreto que no se imprime: `Debug` y `Display` muestran solo asteriscos.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("***")
    }
}

impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("***")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("almacén de credenciales no disponible: {0}")]
pub struct CredentialError(pub String);

pub trait CredentialStore: Send + Sync {
    /// `Ok(None)` si no hay credencial guardada con ese nombre.
    fn get(&self, name: &str) -> Result<Option<Secret>, CredentialError>;
    fn set(&self, name: &str, secret: &Secret) -> Result<(), CredentialError>;
    fn delete(&self, name: &str) -> Result<(), CredentialError>;
}

/// Almacén en memoria para pruebas.
#[derive(Default)]
pub struct MemoryCredentialStore {
    values: Mutex<HashMap<String, Secret>>,
}

impl CredentialStore for MemoryCredentialStore {
    fn get(&self, name: &str) -> Result<Option<Secret>, CredentialError> {
        Ok(self.values.lock().expect("mutex").get(name).cloned())
    }

    fn set(&self, name: &str, secret: &Secret) -> Result<(), CredentialError> {
        self.values
            .lock()
            .expect("mutex")
            .insert(name.to_owned(), secret.clone());
        Ok(())
    }

    fn delete(&self, name: &str) -> Result<(), CredentialError> {
        self.values.lock().expect("mutex").remove(name);
        Ok(())
    }
}

/// Una entrada del Administrador de credenciales admite 2560 bytes y `keyring` la guarda en
/// UTF-16: unos 1280 caracteres. Los tokens OAuth de Microsoft no caben (D121).
pub const CHUNK_CHARS: usize = 1000;
/// Valor de la entrada principal cuando el secreto se reparte en trozos `<nombre>#1..N`.
const CHUNK_MARKER: &str = "nestadmin-trozos:";

/// Reparte los secretos largos en varias entradas del almacén interior (D121). Los cortos se
/// guardan tal cual, de modo que las credenciales ya guardadas se siguen leyendo igual.
pub struct ChunkedStore<S> {
    inner: S,
    chunk: usize,
}

impl<S: CredentialStore> ChunkedStore<S> {
    pub fn new(inner: S, chunk: usize) -> Self {
        assert!(chunk > 0, "tamaño de trozo nulo");
        Self { inner, chunk }
    }

    fn part(name: &str, index: usize) -> String {
        format!("{name}#{index}")
    }

    /// Número de trozos de la credencial guardada; 0 si no está troceada o no existe.
    fn parts_of(&self, name: &str) -> Result<usize, CredentialError> {
        match self.inner.get(name)? {
            Some(value) => match value.expose().strip_prefix(CHUNK_MARKER) {
                Some(count) => count.parse().map_err(|_| invalid()),
                None => Ok(0),
            },
            None => Ok(0),
        }
    }

    fn delete_parts(
        &self,
        name: &str,
        range: std::ops::RangeInclusive<usize>,
    ) -> Result<(), CredentialError> {
        for index in range {
            self.inner.delete(&Self::part(name, index))?;
        }
        Ok(())
    }
}

fn invalid() -> CredentialError {
    CredentialError("credencial con codificación inválida".into())
}

impl<S: CredentialStore> CredentialStore for ChunkedStore<S> {
    fn get(&self, name: &str) -> Result<Option<Secret>, CredentialError> {
        let Some(value) = self.inner.get(name)? else {
            return Ok(None);
        };
        let Some(count) = value.expose().strip_prefix(CHUNK_MARKER) else {
            return Ok(Some(value));
        };
        let count: usize = count.parse().map_err(|_| invalid())?;
        let mut whole = String::new();
        for index in 1..=count {
            // Si falta un trozo la credencial está incompleta: no se devuelve a medias.
            let part = self
                .inner
                .get(&Self::part(name, index))?
                .ok_or_else(invalid)?;
            whole.push_str(part.expose());
        }
        Ok(Some(Secret(whole)))
    }

    fn set(&self, name: &str, secret: &Secret) -> Result<(), CredentialError> {
        let previous = self.parts_of(name)?;
        let chars: Vec<char> = secret.expose().chars().collect();
        if chars.len() <= self.chunk && !secret.expose().starts_with(CHUNK_MARKER) {
            self.inner.set(name, secret)?;
            return self.delete_parts(name, 1..=previous);
        }
        let parts: Vec<String> = chars
            .chunks(self.chunk)
            .map(|c| c.iter().collect())
            .collect();
        for (index, part) in parts.iter().enumerate() {
            self.inner
                .set(&Self::part(name, index + 1), &Secret::new(part.as_str()))?;
        }
        // La entrada principal se escribe la última.
        self.inner
            .set(name, &Secret(format!("{CHUNK_MARKER}{}", parts.len())))?;
        self.delete_parts(name, parts.len() + 1..=previous)
    }

    fn delete(&self, name: &str) -> Result<(), CredentialError> {
        let previous = self.parts_of(name).unwrap_or(0);
        self.inner.delete(name)?;
        self.delete_parts(name, 1..=previous)
    }
}

/// Administrador de credenciales de Windows mediante `keyring-core`, con los secretos largos
/// repartidos en trozos (D121).
#[cfg(windows)]
pub struct WindowsCredentialStore(ChunkedStore<WindowsEntries>);

#[cfg(windows)]
impl WindowsCredentialStore {
    pub fn new() -> Result<Self, CredentialError> {
        let store = windows_native_keyring_store::Store::new().map_err(platform)?;
        keyring_core::set_default_store(store);
        Ok(Self(ChunkedStore::new(WindowsEntries, CHUNK_CHARS)))
    }
}

#[cfg(windows)]
impl CredentialStore for WindowsCredentialStore {
    fn get(&self, name: &str) -> Result<Option<Secret>, CredentialError> {
        self.0.get(name)
    }

    fn set(&self, name: &str, secret: &Secret) -> Result<(), CredentialError> {
        self.0.set(name, secret)
    }

    fn delete(&self, name: &str) -> Result<(), CredentialError> {
        self.0.delete(name)
    }
}

/// Entradas sueltas del Administrador de credenciales, sin trocear.
#[cfg(windows)]
pub struct WindowsEntries;

#[cfg(windows)]
impl WindowsEntries {
    fn entry(name: &str) -> Result<keyring_core::Entry, CredentialError> {
        keyring_core::Entry::new(SERVICE, name).map_err(platform)
    }
}

/// Solo el tipo de error: los detalles de plataforma no salen del almacén.
#[cfg(windows)]
fn platform(error: keyring_core::Error) -> CredentialError {
    let kind = match error {
        keyring_core::Error::NoStorageAccess(_) => "sin acceso al almacén",
        keyring_core::Error::PlatformFailure(_) => "fallo de la plataforma",
        keyring_core::Error::BadEncoding(_) => "credencial con codificación inválida",
        keyring_core::Error::TooLong(..) => "credencial demasiado larga para el almacén",
        _ => "error del almacén",
    };
    CredentialError(kind.into())
}

#[cfg(windows)]
impl CredentialStore for WindowsEntries {
    fn get(&self, name: &str) -> Result<Option<Secret>, CredentialError> {
        match Self::entry(name)?.get_password() {
            Ok(value) => Ok(Some(Secret(value))),
            Err(keyring_core::Error::NoEntry) => Ok(None),
            Err(e) => Err(platform(e)),
        }
    }

    fn set(&self, name: &str, secret: &Secret) -> Result<(), CredentialError> {
        Self::entry(name)?
            .set_password(secret.expose())
            .map_err(platform)
    }

    fn delete(&self, name: &str) -> Result<(), CredentialError> {
        match Self::entry(name)?.delete_credential() {
            Ok(()) | Err(keyring_core::Error::NoEntry) => Ok(()),
            Err(e) => Err(platform(e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Almacén con un límite de longitud como el de Windows, para comprobar el troceado.
    #[derive(Default)]
    struct Limited(MemoryCredentialStore);

    impl CredentialStore for Limited {
        fn get(&self, name: &str) -> Result<Option<Secret>, CredentialError> {
            self.0.get(name)
        }
        fn set(&self, name: &str, secret: &Secret) -> Result<(), CredentialError> {
            if secret.expose().chars().count() > 20 {
                return Err(CredentialError("demasiado larga".into()));
            }
            self.0.set(name, secret)
        }
        fn delete(&self, name: &str) -> Result<(), CredentialError> {
            self.0.delete(name)
        }
    }

    fn store() -> ChunkedStore<Limited> {
        ChunkedStore::new(Limited::default(), 10)
    }

    fn raw(store: &ChunkedStore<Limited>, name: &str) -> Option<String> {
        store
            .inner
            .get(name)
            .unwrap()
            .map(|s| s.expose().to_owned())
    }

    #[test]
    fn short_secrets_are_stored_as_is() {
        let store = store();
        store.set("clave", &Secret::new("corta")).unwrap();
        assert_eq!(raw(&store, "clave").as_deref(), Some("corta"));
        assert_eq!(store.get("clave").unwrap().unwrap().expose(), "corta");
        assert!(store.get("otra").unwrap().is_none());
    }

    #[test]
    fn long_secrets_are_split_and_joined() {
        let store = store();
        let long = "abcdefghij".repeat(3) + "xyz";
        store
            .set("mail/microsoft-1", &Secret::new(long.as_str()))
            .unwrap();
        assert_eq!(
            raw(&store, "mail/microsoft-1").as_deref(),
            Some("nestadmin-trozos:4")
        );
        assert_eq!(raw(&store, "mail/microsoft-1#4").as_deref(), Some("xyz"));
        assert_eq!(
            store.get("mail/microsoft-1").unwrap().unwrap().expose(),
            long
        );
    }

    #[test]
    fn replacing_and_deleting_leave_no_stale_parts() {
        let store = store();
        store.set("t", &Secret::new("a".repeat(35))).unwrap();
        store.set("t", &Secret::new("b".repeat(15))).unwrap();
        assert_eq!(store.get("t").unwrap().unwrap().expose(), "b".repeat(15));
        assert!(raw(&store, "t#3").is_none() && raw(&store, "t#4").is_none());
        store.set("t", &Secret::new("corta")).unwrap();
        assert!(raw(&store, "t#1").is_none() && raw(&store, "t#2").is_none());
        store.set("t", &Secret::new("c".repeat(25))).unwrap();
        store.delete("t").unwrap();
        assert!(store.get("t").unwrap().is_none());
        assert!((1..=3).all(|i| raw(&store, &format!("t#{i}")).is_none()));
    }

    #[test]
    fn a_missing_part_is_an_error_not_a_partial_secret() {
        let store = store();
        store.set("t", &Secret::new("d".repeat(25))).unwrap();
        store.inner.delete("t#2").unwrap();
        assert!(store.get("t").is_err());
    }

    #[test]
    fn a_secret_that_looks_like_the_marker_is_still_recovered() {
        let store = store();
        store.set("t", &Secret::new("nestadmin-trozos:1")).unwrap();
        assert_eq!(
            store.get("t").unwrap().unwrap().expose(),
            "nestadmin-trozos:1"
        );
    }

    /// Un token de Microsoft realista (unos 3 000 caracteres) cabe repartido en trozos que
    /// respetan el límite real de Windows (1 280 caracteres UTF-16).
    #[test]
    fn microsoft_sized_tokens_fit_windows_limits() {
        const { assert!(CHUNK_CHARS * 2 <= 2560) };
        let store = ChunkedStore::new(MemoryCredentialStore::default(), CHUNK_CHARS);
        let token = "x".repeat(3000);
        store
            .set("mail/microsoft-1", &Secret::new(token.as_str()))
            .unwrap();
        assert_eq!(
            store.get("mail/microsoft-1").unwrap().unwrap().expose(),
            token
        );
    }
}
