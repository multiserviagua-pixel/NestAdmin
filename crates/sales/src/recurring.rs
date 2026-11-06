//! Facturas recurrentes: plantilla y calendario a borradores (R17, D87).
//!
//! D87: plantilla con cliente, líneas, periodicidad (mensual, trimestral o anual), primer día
//! y fin opcional. Al abrir la aplicación y periódicamente se crea el borrador de cada periodo
//! vencido, identificado por plantilla y periodo para no duplicarlo tras un corte. Emitir el
//! borrador requiere revisión, salvo una regla expresa activa de tipo «emitir recurrente» con
//! la plantilla como condición.
//!
//! Este módulo es puro: sin programador ni reloj. Quien llama pasa «hoy» y los periodos que ya
//! tienen borrador; la persistencia hará única la clave (plantilla, periodo) en la misma
//! transacción que crea el borrador.
//!
//! Interpretaciones conservadoras:
//! * El **periodo `n`** (desde 0) empieza en `primer día + n × paso` meses, siempre sumado
//!   desde el primer día (sin deriva: 31 ene → 28 feb → 31 mar). Un periodo está **vencido**
//!   cuando su fecha de inicio es anterior o igual a hoy. Cambiar el primer día de una plantilla
//!   desplaza los índices; la persistencia debe crear una plantilla nueva en ese caso.
//! * `fin` es inclusivo: no se genera un periodo cuyo inicio sea posterior al fin.
//! * Tras un corte largo se generan **todos** los periodos vencidos que falten, uno por
//!   periodo. El inicio del periodo es la **fecha de operación** (`period_start`); la **fecha
//!   de expedición** del borrador es la fecha real de la generación (`hoy`), nunca una fecha
//!   pasada. Un tope de [`MAX_PERIODS`] evita bucles sobre plantillas con fechas absurdas.
//! * Con la regla expresa activa solo se puede emitir automáticamente el **periodo actual**
//!   (el que contiene a hoy). Los periodos atrasados se generan siempre como borradores que
//!   requieren revisión ([`IssueMode::RequiresReview`]), porque emitirlos sin ver sería
//!   expedir facturas de fechas pasadas.
//! * Las líneas de la plantilla se validan con `calc` (D75) al crearla, y cada borrador lleva
//!   una copia de ellas.
//! * Los borradores no llevan número (D73); su emisión es otra operación y exige revisión salvo
//!   la regla expresa de D87 ([`issue_mode`]).

use std::collections::BTreeSet;
use std::fmt;

use nestadmin_documents::IsoDate;

use crate::calc::{calculate, CalcError, LineInput, LineMode};
use crate::dates;

/// Máximo de periodos que se recorren o generan en una llamada.
pub const MAX_PERIODS: u32 = 1_200;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Frequency {
    Monthly,
    Quarterly,
    Annual,
}

impl Frequency {
    pub fn months(self) -> u32 {
        match self {
            Self::Monthly => 1,
            Self::Quarterly => 3,
            Self::Annual => 12,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Monthly => "monthly",
            Self::Quarterly => "quarterly",
            Self::Annual => "annual",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        [Self::Monthly, Self::Quarterly, Self::Annual]
            .into_iter()
            .find(|f| f.as_str() == text)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RecurringTemplate {
    pub id: u64,
    pub customer_id: u64,
    pub lines: Vec<LineInput>,
    pub frequency: Frequency,
    pub first_day: IsoDate,
    /// Último día en que puede empezar un periodo (inclusivo).
    pub end: Option<IsoDate>,
}

/// Identidad de un borrador recurrente: plantilla y periodo (D87).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PeriodKey {
    pub template_id: u64,
    pub period: u32,
}

impl fmt::Display for PeriodKey {
    /// Clave de idempotencia estable, p. ej. `recurring:7:3`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "recurring:{}:{}", self.template_id, self.period)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct DraftPlan {
    pub key: PeriodKey,
    pub customer_id: u64,
    /// Fecha de operación: inicio del periodo.
    pub period_start: IsoDate,
    /// Fecha de expedición del borrador: la fecha real de generación (`hoy`).
    pub issue_date: IsoDate,
    /// Cómo se puede emitir este borrador concreto (D87).
    pub mode: IssueMode,
    pub lines: Vec<LineInput>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RecurringError {
    #[error("el primer día no es una fecha válida")]
    InvalidFirstDay,
    #[error("la fecha de fin no es válida o es anterior al primer día")]
    InvalidEnd,
    #[error("la fecha de hoy no es válida")]
    InvalidToday,
    #[error("líneas de la plantilla: {0}")]
    Lines(#[from] CalcError),
    #[error("demasiados periodos pendientes (máximo {MAX_PERIODS})")]
    TooManyPeriods,
}

/// Cómo se emite un borrador recurrente (D87).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IssueMode {
    /// Por defecto: el borrador espera revisión del usuario.
    RequiresReview,
    /// Solo con una regla expresa activa «emitir recurrente» para esa plantilla.
    AutoIssueByRule,
}

pub fn issue_mode(express_rule_active_for_template: bool) -> IssueMode {
    if express_rule_active_for_template {
        IssueMode::AutoIssueByRule
    } else {
        IssueMode::RequiresReview
    }
}

impl RecurringTemplate {
    pub fn validate(&self) -> Result<(), RecurringError> {
        if !dates::is_valid(self.first_day) {
            return Err(RecurringError::InvalidFirstDay);
        }
        if let Some(end) = self.end {
            if !dates::is_valid(end) || end < self.first_day {
                return Err(RecurringError::InvalidEnd);
            }
        }
        calculate(&self.lines, None, LineMode::Standard)?;
        Ok(())
    }

    /// `true` si `period` es el periodo actual: empieza hoy o antes y el siguiente empieza
    /// después de hoy (o se sale del calendario). No depende de `end`.
    pub fn is_current(&self, period: u32, today: IsoDate) -> bool {
        let started = self.period_start(period).is_some_and(|s| s <= today);
        let next_later = period
            .checked_add(1)
            .and_then(|n| self.period_start(n))
            .is_none_or(|s| s > today);
        started && next_later
    }

    /// Inicio del periodo `period`; `None` si se sale del calendario soportado.
    pub fn period_start(&self, period: u32) -> Option<IsoDate> {
        let months = period.checked_mul(self.frequency.months())?;
        dates::add_months(self.first_day, months)
    }
}

/// Borradores que faltan en la fecha `today`: cada periodo vencido (inicio ≤ hoy y ≤ fin) cuyo
/// índice no esté en `existing` (D87). Orden cronológico. Llamarla dos veces con los mismos
/// `existing` actualizados no produce duplicados. `express_rule_active` indica la regla expresa
/// «emitir recurrente» de la plantilla: solo el periodo actual sale como `AutoIssueByRule`.
pub fn due_drafts(
    template: &RecurringTemplate,
    today: IsoDate,
    existing: &BTreeSet<u32>,
    express_rule_active: bool,
) -> Result<Vec<DraftPlan>, RecurringError> {
    template.validate()?;
    if !dates::is_valid(today) {
        return Err(RecurringError::InvalidToday);
    }
    let mut plans = Vec::new();
    for period in 0..MAX_PERIODS {
        let Some(start) = template.period_start(period) else {
            break;
        };
        if start > today || template.end.is_some_and(|end| start > end) {
            return Ok(plans);
        }
        if existing.contains(&period) {
            continue;
        }
        plans.push(DraftPlan {
            key: PeriodKey {
                template_id: template.id,
                period,
            },
            customer_id: template.customer_id,
            period_start: start,
            issue_date: today,
            mode: if template.is_current(period, today) {
                issue_mode(express_rule_active)
            } else {
                IssueMode::RequiresReview
            },
            lines: template.lines.clone(),
        });
    }
    // Se agotó el tope sin llegar a hoy ni al fin, salvo que el calendario terminara antes.
    match template.period_start(MAX_PERIODS) {
        Some(start) if start <= today && !template.end.is_some_and(|end| start > end) => {
            Err(RecurringError::TooManyPeriods)
        }
        _ => Ok(plans),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calc::TaxRate;
    use rust_decimal::Decimal;
    use std::str::FromStr;

    fn date(y: u16, m: u8, d: u8) -> IsoDate {
        IsoDate {
            year: y,
            month: m,
            day: d,
        }
    }

    fn template(frequency: Frequency, first: IsoDate, end: Option<IsoDate>) -> RecurringTemplate {
        RecurringTemplate {
            id: 7,
            customer_id: 3,
            lines: vec![LineInput {
                description: "Cuota sintética".into(),
                quantity: Decimal::ONE,
                unit_price: Decimal::from_str("100.00").unwrap(),
                discount: Decimal::ZERO,
                tax_rate: TaxRate::TwentyOne,
                exemption: None,
            }],
            frequency,
            first_day: first,
            end,
        }
    }

    fn dates_of(plans: &[DraftPlan]) -> Vec<IsoDate> {
        plans.iter().map(|p| p.period_start).collect()
    }

    #[test]
    fn d87_monthly_generates_one_draft_per_due_period() {
        let t = template(Frequency::Monthly, date(2026, 1, 15), None);
        let plans = due_drafts(&t, date(2026, 3, 20), &BTreeSet::new(), false).unwrap();
        assert_eq!(
            dates_of(&plans),
            vec![date(2026, 1, 15), date(2026, 2, 15), date(2026, 3, 15)]
        );
        assert_eq!(plans[2].key.period, 2);
        assert_eq!(plans[2].key.to_string(), "recurring:7:2");
        assert_eq!(plans[0].customer_id, 3);
        assert_eq!(plans[0].lines, t.lines);
    }

    #[test]
    fn d87_period_starting_today_is_due_and_future_is_not() {
        let t = template(Frequency::Monthly, date(2026, 1, 15), None);
        let plans = due_drafts(&t, date(2026, 2, 15), &BTreeSet::new(), false).unwrap();
        assert_eq!(plans.len(), 2);
        let plans = due_drafts(&t, date(2026, 2, 14), &BTreeSet::new(), false).unwrap();
        assert_eq!(plans.len(), 1);
        let plans = due_drafts(&t, date(2025, 12, 31), &BTreeSet::new(), false).unwrap();
        assert!(plans.is_empty());
    }

    #[test]
    fn d87_no_duplicates_after_a_cut() {
        let t = template(Frequency::Monthly, date(2026, 1, 1), None);
        let today = date(2026, 4, 10);
        let first = due_drafts(&t, today, &BTreeSet::new(), false).unwrap();
        assert_eq!(first.len(), 4);
        // Corte tras guardar solo los dos primeros: se generan los dos que faltan.
        let existing: BTreeSet<u32> = first.iter().take(2).map(|p| p.key.period).collect();
        let rest = due_drafts(&t, today, &existing, false).unwrap();
        assert_eq!(
            rest.iter().map(|p| p.key.period).collect::<Vec<_>>(),
            [2, 3]
        );
        // Con todos guardados no hay nada que crear.
        let all: BTreeSet<u32> = first.iter().map(|p| p.key.period).collect();
        assert!(due_drafts(&t, today, &all, false).unwrap().is_empty());
    }

    #[test]
    fn d87_existing_gap_in_the_middle_is_filled_only_for_missing() {
        let t = template(Frequency::Monthly, date(2026, 1, 1), None);
        let existing: BTreeSet<u32> = [0, 2].into_iter().collect();
        let plans = due_drafts(&t, date(2026, 4, 1), &existing, false).unwrap();
        assert_eq!(
            plans.iter().map(|p| p.key.period).collect::<Vec<_>>(),
            [1, 3]
        );
    }

    #[test]
    fn d87_quarterly_and_annual_steps() {
        let q = template(Frequency::Quarterly, date(2026, 1, 31), None);
        let plans = due_drafts(&q, date(2026, 12, 31), &BTreeSet::new(), false).unwrap();
        assert_eq!(
            dates_of(&plans),
            vec![
                date(2026, 1, 31),
                date(2026, 4, 30),
                date(2026, 7, 31),
                date(2026, 10, 31)
            ]
        );
        let a = template(Frequency::Annual, date(2024, 2, 29), None);
        let plans = due_drafts(&a, date(2028, 3, 1), &BTreeSet::new(), false).unwrap();
        assert_eq!(
            dates_of(&plans),
            vec![
                date(2024, 2, 29),
                date(2025, 2, 28),
                date(2026, 2, 28),
                date(2027, 2, 28),
                date(2028, 2, 29)
            ]
        );
    }

    #[test]
    fn d87_monthly_end_of_month_has_no_drift() {
        let t = template(Frequency::Monthly, date(2026, 1, 31), None);
        let plans = due_drafts(&t, date(2026, 4, 30), &BTreeSet::new(), false).unwrap();
        assert_eq!(
            dates_of(&plans),
            vec![
                date(2026, 1, 31),
                date(2026, 2, 28),
                date(2026, 3, 31),
                date(2026, 4, 30)
            ]
        );
    }

    #[test]
    fn d87_end_date_is_inclusive_and_stops_generation() {
        let t = template(Frequency::Monthly, date(2026, 1, 1), Some(date(2026, 3, 1)));
        let plans = due_drafts(&t, date(2027, 1, 1), &BTreeSet::new(), false).unwrap();
        assert_eq!(plans.len(), 3);
        let t = template(
            Frequency::Monthly,
            date(2026, 1, 1),
            Some(date(2026, 2, 28)),
        );
        let plans = due_drafts(&t, date(2027, 1, 1), &BTreeSet::new(), false).unwrap();
        assert_eq!(plans.len(), 2);
    }

    #[test]
    fn d87_template_validation() {
        let mut t = template(Frequency::Monthly, date(2026, 2, 30), None);
        assert_eq!(t.validate(), Err(RecurringError::InvalidFirstDay));
        t = template(
            Frequency::Monthly,
            date(2026, 5, 1),
            Some(date(2026, 4, 30)),
        );
        assert_eq!(t.validate(), Err(RecurringError::InvalidEnd));
        t = template(Frequency::Monthly, date(2026, 5, 1), None);
        t.lines.clear();
        assert_eq!(t.validate(), Err(RecurringError::Lines(CalcError::NoLines)));
        t = template(Frequency::Monthly, date(2026, 5, 1), None);
        assert_eq!(
            due_drafts(&t, date(2026, 13, 1), &BTreeSet::new(), false).unwrap_err(),
            RecurringError::InvalidToday
        );
    }

    #[test]
    fn d87_runaway_template_is_capped() {
        let t = template(Frequency::Monthly, date(1900, 1, 1), None);
        assert_eq!(
            due_drafts(&t, date(2026, 1, 1), &BTreeSet::new(), false).unwrap_err(),
            RecurringError::TooManyPeriods
        );
    }

    #[test]
    fn d87_long_cut_with_express_rule_only_current_period_auto_issues() {
        let t = template(Frequency::Monthly, date(2026, 1, 15), None);
        let today = date(2026, 5, 20);
        let plans = due_drafts(&t, today, &BTreeSet::new(), true).unwrap();
        assert_eq!(plans.len(), 5);
        for p in &plans[..4] {
            assert_eq!(p.mode, IssueMode::RequiresReview, "{:?}", p.period_start);
        }
        assert_eq!(plans[4].period_start, date(2026, 5, 15));
        assert_eq!(plans[4].mode, IssueMode::AutoIssueByRule);
        // La fecha de expedición es la real, no la del periodo.
        assert!(plans.iter().all(|p| p.issue_date == today));
        // Sin regla expresa todo requiere revisión.
        let plans = due_drafts(&t, today, &BTreeSet::new(), false).unwrap();
        assert!(plans.iter().all(|p| p.mode == IssueMode::RequiresReview));
    }

    #[test]
    fn d87_ended_template_has_no_auto_issue_for_stale_periods() {
        let t = template(Frequency::Monthly, date(2026, 1, 1), Some(date(2026, 3, 1)));
        let plans = due_drafts(&t, date(2027, 1, 1), &BTreeSet::new(), true).unwrap();
        assert_eq!(plans.len(), 3);
        assert!(plans.iter().all(|p| p.mode == IssueMode::RequiresReview));
    }

    #[test]
    fn d87_issue_requires_review_unless_express_rule() {
        assert_eq!(issue_mode(false), IssueMode::RequiresReview);
        assert_eq!(issue_mode(true), IssueMode::AutoIssueByRule);
    }

    #[test]
    fn d87_frequency_round_trip() {
        for f in [Frequency::Monthly, Frequency::Quarterly, Frequency::Annual] {
            assert_eq!(Frequency::parse(f.as_str()), Some(f));
        }
        assert_eq!(Frequency::parse("weekly"), None);
        assert_eq!(Frequency::Quarterly.months(), 3);
    }

    #[test]
    fn d87_calendar_end_does_not_error() {
        let t = template(Frequency::Annual, date(9998, 6, 1), None);
        let plans = due_drafts(&t, date(9999, 12, 31), &BTreeSet::new(), false).unwrap();
        assert_eq!(plans.len(), 2);
    }
}
