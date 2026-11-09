//! Capa con modelo de las sugerencias de reglas (D134). El código calcula las candidatas y su
//! evidencia (`suggest`, D132); aquí un modelo de IA solo las **afina**: las ordena, propone
//! ocultar las poco útiles, pregunta por proveedores que quizá son el mismo, propone otro límite
//! y comenta. Nunca crea candidatas, activa reglas ni pone cifras de evidencia. Si el modelo no
//! está o falla, las sugerencias por código siguen igual, con un aviso.
//!
//! El trabajo se divide como el de las tareas: `begin_refinement` prepara la petición con datos
//! agregados (D102), **reserva** el conjunto de datos antes de enviar nada y toma el turno,
//! `RefineJob::run` llama al proveedor sin acceso a `App`, y `finish_refinement` valida la
//! respuesta y la guarda. El afinado automático está desactivado hasta que la persona lo
//! consiente, ligado al proveedor y modelo confirmados.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use nestadmin_ai::refine::{
    self as ai, is_format_char, personal_data_in, RefineCandidate, Rejection,
};
use nestadmin_ai::{GenerateRequest, GenerateResult, Provider, ProviderKind};
use nestadmin_documents::parse_amount;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::rules::is_public_mail_domain;
use crate::suggest::{fingerprint_of, is_positive_amount, Candidate, Suggestions};
use crate::{
    check_rule, generate_with_fallback, AiWait, App, AppError, AppResult, LocalFallback,
    ProviderInfo, REGISTER_INVOICE,
};

/// Señales válidas nuevas desde el último intento que hacen falta para que la versión final
/// vuelva a afinar sola (D134).
pub const MIN_NEW_SIGNALS: u32 = 5;
/// Intentos como mucho por conjunto de datos en el modo automático, contando el primero, y
/// esperas (ms) tras el primer y el segundo fallo transitorio: 15 minutos y 1 hora (D134). Una
/// clave rechazada no se reintenta sola.
pub const MAX_AUTO_ATTEMPTS: u32 = 3;
pub const AUTO_BACKOFF_MS: [i64; 2] = [15 * 60_000, 60 * 60_000];
/// Versión del formato guardado.
const STORED_VERSION: u32 = 2;
const MAX_NAME_CHARS: usize = 80;
/// Ajuste (`rule_settings`) con el consentimiento del afinado automático.
const CONSENT_KEY: &str = "refine_auto_consent";

/// Cómo se afinan las sugerencias según el canal (D134).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RefineMode {
    /// Versiones de prueba (feature `lectura-bajo-demanda`): solo con el botón «Afinar con IA»,
    /// tras mostrar a qué proveedor se envía y qué datos.
    OnDemand,
    /// Versión final: por lotes, cuando se acumulan señales válidas nuevas **y la persona ha
    /// dado su consentimiento**; desactivado por defecto.
    Automatic,
}

impl RefineMode {
    /// El modo del canal con el que se compiló. La lectura inicial (E5) usa el mismo.
    pub const fn of_build() -> Self {
        if cfg!(feature = "lectura-bajo-demanda") {
            Self::OnDemand
        } else {
            Self::Automatic
        }
    }
}

/// Quién pide el afinado.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefineTrigger {
    /// La persona, con el botón y tras ver la vista previa (en los dos modos).
    Demand,
    /// El lote automático (solo en la versión final y con consentimiento).
    Auto,
}

/// Lo que la persona vio en la vista previa. `begin_refinement_with` rechaza el envío si el proveedor,
/// el modelo, el servidor o lo que se enviaría ya no coinciden, como `grant_refine_consent`.
#[derive(Debug, Clone, Deserialize)]
pub struct RefineExpectation {
    pub provider: String,
    pub model: String,
    #[serde(default)]
    pub host: String,
    pub input_fp: String,
}

/// Candidata que el modelo propone ocultar. No se borra: sigue visible y recuperable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HiddenByModel {
    pub fingerprint: String,
    /// `low_value`, `likely_one_off` o `unclear_name`.
    pub reason: String,
}

/// Límite propuesto por el modelo que pasa `check_rule` y la simulación **al leerlo**: las
/// cifras de `considered` y `would_automate` se recalculan cada vez, no se guardan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LimitProposal {
    pub fingerprint: String,
    pub max_total: String,
    /// Huella de las condiciones con este límite: es la que exige aceptar la propuesta.
    pub shown: String,
    pub considered: usize,
    pub would_automate: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct StoredLimit {
    fingerprint: String,
    max_total: String,
}

/// Parte de la respuesta que se descartó, con su motivo (sin contenido).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dropped {
    /// `limit`.
    pub kind: String,
    pub fingerprint: String,
    pub code: String,
}

/// Último resultado válido. Cada elemento vale solo para la candidata (huella de condiciones
/// `fingerprint`) cuyo estado de entonces (`cands`: huella → resumen de lo enviado y de `shown`)
/// coincide con el de ahora: aceptar o descartar otra candidata no lo caduca.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Good {
    at: i64,
    provider: Option<String>,
    model: Option<String>,
    fallback: Option<Value>,
    cands: BTreeMap<String, String>,
    order: Vec<String>,
    hide: Vec<HiddenByModel>,
    groups: Vec<Vec<String>>,
    limits: Vec<StoredLimit>,
    dropped: Vec<Dropped>,
    comment: String,
    /// Motivo por el que solo se descartó el comentario (cifras en letra o afirmaciones).
    #[serde(default)]
    comment_dropped: Option<String>,
}

/// Último intento. Se guarda **antes** de llamar al proveedor (`started`): un fallo al guardar el
/// resultado, un cierre durante la llamada o un pánico no repiten el envío.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Attempt {
    input_fp: String,
    /// `started`, `ok` o `failed`.
    state: String,
    at: i64,
    signal_watermark: i64,
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    detail: Option<String>,
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    fallback: Option<Value>,
    /// Intentos fallidos de este conjunto de datos.
    #[serde(default)]
    failures: u32,
    /// Cuándo volverá a intentarlo sola (solo fallos transitorios, modo automático).
    #[serde(default)]
    next_retry_at: Option<i64>,
}

/// Lo que se guarda en `rule_suggestions.model_json`: solo huellas, códigos y el comentario.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Record {
    v: u32,
    #[serde(default)]
    ok: Option<Good>,
    #[serde(default)]
    attempt: Option<Attempt>,
}

/// Consentimiento del afinado automático.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConsentView {
    /// Solo hace falta en la versión final con un proveedor externo.
    pub required: bool,
    /// `not_needed`, `missing`, `granted` o `stale` (el proveedor o el modelo cambiaron).
    pub state: &'static str,
    /// Proveedor, modelo y servidor (host) a los que se consintió, o a los que se consentiría ahora
    /// con `missing`. El consentimiento está ligado a los tres.
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
    at: i64,
}

/// Estado del afinado para la pestaña Sugeridas.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RefinementView {
    pub mode: RefineMode,
    /// `refined`, `stale` (hay un resultado anterior que ya no corresponde a ninguna candidata),
    /// `running` o `not_refined`.
    pub status: &'static str,
    /// Por qué no está afinado o por qué falló el último intento: `no_candidates`, `on_demand`,
    /// `auto_consent_missing`, `auto_consent_stale`, `waiting_signals`, `pending_auto`,
    /// `interrupted`, una espera de IA (`needs_choice`, `needs_key`, `needs_consent`,
    /// `invalid_key`) o el fallo (`unavailable`, `timeout`, `rate_limited`, `unauthorized`,
    /// `invalid_response`, `rejected`).
    pub reason: Option<String>,
    /// Con `rejected`, el motivo de la validación.
    pub detail: Option<String>,
    /// Cuándo se aplicó el resultado que se muestra, o cuándo falló el intento de estos datos.
    pub ran_at: Option<i64>,
    /// Cuándo se obtuvo el último resultado válido, aunque ya no esté vigente.
    pub last_ok_at: Option<i64>,
    pub provider: Option<String>,
    pub model: Option<String>,
    /// `{from, reason}` si respondió la IA local de respaldo (D101).
    pub fallback: Option<Value>,
    /// Huellas de candidatas en orden de utilidad, solo las que siguen como estaban.
    pub order: Vec<String>,
    pub hide: Vec<HiddenByModel>,
    /// Grupos de candidatas cuyo proveedor quizá es el mismo: solo una pregunta.
    pub groups: Vec<Vec<String>>,
    pub limits: Vec<LimitProposal>,
    pub dropped: Vec<Dropped>,
    /// Texto plano; la interfaz lo rotula «Comentario de la IA» y añade su nota fija. Solo se
    /// muestra mientras **todas** las candidatas del afinado sigan como estaban.
    pub comment: Option<String>,
    /// El comentario se descartó (y el resto de la respuesta se conservó) por este motivo.
    pub comment_dropped: Option<String>,
    /// Candidatas actuales, cuántas se envían (tope de 30) y a cuántas se aplica el resultado.
    pub candidates: usize,
    pub sent: usize,
    pub covered: usize,
    /// Señales válidas nuevas desde el último intento y las que hacen falta.
    pub new_signals: u32,
    pub min_new_signals: u32,
    /// Cuándo volverá a intentarlo sola tras un fallo transitorio, y si ya no lo hará.
    pub retry_at: Option<i64>,
    pub gave_up: bool,
    pub failures: u32,
    pub max_attempts: u32,
    pub consent: ConsentView,
}

/// Lo que se va a enviar y a quién, para mostrarlo antes de pedir el afinado o de consentir.
#[derive(Debug, Clone, Serialize)]
pub struct RefinePreview {
    pub mode: RefineMode,
    pub provider: ProviderInfo,
    /// Servidor (host) real al que saldrían los datos; `None` con la IA local.
    pub host: Option<String>,
    /// Huella de lo que se enviaría: `refine_suggestions` rechaza si ya no coincide.
    pub input_fp: String,
    /// El proveedor es externo: los datos salen del equipo.
    pub external: bool,
    /// Durante el periodo de D101 tras un `unavailable`, la petición va **primero** a la IA
    /// local y solo si esta falla, al proveedor elegido.
    pub via_local_first: bool,
    /// Con un externo, si un fallo se repetiría con la IA local si está preparada (D101).
    pub local_fallback: bool,
    /// Si hay una espera de IA, nada se enviaría.
    pub blocked: Option<AiWait>,
    pub blocked_message: Option<String>,
    pub consent: ConsentView,
    /// Exactamente lo que viaja, candidata a candidata (los mismos campos que la petición).
    pub candidates: Vec<RefineCandidate>,
    /// Candidatas calculadas; se envían como mucho 30.
    pub candidates_total: usize,
    pub discarded_by_user: u32,
    /// Qué no viaja nunca.
    pub never_sent: Vec<&'static str>,
}

/// Candidatas que se envían, con su identificador opaco, y las huellas que las ligan al
/// resultado.
struct Batch {
    candidates: Vec<Candidate>,
    ids: Vec<String>,
    payload: Vec<RefineCandidate>,
    /// Resumen de cada candidata (lo enviado, su huella y `shown`): lo que el resultado exige que
    /// siga igual para aplicarse a ella.
    cfps: Vec<String>,
    total: usize,
    discarded: u32,
    content: String,
    /// Huella de todo lo enviado: un conjunto de datos ya enviado no se vuelve a enviar.
    input_fp: String,
}

/// El turno de afinar: solo uno a la vez. Se libera al soltarlo, también si el trabajo entra en
/// pánico o la tarea se abandona.
struct Turn(Arc<AtomicBool>);

impl Turn {
    fn take(flag: &Arc<AtomicBool>) -> Option<Self> {
        flag.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .ok()
            .map(|_| Self(Arc::clone(flag)))
    }
}

impl Drop for Turn {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// Tarea del hilo de trabajo: la petición lista para el proveedor.
pub struct RefineJob {
    provider: Arc<dyn Provider>,
    fallback: Option<LocalFallback>,
    fallback_until: Arc<Mutex<Option<Instant>>>,
    request: GenerateRequest,
    batch: Batch,
    watermark: i64,
    asked: (String, String),
    turn: Turn,
}

/// Respuesta del proveedor (o su fallo) lista para `finish_refinement`.
pub struct RefineCompleted {
    batch: Batch,
    watermark: i64,
    asked: (String, String),
    result: Result<GenerateResult, nestadmin_core::TaskError>,
    unauthorized: bool,
    fallback: Option<Value>,
    /// El proveedor con el que se pidió: si ya no es el activo (la clave o el proveedor cambiaron
    /// mientras tanto), un `unauthorized` no vuelve a poner la espera de clave.
    used: Arc<dyn Provider>,
    turn: Turn,
}

impl RefineJob {
    /// Llama al proveedor elegido (con el respaldo local de D101 si procede). Nunca a otro
    /// proveedor externo. Un fallo se devuelve tal cual.
    pub fn run(self) -> RefineCompleted {
        let attempt = generate_with_fallback(
            &*self.provider,
            self.fallback.as_ref(),
            &self.fallback_until,
            &self.request,
        );
        RefineCompleted {
            batch: self.batch,
            watermark: self.watermark,
            asked: self.asked,
            result: attempt.result,
            unauthorized: attempt.unauthorized,
            fallback: attempt.fallback,
            used: self.provider,
            turn: self.turn,
        }
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_millis()).ok())
        .unwrap_or(0)
}

/// Un nombre que viaja al modelo: sin caracteres de control ni `<` `>` (que podrían imitar los
/// delimitadores) y recortado. Sigue siendo un dato no fiable.
fn clean_name(raw: &str) -> String {
    raw.chars()
        .filter(|c| !c.is_control() && !is_format_char(*c) && !matches!(c, '<' | '>'))
        .take(MAX_NAME_CHARS)
        .collect::<String>()
        .trim()
        .to_owned()
}

/// Nombre del proveedor para la petición, o el motivo por el que no se envía: si lleva un
/// correo, un NIF, un IBAN o un teléfono, el nombre **no sale** (B2). Un nombre sin nada de eso
/// puede aun así identificar a una persona; el aviso de la vista previa lo dice.
fn supplier_for_model(name: &str) -> (Option<String>, Option<&'static str>) {
    // Se examina lo que saldría (ya sin `<`, `>` ni caracteres invisibles, que cambian la lectura
    // de un teléfono) y también el original.
    let clean = clean_name(name);
    if let Some(code) = personal_data_in(&clean).or_else(|| personal_data_in(name)) {
        return (None, Some(code));
    }
    ((!clean.is_empty()).then_some(clean), None)
}

/// Dominio del remitente para la petición. Un dominio de correo público (gmail.com…) identifica
/// a una persona y no aporta nada: no sale. Un dominio propio sí sale (es lo que permite
/// reconocer a la empresa); puede identificar a una persona o a un negocio pequeño y el aviso
/// lo dice.
fn domain_for_model(conditions: &Value) -> (Option<String>, Option<&'static str>) {
    let Some(domain) = conditions["sender"]
        .as_str()
        .and_then(|s| s.rsplit_once('@'))
        .map(|(_, domain)| clean_name(&domain.to_ascii_lowercase()))
        .filter(|d| !d.is_empty())
    else {
        return (None, None);
    };
    if is_public_mail_domain(&domain) {
        return (None, Some("public_mail_domain"));
    }
    (Some(domain), None)
}

fn hex(digest: &[u8]) -> String {
    digest.iter().take(10).map(|b| format!("{b:02x}")).collect()
}

fn build_batch(suggestions: &Suggestions) -> Batch {
    let candidates: Vec<Candidate> = suggestions
        .candidates
        .iter()
        .take(ai::MAX_REFINE_CANDIDATES)
        .cloned()
        .collect();
    let ids: Vec<String> = (1..=candidates.len()).map(|n| format!("c{n}")).collect();
    let payload: Vec<RefineCandidate> = candidates
        .iter()
        .zip(&ids)
        .map(|(c, id)| {
            let invoice = c.action_kind == REGISTER_INVOICE;
            let (supplier, domain, withheld) = if invoice {
                let (name, why) = supplier_for_model(&c.subject);
                (name, None, why)
            } else {
                let (domain, why) = domain_for_model(&c.conditions);
                (None, domain, why)
            };
            RefineCandidate {
                id: id.clone(),
                kind: if invoice { "invoice" } else { "reply" },
                supplier,
                category: c.category.clone(),
                sender_domain: domain,
                count: c.evidence.count,
                seen: c.evidence.seen,
                earlier_breaks: c.evidence.earlier_breaks,
                amount_min: invoice.then(|| c.evidence.amount_min.clone()).flatten(),
                amount_max: invoice.then(|| c.evidence.limit.clone()).flatten(),
                discarded_before: c.evidence.reappeared.is_some(),
                withheld,
            }
        })
        .collect();
    let cfps: Vec<String> = candidates
        .iter()
        .zip(&payload)
        .map(|(c, p)| {
            let mut anonymous = p.clone();
            anonymous.id.clear();
            let digest = Sha256::digest(
                format!(
                    "{}\0{}\0{}",
                    serde_json::to_string(&anonymous).unwrap_or_default(),
                    c.fingerprint,
                    c.shown
                )
                .as_bytes(),
            );
            hex(&digest)
        })
        .collect();
    let discarded = suggestions.hidden.discarded;
    let content = ai::content(&payload, discarded);
    let mut digest = Sha256::new();
    digest.update(content.as_bytes());
    for (c, cfp) in candidates.iter().zip(&cfps) {
        digest.update(b"\0");
        digest.update(c.fingerprint.as_bytes());
        digest.update(cfp.as_bytes());
    }
    Batch {
        total: suggestions.candidates.len(),
        candidates,
        ids,
        payload,
        cfps,
        discarded,
        content,
        input_fp: hex(&digest.finalize()),
    }
}

fn wait_code(wait: AiWait) -> &'static str {
    match wait {
        AiWait::NeedsChoice => "needs_choice",
        AiWait::NeedsKey => "needs_key",
        AiWait::NeedsConsent => "needs_consent",
        AiWait::InvalidKey => "invalid_key",
    }
}

/// Fallos que pueden pasar solos (red, cuota, servicio): se reintentan con espera y tope.
fn is_transient(code: &str) -> bool {
    matches!(code, "unavailable" | "timeout" | "rate_limited")
}

fn describe(asked: &(String, String), fallback: &Option<Value>) -> String {
    let backup = match fallback {
        Some(note) => format!(
            "; respondió la IA local de respaldo (desde {})",
            note["from"].as_str().unwrap_or("?")
        ),
        None => String::new(),
    };
    format!("proveedor {}, modelo {}{backup}", asked.0, asked.1)
}

impl App {
    /// Modo de afinado de este canal.
    pub fn refine_mode(&self) -> RefineMode {
        self.refine_mode
    }

    /// Fija el modo (las pruebas comprueban los dos; la aplicación usa el del canal).
    #[doc(hidden)]
    pub fn set_refine_mode(&mut self, mode: RefineMode) {
        self.refine_mode = mode;
    }

    /// Servidor real al que saldrían los datos (vacío con la IA local o si el proveedor no lo dice).
    pub(crate) fn host(&self) -> String {
        self.provider.destination().unwrap_or_default()
    }

    pub(crate) fn asked(&self) -> (String, String) {
        (
            self.provider.kind().to_string(),
            self.provider.model().to_owned(),
        )
    }

    fn record(&self) -> AppResult<Record> {
        Ok(self
            .store
            .rule_refinement()?
            .and_then(|json| serde_json::from_value::<Record>(json).ok())
            .filter(|r| r.v == STORED_VERSION)
            .unwrap_or(Record {
                v: STORED_VERSION,
                ok: None,
                attempt: None,
            }))
    }

    fn save(&mut self, record: &Record, outcome: &str, detail: &str) -> AppResult<()> {
        let json = serde_json::to_value(record).map_err(nestadmin_core::CoreError::from)?;
        self.store.save_refinement(&json, outcome, detail)?;
        Ok(())
    }

    /// Señales válidas guardadas después de `watermark` y el mayor id de señal de cualquier
    /// estado.
    fn signal_counts(&self, watermark: i64) -> AppResult<(u32, i64)> {
        let rows = self.store.rule_signals()?;
        let new = rows
            .iter()
            .filter(|r| r.state == "valid" && r.id > watermark)
            .count();
        let max = rows.iter().map(|r| r.id).max().unwrap_or(0);
        Ok((u32::try_from(new).unwrap_or(u32::MAX), max))
    }

    /// Consentimiento del afinado automático frente al proveedor y modelo de ahora.
    pub fn refine_consent(&self) -> AppResult<ConsentView> {
        let (provider, model) = self.asked();
        let host = self.host();
        let external = self.provider.kind() != ProviderKind::Ollama;
        let view =
            |state: &'static str, p: String, m: String, h: String, at: Option<i64>| ConsentView {
                required: true,
                state,
                provider: Some(p),
                model: Some(m),
                host: Some(h).filter(|h| !h.is_empty()),
                at,
            };
        if self.refine_mode != RefineMode::Automatic || !external {
            return Ok(ConsentView {
                required: false,
                state: "not_needed",
                provider: None,
                model: None,
                host: None,
                at: None,
            });
        }
        let stored = self
            .store
            .rule_setting(CONSENT_KEY)?
            .and_then(|json| serde_json::from_str::<StoredConsent>(&json).ok());
        Ok(match stored {
            None => view("missing", provider, model, host, None),
            // Proveedor, modelo **y servidor**: el mismo modelo en otro servidor es otro destino.
            Some(c) if c.provider == provider && c.model == model && c.host == host => {
                view("granted", c.provider, c.model, c.host, Some(c.at))
            }
            Some(c) => view("stale", c.provider, c.model, c.host, Some(c.at)),
        })
    }

    /// Si el proveedor, el modelo o el servidor activos ya no son los del consentimiento guardado,
    /// lo retira (con constancia en el historial) en lugar de dejarlo dormido: así no revive si
    /// después se vuelve al destino anterior. Se llama al cambiar de proveedor.
    pub(crate) fn drop_consent_if_destination_changed(&mut self) {
        let Ok(Some(json)) = self.store.rule_setting(CONSENT_KEY) else {
            return;
        };
        let (provider, model) = self.asked();
        let same = serde_json::from_str::<StoredConsent>(&json)
            .is_ok_and(|c| c.provider == provider && c.model == model && c.host == self.host());
        if !same {
            let _ = self.store.delete_rule_setting(
                CONSENT_KEY,
                "consentimiento del afinado automático de sugerencias retirado: cambió el proveedor, el modelo o el servidor",
            );
        }
    }

    /// La persona activa el afinado automático para el proveedor y el modelo que vio (`provider`
    /// y `model` deben coincidir con los de ahora, para que consienta a lo que se le mostró).
    /// Queda en el historial. Solo en la versión final y con un proveedor externo. Si el
    /// proveedor o el modelo cambian, deja de valer.
    pub fn grant_refine_consent(
        &mut self,
        provider: &str,
        model: &str,
        host: &str,
    ) -> AppResult<ConsentView> {
        if self.refine_mode != RefineMode::Automatic {
            return Err(AppError::Invalid(
                "En esta versión de prueba la IA solo afina cuando usted lo pide: no hay nada que activar."
                    .into(),
            ));
        }
        if self.provider.kind() == ProviderKind::Ollama {
            return Err(AppError::Invalid(
                "Con la IA local los datos no salen del equipo: no hace falta consentimiento."
                    .into(),
            ));
        }
        if self.asked() != (provider.to_owned(), model.to_owned()) || self.host() != host {
            return Err(AppError::Invalid(
                "El proveedor, el modelo o el servidor han cambiado desde que lo vio: revise el aviso de nuevo antes de aceptar."
                    .into(),
            ));
        }
        let at = now_ms();
        self.store.set_rule_setting(
            CONSENT_KEY,
            &json!({ "provider": provider, "model": model, "host": host, "at": at }).to_string(),
            &format!(
                "consentimiento del afinado automático de sugerencias concedido: proveedor {provider}, modelo {model}, servidor {host}"
            ),
        )?;
        self.auto_skip = None;
        self.refine_consent()
    }

    /// Retira el consentimiento: el afinado automático deja de enviar nada. Queda en el
    /// historial. Retirarlo dos veces no cambia nada.
    pub fn withdraw_refine_consent(&mut self) -> AppResult<ConsentView> {
        self.store.delete_rule_setting(
            CONSENT_KEY,
            "consentimiento del afinado automático de sugerencias retirado",
        )?;
        self.refine_consent()
    }

    /// Qué se enviaría y a quién. Solo lectura: no llama al proveedor.
    pub fn refinement_preview(&self) -> AppResult<RefinePreview> {
        let batch = build_batch(&self.rule_suggestions()?);
        let provider = self.provider();
        let external = provider.provider != ProviderKind::Ollama;
        Ok(RefinePreview {
            mode: self.refine_mode,
            external,
            via_local_first: external && self.fallback.is_some() && self.fallback_period_active(),
            local_fallback: external && self.fallback.is_some(),
            blocked: self.ai_wait,
            blocked_message: self.ai_wait.map(|w| w.message().to_owned()),
            consent: self.refine_consent()?,
            provider,
            host: self.provider.destination(),
            input_fp: batch.input_fp.clone(),
            candidates_total: batch.total,
            candidates: batch.payload,
            discarded_by_user: batch.discarded,
            never_sent: vec![
                "cuerpos de correo",
                "adjuntos y documentos",
                "asuntos",
                "direcciones de correo completas",
                "identificadores de tarea y huellas",
            ],
        })
    }

    /// Estado del afinado frente a las candidatas de ahora. Solo lectura. Cada parte del
    /// resultado guardado se aplica solo a la candidata a la que correspondía (misma huella,
    /// mismos datos enviados y mismo `shown`); aceptar o descartar otra no la caduca.
    pub fn refinement_view(&self) -> AppResult<RefinementView> {
        self.refinement_view_for(&self.rule_suggestions()?)
    }

    pub(crate) fn refinement_view_for(
        &self,
        suggestions: &Suggestions,
    ) -> AppResult<RefinementView> {
        let batch = build_batch(suggestions);
        let record = self.record()?;
        let (new_signals, _) =
            self.signal_counts(record.attempt.as_ref().map_or(0, |a| a.signal_watermark))?;
        let mut view = RefinementView {
            mode: self.refine_mode,
            status: "not_refined",
            reason: None,
            detail: None,
            ran_at: None,
            last_ok_at: record.ok.as_ref().map(|g| g.at),
            provider: None,
            model: None,
            fallback: None,
            order: Vec::new(),
            hide: Vec::new(),
            groups: Vec::new(),
            limits: Vec::new(),
            dropped: Vec::new(),
            comment: None,
            comment_dropped: None,
            candidates: batch.total,
            sent: batch.candidates.len(),
            covered: 0,
            new_signals,
            min_new_signals: MIN_NEW_SIGNALS,
            retry_at: None,
            gave_up: false,
            failures: 0,
            max_attempts: MAX_AUTO_ATTEMPTS,
            consent: self.refine_consent()?,
        };
        if self.refining.load(Ordering::SeqCst) {
            view.status = "running";
            return Ok(view);
        }
        if batch.candidates.is_empty() {
            view.reason = Some("no_candidates".into());
            return Ok(view);
        }
        // Lo guardado que corresponde a una candidata de ahora.
        let current: HashMap<&str, (&Candidate, &str)> = batch
            .candidates
            .iter()
            .zip(&batch.cfps)
            .map(|(c, cfp)| (c.fingerprint.as_str(), (c, cfp.as_str())))
            .collect();
        if let Some(good) = &record.ok {
            let valid = |fp: &str| {
                current
                    .get(fp)
                    .is_some_and(|(_, cfp)| good.cands.get(fp).is_some_and(|stored| stored == cfp))
            };
            view.covered = batch
                .candidates
                .iter()
                .filter(|c| valid(&c.fingerprint))
                .count();
            if view.covered > 0 {
                view.status = "refined";
                view.ran_at = Some(good.at);
                view.provider = good.provider.clone();
                view.model = good.model.clone();
                view.fallback = good.fallback.clone();
                view.order = good.order.iter().filter(|f| valid(f)).cloned().collect();
                view.hide = good
                    .hide
                    .iter()
                    .filter(|h| valid(&h.fingerprint))
                    .cloned()
                    .collect();
                view.groups = good
                    .groups
                    .iter()
                    .filter(|g| g.iter().all(|f| valid(f)))
                    .cloned()
                    .collect();
                view.dropped = good
                    .dropped
                    .iter()
                    .filter(|d| valid(&d.fingerprint))
                    .cloned()
                    .collect();
                // La simulación y `check_rule` se repiten con los datos de ahora.
                for stored in good.limits.iter().filter(|l| valid(&l.fingerprint)) {
                    let candidate = current[stored.fingerprint.as_str()].0;
                    match self.check_limit(candidate, &stored.max_total) {
                        Ok(proposal) => view.limits.push(proposal),
                        Err(code) => view.dropped.push(Dropped {
                            kind: "limit".into(),
                            fingerprint: stored.fingerprint.clone(),
                            code: code.into(),
                        }),
                    }
                }
                // El comentario habla del conjunto: solo vale si todas las candidatas con las que
                // se pidió siguen como estaban.
                if good.cands.keys().all(|fp| valid(fp)) {
                    view.comment = Some(good.comment.clone());
                    view.comment_dropped = good.comment_dropped.clone();
                }
            } else {
                view.status = "stale";
            }
        }
        // El último intento de estos datos, para explicar por qué no hay uno nuevo.
        let attempt = record
            .attempt
            .as_ref()
            .filter(|a| a.input_fp == batch.input_fp);
        match attempt {
            Some(a) if a.state == "failed" && self.key_was_fixed(a) => {}
            Some(a) if a.state == "failed" => {
                view.failures = a.failures;
                view.ran_at = view.ran_at.or(Some(a.at));
                view.reason = a.code.clone();
                view.detail = a.detail.clone();
                view.retry_at = a.next_retry_at;
                view.gave_up = a
                    .code
                    .as_deref()
                    .is_some_and(|c| is_transient(c) && a.failures >= MAX_AUTO_ATTEMPTS);
            }
            Some(a) if a.state == "started" => {
                view.reason = Some("interrupted".into());
                view.ran_at = view.ran_at.or(Some(a.at));
            }
            Some(a) if a.state == "ok" && view.covered == batch.candidates.len() => {
                view.failures = 0;
            }
            _ => {}
        }
        if view.reason.is_none() && view.covered < batch.candidates.len() {
            view.reason = Some(self.pending_reason(&view).into());
        }
        Ok(view)
    }

    /// Un fallo `unauthorized` ya no vale si la espera de clave se levantó (la persona corrigió
    /// la clave): no se sigue mostrando el error antiguo y se puede volver a intentar.
    fn key_was_fixed(&self, attempt: &Attempt) -> bool {
        // Solo si en esta sesión se levantó la espera de clave o cambió el proveedor: tras reiniciar,
        // el error antiguo no autoriza una llamada por arranque con una clave que puede seguir revocada.
        attempt.code.as_deref() == Some("unauthorized") && self.key_fixed && self.ai_wait.is_none()
    }

    fn pending_reason(&self, view: &RefinementView) -> &'static str {
        if let Some(wait) = self.ai_wait {
            wait_code(wait)
        } else if self.refine_mode == RefineMode::OnDemand {
            "on_demand"
        } else {
            match view.consent.state {
                "missing" => "auto_consent_missing",
                "stale" => "auto_consent_stale",
                _ if view.new_signals < MIN_NEW_SIGNALS => "waiting_signals",
                _ => "pending_auto",
            }
        }
    }

    /// Prepara un afinado y toma el turno, o devuelve `None` si no hay nada que hacer: otro
    /// afinado en curso, una espera de IA (sin proveedor confirmado no sale nada), ninguna
    /// candidata, o el mismo conjunto de datos ya enviado.
    ///
    /// **Antes de devolver el trabajo se guarda el intento** (`started`): así un fallo al
    /// guardar el resultado, un cierre durante la llamada o un pánico no repiten el envío.
    ///
    /// *Automático* (solo la versión final): exige el **consentimiento** vigente para el
    /// proveedor y modelo de ahora (con un proveedor externo), al menos [`MIN_NEW_SIGNALS`]
    /// señales válidas nuevas, y un conjunto de datos no enviado antes; un fallo transitorio
    /// (red, cuota, servicio) se reintenta con espera (15 min y 1 h) hasta [`MAX_AUTO_ATTEMPTS`]
    /// intentos; una clave rechazada no se reintenta hasta que se corrija. *Con el botón*
    /// (los dos modos): un fallo del mismo conjunto se puede reintentar sin límite; un éxito no
    /// se repite. `Auto` en una versión de prueba es un error.
    pub fn begin_refinement(&mut self, trigger: RefineTrigger) -> AppResult<Option<RefineJob>> {
        self.begin_refinement_with(trigger, None)
    }

    /// Como [`App::begin_refinement`]; con `expect` (lo que la persona vio en la vista previa) se
    /// rechaza el envío si el proveedor, el modelo, el servidor o los datos ya no coinciden.
    pub fn begin_refinement_with(
        &mut self,
        trigger: RefineTrigger,
        expect: Option<&RefineExpectation>,
    ) -> AppResult<Option<RefineJob>> {
        if let Some(e) = expect {
            if self.asked() != (e.provider.clone(), e.model.clone()) || self.host() != e.host {
                return Err(AppError::Invalid(
                    "El proveedor, el modelo o el servidor han cambiado desde que vio el aviso: revíselo de nuevo antes de enviar."
                        .into(),
                ));
            }
        }
        if trigger == RefineTrigger::Auto && self.refine_mode == RefineMode::OnDemand {
            return Err(AppError::Invalid(
                "En esta versión de prueba la IA solo afina cuando usted lo pide.".into(),
            ));
        }
        if self.refining.load(Ordering::SeqCst) || self.ai_wait.is_some() {
            return Ok(None);
        }
        let record = self.record()?;
        let now = now_ms();
        let auto = trigger == RefineTrigger::Auto;
        // Sin consentimiento vigente para este proveedor y modelo, el lote no envía nada.
        if auto && matches!(self.refine_consent()?.state, "missing" | "stale") {
            return Ok(None);
        }
        // Reintento de un fallo transitorio que ya toca, o de una clave corregida.
        let mut retry_due = record.attempt.as_ref().is_some_and(|a| {
            a.state == "failed"
                && (self.key_was_fixed(a)
                    || (a.code.as_deref().is_some_and(is_transient)
                        && a.failures < MAX_AUTO_ATTEMPTS
                        && a.next_retry_at.is_some_and(|t| now >= t)))
        });
        let (new_signals, max_signal) =
            self.signal_counts(record.attempt.as_ref().map_or(0, |a| a.signal_watermark))?;
        let gate = new_signals >= MIN_NEW_SIGNALS && self.auto_skip != Some(max_signal);
        // Un reintento vencido cuyo conjunto ya cambió se descartó con este mismo id de señal: no se
        // vuelve a calcular en cada vuelta del hilo.
        if self.retry_obsolete == Some(max_signal) {
            retry_due = false;
        }
        if auto && !retry_due && !gate {
            return Ok(None);
        }
        let suggestions = self.rule_suggestions()?;
        let batch = build_batch(&suggestions);
        if let Some(e) = expect {
            if e.input_fp != batch.input_fp {
                return Err(AppError::Invalid(
                    "Lo que se iba a enviar ha cambiado desde que vio el aviso: revíselo de nuevo antes de enviar."
                        .into(),
                ));
            }
        }
        if batch.candidates.is_empty() {
            if auto {
                self.auto_skip = Some(max_signal);
            }
            return Ok(None);
        }
        let same = record
            .attempt
            .as_ref()
            .filter(|a| a.input_fp == batch.input_fp);
        match same {
            Some(a) if a.state == "ok" => {
                if auto {
                    self.auto_skip = Some(max_signal);
                }
                return Ok(None);
            }
            Some(_) if auto && !retry_due => {
                // Ya enviado (en curso, interrumpido o fallido sin reintento): una vez por
                // conjunto de datos.
                self.auto_skip = Some(max_signal);
                return Ok(None);
            }
            None if auto && !gate => {
                // El reintento pendiente era de otros datos y aún no hay señales para estos.
                self.retry_obsolete = Some(max_signal);
                return Ok(None);
            }
            _ => {}
        }
        let failures = same.map_or(0, |a| a.failures);
        let Some(turn) = Turn::take(&self.refining) else {
            return Ok(None);
        };
        let asked = self.asked();
        // Se reserva el conjunto de datos antes de llamar al proveedor.
        let mut reserved = record;
        reserved.attempt = Some(Attempt {
            input_fp: batch.input_fp.clone(),
            state: "started".into(),
            at: now,
            signal_watermark: suggestions.watermark,
            code: None,
            detail: None,
            provider: Some(asked.0.clone()),
            model: Some(asked.1.clone()),
            fallback: None,
            failures,
            next_retry_at: None,
        });
        self.save(
            &reserved,
            "started",
            &format!(
                "afinado con IA iniciado con {} candidatas: proveedor {}, modelo {}",
                batch.candidates.len(),
                asked.0,
                asked.1
            ),
        )?;
        let request = ai::request(&format!("refine-{}", batch.input_fp), &batch.content);
        Ok(Some(RefineJob {
            provider: Arc::clone(&self.provider),
            fallback: self.fallback.clone(),
            fallback_until: Arc::clone(&self.fallback_until),
            request,
            batch,
            watermark: suggestions.watermark,
            asked,
            turn,
        }))
    }

    /// Valida la respuesta y la guarda (o guarda el motivo del fallo). Si no pasa la
    /// validación se descarta entera y queda el motivo, sin contenido. Un fallo no borra el
    /// último resultado válido. Las sugerencias por código no cambian en ningún caso.
    pub fn finish_refinement(&mut self, completed: RefineCompleted) -> AppResult<RefinementView> {
        let RefineCompleted {
            batch,
            watermark,
            asked,
            result,
            unauthorized,
            fallback,
            used,
            turn,
        } = completed;
        if unauthorized && Arc::ptr_eq(&used, &self.provider) {
            // Como en las tareas (D101): la clave no sirve y nada más sale hasta corregirla. Si el
            // proveedor ya cambió mientras tanto (se corrigió la clave), el aviso es de la clave
            // antigua y no se vuelve a poner la espera.
            self.ai_wait = Some(AiWait::InvalidKey);
            self.key_fixed = false;
        }
        let mut record = self.record()?;
        let previous = record
            .attempt
            .as_ref()
            .filter(|a| a.input_fp == batch.input_fp)
            .map_or(0, |a| a.failures);
        let mut attempt = Attempt {
            input_fp: batch.input_fp.clone(),
            state: "failed".into(),
            at: now_ms(),
            signal_watermark: watermark,
            code: None,
            detail: None,
            provider: Some(asked.0.clone()),
            model: Some(asked.1.clone()),
            fallback: fallback.clone(),
            failures: previous,
            next_retry_at: None,
        };
        match result {
            Err(error) => attempt.code = Some(error.code),
            Ok(generated) => {
                attempt.provider = Some(generated.provider.to_string());
                attempt.model = Some(generated.model.clone());
                match self.interpret(&batch, &generated.output) {
                    Ok(done) => {
                        attempt.state = "ok".into();
                        attempt.failures = 0;
                        record.ok = Some(Good {
                            at: attempt.at,
                            provider: attempt.provider.clone(),
                            model: attempt.model.clone(),
                            fallback: fallback.clone(),
                            cands: batch
                                .candidates
                                .iter()
                                .zip(&batch.cfps)
                                .map(|(c, cfp)| (c.fingerprint.clone(), cfp.clone()))
                                .collect(),
                            order: done.order,
                            hide: done.hide,
                            groups: done.groups,
                            limits: done.limits,
                            dropped: done.dropped,
                            comment: done.comment,
                            comment_dropped: done.comment_dropped,
                        });
                    }
                    Err(rejection) => {
                        attempt.code = Some("rejected".into());
                        attempt.detail = Some(rejection.code().into());
                    }
                }
            }
        }
        if attempt.state == "failed" {
            attempt.failures = previous + 1;
            let transient = attempt.code.as_deref().is_some_and(is_transient);
            if transient && attempt.failures < MAX_AUTO_ATTEMPTS {
                let index = usize::try_from(attempt.failures - 1)
                    .unwrap_or(0)
                    .min(AUTO_BACKOFF_MS.len() - 1);
                attempt.next_retry_at = Some(attempt.at + AUTO_BACKOFF_MS[index]);
            }
        }
        let outcome = if attempt.state == "ok" {
            "ok"
        } else {
            "failed"
        };
        let how = describe(
            &(
                attempt.provider.clone().unwrap_or_default(),
                attempt.model.clone().unwrap_or_default(),
            ),
            &fallback,
        );
        let detail = match (&attempt.code, &attempt.detail) {
            (Some(code), Some(why)) => format!("afinado con IA descartado: {code} ({why}); {how}"),
            (Some(code), None) => format!("afinado con IA fallido: {code}; {how}"),
            _ => format!(
                "afinado con IA aplicado a {} candidatas; {how}",
                batch.candidates.len()
            ),
        };
        record.attempt = Some(attempt);
        let saved = self.save(&record, outcome, &detail);
        drop(turn);
        saved?;
        self.refinement_view()
    }

    /// Un afinado completo en el hilo actual (pruebas y llamadas sencillas). La aplicación de
    /// escritorio usa los tres pasos para no bloquear la ventana durante la llamada.
    pub fn refine_suggestions(&mut self, trigger: RefineTrigger) -> AppResult<RefinementView> {
        match self.begin_refinement(trigger)? {
            Some(job) => {
                let completed = job.run();
                self.finish_refinement(completed)
            }
            None => self.refinement_view(),
        }
    }

    /// Convierte la respuesta ya validada por esquema en lo que se guarda: solo huellas de las
    /// candidatas enviadas, los límites que pasaron las comprobaciones y los descartes.
    fn interpret(&self, batch: &Batch, output: &Value) -> Result<Interpreted, Rejection> {
        let parsed = ai::parse(output, &batch.ids)?;
        let by_id: HashMap<&str, &Candidate> = batch
            .ids
            .iter()
            .map(String::as_str)
            .zip(&batch.candidates)
            .collect();
        let fp = |id: &str| by_id[id].fingerprint.clone();
        // Preguntar si dos proveedores son el mismo solo tiene sentido entre facturas.
        if parsed
            .groups
            .iter()
            .flatten()
            .any(|id| by_id[id.as_str()].action_kind != REGISTER_INVOICE)
        {
            return Err(Rejection::GroupNotInvoice);
        }
        let mut limits = Vec::new();
        let mut dropped = Vec::new();
        // Una candidata cuyo nombre no se envió no puede agruparse por nombre: la pregunta se descarta.
        let withheld: std::collections::HashSet<&str> = batch
            .ids
            .iter()
            .zip(&batch.payload)
            .filter(|(_, p)| p.withheld.is_some())
            .map(|(id, _)| id.as_str())
            .collect();
        let mut groups = Vec::new();
        for group in &parsed.groups {
            if group.iter().any(|id| withheld.contains(id.as_str())) {
                dropped.push(Dropped {
                    kind: "group".into(),
                    fingerprint: fp(&group[0]),
                    code: "withheld_member".into(),
                });
            } else {
                groups.push(group.iter().map(|id| fp(id)).collect());
            }
        }
        for (id, amount) in &parsed.limits {
            let candidate = by_id[id.as_str()];
            match self.check_limit(candidate, amount) {
                Ok(proposal) => limits.push(StoredLimit {
                    fingerprint: proposal.fingerprint,
                    max_total: proposal.max_total,
                }),
                Err(code) => dropped.push(Dropped {
                    kind: "limit".into(),
                    fingerprint: candidate.fingerprint.clone(),
                    code: code.into(),
                }),
            }
        }
        Ok(Interpreted {
            order: parsed.order.iter().map(|id| fp(id)).collect(),
            hide: parsed
                .hide
                .iter()
                .map(|(id, reason)| HiddenByModel {
                    fingerprint: fp(id),
                    reason: reason.clone(),
                })
                .collect(),
            groups,
            limits,
            dropped,
            comment: parsed.comment,
            comment_dropped: parsed.comment_dropped.map(str::to_owned),
        })
    }

    /// Comprueba un límite propuesto por el modelo para una candidata de factura: importe
    /// legible y positivo, dentro del rango observado, distinto del del código, regla válida
    /// (`check_rule`) y simulación que la habría aplicado a alguna factura sin dejar sin
    /// efecto a otra regla activa. Devuelve el código del primer motivo de descarte.
    pub(crate) fn check_limit(
        &self,
        candidate: &Candidate,
        proposed: &str,
    ) -> Result<LimitProposal, &'static str> {
        // Un límite no sale de lo visto: sin importes aprobados la IA tampoco propone ninguno.
        if candidate.evidence.needs_limit {
            return Err("no_approved_amounts");
        }
        let supplier = candidate.conditions["supplier"].as_str();
        let (true, Some(supplier), Some(code_limit)) = (
            candidate.action_kind == REGISTER_INVOICE,
            supplier,
            candidate.evidence.limit.as_deref(),
        ) else {
            return Err("not_invoice");
        };
        let amount = parse_amount("total", proposed).map_err(|_| "bad_amount")?;
        if !is_positive_amount(&amount.to_string()) {
            return Err("not_positive");
        }
        let max = parse_amount("total", code_limit).map_err(|_| "no_range")?;
        let min = candidate
            .evidence
            .amount_min
            .as_deref()
            .and_then(|m| parse_amount("total", m).ok())
            .unwrap_or(max);
        if amount < min || amount > max {
            return Err("out_of_range");
        }
        if amount == max {
            return Err("same_as_code");
        }
        let conditions = json!({ "supplier": supplier, "max_total": amount.to_string() });
        check_rule(REGISTER_INVOICE, &conditions, false).map_err(|_| "invalid_rule")?;
        let sim = self
            .simulate_rule(REGISTER_INVOICE, &conditions, None, false, None)
            .map_err(|_| "simulation_failed")?;
        if sim.would_automate == 0 {
            return Err("simulation_no_effect");
        }
        if !sim.would_block_other.is_empty() {
            return Err("simulation_blocks_other");
        }
        Ok(LimitProposal {
            fingerprint: candidate.fingerprint.clone(),
            max_total: amount.to_string(),
            shown: fingerprint_of(&[&candidate.fingerprint, &conditions.to_string()]),
            considered: sim.considered,
            would_automate: sim.would_automate,
        })
    }
}

struct Interpreted {
    order: Vec<String>,
    hide: Vec<HiddenByModel>,
    groups: Vec<Vec<String>>,
    limits: Vec<StoredLimit>,
    dropped: Vec<Dropped>,
    comment: String,
    comment_dropped: Option<String>,
}
