//! Frontera con `crates/verifactu` (D83; PROPUESTA, no es una decisión numerada).
//!
//! Este crate no calcula huellas, cadenas, QR ni XML: eso es de `crates/verifactu`. Aquí solo
//! se define el dato mínimo, ya validado y calculado, que una factura emitida entrega para que
//! `build_record` lo consuma: emisor, número y serie, fecha, tipo de factura, desglose y
//! totales. Los campos y el formato exactos del registro son los de la documentación técnica
//! de la AEAT (contratos.md, Verifactu); si `verifactu` necesita más datos, se amplía este
//! tipo, no se duplica la lógica.
//!
//! `F3` (factura que sustituye a simplificadas) no se genera en esta etapa: no hay decisión
//! que la prevea.

use nestadmin_documents::IsoDate;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::calc::{TaxBreakdown, Totals};
use crate::content::{
    prepare_issue, FieldIssue, InvoiceContent, Problem, RectificationKind, RectificationMethod,
};
use crate::series::{DocumentKind, DocumentNumber};

/// Tipo de factura de un registro de alta (D83).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InvoiceTypeCode {
    F1,
    F2,
    R1,
    R2,
    R3,
    R4,
    R5,
}

impl InvoiceTypeCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::F1 => "F1",
            Self::F2 => "F2",
            Self::R1 => "R1",
            Self::R2 => "R2",
            Self::R3 => "R3",
            Self::R4 => "R4",
            Self::R5 => "R5",
        }
    }
}

/// Factura rectificada. Por sustitución lleva además la base y la cuota rectificadas, que el
/// registro de la AEAT exige en ese caso (D113); por diferencias son `None`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RectifiedRef {
    pub number: DocumentNumber,
    pub issue_date: IsoDate,
    pub kind: RectificationKind,
    pub method: RectificationMethod,
    pub base: Option<Decimal>,
    pub tax: Option<Decimal>,
}

/// Datos de una factura emitida que consume `verifactu::build_record`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvoiceRecordInput {
    /// NIF del emisor, normalizado (mayúsculas, sin espacios ni guiones).
    pub issuer_tax_id: String,
    pub number: DocumentNumber,
    pub issue_date: IsoDate,
    pub invoice_type: InvoiceTypeCode,
    pub recipient_name: Option<String>,
    pub recipient_tax_id: Option<String>,
    pub rectifies: Option<RectifiedRef>,
    pub breakdown: Vec<TaxBreakdown>,
    pub tax_total: Decimal,
    pub total: Decimal,
}

fn normalize(id: &str) -> String {
    id.chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .collect::<String>()
        .to_uppercase()
}

/// Valida el contenido (D80) y construye la entrada del registro. `number` es el que acaba
/// de asignarse (D73) y debe pertenecer a la serie del tipo de documento y del año de la
/// fecha de expedición.
pub fn record_input(
    content: &InvoiceContent,
    number: DocumentNumber,
    totals: &Totals,
) -> Result<InvoiceRecordInput, Vec<FieldIssue>> {
    let checked = prepare_issue(content)?;
    let invalid = |why: &str| {
        vec![FieldIssue {
            field: "number".into(),
            problem: Problem::Invalid(why.into()),
        }]
    };
    if checked != *totals {
        return Err(vec![FieldIssue {
            field: "totals".into(),
            problem: Problem::Invalid("los totales no corresponden al contenido".into()),
        }]);
    }
    let issue_date = content.issue_date.ok_or_else(|| {
        vec![FieldIssue {
            field: "issue_date".into(),
            problem: Problem::Missing,
        }]
    })?;
    let incomplete = |field: &str| {
        vec![FieldIssue {
            field: field.into(),
            problem: Problem::Missing,
        }]
    };
    if number.series.kind != content.kind {
        return Err(invalid("la serie no corresponde al tipo de documento"));
    }
    if number.series.year != issue_date.year {
        return Err(invalid(
            "el año de la serie no es el de la fecha de expedición",
        ));
    }
    let rectifies = match (&content.rectification, content.kind) {
        (Some(r), DocumentKind::CorrectiveInvoice) => {
            match (r.rectified_number, r.rectified_date, r.kind, r.method) {
                (Some(number), Some(issue_date), Some(kind), Some(method)) => {
                    let (base, tax) = match method {
                        RectificationMethod::Substitution => {
                            let base = r
                                .rectified_base
                                .ok_or_else(|| incomplete("rectification.rectified_base"))?;
                            let tax = r
                                .rectified_tax
                                .ok_or_else(|| incomplete("rectification.rectified_tax"))?;
                            (Some(base), Some(tax))
                        }
                        RectificationMethod::Differences => (None, None),
                    };
                    Some(RectifiedRef {
                        number,
                        issue_date,
                        kind,
                        method,
                        base,
                        tax,
                    })
                }
                _ => return Err(incomplete("rectification")),
            }
        }
        _ => None,
    };
    let invoice_type = match (content.kind, rectifies.as_ref()) {
        (DocumentKind::Invoice, _) => InvoiceTypeCode::F1,
        (DocumentKind::SimplifiedInvoice, _) => InvoiceTypeCode::F2,
        (_, Some(r)) => match r.kind {
            RectificationKind::R1 => InvoiceTypeCode::R1,
            RectificationKind::R2 => InvoiceTypeCode::R2,
            RectificationKind::R3 => InvoiceTypeCode::R3,
            RectificationKind::R4 => InvoiceTypeCode::R4,
            RectificationKind::R5 => InvoiceTypeCode::R5,
        },
        _ => {
            return Err(invalid(
                "el tipo de documento no genera un registro de factura",
            ))
        }
    };
    let text = |v: &Option<String>| {
        v.as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    let recipient = content.recipient.as_ref();
    Ok(InvoiceRecordInput {
        issuer_tax_id: normalize(content.issuer.tax_id.as_deref().unwrap_or_default()),
        number,
        issue_date,
        invoice_type,
        recipient_name: recipient.and_then(|r| text(&r.name)),
        recipient_tax_id: recipient
            .and_then(|r| text(&r.tax_id))
            .map(|s| normalize(&s)),
        rectifies,
        breakdown: totals.breakdown.clone(),
        tax_total: totals.tax_total,
        total: totals.total,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calc::{LineInput, TaxRate};
    use crate::content::{Party, Rectification};
    use crate::series::SeriesKey;
    use rust_decimal::Decimal;

    fn party(nif: &str) -> Party {
        Party {
            name: Some("Entidad Ficticia".into()),
            tax_id: Some(nif.into()),
            address: Some("Calle Ficticia 1".into()),
        }
    }

    fn content(kind: DocumentKind) -> InvoiceContent {
        InvoiceContent {
            kind,
            issue_date: Some(IsoDate {
                year: 2026,
                month: 11,
                day: 6,
            }),
            operation_date: None,
            issuer: party("b-12345674"),
            recipient: Some(party("A12345674")),
            lines: vec![LineInput {
                description: "Servicio".into(),
                quantity: Decimal::ONE,
                unit_price: Decimal::from(100),
                discount: Decimal::ZERO,
                tax_rate: TaxRate::TwentyOne,
                exemption: None,
            }],
            withholding_percent: None,
            rectification: None,
            recipient_requested_tax_id: false,
            art_4_2_declared: false,
        }
    }

    fn number(kind: DocumentKind, year: u16) -> DocumentNumber {
        DocumentNumber {
            series: SeriesKey::new(kind, year).unwrap(),
            sequence: 7,
        }
    }

    #[test]
    fn d83_full_invoice_is_f1_with_normalized_ids() {
        let c = content(DocumentKind::Invoice);
        let t = prepare_issue(&c).unwrap();
        let r = record_input(&c, number(DocumentKind::Invoice, 2026), &t).unwrap();
        assert_eq!(r.invoice_type, InvoiceTypeCode::F1);
        assert_eq!(r.issuer_tax_id, "B12345674");
        assert_eq!(r.recipient_tax_id.as_deref(), Some("A12345674"));
        assert_eq!(r.number.to_string(), "F-2026-0007");
        assert_eq!(r.total.to_string(), "121.00");
        assert_eq!(r.tax_total.to_string(), "21.00");
        assert_eq!(r.breakdown.len(), 1);
        assert!(r.rectifies.is_none());
    }

    #[test]
    fn d83_simplified_is_f2() {
        let mut c = content(DocumentKind::SimplifiedInvoice);
        c.recipient = None;
        let t = prepare_issue(&c).unwrap();
        let r = record_input(&c, number(DocumentKind::SimplifiedInvoice, 2026), &t).unwrap();
        assert_eq!(r.invoice_type, InvoiceTypeCode::F2);
        assert_eq!(r.recipient_tax_id, None);
    }

    #[test]
    fn d83_corrective_carries_type_and_reference() {
        let mut c = content(DocumentKind::CorrectiveInvoice);
        c.rectification = Some(Rectification {
            rectified_number: Some(number(DocumentKind::Invoice, 2026)),
            rectified_date: Some(IsoDate {
                year: 2026,
                month: 10,
                day: 1,
            }),
            kind: Some(RectificationKind::R4),
            method: Some(RectificationMethod::Substitution),
            rectified_base: Some(Decimal::from(80)),
            rectified_tax: Some(Decimal::new(1680, 2)),
        });
        let t = prepare_issue(&c).unwrap();
        let r = record_input(&c, number(DocumentKind::CorrectiveInvoice, 2026), &t).unwrap();
        assert_eq!(r.invoice_type, InvoiceTypeCode::R4);
        assert_eq!(r.invoice_type.as_str(), "R4");
        let rectified = r.rectifies.unwrap();
        assert_eq!(rectified.number.to_string(), "F-2026-0007");
        // Por sustitución constan la base y la cuota rectificadas (D113).
        assert_eq!(rectified.base, Some(Decimal::from(80)));
        assert_eq!(rectified.tax, Some(Decimal::new(1680, 2)));
    }

    #[test]
    fn d99_differences_carry_no_rectified_amounts() {
        let mut c = content(DocumentKind::CorrectiveInvoice);
        c.rectification = Some(Rectification {
            rectified_number: Some(number(DocumentKind::Invoice, 2026)),
            rectified_date: Some(IsoDate {
                year: 2026,
                month: 10,
                day: 1,
            }),
            kind: Some(RectificationKind::R1),
            method: Some(RectificationMethod::Differences),
            rectified_base: None,
            rectified_tax: None,
        });
        c.lines[0].quantity = Decimal::from(-1);
        let t = prepare_issue(&c).unwrap();
        let r = record_input(&c, number(DocumentKind::CorrectiveInvoice, 2026), &t).unwrap();
        let rectified = r.rectifies.unwrap();
        assert_eq!((rectified.base, rectified.tax), (None, None));
    }

    #[test]
    fn d83_incomplete_corrective_reports_a_field_not_an_empty_list() {
        let mut c = content(DocumentKind::CorrectiveInvoice);
        c.rectification = Some(Rectification::default());
        // El contenido ya se rechaza antes con la lista completa de campos.
        let t = prepare_issue(&content(DocumentKind::Invoice)).unwrap();
        let err = record_input(&c, number(DocumentKind::CorrectiveInvoice, 2026), &t).unwrap_err();
        assert!(!err.is_empty());
        // Un presupuesto no genera registro y lo dice.
        let mut q = content(DocumentKind::Invoice);
        q.kind = DocumentKind::Quote;
        let err = record_input(&q, number(DocumentKind::Quote, 2026), &t).unwrap_err();
        assert!(!err.is_empty());
    }

    #[test]
    fn d83_rejects_wrong_series_year_or_stale_totals() {
        let c = content(DocumentKind::Invoice);
        let t = prepare_issue(&c).unwrap();
        assert!(record_input(&c, number(DocumentKind::SimplifiedInvoice, 2026), &t).is_err());
        assert!(record_input(&c, number(DocumentKind::Invoice, 2025), &t).is_err());
        let mut other = t.clone();
        other.total += Decimal::ONE;
        let err = record_input(&c, number(DocumentKind::Invoice, 2026), &other).unwrap_err();
        assert_eq!(err[0].field, "totals");
    }

    #[test]
    fn d83_incomplete_content_is_not_handed_over() {
        let mut c = content(DocumentKind::Invoice);
        c.issuer.tax_id = None;
        let t = prepare_issue(&content(DocumentKind::Invoice)).unwrap();
        let err = record_input(&c, number(DocumentKind::Invoice, 2026), &t).unwrap_err();
        assert_eq!(err[0].field, "issuer.tax_id");
    }
}
