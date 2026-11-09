//! Lo que el aprendizaje deja para el resto de la aplicación (D171, D172): señales `learned` para
//! `suggest` y recuentos de `reply_style` para la línea de confianza de la redacción. Solo claves
//! de señal, ids y enums; nunca texto de correo.

use nestadmin_core::{LearnedEvidence, LearnedSignal};

use crate::learn_state::LearnedMessage;
use crate::rules::is_email_address;

/// Valores cerrados de `reply_kind` (D170).
pub const REPLY_KINDS: [&str; 6] = ["confirm", "inform", "schedule", "quote", "decline", "other"];

/// Un recuento mínimo para hablar de una respuesta habitual (D171).
pub const HABIT_MIN: i64 = 2;

/// Evidencia de un resultado `done` (D172). Una señal `learned` por correo recibido que cumple
/// todo: remitente no automático y respuesta propia (ya garantizados por la recogida, D167-D168),
/// `needs_reply` verdadero, categoría distinta de `otros` y una dirección simple. El dominio
/// público no excluye la señal: vale para la regla de dirección exacta (D126) y solo las reglas
/// por dominio lo rechazan, en `suggest`.
///
/// `reply_style` recoge `(categoría, reply_kind)` de **todos** los pares con `needs_reply`
/// verdadero y un `reply_kind` conocido, también los de `otros`.
pub fn learned_evidence(source: &str, pairs: &[LearnedMessage]) -> LearnedEvidence {
    let mut out = LearnedEvidence::default();
    for p in pairs.iter().filter(|p| p.needs_reply) {
        let category = p.category.trim();
        if category.is_empty() || category.contains('|') {
            continue;
        }
        if REPLY_KINDS.contains(&p.reply_kind.as_str()) {
            out.reply_style
                .push((category.to_owned(), p.reply_kind.clone()));
        }
        let address = p.address.trim().to_ascii_lowercase();
        if category == "otros" || p.message_id.is_empty() || !is_email_address(&address) {
            continue;
        }
        out.signals.push(LearnedSignal {
            subject_key: format!("{category}|{address}"),
            source_key: format!("learned|{source}|{}", p.message_id),
        });
    }
    out
}

/// Respuesta habitual de una categoría: la de mayor recuento, con al menos [`HABIT_MIN`] y sin
/// empate en la cabeza. `other` no dice nada y no cuenta como habitual.
pub fn dominant_reply_kind(rows: &[(String, String, i64)], category: &str) -> Option<String> {
    let mut mine: Vec<&(String, String, i64)> = rows.iter().filter(|r| r.0 == category).collect();
    mine.sort_by_key(|r| std::cmp::Reverse(r.2));
    let top = mine.first()?;
    if top.2 < HABIT_MIN || mine.get(1).is_some_and(|second| second.2 == top.2) {
        return None;
    }
    (top.1 != "other" && REPLY_KINDS.contains(&top.1.as_str())).then(|| top.1.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair(id: &str, address: &str, category: &str, needs: bool, kind: &str) -> LearnedMessage {
        LearnedMessage {
            message_id: id.into(),
            address: address.into(),
            category: category.into(),
            needs_reply: needs,
            reply_kind: kind.into(),
        }
    }

    #[test]
    fn only_pairs_that_need_a_reply_outside_otros_become_signals() {
        let e = learned_evidence(
            "account:a",
            &[
                pair("1", "Ana@Alfa.test", "consulta", true, "confirm"),
                pair("2", "ana@alfa.test", "consulta", false, "inform"),
                pair("3", "ana@alfa.test", "otros", true, "inform"),
                pair("4", "no es una direccion", "consulta", true, "inform"),
                pair("5", "luis@gmail.com", "consulta", true, "quote"),
            ],
        );
        let keys: Vec<_> = e.signals.iter().map(|s| s.subject_key.as_str()).collect();
        assert_eq!(keys, ["consulta|ana@alfa.test", "consulta|luis@gmail.com"]);
        assert_eq!(e.signals[0].source_key, "learned|account:a|1");
        // El estilo cuenta todos los pares con respuesta necesaria y tipo conocido.
        assert_eq!(e.reply_style.len(), 4);
    }

    #[test]
    fn a_dominant_kind_needs_two_and_no_tie() {
        let rows = |v: &[(&str, &str, i64)]| -> Vec<(String, String, i64)> {
            v.iter()
                .map(|(a, b, n)| (a.to_string(), b.to_string(), *n))
                .collect()
        };
        assert_eq!(
            dominant_reply_kind(&rows(&[("c", "confirm", 3), ("c", "inform", 1)]), "c").as_deref(),
            Some("confirm")
        );
        assert_eq!(
            dominant_reply_kind(&rows(&[("c", "confirm", 1)]), "c"),
            None
        );
        assert_eq!(
            dominant_reply_kind(&rows(&[("c", "confirm", 2), ("c", "inform", 2)]), "c"),
            None
        );
        assert_eq!(dominant_reply_kind(&rows(&[("c", "other", 5)]), "c"), None);
        assert_eq!(
            dominant_reply_kind(&rows(&[("c", "confirm", 5)]), "d"),
            None
        );
    }
}
