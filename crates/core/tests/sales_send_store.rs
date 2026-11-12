//! Envío de documentos de venta por correo en el núcleo (E4b, D233, D235, D238, D239; D197):
//! migración 14 a 15 con copia previa, tablas de solo inserción, cierre idempotente con
//! `INSERT OR IGNORE`, estado `sent` solo desde `issued`, una tarea abierta por documento, disparo
//! único de la regla al emitir y anulación bloqueada con un intento sin cerrar. Solo datos
//! sintéticos. El correo, el PDF y la recuperación con cuentas están en
//! `crates/app/tests/sales_send.rs`.

use nestadmin_core::sales::{
    is_sendable, send_input_ref, Company, CustomerData, DraftInput, DraftLine, NewSalesSend,
    VerifactuContext, VerifactuEnvironment, CODE_SEND_IN_PROGRESS, SEND_SALES_DOCUMENT,
};
use nestadmin_core::store::{backup_path, SCHEMA_VERSION};
use nestadmin_core::{ActionRequest, Authorization, Connector, ConnectorError, Store, TaskState};
use nestadmin_documents::IsoDate;
use nestadmin_sales::{DocumentKind, LineInput, Status, TaxRate};
use nestadmin_verifactu::{Fecha, FechaHoraHuso, Persona, SistemaInformatico};
use rust_decimal::Decimal;
use serde_json::{json, Value};

fn date(text: &str) -> IsoDate {
    IsoDate::parse("test", text).unwrap()
}

fn ctx() -> VerifactuContext {
    let base = FechaHoraHuso::new(Fecha::new(2026, 3, 1).unwrap(), 10, 0, 0, 60)
        .unwrap()
        .unix();
    VerifactuContext {
        system: SistemaInformatico {
            productor: Persona::con_nif("Productor Demo SL", "B00000034"),
            nombre_sistema: "NestAdmin".into(),
            id_sistema: "NA".into(),
            version: "0.1.0".into(),
            numero_instalacion: "INST-0001".into(),
            solo_verifactu: true,
            multi_ot: false,
            indicador_multiples_ot: false,
        },
        now: FechaHoraHuso::desde_unix(base, 60).unwrap(),
        environment: VerifactuEnvironment::Simulador,
    }
}

fn setup(store: &mut Store) -> i64 {
    store
        .set_sales_company(&Company {
            name: Some("Taller Demo SL".into()),
            tax_id: Some("B12345674".into()),
            address: Some("Calle Ficticia 1, 28000 Madrid".into()),
            email: Some("demo@example.test".into()),
            iban: None,
            logo: None,
            logo_mime: None,
        })
        .unwrap();
    store
        .create_customer(&CustomerData {
            name: "Cliente Uno SA".into(),
            tax_id: Some("12345678Z".into()),
            address: Some("Avenida Inventada 2, 08000 Barcelona".into()),
            email: Some("cliente@example.test".into()),
        })
        .unwrap()
}

fn draft(store: &mut Store, kind: DocumentKind, customer: i64) -> i64 {
    let mut input = DraftInput::new(kind);
    input.customer_id = Some(customer);
    input.lines = vec![DraftLine::from(LineInput {
        description: "Trabajo".into(),
        quantity: "1".parse::<Decimal>().unwrap(),
        unit_price: "10".parse::<Decimal>().unwrap(),
        discount: Decimal::ZERO,
        tax_rate: TaxRate::TwentyOne,
        exemption: None,
    })];
    store.create_sales_draft(&input).unwrap()
}

fn issue(store: &mut Store, kind: DocumentKind, customer: i64) -> i64 {
    let id = draft(store, kind, customer);
    if kind.is_invoice() {
        store
            .issue_sales_invoice_registered(id, date("2026-03-01"), date("2026-03-01"), &ctx())
            .unwrap();
    } else {
        store
            .issue_sales_document(id, date("2026-03-01"), date("2026-03-01"))
            .unwrap();
    }
    id
}

/// Contenido de la tarea de envío; solo lo que el núcleo guarda tal cual.
fn result(document: i64) -> Value {
    json!({ "type": SEND_SALES_DOCUMENT, "version": 1, "document": { "id": document } })
}

fn sha(tag: char) -> String {
    tag.to_string().repeat(64)
}

fn send(document: i64, task: i64, version: i64, tag: char) -> NewSalesSend {
    NewSalesSend {
        document_id: document,
        task_id: task,
        version,
        idempotency_key: format!("task-{task}/{SEND_SALES_DOCUMENT}/v{version}"),
        account: "simulador".into(),
        account_address: Some("administracion@nestadmin.test".into()),
        recipient: "cliente@example.test".into(),
        subject: "Presupuesto P-2026-0001 — Taller Demo SL".into(),
        attachment_name: "P-2026-0001.pdf".into(),
        attachment_size: 1234,
        attachment_sha256: sha(tag),
        authorization: "aprobación".into(),
        sent_message_id: Some("S-0001".into()),
    }
}

fn count(path: &std::path::Path, table: &str) -> i64 {
    rusqlite::Connection::open(path)
        .unwrap()
        .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}

fn has_table(path: &std::path::Path, name: &str) -> bool {
    rusqlite::Connection::open(path)
        .unwrap()
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
            [name],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
        == 1
}

/// Un conector cuyo envío falla sin saberse si salió: deja la acción sin cerrar.
struct Failing;

impl Connector for Failing {
    fn lookup(&self, _key: &str) -> Result<Option<Value>, ConnectorError> {
        Ok(None)
    }
    fn execute(&self, _key: &str, _payload: &Value) -> Result<Value, ConnectorError> {
        Err(ConnectorError {
            code: "unavailable".into(),
            message: "sin respuesta".into(),
        })
    }
}

#[test]
fn a_version_14_database_migrates_to_15_with_a_copy_and_keeps_its_documents() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nestadmin.db");
    let id;
    {
        let mut store = Store::open(&path).unwrap();
        let customer = setup(&mut store);
        id = issue(&mut store, DocumentKind::Quote, customer);
    }
    // Como la dejaría la versión 14: sin las tablas de envíos.
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch(
        "DROP TABLE verifactu_authorization_halts; DROP TABLE verifactu_incidents; DROP TABLE verifactu_unsent_attempts; DROP TABLE verifactu_tls12_preferences; DROP TABLE verifactu_remission_settings; DROP TABLE verifactu_subsanations; DROP TABLE verifactu_record_results; DROP TABLE verifactu_batch_results; DROP TABLE verifactu_batch_attempts; DROP TABLE verifactu_batch_records; DROP TABLE verifactu_batches; DROP TABLE verifactu_record_environments; DROP TABLE verifactu_environment_changes; DROP TABLE sales_send_triggers; DROP TABLE sales_document_sends; PRAGMA user_version = 14;",
    )
    .unwrap();
    drop(conn);

    let store = Store::open(&path).unwrap();
    assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
    assert_eq!(SCHEMA_VERSION, 17);
    let backup = backup_path(&path, 14);
    assert_eq!(backup.file_name().unwrap(), "nestadmin.esquema-14.db");
    assert!(backup.exists(), "copia previa a migrar");
    assert!(!has_table(&backup, "sales_document_sends"));
    assert!(!has_table(&backup, "sales_send_triggers"));
    drop(store);
    for table in ["sales_document_sends", "sales_send_triggers"] {
        assert!(has_table(&path, table), "{table}");
        assert_eq!(count(&path, table), 0, "{table}");
    }

    // Los datos previos se conservan y el documento ya se puede enviar.
    let mut store = Store::open(&path).unwrap();
    let doc = store.sales_document(id).unwrap();
    assert_eq!(doc.number.as_deref(), Some("P-2026-0001"));
    assert_eq!(doc.status, Status::Issued);
    let task = store
        .enqueue_sales_send(id, &result(id), "preparado", false)
        .unwrap()
        .unwrap();
    assert_eq!(task.state, TaskState::AwaitingReview);
    assert_eq!(task.input_ref, send_input_ref(id));
}

#[test]
fn the_send_history_and_triggers_are_insert_only() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nestadmin.db");
    let mut store = Store::open(&path).unwrap();
    let customer = setup(&mut store);
    let id = issue(&mut store, DocumentKind::Quote, customer);
    let task = store
        .enqueue_sales_send(id, &result(id), "preparado", true)
        .unwrap()
        .unwrap();
    store
        .complete_sales_send(task.id, &result(id), &send(id, task.id, 1, 'a'))
        .unwrap();
    drop(store);

    let conn = rusqlite::Connection::open(&path).unwrap();
    for table in ["sales_document_sends", "sales_send_triggers"] {
        assert_eq!(count(&path, table), 1, "{table}");
        let update = conn.execute(&format!("UPDATE {table} SET task_id = task_id"), []);
        assert!(update.is_err(), "{table}: no se modifica");
        let delete = conn.execute(&format!("DELETE FROM {table}"), []);
        assert!(delete.is_err(), "{table}: no se elimina");
        assert_eq!(count(&path, table), 1, "{table}: sigue intacta");
    }
}

#[test]
fn closing_a_send_twice_inserts_one_row_and_changes_the_status_once() {
    let mut store = Store::open_in_memory().unwrap();
    let customer = setup(&mut store);
    let id = issue(&mut store, DocumentKind::Quote, customer);
    let task = store
        .enqueue_sales_send(id, &result(id), "preparado", false)
        .unwrap()
        .unwrap();
    let new = send(id, task.id, 1, 'a');

    let first = store
        .complete_sales_send(task.id, &result(id), &new)
        .unwrap();
    assert!(first.inserted);
    assert!(first.status_changed);
    assert_eq!(first.status_error, None);
    assert_eq!(first.task.state, TaskState::Completed);
    assert_eq!(store.sales_document(id).unwrap().status, Status::Sent);

    // Recuperar el mismo cierre (corte, reconciliación al abrir): ni otra fila ni otro cambio.
    let again = store
        .complete_sales_send(task.id, &result(id), &new)
        .unwrap();
    assert!(!again.inserted);
    assert!(!again.status_changed);
    assert_eq!(again.task.state, TaskState::Completed);
    // `INSERT OR IGNORE`: la misma clave con otro contenido no sustituye la fila existente.
    let other = NewSalesSend {
        recipient: "otro@example.test".into(),
        ..new.clone()
    };
    assert!(
        !store
            .complete_sales_send(task.id, &result(id), &other)
            .unwrap()
            .inserted
    );
    let rows = store.sales_document_sends(id).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].recipient, "cliente@example.test");
    assert_eq!(rows[0].attachment_sha256, sha('a'));
    assert_eq!(rows[0].authorization, "aprobación");
    let transitions = store
        .audit_log()
        .unwrap()
        .into_iter()
        .filter(|e| {
            e.action == "sales.transition" && e.detail.as_deref().unwrap_or("").contains("\"sent\"")
        })
        .count();
    assert_eq!(transitions, 1, "una sola transición a enviado");
}

#[test]
fn a_document_goes_to_sent_only_from_issued() {
    let mut store = Store::open_in_memory().unwrap();
    let customer = setup(&mut store);
    // Un presupuesto ya enviado se reenvía sin cambiar su estado: dos filas, un `sent`.
    let quote = issue(&mut store, DocumentKind::Quote, customer);
    let first = store
        .enqueue_sales_send(quote, &result(quote), "preparado", false)
        .unwrap()
        .unwrap();
    store
        .complete_sales_send(first.id, &result(quote), &send(quote, first.id, 1, 'a'))
        .unwrap();
    let second = store
        .enqueue_sales_send(quote, &result(quote), "preparado", false)
        .unwrap()
        .unwrap();
    let done = store
        .complete_sales_send(second.id, &result(quote), &send(quote, second.id, 1, 'b'))
        .unwrap();
    assert!(done.inserted);
    assert!(!done.status_changed);
    assert_eq!(done.status_error, None);
    assert_eq!(store.sales_document(quote).unwrap().status, Status::Sent);
    assert_eq!(store.sales_document_sends(quote).unwrap().len(), 2);

    // Un presupuesto aceptado tampoco cambia: el estado no retrocede ni se toca.
    let accepted = issue(&mut store, DocumentKind::Quote, customer);
    store
        .transition_sales_document(accepted, Status::Accepted)
        .unwrap();
    let task = store
        .enqueue_sales_send(accepted, &result(accepted), "preparado", false)
        .unwrap()
        .unwrap();
    let done = store
        .complete_sales_send(task.id, &result(accepted), &send(accepted, task.id, 1, 'c'))
        .unwrap();
    assert!(done.inserted && !done.status_changed);
    assert_eq!(
        store.sales_document(accepted).unwrap().status,
        Status::Accepted
    );

    // Un albarán entregado (reenvío) conserva `delivered`.
    let note = issue(&mut store, DocumentKind::DeliveryNote, customer);
    store
        .transition_sales_document(note, Status::Delivered)
        .unwrap();
    let task = store
        .enqueue_sales_send(note, &result(note), "preparado", false)
        .unwrap()
        .unwrap();
    store
        .complete_sales_send(task.id, &result(note), &send(note, task.id, 1, 'd'))
        .unwrap();
    assert_eq!(
        store.sales_document(note).unwrap().status,
        Status::Delivered
    );
}

#[test]
fn sendable_states_follow_the_decision_table() {
    use DocumentKind::*;
    for kind in [Quote, Invoice, SimplifiedInvoice, CorrectiveInvoice] {
        assert!(is_sendable(kind, Status::Issued), "{kind:?}");
        assert!(is_sendable(kind, Status::Sent), "{kind:?}");
        assert!(!is_sendable(kind, Status::Draft), "{kind:?}");
        assert!(!is_sendable(kind, Status::Voided), "{kind:?}");
        assert!(!is_sendable(kind, Status::Delivered), "{kind:?}");
    }
    for status in [
        Status::Accepted,
        Status::Rejected,
        Status::Expired,
        Status::Converted,
    ] {
        assert!(!is_sendable(Quote, status), "{status:?}");
    }
    for status in [Status::Issued, Status::Sent, Status::Delivered] {
        assert!(is_sendable(DeliveryNote, status), "{status:?}");
    }
    for status in [Status::Draft, Status::Invoiced] {
        assert!(!is_sendable(DeliveryNote, status), "{status:?}");
    }
}

#[test]
fn only_one_send_task_is_open_per_document_and_the_issue_trigger_fires_once() {
    let mut store = Store::open_in_memory().unwrap();
    let customer = setup(&mut store);
    let id = issue(&mut store, DocumentKind::Quote, customer);

    let first = store
        .enqueue_sales_send(id, &result(id), "preparado", false)
        .unwrap()
        .unwrap();
    assert_eq!(
        store.open_sales_send_task(id).unwrap().unwrap().id,
        first.id
    );
    let blocked = store
        .enqueue_sales_send(id, &result(id), "preparado", false)
        .unwrap_err();
    assert_eq!(blocked.code(), CODE_SEND_IN_PROGRESS);
    // Cerrada la tarea, otro envío manual es posible (reenviar).
    store.cancel(first.id).unwrap();
    assert!(store.open_sales_send_task(id).unwrap().is_none());

    // El disparo de la regla al emitir: una sola vez por documento, aunque la tarea se cierre.
    let issued = issue(&mut store, DocumentKind::Quote, customer);
    let task = store
        .enqueue_sales_send(issued, &result(issued), "al emitir", true)
        .unwrap()
        .unwrap();
    assert!(store
        .enqueue_sales_send(issued, &result(issued), "al emitir", true)
        .unwrap()
        .is_none());
    store.cancel(task.id).unwrap();
    assert!(store
        .enqueue_sales_send(issued, &result(issued), "al emitir", true)
        .unwrap()
        .is_none());
    let tasks = store
        .all_tasks()
        .unwrap()
        .into_iter()
        .filter(|t| t.input_ref == send_input_ref(issued))
        .count();
    assert_eq!(tasks, 1, "el disparo no creó otra tarea");
    // El botón manual sigue disponible.
    assert!(store
        .enqueue_sales_send(issued, &result(issued), "manual", false)
        .unwrap()
        .is_some());
}

#[test]
fn a_send_attempt_without_a_close_blocks_voiding_and_a_review_only_task_is_cancelled() {
    let mut store = Store::open_in_memory().unwrap();
    let customer = setup(&mut store);

    // Solo en revisión, sin intento: anular la factura cancela la tarea.
    let quiet = issue(&mut store, DocumentKind::Invoice, customer);
    let review = store
        .enqueue_sales_send(quiet, &result(quiet), "preparado", false)
        .unwrap()
        .unwrap();
    let voided = store.void_sales_invoice_registered(quiet, &ctx()).unwrap();
    assert_eq!(voided.status, Status::Voided);
    assert_eq!(store.task(review.id).unwrap().state, TaskState::Cancelled);

    // Con un intento sin cerrar (el envío falló sin saberse si salió): no se anula.
    let id = issue(&mut store, DocumentKind::Invoice, customer);
    let task = store
        .enqueue_sales_send(id, &result(id), "preparado", false)
        .unwrap()
        .unwrap();
    let request = ActionRequest {
        task_id: task.id,
        kind: SEND_SALES_DOCUMENT.into(),
        version: 1,
        payload: json!({ "draft_id": "simulador:D-0001" }),
        authorization: Authorization::Approval,
    };
    assert!(store.perform_action(&request, &Failing).is_err());
    let blocked = store.void_sales_invoice_registered(id, &ctx()).unwrap_err();
    assert_eq!(blocked.code(), CODE_SEND_IN_PROGRESS);
    assert_eq!(store.sales_document(id).unwrap().status, Status::Issued);
    assert_eq!(
        store.task(task.id).unwrap().state,
        TaskState::AwaitingReview,
        "la tarea no se tocó"
    );
    let records = store.verifactu_records().unwrap();
    assert!(
        !records
            .iter()
            .any(|r| r.document_id == id
                && r.kind == nestadmin_core::sales::VerifactuKind::Anulacion),
        "no se escribió ningún registro de anulación"
    );

    // Cuando el intento se cierra como no realizado, la anulación procede y cancela la tarea.
    let action = store.task_actions(task.id).unwrap().remove(0);
    store
        .supersede_action(task.id, &action, &Authorization::Approval, "no salió")
        .unwrap();
    let voided = store.void_sales_invoice_registered(id, &ctx()).unwrap();
    assert_eq!(voided.status, Status::Voided);
    assert_eq!(store.task(task.id).unwrap().state, TaskState::Cancelled);
}

#[test]
fn a_confirmed_action_whose_task_is_still_open_also_blocks_voiding() {
    let mut store = Store::open_in_memory().unwrap();
    let customer = setup(&mut store);
    let id = issue(&mut store, DocumentKind::Invoice, customer);
    let task = store
        .enqueue_sales_send(id, &result(id), "preparado", false)
        .unwrap()
        .unwrap();
    struct Sends;
    impl Connector for Sends {
        fn lookup(&self, _key: &str) -> Result<Option<Value>, ConnectorError> {
            Ok(None)
        }
        fn execute(&self, _key: &str, _payload: &Value) -> Result<Value, ConnectorError> {
            Ok(json!({ "receipt_id": "S-0001" }))
        }
    }
    let request = ActionRequest {
        task_id: task.id,
        kind: SEND_SALES_DOCUMENT.into(),
        version: 1,
        payload: json!({ "draft_id": "simulador:D-0001" }),
        authorization: Authorization::Approval,
    };
    store.perform_action(&request, &Sends).unwrap();
    // El correo salió pero la tarea aún no se cerró: anular dejaría una factura anulada enviada.
    let blocked = store.void_sales_invoice_registered(id, &ctx()).unwrap_err();
    assert_eq!(blocked.code(), CODE_SEND_IN_PROGRESS);
    store
        .complete_sales_send(task.id, &result(id), &send(id, task.id, 1, 'a'))
        .unwrap();
    assert_eq!(store.sales_document(id).unwrap().status, Status::Sent);
    // Enviada, la factura ya no se anula (se rectifica, D82).
    assert!(store.void_sales_invoice_registered(id, &ctx()).is_err());
}
