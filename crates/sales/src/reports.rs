//! Libros de facturas, resumen trimestral de IVA, gastos por categoría y su exportación CSV
//! (R20, D89). Agregaciones puras sobre entradas mínimas, sin E/S: la escritura va a un
//! `impl Write` (ver [`crate::csv`]).
//!
//! D89: «libros de facturas emitidas y recibidas por periodo, y resumen trimestral de IVA
//! repercutido y soportado como ayuda para el modelo 303, sin presentar nada».
//!
//! **Integración pendiente.** Las facturas emitidas vendrán de la persistencia de venta
//! (migración 3, D72) y las recibidas de `nestadmin-documents`, cuyo `Invoice` aún no lleva el
//! NIF del proveedor, el desglose por tipo de IVA ni la categoría de gasto que añade D89. Por
//! eso este módulo define entradas mínimas ([`IssuedEntry`], [`ReceivedEntry`]) y quien integre
//! las construirá; una factura recibida sin fecha o sin desglose no puede entrar en un
//! informe y debe listarse aparte como incidencia (aquí: [`ReportError::MissingBreakdown`]).
//! El XLSX de D89 (`rust_xlsxwriter`) no se aborda: añade una dependencia.
//!
//! Interpretaciones conservadoras:
//! * **Una fila por factura y por tipo/causa** del desglose; el «Total» de una fila es base +
//!   cuota de esa fila (no el total de la factura, para que las columnas sumen).
//! * **Anuladas (D82)**: aparecen en el libro con estado `anulada` e importes `0,00` (el
//!   número no se reutiliza y debe verse), y no cuentan en ningún resumen.
//! * **Rectificativas por diferencias (D81)**: sus importes (posiblemente negativos) suman tal
//!   cual. **Por sustitución**: el efecto fiscal es la diferencia con la factura sustituida,
//!   por lo que la entrada lleva [`IssuedEntry::replaced`] (desglose de la original) y el
//!   resumen resta ese desglose; el libro lista los importes tal como se emitieron.
//! * El periodo se filtra por fecha de expedición (emitidas) y por fecha de la factura
//!   (recibidas), ambas inclusivas; no se modela la fecha de devengo ni la de contabilización.
//! * El resumen no aplica prorrata, deducibilidad, recargo de equivalencia ni compensaciones:
//!   es repercutido menos soportado, como ayuda. Orden de salida: por tipo ascendente.
//! * Gastos sin categoría se agrupan bajo «(sin categoría)», de forma explícita, y van al final.

use std::collections::BTreeMap;
use std::io::Write;

use nestadmin_documents::IsoDate;
use rust_decimal::Decimal;

use crate::calc::{round_money, TaxBreakdown};
use crate::csv::{Cell, CsvError, CsvOptions, CsvWriter};
use crate::dates;
use crate::series::{DocumentKind, DocumentNumber};

/// Etiqueta explícita de los gastos sin categoría.
pub const UNCATEGORIZED: &str = "(sin categoría)";

/// Rango de fechas inclusivo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DateRange {
    pub from: IsoDate,
    pub to: IsoDate,
}

#[derive(Debug, thiserror::Error)]
pub enum ReportError {
    #[error("rango de fechas inválido")]
    InvalidRange,
    #[error("trimestre inválido (1 a 4)")]
    InvalidQuarter,
    #[error("fecha inválida en {0}")]
    InvalidDate(String),
    #[error("el número {0} está repetido")]
    DuplicateNumber(String),
    #[error("{0} no es una factura")]
    NotAnInvoice(String),
    #[error("{0} no tiene desglose de IVA")]
    MissingBreakdown(String),
    #[error(transparent)]
    Csv(#[from] CsvError),
}

impl DateRange {
    pub fn new(from: IsoDate, to: IsoDate) -> Result<Self, ReportError> {
        if !dates::is_valid(from) || !dates::is_valid(to) || from > to {
            return Err(ReportError::InvalidRange);
        }
        Ok(Self { from, to })
    }

    /// Trimestre natural `quarter` (1 a 4) de `year`.
    pub fn quarter(year: u16, quarter: u8) -> Result<Self, ReportError> {
        if !(1..=4).contains(&quarter) || year == 0 {
            return Err(ReportError::InvalidQuarter);
        }
        let first_month = (quarter - 1) * 3 + 1;
        let last_month = first_month + 2;
        Self::new(
            IsoDate {
                year,
                month: first_month,
                day: 1,
            },
            IsoDate {
                year,
                month: last_month,
                day: dates::days_in_month(year, last_month),
            },
        )
    }

    pub fn contains(&self, date: IsoDate) -> bool {
        self.from <= date && date <= self.to
    }
}

/// Factura emitida tal como la necesitan los libros.
#[derive(Debug, Clone, PartialEq)]
pub struct IssuedEntry {
    pub number: DocumentNumber,
    pub kind: DocumentKind,
    pub issue_date: IsoDate,
    pub customer_name: Option<String>,
    pub customer_tax_id: Option<String>,
    pub voided: bool,
    pub breakdown: Vec<TaxBreakdown>,
    /// Solo en una rectificativa por sustitución: desglose de la factura sustituida.
    pub replaced: Vec<TaxBreakdown>,
}

/// Línea de IVA de una factura recibida; el tipo es libre porque viene de una extracción.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceivedTaxLine {
    pub percent: Decimal,
    pub base: Decimal,
    pub tax: Decimal,
}

/// Factura recibida tal como la necesitan los libros.
#[derive(Debug, Clone, PartialEq)]
pub struct ReceivedEntry {
    pub supplier_name: Option<String>,
    pub supplier_tax_id: Option<String>,
    pub number: Option<String>,
    pub date: IsoDate,
    pub lines: Vec<ReceivedTaxLine>,
    pub expense_category: Option<String>,
}

impl ReceivedEntry {
    fn label(&self) -> String {
        match (&self.number, &self.supplier_name) {
            (Some(n), _) => format!("la factura recibida {n}"),
            (None, Some(s)) => format!("una factura recibida de {s}"),
            (None, None) => format!("una factura recibida del {}", self.date),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VatLine {
    pub percent: Decimal,
    pub base: Decimal,
    pub tax: Decimal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VatSummary {
    pub range: DateRange,
    pub output: Vec<VatLine>,
    pub input: Vec<VatLine>,
    pub output_base: Decimal,
    pub output_tax: Decimal,
    pub input_base: Decimal,
    pub input_tax: Decimal,
    /// Cuota repercutida − cuota soportada (positivo: a ingresar; negativo: a compensar).
    pub balance: Decimal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpenseLine {
    pub category: Option<String>,
    pub base: Decimal,
    pub tax: Decimal,
}

fn check_issued(entries: &[IssuedEntry]) -> Result<(), ReportError> {
    let mut seen = std::collections::HashSet::new();
    for e in entries {
        let number = e.number.to_string();
        if !e.kind.is_invoice() {
            return Err(ReportError::NotAnInvoice(number));
        }
        if !dates::is_valid(e.issue_date) {
            return Err(ReportError::InvalidDate(number));
        }
        if !seen.insert(e.number) {
            return Err(ReportError::DuplicateNumber(number));
        }
    }
    Ok(())
}

fn issued_in_range<'a>(
    entries: &'a [IssuedEntry],
    range: &DateRange,
) -> Result<Vec<&'a IssuedEntry>, ReportError> {
    check_issued(entries)?;
    let mut selected: Vec<&IssuedEntry> = entries
        .iter()
        .filter(|e| range.contains(e.issue_date))
        .collect();
    for e in &selected {
        if !e.voided && e.breakdown.is_empty() {
            return Err(ReportError::MissingBreakdown(e.number.to_string()));
        }
    }
    selected.sort_by_key(|e| (e.issue_date, e.number));
    Ok(selected)
}

fn received_in_range<'a>(
    entries: &'a [ReceivedEntry],
    range: &DateRange,
) -> Result<Vec<&'a ReceivedEntry>, ReportError> {
    for e in entries {
        if !dates::is_valid(e.date) {
            return Err(ReportError::InvalidDate(e.label()));
        }
    }
    let mut selected: Vec<&ReceivedEntry> =
        entries.iter().filter(|e| range.contains(e.date)).collect();
    for e in &selected {
        if e.lines.is_empty() {
            return Err(ReportError::MissingBreakdown(e.label()));
        }
    }
    selected.sort_by(|a, b| {
        (a.date, &a.number, &a.supplier_tax_id, &a.supplier_name).cmp(&(
            b.date,
            &b.number,
            &b.supplier_tax_id,
            &b.supplier_name,
        ))
    });
    Ok(selected)
}

fn sorted_breakdown(breakdown: &[TaxBreakdown]) -> Vec<&TaxBreakdown> {
    let mut rows: Vec<&TaxBreakdown> = breakdown.iter().collect();
    rows.sort_by(|a, b| (a.tax_rate, &a.exemption).cmp(&(b.tax_rate, &b.exemption)));
    rows
}

fn add_to(
    map: &mut BTreeMap<Decimal, (Decimal, Decimal)>,
    percent: Decimal,
    base: Decimal,
    tax: Decimal,
) {
    let slot = map
        .entry(percent.normalize())
        .or_insert((Decimal::ZERO, Decimal::ZERO));
    slot.0 += base;
    slot.1 += tax;
}

fn finish_lines(map: BTreeMap<Decimal, (Decimal, Decimal)>) -> (Vec<VatLine>, Decimal, Decimal) {
    let mut base_total = Decimal::ZERO;
    let mut tax_total = Decimal::ZERO;
    let lines = map
        .into_iter()
        .map(|(percent, (base, tax))| {
            base_total += base;
            tax_total += tax;
            VatLine {
                percent,
                base: round_money(base),
                tax: round_money(tax),
            }
        })
        .collect();
    (lines, round_money(base_total), round_money(tax_total))
}

/// Resumen de IVA repercutido y soportado del rango (D89).
pub fn vat_summary(
    issued: &[IssuedEntry],
    received: &[ReceivedEntry],
    range: DateRange,
) -> Result<VatSummary, ReportError> {
    let mut output = BTreeMap::new();
    for e in issued_in_range(issued, &range)? {
        if e.voided {
            continue;
        }
        for b in &e.breakdown {
            add_to(&mut output, b.tax_rate.percent(), b.base, b.tax);
        }
        for b in &e.replaced {
            add_to(&mut output, b.tax_rate.percent(), -b.base, -b.tax);
        }
    }
    let mut input = BTreeMap::new();
    for e in received_in_range(received, &range)? {
        for l in &e.lines {
            add_to(&mut input, l.percent, l.base, l.tax);
        }
    }
    let (output, output_base, output_tax) = finish_lines(output);
    let (input, input_base, input_tax) = finish_lines(input);
    Ok(VatSummary {
        range,
        output,
        input,
        output_base,
        output_tax,
        input_base,
        input_tax,
        balance: round_money(output_tax - input_tax),
    })
}

/// Base y cuota de las facturas recibidas del rango por categoría de gasto. Las categorías con
/// nombre van en orden alfabético y «sin categoría» (`None`) al final.
pub fn expenses_by_category(
    received: &[ReceivedEntry],
    range: DateRange,
) -> Result<Vec<ExpenseLine>, ReportError> {
    let mut map: BTreeMap<Option<String>, (Decimal, Decimal)> = BTreeMap::new();
    for e in received_in_range(received, &range)? {
        let key = e
            .expense_category
            .as_deref()
            .map(str::trim)
            .filter(|c| !c.is_empty())
            .map(str::to_owned);
        let slot = map.entry(key).or_insert((Decimal::ZERO, Decimal::ZERO));
        for l in &e.lines {
            slot.0 += l.base;
            slot.1 += l.tax;
        }
    }
    let mut lines: Vec<ExpenseLine> = map
        .into_iter()
        .map(|(category, (base, tax))| ExpenseLine {
            category,
            base: round_money(base),
            tax: round_money(tax),
        })
        .collect();
    // `None` ordena antes que `Some` en `Option`; se mueve al final de forma explícita.
    lines.sort_by(|a, b| match (&a.category, &b.category) {
        (None, None) => std::cmp::Ordering::Equal,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (Some(_), None) => std::cmp::Ordering::Less,
        (Some(x), Some(y)) => x.cmp(y),
    });
    Ok(lines)
}

/// Libro de facturas emitidas del rango en CSV (D89).
pub fn write_issued_book<W: Write>(
    out: W,
    entries: &[IssuedEntry],
    range: DateRange,
    options: CsvOptions,
) -> Result<W, ReportError> {
    let selected = issued_in_range(entries, &range)?;
    let mut w = CsvWriter::new(
        out,
        options,
        &[
            "Número",
            "Fecha de expedición",
            "Tipo de documento",
            "NIF cliente",
            "Cliente",
            "Estado",
            "Tipo de IVA (%)",
            "Causa de exención",
            "Base imponible",
            "Cuota IVA",
            "Total",
        ],
    )?;
    for e in selected {
        let head = |state: &str| {
            vec![
                Cell::text(e.number.to_string()),
                Cell::Date(e.issue_date),
                Cell::text(e.kind.as_str()),
                Cell::opt_text(e.customer_tax_id.as_deref()),
                Cell::opt_text(e.customer_name.as_deref()),
                Cell::text(state),
            ]
        };
        if e.voided {
            let mut row = head("anulada");
            row.extend([
                Cell::Empty,
                Cell::Empty,
                Cell::Amount(Decimal::ZERO),
                Cell::Amount(Decimal::ZERO),
                Cell::Amount(Decimal::ZERO),
            ]);
            w.row(&row)?;
            continue;
        }
        for b in sorted_breakdown(&e.breakdown) {
            let mut row = head("emitida");
            row.extend([
                Cell::Number(b.tax_rate.percent()),
                Cell::opt_text(b.exemption.as_ref().map(|x| x.cause.as_str())),
                Cell::Amount(b.base),
                Cell::Amount(b.tax),
                Cell::Amount(b.base + b.tax),
            ]);
            w.row(&row)?;
        }
    }
    Ok(w.finish()?)
}

/// Libro de facturas recibidas del rango en CSV (D89).
pub fn write_received_book<W: Write>(
    out: W,
    entries: &[ReceivedEntry],
    range: DateRange,
    options: CsvOptions,
) -> Result<W, ReportError> {
    let selected = received_in_range(entries, &range)?;
    let mut w = CsvWriter::new(
        out,
        options,
        &[
            "Fecha",
            "Número",
            "NIF proveedor",
            "Proveedor",
            "Categoría de gasto",
            "Tipo de IVA (%)",
            "Base imponible",
            "Cuota IVA",
            "Total",
        ],
    )?;
    for e in selected {
        let mut lines: Vec<&ReceivedTaxLine> = e.lines.iter().collect();
        lines.sort_by_key(|l| l.percent);
        for l in lines {
            w.row(&[
                Cell::Date(e.date),
                Cell::opt_text(e.number.as_deref()),
                Cell::opt_text(e.supplier_tax_id.as_deref()),
                Cell::opt_text(e.supplier_name.as_deref()),
                Cell::opt_text(e.expense_category.as_deref()),
                Cell::Number(l.percent),
                Cell::Amount(l.base),
                Cell::Amount(l.tax),
                Cell::Amount(l.base + l.tax),
            ])?;
        }
    }
    Ok(w.finish()?)
}

/// Resumen de IVA en CSV: filas por tipo, totales y resultado (D89).
pub fn write_vat_summary<W: Write>(
    out: W,
    summary: &VatSummary,
    options: CsvOptions,
) -> Result<W, ReportError> {
    let mut w = CsvWriter::new(
        out,
        options,
        &["Concepto", "Tipo de IVA (%)", "Base imponible", "Cuota IVA"],
    )?;
    let mut section = |label: &str,
                       total_label: &str,
                       lines: &[VatLine],
                       base: Decimal,
                       tax: Decimal|
     -> Result<(), CsvError> {
        for l in lines {
            w.row(&[
                Cell::text(label),
                Cell::Number(l.percent),
                Cell::Amount(l.base),
                Cell::Amount(l.tax),
            ])?;
        }
        w.row(&[
            Cell::text(total_label),
            Cell::Empty,
            Cell::Amount(base),
            Cell::Amount(tax),
        ])
    };
    section(
        "IVA repercutido",
        "Total IVA repercutido",
        &summary.output,
        summary.output_base,
        summary.output_tax,
    )?;
    section(
        "IVA soportado",
        "Total IVA soportado",
        &summary.input,
        summary.input_base,
        summary.input_tax,
    )?;
    w.row(&[
        Cell::text("Resultado (repercutido menos soportado)"),
        Cell::Empty,
        Cell::Empty,
        Cell::Amount(summary.balance),
    ])?;
    Ok(w.finish()?)
}

/// Gastos por categoría en CSV (D89).
pub fn write_expenses<W: Write>(
    out: W,
    lines: &[ExpenseLine],
    options: CsvOptions,
) -> Result<W, ReportError> {
    let mut w = CsvWriter::new(
        out,
        options,
        &["Categoría de gasto", "Base imponible", "Cuota IVA", "Total"],
    )?;
    for l in lines {
        w.row(&[
            Cell::text(l.category.as_deref().unwrap_or(UNCATEGORIZED)),
            Cell::Amount(l.base),
            Cell::Amount(l.tax),
            Cell::Amount(l.base + l.tax),
        ])?;
    }
    Ok(w.finish()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calc::{Exemption, ExemptionKind, TaxRate};
    use std::str::FromStr;

    fn dec(s: &str) -> Decimal {
        Decimal::from_str(s).unwrap()
    }

    fn date(y: u16, m: u8, d: u8) -> IsoDate {
        IsoDate {
            year: y,
            month: m,
            day: d,
        }
    }

    fn row(rate: TaxRate, base: &str, tax: &str) -> TaxBreakdown {
        TaxBreakdown {
            tax_rate: rate,
            exemption: None,
            base: dec(base),
            tax: dec(tax),
        }
    }

    fn issued(
        number: &str,
        kind: DocumentKind,
        d: IsoDate,
        customer: Option<(&str, &str)>,
        breakdown: Vec<TaxBreakdown>,
    ) -> IssuedEntry {
        IssuedEntry {
            number: DocumentNumber::parse(number).unwrap(),
            kind,
            issue_date: d,
            customer_name: customer.map(|c| c.0.to_owned()),
            customer_tax_id: customer.map(|c| c.1.to_owned()),
            voided: false,
            breakdown,
            replaced: Vec::new(),
        }
    }

    fn sample_issued() -> Vec<IssuedEntry> {
        let uno = Some(("Cliente Uno SL", "B00000001"));
        let dos = Some(("Cliente Dos; S.L.", "B00000002"));
        let mut voided = issued(
            "F-2026-0003",
            DocumentKind::Invoice,
            date(2026, 3, 1),
            uno,
            vec![row(TaxRate::TwentyOne, "1000.00", "210.00")],
        );
        voided.voided = true;
        vec![
            // Desordenadas a propósito: el libro ordena por fecha y número.
            issued(
                "FR-2026-0001",
                DocumentKind::CorrectiveInvoice,
                date(2026, 3, 20),
                uno,
                vec![row(TaxRate::TwentyOne, "-20.00", "-4.20")],
            ),
            issued(
                "F-2026-0004",
                DocumentKind::Invoice,
                date(2026, 4, 2),
                uno,
                vec![row(TaxRate::TwentyOne, "500.00", "105.00")],
            ),
            voided,
            issued(
                "FS-2026-0001",
                DocumentKind::SimplifiedInvoice,
                date(2026, 2, 11),
                None,
                vec![row(TaxRate::TwentyOne, "10.00", "2.10")],
            ),
            issued(
                "F-2026-0002",
                DocumentKind::Invoice,
                date(2026, 2, 10),
                dos,
                vec![row(TaxRate::TwentyOne, "200.00", "42.00")],
            ),
            issued(
                "F-2026-0001",
                DocumentKind::Invoice,
                date(2026, 1, 15),
                uno,
                vec![
                    row(TaxRate::TwentyOne, "100.00", "21.00"),
                    row(TaxRate::Ten, "50.00", "5.00"),
                ],
            ),
        ]
    }

    fn received(
        number: Option<&str>,
        supplier: Option<(&str, &str)>,
        d: IsoDate,
        category: Option<&str>,
        lines: &[(&str, &str, &str)],
    ) -> ReceivedEntry {
        ReceivedEntry {
            supplier_name: supplier.map(|s| s.0.to_owned()),
            supplier_tax_id: supplier.map(|s| s.1.to_owned()),
            number: number.map(str::to_owned),
            date: d,
            lines: lines
                .iter()
                .map(|(p, b, t)| ReceivedTaxLine {
                    percent: dec(p),
                    base: dec(b),
                    tax: dec(t),
                })
                .collect(),
            expense_category: category.map(str::to_owned),
        }
    }

    fn sample_received() -> Vec<ReceivedEntry> {
        vec![
            received(
                None,
                None,
                date(2026, 3, 31),
                None,
                &[("21", "20.00", "4.20"), ("10", "10.00", "1.00")],
            ),
            received(
                Some("PROV-001"),
                Some(("Proveedor Uno", "A00000001")),
                date(2026, 1, 20),
                Some("Suministros"),
                &[("21", "80.00", "16.80")],
            ),
            received(
                Some("PROV-002"),
                Some(("Proveedor Dos", "A00000002")),
                date(2026, 4, 1),
                Some("Suministros"),
                &[("21", "999.00", "209.79")],
            ),
        ]
    }

    fn q1() -> DateRange {
        DateRange::quarter(2026, 1).unwrap()
    }

    fn utf8(bytes: Vec<u8>) -> String {
        String::from_utf8(bytes).unwrap()
    }

    #[test]
    fn d89_quarter_ranges() {
        let r = DateRange::quarter(2026, 1).unwrap();
        assert_eq!((r.from, r.to), (date(2026, 1, 1), date(2026, 3, 31)));
        let r = DateRange::quarter(2028, 2).unwrap();
        assert_eq!((r.from, r.to), (date(2028, 4, 1), date(2028, 6, 30)));
        let r = DateRange::quarter(2028, 4).unwrap();
        assert_eq!((r.from, r.to), (date(2028, 10, 1), date(2028, 12, 31)));
        assert!(DateRange::quarter(2026, 0).is_err());
        assert!(DateRange::quarter(2026, 5).is_err());
        assert!(DateRange::new(date(2026, 2, 1), date(2026, 1, 1)).is_err());
        assert!(DateRange::new(date(2026, 2, 30), date(2026, 3, 1)).is_err());
        assert!(r.contains(date(2028, 10, 1)) && r.contains(date(2028, 12, 31)));
        assert!(!r.contains(date(2028, 9, 30)));
    }

    #[test]
    fn d89_issued_book_golden() {
        let out = write_issued_book(
            Vec::new(),
            &sample_issued(),
            q1(),
            CsvOptions { bom: false },
        )
        .unwrap();
        let expected = "Número;Fecha de expedición;Tipo de documento;NIF cliente;Cliente;Estado;Tipo de IVA (%);Causa de exención;Base imponible;Cuota IVA;Total\r\n\
F-2026-0001;2026-01-15;invoice;B00000001;Cliente Uno SL;emitida;10;;50,00;5,00;55,00\r\n\
F-2026-0001;2026-01-15;invoice;B00000001;Cliente Uno SL;emitida;21;;100,00;21,00;121,00\r\n\
F-2026-0002;2026-02-10;invoice;B00000002;\"Cliente Dos; S.L.\";emitida;21;;200,00;42,00;242,00\r\n\
FS-2026-0001;2026-02-11;simplified_invoice;;;emitida;21;;10,00;2,10;12,10\r\n\
F-2026-0003;2026-03-01;invoice;B00000001;Cliente Uno SL;anulada;;;0,00;0,00;0,00\r\n\
FR-2026-0001;2026-03-20;corrective_invoice;B00000001;Cliente Uno SL;emitida;21;;-20,00;-4,20;-24,20\r\n";
        assert_eq!(utf8(out), expected);
    }

    #[test]
    fn d89_issued_book_is_deterministic_regardless_of_input_order() {
        let mut reversed = sample_issued();
        reversed.reverse();
        let a =
            write_issued_book(Vec::new(), &sample_issued(), q1(), CsvOptions::default()).unwrap();
        let b = write_issued_book(Vec::new(), &reversed, q1(), CsvOptions::default()).unwrap();
        assert_eq!(a, b);
        assert_eq!(&a[..3], b"\xEF\xBB\xBF");
    }

    #[test]
    fn d89_issued_book_shows_exemption_cause() {
        let mut e = issued(
            "F-2026-0001",
            DocumentKind::Invoice,
            date(2026, 1, 5),
            Some(("Cliente", "B00000001")),
            vec![TaxBreakdown {
                tax_rate: TaxRate::Zero,
                exemption: Some(Exemption {
                    kind: ExemptionKind::Exempt,
                    cause: "Exenta art. sintético".into(),
                }),
                base: dec("30.00"),
                tax: dec("0.00"),
            }],
        );
        e.replaced = Vec::new();
        let out = write_issued_book(Vec::new(), &[e], q1(), CsvOptions { bom: false }).unwrap();
        assert!(utf8(out).ends_with(
            "F-2026-0001;2026-01-05;invoice;B00000001;Cliente;emitida;0;Exenta art. sintético;30,00;0,00;30,00\r\n"
        ));
    }

    #[test]
    fn d89_issued_book_rejects_inconsistent_data() {
        let mut dup = sample_issued();
        dup.push(dup[0].clone());
        assert!(matches!(
            write_issued_book(Vec::new(), &dup, q1(), CsvOptions::default()),
            Err(ReportError::DuplicateNumber(_))
        ));
        let mut empty = sample_issued();
        empty[0].breakdown.clear();
        assert!(matches!(
            write_issued_book(Vec::new(), &empty, q1(), CsvOptions::default()),
            Err(ReportError::MissingBreakdown(_))
        ));
        let mut quote = sample_issued();
        quote[0].kind = DocumentKind::Quote;
        assert!(matches!(
            write_issued_book(Vec::new(), &quote, q1(), CsvOptions::default()),
            Err(ReportError::NotAnInvoice(_))
        ));
        let mut bad_date = sample_issued();
        bad_date[0].issue_date = date(2026, 2, 30);
        assert!(matches!(
            write_issued_book(Vec::new(), &bad_date, q1(), CsvOptions::default()),
            Err(ReportError::InvalidDate(_))
        ));
    }

    #[test]
    fn d89_empty_period_writes_only_headers() {
        let out = write_issued_book(
            Vec::new(),
            &sample_issued(),
            DateRange::quarter(2025, 1).unwrap(),
            CsvOptions { bom: false },
        )
        .unwrap();
        assert_eq!(utf8(out).lines().count(), 1);
    }

    #[test]
    fn d89_received_book_golden() {
        let out = write_received_book(
            Vec::new(),
            &sample_received(),
            q1(),
            CsvOptions { bom: false },
        )
        .unwrap();
        let expected = "Fecha;Número;NIF proveedor;Proveedor;Categoría de gasto;Tipo de IVA (%);Base imponible;Cuota IVA;Total\r\n\
2026-01-20;PROV-001;A00000001;Proveedor Uno;Suministros;21;80,00;16,80;96,80\r\n\
2026-03-31;;;;;10;10,00;1,00;11,00\r\n\
2026-03-31;;;;;21;20,00;4,20;24,20\r\n";
        assert_eq!(utf8(out), expected);
    }

    #[test]
    fn d89_received_without_breakdown_is_an_explicit_error() {
        let mut r = sample_received();
        r[0].lines.clear();
        assert!(matches!(
            write_received_book(Vec::new(), &r, q1(), CsvOptions::default()),
            Err(ReportError::MissingBreakdown(_))
        ));
        // Fuera del periodo no molesta.
        r[2].lines.clear();
        let only_april = DateRange::new(date(2026, 4, 1), date(2026, 4, 30)).unwrap();
        assert!(matches!(
            write_received_book(Vec::new(), &r, only_april, CsvOptions::default()),
            Err(ReportError::MissingBreakdown(_))
        ));
        assert!(write_received_book(
            Vec::new(),
            &r,
            DateRange::new(date(2026, 5, 1), date(2026, 5, 31)).unwrap(),
            CsvOptions::default()
        )
        .is_ok());
    }

    #[test]
    fn d89_vat_summary_known_result() {
        let s = vat_summary(&sample_issued(), &sample_received(), q1()).unwrap();
        // Repercutido 21 %: 100 + 200 + 10 - 20 = 290 / 21 + 42 + 2,10 - 4,20 = 60,90;
        // 10 %: 50 / 5. La anulada y la de abril no cuentan.
        assert_eq!(
            s.output,
            vec![
                VatLine {
                    percent: dec("10"),
                    base: dec("50.00"),
                    tax: dec("5.00")
                },
                VatLine {
                    percent: dec("21"),
                    base: dec("290.00"),
                    tax: dec("60.90")
                },
            ]
        );
        assert_eq!(s.output_base.to_string(), "340.00");
        assert_eq!(s.output_tax.to_string(), "65.90");
        // Soportado 21 %: 80 + 20 = 100 / 21,00; 10 %: 10 / 1.
        assert_eq!(s.input_base.to_string(), "110.00");
        assert_eq!(s.input_tax.to_string(), "22.00");
        assert_eq!(s.input.len(), 2);
        assert_eq!(s.balance.to_string(), "43.90");
    }

    #[test]
    fn d89_vat_summary_csv_golden() {
        let s = vat_summary(&sample_issued(), &sample_received(), q1()).unwrap();
        let out = write_vat_summary(Vec::new(), &s, CsvOptions { bom: false }).unwrap();
        let expected = "Concepto;Tipo de IVA (%);Base imponible;Cuota IVA\r\n\
IVA repercutido;10;50,00;5,00\r\n\
IVA repercutido;21;290,00;60,90\r\n\
Total IVA repercutido;;340,00;65,90\r\n\
IVA soportado;10;10,00;1,00\r\n\
IVA soportado;21;100,00;21,00\r\n\
Total IVA soportado;;110,00;22,00\r\n\
Resultado (repercutido menos soportado);;;43,90\r\n";
        assert_eq!(utf8(out), expected);
    }

    #[test]
    fn d89_vat_summary_substitution_counts_only_the_difference() {
        let original = issued(
            "F-2026-0001",
            DocumentKind::Invoice,
            date(2026, 1, 10),
            Some(("C", "B00000001")),
            vec![row(TaxRate::TwentyOne, "100.00", "21.00")],
        );
        let mut substitution = issued(
            "FR-2026-0001",
            DocumentKind::CorrectiveInvoice,
            date(2026, 2, 10),
            Some(("C", "B00000001")),
            vec![row(TaxRate::TwentyOne, "120.00", "25.20")],
        );
        substitution.replaced = vec![row(TaxRate::TwentyOne, "100.00", "21.00")];
        let s = vat_summary(&[original, substitution], &[], q1()).unwrap();
        // 100 (original) + (120 - 100) = 120; cuota 21 + 4,20 = 25,20.
        assert_eq!(s.output_base.to_string(), "120.00");
        assert_eq!(s.output_tax.to_string(), "25.20");
    }

    #[test]
    fn d89_vat_summary_negative_balance_is_to_compensate() {
        let s = vat_summary(&[], &sample_received(), q1()).unwrap();
        assert_eq!(s.output_tax.to_string(), "0.00");
        assert_eq!(s.balance.to_string(), "-22.00");
        assert!(s.output.is_empty());
    }

    #[test]
    fn d89_vat_summary_rounds_and_merges_equivalent_percents() {
        let r = vec![
            received(
                None,
                None,
                date(2026, 1, 1),
                None,
                &[("21.0", "0.005", "0.001")],
            ),
            received(
                None,
                None,
                date(2026, 1, 2),
                None,
                &[("21", "0.005", "0.001")],
            ),
        ];
        let s = vat_summary(&[], &r, q1()).unwrap();
        assert_eq!(s.input.len(), 1);
        assert_eq!(s.input[0].base.to_string(), "0.01");
        assert_eq!(s.input[0].percent.to_string(), "21");
    }

    #[test]
    fn d89_expenses_by_category_golden() {
        let mut r = sample_received();
        r.push(received(
            Some("PROV-003"),
            Some(("Proveedor Tres", "A00000003")),
            date(2026, 2, 2),
            Some("Alquiler"),
            &[("21", "300.00", "63.00")],
        ));
        r.push(received(
            Some("PROV-004"),
            None,
            date(2026, 2, 3),
            Some("  "),
            &[("4", "10.00", "0.40")],
        ));
        let lines = expenses_by_category(&r, q1()).unwrap();
        let out = write_expenses(Vec::new(), &lines, CsvOptions { bom: false }).unwrap();
        let expected = "Categoría de gasto;Base imponible;Cuota IVA;Total\r\n\
Alquiler;300,00;63,00;363,00\r\n\
Suministros;80,00;16,80;96,80\r\n\
(sin categoría);40,00;5,60;45,60\r\n";
        assert_eq!(utf8(out), expected);
    }

    #[test]
    fn d89_expenses_empty_range() {
        let lines =
            expenses_by_category(&sample_received(), DateRange::quarter(2025, 3).unwrap()).unwrap();
        assert!(lines.is_empty());
    }
}
