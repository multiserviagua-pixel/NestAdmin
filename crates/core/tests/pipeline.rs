//! Bloqueo derivado de la cola por ejecuciones de pipeline (D148): predicado único de
//! «lista», `claim`, `blocked_by`, recuperación y atomicidad de las altas. Datos sintéticos.

use std::path::Path;

use nestadmin_core::{CoreError, NewStep, Store, TaskError, TaskState};
use serde_json::json;

fn open(dir: &tempfile::TempDir) -> Store {
    Store::open(&dir.path().join("t.db")).unwrap()
}

fn step(stage: i64, kind: &str, key: Option<&str>) -> NewStep {
    NewStep {
        stage,
        kind: kind.into(),
        label: format!("{kind} {}", key.unwrap_or("")),
        attachment_key: key.map(Into::into),
        subject: false,
    }
}

fn subject(stage: i64, kind: &str) -> NewStep {
    NewStep {
        subject: true,
        ..step(stage, kind, None)
    }
}

/// Crea los pasos pendientes de una ejecución como tareas, en el orden de los pasos.
fn download_all(store: &mut Store, subject_task: i64) -> Vec<i64> {
    let run = store.pipeline_run_of(subject_task).unwrap().unwrap();
    let mut out = Vec::new();
    for s in store.pipeline_steps(run).unwrap() {
        if s.task_id.is_none() {
            out.push(store.enqueue_for_step(s.id, "invoice", "{}").unwrap().id);
        }
    }
    out
}

fn ready_ids(store: &Store) -> Vec<i64> {
    store.ready_tasks().unwrap().iter().map(|t| t.id).collect()
}

fn error() -> TaskError {
    TaskError {
        code: "unavailable".into(),
        message: "sin conexión".into(),
        recoverable: true,
    }
}

/// Lleva una tarea en cola al estado pedido por el camino permitido.
fn put_in(store: &mut Store, id: i64, state: TaskState) {
    match state {
        TaskState::Queued => {}
        TaskState::Running => {
            store.claim(id).unwrap();
        }
        TaskState::AwaitingReview => {
            store.claim(id).unwrap();
            store.await_review(id, &json!({})).unwrap();
        }
        TaskState::Completed => {
            store.claim(id).unwrap();
            store.complete(id, &json!({})).unwrap();
        }
        TaskState::Failed => {
            store.claim(id).unwrap();
            store.fail(id, &error()).unwrap();
        }
        TaskState::Cancelled => {
            store.cancel(id).unwrap();
        }
    }
}

/// Una factura por adjunto en la etapa 0 y la respuesta (tarea del correo) en la etapa 1.
/// Devuelve (respuesta, facturas).
fn run_with(store: &mut Store, invoices: usize) -> (i64, Vec<i64>) {
    let mut steps: Vec<NewStep> = (0..invoices)
        .map(|n| step(0, "invoice", Some(&format!("m#{n}"))))
        .collect();
    steps.push(subject(1, "reply"));
    let reply = store
        .enqueue_with_run("email", "{}", "t", 1, "[]", &steps)
        .unwrap()
        .id;
    let tasks = download_all(store, reply);
    (reply, tasks)
}

fn waive(path: &Path, label_like: &str) {
    rusqlite::Connection::open(path)
        .unwrap()
        .execute(
            "UPDATE pipeline_steps SET waived_at = 1, waived_by_note = 'prueba' WHERE label LIKE ?1",
            [label_like],
        )
        .unwrap();
}

#[test]
fn without_runs_the_order_is_plain_fifo() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = open(&dir);
    let ids: Vec<i64> = (0..4)
        .map(|_| store.enqueue("email", "{}").unwrap().id)
        .collect();
    assert_eq!(ready_ids(&store), ids);
    for id in &ids {
        assert_eq!(store.claim_next().unwrap().unwrap().id, *id);
    }
    assert!(store.claim_next().unwrap().is_none());
}

#[test]
fn a_blocked_head_does_not_stall_the_queue() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = open(&dir);
    // La respuesta (id menor) está bloqueada; la factura y la libre salen antes.
    let (reply, invoices) = run_with(&mut store, 1);
    let free = store.enqueue("email", "{}").unwrap().id;
    assert!(reply < invoices[0]);
    assert_eq!(ready_ids(&store), vec![invoices[0], free]);
    assert_eq!(store.claim_next().unwrap().unwrap().id, invoices[0]);
    assert_eq!(store.claim_next().unwrap().unwrap().id, free);
    assert!(store.claim_next().unwrap().is_none(), "la respuesta espera");
    assert_eq!(store.task(reply).unwrap().state, TaskState::Queued);
    assert_eq!(store.task(reply).unwrap().attempts, 0);
}

#[test]
fn only_completed_unblocks_and_waived_unblocks() {
    for (state, unblocks) in [
        (TaskState::Queued, false),
        (TaskState::Running, false),
        (TaskState::AwaitingReview, false),
        (TaskState::Failed, false),
        (TaskState::Cancelled, false),
        (TaskState::Completed, true),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let mut store = open(&dir);
        let (reply, invoices) = run_with(&mut store, 1);
        put_in(&mut store, invoices[0], state);
        assert_eq!(
            ready_ids(&store).contains(&reply),
            unblocks,
            "{state} -> {unblocks}"
        );
        assert_eq!(store.blocked_by(reply).unwrap().is_empty(), unblocks);
    }

    // Un paso omitido deja de retener, sea cual sea el estado de su tarea.
    let dir = tempfile::tempdir().unwrap();
    let mut store = open(&dir);
    let (reply, invoices) = run_with(&mut store, 1);
    put_in(&mut store, invoices[0], TaskState::Cancelled);
    assert!(!ready_ids(&store).contains(&reply));
    waive(&dir.path().join("t.db"), "invoice%");
    assert!(ready_ids(&store).contains(&reply));
    assert!(store.blocked_by(reply).unwrap().is_empty());
}

#[test]
fn a_step_without_task_yet_holds_back_later_stages() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = open(&dir);
    let reply = store
        .enqueue_with_run(
            "email",
            "{}",
            "t",
            1,
            "[]",
            &[
                step(0, "invoice", Some("m#0")),
                step(0, "invoice", Some("m#1")),
                subject(1, "reply"),
            ],
        )
        .unwrap()
        .id;
    let run = store.pipeline_run_of(reply).unwrap().unwrap();
    let steps = store.pipeline_steps(run).unwrap();
    let done = store
        .enqueue_for_step(steps[0].id, "invoice", "{}")
        .unwrap()
        .id;
    put_in(&mut store, done, TaskState::Completed);
    assert!(!ready_ids(&store).contains(&reply));
    let blockers = store.blocked_by(reply).unwrap();
    assert_eq!(blockers.len(), 1);
    assert_eq!((blockers[0].task_id, blockers[0].task_state), (None, None));

    // Descargado el adjunto, su tarea retiene hasta completarse.
    let late = store
        .enqueue_for_step(blockers[0].step_id, "invoice", "{}")
        .unwrap()
        .id;
    let blockers = store.blocked_by(reply).unwrap();
    assert_eq!(blockers[0].task_id, Some(late));
    assert_eq!(blockers[0].task_state, Some(TaskState::Queued));
    // Repetir la descarga devuelve la misma tarea y no encola otra.
    let total = store.all_tasks().unwrap().len();
    let again = store
        .enqueue_for_step(blockers[0].step_id, "invoice", "[1]")
        .unwrap();
    assert_eq!(again.id, late);
    assert_eq!(again.input_ref, "{}");
    assert_eq!(store.all_tasks().unwrap().len(), total);
    // Un paso inexistente es un error propio.
    assert!(matches!(
        store.enqueue_for_step(9999, "invoice", "{}"),
        Err(CoreError::PipelineStepNotFound(9999))
    ));
    put_in(&mut store, late, TaskState::Completed);
    assert!(ready_ids(&store).contains(&reply));
}

/// D68: una parte añadida a la misma etapa vuelve a retener la respuesta; añadirla en una
/// etapa que retendría una tarea ya tomada se rechaza.
#[test]
fn a_late_part_holds_the_reply_unless_it_was_already_taken() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = open(&dir);
    let (reply, invoices) = run_with(&mut store, 1);
    let run = store.pipeline_run_of(reply).unwrap().unwrap();
    put_in(&mut store, invoices[0], TaskState::Completed);
    assert!(ready_ids(&store).contains(&reply), "parte 1 completada");

    let part2 = store
        .add_step_with_task(run, &step(0, "invoice", Some("m#0/p2")), "invoice", "{}")
        .unwrap();
    assert_eq!(part2.state, TaskState::Queued);
    assert!(
        !ready_ids(&store).contains(&reply),
        "retenida por la parte 2"
    );
    assert_eq!(store.blocked_by(reply).unwrap()[0].task_id, Some(part2.id));
    put_in(&mut store, part2.id, TaskState::Completed);
    assert_eq!(store.claim_next().unwrap().unwrap().id, reply);

    // La respuesta ya está tomada: añadir una parte en la etapa 0 la retendría de nuevo.
    let total = store.all_tasks().unwrap().len();
    assert!(matches!(
        store.add_step_with_task(run, &step(0, "invoice", Some("m#0/p3")), "invoice", "{}"),
        Err(CoreError::PipelineStageTooLate(0))
    ));
    assert_eq!(store.all_tasks().unwrap().len(), total, "sin tarea ni paso");
    assert_eq!(store.pipeline_steps(run).unwrap().len(), 3);
    // Una ejecución inexistente es un error propio.
    assert!(matches!(
        store.add_step_with_task(999, &step(0, "invoice", Some("z")), "invoice", "{}"),
        Err(CoreError::PipelineRunNotFound(999))
    ));
}

/// Propiedad: una tarea en cola está en `ready_tasks()` si y solo si `blocked_by` está vacío.
#[test]
fn ready_iff_not_blocked_across_scenarios() {
    let states = [
        TaskState::Queued,
        TaskState::Running,
        TaskState::AwaitingReview,
        TaskState::Failed,
        TaskState::Cancelled,
        TaskState::Completed,
    ];
    for (n, first) in states.iter().enumerate() {
        for (m, second) in states.iter().enumerate() {
            for waive_second in [false, true] {
                for pending_attachment in [false, true] {
                    // Los escenarios con paso omitido necesitan archivo (se omite por SQL);
                    // los demás van en memoria, que es mucho más rápido.
                    if waive_second && !matches!(second, TaskState::Failed | TaskState::Cancelled) {
                        continue;
                    }
                    let dir = tempfile::tempdir().unwrap();
                    let mut store = if waive_second {
                        open(&dir)
                    } else {
                        Store::open_in_memory().unwrap()
                    };
                    let (_, invoices) = run_with(&mut store, 2);
                    // Segunda ejecución: etapa 0, otra etapa 1 y quizá un adjunto sin tarea.
                    let mut steps = vec![step(0, "invoice", Some("x#0")), subject(1, "reply")];
                    if pending_attachment {
                        steps.push(step(0, "invoice", Some("x#1")));
                    }
                    let second_subject = store
                        .enqueue_with_run("email", "{}", "t", 1, "[]", &steps)
                        .unwrap()
                        .id;
                    let run = store.pipeline_run_of(second_subject).unwrap().unwrap();
                    let x0 = store
                        .pipeline_steps(run)
                        .unwrap()
                        .into_iter()
                        .find(|s| s.attachment_key.as_deref() == Some("x#0"))
                        .unwrap();
                    let mid = store.enqueue_for_step(x0.id, "invoice", "{}").unwrap().id;
                    put_in(&mut store, invoices[0], *first);
                    put_in(&mut store, invoices[1], *second);
                    put_in(&mut store, mid, states[(n + m) % states.len()]);
                    if waive_second {
                        waive(&dir.path().join("t.db"), "invoice m#1");
                    }
                    let ready = ready_ids(&store);
                    for task in store.tasks_in(TaskState::Queued).unwrap() {
                        assert_eq!(
                            ready.contains(&task.id),
                            store.blocked_by(task.id).unwrap().is_empty(),
                            "tarea {} con {first}/{second} waived={waive_second} pendiente={pending_attachment}",
                            task.id
                        );
                    }
                    // La etapa 0 nunca está bloqueada.
                    assert!(store.blocked_by(invoices[0]).unwrap().is_empty());
                }
            }
        }
    }
}

#[test]
fn claim_rejects_a_blocked_task_without_spending_an_attempt() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = open(&dir);
    let (reply, invoices) = run_with(&mut store, 1);
    let before = store.transitions(reply).unwrap().len();
    assert!(matches!(
        store.claim(reply),
        Err(CoreError::TaskBlocked(id)) if id == reply
    ));
    let task = store.task(reply).unwrap();
    assert_eq!((task.state, task.attempts), (TaskState::Queued, 0));
    assert_eq!(store.transitions(reply).unwrap().len(), before);
    // Una tarea que no está en cola sigue dando el error de transición de siempre.
    put_in(&mut store, invoices[0], TaskState::Failed);
    assert!(matches!(
        store.claim(invoices[0]),
        Err(CoreError::InvalidTransition { .. })
    ));
    // Con el paso previo completado, se puede tomar.
    store.retry(invoices[0]).unwrap();
    put_in(&mut store, invoices[0], TaskState::Completed);
    assert_eq!(store.claim(reply).unwrap().attempts, 1);
}

#[test]
fn the_block_survives_a_restart_and_recover_does_not_skip_it() {
    let dir = tempfile::tempdir().unwrap();
    let (reply, invoice) = {
        let mut store = open(&dir);
        let (reply, invoices) = run_with(&mut store, 1);
        // Mitad de la ejecución: la factura en curso.
        store.claim(invoices[0]).unwrap();
        (reply, invoices[0])
    };
    let mut store = open(&dir);
    assert_eq!(store.recover().unwrap(), vec![invoice]);
    assert_eq!(store.task(invoice).unwrap().state, TaskState::Queued);
    assert_eq!(ready_ids(&store), vec![invoice]);
    assert_eq!(store.blocked_by(reply).unwrap().len(), 1);
    assert!(matches!(store.claim(reply), Err(CoreError::TaskBlocked(_))));
    // Se completa la factura y, tras otro reinicio, la respuesta sale.
    put_in(&mut store, invoice, TaskState::Completed);
    drop(store);
    let mut store = open(&dir);
    assert_eq!(store.claim_next().unwrap().unwrap().id, reply);
}

#[test]
fn a_failed_creation_leaves_no_task_run_or_step() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = open(&dir);
    let counts = |dir: &tempfile::TempDir| -> Vec<i64> {
        let conn = rusqlite::Connection::open(dir.path().join("t.db")).unwrap();
        [
            "tasks",
            "task_transitions",
            "pipeline_runs",
            "pipeline_steps",
        ]
        .iter()
        .map(|t| {
            conn.query_row(&format!("SELECT count(*) FROM {t}"), [], |r| r.get(0))
                .unwrap()
        })
        .collect()
    };
    // Mismo adjunto dos veces en la misma ejecución: falla al insertar el segundo paso.
    let duplicated = store.enqueue_with_run(
        "email",
        "{}",
        "t",
        1,
        "[]",
        &[
            step(0, "invoice", Some("m#0")),
            step(0, "invoice", Some("m#0")),
            subject(1, "reply"),
        ],
    );
    assert!(duplicated.is_err());
    assert_eq!(counts(&dir), vec![0, 0, 0, 0]);

    // Ningún paso (o más de uno) es la tarea sujeto: error propio, sin filas.
    for steps in [
        vec![step(0, "invoice", Some("m#0"))],
        vec![subject(0, "reply"), subject(1, "reply")],
        vec![],
    ] {
        assert!(matches!(
            store.enqueue_with_run("email", "{}", "t", 1, "[]", &steps),
            Err(CoreError::PipelineSubjectNotAStep)
        ));
    }
    assert_eq!(counts(&dir), vec![0, 0, 0, 0]);

    // Con pasos correctos, la tarea queda como en un enqueue normal.
    let task = store
        .enqueue_with_run(
            "email",
            "{}",
            "t",
            1,
            "[]",
            &[step(0, "invoice", Some("m#0")), subject(1, "reply")],
        )
        .unwrap();
    assert_eq!((task.state, task.attempts), (TaskState::Queued, 0));
    assert_eq!(
        store.transitions(task.id).unwrap(),
        vec![(None, "queued".to_owned(), "creada".to_owned())]
    );
    assert_eq!(counts(&dir), vec![1, 1, 1, 2]);
}

// ---- Fase 3: omitir un paso (D150) y plantillas versionadas (D149) ----

#[test]
fn a_part_task_has_its_own_kind_not_the_step_kind() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = open(&dir);
    let (reply, invoices) = run_with(&mut store, 1);
    let run = store.pipeline_run_of(reply).unwrap().unwrap();
    let part = store
        .add_step_with_task(run, &step(0, "attachment_invoices", None), "invoice", "{}")
        .unwrap();
    assert_eq!(part.kind, "invoice");
    let linked = store.pipeline_step_of_task(part.id).unwrap().unwrap();
    assert_eq!(
        (linked.kind.as_str(), linked.run_id),
        ("attachment_invoices", run)
    );
    assert!(store.pipeline_step_of_task(invoices[0]).unwrap().is_some());
    assert!(store.pipeline_step_of_task(9999).unwrap().is_none());
}

#[test]
fn waiving_needs_a_stuck_step_that_holds_something_and_leaves_an_audit_event_without_content() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = open(&dir);
    let (reply, invoices) = run_with(&mut store, 2);
    let run = store.pipeline_run_of(reply).unwrap().unwrap();
    let steps = store.pipeline_steps(run).unwrap();
    let step_of = |task: i64| steps.iter().find(|s| s.task_id == Some(task)).unwrap().id;
    // Una factura que avanza (en cola) o en revisión no se omite; la respuesta tampoco.
    assert!(matches!(
        store.waive_pipeline_step(step_of(invoices[0]), None),
        Err(CoreError::PipelineStepNotRetaining(_))
    ));
    put_in(&mut store, invoices[0], TaskState::AwaitingReview);
    assert!(store
        .waive_pipeline_step(step_of(invoices[0]), None)
        .is_err());
    assert!(store.waive_pipeline_step(step_of(reply), None).is_err());
    assert!(matches!(
        store.waive_pipeline_step(9999, None),
        Err(CoreError::PipelineStepNotFound(9999))
    ));
    // Cancelada: se puede omitir, con nota; la segunda factura sigue reteniendo.
    store.cancel(invoices[0]).unwrap();
    store
        .waive_pipeline_step(step_of(invoices[0]), Some("nota privada"))
        .unwrap();
    assert!(store.has_waived_prerequisite(reply).unwrap());
    assert!(!store.has_waived_prerequisite(invoices[1]).unwrap());
    assert!(!ready_ids(&store).contains(&reply));
    // Repetirlo es idempotente: `false`, sin segundo evento.
    assert!(!store
        .waive_pipeline_step(step_of(invoices[0]), None)
        .unwrap());
    // Fallida: también. Con las dos omitidas, la respuesta queda lista.
    put_in(&mut store, invoices[1], TaskState::Failed);
    assert!(store
        .waive_pipeline_step(step_of(invoices[1]), None)
        .unwrap());
    assert!(ready_ids(&store).contains(&reply));
    let events: Vec<_> = store
        .audit_log()
        .unwrap()
        .into_iter()
        .filter(|e| e.action == "pipeline_step")
        .collect();
    assert_eq!(events.len(), 2);
    for event in &events {
        assert_eq!(
            (event.task_id, event.status.as_str()),
            (Some(reply), "waived")
        );
        let detail = event.detail.clone().unwrap();
        assert!(!detail.contains("nota privada"), "{detail}");
    }
}

#[test]
fn a_waive_is_rejected_when_nothing_queued_depends_on_the_step() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = open(&dir);
    let (reply, invoices) = run_with(&mut store, 1);
    let run = store.pipeline_run_of(reply).unwrap().unwrap();
    let step_id = store.pipeline_steps(run).unwrap()[0].id;
    store.cancel(invoices[0]).unwrap();
    // La respuesta ya se canceló: omitir no libera nada.
    store.cancel(reply).unwrap();
    assert!(matches!(
        store.waive_pipeline_step(step_id, None),
        Err(CoreError::PipelineStepNotRetaining(_))
    ));
    assert_eq!(
        store
            .pipeline_steps(run)
            .unwrap()
            .iter()
            .filter(|s| s.waived_at.is_some())
            .count(),
        0
    );
}

#[test]
fn a_pending_attachment_keeps_a_short_error_and_a_link_clears_it() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = open(&dir);
    let task = store
        .enqueue_with_run(
            "email",
            "{}",
            "t",
            1,
            "[]",
            &[
                step(1, "attachment_invoices", Some("m#a#1")),
                subject(2, "reply"),
            ],
        )
        .unwrap();
    let pending = store.pending_attachment_steps(task.id).unwrap();
    assert_eq!(pending.len(), 1);
    store
        .record_step_error(pending[0].id, "unavailable")
        .unwrap();
    let again = store.pending_attachment_steps(task.id).unwrap();
    assert_eq!(again[0].last_error.as_deref(), Some("unavailable"));
    let invoice = store
        .enqueue_for_step(pending[0].id, "invoice", "{}")
        .unwrap();
    assert!(store.pending_attachment_steps(task.id).unwrap().is_empty());
    // Con tarea, el código ya no cambia, y repetir el enlace devuelve la misma tarea.
    store.record_step_error(pending[0].id, "otro").unwrap();
    let run = store.pipeline_run_of(task.id).unwrap().unwrap();
    assert_eq!(store.pipeline_steps(run).unwrap()[0].last_error, None);
    assert_eq!(
        store
            .enqueue_for_step(pending[0].id, "invoice", "{}")
            .unwrap()
            .id,
        invoice.id
    );
    assert_eq!(store.pipeline_runs(10).unwrap().len(), 1);
    assert_eq!(
        store.pipeline_run(run).unwrap().unwrap().subject_task_id,
        task.id
    );
}

#[test]
fn templates_are_versioned_archived_without_deleting_and_keep_their_state() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = open(&dir);
    let steps = json!([{ "kind": "attachment_invoices" }, { "kind": "reply" }]);
    let v1 = store
        .save_task_template("t", "Uno", &json!({}), &steps, false)
        .unwrap();
    assert_eq!((v1.version, v1.active, v1.archived), (1, false, false));
    let v2 = store.set_task_template_active("t", true).unwrap();
    assert_eq!((v2.version, v2.active), (2, true));
    // Activar lo que ya está activo no crea versión; editar conserva el estado.
    assert_eq!(
        store.set_task_template_active("t", true).unwrap().version,
        2
    );
    let v3 = store
        .save_task_template(
            "t",
            "Dos",
            &json!({ "sender_domain": "a.test" }),
            &steps,
            false,
        )
        .unwrap();
    assert_eq!((v3.version, v3.active, v3.name.as_str()), (3, true, "Dos"));
    let v4 = store.set_task_template_archived("t", true).unwrap();
    assert_eq!((v4.version, v4.active, v4.archived), (4, false, true));
    assert!(matches!(
        store.save_task_template("t", "x", &json!({}), &steps, false),
        Err(CoreError::TemplateArchived(_))
    ));
    assert!(matches!(
        store.set_task_template_active("t", true),
        Err(CoreError::TemplateArchived(_))
    ));
    assert!(matches!(
        store.set_task_template_active("nada", true),
        Err(CoreError::TemplateNotFound(_))
    ));
    let history = store.task_template_history("t").unwrap();
    assert_eq!(
        history.iter().map(|t| t.version).collect::<Vec<_>>(),
        [1, 2, 3, 4]
    );
    assert_eq!(history[0].name, "Uno");
    assert_eq!(store.task_templates().unwrap().len(), 1);
    assert_eq!(store.task_templates().unwrap()[0].version, 4);
    // Las ejecuciones guardan su copia de los pasos: editar la plantilla no la toca.
    let task = store
        .enqueue_with_run(
            "email",
            "{}",
            "t",
            2,
            &steps.to_string(),
            &[
                step(1, "attachment_invoices", Some("k")),
                subject(2, "reply"),
            ],
        )
        .unwrap();
    store.set_task_template_archived("t", false).unwrap();
    let run = store.pipeline_run_of(task.id).unwrap().unwrap();
    let saved = store.pipeline_run(run).unwrap().unwrap();
    assert_eq!((saved.template_version, saved.steps), (2, steps));
    assert!(store
        .audit_log()
        .unwrap()
        .iter()
        .any(|e| e.action == "task_template" && e.detail.as_deref() == Some("t v4")));
}

#[test]
fn a_rejected_attachment_is_not_pending_again() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = open(&dir);
    let task = store
        .enqueue_with_run(
            "email",
            "{}",
            "t",
            1,
            "[]",
            &[
                step(1, "attachment_invoices", Some("k#1")),
                step(1, "attachment_invoices", Some("k#2")),
                subject(2, "reply"),
            ],
        )
        .unwrap();
    let pending = store.pending_attachment_steps(task.id).unwrap();
    assert_eq!(pending.len(), 2);
    store
        .record_step_error(pending[0].id, nestadmin_core::ATTACHMENT_REJECTED)
        .unwrap();
    let still = store.pending_attachment_steps(task.id).unwrap();
    assert_eq!(
        still.iter().map(|s| s.id).collect::<Vec<_>>(),
        [pending[1].id]
    );
    assert_eq!(store.rejected_attachment_keys(task.id).unwrap(), ["k#1"]);
}

#[test]
fn the_overview_queries_keep_queued_runs_and_batch_the_steps() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = open(&dir);
    let (first, _) = run_with(&mut store, 1);
    for _ in 0..3 {
        let (reply, invoices) = run_with(&mut store, 1);
        put_in(&mut store, invoices[0], TaskState::Completed);
        put_in(&mut store, reply, TaskState::Completed);
    }
    let recent = store.pipeline_runs_for_overview(2).unwrap();
    let subjects: Vec<i64> = recent.iter().map(|r| r.subject_task_id).collect();
    assert!(subjects.contains(&first), "la ejecución en cola va siempre");
    assert_eq!(recent.len(), 3);
    let ids: Vec<i64> = recent.iter().map(|r| r.id).collect();
    let steps = store.pipeline_steps_with_state(&ids).unwrap();
    assert_eq!(steps.len(), 6);
    assert!(steps
        .iter()
        .any(|(s, state)| s.kind == "reply" && *state == Some(TaskState::Queued)));
    assert!(store.pipeline_steps_with_state(&[]).unwrap().is_empty());
}

#[test]
fn a_late_link_to_a_waived_step_leaves_an_audit_event() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = open(&dir);
    let reply = store
        .enqueue_with_run(
            "email",
            "{}",
            "t",
            1,
            "[]",
            &[
                step(1, "attachment_invoices", Some("k#1")),
                subject(2, "reply"),
            ],
        )
        .unwrap();
    let run = store.pipeline_run_of(reply.id).unwrap().unwrap();
    let step_id = store.pipeline_steps(run).unwrap()[0].id;
    assert!(store.waive_pipeline_step(step_id, None).unwrap());
    let late = store.enqueue_for_step(step_id, "invoice", "{}").unwrap();
    let events: Vec<_> = store
        .audit_log()
        .unwrap()
        .into_iter()
        .filter(|e| e.status == "linked_after_waive")
        .collect();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].task_id, Some(reply.id));
    assert!(events[0]
        .detail
        .as_deref()
        .unwrap()
        .contains(&late.id.to_string()));
    // Repetir el enlace no añade otro evento ni otra tarea.
    assert_eq!(
        store.enqueue_for_step(step_id, "invoice", "{}").unwrap().id,
        late.id
    );
    assert_eq!(
        store
            .audit_log()
            .unwrap()
            .iter()
            .filter(|e| e.status == "linked_after_waive")
            .count(),
        1
    );
}
