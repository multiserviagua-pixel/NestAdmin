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
        if self.rule(id)?.is_some() {
            return Err(CoreError::IdempotencyConflict(format!("regla {id}")));
        }
        self.insert_rule_version(id, 1, action_kind, conditions, false)
    }

    /// Nueva versión con otras condiciones; conserva el estado activo de la anterior.
    pub fn update_rule(&mut self, id: &str, conditions: &Value) -> Result<Rule> {
        let current = self
            .rule(id)?
            .ok_or_else(|| CoreError::RuleNotFound(id.to_owned()))?;
        self.insert_rule_version(
            id,
            current.version + 1,
            &current.action_kind,
            conditions,
            current.active,
        )
    }

    /// Nueva versión con el estado indicado; si ya lo tiene, no crea versión.
    pub fn set_rule_active(&mut self, id: &str, active: bool) -> Result<Rule> {
        let current = self
            .rule(id)?
            .ok_or_else(|| CoreError::RuleNotFound(id.to_owned()))?;
        if current.active == active {
            return Ok(current);
        }
        self.insert_rule_version(
            id,
            current.version + 1,
            &current.action_kind,
            &current.conditions,
            active,
        )
    }

    /// Versión vigente (la más reciente) de una regla.
    pub fn rule(&self, id: &str) -> Result<Option<Rule>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id, version, action_kind, conditions, active, created_at FROM rules
                 WHERE id = ?1 ORDER BY version DESC LIMIT 1",
                [id],
                rule_from_row,
            )
            .optional()?)
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

    fn insert_rule_version(
        &mut self,
        id: &str,
        version: i64,
        action_kind: &str,
        conditions: &Value,
        active: bool,
    ) -> Result<Rule> {
        let tx = self.conn.transaction()?;
        let now = now_ms();
        tx.execute(
            "INSERT INTO rules (id, version, action_kind, conditions, active, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![id, version, action_kind, serde_json::to_string(conditions)?, active, now],
        )?;
        tx.execute(
            "INSERT INTO audit_log (at, task_id, action, status, authorization, detail) VALUES (?1, NULL, 'rule', ?2, NULL, ?3)",
            params![now, if active { "active" } else { "inactive" }, format!("{id} v{version} ({action_kind})")],
        )?;
        tx.commit()?;
        Ok(Rule {
            id: id.to_owned(),
            version,
            action_kind: action_kind.to_owned(),
            conditions: conditions.clone(),
            active,
            created_at: now,
        })
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
