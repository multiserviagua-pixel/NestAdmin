//! D146: un 404 al generar (modelo o dirección que no sirven), un 402 (sin crédito) y un 4xx que
//! rechaza una petición no son «proveedor no disponible». Los dos primeros dejan las tareas en
//! `queued` sin gastar intentos y sin ninguna llamada al respaldo ni a otro proveedor; el
//! tercero falla solo esa tarea. Datos sintéticos, proveedores simulados y, para el cuerpo del
//! 404, un servidor local en 127.0.0.1.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use nestadmin_ai::ollama_cloud::OllamaCloudProvider;
use nestadmin_ai::{
    AiError, GenerateRequest, GenerateResult, ModelUnavailableCause, Provider, ProviderKind,
    Secret, Task as AiTask, Usage,
};
use nestadmin_app::{AiWait, App, AppPaths, LocalFallback};
use nestadmin_core::{Store, Task, TaskState};
use nestadmin_extract::{ExtractError, Extractor, Ocr};
use serde_json::{json, Value};

struct Fake {
    kind: ProviderKind,
    url: Option<String>,
    model: String,
    error: Mutex<Option<AiError>>,
    calls: AtomicUsize,
}

impl Fake {
    fn new(kind: ProviderKind, model: &str, error: Option<AiError>) -> Arc<Self> {
        Arc::new(Self {
            kind,
            url: None,
            model: model.to_owned(),
            error: Mutex::new(error),
            calls: AtomicUsize::new(0),
        })
    }
    /// Como el compatible con OpenAI: lleva la dirección completa configurada.
    fn compat(model: &str, url: &str, error: Option<AiError>) -> Arc<Self> {
        Arc::new(Self {
            kind: ProviderKind::OpenAiCompatible,
            url: Some(url.to_owned()),
            model: model.to_owned(),
            error: Mutex::new(error),
            calls: AtomicUsize::new(0),
        })
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
        &self.model
    }
    fn destination(&self) -> Option<String> {
        self.url.as_ref().map(|_| "api.ejemplo.invalid".to_owned())
    }
    fn configured_url(&self) -> Option<String> {
        self.url.clone()
    }
    fn generate(&self, request: &GenerateRequest) -> Result<GenerateResult, AiError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if let Some(error) = self.error.lock().unwrap().clone() {
            return Err(error);
        }
        let output = match request.task {
            AiTask::Extraction => serde_json::from_str(&request.content).unwrap(),
            _ => json!({ "category": "solicitud_factura", "draft": "Adjuntamos la factura." }),
        };
        Ok(GenerateResult {
            request_id: request.request_id.clone(),
            provider: self.kind,
            model: self.model.clone(),
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

struct World {
    app: App,
    external: Arc<Fake>,
    local: Arc<Fake>,
    /// Un proveedor externo distinto del elegido que el respaldo podría devolver por error.
    other_external: Arc<Fake>,
    asked: Arc<AtomicUsize>,
    _dir: tempfile::TempDir,
}

fn open_app(dir: &tempfile::TempDir, provider: Arc<dyn Provider>) -> (App, AppPaths) {
    let paths = AppPaths::new(dir.path());
    let extractor = Extractor::new(dir.path().join("sin-pdfium"), Box::new(NoOcr));
    let app = App::open(&paths, provider, Arc::new(extractor)).unwrap();
    (app, paths)
}

/// El externo elegido con el error dado y un respaldo local preparado que cuenta sus usos.
fn world(error: Option<AiError>) -> World {
    let dir = tempfile::tempdir().unwrap();
    let external = Fake::new(ProviderKind::OllamaCloud, "gemma4:31b", error);
    let local = Fake::new(ProviderKind::Ollama, "qwen3:8b", None);
    let other_external = Fake::new(ProviderKind::Anthropic, "otro", None);
    let (mut app, _) = open_app(&dir, Arc::clone(&external) as Arc<dyn Provider>);
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
        other_external,
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

fn import(app: &mut App, number: &str, day: u32) -> Task {
    app.import_document(&format!("{number}.txt"), &invoice(number, day))
        .unwrap()
}

fn task(app: &App, id: i64) -> Task {
    app.tasks()
        .unwrap()
        .into_iter()
        .find(|t| t.id == id)
        .unwrap()
}

fn not_found(cause: ModelUnavailableCause, status: u16) -> AiError {
    AiError::ModelUnavailable { cause, status }
}

fn ai_fallback_events(app: &App) -> usize {
    app.audit_log()
        .unwrap()
        .into_iter()
        .filter(|e| e.action == "ai_fallback")
        .count()
}

#[test]
fn a_404_or_a_402_keeps_the_task_queued_and_never_uses_the_fallback() {
    let errors = [
        not_found(ModelUnavailableCause::ModelNotFound, 404),
        not_found(ModelUnavailableCause::RouteNotFound, 404),
        not_found(ModelUnavailableCause::Other, 410),
        AiError::NoCredit,
    ];
    for error in errors {
        let mut w = world(Some(error.clone()));
        let first = import(&mut w.app, "F-1", 1);
        let second = import(&mut w.app, "F-2", 2);
        let released = w.app.process_next().unwrap().unwrap();
        assert_eq!(released.id, first.id, "{error:?}");
        assert_eq!(released.state, TaskState::Queued, "no falla: {error:?}");
        assert_eq!(released.attempts, 0, "no gasta intento: {error:?}");
        assert!(released.error.is_none());
        let wait = w.app.ai_wait().expect("hay espera");
        assert!(wait.is_provider_response());
        assert_eq!(wait.code(), error.code());

        // Ni el respaldo local, ni otro proveedor externo, ni siquiera se pide el local.
        assert_eq!((w.external.calls(), w.local.calls()), (1, 0), "{error:?}");
        assert_eq!(w.other_external.calls(), 0);
        assert_eq!(w.asked.load(Ordering::SeqCst), 0, "ni se pide el local");
        assert_eq!(ai_fallback_events(&w.app), 0);
        assert!(!w.app.fallback_period_active());

        // Con la espera puesta, nada más sale y nadie gasta intentos.
        assert!(w.app.process_next().unwrap().is_none());
        assert_eq!(w.external.calls(), 1, "ni una llamada más");
        for id in [first.id, second.id] {
            let t = task(&w.app, id);
            assert_eq!((t.state, t.attempts), (TaskState::Queued, 0), "{id}");
        }
    }
}

#[test]
fn the_waiting_text_is_exactly_the_one_for_each_cause() {
    let cases = [
        (
            not_found(ModelUnavailableCause::ModelNotFound, 404),
            "Ollama cloud no encuentra el modelo «gemma4:31b» (error 404). Las tareas esperan sin gastar intentos.",
        ),
        (
            not_found(ModelUnavailableCause::RouteNotFound, 404),
            "Ollama cloud respondió con un error 404 sin indicar la causa. Las tareas esperan sin gastar intentos. Pruebe de nuevo o elija otro modelo.",
        ),
        (
            not_found(ModelUnavailableCause::Other, 404),
            "Ollama cloud respondió con un error 404 usando el modelo «gemma4:31b». No consta la causa exacta; puede que el modelo no esté disponible para su cuenta. Las tareas esperan sin gastar intentos.",
        ),
        (
            AiError::NoCredit,
            "Ollama cloud indica que la cuenta no tiene crédito (error 402). Las tareas esperan sin gastar intentos.",
        ),
    ];
    for (error, text) in cases {
        let mut w = world(Some(error));
        import(&mut w.app, "F-1", 1);
        w.app.process_next().unwrap();
        assert_eq!(w.app.ai_wait_message(None).as_deref(), Some(text));
    }
    // Con el compatible con OpenAI, la ruta cita la dirección configurada.
    let dir = tempfile::tempdir().unwrap();
    let compat = Fake::new(
        ProviderKind::OpenAiCompatible,
        "m",
        Some(not_found(ModelUnavailableCause::RouteNotFound, 404)),
    );
    let (mut app, _) = open_app(&dir, compat as Arc<dyn Provider>);
    import(&mut app, "F-1", 1);
    app.process_next().unwrap();
    let text = app
        .ai_wait_message(Some("https://api.ejemplo.invalid/v1"))
        .unwrap();
    assert!(
        text.ends_with(" Compruebe la dirección en Configuración. Dirección configurada: https://api.ejemplo.invalid/v1."),
        "{text}"
    );
}

#[test]
fn a_rejected_request_fails_only_that_task_and_the_next_one_runs() {
    let mut w = world(Some(AiError::RequestRejected {
        status: 422,
        kind: Some("invalid_request_error".into()),
    }));
    let first = import(&mut w.app, "F-1", 1);
    let second = import(&mut w.app, "F-2", 2);
    let failed = w.app.process_next().unwrap().unwrap();
    assert_eq!(failed.id, first.id);
    assert_eq!(failed.state, TaskState::Failed);
    let error = failed.error.unwrap();
    assert_eq!(error.code, "request_rejected");
    assert!(error.recoverable, "se puede reintentar");
    assert_eq!(
        error.message,
        "Ollama cloud rechazó esta tarea (error 422). Las demás siguen en marcha. Puede reintentarla o elegir otro modelo."
    );
    assert_eq!(w.app.ai_wait(), None, "sin espera global");
    assert_eq!((w.local.calls(), w.other_external.calls()), (0, 0));
    assert_eq!(w.asked.load(Ordering::SeqCst), 0, "sin respaldo");

    // La siguiente se procesa de inmediato (y vuelve a fallar solo ella: el proveedor sigue igual).
    *w.external.error.lock().unwrap() = None;
    let next = w.app.process_next().unwrap().unwrap();
    assert_eq!(next.id, second.id);
    assert_eq!(next.state, TaskState::AwaitingReview);
    assert_eq!(task(&w.app, first.id).state, TaskState::Failed);
}

#[test]
fn changing_the_model_clears_the_wait_but_resolving_the_same_provider_does_not() {
    let mut w = world(Some(not_found(ModelUnavailableCause::ModelNotFound, 404)));
    import(&mut w.app, "F-1", 1);
    w.app.process_next().unwrap();
    assert!(w.app.ai_wait().is_some());

    // Volver a calcular el mismo proveedor (otra instancia, mismo modelo) no levanta nada.
    let same = Fake::new(ProviderKind::OllamaCloud, "gemma4:31b", None);
    w.app.select_provider(same as Arc<dyn Provider>);
    w.app.set_ai_wait(None);
    assert!(w.app.ai_wait().is_some(), "la espera sigue");

    // Otro modelo: la espera se limpia y la tarea se retoma sin haber gastado intentos.
    let other = Fake::new(ProviderKind::OllamaCloud, "gpt-oss:120b", None);
    w.app
        .select_provider(Arc::clone(&other) as Arc<dyn Provider>);
    w.app.set_ai_wait(None);
    assert_eq!(w.app.ai_wait(), None);
    let done = w.app.process_next().unwrap().unwrap();
    assert_eq!((done.state, done.attempts), (TaskState::AwaitingReview, 1));
    assert_eq!(other.calls(), 1);
}

#[test]
fn a_job_in_flight_with_the_old_model_does_not_bring_the_wait_back() {
    let mut w = world(Some(not_found(ModelUnavailableCause::ModelNotFound, 404)));
    let queued = import(&mut w.app, "F-1", 1);
    // La llamada sale con el modelo anterior…
    let job = w.app.begin_next().unwrap().unwrap();
    // …y mientras tanto la persona cambia el modelo.
    let other = Fake::new(ProviderKind::OllamaCloud, "gpt-oss:120b", None);
    w.app
        .select_provider(Arc::clone(&other) as Arc<dyn Provider>);
    w.app.set_ai_wait(None);
    // La respuesta antigua llega tarde: la tarea vuelve a la cola sin gastar intento, pero no
    // reactiva la espera del modelo que ya no está elegido.
    let released = w.app.finish(job.run()).unwrap();
    assert_eq!(released.id, queued.id);
    assert_eq!((released.state, released.attempts), (TaskState::Queued, 0));
    assert_eq!(w.app.ai_wait(), None, "la espera no se reactiva");
    let done = w.app.process_next().unwrap().unwrap();
    assert_eq!(done.state, TaskState::AwaitingReview);
    assert_eq!(other.calls(), 1);
}

#[test]
fn lifting_the_wait_after_a_successful_probe_resumes_the_queue() {
    let mut w = world(Some(AiError::NoCredit));
    let queued = import(&mut w.app, "F-1", 1);
    w.app.process_next().unwrap();
    assert_eq!(w.app.ai_wait(), Some(AiWait::NoCredit));
    assert!(w.app.process_next().unwrap().is_none());
    // La prueba sintética (D122) o el sondeo tuvo éxito: el proveedor ya responde.
    *w.external.error.lock().unwrap() = None;
    assert!(w.app.lift_provider_wait());
    assert!(!w.app.lift_provider_wait(), "ya no queda nada que levantar");
    let done = w.app.process_next().unwrap().unwrap();
    assert_eq!(done.id, queued.id);
    assert_eq!(done.state, TaskState::AwaitingReview);
}

#[test]
fn lifting_never_removes_a_configuration_wait() {
    let mut w = world(None);
    w.app.set_ai_wait(Some(AiWait::NeedsKey));
    assert!(!w.app.lift_provider_wait());
    assert_eq!(w.app.ai_wait(), Some(AiWait::NeedsKey));
}

#[test]
fn retrying_a_task_that_failed_with_a_404_creates_no_action_and_runs_once() {
    // Una tarea fallada antes de D146 con `unavailable: estado HTTP 404`: no llegó a actuar.
    let mut w = world(None);
    w.app.set_local_fallback(None);
    *w.external.error.lock().unwrap() = Some(AiError::Unavailable("estado HTTP 404".into()));
    let queued = import(&mut w.app, "F-1", 1);
    let failed = w.app.process_next().unwrap().unwrap();
    assert_eq!(failed.state, TaskState::Failed);
    assert_eq!(failed.error.as_ref().unwrap().code, "unavailable");
    let store = Store::open(&w._dir.path().join("nestadmin.db")).unwrap();
    assert!(store.task_actions(queued.id).unwrap().is_empty());

    *w.external.error.lock().unwrap() = None;
    w.app.retry(queued.id).unwrap();
    let done = w.app.process_next().unwrap().unwrap();
    assert_eq!(done.state, TaskState::AwaitingReview);
    // Reintentar no envió ni registró nada: sin filas en `actions`.
    assert!(store.task_actions(queued.id).unwrap().is_empty());
    assert!(w
        .app
        .audit_log()
        .unwrap()
        .iter()
        .all(|e| e.action != "send_draft" && e.action != "register_invoice"));
}

/// Servidor que contesta 404 con un cuerpo que repite el contenido enviado.
fn serve_404(body: String) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut length = 0;
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                break;
            }
            if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                length = v.trim().parse().unwrap_or(0);
            }
        }
        let mut sent = vec![0; length];
        reader.read_exact(&mut sent).unwrap();
        let _ = write!(
            stream,
            "HTTP/1.1 404 X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
    });
    url
}

#[test]
fn the_body_of_a_404_never_reaches_the_task_the_history_or_the_waiting_text() {
    const MARK: &str = "TEXTO_MARCADO_DEL_CONTENIDO_ENVIADO";
    let url = serve_404(json!({ "error": format!("model \"{MARK}\" not found") }).to_string());
    let provider = OllamaCloudProvider::with_endpoint(
        &format!("{url}/api"),
        "gemma4:31b",
        Secret::new("clave_sintetica_no_real"),
        Duration::from_secs(5),
    );
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _) = open_app(&dir, Arc::new(provider));
    let queued = import(&mut app, "F-1", 1);
    let released = app.process_next().unwrap().unwrap();
    assert_eq!(released.state, TaskState::Queued);
    assert_eq!(
        app.ai_wait(),
        Some(AiWait::ModelUnavailable {
            cause: ModelUnavailableCause::ModelNotFound,
            status: 404
        })
    );
    let shown: Value = json!({
        "tasks": app.tasks().unwrap(),
        "history": app.audit_log().unwrap(),
        "wait": app.ai_wait_message(None),
        "task": task(&app, queued.id),
    });
    let printed = shown.to_string();
    assert!(!printed.contains(MARK), "{printed}");
    assert!(!printed.contains("estado HTTP 404"), "{printed}");
    assert!(printed.contains("no encuentra el modelo"), "{printed}");
}

#[test]
fn a_split_invoice_still_runs_while_the_model_wait_holds_the_rest() {
    let mut w = world(None);
    let mut both: Value = serde_json::from_str(&invoice("F-3", 3)).unwrap();
    both["other_invoices"] = json!([serde_json::from_str::<Value>(&invoice("F-4", 4)).unwrap()]);
    w.app.import_document("dos.txt", &both.to_string()).unwrap();
    let first = w.app.process_next().unwrap().unwrap();
    let plain = import(&mut w.app, "F-9", 9);
    let split = w
        .app
        .tasks()
        .unwrap()
        .into_iter()
        .find(|t| t.id != first.id && t.id != plain.id)
        .unwrap();
    let calls = w.external.calls();
    w.app.set_ai_wait(Some(AiWait::ModelUnavailable {
        cause: ModelUnavailableCause::Other,
        status: 404,
    }));
    let done = w.app.process_next().unwrap().unwrap();
    assert_eq!(done.id, split.id, "la separada no necesita la IA");
    assert_eq!(done.state, TaskState::AwaitingReview);
    assert!(w.app.process_next().unwrap().is_none());
    assert_eq!(w.external.calls(), calls, "ni una llamada");
    assert_eq!(task(&w.app, plain.id).state, TaskState::Queued);
}

#[test]
fn correcting_the_path_of_the_compatible_url_lifts_the_wait() {
    let dir = tempfile::tempdir().unwrap();
    let route = not_found(ModelUnavailableCause::RouteNotFound, 404);
    let wrong = Fake::compat("m", "https://api.ejemplo.invalid/v9", Some(route));
    let (mut app, _) = open_app(&dir, wrong as Arc<dyn Provider>);
    import(&mut app, "F-1", 1);
    app.process_next().unwrap();
    assert!(app.ai_wait().is_some());
    // Mismo host y modelo, otra ruta: es otra dirección y la espera ya no vale.
    let fixed = Fake::compat("m", "https://api.ejemplo.invalid/v1", None);
    app.select_provider(fixed as Arc<dyn Provider>);
    app.set_ai_wait(None);
    assert_eq!(app.ai_wait(), None);
}

/// D146 es solo de proveedores externos: un 404 del Ollama local (aquí, en `/api/show`) sigue
/// siendo `unavailable` y la tarea falla como hasta ahora, sin espera (D101, R03).
#[test]
fn a_404_from_the_local_provider_fails_the_task_and_creates_no_wait() {
    let url = serve_404(json!({ "error": "model \"m\" not found" }).to_string());
    let provider =
        nestadmin_ai::ollama::OllamaProvider::new(&url, "m", Duration::from_secs(5)).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _) = open_app(&dir, Arc::new(provider));
    import(&mut app, "F-1", 1);
    let failed = app.process_next().unwrap().unwrap();
    assert_eq!(failed.state, TaskState::Failed);
    assert_eq!(failed.error.unwrap().code, "unavailable");
    assert_eq!(app.ai_wait(), None);
}
