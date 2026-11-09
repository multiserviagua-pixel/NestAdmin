//! Perfil del negocio (D163): horario, tono, firma y políticas que la persona escribe y que **solo**
//! la llamada de redacción puede recibir (nunca la clasificación ni la extracción). Excepción a
//! D102, como el afinado de D134, con las mismas garantías: vista previa literal, consentimiento
//! aparte ligado a proveedor, modelo y servidor, y retirada al cambiar de destino.
//!
//! *Decisiones de diseño.*
//! - **El consentimiento también cae al cambiar el contenido.** La persona consiente a lo que vio
//!   en la vista previa; guardar un perfil con otro bloque cambia lo que sale y debe volver a verse.
//!   Además `fp` (huella del bloque exacto) se comprueba al usarlo, así que aunque el borrado
//!   fallara no saldría nada.
//! - **Con la IA local, el perfil se usa sin consentimiento extra** (D102: no sale del equipo), también
//!   cuando el respaldo local atiende una redacción del proveedor externo. Esa redacción puede haber
//!   salido ya con el perfil hacia el externo si había consentimiento; sin él, solo va al local.
//! - **El texto no se guarda en ningún resultado**: solo `profile_fp`.

use nestadmin_ai::tasks::{draft_flags, profile_section};
use nestadmin_ai::ProviderKind;
use nestadmin_core::{BusinessProfile, MAX_PROFILE_CHARS};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::learn_state::LearnedStyle;
use crate::{App, AppError, AppResult, ProviderInfo};

/// Prefijo de las claves de `rule_settings` con la decisión sobre la propuesta de perfil aprendida
/// (D171): `learned_profile:<cuenta>` guarda `{status, input_fp, at}`, nunca texto.
const LEARNED_PROFILE_PREFIX: &str = "learned_profile:";

/// Ajuste (`rule_settings`) con el consentimiento del perfil.
const CONSENT_KEY: &str = "business_profile_consent";

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_millis()).ok())
        .unwrap_or(0)
}

/// Aviso fijo que la interfaz muestra junto al editor (D163).
pub const PROFILE_NOTICE: &str =
    "Escriba solo lo que pueda leer cualquier remitente: la IA puede citarlo en una respuesta.";

/// Un hallazgo del examen del contenido: en qué campo y de qué tipo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProfileFinding {
    pub field: &'static str,
    /// Bloqueos: `iban`, `card`, `secret`. Avisos: `phone`, `email`.
    pub code: &'static str,
}

/// Perfil guardado, con su tamaño. Sin perfil, los campos van vacíos y `saved` es falso.
#[derive(Debug, Clone, Serialize)]
pub struct ProfileView {
    pub profile: BusinessProfile,
    pub saved: bool,
    pub updated_at: Option<i64>,
    pub chars: usize,
    pub max_chars: usize,
}

/// Resultado de guardar: el perfil y los avisos (teléfono, correo) que no bloquean.
#[derive(Debug, Clone, Serialize)]
pub struct ProfileSaved {
    pub view: ProfileView,
    pub warnings: Vec<ProfileFinding>,
    /// El consentimiento se retiró porque cambió lo que saldría.
    pub consent_withdrawn: bool,
}

/// Consentimiento del perfil frente al destino y al bloque de ahora.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProfileConsentView {
    /// Solo hace falta con un proveedor externo.
    pub required: bool,
    /// `not_needed` (IA local), `missing`, `granted` o `stale` (cambió el destino o el contenido).
    pub state: &'static str,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub host: Option<String>,
    pub at: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
struct StoredConsent {
    provider: String,
    model: String,
    #[serde(default)]
    host: String,
    fp: String,
    at: i64,
}

/// Lo que saldría en la redacción y a quién, para mostrarlo antes de consentir.
#[derive(Debug, Clone, Serialize)]
pub struct ProfilePreview {
    pub provider: ProviderInfo,
    /// Servidor real al que saldría; `None` con la IA local.
    pub host: Option<String>,
    /// El proveedor es externo: el perfil saldría del equipo.
    pub external: bool,
    /// Hay un perfil con contenido.
    pub has_profile: bool,
    /// El bloque literal que se añade a las instrucciones de la redacción; `None` si no hay perfil
    /// o no puede enviarse.
    pub block: Option<String>,
    /// Huella corta del bloque; `grant_profile_consent` la exige.
    pub fp: Option<String>,
    /// Por qué el perfil guardado no puede enviarse (`iban`, `card`, `secret`, `too_long`).
    pub blocked: Option<&'static str>,
    /// Si la próxima redacción lo llevaría ahora mismo (con el proveedor elegido).
    pub will_send: bool,
    /// Con un externo y respaldo local, si un fallo se repetiría con la IA local, que también
    /// recibe el perfil sin salir del equipo.
    pub local_fallback: bool,
    pub consent: ProfileConsentView,
    pub warnings: Vec<ProfileFinding>,
    pub notice: &'static str,
}

/// Perfil listo para la redacción de una tarea (privado del crate).
#[derive(Clone)]
pub(crate) struct ProfileUse {
    /// Texto compuesto, el que `profile_section` envuelve (y el que valida los contactos del
    /// borrador). Solo vive en memoria.
    pub text: String,
    pub fp: String,
    /// El proveedor elegido puede recibirlo: es local o hay consentimiento vigente.
    pub primary_ok: bool,
}

// ---- Propuesta de perfil aprendida (D171) ----

/// Propuesta de edición del perfil a partir del estilo aprendido en la primera carga, con su vista
/// de antes y después. Solo toca el campo `tone`; la firma nunca se propone (no salió del equipo y
/// no se tiene).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LearnedProfileProposal {
    pub account: String,
    /// Huella del resultado aprendido del que sale.
    pub input_fp: String,
    /// Huella de antes y después: aceptar exige la misma (si el perfil cambió, hay que revisarla).
    pub fp: String,
    pub before: BusinessProfile,
    pub after: BusinessProfile,
    /// Campos que cambian, con su nombre estable.
    pub changed: Vec<&'static str>,
    pub chars_after: usize,
    pub max_chars: usize,
    /// Hallazgos que impedirían guardar el «después» (IBAN, tarjeta, clave o exceso de tamaño).
    pub blocking: Vec<ProfileFinding>,
    pub warnings: Vec<ProfileFinding>,
}

/// Una línea del estilo aprendido (saludo o despedida), en una sola línea y sin invisibles.
fn one_line(text: &str) -> String {
    clean(text).split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Texto del campo «tono» para un estilo aprendido: palabras fijas por enum («formal», «cercano»,
/// «neutro», «breve», «detallado») y, tal cual, el saludo y la despedida ya confirmados en local.
/// `None` si el estilo no aporta nada.
pub fn learned_tone_text(style: &LearnedStyle) -> Option<String> {
    let tone = match style.tone.as_deref() {
        Some("formal") => Some("formal"),
        Some("cercano") => Some("cercano"),
        Some("neutro") => Some("neutro"),
        _ => None,
    };
    let length = match style.length.as_deref() {
        Some("breve") => Some("breve"),
        Some("detallado") => Some("detallado"),
        _ => None,
    };
    let mut text = [tone, length]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(", ");
    for (label, line) in [
        ("Saludo habitual", &style.greeting),
        ("Despedida habitual", &style.closing),
    ] {
        if let Some(line) = line.as_deref().map(one_line).filter(|l| !l.is_empty()) {
            if !text.is_empty() {
                text.push_str(". ");
            }
            text.push_str(&format!("{label}: «{line}»"));
        }
    }
    if text.is_empty() {
        return None;
    }
    if !text.ends_with('.') {
        text.push('.');
    }
    Some(text)
}

/// Perfil «después»: el de antes con el campo `tone` del estilo aprendido. La firma, el horario,
/// las políticas y el nombre no se tocan. `None` si no hay nada que proponer o no cambia nada.
pub fn propose_profile(style: &LearnedStyle, before: &BusinessProfile) -> Option<BusinessProfile> {
    let tone = learned_tone_text(style)?;
    if before.tone.trim() == tone {
        return None;
    }
    Some(BusinessProfile {
        tone,
        ..before.clone()
    })
}

/// Huella de la propuesta: el «antes» y el «después» exactos.
fn proposal_fingerprint(before: &BusinessProfile, after: &BusinessProfile) -> String {
    let mut text = String::new();
    for p in [before, after] {
        for (field, value) in p.fields() {
            text.push_str(field);
            text.push('\u{1f}');
            text.push_str(value);
            text.push('\u{1e}');
        }
        text.push('\u{1d}');
    }
    fingerprint(&text)
}

// ---- Examen del contenido ----

/// Hallazgos que bloquean el guardado, por campo.
pub fn blocking_findings(profile: &BusinessProfile) -> Vec<ProfileFinding> {
    let mut found = Vec::new();
    if profile.chars() > MAX_PROFILE_CHARS {
        found.push(ProfileFinding {
            field: "total",
            code: "too_long",
        });
    }
    for (field, text) in profile.fields() {
        let text = &examined(text);
        if draft_flags(text).contains(&"iban") || has_compact_iban(text) {
            found.push(ProfileFinding {
                field,
                code: "iban",
            });
        }
        if has_card_number(text) {
            found.push(ProfileFinding {
                field,
                code: "card",
            });
        }
        if looks_like_secret(text) {
            found.push(ProfileFinding {
                field,
                code: "secret",
            });
        }
    }
    found
}

/// Avisos que no bloquean: teléfono y correo (el negocio puede querer publicarlos).
pub fn warning_findings(profile: &BusinessProfile) -> Vec<ProfileFinding> {
    let mut found = Vec::new();
    for (field, text) in profile.fields() {
        let flags = draft_flags(&examined(text));
        if flags.contains(&"phone") {
            found.push(ProfileFinding {
                field,
                code: "phone",
            });
        }
        if flags.contains(&"email") {
            found.push(ProfileFinding {
                field,
                code: "email",
            });
        }
    }
    found
}

fn block_message(findings: &[ProfileFinding]) -> String {
    let has = |code: &str| findings.iter().any(|f| f.code == code);
    let mut parts = Vec::new();
    if has("too_long") {
        parts.push(format!(
            "El perfil supera los {MAX_PROFILE_CHARS} caracteres: acórtelo."
        ));
    }
    if has("iban") {
        parts.push("Hay un IBAN: quítelo, no puede ir en el perfil.".to_owned());
    }
    if has("card") {
        parts.push("Hay un número de tarjeta: quítelo, no puede ir en el perfil.".to_owned());
    }
    if has("secret") {
        parts.push("Hay algo que parece una clave o un token: quítelo.".to_owned());
    }
    parts.join(" ")
}

/// El texto tal como se examina: el que saldría (sin caracteres invisibles de formato) y, además,
/// con las letras y cifras de ancho completo pasadas a ASCII y los espacios no separables como
/// espacios, para que ninguna de esas escrituras esquive el examen.
pub(crate) fn examined(text: &str) -> String {
    clean(text)
        .chars()
        .map(|c| match c {
            '\u{FF01}'..='\u{FF5E}' => char::from_u32(c as u32 - 0xFEE0).unwrap_or(c),
            '\u{00A0}' | '\u{202F}' | '\u{2007}' => ' ',
            _ => c,
        })
        .collect()
}

/// Un IBAN escrito con cualquier separación («ES 91 2100…», con guiones o puntos): dos letras, dos
/// cifras y hasta 30 caracteres más que cumplen el control módulo 97, buscados sobre el texto sin
/// separadores. El control evita confundirlo con texto corriente.
pub(crate) fn has_compact_iban(text: &str) -> bool {
    let compact: Vec<char> = text
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_uppercase())
        .collect();
    (0..compact.len()).any(|i| {
        compact.get(i..i + 4).is_some_and(|head| {
            head[..2].iter().all(char::is_ascii_uppercase)
                && head[2..].iter().all(char::is_ascii_digit)
        }) && (15..=34)
            .filter(|len| i + len <= compact.len())
            .any(|len| iban_mod97(&compact[i..i + len]))
    })
}

pub(crate) fn iban_mod97(window: &[char]) -> bool {
    let rotated = window[4..].iter().chain(&window[..4]);
    let mut rest: u32 = 0;
    for c in rotated {
        let Some(value) = c.to_digit(36) else {
            return false;
        };
        rest = if value > 9 {
            (rest * 100 + value) % 97
        } else {
            (rest * 10 + value) % 97
        };
    }
    rest == 1
}

/// Un número de tarjeta: 13 a 19 cifras seguidas, o en grupos de al menos cuatro separados por
/// espacios, puntos o guiones (hasta tres caracteres), que cumplen la comprobación de Luhn. Un teléfono («600 123 123») o una fecha
/// («2026-10-01») no son grupos de cuatro y no cuentan.
pub(crate) fn has_card_number(text: &str) -> bool {
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if !chars[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        // Una racha de grupos de cifras separados por un solo espacio o guion.
        let mut groups: Vec<String> = Vec::new();
        let mut j = i;
        loop {
            let start = j;
            while j < chars.len() && chars[j].is_ascii_digit() {
                j += 1;
            }
            groups.push(chars[start..j].iter().collect());
            let mut k = j;
            while k < chars.len()
                && k - j < 3
                && (chars[k].is_whitespace() || matches!(chars[k], '.' | '-'))
            {
                k += 1;
            }
            if k > j && k < chars.len() && chars[k].is_ascii_digit() {
                j = k;
            } else {
                break;
            }
        }
        let digits: String = groups.concat();
        if (13..=19).contains(&digits.len()) && groups.iter().all(|g| g.len() >= 4) && luhn(&digits)
        {
            return true;
        }
        // Un grupo largo suelto también cuenta aunque venga pegado a otros pequeños.
        if groups
            .iter()
            .any(|g| (13..=19).contains(&g.len()) && luhn(g))
        {
            return true;
        }
        i = j.max(i + 1);
    }
    false
}

pub(crate) fn luhn(digits: &str) -> bool {
    let mut sum = 0;
    for (i, c) in digits.chars().rev().enumerate() {
        let Some(mut d) = c.to_digit(10) else {
            return false;
        };
        if i % 2 == 1 {
            d *= 2;
            if d > 9 {
                d -= 9;
            }
        }
        sum += d;
    }
    sum % 10 == 0
}

/// Algo que parece una clave, un token o una contraseña: prefijos conocidos, claves privadas,
/// «clave: valor» y palabras largas de letras y cifras mezcladas. Es conservador: ante la duda
/// bloquea, y la persona puede acortar o reescribir.
pub(crate) fn looks_like_secret(text: &str) -> bool {
    // Sin caracteres invisibles que partan un prefijo: `examined` ya los quitó en quien llama.
    let lower = text.to_lowercase();
    if lower.contains("-----begin") || lower.contains("bearer ") {
        return true;
    }
    // «contraseña: abc123», «api key = xyz…».
    for keyword in [
        "password",
        "passwd",
        "contraseña",
        "secret",
        "api key",
        "api_key",
        "apikey",
        "token",
        "clave",
    ] {
        let mut from = 0;
        while let Some(found) = lower[from..].find(keyword) {
            let rest = lower[from + found + keyword.len()..].trim_start();
            if let Some(value) = rest.strip_prefix([':', '=']) {
                if value
                    .trim_start()
                    .chars()
                    .take_while(|c| !c.is_whitespace())
                    .count()
                    >= 6
                {
                    return true;
                }
            }
            from += found + keyword.len();
        }
    }
    text.split(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '+' | '=')))
        .any(|token| {
            let known_prefix = [
                "sk-",
                "sk_",
                "pk_",
                "rk_",
                "ghp_",
                "gho_",
                "ghs_",
                "github_pat_",
                "xoxb-",
                "xoxp-",
                "xoxa-",
                "glpat-",
                "AKIA",
                "AIza",
                "eyJ",
            ]
            .iter()
            .any(|p| token.starts_with(p) && token.len() >= p.len() + 8);
            let letters = token.chars().any(|c| c.is_ascii_alphabetic());
            let digits = token.chars().any(|c| c.is_ascii_digit());
            let mixed_long = token.len() >= 24 && letters && digits;
            let hex_long = token.len() >= 32 && token.chars().all(|c| c.is_ascii_hexdigit());
            known_prefix || mixed_long || hex_long
        })
}

// ---- Composición y huella ----

fn clean(field: &str) -> String {
    field
        .chars()
        .filter(|c| !nestadmin_ai::refine::is_format_char(*c))
        .collect::<String>()
        .trim()
        .to_owned()
}

/// Texto del perfil tal como lo recibe `profile_section`: un campo por línea, solo los rellenados.
pub fn compose(profile: &BusinessProfile) -> String {
    let label = |field: &str| match field {
        "display_name" => "Nombre del negocio",
        "hours" => "Horario",
        "tone" => "Tono",
        "signature" => "Firma",
        _ => "Políticas",
    };
    profile
        .fields()
        .iter()
        .filter_map(|(field, value)| {
            let value = clean(value);
            (!value.is_empty()).then(|| format!("{}: {value}", label(field)))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Huella corta del bloque exacto que saldría.
fn fingerprint(section: &str) -> String {
    let digest = Sha256::digest(section.as_bytes());
    digest.iter().take(8).map(|b| format!("{b:02x}")).collect()
}

impl App {
    /// Perfil guardado (o vacío).
    pub fn business_profile(&self) -> AppResult<ProfileView> {
        let stored = self.store.business_profile()?;
        let saved = stored.is_some();
        let (profile, updated_at) = match stored {
            Some((profile, at)) => (profile, Some(at)),
            None => (BusinessProfile::default(), None),
        };
        Ok(ProfileView {
            chars: profile.chars(),
            max_chars: MAX_PROFILE_CHARS,
            profile,
            saved,
            updated_at,
        })
    }

    /// Datos de la empresa para precargar el formulario: nombre y una firma con la dirección y el
    /// correo. Nunca el NIF ni el IBAN. Solo lee; nada se guarda ni se envía.
    pub fn business_profile_prefill(&self) -> AppResult<BusinessProfile> {
        let Some(company) = self.store.sales_company().map_err(|e| {
            AppError::Invalid(format!("no se pudieron leer los datos de la empresa: {e}"))
        })?
        else {
            return Ok(BusinessProfile::default());
        };
        let name = company.name.unwrap_or_default().trim().to_owned();
        let signature = [
            Some(name.as_str()),
            company.address.as_deref(),
            company.email.as_deref(),
        ]
        .into_iter()
        .flatten()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
        Ok(BusinessProfile {
            display_name: name,
            signature,
            ..Default::default()
        })
    }

    /// Guarda el perfil. Bloquea el exceso de tamaño, los IBAN, las tarjetas y lo que parezca una
    /// clave; avisa, sin bloquear, de teléfonos y correos. Un perfil en blanco lo borra. Si cambia
    /// lo que saldría, retira el consentimiento.
    pub fn save_business_profile(&mut self, input: BusinessProfile) -> AppResult<ProfileSaved> {
        let profile = BusinessProfile {
            display_name: input.display_name.trim().to_owned(),
            hours: input.hours.trim().to_owned(),
            tone: input.tone.trim().to_owned(),
            signature: input.signature.trim().to_owned(),
            policies: input.policies.trim().to_owned(),
        };
        let blocking = blocking_findings(&profile);
        if !blocking.is_empty() {
            return Err(AppError::Invalid(block_message(&blocking)));
        }
        let warnings = warning_findings(&profile);
        let before = self.profile_fingerprint();
        if profile.is_blank() {
            self.store.clear_business_profile()?;
        } else {
            self.store.save_business_profile(&profile)?;
        }
        let consent_withdrawn = self.drop_profile_consent_if_changed(before.as_deref());
        Ok(ProfileSaved {
            view: self.business_profile()?,
            warnings,
            consent_withdrawn,
        })
    }

    /// Borra el perfil y retira el consentimiento.
    pub fn clear_business_profile(&mut self) -> AppResult<ProfileSaved> {
        self.save_business_profile(BusinessProfile::default())
    }

    /// Huella del bloque que saldría ahora, si hay un perfil que pueda enviarse.
    fn profile_fingerprint(&self) -> Option<String> {
        self.profile_block().map(|(_, _, fp)| fp)
    }

    /// Texto compuesto, bloque exacto (con su separación) y su huella, si hay un perfil con contenido que pase el
    /// examen. Un perfil que ya no lo pasa (guardado antes de endurecerse) no sale.
    fn profile_block(&self) -> Option<(String, String, String)> {
        let (profile, _) = self.store.business_profile().ok().flatten()?;
        if profile.is_blank() || !blocking_findings(&profile).is_empty() {
            return None;
        }
        let raw = compose(&profile);
        let section = profile_section(&raw)?;
        let fp = fingerprint(&section);
        Some((raw, section, fp))
    }

    /// Servidor y proveedor activos, y si salen del equipo.
    fn profile_destination(&self) -> (String, String, String, bool) {
        let (provider, model) = self.asked();
        let external = self.provider.kind() != ProviderKind::Ollama;
        (provider, model, self.host(), external)
    }

    /// Consentimiento frente al destino y al bloque de ahora.
    pub fn profile_consent(&self) -> AppResult<ProfileConsentView> {
        let (provider, model, host, external) = self.profile_destination();
        if !external {
            return Ok(ProfileConsentView {
                required: false,
                state: "not_needed",
                provider: None,
                model: None,
                host: None,
                at: None,
            });
        }
        let current = self.profile_fingerprint();
        let view = |state, p: String, m: String, h: String, at| ProfileConsentView {
            required: true,
            state,
            provider: Some(p),
            model: Some(m),
            host: Some(h).filter(|h| !h.is_empty()),
            at,
        };
        let stored = self
            .store
            .rule_setting(CONSENT_KEY)?
            .and_then(|json| serde_json::from_str::<StoredConsent>(&json).ok());
        Ok(match stored {
            None => view("missing", provider, model, host, None),
            Some(c)
                if c.provider == provider
                    && c.model == model
                    && c.host == host
                    && current.as_deref() == Some(c.fp.as_str()) =>
            {
                view("granted", c.provider, c.model, c.host, Some(c.at))
            }
            Some(c) => view("stale", c.provider, c.model, c.host, Some(c.at)),
        })
    }

    /// Qué saldría en la redacción y a quién. Solo lectura: no llama al proveedor.
    pub fn profile_preview(&self) -> AppResult<ProfilePreview> {
        let saved = self.store.business_profile()?.map(|(p, _)| p);
        let has_profile = saved.as_ref().is_some_and(|p| !p.is_blank());
        let blocked = saved
            .as_ref()
            .filter(|p| !p.is_blank())
            .and_then(|p| blocking_findings(p).first().map(|f| f.code));
        let warnings = saved.as_ref().map(warning_findings).unwrap_or_default();
        let block = self.profile_block();
        let (_, _, host, external) = self.profile_destination();
        let consent = self.profile_consent()?;
        let will_send = block.is_some() && (!external || consent.state == "granted");
        Ok(ProfilePreview {
            provider: self.provider(),
            host: Some(host).filter(|h| !h.is_empty()),
            external,
            has_profile,
            block: block
                .as_ref()
                .map(|(_, section, _)| section.trim_start_matches('\n').to_owned()),
            fp: block.map(|(_, _, fp)| fp),
            blocked,
            will_send,
            local_fallback: external && self.fallback.is_some(),
            consent,
            warnings,
            notice: PROFILE_NOTICE,
        })
    }

    /// La persona consiente enviar el perfil que vio al proveedor, modelo y servidor que vio.
    /// Rechaza si algo cambió desde entonces. Con la IA local no hace falta.
    pub fn grant_profile_consent(
        &mut self,
        provider: &str,
        model: &str,
        host: &str,
        fp: &str,
    ) -> AppResult<ProfileConsentView> {
        let (p, m, h, external) = self.profile_destination();
        if !external {
            return Err(AppError::Invalid(
                "Con la IA local el perfil no sale del equipo: no hace falta consentimiento."
                    .into(),
            ));
        }
        let Some(current) = self.profile_fingerprint() else {
            return Err(AppError::Invalid(
                "No hay un perfil que enviar: guárdelo primero.".into(),
            ));
        };
        if (p.as_str(), m.as_str(), h.as_str()) != (provider, model, host) || current != fp {
            return Err(AppError::Invalid(
                "El proveedor, el modelo, el servidor o el perfil han cambiado desde que lo vio: revise la vista previa de nuevo antes de aceptar."
                    .into(),
            ));
        }
        self.store.set_rule_setting(
            CONSENT_KEY,
            &json!({ "provider": provider, "model": model, "host": host, "fp": fp, "at": now_ms() })
                .to_string(),
            &format!(
                "consentimiento del perfil del negocio concedido: proveedor {provider}, modelo {model}, servidor {host}"
            ),
        )?;
        self.profile_consent()
    }

    /// Retira el consentimiento: la redacción deja de llevar el perfil a un proveedor externo.
    pub fn withdraw_profile_consent(&mut self) -> AppResult<ProfileConsentView> {
        self.store.delete_rule_setting(
            CONSENT_KEY,
            "consentimiento del perfil del negocio retirado",
        )?;
        self.profile_consent()
    }

    /// Retira el consentimiento si el proveedor, el modelo o el servidor ya no son los suyos. Se
    /// llama al cambiar de proveedor.
    pub(crate) fn drop_profile_consent_if_destination_changed(&mut self) {
        let Ok(Some(json)) = self.store.rule_setting(CONSENT_KEY) else {
            return;
        };
        let (provider, model) = self.asked();
        let same = serde_json::from_str::<StoredConsent>(&json)
            .is_ok_and(|c| c.provider == provider && c.model == model && c.host == self.host());
        if !same {
            let _ = self.store.delete_rule_setting(
                CONSENT_KEY,
                "consentimiento del perfil del negocio retirado: cambió el proveedor, el modelo o el servidor",
            );
        }
    }

    /// Retira el consentimiento si el bloque que saldría ya no es el que se consintió.
    fn drop_profile_consent_if_changed(&mut self, before: Option<&str>) -> bool {
        let after = self.profile_fingerprint();
        if before == after.as_deref() {
            return false;
        }
        self.store
            .delete_rule_setting(
                CONSENT_KEY,
                "consentimiento del perfil del negocio retirado: cambió el perfil",
            )
            .unwrap_or(false)
    }

    /// Decisión guardada sobre la propuesta de una cuenta: `(estado, input_fp)`.
    fn learned_profile_decision(&self, account: &str) -> AppResult<Option<(String, String)>> {
        Ok(self
            .store
            .rule_setting(&format!("{LEARNED_PROFILE_PREFIX}{account}"))?
            .and_then(|json| serde_json::from_str::<serde_json::Value>(&json).ok())
            .and_then(|v| {
                Some((
                    v["status"].as_str()?.to_owned(),
                    v["input_fp"].as_str()?.to_owned(),
                ))
            }))
    }

    /// Propuesta de perfil de lo aprendido en una cuenta (D171). Solo lectura. `None` si no hay
    /// aprendizaje terminado, si no aporta un estilo nuevo o si la persona ya la aceptó o la
    /// descartó. Nada de lo que se muestra sale del equipo.
    pub fn learned_profile_proposal(
        &self,
        account: &str,
    ) -> AppResult<Option<LearnedProfileProposal>> {
        let Some(learned) = self.learned_result(account)? else {
            return Ok(None);
        };
        if self
            .learned_profile_decision(account)?
            .is_some_and(|(_, fp)| fp == learned.input_fp)
        {
            return Ok(None);
        }
        let before = self.business_profile()?.profile;
        let Some(after) = propose_profile(&learned.style, &before) else {
            return Ok(None);
        };
        let changed = before
            .fields()
            .iter()
            .zip(after.fields())
            .filter(|((_, a), (_, b))| a.trim() != b.trim())
            .map(|((field, _), _)| *field)
            .collect();
        Ok(Some(LearnedProfileProposal {
            account: account.to_owned(),
            input_fp: learned.input_fp,
            fp: proposal_fingerprint(&before, &after),
            blocking: blocking_findings(&after),
            warnings: warning_findings(&after),
            chars_after: after.chars(),
            max_chars: MAX_PROFILE_CHARS,
            changed,
            before,
            after,
        }))
    }

    /// Acepta la propuesta que la persona vio (`fp`) por el guardado normal del perfil: mismos
    /// bloqueos (IBAN, tarjeta, clave), mismo tope de 2.000 caracteres y misma retirada del
    /// consentimiento del perfil si cambia lo que saldría (D163). Si el perfil cambió desde que la
    /// vio, se rechaza. Repetirlo tras aceptarla no cambia nada.
    pub fn accept_learned_profile(&mut self, account: &str, fp: &str) -> AppResult<ProfileSaved> {
        let Some(proposal) = self.learned_profile_proposal(account)? else {
            let accepted = match self.learned_result(account)? {
                Some(l) => self
                    .learned_profile_decision(account)?
                    .is_some_and(|(status, input)| status == "accepted" && input == l.input_fp),
                None => false,
            };
            if accepted {
                return Ok(ProfileSaved {
                    view: self.business_profile()?,
                    warnings: Vec::new(),
                    consent_withdrawn: false,
                });
            }
            return Err(AppError::Invalid(
                "No hay una propuesta de perfil vigente para esta cuenta.".into(),
            ));
        };
        if proposal.fp != fp {
            return Err(AppError::Invalid(
                "El perfil ha cambiado desde que vio la propuesta: revise el antes y el después de nuevo."
                    .into(),
            ));
        }
        let saved = self.save_business_profile(proposal.after)?;
        self.store.set_rule_setting(
            &format!("{LEARNED_PROFILE_PREFIX}{account}"),
            &json!({ "status": "accepted", "input_fp": proposal.input_fp, "at": now_ms() })
                .to_string(),
            &format!(
                "propuesta de perfil aprendida aceptada: campos {}",
                proposal.changed.join(", ")
            ),
        )?;
        Ok(saved)
    }

    /// Descarta la propuesta: queda registrado, sin contenido, y no vuelve a ofrecerse para ese
    /// aprendizaje. Descartar dos veces no cambia nada.
    pub fn dismiss_learned_profile(&mut self, account: &str) -> AppResult<()> {
        let Some(learned) = self.learned_result(account)? else {
            return Err(AppError::Invalid(
                "Esta cuenta no tiene nada aprendido que proponer.".into(),
            ));
        };
        if self
            .learned_profile_decision(account)?
            .is_some_and(|(_, fp)| fp == learned.input_fp)
        {
            return Ok(());
        }
        self.store.set_rule_setting(
            &format!("{LEARNED_PROFILE_PREFIX}{account}"),
            &json!({ "status": "dismissed", "input_fp": learned.input_fp, "at": now_ms() })
                .to_string(),
            "propuesta de perfil aprendida descartada",
        )?;
        Ok(())
    }

    /// Perfil para la redacción de la tarea que se toma ahora. Fallo cerrado: sin perfil, con un
    /// perfil que no pasa el examen o ante cualquier error de lectura, no hay perfil.
    pub(crate) fn profile_for_draft(&self) -> Option<ProfileUse> {
        let (text, _, fp) = self.profile_block()?;
        let (_, _, _, external) = self.profile_destination();
        let primary_ok = !external || self.profile_consent().is_ok_and(|c| c.state == "granted");
        Some(ProfileUse {
            text,
            fp,
            primary_ok,
        })
    }
}
