use crate::{normalize_tax_id, CertError, CertificateInfo, CertificateKind, KeyInfo, DAY_MS};

/// Tamaño mínimo de una clave RSA admitida.
pub const MIN_RSA_BITS: usize = 2048;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValidityStatus {
    Valid,
    Expired,
    NotYetValid,
    /// El uso de clave extendido existe y no incluye `clientAuth`.
    NoClientAuth,
    /// RSA menor de 2048 bits, curva distinta de P-256 y P-384, u otro algoritmo.
    KeyUnsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NifMatch {
    Holder,
    Represented,
    /// No coincide (o el certificado no trae NIF). No bloquea (D283): decide quien llama.
    None,
}

/// Aviso de caducidad: el umbral más cercano ya alcanzado.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExpiryWarning {
    Days60,
    Days30,
    Days7,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Validation {
    pub status: ValidityStatus,
    pub nif_match: NifMatch,
    /// Solo con estado `Valid`.
    pub expiry_warning: Option<ExpiryWarning>,
    /// Días completos hasta `not_after` (negativo si ya caducó).
    pub days_remaining: i64,
    /// El certificado es de sello: B3 elige `www10` o `prewww10` (D279).
    pub is_seal: bool,
}

impl Validation {
    /// Convierte el estado en el error de contrato; los avisos y el NIF no bloquean.
    pub fn into_result(self) -> Result<Self, CertError> {
        match self.status {
            ValidityStatus::Valid => Ok(self),
            ValidityStatus::Expired | ValidityStatus::NotYetValid => Err(CertError::Expired),
            ValidityStatus::NoClientAuth => Err(CertError::Rejected),
            ValidityStatus::KeyUnsupported => Err(CertError::KeyUnsupported),
        }
    }
}

fn key_supported(key: &KeyInfo) -> bool {
    match key {
        KeyInfo::Rsa { bits } => *bits >= MIN_RSA_BITS,
        KeyInfo::Ec { curve } => matches!(*curve, "P-256" | "P-384"),
        KeyInfo::Other => false,
    }
}

/// Valida un certificado ya analizado frente al obligado y el reloj (`now_ms`, Unix en ms).
pub fn validate(info: &CertificateInfo, obligado_nif: &str, now_ms: i64) -> Validation {
    let status = if now_ms > info.not_after_ms {
        ValidityStatus::Expired
    } else if now_ms < info.not_before_ms {
        ValidityStatus::NotYetValid
    } else if !info.client_auth {
        ValidityStatus::NoClientAuth
    } else if !key_supported(&info.key) {
        ValidityStatus::KeyUnsupported
    } else {
        ValidityStatus::Valid
    };
    let target = normalize_tax_id(obligado_nif);
    let eq = |n: &Option<String>| !target.is_empty() && n.as_deref() == Some(target.as_str());
    let nif_match = if eq(&info.holder_nif) {
        NifMatch::Holder
    } else if eq(&info.represented_nif) {
        NifMatch::Represented
    } else {
        NifMatch::None
    };
    let remaining = info.not_after_ms.saturating_sub(now_ms);
    let expiry_warning = if status != ValidityStatus::Valid {
        None
    } else if remaining <= 7 * DAY_MS {
        Some(ExpiryWarning::Days7)
    } else if remaining <= 30 * DAY_MS {
        Some(ExpiryWarning::Days30)
    } else if remaining <= 60 * DAY_MS {
        Some(ExpiryWarning::Days60)
    } else {
        None
    };
    Validation {
        status,
        nif_match,
        expiry_warning,
        days_remaining: remaining.div_euclid(DAY_MS),
        is_seal: info.kind == CertificateKind::Sello,
    }
}
