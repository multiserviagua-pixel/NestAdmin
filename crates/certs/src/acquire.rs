//! Obtención de la clave de firma de un certificado (D278, D280). La clave nunca sale de
//! Windows: se firma con CNG. La aplicación nunca maneja el PIN; si hace falta, lo pide Windows
//! en su propio diálogo y solo en modo [`AcquireMode::Interactive`].

use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc};
use std::time::Duration;

use rustls::sign::{Signer, SigningKey};
use rustls::{Error as TlsError, SignatureAlgorithm, SignatureScheme};
use rustls_cng::cert::CertContext;
use rustls_cng::error::CngError;
use rustls_cng::key::{AlgorithmGroup, NCryptKey, SignaturePadding};
use rustls_cng::signer::CngSigningKey;

use crate::store::{
    acquire_raw, build_chain, find_by_thumbprint, pick_pfx_leaf, provider_of, Store,
};
use crate::vault;
use crate::{inspect, validate, CertError, CertificateInfo, CertificateSource, KeyProvider};

/// Cómo se pide la clave.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcquireMode {
    /// Sin ventana ni PIN: si la clave necesita PIN, confirmación o una tarjeta que no está,
    /// devuelve `NeedsUser`. Es el modo de la remisión automática.
    Silent,
    /// Windows puede mostrar su diálogo (PIN, tarjeta). Es el modo de «Remitir ahora».
    Interactive,
}

/// Lo que se retiene mientras se use la clave: el contexto del certificado y el almacén que lo
/// sujeta (con un PFX sin persistir, la clave vive en el contexto).
#[derive(Debug)]
struct Keep {
    _context: CertContext,
    _store: Arc<Store>,
}

/// Clave de firma de un certificado de Windows lista para `rustls`: implementa `SigningKey` y
/// sujeta lo que necesita mientras exista (ella o cualquier firmante que de ella salga).
#[derive(Debug)]
pub struct CertKey {
    inner: CngSigningKey,
    keep: Arc<Keep>,
}

impl SigningKey for CertKey {
    fn choose_scheme(&self, offered: &[SignatureScheme]) -> Option<Box<dyn Signer>> {
        let inner = self.inner.choose_scheme(offered)?;
        Some(Box::new(KeepSigner {
            inner,
            _keep: self.keep.clone(),
        }))
    }

    fn algorithm(&self) -> SignatureAlgorithm {
        self.inner.algorithm()
    }
}

#[derive(Debug)]
struct KeepSigner {
    inner: Box<dyn Signer>,
    _keep: Arc<Keep>,
}

impl Signer for KeepSigner {
    fn sign(&self, message: &[u8]) -> Result<Vec<u8>, TlsError> {
        self.inner.sign(message)
    }

    fn scheme(&self) -> SignatureScheme {
        self.inner.scheme()
    }
}

/// Cadena de certificados y clave de firma, lista para un `ResolvesClientCert` (B3).
pub struct CertifiedSigner {
    /// Certificado final primero, después los intermedios (sin la raíz), en DER.
    pub chain: Vec<Vec<u8>>,
    pub key: Arc<CertKey>,
    /// Datos del certificado final (con datos personales: solo para la interfaz).
    pub info: CertificateInfo,
    pub provider: KeyProvider,
}

impl std::fmt::Debug for CertifiedSigner {
    /// Solo el número de certificados y el proveedor: nada del titular, el NIF ni la huella.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CertifiedSigner")
            .field("chain_len", &self.chain.len())
            .field("provider", &self.provider)
            .finish_non_exhaustive()
    }
}

// Códigos HRESULT de Windows que se traducen (solo para clasificar; no se muestran).
const NTE_BAD_ALGID: u32 = 0x8009_0008;
const NTE_NO_KEY: u32 = 0x8009_000D;
const NTE_NOT_FOUND: u32 = 0x8009_0011;
const NTE_BAD_KEYSET: u32 = 0x8009_0016;
const NTE_KEYSET_NOT_DEF: u32 = 0x8009_0019;
const NTE_SILENT_CONTEXT: u32 = 0x8009_0022;
const NTE_NOT_SUPPORTED: u32 = 0x8009_0029;
const NTE_USER_CANCELLED: u32 = 0x8009_0036;
const CRYPT_E_NOT_FOUND: u32 = 0x8009_2004;
const CRYPT_E_NO_KEY_PROPERTY: u32 = 0x8009_200B;
const ERROR_CANCELLED: u32 = 1223;
/// `HRESULT_FROM_WIN32(ERROR_CANCELLED)`.
const HRESULT_CANCELLED: u32 = 0x8007_04C7;
const NTE_DEVICE_NOT_READY: u32 = 0x8009_0030;
const NTE_DEVICE_NOT_FOUND: u32 = 0x8009_0035;
/// `SCARD_W_CHV_BLOCKED`: la tarjeta está bloqueada; pedir el PIN otra vez no sirve.
const SCARD_W_CHV_BLOCKED: u32 = 0x8010_006C;

fn map_os_error(code: u32) -> CertError {
    match code {
        // Una tarjeta bloqueada es inútil: no se vuelve a pedir el PIN.
        SCARD_W_CHV_BLOCKED => CertError::Rejected,
        // PIN, confirmación, dispositivo no listo o ausente, tarjeta ausente o retirada
        // (SCARD_*, 0x8010xxxx) y bloqueo o fallo transitorio del TPM (0x8028xxxx).
        NTE_SILENT_CONTEXT | NTE_USER_CANCELLED | ERROR_CANCELLED | HRESULT_CANCELLED
        | NTE_DEVICE_NOT_READY | NTE_DEVICE_NOT_FOUND => CertError::NeedsUser,
        c if c >> 16 == 0x8010 || c >> 16 == 0x8028 => CertError::NeedsUser,
        NTE_BAD_KEYSET
        | NTE_KEYSET_NOT_DEF
        | NTE_NO_KEY
        | NTE_NOT_FOUND
        | CRYPT_E_NOT_FOUND
        | CRYPT_E_NO_KEY_PROPERTY => CertError::Missing,
        NTE_BAD_ALGID | NTE_NOT_SUPPORTED => CertError::KeyUnsupported,
        _ => CertError::Rejected,
    }
}

fn map_cng(error: &CngError) -> CertError {
    match error {
        CngError::WindowsError(code) => map_os_error(*code),
        CngError::UnsupportedKeyAlgorithmGroup => CertError::KeyUnsupported,
        _ => CertError::Rejected,
    }
}

/// Firma una huella de prueba para comprobar que la clave responde sin ayuda (o, en modo
/// interactivo, para que Windows pida ya el PIN). El resultado se descarta.
fn probe(key: &NCryptKey) -> Result<(), CertError> {
    let padding = match key.algorithm_group().map_err(|e| map_cng(&e))? {
        AlgorithmGroup::Rsa => SignaturePadding::Pkcs1,
        // Windows etiqueta como ECDH muchas claves EC importadas que sí firman (los PFX con
        // curva P-256, por ejemplo): se intenta firmar y, si la clave de verdad no firma,
        // `NTE_NOT_SUPPORTED` o `NTE_BAD_ALGID` dan `KeyUnsupported` (`map_os_error`).
        AlgorithmGroup::Ecdsa | AlgorithmGroup::Ecdh => SignaturePadding::None,
    };
    key.sign(&[0x5A; 32], padding)
        .map(|_| ())
        .map_err(|e| map_cng(&e))
}

/// Resuelve la fuente en un contexto de certificado y su almacén.
fn resolve(
    source: &CertificateSource,
    store_dir: &Path,
    entropy: &[u8],
) -> Result<(CertContext, Arc<Store>, KeyProvider), CertError> {
    match source {
        CertificateSource::Store { thumbprint_sha256 } => {
            let (ctx, store) = find_by_thumbprint(thumbprint_sha256).ok_or(CertError::Missing)?;
            let provider = provider_of(&ctx).ok_or(CertError::Missing)?;
            if provider == KeyProvider::LegacyCsp {
                return Err(CertError::KeyUnsupported);
            }
            Ok((ctx, store, provider))
        }
        CertificateSource::Pfx { reference } => {
            let secret = vault::load(reference, store_dir, entropy).ok_or(CertError::Missing)?;
            let store =
                Store::import_pfx(&secret.pfx, &secret.password).map_err(|_| CertError::Missing)?;
            drop(secret);
            let ctx = pick_pfx_leaf(&store).ok_or(CertError::Missing)?;
            Ok((ctx, Arc::new(store), KeyProvider::SoftwareCng))
        }
    }
}

/// Obtiene la cadena y la clave de firma de `source`.
///
/// **Los proveedores de tarjeta o de la nube pueden ignorar el modo silencioso y colgarse**
/// (D280). Por eso B3 ejecutará `acquire` fuera del hilo de la remisión y con un tiempo límite;
/// si se agota, el resultado es `NeedsUser`. [`acquire_with_timeout`] lo hace ya.
///
/// Orden: se resuelve el certificado (`Missing`), se valida con el reloj `now_ms` (`Expired`,
/// `KeyUnsupported`, `Rejected` si no sirve para `clientAuth`) y, solo entonces, se abre la
/// clave y se firma una huella de prueba. En `Silent` no se muestra nada: si Windows necesita
/// al usuario devuelve `NeedsUser` (D280: no se anota ningún intento). Ningún error lleva texto
/// del sistema, NIF ni nombre.
pub fn acquire(
    source: &CertificateSource,
    store_dir: &Path,
    entropy: &[u8],
    mode: AcquireMode,
    now_ms: i64,
) -> Result<CertifiedSigner, CertError> {
    let (context, store, provider) = resolve(source, store_dir, entropy)?;
    let info = inspect(context.as_der())?;
    // El NIF del obligado no interviene aquí: la coincidencia no bloquea (D283).
    validate(&info, "", now_ms).into_result()?;

    // La clave que Windows deja en el contexto solo existe en la vía del PFX.
    let from_pfx = matches!(source, CertificateSource::Pfx { .. });
    let key = acquire_raw(&context, mode == AcquireMode::Silent, from_pfx).map_err(map_os_error)?;
    probe(&key)?;
    let inner = CngSigningKey::new(key).map_err(|e| map_cng(&e))?;
    // Sin descargas de red; si no se puede construir, es un error, no solo el final.
    let chain = build_chain(&context, &store).ok_or(CertError::Rejected)?;
    Ok(CertifiedSigner {
        chain,
        key: Arc::new(CertKey {
            inner,
            keep: Arc::new(Keep {
                _context: context,
                _store: store,
            }),
        }),
        info,
        provider,
    })
}

/// Ejecuta `f` en un hilo y espera como mucho `timeout`; si se agota, `NeedsUser`. El hilo
/// sigue por su cuenta y su resultado se descarta.
fn run_with_timeout<T: Send + 'static>(
    timeout: Duration,
    f: impl FnOnce() -> Result<T, CertError> + Send + 'static,
) -> Result<T, CertError> {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("certs-acquire".into())
        .spawn(move || {
            let _ = tx.send(f());
        })
        .map_err(|_| CertError::Rejected)?;
    rx.recv_timeout(timeout)
        .unwrap_or(Err(CertError::NeedsUser))
}

/// [`acquire`] en un hilo aparte con tiempo límite: un proveedor que ignora el modo silencioso
/// y se cuelga devuelve `NeedsUser` al agotarse `timeout`, sin bloquear a quien llama.
pub fn acquire_with_timeout(
    source: &CertificateSource,
    store_dir: &Path,
    entropy: &[u8],
    mode: AcquireMode,
    now_ms: i64,
    timeout: Duration,
) -> Result<CertifiedSigner, CertError> {
    let source = source.clone();
    let store_dir: PathBuf = store_dir.to_path_buf();
    let entropy = entropy.to_vec();
    run_with_timeout(timeout, move || {
        acquire(&source, &store_dir, &entropy, mode, now_ms)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_codes_map_to_stable_errors() {
        // Sin hardware no se puede provocar un PIN: se comprueba la traducción de los códigos.
        assert_eq!(map_os_error(NTE_SILENT_CONTEXT), CertError::NeedsUser);
        assert_eq!(map_os_error(NTE_USER_CANCELLED), CertError::NeedsUser);
        assert_eq!(map_os_error(0x8010_000C), CertError::NeedsUser); // sin tarjeta
        assert_eq!(map_os_error(0x8010_0069), CertError::NeedsUser); // tarjeta retirada
        assert_eq!(map_os_error(NTE_BAD_KEYSET), CertError::Missing);
        assert_eq!(map_os_error(CRYPT_E_NO_KEY_PROPERTY), CertError::Missing);
        assert_eq!(map_os_error(NTE_BAD_ALGID), CertError::KeyUnsupported);
        assert_eq!(map_os_error(HRESULT_CANCELLED), CertError::NeedsUser);
        assert_eq!(map_os_error(NTE_DEVICE_NOT_READY), CertError::NeedsUser);
        assert_eq!(map_os_error(NTE_DEVICE_NOT_FOUND), CertError::NeedsUser);
        assert_eq!(map_os_error(0x8028_0921), CertError::NeedsUser); // TPM bloqueado
        assert_eq!(map_os_error(SCARD_W_CHV_BLOCKED), CertError::Rejected);
        assert_eq!(map_os_error(0xDEAD_BEEF), CertError::Rejected);
        assert_eq!(
            map_cng(&CngError::UnsupportedKeyAlgorithmGroup),
            CertError::KeyUnsupported
        );
    }

    #[test]
    fn a_hung_provider_times_out_as_needs_user() {
        let hung = run_with_timeout(Duration::from_millis(50), || {
            std::thread::sleep(Duration::from_secs(2));
            Ok(1)
        });
        assert_eq!(hung.unwrap_err(), CertError::NeedsUser);
        assert_eq!(
            run_with_timeout(Duration::from_secs(10), || Ok(7)).unwrap(),
            7
        );
        let failed: Result<u8, _> =
            run_with_timeout(Duration::from_secs(10), || Err(CertError::Missing));
        assert_eq!(failed.unwrap_err(), CertError::Missing);
    }
}
