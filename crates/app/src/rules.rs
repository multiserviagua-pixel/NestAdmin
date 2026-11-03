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

fn matches(rule: &Rule, result: &Value) -> bool {
    let condition = |key: &str| rule.conditions.get(key).and_then(Value::as_str);
    match rule.action_kind.as_str() {
        REGISTER_INVOICE => {
            let Some(supplier) = condition("supplier") else {
                return false;
            };
            let actual = result["fields"]["supplier"].as_str().unwrap_or("");
            if !actual.trim().eq_ignore_ascii_case(supplier.trim()) {
                return false;
            }
            match condition("max_total") {
                None => true,
                Some(limit) => {
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
            }
        }
        SEND_DRAFT => condition("category").is_some_and(|c| result["category"].as_str() == Some(c)),
        _ => false,
    }
}
