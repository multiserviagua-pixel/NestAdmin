//! Sugerencias de reglas calculadas solo por código (D132, R24). `suggest` es pura y
//! determinista: no lee la base de datos, no usa el reloj ni la IA y no escribe nada. Recibe
//! señales ya guardadas (D129), las reglas, las decisiones previas del usuario y los umbrales,
//! y devuelve las candidatas con su evidencia en un orden estable.
//!
//! Una sugerencia nunca se activa sola: aceptarla crea una regla **inactiva**. `Signal` no
//! tiene campo de cuerpo ni de asunto: el texto de un correo no puede provocar una
//! sugerencia.

use std::collections::{BTreeMap, HashMap, HashSet};

use nestadmin_core::{Rule, SuggestionRow, Task};
use nestadmin_documents::parse_amount;
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::rules::{
    analyze, compare, evaluate_result, foreign_currencies, is_email_address, is_public_mail_domain,
    shows_euro, FindingKind, Marks, Relation, RuleDecision, RuleFinding, Side,
};
use crate::{check_rule, App, AppError, AppResult, EMAIL, INVOICE, REGISTER_INVOICE, SEND_DRAFT};

/// Id de la regla hipotética de una candidata en los hallazgos: no es un id real
/// (`check_rule_id` lo rechaza).
pub const SUGGESTED: &str = "(sugerida)";
/// Ajustes en `rule_settings`.
pub const SETTING_INVOICE_MIN: &str = "suggest_invoice_min";
pub const SETTING_REPLY_MIN: &str = "suggest_reply_min";
pub const SETTING_LEARNED_REPLY_MIN: &str = "suggest_learned_reply_min";
/// Facturas limpias del mismo proveedor que hacen falta por defecto.
pub const DEFAULT_INVOICE_MIN: u32 = 3;
/// Respuestas aprobadas sin edición, por remitente y categoría, por defecto.
pub const DEFAULT_REPLY_MIN: u32 = 5;
/// Pares aprendidos en la primera carga (D172) con la misma dirección y categoría, por defecto.
/// Es un umbral propio: lo aprendido no se mezcla con las aprobaciones.
pub const DEFAULT_LEARNED_REPLY_MIN: u32 = 3;
/// Pares y direcciones mínimas para proponer una regla por dominio desde lo aprendido: el
/// umbral `learned_reply` nunca la baja de aquí (D172).
pub const DOMAIN_MIN_PAIRS: u32 = 3;
pub const DOMAIN_MIN_ADDRESSES: usize = 2;
/// Origen de una candidata: aprobaciones de la persona o pares aprendidos en la primera carga.
pub const ORIGIN_APPROVED: &str = "approved";
pub const ORIGIN_LEARNED: &str = "learned";
/// Observaciones mínimas de facturas que solo se *vieron* en la primera lectura (E5, D133) para
/// sugerir una regla cuando la persona no ha aprobado suficientes: sin una aprobación humana que
/// confirme que el modelo leyó bien las cifras, se exige más repetición que con aprobaciones.
/// El umbral de facturas ajustable (`suggest_invoice_min`) puede subirla, nunca bajarla.
pub const SEEN_INVOICE_MIN: u32 = 5;
/// Una sola observación nunca basta: el mínimo de ambos umbrales.
pub const MIN_THRESHOLD: u32 = 2;
/// Máximo razonable de un umbral.
pub const MAX_THRESHOLD: u32 = 50;
/// Cuántas pistas de «cuánto falta» se devuelven como mucho.
const MAX_PROGRESS: usize = 5;
/// Tareas citadas como mucho en el origen de la regla.
const MAX_ORIGIN_TASKS: usize = 20;
const MAX_NAME_CHARS: usize = 80;

/// Umbrales de evidencia. Siempre dentro de `MIN_THRESHOLD..=MAX_THRESHOLD`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Thresholds {
    /// Facturas limpias del mismo proveedor.
    pub invoice: u32,
    /// Respuestas aprobadas sin edición por remitente y categoría.
    pub reply: u32,
    /// Pares aprendidos en la primera carga por remitente y categoría (D172).
    pub learned_reply: u32,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self {
            invoice: DEFAULT_INVOICE_MIN,
            reply: DEFAULT_REPLY_MIN,
            learned_reply: DEFAULT_LEARNED_REPLY_MIN,
        }
    }
}

/// Ajustes visibles en el panel: los umbrales y sus límites.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ThresholdSettings {
    pub invoice: u32,
    pub reply: u32,
    pub learned_reply: u32,
    pub min: u32,
    pub max: u32,
    pub default_invoice: u32,
    pub default_reply: u32,
    pub default_learned_reply: u32,
}

/// Una señal de aprendizaje (D129) con lo que `suggest` necesita. Sin cuerpo ni asunto.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signal {
    /// Orden de llegada (creciente).
    pub id: i64,
    /// Momento en que se guardó (ms); solo se muestra, nunca ordena ni decide.
    pub at: i64,
    /// `approved`, `edited`, `rejected`, `observed` o `learned` (D172: un par aprendido en la
    /// primera carga, sin tarea).
    pub kind: String,
    pub action_kind: String,
    /// Factura: proveedor recortado y en minúsculas. Correo: `categoría|remitente`.
    pub subject_key: String,
    /// `unedited`, `edited` o `unknown`.
    pub edit_state: String,
    /// Tarea de la señal; 0 en las `learned`, que no tienen.
    pub task_id: i64,
    /// SHA-256 del documento de una factura; en `learned`, la fuente y el id del correo recibido.
    pub source_key: Option<String>,
    /// `valid`, `pending` o `void`. Solo cuentan las `valid`.
    pub state: String,
    /// Factura: proveedor tal como lo escribe el documento.
    pub display: Option<String>,
    /// Factura: total canónico («1200.00») si se pudo leer.
    pub amount: Option<String>,
    /// Factura: una regla sin límite habría podido automatizarla (sin incidencias, con euro
    /// visible, sin importe dudoso ni factura separada). Los correos no lo usan.
    pub clean: bool,
    /// Factura marcada solo como posible duplicada (D132): aprobarla o rechazarla no suma ni
    /// corta la serie.
    pub neutral: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecisionKind {
    Discarded,
    Accepted,
}

/// Decisión previa del usuario sobre una sugerencia, por huella de condiciones.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    pub fingerprint: String,
    pub kind: DecisionKind,
    /// Observaciones de la serie cuando se descartó.
    pub baseline: u32,
    /// Mayor id de señal que existía al descartar: lo posterior es evidencia nueva.
    pub watermark: i64,
}

/// Si una sugerencia descartada vuelve a aparecer, y por qué.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Reappearance {
    /// Observaciones que había al descartarla.
    pub at_discard: u32,
    /// Observaciones nuevas desde entonces.
    pub new_since: u32,
    /// Las que hacían falta para que reapareciera (el doble de las que había).
    pub needed_new: u32,
}

/// Datos de la serie que sostiene la sugerencia. Todas las cifras salen de aquí.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Evidence {
    /// Observaciones de la serie actual, sin repetidas.
    pub count: u32,
    pub threshold: u32,
    /// Veces que la serie se cortó antes por una edición, un rechazo, un dato sin clasificar
    /// o una factura con incidencias. Cero: ningún corte.
    pub earlier_breaks: u32,
    /// Cuándo se guardó la última observación (ms).
    pub last_at: i64,
    pub task_ids: Vec<i64>,
    /// Facturas: los importes de las facturas **aprobadas** por la persona, en orden. Las solo
    /// vistas en la primera lectura no aportan importes (D133).
    pub amounts: Vec<String>,
    /// Facturas: límite propuesto (el mayor de los importes aprobados). `None` si no hay ninguna
    /// aprobada: un límite no sale de lo que solo se ha visto.
    pub limit: Option<String>,
    /// Facturas sin ninguna aprobada: la persona debe escribir el límite; no se crea con un clic.
    pub needs_limit: bool,
    /// `approved` (aprobaciones de la persona) o `learned` (pares de la primera carga, D172). Una
    /// candidata nunca mezcla las dos series.
    pub origin: &'static str,
    /// Cuántas de las `count` son pares aprendidos (solo con origen `learned`; si no, cero).
    pub learned: u32,
    /// Direcciones distintas de la serie (una, salvo en las reglas por dominio aprendidas).
    pub addresses: u32,
    /// Cuántas de las `count` son aprobaciones de la persona (ya ejecutadas). Con origen
    /// `learned` es cero y `seen` también.
    pub approved: u32,
    /// Cuántas de las `count` solo se vieron en la primera lectura (E5): facturas existentes que
    /// la persona nunca aprobó. `approved + seen == count`.
    pub seen: u32,
    /// Facturas: el menor y el segundo mayor de los importes observados.
    pub amount_min: Option<String>,
    pub amount_second: Option<String>,
    /// Facturas: el mayor es al menos el triple del segundo mayor.
    pub limit_outlier: bool,
    pub reappeared: Option<Reappearance>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Candidate {
    /// Huella de las condiciones que identifican la sugerencia (sin el límite, que depende
    /// de los importes observados).
    pub fingerprint: String,
    /// Huella de las condiciones tal como se muestran (incluido el límite): aceptar exige la
    /// misma, como `plan_fingerprint` al unificar (D131).
    pub shown: String,
    pub action_kind: String,
    /// Nombre propuesto para la regla.
    pub name: String,
    pub conditions: Value,
    /// Proveedor o remitente, para el título.
    pub subject: String,
    pub category: Option<String>,
    pub evidence: Evidence,
    /// Contradicciones que crearía la regla **si se activara**, calculadas con `analyze`; la
    /// candidata va como `(sugerida)`. Al aceptarla nace inactiva y no choca con nada.
    pub overlaps: Vec<RuleFinding>,
}

/// Lo que falta para que algo se sugiera.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Progress {
    /// `approved` o `learned`.
    pub origin: &'static str,
    pub action_kind: String,
    pub subject: String,
    pub category: Option<String>,
    pub count: u32,
    pub approved: u32,
    pub seen: u32,
    pub needed: u32,
    pub missing: u32,
}

/// Lo que no se muestra y por qué (para no ocultar sin decirlo).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Hidden {
    /// Ya existe una regla (activa o inactiva) con las mismas condiciones.
    pub covered: u32,
    /// Descartadas por el usuario y sin evidencia nueva suficiente.
    pub discarded: u32,
    /// Ya aceptadas.
    pub accepted: u32,
    /// Condiciones que no pasan la validación de reglas (por ejemplo, datos ilegibles).
    pub invalid: u32,
    /// Respuestas de la categoría `otros`, que no se sugiere nunca (D132).
    pub not_suggested: u32,
    /// Proveedores cuyas facturas solo leídas no cuentan porque basta una con incidencias, otra
    /// moneda o un total no positivo (D133). Sus aprobaciones se conservan.
    pub seen_invalidated: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Suggestions {
    pub candidates: Vec<Candidate>,
    pub progress: Vec<Progress>,
    /// Señales válidas que se miraron. Cero: aún no hay datos.
    pub observed: u32,
    /// De ellas, decisiones de la persona (aprobar, editar o rechazar).
    pub decisions: u32,
    /// De ellas, facturas vistas en la primera lectura (E5): no son decisiones de nadie.
    pub seen: u32,
    /// Señales `learned` válidas (D172): ni cuentan en `observed` ni son decisiones.
    pub learned: u32,
    /// Observaciones que cuentan como evidencia en las series actuales (de todos los sujetos,
    /// se muestren o no).
    pub counted: u32,
    pub hidden: Hidden,
    pub thresholds: ThresholdSettings,
    /// Mayor id de señal de la entrada (de cualquier estado): al descartar, lo posterior es
    /// evidencia nueva.
    pub watermark: i64,
}

/// Serie de observaciones de un sujeto.
struct Series<'a> {
    items: Vec<&'a Signal>,
    breaks: u32,
    /// Alguna factura solo vista de este proveedor no sirve (incidencias, otra moneda, total no
    /// positivo): ninguna de las vistas cuenta (D133).
    seen_blocked: bool,
}

impl Series<'_> {
    /// Aprobaciones de la persona entre las observaciones de la serie.
    fn approved(&self) -> u32 {
        let n = self.items.iter().filter(|s| s.kind == "approved").count();
        u32::try_from(n).unwrap_or(u32::MAX)
    }

    /// Facturas que solo se vieron en la primera lectura.
    fn seen(&self) -> u32 {
        let n = self.items.iter().filter(|s| s.kind == "observed").count();
        u32::try_from(n).unwrap_or(u32::MAX)
    }
}

/// Observaciones que hacen falta para sugerir (D132, D133): con aprobaciones de la persona al
/// menos iguales al umbral, el umbral; si no, y habiendo vistas en la primera lectura, el mayor
/// de ese umbral y [`SEEN_INVOICE_MIN`]. Sin vistas, el umbral de siempre.
fn needed_for(invoice: bool, approved: u32, seen: u32, thresholds: Thresholds) -> u32 {
    if !invoice {
        return thresholds.reply;
    }
    if approved >= thresholds.invoice || seen == 0 {
        thresholds.invoice
    } else {
        thresholds.invoice.max(SEEN_INVOICE_MIN)
    }
}

/// Recorre las señales de un sujeto y devuelve la serie actual.
///
/// **Decisiones de la persona** (en orden de llegada): cuenta una aprobación válida sin edición
/// (en facturas, además limpia y con importe positivo). Una edición, un rechazo, un «sin
/// clasificar» o una factura con incidencias **reinician** la serie: lo anterior ya no sostiene la
/// sugerencia y hacen falta nuevas observaciones tras el corte. No son evidencia, ni cortan, los
/// reintentos de una misma tarea, los documentos con el mismo hash ni las facturas marcadas solo
/// como posible duplicado (`neutral`).
///
/// **Facturas solo vistas en la primera lectura** (D133): lo que nadie decidió no corta
/// aprobaciones, y su recuento no depende del orden de lectura. Cuentan las limpias, con importe
/// positivo, una por documento y tarea y que la persona no haya decidido (ni aprobado, ni
/// rechazado, ni editado). Si **alguna** vista de ese proveedor tiene incidencias, otra moneda o un
/// total no positivo, ninguna de las vistas cuenta (la serie de lo visto se invalida entera, no
/// solo lo anterior), y las aprobaciones quedan como estaban. Nunca hay vistas en respuestas.
fn series<'a>(group: &[&'a Signal], invoice: bool) -> Series<'a> {
    let mut items: Vec<&Signal> = Vec::new();
    let mut breaks = 0;
    let mut seen_tasks: HashSet<i64> = HashSet::new();
    let mut seen_sources: HashSet<&str> = HashSet::new();
    let mut break_tasks: HashSet<i64> = HashSet::new();
    let mut decided_sources: HashSet<&str> = HashSet::new();
    for &s in group {
        // `learned` tiene su propia serie (`learned_series`): no suma ni corta la de aprobaciones.
        if matches!(s.kind.as_str(), "observed" | "learned") {
            continue;
        }
        if let Some(k) = s.source_key.as_deref() {
            decided_sources.insert(k);
        }
        if s.neutral {
            continue;
        }
        let counts = s.kind == "approved"
            && s.edit_state == "unedited"
            // Una factura de total cero o negativo (rectificativa) no es una factura normal: no
            // cuenta, corta la serie y nunca propone un límite.
            && (!invoice
                || (s.clean && s.amount.as_deref().is_some_and(is_positive_amount)));
        if counts {
            let repeated_source = s
                .source_key
                .as_deref()
                .is_some_and(|k| !seen_sources.insert(k));
            if repeated_source || !seen_tasks.insert(s.task_id) {
                continue;
            }
            items.push(s);
        } else {
            // Todo corte reinicia la serie, también el segundo de la misma tarea; una tarea
            // editada y aprobada deja dos señales pero cuenta como un solo corte.
            items.clear();
            if break_tasks.insert(s.task_id) {
                breaks += 1;
            }
        }
    }
    let mut seen_blocked = false;
    if invoice {
        let observed: Vec<&Signal> = group
            .iter()
            .copied()
            .filter(|s| s.kind == "observed" && !s.neutral)
            .collect();
        let usable = |s: &Signal| {
            s.clean
                && s.edit_state == "unedited"
                && s.amount.as_deref().is_some_and(is_positive_amount)
        };
        seen_blocked = !observed.is_empty() && !observed.iter().all(|s| usable(s));
        if !seen_blocked {
            for s in observed {
                let source_known = s
                    .source_key
                    .as_deref()
                    .is_some_and(|k| decided_sources.contains(k) || !seen_sources.insert(k));
                if source_known || !seen_tasks.insert(s.task_id) {
                    continue;
                }
                items.push(s);
            }
        }
    }
    items.sort_by_key(|s| s.id);
    Series {
        items,
        breaks,
        seen_blocked,
    }
}

/// Una serie lista para convertirse en candidata, de aprobaciones o aprendida.
struct Unit<'a> {
    action_kind: &'a str,
    series: Series<'a>,
    approved: u32,
    seen: u32,
    needed: u32,
    draft: Draft,
    origin: &'static str,
    addresses: u32,
}

/// Serie de pares aprendidos de un sujeto (D172), en orden de llegada. Cada correo recibido cuenta
/// una vez. Una aprobación sin edición posterior no la toca (es otra serie y no se mezcla); una
/// edición, un rechazo o una aprobación «sin clasificar» del mismo sujeto la **reinician**, igual
/// que reinician la de aprobaciones.
fn learned_series<'a>(group: &[&'a Signal]) -> Series<'a> {
    let mut items: Vec<&Signal> = Vec::new();
    let mut seen_sources: HashSet<&str> = HashSet::new();
    let mut breaks = 0;
    let mut break_tasks: HashSet<i64> = HashSet::new();
    for &s in group {
        match s.kind.as_str() {
            "learned" => {
                if s.source_key
                    .as_deref()
                    .is_none_or(|k| seen_sources.insert(k))
                {
                    items.push(s);
                }
            }
            "approved" if s.edit_state == "unedited" => {}
            "approved" | "edited" | "rejected" => {
                items.clear();
                if break_tasks.insert(s.task_id) {
                    breaks += 1;
                }
            }
            _ => {}
        }
    }
    items.sort_by_key(|s| s.id);
    Series {
        items,
        breaks,
        seen_blocked: false,
    }
}

/// Unidad aprendida por dirección exacta y categoría (D172). Sin pares aprendidos, `None`.
fn learned_address_unit<'a>(
    subject_key: &str,
    group: &[&'a Signal],
    thresholds: Thresholds,
) -> Option<Unit<'a>> {
    if !group.iter().any(|s| s.kind == "learned") {
        return None;
    }
    // Nunca «otros»: lo aprendido tampoco lo sugiere (D132).
    if subject_key
        .split_once('|')
        .is_none_or(|(c, _)| c == "otros")
    {
        return None;
    }
    let series = learned_series(group);
    if series.items.is_empty() {
        return None;
    }
    let mut draft = draft_of(SEND_DRAFT, subject_key, &series)?;
    // Los pares aprendidos no tienen tarea que citar en el origen de la regla.
    draft.items = Vec::new();
    Some(Unit {
        action_kind: SEND_DRAFT,
        series,
        approved: 0,
        seen: 0,
        needed: thresholds.learned_reply,
        draft,
        origin: ORIGIN_LEARNED,
        addresses: 1,
    })
}

/// Unidades aprendidas por dominio (D172): un dominio **no público**, con al menos
/// [`DOMAIN_MIN_ADDRESSES`] direcciones distintas y [`DOMAIN_MIN_PAIRS`] pares (el umbral
/// `learned_reply`, si es mayor). Una edición o un rechazo de cualquier dirección del dominio en
/// esa categoría reinicia la serie. Nunca «cualquier remitente» ni `otros`.
fn learned_domain_units<'a>(valid: &[&'a Signal], thresholds: Thresholds) -> Vec<Unit<'a>> {
    // (categoría, dominio) -> señales de envío de ese dominio, en orden de llegada.
    let mut by_domain: BTreeMap<(&str, &str), Vec<&'a Signal>> = BTreeMap::new();
    for &s in valid.iter().filter(|s| s.action_kind == SEND_DRAFT) {
        let Some((category, sender)) = s.subject_key.split_once('|') else {
            continue;
        };
        if category.is_empty()
            || category == "otros"
            || !is_email_address(sender)
            || sender != sender.to_ascii_lowercase()
        {
            continue;
        }
        let Some((_, domain)) = sender.rsplit_once('@') else {
            continue;
        };
        if is_public_mail_domain(domain) {
            continue;
        }
        by_domain.entry((category, domain)).or_default().push(s);
    }
    let needed = thresholds.learned_reply.max(DOMAIN_MIN_PAIRS);
    let mut out = Vec::new();
    for ((category, domain), group) in by_domain {
        if !group.iter().any(|s| s.kind == "learned") {
            continue;
        }
        let series = learned_series(&group);
        let addresses: HashSet<&str> = series
            .items
            .iter()
            .filter_map(|s| s.subject_key.split_once('|').map(|(_, a)| a))
            .collect();
        // Con menos direcciones de las debidas no es una sugerencia ni «falta poco»: una sola
        // dirección ya la cubre la regla exacta.
        if series.items.is_empty() || addresses.len() < DOMAIN_MIN_ADDRESSES {
            continue;
        }
        let conditions = json!({ "category": category, "sender_domain": domain });
        let draft = Draft {
            fingerprint: fingerprint_of(&[
                SEND_DRAFT,
                "category",
                category,
                "sender_domain",
                domain,
            ]),
            name: sanitize_name(&format!("Respuestas a @{domain} ({category})")),
            conditions_no_limit: conditions.clone(),
            conditions,
            subject: format!("@{domain}"),
            category: Some(category.to_owned()),
            items: Vec::new(),
        };
        out.push(Unit {
            action_kind: SEND_DRAFT,
            series,
            approved: 0,
            seen: 0,
            needed,
            draft,
            origin: ORIGIN_LEARNED,
            addresses: u32::try_from(addresses.len()).unwrap_or(u32::MAX),
        });
    }
    out
}

/// Un importe canónico («1200.00») es positivo: no es cero ni negativo (rectificativas).
pub(crate) fn is_positive_amount(amount: &str) -> bool {
    !amount.starts_with('-') && amount.chars().any(|c| c.is_ascii_digit() && c != '0')
}

pub(crate) fn sanitize_name(raw: &str) -> String {
    raw.chars()
        .filter(|c| !c.is_control())
        .take(MAX_NAME_CHARS)
        .collect::<String>()
        .trim()
        .to_owned()
}

pub(crate) fn fingerprint_of(parts: &[&str]) -> String {
    let digest = Sha256::digest(parts.join("\u{1f}").as_bytes());
    digest.iter().take(10).map(|b| format!("{b:02x}")).collect()
}

fn hypothetical(action_kind: &str, conditions: &Value) -> Rule {
    Rule {
        id: SUGGESTED.into(),
        version: 1,
        action_kind: action_kind.into(),
        conditions: conditions.clone(),
        // Los hallazgos describen lo que pasaría al activarla; la regla que se cree nace
        // inactiva de todos modos.
        active: true,
        created_at: 0,
    }
}

/// Una candidata antes de comprobarla contra las reglas existentes.
struct Draft {
    fingerprint: String,
    name: String,
    conditions: Value,
    conditions_no_limit: Value,
    subject: String,
    category: Option<String>,
    items: Vec<i64>,
}

/// Calcula las sugerencias. Pura: mismos datos, mismo resultado, mismo orden.
///
/// - Solo cuentan las señales `valid`; `pending` y `void` no existen para esta función.
/// - Factura: `thresholds.invoice` facturas limpias del mismo proveedor proponen «registrar
///   sin revisión» con un límite igual al mayor importe observado.
/// - Respuesta: `thresholds.reply` respuestas aprobadas sin edición de la misma dirección y
///   categoría; nunca «cualquier remitente» (D126).
/// - No propone lo que ya cubre una regla no archivada con las mismas condiciones. Si
///   solaparía con otra, la candidata lleva los hallazgos de `analyze` que lo explican.
/// - Una sugerencia descartada reaparece solo si la evidencia nueva posterior al descarte es
///   al menos el doble de la que había al descartarla.
pub fn suggest(
    signals: &[Signal],
    rules: &[Rule],
    marks: &Marks,
    decisions: &[Decision],
    thresholds: Thresholds,
) -> Suggestions {
    let thresholds = Thresholds {
        invoice: thresholds.invoice.clamp(MIN_THRESHOLD, MAX_THRESHOLD),
        reply: thresholds.reply.clamp(MIN_THRESHOLD, MAX_THRESHOLD),
        learned_reply: thresholds.learned_reply.clamp(MIN_THRESHOLD, MAX_THRESHOLD),
    };
    let mut out = Suggestions {
        candidates: Vec::new(),
        progress: Vec::new(),
        observed: 0,
        decisions: 0,
        seen: 0,
        learned: 0,
        counted: 0,
        hidden: Hidden::default(),
        thresholds: ThresholdSettings {
            invoice: thresholds.invoice,
            reply: thresholds.reply,
            learned_reply: thresholds.learned_reply,
            min: MIN_THRESHOLD,
            max: MAX_THRESHOLD,
            default_invoice: DEFAULT_INVOICE_MIN,
            default_reply: DEFAULT_REPLY_MIN,
            default_learned_reply: DEFAULT_LEARNED_REPLY_MIN,
        },
        watermark: signals.iter().map(|s| s.id).max().unwrap_or(0),
    };

    let mut valid: Vec<&Signal> = signals.iter().filter(|s| s.state == "valid").collect();
    valid.sort_by_key(|s| s.id);
    out.learned =
        u32::try_from(valid.iter().filter(|s| s.kind == "learned").count()).unwrap_or(u32::MAX);
    out.observed = u32::try_from(valid.len()).unwrap_or(u32::MAX) - out.learned;
    out.seen =
        u32::try_from(valid.iter().filter(|s| s.kind == "observed").count()).unwrap_or(u32::MAX);
    out.decisions = out.observed - out.seen;

    let mut groups: BTreeMap<(&str, &str), Vec<&Signal>> = BTreeMap::new();
    for s in &valid {
        if matches!(s.action_kind.as_str(), REGISTER_INVOICE | SEND_DRAFT) {
            groups
                .entry((s.action_kind.as_str(), s.subject_key.as_str()))
                .or_default()
                .push(s);
        }
    }

    let live: Vec<Rule> = rules
        .iter()
        .filter(|r| !marks.archived.contains(&r.id))
        .cloned()
        .collect();
    let decision_of: HashMap<&str, &Decision> = decisions
        .iter()
        .map(|d| (d.fingerprint.as_str(), d))
        .collect();

    // Primera pasada: una unidad por serie con evidencia. Las de aprobaciones y facturas primero;
    // después las aprendidas (D172), que nunca se mezclan con ellas.
    let mut units: Vec<Unit<'_>> = Vec::new();
    let mut learned_units: Vec<Unit<'_>> = Vec::new();
    for ((action_kind, subject_key), group) in &groups {
        let (action_kind, subject_key) = (*action_kind, *subject_key);
        let invoice = action_kind == REGISTER_INVOICE;
        'approved: {
            let series = series(group, invoice);
            if series.seen_blocked {
                out.hidden.seen_invalidated += 1;
            }
            let (approved, seen) = (series.approved(), series.seen());
            let needed = needed_for(invoice, approved, seen, thresholds);
            let count = u32::try_from(series.items.len()).unwrap_or(u32::MAX);
            out.counted = out.counted.saturating_add(count);
            if count == 0 {
                break 'approved;
            }
            // Un correo manipulado puede dirigir la clasificación hacia «otros»: no se sugiere
            // contestar sin revisión en esa categoría (se puede crear a mano).
            if !invoice
                && subject_key
                    .split_once("|")
                    .is_some_and(|(c, _)| c == "otros")
            {
                out.hidden.not_suggested += 1;
                break 'approved;
            }
            let Some(draft) = draft_of(action_kind, subject_key, &series) else {
                out.hidden.invalid += 1;
                break 'approved;
            };
            units.push(Unit {
                action_kind,
                series,
                approved,
                seen,
                needed,
                draft,
                origin: ORIGIN_APPROVED,
                addresses: 1,
            });
        }
        if action_kind == SEND_DRAFT {
            if let Some(unit) = learned_address_unit(subject_key, group, thresholds) {
                learned_units.push(unit);
            }
        }
    }
    learned_units.extend(learned_domain_units(&valid, thresholds));
    units.extend(learned_units);

    let mut emitted: HashSet<String> = HashSet::new();
    for unit in units {
        let Unit {
            action_kind,
            series,
            approved,
            seen,
            needed,
            draft,
            origin,
            addresses,
        } = unit;
        let invoice = action_kind == REGISTER_INVOICE;
        let learned = origin == ORIGIN_LEARNED;
        let count = u32::try_from(series.items.len()).unwrap_or(u32::MAX);
        // La misma regla (mismas condiciones) ya sugerida por sus aprobaciones: una sola tarjeta.
        if learned && emitted.contains(&draft.fingerprint) {
            continue;
        }
        // Cubierta: una regla (activa o no) con las mismas condiciones, o una regla **activa**
        // que la contiene. Esta última ya automatiza lo mismo, y la candidata, al activarse,
        // haría que esas tareas coincidieran con dos reglas y fueran a revisión. Una regla
        // inactiva más amplia no se oculta: se muestra con su aviso.
        let covered_by = |conditions: &Value| {
            let h = hypothetical(action_kind, conditions);
            live.iter()
                .filter(|r| r.action_kind == action_kind)
                .any(|r| match compare(&h, r, marks) {
                    Relation::Equal => true,
                    Relation::Contains {
                        wider: Side::Second,
                    } => r.active,
                    _ => false,
                })
        };
        if count < needed {
            // Todavía no es una sugerencia. Lo que ya cubre una regla o está aceptado o
            // descartado se cuenta como oculto (con el mismo criterio que arriba) y no
            // promete «faltan N»; el resto solo dice cuánto falta.
            if covered_by(&draft.conditions_no_limit) {
                out.hidden.covered += 1;
            } else {
                match decision_of.get(draft.fingerprint.as_str()) {
                    Some(d) if d.kind == DecisionKind::Accepted => out.hidden.accepted += 1,
                    Some(_) => out.hidden.discarded += 1,
                    None => out.progress.push(Progress {
                        origin,
                        action_kind: action_kind.into(),
                        subject: draft.subject.clone(),
                        category: draft.category.clone(),
                        count,
                        approved,
                        seen,
                        needed,
                        missing: needed - count,
                    }),
                }
            }
            continue;
        }
        if check_rule(action_kind, &draft.conditions, false).is_err() {
            out.hidden.invalid += 1;
            continue;
        }
        if covered_by(&draft.conditions) {
            out.hidden.covered += 1;
            continue;
        }
        let mut reappeared = None;
        match decision_of.get(draft.fingerprint.as_str()) {
            Some(d) if d.kind == DecisionKind::Accepted => {
                out.hidden.accepted += 1;
                continue;
            }
            Some(d) => {
                let new_since =
                    u32::try_from(series.items.iter().filter(|s| s.id > d.watermark).count())
                        .unwrap_or(u32::MAX);
                if u64::from(new_since) < 2 * u64::from(d.baseline) {
                    out.hidden.discarded += 1;
                    continue;
                }
                reappeared = Some(Reappearance {
                    at_discard: d.baseline,
                    new_since,
                    needed_new: d.baseline.saturating_mul(2),
                });
            }
            None => {}
        }
        let last = series.items.last().copied();
        // Los importes y el límite salen solo de lo aprobado por la persona (D133).
        let amounts: Vec<String> = series
            .items
            .iter()
            .filter(|s| s.kind == "approved")
            .filter_map(|s| s.amount.clone())
            .collect();
        let limit = draft.conditions["max_total"].as_str().map(str::to_owned);
        let (amount_min, amount_second, limit_outlier) = amount_stats(&amounts);
        let shown = fingerprint_of(&[&draft.fingerprint, &draft.conditions.to_string()]);
        let overlaps = {
            let mut all: Vec<Rule> = live
                .iter()
                .filter(|r| r.action_kind == action_kind)
                .cloned()
                .collect();
            all.push(hypothetical(action_kind, &draft.conditions));
            analyze(
                &all,
                &Marks {
                    any_sender: marks.any_sender.clone(),
                    archived: HashSet::new(),
                },
            )
            .into_iter()
            .filter(|f| {
                f.kind != FindingKind::Defective && f.rules.iter().any(|r| r.id == SUGGESTED)
            })
            .collect()
        };
        emitted.insert(draft.fingerprint.clone());
        out.candidates.push(Candidate {
            shown,
            fingerprint: draft.fingerprint,
            action_kind: action_kind.into(),
            name: draft.name,
            conditions: draft.conditions,
            subject: draft.subject,
            category: draft.category,
            evidence: Evidence {
                count,
                threshold: needed,
                earlier_breaks: series.breaks,
                origin,
                learned: if learned { count } else { 0 },
                addresses,
                approved,
                seen,
                last_at: last.map_or(0, |s| s.at),
                task_ids: draft.items,
                amounts,
                limit,
                needs_limit: invoice && approved == 0,
                amount_min,
                amount_second,
                limit_outlier,
                reappeared,
            },
            overlaps,
        });
    }

    out.candidates.sort_by(|a, b| {
        b.evidence
            .count
            .cmp(&a.evidence.count)
            .then_with(|| a.action_kind.cmp(&b.action_kind))
            .then_with(|| a.subject.cmp(&b.subject))
            .then_with(|| a.category.cmp(&b.category))
            .then_with(|| a.fingerprint.cmp(&b.fingerprint))
    });
    // Una serie aprendida que ya es una tarjeta no promete además «faltan N» por sus aprobaciones.
    let shown: HashSet<(&str, &Option<String>, &str)> = out
        .candidates
        .iter()
        .map(|c| (c.action_kind.as_str(), &c.category, c.subject.as_str()))
        .collect();
    out.progress
        .retain(|p| !shown.contains(&(p.action_kind.as_str(), &p.category, p.subject.as_str())));
    out.progress.sort_by(|a, b| {
        a.missing
            .cmp(&b.missing)
            .then_with(|| b.count.cmp(&a.count))
            .then_with(|| a.action_kind.cmp(&b.action_kind))
            .then_with(|| a.subject.cmp(&b.subject))
            .then_with(|| a.category.cmp(&b.category))
    });
    out.progress.truncate(MAX_PROGRESS);
    out
}

/// Menor importe, segundo mayor y si el mayor es al menos el triple del segundo mayor.
fn amount_stats(amounts: &[String]) -> (Option<String>, Option<String>, bool) {
    let mut parsed: Vec<_> = amounts
        .iter()
        .filter_map(|a| parse_amount("total", a).ok())
        .collect();
    parsed.sort_by(|a, b| b.cmp(a));
    match parsed.as_slice() {
        [max, .., min] => {
            let second = parsed[1];
            (
                Some(min.to_string()),
                Some(second.to_string()),
                is_positive_amount(&second.to_string()) && *max >= second + second + second,
            )
        }
        _ => (None, None, false),
    }
}

/// Condiciones y datos de la candidata de un sujeto. `None` si el sujeto no sirve (proveedor
/// vacío o con caracteres de control, remitente que no es una dirección simple…).
fn draft_of(action_kind: &str, subject_key: &str, series: &Series<'_>) -> Option<Draft> {
    let last = series.items.last()?;
    let task_ids: Vec<i64> = series.items.iter().map(|s| s.task_id).collect();
    if action_kind == REGISTER_INVOICE {
        let supplier = last
            .display
            .as_deref()
            .map(str::trim)
            .filter(|d| !d.is_empty())
            .unwrap_or(subject_key);
        if subject_key.trim().is_empty()
            || supplier.is_empty()
            || supplier.chars().any(char::is_control)
        {
            return None;
        }
        // El límite sale solo de importes aprobados; sin aprobaciones no hay límite que proponer.
        let limit = series
            .items
            .iter()
            .filter(|s| s.kind == "approved")
            .filter_map(|s| s.amount.as_deref())
            .filter(|a| is_positive_amount(a))
            .filter_map(|a| parse_amount("total", a).ok())
            .max()
            .map(|m| m.to_string())
            .filter(|l| is_positive_amount(l));
        let name = sanitize_name(&format!("Facturas de {supplier}"));
        let conditions = match &limit {
            Some(limit) => json!({ "supplier": supplier, "max_total": limit }),
            None => json!({ "supplier": supplier }),
        };
        return Some(Draft {
            fingerprint: fingerprint_of(&[REGISTER_INVOICE, "supplier", subject_key.trim()]),
            name,
            conditions,
            conditions_no_limit: json!({ "supplier": supplier }),
            subject: supplier.to_owned(),
            category: None,
            items: task_ids,
        });
    }
    // La clave es `categoría|remitente`: la categoría no lleva «|», pero una dirección sí
    // puede (`x|jefe@empresa.test` es una dirección simple válida).
    let (category, sender) = subject_key.split_once("|")?;
    if category.trim().is_empty()
        || !is_email_address(sender)
        || sender != sender.to_ascii_lowercase()
    {
        return None;
    }
    let conditions = json!({ "category": category, "sender": sender });
    Some(Draft {
        fingerprint: fingerprint_of(&[SEND_DRAFT, "category", category, "sender", sender]),
        name: sanitize_name(&format!("Respuestas a {sender} ({category})")),
        conditions_no_limit: conditions.clone(),
        conditions,
        subject: sender.to_owned(),
        category: Some(category.to_owned()),
        items: task_ids,
    })
}

// ---- Integración con la aplicación ----

/// Lo que se precarga en el formulario al crear una regla desde una revisión (R24). Solo
/// lectura: crear la regla sigue siendo `create_rule`, que nace inactiva.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RuleSeed {
    pub action_kind: String,
    pub conditions: Value,
    pub name: String,
    /// Facturas: el límite es el total de esta factura, no una estadística.
    pub limit_from_this_invoice: bool,
    /// Facturas: por qué no se precargó el límite (solo el proveedor), si es el caso.
    pub limit_skipped: Option<LimitSkipped>,
}

/// Una factura que solo se leyó en la primera lectura (D133).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SeenInvoice {
    pub task_id: i64,
    pub document: Option<String>,
    /// Fecha de la factura tal como la leyó el modelo; `None` si no constaba.
    pub date: Option<String>,
    pub amount: Option<String>,
    /// Cuándo se leyó (ms).
    pub read_at: i64,
}

/// Por qué una factura no aporta un límite fiable.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LimitSkipped {
    /// `foreign_currency`, `currency_not_shown`, `currency_not_checked`, `amount_review` o
    /// `no_total`.
    pub code: String,
    /// Monedas distintas del euro que menciona el documento (`foreign_currency`).
    pub currencies: Vec<String>,
}

/// Lo que se crea al aceptar una sugerencia (ver `App::accept_plan`).
pub(crate) struct AcceptPlan {
    pub(crate) action_kind: String,
    pub(crate) conditions: Value,
    pub(crate) name: String,
    pub(crate) origin: Value,
    pub(crate) evidence: Value,
}

impl App {
    /// Señales con lo que necesita `suggest`. Solo lectura. Solo se leen las tareas de
    /// facturas con una aprobación o un rechazo válidos (proveedor, importe, si estaba limpia
    /// o era solo un posible duplicado); no se analizan todas las tareas.
    fn suggestion_signals(&self) -> AppResult<Vec<Signal>> {
        let rows = self.store.rule_signals()?;
        let mut ids: Vec<i64> = rows
            .iter()
            .filter(|r| {
                r.action_kind == REGISTER_INVOICE
                    && r.state == "valid"
                    && matches!(r.kind.as_str(), "approved" | "rejected" | "observed")
            })
            .filter_map(|r| r.task_id)
            .collect();
        ids.sort_unstable();
        ids.dedup();
        let tasks: HashMap<i64, Task> = self
            .store
            .tasks_by_ids(&ids)?
            .into_iter()
            .map(|t| (t.id, t))
            .collect();
        let mut out = Vec::new();
        for row in rows {
            let mut signal = Signal {
                id: row.id,
                at: row.at,
                kind: row.kind,
                action_kind: row.action_kind,
                subject_key: row.subject_key,
                edit_state: row.edit_state,
                // Las `learned` no tienen tarea (D172).
                task_id: row.task_id.unwrap_or(0),
                source_key: row.source_key,
                state: row.state,
                display: None,
                amount: None,
                clean: false,
                neutral: false,
            };
            if let Some(result) = tasks
                .get(&signal.task_id)
                .filter(|t| t.kind == INVOICE)
                .and_then(|t| t.result.as_ref())
                .filter(|r| r.is_object())
            {
                fill_invoice(&mut signal, result);
            }
            out.push(signal);
        }
        Ok(out)
    }

    fn suggestion_decisions(&self) -> AppResult<Vec<Decision>> {
        Ok(self
            .store
            .rule_suggestion_rows()?
            .into_iter()
            .filter_map(decision_of_row)
            .collect())
    }

    /// Umbrales vigentes. Un valor guardado fuera de rango o ilegible (editado a mano) se
    /// ignora y se usa el de fábrica: el mínimo se aplica también aquí, no solo al guardar.
    pub fn suggestion_thresholds(&self) -> AppResult<ThresholdSettings> {
        let read = |key: &str, default: u32| -> AppResult<u32> {
            Ok(self
                .store
                .rule_setting(key)?
                .and_then(|v| v.trim().parse::<u32>().ok())
                .filter(|v| (MIN_THRESHOLD..=MAX_THRESHOLD).contains(v))
                .unwrap_or(default))
        };
        Ok(ThresholdSettings {
            invoice: read(SETTING_INVOICE_MIN, DEFAULT_INVOICE_MIN)?,
            reply: read(SETTING_REPLY_MIN, DEFAULT_REPLY_MIN)?,
            learned_reply: read(SETTING_LEARNED_REPLY_MIN, DEFAULT_LEARNED_REPLY_MIN)?,
            min: MIN_THRESHOLD,
            max: MAX_THRESHOLD,
            default_invoice: DEFAULT_INVOICE_MIN,
            default_reply: DEFAULT_REPLY_MIN,
            default_learned_reply: DEFAULT_LEARNED_REPLY_MIN,
        })
    }

    /// Cambia un umbral (`invoice`, `reply` o `learned_reply`). Se valida aquí, no solo en la interfaz: entre
    /// `MIN_THRESHOLD` y `MAX_THRESHOLD`. Queda en el historial. Bajar un umbral no activa ni
    /// crea nada: solo puede hacer que aparezcan sugerencias, que siguen necesitando aceptación.
    pub fn set_suggestion_threshold(
        &mut self,
        which: &str,
        value: i64,
    ) -> AppResult<ThresholdSettings> {
        let (key, label) = match which {
            "invoice" => (SETTING_INVOICE_MIN, "facturas"),
            "reply" => (SETTING_REPLY_MIN, "respuestas"),
            "learned_reply" => (SETTING_LEARNED_REPLY_MIN, "respuestas aprendidas"),
            other => {
                return Err(AppError::Invalid(format!(
                    "umbral desconocido: {other}; use «invoice», «reply» o «learned_reply»"
                )))
            }
        };
        let valid = u32::try_from(value)
            .ok()
            .filter(|v| (MIN_THRESHOLD..=MAX_THRESHOLD).contains(v))
            .ok_or_else(|| {
                AppError::Invalid(format!(
                    "el umbral debe estar entre {MIN_THRESHOLD} y {MAX_THRESHOLD}: con una sola observación no se sugiere nada"
                ))
            })?;
        let current = self.suggestion_thresholds()?;
        let before = match which {
            "invoice" => current.invoice,
            "learned_reply" => current.learned_reply,
            _ => current.reply,
        };
        if before != valid {
            self.store.set_rule_setting(
                key,
                &valid.to_string(),
                &format!("umbral de {label} para sugerir reglas: {before} -> {valid}"),
            )?;
        }
        self.suggestion_thresholds()
    }

    /// Sugerencias de reglas calculadas por código. Solo lectura: no escribe nada (ni en el
    /// historial ni en las tareas), no llama a la IA y no contacta a ningún proveedor.
    pub fn rule_suggestions(&self) -> AppResult<Suggestions> {
        let (rules, marks) = self.analysis_inputs()?;
        let t = self.suggestion_thresholds()?;
        Ok(suggest(
            &self.suggestion_signals()?,
            &rules,
            &marks,
            &self.suggestion_decisions()?,
            Thresholds {
                invoice: t.invoice,
                reply: t.reply,
                learned_reply: t.learned_reply,
            },
        ))
    }

    /// Descarta una sugerencia vigente. Persiste: solo reaparece con el doble de evidencia
    /// nueva posterior a este momento. Descartar dos veces no cambia nada.
    pub fn discard_suggestion(&mut self, fingerprint: &str) -> AppResult<()> {
        let all = self.rule_suggestions()?;
        let Some(c) = all.candidates.iter().find(|c| c.fingerprint == fingerprint) else {
            return match self
                .store
                .rule_suggestion_rows()?
                .iter()
                .find(|r| r.fingerprint == fingerprint)
            {
                Some(r) if r.status == "discarded" => Ok(()),
                Some(_) => Err(AppError::Invalid(
                    "esa sugerencia ya se aceptó: no se puede descartar".into(),
                )),
                None => Err(AppError::Invalid(stale_message())),
            };
        };
        let evidence = json!({
            "baseline": c.evidence.count,
            "watermark": all.watermark,
            "action_kind": c.action_kind,
        });
        self.store.discard_suggestion(
            fingerprint,
            &evidence,
            &format!(
                "sugerencia {fingerprint} descartada con {} observaciones",
                c.evidence.count
            ),
        )?;
        Ok(())
    }

    /// Crea la regla de una sugerencia vigente, **inactiva**, con su origen, pasando por
    /// `check_rule` y auditada. Repetirlo (reintento o doble clic) devuelve la misma regla
    /// sin crear otra. Nunca la activa.
    ///
    /// `shown` es la huella de las condiciones que la persona vio (`Candidate.shown`): si los
    /// datos cambiaron desde entonces (otro límite, otro proveedor), se rechaza y hay que
    /// revisar la tarjeta de nuevo, como `plan_fingerprint` al unificar (D131).
    pub fn accept_suggestion(
        &mut self,
        fingerprint: &str,
        shown: &str,
        name: Option<&str>,
    ) -> AppResult<crate::panel::RuleView> {
        self.accept_suggestion_with_limit(fingerprint, shown, name, None)
    }

    /// Como [`App::accept_suggestion`], eligiendo el límite: `None` es el que calculó el código;
    /// `Some(importe)` es el que propuso la IA (D134), que debe seguir vigente y pasar otra vez
    /// `check_rule` y la simulación. `shown` es entonces la huella de la propuesta de la IA
    /// (`LimitProposal.shown`), no la de la tarjeta, de modo que lo que se crea es lo que la
    /// persona vio. La IA no decide nada: es la persona quien elige un límite u otro.
    pub fn accept_suggestion_with_limit(
        &mut self,
        fingerprint: &str,
        shown: &str,
        name: Option<&str>,
        model_limit: Option<&str>,
    ) -> AppResult<crate::panel::RuleView> {
        self.accept_inner(fingerprint, shown, name, model_limit, None)
    }

    /// Crea la regla de una sugerencia de facturas **solo vistas** (sin ninguna aprobada) con el
    /// límite que escribe la persona: obligatorio y positivo. Es la única forma de aceptarla; con
    /// un clic no se crea nada ni se crea nunca una regla sin límite por defecto (D133).
    pub fn accept_suggestion_with_user_limit(
        &mut self,
        fingerprint: &str,
        shown: &str,
        name: Option<&str>,
        limit: &str,
    ) -> AppResult<crate::panel::RuleView> {
        self.accept_inner(fingerprint, shown, name, None, Some(limit))
    }

    fn accept_inner(
        &mut self,
        fingerprint: &str,
        shown: &str,
        name: Option<&str>,
        model_limit: Option<&str>,
        user_limit: Option<&str>,
    ) -> AppResult<crate::panel::RuleView> {
        // Ya aceptada: la misma regla, sin crear nada.
        if let Some(id) = self
            .store
            .rule_suggestion_rows()?
            .iter()
            .find(|r| r.fingerprint == fingerprint && r.status == "accepted")
            .and_then(|r| r.evidence["rule_id"].as_str().map(str::to_owned))
        {
            if self.store.rule(&id)?.is_some() {
                return self.rule_view(&id);
            }
        }
        let all = self.rule_suggestions()?;
        let plan = self.accept_plan(&all, fingerprint, shown, name, model_limit, user_limit)?;
        let id = self.unique_rule_id(&plan.name)?;
        let (rule, _created) = self.store.accept_suggestion(
            fingerprint,
            &id,
            &plan.action_kind,
            &plan.conditions,
            Some(&plan.name),
            &plan.origin,
            &plan.evidence,
        )?;
        self.rule_view(&rule.id)
    }

    /// Lo que se crea al aceptar una sugerencia vigente: condiciones ya comprobadas con
    /// `check_rule`, nombre, origen y evidencia. Solo lectura. Lo comparten la aceptación de una
    /// tarjeta y la del lote aprendido (D173), que así no duplican la lógica.
    pub(crate) fn accept_plan(
        &self,
        all: &Suggestions,
        fingerprint: &str,
        shown: &str,
        name: Option<&str>,
        model_limit: Option<&str>,
        user_limit: Option<&str>,
    ) -> AppResult<AcceptPlan> {
        let c = all
            .candidates
            .iter()
            .find(|c| c.fingerprint == fingerprint)
            .ok_or_else(|| AppError::Invalid(stale_message()))?;
        let changed = || {
            AppError::Invalid(
                "La sugerencia ha cambiado desde que la vio (otro límite u otros datos): revise la tarjeta de nuevo antes de crear la regla."
                    .into(),
            )
        };
        let mut conditions = c.conditions.clone();
        let mut limit_source = "code";
        if c.evidence.needs_limit {
            // Solo vistas: la regla necesita un límite que escriba la persona.
            let Some(typed) = user_limit else {
                return Err(AppError::Invalid(
                    "Estas facturas solo se han leído: indique usted el límite de la regla; no se crea sin él."
                        .into(),
                ));
            };
            if c.shown != shown {
                return Err(changed());
            }
            let amount = parse_amount("total", typed)
                .ok()
                .map(|a| a.to_string())
                .filter(|a| is_positive_amount(a))
                .ok_or_else(|| {
                    AppError::Invalid(
                        "El límite debe ser un importe positivo, por ejemplo 500,00.".into(),
                    )
                })?;
            conditions["max_total"] = json!(amount);
            limit_source = "user";
        } else if user_limit.is_some() {
            return Err(AppError::Invalid(
                "Esta sugerencia ya lleva un límite calculado de sus aprobaciones: use «Crear regla»."
                    .into(),
            ));
        } else {
            match model_limit {
                None if c.shown != shown => return Err(changed()),
                None => {}
                Some(limit) => {
                    let proposal = self
                    .refinement_view_for(all)?
                    .limits
                    .into_iter()
                    .find(|l| l.fingerprint == c.fingerprint && l.max_total == limit)
                    .ok_or_else(|| {
                        AppError::Invalid(
                            "La propuesta de límite de la IA ya no está vigente: los datos cambiaron. Actualice la lista."
                                .into(),
                        )
                    })?;
                    if proposal.shown != shown {
                        return Err(changed());
                    }
                    // Se vuelve a comprobar con los datos de ahora, como cuando se propuso.
                    let checked = self.check_limit(c, &proposal.max_total).map_err(|code| {
                    AppError::Invalid(format!(
                        "La propuesta de límite de la IA ya no es válida ({code}). Actualice la lista."
                    ))
                })?;
                    if checked.shown != shown {
                        return Err(changed());
                    }
                    conditions["max_total"] = json!(checked.max_total);
                    limit_source = "model";
                }
            }
        }
        check_rule(&c.action_kind, &conditions, false)?;
        let name = match name.map(sanitize_name).filter(|n| !n.is_empty()) {
            Some(n) => n,
            None => c.name.clone(),
        };
        let mut task_ids = c.evidence.task_ids.clone();
        let skip = task_ids.len().saturating_sub(MAX_ORIGIN_TASKS);
        task_ids.drain(..skip);
        let origin = json!({
            "kind": "suggestion",
            "fingerprint": c.fingerprint,
            "count": c.evidence.count,
            "threshold": c.evidence.threshold,
            "earlier_breaks": c.evidence.earlier_breaks,
            // `learned`: aprendida en la primera carga (D172); si no, aprobaciones de la persona.
            "evidence_origin": c.evidence.origin,
            "learned": c.evidence.learned,
            "addresses": c.evidence.addresses,
            "approved": c.evidence.approved,
            "seen": c.evidence.seen,
            "limit": conditions["max_total"].as_str().or(c.evidence.limit.as_deref()),
            "limit_source": limit_source,
            "code_limit": c.evidence.limit,
            "task_ids": task_ids,
            "reappeared": c.evidence.reappeared.is_some(),
        });
        let evidence = json!({ "baseline": c.evidence.count, "watermark": all.watermark });
        Ok(AcceptPlan {
            action_kind: c.action_kind.clone(),
            conditions,
            name,
            origin,
            evidence,
        })
    }

    /// Las facturas que solo se leyeron (primera lectura) en la serie de una sugerencia, con el
    /// nombre del documento, la fecha de la factura y su importe, para que la persona las vea
    /// antes de decidir (D133). Solo lectura.
    pub fn seen_invoices(&self, fingerprint: &str) -> AppResult<Vec<SeenInvoice>> {
        let all = self.rule_suggestions()?;
        let Some(c) = all.candidates.iter().find(|c| c.fingerprint == fingerprint) else {
            return Err(AppError::Invalid(stale_message()));
        };
        let mut out = Vec::new();
        for task in self.store.tasks_by_ids(&c.evidence.task_ids)? {
            if !crate::task_is_initial_read(&task) {
                continue;
            }
            let Some(result) = task.result.as_ref() else {
                continue;
            };
            let field = |name: &str| result["fields"][name].as_str().map(str::to_owned);
            out.push(SeenInvoice {
                task_id: task.id,
                document: result["document"].as_str().map(str::to_owned),
                date: field("date"),
                amount: field("total"),
                read_at: task.updated_at,
            });
        }
        Ok(out)
    }

    /// Datos para precargar una regla desde una tarea en revisión. Solo lectura; la regla se
    /// crea después con `create_rule`, siempre inactiva.
    pub fn rule_seed_for_task(&self, task_id: i64) -> AppResult<RuleSeed> {
        let task = self
            .store
            .task_by_id(task_id)?
            .ok_or_else(|| AppError::Invalid(format!("tarea {task_id} inexistente")))?;
        let result = task
            .result
            .as_ref()
            .filter(|r| r.is_object())
            .ok_or_else(|| AppError::Invalid("la tarea no tiene resultado".into()))?;
        if task.kind == INVOICE {
            let supplier = result["fields"]["supplier"]
                .as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty() && !s.chars().any(char::is_control))
                .ok_or_else(|| {
                    AppError::Invalid("la factura no indica proveedor: no hay qué precargar".into())
                })?;
            let total = result["fields"]["total"]
                .as_str()
                .and_then(|t| parse_amount("total", t).ok())
                .map(|t| t.to_string());
            // El límite está en euros: solo se precarga con el euro comprobado, sin otra
            // moneda mencionada, sin importe dudoso y con un total legible.
            let skipped = |code: &str, currencies: Vec<String>| {
                Some(LimitSkipped {
                    code: code.into(),
                    currencies,
                })
            };
            let limit_skipped = match foreign_currencies(result) {
                None => skipped("currency_not_checked", Vec::new()),
                Some(list) if !list.is_empty() => skipped("foreign_currency", list),
                Some(_) if !shows_euro(result) => skipped("currency_not_shown", Vec::new()),
                Some(_) if result["amount_review"].as_str().is_some() => {
                    skipped("amount_review", Vec::new())
                }
                Some(_) if total.is_none() => skipped("no_total", Vec::new()),
                Some(_) => None,
            };
            let mut conditions = json!({ "supplier": supplier });
            if let (None, Some(total)) = (&limit_skipped, &total) {
                conditions["max_total"] = json!(total);
            }
            return Ok(RuleSeed {
                action_kind: REGISTER_INVOICE.into(),
                name: sanitize_name(&format!("Facturas de {supplier}")),
                limit_from_this_invoice: limit_skipped.is_none(),
                limit_skipped,
                conditions,
            });
        }
        if task.kind != EMAIL {
            return Err(AppError::Invalid("esta tarea no admite una regla".into()));
        }
        let category = result["category"].as_str().unwrap_or("");
        let sender = result["message"]["from"]
            .as_str()
            .map(|s| s.trim().to_ascii_lowercase())
            .filter(|s| is_email_address(s))
            .ok_or_else(|| {
                AppError::Invalid(
                    "el correo no indica un remitente que se pueda usar en una regla".into(),
                )
            })?;
        let conditions = json!({ "category": category, "sender": sender });
        check_rule(SEND_DRAFT, &conditions, false)?;
        Ok(RuleSeed {
            action_kind: SEND_DRAFT.into(),
            name: sanitize_name(&format!("Respuestas a {sender} ({category})")),
            limit_from_this_invoice: false,
            limit_skipped: None,
            conditions,
        })
    }
}

pub(crate) fn stale_message() -> String {
    "La sugerencia ya no está vigente: los datos cambiaron o ya se resolvió. Actualice la lista."
        .into()
}

/// Rellena proveedor, importe y limpieza de una aprobación de factura a partir del resultado
/// de su tarea. Una tarea cuya decisión tomó una regla no es una decisión del usuario.
fn fill_invoice(signal: &mut Signal, result: &Value) {
    // Una tarea cuya decisión tomó una regla no es una decisión del usuario. Un rechazo sí
    // lo es aunque el resultado conserve la autorización de la regla (la regla lo intentó, el
    // conector falló y `retry` la devolvió a revisión) o su intento: `reject` solo actúa desde
    // `AwaitingReview`.
    if signal.kind != "rejected"
        && (result["authorization"]["type"] == "rule" || result["rule_attempt"]["type"] == "rule")
    {
        return;
    }
    let supplier = result["fields"]["supplier"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    // «Limpia» = una regla de ese proveedor, sin límite, habría podido registrarla: sin
    // incidencias, con el euro visible, sin importe dudoso ni factura separada.
    let is_clean = |result: &Value| {
        supplier.is_some_and(|supplier| {
            let probe = Rule {
                id: "(limpia)".into(),
                version: 1,
                action_kind: REGISTER_INVOICE.into(),
                conditions: json!({ "supplier": supplier }),
                active: true,
                created_at: 0,
            };
            matches!(
                evaluate_result(&[probe], &HashSet::new(), REGISTER_INVOICE, result).decision,
                RuleDecision::Automate { .. }
            )
        })
    };
    // Un posible duplicado es neutral (ni suma ni corta) solo si, sin el duplicado, la factura
    // habría sido limpia: moneda correcta, sin otras incidencias, sin campos ilegibles y con
    // la edición conocida. Una factura en USD o sin euro visible que además sea duplicada
    // corta igual que sin el duplicado.
    let issues = result["issues"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    if !issues.is_empty()
        && issues.iter().all(|i| i == "possible_duplicate")
        && signal.edit_state != "unknown"
    {
        let mut without = result.clone();
        without["issues"] = json!([]);
        signal.neutral = is_clean(&without);
    }
    if !matches!(signal.kind.as_str(), "approved" | "observed") {
        return;
    }
    signal.display = supplier.map(str::to_owned);
    signal.amount = result["fields"]["total"]
        .as_str()
        .and_then(|t| parse_amount("total", t).ok())
        .map(|t| t.to_string());
    signal.clean = is_clean(result);
}

fn decision_of_row(row: SuggestionRow) -> Option<Decision> {
    let kind = match row.status.as_str() {
        "discarded" => DecisionKind::Discarded,
        "accepted" => DecisionKind::Accepted,
        _ => return None,
    };
    // Datos ilegibles: la sugerencia queda oculta (nunca reaparece por un fallo de lectura).
    let baseline = row.evidence["baseline"]
        .as_u64()
        .and_then(|v| u32::try_from(v).ok())
        .unwrap_or(u32::MAX / 2);
    let watermark = row.evidence["watermark"].as_i64().unwrap_or(i64::MAX);
    Some(Decision {
        fingerprint: row.fingerprint,
        kind,
        baseline,
        watermark,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inv(id: i64, task: i64, supplier: &str, amount: &str) -> Signal {
        Signal {
            id,
            at: 1_000 + id,
            kind: "approved".into(),
            action_kind: REGISTER_INVOICE.into(),
            subject_key: supplier.trim().to_ascii_lowercase(),
            edit_state: "unedited".into(),
            task_id: task,
            source_key: Some(format!("hash-{task}")),
            state: "valid".into(),
            display: Some(supplier.into()),
            amount: Some(amount.into()),
            clean: true,
            neutral: false,
        }
    }

    fn reply(id: i64, task: i64, sender: &str, edit: &str) -> Signal {
        Signal {
            id,
            at: 1_000 + id,
            kind: "approved".into(),
            action_kind: SEND_DRAFT.into(),
            subject_key: format!("solicitud_factura|{sender}"),
            edit_state: edit.into(),
            task_id: task,
            source_key: None,
            state: "valid".into(),
            display: None,
            amount: None,
            clean: false,
            neutral: false,
        }
    }

    fn run(signals: &[Signal]) -> Suggestions {
        suggest(signals, &[], &Marks::default(), &[], Thresholds::default())
    }

    #[test]
    fn three_clean_invoices_propose_the_largest_amount_as_limit() {
        let s = run(&[
            inv(1, 1, "Alfa SL", "800.00"),
            inv(2, 2, "Alfa SL", "1200.00"),
            inv(3, 3, "Alfa SL", "950.50"),
        ]);
        assert_eq!(s.candidates.len(), 1);
        let c = &s.candidates[0];
        assert_eq!(
            c.conditions,
            json!({ "supplier": "Alfa SL", "max_total": "1200.00" })
        );
        assert_eq!(c.evidence.limit.as_deref(), Some("1200.00"));
        assert_eq!(c.evidence.amounts, ["800.00", "1200.00", "950.50"]);
        assert_eq!((c.evidence.count, c.evidence.threshold), (3, 3));
    }

    #[test]
    fn two_invoices_only_report_what_is_missing() {
        let s = run(&[
            inv(1, 1, "Alfa SL", "800.00"),
            inv(2, 2, "Alfa SL", "900.00"),
        ]);
        assert!(s.candidates.is_empty());
        assert_eq!(s.progress.len(), 1);
        assert_eq!((s.progress[0].count, s.progress[0].missing), (2, 1));
        assert_eq!(s.observed, 2);
    }

    #[test]
    fn pending_and_void_signals_never_count() {
        for state in ["pending", "void"] {
            let mut signals = vec![
                inv(1, 1, "Alfa SL", "800.00"),
                inv(2, 2, "Alfa SL", "900.00"),
                inv(3, 3, "Alfa SL", "950.00"),
            ];
            signals[2].state = state.into();
            let s = run(&signals);
            assert!(s.candidates.is_empty(), "{state}");
            assert_eq!(s.observed, 2, "{state}");
        }
    }

    #[test]
    fn repeated_hashes_and_retries_of_one_task_are_one_observation() {
        let mut same_hash = inv(2, 2, "Alfa SL", "900.00");
        same_hash.source_key = Some("hash-1".into());
        let retry = inv(3, 1, "Alfa SL", "800.00");
        let s = run(&[inv(1, 1, "Alfa SL", "800.00"), same_hash, retry]);
        assert!(s.candidates.is_empty());
        assert_eq!(s.progress[0].count, 1, "una sola observación real");
    }

    #[test]
    fn an_unclean_invoice_or_an_unreadable_total_is_not_evidence_and_restarts_the_series() {
        let mut dirty = inv(3, 3, "Alfa SL", "900.00");
        dirty.clean = false;
        let s = run(&[
            inv(1, 1, "Alfa SL", "800.00"),
            inv(2, 2, "Alfa SL", "800.00"),
            dirty,
            inv(4, 4, "Alfa SL", "800.00"),
        ]);
        assert!(s.candidates.is_empty());
        let mut no_amount = inv(3, 3, "Alfa SL", "1.00");
        no_amount.amount = None;
        let s = run(&[
            inv(1, 1, "Alfa SL", "800.00"),
            inv(2, 2, "Alfa SL", "800.00"),
            no_amount,
        ]);
        assert!(s.candidates.is_empty());
    }

    #[test]
    fn replies_need_five_unedited_approvals_from_one_exact_address() {
        let four: Vec<Signal> = (1..=4)
            .map(|i| reply(i, i, "pedidos@ejemplo.test", "unedited"))
            .collect();
        assert!(run(&four).candidates.is_empty());
        let mut five = four.clone();
        five.push(reply(5, 5, "pedidos@ejemplo.test", "unedited"));
        let s = run(&five);
        assert_eq!(s.candidates.len(), 1);
        assert_eq!(
            s.candidates[0].conditions,
            json!({ "category": "solicitud_factura", "sender": "pedidos@ejemplo.test" })
        );
        // Direcciones distintas no se suman: nunca «cualquier remitente».
        let mixed: Vec<Signal> = (1..=5)
            .map(|i| reply(i, i, &format!("a{i}@ejemplo.test"), "unedited"))
            .collect();
        assert!(run(&mixed).candidates.is_empty());
        let nobody: Vec<Signal> = (1..=5).map(|i| reply(i, i, "", "unedited")).collect();
        let s = run(&nobody);
        assert!(s.candidates.is_empty());
        assert_eq!(s.hidden.invalid, 1, "un remitente vacío no se sugiere");
    }

    #[test]
    fn an_edit_restarts_the_reply_series_and_unknown_is_not_unedited() {
        let sender = "pedidos@ejemplo.test";
        // Cinco aprobaciones sin cambios, pero con una edición entre la segunda y la tercera.
        let mut signals: Vec<Signal> = (1..=5)
            .map(|i| reply(i * 10, i, sender, "unedited"))
            .collect();
        let mut edit = reply(25, 20, sender, "edited");
        edit.kind = "edited".into();
        signals.push(edit);
        assert!(
            run(&signals).candidates.is_empty(),
            "la edición reinicia la serie"
        );
        // Con cinco o más aprobaciones tras la edición (3 + 5) vuelve a sostenerse.
        signals.extend((6..=10).map(|i| reply(i * 10 + 100, i, sender, "unedited")));
        let s = run(&signals);
        assert_eq!(s.candidates.len(), 1);
        assert_eq!(s.candidates[0].evidence.earlier_breaks, 1);
        assert_eq!(
            s.candidates[0].evidence.count, 8,
            "las 3 de antes de las 5 nuevas, tras la edición"
        );

        // Una edición y su aprobación posterior (misma tarea) son un solo corte.
        let mut approved_edited = reply(26, 20, sender, "edited");
        approved_edited.kind = "approved".into();
        signals.push(approved_edited);
        assert_eq!(run(&signals).candidates[0].evidence.earlier_breaks, 1);

        let mut unknown: Vec<Signal> = (1..=5).map(|i| reply(i, i, sender, "unedited")).collect();
        unknown[4].edit_state = "unknown".into();
        assert!(
            run(&unknown).candidates.is_empty(),
            "sin clasificar no cuenta"
        );
    }

    #[test]
    fn a_rejection_restarts_the_series() {
        let mut signals: Vec<Signal> = (1..=3).map(|i| inv(i, i, "Alfa SL", "100.00")).collect();
        let mut rejected = inv(4, 4, "Alfa SL", "100.00");
        rejected.kind = "rejected".into();
        signals.push(rejected);
        assert!(run(&signals).candidates.is_empty());
    }

    #[test]
    fn same_data_gives_same_suggestions_in_the_same_order() {
        let mut signals = Vec::new();
        let mut id = 0;
        for (name, n) in [("Zeta SL", 3), ("Alfa SL", 4), ("Beta SL", 3)] {
            for _ in 0..n {
                id += 1;
                signals.push(inv(id, id, name, "100.00"));
            }
        }
        let a = run(&signals);
        let mut shuffled = signals.clone();
        shuffled.reverse();
        let b = run(&shuffled);
        assert_eq!(a, b);
        let names: Vec<&str> = a.candidates.iter().map(|c| c.subject.as_str()).collect();
        assert_eq!(names, ["Alfa SL", "Beta SL", "Zeta SL"]);
    }

    fn rule(id: &str, active: bool, conditions: Value, kind: &str) -> Rule {
        Rule {
            id: id.into(),
            version: 1,
            action_kind: kind.into(),
            conditions,
            active,
            created_at: 0,
        }
    }

    #[test]
    fn an_existing_rule_with_the_same_conditions_hides_the_candidate_and_a_wider_one_explains() {
        let signals: Vec<Signal> = (1..=3).map(|i| inv(i, i, "Alfa SL", "500.00")).collect();
        for active in [true, false] {
            let rules = [rule(
                "alfa",
                active,
                json!({ "supplier": "alfa sl", "max_total": "500.00" }),
                REGISTER_INVOICE,
            )];
            let s = suggest(
                &signals,
                &rules,
                &Marks::default(),
                &[],
                Thresholds::default(),
            );
            assert!(s.candidates.is_empty(), "activa={active}");
            assert_eq!(s.hidden.covered, 1);
            assert!(s.progress.is_empty());
        }
        // Una regla activa más amplia ya automatiza eso: activar la candidata mandaría a
        // revisión lo que hoy se automatiza. Se oculta, y se dice.
        let wider = |active: bool| {
            [rule(
                "alfa-libre",
                active,
                json!({ "supplier": "Alfa SL" }),
                REGISTER_INVOICE,
            )]
        };
        let s = suggest(
            &signals,
            &wider(true),
            &Marks::default(),
            &[],
            Thresholds::default(),
        );
        assert!(s.candidates.is_empty());
        assert_eq!(s.hidden.covered, 1);
        // Inactiva, sí se muestra, con su aviso.
        let s = suggest(
            &signals,
            &wider(false),
            &Marks::default(),
            &[],
            Thresholds::default(),
        );
        assert_eq!(s.candidates.len(), 1);
        let overlap = &s.candidates[0].overlaps;
        assert_eq!(overlap.len(), 1);
        assert_eq!(overlap[0].kind, FindingKind::Overlap);
        assert!(
            !overlap[0].active_conflict,
            "la otra regla está inactiva: solo chocarían si se activan las dos"
        );
        assert!(overlap[0].rules.iter().any(|r| r.id == SUGGESTED));
        // Una regla activa más estrecha sí deja ver la candidata, con el choque explicado.
        let narrower = [rule(
            "alfa-100",
            true,
            json!({ "supplier": "Alfa SL", "max_total": "100.00" }),
            REGISTER_INVOICE,
        )];
        let s = suggest(
            &signals,
            &narrower,
            &Marks::default(),
            &[],
            Thresholds::default(),
        );
        assert_eq!(s.candidates.len(), 1);
        assert!(s.candidates[0].overlaps[0].active_conflict);
        // Una regla archivada no cuenta.
        let mut marks = Marks::default();
        marks.archived.insert("alfa-libre".into());
        let s = suggest(&signals, &wider(true), &marks, &[], Thresholds::default());
        assert!(s.candidates[0].overlaps.is_empty());
    }

    #[test]
    fn a_discarded_suggestion_needs_twice_the_evidence_gathered_since() {
        let mut signals: Vec<Signal> = (1..=3).map(|i| inv(i, i, "Alfa SL", "500.00")).collect();
        let fp = run(&signals).candidates[0].fingerprint.clone();
        let decision = Decision {
            fingerprint: fp,
            kind: DecisionKind::Discarded,
            baseline: 3,
            watermark: 3,
        };
        let go = |signals: &[Signal]| {
            suggest(
                signals,
                &[],
                &Marks::default(),
                std::slice::from_ref(&decision),
                Thresholds::default(),
            )
        };
        assert!(go(&signals).candidates.is_empty());
        assert_eq!(go(&signals).hidden.discarded, 1);
        for i in 4..=8 {
            signals.push(inv(i, i, "Alfa SL", "500.00"));
        }
        assert!(
            go(&signals).candidates.is_empty(),
            "5 nuevas < el doble de 3"
        );
        signals.push(inv(9, 9, "Alfa SL", "500.00"));
        let s = go(&signals);
        assert_eq!(s.candidates.len(), 1);
        assert_eq!(
            s.candidates[0].evidence.reappeared,
            Some(Reappearance {
                at_discard: 3,
                new_since: 6,
                needed_new: 6
            })
        );
    }

    #[test]
    fn thresholds_below_the_minimum_are_raised_inside_suggest_too() {
        let one = [inv(1, 1, "Alfa SL", "10.00")];
        let s = suggest(
            &one,
            &[],
            &Marks::default(),
            &[],
            Thresholds {
                invoice: 0,
                reply: 1,
                learned_reply: 0,
            },
        );
        assert!(
            s.candidates.is_empty(),
            "una sola observación nunca sugiere"
        );
        assert_eq!(s.thresholds.invoice, MIN_THRESHOLD);
    }

    #[test]
    fn a_supplier_with_control_characters_is_not_suggested() {
        let signals: Vec<Signal> = (1..=3).map(|i| inv(i, i, "Alfa\nSL", "10.00")).collect();
        let s = run(&signals);
        assert!(s.candidates.is_empty());
        assert_eq!(s.hidden.invalid, 1);
    }

    #[test]
    fn the_fingerprint_does_not_depend_on_the_proposed_limit() {
        let a = run(&(1..=3)
            .map(|i| inv(i, i, "Alfa SL", "100.00"))
            .collect::<Vec<_>>());
        let b = run(&(1..=3)
            .map(|i| inv(i, i, "Alfa SL", "999.00"))
            .collect::<Vec<_>>());
        assert_eq!(a.candidates[0].fingerprint, b.candidates[0].fingerprint);
        assert_ne!(a.candidates[0].conditions, b.candidates[0].conditions);
    }

    #[test]
    fn a_second_signal_of_the_same_task_also_restarts_the_series() {
        let sender = "pedidos@ejemplo.test";
        for second in ["edited", "rejected"] {
            // T1 se edita (señal 10), se aprueban T2 a T6 (11 a 15) y luego T1 vuelve (16).
            let mut signals = vec![{
                let mut e = reply(10, 1, sender, "edited");
                e.kind = "edited".into();
                e
            }];
            signals.extend((2..=6).map(|t| reply(9 + t, t, sender, "unedited")));
            assert_eq!(
                run(&signals).candidates.len(),
                1,
                "{second}: cinco tras la edición"
            );
            let mut again = reply(
                16,
                1,
                sender,
                if second == "edited" {
                    "edited"
                } else {
                    "unedited"
                },
            );
            again.kind = if second == "edited" {
                "approved"
            } else {
                "rejected"
            }
            .into();
            signals.push(again);
            let s = run(&signals);
            assert!(
                s.candidates.is_empty(),
                "{second}: la segunda señal de T1 reinicia"
            );
            assert_eq!(s.counted, 0);
        }
        // Y sigue contando un solo corte por tarea.
        let mut signals = vec![
            {
                let mut e = reply(1, 1, sender, "edited");
                e.kind = "edited".into();
                e
            },
            reply(2, 1, sender, "edited"),
        ];
        signals.extend((2..=6).map(|t| reply(1 + t, t, sender, "unedited")));
        assert_eq!(run(&signals).candidates[0].evidence.earlier_breaks, 1);
    }

    #[test]
    fn a_possible_duplicate_is_neutral_it_neither_counts_nor_cuts() {
        let mut dup = inv(3, 30, "Alfa SL", "500.00");
        dup.neutral = true;
        let mut rejected_dup = inv(5, 31, "Alfa SL", "500.00");
        rejected_dup.kind = "rejected".into();
        rejected_dup.neutral = true;
        let s = run(&[
            inv(1, 1, "Alfa SL", "500.00"),
            inv(2, 2, "Alfa SL", "500.00"),
            dup,
            rejected_dup,
        ]);
        assert!(s.candidates.is_empty(), "el duplicado no suma");
        assert_eq!(s.progress[0].count, 2, "ni corta");
    }

    #[test]
    fn the_other_category_is_never_suggested_for_replies() {
        let signals: Vec<Signal> = (1..=5)
            .map(|i| {
                let mut s = reply(i, i, "pedidos@ejemplo.test", "unedited");
                s.subject_key = "otros|pedidos@ejemplo.test".into();
                s
            })
            .collect();
        let s = run(&signals);
        assert!(s.candidates.is_empty());
        assert_eq!(s.hidden.not_suggested, 1);
        assert_eq!(
            s.counted, 5,
            "las observaciones constan aunque no se sugiera"
        );
    }

    #[test]
    fn a_pipe_inside_the_address_does_not_split_the_key_wrongly() {
        let signals: Vec<Signal> = (1..=5)
            .map(|i| reply(i, i, "x|jefe@empresa.test", "unedited"))
            .collect();
        let s = run(&signals);
        assert_eq!(s.candidates.len(), 1);
        assert_eq!(
            s.candidates[0].conditions,
            json!({ "category": "solicitud_factura", "sender": "x|jefe@empresa.test" })
        );
        assert_eq!(
            s.candidates[0].category.as_deref(),
            Some("solicitud_factura")
        );
    }

    #[test]
    fn the_limit_comes_with_range_second_largest_and_an_outlier_flag() {
        let s = run(&[
            inv(1, 1, "Alfa SL", "100.00"),
            inv(2, 2, "Alfa SL", "120.00"),
            inv(3, 3, "Alfa SL", "360.00"),
        ]);
        let e = &s.candidates[0].evidence;
        assert_eq!(e.limit.as_deref(), Some("360.00"));
        assert_eq!(e.amount_min.as_deref(), Some("100.00"));
        assert_eq!(e.amount_second.as_deref(), Some("120.00"));
        assert!(e.limit_outlier, "360 es el triple de 120");
        let s = run(&[
            inv(1, 1, "Alfa SL", "100.00"),
            inv(2, 2, "Alfa SL", "120.00"),
            inv(3, 3, "Alfa SL", "359.99"),
        ]);
        assert!(!s.candidates[0].evidence.limit_outlier);
    }

    #[test]
    fn the_shown_fingerprint_follows_the_displayed_conditions() {
        let a = run(&(1..=3)
            .map(|i| inv(i, i, "Alfa SL", "100.00"))
            .collect::<Vec<_>>());
        let b = run(&(1..=3)
            .map(|i| inv(i, i, "Alfa SL", "999.00"))
            .collect::<Vec<_>>());
        assert_eq!(a.candidates[0].fingerprint, b.candidates[0].fingerprint);
        assert_ne!(a.candidates[0].shown, b.candidates[0].shown);
    }

    fn seen(id: i64, task: i64, supplier: &str, amount: &str) -> Signal {
        Signal {
            kind: "observed".into(),
            ..inv(id, task, supplier, amount)
        }
    }

    #[test]
    fn invoices_only_seen_need_five_and_say_none_was_approved() {
        let four: Vec<Signal> = (1..=4).map(|i| seen(i, i, "Alfa SL", "100.00")).collect();
        let s = run(&four);
        assert!(s.candidates.is_empty());
        let p = &s.progress[0];
        assert_eq!(
            (p.count, p.approved, p.seen, p.needed, p.missing),
            (4, 0, 4, 5, 1)
        );
        assert_eq!((s.observed, s.seen, s.decisions), (4, 4, 0));
        let five: Vec<Signal> = (1..=5).map(|i| seen(i, i, "Alfa SL", "100.00")).collect();
        let c = &run(&five).candidates[0];
        assert_eq!(
            (
                c.evidence.count,
                c.evidence.approved,
                c.evidence.seen,
                c.evidence.threshold
            ),
            (5, 0, 5, 5)
        );
        // Subir el umbral sube también este mínimo; bajarlo nunca lo baja de 5.
        let low = suggest(
            &four,
            &[],
            &Marks::default(),
            &[],
            Thresholds {
                invoice: 2,
                reply: 5,
                learned_reply: 3,
            },
        );
        assert!(low.candidates.is_empty());
        let high = suggest(
            &five,
            &[],
            &Marks::default(),
            &[],
            Thresholds {
                invoice: 8,
                reply: 5,
                learned_reply: 3,
            },
        );
        assert!(high.candidates.is_empty());
    }

    #[test]
    fn a_seen_signal_never_counts_for_replies_and_enough_approvals_use_the_usual_threshold() {
        let mut replies: Vec<Signal> = (1..=8)
            .map(|i| Signal {
                kind: "observed".into(),
                ..reply(i, i, "cliente@ejemplo.test", "unedited")
            })
            .collect();
        let s = run(&replies);
        assert!(s.candidates.is_empty() && s.progress.is_empty());
        replies.clear();
        // Tres aprobadas bastan aunque haya vistas; con dos aprobadas y dos vistas faltan.
        let mut mixed: Vec<Signal> = (1..=3).map(|i| inv(i, i, "Alfa SL", "100.00")).collect();
        mixed.push(seen(4, 4, "Alfa SL", "150.00"));
        let c = &run(&mixed).candidates[0];
        assert_eq!(
            (
                c.evidence.count,
                c.evidence.approved,
                c.evidence.seen,
                c.evidence.threshold
            ),
            (4, 3, 1, 3)
        );
        // El límite sale solo de lo aprobado (100): la factura vista de 150 no lo sube.
        assert_eq!(c.evidence.limit.as_deref(), Some("100.00"));
        assert!(!c.evidence.needs_limit);
        let few = vec![
            inv(1, 1, "Beta SL", "100.00"),
            inv(2, 2, "Beta SL", "100.00"),
            seen(3, 3, "Beta SL", "100.00"),
            seen(4, 4, "Beta SL", "100.00"),
        ];
        assert!(run(&few).candidates.is_empty());
    }

    #[test]
    fn seen_only_invoices_propose_no_limit_and_need_the_person_to_write_one() {
        let five: Vec<Signal> = (1..=5).map(|i| seen(i, i, "Alfa SL", "9000.00")).collect();
        let c = &run(&five).candidates[0];
        assert!(c.evidence.needs_limit);
        assert_eq!(c.evidence.limit, None);
        assert!(c.evidence.amounts.is_empty() && !c.evidence.limit_outlier);
        assert_eq!(c.conditions, json!({ "supplier": "Alfa SL" }));
    }

    #[test]
    fn an_unclean_seen_invoice_never_erases_approvals_and_the_order_does_not_matter() {
        let approved: Vec<Signal> = (1..=3)
            .map(|i| inv(10 + i, i, "Alfa SL", "100.00"))
            .collect();
        let bad = Signal {
            clean: false,
            ..seen(20, 20, "Alfa SL", "100.00")
        };
        // La vista con incidencias llega antes o después de las aprobaciones: el resultado es el mismo.
        for order in [[0usize, 1], [1, 0]] {
            let mut signals = Vec::new();
            let blocks = [approved.clone(), vec![bad.clone()]];
            for i in order {
                signals.extend(blocks[i].clone());
            }
            for (n, sig) in signals.iter_mut().enumerate() {
                sig.id = i64::try_from(n).unwrap() + 1;
            }
            let c = &run(&signals).candidates[0];
            assert_eq!(
                (c.evidence.approved, c.evidence.seen, c.evidence.count),
                (3, 0, 3)
            );
            assert_eq!(c.evidence.earlier_breaks, 0);
        }
    }

    #[test]
    fn one_bad_seen_invoice_is_reported_as_hidden_and_the_approvals_stay() {
        // Cinco vistas y una con incidencias, quizá de un tercero: ninguna vista cuenta y se dice.
        let mut signals: Vec<Signal> = (1..=5).map(|i| seen(i, i, "Alfa SL", "100.00")).collect();
        signals.push(Signal {
            clean: false,
            ..seen(6, 6, "Alfa SL", "100.00")
        });
        let s = run(&signals);
        assert!(s.candidates.is_empty() && s.progress.is_empty());
        assert_eq!((s.counted, s.hidden.seen_invalidated), (0, 1));
        // Con aprobaciones, estas se conservan y lo invalidado también se cuenta.
        let mut with_approvals: Vec<Signal> = (1..=3)
            .map(|i| inv(10 + i, 10 + i, "Beta SL", "100.00"))
            .collect();
        with_approvals.push(Signal {
            clean: false,
            ..seen(20, 20, "Beta SL", "100.00")
        });
        let s = run(&with_approvals);
        assert_eq!(s.candidates[0].evidence.approved, 3);
        assert_eq!(s.hidden.seen_invalidated, 1);
        // Sin ninguna vista inválida no se dice nada.
        assert_eq!(
            run(&[seen(1, 1, "Gamma SL", "100.00")])
                .hidden
                .seen_invalidated,
            0
        );
    }

    #[test]
    fn a_document_the_person_already_decided_is_not_only_seen() {
        let mut signals: Vec<Signal> = (1..=5).map(|i| seen(i, i, "Alfa SL", "100.00")).collect();
        // La persona aprobó una copia del primer documento: ya no es «solo vista».
        signals.push(Signal {
            source_key: Some("hash-1".into()),
            ..inv(30, 30, "Alfa SL", "100.00")
        });
        let s = run(&signals);
        let e = &s.candidates[0].evidence;
        assert_eq!((e.approved, e.seen, e.count), (1, 4, 5));
    }

    #[test]
    fn an_unclean_seen_invoice_cuts_the_series_like_an_unclean_approved_one() {
        let mut signals: Vec<Signal> = (1..=5).map(|i| seen(i, i, "Alfa SL", "100.00")).collect();
        signals.push(Signal {
            clean: false,
            ..seen(6, 6, "Alfa SL", "100.00")
        });
        let s = run(&signals);
        assert!(s.candidates.is_empty());
        assert_eq!(s.counted, 0);
    }

    fn row(kind: &str, edit_state: &str) -> Signal {
        let mut s = inv(1, 1, "Alfa SL", "100.00");
        s.kind = kind.into();
        s.edit_state = edit_state.into();
        s.clean = false;
        s.neutral = false;
        s.display = None;
        s.amount = None;
        s
    }

    fn invoice_result(issues: Value, evidence: Value) -> Value {
        json!({
            "version": 1,
            "fields": { "supplier": "Alfa SL", "total": "121.00" },
            "issues": issues,
            "unreadable": [],
            "currency_evidence": evidence,
        })
    }

    #[test]
    fn a_duplicate_is_neutral_only_if_it_would_otherwise_be_clean() {
        let euro = json!({ "euro": true, "foreign": [] });
        let dup = json!(["possible_duplicate"]);
        let mut ok = row("approved", "unedited");
        fill_invoice(&mut ok, &invoice_result(dup.clone(), euro.clone()));
        assert!(ok.neutral, "duplicado en euros: neutral");
        let mut rejected = row("rejected", "unedited");
        fill_invoice(&mut rejected, &invoice_result(dup.clone(), euro.clone()));
        assert!(rejected.neutral);
        // En USD, sin euro visible, con otra incidencia o sin clasificar: corta como siempre.
        for (label, issues, evidence, edit) in [
            (
                "usd",
                dup.clone(),
                json!({ "euro": false, "foreign": ["USD"] }),
                "unedited",
            ),
            (
                "sin euro",
                dup.clone(),
                json!({ "euro": false, "foreign": [] }),
                "unedited",
            ),
            (
                "otra incidencia",
                json!(["possible_duplicate", "total_mismatch"]),
                euro.clone(),
                "unedited",
            ),
            ("sin clasificar", dup.clone(), euro.clone(), "unknown"),
        ] {
            for kind in ["approved", "rejected"] {
                let mut s = row(kind, edit);
                fill_invoice(&mut s, &invoice_result(issues.clone(), evidence.clone()));
                assert!(!s.neutral, "{label} {kind}");
            }
        }
    }

    #[test]
    fn zero_or_negative_totals_are_not_evidence_and_never_give_a_limit() {
        for bad in ["0.00", "-121.00"] {
            let signals = [
                inv(1, 1, "Alfa SL", "100.00"),
                inv(2, 2, "Alfa SL", "100.00"),
                inv(3, 3, "Alfa SL", bad),
            ];
            let s = run(&signals);
            assert!(s.candidates.is_empty(), "{bad}");
            assert_eq!(
                s.counted, 0,
                "{bad}: corta la serie, no queda nada que cuente"
            );
        }
        // Sin el importe malo, tres positivas sí.
        let s = run(&(1..=3)
            .map(|i| inv(i, i, "Alfa SL", "0.01"))
            .collect::<Vec<_>>());
        assert_eq!(s.candidates[0].evidence.limit.as_deref(), Some("0.01"));
        assert!(!is_positive_amount("0.00") && !is_positive_amount("-5.00"));
    }

    #[test]
    fn below_the_threshold_covered_discarded_and_accepted_series_are_counted_as_hidden() {
        let signals = [
            inv(1, 1, "Alfa SL", "100.00"),
            inv(2, 2, "Beta SL", "100.00"),
            inv(3, 3, "Gamma SL", "100.00"),
            inv(4, 4, "Delta SL", "100.00"),
        ];
        let rules = [rule(
            "alfa",
            false,
            json!({ "supplier": "Alfa SL" }),
            REGISTER_INVOICE,
        )];
        let decide = |name: &str, kind| Decision {
            fingerprint: fingerprint_of(&[REGISTER_INVOICE, "supplier", name]),
            kind,
            baseline: 3,
            watermark: 0,
        };
        let decisions = [
            decide("beta sl", DecisionKind::Discarded),
            decide("gamma sl", DecisionKind::Accepted),
        ];
        let s = suggest(
            &signals,
            &rules,
            &Marks::default(),
            &decisions,
            Thresholds::default(),
        );
        assert_eq!(
            (s.hidden.covered, s.hidden.discarded, s.hidden.accepted),
            (1, 1, 1)
        );
        assert_eq!(s.progress.len(), 1, "solo Delta promete «faltan»");
        assert_eq!(s.progress[0].subject, "Delta SL");
        assert_eq!(s.counted, 4);
        // Una regla inactiva más amplia no cubre: «Delta» con una regla inactiva sin límite
        // (igual) sí, pero una más estrecha activa o inactiva no.
        let narrower = [rule(
            "delta-100",
            true,
            json!({ "supplier": "Delta SL", "max_total": "100.00" }),
            REGISTER_INVOICE,
        )];
        let s = suggest(
            &signals[3..],
            &narrower,
            &Marks::default(),
            &[],
            Thresholds::default(),
        );
        assert_eq!(s.progress.len(), 1);
    }

    // ---- Evidencia `learned` (D172) ----

    fn learned(id: i64, key: &str, message: &str) -> Signal {
        Signal {
            id,
            at: 1_000 + id,
            kind: "learned".into(),
            action_kind: SEND_DRAFT.into(),
            subject_key: key.into(),
            edit_state: "unknown".into(),
            task_id: 0,
            source_key: Some(format!("learned|account:a|{message}")),
            state: "valid".into(),
            display: None,
            amount: None,
            clean: false,
            neutral: false,
        }
    }

    fn decision(id: i64, task: i64, key: &str, kind: &str, edit: &str) -> Signal {
        Signal {
            kind: kind.into(),
            subject_key: key.into(),
            edit_state: edit.into(),
            ..reply(id, task, "x", edit)
        }
    }

    const ANA: &str = "consulta_pago|ana@alfa.test";

    #[test]
    fn three_learned_pairs_of_one_address_and_category_suggest_a_rule_with_learned_origin() {
        let two = [learned(1, ANA, "m1"), learned(2, ANA, "m2")];
        let s = run(&two);
        assert!(s.candidates.is_empty());
        assert_eq!((s.learned, s.observed, s.decisions), (2, 0, 0));
        assert_eq!(s.progress.len(), 1);
        assert_eq!(
            (
                s.progress[0].origin,
                s.progress[0].needed,
                s.progress[0].missing
            ),
            ("learned", 3, 1)
        );
        let s = run(&[two[0].clone(), two[1].clone(), learned(3, ANA, "m3")]);
        assert_eq!(s.candidates.len(), 1);
        let c = &s.candidates[0];
        assert_eq!(
            c.conditions,
            json!({ "category": "consulta_pago", "sender": "ana@alfa.test" })
        );
        let e = &c.evidence;
        assert_eq!(
            (e.origin, e.learned, e.count, e.threshold),
            ("learned", 3, 3, 3)
        );
        assert_eq!((e.approved, e.seen, e.addresses), (0, 0, 1));
        assert!(e.task_ids.is_empty(), "no hay tareas que citar");
        assert!(
            s.progress.is_empty(),
            "una tarjeta no promete además «faltan N»"
        );
    }

    #[test]
    fn a_repeated_source_counts_once() {
        let s = run(&[
            learned(1, ANA, "m1"),
            learned(2, ANA, "m1"),
            learned(3, ANA, "m2"),
        ]);
        assert!(s.candidates.is_empty());
        assert_eq!(s.progress[0].count, 2);
    }

    #[test]
    fn learned_threshold_defaults_to_three_and_never_goes_below_two() {
        let pairs = [learned(1, ANA, "m1"), learned(2, ANA, "m2")];
        let at = |learned_reply| {
            suggest(
                &pairs,
                &[],
                &Marks::default(),
                &[],
                Thresholds {
                    learned_reply,
                    ..Thresholds::default()
                },
            )
        };
        assert_eq!(Thresholds::default().learned_reply, 3);
        assert!(run(&pairs).candidates.is_empty());
        assert_eq!(at(2).candidates.len(), 1);
        let s = at(0);
        assert_eq!(s.thresholds.learned_reply, MIN_THRESHOLD);
        assert_eq!(s.candidates.len(), 1);
        assert!(run(&pairs[..1]).candidates.is_empty());
        assert!(at(1).candidates[0].evidence.count >= 2);
    }

    #[test]
    fn the_other_category_is_never_suggested_from_learned_pairs() {
        let key = "otros|ana@alfa.test";
        let s = run(&[
            learned(1, key, "a"),
            learned(2, key, "b"),
            learned(3, key, "c"),
        ]);
        assert!(s.candidates.is_empty() && s.progress.is_empty());
    }

    #[test]
    fn an_edit_or_a_rejection_after_the_pairs_breaks_the_learned_series() {
        for (kind, edit) in [
            ("edited", "edited"),
            ("rejected", "unedited"),
            ("approved", "unknown"),
        ] {
            let mut signals = vec![
                learned(1, ANA, "m1"),
                learned(2, ANA, "m2"),
                learned(3, ANA, "m3"),
                decision(4, 40, ANA, kind, edit),
            ];
            let s = run(&signals);
            assert!(s.candidates.is_empty(), "{kind}");
            assert_eq!(s.progress.len(), 0, "{kind}: la serie quedó vacía");
            // Tres pares nuevos después del corte vuelven a sostenerla.
            signals.extend([
                learned(5, ANA, "m5"),
                learned(6, ANA, "m6"),
                learned(7, ANA, "m7"),
            ]);
            let s = run(&signals);
            assert_eq!(s.candidates.len(), 1, "{kind}");
            assert_eq!(s.candidates[0].evidence.earlier_breaks, 1, "{kind}");
        }
    }

    #[test]
    fn learned_pairs_never_mix_with_approved_ones() {
        let approved = |id: i64| {
            let mut s = reply(id, id, "ana@alfa.test", "unedited");
            s.subject_key = ANA.into();
            s
        };
        // Cuatro aprobaciones sin edición (umbral 5) y tres pares aprendidos: la tarjeta es solo de
        // lo aprendido, y las aprobaciones no suman a sus recuentos ni a la inversa.
        let mut signals: Vec<Signal> = (1..=4).map(approved).collect();
        signals.extend([
            learned(10, ANA, "a"),
            learned(11, ANA, "b"),
            learned(12, ANA, "c"),
        ]);
        let s = run(&signals);
        assert_eq!(s.candidates.len(), 1);
        let e = &s.candidates[0].evidence;
        assert_eq!(
            (e.origin, e.count, e.learned, e.approved),
            ("learned", 3, 3, 0)
        );
        assert_eq!((s.observed, s.decisions, s.learned), (4, 4, 3));
        // Con la quinta aprobación sale la de aprobaciones: una sola tarjeta para esas condiciones.
        signals.push(approved(13));
        let s = run(&signals);
        assert_eq!(s.candidates.len(), 1);
        let e = &s.candidates[0].evidence;
        assert_eq!(
            (e.origin, e.count, e.learned, e.approved),
            ("approved", 5, 0, 5)
        );
    }

    #[test]
    fn an_unedited_approval_does_not_cut_the_learned_series() {
        let mut approved = reply(4, 4, "ana@alfa.test", "unedited");
        approved.subject_key = ANA.into();
        let s = run(&[
            learned(1, ANA, "m1"),
            learned(2, ANA, "m2"),
            approved,
            learned(5, ANA, "m3"),
        ]);
        assert_eq!(s.candidates[0].evidence.learned, 3);
    }

    fn pairs_of(addresses: &[&str], category: &str) -> Vec<Signal> {
        addresses
            .iter()
            .enumerate()
            .map(|(i, a)| learned(i as i64 + 1, &format!("{category}|{a}"), &format!("m{i}")))
            .collect()
    }

    fn domain_candidates(s: &Suggestions) -> Vec<&Candidate> {
        s.candidates
            .iter()
            .filter(|c| c.conditions.get("sender_domain").is_some())
            .collect()
    }

    #[test]
    fn a_domain_rule_needs_two_addresses_and_three_pairs_of_a_non_public_domain() {
        // Tres pares de dos direcciones: sale la de dominio.
        let s = run(&pairs_of(
            &["ana@alfa.test", "luis@alfa.test", "ana@alfa.test"],
            "consulta_pago",
        ));
        let domain = domain_candidates(&s);
        assert_eq!(domain.len(), 1);
        assert_eq!(
            domain[0].conditions,
            json!({ "category": "consulta_pago", "sender_domain": "alfa.test" })
        );
        let e = &domain[0].evidence;
        assert_eq!((e.origin, e.addresses, e.count), ("learned", 2, 3));
        // Dos pares de dos direcciones no bastan.
        let s = run(&pairs_of(
            &["ana@alfa.test", "luis@alfa.test"],
            "consulta_pago",
        ));
        assert!(s.candidates.is_empty());
        // Tres pares de una sola dirección: solo la regla de dirección exacta.
        let s = run(&pairs_of(
            &["ana@alfa.test", "ana@alfa.test", "ana@alfa.test"],
            "consulta_pago",
        ));
        assert!(domain_candidates(&s).is_empty());
        assert_eq!(s.candidates.len(), 1);
        // El umbral por dominio nunca baja de tres pares aunque `learned_reply` sea 2.
        let s = suggest(
            &pairs_of(&["ana@alfa.test", "luis@alfa.test"], "consulta_pago"),
            &[],
            &Marks::default(),
            &[],
            Thresholds {
                learned_reply: 2,
                ..Thresholds::default()
            },
        );
        assert!(domain_candidates(&s).is_empty());
    }

    #[test]
    fn a_public_domain_gets_no_domain_rule_but_an_exact_address_rule_is_fine() {
        let s = run(&pairs_of(
            &[
                "ana@gmail.com",
                "luis@gmail.com",
                "marta@gmail.com",
                "ana@gmail.com",
            ],
            "consulta_pago",
        ));
        assert!(domain_candidates(&s).is_empty());
        let s = run(&pairs_of(
            &["ana@gmail.com", "ana@gmail.com", "ana@gmail.com"],
            "consulta_pago",
        ));
        assert_eq!(s.candidates.len(), 1);
        assert_eq!(
            s.candidates[0].conditions,
            json!({ "category": "consulta_pago", "sender": "ana@gmail.com" })
        );
    }

    #[test]
    fn a_rejection_of_any_address_of_the_domain_breaks_the_domain_series() {
        let mut signals = pairs_of(
            &["ana@alfa.test", "luis@alfa.test", "marta@alfa.test"],
            "consulta_pago",
        );
        assert_eq!(domain_candidates(&run(&signals)).len(), 1);
        signals.push(decision(
            9,
            90,
            "consulta_pago|luis@alfa.test",
            "rejected",
            "unedited",
        ));
        assert!(run(&signals).candidates.is_empty());
    }

    #[test]
    fn learned_never_produces_an_any_sender_rule() {
        let s = run(&pairs_of(
            &["ana@alfa.test", "luis@beta.test", "marta@gamma.test"],
            "consulta_pago",
        ));
        assert!(s.candidates.is_empty());
    }
}
