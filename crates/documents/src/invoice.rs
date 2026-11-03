//! Modelo de factura con los seis campos del contrato del evaluador. Cada campo puede
//! faltar (`None`) y conserva opcionalmente la evidencia literal del documento.
//!
//! Los importes son euros con punto decimal y exactamente dos decimales (D04); las fechas,
//! ISO `AAAA-MM-DD`. Un valor ilegible es un error explícito, nunca un cero ni una fecha
//! aproximada.

use std::fmt;
use std::str::FromStr;

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::error::{DocumentError, Result};

/// Valor normalizado junto con el fragmento literal del documento del que procede.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Evidenced<T> {
    pub value: T,
    pub evidence: Option<String>,
}

impl<T> Evidenced<T> {
    pub fn new(value: T) -> Self {
        Self {
            value,
            evidence: None,
        }
    }

    pub fn with_evidence(value: T, evidence: impl Into<String>) -> Self {
        Self {
            value,
            evidence: Some(evidence.into()),
        }
    }
}

/// Campos tal como los entrega un extractor: texto sin interpretar o ausencia explícita.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawInvoice {
    pub supplier: Option<Evidenced<String>>,
    pub invoice_number: Option<Evidenced<String>>,
    pub date: Option<Evidenced<String>>,
    pub subtotal: Option<Evidenced<String>>,
    pub tax: Option<Evidenced<String>>,
    pub total: Option<Evidenced<String>>,
}

/// Fecha de calendario validada, sin zona horaria.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct IsoDate {
    pub year: u16,
    pub month: u8,
    pub day: u8,
}

impl IsoDate {
    /// Acepta solo `AAAA-MM-DD` con un día que exista en el calendario gregoriano.
    pub fn parse(field: &'static str, literal: &str) -> Result<Self> {
        let invalid = || DocumentError::InvalidDate {
            field,
            literal: literal.to_owned(),
        };
        let text = literal.trim();
        let bytes = text.as_bytes();
        let digits_at = |range: std::ops::Range<usize>| bytes[range].iter().all(u8::is_ascii_digit);
        if bytes.len() != 10
            || bytes[4] != b'-'
            || bytes[7] != b'-'
            || !digits_at(0..4)
            || !digits_at(5..7)
            || !digits_at(8..10)
        {
            return Err(invalid());
        }
        // Los rangos solo contienen dígitos ASCII, así que la conversión no puede fallar.
        let year: u16 = text[0..4].parse().map_err(|_| invalid())?;
        let month: u8 = text[5..7].parse().map_err(|_| invalid())?;
        let day: u8 = text[8..10].parse().map_err(|_| invalid())?;
        if year == 0 || !(1..=12).contains(&month) || day == 0 || day > days_in_month(year, month) {
            return Err(invalid());
        }
        Ok(Self { year, month, day })
    }
}

impl fmt::Display for IsoDate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }
}

fn days_in_month(year: u16, month: u8) -> u8 {
    match month {
        2 if (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400) => {
            29
        }
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Convierte un importe en euros con punto decimal y dos decimales, p. ej. `121.00`,
/// `121.00 EUR` o `121.00 €`. Se rechazan la coma decimal, los separadores de miles y
/// cualquier otra precisión: adivinar el formato podría multiplicar un importe por cien.
pub fn parse_amount(field: &'static str, literal: &str) -> Result<Decimal> {
    let invalid = || DocumentError::InvalidAmount {
        field,
        literal: literal.to_owned(),
    };
    let text = literal.trim();
    let text = text
        .strip_suffix("EUR")
        .or_else(|| text.strip_suffix('€'))
        .unwrap_or(text)
        .trim_end();
    let unsigned = text.strip_prefix('-').unwrap_or(text);
    let well_formed = match unsigned.split_once('.') {
        Some((units, cents)) => {
            !units.is_empty()
                && units.bytes().all(|b| b.is_ascii_digit())
                && cents.len() == 2
                && cents.bytes().all(|b| b.is_ascii_digit())
        }
        None => false,
    };
    if !well_formed {
        return Err(invalid());
    }
    Decimal::from_str(text).map_err(|_| invalid())
}

/// Factura con valores normalizados. `None` significa que el documento no aporta el dato.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Invoice {
    pub supplier: Option<Evidenced<String>>,
    pub invoice_number: Option<Evidenced<String>>,
    pub date: Option<Evidenced<IsoDate>>,
    pub subtotal: Option<Evidenced<Decimal>>,
    pub tax: Option<Evidenced<Decimal>>,
    pub total: Option<Evidenced<Decimal>>,
}

impl Invoice {
    /// Valida y normaliza los campos. Un texto vacío equivale a ausencia (como en el
    /// evaluador); un importe o una fecha ilegibles devuelven error en lugar de un valor.
    pub fn from_raw(raw: &RawInvoice) -> Result<Self> {
        Ok(Self {
            supplier: text_field(&raw.supplier),
            invoice_number: text_field(&raw.invoice_number),
            date: parsed_field(&raw.date, |v| IsoDate::parse("date", v))?,
            subtotal: parsed_field(&raw.subtotal, |v| parse_amount("subtotal", v))?,
            tax: parsed_field(&raw.tax, |v| parse_amount("tax", v))?,
            total: parsed_field(&raw.total, |v| parse_amount("total", v))?,
        })
    }

    /// Campos en el formato del contrato del evaluador: cadenas normalizadas o `None`.
    pub fn contract_fields(&self) -> [(&'static str, Option<String>); 6] {
        let amount = |f: &Option<Evidenced<Decimal>>| f.as_ref().map(|e| format!("{:.2}", e.value));
        [
            ("supplier", self.supplier.as_ref().map(|e| e.value.clone())),
            (
                "invoice_number",
                self.invoice_number.as_ref().map(|e| e.value.clone()),
            ),
            ("date", self.date.as_ref().map(|e| e.value.to_string())),
            ("subtotal", amount(&self.subtotal)),
            ("tax", amount(&self.tax)),
            ("total", amount(&self.total)),
        ]
    }
}

fn present(field: &Option<Evidenced<String>>) -> Option<&Evidenced<String>> {
    field.as_ref().filter(|e| !e.value.trim().is_empty())
}

fn text_field(field: &Option<Evidenced<String>>) -> Option<Evidenced<String>> {
    present(field).map(|e| Evidenced {
        value: e.value.trim().to_owned(),
        evidence: e.evidence.clone(),
    })
}

fn parsed_field<T>(
    field: &Option<Evidenced<String>>,
    parse: impl Fn(&str) -> Result<T>,
) -> Result<Option<Evidenced<T>>> {
    present(field)
        .map(|e| {
            Ok(Evidenced {
                value: parse(&e.value)?,
                evidence: e.evidence.clone(),
            })
        })
        .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amounts_accept_only_two_decimals_with_point() {
        assert_eq!(
            parse_amount("total", "121.00").unwrap(),
            Decimal::new(12100, 2)
        );
        assert_eq!(
            parse_amount("total", " 121.00 EUR ").unwrap(),
            Decimal::new(12100, 2)
        );
        assert_eq!(
            parse_amount("total", "121.00€").unwrap(),
            Decimal::new(12100, 2)
        );
        assert_eq!(
            parse_amount("total", "-5.10").unwrap(),
            Decimal::new(-510, 2)
        );
        for bad in [
            "121,00", "1.210,00", "1,210.00", "121", "121.0", "121.000", ".50", "abc", "", "EUR",
            "12a.00", "--1.00",
        ] {
            let err = parse_amount("total", bad).unwrap_err();
            assert_eq!(err.code(), "invalid_amount", "{bad:?}");
        }
    }

    #[test]
    fn dates_must_exist_in_the_calendar() {
        assert_eq!(
            IsoDate::parse("date", "2024-02-29").unwrap().to_string(),
            "2024-02-29"
        );
        for bad in [
            "2026-02-29",
            "2026-13-01",
            "2026-04-31",
            "0000-01-01",
            "10/01/2026",
            "2026-1-10",
            "2026-01-10T00:00",
        ] {
            assert_eq!(
                IsoDate::parse("date", bad).unwrap_err().code(),
                "invalid_date",
                "{bad:?}"
            );
        }
    }
}
