//! Análisis y validación de certificados X.509 para la remisión Verifactu (T18b, D278, R35).
//!
//! Parte pura y multiplataforma: recibe el DER de un certificado y devuelve su huella, tipo,
//! NIF, vigencia, uso `clientAuth` y clave. No toca el almacén de Windows, ni PFX, ni DPAPI,
//! ni la red (eso llega en B2 y B3). El nombre y el NIF del titular son datos personales:
//! solo para la interfaz, nunca en errores ni registros, y el `Debug` de [`CertificateInfo`]
//! los oculta.

mod error;
mod info;
mod nif;
mod validate;

pub use error::CertError;
pub use info::{inspect, CertificateInfo, CertificateKind, KeyInfo};
pub use nif::{normalize_tax_id, valid_tax_id};
pub use validate::{validate, ExpiryWarning, NifMatch, Validation, ValidityStatus, MIN_RSA_BITS};

/// Milisegundos de un día, para los avisos de caducidad.
pub(crate) const DAY_MS: i64 = 86_400_000;
