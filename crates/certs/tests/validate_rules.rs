mod common;

use common::*;
use nestadmin_certs::{
    inspect, validate, CertError, CertificateInfo, ExpiryWarning, NifMatch, ValidityStatus,
};
use rcgen::ExtendedKeyUsagePurpose;

fn info(spec: &Spec) -> CertificateInfo {
    inspect(&make(spec)).unwrap()
}

/// Un instante en mitad de la vigencia del certificado.
fn mid(i: &CertificateInfo) -> i64 {
    i.not_before_ms + (i.not_after_ms - i.not_before_ms) / 2
}

#[test]
fn valido_y_sin_aviso() {
    let i = info(&Spec::persona_fisica());
    let v = validate(&i, HOLDER_NIF, mid(&i));
    assert_eq!(v.status, ValidityStatus::Valid);
    assert_eq!(v.expiry_warning, None);
    assert!(!v.is_seal);
    assert!(v.into_result().is_ok());
}

#[test]
fn caducado() {
    let i = info(&Spec::persona_fisica().validity((2020, 1, 1), (2021, 1, 1)));
    let v = validate(&i, HOLDER_NIF, i.not_after_ms + DAY_MS);
    assert_eq!(v.status, ValidityStatus::Expired);
    assert_eq!(v.expiry_warning, None);
    assert!(v.days_remaining < 0);
    assert_eq!(v.into_result().unwrap_err(), CertError::Expired);
}

#[test]
fn todavia_no_valido() {
    let i = info(&Spec::persona_fisica().validity((2040, 1, 1), (2041, 1, 1)));
    let v = validate(&i, HOLDER_NIF, i.not_before_ms - DAY_MS);
    assert_eq!(v.status, ValidityStatus::NotYetValid);
    assert_eq!(v.into_result().unwrap_err().code(), "certificate_expired");
}

#[test]
fn sin_client_auth() {
    let spec = Spec::persona_fisica().eku(Some(vec![ExtendedKeyUsagePurpose::EmailProtection]));
    let i = info(&spec);
    assert!(!i.client_auth);
    let v = validate(&i, HOLDER_NIF, mid(&i));
    assert_eq!(v.status, ValidityStatus::NoClientAuth);
    assert_eq!(v.into_result().unwrap_err(), CertError::Rejected);
}

#[test]
fn sin_extension_eku_vale_para_cualquier_uso() {
    let i = info(&Spec::persona_fisica().eku(None));
    assert!(i.client_auth);
    assert_eq!(
        validate(&i, HOLDER_NIF, mid(&i)).status,
        ValidityStatus::Valid
    );
}

#[test]
fn eku_con_client_auth_y_otros_usos() {
    let spec = Spec::persona_fisica().eku(Some(vec![
        ExtendedKeyUsagePurpose::EmailProtection,
        ExtendedKeyUsagePurpose::ClientAuth,
    ]));
    assert!(info(&spec).client_auth);
}

#[test]
fn rsa_1024_rechazada() {
    let i = info(&Spec::persona_fisica().key(Key::Rsa(1024)));
    assert_eq!(i.key, nestadmin_certs::KeyInfo::Rsa { bits: 1024 });
    let v = validate(&i, HOLDER_NIF, mid(&i));
    assert_eq!(v.status, ValidityStatus::KeyUnsupported);
    assert_eq!(v.into_result().unwrap_err(), CertError::KeyUnsupported);
}

#[test]
fn rsa_2048_y_p256_admitidas() {
    let rsa = info(&Spec::persona_fisica().key(Key::Rsa(2048)));
    assert_eq!(rsa.key, nestadmin_certs::KeyInfo::Rsa { bits: 2048 });
    assert_eq!(
        validate(&rsa, HOLDER_NIF, mid(&rsa)).status,
        ValidityStatus::Valid
    );
    let ec = info(&Spec::persona_fisica().key(Key::P256));
    assert_eq!(
        validate(&ec, HOLDER_NIF, mid(&ec)).status,
        ValidityStatus::Valid
    );
}

#[test]
fn p384_admitida() {
    let i = info(&Spec::persona_fisica().key(Key::P384));
    assert_eq!(i.key, nestadmin_certs::KeyInfo::Ec { curve: "P-384" });
    assert_eq!(
        validate(&i, HOLDER_NIF, mid(&i)).status,
        ValidityStatus::Valid
    );
}

#[test]
fn nif_del_titular_del_representado_y_distinto() {
    let i = info(&Spec::representante());
    let now = mid(&i);
    assert_eq!(validate(&i, HOLDER_NIF, now).nif_match, NifMatch::Holder);
    assert_eq!(
        validate(&i, ENTITY_CIF, now).nif_match,
        NifMatch::Represented
    );
    assert_eq!(
        validate(&i, " b-1234 5674 ", now).nif_match,
        NifMatch::Represented
    );
    let other = validate(&i, OTHER_NIF, now);
    assert_eq!(other.nif_match, NifMatch::None);
    // No coincidir no bloquea (D283).
    assert_eq!(other.status, ValidityStatus::Valid);
    assert_eq!(validate(&i, "", now).nif_match, NifMatch::None);
}

#[test]
fn umbrales_de_caducidad() {
    let i = info(&Spec::persona_fisica());
    let at = |days: i64| validate(&i, HOLDER_NIF, i.not_after_ms - days * DAY_MS + 1);
    assert_eq!(at(90).expiry_warning, None);
    assert_eq!(at(61).expiry_warning, None);
    assert_eq!(at(60).expiry_warning, Some(ExpiryWarning::Days60));
    assert_eq!(at(59).expiry_warning, Some(ExpiryWarning::Days60));
    assert_eq!(at(31).expiry_warning, Some(ExpiryWarning::Days60));
    assert_eq!(at(30).expiry_warning, Some(ExpiryWarning::Days30));
    assert_eq!(at(29).expiry_warning, Some(ExpiryWarning::Days30));
    assert_eq!(at(8).expiry_warning, Some(ExpiryWarning::Days30));
    assert_eq!(at(7).expiry_warning, Some(ExpiryWarning::Days7));
    assert_eq!(at(6).expiry_warning, Some(ExpiryWarning::Days7));
    assert_eq!(at(1).expiry_warning, Some(ExpiryWarning::Days7));
    assert_eq!(at(59).days_remaining, 58);
}

#[test]
fn sello_se_indica() {
    let i = info(&Spec::sello());
    assert!(validate(&i, ENTITY_CIF, mid(&i)).is_seal);
    let p = info(&Spec::persona_fisica());
    assert!(!validate(&p, HOLDER_NIF, mid(&p)).is_seal);
}
