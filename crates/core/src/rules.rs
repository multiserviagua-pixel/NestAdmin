//! Reglas expresas de automatización (R07, contratos.md). Cada cambio de condiciones o de
//! estado crea una versión nueva y las anteriores se conservan, de modo que el historial
//! puede citar la versión exacta que autorizó una acción. La evaluación de las condiciones
//! corresponde a quien conoce los datos validados; aquí solo se guardan.

use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{CoreError, Result};
use crate::store::{now_ms, Store};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Rule {
    pub id: String,
    pub version: i64,
    pub action_kind: String,
    pub conditions: Value,
    pub active: bool,
    pub created_at: i64,
}

impl Store {
    /// Crea la versión 1 de una regla, inactiva: activarla es otra decisión explícita.
    pub fn create_rule(&mut self, id: &str, action_kind: &str, conditions: &Value) -> Result<Rule> {
        self.create_rule_named(id, action_kind, conditions, None, false)
    }

    /// Como `create_rule`, con nombre visible (D124) y, si `any_sender_ack`, la validación
    /// avanzada sin remitente (D126), todo en una transacción.
    pub fn create_rule_named(
        &mut self,
        id: &str,
        action_kind: &str,
        conditions: &Value,
        name: Option<&str>,
        any_sender_ack: bool,
    ) -> Result<Rule> {
        let tx = self.conn.transaction()?;
        if current_rule(&tx, id)?.is_some() {
            return Err(CoreError::IdempotencyConflict(format!("regla {id}")));
        }
        let rule = insert_rule_version(&tx, id, 1, action_kind, conditions, false)?;
        if name.is_some() || any_sender_ack {
            tx.execute(
                "INSERT INTO rule_meta (rule_id, name, allow_any_sender_ack_at) VALUES (?1, ?2, ?3)",
                params![id, name, any_sender_ack.then_some(rule.created_at)],
            )?;
        }
        if any_sender_ack {
            audit_any_sender(&tx, &rule, true, "validada al crear la regla")?;
        }
        tx.commit()?;
        Ok(rule)
    }

    /// Nueva versión con otras condiciones; conserva el estado activo de la anterior. Una
    /// regla archivada no se edita (D128). Cambiar las condiciones borra la validación
    /// avanzada sin remitente, que se dio a las condiciones anteriores (D126). Para editar
    /// con la comprobación de D186, véase `update_rule_guarded`.
    pub fn update_rule(&mut self, id: &str, conditions: &Value) -> Result<Rule> {
        Ok(self.update_rule_guarded(id, conditions, &|_, _| true)?.0)
    }

    /// Como `update_rule`, pero una regla activa solo sigue activa si `keep_active(anterior,
    /// tenía_validación_avanzada)` lo permite (D186c). Si no, la versión nueva nace inactiva en
    /// la misma transacción y la entrada del historial lo dice. Devuelve la regla guardada y si
    /// quedó desactivada por la edición. La condición se evalúa dentro de la transacción, sobre
    /// la versión vigente leída en ella.
    pub fn update_rule_guarded(
        &mut self,
        id: &str,
        conditions: &Value,
        keep_active: &dyn Fn(&Rule, bool) -> bool,
    ) -> Result<(Rule, bool)> {
        let tx = self.conn.transaction()?;
        let current = live_rule(&tx, id)?;
        let (active, deactivated) = decide_active(&tx, &current, keep_active)?;
        let rule = insert_rule_version_noting(
            &tx,
            id,
            current.version + 1,
            &current.action_kind,
            conditions,
            active,
            deactivated.then_some(DEACTIVATED_BY_EDIT),
        )?;
        let had_ack = tx.execute(
            "UPDATE rule_meta SET allow_any_sender_ack_at = NULL
             WHERE rule_id = ?1 AND allow_any_sender_ack_at IS NOT NULL",
            [id],
        )?;
        if had_ack > 0 {
            audit_any_sender(&tx, &rule, false, "retirada: las condiciones cambiaron")?;
        }
        tx.commit()?;
        Ok((rule, deactivated))
    }

    /// Nueva versión con el estado indicado; si ya lo tiene, no crea versión. Una regla
    /// archivada no se activa (D128).
    pub fn set_rule_active(&mut self, id: &str, active: bool) -> Result<Rule> {
        let tx = self.conn.transaction()?;
        let current = if active {
            live_rule(&tx, id)?
        } else {
            current_rule(&tx, id)?.ok_or_else(|| CoreError::RuleNotFound(id.to_owned()))?
        };
        if current.active == active {
            return Ok(current);
        }
        let rule = insert_rule_version(
            &tx,
            id,
            current.version + 1,
            &current.action_kind,
            &current.conditions,
            active,
        )?;
        tx.commit()?;
        Ok(rule)
    }

    /// Versión vigente (la más reciente) de una regla.
    pub fn rule(&self, id: &str) -> Result<Option<Rule>> {
        current_rule(&self.conn, id)
    }

    /// Versiones vigentes de todas las reglas, por identificador.
    pub fn rules(&self) -> Result<Vec<Rule>> {
        let mut stmt = self.conn.prepare(
            "SELECT r.id, r.version, r.action_kind, r.conditions, r.active, r.created_at FROM rules r
             WHERE r.version = (SELECT MAX(version) FROM rules WHERE id = r.id) ORDER BY r.id",
        )?;
        let rows = stmt.query_map([], rule_from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Todas las versiones de una regla, de la primera a la vigente.
    pub fn rule_history(&self, id: &str) -> Result<Vec<Rule>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, version, action_kind, conditions, active, created_at FROM rules
             WHERE id = ?1 ORDER BY version",
        )?;
        let rows = stmt.query_map([id], rule_from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
}

fn rule_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Rule> {
    let conditions: String = row.get(3)?;
    Ok(Rule {
        id: row.get(0)?,
        version: row.get(1)?,
        action_kind: row.get(2)?,
        conditions: serde_json::from_str(&conditions).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(3, rusqlite::types::Type::Text, e.into())
        })?,
        active: row.get(4)?,
        created_at: row.get(5)?,
    })
}

/// Versión vigente de una regla.
pub(crate) fn current_rule(conn: &rusqlite::Connection, id: &str) -> Result<Option<Rule>> {
    Ok(conn
        .query_row(
            "SELECT id, version, action_kind, conditions, active, created_at FROM rules
             WHERE id = ?1 ORDER BY version DESC LIMIT 1",
            [id],
            rule_from_row,
        )
        .optional()?)
}

/// Versión vigente de una regla que existe y no está archivada.
pub(crate) fn live_rule(conn: &rusqlite::Connection, id: &str) -> Result<Rule> {
    let rule = current_rule(conn, id)?.ok_or_else(|| CoreError::RuleNotFound(id.to_owned()))?;
    if is_archived(conn, id)? {
        return Err(CoreError::RuleArchived(id.to_owned()));
    }
    Ok(rule)
}

pub(crate) fn is_archived(conn: &rusqlite::Connection, id: &str) -> Result<bool> {
    Ok(conn
        .query_row(
            "SELECT archived_at IS NOT NULL FROM rule_meta WHERE rule_id = ?1",
            [id],
            |r| r.get::<_, bool>(0),
        )
        .optional()?
        .unwrap_or(false))
}

/// Nota del historial cuando editar una regla activa la deja inactiva (D186c).
pub(crate) const DEACTIVATED_BY_EDIT: &str =
    "desactivada al editarla: el cambio amplía lo que haría sin revisión";

/// Decide el estado de la versión nueva al editar: una regla inactiva sigue inactiva; una
/// activa conserva el estado solo si `keep_active` lo permite. Devuelve `(activa, desactivada)`.
pub(crate) fn decide_active(
    tx: &rusqlite::Transaction<'_>,
    current: &Rule,
    keep_active: &dyn Fn(&Rule, bool) -> bool,
) -> Result<(bool, bool)> {
    if !current.active {
        return Ok((false, false));
    }
    let had_ack = tx
        .query_row(
            "SELECT allow_any_sender_ack_at IS NOT NULL FROM rule_meta WHERE rule_id = ?1",
            [&current.id],
            |r| r.get::<_, bool>(0),
        )
        .optional()?
        .unwrap_or(false);
    let keep = keep_active(current, had_ack);
    Ok((keep, !keep))
}

/// Inserta una versión y la deja en el historial visible.
pub(crate) fn insert_rule_version(
    tx: &rusqlite::Transaction<'_>,
    id: &str,
    version: i64,
    action_kind: &str,
    conditions: &Value,
    active: bool,
) -> Result<Rule> {
    insert_rule_version_noting(tx, id, version, action_kind, conditions, active, None)
}

/// Como `insert_rule_version`, con una nota opcional al final del detalle del historial.
pub(crate) fn insert_rule_version_noting(
    tx: &rusqlite::Transaction<'_>,
    id: &str,
    version: i64,
    action_kind: &str,
    conditions: &Value,
    active: bool,
    note: Option<&str>,
) -> Result<Rule> {
    let now = now_ms();
    tx.execute(
        "INSERT INTO rules (id, version, action_kind, conditions, active, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![id, version, action_kind, serde_json::to_string(conditions)?, active, now],
    )?;
    let detail = match note {
        Some(note) => format!("{id} v{version} ({action_kind}): {note}"),
        None => format!("{id} v{version} ({action_kind})"),
    };
    tx.execute(
        "INSERT INTO audit_log (at, task_id, action, status, authorization, detail) VALUES (?1, NULL, 'rule', ?2, NULL, ?3)",
        params![now, if active { "active" } else { "inactive" }, detail],
    )?;
    Ok(Rule {
        id: id.to_owned(),
        version,
        action_kind: action_kind.to_owned(),
        conditions: conditions.clone(),
        active,
        created_at: now,
    })
}

/// La validación avanzada sin remitente queda siempre en el historial, con su motivo (D126).
pub(crate) fn audit_any_sender(
    tx: &rusqlite::Transaction<'_>,
    rule: &Rule,
    acknowledged: bool,
    reason: &str,
) -> Result<()> {
    tx.execute(
        "INSERT INTO audit_log (at, task_id, action, status, authorization, detail) VALUES (?1, NULL, 'rule_any_sender', ?2, NULL, ?3)",
        params![
            now_ms(),
            if acknowledged { "acknowledged" } else { "revoked" },
            format!("{} v{}: {reason}", rule.id, rule.version)
        ],
    )?;
    Ok(())
}
