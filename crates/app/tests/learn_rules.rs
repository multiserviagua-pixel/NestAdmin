//! E6 (D173, D174; T33f, T33g): aceptar y activar en lote las reglas aprendidas y el Rodaje de sus
//! primeros envíos, con un proveedor espía y datos sintéticos. La aceptación es atómica, rechaza una
//! huella caducada, deshace todo si una regla falla y es idempotente; una regla aprendida activa pasa
//! siempre por la compuerta de envío y los 3 primeros envíos válidos van a revisión.

use std::path::Path;
use std::sync::{Arc, Mutex};

use nestadmin_ai::{
    AiError, GenerateRequest, GenerateResult, Provider, ProviderKind, Task as AiTask, Usage,
};
use nestadmin_app::initial_read::ReadExpectation;
use nestadmin_app::learn_rules::LearnedBatch;
use nestadmin_app::panel::NewRule;
use nestadmin_app::rules::SEND_DRAFT;
use nestadmin_app::{App, AppError, AppPaths, EMAIL};
use nestadmin_core::{Task, TaskState};
use nestadmin_extract::{ExtractError, Extractor, Ocr};
use nestadmin_simulators::{Message, SentMessage};
use rusqlite::OptionalExtension;
use serde_json::{json, Value};

const ACCOUNT: &str = "simulador";
const REPLY_SENTINEL: &str = "RESPUESTA-SENTINELA-456";
const RODAJE_TEXT: &str = "En rodaje: esta regla aprendida enviará sola a partir del envío n.º 4";

// ---- Proveedor espía ----

/// Aprende con marcas en el recibido (`CATOTROS`, `NOREPLY`, `KINDQUOTE`, `KINDINFORM`) y redacta un
/// borrador con enlace si el correo trae `CON-URL`.
struct Spy {
    requests: Mutex<Vec<GenerateRequest>>,
}

impl Spy {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            requests: Mutex::new(Vec::new()),
        })
    }
}

impl Provider for Spy {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Ollama
    }
    fn model(&self) -> &str {
        "modelo-local"
    }
    fn destination(&self) -> Option<String> {
        None
    }
    fn generate(&self, request: &GenerateRequest) -> Result<GenerateResult, AiError> {
        self.requests.lock().unwrap().push(request.clone());
        let output = match request.task {
            AiTask::LearnReplies => {
                let pairs: Vec<Value> = request
                    .content
                    .split("<<<CORREO>>>")
                    .filter_map(|block| {
                        let id = block.lines().find_map(|l| l.strip_prefix("id: "))?;
                        let category = if block.contains("CATOTROS") {
                            "otros"
                        } else {
                            "solicitud_factura"
                        };
                        let kind = if block.contains("KINDQUOTE") {
                            "quote"
                        } else if block.contains("KINDINFORM") {
                            "inform"
                        } else {
                            "confirm"
                        };
                        Some(json!({
                            "pair_id": id,
                            "category": category,
                            "needs_reply": !block.contains("NOREPLY"),
                            "reply_kind": kind,
                        }))
                    })
                    .collect();
                json!({ "style": { "tone": "formal", "length": "breve", "greeting": null, "closing": null },
                        "pairs": pairs })
            }
            AiTask::Classification => json!({
                "category": "solicitud_factura",
                "needs_reply": !request.content.contains("NOREPLY"),
            }),
            _ if request.content.contains("CON-URL") => {
                json!({ "draft": "Vea https://enlace.example/doc" })
            }
            _ => json!({ "draft": "Adjuntamos lo solicitado." }),
        };
        Ok(GenerateResult {
            request_id: request.request_id.clone(),
            provider: ProviderKind::Ollama,
            model: "modelo-local".into(),
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
    app.set_refine_mode(nestadmin_app::refine::RefineMode::OnDemand);
    app
}

fn process_all(app: &mut App) {
    while app.process_next().unwrap().is_some() {}
}

fn finish_first_read(app: &mut App) {
    let info = app.provider();
    let expect = ReadExpectation {
        provider: info.provider.to_string(),
        model: info.model,
        host: String::new(),
        volume: 25,
    };
    app.start_initial_read(ACCOUNT, Some(25), Some(&expect))
        .unwrap();
    process_all(app);
}

fn message(i: usize, from: &str, body: &str) -> Message {
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
}

/// Diez correos de personas, todos con su respuesta propia salvo el 9 y el 10: 1-3 Ana (alfa.test),
/// 4-5 Luis (alfa.test), 6-8 Marta (gmail.com, dominio público), 9 Pedro (no pide respuesta) y
/// 10 Eva (categoría `otros`). Salen tres candidatas: Ana, Marta y el dominio alfa.test.
fn senders() -> Vec<&'static str> {
    vec![
        "ana@alfa.test",
        "ana@alfa.test",
        "ana@alfa.test",
        "luis@alfa.test",
        "luis@alfa.test",
        "marta@gmail.com",
        "marta@gmail.com",
        "marta@gmail.com",
        "pedro@alfa.test",
        "eva@alfa.test",
    ]
}

fn mailbox(dir: &Path) {
    let mut messages = Vec::new();
    for (index, from) in senders().iter().enumerate() {
        let i = index + 1;
        let marks = match i {
            6 => " KINDQUOTE",
            9 => " NOREPLY",
            10 => " CATOTROS KINDINFORM",
            _ => "",
        };
        messages.push(message(i, from, &format!("Pregunta {i}.{marks}")).with_checked_signals(&[]));
    }
    let state = json!({
        "version": 1,
        "state": { "messages": messages, "drafts": [], "sent": [], "files": [] },
    });
    std::fs::write(dir.join("simulador-correo.json"), state.to_string()).unwrap();
}

fn add_replies(app: &App) {
    for (index, to) in senders().iter().enumerate() {
        let i = index + 1;
        app.simulators()
            .0
            .add_sent_message(SentMessage {
                id: format!("s{i}"),
                thread_key: format!("t{i}"),
                date: Some("2026-11-01T12:00:00Z".into()),
                body: Some(format!("Respuesta {i}. {REPLY_SENTINEL}")),
                to: vec![(*to).into()],
            })
            .unwrap();
    }
}

fn sql(dir: &Path) -> rusqlite::Connection {
    rusqlite::Connection::open(dir.join("nestadmin.db")).unwrap()
}

/// Instalación con el aprendizaje terminado (`done`) por la IA local.
fn learned() -> (tempfile::TempDir, App) {
    let dir = tempfile::tempdir().unwrap();
    mailbox(dir.path());
    let mut app = open(dir.path(), Spy::new());
    add_replies(&app);
    finish_first_read(&mut app);
    app.run_learning_ticks().unwrap();
    assert_eq!(app.learning_status(ACCOUNT).unwrap().state, "done");
    (dir, app)
}

fn audit(dir: &Path) -> Vec<(String, String, String)> {
    let conn = sql(dir);
    let mut stmt = conn
        .prepare("SELECT action, status, detail FROM audit_log ORDER BY id")
        .unwrap();
    let rows = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap();
    rows.map(Result::unwrap).collect()
}

fn count(dir: &Path, query: &str) -> i64 {
    sql(dir).query_row(query, [], |r| r.get(0)).unwrap()
}

fn rule_rows(dir: &Path) -> i64 {
    count(dir, "SELECT COUNT(*) FROM rules")
}

fn accepted_rows(dir: &Path) -> i64 {
    count(
        dir,
        "SELECT COUNT(*) FROM rule_suggestions WHERE status = 'accepted'",
    )
}

fn id_of(batch: &LearnedBatch, key: &str, value: &str) -> String {
    batch
        .items
        .iter()
        .find(|i| i.conditions[key] == value)
        .unwrap_or_else(|| panic!("falta la candidata {key}={value}"))
        .id
        .clone()
}

fn all_ids(batch: &LearnedBatch) -> Vec<String> {
    batch.items.iter().map(|i| i.id.clone()).collect()
}

fn rodaje_left(dir: &Path, rule_id: &str) -> Option<i64> {
    sql(dir)
        .query_row(
            "SELECT json_extract(origin_json, '$.rodaje.left') FROM rule_meta WHERE rule_id = ?1",
            [rule_id],
            |r| r.get(0),
        )
        .optional()
        .unwrap()
        .flatten()
}

fn set_rodaje(dir: &Path, rule_id: &str, left: i64) {
    sql(dir)
        .execute(
            "UPDATE rule_meta SET origin_json = json_set(origin_json, '$.rodaje.left', ?2) WHERE rule_id = ?1",
            rusqlite::params![rule_id, left],
        )
        .unwrap();
}

/// Acepta solo la regla de Ana (dirección exacta, la única que coincide con sus correos) y
/// devuelve su identificador de regla.
fn accept_ana(app: &mut App) -> String {
    let batch = app.learned_suggestions_batch().unwrap();
    let ana = id_of(&batch, "sender", "ana@alfa.test");
    let outcome = app
        .accept_and_activate_learned(&batch.batch_fp, &[ana])
        .unwrap();
    assert!(outcome.applied && outcome.activated == 1, "{outcome:?}");
    outcome.rules[0].rule_id.clone()
}

fn new_mail(app: &App, n: usize, from: &str, body: &str, headers: Option<&[(&str, &str)]>) {
    let mut m = message(n, from, body);
    if let Some(headers) = headers {
        m = m.with_checked_signals(headers);
    }
    app.simulators().0.add_message(m).unwrap();
}

/// La tarea del correo `m<n>`.
fn task_of(app: &App, n: usize) -> Task {
    let wanted = format!("m{n}\"");
    let mut found: Vec<Task> = app
        .tasks()
        .unwrap()
        .into_iter()
        .filter(|t| t.kind == EMAIL && t.input_ref.contains(&wanted))
        .collect();
    assert_eq!(found.len(), 1, "una tarea del correo m{n}");
    found.remove(0)
}

fn result_of(task: &Task) -> Value {
    task.result.clone().unwrap()
}

fn code(task: &Task) -> String {
    result_of(task)["evaluation"]["code"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn sent(app: &App) -> usize {
    app.simulators().0.sent().unwrap().len()
}

fn mail_and_process(app: &mut App, n: usize, body: &str, headers: Option<&[(&str, &str)]>) -> Task {
    new_mail(app, n, "ana@alfa.test", body, headers);
    app.fetch_mail().unwrap();
    process_all(app);
    task_of(app, n)
}

// ---- Lectura de la tanda ----

#[test]
fn the_batch_lists_the_pending_learned_rules_with_a_stable_fingerprint_and_no_content() {
    let (dir, app) = learned();
    let before = audit(dir.path()).len();
    let batch = app.learned_suggestions_batch().unwrap();
    assert_eq!(batch.items.len(), 3);
    assert_eq!(batch.rodaje_sends, 3);
    let ana = batch
        .items
        .iter()
        .find(|i| i.conditions["sender"] == "ana@alfa.test")
        .unwrap();
    assert_eq!((ana.count, ana.learned, ana.addresses), (3, 3, 1));
    let domain = batch
        .items
        .iter()
        .find(|i| i.conditions["sender_domain"] == "alfa.test")
        .unwrap();
    assert_eq!((domain.count, domain.addresses), (5, 2));

    // Estable: la misma huella mientras nada cambie, y solo lectura (nada se escribe).
    assert_eq!(batch, app.learned_suggestions_batch().unwrap());
    assert_eq!(audit(dir.path()).len(), before);
    assert_eq!(rule_rows(dir.path()), 0);

    // Solo recuentos y direcciones: ningún texto de los correos ni de las respuestas.
    let text = serde_json::to_string(&batch).unwrap();
    for leaked in [REPLY_SENTINEL, "Pregunta", "Respuesta 1", "Consulta"] {
        assert!(!text.contains(leaked), "se coló {leaked}");
    }
}

#[test]
fn the_fingerprint_changes_when_the_batch_changes() {
    let (_dir, mut app) = learned();
    let first = app.learned_suggestions_batch().unwrap();
    let ana = id_of(&first, "sender", "ana@alfa.test");
    app.discard_suggestion(&ana).unwrap();
    let second = app.learned_suggestions_batch().unwrap();
    assert_eq!(second.items.len(), 2);
    assert_ne!(first.batch_fp, second.batch_fp);
}

// ---- T33f: aceptar y activar en lote ----

#[test]
fn accepting_the_batch_activates_every_rule_with_rodaje_origin_and_audit_without_content() {
    let (dir, mut app) = learned();
    let batch = app.learned_suggestions_batch().unwrap();
    let outcome = app
        .accept_and_activate_learned(&batch.batch_fp, &all_ids(&batch))
        .unwrap();
    assert!(outcome.applied, "{outcome:?}");
    assert_eq!((outcome.activated, outcome.already_active), (3, 0));
    assert!(outcome.failed.is_none());
    assert!(outcome
        .rules
        .iter()
        .all(|r| r.status == "activated" && r.rodaje_left == Some(3)));

    // Las tres quedan activas: v1 inactiva y v2 activa.
    let rules = app.rules().unwrap();
    assert_eq!(rules.len(), 3);
    assert!(rules.iter().all(|r| r.active && r.version == 2));
    for r in &outcome.rules {
        let origin: Value = serde_json::from_str(
            &sql(dir.path())
                .query_row(
                    "SELECT origin_json FROM rule_meta WHERE rule_id = ?1",
                    [&r.rule_id],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
        )
        .unwrap();
        assert_eq!(origin["kind"], "suggestion");
        assert_eq!(origin["evidence_origin"], "learned");
        assert_eq!(origin["batch"], json!(true));
        assert_eq!(origin["rodaje"], json!({ "left": 3, "of": 3 }));
    }

    // Una auditoría por regla y una agregada en el Historial, sin contenido.
    let log = audit(dir.path());
    let per_rule = log
        .iter()
        .filter(|(a, s, _)| a == "rule_learned_batch" && s == "activated")
        .count();
    let aggregate: Vec<_> = log
        .iter()
        .filter(|(a, s, _)| a == "rule_learned_batch" && s == "activated_all")
        .collect();
    assert_eq!(per_rule, 3);
    assert_eq!(aggregate.len(), 1);
    assert!(aggregate[0].2.starts_with("3 reglas aprendidas"));
    for (_, _, detail) in &log {
        for leaked in [REPLY_SENTINEL, "Pregunta", "Respuesta", "Consulta"] {
            assert!(!detail.contains(leaked), "se coló {leaked} en {detail}");
        }
    }

    // La tanda ya no tiene nada pendiente.
    assert!(app.learned_suggestions_batch().unwrap().items.is_empty());
}

#[test]
fn a_stale_fingerprint_a_foreign_id_or_no_ids_are_refused_and_write_nothing() {
    let (dir, mut app) = learned();
    let batch = app.learned_suggestions_batch().unwrap();
    let ids = all_ids(&batch);
    let log = audit(dir.path()).len();

    for (fp, ids) in [
        ("0000000000000000".to_owned(), ids.clone()),
        (batch.batch_fp.clone(), vec!["no-existe".to_owned()]),
        (batch.batch_fp.clone(), vec![]),
        (batch.batch_fp.clone(), vec!["  ".to_owned()]),
    ] {
        assert!(
            matches!(
                app.accept_and_activate_learned(&fp, &ids),
                Err(AppError::Invalid(_))
            ),
            "{fp} {ids:?}"
        );
    }
    // La tanda cambia (se descarta una) y la huella vista antes caduca.
    app.discard_suggestion(&ids[0]).unwrap();
    assert!(app
        .accept_and_activate_learned(&batch.batch_fp, &ids[1..])
        .is_err());
    assert_eq!(rule_rows(dir.path()), 0);
    assert_eq!(accepted_rows(dir.path()), 0);
    assert_eq!(
        audit(dir.path()).len(),
        log + 1,
        "solo la entrada de descartar"
    );
}

#[test]
fn a_failing_check_rule_undoes_everything_and_names_the_rule() {
    let (dir, mut app) = learned();
    let batch = app.learned_suggestions_batch().unwrap();
    let marta = id_of(&batch, "sender", "marta@gmail.com");
    let log = audit(dir.path()).len();

    // La comprobación falla en la segunda regla: la primera no queda aceptada ni activa.
    let outcome = app
        .accept_and_activate_learned_with(&batch.batch_fp, &all_ids(&batch), &|kind, cond, any| {
            if cond["sender"] == "marta@gmail.com" {
                Err(AppError::Invalid("condiciones de prueba rechazadas".into()))
            } else {
                nestadmin_app::check_rule(kind, cond, any)
            }
        })
        .unwrap();
    assert!(!outcome.applied);
    assert!(outcome.rules.is_empty());
    let failed = outcome.failed.expect("dice qué regla falló");
    assert_eq!(failed.id, marta);
    assert_eq!(failed.code, "invalid_rule");
    assert!(failed.message.contains("condiciones de prueba rechazadas"));
    assert_eq!(rule_rows(dir.path()), 0);
    assert_eq!(accepted_rows(dir.path()), 0);
    assert_eq!(
        audit(dir.path()).len(),
        log,
        "nada que auditar: no pasó nada"
    );
    // Desmarcando la que falla, el resto se acepta con la misma tanda.
    let rest: Vec<String> = all_ids(&batch)
        .into_iter()
        .filter(|id| *id != marta)
        .collect();
    let ok = app
        .accept_and_activate_learned(&batch.batch_fp, &rest)
        .unwrap();
    assert!(ok.applied && ok.activated == 2, "{ok:?}");
}

#[test]
fn a_write_failure_after_the_first_rule_rolls_the_whole_transaction_back() {
    let (dir, mut app) = learned();
    let batch = app.learned_suggestions_batch().unwrap();
    let ids = all_ids(&batch);
    let log = audit(dir.path()).len();
    // La escritura de la última regla falla (otra conexión añade el disparador): las anteriores
    // ya estaban escritas en la misma transacción y deben deshacerse.
    let last = ids.last().unwrap().clone();
    sql(dir.path())
        .execute_batch(&format!(
            "CREATE TRIGGER prueba_fallo BEFORE INSERT ON rule_suggestions
             WHEN NEW.fingerprint = '{last}'
             BEGIN SELECT RAISE(ABORT, 'fallo de prueba'); END;"
        ))
        .unwrap();
    let outcome = app
        .accept_and_activate_learned(&batch.batch_fp, &ids)
        .unwrap();
    assert!(!outcome.applied);
    let failed = outcome.failed.unwrap();
    assert_eq!(
        (failed.id.as_str(), failed.code),
        (last.as_str(), "write_failed")
    );
    assert_eq!(rule_rows(dir.path()), 0, "las anteriores se deshicieron");
    assert_eq!(accepted_rows(dir.path()), 0);
    assert_eq!(count(dir.path(), "SELECT COUNT(*) FROM rule_meta"), 0);
    assert_eq!(audit(dir.path()).len(), log);

    // Sin el fallo, el mismo intento funciona y no queda nada a medias.
    sql(dir.path())
        .execute_batch("DROP TRIGGER prueba_fallo;")
        .unwrap();
    let ok = app
        .accept_and_activate_learned(&batch.batch_fp, &ids)
        .unwrap();
    assert!(ok.applied && ok.activated == 3, "{ok:?}");
}

#[test]
fn repeating_the_acceptance_creates_nothing_and_says_they_were_already_active() {
    let (dir, mut app) = learned();
    let batch = app.learned_suggestions_batch().unwrap();
    let ids = all_ids(&batch);
    let first = app
        .accept_and_activate_learned(&batch.batch_fp, &ids)
        .unwrap();
    assert_eq!(first.activated, 3);
    let rules = app.rules().unwrap();
    let log = audit(dir.path());

    // Mismo clic otra vez (doble clic, reintento tras un corte): ni reglas ni versiones ni
    // auditoría nuevas, y la respuesta lo dice.
    let again = app
        .accept_and_activate_learned(&batch.batch_fp, &ids)
        .unwrap();
    assert!(again.applied);
    assert_eq!((again.activated, again.already_active), (0, 3));
    assert!(again.rules.iter().all(|r| r.status == "already_active"));
    assert_eq!(
        again.rules.iter().map(|r| &r.rule_id).collect::<Vec<_>>(),
        first.rules.iter().map(|r| &r.rule_id).collect::<Vec<_>>()
    );
    assert_eq!(app.rules().unwrap(), rules);
    assert_eq!(audit(dir.path()), log);
    assert_eq!(accepted_rows(dir.path()), 3);
}

#[test]
fn a_subset_of_the_batch_works_and_the_rest_needs_a_fresh_fingerprint() {
    let (dir, mut app) = learned();
    let batch = app.learned_suggestions_batch().unwrap();
    let ana = id_of(&batch, "sender", "ana@alfa.test");
    let outcome = app
        .accept_and_activate_learned(&batch.batch_fp, std::slice::from_ref(&ana))
        .unwrap();
    assert!(outcome.applied && outcome.activated == 1);
    assert_eq!(app.rules().unwrap().iter().filter(|r| r.active).count(), 1);
    assert_eq!(accepted_rows(dir.path()), 1);

    // La tanda cambió (queda lo no aceptado): la huella vista antes ya no vale para el resto...
    let rest: Vec<String> = all_ids(&batch).into_iter().filter(|i| *i != ana).collect();
    assert!(app
        .accept_and_activate_learned(&batch.batch_fp, &rest)
        .is_err());
    // ...pero con la nueva sí, y repetir la primera sigue siendo idempotente.
    let fresh = app.learned_suggestions_batch().unwrap();
    assert_eq!(fresh.items.len(), 2);
    let done = app
        .accept_and_activate_learned(&fresh.batch_fp, &rest)
        .unwrap();
    assert_eq!(done.activated, 2);
    let again = app
        .accept_and_activate_learned(&batch.batch_fp, &[ana])
        .unwrap();
    assert_eq!((again.activated, again.already_active), (0, 1));
}

#[test]
fn two_rules_with_the_same_name_get_different_ids_inside_one_batch() {
    let (_dir, mut app) = learned();
    // Un nombre ya usado por otra regla: el lote no choca consigo mismo ni con la existente.
    app.create_rule_full(NewRule {
        id: Some("responder-a-ana"),
        name: None,
        action_kind: SEND_DRAFT,
        conditions: &json!({ "category": "documentacion", "sender": "otro@beta.test" }),
        allow_any_sender: false,
    })
    .unwrap();
    let batch = app.learned_suggestions_batch().unwrap();
    let outcome = app
        .accept_and_activate_learned(&batch.batch_fp, &all_ids(&batch))
        .unwrap();
    assert!(outcome.applied, "{outcome:?}");
    let mut ids: Vec<&String> = outcome.rules.iter().map(|r| &r.rule_id).collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), 3);
}

// ---- T33g: compuerta y rodaje ----

#[test]
fn the_gate_keeps_its_priority_over_the_rodaje_and_over_an_active_learned_rule() {
    let (dir, mut app) = learned();
    let rule = accept_ana(&mut app);
    let before = sent(&app);

    // Con el rodaje pendiente, un correo bloqueado va a revisión por su causa y no por «Rodaje».
    let blocked = |app: &mut App, base: usize| {
        let list = mail_and_process(
            app,
            base,
            "Hola",
            Some(&[("List-Unsubscribe", "<mailto:baja@x.test>")]),
        );
        let url = mail_and_process(app, base + 1, "CON-URL", Some(&[]));
        let none = mail_and_process(app, base + 2, "Pregunta NOREPLY", Some(&[]));
        // Señales sin comprobar: el mensaje no las lleva.
        new_mail(app, base + 3, "ana@alfa.test", "Hola", None);
        app.fetch_mail().unwrap();
        process_all(app);
        let unchecked = task_of(app, base + 3);
        [
            (list, "automatic_sender"),
            (url, "draft_flagged"),
            (none, "no_reply_needed"),
            (unchecked, "signals_not_checked"),
        ]
    };
    for (task, expected) in blocked(&mut app, 100) {
        assert_eq!(task.state, TaskState::AwaitingReview, "{expected}");
        assert_eq!(code(&task), expected);
    }
    assert_eq!(sent(&app), before);
    assert_eq!(rodaje_left(dir.path(), &rule), Some(3));

    // Con el rodaje terminado, la regla sigue sin enviar nada de eso.
    set_rodaje(dir.path(), &rule, 0);
    for (task, expected) in blocked(&mut app, 200) {
        assert_eq!(task.state, TaskState::AwaitingReview, "{expected}");
        assert_eq!(code(&task), expected);
    }
    assert_eq!(sent(&app), before);

    // El control: un correo limpio con el rodaje terminado sí sale por la regla.
    let clean = mail_and_process(&mut app, 300, "Necesito la factura.", Some(&[]));
    assert_eq!(clean.state, TaskState::Completed, "{clean:?}");
    assert_eq!(result_of(&clean)["authorization"]["type"], "rule");
    assert_eq!(sent(&app), before + 1);
}

#[test]
fn the_first_three_valid_sends_go_to_review_with_the_rodaje_reason_and_the_fourth_goes_alone() {
    let (dir, mut app) = learned();
    let rule = accept_ana(&mut app);
    let before = sent(&app);

    let mut tasks = Vec::new();
    for n in 11..=13 {
        let task = mail_and_process(&mut app, n, "Necesito la factura.", Some(&[]));
        assert_eq!(task.state, TaskState::AwaitingReview, "m{n}");
        assert_eq!(code(&task), "rodaje");
        let result = result_of(&task);
        assert_eq!(result["review_reason"], RODAJE_TEXT);
        assert_eq!(result["evaluation"]["facts"]["rule_id"], rule.as_str());
        // El motivo no lleva contenido del correo.
        assert!(!result["evaluation"].to_string().contains("Necesito"));
        tasks.push(task);
    }
    assert_eq!(sent(&app), before, "el rodaje no envía");

    // Aprobar sin editar baja el contador un envío cada vez.
    for (done, task) in tasks.iter().enumerate() {
        let version = result_of(task)["version"].as_i64().unwrap_or(1);
        let sent_task = app.approve(task.id, version).unwrap();
        assert_eq!(sent_task.state, TaskState::Completed);
        assert_eq!(
            rodaje_left(dir.path(), &rule),
            Some(2 - done as i64),
            "tras {} aprobaciones",
            done + 1
        );
    }
    assert_eq!(sent(&app), before + 3);

    // El 4.º se envía solo, por la regla.
    let fourth = mail_and_process(&mut app, 14, "Necesito la factura.", Some(&[]));
    assert_eq!(fourth.state, TaskState::Completed, "{fourth:?}");
    assert_eq!(result_of(&fourth)["authorization"]["type"], "rule");
    assert_eq!(sent(&app), before + 4);

    // El Historial deja constancia de que terminó el rodaje, sin contenido.
    let log = audit(dir.path());
    assert!(log
        .iter()
        .any(|(a, s, d)| a == "rule_rodaje" && s == "finished" && d.contains("faltan 0")));
    assert!(log.iter().all(|(_, _, d)| !d.contains("Necesito")));
}

#[test]
fn editing_or_rejecting_does_not_count_and_restarts_an_unfinished_rodaje() {
    let (dir, mut app) = learned();
    let rule = accept_ana(&mut app);
    let before = sent(&app);
    let review = |app: &mut App, n: usize| {
        let task = mail_and_process(app, n, "Necesito la factura.", Some(&[]));
        assert_eq!(code(&task), "rodaje", "m{n}");
        task
    };
    let version = |task: &Task| result_of(task)["version"].as_i64().unwrap_or(1);

    let first = review(&mut app, 21);
    let second = review(&mut app, 22);
    let third = review(&mut app, 23);
    app.approve(first.id, version(&first)).unwrap();
    assert_eq!(rodaje_left(dir.path(), &rule), Some(2));

    // Editar no baja el contador: reinicia el rodaje a medias, y aprobar lo editado no cuenta.
    let edited = app
        .edit_draft(second.id, "Texto corregido a mano.")
        .unwrap();
    assert_eq!(rodaje_left(dir.path(), &rule), Some(3));
    app.approve(edited.id, version(&edited)).unwrap();
    assert_eq!(
        rodaje_left(dir.path(), &rule),
        Some(3),
        "aprobar un envío editado no cuenta"
    );

    // Rechazar tampoco lo baja; con el rodaje a medias lo reinicia.
    let fourth = review(&mut app, 24);
    app.approve(third.id, version(&third)).unwrap();
    assert_eq!(rodaje_left(dir.path(), &rule), Some(2));
    app.reject(fourth.id).unwrap();
    assert_eq!(rodaje_left(dir.path(), &rule), Some(3));
    assert_eq!(sent(&app), before + 3, "solo salieron las aprobadas");

    // Con el rodaje terminado, editar un envío bloqueado por otra causa no lo reinicia.
    set_rodaje(dir.path(), &rule, 0);
    let flagged = mail_and_process(&mut app, 25, "CON-URL", Some(&[]));
    assert_eq!(code(&flagged), "draft_flagged");
    app.edit_draft(flagged.id, "Sin enlace.").unwrap();
    assert_eq!(rodaje_left(dir.path(), &rule), Some(0));
}

#[test]
fn an_approval_that_does_not_end_up_sent_does_not_count() {
    let (dir, mut app) = learned();
    let rule = accept_ana(&mut app);
    let task = mail_and_process(&mut app, 31, "Necesito la factura.", Some(&[]));
    assert_eq!(code(&task), "rodaje");
    // La señal de aprobación sigue pendiente: si la acción no queda confirmada se anula y no baja.
    let version = result_of(&task)["version"].as_i64().unwrap_or(1);
    sql(dir.path())
        .execute_batch(
            "CREATE TRIGGER sin_envio BEFORE INSERT ON actions
             BEGIN SELECT RAISE(ABORT, 'envío imposible'); END;",
        )
        .unwrap();
    let outcome = app.approve(task.id, version);
    sql(dir.path())
        .execute_batch("DROP TRIGGER sin_envio;")
        .unwrap();
    // El envío falla: la tarea no se completa, la señal queda anulada y el contador no baja.
    let done = outcome.expect("el fallo del envío deja la tarea fallida, no un error");
    assert_ne!(done.state, TaskState::Completed);
    let void: i64 = count(
        dir.path(),
        "SELECT COUNT(*) FROM rule_signals WHERE kind = 'approved' AND state = 'void'",
    );
    assert_eq!(void, 1, "la aprobación que no se envió quedó anulada");
    assert_eq!(rodaje_left(dir.path(), &rule), Some(3));
}

#[test]
fn rules_from_other_paths_have_no_rodaje() {
    let (dir, mut app) = learned();
    // Una regla creada a mano y una sugerencia aceptada con la tarjeta de siempre: ninguna lleva
    // rodaje, y la compuerta es lo único que las frena.
    app.create_rule_full(NewRule {
        id: Some("manual-ana"),
        name: None,
        action_kind: SEND_DRAFT,
        conditions: &json!({ "category": "solicitud_factura", "sender": "ana@alfa.test" }),
        allow_any_sender: false,
    })
    .unwrap();
    app.set_rule_active("manual-ana", true).unwrap();
    assert_eq!(rodaje_left(dir.path(), "manual-ana"), None);
    let before = sent(&app);
    let task = mail_and_process(&mut app, 41, "Necesito la factura.", Some(&[]));
    assert_eq!(task.state, TaskState::Completed, "{task:?}");
    assert_eq!(result_of(&task)["authorization"]["type"], "rule");
    assert_eq!(sent(&app), before + 1);

    let batch = app.learned_suggestions_batch().unwrap();
    let marta = id_of(&batch, "sender", "marta@gmail.com");
    let shown = batch
        .items
        .iter()
        .find(|i| i.id == marta)
        .unwrap()
        .shown
        .clone();
    let view = app.accept_suggestion(&marta, &shown, None).unwrap();
    app.set_rule_active(&view.rule.id, true).unwrap();
    assert_eq!(rodaje_left(dir.path(), &view.rule.id), None);
    new_mail(
        &app,
        42,
        "marta@gmail.com",
        "Necesito la factura.",
        Some(&[]),
    );
    app.fetch_mail().unwrap();
    process_all(&mut app);
    let task = task_of(&app, 42);
    assert_eq!(task.state, TaskState::Completed, "{task:?}");
    assert_eq!(code(&task), "automated");
}

// ---- Desactivar las aprendidas ----

#[test]
fn deactivating_the_learned_rules_leaves_only_those_inactive_and_is_idempotent() {
    let (dir, mut app) = learned();
    app.create_rule_full(NewRule {
        id: Some("manual-beta"),
        name: None,
        action_kind: SEND_DRAFT,
        conditions: &json!({ "category": "documentacion", "sender": "otro@beta.test" }),
        allow_any_sender: false,
    })
    .unwrap();
    app.set_rule_active("manual-beta", true).unwrap();
    let batch = app.learned_suggestions_batch().unwrap();
    app.accept_and_activate_learned(&batch.batch_fp, &all_ids(&batch))
        .unwrap();
    assert_eq!(app.rules().unwrap().iter().filter(|r| r.active).count(), 4);

    let outcome = app.deactivate_learned_rules().unwrap();
    assert_eq!((outcome.deactivated.len(), outcome.learned_total), (3, 3));
    let active: Vec<String> = app
        .rules()
        .unwrap()
        .into_iter()
        .filter(|r| r.active)
        .map(|r| r.id)
        .collect();
    assert_eq!(
        active,
        vec!["manual-beta".to_owned()],
        "la manual sigue activa"
    );

    let log = audit(dir.path());
    assert_eq!(
        log.iter()
            .filter(|(a, s, _)| a == "rule_learned_batch" && s == "deactivated")
            .count(),
        3
    );
    assert_eq!(
        log.iter()
            .filter(|(a, s, _)| a == "rule_learned_batch" && s == "deactivated_all")
            .count(),
        1
    );

    // Repetir no escribe nada.
    let rules = app.rules().unwrap();
    let again = app.deactivate_learned_rules().unwrap();
    assert!(again.deactivated.is_empty());
    assert_eq!(app.rules().unwrap(), rules);
    assert_eq!(audit(dir.path()), log);

    // Una regla aprendida desactivada no vuelve sola con repetir la aceptación.
    let repeated = app.accept_and_activate_learned(&batch.batch_fp, &all_ids(&batch));
    assert!(repeated.is_err(), "no se reactiva sin una tanda nueva");
    assert_eq!(app.rules().unwrap().iter().filter(|r| r.active).count(), 1);
}

#[test]
fn an_unreadable_rodaje_counter_fails_closed() {
    let (dir, mut app) = learned();
    let rule = accept_ana(&mut app);
    sql(dir.path())
        .execute(
            "UPDATE rule_meta SET origin_json = json_set(origin_json, '$.rodaje.left', 'tres') WHERE rule_id = ?1",
            [&rule],
        )
        .unwrap();
    let before = sent(&app);
    let task = mail_and_process(&mut app, 51, "Necesito la factura.", Some(&[]));
    assert_eq!(task.state, TaskState::AwaitingReview, "{task:?}");
    assert_eq!(code(&task), "rodaje");
    assert_eq!(sent(&app), before);
}
