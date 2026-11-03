//! Acciones con efectos externos (enviar, registrar) ejecutadas una sola vez por intención.
//!
//! La clave de idempotencia se persiste como `pending` antes de invocar al conector. Si el
//! proceso se interrumpe entre la invocación y la confirmación, la clave pendiente permite
//! preguntar al conector si la acción ya ocurrió antes de repetirla (contratos.md).

use rusqlite::{params, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{CoreError, Result};
use crate::store::{now_ms, Store};

/// Error devuelto por un conector, con los códigos de contratos.md.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("{code}: {message}")]
pub struct ConnectorError {
    pub code: String,
    pub message: String,
}

/// Conector de correo o administración visto desde el núcleo.
pub trait Connector {
    /// Devuelve el recibo de una acción ya ejecutada con esta clave, si existe.
    fn lookup(&self, key: &str) -> std::result::Result<Option<Value>, ConnectorError>;
    /// Ejecuta la acción. Reutilizar la clave no debe repetir el efecto.
    fn execute(&self, key: &str, payload: &Value) -> std::result::Result<Value, ConnectorError>;
}

/// Motivo de autorización que queda en el historial.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Authorization {
    Approval,
    Rule { rule_id: String, rule_version: i64 },
}

impl Authorization {
    fn label(&self) -> String {
        match self {
            Self::Approval => "aprobación".to_owned(),
            Self::Rule {
                rule_id,
                rule_version,
            } => format!("regla {rule_id} v{rule_version}"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ActionRequest {
    pub task_id: i64,
    pub kind: String,
    /// Versión del contenido aprobado; si cambia, es otra intención y otra clave.
    pub version: i64,
    pub payload: Value,
    pub authorization: Authorization,
}

impl ActionRequest {
    pub fn idempotency_key(&self) -> String {
        idempotency_key(self.task_id, &self.kind, self.version)
    }
}

pub fn idempotency_key(task_id: i64, kind: &str, version: i64) -> String {
    format!("task-{task_id}/{kind}/v{version}")
}

#[derive(Debug, Clone, PartialEq)]
pub enum ActionOutcome {
    /// El conector ejecutó la acción en esta llamada.
    Executed(Value),
    /// Ya estaba confirmada localmente; no se llamó al conector.
    AlreadyConfirmed(Value),
    /// Estaba pendiente y el conector confirmó que ya había ocurrido.
    Reconciled(Value),
}

impl ActionOutcome {
    pub fn receipt(&self) -> &Value {
        match self {
            Self::Executed(r) | Self::AlreadyConfirmed(r) | Self::Reconciled(r) => r,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditEntry {
    pub task_id: Option<i64>,
    pub action: String,
    pub status: String,
    pub authorization: Option<String>,
    pub detail: Option<String>,
}

fn audit(
    tx: &Transaction<'_>,
    task_id: i64,
    action: &str,
    status: &str,
    authorization: &str,
    detail: &str,
) -> Result<()> {
    tx.execute(
        "INSERT INTO audit_log (at, task_id, action, status, authorization, detail) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![now_ms(), task_id, action, status, authorization, detail],
    )?;
    Ok(())
}

impl Store {
    pub fn perform_action(
        &mut self,
        request: &ActionRequest,
        connector: &dyn Connector,
    ) -> Result<ActionOutcome> {
        let key = request.idempotency_key();
        let payload = serde_json::to_string(&request.payload)?;
        let authorization = request.authorization.label();

        let tx = self.conn.transaction()?;
        let existing: Option<(String, String, Option<String>)> = tx
            .query_row(
                "SELECT payload, status, receipt FROM actions WHERE idempotency_key = ?1",
                [&key],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let previously_started = match &existing {
            Some((stored, _, _)) if *stored != payload => {
                return Err(CoreError::IdempotencyConflict(key))
            }
            Some((_, status, Some(receipt))) if status == "confirmed" => {
                tx.commit()?;
                return Ok(ActionOutcome::AlreadyConfirmed(serde_json::from_str(
                    receipt,
                )?));
            }
            Some(_) => true,
            None => {
                let now = now_ms();
                tx.execute(
                    "INSERT INTO actions (idempotency_key, task_id, kind, version, payload, authorization, status, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'pending', ?7, ?7)",
                    params![key, request.task_id, request.kind, request.version, payload, authorization, now],
                )?;
                audit(
                    &tx,
                    request.task_id,
                    &request.kind,
                    "pending",
                    &authorization,
                    &key,
                )?;
                false
            }
        };
        tx.commit()?;

        if previously_started {
            match connector.lookup(&key) {
                Ok(Some(receipt)) => {
                    self.confirm(request, &key, &authorization, &receipt, "reconciled")?;
                    return Ok(ActionOutcome::Reconciled(receipt));
                }
                Ok(None) => {}
                Err(error) => return self.record_failure(request, &key, &authorization, error),
            }
        }
        match connector.execute(&key, &request.payload) {
            Ok(receipt) => {
                self.confirm(request, &key, &authorization, &receipt, "confirmed")?;
                Ok(ActionOutcome::Executed(receipt))
            }
            Err(error) => self.record_failure(request, &key, &authorization, error),
        }
    }

    /// Tras una interrupción, confirma las acciones pendientes que el conector ya ejecutó.
    /// Las que no constan siguen pendientes y se reintentan al reprocesar su tarea.
    pub fn reconcile_actions(
        &mut self,
        kind: &str,
        connector: &dyn Connector,
    ) -> Result<Vec<(String, bool)>> {
        let pending: Vec<(String, i64, String)> = {
            let mut stmt = self.conn.prepare(
                "SELECT idempotency_key, task_id, authorization FROM actions WHERE kind = ?1 AND status = 'pending' ORDER BY created_at",
            )?;
            let rows = stmt.query_map([kind], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        let mut outcome = Vec::with_capacity(pending.len());
        for (key, task_id, authorization) in pending {
            let found = match connector.lookup(&key) {
                Ok(Some(receipt)) => {
                    let tx = self.conn.transaction()?;
                    tx.execute(
                        "UPDATE actions SET status = 'confirmed', receipt = ?2, error = NULL, updated_at = ?3 WHERE idempotency_key = ?1",
                        params![key, serde_json::to_string(&receipt)?, now_ms()],
                    )?;
                    audit(&tx, task_id, kind, "reconciled", &authorization, &key)?;
                    tx.commit()?;
                    true
                }
                // Sin respuesta del conector no se puede afirmar nada: la acción sigue pendiente.
                Ok(None) | Err(_) => false,
            };
            outcome.push((key, found));
        }
        Ok(outcome)
    }

    pub fn action_status(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT status FROM actions WHERE idempotency_key = ?1",
                [key],
                |r| r.get(0),
            )
            .optional()?)
    }

    pub fn audit_log(&self) -> Result<Vec<AuditEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT task_id, action, status, authorization, detail FROM audit_log ORDER BY id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(AuditEntry {
                task_id: r.get(0)?,
                action: r.get(1)?,
                status: r.get(2)?,
                authorization: r.get(3)?,
                detail: r.get(4)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    fn confirm(
        &mut self,
        request: &ActionRequest,
        key: &str,
        authorization: &str,
        receipt: &Value,
        status: &str,
    ) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "UPDATE actions SET status = 'confirmed', receipt = ?2, error = NULL, updated_at = ?3 WHERE idempotency_key = ?1",
            params![key, serde_json::to_string(receipt)?, now_ms()],
        )?;
        audit(
            &tx,
            request.task_id,
            &request.kind,
            status,
            authorization,
            key,
        )?;
        tx.commit()?;
        Ok(())
    }

    fn record_failure(
        &mut self,
        request: &ActionRequest,
        key: &str,
        authorization: &str,
        error: ConnectorError,
    ) -> Result<ActionOutcome> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "UPDATE actions SET status = 'failed', error = ?2, updated_at = ?3 WHERE idempotency_key = ?1",
            params![key, serde_json::to_string(&error)?, now_ms()],
        )?;
        audit(
            &tx,
            request.task_id,
            &request.kind,
            "failed",
            authorization,
            &format!("{key}: {}", error.code),
        )?;
        tx.commit()?;
        Err(CoreError::Connector(error))
    }
}
