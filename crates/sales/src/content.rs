//! Contenido obligatorio de la factura y límite de la simplificada (D80, D81, R17).
//!
//! La emisión se rechaza con `invalid_input` si falta un dato obligatorio: [`prepare_issue`]
//! devuelve la lista **completa** de campos ausentes o incorrectos, no solo el primero.
//!
//! * Completa (art. 6 RD 1619/2012): fecha de expedición, nombre, NIF y domicilio de emisor
//!   y destinatario, líneas con descripción, desglose por tipo con base y cuota, total y la
//!   causa de exención (o mención de inversión del sujeto pasivo) en las líneas al 0 %.
//! * Simplificada (art. 7): hasta 400 € con IVA, o 3.000 € si el usuario declara una de las
//!   actividades del art. 4.2; sin NIF del destinatario salvo que lo pida (entonces se exigen
//!   y se validan su NIF y su domicilio, art. 7.2 RD 1619/2012).
//! * Rectificativa (D81): referencia a la factura rectificada (número y fecha), tipo R1–R5 y
//!   método. R5 si y solo si se rectifica una simplificada, y entonces rige el contenido de la
//!   simplificada. Por diferencias, las líneas pueden ser negativas.
//!
//! Interpretaciones conservadoras:
//! * El número no se valida aquí: se asigna al emitir, en la misma transacción (D73).
//! * El límite de la simplificada se compara con el total con IVA, en valor absoluto, y
//!   «hasta» incluye el límite exacto.
//! * Un identificador de destinatario que no sea NIF/NIE/CIF español se rechaza (los clientes
//!   extranjeros quedan fuera de D80/D91 hasta que se decida).
//! * La fecha de la factura rectificada no puede ser posterior a la de expedición.

use nestadmin_documents::IsoDate;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::calc::{calculate, CalcError, LineInput, LineMode, Totals};
use crate::series::{DocumentKind, DocumentNumber};
use crate::tax_id::validate_tax_id;

/// Límite de la factura simplificada con IVA incluido (D80).
pub const SIMPLIFIED_LIMIT: Decimal = Decimal::from_parts(400, 0, 0, false, 0);
/// Límite si el usuario declara una de las actividades del art. 4.2 (D80).
pub const SIMPLIFIED_LIMIT_ART_4_2: Decimal = Decimal::from_parts(3000, 0, 0, false, 0);

pub fn simplified_limit(art_4_2_declared: bool) -> Decimal {
    if art_4_2_declared {
        SIMPLIFIED_LIMIT_ART_4_2
    } else {
        SIMPLIFIED_LIMIT
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Party {
    pub name: Option<String>,
    pub tax_id: Option<String>,
    pub address: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RectificationKind {
    R1,
    R2,
    R3,
    R4,
    R5,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RectificationMethod {
    /// Importes completos nuevos.
    Substitution,
    /// Solo la variación, que puede ser negativa.
    Differences,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Rectification {
    pub rectified_number: Option<DocumentNumber>,
    pub rectified_date: Option<IsoDate>,
    pub kind: Option<RectificationKind>,
    pub method: Option<RectificationMethod>,
    /// Base imponible de la factura rectificada. Obligatoria por sustitución (D99); no se
    /// admite por diferencias.
    pub rectified_base: Option<Decimal>,
    /// Cuota de la factura rectificada. Misma regla que la base.
    pub rectified_tax: Option<Decimal>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct InvoiceContent {
    pub kind: DocumentKind,
    pub issue_date: Option<IsoDate>,
    pub operation_date: Option<IsoDate>,
    pub issuer: Party,
    pub recipient: Option<Party>,
    pub lines: Vec<LineInput>,
    pub withholding_percent: Option<Decimal>,
    pub rectification: Option<Rectification>,
    /// El destinatario de una simplificada pidió consignar su NIF.
    pub recipient_requested_tax_id: bool,
    /// El usuario declaró una actividad del art. 4.2 (límite de 3.000 €).
    pub art_4_2_declared: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Problem {
    Missing,
    Invalid(String),
}

/// Campo ausente o incorrecto. `field` es una ruta estable: `issuer.tax_id`,
/// `recipient.address`, `lines[2].description`, `rectification.kind`, `total`...
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldIssue {
    pub field: String,
    pub problem: Problem,
}

impl FieldIssue {
    fn missing(field: impl Into<String>) -> Self {
        Self {
            field: field.into(),
            problem: Problem::Missing,
        }
    }

    fn invalid(field: impl Into<String>, why: impl Into<String>) -> Self {
        Self {
            field: field.into(),
            problem: Problem::Invalid(why.into()),
        }
    }
}

fn present(value: &Option<String>) -> Option<&str> {
    value.as_deref().map(str::trim).filter(|v| !v.is_empty())
}

fn check_party(
    prefix: &str,
    party: &Party,
    require_all: bool,
    require_tax_id: bool,
    out: &mut Vec<FieldIssue>,
) {
    if require_all && present(&party.name).is_none() {
        out.push(FieldIssue::missing(format!("{prefix}.name")));
    }
    if require_all && present(&party.address).is_none() {
        out.push(FieldIssue::missing(format!("{prefix}.address")));
    }
    match present(&party.tax_id) {
        None if require_all || require_tax_id => {
            out.push(FieldIssue::missing(format!("{prefix}.tax_id")))
        }
        None => {}
        Some(id) => {
            if let Err(e) = validate_tax_id(id) {
                out.push(FieldIssue::invalid(
                    format!("{prefix}.tax_id"),
                    e.to_string(),
                ));
            }
        }
    }
}

fn calc_issue(error: &CalcError, line_override: Option<usize>) -> FieldIssue {
    let at = |line: &usize| line_override.unwrap_or(*line);
    match error {
        CalcError::NoLines => FieldIssue::missing("lines"),
        CalcError::EmptyDescription { line } => {
            FieldIssue::missing(format!("lines[{}].description", at(line)))
        }
        CalcError::NonPositiveQuantity { line } | CalcError::ZeroQuantity { line } => {
            FieldIssue::invalid(format!("lines[{}].quantity", at(line)), error.to_string())
        }
        CalcError::NegativePrice { line } => {
            FieldIssue::invalid(format!("lines[{}].unit_price", at(line)), error.to_string())
        }
        CalcError::DiscountOutOfRange { line } => {
            FieldIssue::invalid(format!("lines[{}].discount", at(line)), error.to_string())
        }
        CalcError::ExemptionRequired { line } => {
            FieldIssue::missing(format!("lines[{}].exemption", at(line)))
        }
        CalcError::ExemptionNotAllowed { line } => {
            FieldIssue::invalid(format!("lines[{}].exemption", at(line)), error.to_string())
        }
        CalcError::UnsupportedTaxRate { .. } => FieldIssue::invalid("lines", error.to_string()),
        CalcError::WithholdingOutOfRange => {
            FieldIssue::invalid("withholding_percent", error.to_string())
        }
        CalcError::Overflow => FieldIssue::invalid("total", error.to_string()),
    }
}

/// `true` si el contenido sigue las reglas de la simplificada (incluida una rectificativa R5).
fn is_simplified_shape(content: &InvoiceContent) -> bool {
    match content.kind {
        DocumentKind::SimplifiedInvoice => true,
        DocumentKind::CorrectiveInvoice => content
            .rectification
            .as_ref()
            .and_then(|r| r.rectified_number.as_ref())
            .is_some_and(|n| n.series.kind == DocumentKind::SimplifiedInvoice),
        _ => false,
    }
}

fn check_rectification(content: &InvoiceContent, out: &mut Vec<FieldIssue>) {
    let Some(r) = content.rectification.as_ref() else {
        out.push(FieldIssue::missing("rectification"));
        return;
    };
    match &r.rectified_number {
        None => out.push(FieldIssue::missing("rectification.rectified_number")),
        Some(n) => {
            if !matches!(
                n.series.kind,
                DocumentKind::Invoice | DocumentKind::SimplifiedInvoice
            ) {
                out.push(FieldIssue::invalid(
                    "rectification.rectified_number",
                    "solo se rectifica una factura completa o simplificada",
                ));
            }
        }
    }
    match (r.rectified_date, content.issue_date) {
        (None, _) => out.push(FieldIssue::missing("rectification.rectified_date")),
        (Some(rectified), Some(issue)) if rectified > issue => out.push(FieldIssue::invalid(
            "rectification.rectified_date",
            "la factura rectificada no puede ser posterior a la rectificativa",
        )),
        _ => {}
    }
    match r.kind {
        None => out.push(FieldIssue::missing("rectification.kind")),
        Some(kind) => {
            if let Some(n) = &r.rectified_number {
                let rectifies_simplified = n.series.kind == DocumentKind::SimplifiedInvoice;
                if rectifies_simplified != (kind == RectificationKind::R5) {
                    out.push(FieldIssue::invalid(
                        "rectification.kind",
                        "R5 corresponde a rectificar una simplificada y solo a ella",
                    ));
                }
            }
        }
    }
    match r.method {
        None => out.push(FieldIssue::missing("rectification.method")),
        Some(RectificationMethod::Substitution) => {
            if r.rectified_base.is_none() {
                out.push(FieldIssue::missing("rectification.rectified_base"));
            }
            if r.rectified_tax.is_none() {
                out.push(FieldIssue::missing("rectification.rectified_tax"));
            }
        }
        Some(RectificationMethod::Differences) => {
            if r.rectified_base.is_some() || r.rectified_tax.is_some() {
                out.push(FieldIssue::invalid(
                    "rectification.rectified_base",
                    "la base y la cuota rectificadas solo constan en la rectificación por sustitución",
                ));
            }
        }
    }
}

fn check(content: &InvoiceContent) -> (Vec<FieldIssue>, Option<Totals>) {
    let mut out = Vec::new();
    if !content.kind.is_invoice() {
        out.push(FieldIssue::invalid(
            "kind",
            "el contenido de D80 solo se exige a facturas",
        ));
        return (out, None);
    }
    if content.issue_date.is_none() {
        out.push(FieldIssue::missing("issue_date"));
    }
    check_party("issuer", &content.issuer, true, true, &mut out);

    let corrective = content.kind == DocumentKind::CorrectiveInvoice;
    if corrective {
        check_rectification(content, &mut out);
    } else if content.rectification.is_some() {
        out.push(FieldIssue::invalid(
            "rectification",
            "solo una rectificativa lleva referencia a otra factura",
        ));
    }

    if is_simplified_shape(content) {
        if let Some(recipient) = &content.recipient {
            check_party(
                "recipient",
                recipient,
                false,
                content.recipient_requested_tax_id,
                &mut out,
            );
            if content.recipient_requested_tax_id && present(&recipient.address).is_none() {
                out.push(FieldIssue::missing("recipient.address"));
            }
        } else if content.recipient_requested_tax_id {
            out.push(FieldIssue::missing("recipient.tax_id"));
            out.push(FieldIssue::missing("recipient.address"));
        }
    } else {
        match &content.recipient {
            Some(recipient) => check_party("recipient", recipient, true, true, &mut out),
            None => out.push(FieldIssue::missing("recipient")),
        }
    }

    let mode = match content
        .rectification
        .as_ref()
        .and_then(|r| r.method)
        .filter(|_| corrective)
    {
        Some(RectificationMethod::Differences) => LineMode::Difference,
        _ => LineMode::Standard,
    };
    // Cada línea por separado para informar de todas las que fallan.
    let mut line_failed = false;
    if content.lines.is_empty() {
        out.push(FieldIssue::missing("lines"));
        line_failed = true;
    }
    for (i, line) in content.lines.iter().enumerate() {
        if let Err(e) = calculate(std::slice::from_ref(line), None, mode) {
            out.push(calc_issue(&e, Some(i + 1)));
            line_failed = true;
        }
    }
    if line_failed {
        return (out, None);
    }
    match calculate(&content.lines, content.withholding_percent, mode) {
        Err(e) => {
            out.push(calc_issue(&e, None));
            (out, None)
        }
        Ok(totals) => {
            if is_simplified_shape(content) {
                let limit = simplified_limit(content.art_4_2_declared);
                if totals.total.abs() > limit {
                    out.push(FieldIssue::invalid(
                        "total",
                        format!("la simplificada no puede superar {limit} EUR con IVA"),
                    ));
                }
            }
            (out, Some(totals))
        }
    }
}

/// Lista de campos ausentes o incorrectos; vacía si la factura puede emitirse.
pub fn validate_for_issue(content: &InvoiceContent) -> Vec<FieldIssue> {
    check(content).0
}

/// Valida y calcula: `Ok(Totals)` solo si no falta nada; si no, la lista completa de
/// problemas (`invalid_input`). No consume número ni cambia estado.
pub fn prepare_issue(content: &InvoiceContent) -> Result<Totals, Vec<FieldIssue>> {
    match check(content) {
        (issues, Some(totals)) if issues.is_empty() => Ok(totals),
        (issues, _) => Err(issues),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calc::{Exemption, TaxQualification, TaxRate};
    use crate::series::SeriesKey;
    use std::str::FromStr;

    fn date(y: u16, m: u8, d: u8) -> IsoDate {
        IsoDate {
            year: y,
            month: m,
            day: d,
        }
    }

    fn dec(s: &str) -> Decimal {
        Decimal::from_str(s).unwrap()
    }

    fn party(name: &str, nif: &str) -> Party {
        Party {
            name: Some(name.into()),
            tax_id: Some(nif.into()),
            address: Some("Calle Ficticia 1, 00000 Ciudad Inventada".into()),
        }
    }

    fn line(price: &str, rate: TaxRate) -> LineInput {
        LineInput {
            description: "Servicio sintético".into(),
            quantity: dec("1"),
            unit_price: dec(price),
            discount: dec("0"),
            tax_rate: rate,
            exemption: None,
        }
    }

    fn exempt_line(price: &str) -> LineInput {
        LineInput {
            exemption: Some(Exemption {
                qualification: TaxQualification::E1,
                description: Some("Exenta (causa sintética)".into()),
            }),
            ..line(price, TaxRate::Zero)
        }
    }

    fn full() -> InvoiceContent {
        InvoiceContent {
            kind: DocumentKind::Invoice,
            issue_date: Some(date(2026, 11, 6)),
            operation_date: None,
            issuer: party("Emisora Ficticia SL", "B12345674"),
            recipient: Some(party("Cliente Inventado SA", "A12345674")),
            lines: vec![line("100", TaxRate::TwentyOne)],
            withholding_percent: None,
            rectification: None,
            recipient_requested_tax_id: false,
            art_4_2_declared: false,
        }
    }

    fn simplified(price: &str) -> InvoiceContent {
        InvoiceContent {
            kind: DocumentKind::SimplifiedInvoice,
            recipient: None,
            lines: vec![line(price, TaxRate::Ten)],
            ..full()
        }
    }

    fn fields(issues: &[FieldIssue]) -> Vec<&str> {
        issues.iter().map(|i| i.field.as_str()).collect()
    }

    fn number(kind: DocumentKind) -> DocumentNumber {
        DocumentNumber {
            series: SeriesKey::new(kind, 2026).unwrap(),
            sequence: 1,
        }
    }

    fn corrective(kind_r: RectificationKind, rectified: DocumentKind) -> InvoiceContent {
        InvoiceContent {
            kind: DocumentKind::CorrectiveInvoice,
            rectification: Some(Rectification {
                rectified_number: Some(number(rectified)),
                rectified_date: Some(date(2026, 10, 1)),
                kind: Some(kind_r),
                method: Some(RectificationMethod::Substitution),
                rectified_base: Some(dec("100.00")),
                rectified_tax: Some(dec("21.00")),
            }),
            ..full()
        }
    }

    #[test]
    fn d80_complete_invoice_passes_and_returns_totals() {
        let totals = prepare_issue(&full()).unwrap();
        assert_eq!(totals.total.to_string(), "121.00");
        assert!(validate_for_issue(&full()).is_empty());
    }

    #[test]
    fn d80_all_missing_fields_are_listed_not_only_the_first() {
        let c = InvoiceContent {
            issue_date: None,
            issuer: Party::default(),
            recipient: Some(Party::default()),
            lines: vec![],
            ..full()
        };
        let issues = prepare_issue(&c).unwrap_err();
        assert_eq!(
            fields(&issues),
            vec![
                "issue_date",
                "issuer.name",
                "issuer.address",
                "issuer.tax_id",
                "recipient.name",
                "recipient.address",
                "recipient.tax_id",
                "lines",
            ]
        );
        assert!(issues.iter().all(|i| i.problem == Problem::Missing));
    }

    #[test]
    fn d80_blank_strings_count_as_missing() {
        let mut c = full();
        c.issuer.name = Some("   ".into());
        c.recipient.as_mut().unwrap().address = Some(String::new());
        assert_eq!(
            fields(&validate_for_issue(&c)),
            vec!["issuer.name", "recipient.address"]
        );
    }

    #[test]
    fn d80_missing_recipient_on_full_invoice() {
        let c = InvoiceContent {
            recipient: None,
            ..full()
        };
        assert_eq!(fields(&validate_for_issue(&c)), vec!["recipient"]);
    }

    #[test]
    fn d80_invalid_tax_ids_are_reported_as_invalid() {
        let mut c = full();
        c.issuer.tax_id = Some("12345678A".into());
        c.recipient.as_mut().unwrap().tax_id = Some("ZZZ".into());
        let issues = validate_for_issue(&c);
        assert_eq!(fields(&issues), vec!["issuer.tax_id", "recipient.tax_id"]);
        assert!(issues
            .iter()
            .all(|i| matches!(i.problem, Problem::Invalid(_))));
    }

    #[test]
    fn d80_line_problems_are_all_listed_with_their_line_number() {
        let mut c = full();
        c.lines = vec![
            LineInput {
                description: String::new(),
                ..line("10", TaxRate::Ten)
            },
            line("10", TaxRate::Ten),
            LineInput {
                quantity: dec("0"),
                ..line("10", TaxRate::Ten)
            },
            line("10", TaxRate::Zero),
        ];
        assert_eq!(
            fields(&validate_for_issue(&c)),
            vec![
                "lines[1].description",
                "lines[3].quantity",
                "lines[4].exemption"
            ]
        );
    }

    #[test]
    fn d80_exemption_qualification_is_the_mention_for_zero_rate() {
        let mut c = full();
        c.lines = vec![LineInput {
            exemption: Some(Exemption {
                qualification: TaxQualification::S2,
                description: Some("Inversión del sujeto pasivo (sintética)".into()),
            }),
            ..line("50", TaxRate::Zero)
        }];
        let t = prepare_issue(&c).unwrap();
        assert_eq!(t.total.to_string(), "50.00");
        assert_eq!(t.breakdown[0].qualification, Some(TaxQualification::S2));
        assert_eq!(
            t.breakdown[0].descriptions,
            vec!["Inversión del sujeto pasivo (sintética)"]
        );
    }

    #[test]
    fn d80_quote_is_not_an_invoice() {
        let c = InvoiceContent {
            kind: DocumentKind::Quote,
            ..full()
        };
        assert_eq!(fields(&validate_for_issue(&c)), vec!["kind"]);
    }

    #[test]
    fn d80_withholding_out_of_range_is_reported() {
        let mut c = full();
        c.withholding_percent = Some(dec("101"));
        assert_eq!(fields(&validate_for_issue(&c)), vec!["withholding_percent"]);
        c.withholding_percent = Some(dec("15"));
        let t = prepare_issue(&c).unwrap();
        assert_eq!(t.amount_due.to_string(), "106.00");
    }

    #[test]
    fn d80_simplified_limit_400_boundaries() {
        // 400 € con IVA exactos: permitido. 400,01: rechazado.
        let mut c = simplified("0");
        c.lines = vec![line("330.5785123966942", TaxRate::TwentyOne)];
        // base 330.58 + cuota 69.42 = 400.00
        assert_eq!(prepare_issue(&c).unwrap().total.to_string(), "400.00");
        c.lines = vec![line("330.59", TaxRate::TwentyOne)];
        // 330.59 + 69.42 = 400.01
        let issues = prepare_issue(&c).unwrap_err();
        assert_eq!(fields(&issues), vec!["total"]);
        assert!(matches!(issues[0].problem, Problem::Invalid(_)));
    }

    #[test]
    fn d80_simplified_limit_3000_needs_art_4_2_declaration() {
        let mut c = simplified("0");
        c.lines = vec![line("2000", TaxRate::TwentyOne)]; // 2420 con IVA
        assert_eq!(fields(&validate_for_issue(&c)), vec!["total"]);
        c.art_4_2_declared = true;
        assert!(prepare_issue(&c).is_ok());
        // 3000 exactos con la declaración: permitido; 3000,01: no.
        c.lines = vec![exempt_line("3000")];
        assert_eq!(prepare_issue(&c).unwrap().total.to_string(), "3000.00");
        c.lines = vec![exempt_line("3000.01")];
        assert_eq!(fields(&validate_for_issue(&c)), vec!["total"]);
        assert_eq!(simplified_limit(false).to_string(), "400");
        assert_eq!(simplified_limit(true).to_string(), "3000");
    }

    #[test]
    fn d80_simplified_without_recipient_tax_id_is_allowed() {
        let mut c = simplified("0");
        c.lines = vec![line("10", TaxRate::Ten)];
        assert!(prepare_issue(&c).is_ok());
        // Con destinatario incompleto también, mientras no pida su NIF.
        c.recipient = Some(Party {
            name: Some("Cliente de mostrador".into()),
            ..Party::default()
        });
        assert!(prepare_issue(&c).is_ok());
    }

    #[test]
    fn d80_simplified_recipient_requesting_tax_id_must_provide_a_valid_one() {
        let mut c = simplified("0");
        c.lines = vec![line("10", TaxRate::Ten)];
        c.recipient_requested_tax_id = true;
        assert_eq!(
            fields(&validate_for_issue(&c)),
            vec!["recipient.tax_id", "recipient.address"]
        );
        c.recipient = Some(party("Cliente Inventado", "12345678A"));
        assert_eq!(fields(&validate_for_issue(&c)), vec!["recipient.tax_id"]);
        c.recipient = Some(party("Cliente Inventado", "12345678Z"));
        assert!(prepare_issue(&c).is_ok());
        // Art. 7.2: si pide su NIF, también consta su domicilio.
        c.recipient.as_mut().unwrap().address = None;
        assert_eq!(fields(&validate_for_issue(&c)), vec!["recipient.address"]);
        c.recipient.as_mut().unwrap().address = Some("  ".into());
        assert_eq!(fields(&validate_for_issue(&c)), vec!["recipient.address"]);
        c.recipient = None;
        assert_eq!(
            fields(&validate_for_issue(&c)),
            vec!["recipient.tax_id", "recipient.address"]
        );
        c.recipient = Some(party("Cliente Inventado", "12345678Z"));
        // Sin pedir el NIF no se exige el domicilio.
        c.recipient_requested_tax_id = false;
        c.recipient.as_mut().unwrap().address = None;
        assert!(prepare_issue(&c).is_ok());
        // Un NIF aportado sin pedirlo también se valida.
        c.recipient_requested_tax_id = false;
        c.recipient.as_mut().unwrap().tax_id = Some("bad".into());
        assert_eq!(fields(&validate_for_issue(&c)), vec!["recipient.tax_id"]);
    }

    #[test]
    fn d80_simplified_still_needs_issuer_and_date() {
        let mut c = simplified("0");
        c.lines = vec![line("10", TaxRate::Ten)];
        c.issue_date = None;
        c.issuer.tax_id = None;
        assert_eq!(
            fields(&validate_for_issue(&c)),
            vec!["issue_date", "issuer.tax_id"]
        );
    }

    #[test]
    fn d81_corrective_requires_full_reference() {
        let c = InvoiceContent {
            kind: DocumentKind::CorrectiveInvoice,
            rectification: None,
            ..full()
        };
        assert_eq!(fields(&validate_for_issue(&c)), vec!["rectification"]);
        let c = InvoiceContent {
            kind: DocumentKind::CorrectiveInvoice,
            rectification: Some(Rectification::default()),
            ..full()
        };
        assert_eq!(
            fields(&validate_for_issue(&c)),
            vec![
                "rectification.rectified_number",
                "rectification.rectified_date",
                "rectification.kind",
                "rectification.method"
            ]
        );
    }

    #[test]
    fn d99_substitution_requires_rectified_base_and_tax() {
        let mut c = corrective(RectificationKind::R1, DocumentKind::Invoice);
        let r = c.rectification.as_mut().unwrap();
        r.rectified_base = None;
        r.rectified_tax = None;
        assert_eq!(
            fields(&validate_for_issue(&c)),
            vec![
                "rectification.rectified_base",
                "rectification.rectified_tax"
            ]
        );
        assert!(
            validate_for_issue(&corrective(RectificationKind::R1, DocumentKind::Invoice))
                .is_empty()
        );
    }

    #[test]
    fn d81_r1_to_r4_rectify_full_invoices_and_r5_only_simplified() {
        for k in [
            RectificationKind::R1,
            RectificationKind::R2,
            RectificationKind::R3,
            RectificationKind::R4,
        ] {
            assert!(validate_for_issue(&corrective(k, DocumentKind::Invoice)).is_empty());
            // R1-R4 sobre una simplificada: incorrecto.
            let mut c = corrective(k, DocumentKind::SimplifiedInvoice);
            c.recipient = None;
            assert_eq!(fields(&validate_for_issue(&c)), vec!["rectification.kind"]);
        }
        // R5 sobre una completa: incorrecto.
        let c = corrective(RectificationKind::R5, DocumentKind::Invoice);
        assert_eq!(fields(&validate_for_issue(&c)), vec!["rectification.kind"]);
        // R5 sobre una simplificada: correcto y sin destinatario obligatorio.
        let mut c = corrective(RectificationKind::R5, DocumentKind::SimplifiedInvoice);
        c.recipient = None;
        assert!(validate_for_issue(&c).is_empty());
    }

    #[test]
    fn d81_cannot_rectify_a_quote_or_a_corrective() {
        for kind in [DocumentKind::Quote, DocumentKind::CorrectiveInvoice] {
            let c = corrective(RectificationKind::R1, kind);
            assert!(fields(&validate_for_issue(&c)).contains(&"rectification.rectified_number"));
        }
    }

    #[test]
    fn d81_rectified_date_cannot_be_after_the_corrective() {
        let mut c = corrective(RectificationKind::R1, DocumentKind::Invoice);
        c.rectification.as_mut().unwrap().rectified_date = Some(date(2026, 11, 7));
        assert_eq!(
            fields(&validate_for_issue(&c)),
            vec!["rectification.rectified_date"]
        );
    }

    #[test]
    fn d81_differences_allow_negative_lines_substitution_does_not() {
        let negative = vec![LineInput {
            quantity: dec("-1"),
            ..line("100", TaxRate::TwentyOne)
        }];
        let mut c = corrective(RectificationKind::R1, DocumentKind::Invoice);
        c.lines = negative.clone();
        assert_eq!(fields(&validate_for_issue(&c)), vec!["lines[1].quantity"]);
        c.rectification.as_mut().unwrap().method = Some(RectificationMethod::Differences);
        // Por diferencias no constan la base ni la cuota rectificadas.
        assert_eq!(
            fields(&validate_for_issue(&c)),
            vec!["rectification.rectified_base"]
        );
        let r = c.rectification.as_mut().unwrap();
        r.rectified_base = None;
        r.rectified_tax = None;
        assert_eq!(prepare_issue(&c).unwrap().total.to_string(), "-121.00");
        // Una factura normal con líneas negativas sigue rechazada.
        let mut f = full();
        f.lines = negative;
        assert_eq!(fields(&validate_for_issue(&f)), vec!["lines[1].quantity"]);
    }

    #[test]
    fn d81_non_corrective_cannot_carry_a_rectification() {
        let mut c = full();
        c.rectification = Some(Rectification::default());
        assert_eq!(fields(&validate_for_issue(&c)), vec!["rectification"]);
    }

    #[test]
    fn d81_r5_simplified_limit_applies_to_absolute_total() {
        let mut c = corrective(RectificationKind::R5, DocumentKind::SimplifiedInvoice);
        c.recipient = None;
        let r = c.rectification.as_mut().unwrap();
        r.method = Some(RectificationMethod::Differences);
        r.rectified_base = None;
        r.rectified_tax = None;
        c.lines = vec![LineInput {
            quantity: dec("-1"),
            ..line("500", TaxRate::TwentyOne)
        }];
        assert_eq!(fields(&validate_for_issue(&c)), vec!["total"]);
    }
}
