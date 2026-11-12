//! Certificados sintéticos generados en tiempo de ejecución con `rcgen` (skill
//! `certificados-de-prueba`). Nada se escribe en disco ni se versiona. Los NIF son ficticios
//! con letra o dígito de control válidos.
#![allow(dead_code)]

use rcgen::{
    date_time_ymd, CertificateParams, CustomExtension, DistinguishedName, DnType, DnValue,
    ExtendedKeyUsagePurpose, Issuer, KeyPair, KeyUsagePurpose, PublicKeyData, SignatureAlgorithm,
    PKCS_ECDSA_P256_SHA256, PKCS_ECDSA_P384_SHA384, PKCS_RSA_SHA256,
};

pub const HOLDER_NIF: &str = "12345678Z";
pub const ENTITY_CIF: &str = "B12345674";
pub const OTHER_NIF: &str = "89890001K";
pub const HOLDER_NAME: &str = "PRUEBA SINTETICA TITULAR";
pub const ISSUER_NAME: &str = "AC PRUEBA SINTETICA";

/// Qué clave lleva el certificado.
#[derive(Clone, Copy)]
pub enum Key {
    P256,
    P384,
    /// Clave pública RSA del tamaño dado (sin clave privada; el certificado lo firma la CA).
    Rsa(usize),
}

#[derive(Clone)]
pub struct Spec {
    pub common_name: Option<&'static str>,
    pub serial_number: Option<String>,
    pub org_identifier: Option<String>,
    pub given_name: bool,
    pub eku: Option<Vec<ExtendedKeyUsagePurpose>>,
    pub validity: ((i32, u8, u8), (i32, u8, u8)),
    pub key: Key,
    pub seal_policy: bool,
}

impl Spec {
    fn base() -> Self {
        Spec {
            common_name: Some(HOLDER_NAME),
            serial_number: None,
            org_identifier: None,
            given_name: false,
            eku: Some(vec![ExtendedKeyUsagePurpose::ClientAuth]),
            validity: ((2026, 1, 1), (2027, 1, 1)),
            key: Key::P256,
            seal_policy: false,
        }
    }

    /// Persona física: `serialNumber` IDCES-<NIF>, nombre y apellidos.
    pub fn persona_fisica() -> Self {
        Spec {
            serial_number: Some(format!("IDCES-{HOLDER_NIF}")),
            given_name: true,
            ..Self::base()
        }
    }

    /// Representante: NIF del representante en `serialNumber`, entidad en `organizationIdentifier`.
    pub fn representante() -> Self {
        Spec {
            org_identifier: Some(format!("VATES-{ENTITY_CIF}")),
            ..Self::persona_fisica()
        }
    }

    /// Sello: entidad en `organizationIdentifier`, sin nombre ni apellidos.
    pub fn sello() -> Self {
        Spec {
            common_name: Some("SELLO SINTETICO"),
            org_identifier: Some(format!("VATES-{ENTITY_CIF}")),
            ..Self::base()
        }
    }

    pub fn eku(mut self, eku: Option<Vec<ExtendedKeyUsagePurpose>>) -> Self {
        self.eku = eku;
        self
    }

    pub fn key(mut self, key: Key) -> Self {
        self.key = key;
        self
    }

    pub fn validity(mut self, from: (i32, u8, u8), to: (i32, u8, u8)) -> Self {
        self.validity = (from, to);
        self
    }

    pub fn serial(mut self, serial: &str) -> Self {
        self.serial_number = Some(serial.to_string());
        self
    }

    pub fn seal_policy(mut self) -> Self {
        self.seal_policy = true;
        self
    }
}

/// Clave pública RSA sintética: solo el módulo y el exponente, sin clave privada.
struct RsaPublic {
    der: Vec<u8>,
}

impl RsaPublic {
    fn new(bits: usize) -> Self {
        let mut n = vec![0x5Au8; bits / 8];
        n[0] |= 0x80; // el bit alto fija el tamaño
        *n.last_mut().unwrap() |= 1; // impar
        let mut int_n = vec![0u8];
        int_n.extend(n);
        let body = [der(0x02, &int_n), der(0x02, &[0x01, 0x00, 0x01])].concat();
        RsaPublic {
            der: der(0x30, &body),
        }
    }
}

impl PublicKeyData for RsaPublic {
    fn der_bytes(&self) -> &[u8] {
        &self.der
    }
    fn algorithm(&self) -> &'static SignatureAlgorithm {
        &PKCS_RSA_SHA256
    }
}

fn der(tag: u8, content: &[u8]) -> Vec<u8> {
    let mut out = vec![tag];
    match content.len() {
        n if n < 0x80 => out.push(n as u8),
        n if n < 0x100 => out.extend([0x81, n as u8]),
        n => out.extend([0x82, (n >> 8) as u8, n as u8]),
    }
    out.extend(content);
    out
}

fn utf8(s: &str) -> DnValue {
    DnValue::Utf8String(s.to_string())
}

/// Devuelve el DER de un certificado sintético firmado por una CA sintética propia.
pub fn make(spec: &Spec) -> Vec<u8> {
    let ca_key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
    let mut ca_params = CertificateParams::default();
    ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    ca_params.key_usages = vec![KeyUsagePurpose::KeyCertSign];
    ca_params.distinguished_name = {
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, ISSUER_NAME);
        dn
    };
    let issuer = Issuer::new(ca_params, ca_key);

    let mut params = CertificateParams::default();
    let mut dn = DistinguishedName::new();
    if let Some(cn) = spec.common_name {
        dn.push(DnType::CommonName, cn);
    }
    if let Some(sn) = &spec.serial_number {
        dn.push(DnType::CustomDnType(vec![2, 5, 4, 5]), utf8(sn));
    }
    if spec.given_name {
        dn.push(DnType::CustomDnType(vec![2, 5, 4, 42]), utf8("PRUEBA"));
        dn.push(DnType::CustomDnType(vec![2, 5, 4, 4]), utf8("SINTETICA"));
    }
    if let Some(org) = &spec.org_identifier {
        dn.push(DnType::OrganizationName, "ENTIDAD SINTETICA SL");
        dn.push(DnType::CustomDnType(vec![2, 5, 4, 97]), utf8(org));
    }
    params.distinguished_name = dn;
    params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    params.extended_key_usages = spec.eku.clone().unwrap_or_default();
    let ((fy, fm, fd), (ty, tm, td)) = spec.validity;
    params.not_before = date_time_ymd(fy, fm, fd);
    params.not_after = date_time_ymd(ty, tm, td);
    if spec.seal_policy {
        // certificatePolicies { QCP-l (0.4.0.194112.1.1) }
        let oid = [0x06, 0x07, 0x04, 0x00, 0x8B, 0xEC, 0x40, 0x01, 0x01];
        let content = der(0x30, &der(0x30, &oid));
        params
            .custom_extensions
            .push(CustomExtension::from_oid_content(&[2, 5, 29, 32], content));
    }

    let cert = match spec.key {
        Key::P256 | Key::P384 => {
            let alg = if matches!(spec.key, Key::P256) {
                &PKCS_ECDSA_P256_SHA256
            } else {
                &PKCS_ECDSA_P384_SHA384
            };
            let key = KeyPair::generate_for(alg).unwrap();
            params.signed_by(&key, &issuer).unwrap()
        }
        Key::Rsa(bits) => params.signed_by(&RsaPublic::new(bits), &issuer).unwrap(),
    };
    cert.der().to_vec()
}

pub const DAY_MS: i64 = 86_400_000;
