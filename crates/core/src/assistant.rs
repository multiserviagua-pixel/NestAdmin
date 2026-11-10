//! Lo que el asistente del chat guarda y consulta en el equipo (D182, D184, D188, R33): sus
//! conversaciones y lo enviado en cada llamada (migración 11), y las lecturas de solo lectura
//! que le sirven de herramientas (agregación sobre el índice, recuentos y eventos de una tarea).
//! Nada de esto sale del equipo. Ninguna lectura devuelve el `detail` libre del historial.

use rusqlite::{named_params, params, OptionalExtension};
use serde::Serialize;
use serde_json::Value;

use crate::error::{CoreError, Result};
use crate::search::{
    count, hit_from_row, unindexed_sql, validate, SearchFilters, SearchHit, SearchKind, FILTER_SQL,
    HIT_COLUMNS, SNIPPET_CHARS,
};
use crate::store::{now_ms, Store};

/// Qué se agrega: los filtros de D180 que admite `aggregate`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AggregateQuery {
    pub kind: Option<SearchKind>,
    /// `AAAA-MM-DD`, inclusive.
    pub from: Option<String>,
    pub to: Option<String>,
    pub party: Option<String>,
    /// Cuenta como pendientes de indexar también las tareas en cola (como la búsqueda).
    pub include_queued: bool,
}

/// Suma de totales de una moneda, en céntimos.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CurrencyTotal {
    /// Vacía si el documento no indica moneda.
    pub currency: String,
    pub documents: usize,
    pub total_minor: i64,
}

/// Cifras exactas sobre el índice. Los documentos cancelados (rechazados por la persona) no se
/// cuentan ni suman; se dice cuántos eran.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Aggregate {
    /// Documentos que cumplen los filtros (sin los cancelados).
    pub documents: usize,
    pub with_total: usize,
    /// Documentos sin importe (por ejemplo, los correos o los que no se pudieron leer).
    pub without_total: usize,
    pub excluded_cancelled: usize,
    /// Tareas todavía sin fila en el índice: la cifra no las incluye.
    pub pending_index: usize,
    /// Suma por moneda; nunca se mezclan monedas.
    pub by_currency: Vec<CurrencyTotal>,
}

/// Una tarea para el listado del asistente.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TaskRow {
    pub task_id: i64,
    pub kind: String,
    pub state: String,
    /// Del índice, si la tarea está indexada.
    pub title: String,
    pub party: String,
    pub doc_date: Option<String>,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TaskCounts {
    pub state: String,
    pub count: usize,
}

/// Evento de una tarea: solo código, estado y fecha (ms desde 1970). Nunca el detalle libre.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TaskEvent {
    pub code: String,
    pub status: String,
    pub at: i64,
}

/// Una llamada a un proveedor, para guardar con su turno.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewCall {
    pub step: u32,
    pub provider: String,
    pub model: String,
    pub host: String,
    /// Respondió la IA local.
    pub local: bool,
    /// Texto literal enviado: instrucciones y contenido.
    pub sent_text: String,
}

/// Lo enviado en una llamada, para «Ver lo enviado».
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SentCall {
    pub step: u32,
    pub provider: String,
    pub model: String,
    pub host: String,
    pub local: bool,
    pub sent_text: String,
    pub at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StoredTurn {
    pub id: i64,
    /// `user` o `assistant`.
    pub role: String,
    pub text: String,
    pub meta: Value,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ConversationView {
    pub id: i64,
    pub created_at: i64,
    pub updated_at: i64,
    pub turns: Vec<StoredTurn>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SavedExchange {
    pub conversation_id: i64,
    pub user_turn_id: i64,
    pub turn_id: i64,
}

fn usize_of(n: i64) -> usize {
    usize::try_from(n).unwrap_or(0)
}

impl Store {
    /// Cifras exactas sobre el índice (D182: las cifras las calcula la aplicación). Un filtro no
    /// válido es `invalid_filter`.
    pub fn aggregate(&self, query: &AggregateQuery) -> Result<Aggregate> {
        let filters = SearchFilters {
            kind: query.kind,
            from: query.from.clone(),
            to: query.to.clone(),
            party: query.party.clone(),
            ..SearchFilters::default()
        };
        validate(&filters)?;
        let kind = filters.kind.map(SearchKind::as_str);
        let base =
            format!("FROM search_index i JOIN tasks t ON t.id = i.task_id WHERE {FILTER_SQL}");
        let counted = |extra: &str| -> Result<usize> {
            count(
                &self.conn,
                &format!("SELECT count(*) {base} {extra}"),
                named_params! {
                    ":kind": kind, ":from": filters.from, ":to": filters.to,
                    ":party": filters.party, ":min": Option::<i64>::None,
                    ":max": Option::<i64>::None, ":status": Option::<String>::None,
                },
            )
        };
        let documents = counted("AND t.state <> 'cancelled'")?;
        let with_total = counted("AND t.state <> 'cancelled' AND i.total_minor IS NOT NULL")?;
        let excluded_cancelled = counted("AND t.state = 'cancelled'")?;
        let mut stmt = self.conn.prepare(&format!(
            "SELECT i.currency, count(*), sum(i.total_minor) {base}
             AND t.state <> 'cancelled' AND i.total_minor IS NOT NULL
             GROUP BY i.currency ORDER BY i.currency"
        ))?;
        let by_currency = stmt
            .query_map(
                named_params! {
                    ":kind": kind, ":from": filters.from, ":to": filters.to,
                    ":party": filters.party, ":min": Option::<i64>::None,
                    ":max": Option::<i64>::None, ":status": Option::<String>::None,
                },
                |r| {
                    Ok(CurrencyTotal {
                        currency: r.get(0)?,
                        documents: usize_of(r.get(1)?),
                        total_minor: r.get(2)?,
                    })
                },
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let pending_index = count(
            &self.conn,
            &format!(
                "SELECT count(*) FROM tasks WHERE {}",
                unindexed_sql(query.include_queued)
            ),
            [],
        )?;
        Ok(Aggregate {
            documents,
            with_total,
            without_total: documents - with_total,
            excluded_cancelled,
            pending_index,
            by_currency,
        })
    }

    /// Tarjetas de búsqueda de estas tareas (las que no están en el índice se omiten), en el
    /// orden pedido.
    pub fn search_hits(&self, ids: &[i64]) -> Result<Vec<SearchHit>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {HIT_COLUMNS}, substr(i.body, 1, {SNIPPET_CHARS})
             FROM search_index i JOIN tasks t ON t.id = i.task_id WHERE i.task_id = ?1"
        ))?;
        let mut out = Vec::new();
        for id in ids {
            if let Some(hit) = stmt.query_row([id], hit_from_row).optional()? {
                out.push(hit);
            }
        }
        Ok(out)
    }

    /// Recuento de tareas por estado.
    pub fn task_counts(&self) -> Result<Vec<TaskCounts>> {
        let mut stmt = self
            .conn
            .prepare("SELECT state, count(*) FROM tasks GROUP BY state ORDER BY state")?;
        let rows = stmt.query_map([], |r| {
            Ok(TaskCounts {
                state: r.get(0)?,
                count: usize_of(r.get(1)?),
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Las tareas de factura o correo más recientes, con el estado indicado si se da.
    pub fn recent_task_rows(&self, state: Option<&str>, limit: usize) -> Result<Vec<TaskRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT t.id, t.kind, t.state, COALESCE(i.title, ''), COALESCE(i.party, ''),
                    i.doc_date, t.updated_at
             FROM tasks t LEFT JOIN search_index i ON i.task_id = t.id
             WHERE t.kind IN ('invoice', 'email') AND (?1 IS NULL OR t.state = ?1)
             ORDER BY t.id DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(
            params![state, i64::try_from(limit).unwrap_or(i64::MAX)],
            |r| {
                Ok(TaskRow {
                    task_id: r.get(0)?,
                    kind: r.get(1)?,
                    state: r.get(2)?,
                    title: r.get(3)?,
                    party: r.get(4)?,
                    doc_date: r.get(5)?,
                    updated_at: r.get(6)?,
                })
            },
        )?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Eventos de una tarea (código, estado y fecha), del más antiguo al más reciente, como
    /// mucho `limit` (los últimos). Mezcla las transiciones de estado (`code` = `state`) y las
    /// entradas del historial con esa tarea; nunca devuelve su detalle libre.
    pub fn task_events(&self, task_id: i64, limit: usize) -> Result<Vec<TaskEvent>> {
        let mut stmt = self.conn.prepare(
            "SELECT code, status, at FROM (
                SELECT action AS code, status, at, id AS ord, 1 AS src
                    FROM audit_log WHERE task_id = ?1
                UNION ALL
                SELECT 'state', to_state, at, id, 0 FROM task_transitions WHERE task_id = ?1
                ORDER BY at DESC, src DESC, ord DESC LIMIT ?2
             ) ORDER BY at, src, ord",
        )?;
        let rows = stmt.query_map(
            params![task_id, i64::try_from(limit).unwrap_or(i64::MAX)],
            |r| {
                Ok(TaskEvent {
                    code: r.get(0)?,
                    status: r.get(1)?,
                    at: r.get(2)?,
                })
            },
        )?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    // ---- Conversaciones (D188) ----

    /// Guarda un intercambio (la pregunta, la respuesta y las llamadas que lo atendieron) en una
    /// transacción. Sin conversación, crea una. `meta` no lleva credenciales ni el texto enviado.
    pub fn assistant_save_exchange(
        &mut self,
        conversation: Option<i64>,
        user_text: &str,
        assistant_text: &str,
        meta: &Value,
        calls: &[NewCall],
    ) -> Result<SavedExchange> {
        let tx = self.conn.transaction()?;
        let now = now_ms();
        let conversation_id = match conversation {
            Some(id) => {
                let touched = tx.execute(
                    "UPDATE assistant_conversations SET updated_at = ?2 WHERE id = ?1",
                    params![id, now],
                )?;
                if touched == 0 {
                    return Err(CoreError::ConversationNotFound(id));
                }
                id
            }
            None => {
                tx.execute(
                    "INSERT INTO assistant_conversations (created_at, updated_at) VALUES (?1, ?1)",
                    [now],
                )?;
                tx.last_insert_rowid()
            }
        };
        let insert_turn = |role: &str, text: &str, meta: &str| -> rusqlite::Result<i64> {
            tx.execute(
                "INSERT INTO assistant_turns (conversation_id, role, text, meta_json, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![conversation_id, role, text, meta, now],
            )?;
            Ok(tx.last_insert_rowid())
        };
        let user_turn_id = insert_turn("user", user_text, "{}")?;
        let turn_id = insert_turn("assistant", assistant_text, &meta.to_string())?;
        for call in calls {
            tx.execute(
                "INSERT INTO assistant_calls (turn_id, step, provider, model, host, local, sent_text, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    turn_id,
                    call.step,
                    call.provider,
                    call.model,
                    call.host,
                    call.local,
                    call.sent_text,
                    now
                ],
            )?;
        }
        tx.commit()?;
        Ok(SavedExchange {
            conversation_id,
            user_turn_id,
            turn_id,
        })
    }

    fn turns_of(&self, conversation_id: i64, limit: Option<usize>) -> Result<Vec<StoredTurn>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, role, text, meta_json, created_at FROM (
                SELECT id, role, text, meta_json, created_at FROM assistant_turns
                WHERE conversation_id = ?1 ORDER BY id DESC LIMIT ?2
             ) ORDER BY id",
        )?;
        let limit = limit.map_or(i64::MAX, |l| i64::try_from(l).unwrap_or(i64::MAX));
        let rows = stmt.query_map(params![conversation_id, limit], |r| {
            let meta: String = r.get(3)?;
            Ok(StoredTurn {
                id: r.get(0)?,
                role: r.get(1)?,
                text: r.get(2)?,
                meta: serde_json::from_str(&meta).unwrap_or(Value::Null),
                created_at: r.get(4)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Los últimos `limit` turnos de una conversación, del más antiguo al más reciente.
    pub fn assistant_recent_turns(&self, id: i64, limit: usize) -> Result<Vec<StoredTurn>> {
        self.turns_of(id, Some(limit))
    }

    /// Una conversación con todos sus turnos, o `None` si no existe.
    pub fn assistant_conversation(&self, id: i64) -> Result<Option<ConversationView>> {
        let head = self
            .conn
            .query_row(
                "SELECT id, created_at, updated_at FROM assistant_conversations WHERE id = ?1",
                [id],
                |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, i64>(2)?,
                    ))
                },
            )
            .optional()?;
        let Some((id, created_at, updated_at)) = head else {
            return Ok(None);
        };
        Ok(Some(ConversationView {
            id,
            created_at,
            updated_at,
            turns: self.turns_of(id, None)?,
        }))
    }

    /// La conversación con la actividad más reciente.
    pub fn assistant_last_conversation(&self) -> Result<Option<ConversationView>> {
        let id: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM assistant_conversations ORDER BY updated_at DESC, id DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .optional()?;
        match id {
            Some(id) => self.assistant_conversation(id),
            None => Ok(None),
        }
    }

    /// Lo enviado en las llamadas de un turno del asistente, por paso.
    pub fn assistant_sent(&self, turn_id: i64) -> Result<Vec<SentCall>> {
        let mut stmt = self.conn.prepare(
            "SELECT step, provider, model, host, local, sent_text, created_at
             FROM assistant_calls WHERE turn_id = ?1 ORDER BY step, id",
        )?;
        let rows = stmt.query_map([turn_id], |r| {
            Ok(SentCall {
                step: r.get(0)?,
                provider: r.get(1)?,
                model: r.get(2)?,
                host: r.get(3)?,
                local: r.get(4)?,
                sent_text: r.get(5)?,
                at: r.get(6)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Borra una conversación con sus turnos y llamadas (en cascada). Devuelve si existía.
    pub fn assistant_delete(&mut self, id: i64) -> Result<bool> {
        self.erase(|tx| {
            Ok(tx.execute("DELETE FROM assistant_conversations WHERE id = ?1", [id])? > 0)
        })
    }

    /// Borra todas las conversaciones. Devuelve cuántas había.
    pub fn assistant_delete_all(&mut self) -> Result<usize> {
        self.erase(|tx| Ok(tx.execute("DELETE FROM assistant_conversations", [])?))
    }

    /// Ejecuta un borrado de conversaciones sin dejar el texto enviado en páginas libres ni en el
    /// WAL: `secure_delete` solo durante este borrado (activarlo en toda la conexión sobrescribiría
    /// también cada borrado del índice de búsqueda, sin necesidad) y después un `wal_checkpoint`
    /// que vacía el WAL. El punto de control es de mejor esfuerzo: si hay un lector, el borrado ya
    /// está hecho y el WAL se vaciará en el siguiente.
    fn erase<T>(
        &mut self,
        delete: impl FnOnce(&rusqlite::Transaction<'_>) -> Result<T>,
    ) -> Result<T> {
        let previous: i64 = self
            .conn
            .query_row("PRAGMA secure_delete", [], |r| r.get(0))?;
        self.conn.pragma_update(None, "secure_delete", 1)?;
        let outcome: Result<T> = (|| {
            let tx = self.conn.transaction()?;
            let removed = delete(&tx)?;
            tx.commit()?;
            Ok(removed)
        })();
        let restored = self.conn.pragma_update(None, "secure_delete", previous);
        let removed = outcome?;
        restored?;
        let _ = self
            .conn
            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()));
        Ok(removed)
    }
}
