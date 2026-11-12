/// Errores de certificado con código estable. El texto es propio y fijo: nunca lleva texto del
/// sistema, NIF ni nombre (D278).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CertError {
    #[error("no hay un certificado vinculado")]
    Missing,
    #[error("el certificado está caducado o todavía no es válido")]
    Expired,
    #[error("la clave del certificado necesita la intervención del usuario")]
    NeedsUser,
    #[error("la clave del certificado no es admitida")]
    KeyUnsupported,
    #[error("el certificado no es apto o fue rechazado")]
    Rejected,
    #[error("el certificado no es un X.509 válido")]
    Invalid,
}

impl CertError {
    /// Código estable (contratos.md); lo usan la cola, la interfaz y los registros.
    pub fn code(&self) -> &'static str {
        match self {
            CertError::Missing => "certificate_missing",
            CertError::Expired => "certificate_expired",
            CertError::NeedsUser => "certificate_needs_user",
            CertError::KeyUnsupported => "certificate_key_unsupported",
            CertError::Rejected => "certificate_rejected",
            CertError::Invalid => "certificate_invalid",
        }
    }
}
