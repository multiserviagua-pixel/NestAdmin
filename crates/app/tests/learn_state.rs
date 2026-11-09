//! E4 (D167, D175, T33d, T33e): estado, reanudación y arranque de «Aprender a responder» con un
//! proveedor espía y datos sintéticos. Un corte en cada estado y la reanudación no duplican
//! llamadas, entradas del Historial ni el resultado; un cambio de destino retiene la fase; una
//! huella distinta exige una vista previa y un consentimiento nuevos; un buzón sin nada que
//! aprender termina en `nothing` con recuentos. La parte «conector sin Enviados» está en las
//! pruebas internas de `learn_state.rs` (un conector propio solo se puede inyectar desde el
//! crate).

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use nestadmin_ai::{
    AiError, GenerateRequest, GenerateResult, Provider, ProviderKind, Task as AiTask, Usage,
};
use nestadmin_app::initial_read::ReadExpectation;
use nestadmin_app::learn_consent::LearnExpectation;
use nestadmin_app::learn_state::{LearnChoiceArg, LearningStatus, LearningStep};
use nestadmin_app::refine::RefineMode;
use nestadmin_app::{App, AppPaths};
use nestadmin_extract::{ExtractError, Extractor, Ocr};
use nestadmin_simulators::{Message, SentMessage};
use serde_json::{json, Value};

const HOST: &str = "ollama.test";
const MODEL: &str = "modelo-sintetico";
const ACCOUNT: &str = "simulador";
const BODY_SENTINEL: &str = "CUERPO-SENTINELA-123";
const REPLY_SENTINEL: &str = "RESPUESTA-SENTINELA-456";

// ---- Proveedor espía ----

struct Spy {
    kind: ProviderKind,
    model: String,
    host: Option<String>,
    learn_calls: AtomicUsize,
    learn_failure: Mutex<Option<AiError>>,
}

impl Spy {
    fn make(kind: ProviderKind, model: &str, host: Option<&str>) -> Arc<Self> {
        Arc::new(Self {
            kind,
            model: model.to_owned(),
            host: host.map(str::to_owned),
            learn_calls: AtomicUsize::new(0),
            learn_failure: Mutex::new(None),
        })
    }
    fn external() -> Arc<Self> {
        Self::make(ProviderKind::OllamaCloud, MODEL, Some(HOST))
    }
    fn local() -> Arc<Self> {
        Self::make(ProviderKind::Ollama, "modelo-local", None)
    }
    fn calls(&self) -> usize {
        self.learn_calls.load(Ordering::SeqCst)
    }
    fn fail_learn(&self, error: AiError) {
        *self.learn_failure.lock().unwrap() = Some(error);
    }
}

impl Provider for Spy {
    fn kind(&self) -> ProviderKind {
        self.kind
    }
    fn model(&self) -> &str {
        &self.model
    }
    fn destination(&self) -> Option<String> {
        self.host.clone()
    }
    fn generate(&self, request: &GenerateRequest) -> Result<GenerateResult, AiError> {
        let output = if request.task == AiTask::LearnReplies {
            self.learn_calls.fetch_add(1, Ordering::SeqCst);
            if let Some(error) = self.learn_failure.lock().unwrap().clone() {
                return Err(error);
            }
            let pairs: Vec<Value> = request
                .content
                .lines()
                .filter_map(|l| l.strip_prefix("id: "))
                .map(|id| {
                    json!({ "pair_id": id, "category": "solicitud_factura", "needs_reply": true,
                            "reply_kind": "confirm" })
                })
                .collect();
            json!({ "style": { "tone": "cercano", "length": "breve", "greeting": "Hola,", "closing": "Un saludo" },
                    "pairs": pairs })
        } else {
            json!({ "category": "solicitud_factura", "draft": "Adjuntamos la factura solicitada." })
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

// ---- Ayudas ----

fn open(dir: &Path, provider: Arc<dyn Provider>) -> App {
    let extractor = Extractor::new(dir.join("sin-pdfium"), Box::new(NoOcr));
    let mut app = App::open(&AppPaths::new(dir), provider, Arc::new(extractor)).unwrap();
    app.set_refine_mode(RefineMode::OnDemand);
    app
}

fn process_all(app: &mut App) {
    while app.process_next().unwrap().is_some() {}
}

fn read_expectation(app: &App) -> ReadExpectation {
    let info = app.provider();
    ReadExpectation {
        provider: info.provider.to_string(),
        model: info.model,
        host: if info.provider == ProviderKind::Ollama {
            String::new()
        } else {
            HOST.into()
        },
        volume: 25,
    }
}

/// Primera lectura de la cuenta del simulador, hasta el final.
fn finish_first_read(app: &mut App) {
    let expect = read_expectation(app);
    app.start_initial_read(ACCOUNT, Some(25), Some(&expect))
        .unwrap();
    process_all(app);
}

fn seed_mail(dir: &Path, messages: Vec<Message>) {
    let state = json!({
        "version": 1,
        "state": { "messages": messages, "drafts": [], "sent": [], "files": [] },
    });
    std::fs::write(dir.join("simulador-correo.json"), state.to_string()).unwrap();
}

fn incoming(i: usize, from: &str, body: &str) -> Message {
    Message {
        id: format!("m{i}"),
        subject: Some(format!("Consulta {i}")),
        from: Some(from.into()),
        to: Some(vec!["administracion@nestadmin.test".into()]),
        body: Some(body.into()),
        date: Some("2026-11-01T10:00:00Z".into()),
        attachments: Vec::new(),
        signals: None,
        thread_key: Some(format!("t{i}")),
    }
    .with_checked_signals(&[])
}

fn automatic(i: usize) -> Message {
    incoming(i, "noreply@tienda.test", "Su pedido ha salido.")
        .with_checked_signals(&[("Auto-Submitted", "auto-generated")])
}

fn sent(app: &App, i: usize, body: &str) {
    app.simulators()
        .0
        .add_sent_message(SentMessage {
            id: format!("s{i}"),
            thread_key: format!("t{i}"),
            date: Some("2026-11-01T12:00:00Z".into()),
            body: Some(body.into()),
            to: vec![sender(i).into()],
        })
        .unwrap();
}

/// Remitente del recibido `i` de `mailbox_with_pairs`: a él va la respuesta.
fn sender(i: usize) -> &'static str {
    match i {
        1 => "ana@cliente.test",
        2 => "luis@proveedor.test",
        3 => "marta@empresa.test",
        6 => "eva@nueva.test",
        _ => "otro@cliente.test",
    }
}

/// Tres correos de personas con su respuesta propia, uno sin respuesta y uno automático.
fn mailbox_with_pairs(dir: &Path) {
    seed_mail(
        dir,
        vec![
            incoming(
                1,
                "ana@cliente.test",
                &format!("{BODY_SENTINEL} ¿Confirman la cita?"),
            ),
            incoming(2, "luis@proveedor.test", "¿Pueden enviarme el presupuesto?"),
            incoming(3, "marta@empresa.test", "Necesito la factura de octubre."),
            incoming(4, "pedro@sinrespuesta.test", "¿Tienen novedades?"),
            automatic(5),
        ],
    );
}

fn add_replies(app: &App) {
    sent(app, 1, &format!("Confirmada. {REPLY_SENTINEL}"));
    sent(app, 2, "Se lo envío mañana.");
    sent(app, 3, "Le adjunto la factura.");
}

fn sql(dir: &Path) -> rusqlite::Connection {
    rusqlite::Connection::open(dir.join("nestadmin.db")).unwrap()
}

fn mark_and_result(dir: &Path) -> (Option<String>, Option<String>) {
    let get = |like: &str| {
        sql(dir)
            .query_row(
                "SELECT value FROM rule_settings WHERE key LIKE ?1",
                [like],
                |r| r.get::<_, String>(0),
            )
            .ok()
    };
    (get("first_learning:%"), get("first_learning_result:%"))
}

fn set_phase(dir: &Path, phase: &str) {
    let (mark, _) = mark_and_result(dir);
    let mut value: Value = serde_json::from_str(&mark.unwrap()).unwrap();
    value["phase"] = json!(phase);
    sql(dir)
        .execute(
            "UPDATE rule_settings SET value = ?1 WHERE key LIKE 'first_learning:%'",
            [value.to_string()],
        )
        .unwrap();
}

fn learning_entries(app: &App) -> Vec<(String, String)> {
    app.audit_log()
        .unwrap()
        .into_iter()
        .filter(|e| e.action == "first_learning")
        .map(|e| (e.status, e.detail.unwrap_or_default()))
        .collect()
}

fn status(app: &App) -> LearningStatus {
    app.learning_status(ACCOUNT).unwrap()
}

fn expectation(app: &mut App) -> LearnExpectation {
    let preview = app
        .account_learning_preview(ACCOUNT)
        .unwrap()
        .preview
        .expect("hay vista previa");
    LearnExpectation {
        provider: preview.provider.provider.to_string(),
        model: preview.provider.model,
        host: preview.host.unwrap_or_default(),
        input_fp: preview.input_fp,
    }
}

/// Un paso completo para las pruebas: la recogida avanza por pasos (una página o un candidato
/// por llamada), así que se repite hasta que no queda nada en curso. `Progress` si hizo algo,
/// `Idle` si no, y `Job` si hay una llamada que ejecutar.
fn tick(app: &mut App) -> LearningStep {
    let mut worked = false;
    loop {
        match app.learning_tick().unwrap() {
            LearningStep::Progress => worked = true,
            LearningStep::Idle if worked => return LearningStep::Progress,
            other => return other,
        }
    }
}

fn is_progress(step: LearningStep) -> bool {
    matches!(step, LearningStep::Progress)
}

fn is_idle(step: LearningStep) -> bool {
    matches!(step, LearningStep::Idle)
}

// ---- T33d: un corte en cada estado ----

#[test]
fn a_cut_in_every_state_resumes_without_duplicating_calls_entries_or_results() {
    let dir = tempfile::tempdir().unwrap();
    mailbox_with_pairs(dir.path());
    let spy = Spy::external();
    let mut app = open(dir.path(), spy.clone());
    add_replies(&app);

    // Antes de terminar la primera lectura no se recoge nada.
    assert_eq!(status(&app).state, "not_started");
    assert!(!status(&app).read_done);
    assert!(is_idle(tick(&mut app)));
    finish_first_read(&mut app);
    assert!(status(&app).read_done);

    // Recoger: con pares y destino externo queda esperando; no se envía nada.
    assert!(is_progress(tick(&mut app)));
    let s = status(&app);
    assert_eq!(s.state, "awaiting_consent");
    assert_eq!((s.pairs, s.counts.as_ref().unwrap().automatic), (3, 1));
    assert_eq!(s.counts.as_ref().unwrap().no_own_reply, 1);
    let first_fp = s.input_fp.clone().unwrap();
    assert_eq!(spy.calls(), 0);
    assert!(is_idle(tick(&mut app)), "una sola vez por proceso");
    assert!(learning_entries(&app).is_empty());

    // La marca guarda ids, recuentos y huella; nunca cuerpos.
    let (mark, result) = mark_and_result(dir.path());
    let mark = mark.unwrap();
    assert!(result.is_none());
    for text in [BODY_SENTINEL, REPLY_SENTINEL, "payload", "<<<CORREO>>>"] {
        assert!(!mark.contains(text), "la marca lleva {text}");
    }
    let value: Value = serde_json::from_str(&mark).unwrap();
    assert_eq!(value["phase"], "awaiting_consent");
    assert_eq!(value["input_fp"], json!(first_fp));
    assert_eq!(
        value["message_ids"],
        json!(["simulador:m1", "simulador:m2", "simulador:m3"])
    );
    assert_eq!(value["counts"]["pairs"], 3);
    assert_eq!(value["destination"]["host"], HOST);

    // Corte en `awaiting_consent`: al reabrir se recoge de nuevo; misma huella, mismo estado.
    drop(app);
    let mut app = open(dir.path(), spy.clone());
    assert_eq!(status(&app).state, "awaiting_consent");
    assert!(is_progress(tick(&mut app)));
    assert_eq!(status(&app).input_fp.as_deref(), Some(first_fp.as_str()));
    assert!(is_idle(tick(&mut app)));
    assert_eq!((spy.calls(), learning_entries(&app).len()), (0, 0));

    // Consentimiento ligado a la huella; un corte lo conserva si nada cambió.
    let seen = expectation(&mut app);
    assert_eq!(seen.input_fp, first_fp);
    assert_eq!(
        app.grant_account_learning_consent(ACCOUNT, &seen)
            .unwrap()
            .state,
        "granted"
    );
    drop(app);
    let mut app = open(dir.path(), spy.clone());
    assert!(is_progress(tick(&mut app)));
    let preview = app.account_learning_preview(ACCOUNT).unwrap();
    assert_eq!(preview.preview.unwrap().consent.state, "granted");

    // Llega correo nuevo: la huella cambia, la vista previa nueva sustituye a la anterior y el
    // consentimiento anterior no vale. Enviar con lo que se vio antes se rechaza sin llamar.
    app.simulators()
        .0
        .add_message(incoming(6, "eva@nueva.test", "Una consulta nueva."))
        .unwrap();
    sent(&app, 6, "Respondida.");
    drop(app);
    let mut app = open(dir.path(), spy.clone());
    assert!(is_progress(tick(&mut app)));
    let s = status(&app);
    assert_eq!((s.state, s.pairs), ("awaiting_consent", 4));
    assert_ne!(s.input_fp.as_deref(), Some(first_fp.as_str()));
    let stale = app.account_learning_preview(ACCOUNT).unwrap();
    assert_eq!(stale.preview.unwrap().consent.state, "stale");
    assert!(app
        .run_account_learning(ACCOUNT, &LearnChoiceArg::Send { expect: seen })
        .is_err());
    assert!(app
        .run_account_learning(
            ACCOUNT,
            &LearnChoiceArg::UseLocal {
                input_fp: first_fp.clone()
            }
        )
        .is_err());
    assert_eq!(spy.calls(), 0, "nada salió con una huella caducada");

    // Corte en `collected`: se reanuda igual que la primera vez (destino externo: espera).
    set_phase(dir.path(), "collected");
    drop(app);
    let mut app = open(dir.path(), spy.clone());
    assert_eq!(status(&app).state, "collected");
    assert!(is_progress(tick(&mut app)));
    assert_eq!(status(&app).state, "awaiting_consent");
    assert_eq!(spy.calls(), 0);

    // Nueva vista previa y nuevo consentimiento; ahora sí se envía, una sola vez.
    let seen = expectation(&mut app);
    app.grant_account_learning_consent(ACCOUNT, &seen).unwrap();
    let done = app
        .run_account_learning(ACCOUNT, &LearnChoiceArg::Send { expect: seen })
        .unwrap();
    assert_eq!((done.state, done.ai.as_deref()), ("done", Some("external")));
    assert_eq!(spy.calls(), 1);
    assert_eq!(learning_entries(&app).len(), 1);
    // B4: el `Debug` de lo aprendido y de los estados no muestra direcciones ni texto.
    let stored = app.learned_result(ACCOUNT).unwrap().unwrap();
    // (El estado puede llevar la dirección del propio buzón; lo aprendido, ninguna.)
    let shown = format!("{stored:?} {:?}", status(&app));
    assert!(!format!("{stored:?}").contains('@'), "{shown}");
    assert!(!shown.contains("cliente.test"), "{shown}");
    let (mark, result) = mark_and_result(dir.path());
    let result_text = result.unwrap();
    let result: Value = serde_json::from_str(&result_text).unwrap();
    assert_eq!(result["source"], "external");
    // Enums cerrados; el saludo y la despedida que inventa el modelo no coinciden con ninguna
    // línea de dos respuestas propias y se descartan.
    assert_eq!(result["style"]["tone"], "cercano");
    assert_eq!(result["style"]["length"], "breve");
    assert!(result["style"]["greeting"].is_null());
    assert!(result["style"]["closing"].is_null());
    let pairs = result["pairs"].as_array().unwrap();
    assert_eq!(pairs.len(), 4);
    // Por id de mensaje real y por dirección real, no por alias; solo enums.
    assert_eq!(pairs[0]["message_id"], "simulador:m1");
    assert_eq!(pairs[0]["address"], "ana@cliente.test");
    assert_eq!(pairs[0]["category"], "solicitud_factura");
    assert_eq!(pairs[0]["needs_reply"], true);
    assert_eq!(pairs[0]["reply_kind"], "confirm");
    for text in [BODY_SENTINEL, REPLY_SENTINEL, "<<<CORREO>>>"] {
        assert!(!result_text.contains(text) && !mark.as_ref().unwrap().contains(text));
    }
    let entry = learning_entries(&app).remove(0);
    assert_eq!(entry.0, "done");
    assert!(!entry.1.contains(BODY_SENTINEL) && !entry.1.contains('@'));

    // Corte en `done`: no vuelve a ejecutarse ni a escribir en el Historial.
    for _ in 0..2 {
        drop(app);
        app = open(dir.path(), spy.clone());
        assert!(is_idle(tick(&mut app)));
        assert_eq!(app.run_learning_ticks().unwrap(), 0);
        assert_eq!(status(&app).state, "done");
    }
    assert_eq!(spy.calls(), 1);
    assert_eq!(learning_entries(&app).len(), 1);
    assert_eq!(mark_and_result(dir.path()).1.unwrap(), result_text);
    assert!(app
        .run_account_learning(
            ACCOUNT,
            &LearnChoiceArg::UseLocal {
                input_fp: "x".into()
            }
        )
        .is_err());
    assert_eq!(app.learned_result(ACCOUNT).unwrap().unwrap().pairs.len(), 4);
}

#[test]
fn a_cut_in_sent_is_never_repeated_by_itself_and_the_person_decides() {
    let dir = tempfile::tempdir().unwrap();
    mailbox_with_pairs(dir.path());
    let spy = Spy::external();
    let mut app = open(dir.path(), spy.clone());
    add_replies(&app);
    finish_first_read(&mut app);
    tick(&mut app);
    let seen = expectation(&mut app);
    app.grant_account_learning_consent(ACCOUNT, &seen).unwrap();

    // Corte 1: la llamada se preparó (`sent`) pero no llegó a salir.
    let job = app
        .begin_account_learning(ACCOUNT, &LearnChoiceArg::Send { expect: seen })
        .unwrap();
    assert_eq!(status(&app).state, "sent");
    assert!(!status(&app).interrupted, "hay un trabajo en curso");
    drop(job);
    // Con el trabajo en curso no se acepta otra orden.
    assert!(app
        .run_account_learning(
            ACCOUNT,
            &LearnChoiceArg::UseLocal {
                input_fp: "x".into()
            }
        )
        .is_err());
    drop(app);
    let mut app = open(dir.path(), spy.clone());
    assert_eq!(status(&app).state, "sent");
    assert!(status(&app).interrupted);
    assert!(is_idle(tick(&mut app)));
    assert_eq!(app.run_learning_ticks().unwrap(), 0);
    assert_eq!((spy.calls(), learning_entries(&app).len()), (0, 0));

    // Reintentar vuelve a recoger y pide decidir de nuevo; no llama a la IA.
    assert_eq!(
        app.retry_learning(ACCOUNT).unwrap().state,
        "awaiting_consent"
    );
    assert_eq!(spy.calls(), 0);

    // Corte 2: la llamada salió pero no se guardó el resultado. El consentimiento anterior se
    // consumió al pasar a `sent`: hace falta uno nuevo.
    let seen = expectation(&mut app);
    app.grant_account_learning_consent(ACCOUNT, &seen).unwrap();
    let job = app
        .begin_account_learning(ACCOUNT, &LearnChoiceArg::Send { expect: seen })
        .unwrap();
    let completed = job.run();
    assert_eq!(spy.calls(), 1);
    drop(completed);
    drop(app);
    let mut app = open(dir.path(), spy.clone());
    assert!(status(&app).interrupted);
    assert!(is_idle(tick(&mut app)));
    assert_eq!(spy.calls(), 1, "reanudar no vuelve a llamar");
    assert!(mark_and_result(dir.path()).1.is_none());
    assert!(learning_entries(&app).is_empty());

    // Solo la persona puede repetirlo; el resultado y el Historial quedan una sola vez.
    app.retry_learning(ACCOUNT).unwrap();
    let seen = expectation(&mut app);
    // Sin un consentimiento nuevo no se reenvía: el anterior se consumió al pasar a `sent`.
    let held_back = app
        .run_account_learning(
            ACCOUNT,
            &LearnChoiceArg::Send {
                expect: seen.clone(),
            },
        )
        .unwrap();
    assert_eq!(held_back.state, "awaiting_consent");
    assert_eq!(spy.calls(), 1, "sin consentimiento nuevo no sale nada");
    app.retry_learning(ACCOUNT).ok();
    let seen = expectation(&mut app);
    app.grant_account_learning_consent(ACCOUNT, &seen).unwrap();
    let done = app
        .run_account_learning(ACCOUNT, &LearnChoiceArg::Send { expect: seen })
        .unwrap();
    assert_eq!(done.state, "done");
    assert_eq!(spy.calls(), 2);
    assert_eq!(learning_entries(&app).len(), 1);
}

#[test]
fn a_failed_call_waits_for_the_person_and_is_not_retried_by_itself() {
    let dir = tempfile::tempdir().unwrap();
    mailbox_with_pairs(dir.path());
    let spy = Spy::external();
    let mut app = open(dir.path(), spy.clone());
    add_replies(&app);
    finish_first_read(&mut app);
    tick(&mut app);
    let seen = expectation(&mut app);
    app.grant_account_learning_consent(ACCOUNT, &seen).unwrap();

    // Un fallo que no activa el respaldo local (sin IA local): sin aprender, con el código.
    spy.fail_learn(AiError::Unauthorized);
    let failed = app
        .run_account_learning(ACCOUNT, &LearnChoiceArg::Send { expect: seen })
        .unwrap();
    assert_eq!(failed.state, "awaiting_consent");
    assert!(failed.failure.is_some());
    assert_eq!(spy.calls(), 1);
    // B1: una clave que no sirve fija la espera de IA, como en las demás tareas.
    assert_eq!(app.ai_wait(), Some(nestadmin_app::AiWait::InvalidKey));
    // El consentimiento se consumió al enviar: repetir sin uno nuevo no reenvía nada (B3).
    app.retry_learning(ACCOUNT).unwrap();
    let again = expectation(&mut app);
    app.set_ai_wait(None);
    let held = app
        .run_account_learning(ACCOUNT, &LearnChoiceArg::Send { expect: again })
        .unwrap();
    assert_eq!(held.state, "awaiting_consent");
    assert_eq!(spy.calls(), 1, "sin consentimiento nuevo no se reenvía");
    // Para el resto de la prueba, como antes del reintento.
    app.set_ai_wait(None);
    assert!(learning_entries(&app).is_empty());
    assert!(mark_and_result(dir.path()).1.is_none());

    // Ni al reanudar ni en los pasos siguientes se vuelve a llamar.
    assert!(is_idle(tick(&mut app)));
    drop(app);
    let mut app = open(dir.path(), spy.clone());
    app.run_learning_ticks().unwrap();
    assert_eq!(spy.calls(), 1);
    assert!(status(&app).failure.is_some());
    assert!(app.retry_learning(ACCOUNT).is_ok());
    assert!(status(&app).failure.is_none());
    assert_eq!(spy.calls(), 1);
}

#[test]
fn a_change_of_destination_holds_the_phase_until_a_new_preview() {
    let dir = tempfile::tempdir().unwrap();
    mailbox_with_pairs(dir.path());
    let spy = Spy::external();
    let mut app = open(dir.path(), spy.clone());
    add_replies(&app);
    finish_first_read(&mut app);
    tick(&mut app);
    let seen = expectation(&mut app);
    app.grant_account_learning_consent(ACCOUNT, &seen).unwrap();
    assert_eq!(status(&app).state, "awaiting_consent");

    // Otro servidor: la fase queda retenida (`held`) y nada sale.
    let other = Spy::make(ProviderKind::OllamaCloud, "otro-modelo", Some("otro.test"));
    app.select_provider(other.clone());
    let held = status(&app);
    assert_eq!(held.state, "held");
    let reason = held.held.as_ref().unwrap();
    assert_eq!(reason.reason, "destination_changed");
    assert_eq!(
        reason.authorized.as_ref().unwrap().host.as_deref(),
        Some(HOST)
    );
    assert_eq!(reason.now.host.as_deref(), Some("otro.test"));
    assert!(is_idle(tick(&mut app)));
    assert!(app
        .run_account_learning(ACCOUNT, &LearnChoiceArg::Send { expect: seen })
        .is_err());
    assert_eq!((spy.calls(), other.calls()), (0, 0));
    // También tras un corte.
    drop(app);
    let mut app = open(dir.path(), other.clone());
    assert_eq!(status(&app).state, "held");
    assert!(is_idle(tick(&mut app)));

    // Una vista previa nueva fija el destino de ahora y el consentimiento no vale.
    let preview = app.account_learning_preview(ACCOUNT).unwrap();
    assert_eq!(preview.status.state, "awaiting_consent");
    assert_eq!(preview.preview.unwrap().consent.state, "missing");
    assert_eq!((spy.calls(), other.calls()), (0, 0));
}

#[test]
fn a_local_destination_runs_locally_once_and_never_again() {
    let dir = tempfile::tempdir().unwrap();
    mailbox_with_pairs(dir.path());
    let spy = Spy::local();
    let mut app = open(dir.path(), spy.clone());
    add_replies(&app);
    finish_first_read(&mut app);

    assert!(app.run_learning_ticks().unwrap() >= 1);
    let done = status(&app);
    assert_eq!((done.state, done.ai.as_deref()), ("done", Some("local")));
    assert_eq!(spy.calls(), 1);
    assert_eq!(learning_entries(&app).len(), 1);
    assert_eq!(app.learned_result(ACCOUNT).unwrap().unwrap().pairs.len(), 3);

    for _ in 0..2 {
        drop(app);
        app = open(dir.path(), spy.clone());
        assert_eq!(app.run_learning_ticks().unwrap(), 0);
    }
    assert_eq!((spy.calls(), learning_entries(&app).len()), (1, 1));

    // Corte entre recoger y decidir (`collected`) con destino local: se ejecuta en local, una vez.
    let dir = tempfile::tempdir().unwrap();
    mailbox_with_pairs(dir.path());
    let spy = Spy::local();
    let mut app = open(dir.path(), spy.clone());
    add_replies(&app);
    finish_first_read(&mut app);
    // Sin llamar a la IA: se deja la marca en `collected` como si se hubiera cortado ahí.
    match tick(&mut app) {
        LearningStep::Job(job) => drop(job),
        _ => panic!("con destino local se espera el trabajo"),
    }
    assert_eq!(spy.calls(), 0);
    set_phase(dir.path(), "collected");
    drop(app);
    let mut app = open(dir.path(), spy.clone());
    assert!(app.run_learning_ticks().unwrap() >= 1);
    assert_eq!((status(&app).state, spy.calls()), ("done", 1));
    assert_eq!(learning_entries(&app).len(), 1);
}

#[test]
fn a_failed_local_call_is_not_repeated_by_the_ticks() {
    let dir = tempfile::tempdir().unwrap();
    mailbox_with_pairs(dir.path());
    let spy = Spy::local();
    spy.fail_learn(AiError::Unavailable("sin servidor".into()));
    let mut app = open(dir.path(), spy.clone());
    add_replies(&app);
    finish_first_read(&mut app);
    app.run_learning_ticks().unwrap();
    assert_eq!(spy.calls(), 1);
    let s = status(&app);
    assert_eq!(s.state, "awaiting_consent");
    assert!(s.failure.is_some());
    for _ in 0..2 {
        drop(app);
        app = open(dir.path(), spy.clone());
        // Se vuelve a recoger una vez (vista previa vigente), pero nunca se llama a la IA.
        app.run_learning_ticks().unwrap();
        assert_eq!(status(&app).state, "awaiting_consent");
    }
    assert_eq!(spy.calls(), 1);
    assert!(learning_entries(&app).is_empty());
}

// ---- T33e: nada que aprender ----

#[test]
fn a_fully_automatic_mailbox_ends_in_nothing_with_counts_and_one_history_entry() {
    let dir = tempfile::tempdir().unwrap();
    seed_mail(dir.path(), vec![automatic(1), automatic(2), automatic(3)]);
    let spy = Spy::external();
    let mut app = open(dir.path(), spy.clone());
    finish_first_read(&mut app);

    assert!(is_progress(tick(&mut app)));
    let s = status(&app);
    assert_eq!(s.state, "nothing");
    let counts = s.counts.unwrap();
    assert_eq!(
        (counts.automatic, counts.pairs, counts.candidates),
        (3, 0, 0)
    );
    assert!(counts.scanned >= 3);
    assert_eq!(spy.calls(), 0, "no sale nada");
    assert!(s.input_fp.is_none());
    let entries = learning_entries(&app);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].0, "nothing");
    assert!(entries[0].1.contains("nada que aprender") && entries[0].1.contains("3 automáticos"));
    assert!(app.learned_result(ACCOUNT).unwrap().is_none());

    // No hay nada que previsualizar, consentir ni enviar, y reanudar no repite nada.
    let preview = app.account_learning_preview(ACCOUNT).unwrap();
    assert!(preview.preview.is_none());
    assert!(app.retry_learning(ACCOUNT).is_err());
    for _ in 0..2 {
        drop(app);
        app = open(dir.path(), spy.clone());
        assert!(is_idle(tick(&mut app)));
        assert_eq!(status(&app).state, "nothing");
    }
    assert_eq!((spy.calls(), learning_entries(&app).len()), (0, 1));
}

#[test]
fn a_mailbox_with_people_but_no_replies_ends_in_nothing_with_the_unanswered_count() {
    let dir = tempfile::tempdir().unwrap();
    seed_mail(
        dir.path(),
        vec![
            incoming(1, "ana@cliente.test", "Hola"),
            incoming(2, "luis@proveedor.test", "Buenas"),
            automatic(3),
        ],
    );
    let spy = Spy::external();
    let mut app = open(dir.path(), spy.clone());
    finish_first_read(&mut app);
    app.run_learning_ticks().unwrap();
    let s = status(&app);
    let counts = s.counts.unwrap();
    assert_eq!(s.state, "nothing");
    assert_eq!(
        (counts.automatic, counts.no_own_reply, counts.pairs),
        (1, 2, 0)
    );
    assert_eq!(spy.calls(), 0);
    assert!(learning_entries(&app)[0].1.contains("2 sin respuesta suya"));
}

#[test]
fn the_first_read_creates_no_tasks_for_learning_and_the_learning_creates_none() {
    let dir = tempfile::tempdir().unwrap();
    mailbox_with_pairs(dir.path());
    let spy = Spy::local();
    let mut app = open(dir.path(), spy.clone());
    add_replies(&app);
    finish_first_read(&mut app);
    let tasks = app.tasks().unwrap().len();
    let actions = sql(dir.path())
        .query_row("SELECT COUNT(*) FROM actions", [], |r| r.get::<_, i64>(0))
        .unwrap();
    app.run_learning_ticks().unwrap();
    assert_eq!(status(&app).state, "done");
    assert_eq!(app.tasks().unwrap().len(), tasks, "aprender no crea tareas");
    assert_eq!(
        sql(dir.path())
            .query_row("SELECT COUNT(*) FROM actions", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        actions
    );
    assert!(app.simulators().0.sent().unwrap().is_empty());
    assert!(app.simulators().0.drafts().unwrap().is_empty());
    assert!(app.rules().unwrap().is_empty(), "no crea reglas");
}

// ---- Revisión: consentimiento retirado, recogida por pasos ----

/// B3: si el consentimiento se retira entre preparar la llamada (`sent`) y ejecutarla, no sale
/// nada y la fase vuelve a esperar a la persona.
#[test]
fn a_consent_withdrawn_between_begin_and_run_sends_nothing() {
    let dir = tempfile::tempdir().unwrap();
    mailbox_with_pairs(dir.path());
    let spy = Spy::external();
    let mut app = open(dir.path(), spy.clone());
    add_replies(&app);
    finish_first_read(&mut app);
    tick(&mut app);
    let seen = expectation(&mut app);
    app.grant_account_learning_consent(ACCOUNT, &seen).unwrap();

    let job = app
        .begin_account_learning(
            ACCOUNT,
            &LearnChoiceArg::Send {
                expect: seen.clone(),
            },
        )
        .unwrap();
    assert_eq!(
        status(&app).state,
        "sent",
        "`sent` se anota antes de la llamada"
    );
    app.withdraw_learning_consent(&seen.input_fp).unwrap();
    let completed = job.run();
    assert_eq!(spy.calls(), 0, "retirado el consentimiento, no sale nada");
    let s = app.finish_learning(completed).unwrap();
    assert_eq!(s.state, "awaiting_consent");
    assert!(s.failure.unwrap().contains("no_consent"));
    assert!(learning_entries(&app).is_empty());
}

/// EXTRA: la recogida va por pasos (una página del buzón o un candidato), no retiene el cerrojo
/// de una vez, guarda el progreso solo en memoria y su resultado es idéntico al de la persona.
#[test]
fn the_collection_goes_step_by_step_and_matches_the_one_shot_result() {
    let dir = tempfile::tempdir().unwrap();
    let messages: Vec<Message> = (1..=45)
        .map(|i| {
            let mut m = incoming(
                i,
                &format!("c{i}@cliente.test"),
                &format!("{BODY_SENTINEL} consulta número {i}."),
            );
            m.date = Some("2026-11-01T10:00:00Z".into());
            m
        })
        .collect();
    seed_mail(dir.path(), messages);
    let spy = Spy::external();
    let mut app = open(dir.path(), spy.clone());
    for i in (1..=45).filter(|i| i % 4 != 0) {
        app.simulators()
            .0
            .add_sent_message(SentMessage {
                id: format!("s{i}"),
                thread_key: format!("t{i}"),
                date: Some("2026-11-01T12:00:00Z".into()),
                body: Some(format!("Respuesta número {i}.")),
                to: vec![format!("c{i}@cliente.test")],
            })
            .unwrap();
    }
    finish_first_read(&mut app);

    // Cada llamada a `learning_tick` es un paso: entre uno y otro la aplicación está libre y
    // no hay nada persistido (ni marca ni cuerpos).
    let mut steps = 0;
    while status(&app).state == "not_started" {
        match app.learning_tick().unwrap() {
            LearningStep::Progress => steps += 1,
            _ => panic!("la recogida debería avanzar paso a paso"),
        }
        if status(&app).state == "not_started" {
            assert!(
                mark_and_result(dir.path()).0.is_none(),
                "sin progreso guardado"
            );
            assert!(app.learning_overview().is_ok());
        }
    }
    assert!(
        steps > 10,
        "{steps} pasos: una página o un candidato por paso"
    );
    let s = status(&app);
    assert_eq!((s.state, s.pairs), ("awaiting_consent", 34));
    let stepped_fp = s.input_fp.clone().unwrap();
    let mark = mark_and_result(dir.path()).0.unwrap();
    assert!(!mark.contains(BODY_SENTINEL) && !mark.contains("Respuesta número"));

    // La persona, también por pasos: mismo resultado.
    assert!(app.start_account_learning_preview(ACCOUNT).unwrap());
    let mut person_steps = 0;
    while app.step_account_learning_preview(ACCOUNT).unwrap() {
        person_steps += 1;
        assert!(app.learning_overview().is_ok());
    }
    assert!(person_steps > 10, "{person_steps}");
    let view = app.account_learning_preview_view(ACCOUNT).unwrap();
    let preview = view.preview.expect("vista previa");
    assert_eq!(preview.input_fp, stepped_fp);
    assert_eq!(preview.pairs, 34);
    // Sin recogida en curso, un paso no hace nada.
    assert!(!app.step_account_learning_preview(ACCOUNT).unwrap());
    // Una sola vez y de golpe (lo que usan las pruebas y las llamadas sin ventana): igual.
    let once = app.account_learning_preview(ACCOUNT).unwrap();
    assert_eq!(once.preview.unwrap().input_fp, stepped_fp);
    assert_eq!(spy.calls(), 0, "recoger no llama a la IA");
}
