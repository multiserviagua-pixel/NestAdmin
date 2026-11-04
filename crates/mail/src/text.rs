//! Normalización de mensajes (D34): HTML a texto, direcciones, fechas y codificación MIME
//! de la respuesta. Sin dependencias externas: las reglas son pocas y se prueban aquí.

use base64::engine::general_purpose::STANDARD;
use base64::Engine;

/// Convierte HTML a texto legible: quita `script`, `style` y comentarios, convierte los
/// cierres de bloque en saltos de línea, descodifica entidades y compacta espacios.
pub fn html_to_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let lower = html.to_ascii_lowercase();
    let mut i = 0;
    while i < html.len() {
        let rest = &lower[i..];
        if rest.starts_with("<!--") {
            i += rest.find("-->").map_or(rest.len(), |end| end + 3);
            continue;
        }
        if rest.starts_with('<') {
            let end = rest.find('>').map_or(rest.len(), |e| e + 1);
            let tag = rest
                .get(1..end.saturating_sub(1))
                .unwrap_or("")
                .trim_start_matches('/')
                .split(|c: char| c.is_whitespace() || c == '/')
                .next()
                .unwrap_or("");
            if (tag == "script" || tag == "style") && !rest.starts_with("</") {
                let close = format!("</{tag}");
                i += rest.find(&close).map_or(rest.len(), |c| {
                    c + rest[c..].find('>').map_or(rest.len() - c, |e| e + 1)
                });
                continue;
            }
            // Un salto de línea por límite de bloque, aunque coincidan el cierre y la apertura.
            let block = matches!(
                tag,
                "br" | "p" | "div" | "li" | "tr" | "h1" | "h2" | "h3" | "h4" | "blockquote"
            );
            if block && (tag == "br" || !out.ends_with('\n')) {
                out.push('\n');
            }
            i += end;
            continue;
        }
        let next = rest.find('<').unwrap_or(rest.len());
        out.push_str(&decode_entities(&html[i..i + next]));
        i += next;
    }
    tidy(&out)
}

fn decode_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('&') {
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        let end = rest[..rest.len().min(12)].find(';');
        let decoded = end.and_then(|end| {
            let name = &rest[1..end];
            let ch = match name {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" | "#39" => Some('\''),
                "nbsp" => Some(' '),
                _ => name
                    .strip_prefix("#x")
                    .or_else(|| name.strip_prefix("#X"))
                    .and_then(|hex| u32::from_str_radix(hex, 16).ok())
                    .or_else(|| name.strip_prefix('#').and_then(|d| d.parse().ok()))
                    .and_then(char::from_u32),
            };
            ch.map(|c| (c, end + 1))
        });
        match decoded {
            Some((c, used)) => {
                out.push(c);
                rest = &rest[used..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Espacios compactados por línea y como mucho una línea en blanco seguida.
fn tidy(text: &str) -> String {
    let mut lines = Vec::new();
    let mut blank = false;
    for line in text.lines() {
        let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if line.is_empty() {
            if !blank && !lines.is_empty() {
                lines.push(String::new());
            }
            blank = true;
        } else {
            lines.push(line);
            blank = false;
        }
    }
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    lines.join("\n")
}

/// Dirección simple `usuario@dominio.tld` sin espacios ni saltos de línea.
pub fn is_address(address: &str) -> bool {
    match address.split_once('@') {
        Some((user, domain)) => {
            !user.is_empty()
                && domain.contains('.')
                && !address.contains(|c: char| {
                    c.is_whitespace() || c.is_control() || c == '<' || c == '>' || c == ','
                })
        }
        None => false,
    }
}

/// Direcciones de una cabecera `To`/`From`: `Nombre <a@b.test>, c@d.test`. Las comas dentro
/// de comillas forman parte del nombre.
pub fn header_addresses(header: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for c in header.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                current.push(c);
            }
            ',' if !quoted => parts.push(std::mem::take(&mut current)),
            _ => current.push(c),
        }
    }
    parts.push(current);
    parts
        .iter()
        .filter_map(|part| {
            let part = part.trim();
            let address = match (part.rfind('<'), part.rfind('>')) {
                (Some(start), Some(end)) if start < end => &part[start + 1..end],
                _ => part,
            };
            let address = address.trim();
            is_address(address).then(|| address.to_owned())
        })
        .collect()
}

/// Valor seguro para una cabecera: sin saltos de línea que permitan inyectar otras.
pub fn header_value(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).collect()
}

/// Codifica una cabecera con caracteres no ASCII (RFC 2047, base64 en UTF-8).
pub fn encode_header(text: &str) -> String {
    let clean = header_value(text);
    if clean.is_ascii() {
        clean
    } else {
        format!("=?UTF-8?B?{}?=", STANDARD.encode(clean.as_bytes()))
    }
}

/// Base64 en líneas de 76 caracteres, como exige MIME.
pub fn mime_base64(bytes: &[u8]) -> String {
    let encoded = STANDARD.encode(bytes);
    encoded
        .as_bytes()
        .chunks(76)
        .map(|c| std::str::from_utf8(c).unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\r\n")
}

/// Bytes de una parte de texto: UTF-8 si es válido; si no, Latin-1, que nunca falla y
/// cubre los correos antiguos en `iso-8859-1`/`windows-1252` mejor que sustituir bytes.
pub fn decode_text(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(text) => text.to_owned(),
        Err(_) => bytes.iter().map(|&b| b as char).collect(),
    }
}

/// Milisegundos desde 1970 a `AAAA-MM-DDTHH:MM:SSZ`, como las fechas del simulador.
pub fn format_utc_millis(millis: u64) -> String {
    let secs = millis / 1000;
    let days = (secs / 86_400) as i64 + 719_468;
    let rem = secs % 86_400;
    let era = days.div_euclid(146_097);
    let doe = days.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3_600,
        rem % 3_600 / 60,
        rem % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_becomes_readable_text() {
        let html = "<html><head><style>p{color:red}</style><script>alert(1)</script></head>\
                    <body><p>Hola&nbsp;Ana,</p><p>Adjunto la factura &amp; el albarán.<br>Total: 12&#44;50&#x20AC;</p>\
                    <!-- comentario --><div>Un saludo</div></body></html>";
        assert_eq!(
            html_to_text(html),
            "Hola Ana,\nAdjunto la factura & el albarán.\nTotal: 12,50€\nUn saludo"
        );
        assert_eq!(html_to_text("a &unknown; b & c"), "a &unknown; b & c");
    }

    #[test]
    fn addresses_are_extracted_from_headers() {
        assert_eq!(
            header_addresses(
                "\"Pérez, Ana\" <ana@cliente.test>, compras@proveedor.test, no-es-direccion"
            ),
            ["ana@cliente.test", "compras@proveedor.test"]
        );
        assert!(!is_address("ana@cliente.test\r\nBcc: x@y.test"));
        assert!(!is_address("sin-arroba.test"));
    }

    #[test]
    fn headers_cannot_be_injected_and_are_encoded() {
        assert_eq!(
            header_value("Asunto\r\nBcc: x@y.test"),
            "AsuntoBcc: x@y.test"
        );
        assert_eq!(encode_header("Re: factura"), "Re: factura");
        assert_eq!(encode_header("Re: año"), "=?UTF-8?B?UmU6IGHDsW8=?=");
    }

    #[test]
    fn text_parts_decode_utf8_or_latin1() {
        assert_eq!(decode_text("año".as_bytes()), "año");
        assert_eq!(decode_text(&[0x61, 0xF1, 0x6F]), "año");
    }

    #[test]
    fn millis_format_as_utc() {
        assert_eq!(format_utc_millis(0), "1970-01-01T00:00:00Z");
        assert_eq!(format_utc_millis(1_793_664_000_000), "2026-11-03T00:00:00Z");
    }
}
