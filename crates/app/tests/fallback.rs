//! T03g y T03h (D101, D102): respaldo del proveedor externo elegido a la IA local. Proveedores
//! simulados con tipo y error configurables y un espía que cuenta cada llamada, datos sintéticos.
//! Solo se admite externo a local: nunca externo a externo ni local a externo, y `unauthorized`
//! e `invalid_response` no lo activan.

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use nestadmin_ai::{
    AiError, GenerateRequest, GenerateResult, Provider, ProviderKind, Task as AiTask, Usage,
};
use nestadmin_app::{AiWait, App, AppPaths, LocalFallback};
use nestadmin_core::{Task, TaskState};
use nestadmin_extract::{ExtractError, Extractor, Ocr};
use serde_json::{json, Value};

/// Proveedor simulado: responde con el JSON de la factura o falla con el error configurado.
struct Fake {
    kind: ProviderKind,
    error: Mutex<Option<AiError>>,
    calls: AtomicUsize,
}

impl Fake {
    fn new(kind: ProviderKind, error: Option<AiError>) -> Arc<Self> {
        Arc::new(Self {
            kind,
            error: Mutex::new(error),
            calls: AtomicUsize::new(0),
        })
    }
    fn set_error(&self, error: Option<AiError>) {
        *self.error.lock().unwrap() = error;
    }
    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl Provider for Fake {
    fn kind(&self) -> ProviderKind {
        self.kind
    }
    fn model(&self) -> &str {
        "simulado"
    }
    fn generate(&self, request: &GenerateRequest) -> Result<GenerateResult, AiError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if let Some(error) = self.error.lock().unwrap().clone() {
            return Err(error);
        }
        let output = match request.task {
            AiTask::Extraction => {
                serde_json::from_str(nestadmin_ai::tasks::document_text(&request.content)).unwrap()
            }
            _ => json!({ "category": "solicitud_factura", "draft": "Adjuntamos la factura." }),
        };
        Ok(GenerateResult {
            request_id: request.request_id.clone(),
            provider: self.kind,
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

/// La aplicación con el externo y el respaldo local indicados, más el contador de veces que
/// se pidió el respaldo.
struct World {
    app: App,
    external: Arc<Fake>,
    local: Arc<Fake>,
    asked: Arc<AtomicUsize>,
    _dir: tempfile::TempDir,
}

fn world(external_kind: ProviderKind, external_error: Option<AiError>) -> World {
    let dir = tempfile::tempdir().unwrap();
    let external = Fake::new(external_kind, external_error);
    let local = Fake::new(ProviderKind::Ollama, None);
    let extractor = Extractor::new(dir.path().join("sin-pdfium"), Box::new(NoOcr));
    let mut app = App::open(
        &AppPaths::new(dir.path()),
        Arc::clone(&external) as Arc<dyn Provider>,
        Arc::new(extractor),
    )
    .unwrap();
    let asked = Arc::new(AtomicUsize::new(0));
    let (spy, provider) = (Arc::clone(&asked), Arc::clone(&local));
    let fallback: LocalFallback = Arc::new(move || {
        spy.fetch_add(1, Ordering::SeqCst);
        Some(Arc::clone(&provider) as Arc<dyn Provider>)
    });
    app.set_local_fallback(Some(fallback));
    World {
        app,
        external,
        local,
        asked,
        _dir: dir,
    }
}

fn invoice(number: &str, day: u32) -> String {
    json!({
        "supplier": "PROVEEDOR-SINTETICO-XYZ", "invoice_number": number,
        "date": format!("2026-10-{day:02}"), "subtotal": "100.00", "tax": "21.00",
        "total": "121.00",
    })
    .to_string()
}

fn run(app: &mut App, number: &str, day: u32) -> Task {
    app.import_document(&format!("{number}.txt"), &invoice(number, day))
        .unwrap();
    app.process_next().unwrap().unwrap()
}

fn ai_fallback_events(app: &App) -> Vec<Value> {
    app.audit_log()
        .unwrap()
        .into_iter()
        .filter(|e| e.action == "ai_fallback")
        .map(|e| {
            assert_eq!(e.status, "used");
            serde_json::from_str(&e.detail.unwrap()).unwrap()
        })
        .collect()
}

#[test]
fn t03h_unavailable_timeout_and_rate_limited_retry_once_with_the_local_provider() {
    for (error, reason) in [
        (AiError::Unavailable("sin conexión".into()), "unavailable"),
        (AiError::Timeout, "timeout"),
        (
            AiError::RateLimited {
                retry_after_ms: Some(1000),
            },
            "rate_limited",
        ),
    ] {
        let mut w = world(ProviderKind::OllamaCloud, Some(error));
        let task = run(&mut w.app, "F-1", 1);
        assert_eq!(task.state, TaskState::AwaitingReview, "{reason}: {task:?}");
        let result = task.result.unwrap();
        assert_eq!(
            result["fallback"],
            json!({ "from": "ollama_cloud", "reason": reason })
        );
        assert_eq!(result["provider"], "ollama", "respondió el local");
        assert_eq!((w.external.calls(), w.local.calls()), (1, 1), "{reason}");
        assert_eq!(
            ai_fallback_events(&w.app),
            vec![json!({ "from": "ollama_cloud", "reason": reason })]
        );
        // Sin contenido en el historial: ni el proveedor de la factura ni el número.
        let history = serde_json::to_string(&w.app.audit_log().unwrap()).unwrap();
        assert!(!history.contains("PROVEEDOR-SINTETICO") && !history.contains("F-1"));
        // Solo `unavailable` abre el periodo de 10 minutos (D101).
        assert_eq!(
            w.app.fallback_period_active(),
            reason == "unavailable",
            "{reason}"
        );
    }
}

#[test]
fn t03h_unauthorized_and_invalid_response_never_use_the_fallback() {
    let mut w = world(ProviderKind::Anthropic, Some(AiError::Unauthorized));
    let task = run(&mut w.app, "F-1", 1);
    assert_eq!(task.state, TaskState::Queued, "la clave pide corrección");
    assert_eq!(w.app.ai_wait(), Some(AiWait::InvalidKey));
    assert_eq!((w.external.calls(), w.local.calls()), (1, 0));
    assert_eq!(w.asked.load(Ordering::SeqCst), 0, "ni se pide el local");

    let mut w = world(
        ProviderKind::OpenAiCompatible,
        Some(AiError::InvalidResponse("no es JSON".into())),
    );
    let task = run(&mut w.app, "F-1", 1);
    assert_eq!(task.state, TaskState::Failed);
    assert_eq!(task.error.unwrap().code, "invalid_response");
    assert_eq!((w.external.calls(), w.local.calls()), (1, 0));
    assert_eq!(w.asked.load(Ordering::SeqCst), 0);
    assert!(ai_fallback_events(&w.app).is_empty());
    assert!(!w.app.fallback_period_active());
}

#[test]
fn t03h_after_unavailable_the_next_tasks_go_straight_to_local_until_the_period_ends() {
    let mut w = world(
        ProviderKind::OllamaCloud,
        Some(AiError::Unavailable("sin conexión".into())),
    );
    run(&mut w.app, "F-1", 1);
    assert!(w.app.fallback_period_active());
    assert_eq!((w.external.calls(), w.local.calls()), (1, 1));

    // Dentro del periodo no se vuelve a llamar al externo caído.
    let second = run(&mut w.app, "F-2", 2);
    assert_eq!((w.external.calls(), w.local.calls()), (1, 2));
    assert_eq!(
        second.result.unwrap()["fallback"],
        json!({ "from": "ollama_cloud", "reason": "unavailable" })
    );

    // Pasados los 10 minutos se vuelve al externo, que ya responde.
    w.app.expire_fallback_period();
    assert!(!w.app.fallback_period_active());
    w.external.set_error(None);
    let third = run(&mut w.app, "F-3", 3);
    assert_eq!((w.external.calls(), w.local.calls()), (2, 2));
    let result = third.result.unwrap();
    assert!(result.get("fallback").is_none(), "{result}");
    assert_eq!(result["provider"], "ollama_cloud");
    assert_eq!(ai_fallback_events(&w.app).len(), 2);

    // Cambiar de proveedor cierra el periodo.
    w.external
        .set_error(Some(AiError::Unavailable("otra vez".into())));
    run(&mut w.app, "F-4", 4);
    assert!(w.app.fallback_period_active());
    w.app
        .select_provider(Fake::new(ProviderKind::Anthropic, None) as Arc<dyn Provider>);
    assert!(!w.app.fallback_period_active());
}

#[test]
fn t03h_rate_limited_and_timeout_open_no_period() {
    for error in [
        AiError::RateLimited {
            retry_after_ms: None,
        },
        AiError::Timeout,
    ] {
        let mut w = world(ProviderKind::OllamaCloud, Some(error));
        run(&mut w.app, "F-1", 1);
        assert!(!w.app.fallback_period_active());
        run(&mut w.app, "F-2", 2);
        assert_eq!(w.external.calls(), 2, "cada tarea prueba antes el externo");
        assert_eq!(w.local.calls(), 2);
    }
}

#[test]
fn t03h_without_a_ready_local_provider_the_external_error_stands() {
    let mut w = world(
        ProviderKind::OllamaCloud,
        Some(AiError::Unavailable("sin conexión".into())),
    );
    let none: LocalFallback = Arc::new(|| None);
    w.app.set_local_fallback(Some(none));
    let task = run(&mut w.app, "F-1", 1);
    assert_eq!(task.state, TaskState::Failed);
    let error = task.error.unwrap();
    assert_eq!(error.code, "unavailable");
    assert!(!error.message.contains("respaldo"), "{}", error.message);
    assert_eq!((w.external.calls(), w.local.calls()), (1, 0));
    assert!(!w.app.fallback_period_active());

    // Con el respaldo desactivado (`local_fallback` apagado) tampoco se pide el local.
    let mut w = world(
        ProviderKind::OllamaCloud,
        Some(AiError::Unavailable("sin conexión".into())),
    );
    w.app.set_local_fallback(None);
    let task = run(&mut w.app, "F-1", 1);
    assert_eq!(task.error.unwrap().code, "unavailable");
    assert_eq!(w.asked.load(Ordering::SeqCst), 0);
    assert_eq!(w.local.calls(), 0);
}

#[test]
fn t03h_when_both_fail_the_task_fails_with_both_errors() {
    let mut w = world(
        ProviderKind::Anthropic,
        Some(AiError::RateLimited {
            retry_after_ms: None,
        }),
    );
    w.local
        .set_error(Some(AiError::Unavailable("Ollama local detenido".into())));
    let task = run(&mut w.app, "F-1", 1);
    assert_eq!(task.state, TaskState::Failed);
    assert_eq!(task.attempts, 1);
    let error = task.error.unwrap();
    assert_eq!(error.code, "rate_limited", "el código es el del externo");
    assert!(error.recoverable);
    assert!(
        error.message.contains("límite") || error.message.contains("rate"),
        "{}",
        error.message
    );
    assert!(
        error.message.contains("respaldo local"),
        "{}",
        error.message
    );
    assert!(
        error.message.contains("Ollama local detenido"),
        "{}",
        error.message
    );
    assert_eq!(
        (w.external.calls(), w.local.calls()),
        (1, 1),
        "un solo reintento"
    );
    assert!(
        ai_fallback_events(&w.app).is_empty(),
        "no se usó el respaldo"
    );
    assert!(!w.app.fallback_period_active());
}

#[test]
fn t03h_a_fallback_that_is_not_the_local_provider_is_never_called() {
    // Aunque quien configura devuelva por error otro proveedor externo, no se le envía nada.
    let mut w = world(
        ProviderKind::OllamaCloud,
        Some(AiError::Unavailable("sin conexión".into())),
    );
    let other_external = Fake::new(ProviderKind::Anthropic, None);
    let other = Arc::clone(&other_external);
    let wrong: LocalFallback = Arc::new(move || Some(Arc::clone(&other) as Arc<dyn Provider>));
    w.app.set_local_fallback(Some(wrong));
    let task = run(&mut w.app, "F-1", 1);
    assert_eq!(task.state, TaskState::Failed);
    assert_eq!(task.error.unwrap().code, "unavailable");
    assert_eq!(other_external.calls(), 0, "ningún segundo externo");
    assert_eq!(w.local.calls(), 0);
}

#[test]
fn t03h_the_local_choice_never_leaves_the_computer() {
    // Con la IA local elegida, un fallo es un fallo: no hay respaldo hacia ningún externo.
    let mut w = world(
        ProviderKind::Ollama,
        Some(AiError::Unavailable("Ollama detenido".into())),
    );
    let task = run(&mut w.app, "F-1", 1);
    assert_eq!(task.state, TaskState::Failed);
    assert_eq!(task.error.unwrap().code, "unavailable");
    assert_eq!(w.asked.load(Ordering::SeqCst), 0, "ni se pide el respaldo");
    assert_eq!((w.external.calls(), w.local.calls()), (1, 0));
    assert!(ai_fallback_events(&w.app).is_empty());
}

#[test]
fn t03h_a_local_failure_inside_the_period_falls_back_to_the_external_once() {
    let mut w = world(
        ProviderKind::OllamaCloud,
        Some(AiError::Unavailable("sin conexión".into())),
    );
    run(&mut w.app, "F-1", 1);
    assert!(w.app.fallback_period_active());
    // El externo se recupera, pero el local falla: se prueba el externo y no se reintenta el local.
    w.external.set_error(None);
    w.local.set_error(Some(AiError::Timeout));
    let task = run(&mut w.app, "F-2", 2);
    assert_eq!(task.state, TaskState::AwaitingReview);
    assert_eq!(w.external.calls(), 2);
    assert_eq!(w.local.calls(), 2);
    assert!(task.result.unwrap().get("fallback").is_none());
}
