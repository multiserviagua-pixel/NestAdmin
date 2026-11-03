//! Comprobaciones programadas de una factura. Nunca se completa un campo ausente: si falta
//! un importe, se señala la ausencia y no se calcula a partir de los otros.

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::duplicates::{DuplicateCandidate, DuplicateRegistry};
use crate::invoice::{FieldError, Invoice};

/// Etiquetas de incidencia del contrato del evaluador.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Issue {
    MissingTax,
    MissingTotal,
    /// Base + IVA distinto del total, con aritmética decimal exacta.
    TotalMismatch,
    PossibleDuplicate,
}

impl Issue {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MissingTax => "missing_tax",
            Self::MissingTotal => "missing_total",
            Self::TotalMismatch => "total_mismatch",
            Self::PossibleDuplicate => "possible_duplicate",
        }
    }
}

/// Incidencias propias de la factura, sin consultar otras facturas. Un IVA o un total
/// ilegibles tampoco constan, así que también se señalan como ausentes; el motivo
/// concreto queda en `Invoice::unreadable` y en `ValidationReport::unreadable`.
pub fn validate(invoice: &Invoice) -> Vec<Issue> {
    let mut issues = Vec::new();
    if invoice.tax.is_none() {
        issues.push(Issue::MissingTax);
    }
    if invoice.total.is_none() {
        issues.push(Issue::MissingTotal);
    }
    // Solo se compara si constan los tres importes; `Decimal` suma sin redondeo binario.
    if let (Some(subtotal), Some(tax), Some(total)) =
        (&invoice.subtotal, &invoice.tax, &invoice.total)
    {
        if exact_sum(subtotal.value, tax.value) != Some(total.value) {
            issues.push(Issue::TotalMismatch);
        }
    }
    issues
}

/// Suma sin pérdida o `None`. Cerca del límite de 96 bits `rust_decimal` no siempre
/// desborda: redondea a menos decimales, y 503.35 + 0.06 daría 503.4, igual a un total de
/// 503.40. Un resultado con menos decimales que los sumandos no es exacto y no puede
/// confirmar el total, igual que un desbordamiento.
fn exact_sum(a: Decimal, b: Decimal) -> Option<Decimal> {
    a.checked_add(b)
        .filter(|sum| sum.scale() >= a.scale().max(b.scale()))
}

/// Resultado de validar una factura dentro de una serie.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationReport {
    pub issues: Vec<Issue>,
    /// Facturas anteriores parecidas y por qué; vacío si no hay ninguna.
    pub duplicates: Vec<DuplicateCandidate>,
    /// Campos presentes pero ilegibles; vacío si todos se interpretaron.
    pub unreadable: Vec<FieldError>,
}

/// Valida la factura, busca posibles duplicados entre las ya procesadas y la añade al
/// registro. Un duplicado se señala; la factura nunca se descarta, aunque tenga campos
/// ilegibles. Repetir el proceso con el mismo `id` (reintento o recuperación de un trabajo)
/// sustituye la entrada sin duplicarla, como describe `DuplicateRegistry::check_and_add`.
pub fn process_invoice(
    registry: &mut DuplicateRegistry,
    id: impl Into<String>,
    invoice: &Invoice,
) -> ValidationReport {
    let mut issues = validate(invoice);
    let duplicates = registry.check_and_add(id, invoice);
    if !duplicates.is_empty() {
        issues.push(Issue::PossibleDuplicate);
    }
    ValidationReport {
        issues,
        duplicates,
        unreadable: invoice.unreadable.clone(),
    }
}
