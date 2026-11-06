//! Estados, transiciones, revisiones, conversión y anulación (D74, D81, D82).
//!
//! Tablas de D74:
//! * Presupuesto: `draft → issued → sent → accepted | rejected | expired`;
//!   `issued → accepted | rejected | expired` (entregado por otra vía); `accepted → converted`.
//! * Albarán emitido: `draft → issued → sent → delivered`; `issued → delivered`;
//!   `→ invoiced` al convertirse en factura.
//! * Factura (completa, simplificada y rectificativa): `draft → issued → sent`; `voided` solo
//!   por D82. Nunca hay revisiones de una factura (D81).
//! * Albarán recibido: `received → reviewed`.
//!
//! Interpretaciones conservadoras (la spec no las fija):
//! * `invoiced` solo se alcanza desde `delivered`, y convertir un albarán exige `delivered`.
//! * Un presupuesto o albarán solo se revisa en `issued` o `sent`; la revisión `n+1` nace en
//!   `issued` (contenido fijo, pendiente de nuevo envío) y la anterior queda sustituida.
//! * Se anula una factura en `issued` o `sent`: D82 cita «emitida por error y no entregada»
//!   como ejemplo, no como límite. Una anulada es final.
//! * Solo se rectifica una factura completa o simplificada en `issued` o `sent`, no una
//!   rectificativa ni una anulada, ni un presupuesto o albarán (`invalid_transition`).
//! * Un presupuesto se convierte en albarán o factura de uno en uno; varios albaranes del
//!   mismo cliente se agrupan en una factura (R16). «Mismo cliente» exige NIF presente e
//!   igual en todos: sin NIF no se puede probar y se rechaza.
//! * Convertir a factura crea una factura completa (`Invoice`); una simplificada se crea a mano.

use serde::{Deserialize, Serialize};

use crate::series::DocumentKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Draft,
    Issued,
    Sent,
    Accepted,
    Rejected,
    Expired,
    Converted,
    Delivered,
    Invoiced,
    Voided,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Issued => "issued",
            Self::Sent => "sent",
            Self::Accepted => "accepted",
            Self::Rejected => "rejected",
            Self::Expired => "expired",
            Self::Converted => "converted",
            Self::Delivered => "delivered",
            Self::Invoiced => "invoiced",
            Self::Voided => "voided",
        }
    }

    /// Estados que existen para ese tipo de documento.
    pub fn is_valid_for(self, kind: DocumentKind) -> bool {
        use Status::*;
        match kind {
            DocumentKind::Quote => matches!(
                self,
                Draft | Issued | Sent | Accepted | Rejected | Expired | Converted
            ),
            DocumentKind::DeliveryNote => {
                matches!(self, Draft | Issued | Sent | Delivered | Invoiced)
            }
            _ => matches!(self, Draft | Issued | Sent | Voided),
        }
    }
}

/// Estado del albarán recibido de un proveedor (D74).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReceivedStatus {
    Received,
    Reviewed,
}

impl ReceivedStatus {
    pub fn transition(self, to: ReceivedStatus) -> Result<(), StatusError> {
        match (self, to) {
            (Self::Received, Self::Reviewed) => Ok(()),
            (from, to) => Err(StatusError::InvalidReceivedTransition { from, to }),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StatusError {
    #[error("el estado {status:?} no existe para {kind:?}")]
    NotApplicable { kind: DocumentKind, status: Status },
    #[error("transición no permitida para {kind:?}: {from:?} a {to:?}")]
    InvalidTransition {
        kind: DocumentKind,
        from: Status,
        to: Status,
    },
    #[error("transición no permitida de albarán recibido: {from:?} a {to:?}")]
    InvalidReceivedTransition {
        from: ReceivedStatus,
        to: ReceivedStatus,
    },
    #[error("un documento en estado {status:?} no se puede editar")]
    Immutable { status: Status },
    #[error("{kind:?} en estado {status:?} no admite una revisión nueva")]
    NotRevisable { kind: DocumentKind, status: Status },
    #[error("{kind:?} en estado {status:?} no se puede rectificar")]
    NotCorrectable { kind: DocumentKind, status: Status },
    #[error("{kind:?} en estado {status:?} no se puede anular")]
    NotVoidable { kind: DocumentKind, status: Status },
    #[error("conversión no permitida: {0}")]
    InvalidConversion(&'static str),
}

impl StatusError {
    /// Código del contrato (`contratos.md`, Facturación > Errores).
    pub fn code(&self) -> &'static str {
        "invalid_transition"
    }
}

/// Estados a los que se puede pasar desde `from` con una transición directa del usuario.
/// `Converted`, `Invoiced` y `Voided` se alcanzan por sus operaciones propias, pero siguen
/// siendo transiciones válidas aquí para que el estado se fije por un único camino.
fn allowed(kind: DocumentKind, from: Status) -> &'static [Status] {
    use Status::*;
    match kind {
        DocumentKind::Quote => match from {
            Draft => &[Issued],
            Issued => &[Sent, Accepted, Rejected, Expired],
            Sent => &[Accepted, Rejected, Expired],
            Accepted => &[Converted],
            _ => &[],
        },
        DocumentKind::DeliveryNote => match from {
            Draft => &[Issued],
            Issued => &[Sent, Delivered],
            Sent => &[Delivered],
            Delivered => &[Invoiced],
            _ => &[],
        },
        _ => match from {
            Draft => &[Issued],
            Issued => &[Sent, Voided],
            Sent => &[Voided],
            _ => &[],
        },
    }
}

/// Valida una transición de estado. No hace nada más: el historial lo escribe quien persiste.
pub fn transition(kind: DocumentKind, from: Status, to: Status) -> Result<(), StatusError> {
    for status in [from, to] {
        if !status.is_valid_for(kind) {
            return Err(StatusError::NotApplicable { kind, status });
        }
    }
    if allowed(kind, from).contains(&to) {
        Ok(())
    } else {
        Err(StatusError::InvalidTransition { kind, from, to })
    }
}

/// Solo un borrador se edita; todo lo emitido se corrige con revisión o rectificativa.
pub fn ensure_editable(status: Status) -> Result<(), StatusError> {
    if status == Status::Draft {
        Ok(())
    } else {
        Err(StatusError::Immutable { status })
    }
}

/// `revise` (D74): solo presupuestos y albaranes emitidos, nunca facturas (D81).
pub fn can_revise(kind: DocumentKind, status: Status) -> Result<(), StatusError> {
    if matches!(kind, DocumentKind::Quote | DocumentKind::DeliveryNote)
        && matches!(status, Status::Issued | Status::Sent)
    {
        Ok(())
    } else {
        Err(StatusError::NotRevisable { kind, status })
    }
}

/// Número de la revisión siguiente (la primera es la 1) si el documento admite revisión.
pub fn next_revision(kind: DocumentKind, status: Status, current: u32) -> Result<u32, StatusError> {
    can_revise(kind, status)?;
    current
        .checked_add(1)
        .ok_or(StatusError::NotRevisable { kind, status })
}

/// `correct` (D81): rectificativa de una factura completa o simplificada ya emitida.
pub fn can_correct(kind: DocumentKind, status: Status) -> Result<(), StatusError> {
    if matches!(
        kind,
        DocumentKind::Invoice | DocumentKind::SimplifiedInvoice
    ) && matches!(status, Status::Issued | Status::Sent)
    {
        Ok(())
    } else {
        Err(StatusError::NotCorrectable { kind, status })
    }
}

/// `void` (D82): cualquier factura emitida que no esté ya anulada.
pub fn can_void(kind: DocumentKind, status: Status) -> Result<(), StatusError> {
    if kind.is_invoice() && matches!(status, Status::Issued | Status::Sent) {
        Ok(())
    } else {
        Err(StatusError::NotVoidable { kind, status })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversionSource {
    pub kind: DocumentKind,
    pub status: Status,
    /// NIF del cliente del documento de origen, si consta.
    pub customer_tax_id: Option<String>,
}

fn normalize_id(id: &str) -> String {
    id.chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .collect::<String>()
        .to_uppercase()
}

/// `convert(ids, to_kind)` (R15, R16, D74). Estado que debe fijarse en los orígenes tras
/// convertir: [`status_after_conversion`].
pub fn can_convert(sources: &[ConversionSource], to: DocumentKind) -> Result<(), StatusError> {
    use StatusError::InvalidConversion as Bad;
    let first = sources.first().ok_or(Bad("no hay documentos de origen"))?;
    match (first.kind, to) {
        (DocumentKind::Quote, DocumentKind::DeliveryNote | DocumentKind::Invoice) => {
            if sources.len() != 1 {
                return Err(Bad("un presupuesto se convierte de uno en uno"));
            }
            if first.status != Status::Accepted {
                return Err(Bad("el presupuesto debe estar aceptado"));
            }
            Ok(())
        }
        (DocumentKind::DeliveryNote, DocumentKind::Invoice) => {
            if sources.iter().any(|s| s.kind != DocumentKind::DeliveryNote) {
                return Err(Bad("no se mezclan albaranes con otros documentos"));
            }
            if sources.iter().any(|s| s.status != Status::Delivered) {
                return Err(Bad("los albaranes deben estar entregados"));
            }
            let ids: Vec<Option<String>> = sources
                .iter()
                .map(|s| {
                    s.customer_tax_id
                        .as_deref()
                        .map(normalize_id)
                        .filter(|id| !id.is_empty())
                })
                .collect();
            match &ids[0] {
                Some(id) if ids.iter().all(|other| other.as_ref() == Some(id)) => Ok(()),
                _ => Err(Bad("los albaranes deben ser del mismo cliente")),
            }
        }
        _ => Err(Bad("combinación de tipos no admitida")),
    }
}

/// Estado de un origen una vez convertido.
pub fn status_after_conversion(kind: DocumentKind) -> Option<Status> {
    match kind {
        DocumentKind::Quote => Some(Status::Converted),
        DocumentKind::DeliveryNote => Some(Status::Invoiced),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use DocumentKind::*;
    use Status::*;

    fn allowed_pairs(kind: DocumentKind) -> Vec<(Status, Status)> {
        let all = [
            Draft, Issued, Sent, Accepted, Rejected, Expired, Converted, Delivered, Invoiced,
            Voided,
        ];
        let mut v = Vec::new();
        for f in all {
            for t in all {
                if transition(kind, f, t).is_ok() {
                    v.push((f, t));
                }
            }
        }
        v
    }

    #[test]
    fn d74_quote_allowed_transitions_are_exactly_the_spec() {
        assert_eq!(
            allowed_pairs(Quote),
            vec![
                (Draft, Issued),
                (Issued, Sent),
                (Issued, Accepted),
                (Issued, Rejected),
                (Issued, Expired),
                (Sent, Accepted),
                (Sent, Rejected),
                (Sent, Expired),
                (Accepted, Converted),
            ]
        );
    }

    #[test]
    fn d74_delivery_note_allowed_transitions_are_exactly_the_spec() {
        assert_eq!(
            allowed_pairs(DeliveryNote),
            vec![
                (Draft, Issued),
                (Issued, Sent),
                (Issued, Delivered),
                (Sent, Delivered),
                (Delivered, Invoiced),
            ]
        );
    }

    #[test]
    fn d74_invoice_kinds_share_the_same_machine() {
        let expected = vec![
            (Draft, Issued),
            (Issued, Sent),
            (Issued, Voided),
            (Sent, Voided),
        ];
        for kind in [Invoice, SimplifiedInvoice, CorrectiveInvoice] {
            assert_eq!(allowed_pairs(kind), expected, "{kind:?}");
        }
    }

    #[test]
    fn d74_forbidden_transitions_have_explicit_errors() {
        // Saltar la emisión, volver atrás, resucitar estados finales, estados de otro tipo.
        assert_eq!(
            transition(Quote, Draft, Sent),
            Err(StatusError::InvalidTransition {
                kind: Quote,
                from: Draft,
                to: Sent
            })
        );
        assert!(transition(Quote, Sent, Issued).is_err());
        assert!(transition(Quote, Rejected, Accepted).is_err());
        assert!(transition(Quote, Converted, Accepted).is_err());
        assert!(transition(Quote, Issued, Converted).is_err());
        assert!(transition(DeliveryNote, Draft, Delivered).is_err());
        assert!(transition(DeliveryNote, Issued, Invoiced).is_err());
        assert!(transition(DeliveryNote, Invoiced, Delivered).is_err());
        assert!(transition(Invoice, Voided, Issued).is_err());
        assert!(transition(Invoice, Sent, Issued).is_err());
        assert!(transition(Invoice, Draft, Voided).is_err());
        assert!(transition(Invoice, Issued, Issued).is_err());
        assert_eq!(
            transition(Invoice, Issued, Accepted),
            Err(StatusError::NotApplicable {
                kind: Invoice,
                status: Accepted
            })
        );
        assert_eq!(
            transition(Quote, Voided, Draft),
            Err(StatusError::NotApplicable {
                kind: Quote,
                status: Voided
            })
        );
        assert_eq!(
            transition(Invoice, Issued, Draft).unwrap_err().code(),
            "invalid_transition"
        );
    }

    #[test]
    fn d74_received_delivery_note_transitions() {
        use ReceivedStatus::*;
        assert!(Received.transition(Reviewed).is_ok());
        assert!(Reviewed.transition(Received).is_err());
        assert!(Received.transition(Received).is_err());
        assert!(Reviewed.transition(Reviewed).is_err());
    }

    #[test]
    fn d74_only_drafts_are_editable() {
        assert!(ensure_editable(Draft).is_ok());
        for s in [Issued, Sent, Accepted, Voided, Delivered] {
            assert_eq!(
                ensure_editable(s),
                Err(StatusError::Immutable { status: s })
            );
        }
    }

    #[test]
    fn d74_revisions_only_for_issued_quotes_and_delivery_notes() {
        assert_eq!(next_revision(Quote, Issued, 1), Ok(2));
        assert_eq!(next_revision(DeliveryNote, Sent, 4), Ok(5));
        for (kind, status) in [
            (Quote, Draft),
            (Quote, Accepted),
            (Quote, Converted),
            (DeliveryNote, Invoiced),
            (DeliveryNote, Delivered),
            (Invoice, Issued),
            (SimplifiedInvoice, Sent),
            (CorrectiveInvoice, Issued),
        ] {
            assert!(
                matches!(
                    next_revision(kind, status, 1),
                    Err(StatusError::NotRevisable { .. })
                ),
                "{kind:?} {status:?}"
            );
        }
        assert!(next_revision(Quote, Issued, u32::MAX).is_err());
    }

    #[test]
    fn d81_only_issued_full_or_simplified_invoices_are_corrected() {
        assert!(can_correct(Invoice, Issued).is_ok());
        assert!(can_correct(SimplifiedInvoice, Sent).is_ok());
        for (kind, status) in [
            (Quote, Issued),
            (DeliveryNote, Issued),
            (Invoice, Draft),
            (Invoice, Voided),
            (CorrectiveInvoice, Issued),
        ] {
            assert_eq!(
                can_correct(kind, status),
                Err(StatusError::NotCorrectable { kind, status })
            );
        }
    }

    #[test]
    fn d82_void_only_issued_invoices() {
        for kind in [Invoice, SimplifiedInvoice, CorrectiveInvoice] {
            assert!(can_void(kind, Issued).is_ok());
            assert!(can_void(kind, Sent).is_ok());
            assert!(can_void(kind, Draft).is_err());
            assert!(can_void(kind, Voided).is_err());
        }
        assert!(can_void(Quote, Issued).is_err());
        assert!(can_void(DeliveryNote, Issued).is_err());
    }

    fn src(kind: DocumentKind, status: Status, nif: Option<&str>) -> ConversionSource {
        ConversionSource {
            kind,
            status,
            customer_tax_id: nif.map(str::to_owned),
        }
    }

    #[test]
    fn d74_quote_conversion_rules() {
        let q = src(Quote, Accepted, Some("12345678Z"));
        assert!(can_convert(std::slice::from_ref(&q), DeliveryNote).is_ok());
        assert!(can_convert(std::slice::from_ref(&q), Invoice).is_ok());
        assert!(can_convert(&[q.clone(), q.clone()], Invoice).is_err());
        assert!(can_convert(&[src(Quote, Issued, None)], Invoice).is_err());
        assert!(can_convert(std::slice::from_ref(&q), Quote).is_err());
        assert!(can_convert(&[q], SimplifiedInvoice).is_err());
        assert!(can_convert(&[], Invoice).is_err());
    }

    #[test]
    fn d74_several_delivery_notes_of_same_customer_become_one_invoice() {
        let a = src(DeliveryNote, Delivered, Some("A12345674"));
        let b = src(DeliveryNote, Delivered, Some(" a-12345674 "));
        assert!(can_convert(&[a.clone(), b], Invoice).is_ok());
        assert!(can_convert(std::slice::from_ref(&a), Invoice).is_ok());
        // Otro cliente, sin NIF, no entregado, mezcla de tipos o destino no admitido.
        let other = src(DeliveryNote, Delivered, Some("B12345674"));
        assert!(can_convert(&[a.clone(), other], Invoice).is_err());
        let none = src(DeliveryNote, Delivered, None);
        assert!(can_convert(&[a.clone(), none.clone()], Invoice).is_err());
        assert!(can_convert(&[none], Invoice).is_err());
        assert!(can_convert(&[src(DeliveryNote, Delivered, Some("  "))], Invoice).is_err());
        let issued = src(DeliveryNote, Issued, Some("A12345674"));
        assert!(can_convert(&[a.clone(), issued], Invoice).is_err());
        let q = src(Quote, Accepted, Some("A12345674"));
        assert!(can_convert(&[a.clone(), q], Invoice).is_err());
        assert!(can_convert(std::slice::from_ref(&a), DeliveryNote).is_err());
        assert!(can_convert(&[src(Invoice, Issued, Some("A12345674"))], Invoice).is_err());
    }

    #[test]
    fn d74_status_after_conversion() {
        assert_eq!(status_after_conversion(Quote), Some(Converted));
        assert_eq!(status_after_conversion(DeliveryNote), Some(Invoiced));
        assert_eq!(status_after_conversion(Invoice), None);
    }
}
