//! Preferencias por remitente (D161) en el núcleo: claves normalizadas, historial en la misma
//! transacción y sin contenido de correos. Datos sintéticos.

use nestadmin_core::mail_signals::PreferenceRoute;
use nestadmin_core::{normalize_key, PreferenceScope, Store};
use serde_json::Value;

#[test]
fn keys_are_normalized_to_lowercase_and_extracted_from_a_display_name() {
    let address = |s: &str| normalize_key(PreferenceScope::Address, s);
    let domain = |s: &str| normalize_key(PreferenceScope::Domain, s);
    assert_eq!(
        address("Ana <ANA@Alfa.Test>").as_deref(),
        Some("ana@alfa.test")
    );
    assert_eq!(
        address("  \"Ana, S.A.\" <Ana+Tag@Alfa.test> ").as_deref(),
        Some("ana+tag@alfa.test")
    );
    assert_eq!(address("ana@alfa.test").as_deref(), Some("ana@alfa.test"));
    for bad in [
        "",
        "sin-arroba",
        "@alfa.test",
        "ana@",
        "ana@sinpunto",
        "a b@alfa.test",
        "ana@.alfa.test",
    ] {
        assert_eq!(address(bad), None, "{bad:?}");
    }
    assert_eq!(domain("Alfa.TEST").as_deref(), Some("alfa.test"));
    assert_eq!(domain("@alfa.test").as_deref(), Some("alfa.test"));
    assert_eq!(domain("Ana <ana@ALFA.test>").as_deref(), Some("alfa.test"));
    assert_eq!(domain("ana@alfa.test").as_deref(), Some("alfa.test"));
    for bad in ["", "sinpunto", "alfa test.com", "alfa..", "@"] {
        assert_eq!(domain(bad), None, "{bad:?}");
    }
}

#[test]
fn setting_changing_and_removing_leave_one_event_each_without_content() {
    let mut store = Store::open_in_memory().unwrap();
    let key = "ana@alfa.test";
    assert_eq!(
        store
            .set_sender_preference(PreferenceScope::Address, key, PreferenceRoute::Informative)
            .unwrap(),
        None
    );
    // Repetir lo mismo no cambia nada ni escribe otro evento.
    assert_eq!(
        store
            .set_sender_preference(PreferenceScope::Address, key, PreferenceRoute::Informative)
            .unwrap(),
        Some(PreferenceRoute::Informative)
    );
    assert_eq!(
        store
            .set_sender_preference(PreferenceScope::Address, key, PreferenceRoute::Review)
            .unwrap(),
        Some(PreferenceRoute::Informative)
    );
    store
        .set_sender_preference(
            PreferenceScope::Domain,
            "alfa.test",
            PreferenceRoute::Informative,
        )
        .unwrap();
    let listed = store.sender_preferences().unwrap();
    assert_eq!(
        listed
            .iter()
            .map(|p| (p.scope.as_str(), p.key.as_str(), p.route.as_str()))
            .collect::<Vec<_>>(),
        vec![
            ("address", key, "review"),
            ("domain", "alfa.test", "informative")
        ]
    );
    assert!(store
        .remove_sender_preference(PreferenceScope::Address, key)
        .unwrap());
    assert!(!store
        .remove_sender_preference(PreferenceScope::Address, key)
        .unwrap());

    let log: Vec<_> = store
        .audit_log()
        .unwrap()
        .into_iter()
        .filter(|e| e.action == "sender_preference")
        .collect();
    assert_eq!(
        log.iter().map(|e| e.status.as_str()).collect::<Vec<_>>(),
        ["set", "set", "set", "removed"]
    );
    let first: Value = serde_json::from_str(log[0].detail.as_deref().unwrap()).unwrap();
    assert_eq!(first["scope"], "address");
    assert_eq!(first["route"], "informative");
    assert_eq!(first["previous"], Value::Null);
    let second: Value = serde_json::from_str(log[1].detail.as_deref().unwrap()).unwrap();
    assert_eq!(second["previous"], "informative");
    for entry in &log {
        assert_eq!(entry.task_id, None);
        let fields = serde_json::from_str::<Value>(entry.detail.as_deref().unwrap()).unwrap();
        let names: Vec<&String> = fields.as_object().unwrap().keys().collect();
        assert!(
            names
                .iter()
                .all(|n| ["scope", "key", "route", "previous"].contains(&n.as_str())),
            "{names:?}"
        );
    }
}

#[test]
fn a_failed_event_leaves_no_preference_behind() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nestadmin.db");
    let mut store = Store::open(&path).unwrap();
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch(
        "CREATE TRIGGER cut BEFORE INSERT ON audit_log WHEN NEW.action = 'sender_preference'
         BEGIN SELECT RAISE(ABORT, 'corte simulado'); END;",
    )
    .unwrap();
    assert!(store
        .set_sender_preference(
            PreferenceScope::Domain,
            "alfa.test",
            PreferenceRoute::Review
        )
        .is_err());
    assert!(store.sender_preferences().unwrap().is_empty());
}
