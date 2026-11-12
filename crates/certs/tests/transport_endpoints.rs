//! B3: tabla de endpoints (constantes compiladas) y puerta de la red real (D279, D285).

use nestadmin_certs::{
    AeatEnvironment, CertificateKind, Channel, Endpoint, MutualTlsTransport, VERIFACTU_PATH,
};

const KINDS: [CertificateKind; 4] = [
    CertificateKind::PersonaFisica,
    CertificateKind::Representante,
    CertificateKind::Sello,
    CertificateKind::Desconocido,
];

#[test]
fn seal_goes_to_the_seal_host_and_the_rest_to_the_personal_one() {
    for kind in KINDS {
        let pre = Endpoint::for_certificate(AeatEnvironment::Preproduction, kind);
        let prod = Endpoint::for_certificate(AeatEnvironment::Production, kind);
        if kind == CertificateKind::Sello {
            assert_eq!(pre.host(), "prewww10.aeat.es");
            assert_eq!(prod.host(), "www10.agenciatributaria.gob.es");
            assert_eq!(pre.channel, Channel::Seal);
        } else {
            assert_eq!(pre.host(), "prewww1.aeat.es");
            assert_eq!(prod.host(), "www1.agenciatributaria.gob.es");
            assert_eq!(pre.channel, Channel::Personal);
        }
    }
}

#[test]
fn urls_are_the_official_ones() {
    let url = |env, channel| {
        Endpoint {
            environment: env,
            channel,
        }
        .url()
    };
    use AeatEnvironment::*;
    assert_eq!(
        url(Preproduction, Channel::Personal),
        "https://prewww1.aeat.es/wlpl/TIKE-CONT/ws/SistemaFacturacion/VerifactuSOAP"
    );
    assert_eq!(
        url(Preproduction, Channel::Seal),
        "https://prewww10.aeat.es/wlpl/TIKE-CONT/ws/SistemaFacturacion/VerifactuSOAP"
    );
    assert_eq!(
        url(Production, Channel::Personal),
        "https://www1.agenciatributaria.gob.es/wlpl/TIKE-CONT/ws/SistemaFacturacion/VerifactuSOAP"
    );
    assert_eq!(
        url(Production, Channel::Seal),
        "https://www10.agenciatributaria.gob.es/wlpl/TIKE-CONT/ws/SistemaFacturacion/VerifactuSOAP"
    );
    // La URL es host + ruta, siempre HTTPS.
    for env in [Preproduction, Production] {
        for channel in [Channel::Personal, Channel::Seal] {
            let e = Endpoint {
                environment: env,
                channel,
            };
            assert_eq!(e.url(), format!("https://{}{}", e.host(), VERIFACTU_PATH));
        }
    }
}

#[cfg(not(feature = "aeat-red"))]
#[test]
fn without_the_network_feature_real_environments_give_a_stable_error() {
    use nestadmin_certs::TransportInitError;
    // `aeat()` falla antes de construir nada: no hay socket, ni DNS, ni TLS.
    for env in [AeatEnvironment::Preproduction, AeatEnvironment::Production] {
        let err = MutualTlsTransport::aeat(env).unwrap_err();
        assert_eq!(err, TransportInitError::NetworkDisabled);
        assert_eq!(err.code(), "network_disabled");
    }
}

#[cfg(feature = "aeat-red")]
#[test]
fn with_the_network_feature_a_transport_never_serves_another_environment() {
    use nestadmin_certs::{
        ClientCredentials, NotSentReason, SendOutcome, SoapTransport, TlsPreference,
    };
    // No se conecta con nada: el entorno distinto se rechaza antes de abrir un socket.
    let transport = MutualTlsTransport::aeat(AeatEnvironment::Preproduction).unwrap();
    let key = {
        use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};
        let k = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
        let der = PrivateKeyDer::from(PrivatePkcs8KeyDer::from(k.serialize_der()));
        rustls::crypto::ring::sign::any_supported_type(&der).unwrap()
    };
    let creds = ClientCredentials::new(vec![], key);
    let report = transport.send(
        Endpoint::for_certificate(AeatEnvironment::Production, CertificateKind::PersonaFisica),
        b"<x/>",
        &creds,
        TlsPreference::Auto,
    );
    assert_eq!(
        report.outcome,
        SendOutcome::NotSent(NotSentReason::WrongEnvironment)
    );
}
