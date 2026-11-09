//! Señales de correo automático (D157): funciones puras sobre la dirección del remitente y
//! las cabeceras. Devuelven códigos estables y nunca copian el valor de una cabecera.
//!
//! `None` en las cabeceras significa «no comprobadas» (p. ej. Graph sin
//! `internetMessageHeaders`): solo se evalúa la dirección, que siempre se puede recalcular
//! con [`address_signals`]. Una lista vacía de cabeceras sí es «comprobadas, sin señales».

/// Señal de correo automático. El código de texto ([`Signal::code`]) es el que se guarda.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Signal {
    /// Remitente `no-reply` y variantes (fuerte).
    NoreplyAddress,
    /// `Auto-Submitted` distinto de `no` (fuerte).
    AutoSubmitted,
    /// `Precedence: junk` (fuerte).
    PrecedenceJunk,
    /// Cabecera `List-Id` (débil).
    ListId,
    /// Cabecera `List-Unsubscribe` (débil).
    ListUnsubscribe,
    /// `Precedence: bulk` (débil).
    PrecedenceBulk,
    /// `Precedence: list` (débil).
    PrecedenceList,
    /// Remitente `mailer-daemon` (rebote).
    BounceAddress,
    /// `Return-Path: <>` (rebote).
    NullReturnPath,
}

impl Signal {
    pub const ALL: [Signal; 9] = [
        Signal::NoreplyAddress,
        Signal::AutoSubmitted,
        Signal::PrecedenceJunk,
        Signal::ListId,
        Signal::ListUnsubscribe,
        Signal::PrecedenceBulk,
        Signal::PrecedenceList,
        Signal::BounceAddress,
        Signal::NullReturnPath,
    ];

    /// Código estable que se persiste.
    pub fn code(self) -> &'static str {
        match self {
            Signal::NoreplyAddress => "noreply_address",
            Signal::AutoSubmitted => "auto_submitted",
            Signal::PrecedenceJunk => "precedence_junk",
            Signal::ListId => "list_id",
            Signal::ListUnsubscribe => "list_unsubscribe",
            Signal::PrecedenceBulk => "precedence_bulk",
            Signal::PrecedenceList => "precedence_list",
            Signal::BounceAddress => "bounce_address",
            Signal::NullReturnPath => "null_return_path",
        }
    }

    /// Inversa de [`Signal::code`]; `None` para un código desconocido.
    pub fn from_code(code: &str) -> Option<Signal> {
        Signal::ALL.into_iter().find(|s| s.code() == code)
    }

    /// Una señal fuerte oculta el correo; un cliente real no debería llevarla.
    pub fn is_strong(self) -> bool {
        matches!(
            self,
            Signal::NoreplyAddress | Signal::AutoSubmitted | Signal::PrecedenceJunk
        )
    }

    /// Una señal débil sola no oculta: solo suma con otras.
    pub fn is_weak(self) -> bool {
        matches!(
            self,
            Signal::ListId
                | Signal::ListUnsubscribe
                | Signal::PrecedenceBulk
                | Signal::PrecedenceList
        )
    }

    /// Rebote: el correo es un aviso de entrega fallida, no un mensaje de una persona.
    pub fn is_bounce(self) -> bool {
        matches!(self, Signal::BounceAddress | Signal::NullReturnPath)
    }
}

/// Ruta local de un correo nuevo (D157, D161): lo decide el código, sin IA.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Route {
    /// Aviso de entrega fallida: revisión sin IA ni borrador.
    Rebote,
    /// Señal fuerte: se cierra como informativo sin IA.
    Informativo,
    /// Sin señal fuerte: sigue el flujo normal con la IA.
    Ia,
}

impl Route {
    /// Código estable que se guarda en `triage.route` (y que usa el corpus de triaje).
    pub fn as_str(self) -> &'static str {
        match self {
            Route::Rebote => "rebote",
            Route::Informativo => "informativo",
            Route::Ia => "ia",
        }
    }
}

/// Preferencia del usuario sobre un remitente (D161). Un clic explícito, nunca aprendida.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PreferenceRoute {
    /// «Siempre a Pendientes»: una señal fuerte cuenta como débil (va a la IA y a revisión).
    Review,
    /// «Siempre a Informativos»: se cierra sin IA.
    Informative,
}

impl PreferenceRoute {
    pub fn as_str(self) -> &'static str {
        match self {
            PreferenceRoute::Review => "review",
            PreferenceRoute::Informative => "informative",
        }
    }

    pub fn from_code(code: &str) -> Option<PreferenceRoute> {
        match code {
            "review" => Some(PreferenceRoute::Review),
            "informative" => Some(PreferenceRoute::Informative),
            _ => None,
        }
    }
}

/// Quién decidió la ruta de un correo nuevo; se guarda en `triage.source`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TriageSource {
    /// Las señales del propio correo.
    Local,
    /// Una preferencia del usuario sobre el remitente.
    Preference,
    /// El último mensaje del hilo es propio (D162).
    Thread,
}

impl TriageSource {
    pub fn as_str(self) -> &'static str {
        match self {
            TriageSource::Local => "local",
            TriageSource::Preference => "preference",
            TriageSource::Thread => "thread",
        }
    }
}

/// Decisión local sobre un correo nuevo, con quién la tomó.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TriageDecision {
    pub route: Route,
    pub source: TriageSource,
    /// El último mensaje del hilo ya era propio (D162).
    pub already_replied: bool,
}

/// Precedencia de D161 para un correo nuevo, sin IA ni red. La primera lectura (D133) se resuelve
/// antes, porque sus mensajes ya tienen tarea: nunca llegan aquí.
///
/// 1. Rebote: revisión, sin que ninguna preferencia ni el hilo lo cambien.
/// 2. Hilo: si el último mensaje es propio (`Some(true)`), informativo `already_replied`. `None`
///    (desconocido o con error) no tiene efecto.
/// 3. Preferencia ya resuelta (dirección y, si no hay, dominio): `Informative` cierra; `Review`
///    trata una señal fuerte como débil y sigue a la IA.
/// 4. Señal fuerte: informativo. 5. Si no, IA.
pub fn decide(
    signals: &[Signal],
    thread_own: Option<bool>,
    preference: Option<PreferenceRoute>,
) -> TriageDecision {
    let (route, source, already_replied) = if signals.iter().any(|s| s.is_bounce()) {
        (Route::Rebote, TriageSource::Local, false)
    } else if thread_own == Some(true) {
        (Route::Informativo, TriageSource::Thread, true)
    } else {
        match preference {
            Some(PreferenceRoute::Informative) => {
                (Route::Informativo, TriageSource::Preference, false)
            }
            Some(PreferenceRoute::Review) => (Route::Ia, TriageSource::Preference, false),
            None => (route(signals), TriageSource::Local, false),
        }
    };
    TriageDecision {
        route,
        source,
        already_replied,
    }
}

/// Dirección completa y dominio, en minúsculas, de `Nombre <a@b>` o `a@b`. Se conserva el
/// sufijo `+etiqueta`: una preferencia es de una dirección exacta. `None` si no hay dirección.
pub fn sender_parts(from: &str) -> Option<(String, String)> {
    let text = match (from.rfind('<'), from.rfind('>')) {
        (Some(open), Some(close)) if open < close => &from[open + 1..close],
        _ => from,
    };
    let text = text.trim().trim_matches('"').trim();
    let (local, domain) = text.rsplit_once('@')?;
    let (local, domain) = (local.trim().to_lowercase(), domain.trim().to_lowercase());
    let valid_domain = !domain.is_empty()
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && domain
            .chars()
            .all(|c| c.is_alphanumeric() || c == '.' || c == '-');
    let valid_local = !local.is_empty() && !local.chars().any(|c| c.is_whitespace() || c == '@');
    (valid_domain && valid_local).then(|| (format!("{local}@{domain}"), domain))
}

/// Rebote antes que señal fuerte; una señal débil sola no oculta.
pub fn route(signals: &[Signal]) -> Route {
    if signals.iter().any(|s| s.is_bounce()) {
        Route::Rebote
    } else if signals.iter().any(|s| s.is_strong()) {
        Route::Informativo
    } else {
        Route::Ia
    }
}

/// Ruta de un mensaje a partir de su remitente y de los códigos guardados (`Message.signals`).
/// Con `None` (sin comprobar) solo cuenta la dirección, que siempre se puede recalcular; un
/// código desconocido se ignora. Devuelve también las señales consideradas, ya ordenadas.
pub fn route_of_message(from: Option<&str>, codes: Option<&[String]>) -> (Route, Vec<Signal>) {
    let mut found = address_signals(from);
    if let Some(codes) = codes {
        found.extend(codes.iter().filter_map(|c| Signal::from_code(c)));
    }
    found.sort();
    found.dedup();
    (route(&found), found)
}

/// Códigos de texto de una lista de señales.
pub fn codes(signals: &[Signal]) -> Vec<String> {
    signals.iter().map(|s| s.code().to_owned()).collect()
}

/// Calcula las señales. `headers = None`: no comprobadas, solo cuenta la dirección. Resultado
/// ordenado y sin repetidos.
pub fn detect(from: Option<&str>, headers: Option<&[(&str, &str)]>) -> Vec<Signal> {
    let mut out = address_signals(from);
    if let Some(headers) = headers {
        out.extend(header_signals(headers));
    }
    out.sort();
    out.dedup();
    out
}

/// Señales de la dirección del remitente (`Nombre <a@b>` o `a@b`).
pub fn address_signals(from: Option<&str>) -> Vec<Signal> {
    let Some(local) = from.and_then(local_part) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    if is_noreply(&local) {
        out.push(Signal::NoreplyAddress);
    }
    if local == "mailer-daemon" {
        out.push(Signal::BounceAddress);
    }
    out
}

/// Señales de las cabeceras; los nombres no distinguen mayúsculas.
pub fn header_signals(headers: &[(&str, &str)]) -> Vec<Signal> {
    let mut out = Vec::new();
    for (name, value) in headers {
        let name = name.trim();
        if name.eq_ignore_ascii_case("auto-submitted") {
            let v = clean_value(value);
            if !v.is_empty() && v != "no" {
                out.push(Signal::AutoSubmitted);
            }
        } else if name.eq_ignore_ascii_case("precedence") {
            match clean_value(value).as_str() {
                "junk" => out.push(Signal::PrecedenceJunk),
                "bulk" => out.push(Signal::PrecedenceBulk),
                "list" => out.push(Signal::PrecedenceList),
                _ => {}
            }
        } else if name.eq_ignore_ascii_case("list-id") {
            if !value.trim().is_empty() {
                out.push(Signal::ListId);
            }
        } else if name.eq_ignore_ascii_case("list-unsubscribe") {
            if !value.trim().is_empty() {
                out.push(Signal::ListUnsubscribe);
            }
        } else if name.eq_ignore_ascii_case("return-path") {
            let compact: String = value.chars().filter(|c| !c.is_whitespace()).collect();
            if compact == "<>" {
                out.push(Signal::NullReturnPath);
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Valor de cabecera sin comentarios `(…)` ni parámetros tras `;`, recortado y en minúsculas.
fn clean_value(value: &str) -> String {
    let mut out = String::new();
    let mut depth = 0u32;
    for c in value.chars() {
        match c {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            ';' if depth == 0 => break,
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out.trim().to_lowercase()
}

/// Parte local en minúsculas, sin el sufijo `+etiqueta`; `None` si no hay dirección.
fn local_part(from: &str) -> Option<String> {
    let text = match (from.rfind('<'), from.rfind('>')) {
        (Some(open), Some(close)) if open < close => &from[open + 1..close],
        _ => from,
    };
    let text = text.trim().trim_matches('"').trim();
    let (local, domain) = text.rsplit_once('@')?;
    if local.is_empty() || domain.is_empty() {
        return None;
    }
    let local = local.trim().to_lowercase();
    let local = local.split_once('+').map_or(local.as_str(), |(l, _)| l);
    (!local.is_empty()).then(|| local.to_owned())
}

const NOREPLY_FORMS: [&str; 11] = [
    "noreply",
    "no-reply",
    "no_reply",
    "no.reply",
    "noresponder",
    "no-responder",
    "no_responder",
    "donotreply",
    "do-not-reply",
    "do_not_reply",
    "do.not.reply",
];

/// Criterio: la forma `no-reply` debe ser la parte local entera o ir separada del resto por
/// `-`, `_` o `.` (prefijo, sufijo o parte intermedia). Así `noreply-accounts`,
/// `account-security-noreply` y `no-reply-ventas` cuentan, pero una subcadena dentro de una
/// palabra (`noreplyinc`, `honoreply`) no: un cliente real no debe quedar oculto por azar.
fn is_noreply(local: &str) -> bool {
    let is_sep = |c: char| matches!(c, '-' | '_' | '.');
    NOREPLY_FORMS.iter().any(|form| {
        local.match_indices(form).any(|(start, _)| {
            let before = local[..start].chars().next_back();
            let after = local[start + form.len()..].chars().next();
            before.is_none_or(is_sep) && after.is_none_or(is_sep)
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(from: &str) -> Vec<Signal> {
        address_signals(Some(from))
    }

    #[test]
    fn noreply_variants_match() {
        for from in [
            "noreply@a.test",
            "NoReply@A.test",
            "No-Reply@a.test",
            "no_reply@a.test",
            "no.reply@a.test",
            "noresponder@a.test",
            "no-responder@a.test",
            "no_responder@a.test",
            "donotreply@a.test",
            "do-not-reply@a.test",
            "do_not_reply@a.test",
            "noreply+abc@a.test",
            "Servicio <no-reply@a.test>",
            "\"Servicio, S.A.\" <NOREPLY@a.test>",
            "noreply-accounts@a.test",
            "account-security-noreply@a.test",
            "no-reply-ventas@a.test",
            "ventas.noreply.es@a.test",
        ] {
            assert_eq!(addr(from), vec![Signal::NoreplyAddress], "{from}");
        }
    }

    #[test]
    fn noreply_does_not_match_substrings_or_absent() {
        for from in [
            "arnold@a.test",
            "noreplyinc@empresa.test",
            "honoreply@a.test",
            "ana@noreply.test",
            "Ana <ana@a.test>",
            "sin-arroba",
            "",
            "@a.test",
        ] {
            assert!(addr(from).is_empty(), "{from}");
        }
        assert!(address_signals(None).is_empty());
    }

    #[test]
    fn bounce_address() {
        assert_eq!(addr("MAILER-DAEMON@a.test"), vec![Signal::BounceAddress]);
        assert_eq!(
            addr("Mail Delivery <mailer-daemon@googlemail.test>"),
            vec![Signal::BounceAddress]
        );
        assert!(addr("mailer-daemonx@a.test").is_empty());
    }

    fn hdr(name: &'static str, value: &'static str) -> Vec<Signal> {
        header_signals(&[(name, value)])
    }

    #[test]
    fn auto_submitted_table() {
        assert_eq!(
            hdr("Auto-Submitted", "auto-generated"),
            vec![Signal::AutoSubmitted]
        );
        assert_eq!(
            hdr("AUTO-SUBMITTED", "Auto-Replied"),
            vec![Signal::AutoSubmitted]
        );
        assert_eq!(
            hdr("auto-submitted", "auto-generated (por el servidor)"),
            vec![Signal::AutoSubmitted]
        );
        assert_eq!(
            hdr("Auto-Submitted", "auto-generated; x=y"),
            vec![Signal::AutoSubmitted]
        );
        assert_eq!(
            hdr("Auto-Submitted", "otro-valor"),
            vec![Signal::AutoSubmitted]
        );
        assert!(hdr("Auto-Submitted", "no").is_empty());
        assert!(hdr("Auto-Submitted", " No ").is_empty());
        assert!(hdr("Auto-Submitted", "NO (humano)").is_empty());
        assert!(hdr("Auto-Submitted", "no; x=y").is_empty());
        assert!(hdr("Auto-Submitted", "").is_empty());
        assert!(header_signals(&[("Subject", "auto-generated")]).is_empty());
        assert!(header_signals(&[]).is_empty());
    }

    #[test]
    fn precedence_table() {
        assert_eq!(hdr("Precedence", "junk"), vec![Signal::PrecedenceJunk]);
        assert_eq!(hdr("precedence", " BULK "), vec![Signal::PrecedenceBulk]);
        assert_eq!(hdr("PRECEDENCE", "List"), vec![Signal::PrecedenceList]);
        assert!(hdr("Precedence", "first-class").is_empty());
        assert!(hdr("Precedence", "").is_empty());
    }

    #[test]
    fn list_headers_and_return_path() {
        assert_eq!(hdr("List-Id", "<novedades.a.test>"), vec![Signal::ListId]);
        assert_eq!(
            hdr("list-unsubscribe", "<mailto:x@a.test>"),
            vec![Signal::ListUnsubscribe]
        );
        assert!(hdr("List-Id", "  ").is_empty());
        assert_eq!(hdr("Return-Path", "<>"), vec![Signal::NullReturnPath]);
        assert_eq!(hdr("return-path", "  < >  "), vec![Signal::NullReturnPath]);
        assert!(hdr("Return-Path", "<ana@a.test>").is_empty());
    }

    #[test]
    fn detect_merges_sorts_and_distinguishes_unchecked() {
        let headers = [
            ("Precedence", "bulk"),
            ("List-Id", "<x.test>"),
            ("precedence", "bulk"),
        ];
        let all = detect(Some("noreply@a.test"), Some(&headers));
        assert_eq!(
            codes(&all),
            vec!["noreply_address", "list_id", "precedence_bulk"]
        );
        // Sin comprobar las cabeceras, solo la dirección.
        assert_eq!(
            detect(Some("noreply@a.test"), None),
            vec![Signal::NoreplyAddress]
        );
        // Comprobadas y sin señales.
        assert!(detect(Some("ana@a.test"), Some(&[])).is_empty());
    }

    #[test]
    fn classification_and_codes_round_trip() {
        for s in Signal::ALL {
            assert_eq!(Signal::from_code(s.code()), Some(s));
            let n = [s.is_strong(), s.is_weak(), s.is_bounce()]
                .iter()
                .filter(|b| **b)
                .count();
            assert_eq!(n, 1, "{} en una sola clase", s.code());
        }
        assert_eq!(Signal::from_code("otra"), None);
    }

    #[test]
    fn routes_follow_the_precedence_and_unchecked_messages_use_the_address() {
        let codes = |c: &[&str]| c.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        let route_of = |from: &str, c: Option<&[String]>| route_of_message(Some(from), c).0;
        // Una señal débil sola no oculta; la fuerte sí; el rebote manda sobre la fuerte.
        assert_eq!(route_of("a@a.test", Some(&codes(&["list_id"]))), Route::Ia);
        assert_eq!(
            route_of("a@a.test", Some(&codes(&["auto_submitted", "list_id"]))),
            Route::Informativo
        );
        assert_eq!(
            route_of(
                "a@a.test",
                Some(&codes(&["auto_submitted", "null_return_path"]))
            ),
            Route::Rebote
        );
        // Sin comprobar, la dirección se recalcula; sin señal, a la IA.
        assert_eq!(route_of("no-reply@a.test", None), Route::Informativo);
        assert_eq!(route_of("mailer-daemon@a.test", None), Route::Rebote);
        assert_eq!(route_of("ana@a.test", None), Route::Ia);
        // Un código desconocido no cuenta; un mensaje sin remitente ni señales va a la IA.
        assert_eq!(route_of("a@a.test", Some(&codes(&["otro"]))), Route::Ia);
        assert_eq!(route_of_message(None, None).0, Route::Ia);
        assert_eq!(Route::Rebote.as_str(), "rebote");
    }
    /// Una fila por cada caso de la precedencia de D161 (la primera lectura queda fuera: sus
    /// mensajes ya tienen tarea y no llegan a `decide`).
    #[test]
    fn decide_follows_the_precedence_table() {
        use PreferenceRoute::{Informative, Review};
        use TriageSource::{Local, Preference, Thread};
        let strong = [Signal::NoreplyAddress];
        let weak = [Signal::ListId];
        let bounce = [Signal::BounceAddress];
        let both = [Signal::NullReturnPath, Signal::AutoSubmitted];
        let none: [Signal; 0] = [];
        // (señales, hilo propio, preferencia, ruta, quién, ya respondido)
        #[allow(clippy::type_complexity)]
        let table: Vec<(
            &[Signal],
            Option<bool>,
            Option<PreferenceRoute>,
            Route,
            TriageSource,
            bool,
        )> = vec![
            // Sin nada: señal fuerte, débil o ninguna, como antes de las preferencias.
            (&strong, None, None, Route::Informativo, Local, false),
            (&weak, None, None, Route::Ia, Local, false),
            (&none, None, None, Route::Ia, Local, false),
            (&bounce, None, None, Route::Rebote, Local, false),
            // El hilo desconocido o ajeno no tiene efecto.
            (&none, Some(false), None, Route::Ia, Local, false),
            (&strong, Some(false), None, Route::Informativo, Local, false),
            // Hilo propio: informativo ya respondido, con o sin señales.
            (&none, Some(true), None, Route::Informativo, Thread, true),
            (&weak, Some(true), None, Route::Informativo, Thread, true),
            (&strong, Some(true), None, Route::Informativo, Thread, true),
            // El hilo va antes que la preferencia, sea cual sea.
            (
                &none,
                Some(true),
                Some(Review),
                Route::Informativo,
                Thread,
                true,
            ),
            (
                &strong,
                Some(true),
                Some(Review),
                Route::Informativo,
                Thread,
                true,
            ),
            (
                &none,
                Some(true),
                Some(Informative),
                Route::Informativo,
                Thread,
                true,
            ),
            // Preferencia «Informativos»: cierra sin IA aunque no haya señal.
            (
                &none,
                None,
                Some(Informative),
                Route::Informativo,
                Preference,
                false,
            ),
            (
                &weak,
                None,
                Some(Informative),
                Route::Informativo,
                Preference,
                false,
            ),
            (
                &strong,
                None,
                Some(Informative),
                Route::Informativo,
                Preference,
                false,
            ),
            (
                &none,
                Some(false),
                Some(Informative),
                Route::Informativo,
                Preference,
                false,
            ),
            // Preferencia «Pendientes»: una señal fuerte cuenta como débil y va a la IA.
            (&strong, None, Some(Review), Route::Ia, Preference, false),
            (&both[1..], None, Some(Review), Route::Ia, Preference, false),
            (&none, None, Some(Review), Route::Ia, Preference, false),
            (&weak, None, Some(Review), Route::Ia, Preference, false),
            // Un rebote no cambia con ninguna preferencia ni con el hilo.
            (
                &bounce,
                None,
                Some(Informative),
                Route::Rebote,
                Local,
                false,
            ),
            (&bounce, None, Some(Review), Route::Rebote, Local, false),
            (&bounce, Some(true), None, Route::Rebote, Local, false),
            (
                &bounce,
                Some(true),
                Some(Informative),
                Route::Rebote,
                Local,
                false,
            ),
            (&both, None, Some(Informative), Route::Rebote, Local, false),
            (&both, Some(true), Some(Review), Route::Rebote, Local, false),
        ];
        for (signals, thread, preference, route, source, replied) in table {
            let got = decide(signals, thread, preference);
            let wanted = TriageDecision {
                route,
                source,
                already_replied: replied,
            };
            assert_eq!(
                got, wanted,
                "{signals:?} hilo={thread:?} preferencia={preference:?}"
            );
        }
    }

    #[test]
    fn sender_parts_lowercases_keeps_the_tag_and_rejects_garbage() {
        assert_eq!(
            sender_parts("Ana <Ana+x@Alfa.Test>"),
            Some(("ana+x@alfa.test".to_owned(), "alfa.test".to_owned()))
        );
        for bad in [
            "",
            "ana",
            "ana@",
            "@alfa.test",
            "ana@alfa",
            "a b@alfa.test",
            "ana@alfa.test.",
            "ana@al fa.test",
        ] {
            assert_eq!(sender_parts(bad), None, "{bad:?}");
        }
        assert_eq!(
            PreferenceRoute::from_code("review"),
            Some(PreferenceRoute::Review)
        );
        assert_eq!(PreferenceRoute::from_code("otra"), None);
    }
}
