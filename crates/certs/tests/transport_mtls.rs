//! B3: transporte SOAP con TLS mutuo contra un servidor en proceso (127.0.0.1) con
//! `WebPkiClientVerifier` y certificados sintéticos de `rcgen`. Nunca se contacta con la AEAT.

mod mtls_support;

use std::sync::Arc;
use std::time::Duration;

use mtls_support::*;
use nestadmin_certs::{
    AeatEnvironment, CertificateKind, ClientCredentials, Endpoint, MutualTlsTransport,
    NoResponseReason, NotSentReason, SendOutcome, SoapTransport, Timeouts, TlsPreference,
};

const BODY: &[u8] = b"<x/>";

fn endpoint() -> Endpoint {
    Endpoint::for_certificate(
        AeatEnvironment::Preproduction,
        CertificateKind::PersonaFisica,
    )
}

struct Fixture {
    ca: Ca,
    server: MtlsServer,
}

fn fixture(behavior: Behavior) -> Fixture {
    let ca = new_ca("AC PRUEBA SINTETICA");
    let server = MtlsServer::start(&ca.der, server_cert(&ca, &["localhost"]), behavior);
    Fixture { ca, server }
}

fn send(fx: &Fixture, creds: &ClientCredentials) -> nestadmin_certs::TransportReport {
    fx.server
        .transport("localhost", &fx.ca.der)
        .send(endpoint(), BODY, creds, TlsPreference::Auto)
}

#[test]
fn valid_certificate_gets_the_simulator_response() {
    let fx = fixture(Behavior::Proxy);
    let creds = client_cert(&fx.ca, Kind::PersonaFisica).credentials();
    let report = send(&fx, &creds);
    let SendOutcome::Response { status, body } = &report.outcome else {
        panic!("se esperaba una respuesta: {report:?}");
    };
    // El cuerpo `<x/>` no es un envío válido: el simulador responde con un `Fault` SOAP.
    assert_eq!(*status, 500);
    assert!(String::from_utf8_lossy(body).contains("Fault"));
    assert!(!report.needs_tls12);
    let seen = fx.server.seen();
    assert_eq!(seen.handshakes_ok, 1);
    assert_eq!(seen.client_kinds, vec![CertificateKind::PersonaFisica]);
    assert_eq!(seen.tls13, vec![true]);
    assert_eq!(seen.requests, vec![BODY.to_vec()]);
    // El simulador de la AEAT recibió lo mismo que el servidor TLS.
    assert_eq!(fx.server.simulator.estado().peticiones.len(), 1);
}

#[test]
fn seal_certificate_is_seen_as_a_seal_and_other_http_statuses_are_responses() {
    let fx = fixture(Behavior::Proxy);
    fx.server.simulator.estado().fallo_http_siguiente = Some(503);
    let creds = client_cert(&fx.ca, Kind::Sello).credentials();
    let report = send(&fx, &creds);
    assert!(matches!(
        report.outcome,
        SendOutcome::Response { status: 503, .. }
    ));
    assert_eq!(fx.server.seen().client_kinds, vec![CertificateKind::Sello]);
}

#[test]
fn client_without_certificate_is_rejected_and_not_sent() {
    let fx = fixture(Behavior::Proxy);
    let other = client_cert(&fx.ca, Kind::PersonaFisica);
    let none = ClientCredentials::new(vec![], other.signing_key());
    let report = send(&fx, &none);
    assert_eq!(
        report.outcome,
        SendOutcome::NotSent(NotSentReason::CertificateMissing),
        "{report:?}"
    );
    let seen = fx.server.seen();
    // D280: el servidor nunca llegó a ver una petición.
    assert!(seen.requests.is_empty());
    assert_eq!(seen.handshakes_ok, 0);
}

#[test]
fn certificate_from_another_ca_is_rejected_and_not_sent() {
    let fx = fixture(Behavior::Proxy);
    let foreign_ca = new_ca("OTRA AC PRUEBA SINTETICA");
    let creds = client_cert(&foreign_ca, Kind::PersonaFisica).credentials();
    let report = send(&fx, &creds);
    assert_eq!(
        report.outcome,
        SendOutcome::NotSent(NotSentReason::CertificateRejected),
        "{report:?}"
    );
    assert!(fx.server.seen().requests.is_empty());
    assert!(!report.outcome.body_may_have_been_sent());
}

#[test]
fn mismatching_server_name_is_not_sent() {
    let fx = fixture(Behavior::Proxy);
    let creds = client_cert(&fx.ca, Kind::PersonaFisica).credentials();
    // El servidor presenta `localhost`; el cliente espera otro nombre.
    let report = fx
        .server
        .transport("otro.example.invalid", &fx.ca.der)
        .send(endpoint(), BODY, &creds, TlsPreference::Auto);
    assert_eq!(
        report.outcome,
        SendOutcome::NotSent(NotSentReason::ServerCertificate)
    );
    assert!(fx.server.seen().requests.is_empty());
}

#[test]
fn untrusted_server_is_not_sent() {
    let fx = fixture(Behavior::Proxy);
    let creds = client_cert(&fx.ca, Kind::PersonaFisica).credentials();
    let unrelated = new_ca("CA NO CONFIADA");
    let report = fx.server.transport("localhost", &unrelated.der).send(
        endpoint(),
        BODY,
        &creds,
        TlsPreference::Auto,
    );
    assert_eq!(
        report.outcome,
        SendOutcome::NotSent(NotSentReason::ServerCertificate)
    );
    assert!(fx.server.seen().requests.is_empty());
}

#[test]
fn a_pkcs1_only_signer_falls_back_to_tls12_and_says_so() {
    let fx = fixture(Behavior::Proxy);
    let Some(rsa) = rsa_client_cert(&fx.ca, Kind::PersonaFisica) else {
        return;
    };
    let creds = ClientCredentials::new(
        vec![rsa.cert_der.clone()],
        Arc::new(Pkcs1Only(rsa.signing_key())),
    );
    let transport = fx.server.transport("localhost", &fx.ca.der);
    let report = transport.send(endpoint(), BODY, &creds, TlsPreference::Auto);
    assert!(
        matches!(report.outcome, SendOutcome::Response { .. }),
        "{report:?}"
    );
    assert!(report.needs_tls12, "debe indicar que hace falta TLS 1.2");
    let seen = fx.server.seen();
    assert_eq!(seen.tls13, vec![false], "la sesión que sirvió fue TLS 1.2");
    assert_eq!(seen.client_kinds, vec![CertificateKind::PersonaFisica]);
    // El primer saludo (TLS 1.3, sin certificado) fue rechazado sin ver ninguna petición.
    assert_eq!(seen.requests.len(), 1);

    // Con la pista, va directo a TLS 1.2: un solo saludo y sin rechazos nuevos.
    let rejected_before = seen.handshakes_rejected;
    let hinted = transport.send(endpoint(), BODY, &creds, TlsPreference::Tls12);
    assert!(matches!(hinted.outcome, SendOutcome::Response { .. }));
    assert!(hinted.needs_tls12);
    assert_eq!(fx.server.seen().handshakes_rejected, rejected_before);
}

#[test]
fn a_key_that_cannot_sign_even_in_tls12_is_key_unsupported() {
    let fx = fixture(Behavior::Proxy);
    // Una clave ECDSA envuelta para que nunca ofrezca nada: ningún esquema, en ninguna versión.
    let material = client_cert(&fx.ca, Kind::PersonaFisica);
    let creds = ClientCredentials::new(
        vec![material.cert_der.clone()],
        Arc::new(Pkcs1Only(material.signing_key())),
    );
    let report = send(&fx, &creds);
    assert_eq!(
        report.outcome,
        SendOutcome::NotSent(NotSentReason::KeyUnsupported),
        "{report:?}"
    );
    assert!(!report.needs_tls12);
    assert!(fx.server.seen().requests.is_empty());
}

#[test]
fn tls12_hint_goes_straight_to_tls12_with_any_key() {
    let fx = fixture(Behavior::Proxy);
    let creds = client_cert(&fx.ca, Kind::PersonaFisica).credentials();
    let report = fx.server.transport("localhost", &fx.ca.der).send(
        endpoint(),
        BODY,
        &creds,
        TlsPreference::Tls12,
    );
    assert!(matches!(report.outcome, SendOutcome::Response { .. }));
    assert!(report.needs_tls12);
    assert_eq!(fx.server.seen().tls13, vec![false]);
}

#[test]
fn server_closing_after_reading_the_body_is_sent_no_response() {
    let fx = fixture(Behavior::CloseAfterRequest);
    let creds = client_cert(&fx.ca, Kind::PersonaFisica).credentials();
    let report = send(&fx, &creds);
    assert_eq!(
        report.outcome,
        SendOutcome::SentNoResponse(NoResponseReason::ConnectionClosed)
    );
    assert!(report.outcome.body_may_have_been_sent());
    // El cuerpo sí llegó: por eso este intento cuenta como envío (D252).
    assert_eq!(fx.server.seen().requests, vec![BODY.to_vec()]);
}

#[test]
fn simulator_losing_its_response_is_sent_no_response() {
    let fx = fixture(Behavior::Proxy);
    fx.server.simulator.estado().perder_siguiente_respuesta = true;
    let creds = client_cert(&fx.ca, Kind::PersonaFisica).credentials();
    let report = send(&fx, &creds);
    assert!(
        matches!(report.outcome, SendOutcome::SentNoResponse(_)),
        "{report:?}"
    );
}

#[test]
fn closed_port_is_not_sent() {
    let ca = new_ca("AC PRUEBA SINTETICA");
    let transport = MutualTlsTransport::for_test(nestadmin_certs::TestTarget {
        addr: closed_port(),
        server_name: "localhost".into(),
        roots_der: vec![ca.der.clone()],
    });
    let creds = client_cert(&ca, Kind::PersonaFisica).credentials();
    let report = transport.send(endpoint(), BODY, &creds, TlsPreference::Auto);
    assert_eq!(report.outcome, SendOutcome::NotSent(NotSentReason::Connect));
    assert_eq!(report.outcome.reason_code(), Some("connect_failed"));
}

#[test]
fn read_timeout_after_writing_is_sent_no_response() {
    let fx = fixture(Behavior::Hang);
    let creds = client_cert(&fx.ca, Kind::PersonaFisica).credentials();
    let transport = fx
        .server
        .transport("localhost", &fx.ca.der)
        .with_timeouts(Timeouts {
            connect: Duration::from_secs(2),
            request: Duration::from_millis(1200),
        });
    let started = std::time::Instant::now();
    let report = transport.send(endpoint(), BODY, &creds, TlsPreference::Auto);
    assert_eq!(
        report.outcome,
        SendOutcome::SentNoResponse(NoResponseReason::ReadTimeout),
        "{report:?}"
    );
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(fx.server.seen().requests.len(), 1);
}

#[test]
fn handshake_that_never_finishes_is_not_sent() {
    // Un servidor TCP que acepta y calla: el saludo TLS no avanza.
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let addr = listener.local_addr().unwrap();
    let ca = new_ca("AC PRUEBA SINTETICA");
    let transport = MutualTlsTransport::for_test(nestadmin_certs::TestTarget {
        addr,
        server_name: "localhost".into(),
        roots_der: vec![ca.der.clone()],
    })
    .with_timeouts(Timeouts {
        connect: Duration::from_secs(2),
        request: Duration::from_millis(500),
    });
    let creds = client_cert(&ca, Kind::PersonaFisica).credentials();
    let report = transport.send(endpoint(), BODY, &creds, TlsPreference::Auto);
    assert_eq!(
        report.outcome,
        SendOutcome::NotSent(NotSentReason::HandshakeTimeout)
    );
    drop(listener);
}

#[test]
fn chunked_responses_are_decoded() {
    let raw = b"HTTP/1.1 200 OK\r\nContent-Type: text/xml\r\nTransfer-Encoding: chunked\r\n\r\n6\r\n<ok/> \r\n5\r\n<y/>\n\r\n0\r\n\r\n".to_vec();
    let fx = fixture(Behavior::Raw(raw));
    let creds = client_cert(&fx.ca, Kind::PersonaFisica).credentials();
    let report = send(&fx, &creds);
    assert_eq!(
        report.outcome,
        SendOutcome::Response {
            status: 200,
            body: b"<ok/> <y/>\n".to_vec()
        }
    );
}

#[test]
fn oversized_responses_are_sent_no_response() {
    let raw = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: 40\r\n\r\n{}",
        "x".repeat(40)
    )
    .into_bytes();
    let fx = fixture(Behavior::Raw(raw));
    let creds = client_cert(&fx.ca, Kind::PersonaFisica).credentials();
    let transport = fx
        .server
        .transport("localhost", &fx.ca.der)
        .with_max_response_bytes(10);
    let report = transport.send(endpoint(), BODY, &creds, TlsPreference::Auto);
    assert_eq!(
        report.outcome,
        SendOutcome::SentNoResponse(NoResponseReason::ResponseTooLarge)
    );
    // Dentro del límite, el mismo contenido llega.
    let ok = fx
        .server
        .transport("localhost", &fx.ca.der)
        .with_max_response_bytes(40)
        .send(endpoint(), BODY, &creds, TlsPreference::Auto);
    assert!(matches!(
        ok.outcome,
        SendOutcome::Response { status: 200, .. }
    ));
}

#[test]
fn garbage_and_truncated_responses_are_sent_no_response() {
    let fx = fixture(Behavior::Raw(b"esto no es http\r\n\r\n".to_vec()));
    let creds = client_cert(&fx.ca, Kind::PersonaFisica).credentials();
    assert_eq!(
        send(&fx, &creds).outcome,
        SendOutcome::SentNoResponse(NoResponseReason::ResponseInvalid)
    );
    let fx = fixture(Behavior::Raw(
        b"HTTP/1.1 200 OK\r\nContent-Length: 50\r\n\r\ncorto".to_vec(),
    ));
    let creds = client_cert(&fx.ca, Kind::PersonaFisica).credentials();
    assert_eq!(
        send(&fx, &creds).outcome,
        SendOutcome::SentNoResponse(NoResponseReason::ConnectionClosed)
    );
}

#[test]
fn reason_codes_are_stable_and_carry_no_system_text() {
    use nestadmin_certs::CertError;
    let codes: Vec<&str> = [
        NotSentReason::Certificate(CertError::NeedsUser),
        NotSentReason::Certificate(CertError::Missing),
        NotSentReason::CertificateRejected,
        NotSentReason::CertificateMissing,
        NotSentReason::KeyUnsupported,
        NotSentReason::Dns,
        NotSentReason::Connect,
        NotSentReason::ConnectTimeout,
        NotSentReason::ServerCertificate,
        NotSentReason::TlsHandshake,
        NotSentReason::HandshakeTimeout,
        NotSentReason::WrongEnvironment,
    ]
    .iter()
    .map(|r| r.code())
    .collect();
    assert_eq!(
        codes,
        [
            "certificate_needs_user",
            "certificate_missing",
            "certificate_rejected",
            "certificate_missing",
            "certificate_key_unsupported",
            "dns_failed",
            "connect_failed",
            "connect_timeout",
            "server_certificate_invalid",
            "tls_handshake_failed",
            "tls_timeout",
            "wrong_environment",
        ]
    );
    for code in codes {
        assert!(code.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'));
    }
}

/// El servidor rechaza el certificado cliente muy tarde: la alerta llega 400 ms después del
/// saludo, bastante más que la gracia de 100 ms con la que el transporte escucha tras el saludo
/// TLS 1.3, y por tanto cuando el cliente ya escribió el cuerpo. Aun así debe ser `NotSent`
/// (D280): el servidor nunca completó el saludo con nuestro certificado, nunca leyó la petición
/// y ningún servidor responde a una petición con una alerta de certificado. Si se clasificara
/// como `SentNoResponse`, un certificado rechazado anotaría un intento que cuenta y un duplicado
/// ajeno podría darse por propio.
#[test]
fn a_certificate_alert_after_the_grace_and_after_writing_the_body_is_still_not_sent() {
    let ca = new_ca("AC PRUEBA SINTETICA");
    let server = MtlsServer::start_slow_rejecting(
        &ca.der,
        server_cert(&ca, &["localhost"]),
        std::time::Duration::from_millis(400),
    );
    let creds = client_cert(&ca, Kind::PersonaFisica).credentials();
    let report =
        server
            .transport("localhost", &ca.der)
            .send(endpoint(), BODY, &creds, TlsPreference::Auto);
    assert!(
        matches!(report.outcome, SendOutcome::NotSent(_)),
        "{:?}",
        report.outcome
    );
    assert!(!report.outcome.body_may_have_been_sent());
    assert!(
        server.seen().requests.is_empty(),
        "el servidor nunca vio el cuerpo"
    );
}
