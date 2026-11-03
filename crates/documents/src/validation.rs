//! Comprobaciones programadas de una factura. Nunca se completa un campo ausente: si falta
//! un importe, se señala la ausencia y no se calcula a partir de los otros.

use serde::{Deserialize, Serialize};

use crate::duplicates::{DuplicateCandidate, DuplicateRegistry};
use crate::invoice::Invoice;

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

/// Incidencias propias de la factura, sin consultar otras facturas.
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
        match subtotal.value.checked_add(tax.value) {
            Some(sum) if sum == total.value => {}
            // Un desbordamiento no puede confirmar el total: se trata como incoherente.
            _ => issues.push(Issue::TotalMismatch),
        }
    }
    issues
}

/// Resultado de validar una factura dentro de una serie.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationReport {
    pub issues: Vec<Issue>,
    /// Facturas anteriores parecidas y por qué; vacío si no hay ninguna.
    pub duplicates: Vec<DuplicateCandidate>,
}

/// Valida la factura, busca posibles duplicados entre las ya procesadas y la añade al
/// registro. Un duplicado se señala; la factura nunca se descarta.
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
    ValidationReport { issues, duplicates }
}
