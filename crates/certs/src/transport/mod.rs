//! Transporte SOAP con TLS mutuo hacia la AEAT (T18b-B3, D279, D280, D285, R35).
//!
//! [`SoapTransport`] envía un cuerpo SOAP y devuelve un [`SendOutcome`] que distingue sin
//! ambigüedad tres casos, para que la capa de remisión anote o no el intento (D280):
//!
//! - [`SendOutcome::NotSent`]: el cuerpo **no llegó a escribirse en el socket** (DNS, conexión,
//!   TLS rechazado en cualquier sentido, certificado no disponible, tiempo agotado antes de
//!   escribir). No cuenta como envío.
//! - [`SendOutcome::SentNoResponse`]: el cuerpo pudo llegar, pero no hay respuesta completa.
//! - [`SendOutcome::Response`]: respuesta HTTP completa (cualquier estado).
//!
//! Los motivos son códigos estables, sin texto del sistema, NIF ni datos del certificado. Los
//! endpoints son constantes compiladas ([`Endpoint`]); la red real solo existe con la
//! característica de Cargo `aeat-red` (D285). La implementación real es [`MutualTlsTransport`]
//! (HTTP/1.1 mínimo sobre `rustls::StreamOwned`, sin `ureq`).

mod endpoints;
mod http;
mod tls;

use std::sync::Arc;

use rustls::pki_types::CertificateDer;
use rustls::sign::SigningKey;

use crate::CertError;

pub use endpoints::{AeatEnvironment, Channel, Endpoint, VERIFACTU_PATH};
#[cfg(feature = "test-support")]
pub use tls::TestTarget;
pub use tls::{MutualTlsTransport, Timeouts, MAX_RESPONSE_BYTES};

/// Cadena y clave de firma con las que se autentica el TLS mutuo. No lleva el contenido a
/// ningún registro: el `Debug` solo muestra el tamaño de la cadena.
#[derive(Clone)]
pub struct ClientCredentials {
    pub(crate) chain: Vec<CertificateDer<'static>>,
    pub(crate) key: Arc<dyn SigningKey>,
}

impl ClientCredentials {
    /// `chain`: certificado final primero y después los intermedios (sin la raíz), en DER.
    pub fn new(chain: Vec<Vec<u8>>, key: Arc<dyn SigningKey>) -> Self {
        Self {
            chain: chain.into_iter().map(CertificateDer::from).collect(),
            key,
        }
    }
}

impl std::fmt::Debug for ClientCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClientCredentials")
            .field("chain_len", &self.chain.len())
            .finish_non_exhaustive()
    }
}

#[cfg(windows)]
impl From<&crate::CertifiedSigner> for ClientCredentials {
    /// La clave sigue en Windows (CNG): solo se comparte el `SigningKey`.
    fn from(signer: &crate::CertifiedSigner) -> Self {
        ClientCredentials::new(signer.chain.clone(), signer.key.clone())
    }
}

/// Qué versión de TLS usar (D279). `Auto` negocia TLS 1.3 y 1.2 y, si la clave no puede firmar
/// con PSS (tarjeta que solo admite PKCS#1 v1.5), repite el saludo una vez con TLS 1.2.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TlsPreference {
    #[default]
    Auto,
    /// Pista de quien llama (recordada por huella): ir directamente a TLS 1.2.
    Tls12,
}

/// Motivo por el que el cuerpo no llegó a escribirse. Códigos estables ([`NotSentReason::code`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotSentReason {
    /// El certificado no se pudo obtener o no sirve (`certificate_*`, incluido `NeedsUser`).
    Certificate(CertError),
    /// El servidor rechazó nuestro certificado (`certificate_rejected`).
    CertificateRejected,
    /// El servidor exigió un certificado y no se le dio ninguno (`certificate_missing`).
    CertificateMissing,
    /// La clave no firma con ningún esquema admitido, ni siquiera en TLS 1.2
    /// (`certificate_key_unsupported`).
    KeyUnsupported,
    /// No se pudo resolver el nombre (`dns_failed`).
    Dns,
    /// Conexión rechazada o inalcanzable (`connect_failed`).
    Connect,
    /// La conexión TCP tardó más del límite (`connect_timeout`).
    ConnectTimeout,
    /// Nosotros rechazamos el certificado del servidor: confianza, nombre o vigencia
    /// (`server_certificate_invalid`).
    ServerCertificate,
    /// Otro fallo del saludo TLS, o el servidor cerró antes de aceptar (`tls_handshake_failed`).
    TlsHandshake,
    /// El saludo TLS no terminó a tiempo (`tls_timeout`).
    HandshakeTimeout,
    /// El transporte es de otro entorno que el endpoint pedido (`wrong_environment`).
    WrongEnvironment,
}

impl NotSentReason {
    pub fn code(&self) -> &'static str {
        match self {
            NotSentReason::Certificate(e) => e.code(),
            NotSentReason::CertificateRejected => "certificate_rejected",
            NotSentReason::CertificateMissing => "certificate_missing",
            NotSentReason::KeyUnsupported => "certificate_key_unsupported",
            NotSentReason::Dns => "dns_failed",
            NotSentReason::Connect => "connect_failed",
            NotSentReason::ConnectTimeout => "connect_timeout",
            NotSentReason::ServerCertificate => "server_certificate_invalid",
            NotSentReason::TlsHandshake => "tls_handshake_failed",
            NotSentReason::HandshakeTimeout => "tls_timeout",
            NotSentReason::WrongEnvironment => "wrong_environment",
        }
    }
}

/// Motivo por el que no hay respuesta completa aunque el cuerpo pudo llegar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoResponseReason {
    /// Se agotó el tiempo esperando la respuesta (`read_timeout`).
    ReadTimeout,
    /// Se agotó el tiempo escribiendo la petición (`write_timeout`).
    WriteTimeout,
    /// Falló la escritura de la petición (`write_failed`).
    WriteFailed,
    /// El servidor cerró o cortó la conexión sin respuesta completa (`connection_closed`).
    ConnectionClosed,
    /// La respuesta no es HTTP/1.1 bien formado (`response_invalid`).
    ResponseInvalid,
    /// La respuesta supera el límite de tamaño (`response_too_large`).
    ResponseTooLarge,
}

impl NoResponseReason {
    pub fn code(&self) -> &'static str {
        match self {
            NoResponseReason::ReadTimeout => "read_timeout",
            NoResponseReason::WriteTimeout => "write_timeout",
            NoResponseReason::WriteFailed => "write_failed",
            NoResponseReason::ConnectionClosed => "connection_closed",
            NoResponseReason::ResponseInvalid => "response_invalid",
            NoResponseReason::ResponseTooLarge => "response_too_large",
        }
    }
}

/// Resultado de un envío (ver el módulo).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendOutcome {
    NotSent(NotSentReason),
    SentNoResponse(NoResponseReason),
    Response { status: u16, body: Vec<u8> },
}

impl SendOutcome {
    /// `true` si el cuerpo pudo llegar al servidor: el intento cuenta como envío (D252, D280).
    pub fn body_may_have_been_sent(&self) -> bool {
        !matches!(self, SendOutcome::NotSent(_))
    }

    /// Código estable del motivo, si no hubo respuesta.
    pub fn reason_code(&self) -> Option<&'static str> {
        match self {
            SendOutcome::NotSent(r) => Some(r.code()),
            SendOutcome::SentNoResponse(r) => Some(r.code()),
            SendOutcome::Response { .. } => None,
        }
    }
}

/// Resultado más la indicación de TLS que debe recordar quien llama (por huella del
/// certificado) para la próxima vez.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransportReport {
    pub outcome: SendOutcome,
    /// `true` si el saludo solo funcionó con TLS 1.2 (la clave no firma con PSS): pasar
    /// [`TlsPreference::Tls12`] la próxima vez evita repetir el saludo.
    pub needs_tls12: bool,
}

/// Envío de un cuerpo SOAP por TLS mutuo. Una sola llamada, sin reintentos propios (la remisión
/// decide cuándo reintentar, D252).
pub trait SoapTransport: Send + Sync {
    fn send(
        &self,
        endpoint: Endpoint,
        body: &[u8],
        credentials: &ClientCredentials,
        tls: TlsPreference,
    ) -> TransportReport;
}

/// Error al pedir un transporte; no se abre ningún socket.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TransportInitError {
    /// La compilación no incluye la red real de la AEAT (característica `aeat-red`, D285).
    #[error("la red real de la AEAT no está incluida en esta compilación")]
    NetworkDisabled,
}

impl TransportInitError {
    pub fn code(&self) -> &'static str {
        match self {
            TransportInitError::NetworkDisabled => "network_disabled",
        }
    }
}
