//! Evaluación de reglas expresas sobre datos ya validados (R07). Una acción se automatiza
//! solo si exactamente una regla activa coincide y los datos no tienen incidencias; en
//! cualquier otro caso queda pendiente de revisión.

use std::cmp::Ordering;
use std::collections::HashSet;

use nestadmin_ai::tasks::REVIEW_ONLY_CATEGORIES;
use nestadmin_core::{Rule, Task};
use nestadmin_documents::parse_amount;
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::{EMAIL, INVOICE};

pub const REGISTER_INVOICE: &str = "register_invoice";
pub const SEND_DRAFT: &str = "send_draft";
/// Acción de enviar por correo un documento de venta emitido (E4b, D238).
pub const SEND_SALES_DOCUMENT: &str = nestadmin_core::sales::SEND_SALES_DOCUMENT;

/// Regla aplicable o motivo por el que no se automatiza.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuleDecision {
    Automate { rule_id: String, rule_version: i64 },
    Review(String),
}

/// Motivo estructurado de una decisión (D125).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasonCode {
    /// Una única regla activa coincide y no hay bloqueos.
    Automated,
    /// Ninguna regla activa coincide.
    NoActiveRuleMatches,
    InvoiceIssues,
    UnreadableFields,
    NoRecipient,
    /// Coinciden varias reglas activas.
    SeveralRulesMatch,
    /// Factura separada de un documento con varias.
    SplitInvoice,
    /// Importe dudoso que confirma una persona.
    AmountReview,
    /// Una regla de envío coincide con la categoría pero no indica remitente y no tiene la
    /// validación avanzada (D126).
    SenderRequired,
    /// El documento menciona otra moneda: solo se registran facturas en euros (D127).
    ForeignCurrency,
    /// La tarea no lleva la comprobación local de moneda (tarea anterior a D127).
    CurrencyNotChecked,
    /// El documento no muestra el euro (`€`, `EUR`, «euro»): no se puede comprobar que sea en
    /// euros, sea texto digital o escaneado (D127).
    CurrencyNotShown,
    /// Se continuó sin un paso del pipeline: la respuesta siempre se revisa (D150).
    PipelineStepWaived,
    /// El mensaje lleva señales de correo automático (D157, D158): ninguna regla responde a un
    /// remitente automático.
    AutomaticSender,
    /// El mensaje no lleva la comprobación de señales (tarea anterior, o proveedor sin
    /// cabeceras): fallo cerrado.
    SignalsNotChecked,
    /// Un destinatario del borrador tiene una dirección automática (recalculada sobre `draft.to`).
    RecipientAutomatic,
    /// La IA considera que el correo no requiere respuesta.
    NoReplyNeeded,
    /// La respuesta la pidió una persona («Redactar»): siempre se revisa.
    Redraft,
    /// El borrador no existe o está vacío.
    EmptyDraft,
    /// La tarea no lleva la comprobación del borrador (tarea anterior): fallo cerrado.
    DraftNotChecked,
    /// El borrador contiene enlaces, direcciones, teléfonos o IBAN (D160).
    DraftFlagged,
    /// La categoría (`otros`, `reclamacion`) siempre va a revisión (D177): defensa por si una
    /// regla antigua la envía sola.
    CategoryReviewOnly,
    /// Una regla aprendida aceptada en lote aún está en rodaje (D174): sus primeros envíos se
    /// revisan. Va después de todos los bloqueos: nunca los sustituye.
    Rodaje,
    /// Una regla de envío de un documento de venta coincide, pero algo del documento, del PDF, de
    /// la cuenta o del destinatario impide enviarlo sin una persona (D238).
    SalesSendBlocked,
    /// El correo del cliente ya no es el destinatario que fija la regla (D238): no se envía solo.
    RecipientChanged,
    /// La acción no es de un tipo que esta versión sepa evaluar: fallo cerrado (D238).
    UnknownAction,
}

/// Resultado de comprobar una regla contra los datos de una tarea. El motivo de no
/// coincidencia usa solo datos que ya están en el resultado de la tarea.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RuleCheck {
    pub id: String,
    pub version: i64,
    pub active: bool,
    pub matches: bool,
    pub mismatch: Option<String>,
    /// Solo una regla de envío que habría coincidido de tener remitente o validación
    /// avanzada (D126). No se serializa si es `false`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub sender_required: bool,
    /// Solo una regla de envío de un documento que coincidiría de no haber cambiado el correo del
    /// cliente respecto al destinatario de la regla (D238). No se serializa si es `false`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub recipient_changed: bool,
}

/// Explicación guardada en `result["evaluation"]`: sin contenido del correo.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Explanation {
    pub code: ReasonCode,
    pub facts: Value,
    pub candidates: Vec<RuleCheck>,
}

impl Explanation {
    /// Texto para el usuario; conserva los motivos de revisión originales.
    pub fn summary(&self) -> String {
        match self.code {
            ReasonCode::Automated => "una regla activa coincide".into(),
            ReasonCode::NoActiveRuleMatches => "ninguna regla activa coincide".into(),
            ReasonCode::InvoiceIssues => "la factura tiene incidencias".into(),
            ReasonCode::UnreadableFields => "hay campos ilegibles".into(),
            ReasonCode::NoRecipient => "el borrador no tiene destinatario".into(),
            ReasonCode::SeveralRulesMatch => format!(
                "{} reglas coinciden; la decisión no es inequívoca",
                self.facts["matching"].as_u64().unwrap_or(0)
            ),
            ReasonCode::SplitInvoice => SPLIT_REASON.into(),
            ReasonCode::AmountReview => self.facts["message"].as_str().unwrap_or("").into(),
            ReasonCode::ForeignCurrency => format!(
                "el documento menciona {}; hoy la aplicación solo registra facturas en euros",
                join_ids(&self.facts["currencies"])
            ),
            ReasonCode::CurrencyNotChecked => CURRENCY_UNCHECKED.into(),
            ReasonCode::CurrencyNotShown => CURRENCY_NOT_SHOWN.into(),
            ReasonCode::PipelineStepWaived => PIPELINE_WAIVED.into(),
            ReasonCode::AutomaticSender => format!(
                "el correo parece automático (señales: {}); ninguna regla responde a un remitente automático",
                join_ids(&self.facts["codes"])
            ),
            ReasonCode::SignalsNotChecked => SIGNALS_UNCHECKED.into(),
            ReasonCode::RecipientAutomatic => {
                "el destinatario del borrador parece una dirección automática".into()
            }
            ReasonCode::NoReplyNeeded => "la IA considera que el correo no requiere respuesta".into(),
            ReasonCode::Redraft => {
                "la respuesta se pidió a mano; una persona debe revisarla siempre".into()
            }
            ReasonCode::EmptyDraft => "el borrador está vacío".into(),
            ReasonCode::DraftNotChecked => DRAFT_UNCHECKED.into(),
            ReasonCode::DraftFlagged => format!(
                "el borrador contiene datos de contacto que no se pueden dar por buenos ({}); compruébelos",
                join_ids(&self.facts["flags"])
            ),
            ReasonCode::CategoryReviewOnly => CATEGORY_REVIEW_ONLY.into(),
            ReasonCode::Rodaje => rodaje_reason(&self.facts),
            ReasonCode::SalesSendBlocked => format!(
                "la regla coincide, pero el envío necesita una persona: {}",
                self.facts["reason"]
                    .as_str()
                    .unwrap_or("falta una comprobación")
            ),
            ReasonCode::RecipientChanged => RECIPIENT_CHANGED.into(),
            ReasonCode::UnknownAction => {
                "la acción no es de un tipo que esta versión sepa evaluar".into()
            }
            ReasonCode::SenderRequired => format!(
                "la regla no indica remitente; añádalo o valide la opción avanzada (regla: {})",
                self.facts["rule_ids"]
                    .as_array()
                    .map(|ids| ids
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(", "))
                    .unwrap_or_default()
            ),
        }
    }

    /// Reglas inactivas que habrían aplicado a estos datos.
    pub fn inactive_would_apply(&self) -> Vec<&RuleCheck> {
        self.candidates
            .iter()
            .filter(|c| !c.active && c.matches)
            .collect()
    }

    /// Explicación de un atajo previo a las reglas (importe dudoso).
    pub fn amount_review(message: &str) -> Self {
        Self {
            code: ReasonCode::AmountReview,
            facts: json!({ "message": message }),
            candidates: Vec::new(),
        }
    }

    /// Explicación de una tarea cuyo pipeline continuó sin un paso (D150).
    pub fn pipeline_step_waived() -> Self {
        Self {
            code: ReasonCode::PipelineStepWaived,
            facts: json!({}),
            candidates: Vec::new(),
        }
    }

    /// Explicación de un envío de rodaje (D174): la regla aprendida coincide y no hay bloqueos, pero
    /// aún no ha enviado sola. Solo ids y recuentos; las candidatas se conservan para las
    /// estadísticas (D129).
    pub fn rodaje(
        rule_id: &str,
        rule_version: i64,
        state: &nestadmin_core::RodajeState,
        candidates: Vec<RuleCheck>,
    ) -> Self {
        Self {
            code: ReasonCode::Rodaje,
            facts: json!({
                "rule_id": rule_id,
                "rule_version": rule_version,
                "left": state.left,
                "of": nestadmin_core::RODAJE_SENDS,
                // Reglas por dominio: direcciones distintas exigidas y ya cubiertas (0 y 0 si no).
                "min_addresses": state.min_addresses,
                "addresses": state.addresses(),
            }),
            candidates,
        }
    }

    /// Explicación de una factura separada de un documento con varias.
    pub fn split_invoice() -> Self {
        Self {
            code: ReasonCode::SplitInvoice,
            facts: json!({}),
            candidates: Vec::new(),
        }
    }
}

/// Motivo cuando el correo del cliente ya no es el destinatario que fija la regla (D238).
pub const RECIPIENT_CHANGED: &str =
    "el correo del cliente ya no es el destinatario de la regla; revise el envío";

/// Motivo cuando el pipeline continuó sin un paso (D150).
pub const PIPELINE_WAIVED: &str =
    "se continuó sin un paso anterior del pipeline; revise antes de enviar";

/// Motivo de un envío de rodaje (D174): «En rodaje: quedan N aprobaciones antes de enviar sola»,
/// y en una regla por dominio, las direcciones distintas que aún faltan por ver. Los datos
/// estructurados van en `facts`.
fn rodaje_reason(facts: &Value) -> String {
    let left = facts["left"]
        .as_i64()
        .unwrap_or(nestadmin_core::RODAJE_SENDS);
    let mut text = if left == 1 {
        "En rodaje: queda 1 aprobación antes de enviar sola".to_owned()
    } else {
        format!("En rodaje: quedan {left} aprobaciones antes de enviar sola")
    };
    let min = facts["min_addresses"].as_u64().unwrap_or(0);
    let seen = facts["addresses"].as_u64().unwrap_or(0);
    if seen < min {
        text.push_str(&format!(
            "; hacen falta aprobaciones de al menos {min} direcciones distintas del dominio (llevan {seen})"
        ));
    }
    text
}

/// Motivo cuando falta la comprobación local de moneda (D127).
pub const CURRENCY_UNCHECKED: &str = "no consta la comprobación de moneda; reprocese o revise";

/// Motivo cuando el mensaje no lleva la comprobación de correo automático (D157).
pub const SIGNALS_UNCHECKED: &str =
    "no consta la comprobación de correo automático; reprocese o revise";

/// Motivo cuando la categoría del correo es de solo revisión (D177).
pub const CATEGORY_REVIEW_ONLY: &str = "esta categoría siempre va a revisión: las reglas no envían respuestas a reclamaciones ni a correos de la categoría «Otros»";

/// Mensaje para la persona al guardar, probar o unificar una regla de esa categoría (D177).
pub const REVIEW_ONLY_MESSAGE: &str = "Esta categoría siempre va a revisión: las reglas no envían respuestas a reclamaciones ni a correos de la categoría «Otros».";

/// Motivo cuando la tarea no lleva la comprobación del borrador (D160).
pub const DRAFT_UNCHECKED: &str = "no consta la comprobación del borrador; reprocese o revise";

/// Motivo cuando el documento no muestra el euro (D127).
pub const CURRENCY_NOT_SHOWN: &str =
    "el documento no muestra la moneda; no se puede comprobar que sea en euros";

fn join_ids(value: &Value) -> String {
    value
        .as_array()
        .map(|ids| {
            ids.iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default()
}

/// Motivo de revisión de las facturas de un documento con varias (D68).
pub const SPLIT_REASON: &str =
    "el documento contiene varias facturas; la IA las separó y cada una se revisa por separado";

/// Decisión y su explicación estructurada (D125).
#[derive(Debug, Clone, PartialEq)]
pub struct Evaluation {
    pub decision: RuleDecision,
    pub explanation: Explanation,
}

impl Evaluation {
    /// Si la decisión es automatizar, la pasa a revisión por el rodaje de la regla (D174), con
    /// el estado `state` (aprobaciones y direcciones pendientes). Solo restringe: una revisión previa (y su motivo, que
    /// viene de los bloqueos de `send_draft_blocker`) no cambia nunca. Devuelve el id de la regla
    /// que automatizaría, para que quien llama sepa si hay rodaje que consultar.
    pub fn automating_rule(&self) -> Option<(&str, i64)> {
        match &self.decision {
            RuleDecision::Automate {
                rule_id,
                rule_version,
            } => Some((rule_id.as_str(), *rule_version)),
            RuleDecision::Review(_) => None,
        }
    }

    pub fn into_rodaje(self, state: &nestadmin_core::RodajeState) -> Self {
        let RuleDecision::Automate {
            rule_id,
            rule_version,
        } = &self.decision
        else {
            return self;
        };
        let explanation =
            Explanation::rodaje(rule_id, *rule_version, state, self.explanation.candidates);
        Evaluation {
            decision: RuleDecision::Review(explanation.summary()),
            explanation,
        }
    }
}

/// `result` es el resultado de la tarea (ver `pipeline`). Condiciones admitidas:
/// - `register_invoice`: `supplier` (obligatoria, igualdad sin mayúsculas), `max_total` y
///   `currency` opcionales (D127).
/// - `send_draft`: `category` (obligatoria) y `sender` o `sender_domain` (D126).
///
/// Comprueba también las reglas inactivas, solo para poder decir que habrían aplicado: solo
/// una regla activa puede automatizar.
///
/// Sin validación avanzada de remitente (D126): una regla de envío sin remitente no coincide.
pub fn evaluate(rules: &[Rule], action_kind: &str, result: &Value) -> Evaluation {
    evaluate_with(rules, &HashSet::new(), action_kind, result)
}

/// Como `evaluate`; `any_sender` son los ids de reglas de envío con la validación avanzada
/// que las deja contestar a cualquier remitente (D126).
pub fn evaluate_with(
    rules: &[Rule],
    any_sender: &HashSet<String>,
    action_kind: &str,
    result: &Value,
) -> Evaluation {
    let candidates: Vec<(&Rule, RuleCheck)> = rules
        .iter()
        .filter(|r| r.action_kind == action_kind)
        .map(|r| {
            let mismatch = mismatch(r, any_sender.contains(&r.id), result);
            let check = RuleCheck {
                id: r.id.clone(),
                version: r.version,
                active: r.active,
                matches: mismatch.is_none(),
                sender_required: mismatch.as_ref().is_some_and(|m| m.sender_required),
                recipient_changed: mismatch.as_ref().is_some_and(|m| m.recipient_changed),
                mismatch: mismatch.map(|m| m.text),
            };
            (r, check)
        })
        .collect();
    let sender_required: Vec<&str> = candidates
        .iter()
        .filter(|(r, c)| r.active && c.sender_required)
        .map(|(r, _)| r.id.as_str())
        .collect();
    let recipient_changed: Vec<&str> = candidates
        .iter()
        .filter(|(r, c)| r.active && c.recipient_changed)
        .map(|(r, _)| r.id.as_str())
        .collect();
    let matching: Vec<&Rule> = candidates
        .iter()
        .filter(|(r, c)| r.active && c.matches)
        .map(|(r, _)| *r)
        .collect();
    let (code, facts, automated) = match (matching.as_slice(), blockers(action_kind, result)) {
        ([], _) if !sender_required.is_empty() => (
            ReasonCode::SenderRequired,
            json!({ "rule_ids": sender_required }),
            None,
        ),
        ([], _) if !recipient_changed.is_empty() => (
            ReasonCode::RecipientChanged,
            json!({ "rule_ids": recipient_changed }),
            None,
        ),
        ([], _) => (ReasonCode::NoActiveRuleMatches, json!({}), None),
        (_, Some((code, facts))) => (code, facts, None),
        ([rule], None) => (
            ReasonCode::Automated,
            json!({ "rule_id": rule.id, "rule_version": rule.version }),
            Some((rule.id.clone(), rule.version)),
        ),
        (several, None) => (
            ReasonCode::SeveralRulesMatch,
            json!({
                "matching": several.len(),
                "rule_ids": several.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            }),
            None,
        ),
    };
    let explanation = Explanation {
        code,
        facts,
        candidates: candidates.into_iter().map(|(_, c)| c).collect(),
    };
    // El texto de revisión sale siempre de `summary()`: origen único.
    let decision = match automated {
        Some((rule_id, rule_version)) => RuleDecision::Automate {
            rule_id,
            rule_version,
        },
        None => RuleDecision::Review(explanation.summary()),
    };
    Evaluation {
        decision,
        explanation,
    }
}

/// Envoltura de `evaluate` que descarta la explicación.
pub fn decide(rules: &[Rule], action_kind: &str, result: &Value) -> RuleDecision {
    evaluate(rules, action_kind, result).decision
}

/// Monedas distintas del euro que consta que menciona una factura (D127): las del texto
/// local (`currency_evidence`) y la moneda explícita de los campos si no es EUR. `None` si la
/// tarea no lleva la comprobación o es defectuosa (tareas anteriores, `foreign` que no es una
/// lista de textos): fallo cerrado.
pub fn foreign_currencies(result: &Value) -> Option<Vec<String>> {
    let evidence = result.get("currency_evidence").filter(|e| e.is_object())?;
    let mut foreign: Vec<String> = evidence
        .get("foreign")?
        .as_array()?
        .iter()
        .map(|c| c.as_str().map(str::to_owned))
        .collect::<Option<_>>()?;
    if let Some(explicit) = result["fields"]["currency"]
        .as_str()
        .map(str::trim)
        .filter(|c| !c.is_empty() && !c.eq_ignore_ascii_case("EUR"))
    {
        foreign.push(explicit.to_ascii_uppercase());
    }
    foreign.sort();
    foreign.dedup();
    Some(foreign)
}

/// El documento muestra el euro (`currency_evidence.euro`): evidencia positiva de que una
/// factura está en euros (D127). Sin ella no se puede comprobar, sea texto digital o OCR.
pub fn shows_euro(result: &Value) -> bool {
    result["currency_evidence"]["euro"].as_bool() == Some(true)
}

/// Motivos que impiden automatizar aunque haya una regla.
fn blockers(action_kind: &str, result: &Value) -> Option<(ReasonCode, Value)> {
    let non_empty = |key: &str| result[key].as_array().is_some_and(|a| !a.is_empty());
    match action_kind {
        REGISTER_INVOICE if non_empty("issues") => Some((ReasonCode::InvoiceIssues, json!({}))),
        REGISTER_INVOICE if non_empty("unreadable") => {
            Some((ReasonCode::UnreadableFields, json!({})))
        }
        REGISTER_INVOICE => match foreign_currencies(result) {
            None => Some((ReasonCode::CurrencyNotChecked, json!({}))),
            Some(list) if !list.is_empty() => {
                Some((ReasonCode::ForeignCurrency, json!({ "currencies": list })))
            }
            Some(_) if !shows_euro(result) => Some((ReasonCode::CurrencyNotShown, json!({}))),
            Some(_) => None,
        },
        SEND_DRAFT => send_draft_blocker(result),
        SEND_SALES_DOCUMENT => sales_send_blocker(result),
        // Fallo cerrado (D238): una acción que esta función no conoce no se automatiza nunca.
        _ => Some((ReasonCode::UnknownAction, json!({}))),
    }
}

/// Bloqueos del envío de un documento de venta (D238). Cada dato ausente o sin comprobar bloquea:
/// `result["checks"]` lo escribe la preparación del envío y sin él nada se automatiza. Los
/// motivos no llevan contenido: solo códigos y textos fijos.
fn sales_send_blocker(result: &Value) -> Option<(ReasonCode, Value)> {
    let checks = &result["checks"];
    let blocked = |code: &str, reason: &str| {
        Some((
            ReasonCode::SalesSendBlocked,
            json!({ "reason": reason, "code": code }),
        ))
    };
    if !checks.is_object() {
        return blocked("not_checked", "no consta la comprobación del envío");
    }
    if checks["sendable"].as_bool() != Some(true) {
        return blocked(
            "status_not_sendable",
            "el documento no está en un estado enviable",
        );
    }
    match checks["pdf"].as_str() {
        Some("ok") => {}
        Some("too_large") => {
            return blocked(
                "attachment_too_large",
                "el PDF supera el tamaño que se puede adjuntar",
            )
        }
        Some(_) => return blocked("pdf_failed", "no se pudo generar el PDF"),
        None => return blocked("not_checked", "no consta la comprobación del PDF"),
    }
    match checks["account"].as_str() {
        Some("ok") => {}
        Some("unavailable") => {
            return blocked(
                "account_unavailable",
                "la cuenta de correo no está disponible",
            )
        }
        Some(_) => {
            return blocked(
                "account_required",
                "no hay una cuenta de correo para enviarlo",
            )
        }
        None => return blocked("not_checked", "no consta la comprobación de la cuenta"),
    }
    match checks["recipient"].as_str() {
        Some("ok") => None,
        Some(_) => blocked("recipient_missing", "el cliente no tiene un correo válido"),
        None => blocked("not_checked", "no consta la comprobación del destinatario"),
    }
}

/// Bloqueos de un borrador de correo (D158, D160). Cada dato ausente, antiguo o sin comprobar
/// bloquea (fallo cerrado), y ninguna preferencia ni dato del propio correo lo desbloquea. Los
/// motivos no llevan contenido: solo códigos y recuentos.
fn send_draft_blocker(result: &Value) -> Option<(ReasonCode, Value)> {
    // `needs_reply` ausente equivale a verdadero; solo `false` bloquea. Va antes que el
    // destinatario: un correo sin respuesta necesaria no tiene borrador.
    if result["needs_reply"].as_bool() == Some(false) {
        return Some((ReasonCode::NoReplyNeeded, json!({})));
    }
    if !result["redraft_of"].is_null() {
        return Some((ReasonCode::Redraft, json!({})));
    }
    let recipients = result["draft"]["to"].as_array();
    if recipients.is_none_or(|a| a.is_empty()) {
        return Some((ReasonCode::NoRecipient, json!({})));
    }
    if result["draft"]["body"]
        .as_str()
        .is_none_or(|body| body.trim().is_empty())
    {
        return Some((ReasonCode::EmptyDraft, json!({})));
    }
    // Señales del mensaje: `None` (ausente o mal formado) es «sin comprobar».
    let signals = result["message"]["signals"].as_array().and_then(|list| {
        list.iter()
            .map(|code| code.as_str().map(str::to_owned))
            .collect::<Option<Vec<String>>>()
    });
    match signals {
        None => return Some((ReasonCode::SignalsNotChecked, json!({}))),
        Some(codes) if !codes.is_empty() => {
            return Some((ReasonCode::AutomaticSender, json!({ "codes": codes })))
        }
        Some(_) => {}
    }
    // La dirección se recalcula siempre sobre los destinatarios: una tarea antigua no lleva esta
    // comprobación y un destinatario no-reply no puede responderse sin una persona.
    let automatic = recipients.into_iter().flatten().any(|to| {
        to.as_str()
            .is_none_or(|a| !nestadmin_core::mail_signals::address_signals(Some(a)).is_empty())
    });
    if automatic {
        return Some((ReasonCode::RecipientAutomatic, json!({})));
    }
    match result["draft_flags"].as_array() {
        None => Some((ReasonCode::DraftNotChecked, json!({}))),
        Some(flags) if !flags.is_empty() => {
            Some((ReasonCode::DraftFlagged, json!({ "flags": flags })))
        }
        Some(_) => {
            let category = result["category"].as_str().unwrap_or("");
            REVIEW_ONLY_CATEGORIES.contains(&category).then(|| {
                (
                    ReasonCode::CategoryReviewOnly,
                    json!({ "category": category }),
                )
            })
        }
    }
}

/// Por qué una regla no coincide. `sender_required`: solo falta el remitente o su validación
/// avanzada (D126); el resto de condiciones sí coincide.
struct Mismatch {
    text: String,
    sender_required: bool,
    /// Solo falta que el destinatario de la regla sea el correo actual del cliente (D238).
    recipient_changed: bool,
}

fn no(text: impl Into<String>) -> Option<Mismatch> {
    Some(Mismatch {
        text: text.into(),
        sender_required: false,
        recipient_changed: false,
    })
}

/// `None` si la regla coincide; si no, el motivo con los datos concretos. Fallo cerrado
/// (D123): una regla con claves desconocidas o con `max_total` que no sea un texto de importe
/// válido no coincide nunca, aunque esté activa.
fn mismatch(rule: &Rule, any_sender_ack: bool, result: &Value) -> Option<Mismatch> {
    let Some(conditions) = rule.conditions.as_object() else {
        return no("las condiciones de la regla no son válidas");
    };
    let allowed: &[&str] = match rule.action_kind.as_str() {
        REGISTER_INVOICE => &["supplier", "max_total", "currency"],
        SEND_DRAFT => &["category", "sender", "sender_domain"],
        SEND_SALES_DOCUMENT => SALES_SEND_CONDITIONS,
        _ => return no("tipo de regla no reconocido"),
    };
    if let Some(key) = conditions.keys().find(|k| !allowed.contains(&k.as_str())) {
        return no(format!("la regla tiene una condición desconocida: «{key}»"));
    }
    let condition = |key: &str| conditions.get(key).and_then(Value::as_str);
    match rule.action_kind.as_str() {
        SEND_SALES_DOCUMENT => sales_send_mismatch(conditions, result),
        REGISTER_INVOICE => {
            let Some(supplier) = condition("supplier") else {
                return no("la regla no indica proveedor");
            };
            // Un proveedor vacío en la regla no coincide con una factura sin proveedor: fallo
            // cerrado.
            if supplier.trim().is_empty() {
                return no("la regla tiene un proveedor vacío");
            }
            let actual = result["fields"]["supplier"].as_str().unwrap_or("");
            if !actual.trim().eq_ignore_ascii_case(supplier.trim()) {
                return no(if actual.trim().is_empty() {
                    format!("la factura no indica proveedor y la regla pide «{supplier}»")
                } else {
                    format!("el proveedor es «{actual}» y la regla pide «{supplier}»")
                });
            }
            // Hoy solo se registran facturas en euros (D91, D127).
            if conditions
                .get("currency")
                .is_some_and(|c| c.as_str() != Some("EUR"))
            {
                return no("la moneda de la regla no es EUR; hoy la aplicación solo registra facturas en euros");
            }
            // Un límite está en euros: si el documento menciona otra moneda, no coincide.
            if conditions.contains_key("max_total") {
                if let Some(list) = foreign_currencies(result).filter(|l| !l.is_empty()) {
                    return no(format!(
                        "el documento menciona {} y el límite está en EUR",
                        list.join(", ")
                    ));
                }
            }
            match conditions.get("max_total") {
                None => None,
                Some(Value::String(limit)) => {
                    let total = result["fields"]["total"].as_str();
                    match (
                        parse_amount("max_total", limit),
                        total.map(|t| parse_amount("total", t)),
                    ) {
                        (Ok(l), Some(Ok(t))) if t <= l => None,
                        (Ok(_), Some(Ok(_))) => no(format!(
                            "el total {} supera el máximo {limit}",
                            total.unwrap_or("")
                        )),
                        (Err(_), _) => no(format!("el máximo «{limit}» no es un importe válido")),
                        // Sin total legible no se puede comprobar el límite.
                        _ => no("la factura no tiene un total legible para comprobar el máximo"),
                    }
                }
                // Límite no textual: la regla es defectuosa y no automatiza.
                Some(_) => no("el máximo de la regla no es un texto de importe"),
            }
        }
        _ => {
            let Some(wanted) = condition("category") else {
                return no("la regla no indica categoría");
            };
            let actual = result["category"].as_str();
            if actual != Some(wanted) {
                return no(format!(
                    "la categoría es «{}» y la regla pide «{wanted}»",
                    actual.unwrap_or("sin categoría")
                ));
            }
            sender_mismatch(conditions, any_sender_ack, result)
        }
    }
}

/// Condiciones de una regla de envío de un documento de venta (D238): `customer_id`, `recipient` y
/// `account` son obligatorias; `document_kinds` es opcional.
pub const SALES_SEND_CONDITIONS: &[&str] =
    &["customer_id", "recipient", "account", "document_kinds"];

/// Coincidencia de una regla de envío de un documento (D238) con los datos de `result`
/// (`customer_id`, `document.kind`, `account.id`, `draft.to` y `customer_email_at_prepare`). Una
/// regla sin `customer_id`, `recipient` o `account` no coincide nunca (fallo cerrado): no hay
/// «cualquier cliente», «cualquier destinatario» ni «cualquier cuenta». La regla solo coincide si
/// el destinatario del borrador es el correo del cliente al preparar y es el que fija la regla:
/// un destinatario editado a mano nunca lo cubre.
fn sales_send_mismatch(
    conditions: &serde_json::Map<String, Value>,
    result: &Value,
) -> Option<Mismatch> {
    let Some(customer) = conditions.get("customer_id").and_then(Value::as_i64) else {
        return no("la regla no indica cliente");
    };
    let Some(recipient) = conditions
        .get("recipient")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|r| is_email_address(r))
    else {
        return no("la regla no indica un destinatario válido");
    };
    let Some(account) = conditions
        .get("account")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|a| !a.is_empty())
    else {
        return no("la regla no indica cuenta de envío");
    };
    match conditions.get("document_kinds") {
        None => {}
        Some(Value::Array(kinds)) if !kinds.is_empty() => {
            let actual = result["document"]["kind"].as_str();
            if !kinds
                .iter()
                .any(|k| k.as_str().is_some() && k.as_str() == actual)
            {
                return no(format!(
                    "el documento es de tipo «{}» y la regla no lo incluye",
                    actual.unwrap_or("desconocido")
                ));
            }
        }
        Some(_) => return no("los tipos de documento de la regla no son una lista válida"),
    }
    if result["customer_id"].as_i64() != Some(customer) {
        return no("el documento es de otro cliente que el de la regla");
    }
    let to = result["draft"]["to"].as_array().filter(|t| t.len() == 1);
    let to = to.and_then(|t| t[0].as_str());
    let at_prepare = result["customer_email_at_prepare"].as_str();
    let same = |a: Option<&str>| a.is_some_and(|a| a.trim().eq_ignore_ascii_case(recipient));
    if !same(at_prepare) {
        return Some(Mismatch {
            text: "el correo del cliente ya no es el destinatario de la regla".into(),
            sender_required: false,
            recipient_changed: true,
        });
    }
    if !same(to) {
        return Some(Mismatch {
            text: "el destinatario del borrador no es el de la regla".into(),
            sender_required: false,
            recipient_changed: true,
        });
    }
    if result["account"]["id"].as_str() != Some(account) {
        return no("la cuenta de envío no es la de la regla");
    }
    None
}

/// Condición de remitente de una regla de envío (D126). La comparación no distingue
/// mayúsculas y usa `message.from`, que es solo la dirección. El dominio es exacto: la parte
/// tras la `@`, sin subdominios.
fn sender_mismatch(
    conditions: &serde_json::Map<String, Value>,
    any_sender_ack: bool,
    result: &Value,
) -> Option<Mismatch> {
    let actual = result["message"]["from"].as_str();
    let (key, wanted) = match (conditions.get("sender"), conditions.get("sender_domain")) {
        (Some(_), Some(_)) => {
            return no("la regla indica a la vez una dirección y un dominio de remitente")
        }
        (Some(value), None) => ("sender", value),
        (None, Some(value)) => ("sender_domain", value),
        (None, None) => {
            return if any_sender_ack {
                None
            } else {
                Some(Mismatch {
                    text: "la regla no indica remitente; añádalo o valide la opción avanzada"
                        .into(),
                    sender_required: true,
                    recipient_changed: false,
                })
            }
        }
    };
    let Some(wanted) = wanted.as_str().map(str::trim).filter(|w| !w.is_empty()) else {
        return no("el remitente de la regla no es un texto válido");
    };
    sender_matches(key, wanted, actual).err().and_then(no)
}

/// Comparación de remitente de D126, compartida por las reglas de envío y por las plantillas
/// de pipeline (D149): `key` es `sender` (dirección exacta) o `sender_domain` (dominio exacto, sin
/// subdominios), sin distinguir mayúsculas. `actual` es `message.from`; si falta, está vacío o no
/// es una dirección simple (`is_email_address`), no coincide. El error es el motivo en texto.
pub fn sender_matches(key: &str, wanted: &str, actual: Option<&str>) -> Result<(), String> {
    let Some(actual) = actual.map(str::trim).filter(|a| !a.is_empty()) else {
        return Err(format!(
            "el correo no indica remitente y la regla pide «{wanted}»"
        ));
    };
    if !is_email_address(actual) {
        return Err(format!(
            "el remitente «{actual}» no es una dirección de correo simple"
        ));
    }
    let same = if key == "sender" {
        actual.eq_ignore_ascii_case(wanted)
    } else {
        actual
            .rsplit_once('@')
            .is_some_and(|(_, domain)| domain.eq_ignore_ascii_case(wanted))
    };
    if same {
        Ok(())
    } else if key == "sender" {
        Err(format!(
            "el remitente es «{actual}» y la regla pide «{wanted}»"
        ))
    } else {
        Err(format!(
            "el remitente es «{actual}», de otro dominio que «{wanted}»"
        ))
    }
}

/// Dominio de correo: etiquetas de letras, cifras y guiones separadas por puntos, con al
/// menos un punto (D126). Sin `@`, espacios ni comodines.
pub fn is_domain(domain: &str) -> bool {
    domain.len() <= 253
        && domain.contains('.')
        && domain.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
}

/// Dirección de correo simple: `local@dominio` con un único `@`, sin espacios, comas ni
/// ángulos (D126). No pretende cubrir todo RFC 5322: una dirección rara se rechaza.
pub fn is_email_address(address: &str) -> bool {
    let Some((local, domain)) = address.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && local.len() <= 64
        && local
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._%+-'=!#$&*/?^`{|}~".contains(&b))
        && is_domain(domain)
}

/// Dominios de correo públicos (gratuitos y masivos) que la interfaz señala: una regla de
/// remitente con uno de ellos abarca a cualquier usuario del servicio, no a una empresa. Solo
/// avisa, no bloquea. Sin distinguir mayúsculas ni espacios en los extremos.
pub fn is_public_mail_domain(domain: &str) -> bool {
    const PUBLIC: &[&str] = &[
        "gmail.com",
        "googlemail.com",
        "outlook.com",
        "outlook.es",
        "hotmail.com",
        "hotmail.es",
        "live.com",
        "msn.com",
        "yahoo.com",
        "yahoo.es",
        "ymail.com",
        "icloud.com",
        "me.com",
        "mac.com",
        "aol.com",
        "proton.me",
        "protonmail.com",
        "gmx.com",
        "gmx.es",
        "gmx.net",
        "mail.com",
        "zoho.com",
        "yandex.com",
        "telefonica.net",
        "movistar.es",
        "orange.es",
    ];
    let domain = domain.trim().to_ascii_lowercase();
    PUBLIC.contains(&domain.as_str())
}

/// Huella estable de las reglas relevantes para un tipo de acción: ids, versiones, estado
/// activo y validación avanzada. Dos evaluaciones con la misma huella usaron las mismas
/// reglas (D129). Las reglas archivadas no entran: no se evalúan.
pub fn rules_fingerprint(
    rules: &[Rule],
    any_sender: &HashSet<String>,
    action_kind: &str,
) -> String {
    let mut parts: Vec<String> = rules
        .iter()
        .filter(|r| r.action_kind == action_kind)
        .map(|r| {
            format!(
                "{}:{}:{}:{}",
                r.id,
                r.version,
                u8::from(r.active),
                u8::from(any_sender.contains(&r.id))
            )
        })
        .collect();
    parts.sort();
    let digest = Sha256::digest(parts.join("|").as_bytes());
    digest.iter().take(8).map(|b| format!("{b:02x}")).collect()
}

/// Evalúa el resultado de una tarea con los atajos previos a las reglas: un importe dudoso
/// (D120) y una factura separada de un documento con varias (D68) nunca se registran por
/// regla. Es el único camino que usan `finish` y la simulación.
pub fn evaluate_result(
    rules: &[Rule],
    any_sender: &HashSet<String>,
    action_kind: &str,
    result: &Value,
) -> Evaluation {
    evaluate_result_in_run(rules, any_sender, action_kind, result, false)
}

/// Como `evaluate_result`, para una tarea de un pipeline. Si se continuó sin un paso anterior
/// (`step_waived`, D150), una decisión de automatizar pasa a revisión con su propia explicación,
/// manteniendo las reglas candidatas para los recuentos (D129). Solo restringe: nunca convierte
/// una revisión en automatización ni cambia el motivo de una revisión que ya lo era.
pub fn evaluate_result_in_run(
    rules: &[Rule],
    any_sender: &HashSet<String>,
    action_kind: &str,
    result: &Value,
    step_waived: bool,
) -> Evaluation {
    let evaluation = evaluate_shortcuts(rules, any_sender, action_kind, result);
    match evaluation.decision {
        RuleDecision::Automate { .. } if step_waived => {
            let mut explanation = Explanation::pipeline_step_waived();
            explanation.candidates = evaluation.explanation.candidates;
            Evaluation {
                decision: RuleDecision::Review(explanation.summary()),
                explanation,
            }
        }
        _ => evaluation,
    }
}

fn evaluate_shortcuts(
    rules: &[Rule],
    any_sender: &HashSet<String>,
    action_kind: &str,
    result: &Value,
) -> Evaluation {
    let shortcut = if let Some(message) = result["amount_review"].as_str() {
        Some(Explanation::amount_review(message))
    } else if result["part"]["of"].as_u64().is_some_and(|n| n > 1) {
        Some(Explanation::split_invoice())
    } else {
        None
    };
    let evaluated = evaluate_with(rules, any_sender, action_kind, result);
    match shortcut {
        // Conserva el código del atajo, pero lista también qué reglas coincidían para que
        // `reviewed` cuente las tareas que una regla habría automatizado (D129).
        Some(mut explanation) => {
            explanation.candidates = evaluated.explanation.candidates;
            Evaluation {
                decision: RuleDecision::Review(explanation.summary()),
                explanation,
            }
        }
        None => evaluated,
    }
}

// ---- Contradicciones y unificación (D130, D131) ----
//
// Las reglas solo autorizan: ninguna prohíbe. El conflicto real viene de D17: si dos reglas
// activas coinciden con una tarea, esa tarea va a revisión. «Alfa sin límite» y «Alfa ≤ 500»
// solo automatizan por encima de 500, al revés de lo que se espera. Todo lo de aquí es puro
// (sin base de datos ni IA) y de solo lectura: nada se aplica sin aceptación.

/// Id de la regla resultante en la simulación de una unificación: no es un id real.
const MERGED: &str = "(unificada)";
/// Tareas que mira la simulación de una unificación.
const MERGE_SIMULATION_LIMIT: usize = 200;

/// Marcas que no están en `Rule`: ids con la validación avanzada de «cualquier remitente»
/// (D126) y ids archivados (D128). Una regla archivada no entra en ningún análisis.
#[derive(Debug, Clone, Default)]
pub struct Marks {
    pub any_sender: HashSet<String>,
    pub archived: HashSet<String>,
}

/// Cuál de las dos reglas comparadas es la más amplia.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    First,
    Second,
}

impl Side {
    fn flip(self) -> Side {
        match self {
            Side::First => Side::Second,
            Side::Second => Side::First,
        }
    }
}

/// Por qué dos reglas no se pueden comparar.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum NotComparable {
    /// Reglas de tipos distintos.
    DifferentKinds,
    /// Tipo que la aplicación no reconoce (por ejemplo, el futuro «emitir recurrente»).
    UnknownKind,
    /// Alguna regla no tiene condiciones válidas o no indica remitente: no actúa.
    Defective { rule_ids: Vec<String> },
}

/// Cómo se relacionan las tareas que cubren dos reglas.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "relation", rename_all = "snake_case")]
pub enum Relation {
    /// Cubren exactamente lo mismo.
    Equal,
    /// Una cubre todo lo de la otra y más.
    Contains {
        wider: Side,
    },
    /// Proveedor con nombre parecido: solo una pregunta, nunca igual.
    Similar,
    /// No comparten ninguna tarea.
    Disjoint,
    NotComparable {
        reason: NotComparable,
    },
}

impl Relation {
    /// La misma relación vista desde la otra regla: `compare(b, a) == compare(a, b).mirrored()`.
    pub fn mirrored(self) -> Relation {
        match self {
            Relation::Contains { wider } => Relation::Contains {
                wider: wider.flip(),
            },
            other => other,
        }
    }
}

/// Lo que impide que una regla actúe.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Defect {
    UnknownKind,
    Invalid(String),
    NoSender,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Scope {
    Address(String),
    Domain(String),
    Any,
}

#[derive(Debug, Clone)]
struct InvoiceCond {
    /// Proveedor tal como lo escribió la regla, recortado.
    supplier: String,
    /// Clave de comparación, igual que `matches`: recortado y sin distinguir mayúsculas.
    key: String,
    /// Límite canónico («500.00») o sin límite.
    limit: Option<String>,
}

#[derive(Debug, Clone)]
struct SendCond {
    category: String,
    scope: Scope,
}

#[derive(Debug, Clone)]
enum Cond {
    Invoice(InvoiceCond),
    Send(SendCond),
}

fn invalid(text: impl Into<String>) -> Defect {
    Defect::Invalid(text.into())
}

fn text_of<'a>(map: &'a serde_json::Map<String, Value>, key: &str) -> Option<&'a str> {
    map.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn parse_cond(rule: &Rule, marks: &Marks) -> Result<Cond, Defect> {
    let allowed: &[&str] = match rule.action_kind.as_str() {
        REGISTER_INVOICE => &["supplier", "max_total", "currency"],
        SEND_DRAFT => &["category", "sender", "sender_domain"],
        _ => return Err(Defect::UnknownKind),
    };
    let Some(map) = rule.conditions.as_object() else {
        return Err(invalid("sus condiciones no son válidas"));
    };
    if let Some(key) = map.keys().find(|k| !allowed.contains(&k.as_str())) {
        return Err(invalid(format!(
            "tiene una condición desconocida («{key}»)"
        )));
    }
    if rule.action_kind == REGISTER_INVOICE {
        let Some(supplier) = text_of(map, "supplier") else {
            return Err(invalid("no indica proveedor"));
        };
        if map
            .get("currency")
            .is_some_and(|c| c.as_str() != Some("EUR"))
        {
            return Err(invalid(
                "su moneda no es el euro y hoy solo se registran facturas en euros",
            ));
        }
        let limit = match map.get("max_total") {
            None => None,
            Some(Value::String(limit)) => Some(
                parse_amount("max_total", limit)
                    .map_err(|_| invalid(format!("su máximo «{limit}» no es un importe válido")))?
                    .to_string(),
            ),
            Some(_) => return Err(invalid("su máximo no es un texto de importe")),
        };
        return Ok(Cond::Invoice(InvoiceCond {
            supplier: supplier.to_owned(),
            key: supplier.to_ascii_lowercase(),
            limit,
        }));
    }
    let Some(category) = text_of(map, "category") else {
        return Err(invalid("no indica categoría"));
    };
    let scope = match (map.get("sender"), map.get("sender_domain")) {
        (Some(_), Some(_)) => {
            return Err(invalid(
                "indica a la vez una dirección y un dominio de remitente",
            ))
        }
        (Some(value), None) => match value.as_str().map(str::trim) {
            Some(address) if is_email_address(address) => {
                Scope::Address(address.to_ascii_lowercase())
            }
            _ => return Err(invalid("su remitente no es una dirección de correo")),
        },
        (None, Some(value)) => match value.as_str().map(str::trim) {
            Some(domain) if is_domain(domain) => Scope::Domain(domain.to_ascii_lowercase()),
            _ => return Err(invalid("su dominio de remitente no es válido")),
        },
        (None, None) if marks.any_sender.contains(&rule.id) => Scope::Any,
        (None, None) => return Err(Defect::NoSender),
    };
    Ok(Cond::Send(SendCond {
        category: category.to_owned(),
        scope,
    }))
}

/// Moneda efectiva de una regla de registro: sin `currency` es el euro. `None` si no es un texto.
fn currency_of(rule: &Rule) -> Option<String> {
    match rule.conditions.get("currency") {
        None => Some("EUR".into()),
        Some(Value::String(s)) => Some(s.trim().to_ascii_uppercase()),
        Some(_) => None,
    }
}

/// Nombre sin puntuación, comas ni espacios y en minúsculas: «Alfa SA» y «ALFA, S.A.» dan lo
/// mismo. Solo sirve para preguntar si es el mismo proveedor, nunca para decidir.
fn fold_supplier(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn cmp_limit(a: &str, b: &str) -> Ordering {
    let parse = |s: &str| parse_amount("max_total", s).ok();
    parse(a).cmp(&parse(b))
}

fn domain_of(address: &str) -> &str {
    address.rsplit_once('@').map_or("", |(_, d)| d)
}

fn compare_cond(a: &Cond, b: &Cond) -> Relation {
    match (a, b) {
        (Cond::Invoice(x), Cond::Invoice(y)) => {
            if x.key == y.key {
                match (&x.limit, &y.limit) {
                    (None, None) => Relation::Equal,
                    (None, Some(_)) => Relation::Contains { wider: Side::First },
                    (Some(_), None) => Relation::Contains {
                        wider: Side::Second,
                    },
                    (Some(l), Some(m)) => match cmp_limit(l, m) {
                        Ordering::Equal => Relation::Equal,
                        Ordering::Greater => Relation::Contains { wider: Side::First },
                        Ordering::Less => Relation::Contains {
                            wider: Side::Second,
                        },
                    },
                }
            } else {
                let folded = fold_supplier(&x.supplier);
                if !folded.is_empty() && folded == fold_supplier(&y.supplier) {
                    Relation::Similar
                } else {
                    Relation::Disjoint
                }
            }
        }
        (Cond::Send(x), Cond::Send(y)) => {
            if x.category != y.category {
                return Relation::Disjoint;
            }
            match (&x.scope, &y.scope) {
                (Scope::Any, Scope::Any) => Relation::Equal,
                (Scope::Any, _) => Relation::Contains { wider: Side::First },
                (_, Scope::Any) => Relation::Contains {
                    wider: Side::Second,
                },
                (Scope::Address(p), Scope::Address(q)) | (Scope::Domain(p), Scope::Domain(q)) => {
                    if p == q {
                        Relation::Equal
                    } else {
                        Relation::Disjoint
                    }
                }
                (Scope::Domain(d), Scope::Address(addr)) => {
                    if domain_of(addr) == d {
                        Relation::Contains { wider: Side::First }
                    } else {
                        Relation::Disjoint
                    }
                }
                (Scope::Address(addr), Scope::Domain(d)) => {
                    if domain_of(addr) == d {
                        Relation::Contains {
                            wider: Side::Second,
                        }
                    } else {
                        Relation::Disjoint
                    }
                }
            }
        }
        _ => Relation::NotComparable {
            reason: NotComparable::DifferentKinds,
        },
    }
}

/// Relación entre lo que cubren dos reglas. Simétrica (`compare(b, a)` es `compare(a, b)`
/// vista desde el otro lado) y puramente sobre las condiciones guardadas. Solo se comparan
/// reglas del mismo tipo; un tipo desconocido, unas condiciones inválidas o una regla de
/// envío sin remitente ni validación avanzada (no actúa) no son comparables.
pub fn compare(a: &Rule, b: &Rule, marks: &Marks) -> Relation {
    if a.action_kind != b.action_kind {
        return Relation::NotComparable {
            reason: NotComparable::DifferentKinds,
        };
    }
    if !matches!(a.action_kind.as_str(), REGISTER_INVOICE | SEND_DRAFT) {
        return Relation::NotComparable {
            reason: NotComparable::UnknownKind,
        };
    }
    // Monedas distintas: no se cruzan, aunque alguna regla sea defectuosa por eso mismo.
    if a.action_kind == REGISTER_INVOICE {
        if let (Some(x), Some(y)) = (currency_of(a), currency_of(b)) {
            if x != y {
                return Relation::Disjoint;
            }
        }
    }
    match (parse_cond(a, marks), parse_cond(b, marks)) {
        (Ok(x), Ok(y)) => compare_cond(&x, &y),
        (x, y) => {
            let mut rule_ids: Vec<String> = [(a, x.is_err()), (b, y.is_err())]
                .into_iter()
                .filter(|(_, bad)| *bad)
                .map(|(r, _)| r.id.clone())
                .collect();
            rule_ids.sort();
            Relation::NotComparable {
                reason: NotComparable::Defective { rule_ids },
            }
        }
    }
}

// ---- Hallazgos ----

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingKind {
    /// Las dos reglas cubren exactamente lo mismo.
    Duplicate,
    /// Una cubre todo lo de la otra y más.
    Overlap,
    /// Proveedores con nombre parecido: ¿son el mismo?
    Similar,
    /// Una regla que no actúa (condiciones inválidas, sin remitente o tipo desconocido).
    Defective,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FindingRule {
    pub id: String,
    pub version: i64,
    pub active: bool,
}

/// Qué ocurre, como código y datos: la interfaz lo redacta (`rules-text.ts`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Effect {
    pub code: String,
    pub data: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RuleFinding {
    /// Clave estable del hallazgo («overlap:a+b»).
    pub key: String,
    pub kind: FindingKind,
    pub action_kind: String,
    /// Reglas implicadas, por identificador.
    pub rules: Vec<FindingRule>,
    /// Ambas reglas están activas: el choque ocurre ya. Si no, solo chocarán si se activan.
    /// Una pregunta de nombre parecido nunca es un choque confirmado.
    pub active_conflict: bool,
    pub effect: Effect,
    /// Las reglas se pueden unificar (`Equal` o `Contains`).
    pub merge_candidate: bool,
    /// Otras reglas activas, ajenas al par, que también cubren lo que solo cubre la más amplia
    /// (la parte que el par dejaría automatizar). En esas tareas también van a revisión.
    pub others: Vec<FindingRule>,
    /// Tareas que ya coincidieron con las dos reglas (de `rule_evaluations`); lo rellena
    /// `App::rule_findings`. `None` si no aplica (nombre parecido, regla defectuosa).
    pub affected_tasks: Option<usize>,
}

fn limit_json(limit: &Option<String>) -> Value {
    limit.as_ref().map_or(Value::Null, |l| json!(l))
}

fn scope_json(scope: &Scope) -> Value {
    match scope {
        Scope::Address(a) => json!({ "scope": "sender", "value": a }),
        Scope::Domain(d) => json!({ "scope": "domain", "value": d }),
        Scope::Any => json!({ "scope": "any", "value": null }),
    }
}

/// Datos de la regla más amplia o más estrecha de un solape.
fn side_json(rule: &Rule, cond: &Cond) -> Value {
    match cond {
        Cond::Invoice(c) => json!({ "id": rule.id, "limit": limit_json(&c.limit) }),
        Cond::Send(c) => {
            let mut v = scope_json(&c.scope);
            v["id"] = json!(rule.id);
            v
        }
    }
}

fn supplier_of(cond: &Cond) -> String {
    match cond {
        Cond::Invoice(i) => i.supplier.clone(),
        Cond::Send(_) => String::new(),
    }
}

fn pair_effect(relation: &Relation, a: (&Rule, &Cond), b: (&Rule, &Cond)) -> Effect {
    match (relation, a.1) {
        (Relation::Similar, _) => Effect {
            code: "similar_supplier".into(),
            data: json!({
                "first": { "id": a.0.id, "supplier": supplier_of(a.1) },
                "second": { "id": b.0.id, "supplier": supplier_of(b.1) },
            }),
        },
        (Relation::Equal, Cond::Invoice(c)) => Effect {
            code: "same_conditions".into(),
            data: json!({ "supplier": c.supplier, "limit": limit_json(&c.limit) }),
        },
        (Relation::Equal, Cond::Send(c)) => {
            let mut data = scope_json(&c.scope);
            data["category"] = json!(c.category);
            Effect {
                code: "same_conditions".into(),
                data,
            }
        }
        (Relation::Contains { wider }, cond) => {
            let (w, n) = if *wider == Side::First {
                (a, b)
            } else {
                (b, a)
            };
            let mut data = json!({
                "wider": side_json(w.0, w.1),
                "narrower": side_json(n.0, n.1),
            });
            match cond {
                Cond::Invoice(c) => data["supplier"] = json!(c.supplier),
                Cond::Send(c) => data["category"] = json!(c.category),
            }
            Effect {
                code: "narrow_blocks_wide".into(),
                data,
            }
        }
        _ => Effect {
            code: "none".into(),
            data: json!({}),
        },
    }
}

fn finding_rule(rule: &Rule) -> FindingRule {
    FindingRule {
        id: rule.id.clone(),
        version: rule.version,
        active: rule.active,
    }
}

fn defect_finding(rule: &Rule, defect: Defect) -> RuleFinding {
    let (code, reason) = match defect {
        Defect::UnknownKind => (
            "not_comparable",
            format!(
                "la aplicación no reconoce el tipo de regla «{}», así que no puede compararla ni aplicarla",
                rule.action_kind
            ),
        ),
        Defect::NoSender => (
            "sender_missing",
            "no indica remitente ni tiene validada la opción avanzada: no actúa".to_owned(),
        ),
        Defect::Invalid(text) => ("invalid_conditions", text),
    };
    RuleFinding {
        key: format!("defective:{}", rule.id),
        kind: FindingKind::Defective,
        action_kind: rule.action_kind.clone(),
        rules: vec![finding_rule(rule)],
        active_conflict: false,
        effect: Effect {
            code: code.into(),
            data: json!({ "reason": reason }),
        },
        merge_candidate: false,
        others: Vec::new(),
        affected_tasks: None,
    }
}

fn severity(f: &RuleFinding) -> u8 {
    match (f.kind, f.active_conflict) {
        (FindingKind::Duplicate | FindingKind::Overlap, true) => 0,
        (FindingKind::Duplicate | FindingKind::Overlap, false) => 1,
        (FindingKind::Similar, _) => 2,
        (FindingKind::Defective, _) => 3,
    }
}

/// Reglas activas, ajenas al par `(i, j)`, que cubren algo de lo que solo cubre la más amplia
/// (lo que el par dejaría automatizar). Una tercera regla que queda dentro de la más estrecha
/// no cuenta: esas tareas ya iban a revisión por el par.
fn region_others(
    i: usize,
    j: usize,
    live: &[&Rule],
    parsed: &[Result<Cond, Defect>],
    relation: &Relation,
) -> Vec<FindingRule> {
    let Relation::Contains { wider } = relation else {
        return Vec::new();
    };
    let (w, n) = if *wider == Side::First {
        (i, j)
    } else {
        (j, i)
    };
    let (Ok(wc), Ok(nc)) = (&parsed[w], &parsed[n]) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (k, rule) in live.iter().enumerate() {
        if k == i || k == j || !rule.active || rule.action_kind != live[i].action_kind {
            continue;
        }
        let Ok(kc) = &parsed[k] else { continue };
        // Debe solaparse con la más amplia (igual o contenida en ella, o que la contenga).
        let overlaps_wide = matches!(
            compare_cond(kc, wc),
            Relation::Equal | Relation::Contains { .. }
        );
        // Y no quedar entera dentro de la más estrecha.
        let inside_narrow = matches!(
            compare_cond(kc, nc),
            Relation::Equal
                | Relation::Contains {
                    wider: Side::Second
                }
        );
        if overlaps_wide && !inside_narrow {
            out.push(finding_rule(rule));
        }
    }
    out
}

/// Contradicciones entre las reglas, un hallazgo por par (y uno por regla que no actúa), de
/// más a menos grave: primero los choques con las dos reglas activas. Las archivadas no
/// entran; las inactivas sí, indicando que solo chocarán si se activan.
pub fn analyze(rules: &[Rule], marks: &Marks) -> Vec<RuleFinding> {
    // Las reglas de envío de documentos de venta no se comparan entre sí (D238): si dos coinciden
    // con un envío, `evaluate_with` lo manda a revisión (`SeveralRulesMatch`).
    let mut live: Vec<&Rule> = rules
        .iter()
        .filter(|r| !marks.archived.contains(&r.id) && r.action_kind != SEND_SALES_DOCUMENT)
        .collect();
    live.sort_by(|a, b| a.id.cmp(&b.id));
    let parsed: Vec<Result<Cond, Defect>> = live.iter().map(|r| parse_cond(r, marks)).collect();
    let mut findings = Vec::new();
    for (rule, cond) in live.iter().zip(&parsed) {
        if let Err(defect) = cond {
            findings.push(defect_finding(rule, defect.clone()));
        }
    }
    for i in 0..live.len() {
        for j in i + 1..live.len() {
            let (a, b) = (live[i], live[j]);
            if a.action_kind != b.action_kind {
                continue;
            }
            // Una regla que no actúa no choca con nadie: ya tiene su propio hallazgo.
            let (Ok(ca), Ok(cb)) = (&parsed[i], &parsed[j]) else {
                continue;
            };
            let relation = compare(a, b, marks);
            let (kind, label) = match relation {
                Relation::Equal => (FindingKind::Duplicate, "duplicate"),
                Relation::Contains { .. } => (FindingKind::Overlap, "overlap"),
                Relation::Similar => (FindingKind::Similar, "similar"),
                Relation::Disjoint | Relation::NotComparable { .. } => continue,
            };
            let real = kind != FindingKind::Similar;
            findings.push(RuleFinding {
                key: format!("{label}:{}+{}", a.id, b.id),
                kind,
                action_kind: a.action_kind.clone(),
                rules: vec![finding_rule(a), finding_rule(b)],
                active_conflict: real && a.active && b.active,
                effect: pair_effect(&relation, (a, ca), (b, cb)),
                merge_candidate: real
                    && !has_review_only_category(a)
                    && !has_review_only_category(b),
                others: if kind == FindingKind::Overlap {
                    region_others(i, j, &live, &parsed, &relation)
                } else {
                    Vec::new()
                },
                affected_tasks: None,
            });
        }
    }
    findings.sort_by(|x, y| {
        severity(x)
            .cmp(&severity(y))
            .then_with(|| x.key.cmp(&y.key))
    });
    findings
}

// ---- Unificación ----

/// Una regla de envío guardada con una categoría de solo revisión (D177): nunca se unifica.
fn has_review_only_category(rule: &Rule) -> bool {
    rule.action_kind == SEND_DRAFT
        && rule.conditions["category"]
            .as_str()
            .is_some_and(|c| REVIEW_ONLY_CATEGORIES.contains(&c))
}

/// Por qué no se puede unificar.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum MergeError {
    #[error("seleccione al menos dos reglas para unificar")]
    TooFew,
    #[error("hay reglas repetidas en la selección")]
    Repeated,
    #[error("la regla {0} no existe")]
    Missing(String),
    #[error("la regla {0} está archivada; restáurela antes de unificarla")]
    Archived(String),
    #[error("la regla {0} es de una categoría que siempre va a revisión; no se puede unificar")]
    ReviewOnly(String),
    #[error("solo se pueden unificar reglas del mismo tipo: no mezcle reglas de registrar facturas con reglas de enviar respuestas")]
    MixedKinds,
    #[error("la aplicación no reconoce este tipo de regla y no puede unificarla")]
    UnknownKind,
    #[error("la regla {0} no actúa ({1}); corríjala antes de unificarla")]
    Defective(String, String),
    #[error("«{0}» y «{1}» tienen un nombre de proveedor parecido, pero la aplicación no puede asegurar que sean el mismo; si lo son, edite una de las dos para dejar el nombre igual y después unifíquelas")]
    SimilarSupplier(String, String),
    #[error(
        "las reglas {0} y {1} cubren casos distintos: no se contradicen y no hay nada que unificar"
    )]
    Disjoint(String, String),
    #[error("las reglas tienen condiciones distintas; elija cuáles debe tener la regla nueva")]
    ChoiceRequired(Vec<MergeOption>),
    #[error("la opción «{0}» no corresponde a ninguna de las reglas elegidas")]
    UnknownChoice(String),
}

/// Una condición posible para la regla nueva: el límite, o el remitente, de alguna original.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MergeOption {
    /// `none` o el límite («500.00») en facturas; `any`, `domain:x` o `sender:x` en envíos.
    pub key: String,
    /// Datos para explicarla: `{limit}` o `{scope, value}`.
    pub scope: Value,
    /// Reglas que ya la tienen.
    pub rule_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PlanRule {
    pub id: String,
    pub version: i64,
    pub active: bool,
    pub conditions: Value,
    /// Tiene la validación avanzada: la regla nueva no la hereda.
    pub any_sender: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MergeAfter {
    pub conditions: Value,
    /// Siempre inactiva.
    pub active: bool,
    pub option_key: String,
    /// Originales con la validación avanzada, que la regla nueva no hereda.
    pub loses_any_sender: Vec<String>,
    /// La regla nueva no indica remitente: no actuará hasta validar la opción avanzada.
    pub needs_sender_validation: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SimTask {
    pub task_id: i64,
    pub label: Option<String>,
    /// Total de la factura, si consta.
    pub total: Option<String>,
    /// Regla que la automatiza hoy, si alguna (resultado real de `evaluate_result`).
    pub automated_before_by: Option<String>,
    /// Regla que la automatizaría con la unificación: `(unificada)` es la regla nueva y
    /// cualquier otro id es una regla ajena a la unificación.
    pub automated_after_by: Option<String>,
}

/// Id con el que la regla nueva aparece en `automated_after_by`.
pub const MERGED_RULE: &str = MERGED;

/// Qué cambiaría en las tareas guardadas, evaluadas con `evaluate_result_in_run` (una respuesta con
/// un paso omitido nunca se automatiza, D150, igual que en `finish_task` y en `simulate_rule`), si
/// la regla nueva estuviera activa y las originales archivadas.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MergeSimulation {
    pub considered: usize,
    pub truncated: bool,
    pub skipped_without_result: usize,
    /// Tareas que alguna regla original o la nueva cubre.
    pub touched: usize,
    /// Hoy van a revisión y con la regla nueva se automatizarían.
    pub start_automating: Vec<SimTask>,
    /// Hoy se automatizan y con la regla nueva irían a revisión.
    pub stop_automating: Vec<SimTask>,
    pub unchanged_automated: usize,
    pub unchanged_review: usize,
    /// Facturas cubiertas sin la comprobación local de moneda (tareas anteriores a D127): van
    /// a revisión con cualquier regla.
    pub without_currency_check: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MergePlan {
    pub action_kind: String,
    pub before: Vec<PlanRule>,
    pub options: Vec<MergeOption>,
    pub chosen: String,
    pub after: MergeAfter,
    /// Lo que pasaría con la regla nueva tal como se crea: sin validación avanzada.
    pub simulation: MergeSimulation,
    /// Solo con «cualquier remitente»: lo que pasaría después de validar la opción avanzada
    /// de la regla nueva. Es solo una hipótesis: la validación no se da al unificar.
    pub simulation_if_validated: Option<MergeSimulation>,
    /// Huella de lo que se vio: `merge_rules` rechaza si las reglas cambiaron después.
    pub fingerprint: String,
}

fn scope_key(cond: &Cond) -> (String, Value) {
    match cond {
        Cond::Invoice(c) => (
            c.limit.clone().unwrap_or_else(|| "none".into()),
            json!({ "limit": limit_json(&c.limit) }),
        ),
        Cond::Send(c) => match &c.scope {
            Scope::Any => ("any".into(), scope_json(&c.scope)),
            Scope::Domain(d) => (format!("domain:{d}"), scope_json(&c.scope)),
            Scope::Address(a) => (format!("sender:{a}"), scope_json(&c.scope)),
        },
    }
}

/// Reglas elegidas, validadas para unificar: existentes, vivas, del mismo tipo, con
/// condiciones válidas y todas `Equal` o `Contains` entre sí. Las devuelve ordenadas por id.
fn merge_members<'a>(
    rules: &'a [Rule],
    marks: &Marks,
    ids: &[String],
) -> Result<Vec<(&'a Rule, Cond)>, MergeError> {
    if ids.len() < 2 {
        return Err(MergeError::TooFew);
    }
    let unique: HashSet<&String> = ids.iter().collect();
    if unique.len() != ids.len() {
        return Err(MergeError::Repeated);
    }
    let mut members: Vec<&Rule> = Vec::new();
    for id in ids {
        let rule = rules
            .iter()
            .find(|r| &r.id == id)
            .ok_or_else(|| MergeError::Missing(id.clone()))?;
        if marks.archived.contains(id) {
            return Err(MergeError::Archived(id.clone()));
        }
        if has_review_only_category(rule) {
            return Err(MergeError::ReviewOnly(id.clone()));
        }
        members.push(rule);
    }
    members.sort_by(|a, b| a.id.cmp(&b.id));
    if members
        .iter()
        .any(|r| r.action_kind != members[0].action_kind)
    {
        return Err(MergeError::MixedKinds);
    }
    let mut parsed = Vec::new();
    for rule in &members {
        match parse_cond(rule, marks) {
            Ok(cond) => parsed.push((*rule, cond)),
            Err(Defect::UnknownKind) => return Err(MergeError::UnknownKind),
            Err(Defect::NoSender) => {
                return Err(MergeError::Defective(
                    rule.id.clone(),
                    "no indica remitente".into(),
                ))
            }
            Err(Defect::Invalid(text)) => return Err(MergeError::Defective(rule.id.clone(), text)),
        }
    }
    for i in 0..parsed.len() {
        for j in i + 1..parsed.len() {
            let (a, b) = (&parsed[i], &parsed[j]);
            match compare_cond(&a.1, &b.1) {
                Relation::Equal | Relation::Contains { .. } => {}
                Relation::Similar => {
                    return Err(MergeError::SimilarSupplier(
                        supplier_of(&a.1),
                        supplier_of(&b.1),
                    ))
                }
                _ => return Err(MergeError::Disjoint(a.0.id.clone(), b.0.id.clone())),
            }
        }
    }
    Ok(parsed)
}

/// Las reglas de una unificación tal como están hoy (ordenadas por id), validadas como en
/// `merge_plan`: sirve para mostrar el «antes» aunque aún falte elegir.
pub fn merge_before(
    rules: &[Rule],
    marks: &Marks,
    ids: &[String],
) -> Result<Vec<PlanRule>, MergeError> {
    Ok(merge_members(rules, marks, ids)?
        .iter()
        .map(|(r, _)| plan_rule(r, marks))
        .collect())
}

fn plan_rule(r: &Rule, marks: &Marks) -> PlanRule {
    PlanRule {
        id: r.id.clone(),
        version: r.version,
        active: r.active,
        conditions: r.conditions.clone(),
        any_sender: marks.any_sender.contains(&r.id),
    }
}

fn options_of(members: &[(&Rule, Cond)]) -> Vec<MergeOption> {
    let mut options: Vec<MergeOption> = Vec::new();
    for (rule, cond) in members {
        let (key, scope) = scope_key(cond);
        match options.iter_mut().find(|o| o.key == key) {
            Some(option) => option.rule_ids.push(rule.id.clone()),
            None => options.push(MergeOption {
                key,
                scope,
                rule_ids: vec![rule.id.clone()],
            }),
        }
    }
    options
}

/// Las condiciones que puede tener la regla nueva (una por cada límite o remitente distinto
/// de las originales). Con una sola, no hace falta elegir.
pub fn merge_options(
    rules: &[Rule],
    marks: &Marks,
    ids: &[String],
) -> Result<Vec<MergeOption>, MergeError> {
    Ok(options_of(&merge_members(rules, marks, ids)?))
}

fn merge_fingerprint(
    members: &[(&Rule, Cond)],
    marks: &Marks,
    chosen: &str,
    live_rules: &[Rule],
) -> String {
    let mut parts: Vec<String> = members
        .iter()
        .map(|(r, _)| {
            format!(
                "{}:{}:{}:{}:{}",
                r.id,
                r.version,
                u8::from(r.active),
                u8::from(marks.any_sender.contains(&r.id)),
                r.conditions
            )
        })
        .collect();
    parts.push(format!("choice={chosen}"));
    parts.push(rules_fingerprint(
        live_rules,
        &marks.any_sender,
        &members[0].0.action_kind,
    ));
    let digest = Sha256::digest(parts.join("|").as_bytes());
    digest.iter().take(8).map(|b| format!("{b:02x}")).collect()
}

fn automated_by(decision: &RuleDecision) -> Option<String> {
    match decision {
        RuleDecision::Automate { rule_id, .. } => Some(rule_id.clone()),
        RuleDecision::Review(_) => None,
    }
}

fn sim_task(
    task: &Task,
    result: &Value,
    invoice: bool,
    before: &RuleDecision,
    after: &RuleDecision,
) -> SimTask {
    SimTask {
        automated_before_by: automated_by(before),
        automated_after_by: automated_by(after),
        task_id: task.id,
        label: if invoice {
            result["document"].as_str()
        } else {
            result["message"]["subject"].as_str()
        }
        .map(str::to_owned),
        total: if invoice {
            result["fields"]["total"].as_str().map(str::to_owned)
        } else {
            None
        },
    }
}

/// Propone la regla resultante de unificar `ids` (reglas `Equal` o `Contains` del mismo tipo)
/// y simula con `evaluate_result_in_run` qué cambiaría en `tasks`. Es de solo lectura. Si las
/// condiciones difieren, `chosen` debe ser la clave de una de las opciones (`merge_options`);
/// sin elección es un error claro (`ChoiceRequired`, con las opciones). Rechaza mezclar tipos,
/// proveedores solo parecidos y reglas archivadas.
pub fn merge_plan(
    rules: &[Rule],
    marks: &Marks,
    ids: &[String],
    chosen: Option<&str>,
    tasks: &[Task],
) -> Result<MergePlan, MergeError> {
    merge_plan_in_runs(rules, marks, ids, chosen, tasks, &HashSet::new())
}

/// Como `merge_plan`, sabiendo qué tareas tienen un paso anterior omitido (`step_waived`, D150):
/// su respuesta siempre va a revisión, así que no cuenta como automatizable ni antes ni después.
pub fn merge_plan_in_runs(
    rules: &[Rule],
    marks: &Marks,
    ids: &[String],
    chosen: Option<&str>,
    tasks: &[Task],
    step_waived: &HashSet<i64>,
) -> Result<MergePlan, MergeError> {
    let members = merge_members(rules, marks, ids)?;
    let options = options_of(&members);
    let chosen = match (
        chosen.map(str::trim).filter(|c| !c.is_empty()),
        options.as_slice(),
    ) {
        (Some(key), _) => options
            .iter()
            .find(|o| o.key == key)
            .map(|o| o.key.clone())
            .ok_or_else(|| MergeError::UnknownChoice(key.to_owned()))?,
        (None, [only]) => only.key.clone(),
        (None, _) => return Err(MergeError::ChoiceRequired(options)),
    };
    let action_kind = members[0].0.action_kind.clone();
    let model = members
        .iter()
        .find(|(_, c)| scope_key(c).0 == chosen)
        .map(|(r, _)| *r)
        .expect("la opción elegida sale de una regla");
    let loses: Vec<String> = members
        .iter()
        .filter(|(r, _)| marks.any_sender.contains(&r.id))
        .map(|(r, _)| r.id.clone())
        .collect();
    let needs_sender_validation = action_kind == SEND_DRAFT && chosen == "any";
    let merged_conditions = model.conditions.clone();
    let current: Vec<Rule> = rules
        .iter()
        .filter(|r| !marks.archived.contains(&r.id))
        .cloned()
        .collect();
    let fingerprint = merge_fingerprint(&members, marks, &chosen, &current);

    // Simulación: hoy (reglas vivas tal como están) frente a después (originales fuera y la
    // regla nueva activa, que es lo que ocurrirá cuando el usuario la active).
    let originals: HashSet<&str> = members.iter().map(|(r, _)| r.id.as_str()).collect();
    let mut future: Vec<Rule> = current
        .iter()
        .filter(|r| !originals.contains(r.id.as_str()))
        .cloned()
        .collect();
    future.push(Rule {
        id: MERGED.into(),
        version: 1,
        action_kind: action_kind.clone(),
        conditions: merged_conditions.clone(),
        active: true,
        created_at: 0,
    });
    let future_marks: HashSet<String> = marks
        .any_sender
        .iter()
        .filter(|id| !originals.contains(id.as_str()))
        .cloned()
        .collect();
    let invoice = action_kind == REGISTER_INVOICE;
    let task_kind = if invoice { INVOICE } else { EMAIL };
    let of_kind: Vec<&Task> = tasks.iter().filter(|t| t.kind == task_kind).collect();
    let simulate = |future_marks: &HashSet<String>| {
        simulate_merge(
            (&current, &marks.any_sender),
            (&future, future_marks),
            &originals,
            &action_kind,
            &of_kind,
            step_waived,
        )
    };
    let simulation = simulate(&future_marks);
    let simulation_if_validated = needs_sender_validation.then(|| {
        let mut validated = future_marks.clone();
        validated.insert(MERGED.to_owned());
        simulate(&validated)
    });
    Ok(MergePlan {
        action_kind,
        before: members.iter().map(|(r, _)| plan_rule(r, marks)).collect(),
        options,
        chosen: chosen.clone(),
        after: MergeAfter {
            conditions: merged_conditions,
            active: false,
            option_key: chosen,
            loses_any_sender: loses,
            needs_sender_validation,
        },
        simulation,
        simulation_if_validated,
        fingerprint,
    })
}

/// Evalúa las tareas con las reglas de hoy y con las de después, con `evaluate_result_in_run`:
/// una tarea de `step_waived` (con un paso anterior omitido) nunca se automatiza, como en
/// `finish_task` (D150). Cada grupo lleva la regla que automatiza, para que el texto no deduzca
/// tramos que los datos no sostienen.
fn simulate_merge(
    (current, current_marks): (&[Rule], &HashSet<String>),
    (future, future_marks): (&[Rule], &HashSet<String>),
    originals: &HashSet<&str>,
    action_kind: &str,
    tasks: &[&Task],
    step_waived: &HashSet<i64>,
) -> MergeSimulation {
    let invoice = action_kind == REGISTER_INVOICE;
    let mut simulation = MergeSimulation {
        considered: 0,
        truncated: tasks.len() > MERGE_SIMULATION_LIMIT,
        skipped_without_result: 0,
        touched: 0,
        start_automating: Vec::new(),
        stop_automating: Vec::new(),
        unchanged_automated: 0,
        unchanged_review: 0,
        without_currency_check: 0,
    };
    for task in tasks.iter().take(MERGE_SIMULATION_LIMIT) {
        let Some(result) = task.result.as_ref().filter(|r| r.is_object()) else {
            simulation.skipped_without_result += 1;
            continue;
        };
        simulation.considered += 1;
        let waived = step_waived.contains(&task.id);
        let before = evaluate_result_in_run(current, current_marks, action_kind, result, waived);
        let after = evaluate_result_in_run(future, future_marks, action_kind, result, waived);
        let covered = before
            .explanation
            .candidates
            .iter()
            .any(|c| c.matches && originals.contains(c.id.as_str()))
            || after
                .explanation
                .candidates
                .iter()
                .any(|c| c.matches && c.id == MERGED);
        if !covered {
            continue;
        }
        simulation.touched += 1;
        if invoice && foreign_currencies(result).is_none() {
            simulation.without_currency_check += 1;
        }
        let item = sim_task(task, result, invoice, &before.decision, &after.decision);
        match (
            item.automated_before_by.is_some(),
            item.automated_after_by.is_some(),
        ) {
            (false, true) => simulation.start_automating.push(item),
            (true, false) => simulation.stop_automating.push(item),
            (true, true) => simulation.unchanged_automated += 1,
            (false, false) => simulation.unchanged_review += 1,
        }
    }
    simulation
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rule(action_kind: &str, conditions: Value) -> Rule {
        Rule {
            id: "r".into(),
            version: 2,
            action_kind: action_kind.into(),
            conditions,
            active: true,
            created_at: 0,
        }
    }

    fn invoice() -> Value {
        json!({
            "fields": { "supplier": "Alfa SL", "total": "121.00", "currency": null },
            "currency_evidence": { "foreign": [], "euro": true },
            "issues": [],
            "unreadable": [],
        })
    }

    fn automates(rule: Rule, result: &Value, kind: &str) -> bool {
        matches!(decide(&[rule], kind, result), RuleDecision::Automate { .. })
    }

    #[test]
    fn well_formed_rules_match() {
        let ok = rule(
            REGISTER_INVOICE,
            json!({ "supplier": "alfa sl", "max_total": "200.00" }),
        );
        assert!(automates(ok, &invoice(), REGISTER_INVOICE));
    }

    #[test]
    fn non_text_max_total_never_automates() {
        for limit in [json!(500), json!(500.0), json!(null), json!(["1.00"])] {
            let r = rule(
                REGISTER_INVOICE,
                json!({ "supplier": "Alfa SL", "max_total": limit }),
            );
            assert_eq!(
                decide(&[r], REGISTER_INVOICE, &invoice()),
                RuleDecision::Review("ninguna regla activa coincide".into())
            );
        }
        let text = rule(
            REGISTER_INVOICE,
            json!({ "supplier": "Alfa SL", "max_total": "mucho" }),
        );
        assert!(!automates(text, &invoice(), REGISTER_INVOICE));
    }

    #[test]
    fn unknown_keys_never_automate() {
        let r = rule(
            REGISTER_INVOICE,
            json!({ "supplier": "Alfa SL", "colour": "rojo" }),
        );
        assert!(!automates(r, &invoice(), REGISTER_INVOICE));
        let result = json!({
            "category": "solicitud_factura",
            "message": { "from": "a@example.test", "signals": [] },
            "draft": { "to": ["a@example.test"], "body": "Hola." },
            "draft_flags": [],
        });
        let ok = rule(
            SEND_DRAFT,
            json!({ "category": "solicitud_factura", "sender": "a@example.test" }),
        );
        assert!(automates(ok, &result, SEND_DRAFT));
        let bad = rule(
            SEND_DRAFT,
            json!({ "category": "solicitud_factura", "sender": "a@example.test", "colour": 1 }),
        );
        assert!(!automates(bad, &result, SEND_DRAFT));
    }

    #[test]
    fn non_object_conditions_never_automate() {
        let r = rule(REGISTER_INVOICE, json!("Alfa SL"));
        assert!(!automates(r, &invoice(), REGISTER_INVOICE));
    }

    fn named(id: &str, active: bool, action_kind: &str, conditions: Value) -> Rule {
        Rule {
            id: id.into(),
            active,
            ..rule(action_kind, conditions)
        }
    }

    fn draft(category: &str, to: Value) -> Value {
        json!({
            "category": category,
            "message": { "from": "a@example.test", "signals": [] },
            "draft": { "to": to, "body": "Hola." },
            "draft_flags": [],
        })
    }

    /// Mismas decisiones y textos que tenía `decide` antes de `evaluate` (D125).
    #[test]
    fn decisions_and_texts_are_preserved() {
        let alfa = || {
            named(
                "a",
                true,
                REGISTER_INVOICE,
                json!({ "supplier": "Alfa SL" }),
            )
        };
        let mut with_issues = invoice();
        with_issues["issues"] = json!(["total_mismatch"]);
        let mut unreadable = invoice();
        unreadable["unreadable"] = json!(["date"]);
        let send = || {
            named(
                "s",
                true,
                SEND_DRAFT,
                json!({ "category": "consulta", "sender_domain": "example.test" }),
            )
        };
        let cases: Vec<(Vec<Rule>, &str, Value, RuleDecision)> = vec![
            (
                vec![alfa()],
                REGISTER_INVOICE,
                invoice(),
                RuleDecision::Automate {
                    rule_id: "a".into(),
                    rule_version: 2,
                },
            ),
            (
                vec![],
                REGISTER_INVOICE,
                invoice(),
                RuleDecision::Review("ninguna regla activa coincide".into()),
            ),
            (
                vec![alfa()],
                REGISTER_INVOICE,
                with_issues,
                RuleDecision::Review("la factura tiene incidencias".into()),
            ),
            (
                vec![alfa()],
                REGISTER_INVOICE,
                unreadable,
                RuleDecision::Review("hay campos ilegibles".into()),
            ),
            (
                vec![send()],
                SEND_DRAFT,
                draft("consulta", json!([])),
                RuleDecision::Review("el borrador no tiene destinatario".into()),
            ),
            (
                vec![send()],
                SEND_DRAFT,
                draft("otra", json!(["a@example.test"])),
                RuleDecision::Review("ninguna regla activa coincide".into()),
            ),
            (
                vec![
                    alfa(),
                    named(
                        "b",
                        true,
                        REGISTER_INVOICE,
                        json!({ "supplier": "ALFA SL" }),
                    ),
                ],
                REGISTER_INVOICE,
                invoice(),
                RuleDecision::Review("2 reglas coinciden; la decisión no es inequívoca".into()),
            ),
        ];
        for (rules, kind, result, expected) in cases {
            let evaluation = evaluate(&rules, kind, &result);
            assert_eq!(evaluation.decision, expected);
            assert_eq!(decide(&rules, kind, &result), expected);
            if let RuleDecision::Review(text) = &expected {
                assert_eq!(&evaluation.explanation.summary(), text);
            }
        }
    }

    #[test]
    fn shortcut_explanations_keep_their_texts() {
        assert_eq!(
            Explanation::split_invoice().summary(),
            "el documento contiene varias facturas; la IA las separó y cada una se revisa por separado"
        );
        assert_eq!(
            Explanation::amount_review("el total no se lee sin duda").summary(),
            "el total no se lee sin duda"
        );
        assert_eq!(
            serde_json::to_value(Explanation::split_invoice().code).unwrap(),
            json!("split_invoice")
        );
    }

    #[test]
    fn inactive_rule_would_apply_but_never_automates() {
        let off = named(
            "off",
            false,
            REGISTER_INVOICE,
            json!({ "supplier": "Alfa SL" }),
        );
        let evaluation = evaluate(&[off], REGISTER_INVOICE, &invoice());
        assert_eq!(
            evaluation.decision,
            RuleDecision::Review("ninguna regla activa coincide".into())
        );
        let would = evaluation.explanation.inactive_would_apply();
        assert_eq!(would.len(), 1);
        assert_eq!(would[0].id, "off");
        assert!(would[0].matches && !would[0].active);
    }

    #[test]
    fn several_matches_list_every_candidate_with_concrete_mismatches() {
        let rules = vec![
            named(
                "a",
                true,
                REGISTER_INVOICE,
                json!({ "supplier": "Alfa SL" }),
            ),
            named(
                "b",
                true,
                REGISTER_INVOICE,
                json!({ "supplier": "Alfa SL", "max_total": "500.00" }),
            ),
            named(
                "c",
                true,
                REGISTER_INVOICE,
                json!({ "supplier": "Beta SL" }),
            ),
            named(
                "d",
                true,
                REGISTER_INVOICE,
                json!({ "supplier": "Alfa SL", "max_total": "100.00" }),
            ),
            named("e", true, SEND_DRAFT, json!({ "category": "consulta" })),
        ];
        let evaluation = evaluate(&rules, REGISTER_INVOICE, &invoice());
        let e = &evaluation.explanation;
        assert_eq!(e.code, ReasonCode::SeveralRulesMatch);
        assert_eq!(e.facts["matching"], 2);
        assert_eq!(e.facts["rule_ids"], json!(["a", "b"]));
        assert_eq!(
            e.candidates.len(),
            4,
            "solo reglas del mismo tipo de acción"
        );
        let by_id = |id: &str| e.candidates.iter().find(|c| c.id == id).unwrap();
        assert!(by_id("a").matches && by_id("b").matches);
        assert_eq!(
            by_id("c").mismatch.as_deref(),
            Some("el proveedor es «Alfa SL» y la regla pide «Beta SL»")
        );
        assert_eq!(
            by_id("d").mismatch.as_deref(),
            Some("el total 121.00 supera el máximo 100.00")
        );
    }

    #[test]
    fn explanation_serializes_without_mail_content() {
        let r = named(
            "s",
            true,
            SEND_DRAFT,
            json!({ "category": "consulta", "sender": "A@Example.test" }),
        );
        let mut result = draft("consulta", json!(["a@example.test"]));
        result["draft"]["body"] = json!("TEXTO PRIVADO");
        let value = serde_json::to_value(evaluate(&[r], SEND_DRAFT, &result).explanation).unwrap();
        assert_eq!(value["code"], "automated");
        assert_eq!(value["facts"], json!({ "rule_id": "s", "rule_version": 2 }));
        assert!(!value.to_string().contains("TEXTO PRIVADO"));
        assert!(!value.to_string().contains("a@example.test"));
    }

    fn mail(from: Option<&str>) -> Value {
        let mut result = json!({
            "category": "solicitud_factura",
            "message": { "signals": [] },
            "draft": { "to": ["a@example.test"], "body": "Hola." },
            "draft_flags": [],
        });
        if let Some(from) = from {
            result["message"]["from"] = json!(from);
        }
        result
    }

    fn send_rule(conditions: Value) -> Rule {
        named("s", true, SEND_DRAFT, conditions)
    }

    #[test]
    fn sender_address_is_exact_and_ignores_case() {
        let r =
            || send_rule(json!({ "category": "solicitud_factura", "sender": "Pedidos@Alfa.test" }));
        assert!(automates(r(), &mail(Some("pedidos@alfa.TEST")), SEND_DRAFT));
        for from in [
            Some("otro@alfa.test"),
            Some("pedidos@mail.alfa.test"),
            Some("xpedidos@alfa.test"),
            Some(""),
            None,
        ] {
            assert!(!automates(r(), &mail(from), SEND_DRAFT), "{from:?}");
        }
        let missing = evaluate(&[r()], SEND_DRAFT, &mail(None)).explanation;
        assert_eq!(
            missing.candidates[0].mismatch.as_deref(),
            Some("el correo no indica remitente y la regla pide «Pedidos@Alfa.test»")
        );
    }

    #[test]
    fn sender_domain_is_exact_without_subdomains() {
        let r =
            || send_rule(json!({ "category": "solicitud_factura", "sender_domain": "Alfa.test" }));
        assert!(automates(r(), &mail(Some("x@ALFA.test")), SEND_DRAFT));
        for from in [
            Some("x@mail.alfa.test"),
            Some("x@alfa.test.evil.test"),
            Some("x@xalfa.test"),
            Some("sin-arroba"),
            None,
        ] {
            assert!(!automates(r(), &mail(from), SEND_DRAFT), "{from:?}");
        }
    }

    #[test]
    fn address_and_domain_together_never_automate() {
        let r = send_rule(json!({
            "category": "solicitud_factura",
            "sender": "a@alfa.test",
            "sender_domain": "alfa.test",
        }));
        assert!(!automates(r, &mail(Some("a@alfa.test")), SEND_DRAFT));
    }

    #[test]
    fn send_rule_without_sender_needs_the_advanced_validation() {
        let r = || send_rule(json!({ "category": "solicitud_factura" }));
        let result = mail(Some("cualquiera@example.test"));
        let evaluation = evaluate(&[r()], SEND_DRAFT, &result);
        assert!(matches!(evaluation.decision, RuleDecision::Review(_)));
        assert_eq!(evaluation.explanation.code, ReasonCode::SenderRequired);
        assert_eq!(
            evaluation.explanation.summary(),
            "la regla no indica remitente; añádalo o valide la opción avanzada (regla: s)"
        );
        assert!(evaluation.explanation.candidates[0].sender_required);
        // Con la validación avanzada de esa regla, y solo de esa, automatiza.
        let ack: HashSet<String> = ["s".to_owned()].into();
        let acked = evaluate_with(&[r()], &ack, SEND_DRAFT, &result);
        assert!(matches!(acked.decision, RuleDecision::Automate { .. }));
        let other: HashSet<String> = ["otra".to_owned()].into();
        assert!(matches!(
            evaluate_with(&[r()], &other, SEND_DRAFT, &result).decision,
            RuleDecision::Review(_)
        ));
        // Otra categoría: el motivo es la categoría, no el remitente.
        let mut other_category = result.clone();
        other_category["category"] = json!("otros");
        assert_eq!(
            evaluate(&[r()], SEND_DRAFT, &other_category)
                .explanation
                .code,
            ReasonCode::NoActiveRuleMatches
        );
    }

    fn with_foreign(codes: &[&str]) -> Value {
        let mut result = invoice();
        result["currency_evidence"]["foreign"] = json!(codes);
        result
    }

    fn why(rule: Rule, result: &Value) -> Option<String> {
        evaluate(&[rule], REGISTER_INVOICE, result)
            .explanation
            .candidates[0]
            .mismatch
            .clone()
    }

    #[test]
    fn a_limit_does_not_apply_to_a_document_that_mentions_another_currency() {
        let limited = || {
            rule(
                REGISTER_INVOICE,
                json!({ "supplier": "Alfa SL", "max_total": "200.00" }),
            )
        };
        // Con el euro a la vista y sin otra moneda, coincide y automatiza.
        assert!(automates(limited(), &invoice(), REGISTER_INVOICE));
        assert_eq!(
            why(limited(), &with_foreign(&["USD"])).as_deref(),
            Some("el documento menciona USD y el límite está en EUR")
        );
        assert_eq!(
            why(limited(), &with_foreign(&["GBP", "USD"])).as_deref(),
            Some("el documento menciona GBP, USD y el límite está en EUR")
        );
        // Una moneda explícita distinta de EUR cuenta igual.
        let mut explicit = invoice();
        explicit["fields"]["currency"] = json!("USD");
        assert!(!automates(limited(), &explicit, REGISTER_INVOICE));
        explicit["fields"]["currency"] = json!("EUR");
        assert!(automates(limited(), &explicit, REGISTER_INVOICE));
    }

    #[test]
    fn a_foreign_currency_blocks_any_rule_with_its_own_reason() {
        let free = || rule(REGISTER_INVOICE, json!({ "supplier": "Alfa SL" }));
        let blocked = evaluate(&[free()], REGISTER_INVOICE, &with_foreign(&["USD"]));
        assert!(matches!(blocked.decision, RuleDecision::Review(_)));
        assert_eq!(blocked.explanation.code, ReasonCode::ForeignCurrency);
        assert_eq!(
            blocked.explanation.summary(),
            "el documento menciona USD; hoy la aplicación solo registra facturas en euros"
        );
        // La regla sí coincide: el bloqueo es de la factura, no de la regla.
        assert!(blocked.explanation.candidates[0].matches);
        assert!(automates(free(), &invoice(), REGISTER_INVOICE));
    }

    #[test]
    fn a_task_without_the_currency_check_fails_closed() {
        let mut old = invoice();
        old.as_object_mut().unwrap().remove("currency_evidence");
        for conditions in [
            json!({ "supplier": "Alfa SL" }),
            json!({ "supplier": "Alfa SL", "max_total": "500.00" }),
        ] {
            let e = evaluate(
                &[rule(REGISTER_INVOICE, conditions)],
                REGISTER_INVOICE,
                &old,
            );
            assert!(matches!(e.decision, RuleDecision::Review(_)));
            assert_eq!(e.explanation.code, ReasonCode::CurrencyNotChecked);
            assert_eq!(
                e.explanation.summary(),
                "no consta la comprobación de moneda; reprocese o revise"
            );
        }
        old["currency_evidence"] = json!("EUR");
        assert!(!automates(
            rule(REGISTER_INVOICE, json!({ "supplier": "Alfa SL" })),
            &old,
            REGISTER_INVOICE
        ));
    }

    #[test]
    fn a_rule_currency_can_only_be_the_euro() {
        let eur = rule(
            REGISTER_INVOICE,
            json!({ "supplier": "Alfa SL", "currency": "EUR" }),
        );
        assert!(automates(eur, &invoice(), REGISTER_INVOICE));
        for bad in [json!("USD"), json!("eur"), json!(978)] {
            let r = rule(
                REGISTER_INVOICE,
                json!({ "supplier": "Alfa SL", "currency": bad }),
            );
            assert!(!automates(r, &invoice(), REGISTER_INVOICE));
        }
    }

    #[test]
    fn address_and_domain_formats() {
        assert!(is_email_address("pedidos+a@proveedor-alfa.example"));
        for bad in [
            "",
            "a",
            "a@b",
            "@alfa.test",
            "a b@alfa.test",
            "a@@alfa.test",
            "<a@alfa.test>",
            "a@alfa.test,b@alfa.test",
            "a@-alfa.test",
        ] {
            assert!(!is_email_address(bad), "{bad}");
        }
        assert!(is_domain("alfa.test"));
        for bad in [
            "alfa",
            "@alfa.test",
            "*.alfa.test",
            "alfa..test",
            "alfa.test.",
        ] {
            assert!(!is_domain(bad), "{bad}");
        }
    }

    #[test]
    fn an_empty_supplier_in_the_rule_never_matches_an_invoice_without_supplier() {
        let mut no_supplier = invoice();
        no_supplier["fields"]["supplier"] = json!(null);
        for stored in ["", "  "] {
            let r = rule(REGISTER_INVOICE, json!({ "supplier": stored }));
            assert!(!automates(r.clone(), &no_supplier, REGISTER_INVOICE));
            assert!(!automates(r.clone(), &invoice(), REGISTER_INVOICE));
            assert_eq!(
                why(r, &no_supplier).as_deref(),
                Some("la regla tiene un proveedor vacío")
            );
        }
        let mut blank = invoice();
        blank["fields"]["supplier"] = json!("   ");
        let r = rule(REGISTER_INVOICE, json!({ "supplier": "  " }));
        assert!(!automates(r, &blank, REGISTER_INVOICE));
    }

    /// Evidencia de moneda con el euro o sin él, y con la página leída por OCR o no.
    fn evidence(euro: bool, ocr: bool) -> Value {
        let mut r = invoice();
        r["currency_evidence"] = json!({ "foreign": [], "euro": euro, "ocr": ocr });
        r
    }

    #[test]
    fn a_document_that_does_not_show_the_euro_goes_to_review() {
        for conditions in [
            json!({ "supplier": "Alfa SL" }),
            json!({ "supplier": "Alfa SL", "max_total": "500.00" }),
        ] {
            let r = || rule(REGISTER_INVOICE, conditions.clone());
            // Sin marca de euro, en texto digital o en OCR, no se puede comprobar la moneda.
            for ocr in [false, true] {
                let e = evaluate(&[r()], REGISTER_INVOICE, &evidence(false, ocr));
                assert_eq!(
                    e.explanation.code,
                    ReasonCode::CurrencyNotShown,
                    "ocr={ocr}"
                );
                assert_eq!(
                    e.explanation.summary(),
                    "el documento no muestra la moneda; no se puede comprobar que sea en euros"
                );
                assert!(matches!(e.decision, RuleDecision::Review(_)));
                // La regla sí coincide: el bloqueo es de la factura.
                assert!(e.explanation.candidates[0].matches);
                assert!(automates(r(), &evidence(true, ocr), REGISTER_INVOICE));
            }
        }
        // Otra moneda mencionada manda sobre la falta del euro.
        let mut both = evidence(false, true);
        both["currency_evidence"]["foreign"] = json!(["USD"]);
        let e = evaluate(
            &[rule(REGISTER_INVOICE, json!({ "supplier": "Alfa SL" }))],
            REGISTER_INVOICE,
            &both,
        );
        assert_eq!(e.explanation.code, ReasonCode::ForeignCurrency);
        // El euro junto a otra moneda no automatiza: queda la extranjera.
        let mut mixed = evidence(true, false);
        mixed["currency_evidence"]["foreign"] = json!(["USD"]);
        assert!(!automates(
            rule(REGISTER_INVOICE, json!({ "supplier": "Alfa SL" })),
            &mixed,
            REGISTER_INVOICE
        ));
    }

    #[test]
    fn malformed_currency_evidence_is_not_checked() {
        let free = || rule(REGISTER_INVOICE, json!({ "supplier": "Alfa SL" }));
        for bad in [
            json!({}),
            json!({ "euro": true }),
            json!({ "foreign": "USD", "euro": true }),
            json!({ "foreign": null, "euro": true }),
            json!({ "foreign": [1, "USD"], "euro": true }),
            json!([]),
            json!("EUR"),
        ] {
            let mut r = invoice();
            r["currency_evidence"] = bad.clone();
            assert!(foreign_currencies(&r).is_none(), "{bad}");
            let e = evaluate(&[free()], REGISTER_INVOICE, &r);
            assert_eq!(e.explanation.code, ReasonCode::CurrencyNotChecked, "{bad}");
        }
        // Una lista vacía y válida sí está comprobada.
        assert_eq!(foreign_currencies(&invoice()), Some(vec![]));
    }

    #[test]
    fn a_sender_that_is_not_one_simple_address_never_matches_a_domain_rule() {
        let r =
            || send_rule(json!({ "category": "solicitud_factura", "sender_domain": "alfa.test" }));
        assert!(automates(r(), &mail(Some("x@alfa.test")), SEND_DRAFT));
        for from in [
            "x@evil.test,y@alfa.test",
            "x@evil.test y@alfa.test",
            "<y@alfa.test>",
            "Nombre <y@alfa.test>",
        ] {
            assert!(!automates(r(), &mail(Some(from)), SEND_DRAFT), "{from}");
        }
    }

    #[test]
    fn shortcuts_keep_their_code_and_list_the_matching_rules() {
        let r = named(
            "a",
            true,
            REGISTER_INVOICE,
            json!({ "supplier": "Alfa SL" }),
        );
        let none = HashSet::new();
        let mut doubtful = invoice();
        doubtful["amount_review"] = json!("el total no se lee sin duda");
        let e = evaluate_result(std::slice::from_ref(&r), &none, REGISTER_INVOICE, &doubtful);
        assert_eq!(e.explanation.code, ReasonCode::AmountReview);
        assert!(matches!(e.decision, RuleDecision::Review(_)));
        assert!(e.explanation.candidates[0].matches);
        let mut split = invoice();
        split["part"] = json!({ "index": 2, "of": 3 });
        let e = evaluate_result(&[r], &none, REGISTER_INVOICE, &split);
        assert_eq!(e.explanation.code, ReasonCode::SplitInvoice);
        assert_eq!(e.explanation.candidates.len(), 1);
    }

    #[test]
    fn public_mail_domains_are_recognised() {
        for domain in ["gmail.com", "Outlook.com", " hotmail.com ", "yahoo.com"] {
            assert!(is_public_mail_domain(domain), "{domain}");
        }
        for domain in ["alfa.test", "mail.gmail.com", "gmail.com.evil.test", ""] {
            assert!(!is_public_mail_domain(domain), "{domain}");
        }
    }

    #[test]
    fn fingerprint_changes_with_versions_state_and_validation() {
        let a = rule(REGISTER_INVOICE, json!({ "supplier": "Alfa SL" }));
        let none = HashSet::new();
        let base = rules_fingerprint(std::slice::from_ref(&a), &none, REGISTER_INVOICE);
        assert_eq!(
            base,
            rules_fingerprint(std::slice::from_ref(&a), &none, REGISTER_INVOICE)
        );
        let mut newer = a.clone();
        newer.version += 1;
        assert_ne!(base, rules_fingerprint(&[newer], &none, REGISTER_INVOICE));
        let mut off = a.clone();
        off.active = false;
        assert_ne!(base, rules_fingerprint(&[off], &none, REGISTER_INVOICE));
        let ack: HashSet<String> = ["r".to_owned()].into();
        assert_ne!(base, rules_fingerprint(&[a], &ack, REGISTER_INVOICE));
        // Las reglas de otro tipo no cambian la huella.
        let other = rule(SEND_DRAFT, json!({ "category": "otros" }));
        let a2 = rule(REGISTER_INVOICE, json!({ "supplier": "Alfa SL" }));
        assert_eq!(
            base,
            rules_fingerprint(&[a2, other], &none, REGISTER_INVOICE)
        );
    }
}

#[cfg(test)]
mod conflict_tests {
    use super::*;
    use nestadmin_core::TaskState;
    use serde_json::json;

    fn rule(id: &str, kind: &str, conditions: Value, active: bool) -> Rule {
        Rule {
            id: id.into(),
            version: 1,
            action_kind: kind.into(),
            conditions,
            active,
            created_at: 0,
        }
    }

    fn inv(id: &str, supplier: &str, max: Option<&str>, active: bool) -> Rule {
        let mut c = json!({ "supplier": supplier });
        if let Some(max) = max {
            c["max_total"] = json!(max);
        }
        rule(id, REGISTER_INVOICE, c, active)
    }

    fn send(id: &str, conditions: Value, active: bool) -> Rule {
        let mut c = json!({ "category": "solicitud_factura" });
        for (k, v) in conditions.as_object().unwrap() {
            c[k] = v.clone();
        }
        rule(id, SEND_DRAFT, c, active)
    }

    fn marks(any: &[&str], archived: &[&str]) -> Marks {
        Marks {
            any_sender: any.iter().map(|s| (*s).to_owned()).collect(),
            archived: archived.iter().map(|s| (*s).to_owned()).collect(),
        }
    }

    fn rel(a: &Rule, b: &Rule, m: &Marks) -> Relation {
        let forward = compare(a, b, m);
        assert_eq!(compare(b, a, m), forward.clone().mirrored(), "simetría");
        forward
    }

    fn ids(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn invoice_relations_follow_the_supplier_and_the_limit() {
        let m = Marks::default();
        let wide = inv("a", "Alfa SL", None, true);
        let narrow = inv("b", "Alfa SL", Some("500.00"), true);
        assert_eq!(
            rel(&wide, &narrow, &m),
            Relation::Contains { wider: Side::First }
        );
        assert_eq!(
            rel(&narrow, &wide, &m),
            Relation::Contains {
                wider: Side::Second
            }
        );
        // Mismo proveedor (sin mayúsculas ni espacios en los bordes) y mismo límite escrito
        // de otra forma: iguales.
        assert_eq!(
            rel(
                &inv("a", " alfa sl ", Some("1.000,00"), true),
                &inv("b", "ALFA SL", Some("1000.00"), true),
                &m
            ),
            Relation::Equal
        );
        assert_eq!(
            rel(&wide, &inv("c", "alfa sl", None, true), &m),
            Relation::Equal
        );
        // Límites distintos: contiene la de límite mayor.
        assert_eq!(
            rel(
                &inv("a", "Alfa SL", Some("1000.00"), true),
                &inv("b", "Alfa SL", Some("500.00"), true),
                &m
            ),
            Relation::Contains { wider: Side::First }
        );
        // Otro proveedor: no se cruzan.
        assert_eq!(
            rel(&wide, &inv("c", "Beta SA", None, true), &m),
            Relation::Disjoint
        );
    }

    #[test]
    fn a_similar_name_is_only_a_question_and_never_equal() {
        let m = Marks::default();
        for (x, y) in [
            ("Alfa SA", "ALFA, S.A."),
            ("Alfa S.L.", "alfa sl"),
            ("Alfa  SL", "Alfa-SL"),
        ] {
            for limit in [None, Some("500.00")] {
                let relation = rel(&inv("a", x, limit, true), &inv("b", y, limit, true), &m);
                assert_eq!(relation, Relation::Similar, "{x} / {y}");
                assert_ne!(relation, Relation::Equal);
            }
        }
        // Parecidos con límites distintos siguen siendo una pregunta, no un contenido.
        assert_eq!(
            rel(
                &inv("a", "Alfa SA", None, true),
                &inv("b", "ALFA, S.A.", Some("500.00"), true),
                &m
            ),
            Relation::Similar
        );
        assert_eq!(
            rel(
                &inv("a", "Alfa SL", None, true),
                &inv("b", "Alfa Sur", None, true),
                &m
            ),
            Relation::Disjoint
        );
    }

    #[test]
    fn rules_in_different_currencies_never_cross() {
        let m = Marks::default();
        let eur = rule(
            "a",
            REGISTER_INVOICE,
            json!({ "supplier": "Alfa SL", "currency": "EUR" }),
            true,
        );
        let usd = rule(
            "b",
            REGISTER_INVOICE,
            json!({ "supplier": "Alfa SL", "currency": "USD" }),
            true,
        );
        let plain = inv("c", "Alfa SL", None, true);
        assert_eq!(rel(&eur, &usd, &m), Relation::Disjoint);
        assert_eq!(rel(&plain, &usd, &m), Relation::Disjoint);
        // Sin moneda es euro: igual que la que lo dice.
        assert_eq!(rel(&eur, &plain, &m), Relation::Equal);
        // Dos reglas en dólares no actúan (D127): no comparables, con las dos citadas.
        let usd2 = rule(
            "d",
            REGISTER_INVOICE,
            json!({ "supplier": "Alfa SL", "currency": "USD" }),
            true,
        );
        assert_eq!(
            rel(&usd, &usd2, &m),
            Relation::NotComparable {
                reason: NotComparable::Defective {
                    rule_ids: ids(&["b", "d"])
                }
            }
        );
    }

    #[test]
    fn send_relations_follow_the_sender_scope() {
        let m = marks(&["any"], &[]);
        let address = send("a", json!({ "sender": "ana@alfa.test" }), true);
        let same = send("b", json!({ "sender": "ANA@alfa.test" }), true);
        let other_address = send("c", json!({ "sender": "luis@alfa.test" }), true);
        let domain = send("d", json!({ "sender_domain": "alfa.test" }), true);
        let sub = send("e", json!({ "sender_domain": "mail.alfa.test" }), true);
        let other_domain = send("f", json!({ "sender_domain": "beta.test" }), true);
        let any = send("any", json!({}), true);
        let any2 = send("any2", json!({}), true);
        let m2 = marks(&["any", "any2"], &[]);

        assert_eq!(rel(&address, &same, &m), Relation::Equal);
        assert_eq!(rel(&address, &other_address, &m), Relation::Disjoint);
        assert_eq!(
            rel(&domain, &address, &m),
            Relation::Contains { wider: Side::First }
        );
        assert_eq!(
            rel(&address, &domain, &m),
            Relation::Contains {
                wider: Side::Second
            }
        );
        // Subdominios: el dominio es exacto.
        assert_eq!(rel(&domain, &sub, &m), Relation::Disjoint);
        assert_eq!(rel(&domain, &other_domain, &m), Relation::Disjoint);
        assert_eq!(
            rel(&address, &other_domain, &m),
            Relation::Disjoint,
            "una dirección no está en un dominio ajeno"
        );
        assert_eq!(
            rel(
                &domain,
                &send("d2", json!({ "sender_domain": "ALFA.test" }), true),
                &m
            ),
            Relation::Equal
        );
        // «Cualquier remitente» (con marca avanzada) contiene a todo lo demás.
        assert_eq!(
            rel(&any, &domain, &m),
            Relation::Contains { wider: Side::First }
        );
        assert_eq!(
            rel(&address, &any, &m),
            Relation::Contains {
                wider: Side::Second
            }
        );
        assert_eq!(rel(&any, &any2, &m2), Relation::Equal);
        // Otra categoría: no se cruzan.
        let mut other_category = send("g", json!({ "sender": "ana@alfa.test" }), true);
        other_category.conditions["category"] = json!("consulta_pago");
        assert_eq!(rel(&address, &other_category, &m), Relation::Disjoint);
    }

    #[test]
    fn a_send_rule_without_sender_or_mark_does_not_act_and_cannot_be_compared() {
        let m = Marks::default();
        let missing = send("a", json!({}), true);
        let domain = send("b", json!({ "sender_domain": "alfa.test" }), true);
        assert_eq!(
            rel(&missing, &domain, &m),
            Relation::NotComparable {
                reason: NotComparable::Defective {
                    rule_ids: ids(&["a"])
                }
            }
        );
        // Con la marca avanzada sí actúa y contiene al dominio.
        assert_eq!(
            rel(&missing, &domain, &marks(&["a"], &[])),
            Relation::Contains { wider: Side::First }
        );
    }

    #[test]
    fn invalid_conditions_different_kinds_and_unknown_kinds_are_not_comparable() {
        let m = Marks::default();
        let good = inv("a", "Alfa SL", None, true);
        for bad in [
            rule("b", REGISTER_INVOICE, json!("texto"), true),
            rule("b", REGISTER_INVOICE, json!({}), true),
            rule(
                "b",
                REGISTER_INVOICE,
                json!({ "supplier": "Alfa SL", "max_total": 500 }),
                true,
            ),
            rule(
                "b",
                REGISTER_INVOICE,
                json!({ "supplier": "Alfa SL", "max_total": "mucho" }),
                true,
            ),
            rule(
                "b",
                REGISTER_INVOICE,
                json!({ "supplier": "Alfa SL", "color": "azul" }),
                true,
            ),
        ] {
            assert_eq!(
                rel(&good, &bad, &m),
                Relation::NotComparable {
                    reason: NotComparable::Defective {
                        rule_ids: ids(&["b"])
                    }
                },
                "{:?}",
                bad.conditions
            );
        }
        assert_eq!(
            rel(
                &good,
                &send("s", json!({ "sender_domain": "alfa.test" }), true),
                &m
            ),
            Relation::NotComparable {
                reason: NotComparable::DifferentKinds
            }
        );
        let future = rule("x", "emit_recurring", json!({ "client": "Alfa" }), true);
        let future2 = rule("y", "emit_recurring", json!({ "client": "Alfa" }), true);
        assert_eq!(
            rel(&future, &future2, &m),
            Relation::NotComparable {
                reason: NotComparable::UnknownKind
            }
        );
    }

    #[test]
    fn compare_is_symmetric_over_a_mixed_pool() {
        let m = marks(&["any"], &[]);
        let pool = vec![
            inv("i1", "Alfa SL", None, true),
            inv("i2", "alfa sl", Some("500.00"), false),
            inv("i3", "ALFA, S.L.", Some("500.00"), true),
            inv("i4", "Beta SA", None, true),
            rule(
                "i5",
                REGISTER_INVOICE,
                json!({ "supplier": "Alfa SL", "currency": "USD" }),
                true,
            ),
            rule("i6", REGISTER_INVOICE, json!(7), true),
            send("s1", json!({ "sender": "ana@alfa.test" }), true),
            send("s2", json!({ "sender_domain": "alfa.test" }), true),
            send("any", json!({}), true),
            send("s3", json!({}), true),
            rule("u1", "emit_recurring", json!({}), true),
        ];
        for a in &pool {
            for b in &pool {
                let forward = compare(a, b, &m);
                assert_eq!(
                    compare(b, a, &m),
                    forward.clone().mirrored(),
                    "{} / {}",
                    a.id,
                    b.id
                );
                if a.id == b.id {
                    continue;
                }
                if matches!(forward, Relation::Similar) {
                    assert_ne!(forward, Relation::Equal);
                }
            }
        }
    }

    #[test]
    fn analyze_gives_one_finding_per_pair_and_skips_archived_rules() {
        let rules = vec![
            inv("a", "Alfa SL", None, true),
            inv("b", "Alfa SL", Some("500.00"), true),
            inv("c", "Alfa SL", Some("500.00"), true),
            inv("old", "Alfa SL", None, true),
            inv("z", "Beta SA", None, true),
        ];
        let findings = analyze(&rules, &marks(&[], &["old"]));
        // a⊃b, a⊃c y b=c: tres pares, uno cada uno; nada de `old` ni de Beta.
        let keys: Vec<&str> = findings.iter().map(|f| f.key.as_str()).collect();
        assert_eq!(keys.len(), 3, "{keys:?}");
        assert!(keys.contains(&"duplicate:b+c"));
        assert!(keys.contains(&"overlap:a+b"));
        assert!(keys.contains(&"overlap:a+c"));
        let unique: HashSet<&&str> = keys.iter().collect();
        assert_eq!(unique.len(), keys.len());
        assert!(findings
            .iter()
            .all(|f| f.active_conflict && f.merge_candidate));
        assert!(findings
            .iter()
            .all(|f| f.rules.iter().all(|r| r.id != "old" && r.id != "z")));
    }

    #[test]
    fn analyze_flags_inactive_pairs_and_orders_by_gravity() {
        let rules = vec![
            inv("a", "Alfa SL", None, true),
            inv("b", "Alfa SL", Some("500.00"), false),
            inv("c", "Gama SL", None, true),
            inv("d", "Gama SL", None, true),
            inv("e", "Delta SA", None, true),
            inv("f", "DELTA, S.A.", None, true),
            send("m", json!({}), true),
        ];
        let findings = analyze(&rules, &Marks::default());
        let order: Vec<(&str, bool)> = findings
            .iter()
            .map(|f| (f.key.as_str(), f.active_conflict))
            .collect();
        assert_eq!(
            order,
            vec![
                ("duplicate:c+d", true),
                ("overlap:a+b", false),
                ("similar:e+f", false),
                ("defective:m", false),
            ]
        );
        let inactive = &findings[1];
        assert!(inactive.rules.iter().any(|r| !r.active));
        assert_eq!(inactive.effect.code, "narrow_blocks_wide");
        assert_eq!(inactive.effect.data["wider"]["limit"], Value::Null);
        assert_eq!(inactive.effect.data["narrower"]["limit"], "500.00");
        assert_eq!(inactive.effect.data["supplier"], "Alfa SL");
        // La pregunta de nombre parecido no se puede unificar ni es un choque.
        assert!(!findings[2].merge_candidate);
        assert_eq!(findings[2].effect.code, "similar_supplier");
        // Una regla de envío sin remitente: pendiente de corregir, no una contradicción.
        assert_eq!(findings[3].kind, FindingKind::Defective);
        assert_eq!(findings[3].effect.code, "sender_missing");
    }

    #[test]
    fn analyze_does_not_cross_kinds_and_reports_unknown_kinds() {
        let rules = vec![
            inv("a", "Alfa SL", None, true),
            send("s", json!({ "sender_domain": "alfa.test" }), true),
            rule("u", "emit_recurring", json!({}), true),
        ];
        let findings = analyze(&rules, &Marks::default());
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].key, "defective:u");
        assert_eq!(findings[0].effect.code, "not_comparable");
    }

    #[test]
    fn analyze_describes_a_sender_overlap_with_concrete_data() {
        let rules = vec![
            send("dom", json!({ "sender_domain": "alfa.test" }), true),
            send("dir", json!({ "sender": "ana@alfa.test" }), true),
        ];
        let findings = analyze(&rules, &Marks::default());
        assert_eq!(findings.len(), 1);
        let data = &findings[0].effect.data;
        assert_eq!(data["category"], "solicitud_factura");
        assert_eq!(data["wider"]["id"], "dom");
        assert_eq!(data["wider"]["scope"], "domain");
        assert_eq!(data["narrower"]["value"], "ana@alfa.test");
    }

    // ---- Unificación ----

    fn task(id: i64, supplier: &str, total: &str) -> Task {
        Task {
            id,
            kind: INVOICE.into(),
            input_ref: format!("{id}.txt"),
            state: TaskState::AwaitingReview,
            attempts: 1,
            result: Some(json!({
                "document": format!("{id}.txt"),
                "fields": { "supplier": supplier, "total": total },
                "currency_evidence": { "foreign": [], "euro": true },
                "issues": [],
                "unreadable": [],
            })),
            error: None,
            created_at: 0,
            updated_at: 0,
        }
    }

    fn alfa_tasks() -> Vec<Task> {
        // Hasta 500: 100, 200, 300, 400 y 500; por encima: 600 y 700; y una de otro proveedor.
        let mut tasks: Vec<Task> = ["100.00", "200.00", "300.00", "400.00", "500.00"]
            .iter()
            .zip(1..)
            .map(|(t, i)| task(i, "Alfa SL", t))
            .collect();
        tasks.push(task(6, "Alfa SL", "600.00"));
        tasks.push(task(7, "alfa sl", "700.00"));
        tasks.push(task(8, "Beta SA", "100.00"));
        tasks
    }

    fn two_rules() -> Vec<Rule> {
        vec![
            inv("alfa-libre", "Alfa SL", None, true),
            inv("alfa-500", "Alfa SL", Some("500.00"), true),
        ]
    }

    #[test]
    fn merge_needs_a_choice_when_the_limits_differ() {
        let rules = two_rules();
        let m = Marks::default();
        let both = ids(&["alfa-500", "alfa-libre"]);
        let options = merge_options(&rules, &m, &both).unwrap();
        assert_eq!(
            options.iter().map(|o| o.key.as_str()).collect::<Vec<_>>(),
            ["500.00", "none"]
        );
        let Err(MergeError::ChoiceRequired(listed)) = merge_plan(&rules, &m, &both, None, &[])
        else {
            panic!("debía pedir una elección");
        };
        assert_eq!(listed, options);
        assert!(matches!(
            merge_plan(&rules, &m, &both, Some("750.00"), &[]),
            Err(MergeError::UnknownChoice(_))
        ));
        let plan = merge_plan(&rules, &m, &both, Some("500.00"), &[]).unwrap();
        assert_eq!(
            plan.after.conditions,
            json!({ "supplier": "Alfa SL", "max_total": "500.00" })
        );
        assert!(!plan.after.active);
        assert_eq!(plan.before.len(), 2);
        // Sin límite.
        let plan = merge_plan(&rules, &m, &both, Some("none"), &[]).unwrap();
        assert_eq!(plan.after.conditions, json!({ "supplier": "Alfa SL" }));
    }

    #[test]
    fn merging_equal_rules_needs_no_choice() {
        let rules = vec![
            inv("a", "Alfa SL", Some("500.00"), true),
            inv("b", "alfa sl", Some("500,00"), false),
        ];
        let plan = merge_plan(&rules, &Marks::default(), &ids(&["b", "a"]), None, &[]).unwrap();
        assert_eq!(plan.chosen, "500.00");
        assert_eq!(plan.options.len(), 1);
        assert_eq!(plan.before[0].id, "a");
    }

    #[test]
    fn the_merge_simulation_matches_production_decisions() {
        let rules = two_rules();
        let m = Marks::default();
        let tasks = alfa_tasks();
        let both = ids(&["alfa-libre", "alfa-500"]);

        // Producción hoy: hasta 500 coinciden las dos (revisión); por encima solo la libre.
        let today: Vec<bool> = tasks
            .iter()
            .map(|t| {
                matches!(
                    evaluate_result(
                        &rules,
                        &HashSet::new(),
                        REGISTER_INVOICE,
                        t.result.as_ref().unwrap()
                    )
                    .decision,
                    RuleDecision::Automate { .. }
                )
            })
            .collect();
        assert_eq!(
            today,
            [false, false, false, false, false, true, true, false]
        );

        // Elegir ≤ 500: 5 dejan de revisarse y 2 por encima dejan de automatizarse.
        let plan = merge_plan(&rules, &m, &both, Some("500.00"), &tasks).unwrap();
        let sim = &plan.simulation;
        assert_eq!(sim.considered, 8);
        assert_eq!(sim.touched, 7, "la de Beta no la toca ninguna regla");
        assert_eq!(
            sim.start_automating
                .iter()
                .map(|t| t.task_id)
                .collect::<Vec<_>>(),
            [1, 2, 3, 4, 5]
        );
        assert_eq!(
            sim.stop_automating
                .iter()
                .map(|t| t.task_id)
                .collect::<Vec<_>>(),
            [6, 7]
        );
        assert_eq!(sim.stop_automating[0].total.as_deref(), Some("600.00"));
        assert_eq!((sim.unchanged_automated, sim.unchanged_review), (0, 0));

        // Producción con la regla nueva como único Alfa: las mismas decisiones.
        let merged = vec![inv("nueva", "Alfa SL", Some("500.00"), true)];
        for (t, expected) in tasks
            .iter()
            .zip([true, true, true, true, true, false, false, false])
        {
            let decision = evaluate_result(
                &merged,
                &HashSet::new(),
                REGISTER_INVOICE,
                t.result.as_ref().unwrap(),
            )
            .decision;
            assert_eq!(
                matches!(decision, RuleDecision::Automate { .. }),
                expected,
                "tarea {}",
                t.id
            );
        }

        // Elegir sin límite: las 5 dejan de revisarse y las dos grandes siguen igual.
        let plan = merge_plan(&rules, &m, &both, Some("none"), &tasks).unwrap();
        let sim = &plan.simulation;
        assert_eq!(sim.start_automating.len(), 5);
        assert!(sim.stop_automating.is_empty());
        assert_eq!(sim.unchanged_automated, 2);
    }

    #[test]
    fn the_merge_simulation_skips_tasks_without_result_and_other_kinds() {
        let rules = two_rules();
        let mut tasks = alfa_tasks();
        tasks[0].result = None;
        let mut mail = task(99, "Alfa SL", "100.00");
        mail.kind = EMAIL.into();
        tasks.push(mail);
        let plan = merge_plan(
            &rules,
            &Marks::default(),
            &ids(&["alfa-libre", "alfa-500"]),
            Some("500.00"),
            &tasks,
        )
        .unwrap();
        assert_eq!(plan.simulation.skipped_without_result, 1);
        assert_eq!(plan.simulation.considered, 7);
    }

    #[test]
    fn merge_rejects_what_cannot_be_unified() {
        let m = Marks::default();
        let rules = vec![
            inv("a", "Alfa SL", None, true),
            inv("b", "Alfa SL", Some("500.00"), true),
            inv("sim", "ALFA, S.L.", None, true),
            inv("beta", "Beta SA", None, true),
            send("s", json!({ "sender_domain": "alfa.test" }), true),
            send("miss", json!({}), true),
            send("miss2", json!({}), true),
            rule("u", "emit_recurring", json!({}), true),
            rule(
                "bad",
                REGISTER_INVOICE,
                json!({ "supplier": "Alfa SL", "max_total": "x" }),
                true,
            ),
            inv("old", "Alfa SL", None, false),
        ];
        let m_arch = marks(&[], &["old"]);
        let merge = |m: &Marks, list: &[&str]| merge_plan(&rules, m, &ids(list), Some("none"), &[]);
        assert_eq!(merge(&m, &["a"]), Err(MergeError::TooFew));
        assert_eq!(merge(&m, &["a", "a"]), Err(MergeError::Repeated));
        assert!(matches!(
            merge(&m, &["a", "nope"]),
            Err(MergeError::Missing(_))
        ));
        assert_eq!(
            merge(&m_arch, &["a", "old"]),
            Err(MergeError::Archived("old".into()))
        );
        assert_eq!(merge(&m, &["a", "s"]), Err(MergeError::MixedKinds));
        assert!(matches!(
            merge(&m, &["a", "sim"]),
            Err(MergeError::SimilarSupplier(..))
        ));
        assert!(matches!(
            merge(&m, &["a", "beta"]),
            Err(MergeError::Disjoint(..))
        ));
        assert!(matches!(
            merge(&m, &["miss", "miss2"]),
            Err(MergeError::Defective(..))
        ));
        assert!(matches!(
            merge(&m, &["a", "bad"]),
            Err(MergeError::Defective(..))
        ));
        assert_eq!(
            merge(&m, &["u", "u2"]).unwrap_err().to_string(),
            "la regla u2 no existe"
        );
        let unknown = vec![
            rule("u", "emit_recurring", json!({}), true),
            rule("v", "emit_recurring", json!({}), true),
        ];
        assert_eq!(
            merge_plan(&unknown, &m, &ids(&["u", "v"]), None, &[]),
            Err(MergeError::UnknownKind)
        );
        // El mensaje del nombre parecido explica el porqué y qué hacer.
        let text = merge(&m, &["a", "sim"]).unwrap_err().to_string();
        assert!(
            text.contains("Alfa SL") && text.contains("ALFA, S.L.") && text.contains("edite"),
            "{text}"
        );
    }

    #[test]
    fn a_merged_rule_never_inherits_the_advanced_mark_and_the_plan_says_so() {
        let rules = vec![
            send("any", json!({}), true),
            send("dom", json!({ "sender_domain": "alfa.test" }), true),
        ];
        let m = marks(&["any"], &[]);
        let both = ids(&["any", "dom"]);
        let plan = merge_plan(&rules, &m, &both, Some("domain:alfa.test"), &[]).unwrap();
        assert_eq!(plan.after.loses_any_sender, ["any"]);
        assert!(!plan.after.needs_sender_validation);
        assert!(
            plan.before
                .iter()
                .find(|r| r.id == "any")
                .unwrap()
                .any_sender
        );
        // Si la elegida es «cualquier remitente», la regla nueva no actúa hasta validarla.
        let plan = merge_plan(&rules, &m, &both, Some("any"), &[]).unwrap();
        assert!(plan.after.needs_sender_validation);
        assert_eq!(
            plan.after.conditions,
            json!({ "category": "solicitud_factura" })
        );
    }

    #[test]
    fn the_plan_fingerprint_changes_with_the_rules_and_the_choice() {
        let m = Marks::default();
        let both = ids(&["alfa-libre", "alfa-500"]);
        let base = merge_plan(&two_rules(), &m, &both, Some("500.00"), &[]).unwrap();
        let same = merge_plan(&two_rules(), &m, &both, Some("500.00"), &alfa_tasks()).unwrap();
        assert_eq!(base.fingerprint, same.fingerprint, "las tareas no entran");
        let other_choice = merge_plan(&two_rules(), &m, &both, Some("none"), &[]).unwrap();
        assert_ne!(base.fingerprint, other_choice.fingerprint);
        let mut changed = two_rules();
        changed[1].version = 2;
        assert_ne!(
            base.fingerprint,
            merge_plan(&changed, &m, &both, Some("500.00"), &[])
                .unwrap()
                .fingerprint
        );
        let mut third = two_rules();
        third.push(inv("beta", "Beta SA", None, true));
        assert_ne!(
            base.fingerprint,
            merge_plan(&third, &m, &both, Some("500.00"), &[])
                .unwrap()
                .fingerprint,
            "otra regla del tipo también cuenta"
        );
    }
}

#[cfg(test)]
mod third_rule_tests {
    use super::*;
    use nestadmin_core::TaskState;
    use serde_json::json;

    fn inv(id: &str, max: Option<&str>) -> Rule {
        let mut c = json!({ "supplier": "Alfa SL" });
        if let Some(max) = max {
            c["max_total"] = json!(max);
        }
        Rule {
            id: id.into(),
            version: 1,
            action_kind: REGISTER_INVOICE.into(),
            conditions: c,
            active: true,
            created_at: 0,
        }
    }

    fn task(id: i64, total: &str, checked: bool) -> Task {
        let mut result = json!({
            "document": format!("{id}.txt"),
            "fields": { "supplier": "Alfa SL", "total": total },
            "issues": [],
            "unreadable": [],
        });
        if checked {
            result["currency_evidence"] = json!({ "foreign": [], "euro": true });
        }
        Task {
            id,
            kind: INVOICE.into(),
            input_ref: format!("{id}.txt"),
            state: TaskState::AwaitingReview,
            attempts: 1,
            result: Some(result),
            error: None,
            created_at: 0,
            updated_at: 0,
        }
    }

    fn ids(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn a_third_active_rule_that_covers_the_remaining_range_is_named_in_the_finding() {
        // A sin límite, B hasta 500 y C hasta 1000, todas activas: con 700 € coinciden A y C.
        let rules = vec![
            inv("a", None),
            inv("b", Some("500.00")),
            inv("c", Some("1000.00")),
        ];
        let findings = analyze(&rules, &Marks::default());
        let by_key = |key: &str| findings.iter().find(|f| f.key == key).unwrap();
        let ab = by_key("overlap:a+b");
        assert_eq!(
            ab.others.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            ["c"]
        );
        // Para A y C, B (hasta 500) queda dentro de C: no cambia lo que se registra solo.
        assert!(by_key("overlap:a+c").others.is_empty());
        // Para B y C, A sí cubre todo lo que solo cubre C.
        let bc = by_key("overlap:b+c");
        assert_eq!(
            bc.others.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            ["a"]
        );
        // Sin tercera regla no hay nada que nombrar; una inactiva tampoco cuenta.
        let two = analyze(&rules[..2], &Marks::default());
        assert!(two[0].others.is_empty());
        let mut inactive = rules.clone();
        inactive[2].active = false;
        let findings = analyze(&inactive, &Marks::default());
        assert!(findings
            .iter()
            .find(|f| f.key == "overlap:a+b")
            .unwrap()
            .others
            .is_empty());
    }

    #[test]
    fn the_merge_simulation_names_the_rule_that_automates_each_task() {
        // A hasta 500 y B hasta 1000 se unifican en 500; C hasta 600 queda fuera.
        let rules = vec![
            inv("a", Some("500.00")),
            inv("b", Some("1000.00")),
            inv("c", Some("600.00")),
        ];
        let tasks = vec![
            task(1, "300.00", true),
            task(2, "550.00", true),
            task(3, "800.00", true),
        ];
        let plan = merge_plan(
            &rules,
            &Marks::default(),
            &ids(&["a", "b"]),
            Some("500.00"),
            &tasks,
        )
        .unwrap();
        let sim = &plan.simulation;
        // 550 €: hoy coinciden B y C (revisión); después solo C, que no es la regla nueva.
        assert_eq!(sim.start_automating.len(), 1);
        assert_eq!(sim.start_automating[0].task_id, 2);
        assert_eq!(sim.start_automating[0].automated_before_by, None);
        assert_eq!(
            sim.start_automating[0].automated_after_by.as_deref(),
            Some("c")
        );
        // 800 €: hoy solo B lo automatiza; después ninguna.
        assert_eq!(sim.stop_automating.len(), 1);
        assert_eq!(sim.stop_automating[0].task_id, 3);
        assert_eq!(
            sim.stop_automating[0].automated_before_by.as_deref(),
            Some("b")
        );
        assert_eq!(sim.stop_automating[0].automated_after_by, None);
        // 300 €: hoy coinciden A, B y C; después la nueva y C: sigue en revisión.
        assert_eq!(sim.unchanged_review, 1);
        assert!(plan.simulation_if_validated.is_none());
    }

    #[test]
    fn any_sender_shows_both_scenarios_and_counts_unchecked_currency() {
        let send = |id: &str, extra: Value| {
            let mut c = json!({ "category": "solicitud_factura" });
            for (k, v) in extra.as_object().unwrap() {
                c[k] = v.clone();
            }
            Rule {
                id: id.into(),
                version: 1,
                action_kind: SEND_DRAFT.into(),
                conditions: c,
                active: true,
                created_at: 0,
            }
        };
        let rules = vec![
            send("any", json!({})),
            send("dom", json!({ "sender_domain": "alfa.test" })),
            send("other", json!({ "sender_domain": "beta.test" })),
        ];
        let marks = Marks {
            any_sender: ["any".to_owned()].into(),
            archived: HashSet::new(),
        };
        let mail = |id: i64, from: &str| Task {
            id,
            kind: EMAIL.into(),
            input_ref: format!("{id}"),
            state: TaskState::AwaitingReview,
            attempts: 1,
            result: Some(json!({
                "category": "solicitud_factura",
                "message": { "from": from, "subject": "Asunto", "signals": [] },
                "draft": { "to": [from], "body": "Hola." },
                "draft_flags": [],
            })),
            error: None,
            created_at: 0,
            updated_at: 0,
        };
        let tasks = vec![mail(1, "ana@alfa.test"), mail(2, "luis@beta.test")];
        let plan = merge_plan(&rules, &marks, &ids(&["any", "dom"]), Some("any"), &tasks).unwrap();
        // Tal como se crea (sin validar) la regla nueva no actúa: el correo de beta.test lo
        // automatiza `other`, que queda sola, y el de alfa.test sigue en revisión.
        let unvalidated = &plan.simulation;
        assert_eq!(unvalidated.start_automating.len(), 1, "{unvalidated:?}");
        assert_eq!(unvalidated.start_automating[0].task_id, 2);
        assert_eq!(
            unvalidated.start_automating[0]
                .automated_after_by
                .as_deref(),
            Some("other")
        );
        assert_eq!(unvalidated.unchanged_review, 1);
        // Tras validar la opción avanzada, la regla nueva automatiza el de alfa.test y el de
        // beta.test coincide con las dos y se revisa.
        let validated = plan.simulation_if_validated.as_ref().unwrap();
        assert_eq!(validated.start_automating.len(), 1);
        assert_eq!(validated.start_automating[0].task_id, 1);
        assert_eq!(
            validated.start_automating[0].automated_after_by.as_deref(),
            Some(MERGED_RULE)
        );
        assert_eq!(validated.unchanged_review, 1);
        assert!(plan.after.needs_sender_validation);

        // Facturas sin comprobación de moneda: se cuentan.
        let invoice_rules = vec![inv("a", None), inv("b", Some("500.00"))];
        let tasks = vec![task(1, "100.00", false), task(2, "100.00", true)];
        let plan = merge_plan(
            &invoice_rules,
            &Marks::default(),
            &ids(&["a", "b"]),
            Some("none"),
            &tasks,
        )
        .unwrap();
        assert_eq!(plan.simulation.without_currency_check, 1);
    }
}
