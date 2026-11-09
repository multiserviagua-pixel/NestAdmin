//! Estado, reanudación y arranque de la fase «Aprender a responder» (E4, D167, D175, R30, T33d,
//! T33e). Une la recogida local (`learn`) y la llamada con consentimiento (`learn_consent`) con
//! una marca persistente por buzón, con el patrón de la marca de la primera lectura (D133).
//!
//! # Marca
//!
//! `rule_settings` `first_learning:account:<proveedor>:<dirección>` (JSON, sin entrada en el
//! Historial al escribirla): `phase` (`collected | awaiting_consent | sent | done | nothing`),
//! `input_fp`, los ids de los recibidos usados, los recuentos ([`LearnCounts`]), el destino
//! (proveedor, modelo y servidor) con el que se mostró la vista previa, quién respondió
//! (`external`/`local`) y las fechas. **Nunca cuerpos de correo ni el texto enviado**. `held` no
//! se guarda: se deduce al comparar el destino de la marca con el de ahora (como en D133).
//!
//! Lo aprendido va en la clave hermana `first_learning_result:<fuente>`: estilo y, por mensaje, solo
//! `category`, `needs_reply` y `reply_kind`, con el id real del mensaje y la **dirección real** del
//! remitente (E5 cuenta por dirección). Son enums y ids, no contenido.
//!
//! # Recogida por pasos
//!
//! La recogida (bandeja y Enviados, hasta unas 350 consultas) no retiene el cerrojo de la
//! aplicación de una vez: un [`Collector`] avanza **un paso por página o por candidato** y quien lo
//! lleva suelta el cerrojo entre un paso y el siguiente, como la primera lectura
//! ([`App::read_step`]). El progreso vive solo en memoria (`LearningRuntime::collecting`); un corte
//! vuelve a empezar. El resultado es idéntico al de recoger de una vez.
//!
//! # Transiciones
//!
//! - [`App::learning_tick`] (lo llama el hilo de trabajo en reposo, también al reanudar): con la
//!   primera lectura de un buzón terminada y sin marca, recoge una sola vez (un paso por llamada).
//!   Con 0 pares: `nothing`
//!   (con recuentos). Con pares y destino externo: `awaiting_consent`, sin enviar nada. Con destino
//!   local: `sent` y devuelve el trabajo para ejecutarlo sin el cerrojo de `App`; al terminar,
//!   `done`.
//! - Reanudación: no se guardan cuerpos. Una marca en `collected` o `awaiting_consent` se vuelve a
//!   recoger **una vez por proceso**; si la huella difiere, la vista previa nueva sustituye a la
//!   anterior y el consentimiento anterior no vale (está ligado a la huella). Una marca en `sent`
//!   cortada no se repite sola: queda `interrupted` y la persona decide con «reintentar». `done` y
//!   `nothing` no vuelven a ejecutarse. Ningún camino automático llama a la IA externa.
//! - Un fallo (de la llamada o de la lectura de la bandeja) deja `awaiting_consent` con `failure`
//!   (o `error`) y no se repite solo.
//! - El Historial recibe **una** entrada agregada al terminar (`done` o `nothing`), escrita en la
//!   misma transacción que la marca: reanudar no la duplica.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use nestadmin_ai::ProviderKind;
use nestadmin_core::LearnedEvidence;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::initial_read::Destination;
use crate::learn::{CollectStep, Collected, Collector, LearnCounts};
use crate::learn_consent::{
    ConsentGate, LearnChoice, LearnExpectation, LearnJob, LearnRun, LearnSource, LearnStatus,
    LearningPreview,
};
use crate::refine::{now_ms, ConsentView};
use crate::{App, AppError, AppResult, LocalFallback};

const MARK_VERSION: u32 = 1;
const COLLECTED: &str = "collected";
const AWAITING: &str = "awaiting_consent";
const SENT: &str = "sent";
const DONE: &str = "done";
const NOTHING: &str = "nothing";
/// Acción y estados de la entrada agregada del Historial.
const AUDIT_ACTION: &str = "first_learning";

// ---- Marca y resultado persistentes ----

/// Fallo registrado en la marca: código y texto sin contenido de correo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LearningIssue {
    pub code: String,
    pub message: String,
}

/// Marca persistente de un buzón. Solo ids, cifras y códigos.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct LearnMark {
    v: u32,
    /// `collected | awaiting_consent | sent | done | nothing`.
    phase: String,
    /// Nombre visible y cuenta (identificador) del buzón.
    label: String,
    target: String,
    /// Huella de lo que saldría ([`Collected::input_fp`]); vacía con 0 pares.
    input_fp: String,
    /// Ids (con prefijo de cuenta) de los recibidos que forman los pares.
    message_ids: Vec<String>,
    counts: LearnCounts,
    /// Destino con el que se mostró la vista previa o se lanzó la llamada.
    destination: Option<Destination>,
    /// Quién respondió: `external` o `local`.
    ai: Option<String>,
    /// Código por el que respondió la IA local de respaldo (D101), si fue así.
    fallback: Option<String>,
    started_at: Option<i64>,
    updated_at: Option<i64>,
    finished_at: Option<i64>,
    /// Por qué no se aprendió en el último intento (códigos), si falló.
    failure: Option<String>,
    /// Primer fallo de Enviados al recoger (código), si lo hubo.
    first_error: Option<String>,
    /// Fallo al leer la bandeja: la recogida no se repite sola.
    error: Option<LearningIssue>,
}

/// Estilo aprendido (D170): campos opcionales y acotados por el validador de E3. El saludo y la
/// despedida salen de líneas de las respuestas propias: el `Debug` no los muestra.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LearnedStyle {
    /// Uno de `formal | cercano | neutro` (enum cerrado, D170).
    pub tone: Option<String>,
    /// Uno de `breve | detallado`.
    #[serde(default)]
    pub length: Option<String>,
    /// Solo si coincide con una línea presente en 2 o más respuestas propias (D170).
    pub greeting: Option<String>,
    pub closing: Option<String>,
}

impl std::fmt::Debug for LearnedStyle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LearnedStyle")
            .field("tone", &self.tone)
            .field("length", &self.length)
            .field("greeting", &self.greeting.as_ref().map(|_| "<oculto>"))
            .field("closing", &self.closing.as_ref().map(|_| "<oculto>"))
            .finish()
    }
}

/// Lo aprendido de un mensaje: ids y enums, nunca texto. La dirección es la real (no el alias) y
/// el `Debug` no la muestra.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LearnedMessage {
    pub message_id: String,
    pub address: String,
    pub category: String,
    pub needs_reply: bool,
    pub reply_kind: String,
}

impl std::fmt::Debug for LearnedMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LearnedMessage")
            .field("message_id", &self.message_id)
            .field("address", &"<oculta>")
            .field("category", &self.category)
            .field("needs_reply", &self.needs_reply)
            .field("reply_kind", &self.reply_kind)
            .finish()
    }
}

/// Resultado guardado para E5 (`first_learning_result:<fuente>`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredLearned {
    pub v: u32,
    /// `external` o `local`.
    pub source: String,
    pub input_fp: String,
    pub at: i64,
    pub style: LearnedStyle,
    pub pairs: Vec<LearnedMessage>,
}

// ---- Vistas ----

/// Retención por cambio de destino (como `Held` de D133).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LearningHeld {
    pub reason: &'static str,
    pub authorized: Option<Destination>,
    pub now: Destination,
}

/// Estado de la fase de un buzón. Solo cifras, ids y códigos.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LearningStatus {
    pub source: String,
    pub account: String,
    pub label: String,
    pub address: Option<String>,
    /// `not_started`, `collected`, `awaiting_consent`, `sent`, `held`, `done` o `nothing`.
    pub state: &'static str,
    /// `sent` sin trabajo en este proceso: se cortó y no se repite sola.
    pub interrupted: bool,
    /// La primera lectura del buzón ya terminó (condición para empezar).
    pub read_done: bool,
    pub input_fp: Option<String>,
    pub counts: Option<LearnCounts>,
    pub pairs: usize,
    pub destination: Option<Destination>,
    pub held: Option<LearningHeld>,
    /// Quién respondió (`external`/`local`) y, si fue la local de respaldo, por qué código.
    pub ai: Option<String>,
    pub fallback: Option<String>,
    pub failure: Option<String>,
    pub first_error: Option<String>,
    pub error: Option<LearningIssue>,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LearningOverview {
    pub accounts: Vec<LearningStatus>,
}

/// Estado y, mientras haya algo que decidir, lo que saldría.
#[derive(Debug, Clone, Serialize)]
pub struct AccountLearningPreview {
    pub status: LearningStatus,
    pub preview: Option<LearningPreview>,
}

/// Elección de la persona ante la vista previa (lo que llega de la ventana).
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LearnChoiceArg {
    /// «Enviar a <proveedor>», con lo que vio.
    Send { expect: LearnExpectation },
    /// «Usar IA local»: con la huella de la vista previa que vio.
    UseLocal { input_fp: String },
}

impl LearnChoiceArg {
    fn input_fp(&self) -> &str {
        match self {
            Self::Send { expect } => &expect.input_fp,
            Self::UseLocal { input_fp } => input_fp,
        }
    }

    fn choice(&self) -> LearnChoice {
        match self {
            Self::Send { expect } => LearnChoice::Send(expect.clone()),
            Self::UseLocal { .. } => LearnChoice::UseLocal,
        }
    }
}

// ---- Trabajo fuera del cerrojo ----

/// Lo que hay que saber de un par para guardar lo aprendido por id y dirección reales. El `Debug`
/// no muestra la dirección.
#[derive(Clone)]
struct PairRef {
    pair_id: String,
    message_id: String,
    address: String,
}

impl std::fmt::Debug for PairRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PairRef")
            .field("pair_id", &self.pair_id)
            .field("message_id", &self.message_id)
            .finish()
    }
}

/// Llamada preparada: no toca `App`, se ejecuta en otro hilo.
pub struct LearningJob {
    source: String,
    input_fp: String,
    refs: Vec<PairRef>,
    inner: LearnJob,
}

impl LearningJob {
    pub fn run(self) -> LearningCompleted {
        LearningCompleted {
            source: self.source,
            input_fp: self.input_fp,
            refs: self.refs,
            run: self.inner.run(),
        }
    }
}

/// Resultado de [`LearningJob::run`], para [`App::finish_learning`].
pub struct LearningCompleted {
    source: String,
    input_fp: String,
    refs: Vec<PairRef>,
    run: LearnRun,
}

/// Qué hizo [`App::learning_tick`].
pub enum LearningStep {
    /// Nada que hacer ahora.
    Idle,
    /// Hubo trabajo local (recoger, reanudar, cerrar con «nada que aprender»).
    Progress,
    /// Hay una llamada que ejecutar sin el cerrojo de `App` (destino local).
    Job(Box<LearningJob>),
}

/// Recogida en curso de un buzón: solo en memoria.
struct Collecting {
    collector: Collector,
    /// Marca previa (para conservar la fecha de inicio y limpiar errores).
    prior: Option<LearnMark>,
    /// Con destino local, lanzar la llamada al terminar (solo en el primer arranque).
    auto_local: bool,
    /// La lleva la ventana (la persona la pidió): el hilo de trabajo no la toca.
    by_person: bool,
}

/// Estado en memoria (no se guarda): reanudaciones ya hechas en este proceso, trabajos en curso,
/// recogidas por pasos y lo recogido, para que lo enviado sea idéntico a la vista previa sin
/// volver a leer el buzón.
#[derive(Default)]
pub(crate) struct LearningRuntime {
    checked: HashSet<String>,
    running: HashSet<String>,
    cache: HashMap<String, Collected>,
    collecting: HashMap<String, Collecting>,
    /// Huellas con consentimiento vigente (se vuelve a comprobar justo antes de enviar).
    pub(crate) gate: ConsentGate,
    /// Resolvedor de la IA local para «Usar IA local», independiente de `local_fallback`.
    pub(crate) local_ai: Option<LocalFallback>,
}

// ---- App ----

impl App {
    fn learning_destination(&self) -> Destination {
        let info = self.provider();
        Destination {
            provider: info.provider.to_string(),
            model: info.model,
            host: self.provider.destination(),
        }
    }

    fn learning_is_external(&self) -> bool {
        self.provider.kind() != ProviderKind::Ollama
    }

    fn load_learn(&self, source: &str) -> AppResult<Option<LearnMark>> {
        Ok(self
            .store
            .learning_mark(source)?
            .and_then(|v| serde_json::from_value(v).ok()))
    }

    fn save_learn(
        &mut self,
        source: &str,
        mark: &LearnMark,
        result: Option<&Value>,
        note: Option<(&str, &str, &Value)>,
    ) -> AppResult<()> {
        self.save_learn_with(source, mark, result, note, None)
    }

    /// Como `save_learn`; con `evidence` guarda en la misma transacción las señales `learned` y
    /// `reply_style` (D171, D172).
    fn save_learn_with(
        &mut self,
        source: &str,
        mark: &LearnMark,
        result: Option<&Value>,
        note: Option<(&str, &str, &Value)>,
        evidence: Option<&LearnedEvidence>,
    ) -> AppResult<()> {
        let value = serde_json::to_value(mark).map_err(|e| AppError::Invalid(e.to_string()))?;
        self.store
            .save_learning(source, &value, result, note, evidence)?;
        Ok(())
    }

    /// Cuenta (identificador) de una fuente `account:…`, con su nombre y dirección.
    fn learning_account(&self, account: &str) -> (String, Option<String>) {
        self.mail_accounts()
            .accounts
            .into_iter()
            .find(|a| a.id == account)
            .map_or((account.to_owned(), None), |a| (a.label, a.address))
    }

    fn learning_source_of(&self, account: &str) -> AppResult<String> {
        self.account_read_source(account)
            .ok_or_else(|| AppError::Invalid("la cuenta no está conectada".into()))
    }

    /// La primera lectura de la fuente ya terminó (con o sin errores de tareas).
    fn learning_read_done(&self, source: &str) -> bool {
        self.initial_read_status(source)
            .is_ok_and(|s| matches!(s.state, "done" | "done_with_errors"))
    }

    fn learning_held(&self, mark: &LearnMark) -> Option<LearningHeld> {
        let now = self.learning_destination();
        (matches!(mark.phase.as_str(), COLLECTED | AWAITING | SENT)
            && mark.destination.as_ref().is_some_and(|d| *d != now))
        .then(|| LearningHeld {
            reason: "destination_changed",
            authorized: mark.destination.clone(),
            now,
        })
    }

    fn learning_status_of(&self, source: &str, account: &str) -> AppResult<LearningStatus> {
        let (label, address) = self.learning_account(account);
        let read_done = self.learning_read_done(source);
        let Some(mark) = self.load_learn(source)? else {
            return Ok(LearningStatus {
                source: source.to_owned(),
                account: account.to_owned(),
                label,
                address,
                state: "not_started",
                interrupted: false,
                read_done,
                input_fp: None,
                counts: None,
                pairs: 0,
                destination: None,
                held: None,
                ai: None,
                fallback: None,
                failure: None,
                first_error: None,
                error: None,
                started_at: None,
                finished_at: None,
            });
        };
        let held = self.learning_held(&mark);
        let state = match mark.phase.as_str() {
            DONE => "done",
            NOTHING => "nothing",
            _ if held.is_some() => "held",
            COLLECTED => "collected",
            SENT => "sent",
            _ => "awaiting_consent",
        };
        let interrupted = mark.phase == SENT && !self.learning.running.contains(source);
        Ok(LearningStatus {
            source: source.to_owned(),
            account: account.to_owned(),
            label: if mark.label.is_empty() {
                label
            } else {
                mark.label.clone()
            },
            address,
            state,
            interrupted,
            read_done,
            input_fp: Some(mark.input_fp.clone()).filter(|f| !f.is_empty()),
            pairs: mark.counts.pairs,
            counts: Some(mark.counts),
            destination: mark.destination,
            held,
            ai: mark.ai,
            fallback: mark.fallback,
            failure: mark.failure,
            first_error: mark.first_error,
            error: mark.error,
            started_at: mark.started_at,
            finished_at: mark.finished_at,
        })
    }

    /// Estado de la fase de cada buzón conectado con primera lectura. Solo lectura: no toca el
    /// buzón ni la red.
    pub fn learning_overview(&self) -> AppResult<LearningOverview> {
        let mut accounts = Vec::new();
        for (source, read) in self.store.read_marks()? {
            if read["kind"] != "account" {
                continue;
            }
            let Some(account) = read["target"].as_str() else {
                continue;
            };
            if self.account_read_source(account).as_deref() != Some(source.as_str()) {
                continue;
            }
            accounts.push(self.learning_status_of(&source, account)?);
        }
        Ok(LearningOverview { accounts })
    }

    /// Estado de la fase de una cuenta.
    pub fn learning_status(&self, account: &str) -> AppResult<LearningStatus> {
        let source = self.learning_source_of(account)?;
        self.learning_status_of(&source, account)
    }

    /// Lo aprendido de una cuenta (estilo y enums por mensaje), si la fase terminó con `done`.
    pub fn learned_result(&self, account: &str) -> AppResult<Option<StoredLearned>> {
        let source = self.learning_source_of(account)?;
        Ok(self
            .store
            .learning_result(&source)?
            .and_then(|v| serde_json::from_value(v).ok()))
    }

    // ---- Recogida y transiciones ----

    /// Escribe el resultado de recoger y decide el paso siguiente. `auto_local`: con destino local
    /// se lanza la llamada sin esperar a nadie (solo en el primer arranque o al reanudar un corte
    /// en `collected`). Devuelve el trabajo si hay que llamar a la IA.
    fn apply_collection(
        &mut self,
        source: &str,
        account: &str,
        prior: Option<&LearnMark>,
        collected: Collected,
        auto_local: bool,
    ) -> AppResult<Option<LearningJob>> {
        let now = now_ms();
        let (label, _) = self.learning_account(account);
        let mut mark = LearnMark {
            v: MARK_VERSION,
            phase: COLLECTED.into(),
            label,
            target: account.to_owned(),
            input_fp: collected.input_fp.clone(),
            message_ids: collected
                .pairs
                .iter()
                .map(|p| p.message_id.clone())
                .collect(),
            counts: collected.counts.clone(),
            destination: Some(self.learning_destination()),
            started_at: prior.and_then(|m| m.started_at).or(Some(now)),
            updated_at: Some(now),
            first_error: collected.first_error.clone(),
            ..LearnMark::default()
        };
        // Primera vez: queda constancia de que se recogió antes de decidir. Un corte aquí se
        // reanuda recogiendo de nuevo (no se guardan cuerpos).
        if prior.is_none() {
            self.save_learn(source, &mark, None, None)?;
        }
        if collected.pairs.is_empty() {
            mark.phase = NOTHING.into();
            // Sin pares no hay texto que enviar ni huella que consentir.
            mark.input_fp = String::new();
            mark.finished_at = Some(now);
            let detail = nothing_note(&mark);
            self.save_learn(
                source,
                &mark,
                None,
                Some((AUDIT_ACTION, NOTHING, &Value::String(detail))),
            )?;
            self.learning.cache.remove(source);
            return Ok(None);
        }
        if self.learning_is_external() || !auto_local {
            mark.phase = AWAITING.into();
            mark.failure = prior.and_then(|m| m.failure.clone());
            self.save_learn(source, &mark, None, None)?;
            self.learning.cache.insert(source.to_owned(), collected);
            return Ok(None);
        }
        // Destino local: se ejecuta en local, sin consentimiento y sin salir del equipo.
        self.learning
            .cache
            .insert(source.to_owned(), collected.clone());
        Ok(Some(self.launch_learning(
            source,
            mark,
            &collected,
            &LearnChoice::UseLocal,
        )?))
    }

    /// Prepara la llamada, anota `sent` y deja el trabajo listo para ejecutar sin el cerrojo.
    fn launch_learning(
        &mut self,
        source: &str,
        mut mark: LearnMark,
        collected: &Collected,
        choice: &LearnChoice,
    ) -> AppResult<LearningJob> {
        let inner = self.begin_learning(collected, choice)?;
        mark.phase = SENT.into();
        mark.destination = Some(self.learning_destination());
        mark.failure = None;
        mark.updated_at = Some(now_ms());
        self.save_learn(source, &mark, None, None)?;
        self.learning.running.insert(source.to_owned());
        let refs = collected
            .pairs
            .iter()
            .map(|p| PairRef {
                pair_id: p.pair_id.clone(),
                message_id: p.message_id.clone(),
                address: collected
                    .aliases
                    .iter()
                    .find(|a| a.alias == p.alias)
                    .map(|a| a.address.clone())
                    .unwrap_or_default(),
            })
            .collect();
        Ok(LearningJob {
            source: source.to_owned(),
            input_fp: collected.input_fp.clone(),
            refs,
            inner,
        })
    }

    /// Anota que no se pudo leer la bandeja: no se repite sola. «Reintentar» lo limpia.
    fn note_learning_error(
        &mut self,
        source: &str,
        account: &str,
        prior: Option<LearnMark>,
        code: &str,
        message: &str,
    ) -> AppResult<()> {
        let (label, _) = self.learning_account(account);
        let mut mark = prior.unwrap_or_else(|| LearnMark {
            v: MARK_VERSION,
            phase: COLLECTED.into(),
            label,
            target: account.to_owned(),
            destination: Some(self.learning_destination()),
            started_at: Some(now_ms()),
            ..LearnMark::default()
        });
        mark.error = Some(LearningIssue {
            code: code.to_owned(),
            message: message.to_owned(),
        });
        mark.updated_at = Some(now_ms());
        self.save_learn(source, &mark, None, None)
    }

    /// Un paso de la fase para los buzones con la primera lectura terminada. Sin la cola en
    /// pausa. No llama nunca a la IA externa; con destino local devuelve el trabajo. Un error de
    /// un buzón queda en su marca y no impide los demás ni se repite cada pocos segundos.
    pub fn learning_tick(&mut self) -> AppResult<LearningStep> {
        if self.paused {
            return Ok(LearningStep::Idle);
        }
        for (source, read) in self.store.read_marks()? {
            if read["kind"] != "account"
                || read["removed"] == true
                || read["stopped"] == true
                || !read["error"].is_null()
            {
                continue;
            }
            let Some(account) = read["target"].as_str().map(str::to_owned) else {
                continue;
            };
            if self.account_read_source(&account).as_deref() != Some(source.as_str())
                || !self.learning_read_done(&source)
            {
                continue;
            }
            // Recogida en curso: un paso por llamada (la lleva este hilo, salvo que la pidiera la
            // persona).
            if let Some(c) = self.learning.collecting.get(&source) {
                if c.by_person {
                    continue;
                }
                let (_, job) = self.step_collection(&source, &account)?;
                return Ok(job.map_or(LearningStep::Progress, |j| LearningStep::Job(Box::new(j))));
            }
            let mark = self.load_learn(&source)?;
            let auto_local = match &mark {
                None => true,
                Some(m) if m.error.is_some() => continue,
                // Cortada entre recoger y decidir: se reanuda como la primera vez.
                Some(m) if m.phase == COLLECTED => m.failure.is_none(),
                Some(m) if m.phase == AWAITING => false,
                // `sent` cortada: no se repite sola. `done` y `nothing`: terminales.
                Some(_) => continue,
            };
            if self.learning.checked.contains(&source)
                || mark
                    .as_ref()
                    .is_some_and(|m| self.learning_held(m).is_some())
            {
                continue;
            }
            // Una sola vez por proceso y buzón: ni recoger ni reanudar se repiten cada pocos
            // segundos. Un error al recoger queda en la marca y tampoco se repite.
            self.learning.checked.insert(source.clone());
            self.begin_collection(&source, &account, mark, auto_local, false);
            return Ok(LearningStep::Progress);
        }
        Ok(LearningStep::Idle)
    }

    /// Prepara una recogida por pasos (solo en memoria). Sustituye a la que hubiera en curso.
    fn begin_collection(
        &mut self,
        source: &str,
        account: &str,
        prior: Option<LearnMark>,
        auto_local: bool,
        by_person: bool,
    ) {
        self.learning.collecting.insert(
            source.to_owned(),
            Collecting {
                collector: Collector::new(account),
                prior,
                auto_local,
                by_person,
            },
        );
    }

    /// Un paso de la recogida de `source` (una página del buzón o un candidato). Devuelve si queda
    /// recogida en curso y, al terminar con destino local, la llamada que ejecutar sin el cerrojo.
    /// Un fallo al leer la bandeja queda en la marca y no se repite solo. Sin recogida en curso,
    /// no hace nada.
    fn step_collection(
        &mut self,
        source: &str,
        account: &str,
    ) -> AppResult<(bool, Option<LearningJob>)> {
        let Some(mut collecting) = self.learning.collecting.remove(source) else {
            return Ok((false, None));
        };
        match collecting.collector.step(&self.mail) {
            Ok(CollectStep::Continue) => {
                self.learning
                    .collecting
                    .insert(source.to_owned(), collecting);
                Ok((true, None))
            }
            Ok(CollectStep::Done(collected)) => {
                let job = self.apply_collection(
                    source,
                    account,
                    collecting.prior.as_ref(),
                    *collected,
                    collecting.auto_local,
                )?;
                Ok((false, job))
            }
            Err(e) => {
                self.note_learning_error(source, account, collecting.prior, &e.code, &e.message)?;
                Ok((false, None))
            }
        }
    }

    /// Vuelve a recoger por orden de la persona y deja la vista previa vigente. Sustituye a la
    /// anterior si la huella cambió (el consentimiento anterior deja de valer) y fija el destino
    /// de ahora, lo que levanta una retención por cambio de destino. Todo en este hilo: la
    /// ventana usa [`App::start_account_learning_preview`] y [`App::step_account_learning_preview`]
    /// para soltar el cerrojo entre pasos.
    pub fn account_learning_preview(&mut self, account: &str) -> AppResult<AccountLearningPreview> {
        if self.start_account_learning_preview(account)? {
            while self.step_account_learning_preview(account)? {}
        }
        self.account_learning_preview_view(account)
    }

    /// Empieza la recogida pedida por la persona (solo en memoria; no lee nada todavía). Devuelve
    /// `false` si no hay nada que recoger (la fase terminó o hay una llamada en curso).
    pub fn start_account_learning_preview(&mut self, account: &str) -> AppResult<bool> {
        let source = self.learning_source_of(account)?;
        let Some(mark) = self.load_learn(&source)? else {
            return Err(AppError::Invalid(
                "La recogida de esta cuenta aún no ha empezado: espere a que termine la primera lectura."
                    .into(),
            ));
        };
        if matches!(mark.phase.as_str(), DONE | NOTHING) || self.learning.running.contains(&source)
        {
            return Ok(false);
        }
        self.start_person_collection(&source, account, mark);
        Ok(true)
    }

    /// Un paso de la recogida que pidió la persona. Devuelve si queda más por hacer.
    pub fn step_account_learning_preview(&mut self, account: &str) -> AppResult<bool> {
        let source = self.learning_source_of(account)?;
        if !self
            .learning
            .collecting
            .get(&source)
            .is_some_and(|c| c.by_person)
        {
            return Ok(false);
        }
        Ok(self.step_collection(&source, account)?.0)
    }

    /// Estado y vista previa vigente (la de la última recogida terminada). Solo lectura.
    pub fn account_learning_preview_view(
        &self,
        account: &str,
    ) -> AppResult<AccountLearningPreview> {
        let source = self.learning_source_of(account)?;
        let Some(mark) = self.load_learn(&source)? else {
            return Err(AppError::Invalid(
                "La recogida de esta cuenta aún no ha empezado.".into(),
            ));
        };
        if matches!(mark.phase.as_str(), DONE | NOTHING) || self.learning.running.contains(&source)
        {
            return Ok(AccountLearningPreview {
                status: self.learning_status_of(&source, account)?,
                preview: None,
            });
        }
        self.preview_of(&source, account)
    }

    /// Reintenta tras un fallo, una lectura de la bandeja fallida o un corte: vuelve a recoger y
    /// deja una vista previa nueva. No llama a la IA. Todo en este hilo; la ventana usa
    /// [`App::start_retry_learning`] y [`App::step_account_learning_preview`].
    pub fn retry_learning(&mut self, account: &str) -> AppResult<LearningStatus> {
        self.start_retry_learning(account)?;
        while self.step_account_learning_preview(account)? {}
        self.learning_status(account)
    }

    /// Empieza el reintento (solo en memoria). Rechaza si no hay nada que reintentar.
    pub fn start_retry_learning(&mut self, account: &str) -> AppResult<()> {
        let source = self.learning_source_of(account)?;
        let Some(mark) = self.load_learn(&source)? else {
            return Err(AppError::Invalid(
                "La recogida de esta cuenta aún no ha empezado.".into(),
            ));
        };
        let interrupted = mark.phase == SENT && !self.learning.running.contains(&source);
        if matches!(mark.phase.as_str(), DONE | NOTHING)
            || self.learning.running.contains(&source)
            || !(mark.error.is_some() || mark.failure.is_some() || interrupted)
        {
            return Err(AppError::Invalid(
                "No hay nada que reintentar en esta cuenta.".into(),
            ));
        }
        self.start_person_collection(&source, account, mark);
        Ok(())
    }

    /// Recogida pedida por la persona: no lanza llamadas (decide ella) y limpia errores previos.
    fn start_person_collection(&mut self, source: &str, account: &str, mut mark: LearnMark) {
        mark.error = None;
        mark.failure = None;
        mark.phase = AWAITING.into();
        self.learning.checked.insert(source.to_owned());
        self.begin_collection(source, account, Some(mark), false, true);
    }

    /// Resolvedor de la IA local para la elección explícita «Usar IA local», independiente del
    /// interruptor de respaldo (`local_fallback`): es una elección de la persona, no un respaldo.
    pub fn set_local_ai(&mut self, local_ai: Option<LocalFallback>) {
        self.learning.local_ai = local_ai;
    }

    fn preview_of(&self, source: &str, account: &str) -> AppResult<AccountLearningPreview> {
        let preview = match self.learning.cache.get(source) {
            Some(collected) => Some(self.learning_preview(collected)?),
            None => None,
        };
        Ok(AccountLearningPreview {
            status: self.learning_status_of(source, account)?,
            preview,
        })
    }

    /// Lo recogido y vigente, listo para consentir o enviar: de la memoria o, tras un corte,
    /// recogido de nuevo. Si la huella cambió, la marca se actualiza con la vista previa nueva.
    fn current_collected(&mut self, source: &str, account: &str) -> AppResult<Collected> {
        let Some(mark) = self.load_learn(source)? else {
            return Err(AppError::Invalid(
                "La recogida de esta cuenta aún no ha empezado.".into(),
            ));
        };
        if mark.phase != AWAITING {
            return Err(AppError::Invalid(
                "Esta cuenta no está esperando una decisión de aprendizaje.".into(),
            ));
        }
        if self.learning_held(&mark).is_some() {
            return Err(AppError::Invalid(
                "El proveedor, el modelo o el servidor han cambiado: genere de nuevo la vista previa."
                    .into(),
            ));
        }
        if let Some(c) = self.learning.cache.get(source) {
            return Ok(c.clone());
        }
        // Sin lo recogido en memoria no se vuelve a leer el buzón bajo el cerrojo: la persona
        // genera la vista previa de nuevo (por pasos) y decide sobre ella.
        let _ = (account, mark);
        Err(AppError::Invalid(
            "La vista previa ya no está en memoria: genérela de nuevo antes de continuar.".into(),
        ))
    }

    /// La persona consiente enviar lo que vio. Rechaza si lo recogido cambió o el destino no es
    /// el que vio (ver [`App::grant_learning_consent`]).
    pub fn grant_account_learning_consent(
        &mut self,
        account: &str,
        expectation: &LearnExpectation,
    ) -> AppResult<ConsentView> {
        let source = self.learning_source_of(account)?;
        let collected = self.current_collected(&source, account)?;
        self.grant_learning_consent(&collected, expectation)
    }

    /// Prepara la llamada elegida por la persona. Rechaza, sin enviar nada, si la huella ya no es
    /// la de la vista previa, si el destino cambió, si hay una espera de IA con el externo o si la
    /// cuenta no está esperando una decisión. Anota `sent` antes de devolver el trabajo.
    pub fn begin_account_learning(
        &mut self,
        account: &str,
        choice: &LearnChoiceArg,
    ) -> AppResult<LearningJob> {
        let source = self.learning_source_of(account)?;
        if self.learning.running.contains(&source) {
            return Err(AppError::Invalid(
                "El aprendizaje de esta cuenta ya está en curso.".into(),
            ));
        }
        let collected = self.current_collected(&source, account)?;
        if collected.input_fp != choice.input_fp() {
            return Err(AppError::Invalid(
                "Lo que se iba a enviar ha cambiado desde que vio la vista previa: revísela de nuevo antes de continuar."
                    .into(),
            ));
        }
        let Some(mark) = self.load_learn(&source)? else {
            return Err(AppError::Invalid("La recogida no ha empezado.".into()));
        };
        self.launch_learning(&source, mark, &collected, &choice.choice())
    }

    /// Cierra una llamada: guarda lo aprendido y `done` con una entrada agregada en el Historial,
    /// o, si no hubo respuesta válida, vuelve a `awaiting_consent` con el código del fallo. Un
    /// resultado que ya no corresponde a la marca (otra huella, ya cerrada) se ignora.
    pub fn finish_learning(&mut self, completed: LearningCompleted) -> AppResult<LearningStatus> {
        let LearningCompleted {
            source,
            input_fp,
            refs,
            run,
        } = completed;
        self.learning.running.remove(&source);
        // Como las demás tareas (D101, D146): una clave que no sirve, un modelo que no existe o la
        // falta de crédito fijan la espera de IA. Si el proveedor ya cambió, el aviso es del
        // anterior y no se vuelve a poner.
        if let (Some(wait), Some(used)) = (run.wait, run.used.as_ref()) {
            if Arc::ptr_eq(used, &self.provider) {
                self.ai_wait = Some(wait);
                if wait == crate::AiWait::InvalidKey {
                    self.key_fixed = false;
                }
            }
        }
        let mark = self.load_learn(&source)?;
        let account = mark.as_ref().map(|m| m.target.clone()).unwrap_or_default();
        let Some(mut mark) = mark.filter(|m| m.phase == SENT && m.input_fp == input_fp) else {
            return self.learning_status_of(&source, &account);
        };
        let now = now_ms();
        mark.updated_at = Some(now);
        mark.counts = run.counts.clone();
        match (run.status, run.learned.as_ref(), run.source) {
            (LearnStatus::Learned, Some(learned), Some(from)) => {
                let ai = match from {
                    LearnSource::External => "external",
                    LearnSource::Local => "local",
                };
                let stored = StoredLearned {
                    v: MARK_VERSION,
                    source: ai.into(),
                    input_fp: input_fp.clone(),
                    at: now,
                    style: LearnedStyle {
                        tone: learned.style.tone.clone(),
                        length: learned.style.length.clone(),
                        greeting: learned.style.greeting.clone(),
                        closing: learned.style.closing.clone(),
                    },
                    pairs: learned
                        .pairs
                        .iter()
                        .filter_map(|p| {
                            let r = refs.iter().find(|r| r.pair_id == p.pair_id)?;
                            Some(LearnedMessage {
                                message_id: r.message_id.clone(),
                                address: r.address.clone(),
                                category: p.category.clone(),
                                needs_reply: p.needs_reply,
                                reply_kind: p.reply_kind.clone(),
                            })
                        })
                        .collect(),
                };
                mark.phase = DONE.into();
                mark.ai = Some(ai.into());
                mark.fallback = (from == LearnSource::Local)
                    .then(|| run.external_failure.clone())
                    .flatten();
                mark.failure = None;
                mark.finished_at = Some(now);
                let detail = done_note(&mark, stored.pairs.len());
                let result =
                    serde_json::to_value(&stored).map_err(|e| AppError::Invalid(e.to_string()))?;
                // La evidencia `learned` y `reply_style` van en la misma transacción (D171, D172).
                let evidence = crate::learn_evidence::learned_evidence(&source, &stored.pairs);
                self.save_learn_with(
                    &source,
                    &mark,
                    Some(&result),
                    Some((AUDIT_ACTION, DONE, &Value::String(detail))),
                    Some(&evidence),
                )?;
                self.learning.cache.remove(&source);
            }
            _ => {
                // Sin respuesta válida: «sin aprender», con el código; la persona decide.
                mark.phase = AWAITING.into();
                mark.failure = Some(
                    [
                        run.external_failure.as_deref(),
                        run.local_failure.as_deref(),
                        run.external_skipped,
                    ]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(","),
                )
                .filter(|f| !f.is_empty())
                .or_else(|| Some("unavailable".into()));
                self.save_learn(&source, &mark, None, None)?;
            }
        }
        self.learning_status_of(&source, &account)
    }

    /// Ejecuta la llamada elegida y la cierra, todo en este hilo (pruebas y llamadas sin
    /// ventana). La ventana usa [`App::begin_account_learning`] y [`App::finish_learning`] para
    /// soltar el cerrojo durante la llamada.
    pub fn run_account_learning(
        &mut self,
        account: &str,
        choice: &LearnChoiceArg,
    ) -> AppResult<LearningStatus> {
        let completed = self.begin_account_learning(account, choice)?.run();
        self.finish_learning(completed)
    }

    /// Pasos de la fase hasta que no quede trabajo, en este hilo (pruebas y llamadas sin
    /// ventana). Ejecuta las llamadas locales. Devuelve cuántos pasos hizo.
    pub fn run_learning_ticks(&mut self) -> AppResult<usize> {
        let mut steps = 0;
        loop {
            match self.learning_tick()? {
                LearningStep::Idle => return Ok(steps),
                LearningStep::Progress => {}
                LearningStep::Job(job) => {
                    let completed = job.run();
                    self.finish_learning(completed)?;
                }
            }
            steps += 1;
        }
    }
}

// ---- Entradas del Historial (solo recuentos) ----

fn counts_text(c: &LearnCounts) -> String {
    format!(
        "{} recibidos examinados, {} automáticos, {} sin respuesta suya, {} por conector sin Enviados, {} descartados por cita dudosa, {} con error al leer Enviados",
        c.scanned, c.automatic, c.no_own_reply, c.no_sent_folder, c.quote_dropped, c.fetch_errors
    )
}

fn nothing_note(mark: &LearnMark) -> String {
    format!(
        "Aprender a responder de «{}»: nada que aprender ({}). No se envió ningún correo ni se creó ninguna regla.",
        mark.label,
        counts_text(&mark.counts)
    )
}

fn done_note(mark: &LearnMark, learned: usize) -> String {
    let who = match mark.ai.as_deref() {
        Some("external") => "la IA externa elegida",
        _ => "la IA local",
    };
    let fallback = mark
        .fallback
        .as_deref()
        .map(|code| format!(" (respaldo local por {code})"))
        .unwrap_or_default();
    format!(
        "Aprender a responder de «{}» terminado con {who}{fallback}: {} pares enviados al análisis, {learned} con resultado ({}). No se envió ningún correo ni se creó ninguna regla.",
        mark.label,
        mark.counts.pairs,
        counts_text(&mark.counts)
    )
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Arc;

    use nestadmin_ai::{AiError, GenerateRequest, GenerateResult, Provider, Usage};
    use nestadmin_core::Connector;
    use nestadmin_extract::{ExtractError, Extractor, Ocr};
    use nestadmin_simulators::{
        MailConnector, MailSimulator, Message, MessagePage, NewDraft, SendReceipt,
    };
    use serde_json::json;

    use super::*;
    use crate::initial_read::ReadExpectation;
    use crate::refine::RefineMode;
    use crate::AppPaths;

    /// Proveedor que falla si se le llama: con un conector sin Enviados no debe salir nada.
    struct NeverCalled;

    impl Provider for NeverCalled {
        fn kind(&self) -> ProviderKind {
            ProviderKind::OllamaCloud
        }
        fn model(&self) -> &str {
            "modelo-sintetico"
        }
        fn destination(&self) -> Option<String> {
            Some("ollama.test".into())
        }
        fn generate(&self, _request: &GenerateRequest) -> Result<GenerateResult, AiError> {
            let _ = Usage::default();
            panic!("no debe llamarse a la IA");
        }
    }

    struct NoOcr;

    impl Ocr for NoOcr {
        fn recognize(&self, _image: &Path, page: usize) -> Result<String, ExtractError> {
            Err(ExtractError::OcrUnavailable(format!("página {page}")))
        }
    }

    /// Conector que envuelve al simulador pero sin Enviados (el valor por defecto del contrato).
    struct NoSent(MailSimulator);

    impl Connector for NoSent {
        fn lookup(&self, key: &str) -> Result<Option<Value>, nestadmin_core::ConnectorError> {
            self.0.lookup(key)
        }
        fn execute(
            &self,
            key: &str,
            payload: &Value,
        ) -> Result<Value, nestadmin_core::ConnectorError> {
            self.0.execute(key, payload)
        }
    }

    impl MailConnector for NoSent {
        fn list_messages(
            &self,
            cursor: Option<&str>,
        ) -> Result<MessagePage, nestadmin_core::ConnectorError> {
            self.0.list_messages(cursor)
        }
        fn create_draft(
            &self,
            message_id: &str,
            draft: &NewDraft,
            key: &str,
        ) -> Result<String, nestadmin_core::ConnectorError> {
            self.0.create_draft(message_id, draft, key)
        }
        fn send_draft(
            &self,
            draft_id: &str,
            key: &str,
        ) -> Result<SendReceipt, nestadmin_core::ConnectorError> {
            self.0.send_draft(draft_id, key)
        }
        fn fetch_attachment(
            &self,
            m: &str,
            a: &str,
        ) -> Result<Vec<u8>, nestadmin_core::ConnectorError> {
            self.0.fetch_attachment(m, a)
        }
    }

    fn incoming(i: usize, from: &str) -> Message {
        Message {
            id: format!("m{i}"),
            subject: Some(format!("Consulta {i}")),
            from: Some(from.into()),
            to: Some(vec!["administracion@nestadmin.test".into()]),
            body: Some("¿Pueden confirmar?".into()),
            date: Some("2026-11-01T10:00:00Z".into()),
            attachments: Vec::new(),
            signals: None,
            thread_key: Some(format!("t{i}")),
        }
        .with_checked_signals(&[])
    }

    /// T33e: un conector sin Enviados no puede emparejar nada: termina en `nothing` con
    /// `no_sent_folder` y los recuentos, sin llamar a la IA, una sola entrada del Historial y sin
    /// repetirse al reanudar.
    #[test]
    fn a_connector_without_sent_ends_in_nothing_with_no_sent_folder() {
        let dir = tempfile::tempdir().unwrap();
        let extractor = Extractor::new(dir.path().join("sin-pdfium"), Box::new(NoOcr));
        let mut app = App::open(
            &AppPaths::new(dir.path()),
            Arc::new(NeverCalled),
            Arc::new(extractor),
        )
        .unwrap();
        app.set_refine_mode(RefineMode::OnDemand);
        let sim = MailSimulator::with_messages(vec![
            incoming(1, "ana@cliente.test"),
            incoming(2, "luis@proveedor.test"),
            incoming(3, "noreply@tienda.test")
                .with_checked_signals(&[("Auto-Submitted", "auto-generated")]),
        ]);
        app.mail
            .insert_test_connector("sin-enviados", Box::new(NoSent(sim)));
        let expect = ReadExpectation {
            provider: "ollama_cloud".into(),
            model: "modelo-sintetico".into(),
            host: "ollama.test".into(),
            volume: 25,
        };
        app.start_initial_read("sin-enviados", Some(25), Some(&expect))
            .unwrap();
        while app.process_next().unwrap().is_some() {}

        assert!(app.run_learning_ticks().unwrap() >= 1);
        let s = app.learning_status("sin-enviados").unwrap();
        assert_eq!(s.state, "nothing");
        let counts = s.counts.unwrap();
        assert_eq!(
            (counts.no_sent_folder, counts.candidates, counts.automatic),
            (2, 2, 1)
        );
        assert_eq!((counts.pairs, counts.no_own_reply), (0, 0));
        let entries: Vec<_> = app
            .audit_log()
            .unwrap()
            .into_iter()
            .filter(|e| e.action == AUDIT_ACTION)
            .collect();
        assert_eq!(entries.len(), 1);
        let detail = entries[0].detail.clone().unwrap();
        assert!(detail.contains("2 por conector sin Enviados"), "{detail}");
        assert_eq!(app.run_learning_ticks().unwrap(), 0);
        assert!(json!(app.learned_result("sin-enviados").unwrap()).is_null());
    }
}
