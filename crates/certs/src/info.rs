use sha2::{Digest, Sha256};
use x509_parser::prelude::*;
use x509_parser::public_key::PublicKey;

use crate::nif::extract_nif;
use crate::CertError;

/// Tipo de certificado a efectos de la remisión: decide el endpoint (D279).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CertificateKind {
    /// Persona física.
    PersonaFisica,
    /// Representante de una persona jurídica o de una entidad sin personalidad.
    Representante,
    /// Sello de entidad (endpoint `www10` o `prewww10`).
    Sello,
    Desconocido,
}

/// Algoritmo y tamaño de la clave pública.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyInfo {
    Rsa {
        bits: usize,
    },
    /// Nombre de la curva: `P-256`, `P-384`, `P-521` o `other`.
    Ec {
        curve: &'static str,
    },
    Other,
}

/// Lo que se lee de un certificado. Contiene datos personales (NIF y nombres): solo para la
/// interfaz; el `Debug` los oculta.
#[derive(Clone, PartialEq, Eq)]
pub struct CertificateInfo {
    /// SHA-256 del DER en hexadecimal minúscula: identifica la elección recordada.
    pub fingerprint: String,
    pub kind: CertificateKind,
    /// NIF del titular (`serialNumber`).
    pub holder_nif: Option<String>,
    /// NIF del representado o de la entidad (`organizationIdentifier`).
    pub represented_nif: Option<String>,
    /// Milisegundos Unix.
    pub not_before_ms: i64,
    pub not_after_ms: i64,
    /// `true` si el uso de clave extendido incluye `clientAuth` o la extensión falta (RFC 5280).
    pub client_auth: bool,
    pub key: KeyInfo,
    pub issuer_name: Option<String>,
    pub subject_name: Option<String>,
}

impl std::fmt::Debug for CertificateInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CertificateInfo")
            .field("fingerprint", &self.fingerprint)
            .field("kind", &self.kind)
            .field("not_before_ms", &self.not_before_ms)
            .field("not_after_ms", &self.not_after_ms)
            .field("client_auth", &self.client_auth)
            .field("key", &self.key)
            .finish_non_exhaustive()
    }
}

const OID_COMMON_NAME: &str = "2.5.4.3";
const OID_SERIAL_NUMBER: &str = "2.5.4.5";
const OID_ORG_IDENTIFIER: &str = "2.5.4.97";
const OID_GIVEN_NAME: &str = "2.5.4.42";
const OID_SURNAME: &str = "2.5.4.4";
/// ETSI QCP-l y QCP-l-qscd (política de certificado de sello).
const SEAL_POLICIES: [&str; 2] = ["0.4.0.194112.1.1", "0.4.0.194112.1.3"];
/// `id-etsi-qct-eseal` dentro de `qcStatements`, codificado como OID (tag, longitud y valor).
const ESEAL_OID_DER: [u8; 9] = [0x06, 0x07, 0x04, 0x00, 0x8E, 0x46, 0x01, 0x06, 0x02];
const OID_QC_STATEMENTS: &str = "1.3.6.1.5.5.7.1.3";

fn attr(name: &X509Name<'_>, oid: &str) -> Option<String> {
    name.iter_attributes()
        .find(|a| a.attr_type().to_id_string() == oid)
        .and_then(|a| a.as_str().ok())
        .map(str::to_string)
        .filter(|s| !s.trim().is_empty())
}

fn key_info(cert: &X509Certificate<'_>) -> KeyInfo {
    let spki = cert.public_key();
    match spki.parsed() {
        Ok(PublicKey::RSA(rsa)) => KeyInfo::Rsa {
            bits: rsa.key_size(),
        },
        Ok(PublicKey::EC(_)) => {
            let curve = spki
                .algorithm
                .parameters
                .as_ref()
                .and_then(|p| p.as_oid().ok())
                .map(|o| o.to_id_string());
            let curve = match curve.as_deref() {
                Some("1.2.840.10045.3.1.7") => "P-256",
                Some("1.3.132.0.34") => "P-384",
                Some("1.3.132.0.35") => "P-521",
                _ => "other",
            };
            KeyInfo::Ec { curve }
        }
        _ => KeyInfo::Other,
    }
}

fn is_seal_by_extensions(cert: &X509Certificate<'_>) -> bool {
    cert.extensions()
        .iter()
        .any(|ext| match ext.parsed_extension() {
            ParsedExtension::CertificatePolicies(policies) => policies
                .iter()
                .any(|p| SEAL_POLICIES.contains(&p.policy_id.to_id_string().as_str())),
            _ => {
                ext.oid.to_id_string() == OID_QC_STATEMENTS
                    && ext
                        .value
                        .windows(ESEAL_OID_DER.len())
                        .any(|w| w == ESEAL_OID_DER)
            }
        })
}

fn classify(cert: &X509Certificate<'_>) -> CertificateKind {
    let subject = cert.subject();
    let has_person =
        attr(subject, OID_GIVEN_NAME).is_some() || attr(subject, OID_SURNAME).is_some();
    let has_org = attr(subject, OID_ORG_IDENTIFIER).is_some();
    if is_seal_by_extensions(cert) {
        CertificateKind::Sello
    } else if has_person && has_org {
        CertificateKind::Representante
    } else if has_person {
        CertificateKind::PersonaFisica
    } else if has_org {
        CertificateKind::Sello
    } else {
        CertificateKind::Desconocido
    }
}

/// Analiza un certificado en DER. `Err(CertError::Invalid)` si no es un X.509 bien formado o
/// sobran bytes tras el certificado.
pub fn inspect(der: &[u8]) -> Result<CertificateInfo, CertError> {
    let (rest, cert) = X509Certificate::from_der(der).map_err(|_| CertError::Invalid)?;
    if !rest.is_empty() {
        return Err(CertError::Invalid);
    }
    let client_auth = match cert.extended_key_usage() {
        Ok(Some(eku)) => eku.value.any || eku.value.client_auth,
        Ok(None) => true,
        Err(_) => return Err(CertError::Invalid),
    };
    let subject = cert.subject();
    Ok(CertificateInfo {
        fingerprint: Sha256::digest(der)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect(),
        kind: classify(&cert),
        holder_nif: attr(subject, OID_SERIAL_NUMBER).and_then(|s| extract_nif(&s)),
        represented_nif: attr(subject, OID_ORG_IDENTIFIER).and_then(|s| extract_nif(&s)),
        not_before_ms: cert.validity().not_before.timestamp().saturating_mul(1000),
        not_after_ms: cert.validity().not_after.timestamp().saturating_mul(1000),
        client_auth,
        key: key_info(&cert),
        issuer_name: attr(cert.issuer(), OID_COMMON_NAME),
        subject_name: attr(subject, OID_COMMON_NAME),
    })
}
