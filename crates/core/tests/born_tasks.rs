//! Tareas que nacen ya completadas o en revisión, y «Redactar respuesta» (D157, D159). Datos
//! sintéticos.

use nestadmin_core::{CoreError, Store, TaskState};
use serde_json::{json, Value};

fn result() -> Value {
    json!({ "type": "email", "version": 1, "triage": { "route": "informativo", "codes": ["noreply_address"], "source": "local" } })
}

#[test]
fn enqueue_closed_creates_a_completed_task_and_its_first_transition_atomically() {
    let mut store = Store::open_in_memory().unwrap();
    let task = store
        .enqueue_closed("email", r#"{"message_id":"M"}"#, &result(), "informativo")
        .unwrap();
    assert_eq!(task.state, TaskState::Completed);
    assert_eq!(task.attempts, 0);
    assert_eq!(
        task.result.as_ref().unwrap()["triage"]["route"],
        "informativo"
    );
    assert_eq!(
        store.transitions(task.id).unwrap(),
        vec![(None, "completed".to_owned(), "informativo".to_owned())]
    );
    // Nunca pasa por la cola: nadie puede tomarla.
    assert!(store.claim_next().unwrap().is_none());
    // Es final: ninguna transición la reabre.
    assert!(matches!(
        store.retry(task.id),
        Err(CoreError::InvalidTransition { .. })
    ));
    assert!(matches!(
        store.cancel(task.id),
        Err(CoreError::InvalidTransition { .. })
    ));
}

#[test]
fn a_failure_while_closing_leaves_neither_a_task_nor_a_transition() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nestadmin.db");
    let mut store = Store::open(&path).unwrap();
    // Un corte entre la tarea y su transición: la transición falla y todo se deshace.
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute_batch(
            "CREATE TRIGGER cut BEFORE INSERT ON task_transitions
             BEGIN SELECT RAISE(ABORT, 'corte simulado'); END;",
        )
        .unwrap();
    assert!(store
        .enqueue_closed("email", "{}", &result(), "informativo")
        .is_err());
    assert!(store
        .enqueue_review("email", "{}", &result(), "rebote")
        .is_err());
    assert!(store.all_tasks().unwrap().is_empty());
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute_batch("DROP TRIGGER cut;")
        .unwrap();
    let task = store
        .enqueue_closed("email", "{}", &result(), "informativo")
        .unwrap();
    assert_eq!(store.all_tasks().unwrap().len(), 1);
    assert_eq!(store.transitions(task.id).unwrap().len(), 1);
}

#[test]
fn enqueue_review_creates_a_task_awaiting_review_that_can_still_be_completed() {
    let mut store = Store::open_in_memory().unwrap();
    let task = store
        .enqueue_review("email", "{}", &json!({ "version": 1 }), "rebote")
        .unwrap();
    assert_eq!(task.state, TaskState::AwaitingReview);
    assert!(store.claim_next().unwrap().is_none());
    assert_eq!(
        store
            .complete(task.id, &json!({ "version": 1 }))
            .unwrap()
            .state,
        TaskState::Completed
    );
}

#[test]
fn a_redraft_is_created_once_archives_its_source_and_a_cancelled_one_frees_the_slot() {
    let mut store = Store::open_in_memory().unwrap();
    let source = store
        .enqueue_review(
            "email",
            r#"{"message_id":"M"}"#,
            &json!({ "version": 1 }),
            "rebote",
        )
        .unwrap();
    let input = format!(r#"{{"message_id":"M","redraft_of":{}}}"#, source.id);
    let archived = json!({ "version": 1, "archived": true });
    let (first, created) = store
        .enqueue_redraft(source.id, &input, Some(&archived))
        .unwrap();
    assert!(created);
    assert_eq!(first.state, TaskState::Queued);
    assert_eq!(store.task(source.id).unwrap().state, TaskState::Completed);
    // Una segunda petición devuelve la misma tarea y no toca nada.
    let (again, created) = store.enqueue_redraft(source.id, &input, None).unwrap();
    assert!(!created);
    assert_eq!(again.id, first.id);
    assert_eq!(store.all_tasks().unwrap().len(), 2);
    // Los eventos de historial no llevan contenido.
    let log = store.audit_log().unwrap();
    assert!(log
        .iter()
        .any(|e| e.action == "archive" && e.task_id == Some(source.id)));
    // Cancelada la de redacción, se puede pedir otra.
    store.cancel(first.id).unwrap();
    let (second, created) = store.enqueue_redraft(source.id, &input, None).unwrap();
    assert!(created && second.id != first.id);
}

#[test]
fn a_redraft_whose_source_is_not_in_review_changes_nothing() {
    let mut store = Store::open_in_memory().unwrap();
    let source = store
        .enqueue_closed("email", "{}", &json!({ "version": 1 }), "informativo")
        .unwrap();
    // Archivar exige revisión: desde `completed` falla y no deja la tarea nueva.
    let error = store
        .enqueue_redraft(source.id, r#"{"redraft_of":1}"#, Some(&json!({})))
        .unwrap_err();
    assert!(matches!(error, CoreError::InvalidTransition { .. }));
    assert_eq!(store.all_tasks().unwrap().len(), 1);
}

#[test]
fn a_redraft_that_collides_with_the_unique_index_leaves_no_partial_effects() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nestadmin.db");
    let mut store = Store::open(&path).unwrap();
    let source = store
        .enqueue_review("email", "{}", &json!({ "version": 1 }), "rebote")
        .unwrap();
    // Una inserción duplicada dentro de la misma operación (lo que haría una carrera entre dos
    // conexiones): el índice único la rechaza y todo se deshace, también el archivado del origen.
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute_batch(
            "CREATE TRIGGER race BEFORE INSERT ON tasks WHEN NEW.input_ref LIKE '%redraft_of%'
             BEGIN
                 INSERT INTO tasks (kind, input_ref, state, created_at, updated_at)
                 VALUES ('email', NEW.input_ref, 'queued', 1, 1);
             END;",
        )
        .unwrap();
    let input = format!(r#"{{"redraft_of":{}}}"#, source.id);
    let archived = json!({ "version": 1, "archived": true });
    assert!(store
        .enqueue_redraft(source.id, &input, Some(&archived))
        .is_err());
    assert_eq!(
        store.task(source.id).unwrap().state,
        TaskState::AwaitingReview,
        "el archivado se deshizo con la inserción"
    );
    assert_eq!(store.all_tasks().unwrap().len(), 1);
    assert!(store.audit_log().unwrap().is_empty());
}
