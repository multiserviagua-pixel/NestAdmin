//! D148-D150: plantillas de pipeline en marcha, con un proveedor simulado, el correo simulado y
//! datos sintéticos (dominio `.test`). Una plantilla solo ordena tareas: nunca autoriza. La
//! respuesta espera a las facturas de sus adjuntos, pasa por su propia regla o revisión (R07), no
//! cambia lo que se envía a la IA (D102) y no duplica ningún efecto al recuperar.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;
use std::sync::{Arc, Mutex};

use nestadmin_ai::{
    AiError, GenerateRequest, GenerateResult, ModelUnavailableCause, Provider, ProviderKind,
    Task as AiTask, Usage,
};
use nestadmin_app::panel::SimulationOutcome;
use nestadmin_app::pipelines::{best_template, PipelineOverview, RunView, StepView};
use nestadmin_app::refine::RefineMode;
use nestadmin_app::rules::ReasonCode;
use nestadmin_app::{App, AppPaths, EMAIL, INVOICE};
use nestadmin_core::{
    ActionRequest, Authorization, Connector, ConnectorError, Store, Task, TaskState, TaskTemplate,
};
use nestadmin_extract::{ExtractError, Extractor, Ocr};
use nestadmin_simulators::{AdminSimulator, FailureMode, Message};
use serde_json::{json, Value};

const ZETA: &str = "proveedor-zeta.test";
const SENDER: &str = "ventas@proveedor-zeta.test";

// ---- Proveedor que registra lo que recibe ----

struct Rec {
    calls: Mutex<Vec<GenerateRequest>>,
    always: Mutex<Option<AiError>>,
}

impl Rec {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            calls: Mutex::new(Vec::new()),
            always: Mutex::new(None),
        })
    }
    fn fail_always(&self, error: Option<AiError>) {
        *self.always.lock().unwrap() = error;
    }
    fn requests(&self, task: AiTask) -> Vec<GenerateRequest> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.task == task)
            .cloned()
            .collect()
    }
}

impl Provider for Rec {
    fn kind(&self) -> ProviderKind {
        ProviderKind::OllamaCloud
    }
    fn model(&self) -> &str {
        "modelo-sintetico"
    }
    fn destination(&self) -> Option<String> {
        Some("ollama.test".into())
    }
    fn generate(&self, request: &GenerateRequest) -> Result<GenerateResult, AiError> {
        self.calls.lock().unwrap().push(request.clone());
        if let Some(error) = self.always.lock().unwrap().clone() {
            return Err(error);
        }
        let output = match request.task {
            AiTask::Extraction => match request.content.find("#DATOS#") {
                Some(at) => serde_json::from_str(&request.content[at + 7..]).unwrap(),
                None => json!({
                    "supplier": "Proveedor Alfa Suministros SL", "invoice_number": "FS-2026-014",
                    "date": "2026-10-01", "subtotal": "120.00", "tax": "25.20", "total": "145.20",
                    "moneda": "EUR",
                }),
            },
            _ => json!({ "category": "solicitud_factura", "draft": "Adjuntamos la factura." }),
        };
        Ok(GenerateResult {
            request_id: request.request_id.clone(),
            provider: ProviderKind::OllamaCloud,
            model: "modelo-sintetico".into(),
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

fn open(dir: &Path, ai: &Arc<Rec>) -> App {
    let extractor = Extractor::new(dir.join("sin-pdfium"), Box::new(NoOcr));
    let mut app = App::open(
        &AppPaths::new(dir),
        Arc::clone(ai) as Arc<dyn Provider>,
        Arc::new(extractor),
    )
    .unwrap();
    app.set_refine_mode(RefineMode::OnDemand);
    app
}

fn sql(dir: &Path) -> rusqlite::Connection {
    let conn = rusqlite::Connection::open(dir.join("nestadmin.db")).unwrap();
    conn.execute_batch("PRAGMA foreign_keys = OFF;").unwrap();
    conn
}

fn count(dir: &Path, query: &str) -> i64 {
    sql(dir).query_row(query, [], |r| r.get(0)).unwrap()
}

/// Filas de `actions` de una clase para una tarea.
fn actions_of(dir: &Path, task: i64, kind: &str) -> i64 {
    count(
        dir,
        &format!("SELECT COUNT(*) FROM actions WHERE task_id = {task} AND kind = '{kind}'"),
    )
}

fn runs(dir: &Path) -> i64 {
    count(dir, "SELECT COUNT(*) FROM pipeline_runs")
}

fn process_all(app: &mut App) -> usize {
    let mut n = 0;
    while app.process_next().unwrap().is_some() {
        n += 1;
    }
    n
}

/// Fecha `AAAA-MM-DD` de hoy (UTC): las plantillas solo se aplican a correo fechado desde su
/// activación (D149), así que los correos de prueba llevan la fecha de hoy.
fn today_text() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let z = (secs / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

// ---- Datos sintéticos ----

fn data(supplier: &str, number: u32) -> Value {
    json!({
        "supplier": supplier, "invoice_number": format!("F-{number}"),
        "date": format!("2026-10-{number:02}"), "subtotal": "100.00", "tax": "21.00",
        "total": "121.00", "moneda": "EUR",
    })
}

/// Documento con una factura; el proveedor simulado lee lo que va tras `#DATOS#`.
fn doc(supplier: &str, number: u32) -> String {
    doc_multi(supplier, &[number])
}

/// Documento que la IA separa en varias facturas (D68).
fn doc_multi(supplier: &str, numbers: &[u32]) -> String {
    let mut first = data(supplier, numbers[0]);
    first["other_invoices"] = json!(numbers[1..]
        .iter()
        .map(|n| data(supplier, *n))
        .collect::<Vec<_>>());
    format!(
        "FACTURA F-{}\nProveedor: {supplier}\nBase imponible: 100,00 EUR\nTotal factura: 121,00 EUR\n#DATOS#{first}",
        numbers[0]
    )
}

fn message(id: &str, from: Option<&str>, date: Option<String>) -> Message {
    Message {
        id: id.into(),
        subject: Some(format!("Asunto {id}")),
        from: from.map(str::to_owned),
        to: Some(vec!["administracion@nestadmin.test".into()]),
        body: Some("Texto sintético del correo.".into()),
        date,
        attachments: Vec::new(),
    }
}

/// Deja un correo propio, con sus adjuntos, en el simulador.
fn mail(app: &App, id: &str, from: Option<&str>, files: &[(&str, String)]) {
    mail_dated(app, id, from, Some(today_text()), files);
}

fn mail_dated(
    app: &App,
    id: &str,
    from: Option<&str>,
    date: Option<String>,
    files: &[(&str, String)],
) {
    let sim = app.simulators().0;
    sim.add_message(message(id, from, date)).unwrap();
    for (name, content) in files {
        sim.add_attachment(id, name, "text/plain", content.as_bytes())
            .unwrap();
    }
}

fn steps() -> Value {
    json!([{ "kind": "attachment_invoices" }, { "kind": "reply" }])
}

/// Guarda y activa una plantilla.
fn template(app: &mut App, id: &str, conditions: Value) -> TaskTemplate {
    app.save_task_template(Some(id), &format!("Plantilla {id}"), &conditions, &steps())
        .unwrap();
    app.set_task_template_active(id, true).unwrap()
}

fn email_task(app: &App, id: &str) -> Task {
    let wanted = format!("simulador:{id}");
    app.tasks()
        .unwrap()
        .into_iter()
        .find(|t| {
            t.kind == EMAIL
                && serde_json::from_str::<Value>(&t.input_ref).unwrap()["message_id"] == wanted
        })
        .unwrap_or_else(|| panic!("sin tarea de correo {id}"))
}

fn invoice_tasks(app: &App, id: &str) -> Vec<Task> {
    let wanted = format!("simulador:{id}");
    app.tasks()
        .unwrap()
        .into_iter()
        .filter(|t| {
            t.kind == INVOICE
                && serde_json::from_str::<Value>(&t.input_ref).unwrap()["origin"]["message_id"]
                    == wanted
        })
        .collect()
}

fn task(app: &App, id: i64) -> Task {
    app.tasks()
        .unwrap()
        .into_iter()
        .find(|t| t.id == id)
        .unwrap()
}

fn run_of(app: &App, subject: i64) -> RunView {
    app.pipeline_overview()
        .unwrap()
        .runs
        .into_iter()
        .find(|r| r.subject_task_id == subject)
        .unwrap_or_else(|| panic!("la tarea {subject} no tiene ejecución"))
}

fn step_of<'a>(run: &'a RunView, kind: &str) -> &'a StepView {
    run.steps.iter().find(|s| s.kind == kind).unwrap()
}

fn approve(app: &mut App, id: i64) -> Task {
    let version = task(app, id).result.unwrap()["version"].as_i64().unwrap();
    app.approve(id, version).unwrap()
}

/// Deja las tareas de ejemplo del simulador hechas, para que no estorben.
fn drain_samples(app: &mut App) {
    app.fetch_mail().unwrap();
    process_all(app);
}

// ---- La plantilla no autoriza ----

#[test]
fn approving_and_registering_the_invoice_leaves_the_reply_in_review_without_actions() {
    let dir = tempfile::tempdir().unwrap();
    let ai = Rec::new();
    let mut app = open(dir.path(), &ai);
    drain_samples(&mut app);
    template(&mut app, "zeta", json!({ "sender_domain": ZETA }));
    // Hay una regla para la factura de Z-2: la registra sola. No hay regla para la respuesta.
    app.create_rule(
        "zeta-auto",
        "register_invoice",
        &json!({ "supplier": "Zeta Auto SL" }),
    )
    .unwrap();
    app.set_rule_active("zeta-auto", true).unwrap();
    mail(
        &app,
        "Z-1",
        Some(SENDER),
        &[("manual.txt", doc("Zeta Manual SL", 1))],
    );
    mail(
        &app,
        "Z-2",
        Some(SENDER),
        &[("auto.txt", doc("Zeta Auto SL", 2))],
    );
    app.fetch_mail().unwrap();
    let (e1, e2) = (email_task(&app, "Z-1"), email_task(&app, "Z-2"));
    assert_eq!((runs(dir.path()), e1.state), (2, TaskState::Queued));
    let classification_before = ai.requests(AiTask::Classification).len();

    process_all(&mut app);
    // La factura con regla se registró sola; la otra espera a una persona; las respuestas no se
    // han pedido a la IA todavía si su factura no está completada.
    let (i1, i2) = (
        invoice_tasks(&app, "Z-1").remove(0),
        invoice_tasks(&app, "Z-2").remove(0),
    );
    assert_eq!(
        (i1.state, i2.state),
        (TaskState::AwaitingReview, TaskState::Completed)
    );
    assert_eq!(task(&app, e1.id).state, TaskState::Queued);
    assert_eq!(task(&app, e1.id).attempts, 0);
    assert_eq!(
        ai.requests(AiTask::Classification).len(),
        classification_before + 1,
        "solo la respuesta desbloqueada gasta IA"
    );
    // Z-2 pasa a revisión: la regla de su factura no autoriza su respuesta.
    assert_eq!(task(&app, e2.id).state, TaskState::AwaitingReview);

    // Aprobar y registrar la otra factura desbloquea su respuesta, también a revisión.
    assert_eq!(approve(&mut app, i1.id).state, TaskState::Completed);
    process_all(&mut app);
    assert_eq!(task(&app, e1.id).state, TaskState::AwaitingReview);
    for e in [e1.id, e2.id] {
        assert_eq!(actions_of(dir.path(), e, "send_draft"), 0, "tarea {e}");
    }
    assert_eq!(
        count(
            dir.path(),
            "SELECT COUNT(*) FROM actions WHERE kind = 'send_draft'"
        ),
        0
    );
    assert_eq!(app.simulators().0.sent().unwrap().len(), 0);
    assert_eq!(app.simulators().0.drafts().unwrap().len(), 0);
    // Cada factura se registró una vez.
    assert_eq!(actions_of(dir.path(), i1.id, "register_invoice"), 1);
    assert_eq!(actions_of(dir.path(), i2.id, "register_invoice"), 1);
    // Ningún cambio de la ejecución dio autorización a la respuesta.
    let result = task(&app, e1.id).result.unwrap();
    assert_eq!(result["evaluation"]["code"], "no_active_rule_matches");
}

#[test]
fn the_reply_is_automated_only_if_its_own_rule_allows_it() {
    let dir = tempfile::tempdir().unwrap();
    let ai = Rec::new();
    let mut app = open(dir.path(), &ai);
    drain_samples(&mut app);
    template(&mut app, "zeta", json!({ "sender_domain": ZETA }));
    app.create_rule(
        "responder-zeta",
        "send_draft",
        &json!({ "category": "solicitud_factura", "sender_domain": ZETA }),
    )
    .unwrap();
    app.set_rule_active("responder-zeta", true).unwrap();
    mail(&app, "Z-1", Some(SENDER), &[("a.txt", doc("Zeta SL", 1))]);
    app.fetch_mail().unwrap();
    let e = email_task(&app, "Z-1");
    let sent_before = app.simulators().0.sent().unwrap().len();

    // Con la regla activa, la respuesta sigue esperando a su factura: nada sale todavía.
    process_all(&mut app);
    assert_eq!(task(&app, e.id).state, TaskState::Queued);
    assert_eq!(app.simulators().0.sent().unwrap().len(), sent_before);

    let invoice = invoice_tasks(&app, "Z-1").remove(0);
    assert_eq!(approve(&mut app, invoice.id).state, TaskState::Completed);
    process_all(&mut app);
    // Su propia regla la automatiza, una sola vez, con su autorización.
    let done = task(&app, e.id);
    assert_eq!(done.state, TaskState::Completed, "{:?}", done.error);
    assert_eq!(actions_of(dir.path(), e.id, "send_draft"), 1);
    assert_eq!(
        count(
            dir.path(),
            &format!(
                "SELECT COUNT(*) FROM actions WHERE task_id = {} AND authorization LIKE 'regla %'",
                e.id
            )
        ),
        1
    );
    assert_eq!(app.simulators().0.sent().unwrap().len(), sent_before + 1);
    // La factura no la autorizó ninguna regla: su aprobación fue de una persona.
    assert_eq!(
        count(
            dir.path(),
            &format!(
                "SELECT COUNT(*) FROM actions WHERE task_id = {} AND authorization LIKE 'regla %'",
                invoice.id
            )
        ),
        0
    );
}

#[test]
fn the_pipelines_module_has_no_way_to_authorize_or_change_the_ai_request() {
    let source = include_str!("../src/pipelines.rs");
    let code: String = source
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    for forbidden in [
        "Authorization",
        "execute(",
        "approve(",
        "perform_action",
        "GenerateRequest",
        "generate_with_fallback",
        "Provider",
    ] {
        assert!(!code.contains(forbidden), "pipelines.rs usa {forbidden}");
    }
}

#[test]
fn the_ai_request_is_byte_for_byte_the_same_with_and_without_a_template() {
    let requests = |with_template: bool| {
        let dir = tempfile::tempdir().unwrap();
        let ai = Rec::new();
        let mut app = open(dir.path(), &ai);
        if with_template {
            template(&mut app, "zeta", json!({ "sender_domain": ZETA }));
        }
        mail(
            &app,
            "Z-1",
            Some(SENDER),
            &[("a.txt", doc("Zeta SL", 1)), ("b.txt", doc("Zeta SL", 2))],
        );
        app.fetch_mail().unwrap();
        assert_eq!(runs(dir.path()), i64::from(with_template));
        // Se aprueban las facturas conforme llegan para que la respuesta salga en ambos casos.
        for _ in 0..4 {
            process_all(&mut app);
            for t in app.tasks().unwrap() {
                if t.kind == INVOICE && t.state == TaskState::AwaitingReview {
                    approve(&mut app, t.id);
                }
            }
        }
        let e = email_task(&app, "Z-1");
        assert_eq!(e.state, TaskState::AwaitingReview);
        let mut all: Vec<String> = ai
            .calls
            .lock()
            .unwrap()
            .iter()
            .map(|r| {
                format!(
                    "{}|{:?}|{}|{}|{}",
                    r.request_id,
                    r.task,
                    r.instructions,
                    r.content,
                    r.schema.clone().unwrap_or(Value::Null)
                )
            })
            .collect();
        all.sort();
        all
    };
    let (with, without) = (requests(true), requests(false));
    assert!(with.len() >= 9, "faltan peticiones: {with:?}");
    assert_eq!(with, without);
}

// ---- Recuperación ----

/// Conector que ejecuta el efecto y corta el proceso justo después, antes de que la aplicación
/// lo anote como confirmado.
struct CutAfterEffect(AdminSimulator);

impl Connector for CutAfterEffect {
    fn lookup(&self, key: &str) -> Result<Option<Value>, ConnectorError> {
        self.0.lookup(key)
    }
    fn execute(&self, key: &str, payload: &Value) -> Result<Value, ConnectorError> {
        let _ = self.0.execute(key, payload);
        panic!("corte simulado tras el conector");
    }
}

#[test]
fn a_cut_after_the_connector_keeps_the_reply_blocked_until_reconciled_without_duplicating() {
    let dir = tempfile::tempdir().unwrap();
    let ai = Rec::new();
    let mut app = open(dir.path(), &ai);
    drain_samples(&mut app);
    template(&mut app, "zeta", json!({ "sender_domain": ZETA }));
    mail(&app, "Z-1", Some(SENDER), &[("a.txt", doc("Zeta SL", 1))]);
    app.fetch_mail().unwrap();
    process_all(&mut app);
    let (e, i) = (
        email_task(&app, "Z-1"),
        invoice_tasks(&app, "Z-1").remove(0),
    );
    assert_eq!(
        (task(&app, e.id).state, i.state),
        (TaskState::Queued, TaskState::AwaitingReview)
    );
    let payload = task(&app, i.id).result.unwrap()["action"]["payload"].clone();
    let before = app.simulators().1.record_count().unwrap();
    drop(app);

    // La persona aprueba y el registro ocurre, pero el proceso se corta antes de anotarlo.
    let mut store = Store::open(&dir.path().join("nestadmin.db")).unwrap();
    let admin = AdminSimulator::open(&dir.path().join("simulador-administracion.json")).unwrap();
    let request = ActionRequest {
        task_id: i.id,
        kind: "register_invoice".into(),
        version: 1,
        payload,
        authorization: Authorization::Approval,
    };
    let cut = catch_unwind(AssertUnwindSafe(|| {
        store.perform_action(&request, &CutAfterEffect(admin))
    }));
    assert!(cut.is_err(), "corte simulado");
    // Mientras no se reconcilia, la respuesta sigue retenida y la factura sin completar.
    assert!(store.ready_tasks().unwrap().iter().all(|t| t.id != e.id));
    assert_eq!(store.task(i.id).unwrap().state, TaskState::AwaitingReview);
    drop(store);

    // Al reabrir se reconcilia con el conector: la factura se completa, la respuesta se
    // desbloquea (y va a revisión) y el registro no se repite.
    let mut app = open(dir.path(), &ai);
    assert_eq!(task(&app, i.id).state, TaskState::Completed);
    assert_eq!(app.simulators().1.record_count().unwrap(), before + 1);
    process_all(&mut app);
    assert_eq!(task(&app, e.id).state, TaskState::AwaitingReview);
    assert_eq!(actions_of(dir.path(), i.id, "register_invoice"), 1);
    assert_eq!(actions_of(dir.path(), e.id, "send_draft"), 0);
    assert_eq!(app.simulators().1.record_count().unwrap(), before + 1);
}

#[test]
fn a_restart_in_the_middle_of_a_run_keeps_the_order_and_creates_nothing_twice() {
    let dir = tempfile::tempdir().unwrap();
    let ai = Rec::new();
    let mut app = open(dir.path(), &ai);
    drain_samples(&mut app);
    template(&mut app, "zeta", json!({ "sender_domain": ZETA }));
    mail(
        &app,
        "Z-1",
        Some(SENDER),
        &[("a.txt", doc("Zeta SL", 1)), ("b.txt", doc("Zeta SL", 2))],
    );
    app.fetch_mail().unwrap();
    let e = email_task(&app, "Z-1");

    // Una factura en curso cuando se corta el proceso.
    let job = app.begin_next().unwrap().unwrap();
    assert_eq!(job.task.kind, INVOICE);
    drop(job);
    drop(app);
    let mut app = open(dir.path(), &ai);
    let tasks_before = app.tasks().unwrap().len();
    assert_eq!(task(&app, e.id).state, TaskState::Queued);
    // Consultar otra vez no vuelve a crear ni la ejecución ni sus pasos ni las facturas.
    app.fetch_mail().unwrap();
    assert_eq!(app.tasks().unwrap().len(), tasks_before);
    assert_eq!(runs(dir.path()), 1);
    assert_eq!(count(dir.path(), "SELECT COUNT(*) FROM pipeline_steps"), 3);

    process_all(&mut app);
    assert_eq!(task(&app, e.id).state, TaskState::Queued, "esperan las dos");
    let invoices = invoice_tasks(&app, "Z-1");
    assert_eq!(invoices.len(), 2);
    assert!(invoices
        .iter()
        .all(|t| t.state == TaskState::AwaitingReview));
    approve(&mut app, invoices[0].id);
    process_all(&mut app);
    assert_eq!(task(&app, e.id).state, TaskState::Queued, "falta una");
    approve(&mut app, invoices[1].id);
    process_all(&mut app);
    assert_eq!(task(&app, e.id).state, TaskState::AwaitingReview);
    assert_eq!(runs(dir.path()), 1);
}

#[test]
fn a_model_404_during_a_run_leaves_the_task_queued_without_spending_attempts() {
    let dir = tempfile::tempdir().unwrap();
    let ai = Rec::new();
    let mut app = open(dir.path(), &ai);
    drain_samples(&mut app);
    template(&mut app, "zeta", json!({ "sender_domain": ZETA }));
    mail(&app, "Z-1", Some(SENDER), &[("a.txt", doc("Zeta SL", 1))]);
    app.fetch_mail().unwrap();
    let (e, i) = (
        email_task(&app, "Z-1"),
        invoice_tasks(&app, "Z-1").remove(0),
    );

    ai.fail_always(Some(AiError::ModelUnavailable {
        cause: ModelUnavailableCause::ModelNotFound,
        status: 404,
    }));
    let released = app.process_next().unwrap().unwrap();
    assert_eq!(released.id, i.id);
    assert_eq!((released.state, released.attempts), (TaskState::Queued, 0));
    assert!(released.error.is_none());
    assert_eq!(app.ai_wait().unwrap().code(), "model_unavailable");
    // Nada más avanza (la respuesta ni siquiera estaba lista) y nadie gastó intentos.
    assert!(app.process_next().unwrap().is_none());
    assert_eq!(
        (task(&app, e.id).state, task(&app, e.id).attempts),
        (TaskState::Queued, 0)
    );
    let run = run_of(&app, e.id);
    assert_eq!(
        step_of(&run, "reply").blocked.as_ref().unwrap().reason,
        "waiting"
    );

    // Arreglado el modelo, todo sigue con el mismo orden y sin duplicar.
    ai.fail_always(None);
    assert!(app.lift_provider_wait());
    process_all(&mut app);
    assert_eq!(task(&app, i.id).state, TaskState::AwaitingReview);
    assert_eq!(task(&app, e.id).state, TaskState::Queued);
    approve(&mut app, i.id);
    process_all(&mut app);
    assert_eq!(task(&app, e.id).state, TaskState::AwaitingReview);
    assert_eq!(task(&app, i.id).attempts, 1);
}

// ---- Partes de una factura (D68) ----

#[test]
fn split_invoice_parts_join_the_stage_and_the_reply_waits_for_all_of_them() {
    let dir = tempfile::tempdir().unwrap();
    let ai = Rec::new();
    let mut app = open(dir.path(), &ai);
    drain_samples(&mut app);
    template(&mut app, "zeta", json!({ "sender_domain": ZETA }));
    // Una regla de envío que, de no esperar, automatizaría la respuesta.
    app.create_rule(
        "responder-zeta",
        "send_draft",
        &json!({ "category": "solicitud_factura", "sender_domain": ZETA }),
    )
    .unwrap();
    app.set_rule_active("responder-zeta", true).unwrap();
    mail(
        &app,
        "Z-1",
        Some(SENDER),
        &[("dos.txt", doc_multi("Zeta SL", &[1, 2]))],
    );
    app.fetch_mail().unwrap();
    let e = email_task(&app, "Z-1");
    let first = app.process_next().unwrap().unwrap();
    assert_eq!(first.state, TaskState::AwaitingReview);

    // La segunda factura es otro paso de la etapa 1 de la misma ejecución.
    let run = run_of(&app, e.id);
    let stage_one: Vec<&StepView> = run.steps.iter().filter(|s| s.stage == 1).collect();
    assert_eq!(stage_one.len(), 2, "{run:?}");
    assert!(stage_one.iter().all(|s| s.kind == "attachment_invoices"));
    assert_eq!(run.steps.iter().filter(|s| s.stage == 2).count(), 1);
    assert_eq!(runs(dir.path()), 1);

    process_all(&mut app);
    let invoices = invoice_tasks(&app, "Z-1");
    assert_eq!(invoices.len(), 2);
    approve(&mut app, invoices[0].id);
    process_all(&mut app);
    assert_eq!(
        task(&app, e.id).state,
        TaskState::Queued,
        "falta la parte 2"
    );
    approve(&mut app, invoices[1].id);
    process_all(&mut app);
    // Con las dos hechas, la respuesta llega y su regla la envía (nunca antes).
    assert_eq!(task(&app, e.id).state, TaskState::Completed);
    assert_eq!(actions_of(dir.path(), e.id, "send_draft"), 1);
    assert_eq!(runs(dir.path()), 1);
    // Reprocesar no crea otra parte.
    app.fetch_mail().unwrap();
    assert_eq!(invoice_tasks(&app, "Z-1").len(), 2);
}

// ---- Adjuntos que no bajan y «Continuar sin ese paso» ----

#[test]
fn an_attachment_that_does_not_download_holds_the_reply_and_a_retry_links_it_once() {
    let dir = tempfile::tempdir().unwrap();
    let ai = Rec::new();
    let mut app = open(dir.path(), &ai);
    drain_samples(&mut app);
    template(&mut app, "zeta", json!({ "sender_domain": ZETA }));
    mail(&app, "Z-1", Some(SENDER), &[("a.txt", doc("Zeta SL", 1))]);
    app.simulators()
        .0
        .set_attachment_failure(Some(FailureMode::Unavailable));
    let report = app.fetch_mail().unwrap();
    assert_eq!(report.accounts[0].attachments.failed.len(), 1);
    let e = email_task(&app, "Z-1");
    assert!(invoice_tasks(&app, "Z-1").is_empty());
    process_all(&mut app);
    assert_eq!(
        (task(&app, e.id).state, task(&app, e.id).attempts),
        (TaskState::Queued, 0)
    );

    // «Bloqueada» por el adjunto que falta, con un código corto y sin contenido.
    let run = run_of(&app, e.id);
    let attachment = step_of(&run, "attachment_invoices");
    assert_eq!(attachment.task_id, None);
    assert_eq!(attachment.error_code.as_deref(), Some("unavailable"));
    let blocked = step_of(&run, "reply").blocked.as_ref().unwrap();
    assert_eq!(blocked.reason, "blocked");
    assert_eq!(blocked.by[0].code, "attachment_missing");
    assert_eq!(blocked.by[0].error_code.as_deref(), Some("unavailable"));
    // Reintentar con el fallo activo no duplica nada.
    app.fetch_mail().unwrap();
    assert_eq!(runs(dir.path()), 1);
    assert_eq!(count(dir.path(), "SELECT COUNT(*) FROM pipeline_steps"), 2);

    // Cuando baja, se enlaza una sola vez y borra el fallo anotado.
    app.simulators().0.set_attachment_failure(None);
    let retried = app.fetch_mail().unwrap();
    assert_eq!(retried.accounts[0].attachments.imported, 1);
    app.fetch_mail().unwrap();
    assert_eq!(invoice_tasks(&app, "Z-1").len(), 1);
    assert_eq!(count(dir.path(), "SELECT COUNT(*) FROM pipeline_steps"), 2);
    let run = run_of(&app, e.id);
    let attachment = step_of(&run, "attachment_invoices");
    assert_eq!(attachment.task_id, Some(invoice_tasks(&app, "Z-1")[0].id));
    assert_eq!(attachment.error_code, None);
    assert_eq!(
        step_of(&run, "reply").blocked.as_ref().unwrap().reason,
        "waiting"
    );
    process_all(&mut app);
    let id = invoice_tasks(&app, "Z-1")[0].id;
    approve(&mut app, id);
    process_all(&mut app);
    assert_eq!(task(&app, e.id).state, TaskState::AwaitingReview);
}

#[test]
fn continuing_without_a_step_releases_the_reply_and_forces_review_even_with_a_rule() {
    let dir = tempfile::tempdir().unwrap();
    let ai = Rec::new();
    let mut app = open(dir.path(), &ai);
    drain_samples(&mut app);
    template(&mut app, "zeta", json!({ "sender_domain": ZETA }));
    app.create_rule(
        "responder-zeta",
        "send_draft",
        &json!({ "category": "solicitud_factura", "sender_domain": ZETA }),
    )
    .unwrap();
    app.set_rule_active("responder-zeta", true).unwrap();
    mail(&app, "Z-1", Some(SENDER), &[("a.txt", doc("Zeta SL", 1))]);
    app.simulators()
        .0
        .set_attachment_failure(Some(FailureMode::Unavailable));
    app.fetch_mail().unwrap();
    let e = email_task(&app, "Z-1");
    process_all(&mut app);
    assert_eq!(task(&app, e.id).state, TaskState::Queued);

    let run = run_of(&app, e.id);
    let (attachment, reply) = (step_of(&run, "attachment_invoices"), step_of(&run, "reply"));
    assert!(attachment.waivable);
    assert!(!reply.waivable, "la respuesta no retiene nada");
    // Un paso que no retiene nada, o que no existe, no se puede omitir.
    assert!(app.waive_pipeline_step(reply.id, "").is_err());
    assert!(app.waive_pipeline_step(9999, "").is_err());
    assert!(app
        .waive_pipeline_step(attachment.id, &"x".repeat(201))
        .is_err());
    assert_eq!(
        count(
            dir.path(),
            "SELECT COUNT(*) FROM pipeline_steps WHERE waived_at IS NOT NULL"
        ),
        0
    );

    let updated = app
        .waive_pipeline_step(attachment.id, "El proveedor la enviará aparte")
        .unwrap();
    let waived = step_of(&updated, "attachment_invoices");
    assert!(waived.waived && !waived.waivable);
    assert_eq!(
        waived.waived_note.as_deref(),
        Some("El proveedor la enviará aparte")
    );
    assert!(step_of(&updated, "reply").blocked.is_none());
    // Auditoría: ids y códigos, nunca la nota ni el nombre del archivo.
    let events: Vec<_> = app
        .audit_log()
        .unwrap()
        .into_iter()
        .filter(|a| a.action == "pipeline_step")
        .collect();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].task_id, Some(e.id));
    let detail = events[0].detail.clone().unwrap();
    assert!(
        !detail.contains("proveedor la enviará") && !detail.contains("a.txt"),
        "{detail}"
    );
    // Repetirlo es idempotente: devuelve la vista, sin error ni segundo evento.
    assert!(app
        .waive_pipeline_step(attachment.id, "")
        .unwrap()
        .steps
        .iter()
        .any(|s| s.waived));
    assert_eq!(
        count(
            dir.path(),
            "SELECT COUNT(*) FROM audit_log WHERE action = 'pipeline_step'"
        ),
        1
    );

    process_all(&mut app);
    // Coincide la regla, pero la respuesta va a revisión: sin acción ni envío.
    let review = task(&app, e.id);
    assert_eq!(
        review.state,
        TaskState::AwaitingReview,
        "{:?}",
        review.error
    );
    let result = review.result.unwrap();
    assert_eq!(result["evaluation"]["code"], "pipeline_step_waived");
    assert!(result["evaluation"]["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c["id"] == "responder-zeta" && c["matches"] == true));
    assert!(result["review_reason"]
        .as_str()
        .unwrap()
        .contains("se continuó sin un paso"));
    assert_eq!(actions_of(dir.path(), e.id, "send_draft"), 0);
    assert_eq!(app.simulators().0.sent().unwrap().len(), 0);
    // Solo restringe: la persona puede aprobarla y se envía una vez.
    assert_eq!(approve(&mut app, e.id).state, TaskState::Completed);
    assert_eq!(app.simulators().0.sent().unwrap().len(), 1);
    // Si el adjunto baja después, se importa igual (no se pierde la factura) y no hay efectos
    // nuevos en la respuesta.
    app.simulators().0.set_attachment_failure(None);
    app.fetch_mail().unwrap();
    assert_eq!(invoice_tasks(&app, "Z-1").len(), 1);
    assert_eq!(actions_of(dir.path(), e.id, "send_draft"), 1);
}

#[test]
fn a_cancelled_step_blocks_the_reply_and_cancelling_it_too_is_not_a_rejection_signal() {
    let dir = tempfile::tempdir().unwrap();
    let ai = Rec::new();
    let mut app = open(dir.path(), &ai);
    drain_samples(&mut app);
    template(&mut app, "zeta", json!({ "sender_domain": ZETA }));
    mail(&app, "Z-1", Some(SENDER), &[("a.txt", doc("Zeta SL", 1))]);
    mail(&app, "Z-2", Some(SENDER), &[("b.txt", doc("Zeta SL", 2))]);
    app.fetch_mail().unwrap();
    let (e1, e2) = (email_task(&app, "Z-1"), email_task(&app, "Z-2"));
    let (i1, i2) = (
        invoice_tasks(&app, "Z-1").remove(0),
        invoice_tasks(&app, "Z-2").remove(0),
    );

    // Z-1: la factura se descarta en revisión (eso sí es una señal de rechazo) y la respuesta
    // queda «Bloqueada», sin gastar IA ni intentos.
    app.process_next().unwrap();
    assert_eq!(task(&app, i1.id).state, TaskState::AwaitingReview);
    assert_eq!(task(&app, i2.id).state, TaskState::Queued);
    let classifications = ai.requests(AiTask::Classification).len();
    assert_eq!(app.reject(i1.id).unwrap().state, TaskState::Cancelled);
    assert_eq!(task(&app, e1.id).state, TaskState::Queued);
    assert_eq!(ai.requests(AiTask::Classification).len(), classifications);
    let blocked = step_of(&run_of(&app, e1.id), "reply")
        .blocked
        .clone()
        .unwrap();
    assert_eq!(
        (blocked.reason, blocked.by[0].code),
        ("blocked", "cancelled")
    );
    let signals = |task: i64| {
        count(
            dir.path(),
            &format!(
                "SELECT COUNT(*) FROM rule_signals WHERE task_id = {task} AND kind = 'rejected'"
            ),
        )
    };
    assert_eq!(signals(i1.id), 1);

    // «Cancelar también»: cancelar la respuesta en cola no es una señal de rechazo.
    assert_eq!(app.reject(e1.id).unwrap().state, TaskState::Cancelled);
    assert_eq!(signals(e1.id), 0);
    assert_eq!(
        count(
            dir.path(),
            &format!(
                "SELECT COUNT(*) FROM rule_signals WHERE task_id = {}",
                e1.id
            )
        ),
        0
    );

    // Z-2: la factura se cancela mientras corre (running a cancelled): tampoco es señal, la
    // respuesta queda bloqueada y la factura tardía no registra nada.
    let job = app.begin_next().unwrap().unwrap();
    assert_eq!(job.task.id, i2.id);
    assert_eq!(app.reject(i2.id).unwrap().state, TaskState::Cancelled);
    let _ = app.finish(job.run());
    assert_eq!(task(&app, i2.id).state, TaskState::Cancelled);
    assert_eq!(signals(i2.id), 0);
    assert_eq!(actions_of(dir.path(), i2.id, "register_invoice"), 0);
    process_all(&mut app);
    assert_eq!(task(&app, e2.id).state, TaskState::Queued);
    let blocked = step_of(&run_of(&app, e2.id), "reply")
        .blocked
        .clone()
        .unwrap();
    assert_eq!(
        (blocked.reason, blocked.by[0].code),
        ("blocked", "cancelled")
    );
    assert_eq!(app.simulators().0.sent().unwrap().len(), 0);
}

// ---- Sin plantilla, primera lectura y no retroactivo ----

#[test]
fn without_a_matching_template_the_flow_is_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let ai = Rec::new();
    let mut app = open(dir.path(), &ai);
    // Una plantilla inactiva, otra con otro remitente y un correo sin adjuntos admitidos.
    app.save_task_template(Some("inactiva"), "Inactiva", &json!({}), &steps())
        .unwrap();
    template(
        &mut app,
        "otro",
        json!({ "sender": "otra@otro-proveedor.test" }),
    );
    mail(&app, "Z-1", Some(SENDER), &[("a.txt", doc("Zeta SL", 1))]);
    mail(
        &app,
        "Z-2",
        Some(SENDER),
        &[("hoja.xlsx", "no admitido".into())],
    );
    app.fetch_mail().unwrap();
    assert_eq!(runs(dir.path()), 0);
    assert_eq!(count(dir.path(), "SELECT COUNT(*) FROM pipeline_steps"), 0);
    assert!(app.pipeline_overview().unwrap().runs.is_empty());
    // Las respuestas salen en el orden de siempre, sin esperar a las facturas.
    let first = app.process_next().unwrap().unwrap();
    assert_eq!(first.kind, EMAIL);
    process_all(&mut app);
    for id in ["Z-1", "Z-2"] {
        assert_eq!(email_task(&app, id).state, TaskState::AwaitingReview);
    }
    // Con una plantilla "cualquiera" activa, un correo sin adjuntos admitidos tampoco la abre.
    template(&mut app, "cualquiera", json!({}));
    mail(
        &app,
        "Z-3",
        Some(SENDER),
        &[("hoja.xlsx", "no admitido".into())],
    );
    mail(&app, "Z-4", Some(SENDER), &[]);
    app.fetch_mail().unwrap();
    assert_eq!(runs(dir.path()), 0);
}

#[test]
fn a_mail_already_known_when_a_template_is_activated_gets_no_run() {
    let dir = tempfile::tempdir().unwrap();
    let ai = Rec::new();
    let mut app = open(dir.path(), &ai);
    mail(&app, "Z-1", Some(SENDER), &[("a.txt", doc("Zeta SL", 1))]);
    app.fetch_mail().unwrap();
    let before = app.tasks().unwrap();
    let (e1, i1) = (
        email_task(&app, "Z-1"),
        invoice_tasks(&app, "Z-1").remove(0),
    );
    assert!(before.iter().all(|t| t.state == TaskState::Queued));

    template(&mut app, "zeta", json!({ "sender_domain": ZETA }));
    app.fetch_mail().unwrap();
    assert_eq!(runs(dir.path()), 0, "no es retroactiva");
    assert_eq!(count(dir.path(), "SELECT COUNT(*) FROM pipeline_steps"), 0);
    assert_eq!(app.tasks().unwrap().len(), before.len());
    assert!(app.pipeline_overview().unwrap().runs.is_empty());
    // Sin esperar a su factura: la respuesta ya conocida llega a revisión aunque la factura
    // siga sin aprobar.
    process_all(&mut app);
    assert_eq!(task(&app, e1.id).state, TaskState::AwaitingReview);
    assert_eq!(task(&app, i1.id).state, TaskState::AwaitingReview);

    // Un correo nuevo sí la usa.
    mail(&app, "Z-2", Some(SENDER), &[("b.txt", doc("Zeta SL", 2))]);
    app.fetch_mail().unwrap();
    assert_eq!(runs(dir.path()), 1);
    let e2 = email_task(&app, "Z-2");
    assert_eq!(
        app.pipeline_overview().unwrap().runs[0].subject_task_id,
        e2.id
    );
}

#[test]
fn the_first_read_never_creates_a_run() {
    let dir = tempfile::tempdir().unwrap();
    let ai = Rec::new();
    let mut app = open(dir.path(), &ai);
    // Una plantilla "cualquiera" que encajaría con todo lo que tenga un adjunto admitido.
    template(&mut app, "cualquiera", json!({}));
    let old = nestadmin_app::initial_read::today_day() - 30;
    let old = civil(old);
    mail_dated(
        &app,
        "V-1",
        Some(SENDER),
        Some(old),
        &[("vieja.txt", doc("Zeta SL", 3))],
    );
    let status = app
        .start_initial_read(
            "simulador",
            Some(25),
            Some(&nestadmin_app::initial_read::ReadExpectation {
                provider: "ollama_cloud".into(),
                model: "modelo-sintetico".into(),
                host: "ollama.test".into(),
                volume: 25,
            }),
        )
        .unwrap();
    process_all(&mut app);
    assert!(status.tasks.total > 0, "{status:?}");
    // Los correos y facturas de la primera lectura quedan fuera de las plantillas, y una
    // consulta posterior (que los conoce) tampoco los vuelve a mirar.
    assert!(!app.initial_read_tasks().unwrap().is_empty());
    assert_eq!(runs(dir.path()), 0);
    app.fetch_mail().unwrap();
    assert_eq!(runs(dir.path()), 0);
    assert_eq!(count(dir.path(), "SELECT COUNT(*) FROM pipeline_steps"), 0);

    // Control: un correo reciente y nuevo sí abre su ejecución.
    mail(
        &app,
        "V-2",
        Some(SENDER),
        &[("nueva.txt", doc("Zeta SL", 4))],
    );
    app.fetch_mail().unwrap();
    assert_eq!(runs(dir.path()), 1);
    assert_eq!(
        app.pipeline_overview().unwrap().runs[0].subject_task_id,
        email_task(&app, "V-2").id
    );
}

/// `AAAA-MM-DD` de un día contado desde 1970 (copia de la de `initial_read.rs`).
fn civil(days: i64) -> String {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

// ---- Encaje, plantillas y versiones ----

fn tpl(id: &str, conditions: Value, active: bool) -> TaskTemplate {
    TaskTemplate {
        id: id.into(),
        version: 1,
        name: id.into(),
        conditions,
        steps: steps(),
        active,
        archived: false,
        created_at: 0,
    }
}

#[test]
fn the_most_concrete_template_wins_and_an_unusable_sender_only_fits_a_template_without_one() {
    let all = [
        tpl("cualquiera", json!({}), true),
        tpl("dominio", json!({ "sender_domain": ZETA }), true),
        tpl("direccion", json!({ "sender": SENDER }), true),
        tpl(
            "apagada",
            json!({ "sender": "otra@proveedor-zeta.test" }),
            false,
        ),
    ];
    let pick = |from: Option<&str>| best_template(&all, from).map(|t| t.id.as_str());
    assert_eq!(pick(Some(SENDER)), Some("direccion"));
    assert_eq!(pick(Some("VENTAS@Proveedor-Zeta.TEST")), Some("direccion"));
    assert_eq!(pick(Some("otra@proveedor-zeta.test")), Some("dominio"));
    assert_eq!(
        pick(Some("x@sub.proveedor-zeta.test")),
        Some("cualquiera"),
        "dominio exacto (D126)"
    );
    assert_eq!(pick(Some("x@otro.test")), Some("cualquiera"));
    // Sin remitente o sin una dirección simple: solo encaja la que no pide remitente.
    for bad in [
        None,
        Some(""),
        Some("Zeta <ventas@proveedor-zeta.test>"),
        Some("a@b@c.test"),
        Some("ventas"),
    ] {
        assert_eq!(pick(bad), Some("cualquiera"), "{bad:?}");
    }
    let without_any = &all[1..];
    for bad in [None, Some("Zeta <ventas@proveedor-zeta.test>")] {
        assert!(best_template(without_any, bad).is_none(), "{bad:?}");
    }
    // Archivada o inactiva no encaja.
    let mut archived = tpl("a", json!({}), true);
    archived.archived = true;
    assert!(best_template(&[archived], Some(SENDER)).is_none());
}

#[test]
fn matching_in_the_app_picks_by_specificity_and_ignores_a_from_that_is_not_an_address() {
    let dir = tempfile::tempdir().unwrap();
    let ai = Rec::new();
    let mut app = open(dir.path(), &ai);
    drain_samples(&mut app);
    template(&mut app, "cualquiera", json!({}));
    template(&mut app, "dominio", json!({ "sender_domain": ZETA }));
    template(&mut app, "direccion", json!({ "sender": SENDER }));
    for (id, from) in [
        ("A-1", Some(SENDER)),
        ("A-2", Some("otra@proveedor-zeta.test")),
        ("A-3", Some("x@otro.test")),
        ("A-4", Some("Zeta Ventas <ventas@proveedor-zeta.test>")),
        ("A-5", None),
    ] {
        mail(&app, id, from, &[("a.txt", doc("Zeta SL", 5))]);
    }
    app.fetch_mail().unwrap();
    let used = |id: &str| run_of(&app, email_task(&app, id).id).template_id;
    assert_eq!(used("A-1"), "direccion");
    assert_eq!(used("A-2"), "dominio");
    assert_eq!(used("A-3"), "cualquiera");
    assert_eq!(
        used("A-4"),
        "cualquiera",
        "el from no es una dirección simple"
    );
    assert_eq!(used("A-5"), "cualquiera");

    // Sin una plantilla "cualquiera", un from inválido o ausente no encaja con ninguna.
    app.set_task_template_active("cualquiera", false).unwrap();
    for (id, from) in [
        ("B-1", Some("Zeta Ventas <ventas@proveedor-zeta.test>")),
        ("B-2", None),
    ] {
        mail(&app, id, from, &[("b.txt", doc("Zeta SL", 6))]);
    }
    let runs_before = runs(dir.path());
    app.fetch_mail().unwrap();
    assert_eq!(runs(dir.path()), runs_before);
}

#[test]
fn two_active_templates_cannot_share_a_condition_and_unknown_steps_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let ai = Rec::new();
    let mut app = open(dir.path(), &ai);
    template(&mut app, "uno", json!({ "sender_domain": ZETA }));
    // Guardar otra con la misma condición, inactiva, se permite; activarla, no.
    app.save_task_template(
        Some("dos"),
        "Dos",
        &json!({ "sender_domain": "PROVEEDOR-zeta.test" }),
        &steps(),
    )
    .unwrap();
    assert!(app.set_task_template_active("dos", true).is_err());
    assert!(
        !app.task_templates()
            .unwrap()
            .iter()
            .find(|t| t.id == "dos")
            .unwrap()
            .active
    );
    // Cambiar la condición de una activa a una ocupada tampoco.
    template(&mut app, "tres", json!({ "sender": SENDER }));
    assert!(app
        .save_task_template(
            Some("tres"),
            "Tres",
            &json!({ "sender_domain": ZETA }),
            &steps()
        )
        .is_err());
    // Al desactivar la primera, la otra se puede activar; y "cualquiera" solo cabe una.
    app.set_task_template_active("uno", false).unwrap();
    app.set_task_template_active("dos", true).unwrap();
    template(&mut app, "todo", json!({}));
    app.save_task_template(Some("todo-2"), "Todo 2", &json!({}), &steps())
        .unwrap();
    assert!(app.set_task_template_active("todo-2", true).is_err());

    // Catálogo cerrado y condiciones válidas.
    let stored = app.task_templates().unwrap().len();
    let save = |app: &mut App, conditions: Value, steps: Value| {
        app.save_task_template(None, "Mala", &conditions, &steps)
    };
    for bad_steps in [
        json!([{ "kind": "attachment_invoices" }, { "kind": "send_reply" }]),
        json!([{ "kind": "classify" }, { "kind": "reply" }]),
        json!([{ "kind": "attachment_invoices" }]),
        json!([{ "kind": "reply" }, { "kind": "attachment_invoices" }]),
        json!([{ "kind": "attachment_invoices" }, { "kind": "reply" }, { "kind": "reply" }]),
        json!([{ "kind": "attachment_invoices" }, { "kind": "reply", "action": "send_draft" }]),
        json!([{ "kind": "attachment_invoices", "automate": true }, { "kind": "reply" }]),
        json!([{ "type": "reply" }]),
        json!("reply"),
        json!([]),
    ] {
        let error = save(&mut app, json!({}), bad_steps.clone())
            .unwrap_err()
            .to_string();
        assert!(!error.is_empty(), "{bad_steps}");
    }
    let unknown = save(
        &mut app,
        json!({}),
        json!([{ "kind": "attachment_invoices" }, { "kind": "send_reply" }]),
    )
    .unwrap_err()
    .to_string();
    assert!(unknown.contains("tipo de paso desconocido"), "{unknown}");
    for bad_conditions in [
        json!({ "sender": "no-es-una-direccion" }),
        json!({ "sender": "Zeta <a@b.test>" }),
        json!({ "sender_domain": "sin-punto" }),
        json!({ "sender_domain": "*.proveedor-zeta.test" }),
        json!({ "sender": SENDER, "sender_domain": ZETA }),
        json!({ "category": "solicitud_factura" }),
        json!({ "sender": 3 }),
        json!(["sender"]),
    ] {
        assert!(
            save(&mut app, bad_conditions.clone(), steps()).is_err(),
            "{bad_conditions}"
        );
    }
    assert!(app
        .save_task_template(None, "  ", &json!({}), &steps())
        .is_err());
    assert!(app
        .save_task_template(Some("Con Mayúsculas"), "x", &json!({}), &steps())
        .is_err());
    assert_eq!(
        app.task_templates().unwrap().len(),
        stored,
        "nada se guardó"
    );
}

#[test]
fn editing_a_template_keeps_versions_and_does_not_change_a_run_in_progress() {
    let dir = tempfile::tempdir().unwrap();
    let ai = Rec::new();
    let mut app = open(dir.path(), &ai);
    drain_samples(&mut app);
    let v1 = template(&mut app, "zeta", json!({ "sender_domain": ZETA }));
    assert_eq!(v1.version, 2, "crear y activar");
    mail(&app, "Z-1", Some(SENDER), &[("a.txt", doc("Zeta SL", 1))]);
    app.fetch_mail().unwrap();
    let e = email_task(&app, "Z-1");
    let before = run_of(&app, e.id);
    assert_eq!(
        (before.template_id.as_str(), before.template_version),
        ("zeta", v1.version)
    );

    // Se edita (nombre y condición) y se archiva otra copia: la ejecución sigue igual.
    let v2 = app
        .save_task_template(
            Some("zeta"),
            "Zeta renombrada",
            &json!({ "sender_domain": "otro-dominio.test" }),
            &steps(),
        )
        .unwrap();
    assert_eq!(v2.version, v1.version + 1);
    assert!(v2.active, "editar conserva el estado activo");
    let after = run_of(&app, e.id);
    assert_eq!(after.template_version, v1.version);
    assert_eq!(after.template_name.as_deref(), Some("Plantilla zeta"));
    assert_eq!(after.steps.len(), before.steps.len());
    assert_eq!(after.steps[0].label, before.steps[0].label);
    process_all(&mut app);
    let id = invoice_tasks(&app, "Z-1")[0].id;
    approve(&mut app, id);
    process_all(&mut app);
    assert_eq!(task(&app, e.id).state, TaskState::AwaitingReview);
    // Los correos nuevos usan la versión nueva (y su condición).
    mail(&app, "Z-2", Some(SENDER), &[("b.txt", doc("Zeta SL", 2))]);
    app.fetch_mail().unwrap();
    assert_eq!(
        runs(dir.path()),
        1,
        "la versión 2 ya no encaja con este remitente"
    );

    // Archivar conserva todas las versiones y la deja inactiva y sin editar.
    let archived = app.archive_task_template("zeta", true).unwrap();
    assert!(archived.archived && !archived.active);
    assert_eq!(
        count(
            dir.path(),
            "SELECT COUNT(*) FROM task_templates WHERE id = 'zeta'"
        ),
        4
    );
    assert!(app
        .save_task_template(Some("zeta"), "x", &json!({}), &steps())
        .is_err());
    assert!(app.set_task_template_active("zeta", true).is_err());
    let restored = app.archive_task_template("zeta", false).unwrap();
    assert!(!restored.archived && !restored.active);
    assert!(app.set_task_template_active("zeta", true).unwrap().active);
}

#[test]
fn the_overview_has_codes_and_no_mail_content() {
    let dir = tempfile::tempdir().unwrap();
    let ai = Rec::new();
    let mut app = open(dir.path(), &ai);
    drain_samples(&mut app);
    template(&mut app, "zeta", json!({ "sender_domain": ZETA }));
    mail(&app, "Z-1", Some(SENDER), &[("a.txt", doc("Zeta SL", 1))]);
    app.fetch_mail().unwrap();
    process_all(&mut app);
    let overview: PipelineOverview = app.pipeline_overview().unwrap();
    let text = serde_json::to_string(&overview).unwrap();
    assert!(
        !text.contains("Texto sintético") && !text.contains("Asunto Z-1"),
        "{text}"
    );
    assert!(!text.contains(SENDER), "{text}");
    let run = &overview.runs[0];
    assert_eq!(run.template_name.as_deref(), Some("Plantilla zeta"));
    let reply = step_of(run, "reply");
    let blocked = reply.blocked.as_ref().unwrap();
    assert_eq!((blocked.reason, blocked.by[0].code), ("waiting", "waiting"));
    assert_eq!(blocked.by[0].task_state, Some(TaskState::AwaitingReview));
    assert!(
        !step_of(run, "attachment_invoices").waivable,
        "avanza: no hace falta omitirlo"
    );
    assert!(reply.task_state == Some(TaskState::Queued) && !reply.waived);
}

// ---- Revisión de la fase 3: partes tardías, fechas, adjuntos rechazados, vista y baja ----

#[test]
fn a_part_after_continuing_without_the_step_is_enqueued_outside_the_run_and_keeps_the_result() {
    let dir = tempfile::tempdir().unwrap();
    let ai = Rec::new();
    let mut app = open(dir.path(), &ai);
    drain_samples(&mut app);
    template(&mut app, "zeta", json!({ "sender_domain": ZETA }));
    mail(
        &app,
        "Z-1",
        Some(SENDER),
        &[("a.txt", doc_multi("Zeta SL", &[1, 2]))],
    );
    app.fetch_mail().unwrap();
    let e = email_task(&app, "Z-1");
    // La factura falla; se continúa sin ese paso y la respuesta avanza (a revisión).
    ai.fail_always(Some(AiError::Unavailable("sin conexión".into())));
    let failed = app.process_next().unwrap().unwrap();
    assert_eq!(failed.state, TaskState::Failed);
    ai.fail_always(None);
    let run = run_of(&app, e.id);
    let step = step_of(&run, "attachment_invoices").id;
    app.waive_pipeline_step(step, "").unwrap();
    process_all(&mut app);
    assert_eq!(task(&app, e.id).state, TaskState::AwaitingReview);

    // Se reintenta la factura y la IA devuelve dos: no falla y no pierde ni resultado ni partes.
    app.retry(failed.id).unwrap();
    process_all(&mut app);
    let first = task(&app, failed.id);
    assert_eq!(first.state, TaskState::AwaitingReview, "{:?}", first.error);
    assert_eq!(first.result.as_ref().unwrap()["part"]["of"], 2);
    let invoices = invoice_tasks(&app, "Z-1");
    assert_eq!(invoices.len(), 2);
    let part = invoices.iter().find(|t| t.id != failed.id).unwrap();
    let input: Value = serde_json::from_str(&part.input_ref).unwrap();
    assert_eq!(input["part"]["split_from"], failed.id);
    assert_eq!(part.state, TaskState::AwaitingReview, "D68: a revisión");
    // Está fuera de la ejecución (la respuesta ya empezó) y no se duplica al volver a mirar.
    assert_eq!(count(dir.path(), "SELECT COUNT(*) FROM pipeline_steps"), 2);
    app.fetch_mail().unwrap();
    process_all(&mut app);
    assert_eq!(invoice_tasks(&app, "Z-1").len(), 2);
}

#[test]
fn an_attachment_that_arrives_after_continuing_is_imported_and_the_review_stays() {
    let dir = tempfile::tempdir().unwrap();
    let ai = Rec::new();
    let mut app = open(dir.path(), &ai);
    drain_samples(&mut app);
    template(&mut app, "zeta", json!({ "sender_domain": ZETA }));
    app.create_rule(
        "responder-zeta",
        "send_draft",
        &json!({ "category": "solicitud_factura", "sender_domain": ZETA }),
    )
    .unwrap();
    app.set_rule_active("responder-zeta", true).unwrap();
    mail(&app, "Z-1", Some(SENDER), &[("a.txt", doc("Zeta SL", 1))]);
    app.simulators()
        .0
        .set_attachment_failure(Some(FailureMode::Unavailable));
    app.fetch_mail().unwrap();
    let e = email_task(&app, "Z-1");
    let step = step_of(&run_of(&app, e.id), "attachment_invoices").id;
    app.waive_pipeline_step(step, "").unwrap();
    process_all(&mut app);
    assert_eq!(task(&app, e.id).state, TaskState::AwaitingReview);

    // El adjunto llega: se importa igual para no perder la factura, consta en el historial y la
    // respuesta sigue con su revisión forzada.
    app.simulators().0.set_attachment_failure(None);
    app.fetch_mail().unwrap();
    let invoices = invoice_tasks(&app, "Z-1");
    assert_eq!(invoices.len(), 1);
    process_all(&mut app);
    assert_eq!(task(&app, invoices[0].id).state, TaskState::AwaitingReview);
    assert_eq!(
        approve(&mut app, invoices[0].id).state,
        TaskState::Completed
    );
    assert_eq!(
        actions_of(dir.path(), invoices[0].id, "register_invoice"),
        1
    );
    let run = run_of(&app, e.id);
    let attachment = step_of(&run, "attachment_invoices");
    assert!(attachment.waived && attachment.linked_after_waive);
    assert_eq!(attachment.task_id, Some(invoices[0].id));
    assert_eq!(
        count(
            dir.path(),
            "SELECT COUNT(*) FROM audit_log WHERE action = 'pipeline_step' AND status = 'linked_after_waive'"
        ),
        1
    );
    let reply = task(&app, e.id).result.unwrap();
    assert_eq!(reply["evaluation"]["code"], "pipeline_step_waived");
    assert_eq!(actions_of(dir.path(), e.id, "send_draft"), 0);
    // Repetir la consulta no vuelve a importarla ni repite el evento.
    app.fetch_mail().unwrap();
    assert_eq!(invoice_tasks(&app, "Z-1").len(), 1);
    assert_eq!(
        count(
            dir.path(),
            "SELECT COUNT(*) FROM audit_log WHERE status = 'linked_after_waive'"
        ),
        1
    );
}

#[test]
fn waiving_twice_is_idempotent_without_a_second_audit_event() {
    let dir = tempfile::tempdir().unwrap();
    let ai = Rec::new();
    let mut app = open(dir.path(), &ai);
    drain_samples(&mut app);
    template(&mut app, "zeta", json!({ "sender_domain": ZETA }));
    mail(&app, "Z-1", Some(SENDER), &[("a.txt", doc("Zeta SL", 1))]);
    app.simulators()
        .0
        .set_attachment_failure(Some(FailureMode::Unavailable));
    app.fetch_mail().unwrap();
    let e = email_task(&app, "Z-1");
    let step = step_of(&run_of(&app, e.id), "attachment_invoices").id;
    let (_, first) = app.waive_step(step, "").unwrap();
    let (view, second) = app.waive_step(step, "otra nota").unwrap();
    assert!(first && !second);
    let waived = step_of(&view, "attachment_invoices");
    assert!(
        waived.waived && waived.waived_note.is_none(),
        "la nota no cambia"
    );
    assert_eq!(
        count(
            dir.path(),
            "SELECT COUNT(*) FROM audit_log WHERE action = 'pipeline_step'"
        ),
        1
    );
}

#[test]
fn a_template_applies_only_from_its_activation_day_and_never_to_old_or_undated_mail() {
    let dir = tempfile::tempdir().unwrap();
    let ai = Rec::new();
    let mut app = open(dir.path(), &ai);
    drain_samples(&mut app);
    template(&mut app, "zeta", json!({ "sender_domain": ZETA }));
    let ago = |days: i64| civil(nestadmin_app::initial_read::today_day() - days);
    // Conectar una cuenta con correo antiguo, o subir el límite D44, no abre ejecuciones.
    mail_dated(
        &app,
        "O-1",
        Some(SENDER),
        Some(ago(10)),
        &[("a.txt", doc("Zeta SL", 1))],
    );
    mail_dated(
        &app,
        "O-2",
        Some(SENDER),
        Some(ago(1)),
        &[("b.txt", doc("Zeta SL", 2))],
    );
    mail_dated(
        &app,
        "O-3",
        Some(SENDER),
        None,
        &[("c.txt", doc("Zeta SL", 3))],
    );
    mail_dated(
        &app,
        "O-4",
        Some(SENDER),
        Some("ayer".into()),
        &[("d.txt", doc("Zeta SL", 4))],
    );
    app.set_mail_limit(500).unwrap();
    app.fetch_mail().unwrap();
    assert_eq!(runs(dir.path()), 0);
    assert_eq!(count(dir.path(), "SELECT COUNT(*) FROM pipeline_steps"), 0);
    for id in ["O-1", "O-2", "O-3", "O-4"] {
        assert_eq!(email_task(&app, id).state, TaskState::Queued, "{id}");
        assert_eq!(
            invoice_tasks(&app, id).len(),
            1,
            "{id}: su factura se importa como siempre"
        );
    }
    // Hoy y futuro (la fecha la declara el remitente): sí.
    mail_dated(
        &app,
        "N-1",
        Some(SENDER),
        Some(ago(0)),
        &[("e.txt", doc("Zeta SL", 5))],
    );
    mail_dated(
        &app,
        "N-2",
        Some(SENDER),
        Some(ago(-1)),
        &[("f.txt", doc("Zeta SL", 6))],
    );
    app.fetch_mail().unwrap();
    assert_eq!(runs(dir.path()), 2);
}

#[test]
fn the_activation_day_is_the_last_time_the_template_became_active() {
    let version = |n: i64, active: bool, at: i64| TaskTemplate {
        version: n,
        active,
        created_at: at,
        ..tpl("t", json!({}), active)
    };
    use nestadmin_core::active_since;
    // Creada inactiva, activada, editada activa: cuenta la activación.
    let h = [
        version(1, false, 10),
        version(2, true, 20),
        version(3, true, 30),
    ];
    assert_eq!(active_since(&h), Some(20));
    // Desactivada y reactivada: cuenta la reactivación.
    let h = [
        version(1, false, 10),
        version(2, true, 20),
        version(3, false, 30),
        version(4, true, 40),
        version(5, true, 50),
    ];
    assert_eq!(active_since(&h), Some(40));
    // Vigente inactiva: no hay día.
    let h = [version(1, true, 10), version(2, false, 20)];
    assert_eq!(active_since(&h), None);
}

/// Un adjunto con estado propio escrito antes de abrir la aplicación (nombre, tamaño declarado y
/// contenido), para los casos que el simulador no deja crear con su interfaz.
fn seed_attachment(dir: &Path, id: &str, name: &str, size: Option<u64>, content: &[u8]) {
    let mut message = message(id, Some(SENDER), Some(today_text()));
    message.attachments = vec![nestadmin_simulators::Attachment {
        id: "A-1".into(),
        name: Some(name.into()),
        mime: Some("text/plain".into()),
        size,
    }];
    let state = json!({
        "version": 1,
        "state": {
            "messages": [message], "drafts": [], "sent": [],
            "files": [{ "message_id": id, "attachment_id": "A-1", "content": content }],
        },
    });
    std::fs::write(dir.join("simulador-correo.json"), state.to_string()).unwrap();
}

#[test]
fn an_attachment_that_can_never_be_imported_is_rejected_once_and_only_continuing_leaves_it() {
    // Tamaño declarado sobre el máximo, vacío y rechazado al comprobarlo ya bajado.
    let cases: [(&str, Option<u64>); 3] = [
        ("enorme.txt", Some(60_000_000)),
        ("vacio.txt", Some(0)),
        ("rechazado.txt", None),
    ];
    for (name, size) in cases {
        let dir = tempfile::tempdir().unwrap();
        seed_attachment(dir.path(), "R-1", name, size, b"");
        let ai = Rec::new();
        let mut app = open(dir.path(), &ai);
        template(&mut app, "zeta", json!({ "sender_domain": ZETA }));
        app.fetch_mail().unwrap();
        let e = email_task(&app, "R-1");
        let run = run_of(&app, e.id);
        let step = step_of(&run, "attachment_invoices").clone();
        assert_eq!(
            step.error_code.as_deref(),
            Some("attachment_rejected"),
            "{name}"
        );
        assert_eq!(step.task_id, None);
        let blocked = step_of(&run, "reply").blocked.clone().unwrap();
        assert_eq!(
            (blocked.reason, blocked.by[0].code),
            ("blocked", "attachment_rejected"),
            "{name}"
        );
        assert!(step.waivable, "{name}");

        // No se vuelve a descargar: con la descarga fallando, el código no cambia.
        app.simulators()
            .0
            .set_attachment_failure(Some(FailureMode::Unavailable));
        let report = app.fetch_mail().unwrap();
        let code = step_of(&run_of(&app, e.id), "attachment_invoices")
            .error_code
            .clone();
        assert_eq!(code.as_deref(), Some("attachment_rejected"), "{name}");
        assert!(
            report.accounts[0]
                .attachments
                .failed
                .iter()
                .all(|f| f.name != name || f.code != "unavailable"),
            "{name}: no debe pedir el adjunto otra vez"
        );
        app.simulators().0.set_attachment_failure(None);
        process_all(&mut app);
        assert_eq!(task(&app, e.id).state, TaskState::Queued, "{name}");
        // La salida es continuar sin ese paso; la respuesta va a revisión.
        app.waive_pipeline_step(step.id, "").unwrap();
        process_all(&mut app);
        assert_eq!(task(&app, e.id).state, TaskState::AwaitingReview, "{name}");
    }
}

#[test]
fn the_overview_always_has_the_queued_runs_and_waive_returns_its_own_run() {
    let dir = tempfile::tempdir().unwrap();
    let ai = Rec::new();
    let mut app = open(dir.path(), &ai);
    drain_samples(&mut app);
    template(&mut app, "zeta", json!({ "sender_domain": ZETA }));
    mail(&app, "Z-1", Some(SENDER), &[("a.txt", doc("Zeta SL", 1))]);
    app.simulators()
        .0
        .set_attachment_failure(Some(FailureMode::Unavailable));
    app.fetch_mail().unwrap();
    let e = email_task(&app, "Z-1");
    drop(app);
    // 600 ejecuciones más recientes, ya terminadas: la de Z-1, más antigua, sigue en cola.
    synthetic_runs(dir.path(), 600, false);
    let app = open(dir.path(), &ai);
    let overview = app.pipeline_overview().unwrap();
    assert_eq!(
        overview.runs.len(),
        501,
        "500 recientes y la que sigue en cola"
    );
    assert!(overview.runs.iter().any(|r| r.subject_task_id == e.id));
    let mut app = app;
    let step = step_of(&run_of(&app, e.id), "attachment_invoices").id;
    let view = app.waive_pipeline_step(step, "").unwrap();
    assert_eq!(view.subject_task_id, e.id);
    assert!(step_of(&view, "attachment_invoices").waived);
}

/// Inserta `n` ejecuciones sintéticas (correo + dos facturas) en una transacción. Con `mixed`,
/// los estados varían para comparar la vista con `blocked_by`; sin él, todas están terminadas.
fn synthetic_runs(dir: &Path, n: usize, mixed: bool) {
    let mut conn = sql(dir);
    let tx = conn.transaction().unwrap();
    let states = [
        "queued",
        "failed",
        "cancelled",
        "awaiting_review",
        "completed",
        "running",
    ];
    for i in 0..n {
        let mut ids = Vec::new();
        for (j, kind) in ["email", "invoice", "invoice"].iter().enumerate() {
            let state = if !mixed {
                "completed"
            } else if j == 0 {
                "queued"
            } else {
                states[(i + j) % states.len()]
            };
            tx.execute(
                "INSERT INTO tasks (kind, input_ref, state, created_at, updated_at) VALUES (?1, '{}', ?2, 1, 1)",
                rusqlite::params![kind, state],
            )
            .unwrap();
            ids.push(tx.last_insert_rowid());
        }
        tx.execute(
            "INSERT INTO pipeline_runs (template_id, template_version, steps_json, subject_task_id, created_at) VALUES ('zeta', 1, '[]', ?1, 1)",
            [ids[0]],
        )
        .unwrap();
        let run = tx.last_insert_rowid();
        for (stage, kind, task, key) in [
            (
                1,
                "attachment_invoices",
                Some(ids[1]),
                Some(format!("k{i}a")),
            ),
            (
                1,
                "attachment_invoices",
                if mixed && i % 7 == 0 {
                    None
                } else {
                    Some(ids[2])
                },
                Some(format!("k{i}b")),
            ),
            (2, "reply", Some(ids[0]), None),
        ] {
            let waived = mixed
                && i % 11 == 0
                && stage == 1
                && key.as_deref().is_some_and(|k| k.ends_with('a'));
            tx.execute(
                "INSERT INTO pipeline_steps (run_id, stage, kind, label, attachment_key, task_id, waived_at) VALUES (?1, ?2, ?3, 'x', ?4, ?5, ?6)",
                rusqlite::params![run, stage, kind, key, task, waived.then_some(1)],
            )
            .unwrap();
        }
    }
    tx.commit().unwrap();
}

#[test]
fn the_overview_matches_blocked_by_and_stays_fast_with_500_runs() {
    let dir = tempfile::tempdir().unwrap();
    let ai = Rec::new();
    drop(open(dir.path(), &ai));
    synthetic_runs(dir.path(), 500, true);
    let app = open(dir.path(), &ai);
    let started = std::time::Instant::now();
    let overview = app.pipeline_overview().unwrap();
    let elapsed = started.elapsed();
    eprintln!("pipeline_overview con 500 ejecuciones sintéticas: {elapsed:?}");
    assert_eq!(overview.runs.len(), 500);
    assert!(elapsed < std::time::Duration::from_secs(2), "{elapsed:?}");
    drop(app);

    // La vista dice lo mismo que `blocked_by` y que el predicado de «lista».
    let store = Store::open(&dir.path().join("nestadmin.db")).unwrap();
    let ready: std::collections::HashSet<i64> =
        store.ready_tasks().unwrap().iter().map(|t| t.id).collect();
    let mut checked = 0;
    for run in &overview.runs {
        for step in &run.steps {
            let (Some(task_id), Some(TaskState::Queued)) = (step.task_id, step.task_state) else {
                continue;
            };
            let expected: Vec<(i64, Option<TaskState>)> = store
                .blocked_by(task_id)
                .unwrap()
                .iter()
                .map(|b| (b.step_id, b.task_state))
                .collect();
            let got: Vec<(i64, Option<TaskState>)> = step
                .blocked
                .iter()
                .flat_map(|b| b.by.iter().map(|x| (x.step_id, x.task_state)))
                .collect();
            assert_eq!(got, expected, "tarea {task_id}");
            assert_eq!(
                step.blocked.is_none(),
                ready.contains(&task_id),
                "tarea {task_id}"
            );
            checked += 1;
        }
    }
    assert!(checked >= 500, "{checked}");
}

#[test]
fn a_part_of_an_invoice_cancelled_while_running_creates_no_new_tasks() {
    let dir = tempfile::tempdir().unwrap();
    let ai = Rec::new();
    let mut app = open(dir.path(), &ai);
    let imported = app
        .import_document("dos.txt", &doc_multi("Zeta SL", &[1, 2]))
        .unwrap();
    let job = app.begin_next().unwrap().unwrap();
    assert_eq!(app.reject(imported.id).unwrap().state, TaskState::Cancelled);
    let _ = app.finish(job.run());
    assert_eq!(task(&app, imported.id).state, TaskState::Cancelled);
    assert_eq!(app.tasks().unwrap().len(), 1, "no se crean partes");
    // En el flujo normal la factura en curso sí las crea.
    let ok = app
        .import_document("tres.txt", &doc_multi("Zeta SL", &[3, 4]))
        .unwrap();
    process_all(&mut app);
    assert_eq!(task(&app, ok.id).state, TaskState::AwaitingReview);
    assert_eq!(app.tasks().unwrap().len(), 3);
}

#[test]
fn the_rule_simulation_does_not_count_a_reply_with_a_waived_step_as_automatable() {
    let dir = tempfile::tempdir().unwrap();
    let ai = Rec::new();
    let mut app = open(dir.path(), &ai);
    drain_samples(&mut app);
    template(&mut app, "zeta", json!({ "sender_domain": ZETA }));
    // Z-1: su adjunto está vacío (nunca se importará) y se continúa sin él. Z-2: normal.
    mail(&app, "Z-1", Some(SENDER), &[("a.txt", String::new())]);
    mail(&app, "Z-2", Some(SENDER), &[("b.txt", doc("Zeta SL", 2))]);
    app.fetch_mail().unwrap();
    let e1 = email_task(&app, "Z-1");
    let step = step_of(&run_of(&app, e1.id), "attachment_invoices").id;
    app.waive_pipeline_step(step, "").unwrap();
    process_all(&mut app);
    let invoice = invoice_tasks(&app, "Z-2").remove(0);
    approve(&mut app, invoice.id);
    process_all(&mut app);
    let e2 = email_task(&app, "Z-2");
    assert_eq!(task(&app, e1.id).state, TaskState::AwaitingReview);
    assert_eq!(task(&app, e2.id).state, TaskState::AwaitingReview);

    let conditions = json!({ "category": "solicitud_factura", "sender_domain": ZETA });
    let simulation = app
        .simulate_rule("send_draft", &conditions, None, false, None)
        .unwrap();
    let waived = simulation
        .items
        .iter()
        .find(|i| i.task_id == e1.id)
        .expect("la respuesta con paso omitido está en la simulación");
    assert_eq!(waived.outcome, SimulationOutcome::Review);
    assert_eq!(waived.reason_code, Some(ReasonCode::PipelineStepWaived));
    let normal = simulation
        .items
        .iter()
        .find(|i| i.task_id == e2.id)
        .unwrap();
    assert_eq!(normal.outcome, SimulationOutcome::Automate);
    assert_eq!(simulation.would_automate, 1);
}
