use std::fmt;
use std::str::FromStr;

use rusqlite::{params, OptionalExtension, Row, Transaction};
use serde::{Deserialize, Serialize};
use serde_json::Value;

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

fn load(tx: &Transaction<'_>, id: i64) -> Result<Task> {
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

impl Store {
    pub fn enqueue(&mut self, kind: &str, input_ref: &str) -> Result<Task> {
        let tx = self.conn.transaction()?;
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
        let task = load(&tx, id)?;
        tx.commit()?;
        Ok(task)
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

    /// Toma la tarea en cola más antigua y la pasa a `running`, contando el intento.
    pub fn claim_next(&mut self) -> Result<Option<Task>> {
        let tx = self.conn.transaction()?;
        let next: Option<i64> = tx
            .query_row(
                "SELECT id FROM tasks WHERE state = 'queued' ORDER BY id LIMIT 1",
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
