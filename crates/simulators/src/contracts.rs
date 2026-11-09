//! Contratos de correo y administración de contratos.md como traits. Quien consulta correo,
//! crea borradores o busca facturas depende de estos traits y no de los simuladores, de modo
//! que un conector real los sustituye sin cambiar a sus llamadores (R06). Ambos extienden el
//! `Connector` del núcleo, que ejecuta y reconcilia por clave las acciones con efectos.

use nestadmin_core::{Connector, ConnectorError};

use crate::admin::{Invoice, InvoiceCriteria, InvoiceMatch, InvoiceReceipt};
use crate::mail::{MessagePage, NewDraft, OwnReply, ReceivedExtra, SendReceipt};

/// Tamaño máximo de un adjunto que se descarga (igual que el de un documento importado).
pub const MAX_ATTACHMENT_BYTES: u64 = 50_000_000;

/// Contrato de correo. Los errores usan los códigos de [`crate::codes`].
pub trait MailConnector: Connector {
    /// Página de mensajes en orden estable; `cursor` es el devuelto por la página anterior.
    fn list_messages(&self, cursor: Option<&str>) -> Result<MessagePage, ConnectorError>;

    /// Crea un borrador de respuesta; repetir la clave con el mismo contenido devuelve el
    /// mismo ID y con otro contenido, `conflict`.
    fn create_draft(
        &self,
        message_id: &str,
        draft: &NewDraft,
        idempotency_key: &str,
    ) -> Result<String, ConnectorError>;

    /// Contenido de un adjunto de [`Message::attachments`](crate::Message). `not_found` si
    /// el mensaje o el adjunto ya no existen; `invalid_input` si supera
    /// [`MAX_ATTACHMENT_BYTES`]. No tiene efectos en el buzón.
    fn fetch_attachment(
        &self,
        message_id: &str,
        attachment_id: &str,
    ) -> Result<Vec<u8>, ConnectorError>;

    /// Si el último mensaje del hilo es propio (D162). Solo mira metadatos del hilo (quién y
    /// cuándo), nunca su contenido, y no tiene efectos en el buzón. `Ok(None)` = no se sabe (el
    /// proveedor no lo permite o el hilo no consta): quien llama sigue el flujo normal y no oculta
    /// nada. Por defecto, desconocido.
    fn last_message_is_ours(&self, _thread_key: &str) -> Result<Option<bool>, ConnectorError> {
        Ok(None)
    }

    /// Si el conector sabe leer la carpeta Enviados (D168). Por defecto no: así se distingue
    /// «sin Enviados» de «sin respuesta propia» (`own_reply_in_thread` devuelve `Ok(None)` en
    /// ambos casos).
    fn supports_sent(&self) -> bool {
        false
    }

    /// Primera respuesta propia (Enviados) del hilo `thread_key` posterior al mensaje recibido
    /// `after_id` (ID del conector, sin prefijo de cuenta). Cuerpo en texto plano tal como llega,
    /// con la cita. Solo lectura. `Ok(None)` = no hay respuesta o el conector no lee Enviados
    /// (consulte [`supports_sent`](Self::supports_sent)). Por defecto, sin Enviados.
    fn own_reply_in_thread(
        &self,
        _thread_key: &str,
        _after_id: &str,
    ) -> Result<Option<OwnReply>, ConnectorError> {
        Ok(None)
    }

    /// Datos extra de un mensaje recibido que solo hacen falta para aprender (D168, D169): la
    /// parte del cuerpo que no es cita del hilo (`uniqueBody` de Graph) y el nombre visible del
    /// remitente. Solo lectura. Por defecto, nada: quien llama usa el cuerpo del mensaje.
    fn received_extra(&self, _message_id: &str) -> Result<ReceivedExtra, ConnectorError> {
        Ok(ReceivedExtra::default())
    }

    /// Envía un borrador una sola vez; repetir la clave devuelve el recibo original.
    fn send_draft(
        &self,
        draft_id: &str,
        idempotency_key: &str,
    ) -> Result<SendReceipt, ConnectorError>;
}

/// Contrato del sistema administrativo. Los errores usan los códigos de [`crate::codes`].
pub trait AdminConnector: Connector {
    /// Candidatos a duplicado con los motivos de coincidencia y las diferencias; no decide.
    fn find_invoices(
        &self,
        criteria: &InvoiceCriteria,
    ) -> Result<Vec<InvoiceMatch>, ConnectorError>;

    /// Registra una factura una sola vez por clave; repetirla devuelve el recibo original.
    fn register_invoice(
        &self,
        invoice: &Invoice,
        idempotency_key: &str,
    ) -> Result<InvoiceReceipt, ConnectorError>;
}
