//! E5 (D171, D172): lo que deja el aprendizaje con un proveedor espía y datos sintéticos. La
//! evidencia `learned` (filtros y reanudación), el umbral `learned_reply`, la propuesta de perfil
//! (antes y después, bloqueos, consentimiento del perfil, descartar) y `reply_style` con su línea de
//! confianza en la redacción, sin que ningún texto aprendido entre en el prompt.

use std::path::Path;
use std::sync::{Arc, Mutex};

use nestadmin_ai::{
    AiError, GenerateRequest, GenerateResult, Provider, ProviderKind, Task as AiTask, Usage,
};
use nestadmin_app::initial_read::ReadExpectation;
use nestadmin_app::{App, AppPaths, EMAIL};
use nestadmin_core::BusinessProfile;
use nestadmin_extract::{ExtractError, Extractor, Ocr};
use nestadmin_simulators::{Message, SentMessage};
use serde_json::{json, Value};

const ACCOUNT: &str = "simulador";
const REPLY_SENTINEL: &str = "RESPUESTA-SENTINELA-456";
const HABIT_CONFIRM: &str = "Respuesta habitual del negocio en esta categoría: confirmar.";
const HABIT_PREFIX: &str = "Respuesta habitual del negocio";

// ---- Proveedor espía ----

/// Aprende con marcas en el recibido: `CATOTROS`, `CATDOC`, `NOREPLY`, `KINDQUOTE`, `KINDINFORM`.
/// Registra todas las peticiones.
struct Spy {
    requests: Mutex<Vec<GenerateRequest>>,
}

impl Spy {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            requests: Mutex::new(Vec::new()),
        })
    }
    fn of(&self, task: AiTask) -> Vec<GenerateRequest> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.task == task)
            .cloned()
            .collect()
    }
    /// Peticiones de ese tipo sobre el correo de asunto «Consulta <n>» (el simulador trae además
    /// sus correos de ejemplo, que también se procesan).
    fn about(&self, task: AiTask, n: usize) -> Vec<GenerateRequest> {
        let subject = format!(
            "Asunto: Consulta {n}
"
        );
        self.of(task)
            .into_iter()
            .filter(|r| r.content.contains(&subject))
            .collect()
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
                        } else if block.contains("CATDOC") {
                            "documentacion"
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
            AiTask::Classification if request.content.contains("CATDOC") => {
                json!({ "category": "documentacion", "needs_reply": true })
            }
            AiTask::Classification => json!({
                "category": "solicitud_factura",
                "needs_reply": !request.content.contains("NOREPLY"),
            }),
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
    .with_checked_signals(&[])
}

/// Diez correos de personas, todos con su respuesta propia salvo que se diga otra cosa:
/// 1-3 Ana (alfa.test), 4-5 Luis (alfa.test), 6-8 Marta (gmail.com, dominio público), 9 Pedro
/// (alfa.test, no pide respuesta) y 10 Eva (alfa.test, categoría `otros`).
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

fn mailbox(dir: &Path) -> Vec<Message> {
    let senders = senders();
    let mut messages = Vec::new();
    for (index, from) in senders.iter().enumerate() {
        let i = index + 1;
        let marks = match i {
            6 => " KINDQUOTE",
            9 => " NOREPLY",
            10 => " CATOTROS KINDINFORM",
            _ => "",
        };
        messages.push(message(i, from, &format!("Pregunta {i}.{marks}")));
    }
    let state = json!({
        "version": 1,
        "state": { "messages": messages, "drafts": [], "sent": [], "files": [] },
    });
    std::fs::write(dir.join("simulador-correo.json"), state.to_string()).unwrap();
    messages
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
fn learned() -> (tempfile::TempDir, App, Arc<Spy>) {
    let dir = tempfile::tempdir().unwrap();
    mailbox(dir.path());
    let spy = Spy::new();
    let mut app = open(dir.path(), spy.clone());
    add_replies(&app);
    finish_first_read(&mut app);
    app.run_learning_ticks().unwrap();
    assert_eq!(app.learning_status(ACCOUNT).unwrap().state, "done");
    (dir, app, spy)
}

fn learned_rows(dir: &Path) -> Vec<(String, Option<i64>, String, String, i64)> {
    let conn = sql(dir);
    let mut stmt = conn
        .prepare(
            "SELECT subject_key, task_id, state, edit_state, content_version
             FROM rule_signals WHERE kind = 'learned' ORDER BY id",
        )
        .unwrap();
    let rows = stmt
        .query_map([], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })
        .unwrap();
    rows.map(Result::unwrap).collect()
}

fn all_signals(dir: &Path) -> Vec<(String, String)> {
    let conn = sql(dir);
    let mut stmt = conn
        .prepare("SELECT kind, subject_key FROM rule_signals ORDER BY id")
        .unwrap();
    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    rows.map(Result::unwrap).collect()
}

fn reply_style(dir: &Path) -> Vec<(String, String, i64)> {
    let conn = sql(dir);
    let mut stmt = conn
        .prepare("SELECT category, reply_kind, n FROM reply_style ORDER BY category, reply_kind")
        .unwrap();
    let rows = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap();
    rows.map(Result::unwrap).collect()
}

/// Sustituye el estilo del resultado guardado (como si el aprendizaje lo hubiera confirmado).
fn set_style(dir: &Path, style: Value) {
    let conn = sql(dir);
    let text: String = conn
        .query_row(
            "SELECT value FROM rule_settings WHERE key LIKE 'first_learning_result:%'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let mut value: Value = serde_json::from_str(&text).unwrap();
    value["style"] = style;
    conn.execute(
        "UPDATE rule_settings SET value = ?1 WHERE key LIKE 'first_learning_result:%'",
        [value.to_string()],
    )
    .unwrap();
}

// ---- D172: evidencia `learned` ----

#[test]
fn learned_signals_are_inserted_with_the_filters_and_resuming_never_duplicates_them() {
    let (dir, app, spy) = learned();
    let rows = learned_rows(dir.path());
    let keys: Vec<&str> = rows.iter().map(|r| r.0.as_str()).collect();
    // Solo los pares que pedían respuesta y no son `otros`: 8. Pedro (sin respuesta necesaria) y
    // Eva (`otros`) no dejan señal; Marta (dominio público) sí, para la regla de dirección exacta.
    assert_eq!(rows.len(), 8, "{keys:?}");
    assert_eq!(
        keys.iter().filter(|k| k.ends_with("ana@alfa.test")).count(),
        3
    );
    assert_eq!(
        keys.iter()
            .filter(|k| k.ends_with("luis@alfa.test"))
            .count(),
        2
    );
    assert_eq!(
        keys.iter()
            .filter(|k| k.ends_with("marta@gmail.com"))
            .count(),
        3
    );
    assert!(keys.iter().all(|k| k.starts_with("solicitud_factura|")));
    assert!(!keys
        .iter()
        .any(|k| k.contains("pedro") || k.contains("eva")));
    // Sin tarea, válidas, versión 0 y el estado de edición honesto: no hubo borrador.
    for (_, task, state, edit, version) in &rows {
        assert_eq!(
            (task.as_ref(), state.as_str(), edit.as_str(), *version),
            (None, "valid", "unknown", 0)
        );
    }
    // No se creó ninguna tarea para ellas.
    assert!(app.tasks().unwrap().iter().all(|t| t.kind != EMAIL));
    let count: i64 = sql(dir.path())
        .query_row("SELECT count(*) FROM rule_signals", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        count, 9,
        "las 8 aprendidas y la factura de ejemplo vista en la primera lectura"
    );
    assert_eq!(
        all_signals(dir.path())
            .iter()
            .filter(|(kind, _)| kind == "learned")
            .count(),
        8
    );

    // Reanudar (reabrir y volver a recorrer los pasos) no añade filas ni cuentas.
    let style_before = reply_style(dir.path());
    drop(app);
    let mut app = open(dir.path(), spy.clone());
    assert_eq!(app.run_learning_ticks().unwrap(), 0);
    assert_eq!(learned_rows(dir.path()).len(), 8);
    assert_eq!(reply_style(dir.path()), style_before);
    // `suggest` no cuenta lo aprendido como decisiones de la persona.
    let s = app.rule_suggestions().unwrap();
    assert_eq!((s.learned, s.observed, s.decisions), (8, 1, 0));
}

#[test]
fn reply_style_counts_only_enums_of_the_pairs_that_need_a_reply() {
    let (dir, _app, _spy) = learned();
    // 6 confirmar + 1 presupuesto en `solicitud_factura`, y el `otros` de Eva (informar); Pedro,
    // que no pedía respuesta, no cuenta.
    assert_eq!(
        reply_style(dir.path()),
        vec![
            ("otros".to_owned(), "inform".to_owned(), 1),
            ("solicitud_factura".to_owned(), "confirm".to_owned(), 7),
            ("solicitud_factura".to_owned(), "quote".to_owned(), 1),
        ]
    );
}

#[test]
fn the_learned_candidates_carry_their_origin_and_counts_and_accepting_records_it() {
    let (dir, mut app, _spy) = learned();
    let s = app.rule_suggestions().unwrap();
    assert_eq!(s.thresholds.learned_reply, 3);
    let find = |key: &str, value: &str| {
        s.candidates
            .iter()
            .find(|c| c.conditions[key] == value)
            .cloned()
    };
    let ana = find("sender", "ana@alfa.test").expect("Ana: 3 pares");
    assert_eq!(ana.evidence.origin, "learned");
    assert_eq!(
        (
            ana.evidence.count,
            ana.evidence.learned,
            ana.evidence.approved
        ),
        (3, 3, 0)
    );
    assert!(
        find("sender", "marta@gmail.com").is_some(),
        "dirección exacta, dominio público"
    );
    assert!(
        find("sender", "luis@alfa.test").is_none(),
        "Luis: solo 2 pares"
    );
    let domain = find("sender_domain", "alfa.test").expect("5 pares de 2 direcciones");
    assert_eq!((domain.evidence.count, domain.evidence.addresses), (5, 2));
    assert!(
        find("sender_domain", "gmail.com").is_none(),
        "dominio público"
    );
    // Nunca «cualquier remitente» ni `otros`.
    assert!(s.candidates.iter().all(|c| {
        c.conditions.get("sender").is_some() || c.conditions.get("sender_domain").is_some()
    }));
    assert!(s
        .candidates
        .iter()
        .all(|c| c.conditions["category"] != "otros"));

    // Aceptar crea la regla inactiva y su origen dice que se aprendió en la primera carga.
    let view = app
        .accept_suggestion(&ana.fingerprint, &ana.shown, None)
        .unwrap();
    assert!(!view.rule.active);
    let origin: String = sql(dir.path())
        .query_row(
            "SELECT origin_json FROM rule_meta WHERE rule_id = ?1",
            [&view.rule.id],
            |r| r.get(0),
        )
        .unwrap();
    let origin: Value = serde_json::from_str(&origin).unwrap();
    assert_eq!(origin["kind"], "suggestion");
    assert_eq!(origin["evidence_origin"], "learned");
    assert_eq!(
        (origin["learned"].clone(), origin["approved"].clone()),
        (json!(3), json!(0))
    );
    assert_eq!(origin["task_ids"], json!([]));
}

#[test]
fn a_later_rejection_or_edit_of_the_same_sender_breaks_the_learned_series() {
    let (dir, app, _spy) = learned();
    assert!(app
        .rule_suggestions()
        .unwrap()
        .candidates
        .iter()
        .any(|c| c.conditions["sender"] == "ana@alfa.test"));
    // Un rechazo de la persona (tarea ficticia solo en la tabla de señales; sin FK en la conexión
    // de la prueba).
    let conn = sql(dir.path());
    conn.execute_batch("PRAGMA foreign_keys = OFF;").unwrap();
    conn.execute(
            "INSERT INTO rule_signals (kind, action_kind, subject_key, edit_state, task_id, content_version, at, state)
             VALUES ('rejected', 'send_draft', 'solicitud_factura|ana@alfa.test', 'unedited', 990, 1, 1, 'valid')",
            [],
        )
        .unwrap();
    let s = app.rule_suggestions().unwrap();
    assert!(
        !s.candidates
            .iter()
            .any(|c| c.conditions["sender"] == "ana@alfa.test"),
        "el rechazo rompe la serie de Ana"
    );
    assert!(
        !s.candidates
            .iter()
            .any(|c| c.conditions["sender_domain"] == "alfa.test"),
        "y la del dominio"
    );
    assert!(s
        .candidates
        .iter()
        .any(|c| c.conditions["sender"] == "marta@gmail.com"));
}

#[test]
fn the_learned_threshold_is_validated_audited_and_has_a_floor_of_two() {
    let (_dir, mut app, _spy) = learned();
    assert_eq!(app.suggestion_thresholds().unwrap().learned_reply, 3);
    assert_eq!(
        app.suggestion_thresholds().unwrap().default_learned_reply,
        3
    );
    for bad in [-1, 0, 1, 51, 1000] {
        assert!(
            app.set_suggestion_threshold("learned_reply", bad).is_err(),
            "{bad}"
        );
    }
    assert_eq!(app.suggestion_thresholds().unwrap().learned_reply, 3);
    let entries = |app: &App| {
        app.audit_log()
            .unwrap()
            .into_iter()
            .filter(|e| e.action == "rule_setting")
            .count()
    };
    let before = entries(&app);
    let t = app.set_suggestion_threshold("learned_reply", 2).unwrap();
    assert_eq!((t.learned_reply, t.reply, t.invoice), (2, 5, 3));
    assert_eq!(entries(&app), before + 1);
    // Con 2, Luis (2 pares) ya llega; el umbral de aprobaciones no cambió.
    let s = app.rule_suggestions().unwrap();
    assert!(s
        .candidates
        .iter()
        .any(|c| c.conditions["sender"] == "luis@alfa.test"));
    // Repetir el mismo valor no ensucia el Historial.
    app.set_suggestion_threshold("learned_reply", 2).unwrap();
    assert_eq!(entries(&app), before + 1);
}

// ---- D171: propuesta de perfil ----

fn existing_profile() -> BusinessProfile {
    BusinessProfile {
        display_name: "Taller Ficticio SL".into(),
        hours: "Lunes a viernes de 9:00 a 14:00".into(),
        tone: "Cercano".into(),
        signature: "Taller Ficticio SL".into(),
        policies: String::new(),
    }
}

#[test]
fn the_profile_proposal_shows_before_and_after_never_touches_the_signature_and_accepting_saves_it()
{
    let (dir, mut app, _spy) = learned();
    set_style(
        dir.path(),
        json!({ "tone": "formal", "length": "breve", "greeting": "Buenos días,", "closing": "Un saludo cordial" }),
    );
    app.save_business_profile(existing_profile()).unwrap();
    let proposal = app.learned_profile_proposal(ACCOUNT).unwrap().unwrap();
    assert_eq!(proposal.before, existing_profile());
    assert_eq!(
        proposal.after.tone,
        "formal, breve. Saludo habitual: «Buenos días,». Despedida habitual: «Un saludo cordial»."
    );
    assert_eq!(proposal.changed, vec!["tone"]);
    // Lo demás, incluida la firma, queda como estaba.
    assert_eq!(
        BusinessProfile {
            tone: String::new(),
            ..proposal.after.clone()
        },
        BusinessProfile {
            tone: String::new(),
            ..existing_profile()
        }
    );
    assert!(proposal.blocking.is_empty());
    // Proponer no guarda nada.
    assert_eq!(app.business_profile().unwrap().profile, existing_profile());

    assert!(app.accept_learned_profile(ACCOUNT, "otra-huella").is_err());
    assert_eq!(app.business_profile().unwrap().profile, existing_profile());
    let saved = app.accept_learned_profile(ACCOUNT, &proposal.fp).unwrap();
    assert_eq!(saved.view.profile, proposal.after);
    // Ya no se ofrece y repetir no cambia nada.
    assert!(app.learned_profile_proposal(ACCOUNT).unwrap().is_none());
    let again = app.accept_learned_profile(ACCOUNT, &proposal.fp).unwrap();
    assert_eq!(again.view.profile, proposal.after);
    let entries: Vec<String> = app
        .audit_log()
        .unwrap()
        .into_iter()
        .filter(|e| e.action == "rule_setting")
        .filter_map(|e| e.detail)
        .filter(|d| d.contains("propuesta de perfil"))
        .collect();
    assert_eq!(entries.len(), 1, "{entries:?}");
    assert!(
        !entries[0].contains("Buenos"),
        "el Historial no lleva el texto"
    );
}

#[test]
fn a_style_with_only_enums_gets_a_fixed_text_and_without_a_style_there_is_no_proposal() {
    let (dir, app, _spy) = learned();
    set_style(
        dir.path(),
        json!({ "tone": "neutro", "length": "detallado" }),
    );
    let proposal = app.learned_profile_proposal(ACCOUNT).unwrap().unwrap();
    assert_eq!(proposal.after.tone, "neutro, detallado.");
    assert!(proposal.after.signature.is_empty());
    set_style(dir.path(), json!({}));
    assert!(app.learned_profile_proposal(ACCOUNT).unwrap().is_none());
}

#[test]
fn accepting_goes_through_the_normal_save_so_blocking_findings_and_the_cap_apply() {
    let (dir, mut app, _spy) = learned();
    // Un saludo con un IBAN no se guarda aunque venga de lo aprendido.
    set_style(
        dir.path(),
        json!({ "tone": "formal", "greeting": "ES91 2100 0418 4502 0005 1332" }),
    );
    let proposal = app.learned_profile_proposal(ACCOUNT).unwrap().unwrap();
    assert!(proposal.blocking.iter().any(|f| f.code == "iban"));
    assert!(app.accept_learned_profile(ACCOUNT, &proposal.fp).is_err());
    assert!(!app.business_profile().unwrap().saved);
    // Sigue pendiente (no quedó registrada como aceptada).
    assert!(app.learned_profile_proposal(ACCOUNT).unwrap().is_some());

    // El tope de 2.000 caracteres vale también para la propuesta.
    app.save_business_profile(BusinessProfile {
        policies: "x".repeat(1990),
        ..BusinessProfile::default()
    })
    .unwrap();
    set_style(dir.path(), json!({ "tone": "formal", "length": "breve" }));
    let proposal = app.learned_profile_proposal(ACCOUNT).unwrap().unwrap();
    assert!(proposal.blocking.iter().any(|f| f.code == "too_long"));
    assert!(app.accept_learned_profile(ACCOUNT, &proposal.fp).is_err());
    assert_eq!(app.business_profile().unwrap().profile.tone, "");
}

#[test]
fn dismissing_is_recorded_without_content_and_the_proposal_does_not_come_back() {
    let (dir, mut app, _spy) = learned();
    set_style(
        dir.path(),
        json!({ "tone": "cercano", "greeting": "Hola Marta,", "closing": null }),
    );
    assert!(app.learned_profile_proposal(ACCOUNT).unwrap().is_some());
    app.dismiss_learned_profile(ACCOUNT).unwrap();
    assert!(app.learned_profile_proposal(ACCOUNT).unwrap().is_none());
    assert!(!app.business_profile().unwrap().saved);
    let before = app.audit_log().unwrap().len();
    app.dismiss_learned_profile(ACCOUNT).unwrap();
    assert_eq!(
        app.audit_log().unwrap().len(),
        before,
        "descartar dos veces no repite"
    );
    let entries: Vec<String> = app
        .audit_log()
        .unwrap()
        .into_iter()
        .filter_map(|e| e.detail)
        .filter(|d| d.contains("propuesta de perfil"))
        .collect();
    assert_eq!(
        entries,
        vec!["propuesta de perfil aprendida descartada".to_owned()]
    );
    // Aceptar algo descartado no existe.
    assert!(app.accept_learned_profile(ACCOUNT, "x").is_err());
}

#[test]
fn accepting_changes_the_profile_fingerprint_so_the_profile_consent_lapses() {
    // Un proveedor externo con consentimiento del perfil: aceptar cambia lo que saldría y lo retira.
    let dir = tempfile::tempdir().unwrap();
    mailbox(dir.path());
    let local = Spy::new();
    let mut app = open(dir.path(), local.clone());
    add_replies(&app);
    finish_first_read(&mut app);
    app.run_learning_ticks().unwrap();
    drop(app);

    struct External(Arc<Spy>);
    impl Provider for External {
        fn kind(&self) -> ProviderKind {
            ProviderKind::OllamaCloud
        }
        fn model(&self) -> &str {
            "modelo-a"
        }
        fn destination(&self) -> Option<String> {
            Some("ollama.test".into())
        }
        fn generate(&self, r: &GenerateRequest) -> Result<GenerateResult, AiError> {
            self.0.generate(r)
        }
    }
    let mut app = open(dir.path(), Arc::new(External(local)));
    set_style(dir.path(), json!({ "tone": "formal", "length": "breve" }));
    app.save_business_profile(existing_profile()).unwrap();
    let preview = app.profile_preview().unwrap();
    let consent = preview.consent.clone();
    app.grant_profile_consent(
        consent.provider.as_deref().unwrap(),
        consent.model.as_deref().unwrap(),
        consent.host.as_deref().unwrap_or(""),
        preview.fp.as_deref().unwrap(),
    )
    .unwrap();
    assert_eq!(app.profile_consent().unwrap().state, "granted");

    let proposal = app.learned_profile_proposal(ACCOUNT).unwrap().unwrap();
    let saved = app.accept_learned_profile(ACCOUNT, &proposal.fp).unwrap();
    assert!(saved.consent_withdrawn);
    assert_eq!(app.profile_consent().unwrap().state, "missing");
}

// ---- D171: línea de confianza en la redacción ----

fn new_mail(app: &mut App, n: usize, body: &str) {
    app.simulators()
        .0
        .add_message(message(n, "nuevo@cliente.test", body))
        .unwrap();
    app.fetch_mail().unwrap();
}

#[test]
fn the_habit_line_appears_with_a_dominant_kind_outside_the_mail_and_no_learned_text_enters() {
    let (_dir, mut app, spy) = learned();
    new_mail(&mut app, 100, "Necesito la factura.");
    process_all(&mut app);
    let drafts = spy.about(AiTask::Draft, 100);
    assert_eq!(drafts.len(), 1);
    let draft = &drafts[0];
    assert!(
        draft.instructions.contains(HABIT_CONFIRM),
        "{}",
        draft.instructions
    );
    assert!(
        !draft.content.contains(HABIT_PREFIX),
        "la línea va fuera del correo"
    );
    // Ninguna petición de redacción ni de clasificación lleva texto aprendido: ni respuestas
    // propias, ni el estilo, ni direcciones del aprendizaje.
    for request in spy.requests.lock().unwrap().iter() {
        if request.task == AiTask::LearnReplies {
            continue;
        }
        let all = format!("{} {}", request.instructions, request.content);
        for text in [
            REPLY_SENTINEL,
            "Respuesta 1.",
            "ana@alfa.test",
            "marta@gmail.com",
        ] {
            assert!(!all.contains(text), "se coló {text}");
        }
    }
    // Una vez enviada la redacción, el resultado no guarda la línea.
    let task = app
        .tasks()
        .unwrap()
        .into_iter()
        .find(|t| t.kind == EMAIL && t.input_ref.contains("m100"))
        .unwrap();
    assert!(!task.result.unwrap().to_string().contains(HABIT_PREFIX));
}

#[test]
fn without_a_dominant_kind_there_is_no_habit_line() {
    let (dir, mut app, spy) = learned();
    // Categoría sin recuentos (`documentacion`): sin línea.
    new_mail(&mut app, 100, "Documentos CATDOC.");
    process_all(&mut app);
    let drafts = spy.about(AiTask::Draft, 100);
    assert_eq!(drafts.len(), 1);
    assert!(!drafts[0].instructions.contains(HABIT_PREFIX));

    // Un empate en la cabeza, menos de dos o solo `other` tampoco dan línea.
    for (n, rows) in [
        "DELETE FROM reply_style; INSERT INTO reply_style VALUES ('solicitud_factura','confirm',2),('solicitud_factura','quote',2);",
        "DELETE FROM reply_style; INSERT INTO reply_style VALUES ('solicitud_factura','confirm',1);",
        "DELETE FROM reply_style; INSERT INTO reply_style VALUES ('solicitud_factura','other',9);",
        "DELETE FROM reply_style;",
    ]
    .into_iter()
    .enumerate()
    {
        sql(dir.path()).execute_batch(rows).unwrap();
        let id = 200 + n;
        new_mail(&mut app, id, "Necesito la factura.");
        process_all(&mut app);
        let drafts = spy.about(AiTask::Draft, id);
        assert_eq!(drafts.len(), 1, "{rows}");
        assert!(!drafts[0].instructions.contains(HABIT_PREFIX), "{rows}");
    }
    // Con otro dominante, el texto es el fijo de ese tipo.
    sql(dir.path())
        .execute_batch("DELETE FROM reply_style; INSERT INTO reply_style VALUES ('solicitud_factura','quote',4),('solicitud_factura','confirm',1);")
        .unwrap();
    new_mail(&mut app, 300, "Necesito la factura.");
    process_all(&mut app);
    let drafts = spy.about(AiTask::Draft, 300);
    assert_eq!(drafts.len(), 1);
    assert!(drafts[0]
        .instructions
        .contains("Respuesta habitual del negocio en esta categoría: enviar un presupuesto."));
    assert!(!drafts[0].instructions.contains(HABIT_CONFIRM));
}

#[test]
fn redrafting_also_carries_the_habit_line_of_the_source_category() {
    let (_dir, mut app, spy) = learned();
    // Sin respuesta necesaria no se redacta; «Redactar respuesta» lo pide la persona.
    new_mail(&mut app, 100, "Gracias, todo recibido. NOREPLY");
    process_all(&mut app);
    assert!(spy.about(AiTask::Draft, 100).is_empty());
    let source = app
        .tasks()
        .unwrap()
        .into_iter()
        .find(|t| t.kind == EMAIL && t.input_ref.contains("m100"))
        .unwrap();
    app.redraft(source.id).unwrap();
    process_all(&mut app);
    let drafts = spy.about(AiTask::Draft, 100);
    assert_eq!(drafts.len(), 1);
    assert!(drafts[0].instructions.contains(HABIT_CONFIRM));
}
