//! Preferencias por remitente (D161) y estado del hilo (D162) en la capa de aplicación, con un
//! proveedor y un correo simulados y datos sintéticos. Una prueba por fila de la precedencia
//! (primera lectura, rebote, hilo propio, preferencia, señal fuerte, IA), la compuerta de reglas
//! intacta, dominios públicos rechazados, claves normalizadas, hilo desconocido sin efecto y un
//! espía de que ningún contenido del hilo sale hacia la IA.

use std::path::Path;
use std::sync::{Arc, Mutex};

use nestadmin_ai::{
    AiError, GenerateRequest, GenerateResult, Provider, ProviderKind, Task as AiTask, Usage,
};
use nestadmin_app::initial_read::{ReadExpectation, READ_FIXED};
use nestadmin_app::panel::NewRule;
use nestadmin_app::rules::SEND_DRAFT;
use nestadmin_app::{App, AppPaths, EMAIL};
use nestadmin_core::{Task, TaskState};
use nestadmin_extract::{ExtractError, Extractor, Ocr};
use nestadmin_simulators::{FailureMode, MailSimulator, Message};
use serde_json::{json, Value};

// ---- Proveedor espía ----

struct Spy {
    requests: Mutex<Vec<GenerateRequest>>,
}

impl Spy {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            requests: Mutex::new(Vec::new()),
        })
    }
    /// Peticiones cuyo contenido lleva `needle`.
    fn seen(&self, needle: &str) -> usize {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.content.contains(needle) || r.instructions.contains(needle))
            .count()
    }
    fn requests_with(&self, needle: &str) -> Vec<GenerateRequest> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.content.contains(needle))
            .cloned()
            .collect()
    }
}

impl Provider for Spy {
    fn kind(&self) -> ProviderKind {
        ProviderKind::OllamaCloud
    }
    fn model(&self) -> &str {
        "simulado"
    }
    fn destination(&self) -> Option<String> {
        Some("ollama.test".into())
    }
    fn generate(&self, request: &GenerateRequest) -> Result<GenerateResult, AiError> {
        self.requests.lock().unwrap().push(request.clone());
        let output = match request.task {
            AiTask::Classification => {
                json!({ "category": "solicitud_factura", "needs_reply": true })
            }
            AiTask::Draft => json!({ "draft": "Adjuntamos la factura solicitada." }),
            AiTask::Extraction => json!({
                "supplier": "Proveedor Alfa Suministros SL", "invoice_number": "FS-2026-014",
                "date": "2026-10-01", "subtotal": "120.00", "tax": "25.20", "total": "145.20",
            }),
            _ => json!({}),
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

// ---- Entorno ----

fn msg(
    id: &str,
    from: &str,
    body: &str,
    thread: Option<&str>,
    headers: &[(&str, &str)],
) -> Message {
    Message {
        id: id.into(),
        subject: Some(format!("Asunto de {id}")),
        from: Some(from.into()),
        to: Some(vec!["administracion@nestadmin.test".into()]),
        body: Some(body.into()),
        date: Some("2026-10-01".into()),
        attachments: Vec::new(),
        signals: None,
        thread_key: thread.map(str::to_owned),
    }
    .with_checked_signals(headers)
}

fn seed(dir: &Path, messages: Vec<Message>) {
    let state = json!({
        "version": 1,
        "state": { "messages": messages, "drafts": [], "sent": [], "files": [] },
    });
    std::fs::write(dir.join("simulador-correo.json"), state.to_string()).unwrap();
}

fn open(dir: &Path, provider: Arc<dyn Provider>) -> App {
    let extractor = Extractor::new(dir.join("sin-pdfium"), Box::new(NoOcr));
    let mut app = App::open(&AppPaths::new(dir), provider, Arc::new(extractor)).unwrap();
    app.set_mail_limit(500).unwrap();
    app
}

fn process_all(app: &mut App) {
    while app.process_next().unwrap().is_some() {}
}

fn input_of(task: &Task) -> Value {
    serde_json::from_str(&task.input_ref).unwrap()
}

/// La tarea de correo de ese mensaje del simulador.
fn mail_task(app: &App, id: &str) -> Task {
    let wanted = format!("simulador:{id}");
    let mut found: Vec<Task> = app
        .tasks()
        .unwrap()
        .into_iter()
        .filter(|t| t.kind == EMAIL && input_of(t)["message_id"] == wanted)
        .collect();
    assert_eq!(found.len(), 1, "una sola tarea de {id}");
    found.remove(0)
}

fn result_of(task: &Task) -> Value {
    task.result.clone().unwrap()
}

fn sim(app: &App) -> &MailSimulator {
    app.simulators().0
}

fn count(dir: &Path, query: &str) -> i64 {
    rusqlite::Connection::open(dir.join("nestadmin.db"))
        .unwrap()
        .query_row(query, [], |r| r.get(0))
        .unwrap()
}

fn prefer(app: &mut App, scope: &str, key: &str, route: &str) {
    app.set_sender_preference(scope, key, route).unwrap();
}

fn send_rule(app: &mut App, id: &str, conditions: Value) {
    app.create_rule_full(NewRule {
        id: Some(id),
        name: None,
        action_kind: SEND_DRAFT,
        conditions: &conditions,
        allow_any_sender: false,
    })
    .unwrap();
    app.set_rule_active(id, true).unwrap();
}

const NOREPLY: &str = "Avisos <No-Reply@Servicio.test>";
const PERSON: &str = "Ana <ana@clienteficticio.test>";

// ---- API: claves, historial, dominios públicos, avisos ----

#[test]
fn keys_are_normalized_listed_audited_without_content_and_removable() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = open(dir.path(), Spy::new());

    let first = app
        .set_sender_preference(
            "address",
            "  \"Ana, S.A.\" <ANA@Cliente.TEST> ",
            "informative",
        )
        .unwrap();
    assert!(first.changed);
    assert_eq!(first.preference.key, "ana@cliente.test");
    assert!(first.warnings.is_empty());
    // Repetir lo mismo con otra grafía no cambia nada ni escribe otro evento.
    let again = app
        .set_sender_preference("address", "ana@cliente.test", "informative")
        .unwrap();
    assert!(!again.changed);
    // Por dominio se admite `Dominio`, `@dominio` o una dirección de él.
    for input in ["Cliente.TEST", "@cliente.test", "Otra <otra@CLIENTE.test>"] {
        let change = app
            .set_sender_preference("domain", input, "review")
            .unwrap();
        assert_eq!(change.preference.key, "cliente.test", "{input}");
    }
    let listed: Vec<_> = app
        .sender_preferences()
        .unwrap()
        .into_iter()
        .map(|p| (p.scope, p.key, p.route))
        .collect();
    assert_eq!(
        listed,
        vec![
            (
                "address".into(),
                "ana@cliente.test".into(),
                "informative".into()
            ),
            ("domain".into(), "cliente.test".into(), "review".into()),
        ]
    );

    // Entradas inválidas: ámbito, ruta o clave; nada se guarda.
    for (scope, key, route) in [
        ("otro", "ana@cliente.test", "review"),
        ("address", "ana@cliente.test", "siempre"),
        ("address", "sin-arroba", "review"),
        ("address", "", "review"),
        ("domain", "sin punto", "review"),
    ] {
        assert!(
            app.set_sender_preference(scope, key, route).is_err(),
            "{scope} {key:?} {route}"
        );
    }
    assert_eq!(app.sender_preferences().unwrap().len(), 2);

    assert!(app
        .remove_sender_preference("address", "ANA@cliente.test")
        .unwrap());
    assert!(!app
        .remove_sender_preference("address", "ana@cliente.test")
        .unwrap());
    assert!(app.remove_sender_preference("otro", "x").is_err());
    assert_eq!(app.sender_preferences().unwrap().len(), 1);

    // Un evento por cambio real (informative, review, quitar), con solo ámbito, clave y rutas.
    let events: Vec<_> = app
        .audit_log()
        .unwrap()
        .into_iter()
        .filter(|e| e.action == "sender_preference")
        .collect();
    assert_eq!(
        events.iter().map(|e| e.status.as_str()).collect::<Vec<_>>(),
        ["set", "set", "removed"]
    );
    for event in &events {
        let detail: Value = serde_json::from_str(event.detail.as_deref().unwrap()).unwrap();
        for field in detail.as_object().unwrap().keys() {
            assert!(
                ["scope", "key", "route", "previous"].contains(&field.as_str()),
                "{field}"
            );
        }
    }
}

#[test]
fn public_mail_domains_are_rejected_for_a_domain_but_an_address_there_is_allowed() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = open(dir.path(), Spy::new());
    for domain in [
        "gmail.com",
        "GMAIL.com",
        "@outlook.com",
        "Alguien <x@hotmail.es>",
        "yahoo.es",
    ] {
        let error = app
            .set_sender_preference("domain", domain, "informative")
            .unwrap_err()
            .to_string();
        assert!(error.contains("público"), "{domain}: {error}");
    }
    assert!(app.sender_preferences().unwrap().is_empty());
    assert_eq!(
        count(
            dir.path(),
            "SELECT COUNT(*) FROM audit_log WHERE action = 'sender_preference'"
        ),
        0
    );
    // Una dirección concreta de un servicio público sí.
    let change = app
        .set_sender_preference("address", "Boletin@Gmail.com", "informative")
        .unwrap();
    assert_eq!(change.preference.key, "boletin@gmail.com");
}

#[test]
fn a_preference_that_contradicts_an_active_send_rule_warns_but_still_saves() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = open(dir.path(), Spy::new());
    send_rule(
        &mut app,
        "por-direccion",
        json!({ "category": "solicitud_factura", "sender": "ana@cliente.test" }),
    );
    send_rule(
        &mut app,
        "por-dominio",
        json!({ "category": "solicitud_factura", "sender_domain": "cliente.test" }),
    );
    send_rule(
        &mut app,
        "otro-dominio",
        json!({ "category": "solicitud_factura", "sender_domain": "otro.test" }),
    );
    send_rule(
        &mut app,
        "inactiva",
        json!({ "category": "solicitud_factura", "sender": "ana@cliente.test" }),
    );
    app.set_rule_active("inactiva", false).unwrap();
    send_rule(
        &mut app,
        "archivada",
        json!({ "category": "solicitud_factura", "sender": "ana@cliente.test" }),
    );
    app.archive_rule("archivada").unwrap();

    let ids = |change: &nestadmin_app::preferences::PreferenceChange| {
        change
            .warnings
            .iter()
            .map(|w| w.rule_id.clone())
            .collect::<Vec<_>>()
    };
    let address = app
        .set_sender_preference("address", "Ana <ana@cliente.test>", "informative")
        .unwrap();
    assert_eq!(ids(&address), ["por-direccion", "por-dominio"]);
    let domain = app
        .set_sender_preference("domain", "cliente.test", "informative")
        .unwrap();
    assert_eq!(ids(&domain), ["por-direccion", "por-dominio"]);
    // Se guarda igualmente; el aviso no bloquea.
    assert_eq!(app.sender_preferences().unwrap().len(), 2);
    // «Siempre a Pendientes» no contradice una regla, y un remitente sin regla tampoco.
    let review = app
        .set_sender_preference("address", "ana@cliente.test", "review")
        .unwrap();
    assert!(review.warnings.is_empty());
    let other = app
        .set_sender_preference("domain", "sinreglas.test", "informative")
        .unwrap();
    assert!(other.warnings.is_empty());
}

// ---- Precedencia en la consulta del buzón ----

fn closed(task: &Task) -> Value {
    assert_eq!(task.state, TaskState::Completed, "{task:?}");
    assert_eq!(task.attempts, 0, "nunca pasó por la cola");
    result_of(task)
}

#[test]
fn row_preference_informative_closes_a_mail_without_signals_locally_by_address_and_by_domain() {
    let dir = tempfile::tempdir().unwrap();
    seed(
        dir.path(),
        vec![
            msg("P1", PERSON, "Cuerpo SENTINELA-P1", None, &[]),
            msg(
                "P2",
                "otra@dominio-marcado.test",
                "Cuerpo SENTINELA-P2",
                None,
                &[],
            ),
            msg(
                "P3",
                "otra@sinmarcar.test",
                "Cuerpo SENTINELA-P3",
                None,
                &[],
            ),
        ],
    );
    let spy = Spy::new();
    let mut app = open(dir.path(), spy.clone());
    prefer(
        &mut app,
        "address",
        "ANA@clienteficticio.test",
        "informative",
    );
    prefer(&mut app, "domain", "dominio-marcado.test", "informative");
    let report = app.fetch_mail().unwrap();
    assert_eq!(report.accounts[0].informative, 2);
    process_all(&mut app);

    let p1 = closed(&mail_task(&app, "P1"));
    assert_eq!(
        p1["triage"],
        json!({ "route": "informativo", "codes": [], "source": "preference", "preference_scope": "address" })
    );
    for absent in ["action", "draft", "category"] {
        assert!(p1.get(absent).is_none(), "{absent}");
    }
    let p2 = closed(&mail_task(&app, "P2"));
    assert_eq!(p2["triage"]["source"], "preference");
    assert_eq!(p2["triage"]["preference_scope"], "domain");
    // El tercero, sin preferencia, sigue el flujo normal.
    assert_ne!(mail_task(&app, "P3").attempts, 0);
    assert_eq!(spy.seen("SENTINELA-P1"), 0);
    assert_eq!(spy.seen("SENTINELA-P2"), 0);
    assert_eq!(spy.seen("SENTINELA-P3"), 2, "clasificación y redacción");
    // Una consulta posterior no duplica nada.
    app.fetch_mail().unwrap();
    assert_eq!(
        count(
            dir.path(),
            "SELECT COUNT(*) FROM tasks WHERE input_ref LIKE '%\"simulador:P1\"%'"
        ),
        1
    );
}

#[test]
fn the_address_preference_beats_the_domain_one_and_removing_it_restores_the_flow() {
    let dir = tempfile::tempdir().unwrap();
    seed(
        dir.path(),
        vec![
            msg(
                "A1",
                "ana@clienteficticio.test",
                "Cuerpo SENTINELA-A1",
                None,
                &[],
            ),
            msg(
                "A2",
                "luis@clienteficticio.test",
                "Cuerpo SENTINELA-A2",
                None,
                &[],
            ),
        ],
    );
    let spy = Spy::new();
    let mut app = open(dir.path(), spy.clone());
    prefer(&mut app, "domain", "clienteficticio.test", "informative");
    prefer(&mut app, "address", "ana@clienteficticio.test", "review");
    app.fetch_mail().unwrap();
    process_all(&mut app);
    // Ana: la dirección manda y va a la IA; Luis: el dominio lo cierra.
    assert_eq!(spy.seen("SENTINELA-A1"), 2);
    assert_eq!(
        result_of(&mail_task(&app, "A1"))["category"],
        "solicitud_factura"
    );
    assert_eq!(spy.seen("SENTINELA-A2"), 0);
    assert_eq!(
        closed(&mail_task(&app, "A2"))["triage"]["preference_scope"],
        "domain"
    );
}

#[test]
fn row_preference_review_treats_a_strong_signal_as_weak_goes_to_the_ai_and_the_rule_stays_blocked()
{
    let dir = tempfile::tempdir().unwrap();
    seed(
        dir.path(),
        vec![
            msg(
                "S1",
                "no-reply@servicio.test",
                "Cuerpo SENTINELA-S1",
                None,
                &[],
            ),
            msg(
                "S2",
                "Otros <no-reply@otro-servicio.test>",
                "Cuerpo SENTINELA-S2",
                None,
                &[],
            ),
        ],
    );
    let spy = Spy::new();
    let mut app = open(dir.path(), spy.clone());
    // Una regla que coincidiría con el correo si la compuerta lo permitiera.
    send_rule(
        &mut app,
        "avisos",
        json!({ "category": "solicitud_factura", "sender": "no-reply@servicio.test" }),
    );
    prefer(&mut app, "address", "no-reply@servicio.test", "review");
    let report = app.fetch_mail().unwrap();
    assert_eq!(
        report.accounts[0].informative, 1,
        "solo S2: sin preferencia"
    );
    process_all(&mut app);

    // S1 pasó por la IA (clasificación y redacción) y quedó en revisión, sin envío.
    assert_eq!(spy.seen("SENTINELA-S1"), 2);
    let s1 = mail_task(&app, "S1");
    assert_eq!(s1.state, TaskState::AwaitingReview);
    let result = result_of(&s1);
    assert_eq!(result["evaluation"]["code"], "automatic_sender");
    assert!(result
        .get("authorization")
        .is_none_or(|a| a["type"] != "rule"));
    assert_eq!(app.simulators().0.sent().unwrap().len(), 0);
    assert_eq!(count(dir.path(), "SELECT COUNT(*) FROM actions"), 0);
    // S2 sigue la ruta local de siempre.
    assert_eq!(closed(&mail_task(&app, "S2"))["triage"]["source"], "local");
    assert_eq!(spy.seen("SENTINELA-S2"), 0);
}

#[test]
fn a_review_preference_never_opens_the_gate_and_clean_mail_still_goes_by_rule() {
    // «Siempre a Pendientes» no cambia la compuerta (D158, D161): el correo limpio sale por regla
    // como siempre y el que lleva una señal de envío automático queda en revisión.
    let dir = tempfile::tempdir().unwrap();
    seed(
        dir.path(),
        vec![
            msg("C1", "cliente@clienteficticio.test", "Hola", None, &[]),
            msg(
                "C2",
                "cliente@clienteficticio.test",
                "Hola",
                None,
                &[("Auto-Submitted", "auto-generated")],
            ),
        ],
    );
    let mut app = open(dir.path(), Spy::new());
    send_rule(
        &mut app,
        "clientes",
        json!({ "category": "solicitud_factura", "sender": "cliente@clienteficticio.test" }),
    );
    prefer(
        &mut app,
        "address",
        "cliente@clienteficticio.test",
        "review",
    );
    app.fetch_mail().unwrap();
    process_all(&mut app);
    assert_eq!(
        mail_task(&app, "C1").state,
        TaskState::Completed,
        "limpio: sale por regla"
    );
    let c2 = mail_task(&app, "C2");
    assert_eq!(c2.state, TaskState::AwaitingReview);
    assert_eq!(result_of(&c2)["evaluation"]["code"], "automatic_sender");
    assert_eq!(app.simulators().0.sent().unwrap().len(), 1);
}

#[test]
fn row_bounce_wins_over_every_preference_and_over_the_thread() {
    let dir = tempfile::tempdir().unwrap();
    seed(
        dir.path(),
        vec![
            msg(
                "B1",
                "Mail Delivery <MAILER-DAEMON@servicio.test>",
                "Cuerpo SENTINELA-B1",
                Some("T-B1"),
                &[],
            ),
            msg(
                "B2",
                "Mail Delivery <mailer-daemon@dominio.test>",
                "Cuerpo SENTINELA-B2",
                Some("T-B2"),
                &[],
            ),
        ],
    );
    let spy = Spy::new();
    let mut app = open(dir.path(), spy.clone());
    sim(&app).set_thread_last_own("T-B1", Some(true));
    sim(&app).set_thread_last_own("T-B2", Some(true));
    prefer(
        &mut app,
        "address",
        "mailer-daemon@servicio.test",
        "informative",
    );
    prefer(&mut app, "domain", "dominio.test", "informative");
    app.fetch_mail().unwrap();
    process_all(&mut app);
    for id in ["B1", "B2"] {
        let task = mail_task(&app, id);
        assert_eq!(task.state, TaskState::AwaitingReview, "{id}");
        let result = result_of(&task);
        assert_eq!(result["triage"]["route"], "rebote", "{id}");
        assert_eq!(result["triage"]["source"], "local", "{id}");
        assert!(result["triage"].get("already_replied").is_none(), "{id}");
    }
    assert_eq!(spy.seen("SENTINELA-B"), 0);
    assert!(
        sim(&app).thread_lookups().is_empty(),
        "un rebote no consulta el hilo"
    );
    // Con «Siempre a Pendientes» también es un rebote.
    prefer(&mut app, "address", "mailer-daemon@servicio.test", "review");
}

#[test]
fn row_first_read_comes_before_preferences_and_the_thread() {
    let dir = tempfile::tempdir().unwrap();
    seed(
        dir.path(),
        vec![msg("L1", PERSON, "Cuerpo SENTINELA-L1", Some("T-L1"), &[])],
    );
    let spy = Spy::new();
    let mut app = open(dir.path(), spy.clone());
    prefer(
        &mut app,
        "address",
        "ana@clienteficticio.test",
        "informative",
    );
    sim(&app).set_thread_last_own("T-L1", Some(true));
    let expect = ReadExpectation {
        provider: "ollama_cloud".into(),
        model: "simulado".into(),
        host: "ollama.test".into(),
        volume: READ_FIXED,
    };
    app.start_initial_read("simulador", None, Some(&expect))
        .unwrap();
    process_all(&mut app);
    // El correo se leyó como primera lectura: ni el cuerpo ni el asunto salen y no es visible.
    assert_eq!(spy.seen("SENTINELA-L1"), 0);
    app.fetch_mail().unwrap();
    process_all(&mut app);
    assert!(app
        .tasks()
        .unwrap()
        .iter()
        .all(|t| input_of(t)["message_id"] != "simulador:L1"));
    assert!(
        !sim(&app).thread_lookups().contains(&"T-L1".to_owned()),
        "la primera lectura no consulta el hilo"
    );
    assert_eq!(
        count(
            dir.path(),
            "SELECT COUNT(*) FROM tasks WHERE input_ref LIKE '%simulador:L1%' AND input_ref LIKE '%initial_read%'"
        ),
        1
    );
    assert_eq!(
        count(
            dir.path(),
            "SELECT COUNT(*) FROM tasks WHERE input_ref LIKE '%simulador:L1%'"
        ),
        1,
        "ninguna tarea informativa añadida"
    );
}

#[test]
fn row_own_thread_closes_as_already_replied_without_the_ai_even_against_a_review_preference() {
    let dir = tempfile::tempdir().unwrap();
    seed(
        dir.path(),
        vec![
            msg("H1", PERSON, "Cuerpo SENTINELA-H1", Some("T-1"), &[]),
            msg("H2", NOREPLY, "Cuerpo SENTINELA-H2", Some("T-2"), &[]),
            msg(
                "H3",
                "luis@clienteficticio.test",
                "Cuerpo SENTINELA-H3",
                Some("T-3"),
                &[],
            ),
            msg(
                "H4",
                "eva@otro.test",
                "Cuerpo SENTINELA-H4",
                Some("T-4"),
                &[],
            ),
        ],
    );
    let spy = Spy::new();
    let mut app = open(dir.path(), spy.clone());
    for thread in ["T-1", "T-2", "T-3", "T-4"] {
        sim(&app).set_thread_last_own(thread, Some(true));
    }
    // El hilo va antes que cualquier preferencia, también «Siempre a Pendientes».
    prefer(&mut app, "address", "ana@clienteficticio.test", "review");
    prefer(&mut app, "address", "no-reply@servicio.test", "review");
    prefer(
        &mut app,
        "address",
        "luis@clienteficticio.test",
        "informative",
    );
    let report = app.fetch_mail().unwrap();
    assert_eq!(report.accounts[0].informative, 4);
    process_all(&mut app);

    for id in ["H1", "H2", "H3", "H4"] {
        let result = closed(&mail_task(&app, id));
        assert_eq!(result["triage"]["route"], "informativo", "{id}");
        assert_eq!(result["triage"]["source"], "thread", "{id}");
        assert_eq!(result["triage"]["already_replied"], true, "{id}");
        assert!(result["triage"].get("preference_scope").is_none(), "{id}");
        for absent in ["action", "draft", "category"] {
            assert!(result.get(absent).is_none(), "{id} {absent}");
        }
        assert_eq!(spy.seen(&format!("SENTINELA-{id}")), 0, "{id}");
    }
    assert_eq!(sim(&app).thread_lookups().len(), 4);
    // Una consulta posterior no duplica ni vuelve a preguntar por lo que ya tiene tarea.
    app.fetch_mail().unwrap();
    assert_eq!(
        count(
            dir.path(),
            "SELECT COUNT(*) FROM tasks WHERE input_ref LIKE '%\"simulador:H1\"%'"
        ),
        1
    );
    assert_eq!(sim(&app).thread_lookups().len(), 4);
}

#[test]
fn an_unknown_foreign_or_failing_thread_has_no_effect_and_never_hides_a_mail() {
    let dir = tempfile::tempdir().unwrap();
    seed(
        dir.path(),
        vec![
            msg("U1", PERSON, "Cuerpo SENTINELA-U1", Some("T-U1"), &[]),
            msg("U2", PERSON, "Cuerpo SENTINELA-U2", Some("T-U2"), &[]),
            msg("U3", PERSON, "Cuerpo SENTINELA-U3", Some("T-U3"), &[]),
            msg("U4", PERSON, "Cuerpo SENTINELA-U4", None, &[]),
            msg("U5", NOREPLY, "Cuerpo SENTINELA-U5", Some("T-U5"), &[]),
        ],
    );
    let spy = Spy::new();
    let mut app = open(dir.path(), spy.clone());
    sim(&app).set_thread_last_own("T-U1", None); // desconocido
    sim(&app).set_thread_last_own("T-U2", Some(false)); // la última palabra es ajena
    sim(&app).set_thread_failure("T-U3", FailureMode::Unavailable); // fallo del proveedor
    sim(&app).set_thread_failure("T-U5", FailureMode::Unauthorized); // credenciales
    app.fetch_mail().unwrap();
    process_all(&mut app);
    for id in ["U1", "U2", "U3", "U4"] {
        let task = mail_task(&app, id);
        assert_eq!(result_of(&task)["category"], "solicitud_factura", "{id}");
        assert!(result_of(&task).get("triage").is_none(), "{id}");
        assert_eq!(spy.seen(&format!("SENTINELA-{id}")), 2, "{id}");
    }
    // Sin clave de hilo no hay consulta; con un fallo, el correo automático sigue su ruta local.
    assert!(!sim(&app).thread_lookups().contains(&String::new()));
    assert_eq!(sim(&app).thread_lookups().len(), 4);
    let u5 = closed(&mail_task(&app, "U5"));
    assert_eq!(u5["triage"]["source"], "local");
    assert!(u5["triage"].get("already_replied").is_none());
}

#[test]
fn no_content_of_the_thread_reaches_the_ai_and_a_foreign_thread_sends_only_the_mail_itself() {
    let dir = tempfile::tempdir().unwrap();
    seed(
        dir.path(),
        vec![
            msg(
                "X1",
                "ana@clienteficticio.test",
                "Respuesta anterior SENTINELA-PREVIO",
                Some("T-X"),
                &[],
            ),
            msg(
                "X2",
                "ana@clienteficticio.test",
                "Pregunta nueva SENTINELA-NUEVO",
                Some("T-X"),
                &[],
            ),
        ],
    );
    let spy = Spy::new();
    let mut app = open(dir.path(), spy.clone());
    sim(&app).set_thread_last_own("T-X", Some(false));
    app.fetch_mail().unwrap();
    process_all(&mut app);

    let new = spy.requests_with("SENTINELA-NUEVO");
    assert_eq!(new.len(), 2, "clasificación y redacción del correo nuevo");
    for request in &new {
        assert!(!request.content.contains("SENTINELA-PREVIO"), "{request:?}");
        assert!(!request.instructions.contains("SENTINELA-PREVIO"));
        // Ni la clave del hilo ni su estado viajan.
        assert!(!request.content.contains("T-X") && !request.instructions.contains("T-X"));
        assert!(!request.content.contains("already_replied"));
    }
    for request in &spy.requests_with("SENTINELA-PREVIO") {
        assert!(!request.content.contains("SENTINELA-NUEVO"));
    }

    // Con hilo propio, el correo no llega a ninguna llamada.
    let dir2 = tempfile::tempdir().unwrap();
    seed(
        dir2.path(),
        vec![msg(
            "Y1",
            "ana@clienteficticio.test",
            "Texto SENTINELA-Y1",
            Some("T-Y"),
            &[],
        )],
    );
    let spy2 = Spy::new();
    let mut app2 = open(dir2.path(), spy2.clone());
    sim(&app2).set_thread_last_own("T-Y", Some(true));
    app2.fetch_mail().unwrap();
    process_all(&mut app2);
    assert_eq!(spy2.seen("SENTINELA-Y1"), 0);
    assert_eq!(spy2.seen("T-Y"), 0);
}

#[test]
fn a_cut_while_closing_by_preference_or_thread_leaves_nothing_half_done_and_never_reaches_the_ai() {
    let dir = tempfile::tempdir().unwrap();
    seed(
        dir.path(),
        vec![
            msg("K1", PERSON, "Cuerpo SENTINELA-K1", None, &[]),
            msg(
                "K2",
                "luis@otro.test",
                "Cuerpo SENTINELA-K2",
                Some("T-K2"),
                &[],
            ),
        ],
    );
    let spy = Spy::new();
    let mut app = open(dir.path(), spy.clone());
    prefer(
        &mut app,
        "address",
        "ana@clienteficticio.test",
        "informative",
    );
    sim(&app).set_thread_last_own("T-K2", Some(true));
    let conn = rusqlite::Connection::open(dir.path().join("nestadmin.db")).unwrap();
    conn.execute_batch(
        "CREATE TRIGGER cut BEFORE INSERT ON task_transitions WHEN NEW.to_state = 'completed'
         BEGIN SELECT RAISE(ABORT, 'corte simulado'); END;",
    )
    .unwrap();
    assert!(app.fetch_mail().is_err());
    process_all(&mut app);
    assert_eq!(spy.seen("SENTINELA-K"), 0);
    assert_eq!(
        count(
            dir.path(),
            "SELECT COUNT(*) FROM tasks WHERE input_ref LIKE '%SENTINELA-K%'"
        ),
        0,
        "no queda nada a medias"
    );
    conn.execute_batch("DROP TRIGGER cut;").unwrap();
    app.fetch_mail().unwrap();
    app.fetch_mail().unwrap();
    assert_eq!(
        closed(&mail_task(&app, "K1"))["triage"]["source"],
        "preference"
    );
    assert_eq!(closed(&mail_task(&app, "K2"))["triage"]["source"], "thread");
    process_all(&mut app);
    assert_eq!(spy.seen("SENTINELA-K"), 0);
}

#[test]
fn changing_a_preference_only_affects_new_mail_and_old_tasks_keep_their_result() {
    let dir = tempfile::tempdir().unwrap();
    seed(
        dir.path(),
        vec![msg("O1", PERSON, "Cuerpo SENTINELA-O1", None, &[])],
    );
    let spy = Spy::new();
    let mut app = open(dir.path(), spy.clone());
    app.fetch_mail().unwrap();
    process_all(&mut app);
    let before = mail_task(&app, "O1");
    prefer(
        &mut app,
        "address",
        "ana@clienteficticio.test",
        "informative",
    );
    app.fetch_mail().unwrap();
    let after = mail_task(&app, "O1");
    assert_eq!(
        (before.state, before.result.clone()),
        (after.state, after.result)
    );
}
