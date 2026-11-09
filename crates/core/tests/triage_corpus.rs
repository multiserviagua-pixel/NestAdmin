//! Paridad de la capa local (D157) con el corpus de triaje (D164): la ruta de producción
//! (`mail_signals::route`) debe coincidir con la esperada en `evaluation/data/expected/triage.json`,
//! que se etiquetó a mano. El evaluador de Python (`evaluate_triage.py`) reimplementa la misma
//! lógica y lo comprueba contra las mismas etiquetas, de modo que ambos lados quedan atados a
//! la referencia y no uno al otro.

use std::fs;
use std::path::PathBuf;

use nestadmin_core::mail_signals::{detect, route};
use serde_json::Value;

fn corpus(kind: &str) -> Vec<Value> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../evaluation/data")
        .join(kind)
        .join("triage.json");
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn local_route_matches_the_hand_labelled_corpus() {
    let inputs = corpus("inputs");
    let expected = corpus("expected");
    assert_eq!(inputs.len(), expected.len());
    assert!(inputs.len() >= 140);
    let mut hidden_false_negatives = 0;
    for (case, gold) in inputs.iter().zip(&expected) {
        let id = case["id"].as_str().unwrap();
        assert_eq!(gold["id"], case["id"]);
        let input = &case["input"];
        let headers: Vec<(&str, &str)> = input["headers"]
            .as_array()
            .unwrap()
            .iter()
            .map(|h| (h["name"].as_str().unwrap(), h["value"].as_str().unwrap()))
            .collect();
        let signals = detect(input["from"].as_str(), Some(&headers));
        let got = route(&signals).as_str();
        assert_eq!(got, gold["expected"]["route"], "{id}");
        if got == "informativo" && gold["expected"]["needs_reply"] == true {
            hidden_false_negatives += 1;
        }
    }
    assert_eq!(hidden_false_negatives, 0);
}
