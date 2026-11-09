//! Ejecuciones de pipeline (D148): datos y consultas. Una ejecución agrupa los pasos de una
//! plantilla en etapas lineales; este módulo no autoriza ni ejecuta nada y no conoce reglas,
//! acciones ni autorización: solo dice qué pasos retienen a una tarea.

use rusqlite::{params, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};
use crate::queue::{enqueue_in, load, Task, TaskState};
use crate::store::{now_ms, Store};

/// Un paso por crear al abrir una ejecución. `attachment_key` evita duplicar un adjunto
/// dentro de la misma ejecución. Los pasos nacen sin tarea (un adjunto por descargar) salvo el
/// que lleva `subject`, cuya tarea es la tarea sujeto que se crea con la ejecución. Nunca se
/// enlaza una tarea ya existente: una plantilla no es retroactiva (D148).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewStep {
    pub stage: i64,
    pub kind: String,
    pub label: String,
    pub attachment_key: Option<String>,
    pub subject: bool,
}

/// Fila de `pipeline_steps`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PipelineStep {
    pub id: i64,
    pub run_id: i64,
    pub stage: i64,
    pub kind: String,
    pub label: String,
    pub attachment_key: Option<String>,
    pub task_id: Option<i64>,
    pub waived_at: Option<i64>,
    pub waived_by_note: Option<String>,
    pub last_error: Option<String>,
}

/// Paso que retiene a una tarea: su tarea y el estado de esta (`None` si aún no existe).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockingStep {
    pub step_id: i64,
    pub stage: i64,
    pub kind: String,
    pub label: String,
    pub task_id: Option<i64>,
    pub task_state: Option<TaskState>,
}

impl Store {
    /// Crea en una sola transacción la tarea sujeto (como un `enqueue` normal), la ejecución y
    /// sus pasos, y devuelve la tarea. Exactamente un paso debe llevar `subject`; si no, o si
    /// algo falla (por ejemplo, un adjunto repetido), no queda ninguna fila.
    pub fn enqueue_with_run(
        &mut self,
        kind: &str,
        input_ref: &str,
        template_id: &str,
        template_version: i64,
        steps_json: &str,
        steps: &[NewStep],
    ) -> Result<Task> {
        if steps.iter().filter(|s| s.subject).count() != 1 {
            return Err(CoreError::PipelineSubjectNotAStep);
        }
        let tx = self.conn.transaction()?;
        let task = enqueue_in(&tx, kind, input_ref)?;
        tx.execute(
            "INSERT INTO pipeline_runs (template_id, template_version, steps_json, subject_task_id, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![template_id, template_version, steps_json, task.id, now_ms()],
        )?;
        let run_id = tx.last_insert_rowid();
        for step in steps {
            insert_step(&tx, run_id, step, step.subject.then_some(task.id))?;
        }
        tx.commit()?;
        Ok(task)
    }

    /// Crea en una sola transacción la tarea de un paso y su vínculo. Si el paso ya tiene
    /// tarea, devuelve esa tarea sin encolar otra, de modo que repetir una descarga no duplica.
    pub fn enqueue_for_step(&mut self, step_id: i64, kind: &str, input_ref: &str) -> Result<Task> {
        let tx = self.conn.transaction()?;
        let linked: Option<Option<i64>> = tx
            .query_row(
                "SELECT task_id FROM pipeline_steps WHERE id = ?1",
                [step_id],
                |r| r.get(0),
            )
            .optional()?;
        let task = match linked {
            None => return Err(CoreError::PipelineStepNotFound(step_id)),
            Some(Some(existing)) => load(&tx, existing)?,
            Some(None) => {
                let task = enqueue_in(&tx, kind, input_ref)?;
                tx.execute(
                    "UPDATE pipeline_steps SET task_id = ?2, last_error = NULL WHERE id = ?1",
                    params![step_id, task.id],
                )?;
                task
            }
        };
        tx.commit()?;
        Ok(task)
    }

    /// Añade a una ejecución un paso y su tarea en una transacción (las partes de una factura
    /// separada, D68). Se rechaza si, en una etapa posterior a `stage`, ya hay una tarea tomada
    /// (que no esté en cola o con intentos): el paso nuevo la volvería a retener.
    pub fn add_step_with_task(
        &mut self,
        run_id: i64,
        step: &NewStep,
        input_ref: &str,
    ) -> Result<Task> {
        let tx = self.conn.transaction()?;
        let exists: Option<i64> = tx
            .query_row(
                "SELECT id FROM pipeline_runs WHERE id = ?1",
                [run_id],
                |r| r.get(0),
            )
            .optional()?;
        if exists.is_none() {
            return Err(CoreError::PipelineRunNotFound(run_id));
        }
        let taken: i64 = tx.query_row(
            "SELECT count(*) FROM pipeline_steps s JOIN tasks t ON t.id = s.task_id
             WHERE s.run_id = ?1 AND s.stage > ?2 AND (t.state <> 'queued' OR t.attempts > 0)",
            params![run_id, step.stage],
            |r| r.get(0),
        )?;
        if taken > 0 {
            return Err(CoreError::PipelineStageTooLate(step.stage));
        }
        let task = enqueue_in(&tx, &step.kind, input_ref)?;
        insert_step(
            &tx,
            run_id,
            &NewStep {
                subject: false,
                ..step.clone()
            },
            Some(task.id),
        )?;
        tx.commit()?;
        Ok(task)
    }

    /// Ejecución cuya tarea sujeto es esta, si la hay.
    pub fn pipeline_run_of(&self, subject_task_id: i64) -> Result<Option<i64>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id FROM pipeline_runs WHERE subject_task_id = ?1",
                [subject_task_id],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// Pasos de una ejecución, por etapa y orden de creación.
    pub fn pipeline_steps(&self, run_id: i64) -> Result<Vec<PipelineStep>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, run_id, stage, kind, label, attachment_key, task_id, waived_at, waived_by_note, last_error
             FROM pipeline_steps WHERE run_id = ?1 ORDER BY stage, id",
        )?;
        let rows = stmt.query_map([run_id], |r| {
            Ok(PipelineStep {
                id: r.get(0)?,
                run_id: r.get(1)?,
                stage: r.get(2)?,
                kind: r.get(3)?,
                label: r.get(4)?,
                attachment_key: r.get(5)?,
                task_id: r.get(6)?,
                waived_at: r.get(7)?,
                waived_by_note: r.get(8)?,
                last_error: r.get(9)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Pasos de etapas anteriores que retienen a la tarea: no omitidos y cuya tarea no está
    /// `completed` (o no existe aún). Vacío si la tarea no es de ningún pipeline o está lista.
    /// Es la misma condición que `READY_SQL`, ahora con el detalle.
    pub fn blocked_by(&self, task_id: i64) -> Result<Vec<BlockingStep>> {
        let mut stmt = self.conn.prepare(
            "SELECT prev.id, prev.stage, prev.kind, prev.label, prev.task_id, prev_task.state
             FROM pipeline_steps me
             JOIN pipeline_steps prev ON prev.run_id = me.run_id AND prev.stage < me.stage
             LEFT JOIN tasks prev_task ON prev_task.id = prev.task_id
             WHERE me.task_id = ?1 AND prev.waived_at IS NULL
               AND (prev_task.id IS NULL OR prev_task.state <> 'completed')
             ORDER BY prev.stage, prev.id",
        )?;
        let rows = stmt.query_map([task_id], |r| {
            let state: Option<String> = r.get(5)?;
            Ok(BlockingStep {
                step_id: r.get(0)?,
                stage: r.get(1)?,
                kind: r.get(2)?,
                label: r.get(3)?,
                task_id: r.get(4)?,
                task_state: state.and_then(|s| s.parse().ok()),
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
}

fn insert_step(
    tx: &Transaction<'_>,
    run_id: i64,
    step: &NewStep,
    task_id: Option<i64>,
) -> Result<()> {
    tx.execute(
        "INSERT INTO pipeline_steps (run_id, stage, kind, label, attachment_key, task_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            run_id,
            step.stage,
            step.kind,
            step.label,
            step.attachment_key,
            task_id
        ],
    )?;
    Ok(())
}
