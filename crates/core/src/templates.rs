//! Plantillas de pipeline (D148, D149), versionadas como las reglas: guardar crea una versión
//! nueva, activar y desactivar también, y archivar conserva todas las versiones. Una ejecución
//! en curso guarda su propia copia de los pasos (`pipeline_runs.steps_json`), de modo que editar
//! una plantilla no la cambia. Este módulo solo guarda: la validación del catálogo y de las
//! condiciones es de la capa de aplicación, y una plantilla nunca autoriza nada.

use rusqlite::{params, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{CoreError, Result};
use crate::store::{now_ms, Store};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskTemplate {
    pub id: String,
    pub version: i64,
    pub name: String,
    /// `{}`, `{"sender": ...}` o `{"sender_domain": ...}`.
    pub conditions: Value,
    /// Pasos del catálogo cerrado, en orden: `[{"kind": "attachment_invoices"}, {"kind": "reply"}]`.
    pub steps: Value,
    pub active: bool,
    pub archived: bool,
    pub created_at: i64,
}

const COLUMNS: &str =
    "id, version, name, conditions_json, steps_json, active, archived_at, created_at";

fn template_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<TaskTemplate> {
    let parse = |index: usize, text: String| -> rusqlite::Result<Value> {
        serde_json::from_str(&text).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(index, rusqlite::types::Type::Text, e.into())
        })
    };
    Ok(TaskTemplate {
        id: row.get(0)?,
        version: row.get(1)?,
        name: row.get(2)?,
        conditions: parse(3, row.get(3)?)?,
        steps: parse(4, row.get(4)?)?,
        active: row.get(5)?,
        archived: row.get::<_, Option<i64>>(6)?.is_some(),
        created_at: row.get(7)?,
    })
}

fn current(tx: &Transaction<'_>, id: &str) -> Result<Option<TaskTemplate>> {
    Ok(tx
        .query_row(
            &format!(
                "SELECT {COLUMNS} FROM task_templates WHERE id = ?1 ORDER BY version DESC LIMIT 1"
            ),
            [id],
            template_from_row,
        )
        .optional()?)
}

struct NewVersion<'a> {
    id: &'a str,
    version: i64,
    name: &'a str,
    conditions: &'a Value,
    steps: &'a Value,
    active: bool,
    archived: bool,
}

fn insert_version(tx: &Transaction<'_>, new: &NewVersion<'_>, event: &str) -> Result<TaskTemplate> {
    let now = now_ms();
    tx.execute(
        "INSERT INTO task_templates (id, version, name, conditions_json, steps_json, active, archived_at, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            new.id,
            new.version,
            new.name,
            serde_json::to_string(new.conditions)?,
            serde_json::to_string(new.steps)?,
            new.active,
            new.archived.then_some(now),
            now
        ],
    )?;
    // Solo el id (un identificador, no el nombre visible ni las condiciones), la versión y el tipo
    // de cambio. El id puede derivarse del nombre al crearla, como el de una regla (`rule`).
    tx.execute(
        "INSERT INTO audit_log (at, task_id, action, status, authorization, detail) VALUES (?1, NULL, 'task_template', ?2, NULL, ?3)",
        params![now, event, format!("{} v{}", new.id, new.version)],
    )?;
    Ok(TaskTemplate {
        id: new.id.to_owned(),
        version: new.version,
        name: new.name.to_owned(),
        conditions: new.conditions.clone(),
        steps: new.steps.clone(),
        active: new.active,
        archived: new.archived,
        created_at: now,
    })
}

impl Store {
    /// Guarda una plantilla: la crea (versión 1, con el estado `active` pedido) o, si ya existe,
    /// añade una versión nueva que conserva su estado activo. Una plantilla archivada no se
    /// edita. Quien llama valida antes los pasos y las condiciones.
    pub fn save_task_template(
        &mut self,
        id: &str,
        name: &str,
        conditions: &Value,
        steps: &Value,
        active_if_new: bool,
    ) -> Result<TaskTemplate> {
        let tx = self.conn.transaction()?;
        let saved = match current(&tx, id)? {
            None => insert_version(
                &tx,
                &NewVersion {
                    id,
                    version: 1,
                    name,
                    conditions,
                    steps,
                    active: active_if_new,
                    archived: false,
                },
                "created",
            )?,
            Some(old) if old.archived => return Err(CoreError::TemplateArchived(id.to_owned())),
            Some(old) => insert_version(
                &tx,
                &NewVersion {
                    id,
                    version: old.version + 1,
                    name,
                    conditions,
                    steps,
                    active: old.active,
                    archived: false,
                },
                "edited",
            )?,
        };
        tx.commit()?;
        Ok(saved)
    }

    /// Nueva versión con el estado indicado; si ya lo tiene, no crea versión. Una plantilla
    /// archivada no se activa.
    pub fn set_task_template_active(&mut self, id: &str, active: bool) -> Result<TaskTemplate> {
        let tx = self.conn.transaction()?;
        let old = current(&tx, id)?.ok_or_else(|| CoreError::TemplateNotFound(id.to_owned()))?;
        if old.archived && active {
            return Err(CoreError::TemplateArchived(id.to_owned()));
        }
        if old.active == active {
            return Ok(old);
        }
        let saved = insert_version(
            &tx,
            &NewVersion {
                id,
                version: old.version + 1,
                name: &old.name,
                conditions: &old.conditions,
                steps: &old.steps,
                active,
                archived: false,
            },
            if active { "activated" } else { "deactivated" },
        )?;
        tx.commit()?;
        Ok(saved)
    }

    /// Archiva (o desarchiva) una plantilla sin borrar ninguna versión: crea una versión nueva,
    /// siempre inactiva. Las ejecuciones en curso conservan sus pasos.
    pub fn set_task_template_archived(&mut self, id: &str, archived: bool) -> Result<TaskTemplate> {
        let tx = self.conn.transaction()?;
        let old = current(&tx, id)?.ok_or_else(|| CoreError::TemplateNotFound(id.to_owned()))?;
        if old.archived == archived {
            return Ok(old);
        }
        let saved = insert_version(
            &tx,
            &NewVersion {
                id,
                version: old.version + 1,
                name: &old.name,
                conditions: &old.conditions,
                steps: &old.steps,
                active: false,
                archived,
            },
            if archived { "archived" } else { "unarchived" },
        )?;
        tx.commit()?;
        Ok(saved)
    }

    /// Versión vigente de cada plantilla, archivadas incluidas, por identificador.
    pub fn task_templates(&self) -> Result<Vec<TaskTemplate>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLUMNS} FROM task_templates t
             WHERE version = (SELECT MAX(version) FROM task_templates WHERE id = t.id) ORDER BY id"
        ))?;
        let rows = stmt.query_map([], template_from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Todas las versiones de una plantilla, de la primera a la vigente.
    pub fn task_template_history(&self, id: &str) -> Result<Vec<TaskTemplate>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLUMNS} FROM task_templates WHERE id = ?1 ORDER BY version"
        ))?;
        let rows = stmt.query_map([id], template_from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
}

/// Instante (ms) desde el que una plantilla está activa de forma continua: la versión que pasó
/// a activa tras la última inactiva. Editar una plantilla activa no lo cambia. `None` si la
/// versión vigente no está activa. `history` va de la primera versión a la vigente.
pub fn active_since(history: &[TaskTemplate]) -> Option<i64> {
    let mut since = None;
    for version in history {
        match (version.active && !version.archived, since) {
            (true, None) => since = Some(version.created_at),
            (false, _) => since = None,
            _ => {}
        }
    }
    since
}

impl Store {
    /// Todas las versiones de todas las plantillas (para nombres por versión), de la primera a
    /// la última de cada una.
    pub fn task_template_versions(&self) -> Result<Vec<TaskTemplate>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLUMNS} FROM task_templates ORDER BY id, version"
        ))?;
        let rows = stmt.query_map([], template_from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
}
