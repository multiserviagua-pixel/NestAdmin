mod common;

use common::*;
use nestadmin_certs::{inspect, CertError};

const ALL: [CertError; 6] = [
    CertError::Missing,
    CertError::Expired,
    CertError::NeedsUser,
    CertError::KeyUnsupported,
    CertError::Rejected,
    CertError::Invalid,
];

#[test]
fn codigos_estables() {
    let codes: Vec<_> = ALL.iter().map(CertError::code).collect();
    assert_eq!(
        codes,
        [
            "certificate_missing",
            "certificate_expired",
            "certificate_needs_user",
            "certificate_key_unsupported",
            "certificate_rejected",
            "certificate_invalid",
        ]
    );
}

#[test]
fn display_y_debug_no_llevan_nif_ni_nombre() {
    let secrets = [HOLDER_NIF, ENTITY_CIF, OTHER_NIF, HOLDER_NAME, ISSUER_NAME];
    for e in ALL {
        let text = format!("{e} {e:?}");
        for s in secrets {
            assert!(!text.contains(s), "{} filtra {s}", e.code());
        }
    }
    // El error de un DER corrupto que sí contiene un NIF tampoco lo repite.
    let mut der = make(&Spec::persona_fisica());
    der.truncate(der.len() - 5);
    let err = inspect(&der).unwrap_err();
    let text = format!("{err} {err:?}");
    for s in secrets {
        assert!(!text.contains(s));
    }
}
