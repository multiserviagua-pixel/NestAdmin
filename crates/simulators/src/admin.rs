//! Simulador del sistema administrativo: búsqueda y registro de facturas con idempotencia.
//! Los importes son texto decimal y se comparan sin pasar por coma flotante.

use std::path::Path;

use nestadmin_core::{Connector, ConnectorError};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::contracts::AdminConnector;
use crate::state::SimState;
use crate::{check_key, codes, error, FailureMode, SimResult};

/// Factura. Cada campo puede faltar (`None`); los importes son decimales en texto ("121.00").
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Invoice {
    pub supplier: Option<String>,
    pub number: Option<String>,
    /// Fecha `AAAA-MM-DD`.
    pub date: Option<String>,
    /// Código de moneda de tres letras mayúsculas.
    pub currency: Option<String>,
    pub base: Option<String>,
    pub taxes: Option<String>,
    pub total: Option<String>,
}

/// Criterios de búsqueda; los campos `None` no se comparan. Sin criterios se devuelven todas.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvoiceCriteria {
    pub supplier: Option<String>,
    pub number: Option<String>,
    pub date: Option<String>,
    pub total: Option<String>,
}

/// Candidato encontrado; no es una decisión de duplicado.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvoiceMatch {
    pub record_id: String,
    pub invoice: Invoice,
    /// Criterios que coinciden ("mismo proveedor", "mismo número"...).
    pub reasons: Vec<String>,
    /// Criterios que no coinciden: valor distinto o ausente en el registro.
    pub differences: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvoiceReceipt {
    pub record_id: String,
    pub idempotency_key: String,
    pub invoice: Invoice,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Record {
    id: String,
    invoice: Invoice,
    /// `None` para los registros de ejemplo, que no proceden de una acción.
    idempotency_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AdminState {
    records: Vec<Record>,
}

pub struct AdminSimulator {
    state: SimState<AdminState>,
}

impl AdminSimulator {
    /// Simulador en memoria con las facturas de ejemplo ya registradas.
    pub fn in_memory() -> Self {
        Self::with_invoices(sample_invoices())
    }

    /// Simulador en memoria con facturas propias (sintéticas) ya registradas.
    pub fn with_invoices(invoices: Vec<Invoice>) -> Self {
        Self {
            state: SimState::in_memory(AdminState::new(invoices)),
        }
    }

    /// Abre o crea el archivo de estado; si no existe, parte de las facturas de ejemplo.
    pub fn open(path: &Path) -> SimResult<Self> {
        Ok(Self {
            state: SimState::open(path, || AdminState::new(sample_invoices()))?,
        })
    }

    pub fn set_failure(&self, failure: Option<FailureMode>) {
        self.state.set_failure(failure);
    }

    /// Número de facturas registradas, incluidas las de ejemplo.
    pub fn record_count(&self) -> SimResult<usize> {
        self.state.read(|s| Ok(s.records.len()))
    }
}

impl AdminConnector for AdminSimulator {
    /// Facturas registradas que coinciden en al menos un criterio, primero las que coinciden
    /// en más. Cada candidato explica qué coincide y qué difiere, para que un mismo proveedor
    /// y número con otro total aparezca como posible duplicado. Un registro sin el campo
    /// pedido no coincide en él: la ausencia se indica, no se interpreta como igualdad.
    fn find_invoices(&self, criteria: &InvoiceCriteria) -> SimResult<Vec<InvoiceMatch>> {
        if let Some(date) = &criteria.date {
            check_date(date)?;
        }
        let total = criteria.total.as_deref().map(parse_decimal).transpose()?;
        let any_criteria = *criteria != InvoiceCriteria::default();
        self.state.read(|s| {
            let mut matches = Vec::new();
            for record in &s.records {
                let inv = &record.invoice;
                let mut reasons = Vec::new();
                let mut differences = Vec::new();
                let mut note = |outcome: Comparison, labels: [&str; 3]| {
                    match outcome {
                        Comparison::Same => reasons.push(labels[0].to_owned()),
                        Comparison::Different => differences.push(labels[1].to_owned()),
                        Comparison::Missing => differences.push(labels[2].to_owned()),
                    };
                };
                if let Some(wanted) = &criteria.supplier {
                    note(
                        compare(inv.supplier.as_deref(), |v| {
                            v.trim().to_lowercase() == wanted.trim().to_lowercase()
                        }),
                        [
                            "mismo proveedor",
                            "proveedor distinto",
                            "proveedor ausente en el registro",
                        ],
                    );
                }
                if let Some(wanted) = &criteria.number {
                    note(
                        compare(inv.number.as_deref(), |v| v.trim() == wanted.trim()),
                        [
                            "mismo número",
                            "número distinto",
                            "número ausente en el registro",
                        ],
                    );
                }
                if let Some(wanted) = &criteria.date {
                    note(
                        compare(inv.date.as_deref(), |v| v == wanted),
                        [
                            "misma fecha",
                            "fecha distinta",
                            "fecha ausente en el registro",
                        ],
                    );
                }
                if let Some(wanted) = &total {
                    note(
                        compare(inv.total.as_deref(), |v| {
                            parse_decimal(v).is_ok_and(|t| t == *wanted)
                        }),
                        [
                            "mismo total",
                            "total distinto",
                            "total ausente en el registro",
                        ],
                    );
                }
                if !any_criteria || !reasons.is_empty() {
                    matches.push(InvoiceMatch {
                        record_id: record.id.clone(),
                        invoice: inv.clone(),
                        reasons,
                        differences,
                    });
                }
            }
            // Orden estable: a igual número de coincidencias se conserva el de registro.
            matches.sort_by_key(|m| std::cmp::Reverse(m.reasons.len()));
            Ok(matches)
        })
    }

    /// Registra una factura una sola vez por clave. Repetir la clave con la misma factura
    /// devuelve el recibo original; con otra factura, `conflict`.
    fn register_invoice(
        &self,
        invoice: &Invoice,
        idempotency_key: &str,
    ) -> SimResult<InvoiceReceipt> {
        check_key(idempotency_key)?;
        validate_invoice(invoice)?;
        self.state.mutate(|s| {
            if let Some(record) = s
                .records
                .iter()
                .find(|r| r.idempotency_key.as_deref() == Some(idempotency_key))
            {
                if record.invoice == *invoice {
                    return Ok((record.receipt(idempotency_key), false));
                }
                return Err(error(
                    codes::CONFLICT,
                    format!("la clave {idempotency_key} ya se usó con otra factura"),
                ));
            }
            let record = Record {
                id: format!("F-{:04}", s.records.len() + 1),
                invoice: invoice.clone(),
                idempotency_key: Some(idempotency_key.to_owned()),
            };
            let receipt = record.receipt(idempotency_key);
            s.records.push(record);
            Ok((receipt, true))
        })
    }
}

/// Acción `register_invoice` vista desde el núcleo; la carga útil es la factura.
impl Connector for AdminSimulator {
    fn lookup(&self, key: &str) -> Result<Option<Value>, ConnectorError> {
        self.state.read(|s| {
            s.records
                .iter()
                .find(|r| r.idempotency_key.as_deref() == Some(key))
                .map(|r| to_value(&r.receipt(key)))
                .transpose()
        })
    }

    fn execute(&self, key: &str, payload: &Value) -> Result<Value, ConnectorError> {
        let invoice: Invoice = serde_json::from_value(payload.clone())
            .map_err(|e| error(codes::INVALID_INPUT, format!("factura no válida: {e}")))?;
        to_value(&self.register_invoice(&invoice, key)?)
    }
}

impl AdminState {
    fn new(invoices: Vec<Invoice>) -> Self {
        let records = invoices
            .into_iter()
            .enumerate()
            .map(|(i, invoice)| Record {
                id: format!("F-{:04}", i + 1),
                invoice,
                idempotency_key: None,
            })
            .collect();
        Self { records }
    }
}

impl Record {
    /// Recibo de la acción identificada por `key`, con la que se encontró el registro; así
    /// un registro de ejemplo, que no tiene clave, nunca muestra una clave inventada.
    fn receipt(&self, key: &str) -> InvoiceReceipt {
        InvoiceReceipt {
            record_id: self.id.clone(),
            idempotency_key: key.to_owned(),
            invoice: self.invoice.clone(),
        }
    }
}

/// Resultado de comparar un campo del registro con un criterio.
enum Comparison {
    Same,
    Different,
    Missing,
}

fn compare(field: Option<&str>, eq: impl Fn(&str) -> bool) -> Comparison {
    match field {
        None => Comparison::Missing,
        Some(value) if eq(value) => Comparison::Same,
        Some(_) => Comparison::Different,
    }
}

fn validate_invoice(invoice: &Invoice) -> SimResult<()> {
    if *invoice == Invoice::default() {
        return Err(error(
            codes::INVALID_INPUT,
            "la factura no tiene ningún dato",
        ));
    }
    for (name, amount) in [
        ("base", &invoice.base),
        ("impuestos", &invoice.taxes),
        ("total", &invoice.total),
    ] {
        if let Some(amount) = amount {
            parse_decimal(amount)
                .map_err(|e| error(codes::INVALID_INPUT, format!("{name}: {}", e.message)))?;
        }
    }
    if let Some(currency) = &invoice.currency {
        if currency.len() != 3 || !currency.bytes().all(|b| b.is_ascii_uppercase()) {
            return Err(error(
                codes::INVALID_INPUT,
                format!("moneda no válida: {currency}"),
            ));
        }
    }
    if let Some(date) = &invoice.date {
        check_date(date)?;
    }
    Ok(())
}

/// Exige una fecha `AAAA-MM-DD` existente: el día se comprueba contra la longitud del mes,
/// con años bisiestos.
fn check_date(date: &str) -> SimResult<()> {
    let parts: Vec<&str> = date.split('-').collect();
    let fields = match parts.as_slice() {
        [y, m, d]
            if y.len() == 4
                && m.len() == 2
                && d.len() == 2
                && date.bytes().all(|b| b.is_ascii_digit() || b == b'-') =>
        {
            y.parse::<u32>()
                .ok()
                .zip(m.parse::<u32>().ok())
                .zip(d.parse::<u32>().ok())
        }
        _ => None,
    };
    let valid = fields.is_some_and(|((year, month), day)| {
        (1..=12).contains(&month) && (1..=days_in_month(year, month)).contains(&day)
    });
    if valid {
        Ok(())
    } else {
        Err(error(
            codes::INVALID_INPUT,
            format!("fecha no válida (AAAA-MM-DD): {date}"),
        ))
    }
}

fn days_in_month(year: u32, month: u32) -> u32 {
    let leap = (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400);
    match month {
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => 31,
    }
}

/// Forma canónica de un decimal en texto (`-?\d+(\.\d+)?`): signo, parte entera sin ceros a
/// la izquierda y parte decimal sin ceros a la derecha. "121.0" y "121.00" son iguales.
fn parse_decimal(text: &str) -> SimResult<(bool, String, String)> {
    let invalid = || error(codes::INVALID_INPUT, format!("importe no decimal: {text}"));
    let (negative, digits) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    let (int, frac) = digits.split_once('.').unwrap_or((digits, ""));
    let all_digits = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
    if int.is_empty() || !all_digits(int) || !all_digits(frac) || digits.ends_with('.') {
        return Err(invalid());
    }
    let int = int.trim_start_matches('0');
    let frac = frac.trim_end_matches('0');
    let is_zero = int.is_empty() && frac.is_empty();
    Ok((negative && !is_zero, int.to_owned(), frac.to_owned()))
}

fn to_value<T: Serialize>(value: &T) -> SimResult<Value> {
    serde_json::to_value(value).map_err(|e| error(codes::UNAVAILABLE, e.to_string()))
}

/// Facturas sintéticas ya registradas en el sistema simulado.
pub fn sample_invoices() -> Vec<Invoice> {
    let text = |s: &str| Some(s.to_owned());
    vec![
        Invoice {
            supplier: text("Proveedor Alfa S.L."),
            number: text("FS-2026-001"),
            date: text("2026-09-15"),
            currency: text("EUR"),
            base: text("100.00"),
            taxes: text("21.00"),
            total: text("121.00"),
        },
        Invoice {
            supplier: text("Servicios Gamma S.A."),
            number: text("G-0042"),
            date: text("2026-09-30"),
            currency: text("EUR"),
            base: text("250.00"),
            taxes: None,
            total: None,
        },
    ]
}
