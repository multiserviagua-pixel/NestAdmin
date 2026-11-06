//! Cobros y estado de cobro derivado (R19, D88).
//!
//! D88: cada cobro tiene fecha, importe y medio (`transfer`, `cash`, `card`, `other`). El
//! estado de cobro **se deriva**, nunca se almacena: sin cobros `pending`; con cobros por
//! debajo del total `partial`; igual al total `paid`; y `overdue` si pasa el vencimiento sin
//! estar cobrada. Un cobro se puede eliminar con entrada en el historial (la entrada la
//! escribe quien llama con el [`Payment`] devuelto); no afecta a Verifactu (D88).
//!
//! Interpretaciones conservadoras (la spec no las fija):
//! * **Importe de referencia** = importe a pagar de D75 (`Totals::amount_due`, es decir, total
//!   menos retención de IRPF) **ajustado por las rectificativas vigentes**. El llamador pasa
//!   el importe propio en [`Receivable::payable`] y las rectificativas vinculadas, de forma
//!   explícita, en [`Receivable::correctives`] (no hay estado oculto):
//!   * **por diferencias**: se suma su `amount_due` (positivo o negativo) al de la original;
//!     la propia rectificativa por diferencias no tiene cobro propio
//!     ([`CollectionState::AppliedToOriginal`]) y sus cobros se registran en la original;
//!   * **por sustitución**: el importe a cobrar es el de la sustituta, que se cobra como una
//!     factura más. La original pasa a [`CollectionState::Substituted`]: no admite cobros
//!     nuevos y los que tenga **se imputan a la sustituta**, para lo que quien llama pasa
//!     junto a los de la sustituta los cobros de la original (los identificadores de cobro
//!     son únicos entre ambas, como en cualquier base de datos);
//!   * solo cuentan las rectificativas `issued` o `sent` (un borrador no está en vigor y una
//!     anulada se ignora); no se mezclan diferencias y sustitución ni hay dos sustituciones
//!     de la misma factura, y esos casos son un error explícito. No hay rectificativa de una
//!     rectificativa (D81).
//! * **Sin devoluciones**: D88 no define cobros negativos. Un cobro debe ser positivo con a lo
//!   sumo dos decimales y no puede superar lo pendiente. Si una rectificativa deja el
//!   importe de referencia por debajo de lo ya cobrado, el pendiente es negativo y el estado
//!   es [`CollectionState::ToRefund`] (a devolver): solo se informa, no se implementan
//!   devoluciones ni se admiten más cobros. Pendiente cero es `paid`.
//! * **Solo facturas** (completa, simplificada o rectificativa). Un borrador no tiene número ni
//!   obligación de cobro (`NotCollectable`). Una **anulada** (D82) no se cobra, pero su estado
//!   es explícito: [`CollectionState::Voided`] o, si tiene cobros, [`CollectionState::
//!   VoidedWithPayments`] (cobros por regularizar); esos cobros se pueden retirar con
//!   [`remove_payment`].
//! * Una factura sin rectificativas cuyo importe a cobrar es cero o negativo no tiene estado de
//!   cobro: [`PaymentError::NothingToCollect`].
//! * **Vencida** = hoy es estrictamente posterior al vencimiento y queda algo por cobrar; la
//!   vencida prevalece sobre la parcial, y el resumen conserva lo cobrado y lo pendiente.
//!   Sin vencimiento nunca vence (R19 lo exige para facturas, pero el dato puede faltar).
//! * El vencimiento por defecto se calcula con [`due_date`] (fecha de expedición + días
//!   indicados por el usuario); la spec no fija un plazo estándar.

use std::collections::HashSet;

use nestadmin_documents::IsoDate;
use rust_decimal::Decimal;

use crate::calc::round_money;
use crate::content::RectificationMethod;
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
    /// Lo cobrado supera el importe de referencia tras una rectificativa: hay que devolver la
    /// diferencia (la devolución no se implementa).
    ToRefund,
    /// Factura original sustituida por una rectificativa: cobra la sustituta.
    Substituted,
    /// Rectificativa por diferencias: su importe se suma al de la original.
    AppliedToOriginal,
    /// Factura anulada sin cobros.
    Voided,
    /// Factura anulada con cobros por regularizar.
    VoidedWithPayments,
}

impl CollectionState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Partial => "partial",
            Self::Paid => "paid",
            Self::Overdue => "overdue",
            Self::ToRefund => "to_refund",
            Self::Substituted => "substituted",
            Self::AppliedToOriginal => "applied_to_original",
            Self::Voided => "voided",
            Self::VoidedWithPayments => "voided_with_payments",
        }
    }
}

/// Rectificativa vinculada a una factura, con lo que importa para su cobro.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkedCorrective {
    pub method: RectificationMethod,
    pub status: Status,
    /// `Totals::amount_due` de la rectificativa; negativo si reduce lo debido.
    pub amount_due: Decimal,
}

impl LinkedCorrective {
    fn is_in_force(&self) -> bool {
        matches!(self.status, Status::Issued | Status::Sent)
    }
}

/// Lo mínimo de una factura que necesita el cobro.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Receivable {
    pub kind: DocumentKind,
    pub status: Status,
    /// Importe propio a cobrar (ver nota del módulo): `Totals::amount_due`.
    pub payable: Decimal,
    pub due_date: Option<IsoDate>,
    /// Si esta factura es una rectificativa, el método con que rectifica.
    pub rectification: Option<RectificationMethod>,
    /// Rectificativas de esta factura (solo en la original), de cualquier estado.
    pub correctives: Vec<LinkedCorrective>,
}

impl Receivable {
    /// Factura sin rectificar ni rectificativa.
    pub fn new(
        kind: DocumentKind,
        status: Status,
        payable: Decimal,
        due_date: Option<IsoDate>,
    ) -> Self {
        Self {
            kind,
            status,
            payable,
            due_date,
            rectification: None,
            correctives: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollectionSummary {
    pub state: CollectionState,
    /// Importe de referencia (propio ajustado por rectificativas); cero si no se cobra aquí.
    pub payable: Decimal,
    pub paid: Decimal,
    /// Pendiente; negativo solo en `ToRefund`.
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
    #[error(
        "la factura está sustituida por una rectificativa: el cobro se registra en la sustituta"
    )]
    Substituted,
    #[error("una rectificativa por diferencias no se cobra por separado: el cobro se registra en la factura original")]
    AppliedToOriginal,
    #[error(
        "no se mezclan rectificativas por diferencias y por sustitución sobre la misma factura"
    )]
    MixedCorrectives,
    #[error("la factura tiene más de una rectificativa por sustitución en vigor")]
    MultipleSubstitutions,
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

/// Qué se hace con los cobros de una factura según su estado y sus rectificativas.
enum Plan {
    Voided,
    Substituted,
    AppliedToOriginal,
    Collect {
        reference: Decimal,
        /// Hay rectificativa en juego: un pendiente negativo es «a devolver» y no un error.
        rectified: bool,
    },
}

fn plan(receivable: &Receivable) -> Result<Plan, PaymentError> {
    if !receivable.kind.is_invoice() {
        return Err(PaymentError::NotAnInvoice(receivable.kind.as_str()));
    }
    match receivable.status {
        Status::Voided => return Ok(Plan::Voided),
        Status::Issued | Status::Sent => {}
        other => return Err(PaymentError::NotCollectable(other.as_str())),
    }
    if receivable.rectification == Some(RectificationMethod::Differences) {
        return Ok(Plan::AppliedToOriginal);
    }
    let in_force: Vec<&LinkedCorrective> = receivable
        .correctives
        .iter()
        .filter(|c| c.is_in_force())
        .collect();
    let substitutions = in_force
        .iter()
        .filter(|c| c.method == RectificationMethod::Substitution)
        .count();
    let differences = in_force.len() - substitutions;
    if substitutions > 1 {
        return Err(PaymentError::MultipleSubstitutions);
    }
    if substitutions == 1 && differences > 0 {
        return Err(PaymentError::MixedCorrectives);
    }
    if substitutions == 1 {
        return Ok(Plan::Substituted);
    }
    let mut reference = receivable.payable;
    for c in &in_force {
        reference = reference
            .checked_add(c.amount_due)
            .ok_or(PaymentError::InvalidAmount)?;
    }
    let rectified =
        differences > 0 || receivable.rectification == Some(RectificationMethod::Substitution);
    if reference <= Decimal::ZERO && !rectified {
        return Err(PaymentError::NothingToCollect);
    }
    Ok(Plan::Collect {
        reference,
        rectified,
    })
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

/// Valida los cobros guardados y devuelve su suma.
fn total_paid(payments: &[Payment]) -> Result<Decimal, PaymentError> {
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
    Ok(paid)
}

/// Estado de cobro derivado en la fecha `today` (D88). Revalida los cobros guardados.
pub fn summarize(
    receivable: &Receivable,
    payments: &[Payment],
    today: IsoDate,
) -> Result<CollectionSummary, PaymentError> {
    let plan = plan(receivable)?;
    let paid = total_paid(payments)?;
    let (reference, rectified) = match plan {
        Plan::Voided => {
            let state = if payments.is_empty() {
                CollectionState::Voided
            } else {
                CollectionState::VoidedWithPayments
            };
            return Ok(no_collection(state, paid));
        }
        Plan::Substituted => return Ok(no_collection(CollectionState::Substituted, paid)),
        Plan::AppliedToOriginal => {
            if !payments.is_empty() {
                return Err(PaymentError::AppliedToOriginal);
            }
            return Ok(no_collection(CollectionState::AppliedToOriginal, paid));
        }
        Plan::Collect {
            reference,
            rectified,
        } => (reference, rectified),
    };
    let outstanding = reference - paid;
    if outstanding < Decimal::ZERO && !rectified {
        return Err(PaymentError::ExceedsOutstanding {
            outstanding: round_money(reference),
        });
    }
    let state = if outstanding < Decimal::ZERO {
        CollectionState::ToRefund
    } else if outstanding.is_zero() {
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
        payable: round_money(reference),
        paid: round_money(paid),
        outstanding: round_money(outstanding),
    })
}

fn no_collection(state: CollectionState, paid: Decimal) -> CollectionSummary {
    CollectionSummary {
        state,
        payable: round_money(Decimal::ZERO),
        paid: round_money(paid),
        outstanding: round_money(Decimal::ZERO),
    }
}

/// Registra un cobro. Si falla, `payments` queda intacto.
pub fn add_payment(
    receivable: &Receivable,
    payments: &mut Vec<Payment>,
    payment: Payment,
    today: IsoDate,
) -> Result<CollectionSummary, PaymentError> {
    let reference = match plan(receivable)? {
        Plan::Voided => return Err(PaymentError::NotCollectable(Status::Voided.as_str())),
        Plan::Substituted => return Err(PaymentError::Substituted),
        Plan::AppliedToOriginal => return Err(PaymentError::AppliedToOriginal),
        Plan::Collect { reference, .. } => reference,
    };
    let mut candidate = payments.clone();
    candidate.push(payment);
    let paid_before: Decimal = payments.iter().map(|p| p.amount).sum();
    let paid_after = total_paid(&candidate)?;
    if paid_after > reference {
        // Nunca se admite un cobro que deje el pendiente por debajo de cero, aunque una
        // rectificativa pueda dejar ya un «a devolver» en cobros anteriores.
        return Err(PaymentError::ExceedsOutstanding {
            outstanding: round_money((reference - paid_before).max(Decimal::ZERO)),
        });
    }
    let summary = summarize(receivable, &candidate, today)?;
    *payments = candidate;
    Ok(summary)
}

/// Elimina un cobro (D88) y lo devuelve para que quien llama deje la entrada en el historial.
/// Se puede retirar también de una factura anulada o sustituida (cobros por regularizar).
pub fn remove_payment(
    receivable: &Receivable,
    payments: &mut Vec<Payment>,
    id: u64,
    today: IsoDate,
) -> Result<(Payment, CollectionSummary), PaymentError> {
    plan(receivable)?;
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
        Receivable::new(DocumentKind::Invoice, Status::Issued, dec(payable), due)
    }

    fn corrective(method: RectificationMethod, amount_due: &str) -> LinkedCorrective {
        LinkedCorrective {
            method,
            status: Status::Issued,
            amount_due: dec(amount_due),
        }
    }

    fn with(correctives: Vec<LinkedCorrective>, payable: &str) -> Receivable {
        Receivable {
            correctives,
            ..invoice(payable, None)
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
    fn d88_draft_and_other_statuses_not_collectable() {
        for status in [Status::Draft, Status::Accepted] {
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
    fn d88_voided_invoice_has_explicit_state_not_an_error() {
        let r = Receivable {
            status: Status::Voided,
            ..invoice("100.00", None)
        };
        let s = summarize(&r, &[], TODAY).unwrap();
        assert_eq!(s.state, CollectionState::Voided);
        assert_eq!(s.state.as_str(), "voided");
        // Con cobros: estado explícito de cobros por regularizar.
        let s = summarize(&r, &[pay(1, "10"), pay(2, "5.50")], TODAY).unwrap();
        assert_eq!(s.state, CollectionState::VoidedWithPayments);
        assert_eq!(s.paid.to_string(), "15.50");
        assert_eq!(s.outstanding.to_string(), "0.00");
        // No se admiten cobros nuevos en una anulada.
        let mut ps = Vec::new();
        assert!(matches!(
            add_payment(&r, &mut ps, pay(1, "10"), TODAY),
            Err(PaymentError::NotCollectable("voided"))
        ));
        assert!(ps.is_empty());
    }

    #[test]
    fn d88_payment_can_be_removed_from_a_voided_invoice() {
        let r = Receivable {
            status: Status::Voided,
            ..invoice("100.00", None)
        };
        let mut ps = vec![pay(1, "10"), pay(2, "20")];
        let (removed, s) = remove_payment(&r, &mut ps, 1, TODAY).unwrap();
        assert_eq!(removed.id, 1);
        assert_eq!(s.state, CollectionState::VoidedWithPayments);
        let (_, s) = remove_payment(&r, &mut ps, 2, TODAY).unwrap();
        assert_eq!(s.state, CollectionState::Voided);
        assert!(ps.is_empty());
        assert_eq!(
            remove_payment(&r, &mut ps, 2, TODAY).unwrap_err(),
            PaymentError::UnknownPayment(2)
        );
    }

    #[test]
    fn d88_differences_corrective_adjusts_the_original_reference_amount() {
        // Original 121,00 y rectificativa por diferencias de -21,00: se cobran 100,00.
        let r = with(
            vec![corrective(RectificationMethod::Differences, "-21.00")],
            "121.00",
        );
        let s = summarize(&r, &[], TODAY).unwrap();
        assert_eq!(
            (s.state, s.payable.to_string()),
            (CollectionState::Pending, "100.00".into())
        );
        // Una rectificativa al alza suma.
        let r = with(
            vec![corrective(RectificationMethod::Differences, "10.00")],
            "121.00",
        );
        assert_eq!(
            summarize(&r, &[], TODAY).unwrap().payable.to_string(),
            "131.00"
        );
        // Varias diferencias se acumulan; un borrador o una anulada no cuentan.
        let mut draft = corrective(RectificationMethod::Differences, "-1000.00");
        draft.status = Status::Draft;
        let mut voided = draft.clone();
        voided.status = Status::Voided;
        let r = with(
            vec![
                corrective(RectificationMethod::Differences, "-21.00"),
                corrective(RectificationMethod::Differences, "-5.00"),
                draft,
                voided,
            ],
            "121.00",
        );
        assert_eq!(
            summarize(&r, &[], TODAY).unwrap().payable.to_string(),
            "95.00"
        );
    }

    #[test]
    fn d88_payments_follow_the_adjusted_amount() {
        let r = with(
            vec![corrective(RectificationMethod::Differences, "-21.00")],
            "121.00",
        );
        let mut ps = Vec::new();
        // 121 ya no es admisible: lo pendiente es 100,00.
        assert_eq!(
            add_payment(&r, &mut ps, pay(1, "121.00"), TODAY).unwrap_err(),
            PaymentError::ExceedsOutstanding {
                outstanding: dec("100.00")
            }
        );
        let s = add_payment(&r, &mut ps, pay(1, "100.00"), TODAY).unwrap();
        assert_eq!(s.state, CollectionState::Paid);
    }

    #[test]
    fn d88_corrective_below_what_was_paid_is_to_refund() {
        // Se cobraron 121,00 y luego se rectificó -21,00: quedan 21,00 a devolver.
        let r = with(
            vec![corrective(RectificationMethod::Differences, "-21.00")],
            "121.00",
        );
        let ps = [pay(1, "121.00")];
        let s = summarize(&r, &ps, TODAY).unwrap();
        assert_eq!(s.state, CollectionState::ToRefund);
        assert_eq!(s.outstanding.to_string(), "-21.00");
        assert_eq!(s.state.as_str(), "to_refund");
        // No se admite otro cobro sobre un «a devolver».
        let mut stored = ps.to_vec();
        assert_eq!(
            add_payment(&r, &mut stored, pay(2, "1.00"), TODAY).unwrap_err(),
            PaymentError::ExceedsOutstanding {
                outstanding: dec("0.00")
            }
        );
        // Sin rectificativa, el mismo exceso sigue siendo un error de datos.
        assert!(matches!(
            summarize(&invoice("100.00", None), &[pay(1, "121.00")], TODAY),
            Err(PaymentError::ExceedsOutstanding { .. })
        ));
    }

    #[test]
    fn d88_fully_credited_invoice_is_paid_or_to_refund_never_an_error() {
        let r = with(
            vec![corrective(RectificationMethod::Differences, "-121.00")],
            "121.00",
        );
        assert_eq!(
            summarize(&r, &[], TODAY).unwrap().state,
            CollectionState::Paid
        );
        assert_eq!(
            summarize(&r, &[pay(1, "50.00")], TODAY).unwrap().state,
            CollectionState::ToRefund
        );
        // Una rectificativa que excede el original deja un importe negativo: a devolver.
        let r = with(
            vec![corrective(RectificationMethod::Differences, "-150.00")],
            "121.00",
        );
        let s = summarize(&r, &[], TODAY).unwrap();
        assert_eq!(s.state, CollectionState::ToRefund);
        assert_eq!(s.outstanding.to_string(), "-29.00");
    }

    #[test]
    fn d88_differences_corrective_has_no_collection_of_its_own() {
        let c = Receivable {
            kind: DocumentKind::CorrectiveInvoice,
            rectification: Some(RectificationMethod::Differences),
            ..invoice("-21.00", None)
        };
        let s = summarize(&c, &[], TODAY).unwrap();
        assert_eq!(s.state, CollectionState::AppliedToOriginal);
        let mut ps = Vec::new();
        assert_eq!(
            add_payment(&c, &mut ps, pay(1, "5"), TODAY).unwrap_err(),
            PaymentError::AppliedToOriginal
        );
        // También con importe positivo: se cobra en la original.
        let c = Receivable {
            payable: dec("30.00"),
            ..c
        };
        assert_eq!(
            summarize(&c, &[], TODAY).unwrap().state,
            CollectionState::AppliedToOriginal
        );
    }

    #[test]
    fn d88_substitution_moves_collection_to_the_substitute() {
        let original = with(
            vec![corrective(RectificationMethod::Substitution, "150.00")],
            "121.00",
        );
        // La original no cobra: sus cobros se imputan a la sustituta.
        let s = summarize(&original, &[pay(1, "121.00")], TODAY).unwrap();
        assert_eq!(s.state, CollectionState::Substituted);
        assert_eq!(s.paid.to_string(), "121.00");
        assert_eq!(s.outstanding.to_string(), "0.00");
        let mut ps = Vec::new();
        assert_eq!(
            add_payment(&original, &mut ps, pay(2, "5"), TODAY).unwrap_err(),
            PaymentError::Substituted
        );
        // La sustituta cobra su importe completo; recibe los cobros de la original.
        let substitute = Receivable {
            kind: DocumentKind::CorrectiveInvoice,
            rectification: Some(RectificationMethod::Substitution),
            ..invoice("150.00", None)
        };
        let s = summarize(&substitute, &[pay(1, "121.00")], TODAY).unwrap();
        assert_eq!(s.state, CollectionState::Partial);
        assert_eq!(s.outstanding.to_string(), "29.00");
        let mut ps = vec![pay(1, "121.00")];
        let s = add_payment(&substitute, &mut ps, pay(3, "29.00"), TODAY).unwrap();
        assert_eq!(s.state, CollectionState::Paid);
        // Si la sustituta baja el importe por debajo de lo cobrado: a devolver.
        let lower = Receivable {
            payable: dec("100.00"),
            ..substitute.clone()
        };
        let s = summarize(&lower, &[pay(1, "121.00")], TODAY).unwrap();
        assert_eq!(s.state, CollectionState::ToRefund);
        assert_eq!(s.outstanding.to_string(), "-21.00");
        // Una sustituta a cero no es un error de «nada que cobrar».
        let zero = Receivable {
            payable: dec("0.00"),
            ..substitute
        };
        assert_eq!(
            summarize(&zero, &[], TODAY).unwrap().state,
            CollectionState::Paid
        );
    }

    #[test]
    fn d88_substitution_voided_or_draft_does_not_substitute() {
        let mut c = corrective(RectificationMethod::Substitution, "150.00");
        c.status = Status::Voided;
        let r = with(vec![c], "121.00");
        let s = summarize(&r, &[], TODAY).unwrap();
        assert_eq!(
            (s.state, s.payable.to_string()),
            (CollectionState::Pending, "121.00".into())
        );
    }

    #[test]
    fn d88_inconsistent_correctives_are_explicit_errors() {
        let both = with(
            vec![
                corrective(RectificationMethod::Differences, "-1.00"),
                corrective(RectificationMethod::Substitution, "100.00"),
            ],
            "121.00",
        );
        assert_eq!(
            summarize(&both, &[], TODAY).unwrap_err(),
            PaymentError::MixedCorrectives
        );
        let two = with(
            vec![
                corrective(RectificationMethod::Substitution, "100.00"),
                corrective(RectificationMethod::Substitution, "90.00"),
            ],
            "121.00",
        );
        assert_eq!(
            summarize(&two, &[], TODAY).unwrap_err(),
            PaymentError::MultipleSubstitutions
        );
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
