//! Conversaciones del asistente y lecturas de sus herramientas (D182, D184, D188). Datos
//! sintéticos.

use nestadmin_core::store::{backup_path, SCHEMA_VERSION};
use nestadmin_core::{
    AggregateQuery, CoreError, NewCall, SearchDoc, SearchKind, SearchStatus, Store, TaskState,
};
use serde_json::json;

fn doc(
    task_id: i64,
    kind: SearchKind,
    party: &str,
    total: Option<i64>,
    currency: &str,
    date: &str,
) -> SearchDoc {
    SearchDoc {
        task_id,
        kind,
        status: SearchStatus::Indexed,
        title: format!("Documento {task_id}"),
        party: party.into(),
        number: format!("N-{task_id}"),
        doc_date: Some(date.into()),
        total_minor: total,
        currency: currency.into(),
        body: "texto".into(),
        detail: String::new(),
    }
}

/// Crea una tarea terminada con su fila de índice.
fn add(
    store: &mut Store,
    kind: SearchKind,
    party: &str,
    total: Option<i64>,
    currency: &str,
    date: &str,
) -> i64 {
    let id = store.enqueue("invoice", "x").unwrap().id;
    store.claim(id).unwrap();
    store
        .complete_indexed(id, &json!({}), &doc(id, kind, party, total, currency, date))
        .unwrap();
    id
}

fn call(step: u32, local: bool, text: &str) -> NewCall {
    NewCall {
        step,
        provider: if local { "ollama" } else { "ollama_cloud" }.into(),
        model: "m".into(),
        host: if local {
            "".into()
        } else {
            "api.ejemplo.test".into()
        },
        local,
        sent_text: text.into(),
    }
}

fn count(path: &std::path::Path, table: &str) -> i64 {
    rusqlite::Connection::open(path)
        .unwrap()
        .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}

#[test]
fn migration_11_applies_on_a_new_database_and_on_a_v10_one() {
    assert_eq!(SCHEMA_VERSION, 14);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("n.db");
    let mut store = Store::open(&path).unwrap();
    assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
    add(
        &mut store,
        SearchKind::Invoice,
        "Alfa SL",
        Some(100),
        "EUR",
        "2026-10-01",
    );
    drop(store);
    for table in [
        "assistant_conversations",
        "assistant_turns",
        "assistant_calls",
    ] {
        assert_eq!(count(&path, table), 0, "{table}");
    }

    // Una base v10: sin las tablas del asistente y con datos previos.
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch(
        "DROP TABLE company_letterheads; ALTER TABLE company DROP COLUMN letterhead_sha256; ALTER TABLE company DROP COLUMN letterhead_top_mm; ALTER TABLE company DROP COLUMN letterhead_right_mm; ALTER TABLE company DROP COLUMN letterhead_bottom_mm; ALTER TABLE company DROP COLUMN letterhead_left_mm; DROP TABLE installation; DROP TABLE verifactu_records; DROP TABLE assistant_calls; DROP TABLE assistant_turns; DROP TABLE assistant_conversations;
         PRAGMA user_version = 10;",
    )
    .unwrap();
    drop(conn);
    let store = Store::open(&path).unwrap();
    assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
    assert_eq!(
        store.search_hits(&[1]).unwrap().len(),
        1,
        "los datos previos se conservan"
    );
    drop(store);
    assert_eq!(count(&path, "assistant_turns"), 0);
    let backup = backup_path(&path, 10);
    assert!(backup.exists(), "copia previa a migrar");
    assert_eq!(
        rusqlite::Connection::open(&backup)
            .unwrap()
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE name = 'assistant_turns'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
}

#[test]
fn aggregate_counts_and_sums_per_currency_and_reports_what_is_missing() {
    let mut store = Store::open_in_memory().unwrap();
    add(
        &mut store,
        SearchKind::Invoice,
        "Alfa SL",
        Some(12_100),
        "EUR",
        "2026-10-01",
    );
    add(
        &mut store,
        SearchKind::Invoice,
        "Alfa SL",
        Some(5_050),
        "EUR",
        "2026-10-15",
    );
    add(
        &mut store,
        SearchKind::Invoice,
        "Beta SA",
        Some(9_999),
        "USD",
        "2026-10-20",
    );
    add(
        &mut store,
        SearchKind::Invoice,
        "Alfa SL",
        None,
        "",
        "2026-10-21",
    );
    add(
        &mut store,
        SearchKind::Email,
        "ana@x.test",
        None,
        "",
        "2026-10-22",
    );
    // Una factura rechazada por la persona no cuenta.
    let rejected = store.enqueue("invoice", "r").unwrap().id;
    store
        .index_document(&doc(
            rejected,
            SearchKind::Invoice,
            "Alfa SL",
            Some(777),
            "EUR",
            "2026-10-23",
        ))
        .unwrap();
    store.cancel(rejected).unwrap();
    // Una tarea sin fila en el índice (terminada) se declara aparte.
    let pending = store.enqueue("invoice", "y").unwrap().id;
    store.claim(pending).unwrap();
    store.complete(pending, &json!({})).unwrap();

    let all = store.aggregate(&AggregateQuery::default()).unwrap();
    assert_eq!(all.documents, 5);
    assert_eq!(all.with_total, 3);
    assert_eq!(all.without_total, 2);
    assert_eq!(all.excluded_cancelled, 1);
    assert_eq!(all.pending_index, 1);
    let totals: Vec<_> = all
        .by_currency
        .iter()
        .map(|c| (c.currency.as_str(), c.documents, c.total_minor))
        .collect();
    assert_eq!(
        totals,
        [("EUR", 2, 17_150), ("USD", 1, 9_999)],
        "monedas sin mezclar"
    );

    let alfa = store
        .aggregate(&AggregateQuery {
            kind: Some(SearchKind::Invoice),
            party: Some("alfa".into()),
            from: Some("2026-10-10".into()),
            to: Some("2026-10-31".into()),
            ..AggregateQuery::default()
        })
        .unwrap();
    assert_eq!(
        (alfa.documents, alfa.with_total, alfa.without_total),
        (2, 1, 1)
    );
    assert_eq!(alfa.by_currency.len(), 1);
    assert_eq!(alfa.by_currency[0].total_minor, 5_050);

    let none = store
        .aggregate(&AggregateQuery {
            party: Some("inexistente".into()),
            ..AggregateQuery::default()
        })
        .unwrap();
    assert_eq!(
        (none.documents, none.with_total, none.by_currency.len()),
        (0, 0, 0)
    );

    let bad = store.aggregate(&AggregateQuery {
        from: Some("ayer".into()),
        ..AggregateQuery::default()
    });
    assert!(matches!(bad, Err(CoreError::InvalidFilter(_))));
}

#[test]
fn conversations_are_saved_read_and_deleted_with_their_turns_and_calls() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("n.db");
    let mut store = Store::open(&path).unwrap();
    assert!(store.assistant_last_conversation().unwrap().is_none());

    let first = store
        .assistant_save_exchange(
            None,
            "¿Hay facturas?",
            "Hay una.",
            &json!({ "outcome": "answered" }),
            &[call(1, false, "texto 1"), call(2, false, "texto 2")],
        )
        .unwrap();
    let again = store
        .assistant_save_exchange(
            Some(first.conversation_id),
            "¿Y de Beta?",
            "Ninguna.",
            &json!({ "outcome": "answered" }),
            &[call(1, true, "texto 3")],
        )
        .unwrap();
    let other = store
        .assistant_save_exchange(None, "Otra", "Respuesta", &json!({}), &[])
        .unwrap();
    assert_eq!(first.conversation_id, again.conversation_id);
    assert_ne!(first.conversation_id, other.conversation_id);

    let view = store
        .assistant_conversation(first.conversation_id)
        .unwrap()
        .unwrap();
    let roles: Vec<_> = view
        .turns
        .iter()
        .map(|t| (t.role.as_str(), t.text.as_str()))
        .collect();
    assert_eq!(
        roles,
        [
            ("user", "¿Hay facturas?"),
            ("assistant", "Hay una."),
            ("user", "¿Y de Beta?"),
            ("assistant", "Ninguna.")
        ]
    );
    assert_eq!(view.turns[1].meta["outcome"], "answered");
    let recent = store
        .assistant_recent_turns(first.conversation_id, 3)
        .unwrap();
    assert_eq!(recent.len(), 3);
    assert_eq!(
        recent[0].text, "Hay una.",
        "los últimos, del más antiguo al más reciente"
    );
    // La última es la de actividad más reciente.
    assert_eq!(
        store.assistant_last_conversation().unwrap().unwrap().id,
        other.conversation_id
    );

    let sent = store.assistant_sent(first.turn_id).unwrap();
    assert_eq!(sent.len(), 2);
    assert_eq!(
        (sent[0].step, sent[0].sent_text.as_str(), sent[0].local),
        (1, "texto 1", false)
    );
    assert_eq!(sent[0].host, "api.ejemplo.test");
    assert!(store.assistant_sent(again.turn_id).unwrap()[0].local);

    // Una conversación inexistente no recibe turnos y no deja nada.
    let missing =
        store.assistant_save_exchange(Some(999), "x", "y", &json!({}), &[call(1, false, "z")]);
    assert!(matches!(missing, Err(CoreError::ConversationNotFound(999))));
    assert_eq!(count(&path, "assistant_turns"), 6);

    // Borrar una conversación borra sus turnos y llamadas, y solo los suyos.
    assert!(store.assistant_delete(first.conversation_id).unwrap());
    assert!(!store.assistant_delete(first.conversation_id).unwrap());
    assert!(store
        .assistant_conversation(first.conversation_id)
        .unwrap()
        .is_none());
    assert_eq!(count(&path, "assistant_turns"), 2);
    assert_eq!(count(&path, "assistant_calls"), 0);
    assert!(store.assistant_sent(first.turn_id).unwrap().is_empty());

    store
        .assistant_save_exchange(
            Some(other.conversation_id),
            "p",
            "r",
            &json!({}),
            &[call(1, false, "t")],
        )
        .unwrap();
    assert_eq!(count(&path, "assistant_calls"), 1);
    assert_eq!(store.assistant_delete_all().unwrap(), 1);
    for table in [
        "assistant_conversations",
        "assistant_turns",
        "assistant_calls",
    ] {
        assert_eq!(count(&path, table), 0, "{table}");
    }
}

#[test]
fn task_events_never_carry_the_free_detail_and_counts_follow_the_states() {
    let mut store = Store::open_in_memory().unwrap();
    let id = add(
        &mut store,
        SearchKind::Invoice,
        "Alfa SL",
        Some(100),
        "EUR",
        "2026-10-01",
    );
    store
        .record_event(
            id,
            "ai_fallback",
            "used",
            &json!({ "detalle": "SECRETO-CENTINELA" }),
        )
        .unwrap();
    let queued = store.enqueue("email", "z").unwrap().id;

    let events = store.task_events(id, 20).unwrap();
    assert!(events
        .iter()
        .any(|e| e.code == "ai_fallback" && e.status == "used"));
    assert!(events
        .iter()
        .any(|e| e.code == "state" && e.status == "completed"));
    assert!(!format!("{events:?}").contains("SECRETO-CENTINELA"));
    assert_eq!(store.task_events(id, 1).unwrap().len(), 1);

    let counts: Vec<_> = store
        .task_counts()
        .unwrap()
        .into_iter()
        .map(|c| (c.state, c.count))
        .collect();
    assert_eq!(counts, [("completed".into(), 1), ("queued".into(), 1)]);
    let rows = store.recent_task_rows(Some("queued"), 8).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].task_id, queued);
    assert_eq!(rows[0].title, "", "sin fila de índice, sin título");
    assert_eq!(store.recent_task_rows(None, 8).unwrap().len(), 2);
    let _ = TaskState::Queued;
}
