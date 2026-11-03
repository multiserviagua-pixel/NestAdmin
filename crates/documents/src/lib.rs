//! Documentos de NestAdmin (R05): entrada de texto, modelo de factura con ausencia
//! explícita, comprobación decimal de importes (D04) y aviso de posibles duplicados.
//!
//! La extracción de campos con IA, el PDF (D09) y el OCR (D10) quedan fuera de este crate
//! por ahora: el texto de un PDF devuelve un error explícito de formato no soportado.

pub mod duplicates;
pub mod error;
pub mod invoice;
pub mod text;
pub mod validation;

pub use duplicates::{DuplicateCandidate, DuplicateReason, DuplicateRegistry};
pub use error::{DocumentError, Result};
pub use invoice::{
    parse_amount, parse_currency, Evidenced, FieldError, FieldErrorKind, Invoice, IsoDate,
    RawInvoice, EUR,
};
pub use text::{DocumentFormat, DocumentText, ExtractedText, LocalDocumentText};
pub use validation::{process_invoice, validate, Issue, ValidationReport};
