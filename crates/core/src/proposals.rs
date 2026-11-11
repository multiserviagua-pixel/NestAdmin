//! Propuestas del asistente del chat (D185, R34): lo que la aplicación guarda cuando el asistente
//! PREPARA una acción. Este módulo solo guarda y consulta filas de `assistant_proposals` (migración
//! 12); ejecutar la acción es de la capa de aplicación y solo ocurre al pulsar el botón de la
//! tarjeta. Nada de esto sale del equipo. Los eventos del historial que deja este módulo no llevan
//! texto del modelo ni de documentos: solo tipo, tarea y efecto.

use rusqlite::{params, OptionalExtension, Row};
use serde_json::Value;

use crate::error::Result;
use crate::store::{now_ms, Store};

/// Los quince tipos de propuesta: diez de la etapa E3 y cinco de la E4 (D191).
pub const PROPOSAL_KINDS: [&str; 15] = [
    "approve",
    "reject",
    "retry",
    "archive",
    "confirm_not_sent",
    "edit_draft",
    "fetch_mail",
    "mail_limit",
    "rename_account",
    "sender_preference",
    "accept_suggestion",
    "discard_suggestion",
    "create_rule",
    "edit_rule",
    "import",
];

/// Origen que llevan en el historial los eventos de una propuesta.
pub const CHAT_ORIGIN: &str = "chat";

/// Una propuesta guardada.
#[derive(Debug, Clone, PartialEq)]
pub struct ProposalRow {
    pub id: i64,
    pub conversation_id: Option<i64>,
    pub turn_id: Option<i64>,
    pub kind: String,
    pub task_id: Option<i64>,
    pub seen_version: Option<i64>,
    /// Huella de lo que mostró la tarjeta (véase la migración 12).
    pub seen_fp: Option<String>,
    pub args: Value,
    pub card: Value,
    pub idem_key: String,
    pub status: String,
    pub receipt: Option<Value>,
    pub created_at: i64,
    pub expires_at: i64,
    pub resolved_at: Option<i64>,
}

/// Lo necesario para guardar una propuesta nueva (siempre `pending`).
#[derive(Debug, Clone)]
pub struct NewProposal<'a> {
    pub conversation_id: Option<i64>,
    pub kind: &'a str,
    pub task_id: Option<i64>,
    pub seen_version: Option<i64>,
    pub seen_fp: &'a str,
    pub args: &'a Value,
    pub card: &'a Value,
    pub idem_key: &'a str,
    pub expires_at: i64,
}

const COLUMNS: &str = "id, conversation_id, turn_id, kind, task_id, seen_version, args_json, \
card_json, idem_key, status, receipt_json, created_at, expires_at, resolved_at, seen_fp";

fn json_of(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or(Value::Null)
}

fn from_row(r: &Row<'_>) -> rusqlite::Result<ProposalRow> {
    let receipt: Option<String> = r.get(10)?;
    Ok(ProposalRow {
        id: r.get(0)?,
        conversation_id: r.get(1)?,
        turn_id: r.get(2)?,
        kind: r.get(3)?,
        task_id: r.get(4)?,
        seen_version: r.get(5)?,
        args: json_of(&r.get::<_, String>(6)?),
        card: json_of(&r.get::<_, String>(7)?),
        idem_key: r.get(8)?,
        status: r.get(9)?,
        receipt: receipt.as_deref().map(json_of),
        created_at: r.get(11)?,
        expires_at: r.get(12)?,
        resolved_at: r.get(13)?,
        seen_fp: r.get(14)?,
    })
}

impl Store {
    /// Guarda una propuesta `pending`. La clave de idempotencia es única.
    pub fn proposal_insert(&self, new: &NewProposal<'_>) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO assistant_proposals
             (conversation_id, kind, task_id, seen_version, args_json, card_json, idem_key,
              status, created_at, expires_at, seen_fp)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'pending', ?8, ?9, ?10)",
            params![
                new.conversation_id,
                new.kind,
                new.task_id,
                new.seen_version,
                new.args.to_string(),
                new.card.to_string(),
                new.idem_key,
                now_ms(),
                new.expires_at,
                new.seen_fp
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn proposal(&self, id: i64) -> Result<Option<ProposalRow>> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {COLUMNS} FROM assistant_proposals WHERE id = ?1"),
                [id],
                from_row,
            )
            .optional()?)
    }

    /// Las propuestas con esos ids, en el orden pedido; las que no existen se omiten.
    pub fn proposals_by_ids(&self, ids: &[i64]) -> Result<Vec<ProposalRow>> {
        let mut out = Vec::new();
        for id in ids {
            if let Some(row) = self.proposal(*id)? {
                out.push(row);
            }
        }
        Ok(out)
    }

    /// Las propuestas de una conversación con su turno ya guardado, de la más antigua a la más
    /// reciente.
    pub fn proposals_of(&self, conversation_id: i64) -> Result<Vec<ProposalRow>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLUMNS} FROM assistant_proposals
             WHERE conversation_id = ?1 AND turn_id IS NOT NULL ORDER BY id"
        ))?;
        let rows = stmt.query_map([conversation_id], from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Una propuesta pendiente y sin caducar idéntica (mismo tipo, tarea, versión, huella y
    /// argumentos) de esta conversación: se reutiliza en vez de crear otra.
    pub fn proposal_identical_pending(
        &self,
        conversation_id: Option<i64>,
        kind: &str,
        task_id: Option<i64>,
        seen_version: Option<i64>,
        seen_fp: &str,
        args: &Value,
    ) -> Result<Option<i64>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id FROM assistant_proposals
                 WHERE conversation_id IS ?1 AND kind = ?2 AND task_id IS ?3
                   AND seen_version IS ?4 AND args_json = ?5 AND seen_fp IS ?7
                   AND status = 'pending' AND expires_at > ?6
                 ORDER BY id LIMIT 1",
                params![
                    conversation_id,
                    kind,
                    task_id,
                    seen_version,
                    args.to_string(),
                    now_ms(),
                    seen_fp
                ],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// Propuestas pendientes y sin caducar de una conversación (o de las del turno en curso, si
    /// aún no tiene conversación).
    pub fn proposals_pending_count(&self, conversation_id: Option<i64>) -> Result<usize> {
        let n: i64 = self.conn.query_row(
            "SELECT count(*) FROM assistant_proposals
             WHERE conversation_id IS ?1 AND status = 'pending' AND expires_at > ?2",
            params![conversation_id, now_ms()],
            |r| r.get(0),
        )?;
        Ok(usize::try_from(n).unwrap_or(0))
    }

    /// Une las propuestas de un turno a su conversación y su turno ya guardados.
    pub fn proposals_attach(&self, ids: &[i64], conversation_id: i64, turn_id: i64) -> Result<()> {
        for id in ids {
            self.conn.execute(
                "UPDATE assistant_proposals SET conversation_id = ?2, turn_id = ?3
                 WHERE id = ?1 AND turn_id IS NULL",
                params![id, conversation_id, turn_id],
            )?;
        }
        Ok(())
    }

    /// Borra las propuestas sin turno guardado (de un turno que no llegó a guardarse, o que no
    /// terminó en respuesta). Con `ids`, solo esas; sin ellos, todas las huérfanas. Devuelve
    /// cuántas.
    pub fn proposals_discard_unattached(&self, ids: Option<&[i64]>) -> Result<usize> {
        let mut removed = 0;
        match ids {
            Some(ids) => {
                for id in ids {
                    removed += self.conn.execute(
                        "DELETE FROM assistant_proposals WHERE id = ?1 AND turn_id IS NULL",
                        [id],
                    )?;
                }
            }
            None => {
                removed = self
                    .conn
                    .execute("DELETE FROM assistant_proposals WHERE turn_id IS NULL", [])?;
            }
        }
        Ok(removed)
    }

    /// Cambia el estado de una propuesta solo si sigue en uno de `from` (comparar y cambiar).
    /// Devuelve si lo cambió. Los estados finales anotan la hora; `receipt` guarda el recibo.
    pub fn proposal_transition(
        &self,
        id: i64,
        from: &[&str],
        to: &str,
        receipt: Option<&Value>,
    ) -> Result<bool> {
        let list = from
            .iter()
            .map(|s| format!("'{}'", s.replace('\'', "")))
            .collect::<Vec<_>>()
            .join(", ");
        let resolved = matches!(
            to,
            "confirmed" | "rejected" | "expired" | "stale" | "failed"
        );
        let changed = self.conn.execute(
            &format!(
                "UPDATE assistant_proposals
                 SET status = ?2, receipt_json = COALESCE(?3, receipt_json),
                     resolved_at = CASE WHEN ?4 THEN ?5 ELSE resolved_at END
                 WHERE id = ?1 AND status IN ({list})"
            ),
            params![id, to, receipt.map(Value::to_string), resolved, now_ms()],
        )?;
        Ok(changed == 1)
    }

    /// Las propuestas de un tipo en un estado (todas las conversaciones), de la más antigua a la más
    /// reciente.
    pub fn proposals_of_kind_in(&self, kind: &str, status: &str) -> Result<Vec<ProposalRow>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLUMNS} FROM assistant_proposals
             WHERE kind = ?1 AND status = ?2 ORDER BY id"
        ))?;
        let rows = stmt.query_map([kind, status], from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Las propuestas `pending` de una tarea, con su turno ya guardado.
    pub fn proposals_open_for_task(&self, task_id: i64) -> Result<Vec<ProposalRow>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLUMNS} FROM assistant_proposals
             WHERE task_id = ?1 AND status = 'pending' AND turn_id IS NOT NULL ORDER BY id"
        ))?;
        let rows = stmt.query_map([task_id], from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Evento `assistant_proposal` del historial, con origen `chat`. `detail` no lleva texto del
    /// modelo ni de documentos: solo tipo, tarea y efecto.
    pub fn proposal_audit(&self, task_id: Option<i64>, status: &str, detail: &Value) -> Result<()> {
        self.conn.execute(
            "INSERT INTO audit_log (at, task_id, action, status, authorization, detail, origin)
             VALUES (?1, ?2, 'assistant_proposal', ?3, NULL, ?4, ?5)",
            params![now_ms(), task_id, status, detail.to_string(), CHAT_ORIGIN],
        )?;
        Ok(())
    }
}
