//! Evidencia local de moneda (D127). El modelo no devuelve la moneda y su instrucción le pide
//! importes sin símbolo, así que la moneda se busca en el texto que el equipo ya extrajo:
//! una función pura y determinista, sin IA. Un falso positivo solo manda la factura a
//! revisión; un falso negativo es una factura en otra moneda sin ninguna marca en el texto,
//! límite conocido de D127.

use serde::{Deserialize, Serialize};

/// Lo que el texto dice de la moneda. Solo códigos: nunca el contenido del texto.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CurrencyEvidence {
    /// Códigos ISO de otras monedas mencionadas, ordenados y sin repetir.
    pub foreign: Vec<String>,
    /// El texto menciona el euro (`€`, `EUR`, «euro»).
    pub euro: bool,
}

/// Símbolos de otras monedas. `$` se trata aparte por sus prefijos (`US$`, `C$`…).
const SYMBOLS: [(char, &str); 9] = [
    ('£', "GBP"),
    ('¥', "JPY"),
    ('₽', "RUB"),
    ('₹', "INR"),
    ('₩', "KRW"),
    ('₺', "TRY"),
    ('₪', "ILS"),
    ('₴', "UAH"),
    ('₫', "VND"),
];

/// Códigos que se buscan como palabra completa y en mayúsculas.
const CODES: [&str; 28] = [
    "USD", "GBP", "CHF", "JPY", "CAD", "AUD", "NZD", "MXN", "CNY", "RMB", "INR", "RUB", "BRL",
    "KRW", "SEK", "NOK", "DKK", "PLN", "CZK", "HUF", "TRY", "ZAR", "AED", "ARS", "CLP", "COP",
    "PEN", "HKD",
];

/// Prefijos de letras antes de `$` y la moneda que indican.
const DOLLARS: [(&str, &str); 6] = [
    ("US", "USD"),
    ("CA", "CAD"),
    ("C", "CAD"),
    ("AU", "AUD"),
    ("A", "AUD"),
    ("MX", "MXN"),
];

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Busca en `text` menciones de otras monedas y del euro. No distingue dónde aparecen: un
/// dólar mencionado en cualquier parte del documento ya impide tratarlo como factura en euros.
pub fn currency_evidence(text: &str) -> CurrencyEvidence {
    let chars: Vec<char> = text.chars().collect();
    let mut foreign: Vec<String> = Vec::new();
    for (i, &c) in chars.iter().enumerate() {
        if c == '$' {
            // Letras mayúsculas pegadas delante (`US$`), sin otra letra antes.
            let mut start = i;
            while start > 0 && chars[start - 1].is_ascii_uppercase() && i - start < 2 {
                start -= 1;
            }
            let prefix: String = chars[start..i].iter().collect();
            let attached = start == 0 || !is_word(chars[start - 1]);
            let code = DOLLARS
                .iter()
                .find(|(p, _)| attached && *p == prefix)
                .map_or("USD", |(_, code)| code);
            foreign.push(code.to_owned());
        } else if let Some((_, code)) = SYMBOLS.iter().find(|(s, _)| *s == c) {
            foreign.push((*code).to_owned());
        }
    }
    // Códigos en palabra completa: se parte el texto en palabras.
    let mut word = String::new();
    let mut words: Vec<String> = Vec::new();
    for c in text.chars().chain(std::iter::once(' ')) {
        if is_word(c) {
            word.push(c);
        } else if !word.is_empty() {
            words.push(std::mem::take(&mut word));
        }
    }
    let mut euro = text.contains('€');
    for w in &words {
        if let Some(code) = CODES.iter().find(|code| **code == w.as_str()) {
            foreign.push((*code).to_owned());
        }
        if w == "EUR" {
            euro = true;
        }
        let lower = w.to_lowercase();
        match lower.as_str() {
            "euro" | "euros" => euro = true,
            "dólar" | "dólares" | "dolar" | "dolares" => foreign.push("USD".to_owned()),
            _ => {}
        }
    }
    // «libras esterlinas»: dos palabras seguidas.
    for pair in words.windows(2) {
        let (a, b) = (pair[0].to_lowercase(), pair[1].to_lowercase());
        if matches!(a.as_str(), "libra" | "libras")
            && matches!(b.as_str(), "esterlina" | "esterlinas")
        {
            foreign.push("GBP".to_owned());
        }
    }
    foreign.sort();
    foreign.dedup();
    CurrencyEvidence { foreign, euro }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn foreign(text: &str) -> Vec<String> {
        currency_evidence(text).foreign
    }

    #[test]
    fn symbols_and_codes_are_found_once_and_sorted() {
        assert_eq!(foreign("Total: $ 500.00"), ["USD"]);
        assert_eq!(foreign("Total US$500.00"), ["USD"]);
        assert_eq!(foreign("Total 500.00 USD"), ["USD"]);
        assert_eq!(foreign("Total £120.00"), ["GBP"]);
        assert_eq!(foreign("C$ 10 y A$ 5 y MX$ 3"), ["AUD", "CAD", "MXN"]);
        assert_eq!(foreign("100 USD, $ 7 y CHF 5 USD"), ["CHF", "USD"]);
        assert_eq!(foreign("¥ 1000 ₽ 5 ₹ 7"), ["INR", "JPY", "RUB"]);
        assert_eq!(foreign("12 dólares y 3 libras esterlinas"), ["GBP", "USD"]);
    }

    #[test]
    fn plain_text_euro_and_words_that_contain_a_code_are_not_foreign() {
        assert_eq!(
            currency_evidence("Total 121,00 €"),
            CurrencyEvidence {
                foreign: vec![],
                euro: true
            }
        );
        assert!(currency_evidence("Total 121.00 EUR").euro);
        assert!(currency_evidence("importe en euros").euro);
        let none = currency_evidence("Total 121.00");
        assert!(none.foreign.is_empty() && !none.euro);
        // Palabras que contienen un código, o minúsculas: no cuentan.
        for text in [
            "USDA SCAD CADENA MUSDX",
            "cadena de pen drive try again",
            "usd_x CAD1 2CAD",
        ] {
            assert!(foreign(text).is_empty(), "{text}");
        }
        // Euro y otra moneda a la vez: queda la extranjera para que lo revise una persona.
        let both = currency_evidence("Total 100 € (aprox. 108 USD)");
        assert_eq!((both.foreign, both.euro), (vec!["USD".to_owned()], true));
    }

    #[test]
    fn the_result_never_carries_text() {
        let e = currency_evidence("Cliente Secreto SL debe $ 5");
        let json = serde_json::to_string(&e).unwrap();
        assert_eq!(json, r#"{"foreign":["USD"],"euro":false}"#);
    }
}
