//! Lógica común a todos los proveedores: borradores idempotentes por clave, envío único y
//! reconciliación por el ID del borrador (D33; en Gmail, también por `Message-ID`, D41). Cada
//! proveedor solo implementa [`MailProvider`].

use std::path::Path;

use nestadmin_core::{Connector, ConnectorError};
use nestadmin_simulators::{codes, MailConnector, MessagePage, NewDraft, SendReceipt};
use serde_json::Value;

use crate::state::{DraftRecord, LocalState, SendRecord};
use crate::{error, MailResult};

/// Borrador creado en el proveedor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatedDraft {
    pub draft_id: String,
    /// ID del mensaje del borrador cuando no coincide con el del borrador (Gmail).
    pub message_id: Option<String>,
    /// Cabecera `Message-ID` que NestAdmin puso al borrador, para encontrarlo una vez enviado
    /// aunque el proveedor le haya dado otro ID (Gmail).
    pub rfc822_id: Option<String>,
}

/// Estado de un borrador en el proveedor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DraftStatus {
    /// Sigue siendo borrador: aún no se ha enviado.
    Draft,
    /// Ya se envió; lleva el ID del mensaje enviado.
    Sent(String),
}

/// Llamadas propias de cada proveedor.
pub trait MailProvider {
    fn list(&self, cursor: Option<&str>) -> MailResult<MessagePage>;
    /// Crea en el proveedor una respuesta en el hilo del mensaje original.
    fn create_reply(&self, message_id: &str, draft: &NewDraft) -> MailResult<CreatedDraft>;
    /// Envía el borrador y devuelve el ID del mensaje enviado.
    fn send(&self, draft_id: &str) -> MailResult<String>;
    /// Si el borrador ya no existe ni consta como enviado, `not_found`: no se reenvía.
    fn status(&self, draft: &CreatedDraft) -> MailResult<DraftStatus>;
}

/// Una cuenta de correo con su proveedor y su registro local.
pub struct Mailbox<P> {
    provider: P,
    state: LocalState,
}

impl<P: MailProvider> Mailbox<P> {
    /// El registro local se guarda en `state_path` (un archivo JSON por cuenta).
    pub fn open(provider: P, state_path: &Path) -> MailResult<Self> {
        Ok(Self {
            provider,
            state: LocalState::open(state_path)?,
        })
    }

    pub fn in_memory(provider: P) -> Self {
        Self {
            provider,
            state: LocalState::in_memory(),
        }
    }

    pub fn provider(&self) -> &P {
        &self.provider
    }

    fn draft(&self, draft_id: &str) -> MailResult<DraftRecord> {
        self.state
            .read(|d| d.drafts.iter().find(|r| r.draft_id == draft_id).cloned())?
            .ok_or_else(|| {
                error(
                    codes::NOT_FOUND,
                    format!("el borrador {draft_id} no lo creó NestAdmin en esta cuenta"),
                )
            })
    }

    fn receipt(record: &DraftRecord, key: &str, sent_id: String) -> SendReceipt {
        SendReceipt {
            receipt_id: sent_id,
            draft_id: record.draft_id.clone(),
            message_id: record.message_id.clone(),
            idempotency_key: key.to_owned(),
        }
    }

    fn confirm(&self, receipt: &SendReceipt) -> MailResult<()> {
        self.state.mutate(|d| {
            if let Some(send) = d
                .sends
                .iter_mut()
                .find(|s| s.key == receipt.idempotency_key)
            {
                send.receipt = Some(receipt.clone());
            }
        })
    }

    /// Pregunta al proveedor por un envío pendiente. `None` si el borrador sigue sin enviar.
    fn reconcile(&self, send: &SendRecord) -> MailResult<Option<SendReceipt>> {
        let record = self.draft(&send.draft_id)?;
        match self.provider.status(&record.created())? {
            DraftStatus::Draft => Ok(None),
            DraftStatus::Sent(sent_id) => {
                let receipt = Self::receipt(&record, &send.key, sent_id);
                self.confirm(&receipt)?;
                Ok(Some(receipt))
            }
        }
    }

    fn send_record(&self, key: &str) -> MailResult<Option<SendRecord>> {
        self.state
            .read(|d| d.sends.iter().find(|s| s.key == key).cloned())
    }
}

fn check_key(key: &str) -> MailResult<()> {
    if key.trim().is_empty() {
        return Err(error(
            codes::INVALID_INPUT,
            "falta la clave de idempotencia",
        ));
    }
    Ok(())
}

fn validate_draft(draft: &NewDraft) -> MailResult<()> {
    if draft.to.is_empty() {
        return Err(error(
            codes::INVALID_INPUT,
            "el borrador no tiene destinatarios",
        ));
    }
    if let Some(bad) = draft.to.iter().find(|a| !crate::text::is_address(a)) {
        return Err(error(
            codes::INVALID_INPUT,
            format!("destinatario no válido: {bad}"),
        ));
    }
    if draft.body.trim().is_empty() {
        return Err(error(codes::INVALID_INPUT, "el borrador está vacío"));
    }
    Ok(())
}

impl<P: MailProvider> MailConnector for Mailbox<P> {
    fn list_messages(&self, cursor: Option<&str>) -> Result<MessagePage, ConnectorError> {
        self.provider.list(cursor)
    }

    /// Repetir la clave con el mismo contenido devuelve el mismo borrador sin crear otro.
    fn create_draft(
        &self,
        message_id: &str,
        draft: &NewDraft,
        idempotency_key: &str,
    ) -> Result<String, ConnectorError> {
        check_key(idempotency_key)?;
        validate_draft(draft)?;
        let existing = self
            .state
            .read(|d| d.drafts.iter().find(|r| r.key == idempotency_key).cloned())?;
        if let Some(record) = existing {
            if record.message_id == message_id && record.content == *draft {
                return Ok(record.draft_id);
            }
            return Err(error(
                codes::CONFLICT,
                format!("la clave {idempotency_key} ya se usó con otro contenido"),
            ));
        }
        let created = self.provider.create_reply(message_id, draft)?;
        let record = DraftRecord {
            key: idempotency_key.to_owned(),
            message_id: message_id.to_owned(),
            content: draft.clone(),
            draft_id: created.draft_id.clone(),
            provider_message_id: created.message_id,
            rfc822_id: created.rfc822_id,
        };
        self.state.mutate(|d| d.drafts.push(record))?;
        Ok(created.draft_id)
    }

    /// Envía una sola vez por clave. El envío queda anotado como pendiente antes de llamar al
    /// proveedor; si el proceso se corta, la siguiente llamada consulta el borrador antes de
    /// repetir nada.
    fn send_draft(
        &self,
        draft_id: &str,
        idempotency_key: &str,
    ) -> Result<SendReceipt, ConnectorError> {
        check_key(idempotency_key)?;
        if let Some(send) = self.send_record(idempotency_key)? {
            if send.draft_id != draft_id {
                return Err(error(
                    codes::CONFLICT,
                    format!("la clave {idempotency_key} ya se usó con otro borrador"),
                ));
            }
            if let Some(receipt) = send.receipt {
                return Ok(receipt);
            }
            if let Some(receipt) = self.reconcile(&send)? {
                return Ok(receipt);
            }
        } else {
            let record = self.draft(draft_id)?;
            let other = self.state.read(|d| {
                d.sends
                    .iter()
                    .any(|s| s.draft_id == record.draft_id && s.key != idempotency_key)
            })?;
            if other {
                return Err(error(
                    codes::CONFLICT,
                    format!("el borrador {draft_id} ya tiene un envío con otra clave"),
                ));
            }
            self.state.mutate(|d| {
                d.sends.push(SendRecord {
                    key: idempotency_key.to_owned(),
                    draft_id: draft_id.to_owned(),
                    receipt: None,
                })
            })?;
        }
        let record = self.draft(draft_id)?;
        let sent_id = self.provider.send(&record.draft_id)?;
        let receipt = Self::receipt(&record, idempotency_key, sent_id);
        self.confirm(&receipt)?;
        Ok(receipt)
    }
}

/// Acción `send_draft` vista desde el núcleo; la carga útil es `{"draft_id": "..."}`.
impl<P: MailProvider> Connector for Mailbox<P> {
    fn lookup(&self, key: &str) -> Result<Option<Value>, ConnectorError> {
        let Some(send) = self.send_record(key)? else {
            return Ok(None);
        };
        let receipt = match send.receipt.clone() {
            Some(receipt) => Some(receipt),
            None => self.reconcile(&send)?,
        };
        receipt
            .map(|r| serde_json::to_value(r).map_err(|e| error(codes::UNAVAILABLE, e.to_string())))
            .transpose()
    }

    fn execute(&self, key: &str, payload: &Value) -> Result<Value, ConnectorError> {
        let draft_id = payload
            .get("draft_id")
            .and_then(Value::as_str)
            .ok_or_else(|| error(codes::INVALID_INPUT, "falta draft_id en la carga útil"))?;
        serde_json::to_value(self.send_draft(draft_id, key)?)
            .map_err(|e| error(codes::UNAVAILABLE, e.to_string()))
    }
}
