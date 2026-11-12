//! Almacén de certificados de Windows (solo lectura) y análisis de PFX en memoria (D278).
//!
//! Nada se instala ni se exporta: el almacén se abre en modo lectura y el PFX se analiza con
//! `PKCS12_NO_PERSIST_KEY`, de modo que la clave vive solo en la memoria del proceso.

use std::ptr;
use std::sync::Arc;

use rustls_cng::cert::CertContext;
use rustls_cng::key::NCryptKey;
use windows_sys::Win32::Foundation::GetLastError;
use windows_sys::Win32::Security::Cryptography::{
    CertCloseStore, CertDuplicateCertificateContext, CertEnumCertificatesInStore,
    CertGetCertificateContextProperty, CertOpenStore, CryptAcquireCertificatePrivateKey,
    NCryptGetProperty, PFXImportCertStore, CERT_CONTEXT, CERT_KEY_CONTEXT,
    CERT_KEY_CONTEXT_PROP_ID, CERT_KEY_PROV_INFO_PROP_ID, CERT_KEY_SPEC, CERT_NCRYPT_KEY_SPEC,
    CERT_STORE_OPEN_EXISTING_FLAG, CERT_STORE_PROV_SYSTEM_W, CERT_STORE_READONLY_FLAG,
    CERT_SYSTEM_STORE_CURRENT_USER_ID, CERT_SYSTEM_STORE_LOCATION_SHIFT,
    CRYPT_ACQUIRE_ONLY_NCRYPT_KEY_FLAG, CRYPT_ACQUIRE_SILENT_FLAG, CRYPT_INTEGER_BLOB,
    CRYPT_KEY_PROV_INFO, HCERTSTORE, HCRYPTPROV_OR_NCRYPT_KEY_HANDLE, NCRYPT_ALLOW_EXPORT_FLAG,
    NCRYPT_EXPORT_POLICY_PROPERTY, PKCS12_ALWAYS_CNG_KSP, PKCS12_NO_PERSIST_KEY,
};
use zeroize::Zeroizing;

use crate::{inspect, CertificateInfo};

/// `CERT_SYSTEM_STORE_LOCAL_MACHINE_ID` (2): el crate `windows-sys` no lo exporta con ese nombre
/// en todas sus versiones, así que se fija aquí.
const LOCAL_MACHINE_ID: u32 = 2;

/// Dónde está el certificado dentro de Windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreLocation {
    CurrentUser,
    LocalMachine,
}

impl StoreLocation {
    fn id(self) -> u32 {
        match self {
            StoreLocation::CurrentUser => CERT_SYSTEM_STORE_CURRENT_USER_ID,
            StoreLocation::LocalMachine => LOCAL_MACHINE_ID,
        }
    }
}

/// Tipo de proveedor de la clave privada, a título informativo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyProvider {
    /// Proveedor CNG de software de Windows.
    SoftwareCng,
    /// CSP heredado (CryptoAPI): `acquire` no lo admite (`KeyUnsupported`).
    LegacyCsp,
    /// Tarjeta inteligente o minidriver.
    SmartCard,
    /// Otro proveedor CNG (TPM, proveedor en la nube...).
    Other,
}

/// Un certificado del almacén con clave privada asociada. Contiene datos personales (el
/// `Debug` de [`CertificateInfo`] los oculta).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreCertificate {
    pub info: CertificateInfo,
    pub location: StoreLocation,
    pub provider: KeyProvider,
    /// Solo informativo: la aplicación nunca exporta claves. `None` si no se puede saber sin
    /// molestar al usuario (tarjeta, TPM, proveedor en la nube).
    pub exportable: Option<bool>,
}

/// Almacén abierto o importado; se cierra al soltarlo. Los contextos de certificado que se
/// obtienen de él siguen siendo válidos después, pero se conserva para sujetar la clave de un
/// PFX sin persistir.
#[derive(Debug)]
pub(crate) struct Store(HCERTSTORE);

// SAFETY: los manejadores de almacén de CryptoAPI se pueden usar desde cualquier hilo
// (rustls-cng hace lo mismo con su `CertStore`).
unsafe impl Send for Store {}
unsafe impl Sync for Store {}

impl Drop for Store {
    fn drop(&mut self) {
        // SAFETY: el manejador es nuestro y solo se cierra aquí.
        unsafe { CertCloseStore(self.0, 0) };
    }
}

impl Store {
    /// Abre `<ubicación>\MY` en modo lectura; `None` si no existe o no hay permiso.
    pub(crate) fn open_my(location: StoreLocation) -> Option<Store> {
        let name: Vec<u16> = "MY".encode_utf16().chain([0]).collect();
        // SAFETY: `name` vive durante la llamada.
        let handle = unsafe {
            CertOpenStore(
                CERT_STORE_PROV_SYSTEM_W,
                0,
                0,
                (location.id() << CERT_SYSTEM_STORE_LOCATION_SHIFT)
                    | CERT_STORE_OPEN_EXISTING_FLAG
                    | CERT_STORE_READONLY_FLAG,
                name.as_ptr() as *const _,
            )
        };
        (!handle.is_null()).then_some(Store(handle))
    }

    /// Analiza un PFX con Windows sin persistir la clave ni instalar nada. `Err` lleva el
    /// código de error de Windows (solo para clasificar; no se muestra).
    pub(crate) fn import_pfx(data: &[u8], password: &str) -> Result<Store, u32> {
        let mut wide: Zeroizing<Vec<u16>> = Zeroizing::new(Vec::with_capacity(password.len() + 1));
        wide.extend(password.encode_utf16());
        wide.push(0);
        let blob = CRYPT_INTEGER_BLOB {
            cbData: data.len() as u32,
            pbData: data.as_ptr() as *mut u8,
        };
        // SAFETY: `blob` y `wide` viven durante la llamada.
        let handle = unsafe {
            PFXImportCertStore(
                &blob,
                wide.as_ptr(),
                PKCS12_NO_PERSIST_KEY | PKCS12_ALWAYS_CNG_KSP,
            )
        };
        if handle.is_null() {
            // SAFETY: lectura del último error del hilo.
            Err(unsafe { GetLastError() })
        } else {
            Ok(Store(handle))
        }
    }

    /// Todos los certificados del almacén (cada uno con su propia referencia).
    pub(crate) fn certificates(&self) -> Vec<CertContext> {
        let mut out = Vec::new();
        let mut prev: *const CERT_CONTEXT = ptr::null();
        loop {
            // SAFETY: `CertEnumCertificatesInStore` libera `prev` en la llamada siguiente; por
            // eso se duplica el contexto que se conserva.
            let next = unsafe { CertEnumCertificatesInStore(self.0, prev) };
            if next.is_null() {
                break;
            }
            let dup = unsafe { CertDuplicateCertificateContext(next) };
            out.push(CertContext::new_owned(dup));
            prev = next;
        }
        out
    }
}

/// Elige en un PFX analizado el certificado con clave privada (con `clientAuth` si hay
/// varios). Misma regla al importar y al obtener la clave.
pub(crate) fn pick_pfx_leaf(store: &Store) -> Option<CertContext> {
    let mut fallback = None;
    for ctx in store.certificates() {
        if acquire_raw(&ctx, true).is_err() {
            continue;
        }
        match inspect(ctx.as_der()) {
            Ok(info) if info.client_auth => return Some(ctx),
            Ok(_) if fallback.is_none() => fallback = Some(ctx),
            _ => {}
        }
    }
    fallback
}

/// Obtiene el manejador CNG de la clave de un contexto. `Err` es el código de Windows.
///
/// Si Windows deja la clave en el contexto (PFX sin persistir), el manejador es prestado y el
/// llamador debe conservar el contexto mientras use la clave; si no, es propio y se libera solo.
pub(crate) fn acquire_raw(ctx: &CertContext, silent: bool) -> Result<NCryptKey, u32> {
    let mut handle = HCRYPTPROV_OR_NCRYPT_KEY_HANDLE::default();
    let mut key_spec = CERT_KEY_SPEC::default();
    let mut caller_free = 0;
    let flags =
        CRYPT_ACQUIRE_ONLY_NCRYPT_KEY_FLAG | if silent { CRYPT_ACQUIRE_SILENT_FLAG } else { 0 };
    // SAFETY: parámetros de salida válidos durante la llamada.
    let ok = unsafe {
        CryptAcquireCertificatePrivateKey(
            ctx.inner(),
            flags,
            ptr::null(),
            &mut handle,
            &mut key_spec,
            &mut caller_free,
        )
    };
    if ok == 0 {
        // SAFETY: lectura del último error del hilo.
        let code = unsafe { GetLastError() };
        // Un PFX sin persistir deja la clave en `CERT_KEY_CONTEXT`, que `CryptAcquire...` no
        // sirve; se lee ese contexto a mano.
        return key_from_context(ctx, silent).ok_or(code);
    }
    let mut key = if caller_free != 0 {
        NCryptKey::new_owned(handle)
    } else {
        NCryptKey::new_borrowed(handle)
    };
    key.set_silent(silent);
    Ok(key)
}

/// Manejador CNG que Windows deja en el contexto de un certificado importado con
/// `PKCS12_NO_PERSIST_KEY` (propiedad `CERT_KEY_CONTEXT`). Es prestado: vive mientras viva el
/// contexto.
fn key_from_context(ctx: &CertContext, silent: bool) -> Option<NCryptKey> {
    let mut raw = CERT_KEY_CONTEXT::default();
    let mut size = std::mem::size_of::<CERT_KEY_CONTEXT>() as u32;
    // SAFETY: `raw` es una CERT_KEY_CONTEXT del tamaño indicado.
    let ok = unsafe {
        CertGetCertificateContextProperty(
            ctx.inner(),
            CERT_KEY_CONTEXT_PROP_ID,
            &mut raw as *mut _ as *mut _,
            &mut size,
        )
    };
    if ok == 0 || raw.dwKeySpec != CERT_NCRYPT_KEY_SPEC {
        return None;
    }
    // SAFETY: con `CERT_NCRYPT_KEY_SPEC` la unión contiene un manejador CNG.
    let handle = unsafe { raw.Anonymous.hNCryptKey };
    if handle == 0 {
        return None;
    }
    let mut key = NCryptKey::new_borrowed(handle);
    key.set_silent(silent);
    Some(key)
}

/// Proveedor de la clave según `CERT_KEY_PROV_INFO`; `None` si el certificado no tiene clave
/// asociada en el almacén.
fn key_provider(ctx: &CertContext) -> Option<KeyProvider> {
    let cert: *const CERT_CONTEXT = ctx.inner();
    let mut size = 0u32;
    // SAFETY: primera llamada para conocer el tamaño; sin búfer.
    if unsafe {
        CertGetCertificateContextProperty(
            cert,
            CERT_KEY_PROV_INFO_PROP_ID,
            ptr::null_mut(),
            &mut size,
        )
    } == 0
        || size == 0
    {
        return None;
    }
    // u64 para garantizar la alineación de la estructura.
    let mut buf = vec![0u64; (size as usize).div_ceil(8)];
    // SAFETY: el búfer es de al menos `size` bytes y está alineado.
    if unsafe {
        CertGetCertificateContextProperty(
            cert,
            CERT_KEY_PROV_INFO_PROP_ID,
            buf.as_mut_ptr() as *mut _,
            &mut size,
        )
    } == 0
    {
        return None;
    }
    // SAFETY: Windows rellenó una CRYPT_KEY_PROV_INFO cuyos punteros apuntan dentro de `buf`.
    let info = unsafe { &*(buf.as_ptr() as *const CRYPT_KEY_PROV_INFO) };
    let name = if info.pwszProvName.is_null() {
        String::new()
    } else {
        let mut len = 0;
        // SAFETY: cadena UTF-16 terminada en cero dentro de `buf`.
        unsafe {
            while *info.pwszProvName.add(len) != 0 {
                len += 1;
            }
            String::from_utf16_lossy(std::slice::from_raw_parts(info.pwszProvName, len))
        }
    };
    Some(classify_provider(&name, info.dwProvType))
}

fn classify_provider(name: &str, prov_type: u32) -> KeyProvider {
    let lower = name.to_lowercase();
    if lower.contains("smart card") || lower.contains("smartcard") {
        KeyProvider::SmartCard
    } else if prov_type != 0 {
        KeyProvider::LegacyCsp
    } else if lower == "microsoft software key storage provider" {
        KeyProvider::SoftwareCng
    } else {
        KeyProvider::Other
    }
}

/// Proveedor de la clave de un contexto del almacén, o `None` si no tiene clave asociada.
pub(crate) fn provider_of(ctx: &CertContext) -> Option<KeyProvider> {
    key_provider(ctx)
}

fn export_policy(key: &NCryptKey) -> Option<bool> {
    let mut policy = [0u8; 4];
    let mut written = 0u32;
    // SAFETY: búfer de 4 bytes válido durante la llamada.
    let status = unsafe {
        NCryptGetProperty(
            key.inner(),
            NCRYPT_EXPORT_POLICY_PROPERTY,
            policy.as_mut_ptr(),
            4,
            &mut written,
            0,
        )
    };
    (status == 0 && written == 4)
        .then(|| u32::from_ne_bytes(policy) & NCRYPT_ALLOW_EXPORT_FLAG != 0)
}

fn exportable_of(ctx: &CertContext, provider: KeyProvider) -> Option<bool> {
    match provider {
        // Las claves de una tarjeta no se exportan.
        KeyProvider::SmartCard => Some(false),
        // Solo se consulta, en silencio, donde no puede aparecer ningún diálogo.
        KeyProvider::SoftwareCng => acquire_raw(ctx, true).ok().and_then(|k| export_policy(&k)),
        KeyProvider::LegacyCsp | KeyProvider::Other => None,
    }
}

/// Lista los certificados con clave privada de `CurrentUser\MY` y, si se puede abrir sin
/// elevar permisos, de `LocalMachine\MY`. Nunca exporta claves ni instala nada; los
/// almacenes que no se pueden abrir se ignoran sin error. Sin duplicados por huella.
pub fn list_store_certificates() -> Vec<StoreCertificate> {
    let mut out: Vec<StoreCertificate> = Vec::new();
    for location in [StoreLocation::CurrentUser, StoreLocation::LocalMachine] {
        let Some(store) = Store::open_my(location) else {
            continue;
        };
        for ctx in store.certificates() {
            let Some(provider) = provider_of(&ctx) else {
                continue;
            };
            let Ok(info) = inspect(ctx.as_der()) else {
                continue;
            };
            if out.iter().any(|c| c.info.fingerprint == info.fingerprint) {
                continue;
            }
            let exportable = exportable_of(&ctx, provider);
            out.push(StoreCertificate {
                info,
                location,
                provider,
                exportable,
            });
        }
    }
    out
}

/// Busca por huella SHA-256 (hexadecimal) en `CurrentUser\MY` y después en `LocalMachine\MY`.
pub(crate) fn find_by_thumbprint(thumbprint: &str) -> Option<(CertContext, Arc<Store>)> {
    use sha2::{Digest, Sha256};
    let wanted = thumbprint.trim().to_ascii_lowercase();
    for location in [StoreLocation::CurrentUser, StoreLocation::LocalMachine] {
        let Some(store) = Store::open_my(location) else {
            continue;
        };
        let store = Arc::new(store);
        for ctx in store.certificates() {
            let digest = Sha256::digest(ctx.as_der());
            let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
            if hex == wanted {
                return Some((ctx, store));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_providers_by_name_and_type() {
        assert_eq!(
            classify_provider("Microsoft Software Key Storage Provider", 0),
            KeyProvider::SoftwareCng
        );
        assert_eq!(
            classify_provider("Microsoft Smart Card Key Storage Provider", 0),
            KeyProvider::SmartCard
        );
        assert_eq!(
            classify_provider("Microsoft Base Smart Card Crypto Provider", 1),
            KeyProvider::SmartCard
        );
        assert_eq!(
            classify_provider("Microsoft Enhanced RSA and AES Cryptographic Provider", 24),
            KeyProvider::LegacyCsp
        );
        assert_eq!(
            classify_provider("Microsoft Platform Crypto Provider", 0),
            KeyProvider::Other
        );
    }
}
