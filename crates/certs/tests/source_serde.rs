//! B2: `CertificateSource` se serializa para la base (multiplataforma).

use nestadmin_certs::CertificateSource;

#[test]
fn store_and_pfx_round_trip_as_json() {
    let store = CertificateSource::Store {
        thumbprint_sha256: "ab".repeat(32),
    };
    let json = serde_json::to_string(&store).unwrap();
    assert_eq!(
        json,
        format!(
            r#"{{"kind":"store","thumbprint_sha256":"{}"}}"#,
            "ab".repeat(32)
        )
    );
    assert_eq!(
        serde_json::from_str::<CertificateSource>(&json).unwrap(),
        store
    );

    let pfx = CertificateSource::Pfx {
        reference: "0123456789abcdef0123456789abcdef".into(),
    };
    let json = serde_json::to_string(&pfx).unwrap();
    assert_eq!(
        json,
        r#"{"kind":"pfx","reference":"0123456789abcdef0123456789abcdef"}"#
    );
    assert_eq!(
        serde_json::from_str::<CertificateSource>(&json).unwrap(),
        pfx
    );
}

#[test]
fn unknown_kinds_are_refused() {
    assert!(serde_json::from_str::<CertificateSource>(r#"{"kind":"otro"}"#).is_err());
    assert!(serde_json::from_str::<CertificateSource>(r#"{"kind":"pfx"}"#).is_err());
}
