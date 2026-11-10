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

/// Partes de una clave `learned|<fuente>|<id del mensaje>`: la fuente (puede llevar `|` en la
/// dirección) y el id, que es lo que sigue al último `|`. `None` si no tiene esa forma.
fn parse_learned_key(source_key: &str) -> Option<(&str, &str)> {
    let rest = source_key.strip_prefix("learned|")?;
    let (source, id) = rest.rsplit_once('|')?;
    (!source.is_empty() && !id.is_empty()).then_some((source, id))
}

/// Id del mensaje de una clave `learned|<fuente>|<id del mensaje>`; si la clave no tiene esa
/// forma, la clave entera (entonces solo la deduplica el índice único).
pub(crate) fn learned_message_id(source_key: &str) -> &str {
    parse_learned_key(source_key).map_or(source_key, |(_, id)| id)
}

/// Id del mensaje sin el prefijo de la cuenta (`<cuenta>:<id interno>`): quitar y volver a añadir
/// un buzón cambia el id de la cuenta, no el del correo.
fn inner_message_id(id: &str) -> &str {
    id.split_once(':').map_or(id, |(_, inner)| inner)
}

/// Proveedor de una fuente de cuenta `account:<proveedor>:<dirección>` o
/// `account:<proveedor>:id:<id de la cuenta>`, y si va por identificador.
fn account_source(source: &str) -> Option<(&str, bool)> {
    let mut parts = source.splitn(3, ':');
    match (parts.next(), parts.next(), parts.next()) {
        (Some("account"), Some(provider), Some(rest)) => Some((provider, rest.starts_with("id:"))),
        _ => None,
    }
}

/// Si dos claves `learned` son del mismo correo (D178): misma fuente e igual id interno del
/// mensaje (sin el prefijo de la cuenta), o, si una de las fuentes va por el identificador de la
/// cuenta (aún sin dirección), otra fuente del mismo proveedor con el mismo id interno.
fn same_learned_message(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    let (Some((source_a, id_a)), Some((source_b, id_b))) =
        (parse_learned_key(a), parse_learned_key(b))
    else {
        return false;
    };
    if inner_message_id(id_a) != inner_message_id(id_b) {
        return false;
    }
    if source_a == source_b {
        return true;
    }
    match (account_source(source_a), account_source(source_b)) {
        (Some((pa, by_id_a)), Some((pb, by_id_b))) => pa == pb && (by_id_a || by_id_b),
        _ => false,
    }
}

/// Si ya hay una fila `learned` de ese mismo correo (ver [`same_learned_message`]).
fn learned_duplicate_exists(conn: &rusqlite::Connection, source_key: &str) -> Result<bool> {
    let inner = inner_message_id(learned_message_id(source_key));
    let mut stmt = conn.prepare(
        "SELECT source_key FROM rule_signals
         WHERE kind = 'learned' AND source_key IS NOT NULL
           AND (source_key = ?1 OR (length(?2) > 0 AND substr(source_key, -length(?2)) = ?2))",
    )?;
    let rows = stmt.query_map(params![source_key, inner], |r| r.get::<_, String>(0))?;
    for row in rows {
        if same_learned_message(&row?, source_key) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Inserta una fila `learned` si no hay ya una de ese correo. La clave es
/// `learned|<fuente>|<id del mensaje>`; la fuente de una cuenta cambia cuando el proveedor da la
/// dirección más tarde y el id del mensaje cambia de prefijo si el buzón se quita y se vuelve a
/// añadir. Por eso la unicidad se comprueba también por fuente + id interno
/// ([`same_learned_message`]), además del índice único de `source_key`: un mismo correo no cuenta
/// dos veces entre la primera carga y el aprendizaje continuo (D172, D178). `false` si ya existía.
pub(crate) fn insert_learned(
    conn: &rusqlite::Connection,
    subject_key: &str,
    source_key: &str,
    at: i64,
) -> Result<bool> {
    if learned_duplicate_exists(conn, source_key)? {
        return Ok(false);
    }
    Ok(conn.execute(
        "INSERT OR IGNORE INTO rule_signals
         (kind, action_kind, subject_key, edit_state, task_id, content_version, source_key, at, state)
         VALUES ('learned', 'send_draft', ?1, 'unknown', NULL, 0, ?2, ?3, 'valid')",
        params![subject_key, source_key, at],
    )? > 0)
}

impl Store {
    /// Si ya hay una fila `learned` con esa clave o de ese mismo mensaje con otra fuente de la
    /// cuenta (la primera carga o una pasada anterior).
    pub fn learned_source_exists(&self, source_key: &str) -> Result<bool> {
        learned_duplicate_exists(&self.conn, source_key)
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

    /// Instantes (ms) de los envíos de respuesta de NestAdmin (`pending`, `failed`, `confirmed`)
    /// de las tareas de correo de la cuenta cuyo resultado no guarda `message.thread_key` (D178):
    /// pueden ser de cualquier hilo, también del que se acaba de recuperar para otra tarea.
    pub fn unthreaded_send_times(&self, account: &str) -> Result<Vec<i64>> {
        let prefix = format!("{account}:");
        let mut stmt = self.conn.prepare(
            "SELECT a.created_at FROM actions a JOIN tasks t ON t.id = a.task_id
             WHERE a.kind = 'send_draft' AND a.status IN ('pending', 'failed', 'confirmed')
               AND CASE WHEN json_valid(t.result) THEN COALESCE(trim(json_extract(t.result, '$.message.thread_key')), '') END = ''
               AND CASE WHEN json_valid(t.result) THEN substr(json_extract(t.result, '$.message.id'), 1, length(?1)) END = ?1",
        )?;
        let rows = stmt.query_map(params![prefix], |r| r.get::<_, i64>(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Guarda una fila `learned` (única por `source_key`) con una entrada del Historial sin
    /// contenido ligada a la tarea de la que sale, en una transacción. `false` si ya existía.
    pub fn insert_learned_outside(&mut self, task_id: i64, signal: &LearnedSignal) -> Result<bool> {
        let tx = self.conn.transaction()?;
        let inserted = insert_learned(&tx, &signal.subject_key, &signal.source_key, now_ms())?;
        if inserted {
            insert_event(&tx, task_id, (MAIL_ACTION, LEARNED_OUTSIDE, &json!({})))?;
        }
        tx.commit()?;
        Ok(inserted)
    }

    /// Guarda el hilo recuperado de una tarea de correo creada antes de que el resultado lo
    /// llevara (D178): `message.thread_key` y `message.thread_recovered = true`, sin tocar nada
    /// más del resultado ni el estado, en una transacción. Solo escribe si la tarea sigue en
    /// `expected_state` con el mismo `updated_at` que al leerla y su mensaje aún no tiene hilo;
    /// si no, devuelve `None`. No cambia `updated_at` (el hilo no es un cambio de la tarea que
    /// deba invalidar otras comprobaciones) ni deja entrada en el Historial.
    pub fn record_recovered_thread(
        &mut self,
        id: i64,
        expected_state: TaskState,
        expected_updated_at: i64,
        thread_key: &str,
    ) -> Result<Option<Task>> {
        let thread_key = thread_key.trim();
        if thread_key.is_empty() {
            return Ok(None);
        }
        let tx = self.conn.transaction()?;
        let current = crate::queue::load(&tx, id)?;
        if current.state != expected_state || current.updated_at != expected_updated_at {
            return Ok(None);
        }
        let Some(mut result) = current.result.clone() else {
            return Ok(None);
        };
        let has_thread = result["message"]["thread_key"]
            .as_str()
            .is_some_and(|k| !k.trim().is_empty());
        let Some(message) = result.get_mut("message").and_then(Value::as_object_mut) else {
            return Ok(None);
        };
        if has_thread {
            return Ok(None);
        }
        message.insert("thread_key".into(), json!(thread_key));
        message.insert("thread_recovered".into(), json!(true));
        tx.execute(
            "UPDATE tasks SET result = ?2 WHERE id = ?1",
            params![id, serde_json::to_string(&result)?],
        )?;
        let task = crate::queue::load(&tx, id)?;
        tx.commit()?;
        Ok(Some(task))
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
            Some(s) => insert_learned(&tx, &s.subject_key, &s.source_key, now_ms())?,
            None => false,
        };
        tx.commit()?;
        Ok(Some((task, inserted)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_message_id_is_what_follows_the_last_pipe() {
        assert_eq!(
            learned_message_id("learned|account:g:a@b.test|a1:m1"),
            "a1:m1"
        );
        // Una dirección sintética con `|` en la parte local no rompe la extracción.
        assert_eq!(
            learned_message_id("learned|account:g:a|b@c.test|a1:m1"),
            "a1:m1"
        );
        // Sin el prefijo `learned|` o sin id, la clave entera.
        assert_eq!(learned_message_id("x|y|z"), "x|y|z");
        assert_eq!(learned_message_id("learned|y|"), "learned|y|");
    }
}
