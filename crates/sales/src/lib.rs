//! Lógica pura de la facturación de venta (R15–R20, fase 11a). Sin E/S, red ni base de datos.
//!
//! * [`calc`]: cálculo decimal de líneas, desglose por tipo y totales (D75, D04).
//! * [`series`]: series, numeración correlativa sin huecos y revisiones (D73, D74).
//! * [`status`]: estados y transiciones de cada tipo de documento (D74, D81, D82).
//! * [`content`]: contenido obligatorio de la factura y límite de la simplificada (D80, D81).
//! * [`tax_id`]: validación de NIF, NIE y CIF con dígito de control (D80).
//! * [`boundary`]: datos mínimos que este crate entrega a `crates/verifactu` (D83).
//!
//! Las tablas de D72 y su migración 3 (D03) viven en `nestadmin-core`; quedan para la parte
//! de persistencia, que consumirá estos tipos.

pub mod boundary;
pub mod calc;
pub mod content;
pub mod series;
pub mod status;
pub mod tax_id;

pub use boundary::{record_input, InvoiceRecordInput, InvoiceTypeCode, RectifiedRef};
pub use calc::{
    calculate, round_money, CalcError, Exemption, ExemptionKind, LineInput, LineMode, TaxBreakdown,
    TaxRate, Totals,
};
pub use content::{
    prepare_issue, simplified_limit, validate_for_issue, FieldIssue, InvoiceContent, Party,
    Problem, Rectification, RectificationKind, RectificationMethod, SIMPLIFIED_LIMIT,
    SIMPLIFIED_LIMIT_ART_4_2,
};
pub use series::{
    audit_sequence, DocumentKind, DocumentNumber, SequenceAudit, SeriesCounter, SeriesError,
    SeriesKey,
};
pub use status::{
    can_convert, can_correct, can_revise, can_void, ensure_editable, next_revision,
    status_after_conversion, transition, ConversionSource, ReceivedStatus, Status, StatusError,
};
pub use tax_id::{validate_tax_id, TaxIdError, TaxIdKind};
