mod common;

use common::*;
use nestadmin_certs::{inspect, valid_tax_id, CertError, CertificateKind, KeyInfo};

#[test]
fn nif_sinteticos_tienen_control_valido() {
    for nif in [HOLDER_NIF, ENTITY_CIF, OTHER_NIF, "X1234567L"] {
        assert!(valid_tax_id(nif), "{nif}");
    }
    assert!(!valid_tax_id("12345678A"), "letra errónea expresa");
}

#[test]
fn persona_fisica() {
    let info = inspect(&make(&Spec::persona_fisica())).unwrap();
    assert_eq!(info.kind, CertificateKind::PersonaFisica);
    assert_eq!(info.holder_nif.as_deref(), Some(HOLDER_NIF));
    assert_eq!(info.represented_nif, None);
    assert!(info.client_auth);
    assert_eq!(info.key, KeyInfo::Ec { curve: "P-256" });
    assert_eq!(info.subject_name.as_deref(), Some(HOLDER_NAME));
    assert_eq!(info.issuer_name.as_deref(), Some(ISSUER_NAME));
    assert_eq!(info.fingerprint.len(), 64);
    assert!(info.not_before_ms < info.not_after_ms);
}

#[test]
fn persona_fisica_con_nif_sin_prefijo() {
    let info = inspect(&make(&Spec::persona_fisica().serial(HOLDER_NIF))).unwrap();
    assert_eq!(info.holder_nif.as_deref(), Some(HOLDER_NIF));
}

#[test]
fn nif_invalido_en_serial_number_no_se_toma() {
    let info = inspect(&make(&Spec::persona_fisica().serial("IDCES-12345678A"))).unwrap();
    assert_eq!(info.holder_nif, None);
}

#[test]
fn representante() {
    let info = inspect(&make(&Spec::representante())).unwrap();
    assert_eq!(info.kind, CertificateKind::Representante);
    assert_eq!(info.holder_nif.as_deref(), Some(HOLDER_NIF));
    assert_eq!(info.represented_nif.as_deref(), Some(ENTITY_CIF));
}

#[test]
fn sello() {
    let info = inspect(&make(&Spec::sello())).unwrap();
    assert_eq!(info.kind, CertificateKind::Sello);
    assert_eq!(info.holder_nif, None);
    assert_eq!(info.represented_nif.as_deref(), Some(ENTITY_CIF));
}

#[test]
fn sello_por_politica_qcp_l() {
    let info = inspect(&make(&Spec::persona_fisica().seal_policy())).unwrap();
    assert_eq!(info.kind, CertificateKind::Sello);
}

#[test]
fn sin_datos_de_titular_es_desconocido() {
    let mut spec = Spec::sello();
    spec.org_identifier = None;
    let info = inspect(&make(&spec)).unwrap();
    assert_eq!(info.kind, CertificateKind::Desconocido);
}

#[test]
fn huella_distingue_certificados_y_es_estable() {
    let der = make(&Spec::persona_fisica());
    assert_eq!(
        inspect(&der).unwrap().fingerprint,
        inspect(&der).unwrap().fingerprint
    );
    let other = make(&Spec::persona_fisica());
    assert_ne!(
        inspect(&der).unwrap().fingerprint,
        inspect(&other).unwrap().fingerprint
    );
}

#[test]
fn der_mal_formado() {
    assert_eq!(inspect(&[]), Err(CertError::Invalid));
    assert_eq!(inspect(b"no es un certificado"), Err(CertError::Invalid));
    let der = make(&Spec::persona_fisica());
    assert_eq!(inspect(&der[..der.len() / 2]), Err(CertError::Invalid));
    let mut trailing = der.clone();
    trailing.push(0);
    assert_eq!(inspect(&trailing), Err(CertError::Invalid));
}

#[test]
fn debug_de_info_oculta_nif_y_nombres() {
    let info = inspect(&make(&Spec::representante())).unwrap();
    let text = format!("{info:?}");
    for secret in [HOLDER_NIF, ENTITY_CIF, HOLDER_NAME, ISSUER_NAME] {
        assert!(!text.contains(secret), "Debug filtra {secret}");
    }
}
