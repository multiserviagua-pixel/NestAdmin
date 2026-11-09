//! Preferencias por remitente (D161). Solo guardan el ámbito (dirección o dominio), la clave
//! normalizada y la ruta; cada cambio deja un evento del historial en la misma transacción, sin
//! contenido de ningún correo.

use rusqlite::{params, OptionalExtension};
use serde::Serialize;
use serde_json::json;

use crate::error::Result;
use crate::mail_signals::{sender_parts, PreferenceRoute};
use crate::store::{now_ms, Store};

/// Ámbito de una preferencia.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PreferenceScope {
    Address,
    Domain,
}

impl PreferenceScope {
    pub fn as_str(self) -> &'static str {
        match self {
            PreferenceScope::Address => "address",
            PreferenceScope::Domain => "domain",
        }
    }

    pub fn from_code(code: &str) -> Option<PreferenceScope> {
        match code {
            "address" => Some(PreferenceScope::Address),
            "domain" => Some(PreferenceScope::Domain),
            _ => None,
        }
    }
}

/// Una preferencia guardada.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SenderPreference {
    pub scope: String,
    pub key: String,
    pub route: String,
    pub updated_at: i64,
}

/// Clave normalizada en minúsculas: de `Nombre <a@b>` o `a@b` la dirección completa; para un
/// dominio, `b`, `@b` o cualquier dirección de `b`. `None` si no es una dirección o un dominio
/// válidos.
pub fn normalize_key(scope: PreferenceScope, input: &str) -> Option<String> {
    let input = input.trim();
    match scope {
        PreferenceScope::Address => sender_parts(input).map(|(address, _)| address),
        PreferenceScope::Domain => {
            if input.contains('@') && !input.starts_with('@') {
                return sender_parts(input).map(|(_, domain)| domain);
            }
            sender_parts(&format!("x@{}", input.trim_start_matches('@'))).map(|(_, d)| d)
        }
    }
}

impl Store {
    /// Todas las preferencias, en orden estable (ámbito y clave).
    pub fn sender_preferences(&self) -> Result<Vec<SenderPreference>> {
        let mut stmt = self.conn.prepare(
            "SELECT scope, key, route, updated_at FROM sender_preferences ORDER BY scope, key",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(SenderPreference {
                scope: r.get(0)?,
                key: r.get(1)?,
                route: r.get(2)?,
                updated_at: r.get(3)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Fija o cambia una preferencia con su evento del historial, en una transacción. La clave
    /// debe venir de [`normalize_key`]. Devuelve la ruta anterior, si la había. Repetir lo mismo
    /// no cambia nada ni escribe otro evento.
    pub fn set_sender_preference(
        &mut self,
        scope: PreferenceScope,
        key: &str,
        route: PreferenceRoute,
    ) -> Result<Option<PreferenceRoute>> {
        let tx = self.conn.transaction()?;
        let previous: Option<PreferenceRoute> = tx
            .query_row(
                "SELECT route FROM sender_preferences WHERE scope = ?1 AND key = ?2",
                params![scope.as_str(), key],
                |r| r.get::<_, String>(0),
            )
            .optional()?
            .and_then(|code| PreferenceRoute::from_code(&code));
        if previous == Some(route) {
            return Ok(previous);
        }
        tx.execute(
            "INSERT INTO sender_preferences (scope, key, route, updated_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(scope, key) DO UPDATE SET route = excluded.route, updated_at = excluded.updated_at",
            params![scope.as_str(), key, route.as_str(), now_ms()],
        )?;
        let detail = json!({
            "scope": scope.as_str(),
            "key": key,
            "route": route.as_str(),
            "previous": previous.map(PreferenceRoute::as_str),
        });
        tx.execute(
            "INSERT INTO audit_log (at, task_id, action, status, authorization, detail) VALUES (?1, NULL, 'sender_preference', 'set', NULL, ?2)",
            params![now_ms(), detail.to_string()],
        )?;
        tx.commit()?;
        Ok(previous)
    }

    /// Quita una preferencia con su evento, en una transacción. Devuelve si existía; si no
    /// existía no escribe nada.
    pub fn remove_sender_preference(&mut self, scope: PreferenceScope, key: &str) -> Result<bool> {
        let tx = self.conn.transaction()?;
        let previous: Option<String> = tx
            .query_row(
                "SELECT route FROM sender_preferences WHERE scope = ?1 AND key = ?2",
                params![scope.as_str(), key],
                |r| r.get(0),
            )
            .optional()?;
        let Some(previous) = previous else {
            return Ok(false);
        };
        tx.execute(
            "DELETE FROM sender_preferences WHERE scope = ?1 AND key = ?2",
            params![scope.as_str(), key],
        )?;
        let detail = json!({ "scope": scope.as_str(), "key": key, "previous": previous });
        tx.execute(
            "INSERT INTO audit_log (at, task_id, action, status, authorization, detail) VALUES (?1, NULL, 'sender_preference', 'removed', NULL, ?2)",
            params![now_ms(), detail.to_string()],
        )?;
        tx.commit()?;
        Ok(true)
    }
}
