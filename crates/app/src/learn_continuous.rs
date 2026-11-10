//! Aprendizaje continuo de las respuestas hechas fuera de NestAdmin (D178, R31, T34a-d).
//!
//! Con la cola vacía, cada cierto tiempo y por buzón, NestAdmin mira en Enviados si la persona ya
//! respondió desde Gmail u Outlook a un correo que tiene como tarea. Si es así:
//!
//! - cuenta una fila `learned` (D172) con la misma clave que la primera carga, de modo que el
//!   índice único impide contar dos veces un correo; y
//! - cierra sin enviar la tarea que esperaba revisión (`closed.reason = replied_outside`), para
//!   que no se apruebe una segunda respuesta.
//!
//! No llama a la IA: la categoría y `needs_reply` son los que el triaje o la clasificación ya
//! dejaron en la tarea. El cuerpo que devuelve el conector se descarta en memoria en cuanto se
//! comprueba el destinatario: no se guarda, no se registra, no entra en ningún `Debug` ni en la
//! auditoría y no sale hacia ninguna IA. El resumen lleva solo recuentos y códigos.
//!
//! # Por pasos
//!
//! Como la recogida de D175, una pasada avanza **una consulta por paso** y quien la lleva suelta el
//! cerrojo de la aplicación entre un paso y el siguiente. Cada paso vuelve a leer la tarea y, si
//! ya no está como estaba al planificar la pasada (la persona la aprobó, archivó, editó o
//! rechazó), no la toca; la escritura repite esa comprobación dentro de su transacción
//! ([`nestadmin_core::Store::close_replied_outside`]).
//!
//! # Fallos
//!
//! Un error de red, de credenciales o del conector detiene la pasada de ese buzón sin escribir
//! nada más y deja su código en el resumen (`last_error`); se reintenta en la siguiente pasada. Con
//! el ajuste desactivado, sin credenciales o con la cuenta desconectada no se hace ninguna consulta.

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{Duration, Instant};

use nestadmin_ai::tasks::{CATEGORIES, REVIEW_ONLY_CATEGORIES};
use nestadmin_core::mail_signals::{decide, route_of_message, sender_parts, Route};
use nestadmin_core::{LearnedSignal, Task, TaskState};
use nestadmin_simulators::Message;
use serde::Serialize;
use serde_json::{json, Value};

use crate::accounts::{split_id, AccountStatus};
use crate::initial_read::{day_of, today_day};
use crate::learn::replies_to;
use crate::refine::now_ms;
use crate::rules::is_email_address;
use crate::{App, AppResult, EMAIL};

/// Días hacia atrás (por la fecha del mensaje o, si falta, la de creación de la tarea) que se
/// vigilan.
pub const CONTINUOUS_WINDOW_DAYS: i64 = 14;
/// Tareas que se comprueban como mucho en una pasada.
pub const CONTINUOUS_MAX_PER_PASS: usize = 25;
/// Separación mínima entre dos pasadas de un mismo buzón.
pub const CONTINUOUS_EVERY: Duration = Duration::from_secs(15 * 60);

const SETTING_KEY: &str = "continuous_learning";
const REPLIED_OUTSIDE: &str = "replied_outside";

// ---- Resumen ----

/// Recuentos de un buzón desde que arrancó la aplicación. Solo cifras y un código.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ContinuousMailbox {
    /// Identificador de la cuenta.
    pub account: String,
    pub label: String,
    pub address: Option<String>,
    /// El ajuste global está activado.
    pub enabled: bool,
    /// Fin de la última pasada (ms desde 1970); `None` si aún no hubo ninguna.
    pub last_pass_at: Option<i64>,
    /// Tareas cuya consulta a Enviados llegó a contestar.
    pub checked: u64,
    /// Respuestas propias válidas halladas (dirigidas al remitente).
    pub observed: u64,
    /// Filas `learned` nuevas.
    pub learned: u64,
    /// Tareas en revisión cerradas como «Respondido fuera de NestAdmin».
    pub closed: u64,
    /// Código del último fallo de la última pasada (`unavailable`, `unauthorized`,
    /// `no_sent_folder`, `signed_out`…); `None` si la última pasada fue bien.
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ContinuousOverview {
    pub enabled: bool,
    pub window_days: i64,
    pub max_per_pass: usize,
    pub every_minutes: u64,
    pub mailboxes: Vec<ContinuousMailbox>,
}

/// Qué hizo una llamada al aprendizaje continuo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContinuousStep {
    /// Nada que hacer ahora (desactivado, fuera de la puerta de tiempo o sin candidatas).
    Idle,
    /// Trabajó, pero no cambió nada (sin respuesta, respuesta a otro destinatario, tarea ya
    /// cambiada…).
    Progress,
    /// Cerró una tarea o insertó evidencia: la ventana debe refrescarse.
    Changed,
}

// ---- Estado en memoria ----

#[derive(Default)]
struct Stats {
    last_pass_at: Option<i64>,
    checked: u64,
    observed: u64,
    learned: u64,
    closed: u64,
    last_error: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// Espera revisión: se consulta siempre y, si hay respuesta, se cierra.
    Review,
    /// Archivada por la persona: solo se consulta si podría dar evidencia.
    Archived,
}

/// Una tarea por comprobar. Sin `Debug`: lleva la dirección del remitente.
struct Candidate {
    task_id: i64,
    /// `updated_at` al planificar: si cambia, la tarea ya no es la que se planificó.
    updated_at: i64,
    kind: Kind,
    /// Id del mensaje sin el prefijo de la cuenta, para el conector.
    inner_id: String,
    thread_key: String,
    address: String,
    /// Instante del recibido (ms) con el que se compara un envío de NestAdmin en el hilo.
    received_ms: i64,
    /// La fila `learned` que daría, si todas las condiciones locales se cumplen.
    evidence: Option<LearnedSignal>,
}

/// Una pasada en curso. Sin `Debug` por la misma razón.
struct Pass {
    account: String,
    queue: VecDeque<Candidate>,
    changed: bool,
}

#[derive(Default)]
pub(crate) struct ContinuousRuntime {
    /// Pasadas en curso por buzón (clave de la fuente).
    passes: HashMap<String, Pass>,
    /// Cuándo empezó la última pasada de cada cuenta (la puerta de 15 minutos).
    last_started: HashMap<String, Instant>,
    /// Cuentas cuya próxima pasada se adelanta (una consulta de correo correcta).
    advanced: HashSet<String>,
    /// Orden de la última comprobación de cada tarea: la que lleva más sin comprobarse va antes.
    checked_order: HashMap<i64, u64>,
    counter: u64,
    stats: HashMap<String, Stats>,
}

// ---- Fechas ----

/// Milisegundos desde 1970 de una fecha `AAAA-MM-DDTHH:MM:SS[.fff](Z|±HH:MM)` (UTC por defecto).
/// `None` si no se puede leer.
fn date_ms(date: &str) -> Option<i64> {
    let date = date.trim();
    let day = day_of(date)?;
    let rest = date.get(10..)?;
    let rest = rest.strip_prefix(['T', ' ']).unwrap_or(rest);
    let number = |text: Option<&str>| -> Option<i64> { text?.parse().ok() };
    let clock = rest.get(..8).unwrap_or("00:00:00");
    let mut parts = clock.split(':');
    let (h, m, s) = (
        number(parts.next())?,
        number(parts.next())?,
        number(parts.next())?,
    );
    if !(0..24).contains(&h) || !(0..60).contains(&m) || !(0..61).contains(&s) {
        return None;
    }
    let tail = rest.get(8..).unwrap_or("");
    let tail = tail.trim_start_matches(|c: char| c == '.' || c.is_ascii_digit());
    let offset_minutes = match tail.chars().next() {
        Some(sign @ ('+' | '-')) => {
            let digits: String = tail[1..].chars().filter(char::is_ascii_digit).collect();
            let (hh, mm) = (digits.get(..2)?, digits.get(2..4).unwrap_or("00"));
            let minutes = hh.parse::<i64>().ok()? * 60 + mm.parse::<i64>().ok()?;
            if sign == '-' {
                -minutes
            } else {
                minutes
            }
        }
        _ => 0,
    };
    Some(day * 86_400_000 + h * 3_600_000 + m * 60_000 + s * 1_000 - offset_minutes * 60_000)
}

// ---- Candidatas ----

/// Las condiciones locales de la evidencia `learned` de un resultado (D178), sin consultar nada.
/// `None` si alguna falla.
fn evidence_of(result: &Value, message: &Message, source: &str) -> Option<LearnedSignal> {
    if result["needs_reply"].as_bool() != Some(true) {
        return None;
    }
    let category = result["category"].as_str()?.trim();
    if !CATEGORIES.contains(&category) || REVIEW_ONLY_CATEGORIES.contains(&category) {
        return None;
    }
    // Señales comprobadas, sin señal fuerte ni rebote: el filtro de D167.
    let from = message.from.as_deref()?;
    let codes = message.signals.as_deref()?;
    let (_, signals) = route_of_message(Some(from), Some(codes));
    if decide(&signals, None, None).route != Route::Ia {
        return None;
    }
    // Una sola dirección.
    if from.matches('@').count() != 1 {
        return None;
    }
    let (address, _) = sender_parts(from)?;
    if !is_email_address(&address) || message.id.is_empty() {
        return None;
    }
    Some(LearnedSignal {
        subject_key: format!("{category}|{address}"),
        source_key: format!("learned|{source}|{}", message.id),
    })
}

fn is_closed_outside(result: &Value) -> bool {
    result["closed"]["reason"] == REPLIED_OUTSIDE
}

impl App {
    fn continuous_enabled_now(&self) -> bool {
        // Sin valor guardado está activado; solo un `false` explícito lo apaga.
        !matches!(
            self.store.rule_setting(SETTING_KEY),
            Ok(Some(value)) if value == "false"
        )
    }

    /// Si el aprendizaje continuo está activado (lo está por defecto).
    pub fn continuous_learning_enabled(&self) -> bool {
        self.continuous_enabled_now()
    }

    /// Activa o desactiva el aprendizaje continuo (D178). Cada cambio queda en el Historial, sin
    /// contenido. Al desactivarlo se descartan las pasadas en curso.
    pub fn set_continuous_learning(&mut self, enabled: bool) -> AppResult<ContinuousOverview> {
        let before = self.continuous_enabled_now();
        if before != enabled {
            let name = |on: bool| if on { "activado" } else { "desactivado" };
            self.store.set_rule_setting(
                SETTING_KEY,
                if enabled { "true" } else { "false" },
                &format!(
                    "aprendizaje continuo de las respuestas hechas fuera: {} -> {}",
                    name(before),
                    name(enabled)
                ),
            )?;
        }
        if !enabled {
            self.continuous.passes.clear();
        }
        self.continuous_learning_overview()
    }

    /// Resumen por buzón: solo recuentos y códigos.
    pub fn continuous_learning_overview(&self) -> AppResult<ContinuousOverview> {
        let enabled = self.continuous_enabled_now();
        let mailboxes = self
            .mail_accounts()
            .accounts
            .into_iter()
            .map(|a| {
                let stats = self
                    .account_read_source(&a.id)
                    .and_then(|source| self.continuous.stats.get(&source));
                ContinuousMailbox {
                    account: a.id,
                    label: a.label,
                    address: a.address,
                    enabled,
                    last_pass_at: stats.and_then(|s| s.last_pass_at),
                    checked: stats.map_or(0, |s| s.checked),
                    observed: stats.map_or(0, |s| s.observed),
                    learned: stats.map_or(0, |s| s.learned),
                    closed: stats.map_or(0, |s| s.closed),
                    last_error: stats.and_then(|s| s.last_error.clone()),
                }
            })
            .collect();
        Ok(ContinuousOverview {
            enabled,
            window_days: CONTINUOUS_WINDOW_DAYS,
            max_per_pass: CONTINUOUS_MAX_PER_PASS,
            every_minutes: CONTINUOUS_EVERY.as_secs() / 60,
            mailboxes,
        })
    }

    /// Una consulta de correo correcta adelanta la siguiente pasada de la cuenta (D178).
    pub(crate) fn continuous_advance(&mut self, account: &str) {
        self.continuous.advanced.insert(account.to_owned());
    }

    /// Un paso del aprendizaje continuo para el hilo en reposo: con la puerta de 15 minutos por
    /// buzón, empieza una pasada o avanza la que haya un solo paso (una consulta a Enviados). No
    /// llama nunca a la IA. Sin la cola en pausa y con el ajuste activado.
    pub fn continuous_learning_tick(&mut self) -> AppResult<ContinuousStep> {
        if self.paused || !self.continuous_enabled_now() {
            return Ok(ContinuousStep::Idle);
        }
        // Una pasada en curso avanza antes de empezar otra.
        let running: Option<String> = self
            .continuous
            .passes
            .values()
            .next()
            .map(|p| p.account.clone());
        if let Some(account) = running {
            return self.step_continuous_pass(&account);
        }
        let now = Instant::now();
        let due: Option<String> = self
            .mail_accounts()
            .accounts
            .into_iter()
            .map(|a| a.id)
            .find(|id| {
                self.continuous.advanced.contains(id)
                    || self
                        .continuous
                        .last_started
                        .get(id)
                        .is_none_or(|t| now.duration_since(*t) >= CONTINUOUS_EVERY)
            });
        let Some(account) = due else {
            return Ok(ContinuousStep::Idle);
        };
        if self.begin_continuous_pass(&account)? == 0 {
            return Ok(ContinuousStep::Idle);
        }
        Ok(ContinuousStep::Progress)
    }

    /// Una pasada completa de un buzón, sin la puerta de tiempo: para pruebas y para quien no
    /// necesita soltar el cerrojo entre pasos. Devuelve `Changed` si cerró alguna tarea o insertó
    /// evidencia.
    pub fn continuous_learning_pass(&mut self, account: &str) -> AppResult<ContinuousStep> {
        let mut result = ContinuousStep::Idle;
        if self.begin_continuous_pass(account)? > 0 {
            result = ContinuousStep::Progress;
            loop {
                match self.step_continuous_pass(account)? {
                    ContinuousStep::Changed => result = ContinuousStep::Changed,
                    ContinuousStep::Progress => {}
                    ContinuousStep::Idle => break,
                }
            }
        }
        Ok(result)
    }

    /// Planifica la pasada de una cuenta (solo lee la base de datos; no consulta nada) y devuelve
    /// cuántas tareas hay que comprobar. Con el ajuste desactivado, sin credenciales, con la cuenta
    /// desconectada o con un conector sin Enviados devuelve 0 sin consultar nada y deja el motivo
    /// en el resumen.
    pub fn begin_continuous_pass(&mut self, account: &str) -> AppResult<usize> {
        self.continuous.advanced.remove(account);
        if !self.continuous_enabled_now() {
            return Ok(0);
        }
        let Some(source) = self.account_read_source(account) else {
            return Ok(0);
        };
        self.continuous
            .last_started
            .insert(account.to_owned(), Instant::now());
        self.continuous.passes.remove(&source);
        let status = self
            .mail_accounts()
            .accounts
            .into_iter()
            .find(|a| a.id == account)
            .map(|a| a.status);
        let refusal = match status {
            Some(AccountStatus::Connected) => None,
            Some(AccountStatus::SignedOut) => Some("signed_out"),
            Some(AccountStatus::NotConfigured) => Some("not_configured"),
            Some(AccountStatus::CredentialsUnavailable) => Some("credentials_unavailable"),
            None => return Ok(0),
        };
        if let Some(code) = refusal {
            self.finish_pass(&source, Some(code.to_owned()));
            return Ok(0);
        }
        match self.mail.supports_sent(account) {
            Ok(true) => {}
            Ok(false) => {
                self.finish_pass(&source, Some("no_sent_folder".to_owned()));
                return Ok(0);
            }
            Err(e) => {
                self.finish_pass(&source, Some(e.code));
                return Ok(0);
            }
        }
        let queue = self.plan_continuous(account, &source)?;
        if queue.is_empty() {
            self.finish_pass(&source, None);
            return Ok(0);
        }
        let count = queue.len();
        self.continuous.passes.insert(
            source,
            Pass {
                account: account.to_owned(),
                queue,
                changed: false,
            },
        );
        Ok(count)
    }

    fn finish_pass(&mut self, source: &str, error: Option<String>) {
        let stats = self.continuous.stats.entry(source.to_owned()).or_default();
        stats.last_pass_at = Some(now_ms());
        stats.last_error = error;
    }

    /// Las tareas de la pasada, con su orden y el tope (D178). Solo lee la base de datos.
    fn plan_continuous(&self, account: &str, source: &str) -> AppResult<VecDeque<Candidate>> {
        let today = today_day();
        let mut seen: HashSet<String> = HashSet::new();
        let mut review: Vec<Candidate> = Vec::new();
        let mut archived: Vec<Candidate> = Vec::new();
        // `all_tasks` va de la más reciente a la más antigua; un mensaje con dos tareas (una
        // redacción pedida) se comprueba una sola vez, con la que espera revisión si la hay.
        let mut tasks = self.store.all_tasks()?;
        tasks.sort_by_key(|t| {
            (
                t.state != TaskState::AwaitingReview,
                std::cmp::Reverse(t.id),
            )
        });
        for task in tasks {
            let Some((candidate, message_id)) = self.candidate_of(&task, account, source, today)?
            else {
                continue;
            };
            if !seen.insert(message_id) {
                continue;
            }
            match candidate.kind {
                Kind::Review => review.push(candidate),
                Kind::Archived => archived.push(candidate),
            }
        }
        let order = |list: &mut Vec<Candidate>, runtime: &ContinuousRuntime| {
            list.sort_by_key(|c| {
                (
                    runtime.checked_order.get(&c.task_id).copied().unwrap_or(0),
                    c.task_id,
                )
            });
        };
        order(&mut review, &self.continuous);
        order(&mut archived, &self.continuous);
        Ok(review
            .into_iter()
            .chain(archived)
            .take(CONTINUOUS_MAX_PER_PASS)
            .collect())
    }

    /// La tarea como candidata de esta pasada, con el id completo de su mensaje, si lo es.
    fn candidate_of(
        &self,
        task: &Task,
        account: &str,
        source: &str,
        today: i64,
    ) -> AppResult<Option<(Candidate, String)>> {
        if task.kind != EMAIL {
            return Ok(None);
        }
        let Some(result) = task.result.as_ref() else {
            return Ok(None);
        };
        let kind = match task.state {
            TaskState::AwaitingReview => Kind::Review,
            TaskState::Completed if result["archived"] == json!(true) => Kind::Archived,
            _ => return Ok(None),
        };
        if is_closed_outside(result) {
            return Ok(None);
        }
        let Ok(message) = serde_json::from_value::<Message>(result["message"].clone()) else {
            return Ok(None);
        };
        let (message_account, inner) = split_id(&message.id);
        let Some(thread_key) = message
            .thread_key
            .as_deref()
            .map(str::trim)
            .filter(|k| !k.is_empty())
        else {
            return Ok(None);
        };
        if message_account != account || inner.is_empty() {
            return Ok(None);
        }
        // Ventana: la fecha del mensaje y, si falta o no se lee, la de creación de la tarea.
        let received_ms = message
            .date
            .as_deref()
            .and_then(date_ms)
            .unwrap_or(task.created_at);
        let day = message
            .date
            .as_deref()
            .and_then(day_of)
            .unwrap_or(task.created_at.div_euclid(86_400_000));
        if today - day > CONTINUOUS_WINDOW_DAYS {
            return Ok(None);
        }
        let evidence = evidence_of(result, &message, source);
        match kind {
            Kind::Review => {
                // Una tarea que ya intentó enviar no se cierra: se confirma o se descarta (como
                // en Archivar) y su respuesta en Enviados puede ser la de NestAdmin.
                if result.get("attempt").is_some() || !self.store.task_actions(task.id)?.is_empty()
                {
                    return Ok(None);
                }
            }
            Kind::Archived => {
                // Las cerradas por el triaje local no las archivó la persona.
                if result.get("triage").is_some_and(|t| !t.is_null()) {
                    return Ok(None);
                }
                // Solo si puede dar evidencia y no la dio ya (primera carga o pasada anterior).
                let Some(signal) = evidence.as_ref() else {
                    return Ok(None);
                };
                if self.store.learned_source_exists(&signal.source_key)?
                    || self.store.task_has_signal(task.id, "rejected")?
                {
                    return Ok(None);
                }
            }
        }
        let Some((address, _)) = message.from.as_deref().and_then(sender_parts) else {
            return Ok(None);
        };
        Ok(Some((
            Candidate {
                task_id: task.id,
                updated_at: task.updated_at,
                kind,
                inner_id: inner.to_owned(),
                thread_key: thread_key.to_owned(),
                address,
                received_ms,
                evidence,
            },
            message.id.clone(),
        )))
    }

    /// Un paso de la pasada de una cuenta: una consulta a Enviados (o descartar una tarea que ya
    /// cambió). `Idle` cuando la pasada terminó o no había ninguna.
    pub fn step_continuous_pass(&mut self, account: &str) -> AppResult<ContinuousStep> {
        let Some(source) = self.account_read_source(account) else {
            return Ok(ContinuousStep::Idle);
        };
        if !self.continuous_enabled_now() {
            self.continuous.passes.remove(&source);
            return Ok(ContinuousStep::Idle);
        }
        let Some(mut pass) = self.continuous.passes.remove(&source) else {
            return Ok(ContinuousStep::Idle);
        };
        let Some(candidate) = pass.queue.pop_front() else {
            self.finish_pass(&source, None);
            return Ok(ContinuousStep::Idle);
        };
        match self.check_candidate(&source, &pass.account, &candidate) {
            Ok(changed) => {
                pass.changed |= changed;
                self.continuous.passes.insert(source, pass);
                Ok(if changed {
                    ContinuousStep::Changed
                } else {
                    ContinuousStep::Progress
                })
            }
            Err(CheckFailure::Connector(code)) => {
                // Un fallo del conector detiene la pasada de este buzón sin escribir nada más.
                self.finish_pass(&source, Some(code));
                Ok(ContinuousStep::Idle)
            }
            Err(CheckFailure::Store(error)) => {
                self.finish_pass(&source, Some("store_error".to_owned()));
                Err(error)
            }
        }
    }

    /// Comprueba una tarea. `Ok(true)` si cambió algo (cerró la tarea o insertó evidencia).
    fn check_candidate(
        &mut self,
        source: &str,
        account: &str,
        candidate: &Candidate,
    ) -> Result<bool, CheckFailure> {
        // La tarea debe seguir como estaba al planificar: otra persona o el hilo de la ventana
        // pudieron aprobarla, archivarla, editarla o rechazarla entre un paso y el siguiente.
        let Some(task) = self
            .store
            .task_by_id(candidate.task_id)
            .map_err(store_failure)?
        else {
            return Ok(false);
        };
        let still = match candidate.kind {
            Kind::Review => task.state == TaskState::AwaitingReview,
            Kind::Archived => {
                task.state == TaskState::Completed
                    && task
                        .result
                        .as_ref()
                        .is_some_and(|r| r["archived"] == json!(true))
            }
        };
        if !still
            || task.updated_at != candidate.updated_at
            || task.result.as_ref().is_some_and(is_closed_outside)
        {
            return Ok(false);
        }
        let reply = self
            .mail
            .own_reply_in_thread(account, &candidate.thread_key, &candidate.inner_id)
            .map_err(|e| CheckFailure::Connector(e.code))?;
        self.continuous.counter += 1;
        let order = self.continuous.counter;
        self.continuous
            .checked_order
            .insert(candidate.task_id, order);
        let stats = self.continuous.stats.entry(source.to_owned()).or_default();
        stats.checked += 1;
        let Some(reply) = reply else {
            return Ok(false);
        };
        // Solo cuenta una respuesta dirigida al remitente. Del conector solo se usan el destino y
        // la fecha: el cuerpo se descarta aquí mismo.
        let replied_at = reply.date.clone();
        let valid = replies_to(&reply.to, &candidate.address);
        drop(reply);
        if !valid {
            return Ok(false);
        }
        stats.observed += 1;

        // ¿Pudo ser la respuesta de NestAdmin (confirmada o en curso, posterior al recibido)? Si
        // es así ya cuenta como `approved`: se prefiere no contar a contar dos veces.
        let evidence = match &candidate.evidence {
            None => None,
            Some(signal) => {
                let sends = self
                    .store
                    .thread_send_times(account, &candidate.thread_key)
                    .map_err(store_failure)?;
                (!sends.iter().any(|at| *at > candidate.received_ms)).then_some(signal)
            }
        };
        match candidate.kind {
            Kind::Review => {
                let mut result = task.result.clone().unwrap_or(Value::Null);
                if let Some(fields) = result.as_object_mut() {
                    fields.remove("action");
                    fields.remove("recovery");
                }
                result["archived"] = json!(true);
                result["closed"] = json!({ "reason": REPLIED_OUTSIDE, "replied_at": replied_at });
                let closed = self
                    .store
                    .close_replied_outside(task.id, candidate.updated_at, &result, evidence)
                    .map_err(store_failure)?;
                let stats = self.continuous.stats.entry(source.to_owned()).or_default();
                Ok(match closed {
                    Some((_, inserted)) => {
                        stats.closed += 1;
                        stats.learned += u64::from(inserted);
                        true
                    }
                    // Cambió justo antes de escribir: no se cierra ni se cuenta nada.
                    None => false,
                })
            }
            Kind::Archived => {
                let Some(signal) = evidence else {
                    return Ok(false);
                };
                let inserted = self
                    .store
                    .insert_learned_outside(task.id, signal)
                    .map_err(store_failure)?;
                let stats = self.continuous.stats.entry(source.to_owned()).or_default();
                stats.learned += u64::from(inserted);
                Ok(inserted)
            }
        }
    }
}

enum CheckFailure {
    Connector(String),
    Store(crate::AppError),
}

fn store_failure(error: nestadmin_core::CoreError) -> CheckFailure {
    CheckFailure::Store(error.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(from: &str, signals: Option<&[&str]>) -> Message {
        Message {
            id: "simulador:m1".into(),
            subject: None,
            from: Some(from.into()),
            to: None,
            body: None,
            date: None,
            attachments: Vec::new(),
            signals: signals.map(|s| s.iter().map(|c| (*c).to_owned()).collect()),
            thread_key: Some("t1".into()),
        }
    }

    fn result(needs_reply: Value, category: &str) -> Value {
        json!({ "needs_reply": needs_reply, "category": category })
    }

    fn evidence(result: &Value, message: &Message) -> Option<(String, String)> {
        evidence_of(result, message, "account:simulador:a@b.test")
            .map(|s| (s.subject_key, s.source_key))
    }

    #[test]
    fn the_evidence_needs_every_local_condition() {
        let ok = message("Ana <Ana@Alfa.test>", Some(&[]));
        assert_eq!(
            evidence(&result(json!(true), "consulta_pago"), &ok),
            Some((
                "consulta_pago|ana@alfa.test".into(),
                "learned|account:simulador:a@b.test|simulador:m1".into()
            ))
        );
        // `needs_reply` falso, ausente o no booleano.
        for needs in [json!(false), Value::Null, json!("true")] {
            assert!(evidence(&result(needs, "consulta_pago"), &ok).is_none());
        }
        // Categorías de solo revisión, fuera del catálogo o ausente.
        for category in ["otros", "reclamacion", "inventada", ""] {
            assert!(
                evidence(&result(json!(true), category), &ok).is_none(),
                "{category}"
            );
        }
        assert!(evidence_of(&json!({ "needs_reply": true }), &ok, "s").is_none());
        // Señales sin comprobar, una señal fuerte o un rebote.
        let unchecked = message("ana@alfa.test", None);
        assert!(evidence(&result(json!(true), "cita"), &unchecked).is_none());
        let bounce = message("mailer-daemon@alfa.test", Some(&[]));
        assert!(evidence(&result(json!(true), "cita"), &bounce).is_none());
        let noreply = message("no-reply@alfa.test", Some(&[]));
        assert!(evidence(&result(json!(true), "cita"), &noreply).is_none());
        // Más de una dirección en el remitente o sin dirección legible.
        let two = message("ana@alfa.test, luis@alfa.test", Some(&[]));
        assert!(evidence(&result(json!(true), "cita"), &two).is_none());
        let none = message("Ana Alfa", Some(&[]));
        assert!(evidence(&result(json!(true), "cita"), &none).is_none());
    }

    #[test]
    fn a_strong_signal_code_is_never_evidence() {
        // `Auto-Submitted` es una señal fuerte: saca el correo de la ruta de la IA. Una señal débil
        // sola (`List-Unsubscribe`) no lo hace, así que no quita la evidencia.
        let codes = nestadmin_core::mail_signals::codes(&nestadmin_core::mail_signals::detect(
            Some("ana@alfa.test"),
            Some(&[("Auto-Submitted", "auto-generated")]),
        ));
        assert!(!codes.is_empty());
        let strong = message(
            "ana@alfa.test",
            Some(&codes.iter().map(String::as_str).collect::<Vec<_>>()),
        );
        assert!(evidence(&result(json!(true), "cita"), &strong).is_none());
        let weak = nestadmin_core::mail_signals::codes(&nestadmin_core::mail_signals::detect(
            Some("ana@alfa.test"),
            Some(&[("List-Unsubscribe", "<mailto:baja@alfa.test>")]),
        ));
        let weak = message(
            "ana@alfa.test",
            Some(&weak.iter().map(String::as_str).collect::<Vec<_>>()),
        );
        assert!(evidence(&result(json!(true), "cita"), &weak).is_some());
    }

    #[test]
    fn dates_are_read_with_their_time_and_offset() {
        assert_eq!(date_ms("1970-01-02T00:00:00Z"), Some(86_400_000));
        assert_eq!(date_ms("1970-01-01T01:00:00+01:00"), Some(0));
        assert_eq!(date_ms("1970-01-01T00:00:00.250Z"), Some(0));
        assert_eq!(date_ms("1970-01-01"), Some(0));
        assert!(date_ms("1970-01-01T10:00:00Z") < date_ms("1970-01-01T12:00:00Z"));
        assert_eq!(date_ms("no es una fecha"), None);
    }
}
