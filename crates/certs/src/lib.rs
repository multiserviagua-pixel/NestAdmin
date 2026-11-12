//! Análisis y validación de certificados X.509 para la remisión Verifactu (T18b, D278, R35).
//!
//! Parte pura y multiplataforma (B1): recibe el DER de un certificado y devuelve su huella,
//! tipo, NIF, vigencia, uso `clientAuth` y clave. Parte de Windows (B2, `cfg(windows)`): lista
//! del almacén (`list_store_certificates`), PFX custodiado con DPAPI (`import_pfx`,
//! `remove_pfx`) y obtención de la clave de firma CNG (`acquire`). La red y el TLS mutuo son
//! de B3. El nombre y el NIF del titular son datos personales: solo para la interfaz, nunca
//! en errores ni registros, y el `Debug` de [`CertificateInfo`] los oculta.

mod error;
mod info;
mod nif;
mod source;
mod validate;

#[cfg(windows)]
mod acquire;
#[cfg(windows)]
mod dpapi;
#[cfg(windows)]
mod store;
#[cfg(windows)]
mod vault;

pub use error::CertError;
pub use info::{inspect, CertificateInfo, CertificateKind, KeyInfo};
pub use nif::{normalize_tax_id, valid_tax_id};
pub use source::CertificateSource;
pub use validate::{validate, ExpiryWarning, NifMatch, Validation, ValidityStatus, MIN_RSA_BITS};

#[cfg(windows)]
pub use acquire::{acquire, acquire_with_timeout, AcquireMode, CertKey, CertifiedSigner};
#[cfg(windows)]
pub use store::{list_store_certificates, KeyProvider, StoreCertificate, StoreLocation};
#[cfg(windows)]
pub use vault::{import_pfx, remove_pfx, PfxError};

/// Milisegundos de un día, para los avisos de caducidad.
pub(crate) const DAY_MS: i64 = 86_400_000;
