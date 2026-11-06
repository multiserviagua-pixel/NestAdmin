//! Cobros y estado de cobro derivado (R19, D88).
//!
//! D88: cada cobro tiene fecha, importe y medio (`transfer`, `cash`, `card`, `other`). El
//! estado de cobro **se deriva**, nunca se almacena: sin cobros `pending`; con cobros por
//! debajo del total `partial`; igual al total `paid`; y `overdue` si pasa el vencimiento sin
//! estar cobrada. Un cobro se puede eliminar con entrada en el historial (la entrada la
//! escribe quien llama con el [`Payment`] devuelto); no afecta a Verifactu (D88).
//!
//! Interpretaciones conservadoras (la spec no las fija):
//! * **Importe a cobrar** = importe a pagar de D75 (`Totals::amount_due`, es decir, total
//!   menos retención de IRPF), porque D75 dice que la retención reduce lo que se paga. El
//!   llamador lo pasa en [`Receivable::payable`].
//! * **Sin devoluciones ni excesos**: D88 no define cobros negativos ni sobrepagos. Un cobro
//!   debe ser positivo con a lo sumo dos decimales y la suma no puede superar el importe a
//!   cobrar. Una devolución o un cobro en exceso se resuelve con la rectificativa (D81) y su
//!   propio cobro o eliminando el cobro erróneo.
//! * **Solo facturas** (completa, simplificada o rectificativa) en `Issued` o `Sent`. Un
//!   borrador no tiene número ni obligación de cobro; una anulada (D82) no se cobra.
//! * Una rectificativa cuyo importe a cobrar es cero o negativo (por diferencias) no tiene
//!   estado de cobro: [`PaymentError::NothingToCollect`].
//! * **Vencida** = hoy es estrictamente posterior al vencimiento y queda algo por cobrar; la
//!   vencida prevalece sobre la parcial, y el resumen conserva lo cobrado y lo pendiente.
//!   Sin vencimiento nunca vence (R19 lo exige para facturas, pero el dato puede faltar).
//! * El vencimiento por defecto se calcula con [`due_date`] (fecha de expedición + días
//!   indicados por el usuario); la spec no fija un plazo estándar.

use std::collections::HashSet;

use nestadmin_documents::IsoDate;
use rust_decimal::Decimal;

use crate::calc::round_money;
use crate::dates;
use crate::series::DocumentKind;
use crate::status::Status;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PaymentMethod {
    Transfer,
    Cash,
    Card,
    Other,
}

impl PaymentMethod {
    pub const ALL: [PaymentMethod; 4] = [Self::Transfer, Self::Cash, Self::Card, Self::Other];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Transfer => "transfer",
            Self::Cash => "cash",
            Self::Card => "card",
            Self::Other => "other",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|m| m.as_str() == text)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Payment {
    /// Identificador asignado por la persistencia; único dentro de la factura.
    pub id: u64,
    pub date: IsoDate,
    pub amount: Decimal,
    pub method: PaymentMethod,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CollectionState {
    Pending,
    Partial,
    Paid,
    Overdue,
}

impl CollectionState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Partial => "partial",
            Self::Paid => "paid",
            Self::Overdue => "overdue",
        }
    }
}

/// Lo mínimo de una factura que necesita el cobro.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Receivable {
    pub kind: DocumentKind,
    pub status: Status,
    /// Importe a cobrar (ver nota del módulo): `Totals::amount_due`.
    pub payable: Decimal,
    pub due_date: Option<IsoDate>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollectionSummary {
    pub state: CollectionState,
    pub payable: Decimal,
    pub paid: Decimal,
    pub outstanding: Decimal,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PaymentError {
    #[error("solo las facturas tienen cobros (documento: {0})")]
    NotAnInvoice(&'static str),
    #[error("una factura en estado {0} no admite cobros")]
    NotCollectable(&'static str),
    #[error("la factura no tiene un importe positivo que cobrar")]
    NothingToCollect,
    #[error("el importe del cobro debe ser positivo y tener a lo sumo dos decimales")]
    InvalidAmount,
    #[error("fecha de cobro inválida")]
    InvalidDate,
    #[error("el cobro supera lo pendiente ({outstanding})")]
    ExceedsOutstanding { outstanding: Decimal },
    #[error("ya existe un cobro con el identificador {0}")]
    DuplicateId(u64),
    #[error("no existe el cobro {0}")]
    UnknownPayment(u64),
}

impl PaymentError {
    /// Código estable de los contratos: todos son `invalid_input`.
    pub fn code(&self) -> &'static str {
        "invalid_input"
    }
}

/// Fecha de vencimiento = expedición + `days` días naturales. `None` si la fecha no es válida
/// o se sale del calendario soportado.
pub fn due_date(issue_date: IsoDate, days: u32) -> Option<IsoDate> {
    dates::add_days(issue_date, i64::from(days))
}

fn ensure_collectable(receivable: &Receivable) -> Result<(), PaymentError> {
    if !receivable.kind.is_invoice() {
        return Err(PaymentError::NotAnInvoice(receivable.kind.as_str()));
    }
    if !matches!(receivable.status, Status::Issued | Status::Sent) {
        return Err(PaymentError::NotCollectable(receivable.status.as_str()));
    }
    if receivable.payable <= Decimal::ZERO {
        return Err(PaymentError::NothingToCollect);
    }
    Ok(())
}

fn check_payment(payment: &Payment) -> Result<(), PaymentError> {
    if payment.amount <= Decimal::ZERO || payment.amount.normalize().scale() > 2 {
        return Err(PaymentError::InvalidAmount);
    }
    if !dates::is_valid(payment.date) {
        return Err(PaymentError::InvalidDate);
    }
    Ok(())
}

/// Estado de cobro derivado en la fecha `today` (D88). Revalida los cobros guardados.
pub fn summarize(
    receivable: &Receivable,
    payments: &[Payment],
    today: IsoDate,
) -> Result<CollectionSummary, PaymentError> {
    ensure_collectable(receivable)?;
    let mut seen = HashSet::new();
    let mut paid = Decimal::ZERO;
    for p in payments {
        check_payment(p)?;
        if !seen.insert(p.id) {
            return Err(PaymentError::DuplicateId(p.id));
        }
        paid = paid
            .checked_add(p.amount)
            .ok_or(PaymentError::InvalidAmount)?;
    }
    if paid > receivable.payable {
        return Err(PaymentError::ExceedsOutstanding {
            outstanding: round_money(receivable.payable),
        });
    }
    let outstanding = receivable.payable - paid;
    let state = if outstanding.is_zero() {
        CollectionState::Paid
    } else if receivable.due_date.is_some_and(|due| today > due) {
        CollectionState::Overdue
    } else if paid.is_zero() {
        CollectionState::Pending
    } else {
        CollectionState::Partial
    };
    Ok(CollectionSummary {
        state,
        payable: round_money(receivable.payable),
        paid: round_money(paid),
        outstanding: round_money(outstanding),
    })
}

/// Registra un cobro. Si falla, `payments` queda intacto.
pub fn add_payment(
    receivable: &Receivable,
    payments: &mut Vec<Payment>,
    payment: Payment,
    today: IsoDate,
) -> Result<CollectionSummary, PaymentError> {
    let mut candidate = payments.clone();
    candidate.push(payment);
    let summary = match summarize(receivable, &candidate, today) {
        Err(PaymentError::ExceedsOutstanding { .. }) => {
            let paid: Decimal = payments.iter().map(|p| p.amount).sum();
            return Err(PaymentError::ExceedsOutstanding {
                outstanding: round_money(receivable.payable - paid),
            });
        }
        other => other?,
    };
    *payments = candidate;
    Ok(summary)
}

/// Elimina un cobro (D88) y lo devuelve para que quien llama deje la entrada en el historial.
pub fn remove_payment(
    receivable: &Receivable,
    payments: &mut Vec<Payment>,
    id: u64,
    today: IsoDate,
) -> Result<(Payment, CollectionSummary), PaymentError> {
    ensure_collectable(receivable)?;
    let position = payments
        .iter()
        .position(|p| p.id == id)
        .ok_or(PaymentError::UnknownPayment(id))?;
    let mut remaining = payments.clone();
    let removed = remaining.remove(position);
    let summary = summarize(receivable, &remaining, today)?;
    *payments = remaining;
    Ok((removed, summary))
}

#[cfg(test)]
mod tests {
    use super::*;
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

    fn invoice(payable: &str, due: Option<IsoDate>) -> Receivable {
        Receivable {
            kind: DocumentKind::Invoice,
            status: Status::Issued,
            payable: dec(payable),
            due_date: due,
        }
    }

    fn pay(id: u64, amount: &str) -> Payment {
        Payment {
            id,
            date: date(2026, 3, 10),
            amount: dec(amount),
            method: PaymentMethod::Transfer,
        }
    }

    const TODAY: IsoDate = IsoDate {
        year: 2026,
        month: 3,
        day: 15,
    };

    #[test]
    fn d88_no_payments_is_pending() {
        let s = summarize(&invoice("121.00", Some(date(2026, 4, 1))), &[], TODAY).unwrap();
        assert_eq!(s.state, CollectionState::Pending);
        assert_eq!(s.paid.to_string(), "0.00");
        assert_eq!(s.outstanding.to_string(), "121.00");
    }

    #[test]
    fn d88_partial_then_paid() {
        let r = invoice("121.00", Some(date(2026, 4, 1)));
        let mut ps = Vec::new();
        let s = add_payment(&r, &mut ps, pay(1, "50.00"), TODAY).unwrap();
        assert_eq!(s.state, CollectionState::Partial);
        assert_eq!(s.outstanding.to_string(), "71.00");
        let s = add_payment(&r, &mut ps, pay(2, "71"), TODAY).unwrap();
        assert_eq!(s.state, CollectionState::Paid);
        assert_eq!(s.outstanding.to_string(), "0.00");
        assert_eq!(ps.len(), 2);
    }

    #[test]
    fn d88_overdue_only_strictly_after_due_date() {
        let r = invoice("100.00", Some(date(2026, 3, 15)));
        assert_eq!(
            summarize(&r, &[], TODAY).unwrap().state,
            CollectionState::Pending
        );
        let late = date(2026, 3, 16);
        assert_eq!(
            summarize(&r, &[], late).unwrap().state,
            CollectionState::Overdue
        );
    }

    #[test]
    fn d88_overdue_prevails_over_partial_and_keeps_amounts() {
        let r = invoice("100.00", Some(date(2026, 3, 1)));
        let s = summarize(&r, &[pay(1, "40.00")], TODAY).unwrap();
        assert_eq!(s.state, CollectionState::Overdue);
        assert_eq!(s.paid.to_string(), "40.00");
        assert_eq!(s.outstanding.to_string(), "60.00");
    }

    #[test]
    fn d88_paid_invoice_is_never_overdue() {
        let r = invoice("100.00", Some(date(2026, 1, 1)));
        let s = summarize(&r, &[pay(1, "100.00")], TODAY).unwrap();
        assert_eq!(s.state, CollectionState::Paid);
    }

    #[test]
    fn d88_without_due_date_never_overdue() {
        let r = invoice("100.00", None);
        let s = summarize(&r, &[], date(2099, 1, 1)).unwrap();
        assert_eq!(s.state, CollectionState::Pending);
    }

    #[test]
    fn d88_payable_is_amount_due_after_withholding() {
        // Total 121,00 con retención 15,00: a cobrar 106,00 (D75).
        let r = invoice("106.00", None);
        let s = summarize(&r, &[pay(1, "106.00")], TODAY).unwrap();
        assert_eq!(s.state, CollectionState::Paid);
    }

    #[test]
    fn d88_overpayment_rejected_and_payments_untouched() {
        let r = invoice("100.00", None);
        let mut ps = vec![pay(1, "60.00")];
        let err = add_payment(&r, &mut ps, pay(2, "40.01"), TODAY).unwrap_err();
        assert_eq!(
            err,
            PaymentError::ExceedsOutstanding {
                outstanding: dec("40.00")
            }
        );
        assert_eq!(ps.len(), 1);
    }

    #[test]
    fn d88_invalid_amounts_rejected() {
        let r = invoice("100.00", None);
        for bad in ["0", "-5.00", "10.001"] {
            let mut ps = Vec::new();
            assert_eq!(
                add_payment(&r, &mut ps, pay(1, bad), TODAY).unwrap_err(),
                PaymentError::InvalidAmount,
                "{bad}"
            );
            assert!(ps.is_empty());
        }
        // Escala 3 con ceros finales es un importe válido.
        let mut ps = Vec::new();
        add_payment(&r, &mut ps, pay(1, "10.000"), TODAY).unwrap();
    }

    #[test]
    fn d88_invalid_date_and_duplicate_id_rejected() {
        let r = invoice("100.00", None);
        let mut ps = Vec::new();
        let mut bad = pay(1, "10");
        bad.date = date(2026, 2, 30);
        assert_eq!(
            add_payment(&r, &mut ps, bad, TODAY).unwrap_err(),
            PaymentError::InvalidDate
        );
        add_payment(&r, &mut ps, pay(1, "10"), TODAY).unwrap();
        assert_eq!(
            add_payment(&r, &mut ps, pay(1, "10"), TODAY).unwrap_err(),
            PaymentError::DuplicateId(1)
        );
    }

    #[test]
    fn d88_draft_voided_and_other_statuses_not_collectable() {
        for status in [Status::Draft, Status::Voided, Status::Accepted] {
            let r = Receivable {
                status,
                ..invoice("100.00", None)
            };
            let err = summarize(&r, &[], TODAY).unwrap_err();
            assert_eq!(err, PaymentError::NotCollectable(status.as_str()));
        }
        for status in [Status::Issued, Status::Sent] {
            let r = Receivable {
                status,
                ..invoice("100.00", None)
            };
            assert!(summarize(&r, &[], TODAY).is_ok());
        }
    }

    #[test]
    fn d88_only_invoices_have_payments() {
        for kind in [DocumentKind::Quote, DocumentKind::DeliveryNote] {
            let r = Receivable {
                kind,
                ..invoice("100.00", None)
            };
            assert!(matches!(
                summarize(&r, &[], TODAY),
                Err(PaymentError::NotAnInvoice(_))
            ));
        }
        for kind in [
            DocumentKind::Invoice,
            DocumentKind::SimplifiedInvoice,
            DocumentKind::CorrectiveInvoice,
        ] {
            let r = Receivable {
                kind,
                ..invoice("100.00", None)
            };
            assert!(summarize(&r, &[], TODAY).is_ok());
        }
    }

    #[test]
    fn d88_zero_or_negative_payable_has_no_collection_state() {
        for p in ["0.00", "-20.00"] {
            let r = Receivable {
                kind: DocumentKind::CorrectiveInvoice,
                ..invoice(p, None)
            };
            assert_eq!(
                summarize(&r, &[], TODAY).unwrap_err(),
                PaymentError::NothingToCollect
            );
        }
    }

    #[test]
    fn d88_remove_payment_returns_it_and_rederives_state() {
        let r = invoice("100.00", None);
        let mut ps = Vec::new();
        add_payment(&r, &mut ps, pay(1, "100.00"), TODAY).unwrap();
        let (removed, s) = remove_payment(&r, &mut ps, 1, TODAY).unwrap();
        assert_eq!(removed.id, 1);
        assert_eq!(s.state, CollectionState::Pending);
        assert!(ps.is_empty());
        assert_eq!(
            remove_payment(&r, &mut ps, 1, TODAY).unwrap_err(),
            PaymentError::UnknownPayment(1)
        );
    }

    #[test]
    fn d88_remove_on_voided_invoice_rejected() {
        let r = Receivable {
            status: Status::Voided,
            ..invoice("100.00", None)
        };
        let mut ps = vec![pay(1, "10")];
        assert!(matches!(
            remove_payment(&r, &mut ps, 1, TODAY),
            Err(PaymentError::NotCollectable(_))
        ));
        assert_eq!(ps.len(), 1);
    }

    #[test]
    fn d88_stored_payments_exceeding_total_are_reported() {
        let r = invoice("100.00", None);
        assert!(matches!(
            summarize(&r, &[pay(1, "70"), pay(2, "40")], TODAY),
            Err(PaymentError::ExceedsOutstanding { .. })
        ));
    }

    #[test]
    fn d88_methods_round_trip() {
        for m in PaymentMethod::ALL {
            assert_eq!(PaymentMethod::parse(m.as_str()), Some(m));
        }
        assert_eq!(PaymentMethod::parse("bizum"), None);
    }

    #[test]
    fn r19_due_date_adds_days() {
        assert_eq!(due_date(date(2026, 1, 20), 30), Some(date(2026, 2, 19)));
        assert_eq!(due_date(date(2026, 12, 15), 30), Some(date(2027, 1, 14)));
        assert_eq!(due_date(date(2026, 1, 20), 0), Some(date(2026, 1, 20)));
    }

    #[test]
    fn d88_state_is_derived_not_stored() {
        // El mismo conjunto de cobros da estados distintos según la fecha: nada se guarda.
        let r = invoice("100.00", Some(date(2026, 3, 20)));
        let ps = [pay(1, "10")];
        assert_eq!(
            summarize(&r, &ps, TODAY).unwrap().state,
            CollectionState::Partial
        );
        assert_eq!(
            summarize(&r, &ps, date(2026, 3, 21)).unwrap().state,
            CollectionState::Overdue
        );
    }
}
