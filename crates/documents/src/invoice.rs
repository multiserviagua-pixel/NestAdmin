//! Modelo de factura con los seis campos del contrato del evaluador y la moneda (D04,
//! contratos.md). Cada campo puede faltar (`None`) y conserva opcionalmente la evidencia
//! literal del documento.
//!
//! Los importes son euros con exactamente dos decimales, con punto o con coma decimal y
//! separadores de miles bien agrupados (D04, D48); las fechas, ISO `AAAA-MM-DD`. Un valor ilegible se anota como error explícito del campo, nunca como
//! un cero ni una fecha aproximada, y no impide validar el resto de la factura.

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
    /// Moneda indicada por el extractor; si falta, se toma del sufijo de un importe.
    pub currency: Option<Evidenced<String>>,
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

/// Separadores de miles admitidos además del punto o la coma: espacio, espacio duro y
/// espacio fino, como se escriben en español.
const SPACES: [char; 3] = [' ', '\u{a0}', '\u{202f}'];

/// Convierte un importe en euros con dos decimales: `1234.56`, `1,234.56`, `1234,56`,
/// `1.234,56` o `1 234,56`, con `EUR` o `€` detrás si se quiere (D48). El separador decimal es
/// el último punto o coma y debe ir seguido de exactamente dos cifras; el otro signo, o un
/// espacio, solo puede separar grupos de tres cifras. Se rechaza lo ambiguo (`1.234`, `1,234`,
/// `121`) y cualquier otra precisión: adivinar el formato podría multiplicar un importe por mil.
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
        .trim_end_matches(|c: char| c.is_whitespace());
    let (negative, unsigned) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    let Some(at) = unsigned.rfind(['.', ',']) else {
        return Err(invalid());
    };
    let decimal = &unsigned[at..at + 1];
    let (units, cents) = (&unsigned[..at], &unsigned[at + 1..]);
    if cents.len() != 2 || !cents.bytes().all(|b| b.is_ascii_digit()) {
        return Err(invalid());
    }
    let thousands: Vec<char> = units.chars().filter(|c| !c.is_ascii_digit()).collect();
    let digits = if thousands.is_empty() {
        units.to_owned()
    } else {
        // Un único separador de miles, distinto del decimal, entre grupos de tres cifras.
        let separator = thousands[0];
        let allowed = (separator != decimal.chars().next().unwrap_or('.'))
            && (matches!(separator, '.' | ',') || SPACES.contains(&separator));
        if !allowed || thousands.iter().any(|&c| c != separator) {
            return Err(invalid());
        }
        let groups: Vec<&str> = units.split(separator).collect();
        let first_ok = (1..=3).contains(&groups[0].len());
        let rest_ok = groups[1..].iter().all(|g| g.len() == 3);
        if !first_ok || !rest_ok {
            return Err(invalid());
        }
        groups.concat()
    };
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(invalid());
    }
    let sign = if negative { "-" } else { "" };
    Decimal::from_str(&format!("{sign}{digits}.{cents}")).map_err(|_| invalid())
}

/// Motivo por el que un importe se recuperó en lugar de leerse sin duda (D49).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryReason {
    /// `121`: sin decimales.
    NoDecimals,
    /// `121,5`: un solo decimal.
    OneDecimal,
    /// `1.234` o `1,234`: el signo podría ser decimal (con tres cifras) o de miles.
    AmbiguousSeparator,
}

impl RecoveryReason {
    pub fn code(self) -> &'static str {
        match self {
            Self::NoDecimals => "no_decimals",
            Self::OneDecimal => "one_decimal",
            Self::AmbiguousSeparator => "ambiguous_separator",
        }
    }
}

/// Lectura probable de un importe que [`parse_amount`] rechaza por ambiguo o incompleto
/// (D49): `121` → 121.00; `121,5` → 121.50; `1.234`, `1,234` o `1.234.567` → separador de
/// miles. Es una propuesta para que la confirme una persona, nunca un valor seguro: lo que
/// no encaja en esos patrones (`abc`, `1,21,00`, `12a.00`) no se recupera.
pub fn recover_amount(literal: &str) -> Option<(Decimal, RecoveryReason)> {
    let text = literal.trim();
    let text = text
        .strip_suffix("EUR")
        .or_else(|| text.strip_suffix('€'))
        .unwrap_or(text)
        .trim_end_matches(|c: char| c.is_whitespace());
    let (sign, unsigned) = match text.strip_prefix('-') {
        Some(rest) => ("-", rest),
        None => ("", text),
    };
    let digits = |t: &str| !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit());
    let parse =
        |units: &str, cents: &str| Decimal::from_str(&format!("{sign}{units}.{cents}")).ok();
    if digits(unsigned) {
        return parse(unsigned, "00").map(|v| (v, RecoveryReason::NoDecimals));
    }
    let separators: Vec<char> = unsigned.chars().filter(|c| !c.is_ascii_digit()).collect();
    let separator = *separators.first()?;
    if !matches!(separator, '.' | ',') || separators.iter().any(|&c| c != separator) {
        return None;
    }
    let groups: Vec<&str> = unsigned.split(separator).collect();
    if !groups.iter().all(|g| digits(g)) {
        return None;
    }
    match groups.as_slice() {
        [units, cents] if cents.len() == 1 => {
            parse(units, &format!("{cents}0")).map(|v| (v, RecoveryReason::OneDecimal))
        }
        [first, rest @ ..]
            if (1..=3).contains(&first.len()) && rest.iter().all(|g| g.len() == 3) =>
        {
            parse(&groups.concat(), "00").map(|v| (v, RecoveryReason::AmbiguousSeparator))
        }
        _ => None,
    }
}

/// Importe que no se pudo leer sin duda y se ha recuperado como propuesta (D49). El campo
/// de la factura lleva el valor propuesto, pero la factura no puede registrarse sin que una
/// persona lo confirme con el documento.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveredAmount {
    pub field: String,
    /// Texto tal como lo devolvió el extractor.
    pub literal: String,
    pub evidence: Option<String>,
    /// Valor propuesto, normalizado con dos decimales y punto.
    pub value: String,
    pub reason: RecoveryReason,
}

/// Moneda admitida. Solo euros por ahora (D04 y contrato del evaluador).
pub const EUR: &str = "EUR";

/// Campo presente en el documento cuyo valor no se pudo interpretar. Se conserva junto a
/// la factura para que el resto de campos siga validándose y el usuario sepa por qué el
/// valor no consta.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldError {
    pub field: String,
    pub kind: FieldErrorKind,
    pub literal: String,
    pub evidence: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldErrorKind {
    InvalidAmount,
    InvalidDate,
    UnsupportedCurrency,
}

impl FieldErrorKind {
    /// Código estable, el mismo que el del `DocumentError` equivalente.
    pub fn code(self) -> &'static str {
        match self {
            Self::InvalidAmount => "invalid_amount",
            Self::InvalidDate => "invalid_date",
            Self::UnsupportedCurrency => "unsupported_currency",
        }
    }
}

impl FieldError {
    pub fn code(&self) -> &'static str {
        self.kind.code()
    }

    /// Convierte un error de interpretación de campo. Los demás errores no proceden de un
    /// campo y se devuelven sin cambios.
    fn from_error(
        error: DocumentError,
        evidence: Option<String>,
    ) -> std::result::Result<Self, DocumentError> {
        let (field, kind, literal) = match error {
            DocumentError::InvalidAmount { field, literal } => {
                (field, FieldErrorKind::InvalidAmount, literal)
            }
            DocumentError::InvalidDate { field, literal } => {
                (field, FieldErrorKind::InvalidDate, literal)
            }
            DocumentError::UnsupportedCurrency { field, literal } => {
                (field, FieldErrorKind::UnsupportedCurrency, literal)
            }
            other => return Err(other),
        };
        Ok(Self {
            field: field.to_owned(),
            kind,
            literal,
            evidence,
        })
    }
}

/// Normaliza una moneda explícita. Solo se admite el euro (`EUR`, `€`, `euro`, `euros`);
/// otra moneda es un error, nunca una conversión.
pub fn parse_currency(field: &'static str, literal: &str) -> Result<String> {
    match literal.trim().to_lowercase().as_str() {
        "eur" | "€" | "euro" | "euros" => Ok(EUR.to_owned()),
        _ => Err(DocumentError::UnsupportedCurrency {
            field,
            literal: literal.to_owned(),
        }),
    }
}

/// Indica si el literal de un importe lleva el sufijo de euro.
fn has_euro_suffix(literal: &str) -> bool {
    let text = literal.trim();
    text.ends_with("EUR") || text.ends_with('€')
}

/// Factura con valores normalizados. `None` significa que el valor no consta: o el
/// documento no lo aporta, o es ilegible y entonces figura en `unreadable`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Invoice {
    pub supplier: Option<Evidenced<String>>,
    pub invoice_number: Option<Evidenced<String>>,
    pub date: Option<Evidenced<IsoDate>>,
    /// Moneda explícita del extractor o, en su defecto, el sufijo de un importe legible.
    /// Nunca se supone sin indicio en el documento.
    pub currency: Option<Evidenced<String>>,
    pub subtotal: Option<Evidenced<Decimal>>,
    pub tax: Option<Evidenced<Decimal>>,
    pub total: Option<Evidenced<Decimal>>,
    /// Campos presentes pero ilegibles, en el orden en que se interpretan.
    pub unreadable: Vec<FieldError>,
    /// Importes dudosos recuperados como propuesta; exigen revisión humana (D49).
    pub recovered: Vec<RecoveredAmount>,
}

impl Invoice {
    /// Valida y normaliza los campos. Un texto vacío equivale a ausencia (como en el
    /// evaluador). Un importe, una fecha o una moneda ilegibles dejan ese campo en `None` y
    /// se anotan en `unreadable`, sin perder el resto de campos ni sustituirlos por un cero.
    pub fn from_raw(raw: &RawInvoice) -> Self {
        let mut unreadable = Vec::new();
        let date = parsed_field(&raw.date, &mut unreadable, |v| IsoDate::parse("date", v));
        let explicit_currency = parsed_field(&raw.currency, &mut unreadable, |v| {
            parse_currency("currency", v)
        });
        let mut recovered = Vec::new();
        let subtotal = amount_field(&raw.subtotal, "subtotal", &mut unreadable, &mut recovered);
        let tax = amount_field(&raw.tax, "tax", &mut unreadable, &mut recovered);
        let total = amount_field(&raw.total, "total", &mut unreadable, &mut recovered);
        // Sin moneda del extractor se toma el sufijo de un importe legible, con ese literal
        // como evidencia. Una moneda explícita ilegible no se suple con otra fuente.
        let currency_unreadable = unreadable.iter().any(|e| e.field == "currency");
        let currency = explicit_currency.or_else(|| {
            if currency_unreadable {
                return None;
            }
            let clean =
                |field: &str, value: bool| value && !recovered.iter().any(|r| r.field == field);
            [
                (&raw.subtotal, clean("subtotal", subtotal.is_some())),
                (&raw.tax, clean("tax", tax.is_some())),
                (&raw.total, clean("total", total.is_some())),
            ]
            .into_iter()
            .find_map(|(source, parsed)| {
                let source = present(source)?;
                (parsed && has_euro_suffix(&source.value)).then(|| {
                    let evidence = source
                        .evidence
                        .clone()
                        .unwrap_or_else(|| source.value.clone());
                    Evidenced::with_evidence(EUR.to_owned(), evidence)
                })
            })
        });
        Self {
            supplier: text_field(&raw.supplier),
            invoice_number: text_field(&raw.invoice_number),
            date,
            currency,
            subtotal,
            tax,
            total,
            unreadable,
            recovered,
        }
    }

    /// `true` si el campo estaba en el documento pero no se pudo interpretar.
    pub fn is_unreadable(&self, field: &str) -> bool {
        self.unreadable.iter().any(|e| e.field == field)
    }

    /// Campos en el formato del contrato del evaluador: cadenas normalizadas o `None`. La
    /// moneda no forma parte de ese contrato y se consulta en `currency`.
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

/// Interpreta un campo presente; si es ilegible lo anota en `unreadable` y devuelve `None`.
/// Importe: legible, recuperado como propuesta (D49) o ilegible.
fn amount_field(
    field: &Option<Evidenced<String>>,
    name: &'static str,
    unreadable: &mut Vec<FieldError>,
    recovered: &mut Vec<RecoveredAmount>,
) -> Option<Evidenced<Decimal>> {
    let e = present(field)?;
    if let Ok(value) = parse_amount(name, &e.value) {
        return Some(Evidenced {
            value,
            evidence: e.evidence.clone(),
        });
    }
    match recover_amount(&e.value) {
        Some((value, reason)) => {
            recovered.push(RecoveredAmount {
                field: name.to_owned(),
                literal: e.value.clone(),
                evidence: e.evidence.clone(),
                value: format!("{value:.2}"),
                reason,
            });
            Some(Evidenced {
                value,
                evidence: e.evidence.clone(),
            })
        }
        None => parsed_field(field, unreadable, |v| parse_amount(name, v)),
    }
}

fn parsed_field<T>(
    field: &Option<Evidenced<String>>,
    unreadable: &mut Vec<FieldError>,
    parse: impl Fn(&str) -> Result<T>,
) -> Option<Evidenced<T>> {
    let e = present(field)?;
    match parse(&e.value) {
        Ok(value) => Some(Evidenced {
            value,
            evidence: e.evidence.clone(),
        }),
        Err(error) => {
            let field_error = FieldError::from_error(error, e.evidence.clone())
                .unwrap_or_else(|other| unreachable!("error ajeno a un campo: {}", other.code()));
            unreadable.push(field_error);
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amounts_accept_point_or_comma_with_two_decimals() {
        let ok = |literal: &str, cents: i64| {
            assert_eq!(
                parse_amount("total", literal).unwrap(),
                Decimal::new(cents, 2),
                "{literal:?}"
            );
        };
        ok("121.00", 12100);
        ok(" 121.00 EUR ", 12100);
        ok("121.00€", 12100);
        ok("-5.10", -510);
        // Formato español y separadores de miles bien agrupados (D48).
        ok("121,00", 12100);
        ok("242,00 €", 24200);
        ok("-5,10 EUR", -510);
        ok("1.210,00", 121000);
        ok("1,210.00", 121000);
        ok("1 210,00", 121000);
        ok("1\u{a0}210,00\u{a0}€", 121000);
        ok("12.345.678,90", 1234567890);
        ok("0,50", 50);
        for bad in [
            // Sin dos decimales: ambiguo o incompleto.
            "121",
            "121.0",
            "121,0",
            "121.000",
            "1.234",
            "1,234",
            "1.210",
            ".50",
            ",50",
            // Separadores mal usados.
            "1,21,00",
            "1.21.00",
            "1.2100,00",
            "12,10.00.00",
            "1.210 000,00",
            "1 21,00",
            "1..210,00",
            "1.210,00.00",
            // No son importes.
            "abc",
            "",
            "EUR",
            "€",
            "12a.00",
            "--1.00",
            "1.210,0a",
            "+1.00",
            "1,210,00",
        ] {
            let err = parse_amount("total", bad).unwrap_err();
            assert_eq!(err.code(), "invalid_amount", "{bad:?}");
        }
    }

    #[test]
    fn doubtful_amounts_are_recovered_only_in_known_patterns() {
        let got =
            |literal: &str| recover_amount(literal).map(|(v, r)| (format!("{v:.2}"), r.code()));
        let some = |v: &str, r: &'static str| Some((v.to_owned(), r));
        assert_eq!(got("121"), some("121.00", "no_decimals"));
        assert_eq!(got("121 €"), some("121.00", "no_decimals"));
        assert_eq!(got("-7"), some("-7.00", "no_decimals"));
        assert_eq!(got("121,5"), some("121.50", "one_decimal"));
        assert_eq!(got("121.5 EUR"), some("121.50", "one_decimal"));
        assert_eq!(got("1.234"), some("1234.00", "ambiguous_separator"));
        assert_eq!(got("1,234"), some("1234.00", "ambiguous_separator"));
        assert_eq!(
            got("12.345.678"),
            some("12345678.00", "ambiguous_separator")
        );
        for none in [
            "",
            "abc",
            "EUR",
            "1,21,00",
            "12a.00",
            "1.2345",
            "1.23.456",
            "1.234,5,6",
            "1,234.5",
            "--1",
            "+1",
            "1..234",
            ".5",
            "1 234",
        ] {
            assert_eq!(got(none), None, "{none:?}");
        }
        // Lo que ya se lee sin duda no es una recuperación.
        assert!(parse_amount("total", "121,00").is_ok());
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
