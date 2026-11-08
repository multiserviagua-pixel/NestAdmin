//! Flujos de la capa de aplicación con un proveedor simulado y datos sintéticos: revisión,
//! aprobación de la versión vista, reglas, duplicados, fallos y recuperación sin efectos
//! duplicados.

use std::path::Path;
use std::sync::{Arc, Mutex};

use nestadmin_ai::{
    AiError, GenerateRequest, GenerateResult, Provider, ProviderKind, Task as AiTask, Usage,
};
use nestadmin_app::panel::NewRule;
use nestadmin_app::{App, AppError, AppPaths, EMAIL, INVOICE};
use nestadmin_core::TaskState;
use nestadmin_extract::{ExtractError, Extractor, Ocr};
use nestadmin_simulators::FailureMode;
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
            // Las facturas de las pruebas son JSON; la del adjunto sintético de M-0001 es texto.
            AiTask::Extraction => serde_json::from_str(&request.content).unwrap_or_else(|_| {
                assert!(
                    request.content.contains("FS-2026-014"),
                    "{}",
                    request.content
                );
                json!({
                    "supplier": "Proveedor Alfa Suministros SL", "invoice_number": "FS-2026-014",
                    "date": "2026-10-01", "subtotal": "120.00", "tax": "25.20", "total": "145.20",
                })
            }),
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
    // Una factura que debe poder automatizarse muestra el euro (D127).
    let mut value: Value = serde_json::from_str(&invoice_plain(supplier, number, total)).unwrap();
    value["moneda"] = json!("EUR");
    value.to_string()
}

/// Como `invoice`, pero sin ninguna marca de moneda en el texto.
fn invoice_plain(supplier: &str, number: &str, total: &str) -> String {
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
fn only_text_pdf_and_image_files_are_imported_and_failures_are_visible() {
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

    // Las fotos se admiten (D67) y van al OCR; una imagen ilegible falla sin resultado.
    let photo = app
        .import_file("Foto.JPG", b"\xFF\xD8\xFF no es JPEG")
        .unwrap();
    let failed = app.process_next().unwrap().unwrap();
    assert_eq!(failed.id, photo.id);
    let error = failed.error.unwrap();
    assert_eq!(
        (error.code.as_str(), error.recoverable),
        ("document_unreadable", false)
    );
    assert!(failed.result.is_none());

    // Un .txt importado como archivo sigue el flujo normal y registra la página.
    app.import_file("a.txt", invoice("Alfa SL", "F-1", "121.00").as_bytes())
        .unwrap();
    let result = app.process_next().unwrap().unwrap().result.unwrap();
    assert_eq!(
        result["pages"],
        json!([{ "number": 1, "method": "plain_text", "chars": 130, "offset": 0 }])
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
fn rules_with_numeric_limit_or_unknown_keys_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = open(&dir, FakeProvider::new());
    for conditions in [
        json!({ "supplier": "Alfa SL", "max_total": 500 }),
        json!({ "supplier": "Alfa SL", "max_total": null }),
        json!({ "supplier": "Alfa SL", "colour": "EUR" }),
        json!({ "supplier": "Alfa SL", "currency": "eur" }),
        json!({ "supplier": "Alfa SL", "currency": "USD" }),
        json!({ "supplier": "Alfa SL", "currency": 978 }),
        json!(["Alfa SL"]),
    ] {
        assert!(app
            .create_rule("r", "register_invoice", &conditions)
            .is_err());
    }
    // Hoy solo se registran facturas en euros (D91): otra moneda se rechaza con un mensaje claro.
    let error = app
        .create_rule(
            "r",
            "register_invoice",
            &json!({ "supplier": "Alfa SL", "currency": "USD" }),
        )
        .unwrap_err()
        .to_string();
    assert!(error.contains("solo registra facturas en euros"), "{error}");
    for conditions in [
        json!({ "category": "solicitud_factura", "colour": "x@example.test" }),
        json!({ "category": "solicitud_factura" }),
    ] {
        assert!(app.create_rule("d", "send_draft", &conditions).is_err());
    }
    assert!(app.rules().unwrap().is_empty());

    app.create_rule("r", "register_invoice", &json!({ "supplier": "Alfa SL" }))
        .unwrap();
    for conditions in [
        json!({ "supplier": "Alfa SL", "max_total": 500 }),
        json!({ "supplier": "Alfa SL", "colour": "EUR" }),
    ] {
        assert!(app.update_rule("r", &conditions).is_err());
    }
    assert_eq!(app.rules().unwrap()[0].version, 1);
}

#[test]
fn invalid_rule_ids_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = open(&dir, FakeProvider::new());
    let conditions = json!({ "supplier": "Alfa SL" });
    for id in ["", "   ", "Alfa", "con espacio", "a_b", "ñ", "a/b"] {
        assert!(
            app.create_rule(id, "register_invoice", &conditions)
                .is_err(),
            "{id:?}"
        );
    }
    assert!(app
        .create_rule(" alfa-2 ", "register_invoice", &conditions)
        .is_ok());
    assert_eq!(app.rules().unwrap()[0].id, "alfa-2");
}

/// Reglas antiguas escritas directamente en el almacén, sin pasar por `check_rule`.
fn legacy_rules_dir() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let mut store = nestadmin_core::Store::open(&dir.path().join("nestadmin.db")).unwrap();
    for (id, conditions) in [
        (
            "numerica",
            json!({ "supplier": "Alfa SL", "max_total": 500 }),
        ),
        (
            "clave-rara",
            json!({ "supplier": "Alfa SL", "colour": "EUR" }),
        ),
    ] {
        store
            .create_rule(id, "register_invoice", &conditions)
            .unwrap();
        store.set_rule_active(id, true).unwrap();
    }
    dir
}

#[test]
fn defective_legacy_rules_never_automate() {
    let dir = legacy_rules_dir();
    let mut app = open(&dir, FakeProvider::new());
    assert!(app.rules().unwrap().iter().all(|r| r.active));
    app.import_document("1.txt", &invoice("Alfa SL", "F-1", "121.00"))
        .unwrap();
    let task = app.process_next().unwrap().unwrap();
    assert_eq!(task.state, TaskState::AwaitingReview);
    assert_eq!(
        task.result.unwrap()["review_reason"],
        "ninguna regla activa coincide"
    );
}

#[test]
fn defective_rules_cannot_be_activated_but_can_be_deactivated() {
    let dir = legacy_rules_dir();
    let mut app = open(&dir, FakeProvider::new());
    // Desactivar siempre funciona.
    assert!(!app.set_rule_active("numerica", false).unwrap().active);
    assert!(!app.set_rule_active("clave-rara", false).unwrap().active);
    // Volver a activarla falla y no cambia el estado.
    for id in ["numerica", "clave-rara"] {
        let error = app.set_rule_active(id, true).unwrap_err().to_string();
        assert!(error.contains("no se puede activar"), "{error}");
    }
    assert!(app.rules().unwrap().iter().all(|r| !r.active));
    // Una regla inexistente sigue dando el mismo error.
    assert!(app.set_rule_active("no-existe", true).is_err());
    assert!(app.set_rule_active("no-existe", false).is_err());
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
    // Regla sin remitente con la validación avanzada (D126): coincide también con el correo
    // sin remitente, que solo el destinatario ausente impide enviar.
    app.create_rule_full(NewRule {
        id: Some("facturas"),
        name: None,
        action_kind: "send_draft",
        conditions: &json!({ "category": "solicitud_factura" }),
        allow_any_sender: true,
    })
    .unwrap();
    app.set_rule_active("facturas", true).unwrap();
    app.fetch_mail().unwrap();
    let mut states = Vec::new();
    while let Some(task) = app.process_next().unwrap() {
        let result = task.result.clone().unwrap();
        // La factura adjunta a M-0001 también se procesa; aquí solo cuentan los correos.
        if let Some(to) = result["draft"]["to"].as_array() {
            states.push((to.len(), task.state));
        }
    }
    assert!(states.contains(&(1, TaskState::Completed)));
    assert!(
        states.contains(&(0, TaskState::AwaitingReview)),
        "M-0006 no tiene remitente"
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

#[test]
fn doubtful_amounts_ask_for_human_review_with_file_and_amount() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = open(&dir, FakeProvider::new());
    // Ni siquiera una regla activa que coincida registra una factura con importes dudosos.
    app.create_rule(
        "alfa",
        "register_invoice",
        &json!({ "supplier": "Alfa SL" }),
    )
    .unwrap();
    app.set_rule_active("alfa", true).unwrap();
    let before = app.simulators().1.record_count().unwrap();

    // Formato español: se lee sin duda y la regla lo registra.
    let spanish = json!({
        "supplier": "Alfa SL", "invoice_number": "F-1", "date": "2026-10-01",
        "subtotal": "1.000,00", "tax": "210,00", "total": "1.210,00 €",
    });
    app.import_document("es.txt", &spanish.to_string()).unwrap();
    let done = app.process_next().unwrap().unwrap();
    assert_eq!(done.state, TaskState::Completed);
    let result = done.result.unwrap();
    assert_eq!(result["fields"]["total"], "1210.00");
    assert_eq!(result["amount_review"], Value::Null);
    assert_eq!(app.simulators().1.record_count().unwrap(), before + 1);

    // «1.000» sin decimales: se propone 1000.00 y se pide revisión con archivo e importe.
    let doubtful = json!({
        "supplier": "Alfa SL", "invoice_number": "F-2", "date": "2026-10-02",
        "subtotal": "1.000", "tax": "210,00", "total": "1.210,00",
    });
    app.import_document("dudosa.txt", &doubtful.to_string())
        .unwrap();
    let task = app.process_next().unwrap().unwrap();
    assert_eq!(task.state, TaskState::AwaitingReview);
    let result = task.result.clone().unwrap();
    assert_eq!(result["issues"], json!(["amount_to_review"]));
    assert_eq!(result["fields"]["subtotal"], "1000.00");
    assert_eq!(
        result["recovered"],
        json!([{
            "field": "subtotal", "literal": "1.000", "evidence": "1.000",
            "value": "1000.00", "reason": "ambiguous_separator",
        }])
    );
    let path = result["document_path"].as_str().unwrap();
    assert!(Path::new(path).is_file(), "la copia del documento existe");
    let reason = result["review_reason"].as_str().unwrap();
    assert_eq!(reason, result["amount_review"].as_str().unwrap());
    for expected in [
        "dudosa.txt",
        path,
        "Base imponible",
        "«1.000»",
        "1000.00",
        "su aprobación",
    ] {
        assert!(
            reason.contains(expected),
            "falta {expected:?} en {reason:?}"
        );
    }
    assert_eq!(app.simulators().1.record_count().unwrap(), before + 1);

    // Una suma que no cuadra también pide revisión con los importes.
    app.import_document("descuadre.txt", &invoice("Alfa SL", "F-3", "130.00"))
        .unwrap();
    let mismatch = app.process_next().unwrap().unwrap();
    assert_eq!(mismatch.state, TaskState::AwaitingReview);
    let reason = mismatch.result.unwrap()["review_reason"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(reason.contains("base 100.00 + IVA 21.00 no coincide con el total 130.00"));

    // La persona revisa y aprueba: se registra una sola vez con el valor confirmado.
    let approved = app.approve(task.id, 1).unwrap();
    assert_eq!(approved.state, TaskState::Completed);
    assert_eq!(app.simulators().1.record_count().unwrap(), before + 2);
}

#[test]
fn imported_documents_record_where_they_came_from() {
    use sha2::{Digest, Sha256};

    let dir = tempfile::tempdir().unwrap();
    let mut app = open(&dir, FakeProvider::new());
    let outside = tempfile::tempdir().unwrap();
    let original = outside.path().join("factura-alfa.txt");
    let content = invoice("Alfa SL", "F-7", "121.00");
    std::fs::write(&original, &content).unwrap();

    let task = app.import_path(&original).unwrap();
    let input: Value = serde_json::from_str(&task.input_ref).unwrap();
    let origin = &input["origin"];
    assert_eq!(origin["kind"], "file");
    assert_eq!(origin["name"], "factura-alfa.txt");
    assert_eq!(origin["original_path"], original.display().to_string());
    assert!(origin["imported_at"].as_u64().unwrap() > 1_700_000_000);
    let digest: String = Sha256::digest(content.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    assert_eq!(origin["sha256"], digest);
    assert_eq!(origin["size"], content.len());

    // El original se abre desde la copia guardada en la carpeta de documentos.
    let copy = app.document_file(task.id).unwrap();
    assert!(copy.starts_with(std::fs::canonicalize(dir.path().join("documentos")).unwrap()));
    assert_eq!(std::fs::read_to_string(&copy).unwrap(), content);

    let done = app.process_next().unwrap().unwrap();
    let result = done.result.unwrap();
    assert_eq!(result["origin"], *origin);
    // Cada campo dice dónde aparece en el texto; el documento de texto es una sola página.
    assert_eq!(result["sources"]["total"]["pages"], json!([1]));
    assert!(result["sources"]["total"]["line"]
        .as_str()
        .unwrap()
        .contains("121.00"));
    assert_eq!(result["sources"]["supplier"]["text"], "Alfa SL");
    // La moneda no aparece en el texto: no tiene procedencia, en lugar de una inventada.
    assert!(result["sources"].get("currency").is_none());

    // Sin ruta conocida, la ruta queda explícitamente ausente.
    let pasted = app
        .import_file("pegada.txt", invoice("Beta SL", "F-8", "121.00").as_bytes())
        .unwrap();
    let pasted: Value = serde_json::from_str(&pasted.input_ref).unwrap();
    assert_eq!(pasted["origin"]["original_path"], Value::Null);

    // Si la copia desaparece, se dice; nunca se abre otra ruta.
    std::fs::remove_file(&copy).unwrap();
    let error = app.document_file(task.id).unwrap_err().to_string();
    assert!(error.contains("ya no está"), "{error}");
}

#[test]
fn mail_origin_is_kept_in_mail_tasks_and_their_attachments() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = open(&dir, FakeProvider::new());
    app.fetch_mail().unwrap();
    let mail_task = app
        .tasks()
        .unwrap()
        .into_iter()
        .find(|t| {
            let input: Value = serde_json::from_str(&t.input_ref).unwrap();
            input["message_id"] == "simulador:M-0005"
        })
        .unwrap();
    let input: Value = serde_json::from_str(&mail_task.input_ref).unwrap();
    assert_eq!(
        input["origin"],
        json!({
            "account": "simulador", "provider": "simulator",
            "label": "Correo simulado", "address": "administracion@nestadmin.test",
        })
    );
    // El simulador trae remitente sintético, destinatarios y fecha.
    assert_eq!(input["message"]["from"], "logistica@transportes-delta.test");
    assert_eq!(
        input["message"]["to"],
        json!(["administracion@nestadmin.test"])
    );
    assert_eq!(input["message"]["date"], "2026-10-06");

    // Un adjunto de ese correo hereda la cuenta, el remitente y el asunto.
    let attachment = app
        .import_attachment(
            mail_task.id,
            "horario.txt",
            invoice("Delta SL", "F-9", "121.00").as_bytes(),
        )
        .unwrap();
    let origin = serde_json::from_str::<Value>(&attachment.input_ref).unwrap()["origin"].clone();
    assert_eq!(origin["kind"], "email");
    assert_eq!(
        origin["account"]["address"],
        "administracion@nestadmin.test"
    );
    assert_eq!(origin["message_id"], "simulador:M-0005");
    assert_eq!(origin["from"], "logistica@transportes-delta.test");
    assert_eq!(origin["subject"], "Cambio de horario");
    assert_eq!(origin["attachment"], "horario.txt");
    assert_eq!(origin["sha256"].as_str().unwrap().len(), 64);
    assert!(app.document_file(attachment.id).is_ok());

    // Solo los correos tienen adjuntos, y un correo no tiene documento que abrir.
    assert!(app
        .import_attachment(attachment.id, "x.txt", b"texto")
        .is_err());
    assert!(app.document_file(mail_task.id).is_err());
}

#[test]
fn mail_attachments_become_documents_once_and_failures_are_retried() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = open(&dir, FakeProvider::new());
    let invoices = |app: &mut App| -> Vec<Value> {
        app.tasks()
            .unwrap()
            .into_iter()
            .filter(|t| t.kind == INVOICE)
            .map(|t| serde_json::from_str::<Value>(&t.input_ref).unwrap()["origin"].clone())
            .collect()
    };

    // M-0001 trae una factura sintética en texto: se importa con el origen del correo.
    let report = app.fetch_mail().unwrap();
    assert_eq!(report.accounts[0].attachments.imported, 1);
    let first = invoices(&mut app);
    assert_eq!(first.len(), 1);
    assert_eq!(first[0]["kind"], "email");
    assert_eq!(first[0]["message_id"], "simulador:M-0001");
    assert_eq!(first[0]["attachment"], "factura-FS-2026-014.txt");
    assert_eq!(first[0]["from"], "facturas@proveedor-alfa.test");

    // Volver a consultar no la duplica.
    let again = app.fetch_mail().unwrap();
    assert_eq!(again.accounts[0].attachments.imported, 0);
    assert_eq!(invoices(&mut app).len(), 1);

    // Un tipo no admitido se omite y se dice; uno que no se puede descargar falla visible
    // y se importa en la consulta siguiente.
    let sim = app.simulators().0;
    sim.add_attachment("M-0002", "contrato.docx", "application/msword", b"x")
        .unwrap();
    sim.add_attachment(
        "M-0002",
        "pedido-2231.pdf",
        "application/pdf",
        b"%PDF-1.4 sintetico",
    )
    .unwrap();
    sim.set_attachment_failure(Some(FailureMode::Unavailable));
    let failed = app.fetch_mail().unwrap();
    let fetch = &failed.accounts[0].attachments;
    assert_eq!(fetch.imported, 0);
    assert_eq!(fetch.skipped, ["contrato.docx"]);
    assert_eq!(fetch.failed.len(), 1);
    assert_eq!(fetch.failed[0].name, "pedido-2231.pdf");
    assert_eq!(fetch.failed[0].code, "unavailable");
    assert_eq!(invoices(&mut app).len(), 1);

    app.simulators().0.set_attachment_failure(None);
    let retried = app.fetch_mail().unwrap();
    assert_eq!(retried.accounts[0].attachments.imported, 1);
    assert!(retried.accounts[0].attachments.failed.is_empty());
    let all = invoices(&mut app);
    assert_eq!(all.len(), 2);
    let pdf = all
        .iter()
        .find(|o| o["attachment"] == "pedido-2231.pdf")
        .unwrap();
    assert_eq!(pdf["subject"], "Consulta sobre el pedido 2231");
    assert_eq!(
        app.fetch_mail().unwrap().accounts[0].attachments.imported,
        0
    );
}

#[test]
fn a_document_with_several_invoices_gives_one_review_per_invoice() {
    let dir = tempfile::tempdir().unwrap();
    let provider = FakeProvider::new();
    let mut app = open(&dir, Arc::clone(&provider));
    // Aunque una regla activa coincida, las facturas separadas pasan por revisión.
    app.create_rule(
        "alfa",
        "register_invoice",
        &json!({ "supplier": "alfa sl" }),
    )
    .unwrap();
    app.set_rule_active("alfa", true).unwrap();

    let mut both: Value = serde_json::from_str(&invoice("Alfa SL", "F-3", "121.00")).unwrap();
    both["other_invoices"] = json!([
        serde_json::from_str::<Value>(&invoice("Alfa SL", "F-4", "121.00")).unwrap(),
        // Un elemento vacío no es una factura y no crea tarea.
        { "supplier": null, "invoice_number": null, "date": null,
          "subtotal": null, "tax": null, "total": null },
    ]);
    app.import_document("foto-dos-facturas.txt", &both.to_string())
        .unwrap();

    let first = app.process_next().unwrap().unwrap();
    assert_eq!(first.state, TaskState::AwaitingReview);
    let result = first.result.unwrap();
    assert_eq!(result["fields"]["invoice_number"], "F-3");
    assert_eq!(result["part"], json!({ "index": 1, "of": 2 }));
    assert!(result["review_reason"]
        .as_str()
        .unwrap()
        .contains("varias facturas"));

    // La segunda reutiliza la extracción: si se llamara otra vez a la IA, fallaría.
    *provider.fail.lock().unwrap() = true;
    let second = app.process_next().unwrap().unwrap();
    assert_eq!(
        second.state,
        TaskState::AwaitingReview,
        "{:?}",
        second.error
    );
    let result = second.result.unwrap();
    assert_eq!(result["fields"]["invoice_number"], "F-4");
    assert_eq!(result["fields"]["supplier"], "Alfa SL");
    assert_eq!(result["part"]["index"], 2);
    assert_eq!(result["part"]["of"], 2);
    assert_eq!(result["part"]["split_from"], first.id);
    assert_eq!(result["document"], "foto-dos-facturas.txt");
    assert_eq!(result["origin"]["kind"], "file");
    assert!(app.process_next().unwrap().is_none(), "solo dos facturas");
    // La IA no se llamó para la segunda: el fallo forzado sigue pendiente.
    assert!(*provider.fail.lock().unwrap());
    assert_eq!(
        app.document_file(second.id).unwrap(),
        app.document_file(first.id).unwrap()
    );
}

#[test]
fn an_update_waits_for_running_work_and_keeps_the_queue() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = open(&dir, FakeProvider::new());
    app.import_document("1.txt", &invoice("Alfa SL", "F-1", "121.00"))
        .unwrap();
    app.import_document("2.txt", &invoice("Beta SL", "F-2", "121.00"))
        .unwrap();

    // Una tarea en marcha: no se puede instalar.
    let job = app.begin_next().unwrap().unwrap();
    app.set_queue_paused(true);
    let busy = app.update_readiness().unwrap();
    assert!(!busy.ready);
    assert_eq!((busy.running, busy.queued, busy.paused), (1, 1, true));

    // En pausa no empieza la siguiente, pero la que estaba en marcha termina.
    assert!(app.begin_next().unwrap().is_none());
    app.finish(job.run()).unwrap();
    let ready = app.update_readiness().unwrap();
    assert!(ready.ready);
    assert_eq!(
        (ready.running, ready.queued, ready.awaiting_review),
        (0, 1, 1)
    );

    // Tras «instalar» (reabrir), la pausa no persiste y la tarea en cola sigue.
    drop(app);
    let mut app = open(&dir, FakeProvider::new());
    assert!(!app.update_readiness().unwrap().paused);
    let next = app.process_next().unwrap().unwrap();
    assert_eq!(next.state, TaskState::AwaitingReview);
    assert_eq!(next.result.unwrap()["fields"]["invoice_number"], "F-2");
}

// ---- Panel de reglas: remitente, moneda, avanzada, archivo, evidencia y simulación ----

/// Contenido de las tablas indicadas, fila a fila, para comparar antes y después.
fn dump(dir: &tempfile::TempDir, tables: &[&str]) -> Vec<(String, Vec<String>)> {
    let conn = rusqlite::Connection::open(dir.path().join("nestadmin.db")).unwrap();
    tables
        .iter()
        .map(|table| {
            let mut stmt = conn
                .prepare(&format!("SELECT * FROM {table} ORDER BY rowid"))
                .unwrap();
            let columns = stmt.column_count();
            let rows = stmt
                .query_map([], |row| {
                    Ok((0..columns)
                        .map(|i| match row.get_ref(i).unwrap() {
                            rusqlite::types::ValueRef::Text(bytes) => {
                                String::from_utf8_lossy(bytes).into_owned()
                            }
                            other => format!("{other:?}"),
                        })
                        .collect::<Vec<_>>()
                        .join("|"))
                })
                .unwrap()
                .map(Result::unwrap)
                .collect();
            (table.to_string(), rows)
        })
        .collect()
}

const PANEL_TABLES: [&str; 5] = [
    "audit_log",
    "tasks",
    "actions",
    "rule_evaluations",
    "rule_signals",
];

fn count(dir: &tempfile::TempDir, table: &str) -> usize {
    dump(dir, &[table])[0].1.len()
}

/// Reglas escritas directamente en el almacén, como las guardaba la versión anterior.
fn with_stored_rules(dir: &tempfile::TempDir, rules: &[(&str, &str, Value)]) {
    let mut store = nestadmin_core::Store::open(&dir.path().join("nestadmin.db")).unwrap();
    for (id, kind, conditions) in rules {
        store.create_rule(id, kind, conditions).unwrap();
        store.set_rule_active(id, true).unwrap();
    }
}

/// Correos de la bandeja de ejemplo procesados: remitente, estado y resultado.
fn mail_states(app: &mut App) -> Vec<(Option<String>, TaskState, Value)> {
    app.fetch_mail().unwrap();
    while app.process_next().unwrap().is_some() {}
    app.tasks()
        .unwrap()
        .into_iter()
        .filter(|t| t.kind == EMAIL)
        .map(|t| {
            let result = t.result.clone().unwrap();
            (
                result["message"]["from"].as_str().map(str::to_owned),
                t.state,
                result,
            )
        })
        .collect()
}

/// Factura cuyo texto lleva una nota (donde se puede ver una marca de moneda). Es JSON para
/// que el proveedor simulado la lea; la extracción local de moneda ve todo el texto.
fn invoice_note(supplier: &str, number: &str, total: &str, note: &str) -> String {
    let mut value: Value = serde_json::from_str(&invoice_plain(supplier, number, total)).unwrap();
    value["nota"] = json!(note);
    value.to_string()
}

#[test]
fn another_currency_mentioned_in_the_document_sends_it_to_review_with_or_without_a_limit() {
    // (nota del documento, ¿se registra por regla?)
    let notes = [
        ("Total $ 121.00", false),
        ("Total US$ 121.00", false),
        ("Total 121.00 USD", false),
        // Código pegado al importe, delante o detrás, y símbolo de ancho completo.
        ("Total USD500.00", false),
        ("Total 500USD", false),
        ("Total ＄121.00", false),
        ("Total £121.00", false),
        // Sin ninguna marca de moneda no se puede comprobar que sea en euros (D127).
        ("Factura sin ninguna marca de moneda", false),
        ("CADENA USDA SCAD", false),
        ("Total 121,00 €", true),
        ("CADENA USDA SCAD y 121,00 EUR", true),
        // Espaciado amplio y paréntesis no esconden otra moneda.
        ("Total 121.00     PEN", false),
        ("Importe (TRY): 121,00", false),
    ];
    for (rule_name, conditions) in [
        (
            "con límite",
            json!({ "supplier": "Alfa SL", "max_total": "500.00" }),
        ),
        ("sin límite", json!({ "supplier": "Alfa SL" })),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let mut app = open(&dir, FakeProvider::new());
        app.create_rule("alfa", "register_invoice", &conditions)
            .unwrap();
        app.set_rule_active("alfa", true).unwrap();
        let before = app.simulators().1.record_count().unwrap();
        for (n, (note, registers)) in notes.iter().enumerate() {
            let number = format!("F-{}", n + 1);
            app.import_document(
                &format!("{n}.txt"),
                &invoice_note("Alfa SL", &number, "121.00", note),
            )
            .unwrap();
            let task = app.process_next().unwrap().unwrap();
            let result = task.result.clone().unwrap();
            assert_eq!(
                task.state == TaskState::Completed,
                *registers,
                "{rule_name}: {note}"
            );
            // Solo códigos, nunca el texto.
            assert!(!result["currency_evidence"].to_string().contains("Total"));
            if !registers {
                let usd_like = result["currency_evidence"]["foreign"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|c| c.as_str().unwrap())
                    .collect::<Vec<_>>()
                    .join(", ");
                if usd_like.is_empty() {
                    // La regla coincide, pero el documento no muestra el euro.
                    assert_eq!(result["evaluation"]["code"], "currency_not_shown", "{note}");
                    assert_eq!(
                        result["review_reason"],
                        "el documento no muestra la moneda; no se puede comprobar que sea en euros"
                    );
                } else if rule_name == "con límite" {
                    // La regla no coincide, con el motivo concreto.
                    assert_eq!(result["review_reason"], "ninguna regla activa coincide");
                    assert_eq!(
                        result["evaluation"]["candidates"][0]["mismatch"],
                        format!("el documento menciona {usd_like} y el límite está en EUR")
                    );
                } else {
                    // La regla coincide, pero la factura no se registra sola.
                    assert_eq!(result["evaluation"]["code"], "foreign_currency");
                    assert_eq!(
                        result["review_reason"],
                        format!(
                            "el documento menciona {usd_like}; hoy la aplicación solo registra facturas en euros"
                        )
                    );
                }
            }
        }
        let registered = notes.iter().filter(|(_, registers)| *registers).count();
        assert_eq!(
            app.simulators().1.record_count().unwrap(),
            before + registered,
            "{rule_name}"
        );
    }
}

// ---- Documentos escaneados: la moneda no se lee (D127) ----

/// OCR que "lee" siempre el mismo texto.
struct FixedText(String);

impl Ocr for FixedText {
    fn recognize(&self, _image: &Path, _page: usize) -> Result<String, ExtractError> {
        Ok(self.0.clone())
    }
}

/// Mapa de bits mínimo de 2x2 píxeles: lo único que importa es que sea una imagen.
fn tiny_bmp() -> Vec<u8> {
    let mut bytes = b"BM".to_vec();
    bytes.extend(70u32.to_le_bytes());
    bytes.extend([0, 0, 0, 0]);
    bytes.extend(54u32.to_le_bytes());
    bytes.extend(40u32.to_le_bytes());
    bytes.extend(2i32.to_le_bytes());
    bytes.extend(2i32.to_le_bytes());
    bytes.extend(1u16.to_le_bytes());
    bytes.extend(24u16.to_le_bytes());
    bytes.extend([0u8; 24]);
    bytes.extend([255u8; 16]);
    bytes
}

#[test]
fn a_scanned_invoice_without_a_legible_euro_goes_to_review_with_or_without_a_limit() {
    // (texto que lee el OCR, ¿se registra por regla?). El `$` se lee como `S`. Sin euro a la
    // vista no se puede comprobar la moneda, con texto digital o con OCR (D127).
    let cases = [
        ("Total S 121.00", false),
        ("Total 121.00", false),
        ("Total € 121.00", true),
        ("Total 121.00 EUR", true),
    ];
    for conditions in [
        json!({ "supplier": "Alfa SL" }),
        json!({ "supplier": "Alfa SL", "max_total": "500.00" }),
    ] {
        for (n, (text, registers)) in cases.iter().enumerate() {
            let dir = tempfile::tempdir().unwrap();
            let json_text = invoice_note("Alfa SL", &format!("F-{}", n + 1), "121.00", text);
            let extractor = Extractor::new(
                dir.path().join("sin-pdfium"),
                Box::new(FixedText(json_text)),
            );
            let mut app = App::open(
                &AppPaths::new(dir.path()),
                FakeProvider::new(),
                Arc::new(extractor),
            )
            .unwrap();
            app.create_rule("alfa", "register_invoice", &conditions)
                .unwrap();
            app.set_rule_active("alfa", true).unwrap();
            let before = app.simulators().1.record_count().unwrap();
            app.import_file("factura.bmp", &tiny_bmp()).unwrap();
            let task = app.process_next().unwrap().unwrap();
            let result = task.result.clone().unwrap();
            assert_eq!(
                result["pages"][0]["method"], "ocr",
                "{text}: debe pasar por OCR"
            );
            assert_eq!(result["currency_evidence"]["ocr"], true);
            assert!(!result["currency_evidence"].to_string().contains("Total"));
            assert_eq!(
                task.state == TaskState::Completed,
                *registers,
                "{conditions} / {text}"
            );
            let registered = usize::from(*registers);
            assert_eq!(
                app.simulators().1.record_count().unwrap(),
                before + registered
            );
            if !registers {
                assert_eq!(result["evaluation"]["code"], "currency_not_shown", "{text}");
                assert_eq!(
                    result["review_reason"],
                    "el documento no muestra la moneda; no se puede comprobar que sea en euros"
                );
            }
        }
    }

    // Un documento con texto digital sin ninguna marca de moneda tampoco se registra solo; con
    // el euro a la vista, sí.
    let dir = tempfile::tempdir().unwrap();
    let mut app = open(&dir, FakeProvider::new());
    app.create_rule(
        "alfa",
        "register_invoice",
        &json!({ "supplier": "Alfa SL" }),
    )
    .unwrap();
    app.set_rule_active("alfa", true).unwrap();
    app.import_document("1.txt", &invoice_plain("Alfa SL", "F-1", "121.00"))
        .unwrap();
    let task = app.process_next().unwrap().unwrap();
    assert_eq!(task.state, TaskState::AwaitingReview);
    let result = task.result.unwrap();
    assert_eq!(result["currency_evidence"]["ocr"], false);
    assert_eq!(result["evaluation"]["code"], "currency_not_shown");
    app.import_document("2.txt", &invoice("Alfa SL", "F-2", "121.00"))
        .unwrap();
    let task = app.process_next().unwrap().unwrap();
    assert_eq!(task.state, TaskState::Completed);
    assert_eq!(task.result.unwrap()["currency_evidence"]["ocr"], false);
}

#[test]
fn a_task_without_the_currency_check_is_never_registered_by_a_rule_and_the_simulation_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = open(&dir, FakeProvider::new());
    app.import_document("1.txt", &invoice("Alfa SL", "F-1", "121.00"))
        .unwrap();
    let old = app.process_next().unwrap().unwrap();
    assert_eq!(old.state, TaskState::AwaitingReview);
    // Una tarea guardada antes de D127: sin la comprobación local de moneda.
    rusqlite::Connection::open(dir.path().join("nestadmin.db"))
        .unwrap()
        .execute_batch("UPDATE tasks SET result = json_remove(result, '$.currency_evidence');")
        .unwrap();
    let before = dump(&dir, &PANEL_TABLES);
    let sim = app
        .simulate_rule(
            "register_invoice",
            &json!({ "supplier": "Alfa SL" }),
            None,
            false,
            None,
        )
        .unwrap();
    assert_eq!(sim.without_currency_check, 1);
    assert_eq!(sim.would_automate, 0);
    assert_eq!(sim.would_review.get("currency_not_checked"), Some(&1));
    assert_eq!(
        sim.items[0].explanation,
        "no consta la comprobación de moneda; reprocese o revise"
    );
    assert_eq!(dump(&dir, &PANEL_TABLES), before);
}

#[test]
fn a_send_rule_acts_only_for_its_sender_address_or_domain() {
    let cases: [(&str, Value, bool); 5] = [
        (
            "dirección con otras mayúsculas",
            json!({ "sender": "FACTURAS@Proveedor-Alfa.test" }),
            true,
        ),
        (
            "dominio",
            json!({ "sender_domain": "proveedor-alfa.TEST" }),
            true,
        ),
        (
            "otra dirección",
            json!({ "sender": "otra@proveedor-alfa.test" }),
            false,
        ),
        (
            "subdominio no vale",
            json!({ "sender_domain": "alfa.test" }),
            false,
        ),
        (
            "dominio que contiene al real",
            json!({ "sender_domain": "xproveedor-alfa.test" }),
            false,
        ),
    ];
    for (name, sender, acts) in cases {
        let dir = tempfile::tempdir().unwrap();
        let mut app = open(&dir, FakeProvider::new());
        let mut conditions = json!({ "category": "solicitud_factura" });
        conditions
            .as_object_mut()
            .unwrap()
            .extend(sender.as_object().unwrap().clone());
        app.create_rule("envio", "send_draft", &conditions).unwrap();
        app.set_rule_active("envio", true).unwrap();
        let states = mail_states(&mut app);
        assert!(!states.is_empty());
        let completed = states
            .iter()
            .filter(|(_, state, _)| *state == TaskState::Completed)
            .count();
        assert_eq!(completed > 0, acts, "{name}");
        for (from, state, _) in &states {
            // Un correo sin remitente (M-0006) nunca coincide con una regla con remitente.
            let own = from.as_deref() == Some("facturas@proveedor-alfa.test");
            assert_eq!(
                *state == TaskState::Completed,
                acts && own,
                "{name}: {from:?}"
            );
        }
        assert_eq!(app.simulators().0.sent().unwrap().len(), completed);
    }
}

#[test]
fn a_stored_send_rule_without_sender_never_automates_and_says_so() {
    let dir = tempfile::tempdir().unwrap();
    with_stored_rules(
        &dir,
        &[(
            "vieja",
            "send_draft",
            json!({ "category": "solicitud_factura" }),
        )],
    );
    let mut app = open(&dir, FakeProvider::new());
    let states = mail_states(&mut app);
    assert!(!states.is_empty());
    for (from, state, result) in &states {
        assert_eq!(*state, TaskState::AwaitingReview, "{from:?}");
        assert_eq!(result["evaluation"]["code"], "sender_required");
        assert_eq!(
            result["review_reason"],
            "la regla no indica remitente; añádalo o valide la opción avanzada (regla: vieja)"
        );
    }
    assert!(app.simulators().0.sent().unwrap().is_empty());
    // No se puede activar de nuevo sin remitente ni validación.
    app.set_rule_active("vieja", false).unwrap();
    let error = app.set_rule_active("vieja", true).unwrap_err().to_string();
    assert!(error.contains("necesita un remitente"), "{error}");
}

#[test]
fn the_advanced_option_is_per_rule_audited_and_erased_by_editing() {
    let dir = tempfile::tempdir().unwrap();
    with_stored_rules(
        &dir,
        &[
            (
                "vieja",
                "send_draft",
                json!({ "category": "solicitud_factura" }),
            ),
            ("otra", "send_draft", json!({ "category": "consulta_pago" })),
        ],
    );
    let mut app = open(&dir, FakeProvider::new());

    // La marca de otra regla no sirve: «vieja» sigue sin automatizar.
    app.set_rule_any_sender("otra", true).unwrap();
    let states = mail_states(&mut app);
    assert!(states
        .iter()
        .all(|(_, state, _)| *state == TaskState::AwaitingReview));

    // Con su propia marca, automatiza los correos con destinatario (en una base nueva, para
    // volver a procesar la misma bandeja).
    let dir2 = tempfile::tempdir().unwrap();
    with_stored_rules(
        &dir2,
        &[(
            "vieja",
            "send_draft",
            json!({ "category": "solicitud_factura" }),
        )],
    );
    let mut acked = open(&dir2, FakeProvider::new());
    acked.set_rule_any_sender("vieja", true).unwrap();
    let states = mail_states(&mut acked);
    let completed = states
        .iter()
        .filter(|(_, state, _)| *state == TaskState::Completed)
        .count();
    assert!(completed > 0);
    assert!(
        states
            .iter()
            .any(|(from, state, _)| from.is_none() && *state == TaskState::AwaitingReview),
        "el correo sin destinatario sigue en revisión"
    );

    // Queda en el historial, con su motivo.
    let audit = acked.audit_log().unwrap();
    let entry = audit
        .iter()
        .find(|e| e.action == "rule_any_sender" && e.status == "acknowledged")
        .expect("la validación consta en el historial");
    assert!(entry.detail.as_deref().unwrap().starts_with("vieja v"));
    assert!(entry
        .detail
        .as_deref()
        .unwrap()
        .contains("cualquier remitente"));

    // Editar las condiciones borra la marca: la regla deja de automatizar.
    let view = |app: &App| {
        app.rule_views()
            .unwrap()
            .into_iter()
            .find(|v| v.rule.id == "vieja")
            .unwrap()
    };
    assert!(view(&acked).any_sender_ack_at.is_some());
    acked
        .update_rule("vieja", &json!({ "category": "solicitud_factura" }))
        .unwrap();
    assert!(view(&acked).any_sender_ack_at.is_none());
    assert!(acked
        .audit_log()
        .unwrap()
        .iter()
        .any(|e| e.action == "rule_any_sender" && e.status == "revoked"));
    // Sin la marca ni remitente, tampoco se guarda otra edición sin remitente.
    assert!(acked
        .update_rule("vieja", &json!({ "category": "otros" }))
        .is_err());
    let again = acked.set_rule_any_sender("vieja", true).unwrap();
    assert!(again.any_sender_ack_at.is_some());
    // Una regla con remitente no admite la opción avanzada.
    acked
        .update_rule(
            "vieja",
            &json!({ "category": "otros", "sender": "a@alfa.test" }),
        )
        .unwrap();
    assert!(acked.set_rule_any_sender("vieja", true).is_err());
}

#[test]
fn creating_rules_generates_ids_from_names_and_the_advanced_option_is_explicit() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = open(&dir, FakeProvider::new());
    let conditions = json!({ "supplier": "Alfa SL" });
    let make = |app: &mut App, id: Option<&str>, name: Option<&str>| {
        app.create_rule_full(NewRule {
            id,
            name,
            action_kind: "register_invoice",
            conditions: &conditions,
            allow_any_sender: false,
        })
    };
    let first = make(&mut app, None, Some("Facturas de Alfa")).unwrap();
    assert_eq!(
        (first.rule.id.as_str(), first.name.as_str()),
        ("facturas-de-alfa", "Facturas de Alfa")
    );
    assert_eq!(first.rule.version, 1);
    assert!(!first.rule.active);
    let second = make(&mut app, None, Some("Facturas de Alfa")).unwrap();
    assert_eq!(second.rule.id, "facturas-de-alfa-2");
    // Con id y sin nombre: se muestra el id; los ids antiguos se conservan tal cual.
    let plain = make(&mut app, Some("vieja-regla"), None).unwrap();
    assert_eq!(
        (plain.rule.id.as_str(), plain.name.as_str()),
        ("vieja-regla", "vieja-regla")
    );
    assert!(make(&mut app, None, None).is_err());
    assert!(make(&mut app, Some("vieja-regla"), None).is_err());
    let renamed = app
        .rename_rule("vieja-regla", Some("La de siempre"))
        .unwrap();
    assert_eq!(renamed.name, "La de siempre");

    // La validación avanzada solo vale para envío sin remitente.
    let send = json!({ "category": "solicitud_factura" });
    let send_rule = |allow| NewRule {
        id: Some("envio"),
        name: None,
        action_kind: "send_draft",
        conditions: &send,
        allow_any_sender: allow,
    };
    assert!(app.create_rule_full(send_rule(false)).is_err());
    assert!(app.create_rule("envio", "send_draft", &send).is_err());
    let advanced = app.create_rule_full(send_rule(true)).unwrap();
    assert!(advanced.any_sender_ack_at.is_some());
    assert!(!advanced.rule.active);
    let on_invoice = NewRule {
        id: Some("x"),
        name: None,
        action_kind: "register_invoice",
        conditions: &conditions,
        allow_any_sender: true,
    };
    assert!(app.create_rule_full(on_invoice).is_err());
    let with_sender = json!({ "category": "solicitud_factura", "sender": "a@alfa.test" });
    let both = NewRule {
        id: Some("y"),
        name: None,
        action_kind: "send_draft",
        conditions: &with_sender,
        allow_any_sender: true,
    };
    assert!(app.create_rule_full(both).is_err());
    // Formato del remitente.
    for bad in [
        json!({ "category": "solicitud_factura", "sender": "no-es-una-direccion" }),
        json!({ "category": "solicitud_factura", "sender_domain": "@alfa.test" }),
        json!({ "category": "solicitud_factura", "sender": "a@alfa.test", "sender_domain": "alfa.test" }),
        json!({ "category": "solicitud_factura", "sender": 7 }),
    ] {
        assert!(app.create_rule("z", "send_draft", &bad).is_err(), "{bad}");
    }
}

#[test]
fn archived_rules_stay_inactive_and_cannot_be_activated_or_edited() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = open(&dir, FakeProvider::new());
    app.create_rule(
        "alfa",
        "register_invoice",
        &json!({ "supplier": "Alfa SL" }),
    )
    .unwrap();
    app.set_rule_active("alfa", true).unwrap();
    let archived = app.archive_rule("alfa").unwrap();
    assert!(!archived.rule.active);
    assert!(archived.archived_at.is_some());
    assert_eq!(
        archived.rule.version, 3,
        "desactivar crea versión: el historial la conserva"
    );

    for error in [
        app.set_rule_active("alfa", true).unwrap_err().to_string(),
        app.update_rule("alfa", &json!({ "supplier": "Beta SL" }))
            .unwrap_err()
            .to_string(),
    ] {
        assert!(error.contains("archivada"), "{error}");
    }
    // No automatiza.
    app.import_document("1.txt", &invoice("Alfa SL", "F-1", "121.00"))
        .unwrap();
    assert_eq!(
        app.process_next().unwrap().unwrap().state,
        TaskState::AwaitingReview
    );
    // Archivar de nuevo no hace nada; desarchivar la devuelve inactiva.
    assert_eq!(app.archive_rule("alfa").unwrap().rule.version, 3);
    let back = app.unarchive_rule("alfa").unwrap();
    assert!(!back.rule.active && back.archived_at.is_none());
    assert!(app.set_rule_active("alfa", true).unwrap().active);
}

#[test]
fn evaluations_signals_and_counters_are_recorded_once_and_without_mail_content() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = open(&dir, FakeProvider::new());
    app.create_rule(
        "alfa",
        "register_invoice",
        &json!({ "supplier": "Alfa SL", "max_total": "500.00" }),
    )
    .unwrap();
    app.set_rule_active("alfa", true).unwrap();
    app.create_rule(
        "envio",
        "send_draft",
        &json!({ "category": "solicitud_factura", "sender": "facturas@proveedor-alfa.test" }),
    )
    .unwrap();

    // Una factura por regla y otra a revisión; los correos a revisión (regla inactiva).
    app.import_document("1.txt", &invoice("Alfa SL", "F-1", "121.00"))
        .unwrap();
    app.import_document("2.txt", &invoice("Beta SL", "B-2", "121.00"))
        .unwrap();
    app.fetch_mail().unwrap();
    while app.process_next().unwrap().is_some() {}
    let tasks = app.tasks().unwrap();
    let by_document = |label: &str| {
        tasks
            .iter()
            .find(|t| t.result.as_ref().is_some_and(|r| r["document"] == label))
            .unwrap()
            .clone()
    };
    assert_eq!(by_document("1.txt").state, TaskState::Completed);
    let beta = by_document("2.txt");
    assert_eq!(beta.state, TaskState::AwaitingReview);

    // Evidencia: una fila por tarea evaluada, con el motivo y sin contenido.
    assert_eq!(
        count(&dir, "rule_evaluations"),
        tasks.iter().filter(|t| t.result.is_some()).count()
    );
    let evaluations = dump(&dir, &["rule_evaluations"])[0].1.join("\n");
    assert!(evaluations.contains("automated") && evaluations.contains("no_active_rule_matches"));

    // Aprobar la factura de revisión deja una señal sin edición; la regla no es señal.
    app.approve(beta.id, 1).unwrap();
    assert_eq!(count(&dir, "rule_signals"), 1);
    let signals = dump(&dir, &["rule_signals"])[0].1.join("\n");
    assert!(signals.contains("approved") && signals.contains("beta sl"));
    assert!(signals.contains("unedited"));

    // Correo: editar y aprobar deja `edited` y `approved` con edit_state `edited`.
    let mail = tasks
        .iter()
        .find(|t| {
            t.kind == EMAIL
                && t.result.as_ref().unwrap()["message"]["from"] == "facturas@proveedor-alfa.test"
        })
        .unwrap()
        .clone();
    app.edit_draft(mail.id, "Texto corregido por una persona.")
        .unwrap();
    app.approve(mail.id, 2).unwrap();
    let signals = dump(&dir, &["rule_signals"])[0].1.join("\n");
    assert!(signals.contains("|edited|send_draft"), "{signals}");
    assert!(signals.contains("|approved|send_draft"), "{signals}");
    assert!(signals.contains("solicitud_factura|facturas@proveedor-alfa.test"));
    assert_eq!(count(&dir, "rule_signals"), 3);

    // Contadores: la regla aplicó una factura.
    let stats = app.rule_stats().unwrap();
    let alfa = stats.iter().find(|s| s.rule_id == "alfa").unwrap();
    assert_eq!((alfa.applied, alfa.unclassified), (1, 0));
    assert!(alfa.last_applied_at.is_some());
    assert_eq!(app.rule_stats().unwrap(), stats);
    let envio = stats.iter().find(|s| s.rule_id == "envio").unwrap();
    assert_eq!(envio.applied, 0);

    // Ninguna tabla nueva contiene el cuerpo de un correo ni del borrador.
    let everything = dump(
        &dir,
        &[
            "rule_evaluations",
            "rule_signals",
            "rule_meta",
            "rule_suggestions",
            "rule_settings",
        ],
    )
    .into_iter()
    .flat_map(|(_, rows)| rows)
    .collect::<Vec<_>>()
    .join("\n");
    for private in [
        "Adjuntamos la factura FS-2026-014",
        "Texto corregido por una persona.",
        "Adjuntamos la factura solicitada.",
        "Recordatorio de renovación",
    ] {
        assert!(!everything.contains(private), "{private}");
    }
}

#[test]
fn a_retry_after_a_connector_failure_does_not_duplicate_evidence_or_counters() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = open(&dir, FakeProvider::new());
    app.create_rule(
        "alfa",
        "register_invoice",
        &json!({ "supplier": "Alfa SL" }),
    )
    .unwrap();
    app.set_rule_active("alfa", true).unwrap();
    let before = app.simulators().1.record_count().unwrap();
    app.simulators()
        .1
        .set_failure(Some(FailureMode::Unavailable));
    app.import_document("1.txt", &invoice("Alfa SL", "F-1", "121.00"))
        .unwrap();
    let failed = app.process_next().unwrap().unwrap();
    assert_eq!(failed.state, TaskState::Failed);
    assert_eq!(count(&dir, "rule_evaluations"), 1);
    assert_eq!(app.rule_stats().unwrap()[0].applied, 0);

    // El intento quedó sin resolver: al reprocesar vuelve a revisión (D50) y una persona
    // lo aprueba. Ni la evidencia ni los contadores se duplican, y esa aprobación no es una
    // señal porque la decisión fue de la regla.
    app.simulators().1.set_failure(None);
    app.retry(failed.id).unwrap();
    let review = app.process_next().unwrap().unwrap();
    assert_eq!(review.state, TaskState::AwaitingReview);
    let done = app.approve(failed.id, 1).unwrap();
    assert_eq!(done.state, TaskState::Completed);
    assert_eq!(
        count(&dir, "rule_evaluations"),
        1,
        "el reintento no repite la evidencia"
    );
    assert_eq!(
        count(&dir, "rule_signals"),
        0,
        "una ejecución por regla no es señal"
    );
    assert_eq!(app.simulators().1.record_count().unwrap(), before + 1);
    // El intento de la regla no llegó a ejecutarse y lo ejecutó una persona al aprobar: la
    // fila lo dice y la regla no cuenta como aplicada.
    let stats = app.rule_stats().unwrap();
    assert_eq!((stats[0].applied, stats[0].unclassified), (0, 0));
    let actions = dump(&dir, &["actions"])[0].1.clone();
    assert_eq!(actions.len(), 1);
    assert!(actions[0].contains("|aprobación|"), "{}", actions[0]);
    assert!(!actions[0].contains("|alfa|"), "{}", actions[0]);

    // Un fallo del modelo antes de evaluar no deja evidencia hasta que se reintenta.
    let provider = FakeProvider::new();
    let dir2 = tempfile::tempdir().unwrap();
    let mut app2 = open(&dir2, provider.clone());
    app2.import_document("2.txt", &invoice("Alfa SL", "F-2", "121.00"))
        .unwrap();
    *provider.fail.lock().unwrap() = true;
    let model_failed = app2.process_next().unwrap().unwrap();
    assert_eq!(model_failed.state, TaskState::Failed);
    assert_eq!(count(&dir2, "rule_evaluations"), 0);
    app2.retry(model_failed.id).unwrap();
    app2.process_next().unwrap();
    assert_eq!(count(&dir2, "rule_evaluations"), 1);
}

#[test]
fn a_rule_that_cannot_record_evidence_still_acts_once() {
    // Si la tabla de evidencia no se puede escribir, la acción sigue su curso una sola vez
    // y el fallo consta en el resultado de la tarea.
    let dir = tempfile::tempdir().unwrap();
    let mut app = open(&dir, FakeProvider::new());
    app.create_rule(
        "alfa",
        "register_invoice",
        &json!({ "supplier": "Alfa SL" }),
    )
    .unwrap();
    app.set_rule_active("alfa", true).unwrap();
    rusqlite::Connection::open(dir.path().join("nestadmin.db"))
        .unwrap()
        .execute_batch("DROP TABLE rule_evaluations;")
        .unwrap();
    let before = app.simulators().1.record_count().unwrap();
    app.import_document("1.txt", &invoice("Alfa SL", "F-1", "121.00"))
        .unwrap();
    let done = app.process_next().unwrap().unwrap();
    assert_eq!(done.state, TaskState::Completed);
    let result = done.result.unwrap();
    assert!(result["evidence_error"]
        .as_str()
        .unwrap()
        .contains("no se guardó la evaluación"));
    assert_eq!(app.simulators().1.record_count().unwrap(), before + 1);
}

#[test]
fn simulating_a_rule_writes_nothing_and_reports_overlaps_and_blocks() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = open(&dir, FakeProvider::new());
    app.create_rule(
        "alfa",
        "register_invoice",
        &json!({ "supplier": "Alfa SL" }),
    )
    .unwrap();
    app.set_rule_active("alfa", true).unwrap();
    // Cuatro facturas: dos limpias de Alfa (la primera sin euro), una con incidencia y una
    // de Beta; más los correos de ejemplo.
    for (name, text) in [
        ("1.txt", invoice("Alfa SL", "F-1", "121.00")),
        ("2.txt", invoice("Alfa SL", "F-2", "121.00")),
        ("3.txt", invoice("Alfa SL", "F-3", "130.00")),
        ("4.txt", invoice("Beta SL", "B-4", "121.00")),
        (
            "5.txt",
            invoice_note("Beta SL", "B-5", "121.00", "Total US$ 121.00"),
        ),
    ] {
        app.import_document(name, &text).unwrap();
    }
    while app.process_next().unwrap().is_some() {}
    app.fetch_mail().unwrap();
    while app.process_next().unwrap().is_some() {}

    let before = dump(&dir, &PANEL_TABLES);
    let conditions = json!({ "supplier": "Alfa SL", "max_total": "500.00" });
    let invoices = app
        .tasks()
        .unwrap()
        .into_iter()
        .filter(|t| t.kind == INVOICE)
        .count();
    let f2 = app
        .tasks()
        .unwrap()
        .into_iter()
        .find(|t| t.result.as_ref().is_some_and(|r| r["document"] == "2.txt"))
        .unwrap()
        .id;
    let sim = app
        .simulate_rule("register_invoice", &conditions, None, false, None)
        .unwrap();
    assert_eq!(sim.considered, invoices);
    assert!(!sim.truncated);
    assert_eq!(sim.skipped_without_result, 0);
    // «alfa» ya cubre las facturas de Alfa: la hipotética solapa con ella en la F-1 y la F-2
    // (limpias) y entonces «alfa» dejaría de actuar. La F-4 es de Beta y el adjunto de
    // ejemplo de otro proveedor: no coinciden. La F-3 coincide, pero su suma no cuadra y una
    // persona debe confirmar el importe (D120).
    assert_eq!(sim.would_automate, 0);
    assert_eq!(sim.overlaps, 2, "{sim:?}");
    assert_eq!(sim.no_match, invoices - 3, "{sim:?}");
    assert_eq!(sim.without_currency_check, 0);
    assert_eq!(sim.would_review.get("amount_review"), Some(&1), "{sim:?}");
    assert_eq!(sim.would_block_other.len(), 1, "{sim:?}");
    assert_eq!(sim.would_block_other[0].rule_id, "alfa");
    assert!(sim.would_block_other[0].task_ids.contains(&f2));
    assert_eq!(sim.would_block_other[0].task_ids.len(), 2);
    assert_eq!(sim.items.len(), invoices);
    assert!(sim.items.iter().all(|i| i.label.is_some()));
    // Una regla para un proveedor sin reglas automatizaría su factura limpia en euros, pero
    // no la que menciona dólares.
    let beta = app
        .simulate_rule(
            "register_invoice",
            &json!({ "supplier": "beta sl", "max_total": "500.00" }),
            None,
            false,
            None,
        )
        .unwrap();
    assert_eq!((beta.would_automate, beta.overlaps), (1, 0), "{beta:?}");
    assert!(beta.would_block_other.is_empty());

    // El límite recorta y lo dice.
    let small = app
        .simulate_rule("register_invoice", &conditions, Some(2), false, None)
        .unwrap();
    assert!(small.truncated && small.considered == 2, "{small:?}");
    assert!(app
        .simulate_rule("register_invoice", &conditions, Some(0), false, None)
        .is_err());
    assert!(app
        .simulate_rule(
            "register_invoice",
            &json!({ "colour": "x" }),
            None,
            false,
            None
        )
        .is_err());

    // Un tipo sin reglas: la hipotética automatizaría lo que coincide, sin solapes.
    let mail = app
        .simulate_rule(
            "send_draft",
            &json!({ "category": "solicitud_factura", "sender": "facturas@proveedor-alfa.test" }),
            None,
            false,
            None,
        )
        .unwrap();
    assert!(mail.would_automate >= 1);
    assert_eq!(mail.overlaps, 0);
    // Sin remitente (y sin validación): no automatizaría y se dice por qué.
    let anyone = app
        .simulate_rule(
            "send_draft",
            &json!({ "category": "solicitud_factura" }),
            None,
            false,
            None,
        )
        .unwrap();
    assert_eq!(anyone.would_automate, 0);
    assert!(
        anyone
            .would_review
            .get("sender_required")
            .copied()
            .unwrap_or(0)
            > 0
    );

    assert_eq!(
        dump(&dir, &PANEL_TABLES),
        before,
        "la simulación no escribe"
    );
    assert!(app.simulators().0.sent().unwrap().is_empty());
}

#[test]
fn rejecting_a_review_leaves_one_rejected_signal_and_other_states_leave_none() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = open(&dir, FakeProvider::new());
    app.import_document("1.txt", &invoice("Alfa SL", "F-1", "121.00"))
        .unwrap();
    let review = app.process_next().unwrap().unwrap();
    assert_eq!(review.state, TaskState::AwaitingReview);
    app.reject(review.id).unwrap();
    let signals = dump(&dir, &["rule_signals"])[0].1.join(
        "
",
    );
    assert!(
        signals.contains("rejected|register_invoice|alfa sl|unedited"),
        "{signals}"
    );
    assert_eq!(count(&dir, "rule_signals"), 1);
    // Una tarea que no estaba en revisión no deja señal al cancelarla.
    app.import_document("2.txt", &invoice("Alfa SL", "F-2", "121.00"))
        .unwrap();
    let queued = app
        .tasks()
        .unwrap()
        .into_iter()
        .find(|t| t.state == TaskState::Queued)
        .unwrap();
    app.reject(queued.id).unwrap();
    assert_eq!(count(&dir, "rule_signals"), 1);
}

#[test]
fn a_signal_that_cannot_be_saved_is_reported_without_blocking_the_action() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = open(&dir, FakeProvider::new());
    app.import_document("1.txt", &invoice("Alfa SL", "F-1", "121.00"))
        .unwrap();
    let review = app.process_next().unwrap().unwrap();
    rusqlite::Connection::open(dir.path().join("nestadmin.db"))
        .unwrap()
        .execute_batch("DROP TABLE rule_signals;")
        .unwrap();
    let before = app.simulators().1.record_count().unwrap();
    let done = app.approve(review.id, 1).unwrap();
    assert_eq!(done.state, TaskState::Completed);
    assert!(done.result.unwrap()["evidence_error"]
        .as_str()
        .unwrap()
        .contains("no se guardó la señal"));
    assert_eq!(app.simulators().1.record_count().unwrap(), before + 1);

    // Editar y rechazar también dejan constancia.
    app.fetch_mail().unwrap();
    while app.process_next().unwrap().is_some() {}
    let mail = app
        .tasks()
        .unwrap()
        .into_iter()
        .find(|t| t.kind == EMAIL && t.state == TaskState::AwaitingReview)
        .unwrap();
    let edited = app.edit_draft(mail.id, "Otro texto.").unwrap();
    assert!(edited.result.unwrap()["evidence_error"]
        .as_str()
        .unwrap()
        .contains("no se guardó la señal"));
    app.reject(mail.id).unwrap();
    let rejected = app
        .tasks()
        .unwrap()
        .into_iter()
        .find(|t| t.id == mail.id)
        .unwrap();
    assert_eq!(rejected.state, TaskState::Cancelled);
    assert!(rejected.result.unwrap()["evidence_error"].is_string());
}

// ---- Revisión del backend de E2 ----

fn signal_states(dir: &tempfile::TempDir) -> Vec<(String, i64, String)> {
    let conn = rusqlite::Connection::open(dir.path().join("nestadmin.db")).unwrap();
    let mut stmt = conn
        .prepare("SELECT kind, task_id, state FROM rule_signals ORDER BY id")
        .unwrap();
    stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

#[test]
fn an_approval_is_a_valid_signal_only_if_the_action_was_confirmed() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = open(&dir, FakeProvider::new());
    app.import_document("1.txt", &invoice("Alfa SL", "F-1", "121.00"))
        .unwrap();
    app.import_document("2.txt", &invoice("Beta SL", "B-2", "121.00"))
        .unwrap();
    let first = app.process_next().unwrap().unwrap();
    let second = app.process_next().unwrap().unwrap();

    // Aprobar con el conector caído: la tarea falla y la aprobación no vale.
    app.simulators()
        .1
        .set_failure(Some(FailureMode::Unavailable));
    let failed = app.approve(first.id, 1).unwrap();
    assert_eq!(failed.state, TaskState::Failed);
    assert_eq!(
        signal_states(&dir),
        [("approved".to_owned(), first.id, "void".to_owned())]
    );
    // Rechazar después tampoco deja una aprobación válida (ni un rechazo: ya no estaba en
    // revisión).
    app.reject(first.id).unwrap();
    assert_eq!(
        signal_states(&dir),
        [("approved".to_owned(), first.id, "void".to_owned())]
    );

    // Con el conector sano la aprobación queda válida, una sola vez.
    app.simulators().1.set_failure(None);
    let done = app.approve(second.id, 1).unwrap();
    assert_eq!(done.state, TaskState::Completed);
    assert_eq!(
        signal_states(&dir),
        [
            ("approved".to_owned(), first.id, "void".to_owned()),
            ("approved".to_owned(), second.id, "valid".to_owned()),
        ]
    );
}

#[test]
fn a_failed_approval_followed_by_a_successful_one_leaves_one_valid_signal() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = open(&dir, FakeProvider::new());
    app.import_document("1.txt", &invoice("Alfa SL", "F-1", "121.00"))
        .unwrap();
    let task = app.process_next().unwrap().unwrap();
    app.simulators()
        .1
        .set_failure(Some(FailureMode::Unavailable));
    assert_eq!(app.approve(task.id, 1).unwrap().state, TaskState::Failed);
    app.simulators().1.set_failure(None);
    app.retry(task.id).unwrap();
    let review = app.process_next().unwrap().unwrap();
    assert_eq!(review.state, TaskState::AwaitingReview);
    assert_eq!(app.approve(task.id, 1).unwrap().state, TaskState::Completed);
    assert_eq!(
        signal_states(&dir),
        [("approved".to_owned(), task.id, "valid".to_owned())]
    );
}

#[test]
fn shortcuts_still_count_the_rules_that_would_have_matched() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = open(&dir, FakeProvider::new());
    app.create_rule(
        "alfa",
        "register_invoice",
        &json!({ "supplier": "Alfa SL" }),
    )
    .unwrap();
    app.set_rule_active("alfa", true).unwrap();
    // La suma no cuadra: importe dudoso (D120), siempre a revisión, pero «alfa» coincidía.
    app.import_document("1.txt", &invoice("Alfa SL", "F-1", "130.00"))
        .unwrap();
    let task = app.process_next().unwrap().unwrap();
    assert_eq!(task.state, TaskState::AwaitingReview);
    let result = task.result.unwrap();
    assert_eq!(result["evaluation"]["code"], "amount_review");
    assert_eq!(result["evaluation"]["candidates"][0]["id"], "alfa");
    assert_eq!(result["evaluation"]["candidates"][0]["matches"], true);
    let stats = app.rule_stats().unwrap();
    assert_eq!(stats[0].reviewed, 1);
    assert_eq!(stats[0].applied, 0);
}

#[test]
fn archiving_a_rule_withdraws_the_any_sender_option_and_unarchiving_does_not_restore_it() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = open(&dir, FakeProvider::new());
    let created = app
        .create_rule_full(NewRule {
            id: Some("vieja"),
            name: None,
            action_kind: "send_draft",
            conditions: &json!({ "category": "solicitud_factura" }),
            allow_any_sender: true,
        })
        .unwrap();
    assert!(created.any_sender_ack_at.is_some());
    let archived = app.archive_rule("vieja").unwrap();
    assert!(archived.any_sender_ack_at.is_none());
    let back = app.unarchive_rule("vieja").unwrap();
    assert!(back.any_sender_ack_at.is_none() && back.archived_at.is_none());
    // Sin la marca no se puede activar una regla sin remitente.
    assert!(app.set_rule_active("vieja", true).is_err());
    let log = app.audit_log().unwrap();
    assert!(
        log.iter().any(|e| e.action == "rule_any_sender"
            && e.status == "revoked"
            && e.detail
                .as_deref()
                .is_some_and(|d| d.contains("se archivó"))),
        "{log:?}"
    );
}

#[test]
fn simulating_with_the_any_sender_hypothesis_never_writes_the_mark() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = open(&dir, FakeProvider::new());
    app.fetch_mail().unwrap();
    while app.process_next().unwrap().is_some() {}
    let conditions = json!({ "category": "solicitud_factura" });
    let without = app
        .simulate_rule("send_draft", &conditions, None, false, None)
        .unwrap();
    assert_eq!(without.would_automate, 0);
    assert!(
        without
            .would_review
            .get("sender_required")
            .copied()
            .unwrap_or(0)
            > 0
    );
    let before = dump(&dir, &["rule_meta", "audit_log", "rules"]);
    let assumed = app
        .simulate_rule("send_draft", &conditions, None, true, None)
        .unwrap();
    assert!(assumed.would_automate >= 1, "{assumed:?}");
    assert_eq!(assumed.would_review.get("sender_required"), None);
    assert_eq!(dump(&dir, &["rule_meta", "audit_log", "rules"]), before);
}
