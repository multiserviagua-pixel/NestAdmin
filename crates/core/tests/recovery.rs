//! Verificación T02 a nivel de núcleo: persistencia, recuperación tras interrupción y
//! ausencia de efectos duplicados. Las interrupciones se simulan cerrando la base de datos
//! (o con un pánico) en el punto crítico y reabriendo el mismo archivo.

use std::cell::RefCell;
use std::collections::HashMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;
use std::rc::Rc;

use nestadmin_core::{
    ActionOutcome, ActionRequest, Authorization, Connector, ConnectorError, CoreError, Store,
    TaskError, TaskState,
};
use serde_json::{json, Value};

/// Sistema externo simulado: conserva los efectos entre "reinicios" del núcleo.
#[derive(Default)]
struct World {
    effects: HashMap<String, Value>,
    executions: usize,
}

struct FakeConnector {
    world: Rc<RefCell<World>>,
    fail_with: Option<&'static str>,
    crash_before_execute: bool,
    crash_after_execute: bool,
}

impl FakeConnector {
    fn new(world: &Rc<RefCell<World>>) -> Self {
        Self {
            world: Rc::clone(world),
            fail_with: None,
            crash_before_execute: false,
            crash_after_execute: false,
        }
    }
}

impl Connector for FakeConnector {
    fn lookup(&self, key: &str) -> Result<Option<Value>, ConnectorError> {
        Ok(self.world.borrow().effects.get(key).cloned())
    }

    fn execute(&self, key: &str, payload: &Value) -> Result<Value, ConnectorError> {
        if let Some(code) = self.fail_with {
            return Err(ConnectorError {
                code: code.into(),
                message: "simulado".into(),
            });
        }
        if self.crash_before_execute {
            panic!("interrupción simulada antes de llegar al sistema externo");
        }
        let mut world = self.world.borrow_mut();
        if let Some(receipt) = world.effects.get(key) {
            return Ok(receipt.clone());
        }
        world.executions += 1;
        let receipt = json!({ "receipt": format!("R-{}", world.executions), "payload": payload });
        world.effects.insert(key.to_owned(), receipt.clone());
        drop(world);
        if self.crash_after_execute {
            panic!("interrupción simulada tras ejecutar y antes de confirmar");
        }
        Ok(receipt)
    }
}

fn db(dir: &tempfile::TempDir) -> std::path::PathBuf {
    dir.path().join("nestadmin.db")
}

fn send_request(task_id: i64, version: i64, body: &str) -> ActionRequest {
    ActionRequest {
        task_id,
        kind: "send_draft".into(),
        version,
        payload: json!({ "draft_id": "B-1", "body": body }),
        authorization: Authorization::Approval,
    }
}

fn reopen(path: &Path) -> Store {
    Store::open(path).expect("reabrir base de datos")
}

#[test]
fn tasks_survive_restart_in_every_state_and_running_is_requeued() {
    let dir = tempfile::tempdir().unwrap();
    let path = db(&dir);
    let error = TaskError {
        code: "ocr_failed".into(),
        message: "simulado".into(),
        recoverable: true,
    };
    let ids = {
        let mut store = Store::open(&path).unwrap();
        let running = store.enqueue("document", "doc-1").unwrap().id;
        let review = store.enqueue("document", "doc-2").unwrap().id;
        let failed = store.enqueue("document", "doc-3").unwrap().id;
        let done = store.enqueue("document", "doc-4").unwrap().id;
        // La toma es FIFO.
        for id in [running, review, failed, done] {
            assert_eq!(store.claim_next().unwrap().unwrap().id, id);
        }
        let queued = store.enqueue("document", "doc-5").unwrap().id;
        store
            .await_review(review, &json!({ "total": "121.00" }))
            .unwrap();
        store.fail(failed, &error).unwrap();
        store.complete(done, &json!({ "ok": true })).unwrap();
        (queued, running, review, failed, done)
        // La base se cierra aquí sin pasar `running` a ningún estado final.
    };
    let (queued, running, review, failed, done) = ids;

    let mut store = reopen(&path);
    assert_eq!(store.recover().unwrap(), vec![running]);
    let task = store.task(running).unwrap();
    assert_eq!(task.state, TaskState::Queued);
    assert_eq!(task.attempts, 1, "el intento interrumpido queda contado");
    assert_eq!(store.task(queued).unwrap().state, TaskState::Queued);
    assert_eq!(
        store.task(review).unwrap().result,
        Some(json!({ "total": "121.00" }))
    );
    assert_eq!(store.task(failed).unwrap().error, Some(error));
    assert_eq!(store.task(done).unwrap().state, TaskState::Completed);
    let history: Vec<String> = store
        .transitions(running)
        .unwrap()
        .into_iter()
        .map(|t| t.2)
        .collect();
    assert_eq!(
        history,
        ["creada", "iniciada", "recuperada tras interrupción"]
    );

    // Una segunda recuperación no encuentra nada que hacer.
    assert!(store.recover().unwrap().is_empty());
}

#[test]
fn invalid_transitions_are_rejected_and_final_states_stay_final() {
    let mut store = Store::open_in_memory().unwrap();
    let id = store.enqueue("email", "msg-1").unwrap().id;
    assert!(matches!(
        store.complete(id, &json!({})),
        Err(CoreError::InvalidTransition { .. })
    ));
    store.claim_next().unwrap();
    store.complete(id, &json!({})).unwrap();
    assert!(matches!(
        store.retry(id),
        Err(CoreError::InvalidTransition { .. })
    ));
    assert!(matches!(
        store.cancel(id),
        Err(CoreError::InvalidTransition { .. })
    ));

    let other = store.enqueue("email", "msg-2").unwrap().id;
    store.cancel(other).unwrap();
    assert!(
        matches!(store.claim_next(), Ok(None)),
        "una tarea cancelada no se vuelve a tomar"
    );
    assert!(matches!(store.task(999), Err(CoreError::TaskNotFound(999))));
}

#[test]
fn failed_task_keeps_error_visible_until_explicit_retry_succeeds() {
    let mut store = Store::open_in_memory().unwrap();
    let id = store.enqueue("document", "doc-1").unwrap().id;
    store.claim_next().unwrap();
    let error = TaskError {
        code: "model_unavailable".into(),
        message: "Ollama no responde".into(),
        recoverable: true,
    };
    store.fail(id, &error).unwrap();
    assert!(
        store.claim_next().unwrap().is_none(),
        "un fallo no se reintenta solo"
    );

    let retried = store.retry(id).unwrap();
    assert_eq!(retried.state, TaskState::Queued);
    assert_eq!(
        retried.error,
        Some(error),
        "el último error sigue visible tras reencolar"
    );
    let running = store.claim_next().unwrap().unwrap();
    assert_eq!(running.attempts, 2);
    let done = store.complete(id, &json!({ "ok": true })).unwrap();
    assert_eq!(done.error, None);
}

#[test]
fn confirmed_action_is_not_executed_again() {
    let world = Rc::new(RefCell::new(World::default()));
    let connector = FakeConnector::new(&world);
    let mut store = Store::open_in_memory().unwrap();
    let task = store.enqueue("email", "msg-1").unwrap().id;
    let request = send_request(task, 1, "Adjuntamos la factura.");

    let first = store.perform_action(&request, &connector).unwrap();
    assert!(matches!(first, ActionOutcome::Executed(_)));
    let second = store.perform_action(&request, &connector).unwrap();
    assert!(matches!(second, ActionOutcome::AlreadyConfirmed(_)));
    assert_eq!(first.receipt(), second.receipt());
    assert_eq!(world.borrow().executions, 1);
}

#[test]
fn interruption_after_external_effect_is_reconciled_without_duplicate() {
    let dir = tempfile::tempdir().unwrap();
    let path = db(&dir);
    let world = Rc::new(RefCell::new(World::default()));
    let task;
    {
        let mut store = Store::open(&path).unwrap();
        task = store.enqueue("email", "msg-1").unwrap().id;
        store.claim_next().unwrap();
        let crashing = FakeConnector {
            crash_after_execute: true,
            ..FakeConnector::new(&world)
        };
        let request = send_request(task, 1, "Adjuntamos la factura.");
        let result = catch_unwind(AssertUnwindSafe(|| {
            store.perform_action(&request, &crashing)
        }));
        assert!(result.is_err(), "la interrupción simulada debe ocurrir");
    }
    assert_eq!(
        world.borrow().executions,
        1,
        "el efecto externo ocurrió una vez"
    );

    let mut store = reopen(&path);
    let key = send_request(task, 1, "").idempotency_key();
    assert_eq!(
        store.action_status(&key).unwrap().as_deref(),
        Some("pending")
    );
    assert_eq!(store.recover().unwrap(), vec![task]);

    let connector = FakeConnector::new(&world);
    let outcome = store
        .perform_action(&send_request(task, 1, "Adjuntamos la factura."), &connector)
        .unwrap();
    assert!(matches!(outcome, ActionOutcome::Reconciled(_)));
    assert_eq!(
        world.borrow().executions,
        1,
        "la recuperación no repite el envío"
    );
    assert_eq!(
        store.action_status(&key).unwrap().as_deref(),
        Some("confirmed")
    );
    let statuses: Vec<String> = store
        .audit_log()
        .unwrap()
        .into_iter()
        .map(|e| e.status)
        .collect();
    assert_eq!(statuses, ["pending", "reconciled"]);
}

#[test]
fn interruption_before_external_effect_executes_exactly_once_on_retry() {
    let dir = tempfile::tempdir().unwrap();
    let path = db(&dir);
    let world = Rc::new(RefCell::new(World::default()));
    let request;
    {
        let mut store = Store::open(&path).unwrap();
        let task = store.enqueue("invoice", "doc-1").unwrap().id;
        request = ActionRequest {
            task_id: task,
            kind: "register_invoice".into(),
            version: 1,
            payload: json!({ "invoice_number": "F-001", "total": "121.00" }),
            authorization: Authorization::Rule {
                rule_id: "R-proveedor-habitual".into(),
                rule_version: 3,
            },
        };
        // El conector no responde: la clave queda persistida antes de cualquier efecto.
        let down = FakeConnector {
            fail_with: Some("unavailable"),
            ..FakeConnector::new(&world)
        };
        assert!(matches!(
            store.perform_action(&request, &down),
            Err(CoreError::Connector(_))
        ));
    }
    let mut store = reopen(&path);
    assert_eq!(
        store
            .action_status(&request.idempotency_key())
            .unwrap()
            .as_deref(),
        Some("failed")
    );
    let outcome = store
        .perform_action(&request, &FakeConnector::new(&world))
        .unwrap();
    assert!(matches!(outcome, ActionOutcome::Executed(_)));
    assert_eq!(world.borrow().executions, 1);
    let log = store.audit_log().unwrap();
    assert_eq!(
        log.iter().map(|e| e.status.as_str()).collect::<Vec<_>>(),
        ["pending", "failed", "confirmed"]
    );
    assert!(log
        .iter()
        .all(|e| e.authorization.as_deref() == Some("regla R-proveedor-habitual v3")));
}

#[test]
fn reconcile_actions_confirms_known_effects_and_keeps_unknown_pending() {
    let dir = tempfile::tempdir().unwrap();
    let path = db(&dir);
    let world = Rc::new(RefCell::new(World::default()));
    let (done, unknown) = {
        let mut store = Store::open(&path).unwrap();
        let a = store.enqueue("email", "msg-1").unwrap().id;
        let b = store.enqueue("email", "msg-2").unwrap().id;
        let crashing = FakeConnector {
            crash_after_execute: true,
            ..FakeConnector::new(&world)
        };
        let _ = catch_unwind(AssertUnwindSafe(|| {
            store.perform_action(&send_request(a, 1, "uno"), &crashing)
        }));
        // Para `b` la interrupción ocurre antes de llegar al sistema externo.
        let early = FakeConnector {
            crash_before_execute: true,
            ..FakeConnector::new(&world)
        };
        let _ = catch_unwind(AssertUnwindSafe(|| {
            store.perform_action(&send_request(b, 1, "dos"), &early)
        }));
        (
            send_request(a, 1, "").idempotency_key(),
            send_request(b, 1, "").idempotency_key(),
        )
    };
    let mut store = reopen(&path);
    let result = store
        .reconcile_actions("send_draft", &FakeConnector::new(&world))
        .unwrap();
    assert_eq!(result, vec![(done.clone(), true), (unknown.clone(), false)]);
    assert_eq!(
        store.action_status(&done).unwrap().as_deref(),
        Some("confirmed")
    );
    assert_eq!(
        store.action_status(&unknown).unwrap().as_deref(),
        Some("pending")
    );
    assert_eq!(world.borrow().executions, 1);
}

#[test]
fn recorded_attempts_survive_restart_and_superseded_actions_are_never_reconciled() {
    let dir = tempfile::tempdir().unwrap();
    let path = db(&dir);
    let world = Rc::new(RefCell::new(World::default()));
    let task = {
        let mut store = Store::open(&path).unwrap();
        let task = store.enqueue("email", "msg-1").unwrap().id;
        store.claim_next().unwrap();
        // El resultado que se va a ejecutar se guarda antes de llamar al conector (D50).
        let attempt = json!({ "version": 1, "attempt": { "version": 1 } });
        store.record_attempt(task, &attempt).unwrap();
        let early = FakeConnector {
            crash_before_execute: true,
            ..FakeConnector::new(&world)
        };
        let _ = catch_unwind(AssertUnwindSafe(|| {
            store.perform_action(&send_request(task, 1, "uno"), &early)
        }));
        task
    };
    let mut store = reopen(&path);
    store.recover().unwrap();
    let recovered = store.task(task).unwrap();
    assert_eq!(recovered.state, TaskState::Queued);
    assert_eq!(recovered.result.unwrap()["attempt"]["version"], 1);
    // Solo se registra en tareas en curso o en revisión.
    assert!(store.record_attempt(task, &json!({})).is_err());

    let actions = store.task_actions(task).unwrap();
    assert_eq!(actions.len(), 1);
    assert!(actions[0].is_unsettled());
    store
        .supersede_action(task, &actions[0], &Authorization::Approval, "prueba")
        .unwrap();
    // Sustituir dos veces no deja dos entradas en el historial.
    store
        .supersede_action(task, &actions[0], &Authorization::Approval, "prueba")
        .unwrap();
    let key = send_request(task, 1, "").idempotency_key();
    assert_eq!(
        store.action_status(&key).unwrap().as_deref(),
        Some("superseded")
    );
    assert!(store
        .reconcile_actions("send_draft", &FakeConnector::new(&world))
        .unwrap()
        .is_empty());
    let log: Vec<String> = store
        .audit_log()
        .unwrap()
        .into_iter()
        .map(|e| e.status)
        .collect();
    assert_eq!(log, ["pending", "superseded"]);
    assert_eq!(world.borrow().executions, 0);
}

#[test]
fn reconcile_action_confirms_a_failed_attempt_that_did_happen() {
    let dir = tempfile::tempdir().unwrap();
    let world = Rc::new(RefCell::new(World::default()));
    let mut store = Store::open(&db(&dir)).unwrap();
    let task = store.enqueue("email", "msg-1").unwrap().id;
    let request = send_request(task, 1, "uno");
    // El efecto ocurre, pero el conector responde con error: queda `failed`.
    FakeConnector::new(&world)
        .execute(&request.idempotency_key(), &request.payload)
        .unwrap();
    let failing = FakeConnector {
        fail_with: Some("unavailable"),
        ..FakeConnector::new(&world)
    };
    assert!(store.perform_action(&request, &failing).is_err());
    let action = store.task_actions(task).unwrap().remove(0);
    assert_eq!(action.status, "failed");
    let receipt = FakeConnector::new(&world)
        .lookup(&action.idempotency_key)
        .unwrap()
        .unwrap();
    store.reconcile_action(task, &action, &receipt).unwrap();
    let action = store.task_actions(task).unwrap().remove(0);
    assert_eq!(action.status, "confirmed");
    assert_eq!(action.receipt, Some(receipt));
    assert_eq!(world.borrow().executions, 1);
}

#[test]
fn changed_content_needs_new_version_and_never_reuses_key() {
    let world = Rc::new(RefCell::new(World::default()));
    let connector = FakeConnector::new(&world);
    let mut store = Store::open_in_memory().unwrap();
    let task = store.enqueue("email", "msg-1").unwrap().id;
    store
        .perform_action(&send_request(task, 1, "Texto aprobado"), &connector)
        .unwrap();

    let edited = send_request(task, 1, "Texto modificado tras aprobar");
    assert!(matches!(
        store.perform_action(&edited, &connector),
        Err(CoreError::IdempotencyConflict(_))
    ));
    assert_eq!(world.borrow().executions, 1);

    let reapproved = send_request(task, 2, "Texto modificado tras aprobar");
    assert!(matches!(
        store.perform_action(&reapproved, &connector).unwrap(),
        ActionOutcome::Executed(_)
    ));
    assert_eq!(world.borrow().executions, 2);
}

#[test]
fn audit_log_does_not_store_action_content() {
    let world = Rc::new(RefCell::new(World::default()));
    let mut store = Store::open_in_memory().unwrap();
    let task = store.enqueue("email", "msg-1").unwrap().id;
    let secret = "token-de-prueba-no-real-123";
    store
        .perform_action(&send_request(task, 1, secret), &FakeConnector::new(&world))
        .unwrap();
    for entry in store.audit_log().unwrap() {
        assert!(!format!("{entry:?}").contains(secret));
    }
}

#[test]
fn newer_schema_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = db(&dir);
    drop(Store::open(&path).unwrap());
    rusqlite::Connection::open(&path)
        .unwrap()
        .pragma_update(None, "user_version", 99)
        .unwrap();
    assert!(matches!(
        Store::open(&path),
        Err(CoreError::UnsupportedSchema { found: 99, .. })
    ));
}
