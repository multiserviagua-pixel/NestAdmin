//! Respaldo de la respuesta del asistente (D269, R33): de dónde pudo salir lo que dice, calculado
//! por la aplicación a partir de lo que de verdad ocurrió en el turno, nunca de lo que diga el
//! modelo. Estado cerrado (`Grounding`) y cifras de la respuesta sin origen (`Unsourced`).
//!
//! Nada de esto se envía al modelo ni a un proveedor externo: se guarda con el turno (`meta`) y se
//! muestra en la ventana.

use std::collections::HashSet;

use nestadmin_ai::figures::{unsourced_figures, Sources, Unsourced};
use nestadmin_core::{SearchHit, StoredTurn};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::tools::{hit_value, Text};

/// De dónde pudo salir la respuesta. Se calcula al cerrar el turno.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Grounding {
    /// Cita al menos un documento válido, que se muestra.
    Documents,
    /// Sin citas, pero en este turno se leyeron datos del equipo (o se creó una propuesta).
    Data,
    /// Se buscó en este turno y la búsqueda no devolvió nada: «no consta» está respaldado.
    NoResults,
    /// Se leyeron documentos en este turno y la respuesta no cita ninguno.
    Uncited,
    /// No se ejecutó con éxito ninguna herramienta de lectura en este turno.
    None,
}

impl Grounding {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Documents => "documents",
            Self::Data => "data",
            Self::NoResults => "no_results",
            Self::Uncited => "uncited",
            Self::None => "none",
        }
    }

    /// ¿Respalda esta respuesta a otras posteriores que repitan sus cifras?
    pub fn backs_later_answers(self) -> bool {
        matches!(self, Self::Documents | Self::Data)
    }
}

/// Una herramienta que se ejecutó en el turno y lo que devolvió. Solo viven en memoria durante el
/// turno (y se descartan al cerrarlo): son la fuente de las cifras que respaldan la respuesta.
#[derive(Debug, Clone)]
pub(crate) struct ToolRecord {
    pub name: String,
    pub value: Value,
}

/// Herramientas que leen datos del equipo y no documentos: sumas y recuentos, tareas, reglas,
/// sugerencias, historial, cuentas y estado de la IA.
const DATA_TOOLS: [&str; 7] = [
    "aggregate",
    "list_tasks",
    "list_rules",
    "list_suggestions",
    "history",
    "mail_accounts",
    "ai_status",
];

impl ToolRecord {
    /// ¿Se ejecutó con éxito? Un rechazo, un error de la herramienta o una propuesta que no llegó a
    /// crearse no cuentan como lectura.
    pub(crate) fn succeeded(&self) -> bool {
        if !self.value.get("error").is_none_or(Value::is_null) {
            return false;
        }
        if self.name.starts_with("propose_") {
            return self.value["status"] == "created";
        }
        true
    }

    fn results(&self) -> usize {
        self.value["results"].as_array().map_or(0, Vec::len)
    }
}

/// El estado de un turno respondido. Precedencia, de arriba abajo:
///
/// 1. `documents`: hay al menos una cita válida mostrada.
/// 2. `data`, si se creó una propuesta: la tarjeta, compuesta por la aplicación con datos reales,
///    es la evidencia visible.
/// 3. `uncited`: se leyeron documentos (`search` con resultados o `get_document`) y ninguno se cita.
/// 4. `data`: se ejecutó con éxito una herramienta de datos (`aggregate`, `list_tasks`,
///    `list_rules`, `list_suggestions`, `history`, `mail_accounts`, `ai_status`).
/// 5. `no_results`: se buscó y la búsqueda no devolvió nada.
/// 6. `none`.
///
/// `open_view` solo navega y no cuenta como lectura.
pub(crate) fn classify(valid_citations: usize, records: &[ToolRecord]) -> Grounding {
    if valid_citations > 0 {
        return Grounding::Documents;
    }
    let ok: Vec<&ToolRecord> = records.iter().filter(|r| r.succeeded()).collect();
    if ok.iter().any(|r| r.name.starts_with("propose_")) {
        return Grounding::Data;
    }
    let read_documents = ok
        .iter()
        .any(|r| (r.name == "search" && r.results() > 0) || r.name == "get_document");
    if read_documents {
        return Grounding::Uncited;
    }
    if ok.iter().any(|r| DATA_TOOLS.contains(&r.name.as_str())) {
        return Grounding::Data;
    }
    if ok.iter().any(|r| r.name == "search") {
        return Grounding::NoResults;
    }
    Grounding::None
}

/// Las fuentes que respaldan las cifras de una respuesta: los resultados de las herramientas de
/// este turno, los documentos citados, la pregunta de la persona y, de la conversación, lo que la
/// persona escribió antes y las respuestas anteriores cuyo respaldo fue `documents` o `data`.
pub(crate) fn sources_for(
    records: &[ToolRecord],
    cited: &[SearchHit],
    question: &str,
    earlier: &[StoredTurn],
) -> Sources {
    let mut sources = Sources::new();
    for record in records.iter().filter(|r| r.succeeded()) {
        sources.add_json(&record.value);
    }
    // Los campos y el fragmento de cada documento citado, tal como están en el índice.
    let text = Text::new(false);
    for hit in cited {
        sources.add_json(&hit_value(hit, &text, true));
    }
    sources.add_text(question);
    for turn in earlier {
        if turn.role == "user" {
            sources.add_text(&turn.text);
            continue;
        }
        let backed = turn.meta["outcome"].as_str() == Some("answered")
            && turn.meta["grounding"]
                .as_str()
                .is_some_and(|g| g == "documents" || g == "data");
        if !backed {
            continue;
        }
        // Las cifras que ese turno ya dio sin origen no respaldan a otro posterior.
        let excluded: HashSet<String> = turn.meta["unsourced_keys"]
            .as_array()
            .map(|keys| {
                keys.iter()
                    .filter_map(|k| k.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        sources.add_backed_text(&turn.text, &excluded);
    }
    sources
}

/// Cifras de la respuesta que no salen de las fuentes.
pub(crate) fn unsourced(
    answer: &str,
    records: &[ToolRecord],
    cited: &[SearchHit],
    question: &str,
    earlier: &[StoredTurn],
) -> Unsourced {
    unsourced_figures(answer, &sources_for(records, cited, question, earlier))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rec(name: &str, value: Value) -> ToolRecord {
        ToolRecord {
            name: name.to_owned(),
            value,
        }
    }

    fn hits() -> Value {
        json!({ "results": [{"ref": "doc:1"}], "total": 1 })
    }

    #[test]
    fn a_valid_citation_is_documents_whatever_else_happened() {
        assert_eq!(classify(1, &[]), Grounding::Documents);
        assert_eq!(
            classify(2, &[rec("aggregate", json!({"documents": 3}))]),
            Grounding::Documents
        );
    }

    #[test]
    fn reading_documents_without_citing_is_uncited() {
        assert_eq!(classify(0, &[rec("search", hits())]), Grounding::Uncited);
        assert_eq!(
            classify(
                0,
                &[rec(
                    "get_document",
                    json!({"ref": "doc:1", "fragments": []})
                )]
            ),
            Grounding::Uncited
        );
        // Aunque también se haya ejecutado una herramienta de datos.
        assert_eq!(
            classify(
                0,
                &[
                    rec("search", hits()),
                    rec("aggregate", json!({"documents": 1}))
                ]
            ),
            Grounding::Uncited
        );
    }

    #[test]
    fn data_tools_give_data() {
        for tool in DATA_TOOLS {
            assert_eq!(
                classify(0, &[rec(tool, json!({"x": 1}))]),
                Grounding::Data,
                "{tool}"
            );
        }
    }

    #[test]
    fn a_created_proposal_is_data_even_after_reading_documents() {
        let records = [
            rec("search", hits()),
            rec(
                "propose_approve",
                json!({"proposal": "prop:1", "status": "created"}),
            ),
        ];
        assert_eq!(classify(0, &records), Grounding::Data);
    }

    #[test]
    fn an_empty_search_is_no_results_and_data_wins_over_it() {
        let empty = rec("search", json!({"results": [], "total": 0}));
        assert_eq!(
            classify(0, std::slice::from_ref(&empty)),
            Grounding::NoResults
        );
        assert_eq!(
            classify(0, &[empty, rec("aggregate", json!({"documents": 0}))]),
            Grounding::Data
        );
    }

    #[test]
    fn no_tool_or_a_failed_one_is_none() {
        assert_eq!(classify(0, &[]), Grounding::None);
        // Un error, una propuesta rechazada y una navegación no son lecturas.
        let records = [
            rec("search", json!({"error": "invalid_filter"})),
            rec("get_document", json!({"error": "not_found"})),
            rec("aggregate", json!({"error": "unavailable"})),
            rec("propose_approve", json!({"error": "not_requested"})),
            rec("propose_reject", json!({"status": "pending"})),
            rec("open_view", json!({"opened": "rules"})),
        ];
        assert_eq!(classify(0, &records), Grounding::None);
    }

    #[test]
    fn only_documents_and_data_back_later_answers() {
        assert!(Grounding::Documents.backs_later_answers());
        assert!(Grounding::Data.backs_later_answers());
        for g in [Grounding::NoResults, Grounding::Uncited, Grounding::None] {
            assert!(!g.backs_later_answers());
        }
    }

    fn turn(role: &str, text: &str, meta: Value) -> StoredTurn {
        StoredTurn {
            id: 1,
            role: role.to_owned(),
            text: text.to_owned(),
            meta,
            created_at: 0,
        }
    }

    #[test]
    fn earlier_turns_back_an_answer_only_when_they_were_backed() {
        let earlier = [
            turn("user", "¿cuánto debo a Ejemplo?", json!({})),
            turn(
                "assistant",
                "Debe 500,00 € (y 77,00 € más)",
                json!({"outcome": "answered", "grounding": "data", "unsourced_keys": ["n:77"]}),
            ),
            turn(
                "assistant",
                "Son 910,00 €",
                json!({"outcome": "answered", "grounding": "none"}),
            ),
            turn(
                "assistant",
                "Son 920,00 €",
                json!({"outcome": "answered", "grounding": "uncited"}),
            ),
            turn("assistant", "Son 930,00 €", json!({"outcome": "answered"})),
        ];
        let answer = "De nuevo 500,00 €, 77,00 €, 910,00 €, 920,00 € y 930,00 €.";
        let u = unsourced(answer, &[], &[], "otra pregunta", &earlier);
        assert_eq!(u.shown, vec!["77,00 €", "910,00 €", "920,00 €", "930,00 €"]);
        assert_eq!(u.total, 4);
    }

    #[test]
    fn tool_results_of_the_turn_back_the_answer_and_failed_ones_do_not() {
        let records = [
            rec(
                "aggregate",
                json!({"by_currency": [{"total": "1234.50", "documents": 14}]}),
            ),
            rec("search", json!({"error": "unavailable", "x": "999,99 €"})),
        ];
        let u = unsourced(
            "Suman 1.234,50 € en 14 facturas; otra de 999,99 €.",
            &records,
            &[],
            "suma",
            &[],
        );
        assert_eq!(u.shown, vec!["999,99 €"]);
    }
}
