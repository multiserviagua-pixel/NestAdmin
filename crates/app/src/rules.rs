//! Evaluación de reglas expresas sobre datos ya validados (R07). Una acción se automatiza
//! solo si exactamente una regla activa coincide y los datos no tienen incidencias; en
//! cualquier otro caso queda pendiente de revisión.

use std::collections::HashSet;

use nestadmin_core::Rule;
use nestadmin_documents::parse_amount;
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

pub const REGISTER_INVOICE: &str = "register_invoice";
pub const SEND_DRAFT: &str = "send_draft";

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

    /// Explicación de una factura separada de un documento con varias.
    pub fn split_invoice() -> Self {
        Self {
            code: ReasonCode::SplitInvoice,
            facts: json!({}),
            candidates: Vec::new(),
        }
    }
}

/// Motivo cuando falta la comprobación local de moneda (D127).
pub const CURRENCY_UNCHECKED: &str = "no consta la comprobación de moneda; reprocese o revise";

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
        SEND_DRAFT
            if result["draft"]["to"]
                .as_array()
                .is_none_or(|a| a.is_empty()) =>
        {
            Some((ReasonCode::NoRecipient, json!({})))
        }
        _ => None,
    }
}

/// Por qué una regla no coincide. `sender_required`: solo falta el remitente o su validación
/// avanzada (D126); el resto de condiciones sí coincide.
struct Mismatch {
    text: String,
    sender_required: bool,
}

fn no(text: impl Into<String>) -> Option<Mismatch> {
    Some(Mismatch {
        text: text.into(),
        sender_required: false,
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
        _ => return no("tipo de regla no reconocido"),
    };
    if let Some(key) = conditions.keys().find(|k| !allowed.contains(&k.as_str())) {
        return no(format!("la regla tiene una condición desconocida: «{key}»"));
    }
    let condition = |key: &str| conditions.get(key).and_then(Value::as_str);
    match rule.action_kind.as_str() {
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

/// Condición de remitente de una regla de envío (D126). La comparación no distingue
/// mayúsculas y usa `message.from`, que es solo la dirección. El dominio es exacto: la parte
/// tras la `@`, sin subdominios.
fn sender_mismatch(
    conditions: &serde_json::Map<String, Value>,
    any_sender_ack: bool,
    result: &Value,
) -> Option<Mismatch> {
    let actual = result["message"]["from"].as_str().map(str::trim);
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
                })
            }
        }
    };
    let Some(wanted) = wanted.as_str().map(str::trim).filter(|w| !w.is_empty()) else {
        return no("el remitente de la regla no es un texto válido");
    };
    let Some(actual) = actual.filter(|a| !a.is_empty()) else {
        return no(format!(
            "el correo no indica remitente y la regla pide «{wanted}»"
        ));
    };
    if !is_email_address(actual) {
        return no(format!(
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
        None
    } else if key == "sender" {
        no(format!(
            "el remitente es «{actual}» y la regla pide «{wanted}»"
        ))
    } else {
        no(format!(
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
            "message": { "from": "a@example.test" },
            "draft": { "to": ["a@example.test"] },
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
            "message": { "from": "a@example.test" },
            "draft": { "to": to },
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
            "message": {},
            "draft": { "to": ["a@example.test"] },
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
