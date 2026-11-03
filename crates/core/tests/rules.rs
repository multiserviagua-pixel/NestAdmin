//! Reglas versionadas (R07): creación inactiva, versiones nuevas en cada cambio,
//! historial conservado y persistencia tras reabrir.

use nestadmin_core::{CoreError, Store};
use serde_json::json;

#[test]
fn rules_are_created_inactive_and_every_change_is_a_new_version() {
    let mut store = Store::open_in_memory().unwrap();
    let rule = store
        .create_rule(
            "proveedor-alfa",
            "register_invoice",
            &json!({ "supplier": "Alfa SL" }),
        )
        .unwrap();
    assert_eq!((rule.version, rule.active), (1, false));

    assert_eq!(
        store
            .set_rule_active("proveedor-alfa", true)
            .unwrap()
            .version,
        2
    );
    // Activar lo ya activo no crea versión.
    assert_eq!(
        store
            .set_rule_active("proveedor-alfa", true)
            .unwrap()
            .version,
        2
    );
    let updated = store
        .update_rule(
            "proveedor-alfa",
            &json!({ "supplier": "Alfa SL", "max_total": "500.00" }),
        )
        .unwrap();
    assert_eq!(
        (updated.version, updated.active),
        (3, true),
        "editar conserva el estado"
    );

    let history = store.rule_history("proveedor-alfa").unwrap();
    assert_eq!(
        history.iter().map(|r| r.version).collect::<Vec<_>>(),
        [1, 2, 3]
    );
    assert_eq!(history[0].conditions, json!({ "supplier": "Alfa SL" }));
    assert_eq!(store.rules().unwrap(), vec![updated]);

    let log: Vec<_> = store
        .audit_log()
        .unwrap()
        .into_iter()
        .map(|e| (e.status, e.detail))
        .collect();
    assert_eq!(log.len(), 3, "cada versión queda en el historial visible");
    assert_eq!(
        log[1].1.as_deref(),
        Some("proveedor-alfa v2 (register_invoice)")
    );
}

#[test]
fn duplicate_or_missing_rules_are_errors() {
    let mut store = Store::open_in_memory().unwrap();
    store.create_rule("r", "send_draft", &json!({})).unwrap();
    assert!(matches!(
        store.create_rule("r", "send_draft", &json!({})),
        Err(CoreError::IdempotencyConflict(_))
    ));
    assert!(matches!(
        store.set_rule_active("x", true),
        Err(CoreError::RuleNotFound(_))
    ));
    assert!(store.rule("x").unwrap().is_none());
}

#[test]
fn rules_survive_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nestadmin.db");
    {
        let mut store = Store::open(&path).unwrap();
        store
            .create_rule(
                "r",
                "send_draft",
                &json!({ "category": "solicitud_factura" }),
            )
            .unwrap();
        store.set_rule_active("r", true).unwrap();
    }
    let store = Store::open(&path).unwrap();
    assert_eq!(store.schema_version().unwrap(), 2);
    let rule = store.rule("r").unwrap().unwrap();
    assert_eq!((rule.version, rule.active), (2, true));
}
