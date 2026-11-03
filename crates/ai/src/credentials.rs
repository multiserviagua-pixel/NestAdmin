//! Credenciales de proveedores externos (D08, R09). En Windows se guardan en el
//! administrador de credenciales; nunca en la base local, registros ni evaluaciones.

use std::collections::HashMap;
use std::fmt;
use std::sync::Mutex;

pub const SERVICE: &str = "NestAdmin";
pub const GROQ_API_KEY: &str = "groq_api_key";

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

/// Administrador de credenciales de Windows mediante `keyring-core`.
#[cfg(windows)]
pub struct WindowsCredentialStore;

#[cfg(windows)]
impl WindowsCredentialStore {
    pub fn new() -> Result<Self, CredentialError> {
        let store = windows_native_keyring_store::Store::new().map_err(platform)?;
        keyring_core::set_default_store(store);
        Ok(Self)
    }

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
        _ => "error del almacén",
    };
    CredentialError(kind.into())
}

#[cfg(windows)]
impl CredentialStore for WindowsCredentialStore {
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
