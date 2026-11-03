//! Detector de posibles duplicados sobre un registro en memoria procesado en orden.
//!
//! Devuelve candidatos con sus motivos y no descarta nada: la decisión es del usuario
//! (contratos.md, administración). Un campo ausente no coincide con nada, para no deducir
//! una coincidencia que el documento no respalda.

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::invoice::{Evidenced, Invoice, IsoDate};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DuplicateReason {
    SameSupplierAndNumber,
    SameSupplierDateAndTotal,
}

/// Factura anterior que se parece a la actual.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DuplicateCandidate {
    pub existing_id: String,
    pub reasons: Vec<DuplicateReason>,
}

/// Claves de comparación de una factura ya vista.
#[derive(Debug, Clone)]
struct Entry {
    id: String,
    supplier: Option<String>,
    number: Option<String>,
    date: Option<IsoDate>,
    total: Option<Decimal>,
}

impl Entry {
    fn from_invoice(id: String, invoice: &Invoice) -> Self {
        Self {
            id,
            supplier: invoice.supplier.as_ref().map(|e| normalize(&e.value)),
            number: invoice.invoice_number.as_ref().map(|e| normalize(&e.value)),
            date: value(&invoice.date),
            total: value(&invoice.total),
        }
    }
}

fn value<T: Copy>(field: &Option<Evidenced<T>>) -> Option<T> {
    field.as_ref().map(|e| e.value)
}

/// Sin distinción de mayúsculas ni de espacios repetidos; no elimina puntuación ni
/// formas jurídicas para no confundir proveedores distintos.
fn normalize(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn both<T: PartialEq>(a: &Option<T>, b: &Option<T>) -> bool {
    matches!((a, b), (Some(a), Some(b)) if a == b)
}

/// Registro de una serie de facturas. Empieza vacío y conserva el orden de llegada.
#[derive(Debug, Default)]
pub struct DuplicateRegistry {
    entries: Vec<Entry>,
}

impl DuplicateRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Candidatos entre las facturas ya registradas, en orden de llegada, sin modificar el
    /// registro.
    pub fn candidates(&self, invoice: &Invoice) -> Vec<DuplicateCandidate> {
        let current = Entry::from_invoice(String::new(), invoice);
        self.entries
            .iter()
            .filter_map(|seen| {
                let mut reasons = Vec::new();
                if both(&seen.supplier, &current.supplier) {
                    if both(&seen.number, &current.number) {
                        reasons.push(DuplicateReason::SameSupplierAndNumber);
                    }
                    if both(&seen.date, &current.date) && both(&seen.total, &current.total) {
                        reasons.push(DuplicateReason::SameSupplierDateAndTotal);
                    }
                }
                (!reasons.is_empty()).then(|| DuplicateCandidate {
                    existing_id: seen.id.clone(),
                    reasons,
                })
            })
            .collect()
    }

    /// Busca candidatos y registra la factura, sea o no un posible duplicado.
    pub fn check_and_add(
        &mut self,
        id: impl Into<String>,
        invoice: &Invoice,
    ) -> Vec<DuplicateCandidate> {
        let found = self.candidates(invoice);
        self.entries.push(Entry::from_invoice(id.into(), invoice));
        found
    }
}
