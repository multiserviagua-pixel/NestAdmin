//! Envío de documentos de venta por correo (E4b, D232–D239, migración 15): lo que guarda la base.
//! El correo y el PDF son de la capa de aplicación; aquí solo hay tres cosas.
//!
//! * El historial de envíos (`sales_document_sends`), de solo inserción: una fila por correo que el
//!   conector confirmó, con el destinatario, el asunto, el nombre, el tamaño y el SHA-256 del
//!   adjunto, quién lo autorizó y el recibo. Nunca el cuerpo ni el PDF.
//! * El cierre de la tarea de envío (`complete_sales_send`): en UNA transacción escribe la fila con
//!   `INSERT OR IGNORE` por clave de idempotencia, pasa el documento de `issued` a `sent` (solo
//!   desde `issued`) y completa la tarea. Repetirlo, por ejemplo al recuperar un corte, no duplica
//!   la fila ni cambia otra vez el estado. Si la transición de estado falla, la tarea se completa
//!   igualmente con `status_error` en su resultado: el correo ya salió y eso no se deshace.
//! * Las reglas de cola de las tareas de envío: una sola tarea abierta por documento, un solo
//!   disparo de la regla por cliente al emitir y la anulación (D82, D235): una factura con un
//!   intento de envío sin cerrar no se anula (`send_in_progress`); si la tarea solo está en
//!   revisión, anular la cancela.

use nestadmin_sales::{transition, DocumentKind, Status};
use rusqlite::{params, Transaction, TransactionBehavior};
use serde::Serialize;
use serde_json::{json, Value};

use super::documents::{load_document, touch_status};
use super::{audit, SalesResult, SalesStoreError};
use crate::error::CoreError;
use crate::queue::{self, Task, TaskState};
use crate::store::{now_ms, Store};

/// Tipo de tarea y de acción del envío de un documento de venta.
pub const SEND_SALES_DOCUMENT: &str = "send_sales_document";
/// Código del error cuando ya hay un envío abierto o con un intento sin cerrar.
pub const CODE_SEND_IN_PROGRESS: &str = "send_in_progress";

/// Referencia de entrada de la tarea de envío de un documento.
pub fn send_input_ref(document_id: i64) -> String {
    format!("sales_document:{document_id}")
}

/// Estados desde los que se puede enviar un documento por correo (D233): presupuesto `issued` y
/// `sent`; albarán también `delivered`; factura, simplificada y rectificativa `issued` y `sent`.
/// Un borrador, un documento anulado, aceptado, rechazado, caducado, convertido o facturado no.
pub fn is_sendable(kind: DocumentKind, status: Status) -> bool {
    match kind {
        DocumentKind::DeliveryNote => {
            matches!(status, Status::Issued | Status::Sent | Status::Delivered)
        }
        _ => matches!(status, Status::Issued | Status::Sent),
    }
}

/// Un envío confirmado, tal como se guarda.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SalesSend {
    pub id: i64,
    pub document_id: i64,
    pub task_id: i64,
    pub version: i64,
    pub idempotency_key: String,
    pub account: String,
    pub account_address: Option<String>,
    pub recipient: String,
    pub subject: String,
    pub attachment_name: String,
    pub attachment_size: i64,
    pub attachment_sha256: String,
    pub authorization: String,
    pub sent_message_id: Option<String>,
    pub sent_at: i64,
}

/// Lo que se escribe al confirmarse el recibo; `id` y `sent_at` los pone la base.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewSalesSend {
    pub document_id: i64,
    pub task_id: i64,
    pub version: i64,
    pub idempotency_key: String,
    pub account: String,
    pub account_address: Option<String>,
    pub recipient: String,
    pub subject: String,
    pub attachment_name: String,
    pub attachment_size: i64,
    pub attachment_sha256: String,
    pub authorization: String,
    pub sent_message_id: Option<String>,
}

/// Resultado de cerrar una tarea de envío.
#[derive(Debug, Clone, PartialEq)]
pub struct SendCompletion {
    pub task: Task,
    /// `false` si la fila ya existía (cierre repetido): no se tocó nada más.
    pub inserted: bool,
    /// El documento pasó de `issued` a `sent` en esta llamada.
    pub status_changed: bool,
    /// Por qué no pudo pasar a `sent` estando `issued` (la tarea se completó igualmente).
    pub status_error: Option<String>,
}

fn core(error: CoreError) -> SalesStoreError {
    match error {
        CoreError::Storage(e) => SalesStoreError::Storage(e),
        other => SalesStoreError::Corrupt(other.to_string()),
    }
}

fn in_progress(message: impl Into<String>) -> SalesStoreError {
    SalesStoreError::External {
        code: CODE_SEND_IN_PROGRESS.to_owned(),
        message: message.into(),
    }
}

/// Estados de una tarea de envío que sigue abierta: puede acabar enviando.
const OPEN_STATES: &str = "('queued', 'running', 'awaiting_review', 'failed')";

fn open_task_ids(tx: &Transaction<'_>, input_ref: &str) -> SalesResult<Vec<i64>> {
    let mut stmt = tx.prepare(&format!(
        "SELECT id FROM tasks WHERE kind = ?1 AND input_ref = ?2 AND state IN {OPEN_STATES} \
         ORDER BY id"
    ))?;
    let rows = stmt.query_map(params![SEND_SALES_DOCUMENT, input_ref], |r| r.get(0))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Anulación (D82, D235), dentro de la transacción de `void_sales_invoice_with`. Una acción de
/// envío sin cerrar (`pending` o `failed`: pudo salir o no) o confirmada cuya tarea aún no se ha
/// cerrado bloquea con `send_in_progress`. Si no, las tareas de envío que solo esperan revisión
/// se cancelan: la factura anulada ya no se envía.
pub(crate) fn guard_void(tx: &Transaction<'_>, document_id: i64) -> SalesResult<()> {
    let input_ref = send_input_ref(document_id);
    let unsettled: i64 = tx.query_row(
        "SELECT count(*) FROM actions a JOIN tasks t ON t.id = a.task_id \
         WHERE t.kind = ?1 AND t.input_ref = ?2 AND a.kind = ?1 AND \
               (a.status IN ('pending', 'failed') \
                OR (a.status = 'confirmed' AND t.state NOT IN ('completed', 'cancelled')))",
        params![SEND_SALES_DOCUMENT, input_ref],
        |r| r.get(0),
    )?;
    if unsettled > 0 {
        return Err(in_progress(
            "hay un envío por correo de esta factura sin cerrar: confirme si salió o descártelo \
             antes de anularla",
        ));
    }
    for id in open_task_ids(tx, &input_ref)? {
        queue::transition(
            tx,
            id,
            TaskState::Cancelled,
            "factura anulada: ya no se envía",
            None,
            None,
        )
        .map_err(core)?;
    }
    Ok(())
}

impl Store {
    /// Crea la tarea de envío de un documento, ya pendiente de revisión, en una transacción. Una
    /// sola tarea abierta por documento: otra da `send_in_progress`. Con `on_issue` es el disparo
    /// de la regla por cliente (D238): a lo sumo uno por documento; si ya hubo, devuelve `None`
    /// sin crear nada.
    pub fn enqueue_sales_send(
        &mut self,
        document_id: i64,
        result: &Value,
        reason: &str,
        on_issue: bool,
    ) -> SalesResult<Option<Task>> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        load_document(&tx, document_id)?;
        if on_issue {
            let fired: i64 = tx.query_row(
                "SELECT count(*) FROM sales_send_triggers WHERE document_id = ?1",
                [document_id],
                |r| r.get(0),
            )?;
            if fired > 0 {
                return Ok(None);
            }
        }
        let input_ref = send_input_ref(document_id);
        if !open_task_ids(&tx, &input_ref)?.is_empty() {
            return Err(in_progress(
                "este documento ya tiene un envío por correo abierto: termínelo o descártelo",
            ));
        }
        let task = queue::insert_born(
            &tx,
            SEND_SALES_DOCUMENT,
            &input_ref,
            TaskState::AwaitingReview,
            result,
            reason,
        )
        .map_err(core)?;
        if on_issue {
            tx.execute(
                "INSERT INTO sales_send_triggers (document_id, task_id, created_at) \
                 VALUES (?1, ?2, ?3)",
                params![document_id, task.id, now_ms()],
            )?;
        }
        tx.commit()?;
        Ok(Some(task))
    }

    /// La tarea de envío abierta de un documento, si la hay.
    pub fn open_sales_send_task(&self, document_id: i64) -> SalesResult<Option<Task>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {} FROM tasks WHERE kind = ?1 AND input_ref = ?2 AND state IN {OPEN_STATES} \
             ORDER BY id DESC LIMIT 1",
            queue::TASK_COLUMNS
        ))?;
        let mut rows = stmt.query(params![SEND_SALES_DOCUMENT, send_input_ref(document_id)])?;
        match rows.next()? {
            Some(row) => Ok(Some(queue::task_from_row(row)?)),
            None => Ok(None),
        }
    }

    /// Cierra una tarea de envío con el recibo ya confirmado (D233, D239): escribe la fila del
    /// historial (`INSERT OR IGNORE` por clave), pasa el documento de `issued` a `sent` si lo
    /// estaba y completa la tarea con `result`, todo en una transacción. Es idempotente: con la
    /// fila ya escrita no cambia el estado del documento ni duplica nada, y una tarea ya
    /// completada se deja como está.
    pub fn complete_sales_send(
        &mut self,
        task_id: i64,
        result: &Value,
        send: &NewSalesSend,
    ) -> SalesResult<SendCompletion> {
        let mut tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let task = queue::load(&tx, task_id).map_err(core)?;
        let inserted = tx.execute(
            "INSERT OR IGNORE INTO sales_document_sends (document_id, task_id, version, \
             idempotency_key, account, account_address, recipient, subject, attachment_name, \
             attachment_size, attachment_sha256, authorization, sent_message_id, sent_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            params![
                send.document_id,
                send.task_id,
                send.version,
                send.idempotency_key,
                send.account,
                send.account_address,
                send.recipient,
                send.subject,
                send.attachment_name,
                send.attachment_size,
                send.attachment_sha256,
                send.authorization,
                send.sent_message_id,
                now_ms(),
            ],
        )? == 1;
        let mut result = result.clone();
        let mut status_changed = false;
        let mut status_error = None;
        if inserted {
            let doc = load_document(&tx, send.document_id)?;
            audit(
                &tx,
                "sales.send",
                &json!({
                    "id": doc.id, "number": doc.number, "revision": doc.revision,
                    "task": task_id, "version": send.version,
                    "authorization": send.authorization,
                }),
            )?;
            if doc.status == Status::Issued {
                let sp = tx.savepoint()?;
                let outcome = (|| -> SalesResult<()> {
                    transition(doc.kind, Status::Issued, Status::Sent)?;
                    touch_status(&sp, doc.id, Status::Issued, Status::Sent)?;
                    audit(
                        &sp,
                        "sales.transition",
                        &json!({
                            "id": doc.id, "number": doc.number, "revision": doc.revision,
                            "from": "issued", "to": "sent",
                        }),
                    )?;
                    Ok(())
                })();
                match outcome {
                    Ok(()) => {
                        sp.commit()?;
                        status_changed = true;
                    }
                    // El correo ya salió: se descarta solo la transición y se deja constancia.
                    Err(error) => {
                        drop(sp);
                        status_error = Some(error.to_string());
                    }
                }
            }
        }
        if let Some(why) = &status_error {
            result["status_error"] = json!(why);
        }
        let task = if task.state.can_move_to(TaskState::Completed) {
            queue::transition(
                &tx,
                task_id,
                TaskState::Completed,
                "completada",
                Some(&result),
                None,
            )
            .map_err(core)?
        } else {
            task
        };
        tx.commit()?;
        Ok(SendCompletion {
            task,
            inserted,
            status_changed,
            status_error,
        })
    }

    /// Envíos confirmados de un documento, del más antiguo al más reciente.
    pub fn sales_document_sends(&self, document_id: i64) -> SalesResult<Vec<SalesSend>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, document_id, task_id, version, idempotency_key, account, account_address, \
             recipient, subject, attachment_name, attachment_size, attachment_sha256, \
             authorization, sent_message_id, sent_at \
             FROM sales_document_sends WHERE document_id = ?1 ORDER BY sent_at, id",
        )?;
        let rows = stmt.query_map([document_id], |r| {
            Ok(SalesSend {
                id: r.get(0)?,
                document_id: r.get(1)?,
                task_id: r.get(2)?,
                version: r.get(3)?,
                idempotency_key: r.get(4)?,
                account: r.get(5)?,
                account_address: r.get(6)?,
                recipient: r.get(7)?,
                subject: r.get(8)?,
                attachment_name: r.get(9)?,
                attachment_size: r.get(10)?,
                attachment_sha256: r.get(11)?,
                authorization: r.get(12)?,
                sent_message_id: r.get(13)?,
                sent_at: r.get(14)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
}
