//! Flujos de la capa de aplicación con un proveedor simulado y datos sintéticos: revisión,
//! aprobación de la versión vista, reglas, duplicados, fallos y recuperación sin efectos
//! duplicados.

use std::path::Path;
use std::sync::{Arc, Mutex};

use nestadmin_ai::{
    AiError, GenerateRequest, GenerateResult, Provider, ProviderKind, Task as AiTask, Usage,
};
use nestadmin_app::{App, AppError, AppPaths, EMAIL, INVOICE};
use nestadmin_core::TaskState;
use nestadmin_extract::{ExtractError, Extractor, Ocr};
use serde_json::{json, Value};

/// Devuelve el JSON incluido en el documento (extracción) o una respuesta fija (correo).
/// `fail` hace que la siguiente llamada falle como si Ollama no respondiera.
struct FakeProvider {
    fail: Mutex<bool>,
}

impl FakeProvider {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            fail: Mutex::new(false),
        })
    }
}

impl Provider for FakeProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Ollama
    }
    fn model(&self) -> &str {
        "simulado"
    }
    fn generate(&self, request: &GenerateRequest) -> Result<GenerateResult, AiError> {
        if std::mem::take(&mut *self.fail.lock().unwrap()) {
            return Err(AiError::Unavailable("sin conexión".into()));
        }
        let output = match request.task {
            AiTask::Extraction => serde_json::from_str(&request.content).unwrap(),
            _ => {
                json!({ "category": "solicitud_factura", "draft": "Adjuntamos la factura solicitada." })
            }
        };
        Ok(GenerateResult {
            request_id: request.request_id.clone(),
            provider: ProviderKind::Ollama,
            model: "simulado".into(),
            output,
            latency_ms: 1,
            usage: Usage::default(),
        })
    }
}

/// La fecha depende del número para que facturas distintas no parezcan duplicadas
/// (mismo proveedor, fecha y total).
fn invoice(supplier: &str, number: &str, total: &str) -> String {
    let day: u32 = number
        .trim_start_matches(|c: char| !c.is_ascii_digit())
        .parse()
        .unwrap();
    json!({
        "supplier": supplier,
        "invoice_number": number,
        "date": format!("2026-10-{day:02}"),
        "subtotal": "100.00",
        "tax": "21.00",
        "total": total,
    })
    .to_string()
}

/// OCR que nunca se usa: estas pruebas solo importan texto (los PDF se prueban en
/// `crates/extract` y en `tests/ollama.rs`).
struct NoOcr;

impl Ocr for NoOcr {
    fn recognize(&self, _image: &Path, page: usize) -> Result<String, ExtractError> {
        Err(ExtractError::OcrUnavailable(format!("página {page}")))
    }
}

fn open(dir: &tempfile::TempDir, provider: Arc<FakeProvider>) -> App {
    let extractor = Extractor::new(dir.path().join("sin-pdfium"), Box::new(NoOcr));
    App::open(&AppPaths::new(dir.path()), provider, Arc::new(extractor)).unwrap()
}

#[test]
fn only_text_and_pdf_files_are_imported_and_pdf_failures_are_visible() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = open(&dir, FakeProvider::new());
    for (name, bytes) in [
        ("hoja.xlsx", &b"PK"[..]),
        ("sin-extension", b"texto"),
        ("vacia.pdf", b""),
    ] {
        assert!(
            matches!(app.import_file(name, bytes), Err(AppError::Invalid(_))),
            "{name}"
        );
    }
    assert!(app.tasks().unwrap().is_empty());

    // Sin PDFium, el PDF falla con un motivo recuperable y sin resultado.
    let task = app.import_file("Factura.PDF", b"%PDF-1.4").unwrap();
    let failed = app.process_next().unwrap().unwrap();
    assert_eq!(failed.id, task.id);
    assert_eq!(failed.state, TaskState::Failed);
    let error = failed.error.unwrap();
    assert_eq!(
        (error.code.as_str(), error.recoverable),
        ("pdf_unavailable", true)
    );
    assert!(failed.result.is_none());

    // Un .txt importado como archivo sigue el flujo normal y registra la página.
    app.import_file("a.txt", invoice("Alfa SL", "F-1", "121.00").as_bytes())
        .unwrap();
    let result = app.process_next().unwrap().unwrap().result.unwrap();
    assert_eq!(
        result["pages"],
        json!([{ "number": 1, "method": "plain_text", "chars": 115 }])
    );
}

#[test]
fn invoice_waits_for_review_and_registers_once_after_approval() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = open(&dir, FakeProvider::new());
    let before = app.simulators().1.record_count().unwrap();

    app.import_document("alfa.txt", &invoice("Alfa SL", "F-1", "121.00"))
        .unwrap();
    let task = app.process_next().unwrap().unwrap();
    assert_eq!(
        task.state,
        TaskState::AwaitingReview,
        "sin regla no hay registro automático"
    );
    let result = task.result.clone().unwrap();
    assert_eq!(result["fields"]["total"], "121.00");
    assert_eq!(result["issues"], json!([]));
    assert_eq!(result["review_reason"], "ninguna regla activa coincide");
    assert_eq!(app.simulators().1.record_count().unwrap(), before);

    let done = app.approve(task.id, 1).unwrap();
    assert_eq!(done.state, TaskState::Completed);
    assert!(done.result.unwrap()["receipt"]["record_id"].is_string());
    assert_eq!(app.simulators().1.record_count().unwrap(), before + 1);
    assert!(
        app.approve(task.id, 1).is_err(),
        "una tarea completada no se aprueba dos veces"
    );
    assert_eq!(app.simulators().1.record_count().unwrap(), before + 1);
    assert!(app.process_next().unwrap().is_none());
}

#[test]
fn issues_and_duplicates_are_reported_by_code() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = open(&dir, FakeProvider::new());
    app.import_document("a.txt", &invoice("Alfa SL", "F-1", "125.00"))
        .unwrap();
    app.import_document("b.txt", &invoice("Alfa SL", "F-1", "125.00"))
        .unwrap();
    let first = app.process_next().unwrap().unwrap().result.unwrap();
    let second = app.process_next().unwrap().unwrap().result.unwrap();
    assert_eq!(first["issues"], json!(["total_mismatch"]));
    assert_eq!(
        second["issues"],
        json!(["total_mismatch", "possible_duplicate"])
    );
    assert_eq!(second["duplicates"].as_array().unwrap().len(), 1);
}

#[test]
fn an_active_matching_rule_automates_only_clean_invoices() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = open(&dir, FakeProvider::new());
    let conditions = json!({ "supplier": "alfa sl", "max_total": "200.00" });
    app.create_rule("alfa", "register_invoice", &conditions)
        .unwrap();

    // Regla inactiva: revisión.
    app.import_document("1.txt", &invoice("Alfa SL", "F-1", "121.00"))
        .unwrap();
    assert_eq!(
        app.process_next().unwrap().unwrap().state,
        TaskState::AwaitingReview
    );

    app.set_rule_active("alfa", true).unwrap();
    app.import_document("2.txt", &invoice("Alfa SL", "F-2", "121.00"))
        .unwrap();
    let automated = app.process_next().unwrap().unwrap();
    assert_eq!(automated.state, TaskState::Completed);
    let result = automated.result.unwrap();
    assert_eq!(
        result["authorization"],
        json!({ "type": "rule", "rule_id": "alfa", "rule_version": 2 })
    );

    // Con incidencias, por encima del límite o de otro proveedor: revisión.
    for (name, text) in [
        ("3.txt", invoice("Alfa SL", "F-3", "130.00")),
        ("4.txt", invoice("Beta SA", "F-4", "121.00")),
    ] {
        app.import_document(name, &text).unwrap();
        assert_eq!(
            app.process_next().unwrap().unwrap().state,
            TaskState::AwaitingReview
        );
    }

    // Dos reglas coincidentes: la decisión no es inequívoca.
    app.create_rule(
        "alfa-2",
        "register_invoice",
        &json!({ "supplier": "Alfa SL" }),
    )
    .unwrap();
    app.set_rule_active("alfa-2", true).unwrap();
    app.import_document("5.txt", &invoice("Alfa SL", "F-5", "121.00"))
        .unwrap();
    let ambiguous = app.process_next().unwrap().unwrap();
    assert_eq!(ambiguous.state, TaskState::AwaitingReview);
    assert!(ambiguous.result.unwrap()["review_reason"]
        .as_str()
        .unwrap()
        .contains("2 reglas"));
}

#[test]
fn invalid_rules_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = open(&dir, FakeProvider::new());
    assert!(app
        .create_rule("r", "register_invoice", &json!({}))
        .is_err());
    assert!(app
        .create_rule(
            "r",
            "register_invoice",
            &json!({ "supplier": "A", "max_total": "5" })
        )
        .is_err());
    assert!(app
        .create_rule("r", "send_draft", &json!({ "category": "otra" }))
        .is_err());
    assert!(app.create_rule("r", "delete_all", &json!({})).is_err());
    assert!(app.rules().unwrap().is_empty());
}

#[test]
fn mail_is_fetched_once_and_edited_drafts_need_new_approval() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = open(&dir, FakeProvider::new());
    let fetched = app.fetch_mail().unwrap();
    assert!(fetched.added > 0);
    assert_eq!(
        app.fetch_mail().unwrap().added,
        0,
        "los mensajes ya encolados no se repiten"
    );

    let task = app.process_next().unwrap().unwrap();
    assert_eq!(task.kind, EMAIL);
    assert_eq!(task.state, TaskState::AwaitingReview);
    let result = task.result.unwrap();
    assert_eq!(result["category"], "solicitud_factura");
    assert_eq!(
        result["draft"]["to"],
        json!(["facturas@proveedor-alfa.test"])
    );

    let edited = app.edit_draft(task.id, "Texto corregido.").unwrap();
    assert_eq!(edited.result.as_ref().unwrap()["version"], 2);
    assert!(matches!(
        app.approve(task.id, 1),
        Err(AppError::StaleVersion {
            approved: 1,
            current: 2
        })
    ));
    assert!(app.simulators().0.sent().unwrap().is_empty());

    let sent = app.approve(task.id, 2).unwrap();
    assert_eq!(sent.state, TaskState::Completed);
    let receipts = app.simulators().0.sent().unwrap();
    assert_eq!(receipts.len(), 1);
    assert_eq!(
        receipts[0].idempotency_key,
        format!("simulador/task-{}/send_draft/v2", task.id),
        "la clave que recibe la cuenta lleva su identificador"
    );
    let drafts = app.simulators().0.drafts().unwrap();
    assert_eq!(drafts.last().unwrap().content.body, "Texto corregido.");
}

#[test]
fn a_draft_without_sender_is_never_sent_by_a_rule() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = open(&dir, FakeProvider::new());
    app.create_rule(
        "facturas",
        "send_draft",
        &json!({ "category": "solicitud_factura" }),
    )
    .unwrap();
    app.set_rule_active("facturas", true).unwrap();
    app.fetch_mail().unwrap();
    let mut states = Vec::new();
    while let Some(task) = app.process_next().unwrap() {
        let result = task.result.clone().unwrap();
        states.push((result["draft"]["to"].as_array().unwrap().len(), task.state));
    }
    assert!(states.contains(&(1, TaskState::Completed)));
    assert!(
        states.contains(&(0, TaskState::AwaitingReview)),
        "M-0005 no tiene remitente"
    );
    assert!(states
        .iter()
        .all(|(to, state)| *to > 0 || *state != TaskState::Completed));
}

#[test]
fn model_failure_is_visible_and_retry_requeues() {
    let dir = tempfile::tempdir().unwrap();
    let provider = FakeProvider::new();
    let mut app = open(&dir, provider.clone());
    app.import_document("a.txt", &invoice("Alfa SL", "F-1", "121.00"))
        .unwrap();
    *provider.fail.lock().unwrap() = true;
    let failed = app.process_next().unwrap().unwrap();
    assert_eq!(failed.state, TaskState::Failed);
    assert_eq!(failed.error.as_ref().unwrap().code, "unavailable");
    assert!(
        failed.result.is_none(),
        "un fallo nunca deja un resultado inventado"
    );

    app.retry(failed.id).unwrap();
    assert_eq!(
        app.process_next().unwrap().unwrap().state,
        TaskState::AwaitingReview
    );
}

#[test]
fn empty_documents_and_unknown_tasks_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = open(&dir, FakeProvider::new());
    assert!(matches!(
        app.import_document("vacio.txt", "  "),
        Err(AppError::Invalid(_))
    ));
    assert!(app.approve(99, 1).is_err());
    assert!(app.tasks().unwrap().is_empty());
}

#[test]
fn restart_recovers_work_without_duplicate_effects() {
    let dir = tempfile::tempdir().unwrap();
    let provider = FakeProvider::new();
    let (approved_id, interrupted_id) = {
        let mut app = open(&dir, provider.clone());
        app.import_document("a.txt", &invoice("Alfa SL", "F-1", "121.00"))
            .unwrap();
        app.import_document("b.txt", &invoice("Beta SA", "F-9", "121.00"))
            .unwrap();
        let first = app.process_next().unwrap().unwrap();
        app.approve(first.id, 1).unwrap();
        // La segunda tarea queda en curso, como si la aplicación se cerrara a mitad.
        let job = app.begin_next().unwrap().unwrap();
        (first.id, job.task.id)
    };
    let records = {
        let app = open(&dir, provider.clone());
        app.simulators().1.record_count().unwrap()
    };

    let mut app = open(&dir, provider);
    let tasks = app.tasks().unwrap();
    let state = |id: i64| tasks.iter().find(|t| t.id == id).unwrap().state;
    assert_eq!(state(approved_id), TaskState::Completed);
    assert_eq!(
        state(interrupted_id),
        TaskState::Queued,
        "vuelve a la cola al reabrir"
    );
    let resumed = app.process_next().unwrap().unwrap();
    assert_eq!(resumed.id, interrupted_id);
    assert_eq!(resumed.state, TaskState::AwaitingReview);
    assert_eq!(
        app.simulators().1.record_count().unwrap(),
        records,
        "ningún registro repetido"
    );

    // El registro de duplicados se reconstruye: la misma factura se detecta tras reiniciar.
    app.import_document("c.txt", &invoice("Alfa SL", "F-1", "121.00"))
        .unwrap();
    let again = app.process_next().unwrap().unwrap().result.unwrap();
    assert_eq!(again["issues"], json!(["possible_duplicate"]));
    assert_eq!(again["provider"], "ollama");
    let kinds: Vec<_> = app.tasks().unwrap().into_iter().map(|t| t.kind).collect();
    assert!(kinds.iter().all(|k| k == INVOICE));
}

#[test]
fn audit_log_records_actions_and_rules() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = open(&dir, FakeProvider::new());
    app.create_rule(
        "alfa",
        "register_invoice",
        &json!({ "supplier": "Alfa SL" }),
    )
    .unwrap();
    app.import_document("a.txt", &invoice("Alfa SL", "F-1", "121.00"))
        .unwrap();
    let task = app.process_next().unwrap().unwrap();
    app.approve(task.id, 1).unwrap();
    let actions: Vec<Value> = app
        .audit_log()
        .unwrap()
        .into_iter()
        .map(|e| json!([e.action, e.status]))
        .collect();
    assert!(actions.contains(&json!(["rule", "inactive"])));
    assert!(
        actions.contains(&json!(["register_invoice", "confirmed"])),
        "{actions:?}"
    );
}
