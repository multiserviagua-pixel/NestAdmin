//! Cierre atómico de una tarea respondida fuera de NestAdmin (D178): solo desde `awaiting_review`
//! y solo si no cambió desde que se leyó; la fila `learned` entra en la misma transacción y es
//! única por `source_key`.

use nestadmin_core::{LearnedSignal, Store, TaskState};
use serde_json::json;

fn signal(key: &str) -> LearnedSignal {
    LearnedSignal {
        subject_key: "consulta|ana@alfa.test".into(),
        source_key: key.into(),
    }
}

fn learned_rows(store: &Store) -> usize {
    ["a", "b"]
        .iter()
        .filter(|k| {
            store
                .learned_source_exists(&format!("learned|x|{k}"))
                .unwrap()
        })
        .count()
}

#[test]
fn a_task_that_changed_after_it_was_read_is_not_closed_and_nothing_is_written() {
    let mut store = Store::open_in_memory().unwrap();
    let task = store
        .enqueue_review("email", "{}", &json!({ "version": 1 }), "borrador")
        .unwrap();

    // Otra `updated_at`: la persona tocó la tarea entre la planificación y la escritura.
    let stale = task.updated_at - 1;
    let closed = store
        .close_replied_outside(
            task.id,
            stale,
            &json!({ "archived": true }),
            Some(&signal("learned|x|a")),
        )
        .unwrap();
    assert!(closed.is_none());
    assert_eq!(
        store.task(task.id).unwrap().state,
        TaskState::AwaitingReview
    );
    assert_eq!(learned_rows(&store), 0);
    assert!(store
        .audit_log()
        .unwrap()
        .iter()
        .all(|e| e.action != "mail"));

    // Con la marca vigente cierra, inserta la fila y deja una sola entrada sin contenido.
    let (done, inserted) = store
        .close_replied_outside(
            task.id,
            task.updated_at,
            &json!({ "archived": true }),
            Some(&signal("learned|x|a")),
        )
        .unwrap()
        .unwrap();
    assert_eq!((done.state, inserted), (TaskState::Completed, true));
    assert_eq!(learned_rows(&store), 1);
    let events: Vec<_> = store
        .audit_log()
        .unwrap()
        .into_iter()
        .filter(|e| e.action == "mail")
        .collect();
    assert_eq!(events.len(), 1);
    assert_eq!(
        (
            events[0].status.as_str(),
            events[0].detail.as_deref(),
            events[0].task_id
        ),
        ("replied_outside", Some("{}"), Some(task.id))
    );

    // Ya cerrada: repetir no cambia nada.
    let again = store
        .close_replied_outside(
            task.id,
            done.updated_at,
            &json!({}),
            Some(&signal("learned|x|b")),
        )
        .unwrap();
    assert!(again.is_none());
    assert_eq!(learned_rows(&store), 1);
}

#[test]
fn a_learned_row_is_unique_by_its_source_key() {
    let mut store = Store::open_in_memory().unwrap();
    let task = store
        .enqueue_review("email", "{}", &json!({}), "borrador")
        .unwrap();
    assert!(store
        .insert_learned_outside(task.id, &signal("learned|x|a"))
        .unwrap());
    assert!(!store
        .insert_learned_outside(task.id, &signal("learned|x|a"))
        .unwrap());
    assert_eq!(learned_rows(&store), 1);
    // Una sola entrada del Historial: la repetición no escribe.
    let notes = store
        .audit_log()
        .unwrap()
        .into_iter()
        .filter(|e| e.action == "mail")
        .count();
    assert_eq!(notes, 1);
}
