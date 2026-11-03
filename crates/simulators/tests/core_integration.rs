//! Verificación T06 con el núcleo: `Store::perform_action` sobre los simuladores, cortes
//! simulados antes y después del efecto externo y reconciliación sin duplicados. El corte
//! se simula con un pánico en un envoltorio del conector y reabriendo base y simulador.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};

use nestadmin_core::{
    ActionOutcome, ActionRequest, Authorization, Connector, ConnectorError, CoreError, Store,
};
use nestadmin_simulators::{
    codes, AdminSimulator, FailureMode, Invoice, MailConnector, MailSimulator, NewDraft,
};
use serde_json::{json, Value};

/// Envoltorio que interrumpe el proceso justo antes o justo después del efecto externo.
struct Interrupting<'a> {
    inner: &'a dyn Connector,
    after_effect: bool,
}

impl Connector for Interrupting<'_> {
    fn lookup(&self, key: &str) -> Result<Option<Value>, ConnectorError> {
        self.inner.lookup(key)
    }

    fn execute(&self, key: &str, payload: &Value) -> Result<Value, ConnectorError> {
        if !self.after_effect {
            panic!("interrupción simulada antes del efecto externo");
        }
        self.inner.execute(key, payload)?;
        panic!("interrupción simulada tras el efecto y antes de confirmar");
    }
}

struct Paths {
    _dir: tempfile::TempDir,
    db: PathBuf,
    mail: PathBuf,
    admin: PathBuf,
}

fn paths() -> Paths {
    let dir = tempfile::tempdir().unwrap();
    Paths {
        db: dir.path().join("nestadmin.db"),
        mail: dir.path().join("mail.json"),
        admin: dir.path().join("admin.json"),
        _dir: dir,
    }
}

/// Prepara una tarea con un borrador creado en el simulador y devuelve la petición de envío.
fn prepare_send(p: &Paths) -> ActionRequest {
    let mut store = Store::open(&p.db).unwrap();
    let task_id = store.enqueue("email", "M-0002").unwrap().id;
    let mail = MailSimulator::open(&p.mail).unwrap();
    let draft = NewDraft {
        to: vec!["cliente.beta@ejemplo.test".into()],
        subject: Some("Re: Consulta sobre el pedido 2231".into()),
        body: "La entrega está prevista para el día 10.".into(),
    };
    let draft_id = mail
        .create_draft("M-0002", &draft, &format!("task-{task_id}/create_draft/v1"))
        .unwrap();
    ActionRequest {
        task_id,
        kind: "send_draft".into(),
        version: 1,
        payload: json!({ "draft_id": draft_id }),
        authorization: Authorization::Approval,
    }
}

fn register_request(store: &mut Store) -> ActionRequest {
    let task_id = store.enqueue("document", "factura-sintetica-1").unwrap().id;
    let invoice = Invoice {
        supplier: Some("Proveedor Delta S.L.".into()),
        number: Some("D-2026-007".into()),
        date: Some("2026-10-20".into()),
        currency: Some("EUR".into()),
        base: Some("50.00".into()),
        taxes: Some("10.50".into()),
        total: Some("60.50".into()),
    };
    ActionRequest {
        task_id,
        kind: "register_invoice".into(),
        version: 1,
        payload: serde_json::to_value(invoice).unwrap(),
        authorization: Authorization::Rule {
            rule_id: "regla-sintetica".into(),
            rule_version: 1,
        },
    }
}

fn crash(db: &Path, request: &ActionRequest, connector: &dyn Connector, after_effect: bool) {
    let mut store = Store::open(db).unwrap();
    let result = catch_unwind(AssertUnwindSafe(|| {
        let _ = store.perform_action(
            request,
            &Interrupting {
                inner: connector,
                after_effect,
            },
        );
    }));
    assert!(result.is_err(), "la interrupción debe ocurrir");
}

#[test]
fn send_interrupted_after_effect_is_reconciled_without_resending() {
    let p = paths();
    let request = prepare_send(&p);
    {
        let mail = MailSimulator::open(&p.mail).unwrap();
        crash(&p.db, &request, &mail, true);
        // Base y simulador se cierran aquí, como en un reinicio.
    }

    let mail = MailSimulator::open(&p.mail).unwrap();
    assert_eq!(
        mail.sent().unwrap().len(),
        1,
        "el efecto sobrevivió al reinicio"
    );
    let mut store = Store::open(&p.db).unwrap();
    let key = request.idempotency_key();
    assert_eq!(
        store.action_status(&key).unwrap().as_deref(),
        Some("pending")
    );

    let outcome = store.perform_action(&request, &mail).unwrap();
    let sent = mail.sent().unwrap();
    assert_eq!(sent.len(), 1, "no se reenvía");
    assert_eq!(
        outcome,
        ActionOutcome::Reconciled(serde_json::to_value(&sent[0]).unwrap())
    );
    assert_eq!(
        store.action_status(&key).unwrap().as_deref(),
        Some("confirmed")
    );
    assert!(matches!(
        store.perform_action(&request, &mail).unwrap(),
        ActionOutcome::AlreadyConfirmed(_)
    ));
    assert_eq!(mail.sent().unwrap().len(), 1);
}

#[test]
fn send_interrupted_before_effect_executes_exactly_once_after_restart() {
    let p = paths();
    let request = prepare_send(&p);
    {
        let mail = MailSimulator::open(&p.mail).unwrap();
        crash(&p.db, &request, &mail, false);
    }
    let mail = MailSimulator::open(&p.mail).unwrap();
    assert!(mail.sent().unwrap().is_empty());
    let mut store = Store::open(&p.db).unwrap();
    assert!(matches!(
        store.perform_action(&request, &mail).unwrap(),
        ActionOutcome::Executed(_)
    ));
    assert_eq!(mail.sent().unwrap().len(), 1);
}

#[test]
fn register_interrupted_after_effect_is_confirmed_by_reconcile_actions() {
    let p = paths();
    let request = {
        let mut store = Store::open(&p.db).unwrap();
        register_request(&mut store)
    };
    {
        let admin = AdminSimulator::open(&p.admin).unwrap();
        crash(&p.db, &request, &admin, true);
    }

    let admin = AdminSimulator::open(&p.admin).unwrap();
    assert_eq!(
        admin.record_count().unwrap(),
        3,
        "2 de ejemplo + 1 registrada"
    );
    let mut store = Store::open(&p.db).unwrap();
    let key = request.idempotency_key();
    assert_eq!(
        store.reconcile_actions("register_invoice", &admin).unwrap(),
        vec![(key.clone(), true)]
    );
    assert_eq!(
        store.action_status(&key).unwrap().as_deref(),
        Some("confirmed")
    );
    assert!(matches!(
        store.perform_action(&request, &admin).unwrap(),
        ActionOutcome::AlreadyConfirmed(_)
    ));
    assert_eq!(admin.record_count().unwrap(), 3, "sin registro duplicado");
    let statuses: Vec<String> = store
        .audit_log()
        .unwrap()
        .into_iter()
        .map(|e| e.status)
        .collect();
    assert_eq!(statuses, ["pending", "reconciled"]);
}

#[test]
fn reconcile_with_failing_lookup_keeps_action_pending_until_connector_answers() {
    let p = paths();
    let request = {
        let mut store = Store::open(&p.db).unwrap();
        register_request(&mut store)
    };
    {
        let admin = AdminSimulator::open(&p.admin).unwrap();
        crash(&p.db, &request, &admin, true);
    }

    let key = request.idempotency_key();
    let admin = AdminSimulator::open(&p.admin).unwrap();
    let mut store = Store::open(&p.db).unwrap();
    for mode in [FailureMode::Unavailable, FailureMode::Unauthorized] {
        admin.set_failure(Some(mode));
        // Sin respuesta del conector no se afirma nada: ni confirmada ni fallida.
        assert_eq!(
            store.reconcile_actions("register_invoice", &admin).unwrap(),
            vec![(key.clone(), false)]
        );
        assert_eq!(
            store.action_status(&key).unwrap().as_deref(),
            Some("pending")
        );
    }

    admin.set_failure(None);
    assert_eq!(
        store.reconcile_actions("register_invoice", &admin).unwrap(),
        vec![(key.clone(), true)]
    );
    assert_eq!(
        store.action_status(&key).unwrap().as_deref(),
        Some("confirmed")
    );
    assert!(matches!(
        store.perform_action(&request, &admin).unwrap(),
        ActionOutcome::AlreadyConfirmed(_)
    ));
    assert_eq!(admin.record_count().unwrap(), 3, "sin registro duplicado");
}

#[test]
fn unavailable_connector_is_recorded_as_failure_and_retry_registers_once() {
    let p = paths();
    let mut store = Store::open(&p.db).unwrap();
    let request = register_request(&mut store);
    let admin = AdminSimulator::open(&p.admin).unwrap();
    admin.set_failure(Some(FailureMode::Unavailable));
    match store.perform_action(&request, &admin) {
        Err(CoreError::Connector(e)) => assert_eq!(e.code, codes::UNAVAILABLE),
        other => panic!("se esperaba unavailable: {other:?}"),
    }
    let key = request.idempotency_key();
    assert_eq!(
        store.action_status(&key).unwrap().as_deref(),
        Some("failed")
    );

    admin.set_failure(None);
    assert_eq!(
        admin.record_count().unwrap(),
        2,
        "un fallo no registra nada"
    );
    assert!(matches!(
        store.perform_action(&request, &admin).unwrap(),
        ActionOutcome::Executed(_)
    ));
    assert!(matches!(
        store.perform_action(&request, &admin).unwrap(),
        ActionOutcome::AlreadyConfirmed(_)
    ));
    assert_eq!(admin.record_count().unwrap(), 3);
}
