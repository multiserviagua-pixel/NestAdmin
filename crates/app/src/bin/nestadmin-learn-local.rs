//! Capa local de la evaluación de «aprender a responder» (D164, T33h). No usa IA ni red.
//!
//! Pasa el corpus sintético `evaluation/data/inputs/learn.json` por la recogida local de
//! `nestadmin_app::learn` (filtro de señales, emparejado, recorte de cita, firma, alias y
//! enmascarado) y escribe, por caso, solo el resultado (`pass` o `discard`), el código del descarte
//! y qué cadenas prohibidas siguen en lo que saldría (índice y tipo). Nunca escribe el texto de
//! los pares ni el de las cadenas prohibidas.
//!
//! Para buscar las cadenas prohibidas lee `evaluation/data/expected/learn.json`: aquí no hay
//! ningún modelo al que esas expectativas puedan llegar.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use nestadmin_app::learn::{
    build_pairs, payload, select_candidates, Collected, LearnCounts, LearnPair,
};
use nestadmin_simulators::{Message, OwnReply};
use serde::Deserialize;
use serde_json::{json, Value};

const USAGE: &str =
    "uso: nestadmin-learn-local --out ARCHIVO.jsonl [--inputs DIR] [--expected DIR]";

#[derive(Deserialize)]
struct Header {
    name: String,
    value: String,
}

#[derive(Deserialize)]
struct OwnIn {
    to: Vec<String>,
    body: Option<String>,
}

#[derive(Deserialize)]
struct Input {
    from: Option<String>,
    to: Vec<String>,
    date: Option<String>,
    subject: Option<String>,
    body: Option<String>,
    headers: Option<Vec<Header>>,
    thread_key: Option<String>,
    own_reply: Option<OwnIn>,
}

#[derive(Deserialize)]
struct Case {
    id: String,
    input: Input,
}

#[derive(Deserialize)]
struct Forbidden {
    kind: String,
    text: String,
}

#[derive(Deserialize)]
struct Expect {
    #[serde(default)]
    forbidden: Vec<Forbidden>,
}

#[derive(Deserialize)]
struct Gold {
    id: String,
    expected: Expect,
}

fn message(case: &Case) -> Message {
    let i = &case.input;
    let message = Message {
        id: case.id.clone(),
        subject: i.subject.clone(),
        from: i.from.clone(),
        to: Some(i.to.clone()),
        body: i.body.clone(),
        date: i.date.clone(),
        attachments: Vec::new(),
        signals: None,
        thread_key: i.thread_key.clone(),
    };
    match &i.headers {
        Some(headers) => {
            let pairs: Vec<(&str, &str)> = headers
                .iter()
                .map(|h| (h.name.as_str(), h.value.as_str()))
                .collect();
            message.with_checked_signals(&pairs)
        }
        None => message,
    }
}

/// Pasa por la recogida local los casos de `cases` (todos los de la tanda, para que la firma
/// compartida se detecte como en la aplicación).
fn collect(cases: &[&Case]) -> Collected {
    let own: HashMap<&str, &OwnIn> = cases
        .iter()
        .filter_map(|c| c.input.own_reply.as_ref().map(|o| (c.id.as_str(), o)))
        .collect();
    let selection = select_candidates(cases.iter().map(|c| message(c)));
    build_pairs(selection, true, |_thread, id| {
        Ok(own.get(id).map(|o| OwnReply {
            id: format!("sent-{id}"),
            date: None,
            body: o.body.clone(),
            to: o.to.clone(),
        }))
    })
}

/// Códigos de descarte de [`LearnCounts`] con su valor.
fn discards(c: &LearnCounts) -> [(&'static str, usize); 10] {
    [
        ("unchecked", c.unchecked),
        ("automatic", c.automatic),
        ("no_sender", c.no_sender),
        ("no_thread", c.no_thread),
        ("no_own_reply", c.no_own_reply),
        ("quote_dropped", c.quote_dropped),
        ("not_reply_to_sender", c.not_reply_to_sender),
        ("signature_dropped", c.signature_dropped),
        ("empty_text", c.empty_text),
        ("unmasked", c.unmasked),
    ]
}

/// Solo letras y cifras, en minúsculas: iguala «ES91 2100-…», «12.345.678-Z» o «(91) 555 01 23».
fn compact(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Minúsculas y espacios compactados.
fn fold(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// ¿Aparece `needle` en `hay` como palabra entera?
fn contains_word(hay: &str, needle: &str) -> bool {
    let hay: Vec<char> = hay.chars().collect();
    let needle: Vec<char> = needle.chars().collect();
    if needle.is_empty() || hay.len() < needle.len() {
        return false;
    }
    (0..=hay.len() - needle.len()).any(|i| {
        hay[i..i + needle.len()] == needle[..]
            && (i == 0 || !hay[i - 1].is_alphanumeric())
            && hay
                .get(i + needle.len())
                .is_none_or(|c| !c.is_alphanumeric())
    })
}

/// ¿Sigue `item` en el texto que saldría?
fn leaks(text: &str, item: &Forbidden) -> bool {
    match item.kind.as_str() {
        "iban" | "dni" | "phone" | "card" => compact(text).contains(&compact(&item.text)),
        "name" | "alias_local" => contains_word(&fold(text), &fold(&item.text)),
        _ => fold(text).contains(&fold(&item.text)),
    }
}

fn run() -> Result<(), String> {
    let mut out = None;
    let mut inputs = PathBuf::from("evaluation/data/inputs");
    let mut expected = PathBuf::from("evaluation/data/expected");
    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        let mut value = || {
            args.next()
                .ok_or_else(|| format!("falta el valor de {flag}"))
        };
        match flag.as_str() {
            "--out" => out = Some(PathBuf::from(value()?)),
            "--inputs" => inputs = PathBuf::from(value()?),
            "--expected" => expected = PathBuf::from(value()?),
            other => return Err(format!("argumento desconocido: {other}")),
        }
    }
    let out = out.ok_or("--out es obligatorio")?;
    let read = |dir: &PathBuf| {
        fs::read_to_string(dir.join("learn.json"))
            .map_err(|e| format!("{}: {e}", dir.join("learn.json").display()))
    };
    let cases: Vec<Case> = serde_json::from_str(&read(&inputs)?).map_err(|e| e.to_string())?;
    let gold: Vec<Gold> = serde_json::from_str(&read(&expected)?).map_err(|e| e.to_string())?;
    let forbidden: HashMap<&str, &Vec<Forbidden>> = gold
        .iter()
        .map(|g| (g.id.as_str(), &g.expected.forbidden))
        .collect();

    let all: Vec<&Case> = cases.iter().collect();
    let batch = collect(&all);
    let passed: HashMap<&str, &LearnPair> = batch
        .pairs
        .iter()
        .map(|p| (p.message_id.as_str(), p))
        .collect();

    let mut lines = Vec::new();
    for case in &cases {
        let result = match passed.get(case.id.as_str()) {
            Some(pair) => {
                let text = payload(std::slice::from_ref(*pair));
                let found: Vec<Value> = forbidden
                    .get(case.id.as_str())
                    .into_iter()
                    .flat_map(|items| items.iter().enumerate())
                    .filter(|(_, item)| leaks(&text, item))
                    .map(|(index, item)| json!({ "index": index, "kind": item.kind }))
                    .collect();
                json!({ "outcome": "pass", "reason": null, "leaks": found })
            }
            None => {
                // El motivo es lo que cambia en los recuentos al quitar este caso de la tanda.
                let rest: Vec<&Case> = all.iter().copied().filter(|c| c.id != case.id).collect();
                let without = discards(&collect(&rest).counts);
                let with = discards(&batch.counts);
                let reasons: Vec<&str> = with
                    .iter()
                    .zip(without.iter())
                    .filter(|((_, a), (_, b))| a > b)
                    .map(|((code, _), _)| *code)
                    .collect();
                let reason = if reasons.is_empty() {
                    "unknown".to_owned()
                } else {
                    reasons.join("+")
                };
                json!({ "outcome": "discard", "reason": reason, "leaks": [] })
            }
        };
        lines.push(json!({ "id": case.id, "result": result }).to_string());
    }
    if let Some(dir) = out.parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    fs::write(&out, lines.join("\n") + "\n").map_err(|e| e.to_string())?;
    let meta = json!({
        "layer": "local",
        "cases": cases.len(),
        "pairs": batch.pairs.len(),
        "counts": batch.counts,
    });
    let meta_path = out.with_extension("meta.json");
    fs::write(
        &meta_path,
        serde_json::to_string_pretty(&meta).map_err(|e| e.to_string())? + "\n",
    )
    .map_err(|e| e.to_string())?;
    println!(
        "{} casos, {} pares; predicciones en {}",
        cases.len(),
        batch.pairs.len(),
        out.display()
    );
    Ok(())
}

fn main() {
    if let Err(message) = run() {
        eprintln!("error: {message}\n{USAGE}");
        std::process::exit(2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(kind: &str, text: &str) -> Forbidden {
        Forbidden {
            kind: kind.into(),
            text: text.into(),
        }
    }

    #[test]
    fn numeric_kinds_ignore_separators_and_case() {
        assert!(leaks(
            "IBAN es91 2100-0418 4502 0005 1332.",
            &item("iban", "ES9121000418450200051332")
        ));
        assert!(leaks("DNI 12.345.678-z", &item("dni", "12345678Z")));
        assert!(leaks(
            "llama al (91) 555 01 23",
            &item("phone", "915550123")
        ));
        assert!(!leaks("llama al [TELÉFONO]", &item("phone", "915550123")));
    }

    #[test]
    fn names_match_whole_words_only() {
        assert!(leaks("Hola Ana, gracias", &item("name", "ana")));
        assert!(!leaks("hasta mañana", &item("name", "ana")));
        assert!(leaks("Hola R1,\n\nsoy ANA GIL", &item("name", "Ana Gil")));
        assert!(!leaks("Hola R1,", &item("alias_local", "carmen")));
    }

    #[test]
    fn text_kinds_match_as_substrings_ignoring_case_and_spacing() {
        assert!(leaks(
            "Administración  ·\nTaller Ficticio S.L.",
            &item("signature", "administración · taller ficticio s.l.")
        ));
        assert!(!leaks("Un saludo,", &item("signature", "Elena Duarte")));
    }

    #[test]
    fn discard_codes_cover_every_counter_that_drops_a_case() {
        let counts = LearnCounts {
            unchecked: 1,
            automatic: 2,
            no_sender: 3,
            no_thread: 4,
            no_own_reply: 5,
            quote_dropped: 6,
            not_reply_to_sender: 7,
            signature_dropped: 8,
            empty_text: 9,
            unmasked: 10,
            ..LearnCounts::default()
        };
        let values: Vec<usize> = discards(&counts).iter().map(|(_, n)| *n).collect();
        assert_eq!(values, (1..=10).collect::<Vec<_>>());
    }
}
