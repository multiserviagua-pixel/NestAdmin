//! T03g (D100, D101): mientras la IA no esté utilizable, las tareas que la necesitan esperan
//! en `queued` sin gastar intentos y sin llamar a ningún proveedor; las que no la necesitan
//! (reanudar una acción ya intentada, una factura ya separada) siguen. Una clave rechazada por
//! el proveedor devuelve la tarea a la cola y pide corregirla; un fallo `unauthorized` del
//! conector de correo sigue siendo un fallo. Datos sintéticos y un proveedor simulado.

use std::collections::HashSet;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use nestadmin_ai::{
    AiError, GenerateRequest, GenerateResult, Provider, ProviderKind, Task as AiTask, Usage,
};
use nestadmin_app::{AiWait, App, AppPaths, EMAIL, INVOICE};
use nestadmin_core::{Task, TaskState};
use nestadmin_extract::{ExtractError, Extractor, Ocr};
use nestadmin_simulators::FailureMode;
use serde_json::{json, Value};

#[derive(Default)]
struct FakeProvider {
    calls: AtomicUsize,
    unauthorized: AtomicBool,
    /// Identificadores de solicitud (el de la tarea) que fallan con `unavailable`.
    fail_for: Mutex<HashSet<String>>,
}

impl Provider for FakeProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::OllamaCloud
    }
    fn model(&self) -> &str {
        "simulado"
    }
    fn generate(&self, request: &GenerateRequest) -> Result<GenerateResult, AiError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.unauthorized.load(Ordering::SeqCst) {
            return Err(AiError::Unauthorized);
        }
        if self.fail_for.lock().unwrap().contains(&request.request_id) {
            return Err(AiError::Unavailable("sin conexión".into()));
        }
        let output = match request.task {
            AiTask::Extraction => serde_json::from_str(&request.content).unwrap_or_else(|_| {
                json!({
                    "supplier": "Proveedor Alfa Suministros SL", "invoice_number": "FS-2026-014",
                    "date": "2026-10-01", "subtotal": "120.00", "tax": "25.20", "total": "145.20",
                })
            }),
            _ => json!({ "category": "solicitud_factura", "draft": "Adjuntamos la factura." }),
        };
        Ok(GenerateResult {
            request_id: request.request_id.clone(),
            provider: ProviderKind::OllamaCloud,
            model: "simulado".into(),
            output,
            latency_ms: 1,
            usage: Usage::default(),
        })
    }
}

struct NoOcr;

impl Ocr for NoOcr {
    fn recognize(&self, _image: &Path, page: usize) -> Result<String, ExtractError> {
        Err(ExtractError::OcrUnavailable(format!("página {page}")))
    }
}

fn open(dir: &tempfile::TempDir, provider: &Arc<FakeProvider>) -> App {
    let extractor = Extractor::new(dir.path().join("sin-pdfium"), Box::new(NoOcr));
    App::open(
        &AppPaths::new(dir.path()),
        Arc::clone(provider) as Arc<dyn Provider>,
        Arc::new(extractor),
    )
    .unwrap()
}

fn invoice(supplier: &str, number: &str, day: u32) -> Value {
    json!({
        "supplier": supplier, "invoice_number": number, "date": format!("2026-10-{day:02}"),
        "subtotal": "100.00", "tax": "21.00", "total": "121.00",
    })
}

fn task(app: &App, id: i64) -> Task {
    app.tasks()
        .unwrap()
        .into_iter()
        .find(|t| t.id == id)
        .unwrap()
}

fn message_task(app: &App, message_id: &str) -> Task {
    app.tasks()
        .unwrap()
        .into_iter()
        .find(|t| {
            t.kind == EMAIL
                && serde_json::from_str::<Value>(&t.input_ref).unwrap()["message_id"] == message_id
        })
        .unwrap()
}

#[test]
fn t03g_ai_tasks_wait_queued_without_attempts_while_a_resume_task_runs() {
    let dir = tempfile::tempdir().unwrap();
    let provider = Arc::new(FakeProvider::default());
    let mut app = open(&dir, &provider);

    // Correos: uno falla (queda para reintentar y necesita la IA) y otro se aprueba con el
    // envío caído, de modo que su reintento solo reanuda la acción ya intentada (D50).
    app.fetch_mail().unwrap();
    let resume_id = message_task(&app, "simulador:M-0002").id;
    let to_fail = app
        .tasks()
        .unwrap()
        .into_iter()
        .filter(|t| t.kind == EMAIL && t.id != resume_id)
        .map(|t| t.id)
        .min()
        .unwrap();
    provider
        .fail_for
        .lock()
        .unwrap()
        .insert(to_fail.to_string());
    while app.process_next().unwrap().is_some() {}
    assert_eq!(task(&app, to_fail).state, TaskState::Failed);
    app.simulators()
        .0
        .set_failure(Some(FailureMode::Unavailable));
    assert_eq!(app.approve(resume_id, 1).unwrap().state, TaskState::Failed);
    app.simulators().0.set_failure(None);
    provider.fail_for.lock().unwrap().clear();
    app.retry(to_fail).unwrap();
    app.retry(resume_id).unwrap();
    let plain = app
        .import_document("sola.txt", &invoice("Beta SA", "B-1", 9).to_string())
        .unwrap();
    assert_eq!(task(&app, plain.id).kind, INVOICE);

    let calls_before = provider.calls.load(Ordering::SeqCst);
    app.set_ai_wait(Some(AiWait::NeedsKey));
    assert_eq!(app.ai_wait(), Some(AiWait::NeedsKey));

    // Se reanuda la acción ya intentada, sin la IA; las demás esperan.
    let resumed = app.process_next().unwrap().unwrap();
    assert_eq!(resumed.id, resume_id);
    assert_eq!(resumed.state, TaskState::AwaitingReview);
    assert!(app.process_next().unwrap().is_none());
    assert_eq!(
        provider.calls.load(Ordering::SeqCst),
        calls_before,
        "ni una llamada"
    );
    for id in [to_fail, plain.id] {
        assert_eq!(task(&app, id).state, TaskState::Queued, "{id}");
    }
    assert_eq!(task(&app, plain.id).attempts, 0, "sin gastar intentos");
    assert_eq!(task(&app, to_fail).attempts, 1, "conserva los anteriores");
    assert!(task(&app, plain.id).error.is_none());

    // Al quitar la espera, las tareas con IA se retoman.
    app.set_ai_wait(None);
    let mut finished = Vec::new();
    while let Some(done) = app.process_next().unwrap() {
        finished.push(done.id);
    }
    for id in [to_fail, plain.id] {
        assert!(finished.contains(&id), "{id}");
        assert_eq!(task(&app, id).state, TaskState::AwaitingReview);
    }
    assert!(provider.calls.load(Ordering::SeqCst) > calls_before);
}

#[test]
fn t03g_a_pending_split_invoice_runs_while_the_ai_waits() {
    let dir = tempfile::tempdir().unwrap();
    let provider = Arc::new(FakeProvider::default());
    let mut app = open(&dir, &provider);
    let mut both = invoice("Alfa SL", "F-3", 3);
    both["other_invoices"] = json!([invoice("Alfa SL", "F-4", 4)]);
    app.import_document("dos.txt", &both.to_string()).unwrap();
    let first = app.process_next().unwrap().unwrap();
    let plain = app
        .import_document("sola.txt", &invoice("Beta SA", "B-1", 9).to_string())
        .unwrap();
    let split = app
        .tasks()
        .unwrap()
        .into_iter()
        .find(|t| t.id != first.id && t.id != plain.id)
        .unwrap();
    assert_eq!(split.state, TaskState::Queued);

    let calls = provider.calls.load(Ordering::SeqCst);
    app.set_ai_wait(Some(AiWait::NeedsConsent));
    // La separada es más antigua que la factura sola y no necesita la IA.
    let done = app.process_next().unwrap().unwrap();
    assert_eq!(done.id, split.id);
    assert_eq!(done.state, TaskState::AwaitingReview);
    assert!(app.process_next().unwrap().is_none());
    assert_eq!(
        provider.calls.load(Ordering::SeqCst),
        calls,
        "ni una llamada"
    );
    assert_eq!(task(&app, plain.id).state, TaskState::Queued);
    assert_eq!(task(&app, plain.id).attempts, 0);
}

#[test]
fn t03g_a_rejected_key_releases_the_task_without_spending_an_attempt() {
    let dir = tempfile::tempdir().unwrap();
    let provider = Arc::new(FakeProvider::default());
    let mut app = open(&dir, &provider);
    let one = app
        .import_document("uno.txt", &invoice("Alfa SL", "F-1", 1).to_string())
        .unwrap();
    let two = app
        .import_document("dos.txt", &invoice("Beta SA", "B-2", 2).to_string())
        .unwrap();

    provider.unauthorized.store(true, Ordering::SeqCst);
    let released = app.process_next().unwrap().unwrap();
    assert_eq!(released.id, one.id);
    assert_eq!(released.state, TaskState::Queued, "no falla");
    assert_eq!(released.attempts, 0, "no gasta intento");
    assert!(released.error.is_none());
    assert_eq!(app.ai_wait(), Some(AiWait::InvalidKey));
    assert!(AiWait::InvalidKey
        .message()
        .contains("no es válida o fue revocada"));
    // Con la espera fijada no se vuelve a llamar al proveedor.
    assert!(app.process_next().unwrap().is_none());
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    assert_eq!(task(&app, two.id).state, TaskState::Queued);

    // Corregida la clave, todo sigue y la primera ya cuenta su primer intento.
    provider.unauthorized.store(false, Ordering::SeqCst);
    app.set_ai_wait(None);
    let done = app.process_next().unwrap().unwrap();
    assert_eq!(done.id, one.id);
    assert_eq!((done.state, done.attempts), (TaskState::AwaitingReview, 1));
}

#[test]
fn t03g_wait_serializes_in_snake_case() {
    assert_eq!(
        serde_json::to_value(AiWait::NeedsChoice).unwrap(),
        "needs_choice"
    );
    assert_eq!(serde_json::to_value(AiWait::NeedsKey).unwrap(), "needs_key");
    assert_eq!(
        serde_json::to_value(AiWait::NeedsConsent).unwrap(),
        "needs_consent"
    );
    assert_eq!(
        serde_json::to_value(AiWait::InvalidKey).unwrap(),
        "invalid_key"
    );
    for wait in [AiWait::NeedsChoice, AiWait::NeedsKey, AiWait::NeedsConsent] {
        assert!(
            wait.message().starts_with("Falta configurar la IA"),
            "{wait:?}"
        );
    }
}
