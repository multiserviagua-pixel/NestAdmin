//! Simulador del sistema administrativo: búsqueda y registro de facturas con idempotencia.
//! Los importes son texto decimal y se comparan sin pasar por coma flotante.

use std::path::Path;

use nestadmin_core::{Connector, ConnectorError};
use serde::{Deserialize, Serialize};
use serde_json::Value;

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

/// Criterios de búsqueda; los campos `None` no filtran. Sin criterios se devuelven todas.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvoiceCriteria {
    pub supplier: Option<String>,
    pub number: Option<String>,
    pub date: Option<String>,
    pub total: Option<String>,
}

/// Candidato encontrado y motivos de la coincidencia; no es una decisión de duplicado.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvoiceMatch {
    pub record_id: String,
    pub invoice: Invoice,
    pub reasons: Vec<String>,
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

    /// Facturas registradas que cumplen todos los criterios presentes. Un registro sin el
    /// campo pedido no coincide: la ausencia no se interpreta como igualdad.
    pub fn find_invoices(&self, criteria: &InvoiceCriteria) -> SimResult<Vec<InvoiceMatch>> {
        if let Some(date) = &criteria.date {
            check_date(date)?;
        }
        let total = criteria.total.as_deref().map(parse_decimal).transpose()?;
        self.state.read(|s| {
            let mut matches = Vec::new();
            for record in &s.records {
                let inv = &record.invoice;
                let mut reasons = Vec::new();
                let mut ok = true;
                if let Some(wanted) = &criteria.supplier {
                    ok &= same(&inv.supplier, wanted, |a, b| {
                        a.trim().to_lowercase() == b.trim().to_lowercase()
                    });
                    reasons.push("mismo proveedor".to_owned());
                }
                if let Some(wanted) = &criteria.number {
                    ok &= same(&inv.number, wanted, |a, b| a.trim() == b.trim());
                    reasons.push("mismo número".to_owned());
                }
                if let Some(wanted) = &criteria.date {
                    ok &= same(&inv.date, wanted, |a, b| a == b);
                    reasons.push("misma fecha".to_owned());
                }
                if let Some(wanted) = &total {
                    ok &= inv
                        .total
                        .as_deref()
                        .and_then(|t| parse_decimal(t).ok())
                        .is_some_and(|t| t == *wanted);
                    reasons.push("mismo total".to_owned());
                }
                if ok {
                    matches.push(InvoiceMatch {
                        record_id: record.id.clone(),
                        invoice: inv.clone(),
                        reasons,
                    });
                }
            }
            Ok(matches)
        })
    }

    /// Registra una factura una sola vez por clave. Repetir la clave con la misma factura
    /// devuelve el recibo original; con otra factura, `conflict`.
    pub fn register_invoice(
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
                    return Ok((record.receipt(), false));
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
            let receipt = record.receipt();
            s.records.push(record);
            Ok((receipt, true))
        })
    }

    /// Número de facturas registradas, incluidas las de ejemplo.
    pub fn record_count(&self) -> SimResult<usize> {
        self.state.read(|s| Ok(s.records.len()))
    }
}

/// Acción `register_invoice` vista desde el núcleo; la carga útil es la factura.
impl Connector for AdminSimulator {
    fn lookup(&self, key: &str) -> Result<Option<Value>, ConnectorError> {
        self.state.read(|s| {
            s.records
                .iter()
                .find(|r| r.idempotency_key.as_deref() == Some(key))
                .map(|r| to_value(&r.receipt()))
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
    /// Solo se llama para registros creados por una acción, que siempre tienen clave.
    fn receipt(&self) -> InvoiceReceipt {
        InvoiceReceipt {
            record_id: self.id.clone(),
            idempotency_key: self.idempotency_key.clone().unwrap_or_default(),
            invoice: self.invoice.clone(),
        }
    }
}

fn same(field: &Option<String>, wanted: &str, eq: impl Fn(&str, &str) -> bool) -> bool {
    field.as_deref().is_some_and(|value| eq(value, wanted))
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

fn check_date(date: &str) -> SimResult<()> {
    let parts: Vec<&str> = date.split('-').collect();
    let valid = matches!(parts.as_slice(), [y, m, d]
        if y.len() == 4 && m.len() == 2 && d.len() == 2
            && date.bytes().all(|b| b.is_ascii_digit() || b == b'-')
            && (1..=12).contains(&m.parse::<u8>().unwrap_or(0))
            && (1..=31).contains(&d.parse::<u8>().unwrap_or(0)));
    if valid {
        Ok(())
    } else {
        Err(error(
            codes::INVALID_INPUT,
            format!("fecha no válida (AAAA-MM-DD): {date}"),
        ))
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
