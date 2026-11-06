//! Cálculo decimal de documentos de venta (D75, D04, R15).
//!
//! Regla de D75, aplicada tal cual:
//! * importe de línea = cantidad × precio × (1 − descuento), redondeado a 2 decimales con la
//!   mitad hacia arriba;
//! * base por tipo = suma de los importes de línea de ese tipo; cuota = base × tipo, redondeada
//!   igual **por tipo** (no por línea); total = bases + cuotas;
//! * tipos 21, 10, 4 y 0 %; el 0 % exige causa de exención o no sujeción;
//! * retención de IRPF opcional por documento: reduce el importe a pagar, no el total.
//!
//! Interpretaciones conservadoras (la spec no las fija):
//! * «Mitad hacia arriba» se aplica como mitad alejándose de cero (`MidpointAwayFromZero`):
//!   coincide con «hacia arriba» en positivos y hace simétrica una rectificativa por
//!   diferencias (importes negativos, D81).
//! * El descuento es una fracción en `[0, 1]` (la fórmula usa `1 − descuento`); la interfaz
//!   convierte desde porcentaje.
//! * La retención se calcula sobre la suma de bases (todas las líneas, exentas incluidas),
//!   en porcentaje `[0, 100]`, redondeada igual que la cuota.
//! * El 0 % se desglosa por **calificación de la operación** ([`TaxQualification`], los valores
//!   que la AEAT define para Verifactu: `S2`, `N1`, `N2`, `E1`–`E6`); dos calificaciones
//!   distintas son dos filas del desglose y nunca se mezclan exenta, no sujeta e inversión del
//!   sujeto pasivo en la misma fila. La descripción libre es un texto aparte que no agrupa.
//!   Un tipo gravado (4, 10, 21 %) es `S1` implícita y no lleva calificación.
//! * Cantidad > 0 y precio ≥ 0 en documentos normales; en una rectificativa por diferencias
//!   (`LineMode::Difference`) la cantidad puede ser negativa (distinta de 0) y el precio
//!   también, porque solo se informa la variación.

use std::collections::{BTreeMap, BTreeSet};

use rust_decimal::{Decimal, RoundingStrategy};
use serde::{Deserialize, Serialize};

/// Moneda única de esta etapa (D72, D91).
pub const CURRENCY: &str = "EUR";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum TaxRate {
    Zero,
    Four,
    Ten,
    TwentyOne,
}

impl TaxRate {
    /// Tipo en porcentaje (21, 10, 4 o 0).
    pub fn percent(self) -> Decimal {
        Decimal::from(match self {
            TaxRate::Zero => 0,
            TaxRate::Four => 4,
            TaxRate::Ten => 10,
            TaxRate::TwentyOne => 21,
        })
    }

    /// Solo se admiten 0, 4, 10 y 21 (`invalid_input`: tipo de IVA no admitido).
    pub fn from_percent(percent: Decimal) -> Result<Self, CalcError> {
        [Self::Zero, Self::Four, Self::Ten, Self::TwentyOne]
            .into_iter()
            .find(|r| r.percent() == percent)
            .ok_or(CalcError::UnsupportedTaxRate { percent })
    }
}

/// Calificación de la operación según los valores de la AEAT para el registro de facturación
/// (`CalificacionOperacion` y `OperacionExenta`, D99): sujeta no exenta (`S1`, `S2`), no sujeta
/// (`N1`, `N2`) y exenta por causa (`E1`–`E6`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum TaxQualification {
    /// Sujeta y no exenta, sin inversión del sujeto pasivo. Es la de los tipos gravados y no
    /// se admite como calificación de una línea al 0 %.
    S1,
    /// Sujeta y no exenta con inversión del sujeto pasivo (mención obligatoria, D80).
    S2,
    /// No sujeta por los artículos 7 y 14 u otros.
    N1,
    /// No sujeta por reglas de localización.
    N2,
    /// Exenta por el artículo 20.
    E1,
    /// Exenta por el artículo 21.
    E2,
    /// Exenta por el artículo 22.
    E3,
    /// Exenta por los artículos 23 y 24.
    E4,
    /// Exenta por el artículo 25.
    E5,
    /// Exenta por otros motivos.
    E6,
}

impl TaxQualification {
    pub const ALL: [TaxQualification; 10] = [
        Self::S1,
        Self::S2,
        Self::N1,
        Self::N2,
        Self::E1,
        Self::E2,
        Self::E3,
        Self::E4,
        Self::E5,
        Self::E6,
    ];

    /// Código de la AEAT.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::S1 => "S1",
            Self::S2 => "S2",
            Self::N1 => "N1",
            Self::N2 => "N2",
            Self::E1 => "E1",
            Self::E2 => "E2",
            Self::E3 => "E3",
            Self::E4 => "E4",
            Self::E5 => "E5",
            Self::E6 => "E6",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|q| q.as_str() == text)
    }

    pub fn is_exempt(self) -> bool {
        matches!(
            self,
            Self::E1 | Self::E2 | Self::E3 | Self::E4 | Self::E5 | Self::E6
        )
    }

    pub fn is_not_subject(self) -> bool {
        matches!(self, Self::N1 | Self::N2)
    }

    pub fn is_reverse_charge(self) -> bool {
        self == Self::S2
    }
}

/// Exención, no sujeción o inversión del sujeto pasivo de una línea al 0 %: la calificación
/// codificada y, opcionalmente, un texto descriptivo para la factura (D99).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Exemption {
    pub qualification: TaxQualification,
    pub description: Option<String>,
}

impl Exemption {
    pub fn new(qualification: TaxQualification) -> Self {
        Self {
            qualification,
            description: None,
        }
    }

    /// Descripción sin espacios sobrantes; `None` si está en blanco.
    pub fn description_text(&self) -> Option<&str> {
        self.description
            .as_deref()
            .map(str::trim)
            .filter(|d| !d.is_empty())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LineInput {
    pub description: String,
    pub quantity: Decimal,
    pub unit_price: Decimal,
    /// Fracción entre 0 y 1 (0 = sin descuento).
    pub discount: Decimal,
    pub tax_rate: TaxRate,
    /// Obligatoria con 0 % (y distinta de `S1`); prohibida con otro tipo.
    pub exemption: Option<Exemption>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineMode {
    /// Cantidad positiva y precio no negativo.
    Standard,
    /// Rectificativa por diferencias (D81): cantidad o precio pueden ser negativos.
    Difference,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaxBreakdown {
    pub tax_rate: TaxRate,
    /// `None` en los tipos gravados (`S1` implícita).
    pub qualification: Option<TaxQualification>,
    /// Descripciones distintas de las líneas del grupo, ordenadas; no agrupan.
    pub descriptions: Vec<String>,
    pub base: Decimal,
    pub tax: Decimal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Totals {
    pub currency: String,
    /// Importe de cada línea, en el orden de entrada.
    pub line_amounts: Vec<Decimal>,
    /// Ordenado por tipo y causa.
    pub breakdown: Vec<TaxBreakdown>,
    pub base_total: Decimal,
    pub tax_total: Decimal,
    /// Bases + cuotas.
    pub total: Decimal,
    pub withholding: Decimal,
    /// Total − retención.
    pub amount_due: Decimal,
}

/// Errores de entrada. `line` es 1-based.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CalcError {
    #[error("el documento no tiene líneas")]
    NoLines,
    #[error("línea {line}: falta la descripción")]
    EmptyDescription { line: usize },
    #[error("línea {line}: la cantidad debe ser positiva")]
    NonPositiveQuantity { line: usize },
    #[error("línea {line}: la cantidad no puede ser cero")]
    ZeroQuantity { line: usize },
    #[error("línea {line}: el precio no puede ser negativo")]
    NegativePrice { line: usize },
    #[error("línea {line}: el descuento debe estar entre 0 y 1")]
    DiscountOutOfRange { line: usize },
    #[error("línea {line}: el tipo 0 % exige una calificación (S2, N1, N2 o E1 a E6)")]
    ExemptionRequired { line: usize },
    #[error("línea {line}: la calificación de exención solo procede con el tipo 0 %")]
    ExemptionNotAllowed { line: usize },
    #[error("tipo de IVA no admitido: {percent}")]
    UnsupportedTaxRate { percent: Decimal },
    #[error("la retención debe estar entre 0 y 100")]
    WithholdingOutOfRange,
    #[error("importe fuera del rango admitido")]
    Overflow,
}

/// Redondeo de D75: 2 decimales, mitad hacia arriba (ver nota del módulo), siempre con
/// escala 2 para que `5` se muestre `5.00`.
pub fn round_money(value: Decimal) -> Decimal {
    let mut rounded = value.round_dp_with_strategy(2, RoundingStrategy::MidpointAwayFromZero);
    if rounded.is_zero() {
        // Evita un «-0.00» al redondear una variación negativa mínima.
        rounded = Decimal::ZERO;
    }
    rounded.rescale(2);
    rounded
}

fn line_amount(index: usize, line: &LineInput, mode: LineMode) -> Result<Decimal, CalcError> {
    let n = index + 1;
    if line.description.trim().is_empty() {
        return Err(CalcError::EmptyDescription { line: n });
    }
    match mode {
        LineMode::Standard => {
            if line.quantity <= Decimal::ZERO {
                return Err(CalcError::NonPositiveQuantity { line: n });
            }
            if line.unit_price < Decimal::ZERO {
                return Err(CalcError::NegativePrice { line: n });
            }
        }
        LineMode::Difference => {
            if line.quantity.is_zero() {
                return Err(CalcError::ZeroQuantity { line: n });
            }
        }
    }
    if line.discount < Decimal::ZERO || line.discount > Decimal::ONE {
        return Err(CalcError::DiscountOutOfRange { line: n });
    }
    match (&line.exemption, line.tax_rate) {
        (None, TaxRate::Zero) => return Err(CalcError::ExemptionRequired { line: n }),
        (Some(e), TaxRate::Zero) if e.qualification == TaxQualification::S1 => {
            return Err(CalcError::ExemptionRequired { line: n })
        }
        (Some(_), r) if r != TaxRate::Zero => {
            return Err(CalcError::ExemptionNotAllowed { line: n })
        }
        _ => {}
    }
    let factor = Decimal::ONE - line.discount;
    let raw = line
        .quantity
        .checked_mul(line.unit_price)
        .and_then(|v| v.checked_mul(factor))
        .ok_or(CalcError::Overflow)?;
    Ok(round_money(raw))
}

/// Calcula líneas, desglose y totales (D75). `withholding_percent` es el porcentaje de IRPF
/// indicado por el usuario, si lo hay.
pub fn calculate(
    lines: &[LineInput],
    withholding_percent: Option<Decimal>,
    mode: LineMode,
) -> Result<Totals, CalcError> {
    if lines.is_empty() {
        return Err(CalcError::NoLines);
    }
    if let Some(p) = withholding_percent {
        if p < Decimal::ZERO || p > Decimal::ONE_HUNDRED {
            return Err(CalcError::WithholdingOutOfRange);
        }
    }
    let mut line_amounts = Vec::with_capacity(lines.len());
    type Group = (Decimal, BTreeSet<String>);
    let mut groups: BTreeMap<(TaxRate, Option<TaxQualification>), Group> = BTreeMap::new();
    for (i, line) in lines.iter().enumerate() {
        let amount = line_amount(i, line, mode)?;
        line_amounts.push(amount);
        let key = (
            line.tax_rate,
            line.exemption.as_ref().map(|e| e.qualification),
        );
        let group = groups.entry(key).or_default();
        group.0 = group.0.checked_add(amount).ok_or(CalcError::Overflow)?;
        if let Some(text) = line
            .exemption
            .as_ref()
            .and_then(Exemption::description_text)
        {
            group.1.insert(text.to_owned());
        }
    }
    let mut breakdown = Vec::with_capacity(groups.len());
    let mut base_total = Decimal::ZERO;
    let mut tax_total = Decimal::ZERO;
    for ((tax_rate, qualification), (base, descriptions)) in groups {
        let tax = round_money(
            base.checked_mul(tax_rate.percent())
                .ok_or(CalcError::Overflow)?
                / Decimal::ONE_HUNDRED,
        );
        base_total = base_total.checked_add(base).ok_or(CalcError::Overflow)?;
        tax_total = tax_total.checked_add(tax).ok_or(CalcError::Overflow)?;
        breakdown.push(TaxBreakdown {
            tax_rate,
            qualification,
            descriptions: descriptions.into_iter().collect(),
            base: round_money(base),
            tax,
        });
    }
    let total = round_money(
        base_total
            .checked_add(tax_total)
            .ok_or(CalcError::Overflow)?,
    );
    let withholding = match withholding_percent {
        Some(p) => round_money(
            base_total.checked_mul(p).ok_or(CalcError::Overflow)? / Decimal::ONE_HUNDRED,
        ),
        None => round_money(Decimal::ZERO),
    };
    let amount_due = round_money(total.checked_sub(withholding).ok_or(CalcError::Overflow)?);
    Ok(Totals {
        currency: CURRENCY.to_owned(),
        line_amounts,
        breakdown,
        base_total: round_money(base_total),
        tax_total: round_money(tax_total),
        total,
        withholding,
        amount_due,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn d(s: &str) -> Decimal {
        Decimal::from_str(s).unwrap()
    }

    fn line(qty: &str, price: &str, discount: &str, rate: TaxRate) -> LineInput {
        LineInput {
            description: "Servicio sintético".into(),
            quantity: d(qty),
            unit_price: d(price),
            discount: d(discount),
            tax_rate: rate,
            exemption: None,
        }
    }

    fn exempt(qty: &str, price: &str, cause: &str) -> LineInput {
        LineInput {
            exemption: Some(Exemption {
                qualification: TaxQualification::E1,
                description: Some(cause.into()),
            }),
            ..line(qty, price, "0", TaxRate::Zero)
        }
    }

    fn calc(lines: &[LineInput]) -> Totals {
        calculate(lines, None, LineMode::Standard).unwrap()
    }

    #[test]
    fn d75_single_line_at_each_rate() {
        for (rate, tax) in [
            (TaxRate::TwentyOne, "21.00"),
            (TaxRate::Ten, "10.00"),
            (TaxRate::Four, "4.00"),
        ] {
            let t = calc(&[line("1", "100", "0", rate)]);
            assert_eq!(t.base_total.to_string(), "100.00");
            assert_eq!(t.tax_total.to_string(), tax);
        }
        let t = calc(&[exempt("1", "100", "Exenta art. 20 (sintética)")]);
        assert_eq!(t.tax_total.to_string(), "0.00");
        assert_eq!(t.total.to_string(), "100.00");
    }

    #[test]
    fn d75_line_amount_rounds_half_up() {
        // 3 x 0.335 = 1.005 -> 1.01 (mitad hacia arriba); no 1.00 (par).
        let t = calc(&[line("3", "0.335", "0", TaxRate::Four)]);
        assert_eq!(t.line_amounts[0].to_string(), "1.01");
    }

    #[test]
    fn d75_line_rounding_below_and_above_midpoint() {
        let t = calc(&[
            line("1", "0.004", "0", TaxRate::TwentyOne),
            line("1", "0.006", "0", TaxRate::TwentyOne),
        ]);
        assert_eq!(t.line_amounts[0].to_string(), "0.00");
        assert_eq!(t.line_amounts[1].to_string(), "0.01");
    }

    #[test]
    fn d75_discount_applies_before_rounding() {
        // 3 x 9.99 x (1 - 0.15) = 25.4745 -> 25.47
        let t = calc(&[line("3", "9.99", "0.15", TaxRate::TwentyOne)]);
        assert_eq!(t.line_amounts[0].to_string(), "25.47");
        // 100% de descuento -> 0.00 y sigue siendo válida.
        let t = calc(&[line("2", "10", "1", TaxRate::TwentyOne)]);
        assert_eq!(t.total.to_string(), "0.00");
    }

    #[test]
    fn d75_tax_is_rounded_per_rate_not_per_line() {
        // Tres líneas de 0.10 al 21 %: por línea serían 0.02 x 3 = 0.06; por tipo, 0.30 x 21 % = 0.063 -> 0.06.
        // Con 0.50 x 3 = 1.50: por línea 0.105 -> 0.11 x 3 = 0.33; por tipo 0.315 -> 0.32.
        let lines = vec![
            line("1", "0.50", "0", TaxRate::TwentyOne),
            line("1", "0.50", "0", TaxRate::TwentyOne),
            line("1", "0.50", "0", TaxRate::TwentyOne),
        ];
        let t = calc(&lines);
        assert_eq!(t.breakdown.len(), 1);
        assert_eq!(t.breakdown[0].base.to_string(), "1.50");
        assert_eq!(t.breakdown[0].tax.to_string(), "0.32");
        assert_eq!(t.total.to_string(), "1.82");
    }

    #[test]
    fn d75_tax_midpoint_rounds_up() {
        // 0.50 x 21 % = 0.105 -> 0.11
        let t = calc(&[line("1", "0.50", "0", TaxRate::TwentyOne)]);
        assert_eq!(t.tax_total.to_string(), "0.11");
        // 0.25 x 10 % = 0.025 -> 0.03
        let t = calc(&[line("1", "0.25", "0", TaxRate::Ten)]);
        assert_eq!(t.tax_total.to_string(), "0.03");
        // 0.10 x 4 % = 0.004 -> 0.00
        let t = calc(&[line("1", "0.10", "0", TaxRate::Four)]);
        assert_eq!(t.tax_total.to_string(), "0.00");
    }

    #[test]
    fn d75_mixed_rates_breakdown_and_totals() {
        let t = calc(&[
            line("2", "50", "0", TaxRate::TwentyOne),
            line("1", "80.00", "0.10", TaxRate::Ten),
            line("4", "2.5", "0", TaxRate::Four),
            line("1", "30", "0", TaxRate::TwentyOne),
        ]);
        let rows: Vec<(TaxRate, String, String)> = t
            .breakdown
            .iter()
            .map(|b| (b.tax_rate, b.base.to_string(), b.tax.to_string()))
            .collect();
        assert_eq!(
            rows,
            vec![
                (TaxRate::Four, "10.00".into(), "0.40".into()),
                (TaxRate::Ten, "72.00".into(), "7.20".into()),
                (TaxRate::TwentyOne, "130.00".into(), "27.30".into()),
            ]
        );
        assert_eq!(t.base_total.to_string(), "212.00");
        assert_eq!(t.tax_total.to_string(), "34.90");
        assert_eq!(t.total.to_string(), "246.90");
        assert_eq!(t.withholding.to_string(), "0.00");
        assert_eq!(t.amount_due, t.total);
    }

    #[test]
    fn d75_zero_rate_requires_cause() {
        let mut l = line("1", "10", "0", TaxRate::Zero);
        assert_eq!(
            calculate(&[l.clone()], None, LineMode::Standard),
            Err(CalcError::ExemptionRequired { line: 1 })
        );
        // S1 no es una calificación válida para el 0 %.
        l.exemption = Some(Exemption::new(TaxQualification::S1));
        assert_eq!(
            calculate(&[l.clone()], None, LineMode::Standard),
            Err(CalcError::ExemptionRequired { line: 1 })
        );
        // Con calificación basta: el texto descriptivo es opcional.
        l.exemption = Some(Exemption::new(TaxQualification::N1));
        assert!(calculate(&[l], None, LineMode::Standard).is_ok());
    }

    #[test]
    fn d75_cause_with_taxed_rate_is_rejected() {
        let mut l = line("1", "10", "0", TaxRate::TwentyOne);
        l.exemption = Some(Exemption::new(TaxQualification::E1));
        assert_eq!(
            calculate(&[l], None, LineMode::Standard),
            Err(CalcError::ExemptionNotAllowed { line: 1 })
        );
    }

    #[test]
    fn d99_zero_rate_groups_by_qualification_not_by_text() {
        let q = |qualification, price: &str| LineInput {
            exemption: Some(Exemption::new(qualification)),
            ..line("1", price, "0", TaxRate::Zero)
        };
        let t = calc(&[
            exempt("1", "10", "Texto A"),
            q(TaxQualification::N1, "5"),
            exempt("1", "2", "Texto B"),
            q(TaxQualification::S2, "7"),
            q(TaxQualification::E4, "1"),
        ]);
        // Exenta, no sujeta e ISP no se mezclan aunque todas sean 0 %.
        let rows: Vec<(Option<TaxQualification>, String)> = t
            .breakdown
            .iter()
            .map(|b| (b.qualification, b.base.to_string()))
            .collect();
        assert_eq!(
            rows,
            vec![
                (Some(TaxQualification::S2), "7.00".into()),
                (Some(TaxQualification::N1), "5.00".into()),
                (Some(TaxQualification::E1), "12.00".into()),
                (Some(TaxQualification::E4), "1.00".into()),
            ]
        );
        // Las descripciones de un mismo grupo se conservan, sin repetir, aparte.
        assert_eq!(t.breakdown[2].descriptions, vec!["Texto A", "Texto B"]);
        assert!(t.breakdown[0].descriptions.is_empty());
        assert_eq!(t.tax_total.to_string(), "0.00");
    }

    #[test]
    fn d99_qualification_codes_round_trip() {
        for q in TaxQualification::ALL {
            assert_eq!(TaxQualification::parse(q.as_str()), Some(q));
        }
        assert_eq!(TaxQualification::parse("E7"), None);
        assert_eq!(TaxQualification::parse("e1"), None);
        assert!(TaxQualification::E6.is_exempt() && !TaxQualification::N2.is_exempt());
        assert!(TaxQualification::N2.is_not_subject());
        assert!(TaxQualification::S2.is_reverse_charge());
        assert!(!TaxQualification::S1.is_reverse_charge());
    }

    #[test]
    fn d75_withholding_reduces_amount_due_not_total() {
        let t = calculate(
            &[line("1", "1000", "0", TaxRate::TwentyOne)],
            Some(d("15")),
            LineMode::Standard,
        )
        .unwrap();
        assert_eq!(t.total.to_string(), "1210.00");
        assert_eq!(t.withholding.to_string(), "150.00");
        assert_eq!(t.amount_due.to_string(), "1060.00");
    }

    #[test]
    fn d75_withholding_is_on_bases_and_rounds_half_up() {
        // base 0.50 x 7 % = 0.035 -> 0.04
        let t = calculate(
            &[line("1", "0.50", "0", TaxRate::TwentyOne)],
            Some(d("7")),
            LineMode::Standard,
        )
        .unwrap();
        assert_eq!(t.withholding.to_string(), "0.04");
        assert_eq!(t.total.to_string(), "0.61");
        assert_eq!(t.amount_due.to_string(), "0.57");
    }

    #[test]
    fn d75_withholding_range() {
        let l = [line("1", "10", "0", TaxRate::Four)];
        assert_eq!(
            calculate(&l, Some(d("-1")), LineMode::Standard),
            Err(CalcError::WithholdingOutOfRange)
        );
        assert_eq!(
            calculate(&l, Some(d("100.01")), LineMode::Standard),
            Err(CalcError::WithholdingOutOfRange)
        );
        assert!(calculate(&l, Some(d("0")), LineMode::Standard).is_ok());
        assert!(calculate(&l, Some(d("100")), LineMode::Standard).is_ok());
    }

    #[test]
    fn d75_input_validation() {
        let ok = line("1", "10", "0", TaxRate::Four);
        let std = |l: LineInput| calculate(&[l], None, LineMode::Standard);
        assert_eq!(
            calculate(&[], None, LineMode::Standard),
            Err(CalcError::NoLines)
        );
        assert_eq!(
            std(LineInput {
                description: "  ".into(),
                ..ok.clone()
            }),
            Err(CalcError::EmptyDescription { line: 1 })
        );
        assert_eq!(
            std(LineInput {
                quantity: d("0"),
                ..ok.clone()
            }),
            Err(CalcError::NonPositiveQuantity { line: 1 })
        );
        assert_eq!(
            std(LineInput {
                quantity: d("-1"),
                ..ok.clone()
            }),
            Err(CalcError::NonPositiveQuantity { line: 1 })
        );
        assert_eq!(
            std(LineInput {
                unit_price: d("-0.01"),
                ..ok.clone()
            }),
            Err(CalcError::NegativePrice { line: 1 })
        );
        assert_eq!(
            std(LineInput {
                discount: d("1.01"),
                ..ok.clone()
            }),
            Err(CalcError::DiscountOutOfRange { line: 1 })
        );
        assert_eq!(
            std(LineInput {
                discount: d("-0.1"),
                ..ok.clone()
            }),
            Err(CalcError::DiscountOutOfRange { line: 1 })
        );
        // El número de línea apunta a la que falla.
        assert_eq!(
            calculate(
                &[
                    ok.clone(),
                    LineInput {
                        description: String::new(),
                        ..ok
                    }
                ],
                None,
                LineMode::Standard
            ),
            Err(CalcError::EmptyDescription { line: 2 })
        );
    }

    #[test]
    fn d75_unsupported_rate_from_percent() {
        assert_eq!(TaxRate::from_percent(d("21")), Ok(TaxRate::TwentyOne));
        assert_eq!(TaxRate::from_percent(d("10.0")), Ok(TaxRate::Ten));
        assert_eq!(TaxRate::from_percent(d("4")), Ok(TaxRate::Four));
        assert_eq!(TaxRate::from_percent(d("0")), Ok(TaxRate::Zero));
        for bad in ["5", "7", "21.5", "-21", "1.4"] {
            assert!(matches!(
                TaxRate::from_percent(d(bad)),
                Err(CalcError::UnsupportedTaxRate { .. })
            ));
        }
    }

    #[test]
    fn d75_difference_mode_allows_negative_and_mirrors_original() {
        let original = [line("3", "0.335", "0", TaxRate::TwentyOne)];
        let negated = [line("-3", "0.335", "0", TaxRate::TwentyOne)];
        let a = calc(&original);
        let b = calculate(&negated, None, LineMode::Difference).unwrap();
        assert_eq!(a.line_amounts[0].to_string(), "1.01");
        assert_eq!(b.line_amounts[0].to_string(), "-1.01");
        assert_eq!(b.total, -a.total);
        // Importe a la baja con precio negativo.
        let c = calculate(
            &[line("1", "-10", "0", TaxRate::TwentyOne)],
            None,
            LineMode::Difference,
        )
        .unwrap();
        assert_eq!(c.total.to_string(), "-12.10");
        // Cantidad cero sigue prohibida; en modo estándar los negativos también.
        assert_eq!(
            calculate(
                &[line("0", "1", "0", TaxRate::Four)],
                None,
                LineMode::Difference
            ),
            Err(CalcError::ZeroQuantity { line: 1 })
        );
        assert!(calculate(&negated, None, LineMode::Standard).is_err());
    }

    #[test]
    fn d75_overflow_is_an_error_not_a_panic() {
        let big = LineInput {
            quantity: Decimal::MAX,
            unit_price: d("2"),
            ..line("1", "1", "0", TaxRate::Four)
        };
        assert_eq!(
            calculate(&[big], None, LineMode::Standard),
            Err(CalcError::Overflow)
        );
    }

    #[test]
    fn d04_amounts_are_exact_decimals_with_two_places() {
        // 0.1 + 0.2 exacto (no 0.30000000000000004).
        let t = calc(&[
            line("1", "0.1", "0", TaxRate::Four),
            line("1", "0.2", "0", TaxRate::Four),
        ]);
        assert_eq!(t.base_total.to_string(), "0.30");
        assert_eq!(t.currency, "EUR");
        // Cantidades fraccionarias.
        let t = calc(&[line("2.5", "19.99", "0", TaxRate::TwentyOne)]);
        assert_eq!(t.line_amounts[0].to_string(), "49.98");
    }

    #[test]
    fn d75_round_money_helper() {
        assert_eq!(round_money(d("2.675")).to_string(), "2.68");
        assert_eq!(round_money(d("-2.675")).to_string(), "-2.68");
        assert_eq!(round_money(d("5")).to_string(), "5.00");
        assert_eq!(round_money(d("0.001")).to_string(), "0.00");
    }
}
