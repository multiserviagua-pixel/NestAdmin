//! Cifras de la respuesta del asistente y su origen (D269, R33).
//!
//! La aplicación no puede saber si un texto es cierto, pero sí de dónde pudo salir cada cifra:
//! extrae de la respuesta los importes, fechas, horas, porcentajes, números de documento y
//! recuentos (`extract_figures`) y comprueba que cada uno aparece, una vez normalizados los
//! separadores y los formatos, en las fuentes del turno (`Sources`): resultados de herramientas,
//! documentos citados, la pregunta de la persona y respuestas anteriores con respaldo.
//!
//! Funciones puras, sin red ni IA. Cada cifra tiene una o varias claves canónicas (una forma
//! ambigua como `06/10/2026` o `1.234` da varias): está respaldada si alguna aparece en las
//! fuentes. Las claves son `n:` (número, para importes, porcentajes y recuentos), `d:` (fecha
//! completa `AAAA-MM-DD`), `md:` (día y mes sin año), `t:` (hora `HH:MM`) y `x:` (número de
//! documento sin separadores y en mayúsculas).

use std::collections::HashSet;

use serde_json::Value;

/// Cifras que se muestran como mucho en el aviso de la ventana.
pub const MAX_SHOWN: usize = 5;
/// Claves como mucho que se guardan de las cifras sin origen de un turno.
pub const MAX_KEYS: usize = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FigureKind {
    Amount,
    Date,
    Time,
    Percent,
    DocNumber,
    /// Recuento de dos o más cifras seguido de lo que se cuenta («12 facturas»).
    Count,
    /// Un número cualquiera; solo se extrae de las fuentes, nunca de la respuesta.
    Bare,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Figure {
    pub kind: FigureKind,
    /// Tal como está escrita en el texto.
    pub text: String,
    /// Claves canónicas; basta que una esté en las fuentes.
    pub keys: Vec<String>,
}

/// Lo que la comprobación halló en una respuesta.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Unsourced {
    /// Hasta `MAX_SHOWN` cifras sin origen, tal como están escritas.
    pub shown: Vec<String>,
    /// Cuántas cifras distintas hay sin origen.
    pub total: usize,
    /// Todas las claves de esas cifras (hasta `MAX_KEYS`): una respuesta posterior no puede
    /// apoyarse en ellas.
    pub keys: Vec<String>,
}

// ---- Normalización ----

fn canon(int: &str, frac: &str) -> String {
    let int = int.trim_start_matches('0');
    let int = if int.is_empty() { "0" } else { int };
    let frac = frac.trim_end_matches('0');
    if frac.is_empty() {
        format!("n:{int}")
    } else {
        format!("n:{int}.{frac}")
    }
}

/// Claves de un número escrito con separadores `.` y `,` (más espacios duros de miles). Una forma
/// ambigua como `1.234` (¿mil doscientos treinta y cuatro o uno con 234 milésimas?) da las dos.
fn number_keys(raw: &str) -> Vec<String> {
    let s: String = raw
        .chars()
        .filter(|c| !matches!(*c, '\u{a0}' | '\u{202f}' | ' '))
        .collect();
    let seps: Vec<(usize, char)> = s
        .char_indices()
        .filter(|(_, c)| *c == '.' || *c == ',')
        .collect();
    let Some(&(last, _)) = seps.last() else {
        return vec![canon(&s, "")];
    };
    let has_dot = seps.iter().any(|(_, c)| *c == '.');
    let has_comma = seps.iter().any(|(_, c)| *c == ',');
    let digits_only = |t: &str| -> String { t.chars().filter(char::is_ascii_digit).collect() };
    if has_dot && has_comma {
        // El último separador es el decimal; los demás, de miles.
        return vec![canon(&digits_only(&s[..last]), &s[last + 1..])];
    }
    if seps.len() > 1 {
        // Varios iguales: todos de miles (1.234.567).
        return vec![canon(&digits_only(&s), "")];
    }
    let left = &s[..last];
    let frac = &s[last + 1..];
    if frac.len() == 3 && left.len() <= 3 && left != "0" {
        // Miles (1.234 = 1234) o decimales (1.234 = 1,234).
        vec![canon(&format!("{left}{frac}"), ""), canon(left, frac)]
    } else {
        vec![canon(left, frac)]
    }
}

/// Forma del número escrito: ¿parece un importe sin símbolo?
fn looks_like_amount(raw: &str) -> bool {
    let s: String = raw
        .chars()
        .filter(|c| !matches!(*c, '\u{a0}' | '\u{202f}' | ' '))
        .collect();
    let seps: Vec<usize> = s
        .char_indices()
        .filter(|(_, c)| *c == '.' || *c == ',')
        .map(|(i, _)| i)
        .collect();
    let Some(&last) = seps.last() else {
        return false;
    };
    let frac = s.len() - last - 1;
    let has_dot = s.contains('.');
    let has_comma = s.contains(',');
    let int_digits = s[..last].chars().filter(char::is_ascii_digit).count();
    let mixed_or_single = (has_dot && has_comma) || seps.len() == 1;
    if frac == 2 && mixed_or_single {
        return true;
    }
    if frac == 1 && mixed_or_single && int_digits >= 4 {
        return true;
    }
    // Miles agrupados: 1.234, 12.500, 1.234.567 (todos los grupos tras el primero, de tres).
    let sep_char = s.as_bytes()[last] as char;
    if frac == 3
        && !(has_dot && has_comma)
        && s.chars().filter(|c| *c == sep_char).count() == seps.len()
    {
        let groups: Vec<&str> = s.split(sep_char).collect();
        return groups[0].len() <= 3
            && !groups[0].is_empty()
            && groups[0] != "0"
            && groups[1..].iter().all(|g| g.len() == 3);
    }
    false
}

// ---- Escaneo ----

const MONTHS: [(&str, u32); 14] = [
    ("enero", 1),
    ("febrero", 2),
    ("marzo", 3),
    ("abril", 4),
    ("mayo", 5),
    ("junio", 6),
    ("julio", 7),
    ("agosto", 8),
    ("septiembre", 9),
    ("setiembre", 9),
    ("octubre", 10),
    ("noviembre", 11),
    ("diciembre", 12),
    ("", 0),
];

const MONTH_ABBR: [(&str, u32); 14] = [
    ("ene", 1),
    ("feb", 2),
    ("mar", 3),
    ("abr", 4),
    ("may", 5),
    ("jun", 6),
    ("jul", 7),
    ("ago", 8),
    ("sep", 9),
    ("sept", 9),
    ("set", 9),
    ("oct", 10),
    ("nov", 11),
    ("dic", 12),
];

/// Lo que se cuenta con un recuento de dos o más cifras.
const COUNT_STEMS: [&str; 15] = [
    "factura",
    "correo",
    "documento",
    "mensaje",
    "tarea",
    "regla",
    "sugerencia",
    "cuenta",
    "proveedor",
    "archivo",
    "resultado",
    "borrador",
    "pendiente",
    "adjunto",
    "propuesta",
];

/// Unidades tras las que un decimal de dos cifras no es un importe.
const UNITS: [&str; 22] = [
    "h", "hs", "hrs", "hora", "horas", "min", "minutos", "seg", "kg", "g", "km", "m", "cm", "mm",
    "l", "litros", "dias", "semanas", "meses", "anos", "x", "mb",
];

fn strip_accent(c: char) -> char {
    match c {
        'á' | 'Á' => 'a',
        'é' | 'É' => 'e',
        'í' | 'Í' => 'i',
        'ó' | 'Ó' => 'o',
        'ú' | 'Ú' | 'ü' => 'u',
        'ñ' | 'Ñ' => 'n',
        other => other.to_ascii_lowercase(),
    }
}

fn days_in(month: u32) -> u32 {
    match month {
        2 => 29,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

fn valid_dm(day: u32, month: u32) -> bool {
    (1..=12).contains(&month) && day >= 1 && day <= days_in(month)
}

struct Scanner {
    c: Vec<char>,
    taken: Vec<bool>,
    found: Vec<(usize, Figure)>,
}

impl Scanner {
    fn new(text: &str) -> Self {
        let c: Vec<char> = text.chars().collect();
        Self {
            taken: vec![false; c.len()],
            c,
            found: Vec::new(),
        }
    }

    fn at(&self, i: usize) -> Option<char> {
        self.c.get(i).copied()
    }

    fn digit_at(&self, i: usize) -> bool {
        self.at(i).is_some_and(|c| c.is_ascii_digit())
    }

    fn free(&self, a: usize, b: usize) -> bool {
        (a..b).all(|i| !self.taken[i])
    }

    /// Cuántos dígitos seguidos hay desde `i`.
    fn run(&self, i: usize) -> usize {
        let mut j = i;
        while self.digit_at(j) {
            j += 1;
        }
        j - i
    }

    fn num(&self, a: usize, b: usize) -> u32 {
        self.c[a..b]
            .iter()
            .collect::<String>()
            .parse()
            .unwrap_or(u32::MAX)
    }

    /// ¿Está el carácter unido a lo anterior (letra, cifra o separador de un código)?
    fn joined_before(&self, i: usize) -> bool {
        if i == 0 {
            return false;
        }
        let p = self.c[i - 1];
        if p.is_alphanumeric() {
            return true;
        }
        if matches!(p, '-' | '/' | '_' | '.' | ',') && i >= 2 {
            return self.c[i - 2].is_alphanumeric();
        }
        false
    }

    fn push(&mut self, a: usize, b: usize, kind: FigureKind, keys: Vec<String>) {
        let text: String = self.c[a..b].iter().collect::<String>().trim().to_owned();
        for t in &mut self.taken[a..b] {
            *t = true;
        }
        self.found.push((a, Figure { kind, text, keys }));
    }

    /// Fin del número escrito con separadores que empieza en `i`.
    fn number_end(&self, i: usize) -> usize {
        let mut j = i;
        loop {
            while self.digit_at(j) {
                j += 1;
            }
            match self.at(j) {
                Some('.' | ',') if self.digit_at(j + 1) => j += 1,
                Some('\u{a0}' | '\u{202f}') if self.run(j + 1) == 3 && !self.digit_at(j + 4) => {
                    j += 1;
                }
                // «1 234,50»: un espacio corriente solo vale de miles si sigue una parte decimal.
                Some(' ')
                    if j > i
                        && j - i <= 3
                        && self.run(i) == j - i
                        && self.run(j + 1) == 3
                        && matches!(self.at(j + 4), Some(',' | '.'))
                        && (1..=2).contains(&self.run(j + 5))
                        && !self.digit_at(j + 5 + self.run(j + 5)) =>
                {
                    j += 1;
                }
                _ => return j,
            }
        }
    }

    // -- Fechas --

    fn scan_dates(&mut self) {
        let mut i = 0;
        while i < self.c.len() {
            if !self.digit_at(i) || self.taken[i] || self.joined_before(i) {
                i += 1;
                continue;
            }
            if let Some((end, keys)) = self.date_at(i) {
                if self.free(i, end) {
                    self.push(i, end, FigureKind::Date, keys);
                    i = end;
                    continue;
                }
            }
            i += self.run(i).max(1);
        }
    }

    fn date_at(&self, i: usize) -> Option<(usize, Vec<String>)> {
        let n1 = self.run(i);
        let a = i + n1;
        let sep = self.at(a)?;
        // AAAA-MM-DD, AAAA/MM/DD, AAAA.MM.DD.
        if n1 == 4 && matches!(sep, '-' | '/' | '.') {
            let n2 = self.run(a + 1);
            if (1..=2).contains(&n2) && self.at(a + 1 + n2) == Some(sep) {
                let n3 = self.run(a + 2 + n2);
                let end = a + 2 + n2 + n3;
                if (1..=2).contains(&n3) && !self.digit_at(end) && !self.more_date(end, sep) {
                    let (y, m, d) = (
                        self.num(i, a),
                        self.num(a + 1, a + 1 + n2),
                        self.num(a + 2 + n2, end),
                    );
                    if valid_dm(d, m) {
                        return Some((end, vec![format!("d:{y:04}-{m:02}-{d:02}")]));
                    }
                }
            }
            return None;
        }
        // D/M/AAAA, D-M-AA, D.M.AAAA (primero el día, como en España).
        if (1..=2).contains(&n1) && matches!(sep, '-' | '/' | '.') {
            let n2 = self.run(a + 1);
            if (1..=2).contains(&n2) && self.at(a + 1 + n2) == Some(sep) {
                let n3 = self.run(a + 2 + n2);
                let end = a + 2 + n2 + n3;
                let year_ok = n3 == 4 || (n3 == 2 && sep != '.');
                if year_ok && !self.more_date(end, sep) {
                    let (p, q) = (self.num(i, a), self.num(a + 1, a + 1 + n2));
                    let year = {
                        let y = self.num(a + 2 + n2, end);
                        if n3 == 2 {
                            2000 + y
                        } else {
                            y
                        }
                    };
                    let mut keys = Vec::new();
                    if valid_dm(p, q) {
                        keys.push(format!("d:{year:04}-{q:02}-{p:02}"));
                    }
                    if valid_dm(q, p) && p != q {
                        keys.push(format!("d:{year:04}-{p:02}-{q:02}"));
                    }
                    if !keys.is_empty() {
                        return Some((end, keys));
                    }
                }
            }
        }
        // 6 de octubre de 2026, 6 oct 2026, 1º de marzo.
        if (1..=2).contains(&n1) {
            return self.textual_date(i, a);
        }
        None
    }

    /// ¿Sigue otro grupo con el mismo separador? Entonces no es una fecha (1.2.3.4).
    fn more_date(&self, end: usize, sep: char) -> bool {
        self.at(end) == Some(sep) && self.digit_at(end + 1)
    }

    fn skip_spaces(&self, mut i: usize) -> usize {
        while self.at(i).is_some_and(|c| c == ' ' || c == '\u{a0}') {
            i += 1;
        }
        i
    }

    fn word_at(&self, i: usize) -> (usize, String) {
        let mut j = i;
        let mut w = String::new();
        while let Some(c) = self.at(j).filter(|c| c.is_alphabetic()) {
            w.push(strip_accent(c));
            j += 1;
        }
        (j, w)
    }

    fn textual_date(&self, i: usize, a: usize) -> Option<(usize, Vec<String>)> {
        let day = self.num(i, a);
        let mut p = a;
        if matches!(self.at(p), Some('º' | '°' | 'ª')) {
            p += 1;
        }
        let q = self.skip_spaces(p);
        if q == p {
            return None;
        }
        let mut p = q;
        let (w, word) = self.word_at(p);
        if word == "de" {
            p = self.skip_spaces(w);
        }
        let (end_word, month_word) = self.word_at(p);
        let full = MONTHS.iter().find(|(n, m)| *m != 0 && *n == month_word);
        let abbr = MONTH_ABBR.iter().find(|(n, _)| *n == month_word);
        let (month, is_full) = match (full, abbr) {
            (Some((_, m)), _) => (*m, true),
            (None, Some((_, m))) => (*m, false),
            _ => return None,
        };
        if !valid_dm(day, month) {
            return None;
        }
        let mut end = end_word;
        if !is_full && self.at(end) == Some('.') {
            end += 1;
        }
        // Año opcional: «de 2026», «del 2026» o «2026».
        let mut y = self.skip_spaces(end);
        let (yw, ywd) = self.word_at(y);
        if ywd == "de" || ywd == "del" {
            y = self.skip_spaces(yw);
        }
        if self.run(y) == 4
            && !self.digit_at(y + 4)
            && !matches!(self.at(y + 4), Some('.' | ',') if self.digit_at(y + 5))
        {
            let year = self.num(y, y + 4);
            return Some((y + 4, vec![format!("d:{year:04}-{month:02}-{day:02}")]));
        }
        if is_full {
            return Some((end, vec![format!("md:{month:02}-{day:02}")]));
        }
        None
    }

    // -- Horas --

    fn scan_times(&mut self) {
        let mut i = 0;
        while i < self.c.len() {
            let after_iso = i >= 2 && self.c[i - 1] == 'T' && self.c[i - 2].is_ascii_digit();
            if !self.digit_at(i)
                || self.taken[i]
                || (self.joined_before(i) && !after_iso)
                || (i > 0 && self.c[i - 1] == ':')
            {
                i += 1;
                continue;
            }
            if let Some((end, h, m)) = self.time_at(i) {
                if self.free(i, end) && h < 24 && m < 60 {
                    self.push(i, end, FigureKind::Time, vec![format!("t:{h:02}:{m:02}")]);
                    i = end;
                    continue;
                }
            }
            i += self.run(i).max(1);
        }
    }

    /// Fin de una indicación de hora («h», «hs», «hrs», «horas») que empieza en `i` tras espacios.
    fn hour_mark(&self, i: usize) -> Option<usize> {
        let p = self.skip_spaces(i);
        let (e, w) = self.word_at(p);
        if matches!(w.as_str(), "h" | "hs" | "hrs" | "horas")
            && !self.at(e).is_some_and(char::is_alphanumeric)
        {
            Some(e)
        } else {
            None
        }
    }

    fn time_at(&self, i: usize) -> Option<(usize, u32, u32)> {
        let n1 = self.run(i);
        if !(1..=2).contains(&n1) {
            return None;
        }
        let a = i + n1;
        let h = self.num(i, a);
        match self.at(a) {
            Some(':') if self.run(a + 1) == 2 => {
                let m = self.num(a + 1, a + 3);
                let mut end = a + 3;
                // Con segundos (10:30:15): la hora es la misma.
                if self.at(end) == Some(':') && self.run(end + 1) == 2 {
                    end += 3;
                }
                if self.digit_at(end)
                    || matches!(self.at(end), Some('.' | ',') if self.digit_at(end + 1))
                {
                    return None;
                }
                Some((end, h, m))
            }
            Some('.') if self.run(a + 1) == 2 && !self.digit_at(a + 3) => {
                let m = self.num(a + 1, a + 3);
                let end = self.hour_mark(a + 3)?;
                Some((end, h, m))
            }
            Some('h' | 'H') if self.run(a + 1) == 2 && !self.digit_at(a + 3) => {
                Some((a + 3, h, self.num(a + 1, a + 3)))
            }
            // «14 h»: solo dos cifras, para no confundirlo con una duración de una cifra.
            _ if n1 == 2 && (10..=23).contains(&h) => {
                let p = self.skip_spaces(a);
                let (e, w) = self.word_at(p);
                (w == "h" && !self.at(e).is_some_and(char::is_alphanumeric)).then_some((e, h, 0))
            }
            _ => None,
        }
    }

    // -- Números de documento --

    fn scan_doc_numbers(&mut self) {
        let n = self.c.len();
        let mut i = 0;
        while i < n {
            let c = self.c[i];
            let starts = c.is_ascii_alphanumeric()
                && (i == 0
                    || !(self.c[i - 1].is_alphanumeric() || matches!(self.c[i - 1], '-' | '/')))
                && !(i > 0 && matches!(self.c[i - 1], '@' | '.' | '_'));
            if !starts {
                i += 1;
                continue;
            }
            let mut j = i;
            while j < n {
                let cj = self.c[j];
                let joiner = matches!(cj, '-' | '/')
                    && j > i
                    && self.c.get(j + 1).is_some_and(char::is_ascii_alphanumeric);
                if cj.is_ascii_alphanumeric() || joiner {
                    j += 1;
                } else {
                    break;
                }
            }
            let token: String = self.c[i..j].iter().collect();
            let next = self.at(j);
            let blocked = matches!(next, Some('@'))
                || (next == Some('.') && self.at(j + 1).is_some_and(char::is_alphabetic))
                || !self.free(i, j);
            if !blocked {
                if let Some(key) = doc_number_key(&token) {
                    self.push(i, j, FigureKind::DocNumber, vec![key]);
                }
            }
            i = j.max(i + 1);
        }
    }

    // -- Porcentajes, importes y recuentos --

    fn scan_numbers(&mut self, bare: bool) {
        let n = self.c.len();
        let mut i = 0;
        while i < n {
            if !self.digit_at(i) || self.taken[i] || self.joined_before(i) {
                i += 1;
                continue;
            }
            let end = self.number_end(i);
            if !self.free(i, end) {
                i = end.max(i + 1);
                continue;
            }
            let raw: String = self.c[i..end].iter().collect();
            let keys = number_keys(&raw);
            let after = self.skip_one_space(end);
            // Porcentaje.
            if self.at(after) == Some('%') {
                self.push(i, after + 1, FigureKind::Percent, keys);
                i = after + 1;
                continue;
            }
            let (w_end, word) = self.word_at(after);
            if word == "por" {
                let q = self.skip_spaces(w_end);
                let (e2, w2) = self.word_at(q);
                if w2 == "ciento" || w2 == "cien" {
                    self.push(i, e2, FigureKind::Percent, keys);
                    i = e2;
                    continue;
                }
            }
            // Importe con símbolo o con forma de importe.
            let marker_after = self.currency_after(end);
            let marker_before = self.currency_before(i);
            if marker_after.is_some() || marker_before.is_some() {
                let start = marker_before.unwrap_or(i);
                let stop = marker_after.unwrap_or(end);
                if self.free(start, stop) {
                    self.push(start, stop, FigureKind::Amount, keys);
                    i = stop;
                    continue;
                }
            }
            let unit = UNITS.contains(&word.as_str());
            if looks_like_amount(&raw) && !unit {
                self.push(i, end, FigureKind::Amount, keys);
                i = end;
                continue;
            }
            // Recuento de dos o más cifras y lo que se cuenta.
            let plain_int = raw.chars().all(|c| c.is_ascii_digit());
            if plain_int
                && raw.len() >= 2
                && after > end - 1
                && COUNT_STEMS
                    .iter()
                    .any(|s| word.starts_with(s) && word.len() <= s.len() + 3)
            {
                self.push(i, w_end, FigureKind::Count, keys);
                i = w_end;
                continue;
            }
            if bare {
                self.push(i, end, FigureKind::Bare, keys);
            }
            i = end;
        }
    }

    fn skip_one_space(&self, i: usize) -> usize {
        if matches!(self.at(i), Some(' ' | '\u{a0}')) {
            i + 1
        } else {
            i
        }
    }

    /// Fin del símbolo o nombre de moneda que sigue al número, si lo hay.
    fn currency_after(&self, end: usize) -> Option<usize> {
        let p = self.skip_one_space(end);
        if let Some('€' | '$') = self.at(p) {
            return Some(p + 1);
        }
        let (e, w) = self.word_at(p);
        (matches!(
            w.as_str(),
            "eur" | "euro" | "euros" | "usd" | "dolar" | "dolares"
        ) && !self.at(e).is_some_and(char::is_alphanumeric))
        .then_some(e)
    }

    /// Inicio del símbolo o nombre de moneda que precede al número, si lo hay.
    fn currency_before(&self, i: usize) -> Option<usize> {
        let mut p = i;
        if p > 0 && matches!(self.c[p - 1], ' ' | '\u{a0}') {
            p -= 1;
        }
        if p == 0 {
            return None;
        }
        if matches!(self.c[p - 1], '€' | '$') {
            return Some(p - 1);
        }
        if p >= 3 {
            let w: String = self.c[p - 3..p].iter().map(|c| strip_accent(*c)).collect();
            let before = p >= 4 && self.c[p - 4].is_alphanumeric();
            if (w == "eur" || w == "usd") && !before {
                return Some(p - 3);
            }
        }
        None
    }
}

/// Clave de un número de documento si la forma del token lo es: letras (1 a 6) y cifras
/// («TN-5531», «F-2026-117», «TN5531»), o `AAAA-NNN` («2026-117»).
fn doc_number_key(token: &str) -> Option<String> {
    let chars: Vec<char> = token.chars().collect();
    let letters = chars.iter().take_while(|c| c.is_ascii_alphabetic()).count();
    let compact = || {
        token
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .collect::<String>()
            .to_uppercase()
    };
    let groups_ok = |rest: &[char]| {
        !rest.is_empty()
            && rest
                .iter()
                .all(|c| c.is_ascii_digit() || matches!(c, '-' | '/'))
            && rest.last().is_some_and(char::is_ascii_digit)
            && !rest
                .windows(2)
                .any(|w| !w[0].is_ascii_digit() && !w[1].is_ascii_digit())
    };
    if letters == 0 {
        // AAAA-NNN, con tres o más cifras tras el separador (dos serían un mes: «2026-10»).
        let parts: Vec<&str> = token.split(['-', '/']).collect();
        if parts.len() == 2
            && parts[0].len() == 4
            && parts[1].len() >= 3
            && parts.iter().all(|p| p.chars().all(|c| c.is_ascii_digit()))
        {
            return Some(format!("x:{}", compact()));
        }
        return None;
    }
    if !(1..=6).contains(&letters) {
        return None;
    }
    let rest = &chars[letters..];
    if !groups_ok(rest) {
        return None;
    }
    let digits = rest.iter().filter(|c| c.is_ascii_digit()).count();
    let separated = matches!(rest[0], '-' | '/');
    let upper = chars[..letters].iter().all(char::is_ascii_uppercase);
    if (separated && digits >= 2) || (!separated && upper && digits >= 3) {
        Some(format!("x:{}", compact()))
    } else {
        None
    }
}

fn scan(text: &str, bare: bool) -> Vec<Figure> {
    let mut s = Scanner::new(text);
    s.scan_dates();
    s.scan_times();
    s.scan_doc_numbers();
    s.scan_numbers(bare);
    let mut found = s.found;
    found.sort_by_key(|(start, _)| *start);
    found.into_iter().map(|(_, f)| f).collect()
}

/// Las cifras de una respuesta: importes, fechas, horas, porcentajes, números de documento y
/// recuentos de dos o más cifras. No cuentan los números sueltos de una cifra, los ordinales de una
/// lista («1.», «2)»), los años solos ni las referencias `doc:N`.
pub fn extract_figures(text: &str) -> Vec<Figure> {
    scan(text, false)
}

// ---- Fuentes ----

/// Dónde puede haber salido una cifra: todo lo que la aplicación sabe de este turno.
#[derive(Debug, Clone, Default)]
pub struct Sources {
    keys: HashSet<String>,
    /// Texto de las fuentes en mayúsculas y sin separadores, para los números de documento.
    compact: String,
}

impl Sources {
    pub fn new() -> Self {
        Self::default()
    }

    fn add_key(&mut self, key: String) {
        if let Some(date) = key.strip_prefix("d:") {
            if date.len() == 10 {
                self.keys.insert(format!("md:{}", &date[5..]));
            }
        }
        self.keys.insert(key);
    }

    /// Añade un texto: todos sus números, fechas, horas y números de documento.
    pub fn add_text(&mut self, text: &str) {
        for figure in scan(text, true) {
            for key in figure.keys {
                self.add_key(key);
            }
        }
        self.compact.extend(
            text.chars()
                .filter(char::is_ascii_alphanumeric)
                .map(|c| c.to_ascii_uppercase()),
        );
        self.compact.push('|');
    }

    /// Añade claves ya calculadas (las de una respuesta anterior con respaldo).
    pub fn add_keys<I: IntoIterator<Item = String>>(&mut self, keys: I) {
        for key in keys {
            self.add_key(key);
        }
    }

    /// Añade un texto con las claves que no se pueden usar: una respuesta anterior cuyas cifras sin
    /// origen no respaldan a otra posterior.
    pub fn add_backed_text(&mut self, text: &str, excluded: &HashSet<String>) {
        for figure in scan(text, false) {
            if figure.keys.iter().any(|k| excluded.contains(k)) {
                continue;
            }
            for key in figure.keys {
                self.add_key(key);
            }
        }
        self.compact.extend(
            text.chars()
                .filter(char::is_ascii_alphanumeric)
                .map(|c| c.to_ascii_uppercase()),
        );
        self.compact.push('|');
    }

    /// Añade un resultado de herramienta o una tarjeta: cada texto y número, los recuentos
    /// (`total: 7`, la longitud de una lista) y los importes en céntimos (`*_minor`).
    pub fn add_json(&mut self, value: &Value) {
        self.add_value(value, "");
    }

    fn add_value(&mut self, value: &Value, field: &str) {
        match value {
            Value::String(s) => self.add_text(s),
            Value::Number(n) => {
                if let Some(u) = n.as_u64() {
                    self.add_key(canon(&u.to_string(), ""));
                    if field.ends_with("_minor") {
                        self.add_key(canon(&(u / 100).to_string(), &format!("{:02}", u % 100)));
                    }
                } else if let Some(i) = n.as_i64() {
                    let abs = i.unsigned_abs();
                    self.add_key(canon(&abs.to_string(), ""));
                    if field.ends_with("_minor") {
                        self.add_key(canon(
                            &(abs / 100).to_string(),
                            &format!("{:02}", abs % 100),
                        ));
                    }
                } else {
                    for key in number_keys(&n.to_string().replace('-', "")) {
                        self.add_key(key);
                    }
                }
            }
            Value::Array(items) => {
                self.add_key(canon(&items.len().to_string(), ""));
                for item in items {
                    self.add_value(item, field);
                }
            }
            Value::Object(map) => {
                for (k, v) in map {
                    self.add_value(v, k);
                }
            }
            Value::Bool(_) | Value::Null => {}
        }
    }

    /// ¿Aparece la cifra, en alguna de sus formas, en las fuentes?
    pub fn backs(&self, figure: &Figure) -> bool {
        if figure.keys.iter().any(|k| self.keys.contains(k)) {
            return true;
        }
        figure.kind == FigureKind::DocNumber
            && figure.keys.iter().any(|k| {
                let compact = &k[2..];
                compact.len() >= 4 && self.compact.contains(compact)
            })
    }
}

/// Cifras de la respuesta que no salen de las fuentes: hasta `MAX_SHOWN`, el total (sin repetir la
/// misma cifra) y todas sus claves.
pub fn unsourced_figures(answer: &str, sources: &Sources) -> Unsourced {
    let mut out = Unsourced::default();
    let mut seen: HashSet<String> = HashSet::new();
    for figure in extract_figures(answer) {
        if sources.backs(&figure) || !seen.insert(figure.keys[0].clone()) {
            continue;
        }
        out.total += 1;
        if out.shown.len() < MAX_SHOWN {
            out.shown.push(figure.text.clone());
        }
        for key in figure.keys {
            if out.keys.len() < MAX_KEYS && !out.keys.contains(&key) {
                out.keys.push(key);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn keys(text: &str) -> Vec<Vec<String>> {
        extract_figures(text).into_iter().map(|f| f.keys).collect()
    }

    fn kinds(text: &str) -> Vec<FigureKind> {
        extract_figures(text).into_iter().map(|f| f.kind).collect()
    }

    fn texts(text: &str) -> Vec<String> {
        extract_figures(text).into_iter().map(|f| f.text).collect()
    }

    fn first_key(text: &str) -> String {
        extract_figures(text)[0].keys[0].clone()
    }

    fn sources(texts: &[&str]) -> Sources {
        let mut s = Sources::new();
        for t in texts {
            s.add_text(t);
        }
        s
    }

    #[test]
    fn the_same_amount_in_every_format_has_the_same_key() {
        for text in [
            "1.234,50 €",
            "1234,50 EUR",
            "1234.50 euros",
            "1234,5 euros",
            "€1.234,50",
            "EUR 1234.5",
            "1 234,50 €",
            "1,234.50 USD",
            "1234,50",
            "1.234,50",
        ] {
            assert_eq!(first_key(text), "n:1234.5", "{text}");
            assert_eq!(kinds(text), vec![FigureKind::Amount], "{text}");
        }
    }

    #[test]
    fn an_amount_without_symbol_needs_the_shape_of_an_amount() {
        assert_eq!(kinds("45,00"), vec![FigureKind::Amount]);
        assert_eq!(kinds("12.500"), vec![FigureKind::Amount]);
        assert!(keys("12.500")[0].contains(&"n:12500".to_owned()));
        assert!(keys("12.500")[0].contains(&"n:12.5".to_owned()));
        assert_eq!(kinds("1.234.567"), vec![FigureKind::Amount]);
        assert_eq!(first_key("1.234.567"), "n:1234567");
        // Un decimal corto, un entero y una duración no son importes.
        assert!(extract_figures("3,5").is_empty());
        assert!(extract_figures("hay 500 cosas").is_empty());
        assert!(extract_figures("tardó 2,75 horas").is_empty());
        assert!(extract_figures("pesa 12,50 kg").is_empty());
        // Con símbolo, cualquier tamaño.
        assert_eq!(first_key("500 EUR"), "n:500");
        assert_eq!(first_key("5 euros"), "n:5");
        assert_eq!(first_key("3,5 €"), "n:3.5");
    }

    #[test]
    fn a_negative_amount_is_read_by_its_absolute_value() {
        assert_eq!(first_key("-45,00 €"), "n:45");
    }

    #[test]
    fn the_symbol_belongs_to_the_displayed_text() {
        assert_eq!(texts("Total: 1.234,50 € en total"), vec!["1.234,50 €"]);
        assert_eq!(texts("Total: € 20,00."), vec!["€ 20,00"]);
        assert_eq!(texts("son 20 euros."), vec!["20 euros"]);
    }

    #[test]
    fn dates_in_every_form_share_the_key() {
        for text in [
            "2026-10-06",
            "06/10/2026",
            "6/10/2026",
            "6-10-2026",
            "06.10.2026",
            "6/10/26",
            "6 de octubre de 2026",
            "6 de Octubre del 2026",
            "6 octubre 2026",
            "6 oct 2026",
            "6 oct. 2026",
            "2026/10/06",
            "2026-11-12T10:30Z y 2026-10-06",
        ] {
            let found = extract_figures(text);
            assert!(
                found
                    .iter()
                    .any(|f| f.kind == FigureKind::Date && f.keys[0] == "d:2026-10-06"),
                "{text}: {found:?}"
            );
        }
    }

    #[test]
    fn an_ambiguous_numeric_date_has_both_readings() {
        assert_eq!(
            keys("06/10/2026"),
            vec![vec!["d:2026-10-06".to_owned(), "d:2026-06-10".to_owned()]]
        );
        // Con el mes mayor de 12 solo hay una lectura.
        assert_eq!(keys("25/10/2026"), vec![vec!["d:2026-10-25".to_owned()]]);
        assert_eq!(keys("10/25/2026"), vec![vec!["d:2026-10-25".to_owned()]]);
    }

    #[test]
    fn a_date_without_year_matches_only_by_day_and_month() {
        assert_eq!(keys("el 6 de octubre"), vec![vec!["md:10-06".to_owned()]]);
        assert_eq!(keys("el 1º de marzo"), vec![vec!["md:03-01".to_owned()]]);
        let s = sources(&["fecha 2026-10-06"]);
        assert!(s.backs(&extract_figures("el 6 de octubre")[0]));
        assert!(!s.backs(&extract_figures("el 7 de octubre")[0]));
    }

    #[test]
    fn what_is_not_a_date_is_left_alone() {
        for text in [
            "31/02/2026",
            "32/01/2026",
            "1.2.3.4",
            "30-13-2026",
            "octubre de 2026",
            "10 mar",
        ] {
            assert!(
                extract_figures(text)
                    .iter()
                    .all(|f| f.kind != FigureKind::Date),
                "{text}"
            );
        }
        // Un año suelto no es una cifra.
        assert!(extract_figures("en 2026 y en 1999").is_empty());
    }

    #[test]
    fn times_in_every_form() {
        for (text, key) in [
            ("a las 9:00", "t:09:00"),
            ("de 9:00 a 14:00", "t:09:00"),
            ("14.30 h", "t:14:30"),
            ("14.30 horas", "t:14:30"),
            ("a las 14h30", "t:14:30"),
            ("a las 14 h", "t:14:00"),
            ("10:30:15", "t:10:30"),
            ("2026-11-12T10:30Z", "t:10:30"),
            ("23:59", "t:23:59"),
        ] {
            let found = extract_figures(text);
            assert!(
                found
                    .iter()
                    .any(|f| f.kind == FigureKind::Time && f.keys[0] == key),
                "{text}: {found:?}"
            );
        }
        let both = extract_figures("de 9:00 a 14:00");
        assert_eq!(both.len(), 2);
    }

    #[test]
    fn what_is_not_a_time_is_left_alone() {
        for text in [
            "25:00",
            "12:75",
            "9 h",
            "tardó 3 horas",
            "14.30",
            "doc:12",
            "1:1",
        ] {
            assert!(
                extract_figures(text)
                    .iter()
                    .all(|f| f.kind != FigureKind::Time),
                "{text}"
            );
        }
        assert!(extract_figures("doc:12 y doc:1234").is_empty());
    }

    #[test]
    fn percentages() {
        assert_eq!(first_key("un 21 %"), "n:21");
        assert_eq!(kinds("un 21%"), vec![FigureKind::Percent]);
        assert_eq!(first_key("el 10,5% más"), "n:10.5");
        assert_eq!(kinds("un 15 por ciento"), vec![FigureKind::Percent]);
        assert_eq!(kinds("un 5%"), vec![FigureKind::Percent]);
    }

    #[test]
    fn document_numbers() {
        for (text, key) in [
            ("la factura F-2026-117", "x:F2026117"),
            ("TN-5531", "x:TN5531"),
            ("TN5531", "x:TN5531"),
            ("fa-123", "x:FA123"),
            ("2026-117", "x:2026117"),
            ("FAC/2026/0042", "x:FAC20260042"),
            ("(INV-0007)", "x:INV0007"),
        ] {
            let found = extract_figures(text);
            assert!(
                found
                    .iter()
                    .any(|f| f.kind == FigureKind::DocNumber && f.keys[0] == key),
                "{text}: {found:?}"
            );
        }
    }

    #[test]
    fn what_is_not_a_document_number_is_left_alone() {
        for text in [
            "A4",
            "IVA21",
            "juan2024@ejemplo.org",
            "www.ABC123.com",
            "2026-10",
            "tn5531",
            "doc:123",
            "TN-5",
            "H2O",
            "mp3",
        ] {
            assert!(
                extract_figures(text)
                    .iter()
                    .all(|f| f.kind != FigureKind::DocNumber),
                "{text}: {:?}",
                extract_figures(text)
            );
        }
    }

    #[test]
    fn single_digits_and_list_ordinals_do_not_count() {
        assert!(extract_figures("1. Primero\n2) Segundo\n3 facturas y 7 correos").is_empty());
        assert!(extract_figures("10. Décimo punto").is_empty());
        // Dos cifras seguidas de lo que se cuenta, sí.
        assert_eq!(kinds("hay 12 facturas pendientes"), vec![FigureKind::Count]);
        assert_eq!(first_key("hay 12 facturas pendientes"), "n:12");
        assert!(extract_figures("12 personas").is_empty());
    }

    #[test]
    fn a_figure_inside_a_date_or_a_time_is_not_counted_twice() {
        let found = extract_figures("el 06/10/2026 a las 14:30 pagó 20,00 €");
        assert_eq!(
            found.iter().map(|f| f.kind).collect::<Vec<_>>(),
            vec![FigureKind::Date, FigureKind::Time, FigureKind::Amount]
        );
    }

    #[test]
    fn the_amount_is_backed_by_any_format_in_the_sources() {
        let s = Sources::new();
        let mut s = s;
        s.add_json(&json!({"total": "1234.50", "total_minor": 123450, "n": 7}));
        for text in ["1.234,50 €", "1234,5 euros", "1234.50 EUR"] {
            assert!(s.backs(&extract_figures(text)[0]), "{text}");
        }
        assert!(!s.backs(&extract_figures("1.200 €")[0]));
        // Los céntimos de `*_minor` también valen.
        let mut m = Sources::new();
        m.add_json(&json!({"by_currency": [{"total_minor": 987654}]}));
        assert!(m.backs(&extract_figures("9.876,54 €")[0]));
    }

    #[test]
    fn a_count_given_by_the_application_backs_the_count_in_the_answer() {
        let mut s = Sources::new();
        s.add_json(&json!({"total": 12, "results": [1, 2, 3]}));
        assert!(s.backs(&extract_figures("hay 12 facturas")[0]));
        assert!(!s.backs(&extract_figures("hay 15 facturas")[0]));
        let mut list = Sources::new();
        list.add_json(&json!({"results": (0..14).map(|i| json!({"ref": i})).collect::<Vec<_>>()}));
        assert!(list.backs(&extract_figures("hay 14 correos")[0]));
    }

    #[test]
    fn a_date_in_the_sources_backs_every_written_form() {
        let mut s = Sources::new();
        s.add_json(&json!({"date": "2026-10-06", "updated": "2026-11-12T10:30Z"}));
        for text in [
            "6 de octubre de 2026",
            "06/10/2026",
            "6 de octubre",
            "a las 10:30",
        ] {
            assert!(s.backs(&extract_figures(text)[0]), "{text}");
        }
        assert!(!s.backs(&extract_figures("7 de octubre de 2026")[0]));
        assert!(!s.backs(&extract_figures("a las 10:31")[0]));
    }

    #[test]
    fn a_document_number_is_backed_even_inside_a_file_name() {
        let s = sources(&["archivo factura_TN-5531.pdf del proveedor"]);
        assert!(s.backs(&extract_figures("la factura TN-5531")[0]));
        assert!(s.backs(&extract_figures("la factura TN5531")[0]));
        assert!(!s.backs(&extract_figures("la factura TN-5532")[0]));
        let s = sources(&["Referencia F-2026-117 y otra"]);
        assert!(s.backs(&extract_figures("F2026117")[0]));
    }

    #[test]
    fn a_number_that_is_part_of_a_supplier_name_in_the_sources_is_backed() {
        let s = sources(&["Talleres TN-5531 S.L.", "Grupo A24B 24,00 €"]);
        assert!(s.backs(&extract_figures("de Talleres TN-5531 S.L.")[0]));
    }

    #[test]
    fn unsourced_names_each_invented_datum_once_and_caps_the_list() {
        let s = sources(&["Factura F-2026-014 de 1.234,56 € del 2026-03-05"]);
        let answer = "La F-2026-014 de 1.234,56 € es del 5 de marzo de 2026, vence el 15/04/2026 \
            y cuesta 99,00 € (21 %) a las 9:00, otra de 12,00 €, otra de 13,00 €, otra de 14,00 € y la F-2026-999; \
            repito 99,00 €.";
        let u = unsourced_figures(answer, &s);
        assert_eq!(u.total, 8);
        assert_eq!(u.shown.len(), MAX_SHOWN);
        assert_eq!(u.shown[0], "15/04/2026");
        assert!(u.keys.contains(&"n:99".to_owned()));
        assert!(!u.keys.iter().any(|k| k == "n:1234.56"));
    }

    #[test]
    fn nothing_is_flagged_when_everything_comes_from_the_sources() {
        let mut s = Sources::new();
        s.add_json(&json!({"results": [{"number": "F-2026-014", "total": "1234.56", "date": "2026-03-05"}], "total": 1}));
        let u = unsourced_figures(
            "Encontré la factura F-2026-014 por 1.234,56 EUR, fechada el 05/03/2026.",
            &s,
        );
        assert_eq!(u, Unsourced::default());
    }

    #[test]
    fn a_rounded_amount_is_unsourced() {
        let mut s = Sources::new();
        s.add_json(&json!({"total": "1234.56"}));
        let u = unsourced_figures("Son unos 1.200 € más o menos.", &s);
        assert_eq!(u.shown, vec!["1.200 €"]);
    }

    #[test]
    fn an_earlier_answer_backs_only_what_it_was_itself_sourced_for() {
        let mut s = Sources::new();
        let mut excluded = HashSet::new();
        excluded.insert("n:77".to_owned());
        s.add_backed_text("Suma 500,00 € y además 77,00 €", &excluded);
        assert!(s.backs(&extract_figures("500 EUR")[0]));
        assert!(!s.backs(&extract_figures("77 EUR")[0]));
    }

    #[test]
    fn the_persons_own_numbers_back_the_answer() {
        let s = sources(&["¿Hay alguna de 450 euros del 3/11/2026 a las 18:15?"]);
        let u = unsourced_figures("Sí: 450,00 € del 3 de noviembre de 2026, a las 18:15.", &s);
        assert_eq!(u.total, 0, "{u:?}");
    }

    #[test]
    fn a_realistic_answer_built_only_from_the_results_has_nothing_unsourced() {
        let mut s = Sources::new();
        s.add_json(&json!({
            "total": 3,
            "results": [
                {"ref": "doc:1", "number": "F-2026-101", "party": "Suministros Ficticios SL", "date": "2026-10-05", "total": "484.00"},
                {"ref": "doc:2", "number": "F-2026-117", "party": "Suministros Ficticios SL", "date": "2026-10-19", "total": "181.50"},
                {"ref": "doc:3", "number": "F-2026-124", "party": "Suministros Ficticios SL", "date": "2026-10-26", "total": "114.95"}
            ],
            "by_currency": [{"total": "780.45", "total_minor": 78045, "documents": 3}]
        }));
        let answer = "Encontré 3 facturas de Suministros Ficticios SL entre el 5 y el 26 de octubre de 2026:\n\
            1. F-2026-101 (484,00 EUR), del 5 de octubre.\n\
            2. F-2026-117 (181,50 EUR), del 19/10/2026.\n\
            3. F-2026-124 (114,95 EUR), del 26 de octubre de 2026.\n\
            En total son 780,45 EUR, unos 15 días de trabajo [doc:1].";
        let u = unsourced_figures(answer, &s);
        assert_eq!(u, Unsourced::default(), "{u:?}");
    }

    #[test]
    fn years_months_and_plain_small_numbers_are_never_flagged() {
        let s = Sources::new();
        for text in [
            "en octubre de 2026",
            "durante 2026 y 2027",
            "hay 3 correos y 7 facturas",
            "hace 15 días",
            "en 12 meses",
            "la página 4 de 10",
            "versión 2.1",
            "ver [doc:1], [doc:23] y [doc:456]",
        ] {
            assert_eq!(unsourced_figures(text, &s).total, 0, "{text}");
        }
    }

    #[test]
    fn markdown_and_references_do_not_confuse_the_scanner() {
        let s = Sources::new();
        let u = unsourced_figures("1. Vea [doc:12] y doc:7.\n2) Luego [enlace retirado].", &s);
        assert_eq!(u.total, 0);
    }
}
