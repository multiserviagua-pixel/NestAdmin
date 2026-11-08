//! Evaluación de reglas expresas sobre datos ya validados (R07). Una acción se automatiza
//! solo si exactamente una regla activa coincide y los datos no tienen incidencias; en
//! cualquier otro caso queda pendiente de revisión.

use nestadmin_core::Rule;
use nestadmin_documents::parse_amount;
use serde_json::Value;

pub const REGISTER_INVOICE: &str = "register_invoice";
pub const SEND_DRAFT: &str = "send_draft";

/// Regla aplicable o motivo por el que no se automatiza.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuleDecision {
    Automate { rule_id: String, rule_version: i64 },
    Review(String),
}

/// `result` es el resultado de la tarea (ver `pipeline`). Condiciones admitidas:
/// - `register_invoice`: `supplier` (obligatoria, igualdad sin mayúsculas) y `max_total` opcional.
/// - `send_draft`: `category` (obligatoria).
pub fn decide(rules: &[Rule], action_kind: &str, result: &Value) -> RuleDecision {
    let blockers = blockers(action_kind, result);
    let matching: Vec<&Rule> = rules
        .iter()
        .filter(|r| r.active && r.action_kind == action_kind && matches(r, result))
        .collect();
    match (matching.as_slice(), blockers) {
        ([], _) => RuleDecision::Review("ninguna regla activa coincide".into()),
        (_, Some(reason)) => RuleDecision::Review(reason),
        ([rule], None) => RuleDecision::Automate {
            rule_id: rule.id.clone(),
            rule_version: rule.version,
        },
        (several, None) => RuleDecision::Review(format!(
            "{} reglas coinciden; la decisión no es inequívoca",
            several.len()
        )),
    }
}

/// Motivos que impiden automatizar aunque haya una regla.
fn blockers(action_kind: &str, result: &Value) -> Option<String> {
    let non_empty = |key: &str| result[key].as_array().is_some_and(|a| !a.is_empty());
    match action_kind {
        REGISTER_INVOICE if non_empty("issues") => Some("la factura tiene incidencias".into()),
        REGISTER_INVOICE if non_empty("unreadable") => Some("hay campos ilegibles".into()),
        SEND_DRAFT
            if result["draft"]["to"]
                .as_array()
                .is_none_or(|a| a.is_empty()) =>
        {
            Some("el borrador no tiene destinatario".into())
        }
        _ => None,
    }
}

/// Fallo cerrado (D123): una regla con claves desconocidas o con `max_total` que no sea un
/// texto de importe válido no coincide nunca, aunque esté activa.
fn matches(rule: &Rule, result: &Value) -> bool {
    let Some(conditions) = rule.conditions.as_object() else {
        return false;
    };
    let allowed: &[&str] = match rule.action_kind.as_str() {
        REGISTER_INVOICE => &["supplier", "max_total"],
        SEND_DRAFT => &["category"],
        _ => return false,
    };
    if conditions.keys().any(|k| !allowed.contains(&k.as_str())) {
        return false;
    }
    let condition = |key: &str| conditions.get(key).and_then(Value::as_str);
    match rule.action_kind.as_str() {
        REGISTER_INVOICE => {
            let Some(supplier) = condition("supplier") else {
                return false;
            };
            let actual = result["fields"]["supplier"].as_str().unwrap_or("");
            if !actual.trim().eq_ignore_ascii_case(supplier.trim()) {
                return false;
            }
            match conditions.get("max_total") {
                None => true,
                Some(Value::String(limit)) => {
                    let total = result["fields"]["total"].as_str();
                    match (
                        parse_amount("max_total", limit),
                        total.map(|t| parse_amount("total", t)),
                    ) {
                        (Ok(limit), Some(Ok(total))) => total <= limit,
                        // Sin total legible no se puede comprobar el límite.
                        _ => false,
                    }
                }
                // Límite no textual: la regla es defectuosa y no automatiza.
                Some(_) => false,
            }
        }
        _ => condition("category").is_some_and(|c| result["category"].as_str() == Some(c)),
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
}
