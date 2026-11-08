//! Evaluación de reglas expresas sobre datos ya validados (R07). Una acción se automatiza
//! solo si exactamente una regla activa coincide y los datos no tienen incidencias; en
//! cualquier otro caso queda pendiente de revisión.

use nestadmin_core::Rule;
use nestadmin_documents::parse_amount;
use serde::Serialize;
use serde_json::{json, Value};

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
/// - `register_invoice`: `supplier` (obligatoria, igualdad sin mayúsculas) y `max_total` opcional.
/// - `send_draft`: `category` (obligatoria).
///
/// Comprueba también las reglas inactivas, solo para poder decir que habrían aplicado: solo
/// una regla activa puede automatizar.
pub fn evaluate(rules: &[Rule], action_kind: &str, result: &Value) -> Evaluation {
    let candidates: Vec<(&Rule, RuleCheck)> = rules
        .iter()
        .filter(|r| r.action_kind == action_kind)
        .map(|r| {
            let mismatch = mismatch(r, result);
            let check = RuleCheck {
                id: r.id.clone(),
                version: r.version,
                active: r.active,
                matches: mismatch.is_none(),
                mismatch,
            };
            (r, check)
        })
        .collect();
    let matching: Vec<&Rule> = candidates
        .iter()
        .filter(|(r, c)| r.active && c.matches)
        .map(|(r, _)| *r)
        .collect();
    let (code, facts, automated) = match (matching.as_slice(), blockers(action_kind, result)) {
        ([], _) => (ReasonCode::NoActiveRuleMatches, json!({}), None),
        (_, Some(code)) => (code, json!({}), None),
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

/// Motivos que impiden automatizar aunque haya una regla.
fn blockers(action_kind: &str, result: &Value) -> Option<ReasonCode> {
    let non_empty = |key: &str| result[key].as_array().is_some_and(|a| !a.is_empty());
    match action_kind {
        REGISTER_INVOICE if non_empty("issues") => Some(ReasonCode::InvoiceIssues),
        REGISTER_INVOICE if non_empty("unreadable") => Some(ReasonCode::UnreadableFields),
        SEND_DRAFT
            if result["draft"]["to"]
                .as_array()
                .is_none_or(|a| a.is_empty()) =>
        {
            Some(ReasonCode::NoRecipient)
        }
        _ => None,
    }
}

/// `None` si la regla coincide; si no, el motivo con los datos concretos. Fallo cerrado
/// (D123): una regla con claves desconocidas o con `max_total` que no sea un texto de importe
/// válido no coincide nunca, aunque esté activa.
fn mismatch(rule: &Rule, result: &Value) -> Option<String> {
    let Some(conditions) = rule.conditions.as_object() else {
        return Some("las condiciones de la regla no son válidas".into());
    };
    let allowed: &[&str] = match rule.action_kind.as_str() {
        REGISTER_INVOICE => &["supplier", "max_total"],
        SEND_DRAFT => &["category"],
        _ => return Some("tipo de regla no reconocido".into()),
    };
    if let Some(key) = conditions.keys().find(|k| !allowed.contains(&k.as_str())) {
        return Some(format!("la regla tiene una condición desconocida: «{key}»"));
    }
    let condition = |key: &str| conditions.get(key).and_then(Value::as_str);
    match rule.action_kind.as_str() {
        REGISTER_INVOICE => {
            let Some(supplier) = condition("supplier") else {
                return Some("la regla no indica proveedor".into());
            };
            let actual = result["fields"]["supplier"].as_str().unwrap_or("");
            if !actual.trim().eq_ignore_ascii_case(supplier.trim()) {
                return Some(if actual.trim().is_empty() {
                    format!("la factura no indica proveedor y la regla pide «{supplier}»")
                } else {
                    format!("el proveedor es «{actual}» y la regla pide «{supplier}»")
                });
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
                        (Ok(_), Some(Ok(_))) => Some(format!(
                            "el total {} supera el máximo {limit}",
                            total.unwrap_or("")
                        )),
                        (Err(_), _) => Some(format!("el máximo «{limit}» no es un importe válido")),
                        // Sin total legible no se puede comprobar el límite.
                        _ => Some(
                            "la factura no tiene un total legible para comprobar el máximo".into(),
                        ),
                    }
                }
                // Límite no textual: la regla es defectuosa y no automatiza.
                Some(_) => Some("el máximo de la regla no es un texto de importe".into()),
            }
        }
        _ => {
            let Some(wanted) = condition("category") else {
                return Some("la regla no indica categoría".into());
            };
            let actual = result["category"].as_str();
            if actual == Some(wanted) {
                None
            } else {
                Some(format!(
                    "la categoría es «{}» y la regla pide «{wanted}»",
                    actual.unwrap_or("sin categoría")
                ))
            }
        }
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
        json!({ "fields": { "supplier": "Alfa SL", "total": "121.00" }, "issues": [], "unreadable": [] })
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
            json!({ "supplier": "Alfa SL", "currency": "EUR" }),
        );
        assert!(!automates(r, &invoice(), REGISTER_INVOICE));
        let result =
            json!({ "category": "solicitud_factura", "draft": { "to": ["a@example.test"] } });
        let ok = rule(SEND_DRAFT, json!({ "category": "solicitud_factura" }));
        assert!(automates(ok, &result, SEND_DRAFT));
        let bad = rule(
            SEND_DRAFT,
            json!({ "category": "solicitud_factura", "sender": "x@example.test" }),
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
        json!({ "category": category, "draft": { "to": to } })
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
        let send = || named("s", true, SEND_DRAFT, json!({ "category": "consulta" }));
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
        let r = named("s", true, SEND_DRAFT, json!({ "category": "consulta" }));
        let mut result = draft("consulta", json!(["a@example.test"]));
        result["draft"]["body"] = json!("TEXTO PRIVADO");
        let value = serde_json::to_value(evaluate(&[r], SEND_DRAFT, &result).explanation).unwrap();
        assert_eq!(value["code"], "automated");
        assert_eq!(value["facts"], json!({ "rule_id": "s", "rule_version": 2 }));
        assert!(!value.to_string().contains("TEXTO PRIVADO"));
        assert!(!value.to_string().contains("a@example.test"));
    }
}
