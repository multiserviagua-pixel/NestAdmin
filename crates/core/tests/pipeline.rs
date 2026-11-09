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
        .add_step_with_task(run, &step(0, "invoice", Some("m#0/p2")), "{}")
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
        store.add_step_with_task(run, &step(0, "invoice", Some("m#0/p3")), "{}"),
        Err(CoreError::PipelineStageTooLate(0))
    ));
    assert_eq!(store.all_tasks().unwrap().len(), total, "sin tarea ni paso");
    assert_eq!(store.pipeline_steps(run).unwrap().len(), 3);
    // Una ejecución inexistente es un error propio.
    assert!(matches!(
        store.add_step_with_task(999, &step(0, "invoice", Some("z")), "{}"),
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
