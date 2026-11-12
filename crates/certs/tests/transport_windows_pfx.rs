//! B3 con Windows: un PFX sintético custodiado con DPAPI, obtenido con `acquire` (firma CNG
//! real, sin tocar el almacén del usuario), frente al servidor TLS mutuo de las pruebas.
#![cfg(windows)]

mod mtls_support;
mod pfx_support;

use std::time::{SystemTime, UNIX_EPOCH};

use mtls_support::*;
use nestadmin_certs::{
    acquire, import_pfx, AcquireMode, AeatEnvironment, CertificateKind, CertificateSource,
    ClientCredentials, Endpoint, SendOutcome, SoapTransport, TlsPreference,
};
use pfx_support::*;

const ENTROPY: &[u8] = b"entropia-de-instalacion-sintetica";

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

#[test]
fn a_pfx_acquired_with_cng_authenticates_against_the_mutual_tls_server() {
    let dir = tempfile::tempdir().unwrap();
    let material = modern(PASSWORD);
    let (reference, _info) = import_pfx(&material.pfx, PASSWORD, dir.path(), ENTROPY).unwrap();
    let signer = acquire(
        &CertificateSource::Pfx { reference },
        dir.path(),
        ENTROPY,
        AcquireMode::Silent,
        now_ms(),
    )
    .unwrap();

    // El servidor confía en la CA de este PFX; su propio certificado sale de otra CA sintética.
    let server_ca = new_ca("AC SERVIDOR PRUEBA SINTETICA");
    let server = MtlsServer::start(
        &material.ca_der,
        server_cert(&server_ca, &["localhost"]),
        Behavior::Proxy,
    );
    let transport = server.transport("localhost", &server_ca.der);
    let endpoint = Endpoint::for_certificate(
        AeatEnvironment::Preproduction,
        CertificateKind::PersonaFisica,
    );
    let report = transport.send(
        endpoint,
        b"<x/>",
        &ClientCredentials::from(&signer),
        TlsPreference::Auto,
    );
    assert!(
        matches!(report.outcome, SendOutcome::Response { status: 500, .. }),
        "{report:?}"
    );
    let seen = server.seen();
    assert_eq!(seen.handshakes_ok, 1);
    assert_eq!(
        seen.client_kinds,
        vec![nestadmin_certs::inspect(&material.cert_der).unwrap().kind]
    );
    assert_eq!(seen.requests, vec![b"<x/>".to_vec()]);
    // Con una clave ECDSA de CNG, el saludo es TLS 1.3 y no hace falta bajar a 1.2.
    assert_eq!(seen.tls13, vec![true]);
    assert!(!report.needs_tls12);
}
