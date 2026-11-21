//! Persistencia de la facturación de venta (D72–D90, migración 3). Toda la lógica de negocio
//! (cálculo, numeración, estados, contenido, cobros, recurrentes) vive en `nestadmin-sales` y
//! aquí solo se lee y se escribe: cada operación pública es una transacción (D73).
//!
//! * [`master`]: empresa (D90), clientes, productos (D79) y albaranes recibidos (D74).
//! * [`documents`]: borradores, emisión, transiciones, revisión, anulación, rectificativas y
//!   conversión.
//! * [`collections`]: cobros (D88) y plantillas recurrentes (D87).
//! * [`verifactu`]: registros Verifactu de alta y anulación, encadenados (fase 11, migración 12).
//! * [`verifactu_remission`]: entorno, lotes de remisión a la AEAT, estado derivado de cada
//!   registro y subsanación (D248-D258, migración 16).

use std::str::FromStr;

use nestadmin_documents::IsoDate;
use nestadmin_sales::{
    DocumentKind, RectificationKind, RectificationMethod, Status, TaxQualification, TaxRate,
};
use rusqlite::{params, Connection};
use rust_decimal::Decimal;

use crate::store::now_ms;

pub mod collections;
pub mod documents;
pub mod error;
pub mod master;
pub mod sends;
pub mod verifactu;
pub mod verifactu_remission;
pub mod verifactu_sending;

pub use collections::{GeneratedDraft, RecurringRecord, RecurringRun};
pub use documents::{
    DraftInput, DraftLine, IssuerSnapshot, RectificationDraft, SalesDocument, SalesFilter,
    SalesLineRecord,
};
pub use error::{SalesResult, SalesStoreError};
pub use master::{
    Company, Customer, CustomerData, Letterhead, LetterheadMargins, ReceivedDeliveryNote,
    MAX_LETTERHEAD_BYTES,
};
pub use sends::{
    is_sendable, send_input_ref, NewSalesSend, SalesSend, SendCompletion, CODE_SEND_IN_PROGRESS,
    DEFAULT_TAXPAYER_ID, SEND_SALES_DOCUMENT,
};
pub use verifactu::{
    ChainStatus, VerifactuContext, VerifactuKind, VerifactuRecordRow, CODE_CHAIN_BROKEN,
    CODE_CLOCK_BEHIND, CODE_RECORD_INVALID,
};
pub use verifactu_remission::{
    BatchClosure, BatchOutcome, RecordEnvironment, VerifactuAttemptTicket, VerifactuBatch,
    VerifactuBatchResult, VerifactuBatchSummary, VerifactuEnvironment, VerifactuRecordState,
    VerifactuStatus, VerifactuSubsanation, CODE_BATCH_OPEN, CODE_BATCH_REJECTED,
    CODE_ENVIRONMENT_BUSY, CODE_ENVIRONMENT_CHANGED, MAX_AUTOMATIC_FAULTS, MIN_RETRY_WAIT_SECONDS,
    OBLIGATION_AUTHORIZATION,
};
pub use verifactu_sending::{
    retry_wait_seconds, AuthorizationHalt, FaultClass, RemissionSettings, SenderCapacity,
    VerifactuIncident, VerifactuUnsentAttempt, WaitMode, AUTHORIZATION_FAULT_CODES,
    CODE_AUTHORIZATION_REJECTED, FAST_RETRIES, INCIDENT_UNSENT_REASONS, MAX_RETRY_WAIT_SECONDS,
    TECHNICAL_FAULT_CODES,
};

// Conversiones entre los tipos puros y el texto guardado.

pub(crate) fn money(value: Decimal) -> String {
    nestadmin_sales::round_money(value).to_string()
}

/// Cantidad, precio o descuento: sin ceros sobrantes.
pub(crate) fn plain(value: Decimal) -> String {
    value.normalize().to_string()
}

pub(crate) fn dec(field: &str, text: &str) -> SalesResult<Decimal> {
    Decimal::from_str(text).map_err(|_| SalesStoreError::Corrupt(format!("importe en {field}")))
}

pub(crate) fn parse_date(field: &'static str, text: &str) -> SalesResult<IsoDate> {
    IsoDate::parse(field, text).map_err(|_| SalesStoreError::Corrupt(format!("fecha en {field}")))
}

pub(crate) fn kind_from(text: &str) -> SalesResult<DocumentKind> {
    DocumentKind::parse(text)
        .ok_or_else(|| SalesStoreError::Corrupt(format!("tipo de documento {text}")))
}

pub(crate) fn status_from(text: &str) -> SalesResult<Status> {
    use Status::*;
    [
        Draft, Issued, Sent, Accepted, Rejected, Expired, Converted, Delivered, Invoiced, Voided,
    ]
    .into_iter()
    .find(|s| s.as_str() == text)
    .ok_or_else(|| SalesStoreError::Corrupt(format!("estado {text}")))
}

pub(crate) fn rect_kind_text(kind: RectificationKind) -> &'static str {
    match kind {
        RectificationKind::R1 => "R1",
        RectificationKind::R2 => "R2",
        RectificationKind::R3 => "R3",
        RectificationKind::R4 => "R4",
        RectificationKind::R5 => "R5",
    }
}

pub(crate) fn rect_kind_from(text: &str) -> SalesResult<RectificationKind> {
    use RectificationKind::*;
    [R1, R2, R3, R4, R5]
        .into_iter()
        .find(|k| rect_kind_text(*k) == text)
        .ok_or_else(|| SalesStoreError::Corrupt(format!("tipo de rectificación {text}")))
}

pub(crate) fn method_text(method: RectificationMethod) -> &'static str {
    match method {
        RectificationMethod::Substitution => "substitution",
        RectificationMethod::Differences => "differences",
    }
}

pub(crate) fn method_from(text: &str) -> SalesResult<RectificationMethod> {
    [
        RectificationMethod::Substitution,
        RectificationMethod::Differences,
    ]
    .into_iter()
    .find(|m| method_text(*m) == text)
    .ok_or_else(|| SalesStoreError::Corrupt(format!("método de rectificación {text}")))
}

pub(crate) fn tax_rate_from(text: &str) -> SalesResult<TaxRate> {
    TaxRate::from_percent(dec("tax_rate", text)?)
        .map_err(|_| SalesStoreError::Corrupt(format!("tipo de IVA {text}")))
}

pub(crate) fn qualification_from(text: &str) -> SalesResult<TaxQualification> {
    TaxQualification::parse(text)
        .ok_or_else(|| SalesStoreError::Corrupt(format!("calificación {text}")))
}

/// Entrada en `audit_log` (la misma tabla que las acciones). `detail` lleva solo identificadores
/// y el número del documento, nunca datos personales.
pub(crate) fn audit(
    conn: &Connection,
    action: &str,
    detail: &serde_json::Value,
) -> SalesResult<()> {
    conn.execute(
        "INSERT INTO audit_log (at, task_id, action, status, authorization, detail) \
         VALUES (?1, NULL, ?2, 'ok', NULL, ?3)",
        params![now_ms(), action, detail.to_string()],
    )?;
    Ok(())
}

/// Texto vacío o solo espacios equivale a ausencia.
pub(crate) fn clean(value: &Option<String>) -> Option<String> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_owned)
}

/// NIF tal como se guarda y se copia en las facturas: sin espacios ni guiones y en mayúsculas,
/// igual que lo normaliza `validate_tax_id` y lo lleva el registro Verifactu (D220). Vacío
/// equivale a ausencia.
pub(crate) fn clean_tax_id(value: &Option<String>) -> Option<String> {
    clean(value)
        .map(|v| {
            v.chars()
                .filter(|c| !c.is_whitespace() && *c != '-')
                .collect::<String>()
                .to_uppercase()
        })
        .filter(|v| !v.is_empty())
}
