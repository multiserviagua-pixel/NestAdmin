//! Evidencia local de moneda (D127). El modelo no devuelve la moneda y su instrucción le pide
//! importes sin símbolo, así que la moneda se busca en el texto que el equipo ya extrajo:
//! una función pura y determinista, sin IA. Un falso positivo solo manda la factura a
//! revisión; un falso negativo es una factura en otra moneda sin ninguna marca en el texto,
//! límite conocido de D127.

use serde::{Deserialize, Serialize};

/// Lo que el texto dice de la moneda. Solo códigos: nunca el contenido del texto.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CurrencyEvidence {
    /// Códigos ISO de otras monedas mencionadas, ordenados y sin repetir. `KR` es una
    /// corona de país no determinado (`kr` solo).
    pub foreign: Vec<String>,
    /// El texto menciona el euro (`€`, `EUR`, «euro»).
    pub euro: bool,
}

/// Símbolos de otras monedas. `$` se trata aparte por sus prefijos (`US$`, `C$`…).
const SYMBOLS: &[(char, &str)] = &[
    ('£', "GBP"),
    ('¥', "JPY"),
    ('₽', "RUB"),
    ('₹', "INR"),
    ('₩', "KRW"),
    ('₺', "TRY"),
    ('₪', "ILS"),
    ('₴', "UAH"),
    ('₫', "VND"),
    ('₱', "PHP"),
    ('₦', "NGN"),
    ('฿', "THB"),
    ('₸', "KZT"),
    ('₼', "AZN"),
    ('₾', "GEL"),
    ('₡', "CRC"),
    ('₲', "PYG"),
    ('₮', "MNT"),
    ('円', "JPY"),
    ('元', "CNY"),
];

/// Códigos ISO 4217 que se aceptan en cualquier caso (`usd 500`): no son palabras corrientes.
const STRONG: &[&str] = &[
    "USD", "GBP", "CHF", "JPY", "CAD", "AUD", "NZD", "MXN", "CNY", "RMB", "INR", "BRL", "KRW",
    "SEK", "NOK", "DKK", "PLN", "CZK", "HUF", "AED", "ARS", "CLP", "HKD", "SGD",
];

/// Códigos que son palabras habituales en español o inglés (`ALL`, `CUP`, `MAD`, `TOP`,
/// `SOS`, `PEN`…): solo cuentan en mayúsculas y con una cifra al lado o tras «moneda».
const AMBIGUOUS: &[&str] = &[
    "ALL", "CUP", "MAD", "TOP", "SOS", "PEN", "COP", "TRY", "GEL", "RON", "DOP", "MOP", "BOB",
    "BAM", "ANG", "AMD", "CRC", "SAR", "ISK", "LAK", "PAB", "NIO", "MNT",
];

/// Resto de ISO 4217 (sin el euro): en mayúsculas y palabra completa.
const OTHER: &[&str] = &[
    "AFN", "AOA", "AWG", "AZN", "BBD", "BDT", "BGN", "BHD", "BIF", "BMD", "BND", "BSD", "BTN",
    "BWP", "BYN", "BZD", "CDF", "CVE", "DJF", "DZD", "EGP", "ERN", "ETB", "FJD", "FKP", "GHS",
    "GIP", "GMD", "GNF", "GTQ", "GYD", "HNL", "HTG", "IDR", "ILS", "IQD", "IRR", "JMD", "JOD",
    "KES", "KGS", "KHR", "KMF", "KPW", "KWD", "KYD", "KZT", "LBP", "LKR", "LRD", "LSL", "LYD",
    "MDL", "MGA", "MKD", "MMK", "MRU", "MUR", "MVR", "MWK", "MYR", "MZN", "NAD", "NGN", "NPR",
    "OMR", "PGK", "PHP", "PKR", "PYG", "QAR", "RSD", "RUB", "RWF", "SBD", "SCR", "SDG", "SHP",
    "SLE", "SLL", "SRD", "SSP", "STN", "SVC", "SYP", "SZL", "THB", "TJS", "TMT", "TND", "TTD",
    "TWD", "TZS", "UAH", "UGX", "UYU", "UZS", "VES", "VND", "VUV", "WST", "XAF", "XCD", "XOF",
    "XPF", "YER", "ZAR", "ZMW", "ZWL",
];

/// Palabras que nombran sin ambigüedad una moneda (en minúsculas).
const NAMES: &[(&str, &str)] = &[
    ("dólar", "USD"),
    ("dólares", "USD"),
    ("dolar", "USD"),
    ("dolares", "USD"),
    ("dollar", "USD"),
    ("dollars", "USD"),
    ("pound", "GBP"),
    ("pounds", "GBP"),
    ("yen", "JPY"),
    ("yenes", "JPY"),
    ("yuan", "CNY"),
    ("kr", "KR"),
    ("zł", "PLN"),
    ("kč", "CZK"),
    // Plurales en español. `pesos` no dice de qué país: `PESO`, como `KR` con las coronas.
    ("pesos", "PESO"),
    ("libras", "GBP"),
    ("francos", "CHF"),
    ("reales", "BRL"),
    ("soles", "PEN"),
];

/// Prefijos de letras antes de `$` y la moneda que indican.
const DOLLARS: &[(&str, &str)] = &[
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

/// Pasa a ASCII las formas de ancho completo (`＄`, `ＵＳＤ`, `５００`) y a su símbolo
/// normal las monedas de ancho completo (`￥`, `￡`, `￦`).
fn normalize(text: &str) -> Vec<char> {
    text.chars()
        .map(|c| match c {
            '\u{FF01}'..='\u{FF5E}' => char::from_u32(c as u32 - 0xFEE0).unwrap_or(c),
            '\u{FFE5}' => '¥',
            '\u{FFE1}' => '£',
            '\u{FFE6}' => '₩',
            '\u{FE69}' => '$',
            other => other,
        })
        .collect()
}

/// Palabra del texto sin las cifras de sus extremos: `USD500` y `500USD` son `USD`.
struct Word {
    text: String,
    start: usize,
    end: usize,
    /// Lleva alguna cifra: hay un importe en esta palabra.
    has_digit: bool,
    /// Las cifras iban pegadas a letras.
    digits_attached: bool,
}

fn words(chars: &[char]) -> Vec<Word> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if !is_word(chars[i]) {
            i += 1;
            continue;
        }
        let start = i;
        while i < chars.len() && is_word(chars[i]) {
            i += 1;
        }
        let raw: String = chars[start..i].iter().collect();
        let text = raw.trim_matches(|c: char| c.is_ascii_digit()).to_owned();
        out.push(Word {
            has_digit: raw.chars().any(|c| c.is_ascii_digit()),
            digits_attached: !text.is_empty() && text.len() != raw.len(),
            text,
            start,
            end: i,
        });
    }
    out
}

/// Separación horizontal entre dos palabras: espacios (también varios o tabuladores), dos
/// puntos y paréntesis, sin salto de línea y de hasta 40 caracteres: `500.00   PEN`,
/// `(TRY): 500,00`.
fn adjacent(chars: &[char], a_end: usize, b_start: usize) -> bool {
    b_start >= a_end
        && b_start - a_end <= 40
        && chars[a_end..b_start]
            .iter()
            .all(|c| matches!(c, ' ' | '\u{a0}' | ':' | '\t' | '(' | ')'))
}

/// `S/` (soles) delante de un importe: `S/ 500`, `S/.500`, `S/ 500,00`. No cuenta `S/N`,
/// `A/S` ni una referencia como `S/2026-001`.
fn is_soles_symbol(chars: &[char], i: usize) -> bool {
    if chars[i] != 'S' || chars.get(i + 1) != Some(&'/') || (i > 0 && is_word(chars[i - 1])) {
        return false;
    }
    let mut j = i + 2;
    while chars
        .get(j)
        .is_some_and(|c| matches!(c, ' ' | '\u{a0}' | '.' | '\t'))
    {
        j += 1;
    }
    if !chars.get(j).is_some_and(char::is_ascii_digit) {
        return false;
    }
    while j < chars.len() {
        let digit = chars[j].is_ascii_digit();
        let sep =
            matches!(chars[j], '.' | ',') && chars.get(j + 1).is_some_and(char::is_ascii_digit);
        if !digit && !sep {
            break;
        }
        j += 1;
    }
    // Una cifra seguida de `-`, `/` o letras es una referencia, no un importe.
    !chars
        .get(j)
        .is_some_and(|c| matches!(c, '-' | '/' | '_') || c.is_alphabetic())
}

/// Busca en `text` menciones de otras monedas y del euro. No distingue dónde aparecen: un
/// dólar mencionado en cualquier parte del documento ya impide tratarlo como factura en euros.
pub fn currency_evidence(text: &str) -> CurrencyEvidence {
    let chars = normalize(text);
    let mut foreign: Vec<String> = Vec::new();
    let mut euro = false;
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
        } else if c == '€' {
            euro = true;
        } else if is_soles_symbol(&chars, i) {
            foreign.push("PEN".to_owned());
        }
    }
    let words = words(&chars);
    for (i, w) in words.iter().enumerate() {
        if w.text.is_empty() {
            continue;
        }
        let upper = w.text.to_uppercase();
        let lower = w.text.to_lowercase();
        if matches!(lower.as_str(), "eur" | "euro" | "euros") {
            euro = true;
        } else if let Some((_, code)) = NAMES.iter().find(|(n, _)| *n == lower) {
            // Palabras corrientes en español: solo con un importe delante (`300 libras`).
            let common = matches!(
                lower.as_str(),
                "pesos" | "reales" | "francos" | "soles" | "libras"
            );
            let amount_before =
                i > 0 && words[i - 1].has_digit && adjacent(&chars, words[i - 1].end, w.start);
            if !common || amount_before {
                foreign.push((*code).to_owned());
            }
        } else if STRONG.contains(&upper.as_str()) {
            foreign.push(upper);
        } else if w.text != upper {
            // El resto de códigos solo cuentan en mayúsculas.
        } else if AMBIGUOUS.contains(&upper.as_str()) {
            // Palabra corriente: solo con un importe al lado o tras «moneda».
            let amount_before =
                i > 0 && words[i - 1].has_digit && adjacent(&chars, words[i - 1].end, w.start);
            let amount_after = words
                .get(i + 1)
                .is_some_and(|n| n.has_digit && adjacent(&chars, w.end, n.start));
            let named = i > 0
                && matches!(
                    words[i - 1].text.to_lowercase().as_str(),
                    "moneda" | "divisa" | "currency"
                )
                && adjacent(&chars, words[i - 1].end, w.start);
            if w.digits_attached || amount_before || amount_after || named {
                foreign.push(upper);
            }
        } else if OTHER.contains(&upper.as_str()) {
            foreign.push(upper);
        }
    }
    // «libras esterlinas»: dos palabras seguidas.
    for pair in words.windows(2) {
        let (a, b) = (pair[0].text.to_lowercase(), pair[1].text.to_lowercase());
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
    fn a_code_glued_to_the_amount_is_found() {
        for text in [
            "Total USD500.00",
            "Total 500USD",
            "Total 500.00USD",
            "Total USD500",
        ] {
            assert_eq!(foreign(text), ["USD"], "{text}");
        }
        // Antes se daba por bueno que `CAD1 2CAD` no se detectaba: era el fallo.
        assert_eq!(foreign("CAD1 2CAD"), ["CAD"]);
        assert!(currency_evidence("Total 500EUR").euro);
        assert!(currency_evidence("Total EUR500").euro);
        assert!(foreign("Total 500EUR").is_empty());
    }

    #[test]
    fn full_width_lowercase_and_named_currencies_are_found() {
        assert_eq!(foreign("Total ￥1000"), ["JPY"]);
        assert_eq!(foreign("Total ＄500"), ["USD"]);
        assert_eq!(foreign("Total ￡20"), ["GBP"]);
        assert_eq!(foreign("Total ＵＳＤ ５００"), ["USD"]);
        assert_eq!(foreign("usd 500"), ["USD"]);
        assert_eq!(foreign("Total 20 dollars"), ["USD"]);
        assert_eq!(foreign("20 pounds"), ["GBP"]);
        assert_eq!(foreign("Total 300 kr"), ["KR"]);
        assert_eq!(foreign("Total 300 SEK"), ["SEK"]);
        assert_eq!(foreign("Total 300 THB y 20 ZAR"), ["THB", "ZAR"]);
        // Códigos que solo existen en el resto de ISO 4217 siguen pidiendo mayúsculas.
        assert!(foreign("thb zar kes").is_empty());
        // `kr` solo cuenta como palabra completa.
        assert!(foreign("krypton okra").is_empty());
    }

    #[test]
    fn common_words_that_are_codes_need_an_amount_or_a_currency_label() {
        for text in [
            "ALL RIGHTS RESERVED",
            "TOP SECRET",
            "SOS urgente",
            "pen drive y PEN drive",
            "Total 500\nALL RIGHTS",
            "MAD MAX",
            "CUP de café, try again",
            "gel y ron, DOP",
        ] {
            assert!(foreign(text).is_empty(), "{text}");
        }
        assert_eq!(foreign("Total 500.00 PEN"), ["PEN"]);
        assert_eq!(foreign("Total PEN 500"), ["PEN"]);
        assert_eq!(foreign("Total PEN500"), ["PEN"]);
        assert_eq!(foreign("Moneda: MAD"), ["MAD"]);
        // En minúsculas, nunca: el texto es corriente.
        assert!(foreign("total 500 pen").is_empty());
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
        // Palabras que contienen un código: no cuentan.
        for text in [
            "USDA SCAD CADENA MUSDX",
            "CADENA USDA SCAD",
            "sin mención",
            "cadena de pen drive try again",
            "usd_x",
        ] {
            assert!(foreign(text).is_empty(), "{text}");
        }
        // Euro y otra moneda a la vez: queda la extranjera para que lo revise una persona.
        let both = currency_evidence("Total 100 € (aprox. 108 USD)");
        assert_eq!((both.foreign, both.euro), (vec!["USD".to_owned()], true));
    }

    #[test]
    fn more_names_and_symbols_are_found() {
        assert_eq!(foreign("Total 500 pesos"), ["PESO"]);
        assert_eq!(foreign("30 libras y 5 francos"), ["CHF", "GBP"]);
        assert_eq!(foreign("Total 80 reales"), ["BRL"]);
        assert_eq!(foreign("Total 100 soles"), ["PEN"]);
        assert_eq!(foreign("Total S/ 100.00"), ["PEN"]);
        assert_eq!(foreign("Total S/.100"), ["PEN"]);
        assert_eq!(foreign("Total 2500 Kč"), ["CZK"]);
        assert_eq!(foreign("合計 5000円"), ["JPY"]);
        assert_eq!(foreign("合计 300元"), ["CNY"]);
        assert_eq!(foreign("Total 500,00 pesos"), ["PESO"]);
        assert_eq!(foreign("Total 300 libras"), ["GBP"]);
        assert_eq!(foreign("Total S/ 500,00"), ["PEN"]);
        // Palabras corrientes y referencias sin importe: no son moneda.
        for text in [
            "consumos reales 121,00 €",
            "pesos de transporte",
            "Factura S/2026-001",
            "las libras y los soles",
            "Ref. S/2026-001 total 100 €",
        ] {
            assert!(foreign(text).is_empty(), "{text}");
        }
        assert!(currency_evidence("consumos reales 121,00 €").euro);
        // `S/N` y `A/S` no son soles.
        assert!(foreign("Ref. S/N 123 y Nordic A/S").is_empty());
    }

    #[test]
    fn wide_spacing_and_parentheses_do_not_hide_a_code() {
        assert_eq!(foreign("Total 500.00   PEN"), ["PEN"]);
        assert_eq!(foreign("Total 500.00\t\tPEN"), ["PEN"]);
        assert_eq!(foreign("Total PEN      500"), ["PEN"]);
        assert_eq!(foreign("Importe (TRY): 500,00"), ["TRY"]);
        assert_eq!(foreign("Moneda   :   MAD"), ["MAD"]);
        // Un salto de línea sigue separando.
        assert!(foreign("Total 500.00\n   PEN drive").is_empty());
        assert!(foreign("TOP      SECRET").is_empty());
    }

    #[test]
    fn the_result_never_carries_text() {
        let e = currency_evidence("Cliente Secreto SL debe $ 5");
        let json = serde_json::to_string(&e).unwrap();
        assert_eq!(json, r#"{"foreign":["USD"],"euro":false}"#);
    }
}
