//! Lo que el almacén aporta al aprendizaje continuo (D178, R31): consultas de solo lectura para
//! decidir qué comprobar y el cierre atómico de una tarea respondida fuera de NestAdmin. Nada de
//! esto guarda ni lee texto de correo: ids, claves de señal, estados y marcas de tiempo.

use rusqlite::{params, OptionalExtension};
use serde_json::{json, Value};

use crate::error::Result;
use crate::queue::{insert_event, transition, Task, TaskState};
use crate::rule_panel::LearnedSignal;
use crate::store::{now_ms, Store};

/// Acción del Historial de una respuesta hecha fuera (D178), sin contenido.
pub const MAIL_ACTION: &str = "mail";
/// Estado de esa entrada cuando la tarea en revisión se cierra.
pub const REPLIED_OUTSIDE: &str = "replied_outside";
/// Estado de la entrada cuando una tarea ya archivada aporta evidencia.
pub const LEARNED_OUTSIDE: &str = "learned_outside";

/// Condición «ya hay una fila `learned` de ese mensaje» sobre el id del mensaje en `?4`.
const SAME_MESSAGE: &str = "kind = 'learned' AND source_key IS NOT NULL
           AND length(source_key) > length(?4)
           AND substr(source_key, length(source_key) - length(?4)) = '|' || ?4";

/// Inserta una fila `learned` si no hay ya una de ese mensaje. La clave es
/// `learned|<fuente>|<id del mensaje>` y la fuente de una cuenta cambia cuando el proveedor da la
/// dirección más tarde (de `…:id:<id>` a `…:<dirección>`); el id del mensaje, que lleva el prefijo
/// de la cuenta, no cambia. Por eso la unicidad se comprueba también por el mensaje (`?4`,
/// [`learned_message_id`]), además del índice único de `source_key`: un mismo correo no cuenta
/// dos veces entre la primera carga y el aprendizaje continuo (D172, D178).
pub(crate) fn insert_learned_sql() -> String {
    format!(
        "INSERT OR IGNORE INTO rule_signals
     (kind, action_kind, subject_key, edit_state, task_id, content_version, source_key, at, state)
     SELECT 'learned', 'send_draft', ?1, 'unknown', NULL, 0, ?2, ?3, 'valid'
     WHERE NOT EXISTS (SELECT 1 FROM rule_signals WHERE {SAME_MESSAGE})"
    )
}

/// Id del mensaje de una clave `learned|<fuente>|<id del mensaje>`; si la clave no tiene esa
/// forma, la clave entera (entonces solo la deduplica el índice único).
pub(crate) fn learned_message_id(source_key: &str) -> &str {
    let mut parts = source_key.splitn(3, '|');
    match (parts.next(), parts.next(), parts.next()) {
        (Some("learned"), Some(_), Some(id)) if !id.is_empty() => id,
        _ => source_key,
    }
}

impl Store {
    /// Si ya hay una fila `learned` con esa clave o de ese mismo mensaje con otra fuente de la
    /// cuenta (la primera carga o una pasada anterior).
    pub fn learned_source_exists(&self, source_key: &str) -> Result<bool> {
        Ok(self
            .conn
            .query_row(
                &format!(
                    "SELECT 1 FROM rule_signals
                     WHERE (kind = 'learned' AND source_key = ?1) OR ({SAME_MESSAGE}) LIMIT 1"
                ),
                params![
                    source_key,
                    rusqlite::types::Null,
                    rusqlite::types::Null,
                    learned_message_id(source_key)
                ],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    /// Si la tarea tiene una señal de ese tipo (`rejected`, `approved`…), sea cual sea su estado.
    pub fn task_has_signal(&self, task_id: i64, kind: &str) -> Result<bool> {
        Ok(self
            .conn
            .query_row(
                "SELECT 1 FROM rule_signals WHERE task_id = ?1 AND kind = ?2 LIMIT 1",
                params![task_id, kind],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    /// Si hay un rechazo o una edición (señales `rejected` y `edited`, no anuladas) del mismo
    /// `categoría|remitente` registrados después de `after_ms` (D178): una fila `learned` que se
    /// insertara ahora contaría tras ese corte de la serie (D172).
    pub fn series_break_after(&self, subject_key: &str, after_ms: i64) -> Result<bool> {
        Ok(self
            .conn
            .query_row(
                "SELECT 1 FROM rule_signals
                 WHERE kind IN ('rejected', 'edited') AND subject_key = ?1 AND at > ?2
                   AND state <> 'void' LIMIT 1",
                params![subject_key, after_ms],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    /// Instantes (ms) de los envíos de respuesta de NestAdmin confirmados o en curso (`pending`,
    /// `failed` sin resolver, `confirmed`) de las tareas de correo del mismo hilo y de la misma
    /// cuenta (el id del mensaje empieza por `<cuenta>:`). Un envío `superseded` no ocurrió.
    pub fn thread_send_times(&self, account: &str, thread_key: &str) -> Result<Vec<i64>> {
        let prefix = format!("{account}:");
        let mut stmt = self.conn.prepare(
            "SELECT a.created_at FROM actions a JOIN tasks t ON t.id = a.task_id
             WHERE a.kind = 'send_draft' AND a.status IN ('pending', 'failed', 'confirmed')
               AND CASE WHEN json_valid(t.result) THEN json_extract(t.result, '$.message.thread_key') END = ?1
               AND CASE WHEN json_valid(t.result) THEN substr(json_extract(t.result, '$.message.id'), 1, length(?2)) END = ?2",
        )?;
        let rows = stmt.query_map(params![thread_key, prefix], |r| r.get::<_, i64>(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Guarda una fila `learned` (única por `source_key`) con una entrada del Historial sin
    /// contenido ligada a la tarea de la que sale, en una transacción. `false` si ya existía.
    pub fn insert_learned_outside(&mut self, task_id: i64, signal: &LearnedSignal) -> Result<bool> {
        let tx = self.conn.transaction()?;
        let inserted = tx.execute(
            &insert_learned_sql(),
            params![
                signal.subject_key,
                signal.source_key,
                now_ms(),
                learned_message_id(&signal.source_key)
            ],
        )? > 0;
        if inserted {
            insert_event(&tx, task_id, (MAIL_ACTION, LEARNED_OUTSIDE, &json!({})))?;
        }
        tx.commit()?;
        Ok(inserted)
    }

    /// Cierra sin enviar una tarea en revisión cuya respuesta se hizo fuera (D178): la pasa a
    /// `completed` con ese resultado, deja la entrada `mail` / `replied_outside` y, si se pide,
    /// inserta la fila `learned`, todo en una transacción. Solo si la tarea sigue en
    /// `awaiting_review` y no cambió desde que se leyó (`expected_updated_at`): si la persona la
    /// aprobó, archivó, editó o rechazó mientras tanto devuelve `None` y no escribe nada.
    /// En el `Some`, el segundo valor dice si la fila `learned` era nueva.
    pub fn close_replied_outside(
        &mut self,
        id: i64,
        expected_updated_at: i64,
        result: &Value,
        signal: Option<&LearnedSignal>,
    ) -> Result<Option<(Task, bool)>> {
        let tx = self.conn.transaction()?;
        let current = crate::queue::load(&tx, id)?;
        if current.state != TaskState::AwaitingReview || current.updated_at != expected_updated_at {
            return Ok(None);
        }
        let task = transition(
            &tx,
            id,
            TaskState::Completed,
            "respondido fuera de NestAdmin",
            Some(result),
            None,
        )?;
        insert_event(&tx, id, (MAIL_ACTION, REPLIED_OUTSIDE, &json!({})))?;
        let inserted = match signal {
            Some(s) => {
                tx.execute(
                    &insert_learned_sql(),
                    params![
                        s.subject_key,
                        s.source_key,
                        now_ms(),
                        learned_message_id(&s.source_key)
                    ],
                )? > 0
            }
            None => false,
        };
        tx.commit()?;
        Ok(Some((task, inserted)))
    }
}
