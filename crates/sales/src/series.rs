//! Series y numeración (D73) y revisiones (D74).
//!
//! * Una serie por tipo y año de emisión: `P`, `A`, `F`, `FS`, `FR`; formato `F-2026-0001`.
//! * Los borradores no tienen número: solo [`SeriesCounter::next_number`] lo propone y
//!   [`SeriesCounter::commit`] lo consume, y `commit` solo acepta exactamente el siguiente,
//!   así que el contador no puede dejar huecos ni repetir. La persistencia debe llamar a
//!   `commit` en la misma transacción que fija el contenido (D73).
//! * Un número anulado (D82) no se reutiliza: el contador solo avanza.
//! * El año de la serie es el de la fecha de expedición.
//!
//! Interpretación conservadora: el correlativo se rellena a 4 cifras como mínimo; pasado
//! 9999 sigue con más cifras en vez de fallar. El albarán recibido conserva el número del
//! proveedor y no pasa por aquí.

use std::fmt;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocumentKind {
    Quote,
    DeliveryNote,
    Invoice,
    SimplifiedInvoice,
    CorrectiveInvoice,
}

impl DocumentKind {
    pub const ALL: [DocumentKind; 5] = [
        Self::Quote,
        Self::DeliveryNote,
        Self::Invoice,
        Self::SimplifiedInvoice,
        Self::CorrectiveInvoice,
    ];

    /// Prefijo de la serie (D73).
    pub fn series_prefix(self) -> &'static str {
        match self {
            Self::Quote => "P",
            Self::DeliveryNote => "A",
            Self::Invoice => "F",
            Self::SimplifiedInvoice => "FS",
            Self::CorrectiveInvoice => "FR",
        }
    }

    /// Valor de `kind` en `sales_documents` (D72).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Quote => "quote",
            Self::DeliveryNote => "delivery_note",
            Self::Invoice => "invoice",
            Self::SimplifiedInvoice => "simplified_invoice",
            Self::CorrectiveInvoice => "corrective_invoice",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.as_str() == text)
    }

    pub fn is_invoice(self) -> bool {
        matches!(
            self,
            Self::Invoice | Self::SimplifiedInvoice | Self::CorrectiveInvoice
        )
    }

    fn from_prefix(prefix: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.series_prefix() == prefix)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SeriesError {
    #[error("número de documento mal formado: {0:?}")]
    Malformed(String),
    #[error("año fuera de rango: {0}")]
    YearOutOfRange(u16),
    #[error("el número {got} no es el siguiente de la serie (se esperaba {expected})")]
    NotNext { expected: String, got: String },
    #[error("el número pertenece a otra serie: {got} en la serie {series}")]
    WrongSeries { series: String, got: String },
    #[error("se agotó el correlativo de la serie")]
    Exhausted,
}

/// Serie: tipo de documento y año de expedición.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SeriesKey {
    pub kind: DocumentKind,
    pub year: u16,
}

impl SeriesKey {
    pub fn new(kind: DocumentKind, year: u16) -> Result<Self, SeriesError> {
        if !(1..=9999).contains(&year) {
            return Err(SeriesError::YearOutOfRange(year));
        }
        Ok(Self { kind, year })
    }
}

impl fmt::Display for SeriesKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{:04}", self.kind.series_prefix(), self.year)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct DocumentNumber {
    pub series: SeriesKey,
    /// Correlativo, desde 1.
    pub sequence: u32,
}

impl fmt::Display for DocumentNumber {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{:04}", self.series, self.sequence)
    }
}

impl DocumentNumber {
    /// Interpreta `F-2026-0001`. Solo acepta la forma canónica (la misma que `Display`).
    pub fn parse(text: &str) -> Result<Self, SeriesError> {
        let bad = || SeriesError::Malformed(text.to_owned());
        let mut parts = text.split('-');
        let (prefix, year, seq) = (
            parts.next().ok_or_else(bad)?,
            parts.next().ok_or_else(bad)?,
            parts.next().ok_or_else(bad)?,
        );
        if parts.next().is_some() {
            return Err(bad());
        }
        let kind = DocumentKind::from_prefix(prefix).ok_or_else(bad)?;
        let numeric = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
        if !numeric(year) || !numeric(seq) || year.len() != 4 {
            return Err(bad());
        }
        let year: u16 = year.parse().map_err(|_| bad())?;
        let sequence: u32 = seq.parse().map_err(|_| bad())?;
        let number = Self {
            series: SeriesKey::new(kind, year)?,
            sequence,
        };
        if sequence == 0 || number.to_string() != text {
            return Err(bad());
        }
        Ok(number)
    }
}

/// Último número consumido de una serie. `last == 0` significa serie sin emitir.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeriesCounter {
    pub series: SeriesKey,
    last: u32,
}

impl SeriesCounter {
    pub fn new(series: SeriesKey) -> Self {
        Self { series, last: 0 }
    }

    /// Reconstruye el contador desde el último número guardado.
    pub fn with_last(series: SeriesKey, last: u32) -> Self {
        Self { series, last }
    }

    pub fn last(&self) -> u32 {
        self.last
    }

    /// Número que recibiría la próxima emisión. No modifica nada.
    pub fn next_number(&self) -> Result<DocumentNumber, SeriesError> {
        let sequence = self.last.checked_add(1).ok_or(SeriesError::Exhausted)?;
        Ok(DocumentNumber {
            series: self.series,
            sequence,
        })
    }

    /// Consume `number`; falla (sin cambiar el contador) si no es exactamente el siguiente
    /// de esta serie. Un error de emisión no debe llamar a este método (no consume número).
    pub fn commit(&mut self, number: &DocumentNumber) -> Result<(), SeriesError> {
        if number.series != self.series {
            return Err(SeriesError::WrongSeries {
                series: self.series.to_string(),
                got: number.to_string(),
            });
        }
        let expected = self.next_number()?;
        if *number != expected {
            return Err(SeriesError::NotNext {
                expected: expected.to_string(),
                got: number.to_string(),
            });
        }
        self.last = number.sequence;
        Ok(())
    }
}

/// Resultado de auditar los correlativos guardados de una serie.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SequenceAudit {
    pub missing: Vec<u32>,
    pub duplicated: Vec<u32>,
}

impl SequenceAudit {
    pub fn is_clean(&self) -> bool {
        self.missing.is_empty() && self.duplicated.is_empty()
    }
}

/// Comprueba que los correlativos emitidos (incluidos los de facturas anuladas, D82) sean
/// exactamente 1..=n sin huecos ni repeticiones. El orden de entrada es indiferente.
pub fn audit_sequence(sequences: &[u32]) -> SequenceAudit {
    let mut sorted = sequences.to_vec();
    sorted.sort_unstable();
    let mut audit = SequenceAudit::default();
    let mut expected = 1u32;
    let mut previous = None;
    for s in sorted {
        if previous == Some(s) {
            if audit.duplicated.last() != Some(&s) {
                audit.duplicated.push(s);
            }
            continue;
        }
        while expected < s {
            audit.missing.push(expected);
            expected += 1;
        }
        expected = s.saturating_add(1);
        previous = Some(s);
    }
    audit
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(kind: DocumentKind, year: u16) -> SeriesKey {
        SeriesKey::new(kind, year).unwrap()
    }

    #[test]
    fn d73_prefix_per_kind_and_format() {
        let expect = [
            (DocumentKind::Quote, "P-2026-0001"),
            (DocumentKind::DeliveryNote, "A-2026-0001"),
            (DocumentKind::Invoice, "F-2026-0001"),
            (DocumentKind::SimplifiedInvoice, "FS-2026-0001"),
            (DocumentKind::CorrectiveInvoice, "FR-2026-0001"),
        ];
        for (kind, text) in expect {
            let n = SeriesCounter::new(key(kind, 2026)).next_number().unwrap();
            assert_eq!(n.to_string(), text);
            assert_eq!(DocumentNumber::parse(text).unwrap(), n);
        }
    }

    #[test]
    fn d73_consecutive_numbers_without_gaps() {
        let mut c = SeriesCounter::new(key(DocumentKind::Invoice, 2027));
        let mut issued = Vec::new();
        for _ in 0..12 {
            let n = c.next_number().unwrap();
            c.commit(&n).unwrap();
            issued.push(n.sequence);
        }
        assert_eq!(issued, (1..=12).collect::<Vec<_>>());
        assert!(audit_sequence(&issued).is_clean());
        assert_eq!(c.next_number().unwrap().to_string(), "F-2027-0013");
    }

    #[test]
    fn d73_peek_does_not_consume_a_number() {
        // Un error de emisión no consume número: next_number sin commit no avanza.
        let c = SeriesCounter::new(key(DocumentKind::Quote, 2026));
        assert_eq!(c.next_number().unwrap(), c.next_number().unwrap());
        assert_eq!(c.last(), 0);
    }

    #[test]
    fn d73_commit_rejects_gap_and_duplicate_without_changing_counter() {
        let k = key(DocumentKind::Invoice, 2026);
        let mut c = SeriesCounter::with_last(k, 5);
        let skip = DocumentNumber {
            series: k,
            sequence: 7,
        };
        let repeat = DocumentNumber {
            series: k,
            sequence: 5,
        };
        assert!(matches!(c.commit(&skip), Err(SeriesError::NotNext { .. })));
        assert!(matches!(
            c.commit(&repeat),
            Err(SeriesError::NotNext { .. })
        ));
        assert_eq!(c.last(), 5);
        let next = c.next_number().unwrap();
        c.commit(&next).unwrap();
        assert_eq!(c.last(), 6);
        // Tras un corte, reconstruir desde el último guardado da el mismo siguiente.
        assert_eq!(
            SeriesCounter::with_last(k, 6)
                .next_number()
                .unwrap()
                .sequence,
            7
        );
    }

    #[test]
    fn d73_series_are_independent_per_year_and_kind() {
        let mut a = SeriesCounter::new(key(DocumentKind::Invoice, 2026));
        let mut b = SeriesCounter::new(key(DocumentKind::Invoice, 2027));
        let mut s = SeriesCounter::new(key(DocumentKind::SimplifiedInvoice, 2026));
        let n = a.next_number().unwrap();
        a.commit(&n).unwrap();
        // El número de otra serie (año o tipo) se rechaza.
        assert!(matches!(b.commit(&n), Err(SeriesError::WrongSeries { .. })));
        assert!(matches!(s.commit(&n), Err(SeriesError::WrongSeries { .. })));
        // El año nuevo empieza en 1.
        assert_eq!(b.next_number().unwrap().to_string(), "F-2027-0001");
    }

    #[test]
    fn d73_voided_number_is_never_reused() {
        // D82: la anulación no devuelve el número; el contador solo avanza.
        let mut c = SeriesCounter::new(key(DocumentKind::Invoice, 2026));
        let first = c.next_number().unwrap();
        c.commit(&first).unwrap();
        // (la factura se anula; el contador no retrocede)
        assert_eq!(c.next_number().unwrap().sequence, 2);
        assert!(audit_sequence(&[1, 2, 3]).is_clean());
    }

    #[test]
    fn d73_sequence_beyond_9999_keeps_going() {
        let k = key(DocumentKind::Invoice, 2026);
        let mut c = SeriesCounter::with_last(k, 9999);
        let n = c.next_number().unwrap();
        assert_eq!(n.to_string(), "F-2026-10000");
        c.commit(&n).unwrap();
        assert_eq!(DocumentNumber::parse("F-2026-10000").unwrap(), n);
    }

    #[test]
    fn d73_counter_exhaustion_is_an_error() {
        let c = SeriesCounter::with_last(key(DocumentKind::Invoice, 2026), u32::MAX);
        assert_eq!(c.next_number(), Err(SeriesError::Exhausted));
    }

    #[test]
    fn d73_parse_rejects_malformed_numbers() {
        for bad in [
            "",
            "F",
            "F-2026",
            "F-2026-",
            "F-26-0001",
            "X-2026-0001",
            "F-2026-0000",
            "F-2026-001",
            "F-2026-0001-1",
            "f-2026-0001",
            "F-2026-00a1",
            "F-0000-0001",
            "F-2026-+001",
            " F-2026-0001",
            "F-2026-00001",
            "P-2026-0001 ",
        ] {
            assert!(DocumentNumber::parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn d73_year_range() {
        assert!(SeriesKey::new(DocumentKind::Quote, 0).is_err());
        assert!(SeriesKey::new(DocumentKind::Quote, 10000).is_err());
        assert!(SeriesKey::new(DocumentKind::Quote, 2026).is_ok());
    }

    #[test]
    fn d73_audit_reports_gaps_and_duplicates() {
        assert!(audit_sequence(&[]).is_clean());
        assert!(audit_sequence(&[3, 1, 2]).is_clean());
        let a = audit_sequence(&[1, 2, 5, 5, 5, 8]);
        assert_eq!(a.missing, vec![3, 4, 6, 7]);
        assert_eq!(a.duplicated, vec![5]);
        // Falta el 1.
        assert_eq!(audit_sequence(&[2, 3]).missing, vec![1]);
    }

    #[test]
    fn d72_kind_strings_round_trip() {
        for k in DocumentKind::ALL {
            assert_eq!(DocumentKind::parse(k.as_str()), Some(k));
        }
        assert_eq!(DocumentKind::parse("order"), None);
        assert!(DocumentKind::Invoice.is_invoice());
        assert!(!DocumentKind::Quote.is_invoice());
    }
}
