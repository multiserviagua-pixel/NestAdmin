use std::fmt;
use std::str::FromStr;

use rusqlite::{params, OptionalExtension, Row, Transaction};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::error::{CoreError, Result};
use crate::store::{now_ms, Store};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    Queued,
    Running,
    AwaitingReview,
    Completed,
    Failed,
    Cancelled,
}

impl TaskState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::AwaitingReview => "awaiting_review",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn is_final(self) -> bool {
        matches!(self, Self::Completed | Self::Cancelled)
    }

    /// Transiciones del contrato de cola (contratos.md).
    pub fn can_move_to(self, to: Self) -> bool {
        use TaskState::*;
        matches!(
            (self, to),
            (Queued, Running)
                | (Queued, Cancelled)
                | (Running, Completed)
                | (Running, AwaitingReview)
                | (Running, Failed)
                | (Running, Queued)
                | (Running, Cancelled)
                | (AwaitingReview, Completed)
                | (AwaitingReview, Failed)
                | (AwaitingReview, Cancelled)
                | (Failed, Queued)
                | (Failed, Cancelled)
        )
    }
}

impl fmt::Display for TaskState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for TaskState {
    type Err = String;
    fn from_str(s: &str) -> std::result::Result<Self, String> {
        Ok(match s {
            "queued" => Self::Queued,
            "running" => Self::Running,
            "awaiting_review" => Self::AwaitingReview,
            "completed" => Self::Completed,
            "failed" => Self::Failed,
            "cancelled" => Self::Cancelled,
            other => return Err(format!("estado desconocido: {other}")),
        })
    }
}

/// Error estructurado de una tarea, sin secretos.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskError {
    pub code: String,
    pub message: String,
    pub recoverable: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Task {
    pub id: i64,
    pub kind: String,
    pub input_ref: String,
    pub state: TaskState,
    pub attempts: i64,
    pub result: Option<Value>,
    pub error: Option<TaskError>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Predicado único de «lista para ejecutarse» (D148), sobre una fila de `tasks` en cola: no
/// existe en su ejecución de pipeline ningún paso de una etapa anterior, sin omitir, cuya
/// tarea no esté `completed` (un paso sin tarea todavía, `task_id` NULL, también retiene).
/// Solo `completed` desbloquea. Una tarea sin ejecución de pipeline está siempre lista.
pub(crate) const READY_SQL: &str = "NOT EXISTS (
    SELECT 1 FROM pipeline_steps me
    JOIN pipeline_steps prev ON prev.run_id = me.run_id AND prev.stage < me.stage
    LEFT JOIN tasks prev_task ON prev_task.id = prev.task_id
    WHERE me.task_id = tasks.id AND prev.waived_at IS NULL
      AND (prev_task.id IS NULL OR prev_task.state <> 'completed')
)";

const TASK_COLUMNS: &str =
    "id, kind, input_ref, state, attempts, result, error, created_at, updated_at";

fn task_from_row(row: &Row<'_>) -> rusqlite::Result<Task> {
    let parse_json = |index: usize, text: Option<String>| -> rusqlite::Result<Option<Value>> {
        text.map(|t| serde_json::from_str(&t))
            .transpose()
            .map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    index,
                    rusqlite::types::Type::Text,
                    e.into(),
                )
            })
    };
    let state: String = row.get(3)?;
    let error = parse_json(6, row.get(6)?)?
        .map(serde_json::from_value)
        .transpose()
        .map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(6, rusqlite::types::Type::Text, e.into())
        })?;
    Ok(Task {
        id: row.get(0)?,
        kind: row.get(1)?,
        input_ref: row.get(2)?,
        state: state.parse().map_err(|e: String| {
            rusqlite::Error::FromSqlConversionFailure(3, rusqlite::types::Type::Text, e.into())
        })?,
        attempts: row.get(4)?,
        result: parse_json(5, row.get(5)?)?,
        error,
        created_at: row.get(7)?,
        updated_at: row.get(8)?,
    })
}

pub(crate) fn load(tx: &Transaction<'_>, id: i64) -> Result<Task> {
    tx.query_row(
        &format!("SELECT {TASK_COLUMNS} FROM tasks WHERE id = ?1"),
        [id],
        task_from_row,
    )
    .optional()?
    .ok_or(CoreError::TaskNotFound(id))
}

/// Cambia el estado validando el contrato y deja constancia de la transición.
fn transition(
    tx: &Transaction<'_>,
    id: i64,
    to: TaskState,
    reason: &str,
    result: Option<&Value>,
    error: Option<&TaskError>,
) -> Result<Task> {
    let task = load(tx, id)?;
    if !task.state.can_move_to(to) {
        return Err(CoreError::InvalidTransition {
            task: id,
            from: task.state,
            to,
        });
    }
    let now = now_ms();
    let attempts = task.attempts + i64::from(to == TaskState::Running);
    // Un resultado o error nuevo sustituye al anterior; al reencolar se conserva el último error visible.
    let result_json = result.map(serde_json::to_string).transpose()?;
    let error_json = error.map(serde_json::to_string).transpose()?;
    tx.execute(
        "UPDATE tasks SET state = ?2, attempts = ?3, updated_at = ?4,
             result = COALESCE(?5, result), error = CASE WHEN ?2 = 'completed' THEN NULL ELSE COALESCE(?6, error) END
         WHERE id = ?1",
        params![id, to.as_str(), attempts, now, result_json, error_json],
    )?;
    tx.execute(
        "INSERT INTO task_transitions (task_id, from_state, to_state, reason, at) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![id, task.state.as_str(), to.as_str(), reason, now],
    )?;
    load(tx, id)
}

/// Alta de una tarea en cola con su transición inicial, dentro de la transacción del llamador.
pub(crate) fn enqueue_in(tx: &Transaction<'_>, kind: &str, input_ref: &str) -> Result<Task> {
    let now = now_ms();
    tx.execute(
        "INSERT INTO tasks (kind, input_ref, state, created_at, updated_at) VALUES (?1, ?2, 'queued', ?3, ?3)",
        params![kind, input_ref, now],
    )?;
    let id = tx.last_insert_rowid();
    tx.execute(
        "INSERT INTO task_transitions (task_id, from_state, to_state, reason, at) VALUES (?1, NULL, 'queued', 'creada', ?2)",
        params![id, now],
    )?;
    load(tx, id)
}

/// Inserta una tarea que nace ya en `state` con su resultado, y su transición desde `NULL`, en
/// la transacción dada. No cambia `can_move_to`: es una creación, no una transición.
fn insert_born(
    tx: &Transaction<'_>,
    kind: &str,
    input_ref: &str,
    state: TaskState,
    result: &Value,
    reason: &str,
) -> Result<Task> {
    let now = now_ms();
    tx.execute(
        "INSERT INTO tasks (kind, input_ref, state, result, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
        params![kind, input_ref, state.as_str(), serde_json::to_string(result)?, now],
    )?;
    let id = tx.last_insert_rowid();
    tx.execute(
        "INSERT INTO task_transitions (task_id, from_state, to_state, reason, at) VALUES (?1, NULL, ?2, ?3, ?4)",
        params![id, state.as_str(), reason, now],
    )?;
    load(tx, id)
}

/// Evento del historial sin contenido (mismo formato que `record_event`) dentro de una transacción.
fn insert_event(tx: &Transaction<'_>, task_id: i64, event: (&str, &str, &Value)) -> Result<()> {
    tx.execute(
        "INSERT INTO audit_log (at, task_id, action, status, authorization, detail) VALUES (?1, ?2, ?3, ?4, NULL, ?5)",
        params![now_ms(), task_id, event.0, event.1, event.2.to_string()],
    )?;
    Ok(())
}

impl Store {
    pub fn enqueue(&mut self, kind: &str, input_ref: &str) -> Result<Task> {
        let tx = self.conn.transaction()?;
        let task = enqueue_in(&tx, kind, input_ref)?;
        tx.commit()?;
        Ok(task)
    }

    /// Crea una tarea ya `completed` con su resultado y su transición desde `NULL`, en una sola
    /// transacción (D157): un corte deja la tarea completa o ninguna, nunca una tarea en cola
    /// que otro paso pudiera llevar a la IA. Solo para tareas que no tienen nada que ejecutar.
    pub fn enqueue_closed(
        &mut self,
        kind: &str,
        input_ref: &str,
        result: &Value,
        reason: &str,
    ) -> Result<Task> {
        let tx = self.conn.transaction()?;
        let task = insert_born(&tx, kind, input_ref, TaskState::Completed, result, reason)?;
        tx.commit()?;
        Ok(task)
    }

    /// Como `enqueue_closed`, pero la tarea nace pendiente de revisión (un rebote, D157): no pasa
    /// por la cola ni por la IA.
    pub fn enqueue_review(
        &mut self,
        kind: &str,
        input_ref: &str,
        result: &Value,
        reason: &str,
    ) -> Result<Task> {
        let tx = self.conn.transaction()?;
        let task = insert_born(
            &tx,
            kind,
            input_ref,
            TaskState::AwaitingReview,
            result,
            reason,
        )?;
        tx.commit()?;
        Ok(task)
    }

    /// Completa una tarea pendiente de revisión con ese resultado y deja un evento del historial
    /// sin contenido, en la misma transacción («Archivar», D159).
    pub fn complete_noting(
        &mut self,
        id: i64,
        result: &Value,
        reason: &str,
        event: (&str, &str, &Value),
    ) -> Result<Task> {
        let tx = self.conn.transaction()?;
        let task = transition(&tx, id, TaskState::Completed, reason, Some(result), None)?;
        insert_event(&tx, id, event)?;
        tx.commit()?;
        Ok(task)
    }

    /// La tarea de «Redactar respuesta» (D159) cuya entrada lleva `redraft_of = source`, si hay una
    /// que no esté cancelada.
    pub fn redraft_of(&self, source: i64) -> Result<Option<Task>> {
        Ok(self
            .conn
            .query_row(
                &format!(
                    "SELECT {TASK_COLUMNS} FROM tasks
                     WHERE CASE WHEN json_valid(input_ref) THEN json_extract(input_ref, '$.redraft_of') END = ?1
                       AND state <> 'cancelled' ORDER BY id LIMIT 1"
                ),
                [source],
                task_from_row,
            )
            .optional()?)
    }

    /// Crea la tarea de «Redactar respuesta» y, si `archive` lleva el resultado, cierra la de origen
    /// (que debe estar pendiente de revisión) en la misma transacción. Idempotente: si ya hay una
    /// tarea de redacción viva para ese origen, la devuelve (`false`) sin crear otra ni tocar el
    /// origen; el índice único de la migración 005 impide el duplicado aunque dos llamadas coincidan.
    pub fn enqueue_redraft(
        &mut self,
        source: i64,
        input_ref: &str,
        archive: Option<&Value>,
    ) -> Result<(Task, bool)> {
        if let Some(existing) = self.redraft_of(source)? {
            return Ok((existing, false));
        }
        match self.insert_redraft(source, input_ref, archive) {
            // Otra conexión se adelantó entre la consulta y la inserción: el índice único lo
            // impidió y la transacción se deshizo (el origen sigue como estaba). Se devuelve la
            // tarea que ganó.
            Err(CoreError::Storage(rusqlite::Error::SqliteFailure(error, message)))
                if error.code == rusqlite::ErrorCode::ConstraintViolation =>
            {
                match self.redraft_of(source)? {
                    Some(existing) => Ok((existing, false)),
                    None => Err(CoreError::Storage(rusqlite::Error::SqliteFailure(
                        error, message,
                    ))),
                }
            }
            other => other,
        }
    }

    fn insert_redraft(
        &mut self,
        source: i64,
        input_ref: &str,
        archive: Option<&Value>,
    ) -> Result<(Task, bool)> {
        let tx = self.conn.transaction()?;
        if let Some(result) = archive {
            transition(
                &tx,
                source,
                TaskState::Completed,
                "archivada al pedir una respuesta",
                Some(result),
                None,
            )?;
            insert_event(&tx, source, ("archive", "archived", &json!({})))?;
        }
        let now = now_ms();
        tx.execute(
            "INSERT INTO tasks (kind, input_ref, state, created_at, updated_at) VALUES ('email', ?1, 'queued', ?2, ?2)",
            params![input_ref, now],
        )?;
        let id = tx.last_insert_rowid();
        tx.execute(
            "INSERT INTO task_transitions (task_id, from_state, to_state, reason, at) VALUES (?1, NULL, 'queued', 'redacción pedida', ?2)",
            params![id, now],
        )?;
        let task = load(&tx, id)?;
        insert_event(&tx, id, ("redraft", "queued", &json!({ "of": source })))?;
        tx.commit()?;
        Ok((task, true))
    }

    pub fn task(&mut self, id: i64) -> Result<Task> {
        let tx = self.conn.transaction()?;
        let task = load(&tx, id)?;
        tx.commit()?;
        Ok(task)
    }

    pub fn tasks_in(&self, state: TaskState) -> Result<Vec<Task>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {TASK_COLUMNS} FROM tasks WHERE state = ?1 ORDER BY id"
        ))?;
        let rows = stmt.query_map([state.as_str()], task_from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Tareas en cola que pueden empezar (`READY_SQL`), de la más antigua a la más reciente.
    /// Una tarea bloqueada por su pipeline no aparece y no frena a las siguientes.
    pub fn ready_tasks(&self) -> Result<Vec<Task>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {TASK_COLUMNS} FROM tasks WHERE state = 'queued' AND {READY_SQL} ORDER BY id"
        ))?;
        let rows = stmt.query_map([], task_from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Toma la tarea lista más antigua y la pasa a `running`, contando el intento.
    pub fn claim_next(&mut self) -> Result<Option<Task>> {
        let tx = self.conn.transaction()?;
        let next: Option<i64> = tx
            .query_row(
                &format!(
                    "SELECT id FROM tasks WHERE state = 'queued' AND {READY_SQL} ORDER BY id LIMIT 1"
                ),
                [],
                |r| r.get(0),
            )
            .optional()?;
        let task = match next {
            Some(id) => Some(transition(
                &tx,
                id,
                TaskState::Running,
                "iniciada",
                None,
                None,
            )?),
            None => None,
        };
        tx.commit()?;
        Ok(task)
    }

    /// Toma una tarea concreta en cola y la pasa a `running`, contando el intento.
    /// Rechaza con `TaskBlocked`, sin gastar intento, una tarea que su pipeline retiene.
    pub fn claim(&mut self, id: i64) -> Result<Task> {
        let tx = self.conn.transaction()?;
        let blocked = tx
            .query_row(
                &format!("SELECT NOT ({READY_SQL}) FROM tasks WHERE id = ?1 AND state = 'queued'"),
                [id],
                |r| r.get::<_, bool>(0),
            )
            .optional()?
            .unwrap_or(false);
        if blocked {
            return Err(CoreError::TaskBlocked(id));
        }
        let task = transition(&tx, id, TaskState::Running, "iniciada", None, None)?;
        tx.commit()?;
        Ok(task)
    }

    /// Devuelve a la cola una tarea en curso que no pudo ejecutarse por causas ajenas a ella
    /// (por ejemplo, la IA no está configurada): no gasta intento y deja constancia.
    pub fn release(&mut self, id: i64, reason: &str) -> Result<Task> {
        let tx = self.conn.transaction()?;
        let task = transition(&tx, id, TaskState::Queued, reason, None, None)?;
        tx.execute(
            "UPDATE tasks SET attempts = MAX(attempts - 1, 0) WHERE id = ?1",
            [id],
        )?;
        let task = Task {
            attempts: (task.attempts - 1).max(0),
            ..task
        };
        tx.commit()?;
        Ok(task)
    }

    pub fn complete(&mut self, id: i64, result: &Value) -> Result<Task> {
        self.apply(id, TaskState::Completed, "completada", Some(result), None)
    }

    pub fn await_review(&mut self, id: i64, result: &Value) -> Result<Task> {
        self.apply(
            id,
            TaskState::AwaitingReview,
            "pendiente de revisión",
            Some(result),
            None,
        )
    }

    pub fn fail(&mut self, id: i64, error: &TaskError) -> Result<Task> {
        self.apply(id, TaskState::Failed, "fallida", None, Some(error))
    }

    /// Reintento explícito del usuario sobre una tarea fallida.
    pub fn retry(&mut self, id: i64) -> Result<Task> {
        self.apply(id, TaskState::Queued, "reintento solicitado", None, None)
    }

    pub fn cancel(&mut self, id: i64) -> Result<Task> {
        self.apply(
            id,
            TaskState::Cancelled,
            "cancelada por el usuario",
            None,
            None,
        )
    }

    /// Sustituye el resultado de una tarea pendiente de revisión (por ejemplo, un borrador
    /// editado) sin cambiar su estado, y deja constancia en el historial. Quien llama es
    /// responsable de cambiar la versión del contenido para que exija nueva aprobación.
    pub fn revise(&mut self, id: i64, result: &Value) -> Result<Task> {
        let tx = self.conn.transaction()?;
        let task = load(&tx, id)?;
        if task.state != TaskState::AwaitingReview {
            return Err(CoreError::InvalidTransition {
                task: id,
                from: task.state,
                to: TaskState::AwaitingReview,
            });
        }
        let now = now_ms();
        tx.execute(
            "UPDATE tasks SET result = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, serde_json::to_string(result)?, now],
        )?;
        tx.execute(
            "INSERT INTO task_transitions (task_id, from_state, to_state, reason, at)
             VALUES (?1, 'awaiting_review', 'awaiting_review', 'contenido revisado', ?2)",
            params![id, now],
        )?;
        let task = load(&tx, id)?;
        tx.commit()?;
        Ok(task)
    }

    /// Guarda en una tarea en curso o en revisión el resultado exacto que se va a ejecutar,
    /// antes de llamar a ningún conector y sin cambiar su estado (D50). Al reprocesarla se
    /// reutiliza este contenido en vez de pedir otro a la IA.
    pub fn record_attempt(&mut self, id: i64, result: &Value) -> Result<Task> {
        let tx = self.conn.transaction()?;
        let task = load(&tx, id)?;
        if !matches!(task.state, TaskState::Running | TaskState::AwaitingReview) {
            return Err(CoreError::InvalidTransition {
                task: id,
                from: task.state,
                to: task.state,
            });
        }
        let now = now_ms();
        tx.execute(
            "UPDATE tasks SET result = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, serde_json::to_string(result)?, now],
        )?;
        tx.execute(
            "INSERT INTO task_transitions (task_id, from_state, to_state, reason, at)
             VALUES (?1, ?2, ?2, 'intento de acción registrado', ?3)",
            params![id, task.state.as_str(), now],
        )?;
        let task = load(&tx, id)?;
        tx.commit()?;
        Ok(task)
    }

    /// Todas las tareas, de la más reciente a la más antigua.
    pub fn all_tasks(&self) -> Result<Vec<Task>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {TASK_COLUMNS} FROM tasks ORDER BY id DESC"
        ))?;
        let rows = stmt.query_map([], task_from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Una tarea por id, solo lectura. `None` si no existe.
    pub fn task_by_id(&self, id: i64) -> Result<Option<Task>> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {TASK_COLUMNS} FROM tasks WHERE id = ?1"),
                [id],
                task_from_row,
            )
            .optional()?)
    }

    /// Las tareas con esos ids, solo lectura; los ids que no existen se omiten.
    pub fn tasks_by_ids(&self, ids: &[i64]) -> Result<Vec<Task>> {
        let mut out = Vec::new();
        for id in ids {
            out.extend(self.task_by_id(*id)?);
        }
        Ok(out)
    }

    /// Tras una interrupción, devuelve a la cola las tareas que estaban en curso.
    /// Las acciones con efectos se reconcilian aparte (`reconcile_actions`).
    pub fn recover(&mut self) -> Result<Vec<i64>> {
        let tx = self.conn.transaction()?;
        let ids: Vec<i64> = {
            let mut stmt =
                tx.prepare("SELECT id FROM tasks WHERE state = 'running' ORDER BY id")?;
            let rows = stmt.query_map([], |r| r.get(0))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        for id in &ids {
            transition(
                &tx,
                *id,
                TaskState::Queued,
                "recuperada tras interrupción",
                None,
                None,
            )?;
        }
        tx.commit()?;
        Ok(ids)
    }

    /// Historial de estados de una tarea, del más antiguo al más reciente.
    pub fn transitions(&self, id: i64) -> Result<Vec<(Option<String>, String, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT from_state, to_state, reason FROM task_transitions WHERE task_id = ?1 ORDER BY id",
        )?;
        let rows = stmt.query_map([id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    fn apply(
        &mut self,
        id: i64,
        to: TaskState,
        reason: &str,
        result: Option<&Value>,
        error: Option<&TaskError>,
    ) -> Result<Task> {
        let tx = self.conn.transaction()?;
        let task = transition(&tx, id, to, reason, result, error)?;
        tx.commit()?;
        Ok(task)
    }
}
