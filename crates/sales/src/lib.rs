//! Lógica pura de la facturación de venta (R15–R20, fase 11a). Sin E/S, red ni base de datos.
//!
//! * [`calc`]: cálculo decimal de líneas, desglose por tipo y totales (D75, D04).
//! * [`series`]: series, numeración correlativa sin huecos y revisiones (D73, D74).
//! * [`status`]: estados y transiciones de cada tipo de documento (D74, D81, D82).
//! * [`content`]: contenido obligatorio de la factura y límite de la simplificada (D80, D81).
//! * [`tax_id`]: validación de NIF, NIE y CIF con dígito de control (D80).
//! * [`payments`]: cobros y estado de cobro derivado (R19, D88).
//! * [`catalog`]: catálogo cuyas líneas copian valores (R20, D79).
//! * [`recurring`]: plantillas y calendario a borradores, sin programador (R17, D87).
//! * [`csv`] y [`reports`]: libros, resumen de IVA, gastos y exportación CSV (R20, D89).
//! * [`dates`]: aritmética de fechas de calendario.
//! * [`boundary`]: datos mínimos que este crate entrega a `crates/verifactu` (D83).
//!
//! Las tablas de D72 y su migración 3 (D03) viven en `nestadmin-core`; quedan para la parte
//! de persistencia, que consumirá estos tipos.

pub mod boundary;
pub mod calc;
pub mod catalog;
pub mod content;
pub mod csv;
pub mod dates;
pub mod payments;
pub mod recurring;
pub mod reports;
pub mod series;
pub mod status;
pub mod tax_id;

pub use boundary::{record_input, InvoiceRecordInput, InvoiceTypeCode, RectifiedRef};
pub use calc::{
    calculate, round_money, CalcError, Exemption, LineInput, LineMode, TaxBreakdown,
    TaxQualification, TaxRate, Totals,
};
pub use catalog::{CatalogError, Product};
pub use content::{
    prepare_issue, simplified_limit, validate_for_issue, FieldIssue, InvoiceContent, Party,
    Problem, Rectification, RectificationKind, RectificationMethod, SIMPLIFIED_LIMIT,
    SIMPLIFIED_LIMIT_ART_4_2,
};
pub use csv::{Cell, CsvError, CsvOptions, CsvWriter};
pub use payments::{
    add_payment, due_date, remove_payment, summarize, CollectionState, CollectionSummary,
    LinkedCorrective, Payment, PaymentError, PaymentMethod, Receivable,
};
pub use recurring::{
    due_drafts, issue_mode, DraftPlan, Frequency, IssueMode, PeriodKey, RecurringError,
    RecurringTemplate,
};
pub use reports::{
    expenses_by_category, vat_summary, write_expenses, write_issued_book, write_received_book,
    write_vat_summary, DateRange, ExpenseLine, IssuedEntry, ReceivedEntry, ReceivedTaxLine,
    ReportError, VatLine, VatSummary,
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
