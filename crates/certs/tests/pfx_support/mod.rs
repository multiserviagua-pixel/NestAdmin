//! Material sintético para las pruebas de B2: certificados con `rcgen` empaquetados en PFX con
//! `p12-keystore`, y PFX antiguos con `openssl` si está en el PATH. Todo se genera en tiempo de
//! ejecución en memoria o en un directorio temporal; no se versiona nada.
#![cfg(windows)]
#![allow(dead_code)]

use p12_keystore::{
    Certificate, EncryptionAlgorithm, KeyStore, KeyStoreEntry, MacAlgorithm, PrivateKey,
    PrivateKeyChain,
};
use rcgen::{
    date_time_ymd, BasicConstraints, CertificateParams, DistinguishedName, DnType, DnValue,
    ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair, KeyUsagePurpose, PKCS_ECDSA_P256_SHA256,
};

pub const PASSWORD: &str = "prueba-sintetica";
/// Un NIF sintético con letra válida y un nombre reconocible: no deben aparecer en ningún error.
pub const NIF: &str = "12345678Z";
pub const NAME: &str = "PRUEBA SINTETICA TITULAR";
/// 2026-06-01T00:00:00Z en milisegundos (dentro de la vigencia 2026-01-01..2030-01-01).
pub const NOW_MS: i64 = 1_780_272_000_000;
pub const AFTER_EXPIRY_MS: i64 = 1_950_000_000_000; // 2031
pub const BEFORE_VALID_MS: i64 = 1_700_000_000_000; // 2023

pub struct Material {
    pub pfx: Vec<u8>,
    pub cert_der: Vec<u8>,
}

fn utf8(s: &str) -> DnValue {
    DnValue::Utf8String(s.to_string())
}

/// Genera una CA y un certificado de persona física con clave P-256.
fn generate() -> (Vec<u8>, Vec<u8>, Vec<u8>, Vec<u8>) {
    let ca_key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
    let mut ca_params = CertificateParams::default();
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca_params.key_usages = vec![KeyUsagePurpose::KeyCertSign];
    let mut ca_dn = DistinguishedName::new();
    ca_dn.push(DnType::CommonName, "AC PRUEBA SINTETICA");
    ca_params.distinguished_name = ca_dn;
    ca_params.not_before = date_time_ymd(2026, 1, 1);
    ca_params.not_after = date_time_ymd(2030, 1, 1);
    let ca_der = ca_params
        .clone()
        .self_signed(&ca_key)
        .unwrap()
        .der()
        .to_vec();
    let issuer = Issuer::new(ca_params, ca_key);

    let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
    let mut params = CertificateParams::default();
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, NAME);
    dn.push(
        DnType::CustomDnType(vec![2, 5, 4, 5]),
        utf8(&format!("IDCES-{NIF}")),
    );
    params.distinguished_name = dn;
    params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
    params.not_before = date_time_ymd(2026, 1, 1);
    params.not_after = date_time_ymd(2030, 1, 1);
    let cert_der = params.signed_by(&key, &issuer).unwrap().der().to_vec();
    (cert_der, ca_der, key.serialize_der(), Vec::new())
}

fn chain_store(cert: &[u8], ca: &[u8], key: &[u8]) -> KeyStore {
    let chain = PrivateKeyChain::new(
        b"prueba".as_slice(),
        PrivateKey::from_der(key).unwrap(),
        [
            Certificate::from_der(cert).unwrap(),
            Certificate::from_der(ca).unwrap(),
        ],
    );
    let mut store = KeyStore::new();
    store.add_entry("prueba", KeyStoreEntry::PrivateKeyChain(chain));
    store
}

/// PFX moderno (PBES2 con AES-256 y HMAC-SHA256).
pub fn modern(password: &str) -> Material {
    let (cert_der, ca, key, _) = generate();
    let pfx = chain_store(&cert_der, &ca, &key)
        .writer(password)
        .write()
        .unwrap();
    Material { pfx, cert_der }
}

/// PFX antiguo: 3DES con SHA-1 y MAC HMAC-SHA1, como los que exportaban Windows y los
/// navegadores de hace años.
pub fn legacy_3des(password: &str) -> Material {
    let (cert_der, ca, key, _) = generate();
    let pfx = chain_store(&cert_der, &ca, &key)
        .writer(password)
        .encryption_algorithm(EncryptionAlgorithm::PbeWithShaAnd3KeyTripleDesCbc)
        .mac_algorithm(MacAlgorithm::HmacSha1)
        .write()
        .unwrap();
    Material { pfx, cert_der }
}

/// PFX sin clave privada: solo un certificado.
pub fn without_key(password: &str) -> Vec<u8> {
    let (cert_der, _, _, _) = generate();
    let mut store = KeyStore::new();
    store.add_entry(
        "solo-certificado",
        KeyStoreEntry::Certificate(Certificate::from_der(&cert_der).unwrap()),
    );
    store.writer(password).write().unwrap()
}

/// Busca `needle` dentro de `haystack`.
pub fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && haystack.windows(needle.len()).any(|w| w == needle)
}

/// Genera con OpenSSL un PFX con clave RSA del tamaño dado y el cifrado indicado. `None` si
/// `openssl` no está en el PATH o no admite las opciones antiguas (sin proveedor `legacy`).
pub struct OpenSsl {
    pub pfx: Vec<u8>,
    pub cert_der: Vec<u8>,
}

pub fn openssl_pfx(bits: u32, legacy_pbe: Option<&str>, password: &str) -> Option<OpenSsl> {
    use std::process::Command;
    let dir = tempfile::tempdir().ok()?;
    let p = |name: &str| dir.path().join(name).to_string_lossy().into_owned();
    let run = |args: &[&str]| {
        Command::new("openssl")
            .args(args)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    };
    let (key, cert, pfx, der) = (p("k.pem"), p("c.pem"), p("a.pfx"), p("c.der"));
    let subject = format!("/CN={NAME}/serialNumber=IDCES-{NIF}");
    let bits_arg = format!("rsa:{bits}");
    if !run(&[
        "req",
        "-x509",
        "-newkey",
        &bits_arg,
        "-nodes",
        "-keyout",
        &key,
        "-out",
        &cert,
        "-subj",
        &subject,
        "-days",
        "3650",
        "-addext",
        "extendedKeyUsage=clientAuth",
    ]) {
        return None;
    }
    if !run(&["x509", "-in", &cert, "-outform", "DER", "-out", &der]) {
        return None;
    }
    let pass = format!("pass:{password}");
    let mut args = vec![
        "pkcs12", "-export", "-inkey", &key, "-in", &cert, "-out", &pfx,
    ];
    if let Some(pbe) = legacy_pbe {
        args.extend([
            "-legacy", "-certpbe", pbe, "-keypbe", pbe, "-macalg", "sha1",
        ]);
    }
    args.extend(["-passout", &pass]);
    if !run(&args) {
        return None;
    }
    Some(OpenSsl {
        pfx: std::fs::read(&pfx).ok()?,
        cert_der: std::fs::read(&der).ok()?,
    })
}
