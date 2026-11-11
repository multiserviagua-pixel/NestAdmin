//! Lógica común a todos los proveedores: borradores idempotentes por clave, envío único y
//! reconciliación por el ID del borrador (D33; en Gmail, también por el hilo y la cabecera
//! `X-NestAdmin-Id`, D41). Cada
//! proveedor solo implementa [`MailProvider`].

use std::path::Path;

use nestadmin_core::{Connector, ConnectorError};
use nestadmin_simulators::{
    codes, MailConnector, MessagePage, NewDraft, OwnReply, OwnReplyMeta, ReceivedExtra, SendReceipt,
};
use serde_json::Value;

use crate::state::{DraftRecord, LocalState, SendRecord};
use crate::{error, MailResult};

/// Borrador creado en el proveedor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatedDraft {
    pub draft_id: String,
    /// ID del mensaje del borrador cuando no coincide con el del borrador (Gmail).
    pub message_id: Option<String>,
    /// Identificador propio que NestAdmin puso al borrador (cabeceras `X-NestAdmin-Id` y
    /// `Message-ID`), para encontrarlo una vez enviado aunque el proveedor le haya dado otro
    /// ID (Gmail).
    pub rfc822_id: Option<String>,
    /// Hilo del borrador (Gmail): Gmail cambia el `Message-ID` y el ID al enviar, pero el
    /// mensaje enviado sigue en el hilo y conserva `X-NestAdmin-Id`.
    pub thread_id: Option<String>,
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
    /// Crea en el proveedor un mensaje nuevo, fuera de hilo, con su adjunto si lo hay (D230). El
    /// contenido ya está validado. Por defecto, `unavailable`: un proveedor que no lo implementa
    /// no finge crear el borrador.
    fn create_message(&self, _draft: &NewDraft) -> MailResult<CreatedDraft> {
        Err(error(
            codes::UNAVAILABLE,
            "este proveedor no sabe crear mensajes nuevos",
        ))
    }
    /// Envía el borrador y devuelve el ID del mensaje enviado.
    fn send(&self, draft_id: &str) -> MailResult<String>;
    /// Si el borrador ya no existe ni consta como enviado, `not_found`: no se reenvía.
    fn status(&self, draft: &CreatedDraft) -> MailResult<DraftStatus>;
    /// Contenido de un adjunto anunciado en la lista de mensajes.
    fn attachment(&self, message_id: &str, attachment_id: &str) -> MailResult<Vec<u8>>;
    /// Si el último mensaje del hilo es propio (D162), solo con metadatos. Por defecto, `None`
    /// (desconocido): un proveedor sin una llamada segura no inventa la respuesta.
    fn last_message_is_ours(&self, _thread_key: &str) -> MailResult<Option<bool>> {
        Ok(None)
    }
    /// Si el proveedor lee la carpeta Enviados (D168). Por defecto no: permite distinguir
    /// «conector sin Enviados» de «sin respuesta propia», pues ambos dan `Ok(None)`.
    fn supports_sent(&self) -> bool {
        false
    }
    /// Primera respuesta propia (Enviados) del hilo `thread_key` posterior al mensaje `after_id`
    /// (D168), con el cuerpo en texto plano tal como llega. Solo lectura. Por defecto,
    /// `Ok(None)` sin llamar a nadie.
    fn own_reply_in_thread(
        &self,
        _thread_key: &str,
        _after_id: &str,
    ) -> MailResult<Option<OwnReply>> {
        Ok(None)
    }
    /// Metadatos (id, fecha y destinatarios) de todas las respuestas propias del hilo posteriores
    /// a `after_id`, por fecha (D178). La petición **no pide ningún cuerpo**. Por defecto, lista
    /// vacía sin llamar a nadie.
    fn own_replies_meta(
        &self,
        _thread_key: &str,
        _after_id: &str,
    ) -> MailResult<Vec<OwnReplyMeta>> {
        Ok(Vec::new())
    }
    /// Hilo de un mensaje recibido, solo con metadatos (D178). Un mensaje que ya no existe es
    /// `not_found`. Por defecto, `Ok(None)` sin llamar a nadie.
    fn thread_of(&self, _message_id: &str) -> MailResult<Option<String>> {
        Ok(None)
    }
    /// Cuerpo sin cita y nombre visible del remitente de un mensaje recibido (solo para
    /// aprender, D169). Por defecto, nada.
    fn received_extra(&self, _message_id: &str) -> MailResult<ReceivedExtra> {
        Ok(ReceivedExtra::default())
    }
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
    if !draft.attachments.is_empty() {
        return Err(error(
            codes::INVALID_INPUT,
            "una respuesta en el hilo no admite adjuntos",
        ));
    }
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
            if !record.standalone && record.message_id == message_id && record.content == *draft {
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
            thread_id: created.thread_id,
            standalone: false,
        };
        self.state.mutate(|d| d.drafts.push(record))?;
        Ok(created.draft_id)
    }

    /// Mensaje nuevo (D229, D230). Valida todo antes de llamar al proveedor (`invalid_input`);
    /// repetir la clave con el mismo destinatario, asunto, cuerpo y metadatos del adjunto
    /// devuelve el mismo borrador sin crear otro, y con otro contenido, `conflict`. El registro
    /// local guarda solo los metadatos del adjunto, nunca sus bytes.
    fn create_new_draft(
        &self,
        draft: &NewDraft,
        idempotency_key: &str,
    ) -> Result<String, ConnectorError> {
        check_key(idempotency_key)?;
        nestadmin_simulators::validate_new_message(draft)?;
        let existing = self
            .state
            .read(|d| d.drafts.iter().find(|r| r.key == idempotency_key).cloned())?;
        if let Some(record) = existing {
            if record.standalone && record.content == *draft {
                return Ok(record.draft_id);
            }
            return Err(error(
                codes::CONFLICT,
                format!("la clave {idempotency_key} ya se usó con otro contenido"),
            ));
        }
        let created = self.provider.create_message(draft)?;
        let mut content = draft.clone();
        for attachment in &mut content.attachments {
            attachment.content.clear();
        }
        let record = DraftRecord {
            key: idempotency_key.to_owned(),
            message_id: String::new(),
            content,
            draft_id: created.draft_id.clone(),
            provider_message_id: created.message_id,
            rfc822_id: created.rfc822_id,
            thread_id: created.thread_id,
            standalone: true,
        };
        self.state.mutate(|d| d.drafts.push(record))?;
        Ok(created.draft_id)
    }

    /// Solo lectura de metadatos: no anota nada en el registro local ni en el buzón.
    fn last_message_is_ours(&self, thread_key: &str) -> Result<Option<bool>, ConnectorError> {
        if thread_key.trim().is_empty() {
            return Ok(None);
        }
        self.provider.last_message_is_ours(thread_key)
    }

    fn supports_sent(&self) -> bool {
        self.provider.supports_sent()
    }

    /// Solo lectura. Claves vacías o un proveedor sin Enviados no llaman al servicio.
    fn own_reply_in_thread(
        &self,
        thread_key: &str,
        after_id: &str,
    ) -> Result<Option<OwnReply>, ConnectorError> {
        if thread_key.trim().is_empty() || after_id.trim().is_empty() {
            return Ok(None);
        }
        self.provider.own_reply_in_thread(thread_key, after_id)
    }

    /// Solo lectura y solo metadatos. Claves vacías no llaman al servicio.
    fn own_replies_meta(
        &self,
        thread_key: &str,
        after_id: &str,
    ) -> Result<Vec<OwnReplyMeta>, ConnectorError> {
        if thread_key.trim().is_empty() || after_id.trim().is_empty() {
            return Ok(Vec::new());
        }
        self.provider.own_replies_meta(thread_key, after_id)
    }

    /// Solo lectura y solo metadatos. Un identificador vacío no llama al servicio.
    fn thread_of(&self, message_id: &str) -> Result<Option<String>, ConnectorError> {
        if message_id.trim().is_empty() {
            return Ok(None);
        }
        self.provider.thread_of(message_id)
    }

    /// Solo lectura. Un identificador vacío no llama al servicio.
    fn received_extra(&self, message_id: &str) -> Result<ReceivedExtra, ConnectorError> {
        if message_id.trim().is_empty() {
            return Ok(ReceivedExtra::default());
        }
        self.provider.received_extra(message_id)
    }

    /// Solo lectura: no anota nada en el registro local ni en el buzón.
    fn fetch_attachment(
        &self,
        message_id: &str,
        attachment_id: &str,
    ) -> Result<Vec<u8>, ConnectorError> {
        if attachment_id.trim().is_empty() {
            return Err(error(
                codes::INVALID_INPUT,
                "falta el identificador del adjunto",
            ));
        }
        let content = self.provider.attachment(message_id, attachment_id)?;
        crate::check_attachment_size(content.len() as u64)?;
        Ok(content)
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Proveedor mínimo que no implementa Enviados: usa los valores por defecto (D168).
    struct NoSent;

    impl MailProvider for NoSent {
        fn list(&self, _: Option<&str>) -> MailResult<MessagePage> {
            unreachable!()
        }
        fn create_reply(&self, _: &str, _: &NewDraft) -> MailResult<CreatedDraft> {
            unreachable!()
        }
        fn send(&self, _: &str) -> MailResult<String> {
            unreachable!()
        }
        fn status(&self, _: &CreatedDraft) -> MailResult<DraftStatus> {
            unreachable!()
        }
        fn attachment(&self, _: &str, _: &str) -> MailResult<Vec<u8>> {
            unreachable!()
        }
    }

    #[test]
    fn a_provider_without_sent_support_says_so_and_finds_nothing() {
        let mailbox = Mailbox::in_memory(NoSent);
        assert!(!MailConnector::supports_sent(&mailbox));
        assert_eq!(
            MailConnector::own_reply_in_thread(&mailbox, "hilo-1", "m-1").unwrap(),
            None
        );
    }
}
