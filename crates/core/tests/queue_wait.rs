//! `claim` y `release` de la cola (D101): tomar una tarea concreta y devolverla sin gastar
//! intento cuando la IA no está utilizable.

use nestadmin_core::{CoreError, Store, TaskState};

#[test]
fn claim_takes_a_specific_task_and_release_spends_no_attempt() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(&dir.path().join("t.db")).unwrap();
    let first = store.enqueue("email", "{}").unwrap();
    let second = store.enqueue("email", "{}").unwrap();

    let taken = store.claim(second.id).unwrap();
    assert_eq!((taken.state, taken.attempts), (TaskState::Running, 1));
    assert_eq!(store.task(first.id).unwrap().state, TaskState::Queued);

    let released = store.release(second.id, "sin IA").unwrap();
    assert_eq!((released.state, released.attempts), (TaskState::Queued, 0));
    let history = store.transitions(second.id).unwrap();
    assert_eq!(
        history.last().unwrap(),
        &(
            Some("running".to_owned()),
            "queued".to_owned(),
            "sin IA".to_owned()
        )
    );

    // Una tarea que ya se había intentado conserva sus intentos anteriores.
    store.claim(second.id).unwrap();
    let error = nestadmin_core::TaskError {
        code: "unavailable".into(),
        message: "sin conexión".into(),
        recoverable: true,
    };
    store.fail(second.id, &error).unwrap();
    store.retry(second.id).unwrap();
    store.claim(second.id).unwrap();
    let again = store.release(second.id, "sin IA").unwrap();
    assert_eq!(again.attempts, 1);

    // Solo se devuelve una tarea en curso y solo se toma una en cola.
    assert!(matches!(
        store.release(again.id, "otra vez"),
        Err(CoreError::InvalidTransition { .. })
    ));
    store.claim(first.id).unwrap();
    assert!(matches!(
        store.claim(first.id),
        Err(CoreError::InvalidTransition { .. })
    ));
}
