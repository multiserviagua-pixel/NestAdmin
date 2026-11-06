//! Escritor CSV determinista para las exportaciones al asesor (R20, D89).
//!
//! D89: «Exportación CSV (UTF-8, separador `;`, decimales con coma)». Escribe a un
//! `impl Write`: no abre archivos ni toca el disco.
//!
//! Decisiones donde la spec calla (conservadoras):
//! * **BOM UTF-8 al inicio** por defecto ([`CsvOptions::default`]), para que Excel en español
//!   reconozca la codificación y no estropee las tildes; se puede desactivar. *Propuesta*: no
//!   consta en D89.
//! * Fin de línea `\r\n` (RFC 4180), también tras la última fila.
//! * Importes con dos decimales, coma decimal y **sin separador de miles** (`1234,50`); el
//!   cero negativo se escribe `0,00`.
//! * Fechas en ISO `AAAA-MM-DD` (inequívocas, ordenables y que Excel interpreta).
//! * Ausencia de dato = celda vacía; el consumidor puede distinguirla de `0,00`.
//! * Entrecomillado solo cuando hace falta (contiene `;`, `"`, salto de línea o espacios
//!   extremos); las comillas se duplican.
//! * **Inyección de fórmulas**: un texto que empieza por `=`, `+`, `-`, `@`, tabulador o
//!   retorno de carro se antepone con `'` para que una hoja de cálculo no lo ejecute. Solo
//!   afecta a celdas de texto, nunca a importes, y altera el dato; es la mitigación habitual.

use std::io::{self, Write};

use nestadmin_documents::IsoDate;
use rust_decimal::Decimal;

use crate::calc::round_money;

const BOM: &[u8] = b"\xEF\xBB\xBF";
const SEPARATOR: u8 = b';';
const EOL: &[u8] = b"\r\n";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CsvOptions {
    pub bom: bool,
}

impl Default for CsvOptions {
    fn default() -> Self {
        Self { bom: true }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Cell {
    Empty,
    Text(String),
    /// Importe en euros (se redondea a dos decimales con la regla de D75).
    Amount(Decimal),
    /// Número sin redondeo forzado (porcentajes): se escribe normalizado con coma decimal.
    Number(Decimal),
    Date(IsoDate),
}

impl Cell {
    pub fn text(value: impl Into<String>) -> Self {
        Self::Text(value.into())
    }

    pub fn opt_text(value: Option<&str>) -> Self {
        value.map_or(Self::Empty, |v| {
            if v.is_empty() {
                Self::Empty
            } else {
                Self::text(v)
            }
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CsvError {
    #[error("error de escritura: {0}")]
    Io(#[from] io::Error),
    #[error("la fila tiene {found} celdas y la cabecera {expected}")]
    ColumnCount { expected: usize, found: usize },
}

/// Importe con coma decimal y dos decimales, sin miles: `1234.5` -> `1234,50`.
pub fn format_amount(value: Decimal) -> String {
    round_money(value).to_string().replace('.', ",")
}

/// Número normalizado con coma decimal: `21.0` -> `21`, `5.50` -> `5,5`.
pub fn format_number(value: Decimal) -> String {
    value.normalize().to_string().replace('.', ",")
}

/// Escapa una celda de texto según las reglas del módulo.
pub fn escape_text(text: &str) -> String {
    let mut value = text.to_owned();
    if matches!(
        value.chars().next(),
        Some('=' | '+' | '-' | '@' | '\t' | '\r')
    ) {
        value.insert(0, '\'');
    }
    let needs_quotes =
        value.contains([';', '"', '\n', '\r']) || value.starts_with(' ') || value.ends_with(' ');
    if needs_quotes {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value
    }
}

fn render(cell: &Cell) -> String {
    match cell {
        Cell::Empty => String::new(),
        Cell::Text(t) => escape_text(t),
        Cell::Amount(d) => format_amount(*d),
        Cell::Number(d) => format_number(*d),
        Cell::Date(d) => d.to_string(),
    }
}

/// Escritor de filas con número fijo de columnas, fijado por la cabecera.
pub struct CsvWriter<W: Write> {
    out: W,
    columns: usize,
}

impl<W: Write> CsvWriter<W> {
    /// Escribe el BOM (si procede) y la fila de cabeceras.
    pub fn new(mut out: W, options: CsvOptions, headers: &[&str]) -> Result<Self, CsvError> {
        if options.bom {
            out.write_all(BOM)?;
        }
        let mut writer = Self {
            out,
            columns: headers.len(),
        };
        let cells: Vec<Cell> = headers.iter().map(|h| Cell::text(*h)).collect();
        writer.row(&cells)?;
        Ok(writer)
    }

    pub fn row(&mut self, cells: &[Cell]) -> Result<(), CsvError> {
        if cells.len() != self.columns {
            return Err(CsvError::ColumnCount {
                expected: self.columns,
                found: cells.len(),
            });
        }
        let mut line = Vec::new();
        for (i, cell) in cells.iter().enumerate() {
            if i > 0 {
                line.push(SEPARATOR);
            }
            line.extend_from_slice(render(cell).as_bytes());
        }
        line.extend_from_slice(EOL);
        // Una sola escritura por fila: un fallo no deja una fila a medias.
        self.out.write_all(&line)?;
        Ok(())
    }

    pub fn finish(mut self) -> Result<W, CsvError> {
        self.out.flush()?;
        Ok(self.out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn dec(s: &str) -> Decimal {
        Decimal::from_str(s).unwrap()
    }

    fn write(options: CsvOptions, headers: &[&str], rows: &[Vec<Cell>]) -> String {
        let mut w = CsvWriter::new(Vec::new(), options, headers).unwrap();
        for r in rows {
            w.row(r).unwrap();
        }
        String::from_utf8(w.finish().unwrap()).unwrap()
    }

    #[test]
    fn d89_amount_format_decimal_comma_two_places_no_thousands() {
        assert_eq!(format_amount(dec("1234.5")), "1234,50");
        assert_eq!(format_amount(dec("0")), "0,00");
        assert_eq!(format_amount(dec("-12.345")), "-12,35");
        assert_eq!(format_amount(dec("-0.001")), "0,00");
        assert_eq!(format_amount(dec("1000000")), "1000000,00");
    }

    #[test]
    fn d89_number_format_normalizes() {
        assert_eq!(format_number(dec("21.00")), "21");
        assert_eq!(format_number(dec("5.50")), "5,5");
        assert_eq!(format_number(dec("0")), "0");
    }

    #[test]
    fn d89_escape_quotes_only_when_needed() {
        assert_eq!(escape_text("Cliente Uno"), "Cliente Uno");
        assert_eq!(escape_text("A; B"), "\"A; B\"");
        assert_eq!(escape_text("dice \"hola\""), "\"dice \"\"hola\"\"\"");
        assert_eq!(escape_text("a\nb"), "\"a\nb\"");
        assert_eq!(escape_text(" x"), "\" x\"");
        assert_eq!(escape_text("Ñandú, S.L."), "Ñandú, S.L.");
    }

    #[test]
    fn d89_formula_injection_is_neutralized_in_text_only() {
        assert_eq!(escape_text("=SUMA(A1)"), "'=SUMA(A1)");
        assert_eq!(escape_text("+34 600"), "'+34 600");
        assert_eq!(escape_text("-x"), "'-x");
        assert_eq!(escape_text("@cmd"), "'@cmd");
        // Con separador, además se entrecomilla.
        assert_eq!(escape_text("=a;b"), "\"'=a;b\"");
        // Los importes negativos no se tocan.
        let s = write(
            CsvOptions { bom: false },
            &["x"],
            &[vec![Cell::Amount(dec("-5"))]],
        );
        assert_eq!(s, "x\r\n-5,00\r\n");
    }

    #[test]
    fn d89_golden_output_with_bom() {
        let s = write(
            CsvOptions::default(),
            &["Fecha", "Cliente", "Base", "Tipo", "Nota"],
            &[
                vec![
                    Cell::Date(IsoDate {
                        year: 2026,
                        month: 3,
                        day: 5,
                    }),
                    Cell::text("Cliente; \"Uno\""),
                    Cell::Amount(dec("100")),
                    Cell::Number(dec("21.00")),
                    Cell::Empty,
                ],
                vec![
                    Cell::Date(IsoDate {
                        year: 2026,
                        month: 12,
                        day: 31,
                    }),
                    Cell::text("Ñu"),
                    Cell::Amount(dec("-0.5")),
                    Cell::Number(dec("4")),
                    Cell::opt_text(Some("")),
                ],
            ],
        );
        assert_eq!(
            s,
            "\u{feff}Fecha;Cliente;Base;Tipo;Nota\r\n\
             2026-03-05;\"Cliente; \"\"Uno\"\"\";100,00;21;\r\n\
             2026-12-31;Ñu;-0,50;4;\r\n"
        );
        assert_eq!(&s.as_bytes()[..3], b"\xEF\xBB\xBF");
    }

    #[test]
    fn d89_bom_can_be_disabled() {
        let s = write(CsvOptions { bom: false }, &["a", "b"], &[]);
        assert_eq!(s, "a;b\r\n");
    }

    #[test]
    fn d89_wrong_column_count_rejected_and_nothing_written_for_row() {
        let mut w = CsvWriter::new(Vec::new(), CsvOptions { bom: false }, &["a", "b"]).unwrap();
        let err = w.row(&[Cell::Empty]).unwrap_err();
        assert!(matches!(
            err,
            CsvError::ColumnCount {
                expected: 2,
                found: 1
            }
        ));
        assert_eq!(w.finish().unwrap(), b"a;b\r\n");
    }

    #[test]
    fn d89_write_failure_is_reported() {
        struct Broken;
        impl Write for Broken {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::Error::other("disco lleno"))
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        assert!(matches!(
            CsvWriter::new(Broken, CsvOptions::default(), &["a"]),
            Err(CsvError::Io(_))
        ));
    }

    #[test]
    fn d89_output_is_deterministic() {
        let rows = vec![vec![Cell::text("x"), Cell::Amount(dec("1.5"))]];
        let a = write(CsvOptions::default(), &["a", "b"], &rows);
        let b = write(CsvOptions::default(), &["a", "b"], &rows);
        assert_eq!(a, b);
    }
}
