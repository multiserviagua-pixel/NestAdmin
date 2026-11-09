//! D148: una tarea retenida por su pipeline no sale de `begin_next` por ninguno de sus dos
//! caminos (normal y con la IA en espera), no gasta intentos y no frena a las demás. En esta
//! fase nada crea ejecuciones: los tests las insertan por SQL. Datos sintéticos.

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use nestadmin_ai::{
    AiError, GenerateRequest, GenerateResult, Provider, ProviderKind, Task as AiTask, Usage,
};
use nestadmin_app::{AiWait, App, AppPaths, EMAIL};
use nestadmin_core::{Task, TaskState};
use nestadmin_extract::{ExtractError, Extractor, Ocr};
use nestadmin_simulators::FailureMode;
use serde_json::{json, Value};

#[derive(Default)]
struct FakeProvider {
    calls: AtomicUsize,
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

/// SOLO PRUEBA EL PREDICADO de selección de `begin_next`. Monta por SQL un estado que la API
/// pública no permite (enlazar a una ejecución tareas ya existentes, incluso con intentos o con
/// una acción pendiente): las plantillas no son retroactivas (D148) y `enqueue_with_run`,
/// `enqueue_for_step` y `add_step_with_task` solo crean tareas nuevas. Que la fase 3 no enlace
/// tareas existentes se prueba allí, con la aplicación real.
/// Abre una ejecución en la que `blocked` (etapa 1) espera a `blocker` (etapa 0).
/// Devuelve el id del paso de `blocker`.
fn block(dir: &tempfile::TempDir, blocker: i64, blocked: i64) -> i64 {
    let conn = rusqlite::Connection::open(dir.path().join("nestadmin.db")).unwrap();
    conn.execute(
        "INSERT INTO pipeline_runs (template_id, template_version, steps_json, subject_task_id, created_at)
         VALUES ('t', 1, '[]', ?1, 1)",
        [blocked],
    )
    .unwrap();
    let run = conn.last_insert_rowid();
    for (stage, task_id) in [(0, blocker), (1, blocked)] {
        conn.execute(
            "INSERT INTO pipeline_steps (run_id, stage, kind, label, task_id) VALUES (?1, ?2, 'x', 'paso', ?3)",
            rusqlite::params![run, stage, task_id],
        )
        .unwrap();
    }
    conn.query_row(
        "SELECT id FROM pipeline_steps WHERE task_id = ?1",
        [blocker],
        |r| r.get(0),
    )
    .unwrap()
}

fn waive(dir: &tempfile::TempDir, step: i64) {
    rusqlite::Connection::open(dir.path().join("nestadmin.db"))
        .unwrap()
        .execute(
            "UPDATE pipeline_steps SET waived_at = 1, waived_by_note = 'prueba' WHERE id = ?1",
            [step],
        )
        .unwrap();
}

#[test]
fn normal_path_skips_a_blocked_head_and_waits_for_the_step_to_complete() {
    let dir = tempfile::tempdir().unwrap();
    let provider = Arc::new(FakeProvider::default());
    let mut app = open(&dir, &provider);
    // La bloqueada es la más antigua; su paso previo, la siguiente.
    let blocked = app
        .import_document("b.txt", &invoice("Beta SA", "B-1", 2).to_string())
        .unwrap();
    let blocker = app
        .import_document("a.txt", &invoice("Alfa SL", "A-1", 1).to_string())
        .unwrap();
    block(&dir, blocker.id, blocked.id);

    let first = app.process_next().unwrap().unwrap();
    assert_eq!(first.id, blocker.id, "la cabeza bloqueada no atasca");
    assert_eq!(first.state, TaskState::AwaitingReview);
    // En revisión no desbloquea: la bloqueada no sale ni gasta intentos.
    assert!(app.process_next().unwrap().is_none());
    let held = task(&app, blocked.id);
    assert_eq!((held.state, held.attempts), (TaskState::Queued, 0));

    // Completado el paso (aprobado y registrado), la bloqueada sale.
    assert_eq!(
        app.approve(blocker.id, 1).unwrap().state,
        TaskState::Completed
    );
    let second = app.process_next().unwrap().unwrap();
    assert_eq!(second.id, blocked.id);
    assert_eq!(second.state, TaskState::AwaitingReview);
    assert_eq!(second.attempts, 1);
}

#[test]
fn ai_wait_path_does_not_resume_a_blocked_split_invoice() {
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
    let step = block(&dir, plain.id, split.id);

    let calls = provider.calls.load(Ordering::SeqCst);
    app.set_ai_wait(Some(AiWait::NeedsConsent));
    // Sin la IA, la separada es lo único que podría salir, pero está retenida por `plain`.
    assert!(app.process_next().unwrap().is_none());
    let held = task(&app, split.id);
    assert_eq!((held.state, held.attempts), (TaskState::Queued, 0));
    assert_eq!(
        provider.calls.load(Ordering::SeqCst),
        calls,
        "ni una llamada"
    );

    // Con el paso omitido, la misma tarea sale por el mismo camino.
    waive(&dir, step);
    let done = app.process_next().unwrap().unwrap();
    assert_eq!(done.id, split.id);
    assert_eq!(done.state, TaskState::AwaitingReview);
}

#[test]
fn ai_wait_path_does_not_resume_a_blocked_attempted_action() {
    let dir = tempfile::tempdir().unwrap();
    let provider = Arc::new(FakeProvider::default());
    let mut app = open(&dir, &provider);
    app.fetch_mail().unwrap();
    let resume_id = app
        .tasks()
        .unwrap()
        .into_iter()
        .find(|t| {
            t.kind == EMAIL
                && serde_json::from_str::<Value>(&t.input_ref).unwrap()["message_id"]
                    == "simulador:M-0002"
        })
        .unwrap()
        .id;
    while app.process_next().unwrap().is_some() {}
    // Se aprueba con el envío caído: queda un intento registrado que solo hay que reanudar.
    app.simulators()
        .0
        .set_failure(Some(FailureMode::Unavailable));
    assert_eq!(app.approve(resume_id, 1).unwrap().state, TaskState::Failed);
    app.simulators().0.set_failure(None);
    app.retry(resume_id).unwrap();
    let plain = app
        .import_document("sola.txt", &invoice("Beta SA", "B-1", 9).to_string())
        .unwrap();
    let step = block(&dir, plain.id, resume_id);

    let calls = provider.calls.load(Ordering::SeqCst);
    app.set_ai_wait(Some(AiWait::NeedsKey));
    assert!(
        app.process_next().unwrap().is_none(),
        "la reanudación espera"
    );
    let held = task(&app, resume_id);
    assert_eq!(held.state, TaskState::Queued);
    assert_eq!(provider.calls.load(Ordering::SeqCst), calls);

    waive(&dir, step);
    let resumed = app.process_next().unwrap().unwrap();
    assert_eq!(resumed.id, resume_id);
    assert_eq!(resumed.state, TaskState::AwaitingReview);
    assert_eq!(provider.calls.load(Ordering::SeqCst), calls);
}
