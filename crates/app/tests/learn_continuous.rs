//! Aprendizaje continuo de las respuestas hechas fuera (D178, R31, T34a-d): con el simulador de
//! correo (que admite carpeta Enviados, un espía de consultas y fallos forzados) y un proveedor
//! de IA espía, solo con datos sintéticos (`*.test`). La IA no se llama nunca en una pasada y el
//! cuerpo de la respuesta no queda en ninguna parte.

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use nestadmin_ai::tasks::document_text;
use nestadmin_ai::{
    AiError, GenerateRequest, GenerateResult, Provider, ProviderKind, Task as AiTask, Usage,
};
use nestadmin_app::learn_continuous::{
    ContinuousStep, CONTINUOUS_MAX_PER_PASS, CONTINUOUS_WINDOW_DAYS,
};
use nestadmin_app::{App, AppPaths, EMAIL};
use nestadmin_core::{Task, TaskState};
use nestadmin_extract::{ExtractError, Extractor, Ocr};
use nestadmin_simulators::{FailureMode, MailSimulator, Message, SentMessage};
use serde_json::{json, Value};

const ACCOUNT: &str = "simulador";
const REPLY_SENTINEL: &str = "RESPUESTA-SENTINELA-789 IBAN ES91 2100 0418 4502 0005 1332";

// ---- Proveedor espía ----

/// Clasifica según marcas del correo: `[[SIN-RESPUESTA]]` (`needs_reply = false`), `[[SIN-CLAVE]]`
/// (sin `needs_reply`), `[[CAT-OTROS]]` y `[[CAT-RECLAMACION]]` (categorías de solo revisión).
struct Spy {
    kind: ProviderKind,
    calls: AtomicUsize,
}

impl Provider for Spy {
    fn kind(&self) -> ProviderKind {
        self.kind
    }
    fn model(&self) -> &str {
        "simulado"
    }
    fn destination(&self) -> Option<String> {
        (self.kind != ProviderKind::Ollama).then(|| "ollama.test".to_owned())
    }
    fn generate(&self, request: &GenerateRequest) -> Result<GenerateResult, AiError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let c = &request.content;
        let output = match request.task {
            AiTask::Extraction => serde_json::from_str(document_text(c)).unwrap_or_else(|_| {
                json!({
                    "supplier": "Proveedor Alfa Suministros SL", "invoice_number": "FS-2026-014",
                    "date": "2026-10-01", "subtotal": "120.00", "tax": "25.20", "total": "145.20",
                })
            }),
            AiTask::Classification if c.contains("[[SIN-RESPUESTA]]") => {
                json!({ "category": "solicitud_factura", "needs_reply": false })
            }
            AiTask::Classification if c.contains("[[CAT-OTROS]]") => {
                json!({ "category": "otros", "needs_reply": true })
            }
            AiTask::Classification if c.contains("[[CAT-RECLAMACION]]") => {
                json!({ "category": "reclamacion", "needs_reply": true })
            }
            AiTask::Classification if c.contains("[[SIN-CLAVE]]") => {
                json!({ "category": "solicitud_factura" })
            }
            AiTask::Classification => {
                json!({ "category": "solicitud_factura", "needs_reply": true })
            }
            AiTask::Draft => json!({ "draft": "Adjuntamos la factura solicitada." }),
            AiTask::LearnReplies => {
                let pairs: Vec<Value> = c
                    .lines()
                    .filter_map(|l| l.strip_prefix("id: "))
                    .map(|id| {
                        json!({ "pair_id": id, "category": "solicitud_factura", "needs_reply": true,
                                "reply_kind": "confirm" })
                    })
                    .collect();
                json!({ "style": { "tone": "cercano", "length": "breve", "greeting": "Hola,", "closing": "Un saludo" },
                        "pairs": pairs })
            }
            _ => json!({}),
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

// ---- Fechas (relativas a hoy: la ventana es de 14 días) ----

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(month <= 2), month, day)
}

/// `AAAA-MM-DDThh:00:00Z` de hace `days` días.
fn at(days: i64, hour: u32) -> String {
    let today = i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            / 86_400,
    )
    .unwrap();
    let (y, m, d) = civil_from_days(today - days);
    format!("{y:04}-{m:02}-{d:02}T{hour:02}:00:00Z")
}

// ---- Entorno ----

struct Env {
    dir: tempfile::TempDir,
    app: App,
    spy: Arc<Spy>,
}

fn sender_of(i: usize) -> String {
    format!("cliente{i}@empresa{i}.test")
}

/// Correo de una persona: hilo propio `t<i>`, recibido hace 2 días a las 10:00, señales
/// comprobadas.
fn incoming(i: usize, from: &str, body: &str) -> Message {
    Message {
        id: format!("m{i}"),
        subject: Some(format!("Consulta {i}")),
        from: Some(from.into()),
        to: Some(vec!["administracion@nestadmin.test".into()]),
        body: Some(body.into()),
        date: Some(at(2, 10)),
        attachments: Vec::new(),
        signals: None,
        thread_key: Some(format!("t{i}")),
    }
    .with_checked_signals(&[])
}

fn seed_mail(dir: &Path, messages: Vec<Message>) {
    let state = json!({
        "version": 1,
        "state": { "messages": messages, "drafts": [], "sent": [], "files": [] },
    });
    std::fs::write(dir.join("simulador-correo.json"), state.to_string()).unwrap();
}

fn process_all(app: &mut App) {
    while app.process_next().unwrap().is_some() {}
}

/// Abre la aplicación con esos mensajes, sin consultarlos todavía.
fn open_env(kind: ProviderKind, messages: Vec<Message>) -> Env {
    let dir = tempfile::tempdir().unwrap();
    seed_mail(dir.path(), messages);
    let spy = Arc::new(Spy {
        kind,
        calls: AtomicUsize::new(0),
    });
    let extractor = Extractor::new(dir.path().join("sin-pdfium"), Box::new(NoOcr));
    let mut app = App::open(&AppPaths::new(dir.path()), spy.clone(), Arc::new(extractor)).unwrap();
    app.set_mail_limit(500).unwrap();
    Env { dir, app, spy }
}

/// Abre la aplicación con esos mensajes, los consulta y los procesa hasta dejarlos en revisión.
fn env(messages: Vec<Message>) -> Env {
    let mut e = open_env(ProviderKind::OllamaCloud, messages);
    e.app.fetch_mail().unwrap();
    process_all(&mut e.app);
    // El simulador trae además su correo de muestra sin fecha ni hilo (M-0003): con la fecha de
    // hoy y sin hilo sería candidato de la recuperación (D178) y ensuciaría los recuentos. Se
    // lo fecha fuera de la ventana, como los demás de muestra.
    e.sql()
        .execute(
            "UPDATE tasks SET result = json_set(result, '$.message.date', '2026-10-03T09:00:00Z')
             WHERE json_extract(result, '$.message.id') = 'simulador:M-0003'",
            [],
        )
        .unwrap();
    e
}

impl Env {
    fn sim(&self) -> &MailSimulator {
        self.app.simulators().0
    }

    /// Una respuesta propia en Enviados al hilo `t<i>`, dos horas después del recibido, con
    /// destinatarios `to`.
    fn reply_to(&self, i: usize, to: &[&str], body: &str) {
        self.sim()
            .add_sent_message(SentMessage {
                id: format!("s{i}"),
                thread_key: format!("t{i}"),
                date: Some(at(2, 12)),
                body: Some(body.into()),
                to: to.iter().map(|a| (*a).to_owned()).collect(),
            })
            .unwrap();
    }

    /// La respuesta habitual al remitente del correo `i`.
    fn reply(&self, i: usize) {
        self.reply_to(i, &[&sender_of(i)], "Hola, ya está resuelto.");
    }

    fn task(&self, i: usize) -> Task {
        self.app
            .tasks()
            .unwrap()
            .into_iter()
            .find(|t| {
                t.kind == EMAIL
                    && serde_json::from_str::<Value>(&t.input_ref).unwrap()["message_id"]
                        == format!("simulador:m{i}")
            })
            .unwrap_or_else(|| panic!("no hay tarea del correo {i}"))
    }

    fn sql(&self) -> rusqlite::Connection {
        rusqlite::Connection::open(self.dir.path().join("nestadmin.db")).unwrap()
    }

    fn learned(&self) -> Vec<(String, String)> {
        let sql = self.sql();
        let mut stmt = sql
            .prepare(
                "SELECT subject_key, source_key FROM rule_signals WHERE kind = 'learned' ORDER BY id",
            )
            .unwrap();
        let rows = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        rows
    }

    fn signals_of(&self, task: i64, kind: &str) -> i64 {
        self.sql()
            .query_row(
                "SELECT count(*) FROM rule_signals WHERE task_id = ?1 AND kind = ?2",
                rusqlite::params![task, kind],
                |r| r.get(0),
            )
            .unwrap()
    }

    fn lookups(&self) -> Vec<(String, String)> {
        self.sim().sent_lookups()
    }

    fn pass(&mut self) -> ContinuousStep {
        self.app.continuous_learning_pass(ACCOUNT).unwrap()
    }

    fn sends(&self) -> usize {
        self.sim().sent().unwrap().len()
    }
}

fn closed_reason(task: &Task) -> Option<String> {
    task.result.as_ref().unwrap()["closed"]["reason"]
        .as_str()
        .map(str::to_owned)
}

fn is_closed_outside(task: &Task) -> bool {
    task.state == TaskState::Completed && closed_reason(task).as_deref() == Some("replied_outside")
}

// ---- T34a ----

#[test]
fn an_outside_reply_to_the_sender_counts_as_evidence_and_closes_the_review_task_once() {
    let mut e = env(vec![
        incoming(1, &sender_of(1), "¿Pueden enviarme la factura?"),
        incoming(2, &sender_of(2), "¿Tienen novedades de mi factura?"),
        incoming(3, &sender_of(3), "Necesito una factura."),
    ]);
    e.reply(1);
    // Sin respuesta (2) y con respuesta a otra persona (3): nada cambia.
    e.reply_to(3, &["otra-persona@empresa.test"], "Reenvío interno.");
    for i in 1..=3 {
        assert_eq!(e.task(i).state, TaskState::AwaitingReview, "tarea {i}");
    }
    let sends = e.sends();

    assert_eq!(e.pass(), ContinuousStep::Changed);

    let closed = e.task(1);
    assert!(is_closed_outside(&closed), "{closed:?}");
    let result = closed.result.clone().unwrap();
    assert_eq!(
        result["closed"],
        json!({ "reason": "replied_outside", "replied_at": at(2, 12) })
    );
    assert_eq!(result["archived"], json!(true));
    assert!(result.get("action").is_none(), "no queda nada que enviar");
    // Sin envío, sin intento y sin señal `rejected` (ni de ninguna otra clase).
    assert_eq!(e.sends(), sends);
    assert!(result.get("attempt").is_none());
    assert_eq!(e.signals_of(closed.id, "rejected"), 0);
    assert_eq!(e.signals_of(closed.id, "approved"), 0);
    // Una sola fila `learned`, con la clave de la primera carga.
    let source = e.app.account_read_source(ACCOUNT).unwrap();
    assert_eq!(
        e.learned(),
        vec![(
            format!("solicitud_factura|{}", sender_of(1)),
            format!("learned|{source}|simulador:m1")
        )]
    );
    // Historial: una entrada `mail` / `replied_outside` sin contenido y la transición.
    let entries: Vec<_> = e
        .app
        .audit_log()
        .unwrap()
        .into_iter()
        .filter(|a| a.action == "mail")
        .collect();
    assert_eq!(entries.len(), 1);
    assert_eq!(
        (
            entries[0].status.as_str(),
            entries[0].detail.as_deref(),
            entries[0].task_id
        ),
        ("replied_outside", Some("{}"), Some(closed.id))
    );
    let reason: String = e
        .sql()
        .query_row(
            "SELECT reason FROM task_transitions WHERE task_id = ?1 ORDER BY id DESC LIMIT 1",
            [closed.id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(reason, "respondido fuera de NestAdmin");
    for i in 2..=3 {
        let t = e.task(i);
        assert_eq!(t.state, TaskState::AwaitingReview, "tarea {i}");
        assert!(closed_reason(&t).is_none());
    }

    // Resumen: solo recuentos y códigos.
    let overview = e.app.continuous_learning_overview().unwrap();
    let mailbox = &overview.mailboxes[0];
    assert_eq!(
        (
            mailbox.checked,
            mailbox.observed,
            mailbox.learned,
            mailbox.closed
        ),
        (3, 1, 1, 1)
    );
    assert!(mailbox.last_error.is_none() && mailbox.last_pass_at.is_some());

    // Segunda pasada: nada nuevo (ni filas, ni entradas, ni tareas cerradas).
    let audit_before = e.app.audit_log().unwrap().len();
    let learned_before = e.learned();
    assert_ne!(e.pass(), ContinuousStep::Changed);
    assert_eq!(e.learned(), learned_before);
    assert_eq!(e.app.audit_log().unwrap().len(), audit_before);
    assert!(is_closed_outside(&e.task(1)));
    assert_eq!(e.task(2).state, TaskState::AwaitingReview);
}

// ---- T34b ----

#[test]
fn without_evidence_the_review_task_is_still_closed_and_a_nestadmin_send_blocks_the_count() {
    let mut messages = vec![
        incoming(1, &sender_of(1), "Necesito la factura."), // evidencia
        incoming(2, &sender_of(2), "Necesito la factura. [[SIN-RESPUESTA]]"),
        incoming(3, &sender_of(3), "Necesito la factura."), // sin `needs_reply` en el resultado
        incoming(4, &sender_of(4), "Necesito la factura. [[CAT-OTROS]]"),
        incoming(5, &sender_of(5), "Necesito la factura. [[CAT-RECLAMACION]]"),
        incoming(6, &sender_of(6), "Necesito la factura."), // señales sin comprobar
        incoming(
            7,
            "Mail Delivery <mailer-daemon@empresa7.test>",
            "No entregado.",
        ), // rebote
        incoming(8, &sender_of(8), "Necesito la factura."), // se rechaza
        incoming(9, &sender_of(9), "Necesito la factura."), // envío de NestAdmin posterior: no se toca
        incoming(10, &sender_of(9), "Otra consulta más."),  // ...en el mismo hilo
        incoming(11, &sender_of(11), "Necesito la factura."), // control: envío anterior al recibido
        incoming(12, &sender_of(11), "Otra consulta más."),
    ];
    messages[5].signals = None;
    messages[9].thread_key = Some("t9".into());
    messages[9].date = Some(at(1, 9));
    messages[11].thread_key = Some("t11".into());
    messages[11].date = Some(at(1, 9));
    let mut e = env(messages);
    for i in [1, 2, 3, 4, 5, 6, 7, 8, 9, 11] {
        e.reply_to(
            i,
            &[&if i == 7 {
                "mailer-daemon@empresa7.test".to_owned()
            } else {
                sender_of(i)
            }],
            "Hola.",
        );
    }
    // Sin la clave `needs_reply` en el resultado (una tarea anterior a D158): tampoco hay evidencia.
    e.sql()
        .execute(
            "UPDATE tasks SET result = json_remove(result, '$.needs_reply') WHERE id = ?1",
            [e.task(3).id],
        )
        .unwrap();
    assert!(e.task(3).result.unwrap().get("needs_reply").is_none());
    // El envío de NestAdmin al mensaje 10 (hilo t9) es posterior al recibido 9 (D178).
    e.app.approve(e.task(10).id, 1).unwrap();
    // El control: el envío del hilo t11 es anterior al recibido 11 (se retrasa su marca de tiempo).
    e.app.approve(e.task(12).id, 1).unwrap();
    e.sql()
        .execute(
            "UPDATE actions SET created_at = created_at - 30 * 86400000 WHERE task_id = ?1",
            [e.task(12).id],
        )
        .unwrap();
    // Un rechazo cancela la tarea: no se consulta ni cuenta.
    let rejected = e.task(8).id;
    e.app.reject(rejected).unwrap();
    assert_eq!(
        e.task(7).state,
        TaskState::AwaitingReview,
        "el rebote espera revisión"
    );

    assert_eq!(e.pass(), ContinuousStep::Changed);

    for i in [1, 2, 3, 4, 5, 6, 7, 11] {
        assert!(is_closed_outside(&e.task(i)), "la tarea {i} debía cerrarse");
    }
    // Con un envío de NestAdmin posterior en el hilo (el 10, aprobado) la 9 no se consulta, no se
    // cierra y no da evidencia, aunque haya una respuesta manual: ese envío pudo ser la respuesta.
    assert_eq!(e.task(9).state, TaskState::AwaitingReview);
    assert!(!e.lookups().iter().any(|(thread, _)| thread == "t9"));
    // Solo el correo limpio (1) y el del control (11) dan evidencia.
    let source = e.app.account_read_source(ACCOUNT).unwrap();
    let keys: Vec<String> = e.learned().into_iter().map(|(_, k)| k).collect();
    assert_eq!(
        keys,
        vec![
            format!("learned|{source}|simulador:m1"),
            format!("learned|{source}|simulador:m11")
        ]
    );
    // El rechazado no se consultó ni se cerró: sigue cancelado, con su señal `rejected`.
    assert_eq!(e.task(8).state, TaskState::Cancelled);
    assert!(!e.lookups().iter().any(|(thread, _)| thread == "t8"));
    assert_eq!(e.signals_of(rejected, "rejected"), 1);
    // Cerrar no registra rechazos: solo existe el del descarte explícito.
    let all_rejected: i64 = e
        .sql()
        .query_row(
            "SELECT count(*) FROM rule_signals WHERE kind = 'rejected'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(all_rejected, 1);
}

#[test]
fn a_mail_already_counted_by_the_first_load_is_not_counted_again() {
    let mut e = env(vec![incoming(1, &sender_of(1), "Necesito la factura.")]);
    e.reply(1);
    let source = e.app.account_read_source(ACCOUNT).unwrap();
    e.sql()
        .execute(
            "INSERT INTO rule_signals (kind, action_kind, subject_key, edit_state, task_id, content_version, source_key, at, state)
             VALUES ('learned', 'send_draft', ?1, 'unknown', NULL, 0, ?2, 0, 'valid')",
            rusqlite::params![
                format!("solicitud_factura|{}", sender_of(1)),
                format!("learned|{source}|simulador:m1")
            ],
        )
        .unwrap();

    e.pass();

    assert!(is_closed_outside(&e.task(1)), "se cierra igualmente");
    assert_eq!(
        e.learned().len(),
        1,
        "la fila de la primera carga sigue siendo la única"
    );
    let overview = e.app.continuous_learning_overview().unwrap();
    assert_eq!(
        (
            overview.mailboxes[0].observed,
            overview.mailboxes[0].learned
        ),
        (1, 0)
    );
}

#[test]
fn an_archived_mail_gives_evidence_only_when_it_could_and_never_twice() {
    let mut e = env(vec![
        incoming(1, &sender_of(1), "Necesito la factura."), // archivada, con respuesta
        incoming(2, &sender_of(2), "Necesito la factura. [[SIN-RESPUESTA]]"), // archivada, sin evidencia posible
        incoming(3, &sender_of(3), "Necesito la factura."), // archivada con un rechazo registrado
        incoming(4, &sender_of(4), "Necesito la factura."), // archivada, sin respuesta
        incoming(5, "Avisos <no-reply@empresa5.test>", "Su pedido ha salido."), // informativo (triaje)
    ]);
    for i in 1..=4 {
        e.app.archive(e.task(i).id).unwrap();
    }
    assert_eq!(
        e.task(5).state,
        TaskState::Completed,
        "el informativo ya nace cerrado"
    );
    for i in [1, 2, 3, 5] {
        e.reply_to(
            i,
            &[&if i == 5 {
                "no-reply@empresa5.test".to_owned()
            } else {
                sender_of(i)
            }],
            "Hola.",
        );
    }
    let three = e.task(3).id;
    e.sql()
        .execute(
            "INSERT INTO rule_signals (kind, action_kind, subject_key, edit_state, task_id, content_version, source_key, at, state)
             VALUES ('rejected', 'send_draft', 'solicitud_factura|x', 'unknown', ?1, 1, NULL, 0, 'valid')",
            [three],
        )
        .unwrap();

    assert_eq!(e.pass(), ContinuousStep::Changed);

    // Solo se consultaron las que podían dar evidencia: la 1 y la 4.
    let mut threads: Vec<String> = e.lookups().into_iter().map(|(t, _)| t).collect();
    threads.sort();
    assert_eq!(threads, ["t1", "t4"]);
    let source = e.app.account_read_source(ACCOUNT).unwrap();
    assert_eq!(
        e.learned().into_iter().map(|(_, k)| k).collect::<Vec<_>>(),
        vec![format!("learned|{source}|simulador:m1")]
    );
    // La tarea archivada no se toca: ya estaba cerrada por la persona.
    let one = e.task(1);
    assert!(
        closed_reason(&one).is_none() && one.result.as_ref().unwrap()["archived"] == json!(true)
    );

    // Segunda pasada: la 1 ya dio su fila y no se vuelve a consultar; la 4 sí (sin respuesta aún).
    let before = e.lookups().len();
    e.pass();
    let again: Vec<String> = e.lookups()[before..]
        .iter()
        .map(|(t, _)| t.clone())
        .collect();
    assert_eq!(again, ["t4"]);
    assert_eq!(e.learned().len(), 1);
}

// ---- T34c ----

#[test]
fn nothing_is_queried_when_disabled_without_a_sent_folder_or_outside_the_window() {
    let mut old = incoming(2, &sender_of(2), "Necesito la factura.");
    old.date = Some(at(CONTINUOUS_WINDOW_DAYS + 6, 10));
    let mut e = env(vec![
        incoming(1, &sender_of(1), "Necesito la factura."),
        old,
    ]);
    e.reply(1);
    e.sim()
        .add_sent_message(SentMessage {
            id: "s2".into(),
            thread_key: "t2".into(),
            date: Some(at(CONTINUOUS_WINDOW_DAYS + 6, 12)),
            body: Some("Hola.".into()),
            to: vec![sender_of(2)],
        })
        .unwrap();

    // Desactivado: ninguna consulta, ni por la puerta de tiempo ni por una pasada forzada.
    let overview = e.app.set_continuous_learning(false).unwrap();
    assert!(!overview.enabled && !overview.mailboxes[0].enabled);
    assert_eq!(e.pass(), ContinuousStep::Idle);
    assert_eq!(
        e.app.continuous_learning_tick().unwrap(),
        ContinuousStep::Idle
    );
    assert!(e.lookups().is_empty());
    let changes: Vec<_> = e
        .app
        .audit_log()
        .unwrap()
        .into_iter()
        .filter(|a| {
            a.action == "rule_setting"
                && a.detail
                    .as_deref()
                    .is_some_and(|d| d.contains("aprendizaje continuo"))
        })
        .collect();
    assert_eq!(changes.len(), 1, "cada cambio queda en el Historial");
    // Repetir el mismo valor no escribe otra entrada.
    e.app.set_continuous_learning(false).unwrap();
    assert_eq!(
        e.app
            .audit_log()
            .unwrap()
            .iter()
            .filter(|a| a.action == "rule_setting"
                && a.detail
                    .as_deref()
                    .is_some_and(|d| d.contains("aprendizaje continuo")))
            .count(),
        1
    );

    // Conector sin Enviados: ninguna consulta y el código lo dice.
    e.app.set_continuous_learning(true).unwrap();
    e.sim().set_no_sent_folder(true);
    assert_eq!(e.pass(), ContinuousStep::Idle);
    assert!(e.lookups().is_empty());
    let overview = e.app.continuous_learning_overview().unwrap();
    assert_eq!(
        overview.mailboxes[0].last_error.as_deref(),
        Some("no_sent_folder")
    );
    assert!(!is_closed_outside(&e.task(1)));

    // Con Enviados: solo el correo dentro de la ventana de 14 días.
    e.sim().set_no_sent_folder(false);
    assert_eq!(e.pass(), ContinuousStep::Changed);
    assert_eq!(
        e.lookups().into_iter().map(|(t, _)| t).collect::<Vec<_>>(),
        ["t1"]
    );
    assert_eq!(
        e.task(2).state,
        TaskState::AwaitingReview,
        "fuera de la ventana: intacta"
    );
    assert!(e.app.continuous_learning_overview().unwrap().mailboxes[0]
        .last_error
        .is_none());
}

#[test]
fn a_pass_checks_at_most_25_tasks_and_the_longest_unchecked_go_first() {
    let total = CONTINUOUS_MAX_PER_PASS + 5;
    let mut e = env((1..=total)
        .map(|i| incoming(i, &sender_of(i), "Necesito la factura."))
        .collect());
    // Orden esperado: las tareas por id (las más antiguas primero).
    let mut ids: Vec<(i64, String)> = (1..=total)
        .map(|i| (e.task(i).id, format!("t{i}")))
        .collect();
    ids.sort();
    let order: Vec<String> = ids.into_iter().map(|(_, t)| t).collect();

    e.pass();
    let first: Vec<String> = e.lookups().into_iter().map(|(t, _)| t).collect();
    assert_eq!(first, order[..CONTINUOUS_MAX_PER_PASS]);

    // La segunda pasada empieza por las cinco que nunca se comprobaron.
    e.pass();
    let all: Vec<String> = e.lookups().into_iter().map(|(t, _)| t).collect();
    assert_eq!(all.len(), 2 * CONTINUOUS_MAX_PER_PASS);
    assert_eq!(
        all[CONTINUOUS_MAX_PER_PASS..CONTINUOUS_MAX_PER_PASS + 5],
        order[CONTINUOUS_MAX_PER_PASS..]
    );
    let overview = e.app.continuous_learning_overview().unwrap();
    assert_eq!(
        overview.mailboxes[0].checked,
        2 * CONTINUOUS_MAX_PER_PASS as u64
    );
}

#[test]
fn a_network_or_credentials_failure_changes_nothing_and_shows_in_the_overview() {
    let mut e = env(vec![
        incoming(1, &sender_of(1), "Necesito la factura."),
        incoming(2, &sender_of(2), "Necesito la factura."),
    ]);
    e.reply(1);
    e.reply(2);
    let audit_before = e.app.audit_log().unwrap().len();

    for (mode, code) in [
        (FailureMode::Unavailable, "unavailable"),
        (FailureMode::Unauthorized, "unauthorized"),
    ] {
        e.sim().set_sent_failure(Some(mode));
        let before = e.lookups().len();
        e.pass();
        // Se detiene en la primera consulta y no escribe nada.
        assert_eq!(e.lookups().len(), before + 1, "{code}");
        assert_eq!(e.task(1).state, TaskState::AwaitingReview);
        assert_eq!(e.task(2).state, TaskState::AwaitingReview);
        assert!(e.learned().is_empty());
        assert_eq!(e.app.audit_log().unwrap().len(), audit_before);
        let overview = e.app.continuous_learning_overview().unwrap();
        assert_eq!(overview.mailboxes[0].last_error.as_deref(), Some(code));
        assert_eq!(overview.mailboxes[0].checked, 0);
    }

    // Al volver el servicio, la siguiente pasada lo hace y el error desaparece.
    e.sim().set_sent_failure(None);
    assert_eq!(e.pass(), ContinuousStep::Changed);
    assert!(is_closed_outside(&e.task(1)) && is_closed_outside(&e.task(2)));
    assert_eq!(e.learned().len(), 2);
    assert!(e.app.continuous_learning_overview().unwrap().mailboxes[0]
        .last_error
        .is_none());
}

#[test]
fn a_pass_never_calls_the_ai_and_the_reply_text_is_stored_nowhere() {
    let mut e = env(vec![
        incoming(1, &sender_of(1), "Necesito la factura."),
        incoming(2, &sender_of(2), "Necesito la factura."),
    ]);
    e.reply_to(1, &[&sender_of(1)], REPLY_SENTINEL);
    e.app.archive(e.task(2).id).unwrap();
    e.reply_to(2, &[&sender_of(2)], REPLY_SENTINEL);
    let calls = e.spy.calls.load(Ordering::SeqCst);

    assert_eq!(e.pass(), ContinuousStep::Changed);
    assert!(is_closed_outside(&e.task(1)));
    assert_eq!(e.learned().len(), 2);
    assert_eq!(
        e.spy.calls.load(Ordering::SeqCst),
        calls,
        "ninguna llamada de IA en la pasada"
    );
    // El conector espía no entregó ningún cuerpo: solo metadatos (D178, D162).
    assert!(!e.lookups().is_empty());
    assert_eq!(e.sim().sent_body_lookups(), 0);

    // El texto de la respuesta no está en ninguna tabla, en el Historial ni en el resumen.
    let needle = "SENTINELA-789";
    let sql = e.sql();
    let tables: Vec<String> = sql
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table'")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    for table in tables {
        let columns: Vec<String> = sql
            .prepare(&format!("SELECT name FROM pragma_table_info('{table}')"))
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        for column in columns {
            let hits: i64 = sql
                .query_row(
                    &format!(
                        "SELECT count(*) FROM \"{table}\" WHERE CAST(\"{column}\" AS TEXT) LIKE ?1"
                    ),
                    [format!("%{needle}%")],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(hits, 0, "{table}.{column}");
        }
    }
    let overview = e.app.continuous_learning_overview().unwrap();
    let shown = format!("{overview:?} {}", serde_json::to_string(&overview).unwrap());
    assert!(!shown.contains(needle) && !shown.contains("IBAN"));
    for entry in e.app.audit_log().unwrap() {
        assert!(!format!("{entry:?}").contains(needle));
    }
}

#[test]
fn a_task_changed_between_planning_and_the_step_is_not_closed_and_nothing_is_duplicated() {
    let mut e = env(vec![
        incoming(1, &sender_of(1), "Necesito la factura."),
        incoming(2, &sender_of(2), "Necesito la factura."),
        incoming(3, &sender_of(3), "Necesito la factura."),
    ]);
    for i in 1..=3 {
        e.reply(i);
    }
    // La pasada se planifica y, antes de que llegue a cada tarea, la persona aprueba la 1 y
    // archiva la 2 (el cerrojo se soltó entre pasos).
    assert_eq!(e.app.begin_continuous_pass(ACCOUNT).unwrap(), 3);
    let sends = e.sends();
    e.app.approve(e.task(1).id, 1).unwrap();
    e.app.archive(e.task(2).id).unwrap();
    assert_eq!(e.sends(), sends + 1);
    while e.app.step_continuous_pass(ACCOUNT).unwrap() != ContinuousStep::Idle {}

    let approved = e.task(1);
    assert_eq!(approved.state, TaskState::Completed);
    assert!(
        closed_reason(&approved).is_none(),
        "la aprobada no se cierra como respondida fuera"
    );
    assert_eq!(
        approved.result.as_ref().unwrap()["authorization"]["type"],
        "approval"
    );
    let archived = e.task(2);
    assert!(closed_reason(&archived).is_none());
    assert!(is_closed_outside(&e.task(3)));
    // Solo la tercera se consultó y dio fila; ni la aprobada ni la archivada duplican nada.
    assert_eq!(
        e.lookups().into_iter().map(|(t, _)| t).collect::<Vec<_>>(),
        ["t3"]
    );
    assert_eq!(e.learned().len(), 1);
    assert_eq!(e.sends(), sends + 1, "no se envía nada más");
    let closings = e
        .app
        .audit_log()
        .unwrap()
        .into_iter()
        .filter(|a| a.action == "mail" && a.status == "replied_outside")
        .count();
    assert_eq!(closings, 1);
}

#[test]
fn the_idle_loop_step_waits_fifteen_minutes_and_a_fetch_brings_the_next_pass_forward_but_not_under_two(
) {
    let mut e = env(vec![incoming(1, &sender_of(1), "Necesito la factura.")]);
    let base = Instant::now();
    let minutes = |m: u64| base + Duration::from_secs(m * 60);
    let run = |app: &mut App, at: Instant| {
        let mut steps = 0;
        while app.continuous_learning_tick_at(at).unwrap() != ContinuousStep::Idle {
            steps += 1;
            assert!(steps < 50);
        }
        steps
    };
    assert!(run(&mut e.app, minutes(0)) > 0, "la primera vez toca ya");
    assert_eq!(e.lookups().len(), 1);
    // Sin respuesta la tarea sigue esperando, pero la puerta de tiempo impide otra pasada.
    assert_eq!(run(&mut e.app, minutes(1)), 0);
    // Una consulta de correo correcta la adelanta, pero no antes de 2 minutos desde la anterior.
    e.app.fetch_mail().unwrap();
    assert_eq!(run(&mut e.app, minutes(1)), 0, "menos de 2 minutos");
    assert_eq!(e.lookups().len(), 1);
    assert!(
        run(&mut e.app, minutes(3)) > 0,
        "adelantada, pasados 2 minutos"
    );
    assert_eq!(e.lookups().len(), 2);
    // Sin adelanto, solo a los 15 minutos de la última.
    assert_eq!(run(&mut e.app, minutes(17)), 0);
    assert!(run(&mut e.app, minutes(19)) > 0);
    assert_eq!(e.lookups().len(), 3);
    // Desactivado: ni la puerta ni el adelanto consultan.
    e.app.set_continuous_learning(false).unwrap();
    e.app.fetch_mail().unwrap();
    assert_eq!(run(&mut e.app, minutes(60)), 0);
    assert_eq!(e.lookups().len(), 3);
}

// ---- T34d (parte de fondo) ----

fn three_replies_from_one_address(addresses: [&str; 3]) -> Env {
    let mut e = env(addresses
        .iter()
        .enumerate()
        .map(|(n, a)| incoming(n + 1, a, "Necesito la factura."))
        .collect());
    for (n, a) in addresses.iter().enumerate() {
        e.reply_to(n + 1, &[*a], "Hola.");
    }
    assert_eq!(e.pass(), ContinuousStep::Changed);
    e
}

fn rodaje_of(e: &Env, rule: &str) -> Value {
    let origin: String = e
        .sql()
        .query_row(
            "SELECT origin_json FROM rule_meta WHERE rule_id = ?1",
            [rule],
            |r| r.get(0),
        )
        .unwrap();
    serde_json::from_str::<Value>(&origin).unwrap()
}

#[test]
fn three_observed_replies_of_one_sender_suggest_a_rule_and_the_card_gives_it_rodaje() {
    let a = "ana@alfa.test";
    let mut e = three_replies_from_one_address([a, a, a]);
    let suggestions = e.app.rule_suggestions().unwrap();
    let candidate = suggestions
        .candidates
        .iter()
        .find(|c| c.subject == a)
        .expect("la sugerencia aprendida");
    assert_eq!(
        (
            candidate.evidence.origin,
            candidate.evidence.count,
            candidate.evidence.learned
        ),
        ("learned", 3, 3)
    );
    let view = e
        .app
        .accept_suggestion(&candidate.fingerprint, &candidate.shown, None)
        .unwrap();
    assert!(!view.rule.active, "la regla sigue naciendo inactiva");
    let origin = rodaje_of(&e, &view.rule.id);
    assert_eq!(origin["evidence_origin"], "learned");
    assert_eq!(origin["rodaje"], json!({ "left": 3, "of": 3 }));
    assert!(origin.get("batch").is_none_or(Value::is_null));
}

#[test]
fn a_domain_rule_accepted_with_its_card_carries_the_address_coverage_in_its_rodaje() {
    let mut e =
        three_replies_from_one_address(["ana@alfa.test", "luis@alfa.test", "luis@alfa.test"]);
    let suggestions = e.app.rule_suggestions().unwrap();
    let candidate = suggestions
        .candidates
        .iter()
        .find(|c| c.conditions.get("sender_domain").is_some())
        .expect("la sugerencia por dominio");
    let view = e
        .app
        .accept_suggestion(&candidate.fingerprint, &candidate.shown, None)
        .unwrap();
    let rodaje = rodaje_of(&e, &view.rule.id)["rodaje"].clone();
    assert_eq!(rodaje["left"], 3);
    assert_eq!(rodaje["min_addresses"], 2);
    assert_eq!(rodaje["address_hashes"], json!([]));
}

#[test]
fn the_batch_also_gives_rodaje_to_a_rule_born_from_observed_replies() {
    let a = "ana@alfa.test";
    let mut e = three_replies_from_one_address([a, a, a]);
    let batch = e.app.learned_suggestions_batch().unwrap();
    assert_eq!(batch.items.len(), 1);
    let ids: Vec<String> = batch.items.iter().map(|i| i.id.clone()).collect();
    let outcome = e
        .app
        .accept_and_activate_learned(&batch.batch_fp, &ids)
        .unwrap();
    assert!(outcome.applied, "{outcome:?}");
    assert_eq!(outcome.rules[0].rodaje_left, Some(3));
    let origin = rodaje_of(&e, &outcome.rules[0].rule_id);
    assert_eq!(origin["batch"], json!(true));
    assert_eq!(origin["rodaje"], json!({ "left": 3, "of": 3 }));
}

// ---- Revisión de arquitectura (A1, M1, M2, B1, B2, B3, fuente única) ----

#[test]
fn nestadmins_reply_to_another_mail_of_the_thread_does_not_close_an_unanswered_task() {
    // M9 y M10 del mismo remitente y del mismo hilo, los dos en revisión. La persona aprueba el 10
    // en NestAdmin y se envía. Nadie sembró ninguna respuesta manual.
    let mut m10 = incoming(10, &sender_of(9), "Otra consulta más.");
    m10.thread_key = Some("t9".into());
    m10.date = Some(at(1, 9));
    let mut e = env(vec![
        incoming(9, &sender_of(9), "Necesito la factura."),
        m10,
    ]);
    e.app.approve(e.task(10).id, 1).unwrap();
    assert_eq!(e.sends(), 1);
    assert_eq!(e.task(9).state, TaskState::AwaitingReview);

    assert_ne!(e.pass(), ContinuousStep::Changed);

    assert_eq!(
        e.task(9).state,
        TaskState::AwaitingReview,
        "sigue pidiendo respuesta"
    );
    assert!(closed_reason(&e.task(9)).is_none());
    assert!(e.learned().is_empty());
    assert!(e.lookups().is_empty(), "ni se consultó Enviados para ella");
    // La segunda pasada tampoco consulta nada.
    e.pass();
    assert!(e.lookups().is_empty());

    // El simulador es realista: lo que envía NestAdmin consta en Enviados del hilo, dirigido al
    // remitente (se comprueba al final porque preguntar cuenta como consulta).
    use nestadmin_simulators::MailConnector;
    let sent = e.sim().own_replies_meta("t9", "m9").unwrap();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].to, vec![sender_of(9)]);
}

#[test]
fn an_archived_mail_with_a_nestadmin_send_after_it_in_the_thread_is_not_queried_either() {
    let mut second = incoming(2, &sender_of(1), "Otra consulta más.");
    second.thread_key = Some("t1".into());
    second.date = Some(at(1, 9));
    let mut e = env(vec![
        incoming(1, &sender_of(1), "Necesito la factura."),
        second,
    ]);
    e.app.archive(e.task(1).id).unwrap();
    e.app.approve(e.task(2).id, 1).unwrap();
    e.pass();
    assert!(e.lookups().is_empty());
    assert!(e.learned().is_empty());
}

#[test]
fn the_first_reply_to_the_sender_counts_even_after_an_internal_forward() {
    let mut e = env(vec![incoming(1, &sender_of(1), "Necesito la factura.")]);
    // Primero un reenvío interno a un tercero y después la respuesta al remitente.
    e.sim()
        .add_sent_message(SentMessage {
            id: "s1-fwd".into(),
            thread_key: "t1".into(),
            date: Some(at(2, 11)),
            body: Some("Reenvío interno.".into()),
            to: vec!["jefe@empresa.test".into()],
        })
        .unwrap();
    e.sim()
        .add_sent_message(SentMessage {
            id: "s1-ans".into(),
            thread_key: "t1".into(),
            date: Some(at(2, 13)),
            body: Some("Respuesta.".into()),
            to: vec![sender_of(1)],
        })
        .unwrap();
    assert_eq!(e.pass(), ContinuousStep::Changed);
    let task = e.task(1);
    assert!(is_closed_outside(&task));
    assert_eq!(
        task.result.unwrap()["closed"]["replied_at"],
        json!(at(2, 13)),
        "la fecha es la de la respuesta al remitente"
    );
    assert_eq!(e.learned().len(), 1);
}

#[test]
fn a_task_that_always_fails_with_its_own_error_does_not_block_the_others() {
    for code in ["invalid_input", "not_found"] {
        let mut e = env(vec![
            incoming(1, &sender_of(1), "Necesito la factura."),
            incoming(2, &sender_of(2), "Necesito la factura."),
            incoming(3, &sender_of(3), "Necesito la factura."),
            incoming(4, &sender_of(4), "Necesito la factura."),
        ]);
        e.sim().set_sent_thread_error("t1", Some(code));
        e.reply(2);
        e.reply(3);

        // La primera de la cola falla siempre; las demás se comprueban en la misma pasada.
        assert_eq!(e.pass(), ContinuousStep::Changed, "{code}");
        assert!(
            is_closed_outside(&e.task(2)) && is_closed_outside(&e.task(3)),
            "{code}"
        );
        assert_eq!(e.task(1).state, TaskState::AwaitingReview);
        let overview = e.app.continuous_learning_overview().unwrap();
        assert!(
            overview.mailboxes[0].last_error.is_none(),
            "{code}: no es un error del buzón"
        );
        assert_eq!(
            overview.mailboxes[0].checked, 3,
            "{code}: la 2, la 3 y la 4"
        );

        // La pasada siguiente también sigue: una respuesta nueva a la 4 se recoge.
        e.reply(4);
        assert_eq!(e.pass(), ContinuousStep::Changed, "{code}");
        assert!(is_closed_outside(&e.task(4)), "{code}");
        assert!(e.lookups().iter().filter(|(t, _)| t == "t1").count() >= 2);
    }
}

#[test]
fn a_rejection_or_an_edit_after_the_reply_cuts_the_evidence_but_one_before_does_not() {
    // A: otro correo del mismo remitente se rechaza después. B: se edita su borrador después.
    // C: se edita, pero la señal es anterior a la respuesta. D: control limpio.
    let mut e = env(vec![
        incoming(1, &sender_of(1), "Necesito la factura."),
        incoming(2, &sender_of(1), "Necesito otra factura."),
        incoming(3, &sender_of(3), "Necesito la factura."),
        incoming(4, &sender_of(3), "Necesito otra factura."),
        incoming(5, &sender_of(5), "Necesito la factura."),
        incoming(6, &sender_of(5), "Necesito otra factura."),
        incoming(7, &sender_of(7), "Necesito la factura."),
    ]);
    for i in [1, 3, 5, 7] {
        e.reply(i);
    }
    e.app.reject(e.task(2).id).unwrap();
    e.app.edit_draft(e.task(4).id, "Texto distinto.").unwrap();
    e.app.edit_draft(e.task(6).id, "Texto distinto.").unwrap();
    // La señal de C se registró antes de la respuesta: se retrasa su marca de tiempo.
    e.sql()
        .execute(
            "UPDATE rule_signals SET at = at - 30 * 86400000 WHERE kind = 'edited' AND task_id = ?1",
            [e.task(6).id],
        )
        .unwrap();

    assert_eq!(e.pass(), ContinuousStep::Changed);

    for i in [1, 3, 5, 7] {
        assert!(
            is_closed_outside(&e.task(i)),
            "la tarea {i} se cierra igualmente"
        );
    }
    let source = e.app.account_read_source(ACCOUNT).unwrap();
    let keys: Vec<String> = e.learned().into_iter().map(|(_, k)| k).collect();
    assert_eq!(
        keys,
        vec![
            format!("learned|{source}|simulador:m5"),
            format!("learned|{source}|simulador:m7")
        ],
        "A y B quedan sin fila; C y D la dan"
    );
}

#[test]
fn an_edited_draft_in_the_task_itself_gives_no_evidence() {
    let mut e = env(vec![incoming(1, &sender_of(1), "Necesito la factura.")]);
    e.app.edit_draft(e.task(1).id, "Texto distinto.").unwrap();
    // Aunque la señal no existiera, el propio borrador editado basta.
    e.sql().execute("DELETE FROM rule_signals", []).unwrap();
    e.reply(1);
    e.pass();
    assert!(is_closed_outside(&e.task(1)));
    assert!(e.learned().is_empty());
}

#[test]
fn a_sender_written_as_its_own_address_in_angle_brackets_is_one_address() {
    let mut e = env(vec![
        incoming(1, "ana@x.test <ana@x.test>", "Necesito la factura."),
        incoming(2, "Ana <a@x.test>, Luis <l@x.test>", "Necesito la factura."),
    ]);
    e.reply_to(1, &["ana@x.test"], "Hola.");
    e.reply_to(2, &["l@x.test"], "Hola.");
    e.pass();
    let source = e.app.account_read_source(ACCOUNT).unwrap();
    assert_eq!(
        e.learned(),
        vec![(
            "solicitud_factura|ana@x.test".to_owned(),
            format!("learned|{source}|simulador:m1")
        )]
    );
}

#[test]
fn redraft_is_refused_on_a_task_closed_as_replied_outside() {
    let mut e = env(vec![
        incoming(1, &sender_of(1), "Consulta. [[SIN-RESPUESTA]]"),
        incoming(2, &sender_of(2), "Consulta. [[SIN-RESPUESTA]]"),
    ]);
    e.reply(1);
    e.pass();
    let closed = e.task(1);
    assert!(is_closed_outside(&closed));
    let error = e.app.redraft(closed.id).unwrap_err().to_string();
    assert!(error.contains("ya lo respondió usted"), "{error}");
    // El control: archivada a mano, sin respuesta fuera, sigue pudiendo redactar.
    e.app.archive(e.task(2).id).unwrap();
    assert!(e.app.redraft(e.task(2).id).is_ok());
}

#[test]
fn the_first_load_and_the_continuous_learning_write_the_same_source_in_the_signal_key() {
    // La primera carga (local, para no necesitar consentimiento) aprende del correo 1, que ya
    // tiene su respuesta; el 2 se responde después y lo recoge el aprendizaje continuo.
    let mut old = incoming(1, &sender_of(1), "Necesito la factura.");
    old.date = Some(at(30, 10));
    let mut e = open_env(
        ProviderKind::Ollama,
        vec![old, incoming(2, &sender_of(2), "Necesito la factura.")],
    );
    e.sim()
        .add_sent_message(SentMessage {
            id: "s1".into(),
            thread_key: "t1".into(),
            date: Some(at(30, 12)),
            body: Some("Hola, aquí la factura.".into()),
            to: vec![sender_of(1)],
        })
        .unwrap();
    e.app
        .set_refine_mode(nestadmin_app::refine::RefineMode::OnDemand);
    let info = e.app.provider();
    let expect = nestadmin_app::initial_read::ReadExpectation {
        provider: info.provider.to_string(),
        model: info.model,
        host: String::new(),
        volume: 25,
    };
    e.app
        .start_initial_read(ACCOUNT, Some(25), Some(&expect))
        .unwrap();
    process_all(&mut e.app);
    e.app.run_learning_ticks().unwrap();
    let first_load = e.learned();
    assert_eq!(
        first_load.len(),
        1,
        "la primera carga dejó su fila: {first_load:?}"
    );

    // El correo 2 entra con una consulta normal y espera revisión; se responde fuera después.
    e.app.fetch_mail().unwrap();
    process_all(&mut e.app);
    e.reply(2);
    e.pass();
    let all = e.learned();
    assert_eq!(all.len(), 2, "{all:?}");
    let source = e.app.account_read_source(ACCOUNT).unwrap();
    let prefix = format!("learned|{source}|");
    for (_, key) in &all {
        assert!(key.starts_with(&prefix), "{key} no empieza por {prefix}");
    }
    assert_eq!(all[0].1, format!("{prefix}simulador:m1"));
    assert_eq!(all[1].1, format!("{prefix}simulador:m2"));
}

// ---- T34c: tareas sin hilo (D178) ----

impl Env {
    /// Deja la tarea del correo `i` como las creadas antes de que se guardara el hilo: sin
    /// `message.thread_key` (misma fila, mismo estado y mismo `updated_at`).
    fn strip_thread(&self, i: usize) {
        let changed = self
            .sql()
            .execute(
                "UPDATE tasks SET result = json_remove(result, '$.message.thread_key') WHERE id = ?1",
                [self.task(i).id],
            )
            .unwrap();
        assert_eq!(changed, 1);
        assert!(self.task(i).result.unwrap()["message"]
            .get("thread_key")
            .is_none());
    }

    fn thread_calls(&self) -> Vec<String> {
        self.sim().thread_of_lookups()
    }
}

fn thread_of_task(task: &Task) -> (Option<String>, Option<bool>) {
    let message = &task.result.as_ref().unwrap()["message"];
    (
        message["thread_key"].as_str().map(str::to_owned),
        message["thread_recovered"].as_bool(),
    )
}

#[test]
fn a_task_without_thread_recovers_it_and_is_then_closed_like_any_other_with_evidence() {
    let mut e = env(vec![
        incoming(1, &sender_of(1), "Necesito la factura."),
        incoming(2, &sender_of(2), "Necesito otra factura."),
        incoming(3, &sender_of(3), "Necesito la tercera."),
    ]);
    // 1 en revisión y 2 archivada, ambas sin hilo; 3 con hilo.
    e.app.archive(e.task(2).id).unwrap();
    e.strip_thread(1);
    e.strip_thread(2);
    for i in 1..=3 {
        e.reply(i);
    }
    let sends = e.sends();
    let calls = e.spy.calls.load(Ordering::SeqCst);

    assert_eq!(e.pass(), ContinuousStep::Changed);

    // Una sola consulta de hilo por tarea sin hilo; la que ya lo tenía no pregunta.
    let mut asked = e.thread_calls();
    asked.sort();
    assert_eq!(asked, ["m1", "m2"]);
    let task1 = e.task(1);
    assert!(is_closed_outside(&task1), "{task1:?}");
    assert_eq!(thread_of_task(&task1), (Some("t1".into()), Some(true)));
    assert_eq!(
        task1.result.as_ref().unwrap()["closed"],
        json!({ "reason": "replied_outside", "replied_at": at(2, 12) })
    );
    // La archivada recupera su hilo y da evidencia, como cualquier otra.
    let task2 = e.task(2);
    assert_eq!(thread_of_task(&task2), (Some("t2".into()), Some(true)));
    assert!(!is_closed_outside(&task2));
    assert!(is_closed_outside(&e.task(3)));
    assert_eq!(thread_of_task(&e.task(3)), (Some("t3".into()), None));
    let source = e.app.account_read_source(ACCOUNT).unwrap();
    let mut keys: Vec<String> = e.learned().into_iter().map(|(_, k)| k).collect();
    keys.sort();
    assert_eq!(
        keys,
        (1..=3)
            .map(|i| format!("learned|{source}|simulador:m{i}"))
            .collect::<Vec<_>>()
    );
    // Solo metadatos, sin IA y sin envíos.
    assert_eq!(e.sim().sent_body_lookups(), 0);
    assert_eq!(e.spy.calls.load(Ordering::SeqCst), calls);
    assert_eq!(e.sends(), sends);

    // La segunda pasada no vuelve a pedir ningún hilo ni duplica nada.
    let learned = e.learned();
    e.pass();
    assert_eq!(e.thread_calls().len(), 2);
    assert_eq!(e.learned(), learned);
}

#[test]
fn without_a_reply_the_recovered_thread_is_kept_and_the_task_stays_in_review() {
    let mut e = env(vec![
        incoming(1, &sender_of(1), "Necesito la factura."),
        incoming(2, &sender_of(2), "Necesito otra factura."),
    ]);
    e.strip_thread(1);
    e.strip_thread(2);
    // Respuesta a un tercero en el hilo 1: tampoco cuenta.
    e.reply_to(1, &["otra-persona@empresa.test"], "Reenvío interno.");
    let updated: Vec<i64> = (1..=2).map(|i| e.task(i).updated_at).collect();

    assert_ne!(e.pass(), ContinuousStep::Changed);

    for i in 1..=2 {
        let task = e.task(i);
        assert_eq!(task.state, TaskState::AwaitingReview);
        assert!(closed_reason(&task).is_none());
        assert_eq!(
            thread_of_task(&task),
            (Some(format!("t{i}")), Some(true)),
            "tarea {i}"
        );
        // El resto del resultado no cambia y la tarea no parece tocada.
        assert_eq!(task.updated_at, updated[i - 1]);
        assert_eq!(task.result.as_ref().unwrap()["needs_reply"], json!(true));
    }
    assert!(e.learned().is_empty());
    assert_eq!(e.thread_calls().len(), 2);
    // Ya con hilo: la pasada siguiente solo consulta Enviados.
    let sent_before = e.lookups().len();
    e.pass();
    assert_eq!(e.thread_calls().len(), 2);
    assert_eq!(e.lookups().len(), sent_before + 2);
}

#[test]
fn a_message_that_is_gone_is_noted_and_not_asked_again_while_other_errors_follow_the_pass_rules() {
    let mut e = env(vec![
        incoming(1, &sender_of(1), "Necesito la factura."),
        incoming(2, &sender_of(2), "Necesito otra factura."),
        incoming(3, &sender_of(3), "Necesito la tercera."),
    ]);
    for i in 1..=3 {
        e.strip_thread(i);
        e.reply(i);
    }
    e.sim().set_thread_of_error("m1", Some("not_found"));
    e.sim().set_thread_of_error("m2", Some("invalid_input"));

    // `not_found` e `invalid_input` saltan su tarea; la tercera sigue.
    assert_eq!(e.pass(), ContinuousStep::Changed);
    assert!(is_closed_outside(&e.task(3)));
    for i in 1..=2 {
        assert_eq!(e.task(i).state, TaskState::AwaitingReview);
        assert!(e.task(i).result.unwrap()["message"]
            .get("thread_key")
            .is_none());
    }
    let mut asked = e.thread_calls();
    asked.sort();
    assert_eq!(asked, ["m1", "m2", "m3"]);
    assert!(e.app.continuous_learning_overview().unwrap().mailboxes[0]
        .last_error
        .is_none());

    // Siguiente pasada: el mensaje que no existe no se pide más; el de `invalid_input` sí.
    e.pass();
    let mut again = e.thread_calls();
    again.sort();
    assert_eq!(again, ["m1", "m2", "m2", "m3"]);
    // Aunque el mensaje vuelva a existir, en esta sesión no se insiste.
    e.sim().set_thread_of_error("m1", None);
    e.sim().set_thread_of_error("m2", None);
    e.pass();
    assert!(!e.thread_calls().iter().skip(4).any(|m| m == "m1"));
    assert_eq!(thread_of_task(&e.task(2)).0.as_deref(), Some("t2"));
    assert!(is_closed_outside(&e.task(2)));
    assert_eq!(e.task(1).state, TaskState::AwaitingReview);

    // Un fallo de red, de límite o de credenciales detiene la pasada sin escribir nada.
    let mut e = env(vec![
        incoming(1, &sender_of(1), "Necesito la factura."),
        incoming(2, &sender_of(2), "Necesito otra factura."),
    ]);
    for i in 1..=2 {
        e.strip_thread(i);
        e.reply(i);
    }
    for (mode, code) in [
        (FailureMode::Unavailable, "unavailable"),
        (FailureMode::Unauthorized, "unauthorized"),
    ] {
        e.sim().set_sent_failure(Some(mode));
        let before = e.thread_calls().len();
        e.pass();
        assert_eq!(e.thread_calls().len(), before + 1, "{code}: se detiene");
        assert!(e.task(1).result.unwrap()["message"]
            .get("thread_key")
            .is_none());
        assert_eq!(e.task(2).state, TaskState::AwaitingReview);
        let overview = e.app.continuous_learning_overview().unwrap();
        assert_eq!(overview.mailboxes[0].last_error.as_deref(), Some(code));
    }
    e.sim().set_sent_failure(None);
    assert_eq!(e.pass(), ContinuousStep::Changed);
    assert!(is_closed_outside(&e.task(1)) && is_closed_outside(&e.task(2)));
}

#[test]
fn a_task_that_changes_after_planning_is_neither_asked_for_its_thread_nor_written() {
    let mut e = env(vec![
        incoming(1, &sender_of(1), "Necesito la factura."),
        incoming(2, &sender_of(2), "Necesito otra factura."),
    ]);
    for i in 1..=2 {
        e.strip_thread(i);
        e.reply(i);
    }
    assert_eq!(e.app.begin_continuous_pass(ACCOUNT).unwrap(), 2);
    // La persona archiva la 1 antes de que llegue su paso.
    e.app.archive(e.task(1).id).unwrap();
    while e.app.step_continuous_pass(ACCOUNT).unwrap() != ContinuousStep::Idle {}

    assert_eq!(e.thread_calls(), ["m2"], "la cambiada no se consulta");
    let archived = e.task(1);
    assert!(archived.result.as_ref().unwrap()["message"]
        .get("thread_key")
        .is_none());
    assert!(closed_reason(&archived).is_none());
    assert!(is_closed_outside(&e.task(2)));
}

#[test]
fn a_nestadmin_send_in_the_recovered_thread_still_blocks_the_task() {
    // Dos correos del mismo hilo: NestAdmin responde al 1 y el 2 (sin hilo guardado) no se cierra
    // con la respuesta que se vería en Enviados.
    let mut first = incoming(1, &sender_of(1), "Necesito la factura.");
    first.thread_key = Some("tx".into());
    let mut second = incoming(2, &sender_of(2), "Otra consulta más.");
    second.thread_key = Some("tx".into());
    let mut e = env(vec![first, second]);
    e.app.approve(e.task(1).id, 1).unwrap();
    e.strip_thread(2);
    e.sim()
        .add_sent_message(SentMessage {
            id: "s2".into(),
            thread_key: "tx".into(),
            date: Some(at(2, 12)),
            body: None,
            to: vec![sender_of(2)],
        })
        .unwrap();

    assert_ne!(e.pass(), ContinuousStep::Changed);

    let task = e.task(2);
    assert_eq!(task.state, TaskState::AwaitingReview);
    assert_eq!(thread_of_task(&task), (Some("tx".into()), Some(true)));
    assert!(closed_reason(&task).is_none());
    assert!(e.lookups().is_empty(), "no se consultó Enviados");
    assert!(e.learned().is_empty());
}

#[test]
fn the_cap_of_25_counts_the_recoveries() {
    // 20 con hilo y 10 sin él, todas respondidas.
    let total = 30;
    let mut e = env((1..=total)
        .map(|i| incoming(i, &sender_of(i), "Necesito la factura."))
        .collect());
    for i in 21..=total {
        e.strip_thread(i);
    }
    for i in 1..=total {
        e.reply(i);
    }
    e.pass();
    let threads = e.thread_calls().len();
    let sent = e.lookups().len();
    // Las 20 con hilo, después las sin hilo hasta agotar las 25 consultas del buzón.
    assert_eq!(threads + sent, CONTINUOUS_MAX_PER_PASS);
    assert_eq!((threads, sent), (3, 22));
    for i in 1..=20 {
        assert!(is_closed_outside(&e.task(i)), "{i}");
    }
    // Las recuperadas sin presupuesto para comprobarse guardan su hilo y siguen en revisión.
    assert!(is_closed_outside(&e.task(21)) && is_closed_outside(&e.task(22)));
    assert_eq!(
        thread_of_task(&e.task(23)),
        (Some("t23".into()), Some(true))
    );
    assert_eq!(e.task(23).state, TaskState::AwaitingReview);
    assert!(e.task(24).result.unwrap()["message"]
        .get("thread_key")
        .is_none());

    // Todo sin hilo: cada tarea cuesta dos consultas (el hilo y Enviados), así que una pasada
    // da para 12 completas y la recuperación de la 13.ª; esa guarda su hilo y espera.
    let mut e = env((1..=total)
        .map(|i| incoming(i, &sender_of(i), "Necesito la factura."))
        .collect());
    for i in 1..=total {
        e.strip_thread(i);
        e.reply(i);
    }
    e.pass();
    assert_eq!((e.thread_calls().len(), e.lookups().len()), (13, 12));
    assert_eq!(
        thread_of_task(&e.task(13)),
        (Some("t13".into()), Some(true))
    );
    assert_eq!(e.task(13).state, TaskState::AwaitingReview);
    assert!((1..=12).all(|i| is_closed_outside(&e.task(i))));
    // La siguiente empieza por las que nunca se comprobaron; la 13.ª, ya anotada, espera su turno.
    e.pass();
    assert_eq!(e.lookups()[12], ("t14".to_owned(), "m14".to_owned()));
    assert_eq!(e.thread_calls().len() + e.lookups().len(), 2 * 25);
}

#[test]
fn review_goes_before_archived_and_having_a_thread_does_not_set_the_order() {
    let mut e = env(vec![
        incoming(1, &sender_of(1), "Necesito la factura."),
        incoming(2, &sender_of(2), "Necesito otra factura."),
        incoming(3, &sender_of(3), "Necesito la tercera."),
    ]);
    e.app.archive(e.task(1).id).unwrap();
    e.strip_thread(1);
    e.strip_thread(2);
    for i in 1..=3 {
        e.reply(i);
    }
    e.pass();
    // Revisión (2 sin hilo y 3 con él, por antigüedad de comprobación) y después la archivada.
    assert_eq!(e.thread_calls(), ["m2", "m1"]);
    let order: Vec<String> = e.lookups().into_iter().map(|(t, _)| t).collect();
    assert_eq!(order, ["t2", "t3", "t1"]);
}

#[test]
fn tasks_without_thread_are_not_starved_by_those_with_one() {
    // 30 en revisión: 25 con hilo y 5 sin él (las últimas), sin respuesta alguna.
    let total = 30;
    let mut e = env((1..=total)
        .map(|i| incoming(i, &sender_of(i), "Necesito la factura."))
        .collect());
    for i in 26..=total {
        e.strip_thread(i);
    }
    e.pass();
    e.pass();
    let mut checked: Vec<String> = e.lookups().into_iter().map(|(t, _)| t).collect();
    checked.sort();
    checked.dedup();
    let mut all: Vec<String> = (1..=total).map(|i| format!("t{i}")).collect();
    all.sort();
    assert_eq!(checked, all, "las 30 se comprobaron al cabo de dos pasadas");
    for i in 26..=total {
        assert_eq!(
            thread_of_task(&e.task(i)),
            (Some(format!("t{i}")), Some(true))
        );
    }
    assert_eq!(e.thread_calls().len(), 5);
}

#[test]
fn a_send_by_nestadmin_in_an_old_task_without_thread_blocks_the_others_recovered() {
    // A (respondida por NestAdmin) y B son del mismo hilo y ninguna guarda el hilo: el envío de A
    // podría ser de ese hilo, así que B no se cierra ni da evidencia (falla cerrado).
    let mut a = incoming(1, &sender_of(1), "Necesito la factura.");
    a.thread_key = Some("tx".into());
    let mut b = incoming(2, &sender_of(2), "Otra consulta más.");
    b.thread_key = Some("tx".into());
    let mut e = env(vec![a, b]);
    e.app.approve(e.task(1).id, 1).unwrap();
    e.strip_thread(1);
    e.strip_thread(2);
    e.sim()
        .add_sent_message(SentMessage {
            id: "s2".into(),
            thread_key: "tx".into(),
            date: Some(at(2, 12)),
            body: None,
            to: vec![sender_of(2)],
        })
        .unwrap();

    for _ in 0..2 {
        assert_ne!(e.pass(), ContinuousStep::Changed);
        let task = e.task(2);
        assert_eq!(task.state, TaskState::AwaitingReview);
        assert!(closed_reason(&task).is_none());
        // El hilo queda guardado, pero no se consulta Enviados ni hay evidencia.
        assert_eq!(thread_of_task(&task), (Some("tx".into()), Some(true)));
        assert!(e.lookups().is_empty());
        assert!(e.learned().is_empty());
    }
    assert_eq!(
        e.thread_calls(),
        ["m2"],
        "la recuperada no vuelve a pedirse"
    );
}
