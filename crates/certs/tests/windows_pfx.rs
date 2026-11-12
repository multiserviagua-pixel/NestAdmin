//! B2: PFX custodiado con DPAPI y clave CNG (solo Windows). Todo sintético y en un directorio
//! temporal; nada se instala en el almacén del usuario.
#![cfg(windows)]

mod pfx_support;

use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use nestadmin_certs::{
    acquire, import_pfx, inspect, remove_pfx, AcquireMode, CertError, CertificateSource, KeyInfo,
    PfxError,
};
use pfx_support::*;
use rustls::sign::SigningKey;
use rustls::SignatureScheme;
use x509_parser::prelude::*;

const ENTROPY: &[u8] = b"entropia-de-instalacion-sintetica";

fn source(reference: &str) -> CertificateSource {
    CertificateSource::Pfx {
        reference: reference.to_string(),
    }
}

fn silent(reference: &str, dir: &Path, entropy: &[u8], now: i64) -> Result<(), CertError> {
    acquire(&source(reference), dir, entropy, AcquireMode::Silent, now).map(|_| ())
}

fn bin_files(dir: &Path) -> Vec<std::path::PathBuf> {
    match fs::read_dir(dir) {
        Ok(rd) => rd.filter_map(|e| e.ok()).map(|e| e.path()).collect(),
        Err(_) => vec![],
    }
}

fn spki_key(cert_der: &[u8]) -> Vec<u8> {
    let (_, cert) = X509Certificate::from_der(cert_der).unwrap();
    cert.public_key().subject_public_key.data.to_vec()
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

#[test]
fn import_acquire_and_sign_with_the_cng_key() {
    let dir = tempfile::tempdir().unwrap();
    let m = modern(PASSWORD);
    let (reference, info) = import_pfx(&m.pfx, PASSWORD, dir.path(), ENTROPY).unwrap();
    assert_eq!(reference.len(), 32);
    assert_eq!(info, inspect(&m.cert_der).unwrap());
    assert_eq!(info.key, KeyInfo::Ec { curve: "P-256" });

    let signer = acquire(
        &source(&reference),
        dir.path(),
        ENTROPY,
        AcquireMode::Silent,
        NOW_MS,
    )
    .unwrap();
    assert_eq!(signer.chain[0], m.cert_der);
    assert_eq!(signer.info.fingerprint, info.fingerprint);

    // Firma por el camino real de rustls y se comprueba con la clave pública del certificado.
    let message = b"mensaje sintetico a firmar";
    let chosen = signer
        .key
        .choose_scheme(&[SignatureScheme::ECDSA_NISTP256_SHA256])
        .expect("la clave P-256 ofrece ECDSA");
    let signature = chosen.sign(message).unwrap();
    ring::signature::UnparsedPublicKey::new(
        &ring::signature::ECDSA_P256_SHA256_ASN1,
        spki_key(&m.cert_der),
    )
    .verify(message, &signature)
    .expect("la firma verifica con la clave pública");
    // Un mensaje distinto no verifica.
    assert!(ring::signature::UnparsedPublicKey::new(
        &ring::signature::ECDSA_P256_SHA256_ASN1,
        spki_key(&m.cert_der),
    )
    .verify(b"otro mensaje", &signature)
    .is_err());
}

#[test]
fn the_signer_keeps_working_after_the_certified_signer_is_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let m = modern(PASSWORD);
    let (reference, _) = import_pfx(&m.pfx, PASSWORD, dir.path(), ENTROPY).unwrap();
    let signer = acquire(
        &source(&reference),
        dir.path(),
        ENTROPY,
        AcquireMode::Silent,
        NOW_MS,
    )
    .unwrap();
    let key = signer.key.clone();
    drop(signer);
    let chosen = key
        .choose_scheme(&[SignatureScheme::ECDSA_NISTP256_SHA256])
        .unwrap();
    drop(key);
    assert!(!chosen.sign(b"x").unwrap().is_empty());
}

#[test]
fn the_bin_file_holds_neither_the_pfx_nor_the_password_in_clear() {
    let dir = tempfile::tempdir().unwrap();
    let m = modern(PASSWORD);
    let (reference, _) = import_pfx(&m.pfx, PASSWORD, dir.path(), ENTROPY).unwrap();
    let files = bin_files(dir.path());
    assert_eq!(files.len(), 1, "solo el .bin, sin temporales: {files:?}");
    assert_eq!(
        files[0].file_name().unwrap().to_string_lossy(),
        format!("{reference}.bin")
    );
    let bytes = fs::read(&files[0]).unwrap();
    assert!(bytes.starts_with(b"NACERT\x01"), "cabecera con versión");
    assert!(!contains(&bytes, PASSWORD.as_bytes()));
    assert!(!contains(&bytes, &m.pfx[..64]));
    assert!(!contains(&bytes, &m.cert_der[..64]));
    assert!(!contains(&bytes, NIF.as_bytes()));
    assert!(!contains(&bytes, NAME.as_bytes()));
}

#[test]
fn wrong_password_and_corrupt_files_are_refused_and_leave_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let m = modern(PASSWORD);
    assert_eq!(
        import_pfx(&m.pfx, "contrasena-erronea", dir.path(), ENTROPY).unwrap_err(),
        PfxError::WrongPassword
    );
    assert_eq!(
        import_pfx(b"esto no es un pfx", PASSWORD, dir.path(), ENTROPY).unwrap_err(),
        PfxError::Invalid
    );
    let mut flipped = m.pfx.clone();
    let mid = flipped.len() / 2;
    flipped[mid] ^= 0xFF;
    assert!(import_pfx(&flipped, PASSWORD, dir.path(), ENTROPY).is_err());
    assert_eq!(
        import_pfx(&m.pfx[..m.pfx.len() / 2], PASSWORD, dir.path(), ENTROPY).unwrap_err(),
        PfxError::Invalid
    );
    assert!(
        bin_files(dir.path()).is_empty(),
        "no queda nada en la custodia"
    );
}

#[test]
fn a_pfx_without_a_private_key_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let pfx = without_key(PASSWORD);
    assert_eq!(
        import_pfx(&pfx, PASSWORD, dir.path(), ENTROPY).unwrap_err(),
        PfxError::NoPrivateKey
    );
    assert!(bin_files(dir.path()).is_empty());
}

#[test]
fn another_entropy_cannot_decrypt() {
    let dir = tempfile::tempdir().unwrap();
    let m = modern(PASSWORD);
    let (reference, _) = import_pfx(&m.pfx, PASSWORD, dir.path(), ENTROPY).unwrap();
    assert_eq!(
        silent(&reference, dir.path(), b"otra-entropia", NOW_MS).unwrap_err(),
        CertError::Missing
    );
    assert_eq!(
        silent(&reference, dir.path(), b"", NOW_MS).unwrap_err(),
        CertError::Missing
    );
    // Con la buena sigue funcionando.
    silent(&reference, dir.path(), ENTROPY, NOW_MS).unwrap();
}

#[test]
fn a_tampered_or_foreign_file_is_missing_not_a_crash() {
    let dir = tempfile::tempdir().unwrap();
    let m = modern(PASSWORD);
    let (reference, _) = import_pfx(&m.pfx, PASSWORD, dir.path(), ENTROPY).unwrap();
    let path = dir.path().join(format!("{reference}.bin"));
    let original = fs::read(&path).unwrap();

    let variants: Vec<Vec<u8>> = vec![
        // un byte del blob de DPAPI alterado
        {
            let mut v = original.clone();
            let n = v.len() - 5;
            v[n] ^= 0x01;
            v
        },
        // cabecera de otra versión
        {
            let mut v = original.clone();
            v[6] = 9;
            v
        },
        // magia distinta
        {
            let mut v = original.clone();
            v[0] = b'X';
            v
        },
        // truncado, solo cabecera y vacío
        original[..original.len() / 2].to_vec(),
        original[..7].to_vec(),
        Vec::new(),
    ];
    for (i, bytes) in variants.iter().enumerate() {
        fs::write(&path, bytes).unwrap();
        assert_eq!(
            silent(&reference, dir.path(), ENTROPY, NOW_MS).unwrap_err(),
            CertError::Missing,
            "variante {i}"
        );
    }
    // Una referencia que no es de la aplicación no sale de la carpeta.
    assert_eq!(
        silent("../../fuera", dir.path(), ENTROPY, NOW_MS).unwrap_err(),
        CertError::Missing
    );
}

#[test]
fn remove_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let m = modern(PASSWORD);
    let (reference, _) = import_pfx(&m.pfx, PASSWORD, dir.path(), ENTROPY).unwrap();
    remove_pfx(&reference, dir.path()).unwrap();
    assert!(bin_files(dir.path()).is_empty());
    remove_pfx(&reference, dir.path()).unwrap();
    remove_pfx("no-es-una-referencia", dir.path()).unwrap();
    remove_pfx(&reference, &dir.path().join("no-existe")).unwrap();
    assert_eq!(
        silent(&reference, dir.path(), ENTROPY, NOW_MS).unwrap_err(),
        CertError::Missing
    );
}

#[test]
fn an_expired_or_not_yet_valid_certificate_is_expired_without_opening_the_key() {
    let dir = tempfile::tempdir().unwrap();
    let m = modern(PASSWORD);
    let (reference, _) = import_pfx(&m.pfx, PASSWORD, dir.path(), ENTROPY).unwrap();
    assert_eq!(
        silent(&reference, dir.path(), ENTROPY, AFTER_EXPIRY_MS).unwrap_err(),
        CertError::Expired
    );
    assert_eq!(
        silent(&reference, dir.path(), ENTROPY, BEFORE_VALID_MS).unwrap_err(),
        CertError::Expired
    );
}

#[test]
fn a_certificate_missing_from_the_store_is_missing() {
    let dir = tempfile::tempdir().unwrap();
    let source = CertificateSource::Store {
        thumbprint_sha256: "00".repeat(32),
    };
    assert_eq!(
        acquire(&source, dir.path(), ENTROPY, AcquireMode::Silent, NOW_MS).unwrap_err(),
        CertError::Missing
    );
    assert_eq!(
        acquire(
            &CertificateSource::Store {
                thumbprint_sha256: "no es hexadecimal".into()
            },
            dir.path(),
            ENTROPY,
            AcquireMode::Interactive,
            NOW_MS
        )
        .unwrap_err(),
        CertError::Missing
    );
}

#[test]
fn importing_installs_nothing_in_the_windows_store() {
    let before: Vec<String> = nestadmin_certs::list_store_certificates()
        .into_iter()
        .map(|c| c.info.fingerprint)
        .collect();
    let dir = tempfile::tempdir().unwrap();
    let m = modern(PASSWORD);
    let (reference, info) = import_pfx(&m.pfx, PASSWORD, dir.path(), ENTROPY).unwrap();
    acquire(
        &source(&reference),
        dir.path(),
        ENTROPY,
        AcquireMode::Silent,
        NOW_MS,
    )
    .unwrap();
    let after: Vec<String> = nestadmin_certs::list_store_certificates()
        .into_iter()
        .map(|c| c.info.fingerprint)
        .collect();
    assert_eq!(before, after);
    assert!(!after.contains(&info.fingerprint));
}

#[test]
fn errors_never_carry_system_text_nif_or_name() {
    let dir = tempfile::tempdir().unwrap();
    let m = modern(PASSWORD);
    let (reference, _) = import_pfx(&m.pfx, PASSWORD, dir.path(), ENTROPY).unwrap();
    let mut texts = vec![
        format!(
            "{}",
            silent(&reference, dir.path(), b"x", NOW_MS).unwrap_err()
        ),
        format!(
            "{:?}",
            silent(&reference, dir.path(), b"x", NOW_MS).unwrap_err()
        ),
        format!(
            "{}",
            silent(&reference, dir.path(), ENTROPY, AFTER_EXPIRY_MS).unwrap_err()
        ),
    ];
    for e in [
        PfxError::WrongPassword,
        PfxError::Invalid,
        PfxError::NoPrivateKey,
        PfxError::Storage,
    ] {
        texts.push(format!("{e} {e:?} {}", e.code()));
    }
    for t in texts {
        for forbidden in [NIF, NAME, PASSWORD, "Windows", "0x8"] {
            assert!(!t.contains(forbidden), "{t}");
        }
    }
}

#[test]
fn an_old_3des_sha1_pfx_works() {
    let dir = tempfile::tempdir().unwrap();
    let m = legacy_3des(PASSWORD);
    let (reference, info) = import_pfx(&m.pfx, PASSWORD, dir.path(), ENTROPY).unwrap();
    assert_eq!(info, inspect(&m.cert_der).unwrap());
    let signer = acquire(
        &source(&reference),
        dir.path(),
        ENTROPY,
        AcquireMode::Silent,
        NOW_MS,
    )
    .unwrap();
    assert!(signer
        .key
        .choose_scheme(&[SignatureScheme::ECDSA_NISTP256_SHA256])
        .unwrap()
        .sign(b"antiguo")
        .is_ok());
}

/// PFX de OpenSSL con RSA: prueba también PKCS#1 y PSS con la clave CNG.
fn rsa_roundtrip(pbe: Option<&str>, label: &str) {
    let Some(m) = openssl_pfx(2048, pbe, PASSWORD) else {
        eprintln!("NO EJECUTADA ({label}): `openssl` no está en el PATH o no admite esas opciones");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let (reference, info) = import_pfx(&m.pfx, PASSWORD, dir.path(), ENTROPY).unwrap();
    assert_eq!(info.key, KeyInfo::Rsa { bits: 2048 });
    let signer = acquire(
        &source(&reference),
        dir.path(),
        ENTROPY,
        AcquireMode::Silent,
        now_ms(),
    )
    .unwrap();
    let key = spki_key(&m.cert_der);
    let message = b"mensaje rsa sintetico";
    let pss = signer
        .key
        .choose_scheme(&[SignatureScheme::RSA_PSS_SHA256])
        .unwrap()
        .sign(message)
        .unwrap();
    ring::signature::UnparsedPublicKey::new(&ring::signature::RSA_PSS_2048_8192_SHA256, &key)
        .verify(message, &pss)
        .unwrap();
    let pkcs1 = signer
        .key
        .choose_scheme(&[SignatureScheme::RSA_PKCS1_SHA256])
        .unwrap()
        .sign(message)
        .unwrap();
    ring::signature::UnparsedPublicKey::new(&ring::signature::RSA_PKCS1_2048_8192_SHA256, &key)
        .verify(message, &pkcs1)
        .unwrap();
}

#[test]
fn rsa_pfx_signs_with_pss_and_pkcs1() {
    rsa_roundtrip(None, "RSA moderno");
}

#[test]
fn old_rc2_and_3des_pfx_from_openssl_work() {
    rsa_roundtrip(Some("PBE-SHA1-RC2-40"), "PFX antiguo RC2-40");
    rsa_roundtrip(Some("PBE-SHA1-3DES"), "PFX antiguo 3DES");
}

#[test]
fn a_small_rsa_key_is_unsupported() {
    let Some(m) = openssl_pfx(1024, None, PASSWORD) else {
        eprintln!("NO EJECUTADA: `openssl` no está en el PATH");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let (reference, _) = import_pfx(&m.pfx, PASSWORD, dir.path(), ENTROPY).unwrap();
    assert_eq!(
        acquire(
            &source(&reference),
            dir.path(),
            ENTROPY,
            AcquireMode::Silent,
            now_ms()
        )
        .map(|_| ())
        .unwrap_err(),
        CertError::KeyUnsupported
    );
}
