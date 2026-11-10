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
//! dejaron en la tarea. Solo se piden **metadatos** a Enviados (`own_replies_meta`: id, fecha y
//! destinatarios; D162): ningún cuerpo llega al equipo por esta vía, así que nada de eso se guarda,
//! se registra ni sale hacia ninguna IA. El resumen lleva solo recuentos y códigos.
//!
//! # Respuestas de NestAdmin
//!
//! Si en el hilo hay un envío de NestAdmin (`pending`, `failed` sin resolver o `confirmed`)
//! posterior al recibido, la tarea no se consulta, no se cierra y no da evidencia: lo que se vería
//! en Enviados puede ser esa respuesta, a otro correo del hilo. Se decide en local, al planificar
//! y otra vez justo antes de consultar.
//!
//! # Tareas sin hilo
//!
//! Las tareas creadas antes de que el resultado guardara `message.thread_key` entran como candidatas
//! con el hilo pendiente y se ordenan como las demás. Su paso pide el hilo con `thread_of`
//! (solo metadatos), lo guarda con una escritura que solo ocurre si la tarea no cambió
//! ([`nestadmin_core::Store::record_recovered_thread`]) y sigue como cualquier otra. Cada
//! consulta, de hilo o de Enviados, descuenta del tope de la pasada.
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
//! nada más y deja su código en el resumen (`last_error`); se reintenta en la siguiente pasada. Un
//! error propio de una tarea (`not_found`, `invalid_input`) la salta y la pasada sigue: la tarea se
//! anota como comprobada antes de consultar. Con el ajuste desactivado, sin credenciales o con la
//! cuenta desconectada no se hace ninguna consulta.
//!
//! # Por cuenta
//!
//! Las pasadas y los recuentos se guardan por identificador de cuenta. Si al dar un paso la cuenta
//! ya no existe o su fuente cambió, la pasada se descarta.

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
/// Separación habitual entre dos pasadas de un mismo buzón.
pub const CONTINUOUS_EVERY: Duration = Duration::from_secs(15 * 60);
/// Separación mínima aunque «Consultar correo» adelante la pasada.
pub const CONTINUOUS_MIN_GAP: Duration = Duration::from_secs(2 * 60);

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
    /// Fuente con la que se contó: si la cuenta cambia de fuente, los recuentos empiezan de cero.
    source: String,
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
    /// Id completo del mensaje (con la cuenta): la clave de lo ya consultado en esta sesión.
    message_id: String,
    /// Hilo del resultado; vacío mientras `thread_pending` (se recupera en su paso).
    thread_key: String,
    /// El resultado no lleva `message.thread_key` (tarea anterior a ese dato): el primer paso
    /// pide el hilo al conector con `thread_of` (D178).
    thread_pending: bool,
    /// El hilo se recuperó con `thread_of` (en una pasada anterior o en esta): las comprobaciones
    /// de envíos de NestAdmin también miran las tareas sin hilo guardado.
    recovered: bool,
    address: String,
    /// Instante del recibido (ms) con el que se compara un envío de NestAdmin en el hilo.
    received_ms: i64,
    /// La fila `learned` que daría, si todas las condiciones locales se cumplen.
    evidence: Option<LearnedSignal>,
}

/// Una pasada en curso. Sin `Debug` por la misma razón.
struct Pass {
    /// Fuente de la cuenta al planificar: si cambia, la pasada se descarta.
    source: String,
    queue: VecDeque<Candidate>,
    /// Consultas al conector que quedan en esta pasada (`thread_of` y `own_replies_meta` cuentan
    /// una cada una).
    budget: usize,
}

#[derive(Default)]
pub(crate) struct ContinuousRuntime {
    /// Pasadas en curso por identificador de cuenta.
    passes: HashMap<String, Pass>,
    /// Cuándo empezó la última pasada de cada cuenta (la puerta de 15 minutos).
    last_started: HashMap<String, Instant>,
    /// Cuentas cuya próxima pasada se adelanta (una consulta de correo correcta).
    advanced: HashSet<String>,
    /// Orden de la última comprobación de cada tarea: la que lleva más sin comprobarse va antes.
    checked_order: HashMap<i64, u64>,
    /// Mensajes (id completo) cuyo hilo no se pudo recuperar (`not_found` o sin hilo): no se
    /// vuelven a pedir en esta sesión (D178).
    no_thread: HashSet<String>,
    counter: u64,
    /// Recuentos por identificador de cuenta.
    stats: HashMap<String, Stats>,
}

impl ContinuousRuntime {
    fn stats_mut(&mut self, account: &str, source: &str) -> &mut Stats {
        let stats = self.stats.entry(account.to_owned()).or_default();
        if stats.source != source {
            *stats = Stats {
                source: source.to_owned(),
                ..Stats::default()
            };
        }
        stats
    }
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
    // Un borrador editado dice que el borrador no servía (como un rechazo, D172).
    if result["edited"] == json!(true) {
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
    // Una sola dirección (analizadas, no contando arrobas).
    if !is_single_address(from) {
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

/// Si el remitente es una sola dirección: se separa por comas y punto y coma fuera de comillas y
/// de ángulos y exactamente un trozo lleva una dirección legible. `"ana@x.test <ana@x.test>"` es
/// una; `Ana <a@x.test>, Luis <l@x.test>` son dos.
fn is_single_address(from: &str) -> bool {
    let mut parts: Vec<String> = vec![String::new()];
    let (mut quoted, mut angle) = (false, false);
    for c in from.chars() {
        match c {
            '"' => quoted = !quoted,
            '<' if !quoted => angle = true,
            '>' if !quoted => angle = false,
            ',' | ';' if !quoted && !angle => {
                parts.push(String::new());
                continue;
            }
            _ => {}
        }
        if let Some(last) = parts.last_mut() {
            last.push(c);
        }
    }
    let addresses = parts
        .iter()
        .filter(|p| p.contains('@'))
        .map(|p| sender_parts(p).map(|(a, _)| a))
        .collect::<Vec<_>>();
    matches!(addresses.as_slice(), [Some(_)])
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
                // Los recuentos son de la fuente actual de la cuenta.
                let source = self.account_read_source(&a.id);
                let stats = self
                    .continuous
                    .stats
                    .get(&a.id)
                    .filter(|s| Some(&s.source) == source.as_ref());
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

    /// Una consulta de correo correcta adelanta la siguiente pasada de la cuenta (D178), con al
    /// menos [`CONTINUOUS_MIN_GAP`] desde la anterior.
    pub(crate) fn continuous_advance(&mut self, account: &str) {
        self.continuous.advanced.insert(account.to_owned());
    }

    /// Descarta las pasadas cuya cuenta ya no existe o cambió de fuente.
    fn drop_orphan_passes(&mut self) {
        let orphans: Vec<String> = self
            .continuous
            .passes
            .iter()
            .filter(|(account, pass)| {
                self.account_read_source(account).as_deref() != Some(pass.source.as_str())
            })
            .map(|(account, _)| account.clone())
            .collect();
        for account in orphans {
            self.continuous.passes.remove(&account);
        }
    }

    /// Un paso del aprendizaje continuo para el hilo en reposo: con la puerta de 15 minutos por
    /// buzón (2 como mínimo si «Consultar correo» la adelanta), empieza una pasada o avanza la que
    /// haya un solo paso (una consulta a Enviados). No llama nunca a la IA. Sin la cola en pausa y
    /// con el ajuste activado.
    pub fn continuous_learning_tick(&mut self) -> AppResult<ContinuousStep> {
        self.continuous_learning_tick_at(Instant::now())
    }

    /// Como [`App::continuous_learning_tick`] con el instante de «ahora» dado (para probar la
    /// puerta de tiempo sin esperar).
    pub fn continuous_learning_tick_at(&mut self, now: Instant) -> AppResult<ContinuousStep> {
        if self.paused || !self.continuous_enabled_now() {
            return Ok(ContinuousStep::Idle);
        }
        // Una pasada en curso avanza antes de empezar otra; una huérfana se descarta.
        self.drop_orphan_passes();
        if let Some(account) = self.continuous.passes.keys().next().cloned() {
            return self.step_continuous_pass(&account);
        }
        let due: Option<String> = self
            .mail_accounts()
            .accounts
            .into_iter()
            .map(|a| a.id)
            .find(|id| match self.continuous.last_started.get(id) {
                None => true,
                Some(t) => {
                    let since = now.duration_since(*t);
                    since >= CONTINUOUS_EVERY
                        || (self.continuous.advanced.contains(id) && since >= CONTINUOUS_MIN_GAP)
                }
            });
        let Some(account) = due else {
            return Ok(ContinuousStep::Idle);
        };
        if self.begin_continuous_pass_at(&account, now)? == 0 {
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
        self.begin_continuous_pass_at(account, Instant::now())
    }

    fn begin_continuous_pass_at(&mut self, account: &str, now: Instant) -> AppResult<usize> {
        self.continuous.advanced.remove(account);
        if !self.continuous_enabled_now() {
            return Ok(0);
        }
        let Some(source) = self.account_read_source(account) else {
            return Ok(0);
        };
        self.continuous.last_started.insert(account.to_owned(), now);
        self.continuous.passes.remove(account);
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
            self.finish_pass(account, &source, Some(code.to_owned()));
            return Ok(0);
        }
        match self.mail.supports_sent(account) {
            Ok(true) => {}
            Ok(false) => {
                self.finish_pass(account, &source, Some("no_sent_folder".to_owned()));
                return Ok(0);
            }
            Err(e) => {
                self.finish_pass(account, &source, Some(e.code));
                return Ok(0);
            }
        }
        let queue = self.plan_continuous(account, &source)?;
        if queue.is_empty() {
            self.finish_pass(account, &source, None);
            return Ok(0);
        }
        let count = queue.len();
        self.continuous.passes.insert(
            account.to_owned(),
            Pass {
                source,
                queue,
                budget: CONTINUOUS_MAX_PER_PASS,
            },
        );
        Ok(count)
    }

    fn finish_pass(&mut self, account: &str, source: &str, error: Option<String>) {
        let stats = self.continuous.stats_mut(account, source);
        stats.last_pass_at = Some(now_ms());
        stats.last_error = error;
    }

    /// Las tareas de la pasada, con su orden y el tope (D178). Solo lee la base de datos.
    fn plan_continuous(&self, account: &str, source: &str) -> AppResult<VecDeque<Candidate>> {
        let today = today_day();
        let mut seen: HashSet<String> = HashSet::new();
        // Primero las que esperan revisión y después las archivadas; dentro de cada grupo, sin
        // mirar si tienen hilo, las no comprobadas en esta sesión y luego las que llevan más
        // tiempo sin comprobarse (D178).
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

    /// Si NestAdmin tiene en ese hilo (misma cuenta) un envío confirmado, en curso o sin resolver
    /// posterior al recibido (D178): la respuesta que se vería en Enviados puede ser la suya.
    /// Con `recovered` (el hilo no estaba en el resultado), falla cerrado: también cuenta un
    /// envío de una tarea de la cuenta que sigue sin hilo guardado, que podría ser de este mismo
    /// hilo sin que se sepa.
    fn nestadmin_sent_after(
        &self,
        account: &str,
        thread_key: &str,
        received_ms: i64,
        recovered: bool,
    ) -> AppResult<bool> {
        let in_thread = self.store.thread_send_times(account, thread_key)?;
        let unknown = if recovered {
            self.store.unthreaded_send_times(account)?
        } else {
            Vec::new()
        };
        Ok(in_thread
            .iter()
            .chain(unknown.iter())
            .any(|at| *at > received_ms))
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
        if message_account != account || inner.is_empty() {
            return Ok(None);
        }
        // Sin `thread_key` (tarea anterior a ese dato) el hilo se pide en su paso (D178), salvo
        // que ya se haya visto que no se puede recuperar.
        let thread_key = message
            .thread_key
            .as_deref()
            .map(str::trim)
            .filter(|k| !k.is_empty());
        let thread_pending = thread_key.is_none();
        if thread_pending && self.continuous.no_thread.contains(&message.id) {
            return Ok(None);
        }
        let thread_key = thread_key.unwrap_or("");
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
        // Un envío de NestAdmin posterior en el hilo: ni se consulta, ni se cierra, ni da evidencia.
        // Sin hilo todavía no se puede saber: se vuelve a comprobar al recuperarlo.
        let recovered = result["message"]["thread_recovered"] == json!(true);
        if !thread_pending
            && self.nestadmin_sent_after(account, thread_key, received_ms, recovered)?
        {
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
                message_id: message.id.clone(),
                thread_key: thread_key.to_owned(),
                thread_pending,
                recovered,
                address,
                received_ms,
                evidence,
            },
            message.id.clone(),
        )))
    }

    /// Un paso de la pasada de una cuenta: una consulta a Enviados (o descartar una tarea que ya
    /// cambió). `Idle` cuando la pasada terminó o no había ninguna. Una pasada cuya cuenta ya no
    /// existe o cambió de fuente se descarta.
    pub fn step_continuous_pass(&mut self, account: &str) -> AppResult<ContinuousStep> {
        let Some(mut pass) = self.continuous.passes.remove(account) else {
            return Ok(ContinuousStep::Idle);
        };
        if !self.continuous_enabled_now()
            || self.account_read_source(account).as_deref() != Some(pass.source.as_str())
        {
            return Ok(ContinuousStep::Idle);
        }
        let source = pass.source.clone();
        // Sin consultas que gastar la pasada termina; el resto se comprueba en la siguiente.
        let candidate = if pass.budget == 0 {
            None
        } else {
            pass.queue.pop_front()
        };
        let Some(candidate) = candidate else {
            self.finish_pass(account, &source, None);
            return Ok(ContinuousStep::Idle);
        };
        match self.check_candidate(account, &source, &candidate, &mut pass.budget) {
            Ok(changed) => {
                self.continuous.passes.insert(account.to_owned(), pass);
                Ok(if changed {
                    ContinuousStep::Changed
                } else {
                    ContinuousStep::Progress
                })
            }
            Err(CheckFailure::Connector(code)) => {
                // Un fallo de red, de límite o de credenciales detiene la pasada de este buzón
                // sin escribir nada más.
                self.finish_pass(account, &source, Some(code));
                Ok(ContinuousStep::Idle)
            }
            Err(CheckFailure::Store(error)) => {
                self.finish_pass(account, &source, Some("store_error".to_owned()));
                Err(error)
            }
        }
    }

    /// Comprueba una tarea. `Ok(true)` si cambió algo (cerró la tarea o insertó evidencia).
    fn check_candidate(
        &mut self,
        account: &str,
        source: &str,
        candidate: &Candidate,
        budget: &mut usize,
    ) -> Result<bool, CheckFailure> {
        // La tarea debe seguir como estaba al planificar: la persona pudo aprobarla, archivarla,
        // editarla o rechazarla entre un paso y el siguiente.
        let Some(mut task) = self
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
        if candidate.thread_pending && self.continuous.no_thread.contains(&candidate.message_id) {
            return Ok(false);
        }
        // Y NestAdmin pudo enviar otra respuesta del hilo desde que se planificó la pasada.
        if !candidate.thread_pending
            && self
                .nestadmin_sent_after(
                    account,
                    &candidate.thread_key,
                    candidate.received_ms,
                    candidate.recovered,
                )
                .map_err(CheckFailure::Store)?
        {
            return Ok(false);
        }
        // Se anota como comprobada antes de consultar: un fallo propio de esta tarea no la deja
        // siempre la primera de la cola; rota con las demás.
        self.continuous.counter += 1;
        let order = self.continuous.counter;
        self.continuous
            .checked_order
            .insert(candidate.task_id, order);
        let mut thread_key = candidate.thread_key.clone();
        if candidate.thread_pending {
            // Recuperar el hilo (D178): solo metadatos y una consulta del tope.
            *budget = budget.saturating_sub(1);
            match self.mail.thread_of(account, &candidate.inner_id) {
                Ok(Some(found)) if !found.trim().is_empty() => {
                    thread_key = found.trim().to_owned();
                }
                // Un mensaje que ya no existe o que no da hilo: no se vuelve a pedir en la sesión.
                Ok(_) => {
                    self.continuous
                        .no_thread
                        .insert(candidate.message_id.clone());
                    return Ok(false);
                }
                Err(e) if e.code == "not_found" => {
                    self.continuous
                        .no_thread
                        .insert(candidate.message_id.clone());
                    return Ok(false);
                }
                Err(e) if e.code == "invalid_input" => return Ok(false),
                Err(e) => return Err(CheckFailure::Connector(e.code)),
            }
            // Solo se escribe si la tarea no cambió desde que se leyó.
            match self
                .store
                .record_recovered_thread(task.id, task.state, task.updated_at, &thread_key)
                .map_err(store_failure)?
            {
                Some(updated) => task = updated,
                None => return Ok(false),
            }
            // Con el hilo ya conocido, NestAdmin pudo haber respondido a otro correo de él.
            if self
                .nestadmin_sent_after(account, &thread_key, candidate.received_ms, true)
                .map_err(CheckFailure::Store)?
            {
                return Ok(false);
            }
            // Sin consultas que gastar, la comprobación sigue en la siguiente pasada.
            if *budget == 0 {
                return Ok(false);
            }
        }
        *budget = budget.saturating_sub(1);
        let replies = match self
            .mail
            .own_replies_meta(account, &thread_key, &candidate.inner_id)
        {
            Ok(replies) => replies,
            // Un error propio de la tarea (el hilo o el mensaje ya no existen) la salta.
            Err(e) if e.code == "not_found" || e.code == "invalid_input" => return Ok(false),
            Err(e) => return Err(CheckFailure::Connector(e.code)),
        };
        self.continuous.stats_mut(account, source).checked += 1;
        // La primera respuesta dirigida al remitente, aunque antes haya otra a un tercero (un
        // reenvío interno). Solo metadatos: no hay cuerpo que descartar.
        let Some(reply) = replies
            .into_iter()
            .find(|r| replies_to(&r.to, &candidate.address))
        else {
            return Ok(false);
        };
        self.continuous.stats_mut(account, source).observed += 1;
        let replied_at = reply.date.clone();

        // Sin evidencia si una edición o un rechazo del mismo remitente y categoría se registraron
        // después de la respuesta (la fila contaría tras ese corte, D172), o si la fecha de la
        // respuesta no se puede leer para compararlos.
        let evidence = match &candidate.evidence {
            None => None,
            Some(signal) => match replied_at.as_deref().and_then(date_ms) {
                None => None,
                Some(reply_ms) => (!self
                    .store
                    .series_break_after(&signal.subject_key, reply_ms)
                    .map_err(store_failure)?)
                .then_some(signal),
            },
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
                let stats = self.continuous.stats_mut(account, source);
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
                self.continuous.stats_mut(account, source).learned += u64::from(inserted);
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

    #[test]
    fn the_sender_must_be_one_parsed_address() {
        for one in [
            "ana@x.test",
            "Ana <ana@x.test>",
            "ana@x.test <ana@x.test>",
            "\"ana@x.test\" <ana@x.test>",
            "\"Pérez, Ana\" <ana@x.test>",
        ] {
            assert!(is_single_address(one), "{one}");
        }
        for many in [
            "ana@x.test, luis@x.test",
            "Ana <a@x.test>, Luis <l@x.test>",
            "ana@x.test; luis@x.test",
            "Ana Alfa",
            "",
        ] {
            assert!(!is_single_address(many), "{many}");
        }
        let ok = message("ana@x.test <ana@x.test>", Some(&[]));
        assert!(evidence(&result(json!(true), "cita"), &ok).is_some());
    }

    #[test]
    fn an_edited_draft_is_never_evidence() {
        let ok = message("ana@alfa.test", Some(&[]));
        let mut edited = result(json!(true), "cita");
        edited["edited"] = json!(true);
        assert!(evidence(&edited, &ok).is_none());
        edited["edited"] = json!(false);
        assert!(evidence(&edited, &ok).is_some());
    }

    // ---- Pasadas por cuenta (M3, B4): cuentas inyectadas con un conector propio ----

    use std::path::Path;
    use std::sync::Arc;

    use nestadmin_ai::{AiError, GenerateRequest, GenerateResult, Provider, ProviderKind};
    use nestadmin_core::Connector;
    use nestadmin_extract::{ExtractError, Extractor, Ocr};
    use nestadmin_simulators::{
        MailConnector, MailSimulator, MessagePage, NewDraft, OwnReplyMeta, SendReceipt, SentMessage,
    };

    use crate::AppPaths;

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
            panic!("no debe llamarse a la IA");
        }
    }

    struct NoOcr;

    impl Ocr for NoOcr {
        fn recognize(&self, _image: &Path, page: usize) -> Result<String, ExtractError> {
            Err(ExtractError::OcrUnavailable(format!("página {page}")))
        }
    }

    /// Un conector con Enviados que envuelve a su propio simulador.
    struct Mailbox(MailSimulator);

    impl Connector for Mailbox {
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

    impl MailConnector for Mailbox {
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
        fn supports_sent(&self) -> bool {
            true
        }
        fn own_replies_meta(
            &self,
            thread_key: &str,
            after_id: &str,
        ) -> Result<Vec<OwnReplyMeta>, nestadmin_core::ConnectorError> {
            self.0.own_replies_meta(thread_key, after_id)
        }
    }

    const FUTURE: &str = "2099-01-01T10:00:00Z";

    fn received(n: usize) -> Message {
        Message {
            id: format!("m{n}"),
            subject: Some(format!("Consulta {n}")),
            from: Some("ana@cliente.test".into()),
            to: None,
            body: Some("Necesito la factura.".into()),
            date: Some(FUTURE.into()),
            attachments: Vec::new(),
            signals: Some(Vec::new()),
            thread_key: Some(format!("t{n}")),
        }
    }

    /// Un buzón con los correos `1..=n`, todos ya respondidos a Ana.
    fn answered_mailbox(n: usize) -> Mailbox {
        let sim = MailSimulator::with_messages((1..=n).map(received).collect());
        for i in 1..=n {
            sim.add_sent_message(SentMessage {
                id: format!("s{i}"),
                thread_key: format!("t{i}"),
                date: Some("2099-01-01T12:00:00Z".into()),
                body: None,
                to: vec!["ana@cliente.test".into()],
            })
            .unwrap();
        }
        Mailbox(sim)
    }

    /// Una tarea en revisión del correo `n` de la cuenta `account`.
    fn review_task(app: &mut App, account: &str, n: usize) -> i64 {
        let mut message = received(n);
        message.id = format!("{account}:m{n}");
        let input = json!({ "message_id": message.id, "account": account });
        let result = json!({
            "type": EMAIL, "version": 1, "message": message,
            "needs_reply": true, "category": "solicitud_factura",
            "draft": { "to": ["ana@cliente.test"], "subject": "Re", "body": "Hola." },
        });
        app.store
            .enqueue_review(EMAIL, &input.to_string(), &result, "borrador")
            .unwrap()
            .id
    }

    fn two_accounts() -> (tempfile::TempDir, App) {
        let dir = tempfile::tempdir().unwrap();
        let extractor = Extractor::new(dir.path().join("sin-pdfium"), Box::new(NoOcr));
        let mut app = App::open(
            &AppPaths::new(dir.path()),
            Arc::new(NeverCalled),
            Arc::new(extractor),
        )
        .unwrap();
        // Las dos cuentas comparten la misma fuente (la misma dirección y proveedor).
        app.mail
            .insert_test_connector("a1", Box::new(answered_mailbox(3)));
        app.mail
            .insert_test_connector("a2", Box::new(answered_mailbox(3)));
        assert_eq!(app.account_read_source("a1"), app.account_read_source("a2"));
        (dir, app)
    }

    fn stats_of<'a>(overview: &'a ContinuousOverview, account: &str) -> &'a ContinuousMailbox {
        overview
            .mailboxes
            .iter()
            .find(|m| m.account == account)
            .unwrap()
    }

    #[test]
    fn two_accounts_with_the_same_source_keep_their_own_pass_and_counts() {
        let (_dir, mut app) = two_accounts();
        let t1 = review_task(&mut app, "a1", 1);
        let t2 = review_task(&mut app, "a2", 1);
        let t3 = review_task(&mut app, "a2", 2);

        assert_eq!(app.begin_continuous_pass("a1").unwrap(), 1);
        assert_eq!(app.begin_continuous_pass("a2").unwrap(), 2);
        assert_eq!(app.continuous.passes.len(), 2, "no se pisan");
        for account in ["a1", "a2", "a1", "a2", "a2", "a1", "a2"] {
            app.step_continuous_pass(account).unwrap();
        }
        for id in [t1, t2, t3] {
            let task = app.store.task_by_id(id).unwrap().unwrap();
            assert!(is_closed_outside(task.result.as_ref().unwrap()), "{id}");
        }
        let overview = app.continuous_learning_overview().unwrap();
        let (a1, a2) = (stats_of(&overview, "a1"), stats_of(&overview, "a2"));
        assert_eq!((a1.checked, a1.observed, a1.closed), (1, 1, 1));
        assert_eq!((a2.checked, a2.observed, a2.closed), (2, 2, 2));
        // Cada cuenta tiene sus filas `learned`: el mismo id de mensaje en dos cuentas son dos
        // correos distintos (el id lleva la cuenta).
        let a1_row = app
            .store
            .learned_source_exists(&format!(
                "learned|{}|a1:m1",
                app.account_read_source("a1").unwrap()
            ))
            .unwrap();
        assert!(a1_row);
    }

    #[test]
    fn an_orphan_pass_is_dropped_and_another_mailbox_carries_on() {
        let (_dir, mut app) = two_accounts();
        review_task(&mut app, "a1", 1);
        let t2 = review_task(&mut app, "a2", 1);
        app.begin_continuous_pass("a1").unwrap();
        app.begin_continuous_pass("a2").unwrap();

        // Se quita la cuenta a1 entre dos pasos: su pasada se descarta, la de a2 sigue.
        app.mail.remove("a1").unwrap();
        let now = Instant::now();
        let mut steps = 0;
        while app.continuous_learning_tick_at(now).unwrap() != ContinuousStep::Idle {
            steps += 1;
            assert!(steps < 20, "el aprendizaje no se queda parado");
        }
        assert!(app.continuous.passes.is_empty());
        let closed = app.store.task_by_id(t2).unwrap().unwrap();
        assert!(is_closed_outside(closed.result.as_ref().unwrap()));
        // Dar un paso a la cuenta que ya no existe no hace nada.
        assert_eq!(
            app.step_continuous_pass("a1").unwrap(),
            ContinuousStep::Idle
        );

        // Cambiar la fuente de una cuenta entre dos pasos descarta su pasada y no la deja colgada.
        let t3 = review_task(&mut app, "a2", 2);
        assert_eq!(app.begin_continuous_pass("a2").unwrap(), 1);
        app.continuous.passes.get_mut("a2").unwrap().source =
            "account:simulator:otro@x.test".into();
        assert_eq!(
            app.step_continuous_pass("a2").unwrap(),
            ContinuousStep::Idle
        );
        assert!(app.continuous.passes.is_empty());
        assert_eq!(
            app.store.task_by_id(t3).unwrap().unwrap().state,
            TaskState::AwaitingReview
        );
        // Y la cuenta puede volver a pasar después.
        assert_eq!(app.begin_continuous_pass("a2").unwrap(), 1);
        while app.step_continuous_pass("a2").unwrap() != ContinuousStep::Idle {}
        assert!(is_closed_outside(
            app.store
                .task_by_id(t3)
                .unwrap()
                .unwrap()
                .result
                .as_ref()
                .unwrap()
        ));
    }
}
