//! Contratos de correo y administración de contratos.md como traits. Quien consulta correo,
//! crea borradores o busca facturas depende de estos traits y no de los simuladores, de modo
//! que un conector real los sustituye sin cambiar a sus llamadores (R06). Ambos extienden el
//! `Connector` del núcleo, que ejecuta y reconcilia por clave las acciones con efectos.

use nestadmin_core::{Connector, ConnectorError};

use crate::admin::{Invoice, InvoiceCriteria, InvoiceMatch, InvoiceReceipt};
use crate::mail::{MessagePage, NewDraft, SendReceipt};

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
