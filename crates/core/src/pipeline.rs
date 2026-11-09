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

/// Fila de `pipeline_runs`. `steps` es la copia de los pasos de la plantilla al abrirse: editar
/// la plantilla después no la cambia.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PipelineRun {
    pub id: i64,
    pub template_id: String,
    pub template_version: i64,
    pub steps: serde_json::Value,
    pub subject_task_id: i64,
    pub created_at: i64,
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
        let linked: Option<(Option<i64>, Option<i64>)> = tx
            .query_row(
                "SELECT task_id, waived_at FROM pipeline_steps WHERE id = ?1",
                [step_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let task = match linked {
            None => return Err(CoreError::PipelineStepNotFound(step_id)),
            Some((Some(existing), _)) => load(&tx, existing)?,
            Some((None, waived_at)) => {
                let task = enqueue_in(&tx, kind, input_ref)?;
                tx.execute(
                    "UPDATE pipeline_steps SET task_id = ?2, last_error = NULL WHERE id = ?1",
                    params![step_id, task.id],
                )?;
                if waived_at.is_some() {
                    // El adjunto llegó después de «Continuar sin ese paso» (D150): se importa para
                    // no perder la factura y consta en el historial, solo con ids.
                    let run: i64 = tx.query_row(
                        "SELECT s.run_id FROM pipeline_steps s WHERE s.id = ?1",
                        [step_id],
                        |r| r.get(0),
                    )?;
                    let subject: i64 = tx.query_row(
                        "SELECT subject_task_id FROM pipeline_runs WHERE id = ?1",
                        [run],
                        |r| r.get(0),
                    )?;
                    let detail = serde_json::json!({
                        "step_id": step_id, "run_id": run, "step_task_id": task.id,
                    });
                    tx.execute(
                        "INSERT INTO audit_log (at, task_id, action, status, authorization, detail) VALUES (?1, ?2, 'pipeline_step', 'linked_after_waive', NULL, ?3)",
                        params![now_ms(), subject, detail.to_string()],
                    )?;
                }
                task
            }
        };
        tx.commit()?;
        Ok(task)
    }

    /// Añade a una ejecución un paso y su tarea, de tipo `task_kind` (distinto del tipo del paso),
    /// en una transacción (las partes de una factura
    /// separada, D68). Se rechaza si, en una etapa posterior a `stage`, ya hay una tarea tomada
    /// (que no esté en cola o con intentos): el paso nuevo la volvería a retener.
    pub fn add_step_with_task(
        &mut self,
        run_id: i64,
        step: &NewStep,
        task_kind: &str,
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
        let task = enqueue_in(&tx, task_kind, input_ref)?;
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

    /// Ejecuciones, de la más reciente a la más antigua, hasta `limit`.
    pub fn pipeline_runs(&self, limit: usize) -> Result<Vec<PipelineRun>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, template_id, template_version, steps_json, subject_task_id, created_at
             FROM pipeline_runs ORDER BY id DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map([limit as i64], run_from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn pipeline_run(&self, run_id: i64) -> Result<Option<PipelineRun>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id, template_id, template_version, steps_json, subject_task_id, created_at
                 FROM pipeline_runs WHERE id = ?1",
                [run_id],
                run_from_row,
            )
            .optional()?)
    }

    pub fn pipeline_step(&self, step_id: i64) -> Result<Option<PipelineStep>> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {STEP_COLUMNS} FROM pipeline_steps WHERE id = ?1"),
                [step_id],
                step_from_row,
            )
            .optional()?)
    }

    /// Paso cuya tarea es esta, si la tarea pertenece a una ejecución.
    pub fn pipeline_step_of_task(&self, task_id: i64) -> Result<Option<PipelineStep>> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {STEP_COLUMNS} FROM pipeline_steps WHERE task_id = ?1"),
                [task_id],
                step_from_row,
            )
            .optional()?)
    }

    /// Pasos de adjunto sin tarea todavía (por descargar) de la ejecución cuyo correo es esta
    /// tarea. Vacío si el correo no tiene ejecución. Un adjunto que nunca podrá importarse
    /// (`ATTACHMENT_REJECTED`) no se vuelve a descargar: no está aquí, sino en
    /// `rejected_attachment_keys`.
    pub fn pending_attachment_steps(&self, subject_task_id: i64) -> Result<Vec<PipelineStep>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {STEP_COLUMNS} FROM pipeline_steps
             WHERE task_id IS NULL AND attachment_key IS NOT NULL
               AND COALESCE(last_error, '') <> '{ATTACHMENT_REJECTED}'
               AND run_id IN (SELECT id FROM pipeline_runs WHERE subject_task_id = ?1)
             ORDER BY id"
        ))?;
        let rows = stmt.query_map([subject_task_id], step_from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Claves de los adjuntos de esta ejecución ya rechazados de forma definitiva.
    pub fn rejected_attachment_keys(&self, subject_task_id: i64) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT attachment_key FROM pipeline_steps
             WHERE task_id IS NULL AND attachment_key IS NOT NULL AND last_error = '{ATTACHMENT_REJECTED}'
               AND run_id IN (SELECT id FROM pipeline_runs WHERE subject_task_id = ?1)"
        ))?;
        let rows = stmt.query_map([subject_task_id], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Ejecuciones para la vista: las `limit` más recientes y, siempre, las cuyo correo sigue en
    /// cola (retenido o esperando), aunque sean más antiguas.
    pub fn pipeline_runs_for_overview(&self, limit: usize) -> Result<Vec<PipelineRun>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, template_id, template_version, steps_json, subject_task_id, created_at
             FROM pipeline_runs
             WHERE id IN (SELECT id FROM pipeline_runs ORDER BY id DESC LIMIT ?1)
                OR subject_task_id IN (SELECT id FROM tasks WHERE state = 'queued')
             ORDER BY id DESC",
        )?;
        let rows = stmt.query_map([limit as i64], run_from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Pasos de varias ejecuciones con el estado de su tarea, en una sola consulta.
    pub fn pipeline_steps_with_state(
        &self,
        run_ids: &[i64],
    ) -> Result<Vec<(PipelineStep, Option<TaskState>)>> {
        if run_ids.is_empty() {
            return Ok(Vec::new());
        }
        let ids: Vec<String> = run_ids.iter().map(i64::to_string).collect();
        let mut stmt = self.conn.prepare(&format!(
            "SELECT s.id, s.run_id, s.stage, s.kind, s.label, s.attachment_key, s.task_id,
                    s.waived_at, s.waived_by_note, s.last_error, t.state
             FROM pipeline_steps s LEFT JOIN tasks t ON t.id = s.task_id
             WHERE s.run_id IN ({}) ORDER BY s.run_id, s.stage, s.id",
            ids.join(",")
        ))?;
        let rows = stmt.query_map([], |r| {
            let state: Option<String> = r.get(10)?;
            Ok((step_from_row(r)?, state.and_then(|s| s.parse().ok())))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Anota el código corto del último fallo de un paso sin tarea (nunca contenido). Un paso
    /// que ya tiene tarea no cambia.
    pub fn record_step_error(&mut self, step_id: i64, code: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE pipeline_steps SET last_error = ?2 WHERE id = ?1 AND task_id IS NULL",
            params![step_id, code],
        )?;
        Ok(())
    }

    /// ¿Se omitió algún paso de etapas anteriores a esta tarea? Entonces su resultado no puede
    /// automatizarse aunque coincida una regla (D150).
    pub fn has_waived_prerequisite(&self, task_id: i64) -> Result<bool> {
        Ok(self.conn.query_row(
            "SELECT EXISTS (
                SELECT 1 FROM pipeline_steps me
                JOIN pipeline_steps prev ON prev.run_id = me.run_id AND prev.stage < me.stage
                WHERE me.task_id = ?1 AND prev.waived_at IS NOT NULL)",
            [task_id],
            |r| r.get(0),
        )?)
    }

    /// «Continuar sin ese paso» (D150): omite un paso que retiene tareas y no puede avanzar
    /// (su tarea falló, se canceló o aún no existe porque falta el adjunto), con una nota
    /// opcional de la persona y un evento de auditoría sin contenido, todo en una transacción.
    /// Se rechaza si el paso no está en ese caso o ninguna tarea en cola depende de él. Repetirlo
    /// sobre un paso ya omitido es idempotente: devuelve `false`, sin segundo evento. Devuelve
    /// `true` si lo omitió ahora. No cambia el estado de ninguna tarea: solo deja de retener.
    pub fn waive_pipeline_step(&mut self, step_id: i64, note: Option<&str>) -> Result<bool> {
        let tx = self.conn.transaction()?;
        let step: Option<WaiveCandidate> = tx
            .query_row(
                "SELECT s.run_id, s.stage, s.kind, s.task_id, t.state, s.waived_at
                 FROM pipeline_steps s LEFT JOIN tasks t ON t.id = s.task_id
                 WHERE s.id = ?1",
                [step_id],
                |r| {
                    Ok(WaiveCandidate {
                        run_id: r.get(0)?,
                        stage: r.get(1)?,
                        kind: r.get(2)?,
                        task_id: r.get(3)?,
                        state: r.get(4)?,
                        waived_at: r.get(5)?,
                    })
                },
            )
            .optional()?;
        let Some(WaiveCandidate {
            run_id,
            stage,
            kind,
            task_id,
            state,
            waived_at,
        }) = step
        else {
            return Err(CoreError::PipelineStepNotFound(step_id));
        };
        let stuck = matches!(state.as_deref(), None | Some("failed") | Some("cancelled"));
        let retains: bool = tx.query_row(
            "SELECT EXISTS (
                SELECT 1 FROM pipeline_steps later JOIN tasks lt ON lt.id = later.task_id
                WHERE later.run_id = ?1 AND later.stage > ?2 AND lt.state = 'queued')",
            params![run_id, stage],
            |r| r.get(0),
        )?;
        if waived_at.is_some() {
            return Ok(false);
        }
        if !stuck || !retains {
            return Err(CoreError::PipelineStepNotRetaining(step_id));
        }
        let now = now_ms();
        tx.execute(
            "UPDATE pipeline_steps SET waived_at = ?2, waived_by_note = ?3 WHERE id = ?1",
            params![step_id, now, note],
        )?;
        let subject: i64 = tx.query_row(
            "SELECT subject_task_id FROM pipeline_runs WHERE id = ?1",
            [run_id],
            |r| r.get(0),
        )?;
        // Solo ids, etapa, tipo y estado de la tarea del paso: ni nombres de archivo ni la nota.
        let detail = serde_json::json!({
            "step_id": step_id,
            "run_id": run_id,
            "stage": stage,
            "kind": kind,
            "step_task_id": task_id,
            "step_task_state": state,
        });
        tx.execute(
            "INSERT INTO audit_log (at, task_id, action, status, authorization, detail) VALUES (?1, ?2, 'pipeline_step', 'waived', NULL, ?3)",
            params![now, subject, detail.to_string()],
        )?;
        tx.commit()?;
        Ok(true)
    }

    /// Pasos de una ejecución, por etapa y orden de creación.
    pub fn pipeline_steps(&self, run_id: i64) -> Result<Vec<PipelineStep>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {STEP_COLUMNS} FROM pipeline_steps WHERE run_id = ?1 ORDER BY stage, id"
        ))?;
        let rows = stmt.query_map([run_id], step_from_row)?;
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

/// Código de `last_error` de un adjunto que nunca podrá importarse (tamaño conocido sobre el
/// máximo, vacío o rechazado al comprobarlo): no se vuelve a descargar y solo se sale de él
/// con «Continuar sin ese paso» (D150).
pub const ATTACHMENT_REJECTED: &str = "attachment_rejected";

/// Paso que se quiere omitir, con el estado de su tarea.
struct WaiveCandidate {
    run_id: i64,
    stage: i64,
    kind: String,
    task_id: Option<i64>,
    state: Option<String>,
    waived_at: Option<i64>,
}

const STEP_COLUMNS: &str =
    "id, run_id, stage, kind, label, attachment_key, task_id, waived_at, waived_by_note, last_error";

fn step_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<PipelineStep> {
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
}

fn run_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<PipelineRun> {
    let steps: String = r.get(3)?;
    Ok(PipelineRun {
        id: r.get(0)?,
        template_id: r.get(1)?,
        template_version: r.get(2)?,
        steps: serde_json::from_str(&steps).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(3, rusqlite::types::Type::Text, e.into())
        })?,
        subject_task_id: r.get(4)?,
        created_at: r.get(5)?,
    })
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
