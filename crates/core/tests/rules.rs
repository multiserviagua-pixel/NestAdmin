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
    assert_eq!(
        store.schema_version().unwrap(),
        nestadmin_core::store::SCHEMA_VERSION
    );
    let rule = store.rule("r").unwrap().unwrap();
    assert_eq!((rule.version, rule.active), (2, true));
}

// ---- Panel de reglas (D124–D129) ----

use nestadmin_core::actions::ConnectorError;
use nestadmin_core::{ActionRequest, Authorization, Connector, EvaluationRecord, SignalRecord};
use serde_json::Value;

struct Accepts;

impl Connector for Accepts {
    fn lookup(&self, _key: &str) -> Result<Option<Value>, ConnectorError> {
        Ok(None)
    }
    fn execute(&self, key: &str, _payload: &Value) -> Result<Value, ConnectorError> {
        Ok(json!({ "key": key }))
    }
}

#[test]
fn names_archive_and_the_advanced_mark_live_in_rule_meta() {
    let mut store = Store::open_in_memory().unwrap();
    let rule = store
        .create_rule_named(
            "envio",
            "send_draft",
            &json!({ "category": "otros" }),
            Some("Respuestas"),
            true,
        )
        .unwrap();
    let meta = store.rule_meta("envio").unwrap().unwrap();
    assert_eq!(meta.name.as_deref(), Some("Respuestas"));
    assert!(meta.allow_any_sender_ack_at.is_some() && meta.archived_at.is_none());
    assert!(matches!(
        store.create_rule_named("envio", "send_draft", &json!({}), None, false),
        Err(CoreError::IdempotencyConflict(_))
    ));

    // Editar las condiciones borra la marca y lo deja en el historial; el nombre se queda.
    store.set_rule_active("envio", true).unwrap();
    let edited = store
        .update_rule("envio", &json!({ "category": "documentacion" }))
        .unwrap();
    assert!(edited.version > rule.version);
    let meta = store.rule_meta("envio").unwrap().unwrap();
    assert!(meta.allow_any_sender_ack_at.is_none());
    assert_eq!(meta.name.as_deref(), Some("Respuestas"));
    let log = store.audit_log().unwrap();
    assert!(log
        .iter()
        .any(|e| e.action == "rule_any_sender" && e.status == "acknowledged"));
    assert!(log
        .iter()
        .any(|e| e.action == "rule_any_sender" && e.status == "revoked"));

    // Archivar: versión inactiva, marca, y ni se activa ni se edita; no hay borrado físico.
    let archived = store.archive_rule("envio").unwrap();
    assert!(!archived.active);
    assert!(store
        .rule_meta("envio")
        .unwrap()
        .unwrap()
        .archived_at
        .is_some());
    assert!(matches!(
        store.set_rule_active("envio", true),
        Err(CoreError::RuleArchived(_))
    ));
    assert!(matches!(
        store.update_rule("envio", &json!({ "category": "otros" })),
        Err(CoreError::RuleArchived(_))
    ));
    // Desactivar una archivada sigue siendo posible (ya lo está), y archivar es idempotente.
    assert!(!store.set_rule_active("envio", false).unwrap().active);
    let versions = store.rule_history("envio").unwrap().len();
    store.archive_rule("envio").unwrap();
    assert_eq!(store.rule_history("envio").unwrap().len(), versions);
    let back = store.unarchive_rule("envio").unwrap();
    assert!(!back.active);
    assert!(store
        .rule_meta("envio")
        .unwrap()
        .unwrap()
        .archived_at
        .is_none());
    assert!(store.set_rule_active("envio", true).unwrap().active);
}

#[test]
fn evaluations_and_signals_ignore_repeats() {
    let mut store = Store::open_in_memory().unwrap();
    let task = store.enqueue("invoice", "{}").unwrap();
    let checks = json!({ "candidates": [] });
    let record = |fp: &'static str, version: i64| EvaluationRecord {
        task_id: task.id,
        content_version: version,
        action_kind: "register_invoice",
        outcome: "review",
        reason_code: "no_active_rule_matches",
        rules_fp: fp,
        checks: &checks,
    };
    assert!(store.record_evaluation(&record("a", 1)).unwrap());
    assert!(!store.record_evaluation(&record("a", 1)).unwrap());
    // Otras reglas o otra versión del contenido sí son otra evaluación.
    assert!(store.record_evaluation(&record("b", 1)).unwrap());
    assert!(store.record_evaluation(&record("a", 2)).unwrap());
    assert_eq!(store.rule_evaluations().unwrap().len(), 3);

    let signal = |kind: &'static str| SignalRecord {
        kind,
        action_kind: "register_invoice",
        subject_key: "alfa sl",
        edit_state: "unedited",
        task_id: task.id,
        content_version: 1,
        source_key: Some("abc"),
        state: "valid",
    };
    assert!(store.record_signal(&signal("approved")).unwrap());
    assert!(!store.record_signal(&signal("approved")).unwrap());
    assert!(store.record_signal(&signal("rejected")).unwrap());
    assert_eq!(store.rule_signals().unwrap().len(), 2);
}

/// Un corte entre la evidencia y el intento deja una evaluación obsoleta: al reprocesar con la
/// misma clave se actualiza mientras la tarea no tenga acción, y después ya no.
#[test]
fn a_stale_evaluation_is_updated_until_the_task_has_an_action() {
    let mut store = Store::open_in_memory().unwrap();
    let task = store.enqueue("invoice", "{}").unwrap();
    let checks = |matches: bool| json!({ "candidates": [{ "id": "a", "version": 1, "active": true, "matches": matches }] });
    let first = checks(true);
    let second = checks(false);
    fn make<'a>(
        task_id: i64,
        outcome: &'a str,
        reason: &'a str,
        checks: &'a serde_json::Value,
    ) -> EvaluationRecord<'a> {
        EvaluationRecord {
            task_id,
            content_version: 1,
            action_kind: "register_invoice",
            outcome,
            reason_code: reason,
            rules_fp: "fp",
            checks,
        }
    }
    assert!(store
        .record_evaluation(&make(task.id, "automated", "automated", &first))
        .unwrap());
    // Lo mismo otra vez no cambia nada.
    assert!(!store
        .record_evaluation(&make(task.id, "automated", "automated", &first))
        .unwrap());
    // El reproceso da otro resultado y la tarea no tiene acción: se actualiza, sin duplicar.
    assert!(store
        .record_evaluation(&make(task.id, "review", "no_active_rule_matches", &second))
        .unwrap());
    let rows = store.rule_evaluations().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        (rows[0].outcome.as_str(), rows[0].reason_code.as_str()),
        ("review", "no_active_rule_matches")
    );
    assert_eq!(rows[0].checks, second);

    // Con una acción registrada, la evaluación guardada autorizó algo y no se toca.
    store
        .perform_action(
            &ActionRequest {
                task_id: task.id,
                kind: "register_invoice".into(),
                version: 1,
                payload: json!({}),
                authorization: Authorization::Approval,
            },
            &Accepts,
        )
        .unwrap();
    assert!(!store
        .record_evaluation(&make(task.id, "automated", "automated", &first))
        .unwrap());
    assert_eq!(store.rule_evaluations().unwrap()[0].outcome, "review");
}

#[test]
fn actions_keep_the_rule_that_authorized_them_and_stats_count_each_once() {
    let mut store = Store::open_in_memory().unwrap();
    for id in ["alfa", "beta"] {
        store
            .create_rule(id, "register_invoice", &json!({ "supplier": id }))
            .unwrap();
    }
    let by_rule = |store: &mut Store, rule: &str, version: i64| {
        let task = store.enqueue("invoice", "{}").unwrap();
        let request = ActionRequest {
            task_id: task.id,
            kind: "register_invoice".into(),
            version: 1,
            payload: json!({ "n": task.id }),
            authorization: Authorization::Rule {
                rule_id: rule.into(),
                rule_version: version,
            },
        };
        store.perform_action(&request, &Accepts).unwrap();
        // Repetir la misma acción no cuenta otra vez.
        store.perform_action(&request, &Accepts).unwrap();
        task.id
    };
    by_rule(&mut store, "alfa", 1);
    let second = by_rule(&mut store, "alfa", 2);
    let person = store.enqueue("invoice", "{}").unwrap();
    store
        .perform_action(
            &ActionRequest {
                task_id: person.id,
                kind: "register_invoice".into(),
                version: 1,
                payload: json!({}),
                authorization: Authorization::Approval,
            },
            &Accepts,
        )
        .unwrap();

    let stats = store.rule_stats().unwrap();
    let alfa = stats.iter().find(|s| s.rule_id == "alfa").unwrap();
    let beta = stats.iter().find(|s| s.rule_id == "beta").unwrap();
    assert_eq!((alfa.applied, alfa.unclassified, alfa.reviewed), (2, 0, 0));
    assert!(alfa.last_applied_at.is_some());
    assert_eq!((beta.applied, beta.last_applied_at), (0, None));
    assert_eq!(store.task_actions(second).unwrap().len(), 1);

    // Una evaluación que fue a revisión con «beta» coincidiendo suma «pasó a revisión»,
    // una sola vez aunque conste con dos huellas.
    let checks = json!({ "candidates": [
        { "id": "beta", "version": 1, "active": true, "matches": true },
        { "id": "alfa", "version": 2, "active": true, "matches": false },
    ] });
    for fp in ["a", "b"] {
        store
            .record_evaluation(&EvaluationRecord {
                task_id: person.id,
                content_version: 1,
                action_kind: "register_invoice",
                outcome: "review",
                reason_code: "invoice_issues",
                rules_fp: fp,
                checks: &checks,
            })
            .unwrap();
    }
    let stats = store.rule_stats().unwrap();
    assert_eq!(
        stats.iter().find(|s| s.rule_id == "beta").unwrap().reviewed,
        1
    );
    assert_eq!(
        stats.iter().find(|s| s.rule_id == "alfa").unwrap().reviewed,
        0
    );
}

#[test]
fn old_actions_are_read_strictly_and_the_rest_is_unclassified() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nestadmin.db");
    let mut store = Store::open(&path).unwrap();
    store
        .create_rule("alfa", "register_invoice", &json!({ "supplier": "Alfa" }))
        .unwrap();
    let task = store.enqueue("invoice", "{}").unwrap();
    drop(store);
    // Filas como las dejaba la versión anterior: sin `rule_id` y con la autorización en texto.
    let conn = rusqlite::Connection::open(&path).unwrap();
    for (key, authorization) in [
        ("k1", "regla alfa v3"),
        ("k2", "regla alfa v"),
        ("k3", "regla alfa vX"),
        ("k4", "aprobación"),
        ("k5", "regla inexistente v1"),
        // Etiqueta de regla, pero el historial dice que ejecutó una persona al aprobar.
        ("k6", "regla alfa v1"),
        // Etiqueta de regla y el historial solo dice «reconciled» con «aprobación»: no se
        // distingue quién lo ejecutó.
        ("k7", "regla alfa v1"),
        // Etiqueta de regla sin ninguna entrada en el historial: no se distingue.
        ("k8", "regla alfa v1"),
    ] {
        conn.execute(
            "INSERT INTO actions (idempotency_key, task_id, kind, version, payload, authorization, status, created_at, updated_at)
             VALUES (?1, ?2, 'register_invoice', 1, '{}', ?3, 'confirmed', 5, 7)",
            rusqlite::params![key, task.id, authorization],
        )
        .unwrap();
    }
    for (key, status, authorization) in [
        ("k1", "confirmed", "regla alfa v3"),
        ("k6", "confirmed", "aprobación"),
        ("k7", "reconciled", "aprobación"),
    ] {
        conn.execute(
            "INSERT INTO audit_log (at, task_id, action, status, authorization, detail)
             VALUES (1, ?1, 'register_invoice', ?2, ?3, ?4)",
            rusqlite::params![task.id, status, authorization, key],
        )
        .unwrap();
    }
    drop(conn);
    let store = Store::open(&path).unwrap();
    let stats = store.rule_stats().unwrap();
    assert_eq!(stats.len(), 1);
    assert_eq!(
        (
            stats[0].applied,
            stats[0].last_applied_at,
            stats[0].unclassified
        ),
        (1, Some(7), 4)
    );
}

// ---- Evidencia aprendida (D171, D172) ----

#[test]
fn learned_evidence_is_saved_with_the_result_once_and_saving_again_never_duplicates_it() {
    use nestadmin_core::{LearnedEvidence, LearnedSignal};
    let mut store = Store::open_in_memory().unwrap();
    let evidence = LearnedEvidence {
        signals: vec![
            LearnedSignal {
                subject_key: "solicitud_factura|ana@alfa.test".into(),
                source_key: "learned|account:a|m1".into(),
            },
            LearnedSignal {
                subject_key: "solicitud_factura|ana@alfa.test".into(),
                source_key: "learned|account:a|m2".into(),
            },
        ],
        reply_style: vec![
            ("solicitud_factura".into(), "confirm".into()),
            ("solicitud_factura".into(), "confirm".into()),
            ("otros".into(), "inform".into()),
        ],
    };
    let mark = json!({ "phase": "done" });
    let result = json!({ "pairs": [] });
    for _ in 0..3 {
        store
            .save_learning("account:a", &mark, Some(&result), None, Some(&evidence))
            .unwrap();
    }
    let signals = store.rule_signals().unwrap();
    assert_eq!(signals.len(), 2);
    assert!(signals
        .iter()
        .all(|s| s.kind == "learned" && s.task_id.is_none() && s.state == "valid"));
    assert_eq!(
        store.reply_style_rows().unwrap(),
        vec![
            ("otros".to_owned(), "inform".to_owned(), 1),
            ("solicitud_factura".to_owned(), "confirm".to_owned(), 2),
        ],
        "los recuentos solo se suman la primera vez que se guarda el resultado"
    );
    // Sin evidencia (por ejemplo, un cambio de marca) no se toca nada.
    store
        .save_learning("account:a", &mark, None, None, None)
        .unwrap();
    assert_eq!(store.rule_signals().unwrap().len(), 2);
}
