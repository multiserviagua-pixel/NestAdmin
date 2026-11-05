//! Simulador de correo: mensajes de ejemplo, borradores y envíos con idempotencia.
//! No envía nada fuera del proceso; un "envío" es un recibo guardado en su estado.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use nestadmin_core::{Connector, ConnectorError};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::contracts::MailConnector;
use crate::state::SimState;
use crate::{check_key, codes, error, FailureMode, SimResult};

/// Mensaje recibido. Los campos ausentes son `None`, nunca texto inventado.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    pub subject: Option<String>,
    pub from: Option<String>,
    /// `None` si se desconocen los destinatarios; nunca una lista vacía en su lugar.
    pub to: Option<Vec<String>>,
    pub body: Option<String>,
    pub date: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessagePage {
    pub messages: Vec<Message>,
    /// Cursor opaco para la página siguiente; `None` si no hay más.
    pub next_cursor: Option<String>,
}

/// Contenido de un borrador solicitado. Destinatarios y cuerpo son obligatorios porque un
/// borrador sin ellos no se puede enviar; el asunto puede faltar.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewDraft {
    pub to: Vec<String>,
    pub subject: Option<String>,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Draft {
    pub id: String,
    pub message_id: String,
    pub content: NewDraft,
    /// Fecha de creación en UTC (`AAAA-MM-DDTHH:MM:SSZ`); `None` si el reloj no la da.
    pub date: Option<String>,
    pub idempotency_key: String,
    /// Recibo del envío, si ya se envió.
    pub sent_receipt: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SendReceipt {
    pub receipt_id: String,
    pub draft_id: String,
    pub message_id: String,
    pub idempotency_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct MailState {
    messages: Vec<Message>,
    drafts: Vec<Draft>,
    sent: Vec<SendReceipt>,
}

pub struct MailSimulator {
    state: SimState<MailState>,
    page_size: usize,
}

const DEFAULT_PAGE_SIZE: usize = 20;

impl MailSimulator {
    /// Simulador en memoria con los mensajes de ejemplo.
    pub fn in_memory() -> Self {
        Self::with_messages(sample_messages())
    }

    /// Simulador en memoria con mensajes propios (sintéticos).
    pub fn with_messages(messages: Vec<Message>) -> Self {
        Self {
            state: SimState::in_memory(MailState::new(messages)),
            page_size: DEFAULT_PAGE_SIZE,
        }
    }

    /// Abre o crea el archivo de estado; si no existe, parte de los mensajes de ejemplo. Un
    /// estado creado por una versión anterior recibe los ejemplos que le falten y el remitente
    /// sintético de los ejemplos que no lo tenían (D48); los mensajes cambiados no se tocan.
    pub fn open(path: &Path) -> SimResult<Self> {
        let simulator = Self {
            state: SimState::open(path, || MailState::new(sample_messages()))?,
            page_size: DEFAULT_PAGE_SIZE,
        };
        simulator.state.mutate(|s| Ok(((), s.refresh_samples())))?;
        Ok(simulator)
    }

    pub fn with_page_size(mut self, page_size: usize) -> Self {
        self.page_size = page_size.max(1);
        self
    }

    pub fn set_failure(&self, failure: Option<FailureMode>) {
        self.state.set_failure(failure);
    }

    pub fn drafts(&self) -> SimResult<Vec<Draft>> {
        self.state.read(|s| Ok(s.drafts.clone()))
    }

    /// Envíos realizados, para comprobar que no hay duplicados.
    pub fn sent(&self) -> SimResult<Vec<SendReceipt>> {
        self.state.read(|s| Ok(s.sent.clone()))
    }
}

impl MailConnector for MailSimulator {
    /// Lista mensajes en orden estable. El cursor es el ID del último mensaje entregado,
    /// así que añadir mensajes nuevos no desplaza las páginas ya leídas.
    fn list_messages(&self, cursor: Option<&str>) -> SimResult<MessagePage> {
        let page_size = self.page_size;
        self.state.read(|s| {
            let start = match cursor {
                None => 0,
                Some(c) => {
                    s.messages
                        .iter()
                        .position(|m| m.id == c)
                        .ok_or_else(|| error(codes::INVALID_INPUT, "cursor desconocido"))?
                        + 1
                }
            };
            let messages: Vec<Message> = s
                .messages
                .iter()
                .skip(start)
                .take(page_size)
                .cloned()
                .collect();
            let next_cursor = if start + messages.len() < s.messages.len() {
                messages.last().map(|m| m.id.clone())
            } else {
                None
            };
            Ok(MessagePage {
                messages,
                next_cursor,
            })
        })
    }

    /// Crea un borrador de respuesta. Repetir la clave con el mismo contenido devuelve el
    /// mismo ID; con otro contenido, `conflict`.
    fn create_draft(
        &self,
        message_id: &str,
        draft: &NewDraft,
        idempotency_key: &str,
    ) -> SimResult<String> {
        check_key(idempotency_key)?;
        validate_draft(draft)?;
        self.state.mutate(|s| {
            if let Some(existing) = s
                .drafts
                .iter()
                .find(|d| d.idempotency_key == idempotency_key)
            {
                if existing.message_id == message_id && existing.content == *draft {
                    return Ok((existing.id.clone(), false));
                }
                return Err(conflict(idempotency_key));
            }
            if !s.messages.iter().any(|m| m.id == message_id) {
                return Err(error(
                    codes::NOT_FOUND,
                    format!("mensaje {message_id} inexistente"),
                ));
            }
            let id = format!("D-{:04}", s.drafts.len() + 1);
            s.drafts.push(Draft {
                id: id.clone(),
                message_id: message_id.to_owned(),
                content: draft.clone(),
                date: utc_now(),
                idempotency_key: idempotency_key.to_owned(),
                sent_receipt: None,
            });
            Ok((id, true))
        })
    }

    /// Envía un borrador una sola vez. La misma clave para el mismo borrador devuelve el
    /// recibo original; otra clave para un borrador ya enviado es `conflict`.
    fn send_draft(&self, draft_id: &str, idempotency_key: &str) -> SimResult<SendReceipt> {
        check_key(idempotency_key)?;
        self.state.mutate(|s| {
            if let Some(receipt) = s.sent.iter().find(|r| r.idempotency_key == idempotency_key) {
                if receipt.draft_id == draft_id {
                    return Ok((receipt.clone(), false));
                }
                return Err(conflict(idempotency_key));
            }
            let receipt_id = format!("S-{:04}", s.sent.len() + 1);
            let draft = s
                .drafts
                .iter_mut()
                .find(|d| d.id == draft_id)
                .ok_or_else(|| {
                    error(codes::NOT_FOUND, format!("borrador {draft_id} inexistente"))
                })?;
            if draft.sent_receipt.is_some() {
                return Err(error(
                    codes::CONFLICT,
                    format!("el borrador {draft_id} ya se envió con otra clave"),
                ));
            }
            draft.sent_receipt = Some(receipt_id.clone());
            let receipt = SendReceipt {
                receipt_id,
                draft_id: draft_id.to_owned(),
                message_id: draft.message_id.clone(),
                idempotency_key: idempotency_key.to_owned(),
            };
            s.sent.push(receipt.clone());
            Ok((receipt, true))
        })
    }
}

/// Acción `send_draft` vista desde el núcleo; la carga útil es `{"draft_id": "..."}`.
impl Connector for MailSimulator {
    fn lookup(&self, key: &str) -> Result<Option<Value>, ConnectorError> {
        self.state.read(|s| {
            s.sent
                .iter()
                .find(|r| r.idempotency_key == key)
                .map(to_value)
                .transpose()
        })
    }

    fn execute(&self, key: &str, payload: &Value) -> Result<Value, ConnectorError> {
        let draft_id = payload
            .get("draft_id")
            .and_then(Value::as_str)
            .ok_or_else(|| error(codes::INVALID_INPUT, "falta draft_id en la carga útil"))?;
        to_value(&self.send_draft(draft_id, key)?)
    }
}

impl MailState {
    fn new(messages: Vec<Message>) -> Self {
        Self {
            messages,
            drafts: Vec::new(),
            sent: Vec::new(),
        }
    }

    /// Completa los ejemplos de un estado anterior. Devuelve si cambió algo.
    fn refresh_samples(&mut self) -> bool {
        let mut changed = false;
        for sample in sample_messages() {
            match self.messages.iter_mut().find(|m| m.id == sample.id) {
                // Mismo ejemplo de antes, sin remitente: recibe el sintético actual.
                Some(stored)
                    if stored.from.is_none()
                        && sample.from.is_some()
                        && stored.subject == sample.subject
                        && stored.body == sample.body =>
                {
                    stored.from = sample.from;
                    changed = true;
                }
                Some(_) => {}
                None => {
                    self.messages.push(sample);
                    changed = true;
                }
            }
        }
        changed
    }
}

fn validate_draft(draft: &NewDraft) -> SimResult<()> {
    if draft.to.is_empty() {
        return Err(error(
            codes::INVALID_INPUT,
            "el borrador no tiene destinatarios",
        ));
    }
    if let Some(bad) = draft.to.iter().find(|a| !is_address(a)) {
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

fn is_address(address: &str) -> bool {
    match address.split_once('@') {
        Some((user, domain)) => {
            !user.is_empty() && domain.contains('.') && !address.contains(char::is_whitespace)
        }
        None => false,
    }
}

fn conflict(key: &str) -> ConnectorError {
    error(
        codes::CONFLICT,
        format!("la clave {key} ya se usó con otro contenido"),
    )
}

fn to_value<T: Serialize>(value: &T) -> SimResult<Value> {
    serde_json::to_value(value).map_err(|e| error(codes::UNAVAILABLE, e.to_string()))
}

/// Hora actual en UTC; `None` si el reloj del sistema es anterior a 1970.
fn utc_now() -> Option<String> {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
    Some(format_utc(secs))
}

/// Segundos desde 1970 a `AAAA-MM-DDTHH:MM:SSZ` (algoritmo de días civiles de Hinnant), sin
/// depender de una biblioteca de fechas.
fn format_utc(secs: u64) -> String {
    let days = (secs / 86_400) as i64 + 719_468;
    let rem = secs % 86_400;
    let era = days.div_euclid(146_097);
    let doe = days.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3_600,
        rem % 3_600 / 60,
        rem % 60
    )
}

/// Mensajes sintéticos de ejemplo (dominio reservado `.test`).
pub fn sample_messages() -> Vec<Message> {
    let message =
        |id: &str, subject: Option<&str>, from: Option<&str>, body: &str, date: Option<&str>| {
            Message {
                id: id.to_owned(),
                subject: subject.map(str::to_owned),
                from: from.map(str::to_owned),
                to: Some(vec!["administracion@nestadmin.test".to_owned()]),
                body: Some(body.to_owned()),
                date: date.map(str::to_owned),
            }
        };
    vec![
        message(
            "M-0001",
            Some("Factura FS-2026-014"),
            Some("facturas@proveedor-alfa.test"),
            "Adjuntamos la factura FS-2026-014 de suministros de oficina.",
            Some("2026-10-01"),
        ),
        message(
            "M-0002",
            Some("Consulta sobre el pedido 2231"),
            Some("cliente.beta@ejemplo.test"),
            "¿Podrían confirmar la fecha de entrega del pedido 2231?",
            Some("2026-10-02"),
        ),
        message(
            "M-0003",
            None,
            Some("avisos@servicio-gamma.test"),
            "Recordatorio de renovación del mantenimiento anual.",
            None,
        ),
        message(
            "M-0004",
            Some("Rectificativa FR-0007"),
            Some("facturas@proveedor-alfa.test"),
            "Enviamos la factura rectificativa FR-0007.",
            Some("2026-10-05"),
        ),
        message(
            "M-0005",
            Some("Cambio de horario"),
            Some("logistica@transportes-delta.test"),
            "El horario de recogida cambia a partir del lunes.",
            Some("2026-10-06"),
        ),
        // Caso de dato ausente a propósito: sin remitente no se puede responder (R06).
        message(
            "M-0006",
            Some("Aviso reenviado sin remitente (prueba de dato ausente)"),
            None,
            "Mensaje reenviado sin cabecera de remitente; la respuesta no se puede enviar.",
            Some("2026-10-07"),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::format_utc;

    #[test]
    fn format_utc_handles_epoch_and_leap_day() {
        assert_eq!(format_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(format_utc(1_709_164_800), "2024-02-29T00:00:00Z");
        assert_eq!(format_utc(1_709_251_199), "2024-02-29T23:59:59Z");
        assert_eq!(format_utc(1_798_761_600), "2027-01-01T00:00:00Z");
    }
}
