//! Primera lectura de correos y archivos (E5, D133). Lee una vez una cuenta (o una carpeta, solo
//! en las versiones de prueba) para **leer las facturas** que ya hay y alimentar las sugerencias
//! de reglas con «repetición observada». No redacta borradores, no envía ningún correo, no
//! registra nada y no crea pendientes. Solo salen hacia el proveedor de IA los **documentos que
//! parecen una factura** según una comprobación local sobre el texto ya extraído (sin IA); los
//! correos (cuerpo y asunto) no salen nunca y se cuentan sin clasificar.
//!
//! - Los correos de los últimos [`RECENT_DAYS`] días, o sin fecha legible, **no se leen**: siguen
//!   el flujo normal. La lectura toma hasta el volumen de mensajes anteriores a esa fecha.
//! - Un tope de documentos enviados (el mismo número que el volumen) acota el coste.
//! - Modo según el canal (el de D134): versión final, al conectar con volumen fijo y proveedor
//!   confirmado; pruebas, bajo demanda con el proveedor visible antes.
//! - La lectura avanza **paso a paso** ([`App::read_step`]): quien la lanza retiene el cerrojo de
//!   `App` durante cada paso (una página del listado, un mensaje con sus descargas) y lo suelta
//!   entre un paso y el siguiente.
//! - Se puede detener y reanudar; si el destino de los datos cambia mientras hay tareas en cola,
//!   lo que queda se retiene hasta que la persona lo confirme con el destino nuevo; quitar la
//!   cuenta cancela lo pendiente.
//! - Una sola vez por buzón: la marca se identifica por proveedor y dirección, no por el
//!   identificador de la cuenta (que se reutiliza).

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use nestadmin_ai::ProviderKind;
use nestadmin_core::{Task, TaskState};
use nestadmin_simulators::Message;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::accounts::{join_id, split_id, AttachmentFetch};
use crate::refine::RefineMode;
use crate::suggest::SEEN_INVOICE_MIN;
use crate::{
    check_document, hex, is_initial_read, supported_document, task_is_initial_read, AiWait, App,
    AppError, AppResult, ProviderInfo, EMAIL, INVOICE, MAX_DOCUMENT_BYTES,
};

/// Volumen mínimo y máximo (mensajes o archivos, y tope de documentos enviados) de una lectura
/// bajo demanda: el rango actual del límite de mensajes por cuenta (D133).
pub const READ_MIN: usize = 25;
pub const READ_MAX: usize = 500;
/// Volumen y tope de documentos de la versión final (D133).
pub const READ_FIXED: usize = 100;
/// Los correos de los últimos días no se leen: siguen el flujo normal (D133).
pub const RECENT_DAYS: i64 = 7;
/// Mensajes que se recorren como mucho buscando los anteriores a esa fecha.
pub const SCAN_MAX: usize = 2000;
/// Fallos que se listan como mucho; el recuento total se da aparte.
const MAX_LISTED_FAILURES: usize = 50;

/// Límites del volumen, para la pantalla.
#[derive(Debug, Clone, Serialize)]
pub struct ReadLimits {
    pub min: usize,
    pub max: usize,
    /// Versión final: el volumen no se elige.
    pub fixed: Option<usize>,
    pub default: usize,
}

/// Lo que la persona vio antes de pedir la lectura (o antes de conectar la cuenta). Si el
/// proveedor, el modelo, el servidor o el volumen ya no coinciden, no se lee.
#[derive(Debug, Clone, Deserialize)]
pub struct ReadExpectation {
    pub provider: String,
    pub model: String,
    #[serde(default)]
    pub host: String,
    pub volume: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadIssue {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NamedReadIssue {
    pub name: String,
    pub code: String,
    pub message: String,
    /// Mensaje al que pertenece el adjunto (`None` en un archivo de carpeta): el reintento vuelve
    /// a pedir solo los de los mensajes con fallo.
    #[serde(default)]
    pub message_id: Option<String>,
}

/// A dónde se envían los datos: proveedor, modelo y servidor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Destination {
    pub provider: String,
    pub model: String,
    /// `None` con la IA local.
    pub host: Option<String>,
}

/// Marca persistente de una fuente. Solo ids, cifras y códigos; el cuerpo de un correo nunca.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct Mark {
    v: u32,
    /// `waiting` (sin proveedor confirmado), `pending` (falta confirmar lo mostrado),
    /// `enqueuing` (listando y encolando) o `queued` (todo encolado: no se repite).
    phase: String,
    /// `account` o `folder`.
    kind: String,
    /// Nombre visible: la cuenta o la ruta de la carpeta.
    label: String,
    /// Identificador de la cuenta o ruta de la carpeta.
    target: String,
    /// Claves anteriores de la misma fuente (por identificador), para encontrar sus tareas.
    aliases: Vec<String>,
    mode: String,
    volume: usize,
    started_at: Option<i64>,
    provider: Option<String>,
    model: Option<String>,
    host: Option<String>,
    /// Destinos autorizados desde el principio, en orden.
    destinations: Vec<Destination>,
    stopped: bool,
    removed: bool,
    /// Último resumen escrito en el Historial, para no repetirlo.
    audited: Option<String>,
    cutoff_day: Option<i64>,
    cursor: Option<String>,
    scan_done: bool,
    scanned: usize,
    scan_capped: bool,
    pending: Vec<Message>,
    pending_files: Vec<String>,
    listed: usize,
    known: usize,
    recent_left_out: usize,
    over_cap: usize,
    imported_attachments: usize,
    skipped_attachments: usize,
    attachment_failures: Vec<NamedReadIssue>,
    error: Option<ReadIssue>,
    /// Mensajes (sin cuerpo) con algún adjunto que falló: lo que reintenta «Reintentar».
    failed_messages: Vec<Message>,
    /// Veces que se reinició el recorrido por un cursor caducado.
    cursor_resets: u32,
}

const MARK_VERSION: u32 = 2;

impl Mark {
    fn destination(&self) -> Option<Destination> {
        Some(Destination {
            provider: self.provider.clone()?,
            model: self.model.clone()?,
            host: self.host.clone(),
        })
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_millis()).ok())
        .unwrap_or(0)
}

// ---- Fechas ----

/// Días desde 1970 de una fecha civil (algoritmo de Hinnant).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Día (días desde 1970) de una fecha que empieza por `AAAA-MM-DD` (las de Gmail, Microsoft y el
/// simulador). `None` si falta o no se puede leer.
pub fn day_of(date: &str) -> Option<i64> {
    let head = date.trim().get(..10)?;
    let mut parts = head.split('-');
    let year: i64 = parts.next()?.parse().ok()?;
    let month: i64 = parts.next()?.parse().ok()?;
    let day: i64 = parts.next()?.parse().ok()?;
    ((1900..=2200).contains(&year) && (1..=12).contains(&month) && (1..=31).contains(&day))
        .then(|| days_from_civil(year, month, day))
}

/// Hoy, en días desde 1970 (UTC).
pub fn today_day() -> i64 {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    i64::try_from(secs / 86_400).unwrap_or(0)
}

/// Un correo es **reciente** si tiene como mucho [`RECENT_DAYS`] días (o es de una fecha futura),
/// y también si no tiene fecha o no se puede leer: en esos casos sigue el flujo normal y la
/// primera lectura no lo toca.
pub fn is_recent(date: Option<&str>, today: i64) -> bool {
    match date.and_then(day_of) {
        Some(day) => today - day <= RECENT_DAYS,
        None => true,
    }
}

// ---- Fuentes ----

/// De dónde se lee.
enum Target {
    Account(String),
    Folder(PathBuf),
}

/// Clave de la fuente «cuenta»: proveedor y dirección normalizada (D133). Con el identificador
/// de la cuenta no basta: se reutiliza al quitar y volver a añadir cuentas.
pub fn account_key(provider: &str, address: &str) -> String {
    format!("account:{provider}:{}", address.trim().to_lowercase())
}

/// Clave de una cuenta de la que no consta la dirección (el proveedor no la dio).
fn account_id_key(provider: &str, id: &str) -> String {
    format!("account:{provider}:id:{id}")
}

/// Clave de la fuente «carpeta»: huella de su ruta (sin distinguir mayúsculas, como Windows).
pub fn folder_key(path: &Path) -> String {
    let full = fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    let text = full.display().to_string().to_lowercase();
    format!("folder:{}", hex(&Sha256::digest(text.as_bytes())))
}

// ---- Estado ----

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ReadCounts {
    pub total: usize,
    pub queued: usize,
    pub running: usize,
    pub completed: usize,
    pub failed: usize,
    pub cancelled: usize,
}

/// Una tarea de la lectura que falló, para reintentarla (sin contenido).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReadFailure {
    pub task_id: i64,
    pub kind: String,
    /// Nombre del documento.
    pub name: Option<String>,
    pub code: String,
    pub message: String,
    pub recoverable: bool,
}

/// Documentos enviados a un destino (recuento por proveedor del final de la lectura).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProviderCount {
    pub provider: String,
    pub model: String,
    pub host: Option<String>,
    pub documents: usize,
}

/// Por qué lo pendiente no sale.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Held {
    /// `stopped` (la detuvo la persona), `destination_changed` o `removed`.
    pub reason: &'static str,
    /// Destino que la persona vio al empezar.
    pub authorized: Option<Destination>,
    /// Destino actual.
    pub now: Destination,
}

/// Estado de la lectura de una fuente, calculado con los datos (marca y tareas).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct InitialReadStatus {
    pub source: String,
    /// `account` o `folder`.
    pub kind: &'static str,
    /// `not_started`, `waiting`, `pending`, `interrupted`, `listing_failed`, `stopped`, `held`,
    /// `reading`, `done_with_errors` o `done`.
    pub state: &'static str,
    pub volume: Option<usize>,
    pub started_at: Option<i64>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub host: Option<String>,
    /// Mensajes o archivos recorridos, los que ya tenían tarea normal (no se tocan), los recientes
    /// que siguen el flujo normal y los correos que quedaron sin clasificar.
    pub listed: usize,
    pub already_known: usize,
    pub recent_left_out: usize,
    pub emails_unclassified: usize,
    /// Documentos: enviados al proveedor, omitidos por no parecer factura y no leídos por el tope.
    pub documents_sent: usize,
    pub documents_skipped: usize,
    pub over_cap: usize,
    pub tasks: ReadCounts,
    pub by_provider: Vec<ProviderCount>,
    pub failures: Vec<ReadFailure>,
    pub attachment_failures: Vec<NamedReadIssue>,
    pub skipped_attachments: usize,
    pub listing_error: Option<ReadIssue>,
    /// Se recorrieron [`SCAN_MAX`] mensajes sin reunir el volumen.
    pub scan_capped: bool,
    /// Lo que falta: en cola, en curso o fallido.
    pub remaining: usize,
    /// Tareas respondidas por la IA local de respaldo (D101).
    pub fallback_used: usize,
    /// Acciones (envíos o registros) registradas para las tareas de esta fuente: debe ser 0.
    pub actions: usize,
    pub waiting_message: Option<String>,
    pub held: Option<Held>,
    /// La cuenta ya no está conectada: el resultado sigue visible.
    pub removed: bool,
}

/// Resumen de las sugerencias que alimenta la lectura (de **todas** las lecturas y decisiones, no
/// solo de la fuente).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ReadSuggestions {
    /// Documentos vistos con una factura legible (señales de observación válidas).
    pub seen_documents: u32,
    pub ready: usize,
    pub closest: usize,
    pub needed: u32,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ReadSource {
    pub source: String,
    /// Identificador de la cuenta o ruta de la carpeta: lo que se pasa para empezar o reanudar.
    pub target: String,
    pub label: String,
    pub address: Option<String>,
    pub status: InitialReadStatus,
}

/// Todo lo que la pantalla necesita antes y después de leer.
#[derive(Debug, Clone, Serialize)]
pub struct InitialReadOverview {
    pub mode: RefineMode,
    pub limits: ReadLimits,
    /// Días recientes que no se leen y tope de documentos enviados.
    pub recent_days: i64,
    pub documents_cap: usize,
    pub provider: ProviderInfo,
    /// Servidor real al que saldrían los datos; `None` con la IA local.
    pub host: Option<String>,
    pub external: bool,
    pub local_fallback: bool,
    pub blocked: Option<AiWait>,
    pub blocked_message: Option<String>,
    pub sends: Vec<&'static str>,
    pub never_sent: Vec<&'static str>,
    pub accounts: Vec<ReadSource>,
    /// Cuentas ya desconectadas cuyo resultado sigue visible.
    pub removed: Vec<ReadSource>,
    pub folders: Vec<ReadSource>,
    pub suggestions: ReadSuggestions,
}

/// Una carpeta antes de leerla.
#[derive(Debug, Clone, Serialize)]
pub struct FolderPreview {
    pub source: String,
    pub path: String,
    /// Archivos de tipo admitido en la carpeta (no se entra en subcarpetas).
    pub files: usize,
    pub status: InitialReadStatus,
}

/// Resultado de un paso de la lectura.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadStep {
    /// Queda trabajo: vuelva a llamar.
    Continue,
    /// Todo encolado (o nada que hacer).
    Done,
    /// Detenida o retenida: no avanza hasta que la persona la continúe.
    Paused,
    /// El listado falló; consta en el estado y se puede reanudar.
    Failed,
}

fn mode_name(mode: RefineMode) -> &'static str {
    match mode {
        RefineMode::Automatic => "automatic",
        RefineMode::OnDemand => "on_demand",
    }
}

/// Un mensaje sin cuerpo: lo único que se conserva de un correo leído (D133).
fn stripped(message: &Message) -> Message {
    Message {
        body: None,
        ..message.clone()
    }
}

impl App {
    fn read_limits(&self) -> ReadLimits {
        ReadLimits {
            min: READ_MIN,
            max: READ_MAX,
            fixed: (self.refine_mode == RefineMode::Automatic).then_some(READ_FIXED),
            default: READ_FIXED,
        }
    }

    fn current_destination(&self) -> Destination {
        let info = self.provider();
        Destination {
            provider: info.provider.to_string(),
            model: info.model,
            host: self.provider.destination(),
        }
    }

    fn load_mark(&self, source: &str) -> AppResult<Option<Mark>> {
        Ok(self
            .store
            .read_mark(source)?
            .and_then(|v| serde_json::from_value(v).ok()))
    }

    fn save_mark(&mut self, source: &str, mark: &Mark) -> AppResult<()> {
        let value = serde_json::to_value(mark).map_err(|e| AppError::Invalid(e.to_string()))?;
        self.store.set_read_mark(source, &value)?;
        Ok(())
    }

    /// Clave de la lectura de una cuenta conectada: proveedor y dirección; si no consta la
    /// dirección, el proveedor y el identificador. `None` si la cuenta no existe.
    pub fn account_read_source(&self, id: &str) -> Option<String> {
        let (kind, address) = self.mail.identity(id)?;
        let provider = kind.prefix();
        Some(
            match address.as_deref().map(str::trim).filter(|a| !a.is_empty()) {
                Some(address) => account_key(provider, address),
                None => account_id_key(provider, id),
            },
        )
    }

    /// Como [`App::account_read_source`], migrando antes una marca guardada por identificador a la
    /// clave de la dirección si esta ya consta (el proveedor la dio después).
    fn resolve_account_source(&mut self, id: &str) -> AppResult<Option<String>> {
        let Some(key) = self.account_read_source(id) else {
            return Ok(None);
        };
        let Some((kind, _)) = self.mail.identity(id) else {
            return Ok(Some(key));
        };
        let old = account_id_key(kind.prefix(), id);
        if old != key && self.load_mark(&key)?.is_none() {
            if let Some(mut mark) = self.load_mark(&old)? {
                mark.aliases.push(old.clone());
                self.save_mark(&key, &mark)?;
                // La marca vieja se vacía: queda sin fase y nadie la lee como lectura.
                self.save_mark(&old, &Mark::default())?;
            }
        }
        Ok(Some(key))
    }

    /// Las tareas de la primera lectura de una fuente.
    fn source_tasks(&self, source: &str) -> AppResult<Vec<Task>> {
        let mut keys = vec![source.to_owned()];
        if let Some(mark) = self.load_mark(source)? {
            keys.extend(mark.aliases);
        }
        Ok(self
            .store
            .all_tasks()?
            .into_iter()
            .filter(|t| {
                serde_json::from_str::<Value>(&t.input_ref).is_ok_and(|i| {
                    is_initial_read(&i)
                        && i["read_source"]
                            .as_str()
                            .is_some_and(|s| keys.iter().any(|k| k == s))
                })
            })
            .collect())
    }

    /// Todas las tareas de la primera lectura (para pruebas y la sección aparte).
    pub fn initial_read_tasks(&self) -> AppResult<Vec<Task>> {
        Ok(self
            .store
            .all_tasks()?
            .into_iter()
            .filter(task_is_initial_read)
            .collect())
    }

    /// Por qué las tareas en cola de una fuente no salen, si es el caso.
    fn hold_reason(&self, mark: &Mark) -> Option<&'static str> {
        if mark.removed {
            return Some("removed");
        }
        if mark.stopped {
            return Some("stopped");
        }
        if matches!(mark.phase.as_str(), "enqueuing" | "queued")
            && mark
                .destination()
                .is_some_and(|d| d != self.current_destination())
        {
            return Some("destination_changed");
        }
        None
    }

    /// Fuentes cuyas tareas en cola no deben salir ahora (D133).
    pub(crate) fn held_read_sources(&self) -> AppResult<HashSet<String>> {
        let mut held = HashSet::new();
        for (source, value) in self.store.read_marks()? {
            let Ok(mark) = serde_json::from_value::<Mark>(value) else {
                continue;
            };
            if !mark.phase.is_empty() && self.hold_reason(&mark).is_some() {
                held.insert(source.clone());
                held.extend(mark.aliases.iter().cloned());
            }
        }
        Ok(held)
    }

    /// Estado de la lectura de una fuente (`account:…` o `folder:…`). Solo lectura.
    pub fn initial_read_status(&self, source: &str) -> AppResult<InitialReadStatus> {
        let with_actions = self.store.task_ids_with_actions()?;
        self.status_of(source, &with_actions, false)
    }

    fn status_of(
        &self,
        source: &str,
        with_actions: &HashSet<i64>,
        removed: bool,
    ) -> AppResult<InitialReadStatus> {
        let mark = self.load_mark(source)?.filter(|m| !m.phase.is_empty());
        let tasks = self.source_tasks(source)?;
        let mut counts = ReadCounts {
            total: tasks.len(),
            ..ReadCounts::default()
        };
        let (mut sent, mut skipped, mut emails, mut fallback_used, mut actions) = (0, 0, 0, 0, 0);
        let mut by_provider: Vec<ProviderCount> = Vec::new();
        let mut failures = Vec::new();
        for task in &tasks {
            if with_actions.contains(&task.id) {
                actions += 1;
            }
            let input: Value = serde_json::from_str(&task.input_ref).unwrap_or(Value::Null);
            match task.state {
                TaskState::Queued => counts.queued += 1,
                TaskState::Running => counts.running += 1,
                TaskState::Cancelled => counts.cancelled += 1,
                TaskState::Failed => {
                    counts.failed += 1;
                    if failures.len() < MAX_LISTED_FAILURES {
                        if let Some(error) = &task.error {
                            failures.push(ReadFailure {
                                task_id: task.id,
                                kind: task.kind.clone(),
                                name: (task.kind == INVOICE)
                                    .then(|| input["document"].as_str().map(str::to_owned))
                                    .flatten(),
                                code: error.code.clone(),
                                message: error.message.clone(),
                                recoverable: error.recoverable,
                            });
                        }
                    }
                }
                TaskState::Completed => {
                    counts.completed += 1;
                    let result = task.result.as_ref();
                    if task.kind == EMAIL {
                        emails += 1;
                    } else if result.is_some_and(|r| r.get("skipped").is_some()) {
                        skipped += 1;
                    } else {
                        sent += 1;
                        if result.is_some_and(|r| r.get("fallback").is_some_and(|f| !f.is_null())) {
                            fallback_used += 1;
                        }
                        if let Some(r) = result {
                            let provider = r["provider"].as_str().unwrap_or("?").to_owned();
                            let model = r["model"].as_str().unwrap_or("?").to_owned();
                            let host = r["host"].as_str().map(str::to_owned);
                            match by_provider.iter_mut().find(|p| {
                                p.provider == provider && p.model == model && p.host == host
                            }) {
                                Some(found) => found.documents += 1,
                                None => by_provider.push(ProviderCount {
                                    provider,
                                    model,
                                    host,
                                    documents: 1,
                                }),
                            }
                        }
                    }
                }
                TaskState::AwaitingReview => {}
            }
        }
        by_provider.sort_by(|a, b| {
            b.documents
                .cmp(&a.documents)
                .then(a.provider.cmp(&b.provider))
        });
        let remaining = counts.queued + counts.running + counts.failed;
        let attachment_problems = mark
            .as_ref()
            .is_some_and(|m| !m.attachment_failures.is_empty());
        let hold = mark.as_ref().and_then(|m| self.hold_reason(m));
        let enqueuing = mark.as_ref().is_some_and(|m| m.phase == "enqueuing");
        let held = hold
            .filter(|_| enqueuing || counts.queued > 0)
            .map(|reason| Held {
                reason,
                authorized: mark.as_ref().and_then(Mark::destination),
                now: self.current_destination(),
            });
        let state = match mark.as_ref().map(|m| m.phase.as_str()) {
            None if tasks.is_empty() => "not_started",
            Some("waiting") => "waiting",
            Some("pending") => "pending",
            _ if held.as_ref().is_some_and(|h| h.reason == "stopped") => "stopped",
            _ if held.is_some() => "held",
            Some("enqueuing") if mark.as_ref().is_some_and(|m| m.error.is_some()) => {
                "listing_failed"
            }
            Some("enqueuing") if self.reads_active.contains(source) => "reading",
            Some("enqueuing") => "interrupted",
            _ if counts.queued + counts.running > 0 => "reading",
            _ if counts.failed > 0 || attachment_problems => "done_with_errors",
            _ => "done",
        };
        let waiting_message = self
            .ai_wait
            .filter(|_| state == "waiting" || counts.queued > 0)
            .map(|w| w.message().to_owned());
        let m = mark.as_ref();
        Ok(InitialReadStatus {
            source: source.to_owned(),
            kind: if source.starts_with("folder:") {
                "folder"
            } else {
                "account"
            },
            state,
            volume: m.map(|m| m.volume).filter(|v| *v > 0),
            started_at: m.and_then(|m| m.started_at),
            provider: m.and_then(|m| m.provider.clone()),
            model: m.and_then(|m| m.model.clone()),
            host: m.and_then(|m| m.host.clone()),
            listed: m.map_or(0, |m| m.listed),
            already_known: m.map_or(0, |m| m.known),
            recent_left_out: m.map_or(0, |m| m.recent_left_out),
            emails_unclassified: emails,
            documents_sent: sent,
            documents_skipped: skipped,
            over_cap: m.map_or(0, |m| m.over_cap),
            tasks: counts,
            by_provider,
            failures,
            attachment_failures: m.map(|m| m.attachment_failures.clone()).unwrap_or_default(),
            skipped_attachments: m.map_or(0, |m| m.skipped_attachments),
            listing_error: m.and_then(|m| m.error.clone()),
            scan_capped: m.is_some_and(|m| m.scan_capped),
            remaining,
            fallback_used,
            actions,
            waiting_message,
            held,
            removed: removed || m.is_some_and(|m| m.removed),
        })
    }

    fn read_suggestions(&self) -> ReadSuggestions {
        match self.rule_suggestions() {
            Ok(s) => ReadSuggestions {
                seen_documents: s.seen,
                ready: s.candidates.iter().filter(|c| c.evidence.seen > 0).count(),
                closest: s.progress.iter().filter(|p| p.seen > 0).count(),
                needed: s.thresholds.invoice.max(SEEN_INVOICE_MIN),
                error: None,
            },
            Err(e) => ReadSuggestions {
                seen_documents: 0,
                ready: 0,
                closest: 0,
                needed: SEEN_INVOICE_MIN,
                error: Some(e.to_string()),
            },
        }
    }

    /// Lo que la pantalla muestra antes de leer y mientras se lee. Solo lectura.
    pub fn initial_read_overview(&self) -> AppResult<InitialReadOverview> {
        let provider = self.provider();
        let external = provider.provider != ProviderKind::Ollama;
        let with_actions = self.store.task_ids_with_actions()?;
        let mut accounts = Vec::new();
        let mut shown: HashSet<String> = HashSet::new();
        for account in self.mail.view().accounts {
            let Some(source) = self.account_read_source(&account.id) else {
                continue;
            };
            // Una marca guardada por identificador (sin dirección conocida entonces) también vale.
            let source = match (self.load_mark(&source)?, self.mail.identity(&account.id)) {
                (None, Some((kind, _)))
                    if self
                        .load_mark(&account_id_key(kind.prefix(), &account.id))?
                        .is_some_and(|m| !m.phase.is_empty()) =>
                {
                    account_id_key(kind.prefix(), &account.id)
                }
                _ => source,
            };
            shown.insert(source.clone());
            let status = self.status_of(&source, &with_actions, false)?;
            accounts.push(ReadSource {
                source,
                target: account.id.clone(),
                label: account.label,
                address: account.address,
                status,
            });
        }
        let (mut folders, mut removed) = (Vec::new(), Vec::new());
        for (source, value) in self.store.read_marks()? {
            let Ok(mark) = serde_json::from_value::<Mark>(value) else {
                continue;
            };
            if mark.phase.is_empty() || shown.contains(&source) {
                continue;
            }
            let is_folder = source.starts_with("folder:");
            let status = self.status_of(&source, &with_actions, !is_folder)?;
            let item = ReadSource {
                source,
                target: mark.target.clone(),
                label: mark.label,
                address: None,
                status,
            };
            if is_folder {
                folders.push(item);
            } else {
                removed.push(item);
            }
        }
        Ok(InitialReadOverview {
            mode: self.refine_mode,
            limits: self.read_limits(),
            recent_days: RECENT_DAYS,
            documents_cap: READ_FIXED,
            host: self.provider.destination(),
            external,
            local_fallback: external && self.fallback.is_some(),
            blocked: self.ai_wait,
            blocked_message: self.ai_wait.map(|w| w.message().to_owned()),
            sends: vec![
                "El texto completo de cada documento adjunto o de la carpeta que parece una factura según una comprobación local, ya extraído en este equipo (el OCR y la extracción de texto son locales), para leer proveedor, número, fechas e importes",
            ],
            never_sent: vec![
                "en esta lectura, el cuerpo ni el asunto de ningún correo: no se envían y quedan sin clasificar (los correos recientes siguen el flujo normal: al consultar el correo salen completos al proveedor y una regla activa puede actuar sobre ellos)",
                "los documentos que no parecen una factura (se cuentan como omitidos) ni los que superan el tope",
                "los archivos originales, las imágenes ni las huellas",
                "claves, credenciales ni el historial de acciones y decisiones",
                "las direcciones de sus buzones ni los nombres de cuenta, salvo que estén escritos en el texto de un documento que sí se envía, que sale completo (D102)",
            ],
            accounts,
            removed,
            folders,
            suggestions: self.read_suggestions(),
            provider,
        })
    }

    /// Una carpeta antes de leerla: cuántos archivos admitidos tiene y si ya se leyó.
    pub fn initial_read_folder_preview(&self, path: &Path) -> AppResult<FolderPreview> {
        let entries = fs::read_dir(path)
            .map_err(|e| AppError::Invalid(format!("no se pudo abrir la carpeta: {e}")))?;
        let files = entries
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
            .filter(|e| e.file_name().to_str().is_some_and(supported_document))
            .count();
        let source = folder_key(path);
        Ok(FolderPreview {
            status: self.initial_read_status(&source)?,
            source,
            path: path.display().to_string(),
            files,
        })
    }

    // ---- Empezar, detener, continuar ----

    /// Empieza la lectura de una cuenta y la lleva hasta el final en este hilo (para pruebas y
    /// llamadas sin ventana). La ventana usa [`App::begin_initial_read`] y [`App::read_step`] para
    /// soltar el cerrojo entre pasos.
    pub fn start_initial_read(
        &mut self,
        account: &str,
        volume: Option<usize>,
        expect: Option<&ReadExpectation>,
    ) -> AppResult<InitialReadStatus> {
        let status = self.begin_initial_read(account, volume, expect)?;
        self.run_read_steps(&status.source)
    }

    /// Lee una carpeta de archivos (solo en las versiones de prueba, bajo demanda), hasta el final.
    pub fn start_initial_read_folder(
        &mut self,
        path: &Path,
        volume: Option<usize>,
        expect: Option<&ReadExpectation>,
    ) -> AppResult<InitialReadStatus> {
        let status = self.begin_initial_read_folder(path, volume, expect)?;
        self.run_read_steps(&status.source)
    }

    /// Valida lo que la persona vio, anota la marca y deja la lectura lista para avanzar con
    /// [`App::read_step`]. No toca el buzón ni la red.
    pub fn begin_initial_read(
        &mut self,
        account: &str,
        volume: Option<usize>,
        expect: Option<&ReadExpectation>,
    ) -> AppResult<InitialReadStatus> {
        self.begin_read(Target::Account(account.to_owned()), volume, expect, false)
    }

    pub fn begin_initial_read_folder(
        &mut self,
        path: &Path,
        volume: Option<usize>,
        expect: Option<&ReadExpectation>,
    ) -> AppResult<InitialReadStatus> {
        self.begin_read(Target::Folder(path.to_owned()), volume, expect, false)
    }

    /// Avanza una lectura hasta el final llamando a [`App::read_step`].
    pub fn run_read_steps(&mut self, source: &str) -> AppResult<InitialReadStatus> {
        while self.read_step(source)? == ReadStep::Continue {}
        self.initial_read_status(source)
    }

    /// Al conectar una cuenta nueva: en la versión final, con lo que se mostró antes de conectar
    /// (`expect`), empieza la lectura con el volumen fijo; si no hay proveedor confirmado queda
    /// **en espera**, y si no se mostró nada o lo mostrado ya no es cierto (el resumen falló, el
    /// proveedor cambió) queda **pendiente** de que la persona lo confirme con un botón. En las
    /// versiones de prueba no hace nada.
    pub fn request_initial_read(
        &mut self,
        account: &str,
        expect: Option<&ReadExpectation>,
    ) -> AppResult<InitialReadStatus> {
        let Some(key) = self.resolve_account_source(account)? else {
            return Err(AppError::Invalid("la cuenta no está conectada".into()));
        };
        if self.refine_mode != RefineMode::Automatic {
            return self.initial_read_status(&key);
        }
        let shown = expect.is_some_and(|e| {
            e.volume == READ_FIXED
                && self.asked() == (e.provider.clone(), e.model.clone())
                && self.host() == e.host
        });
        if !shown {
            // No se mostró nada (el resumen falló) o lo visto ya no vale: no empieza sola, ni
            // siquiera para esperar; queda pendiente de que la persona la empiece con su botón.
            if self.load_mark(&key)?.is_none_or(|m| m.phase.is_empty()) {
                let label = self.account_label(account);
                let mark = Mark {
                    v: MARK_VERSION,
                    phase: "pending".into(),
                    kind: "account".into(),
                    label,
                    target: account.to_owned(),
                    mode: mode_name(self.refine_mode).into(),
                    volume: READ_FIXED,
                    ..Mark::default()
                };
                self.save_mark(&key, &mark)?;
            }
            return self.initial_read_status(&key);
        }
        self.begin_read(Target::Account(account.to_owned()), None, None, true)
    }

    /// Detiene una lectura: no sale nada más y se puede continuar. Deja una entrada en el
    /// Historial.
    pub fn stop_initial_read(&mut self, source: &str) -> AppResult<InitialReadStatus> {
        let Some(mut mark) = self.load_mark(source)?.filter(|m| !m.phase.is_empty()) else {
            return Err(AppError::Invalid("esa lectura no existe".into()));
        };
        mark.stopped = true;
        self.save_mark(source, &mark)?;
        self.refresh_read_gate();
        self.report_read(source, "stopped")?;
        self.initial_read_status(source)
    }

    /// Continúa una lectura detenida o retenida por un cambio de destino, **con el destino de
    /// ahora**: `expect` es lo que la persona vio (proveedor, modelo y servidor actuales).
    pub fn continue_initial_read(
        &mut self,
        source: &str,
        expect: &ReadExpectation,
    ) -> AppResult<InitialReadStatus> {
        let Some(mut mark) = self.load_mark(source)?.filter(|m| !m.phase.is_empty()) else {
            return Err(AppError::Invalid("esa lectura no existe".into()));
        };
        if self.asked() != (expect.provider.clone(), expect.model.clone())
            || self.host() != expect.host
        {
            return Err(AppError::Invalid(
                "El proveedor, el modelo o el servidor han cambiado desde que vio el aviso: revíselo de nuevo antes de continuar."
                    .into(),
            ));
        }
        if mark.removed {
            return Err(AppError::Invalid(
                "La cuenta ya no está conectada: no se puede continuar su lectura.".into(),
            ));
        }
        if self.ai_wait.is_some() {
            return Err(AppError::Invalid(
                "Falta confirmar un proveedor de IA: no se ha enviado nada.".into(),
            ));
        }
        let now = self.current_destination();
        mark.stopped = false;
        mark.audited = None;
        mark.provider = Some(now.provider.clone());
        mark.model = Some(now.model.clone());
        mark.host = now.host.clone();
        if mark.destinations.last() != Some(&now) {
            mark.destinations.push(now);
        }
        self.save_mark(source, &mark)?;
        self.refresh_read_gate();
        self.initial_read_status(source)
    }

    /// Reintenta lo que falló: las tareas con un error recuperable vuelven a la cola (la misma
    /// tarea, sin duplicar) y los adjuntos que no se pudieron descargar se vuelven a pedir.
    /// Devuelve cuántas tareas volvieron a la cola.
    pub fn retry_initial_read(&mut self, source: &str) -> AppResult<usize> {
        let mut retried = 0;
        for task in self.source_tasks(source)? {
            if task.state == TaskState::Failed && task.error.as_ref().is_some_and(|e| e.recoverable)
            {
                self.store.retry(task.id)?;
                retried += 1;
            }
        }
        if let Some(mut mark) = self.load_mark(source)?.filter(|m| m.phase == "queued") {
            if !mark.attachment_failures.is_empty() && !mark.removed {
                // Solo se vuelve a pedir lo que falló, por mensaje (o por archivo en una carpeta),
                // sin recorrer otra vez el buzón. Los fallos constan hasta que se resuelvan.
                if mark.kind == "folder" {
                    mark.pending_files = mark
                        .attachment_failures
                        .iter()
                        .map(|f| f.name.clone())
                        .collect();
                } else {
                    mark.pending = mark.failed_messages.clone();
                }
                mark.phase = "enqueuing".into();
                mark.scan_done = true;
                mark.error = None;
                self.save_mark(source, &mark)?;
                self.reads_active.insert(source.to_owned());
            }
        }
        if let Some(mut mark) = self.load_mark(source)? {
            mark.audited = None;
            self.save_mark(source, &mark)?;
        }
        Ok(retried)
    }

    /// Cancela lo pendiente de una cuenta que se desconecta; el resultado sigue visible.
    pub(crate) fn cancel_initial_read(&mut self, source: &str) -> AppResult<()> {
        for task in self.source_tasks(source)? {
            if task.state == TaskState::Queued {
                self.store.cancel(task.id)?;
            }
        }
        if let Some(mut mark) = self.load_mark(source)?.filter(|m| !m.phase.is_empty()) {
            mark.removed = true;
            if mark.phase == "enqueuing" {
                mark.phase = "queued".into();
                mark.pending.clear();
            }
            self.save_mark(source, &mark)?;
            self.refresh_read_gate();
            self.report_read(source, "removed")?;
        }
        Ok(())
    }

    fn account_label(&self, id: &str) -> String {
        self.mail
            .view()
            .accounts
            .into_iter()
            .find(|a| a.id == id)
            .map_or_else(|| id.to_owned(), |a| a.label)
    }

    fn begin_read(
        &mut self,
        target: Target,
        volume: Option<usize>,
        expect: Option<&ReadExpectation>,
        auto: bool,
    ) -> AppResult<InitialReadStatus> {
        let mode = self.refine_mode;
        let (key, kind, label, target_text) = match &target {
            Target::Account(id) => {
                let key = self
                    .resolve_account_source(id)?
                    .ok_or_else(|| AppError::Invalid("la cuenta no está conectada".into()))?;
                (key, "account", self.account_label(id), id.clone())
            }
            Target::Folder(path) => {
                if mode == RefineMode::Automatic {
                    return Err(AppError::Invalid(
                        "La lectura de una carpeta solo existe en las versiones de prueba.".into(),
                    ));
                }
                (
                    folder_key(path),
                    "folder",
                    path.display().to_string(),
                    path.display().to_string(),
                )
            }
        };
        let existing = self.load_mark(&key)?.filter(|m| !m.phase.is_empty());
        // Una sola vez: ya se encoló todo, no se vuelve a listar ni a llamar a la IA.
        if existing.as_ref().is_some_and(|m| m.phase == "queued") {
            return self.initial_read_status(&key);
        }
        let resumed: Option<Mark> = existing
            .clone()
            .filter(|m| m.phase == "enqueuing" && m.kind == kind);
        let volume = match mode {
            RefineMode::Automatic => {
                if volume.is_some_and(|v| v != READ_FIXED) {
                    return Err(AppError::Invalid(format!(
                        "En esta versión la primera lectura es de {READ_FIXED} mensajes y no se elige."
                    )));
                }
                READ_FIXED
            }
            RefineMode::OnDemand => {
                let chosen = resumed
                    .as_ref()
                    .map(|m| m.volume)
                    .or(volume)
                    .ok_or_else(|| {
                        AppError::Invalid(format!(
                            "Elija cuántos elementos leer, entre {READ_MIN} y {READ_MAX}."
                        ))
                    })?;
                if !(READ_MIN..=READ_MAX).contains(&chosen) {
                    return Err(AppError::Invalid(format!(
                        "El volumen debe estar entre {READ_MIN} y {READ_MAX}."
                    )));
                }
                chosen
            }
        };
        // Lo que la persona vio: obligatorio al pedirlo ella; al conectar lo comprueba
        // `request_initial_read` antes de llegar aquí.
        if !auto {
            let seen = expect.ok_or_else(|| {
                AppError::Invalid(
                    "Antes de leer hay que ver a qué proveedor se envía y cuánto se lee.".into(),
                )
            })?;
            if self.asked() != (seen.provider.clone(), seen.model.clone())
                || self.host() != seen.host
                || seen.volume != volume
            {
                return Err(AppError::Invalid(
                    "El proveedor, el modelo, el servidor o el volumen han cambiado desde que vio el aviso: revíselo de nuevo antes de leer."
                        .into(),
                ));
            }
        }
        // Sin proveedor confirmado no sale nada (D100, D101).
        if let Some(wait) = self.ai_wait {
            if !auto && mode == RefineMode::OnDemand {
                return Err(AppError::Invalid(format!(
                    "{}. No se ha leído ni enviado nada.",
                    wait.message()
                )));
            }
            // Versión final: queda anotada y empieza sola cuando haya proveedor confirmado.
            if existing.as_ref().is_none_or(|m| m.phase == "pending") {
                let mark = Mark {
                    v: MARK_VERSION,
                    phase: "waiting".into(),
                    kind: kind.into(),
                    label,
                    target: target_text,
                    mode: mode_name(mode).into(),
                    volume,
                    ..Mark::default()
                };
                self.save_mark(&key, &mark)?;
            }
            return self.initial_read_status(&key);
        }
        let now = self.current_destination();
        let mut mark = resumed.unwrap_or_else(|| Mark {
            v: MARK_VERSION,
            kind: kind.into(),
            ..Mark::default()
        });
        mark.phase = "enqueuing".into();
        mark.label = label;
        mark.target = target_text;
        mark.mode = mode_name(mode).into();
        mark.volume = volume;
        mark.stopped = false;
        mark.audited = None;
        mark.started_at.get_or_insert_with(now_ms);
        mark.provider = Some(now.provider.clone());
        mark.model = Some(now.model.clone());
        mark.host = now.host.clone();
        if mark.destinations.last() != Some(&now) {
            mark.destinations.push(now);
        }
        mark.error = None;
        if mark.cutoff_day.is_none() {
            mark.cutoff_day = Some(today_day());
        }
        if let Target::Folder(path) = &target {
            if !mark.scan_done {
                match list_folder(path) {
                    Ok(files) => {
                        mark.listed = files.len();
                        mark.pending_files = files;
                        mark.scan_done = true;
                    }
                    Err(issue) => mark.error = Some(issue),
                }
            }
        }
        // Se anota antes de listar nada: un corte deja una lectura reanudable, nunca repetida.
        self.save_mark(&key, &mark)?;
        self.reads_active.insert(key.clone());
        self.initial_read_status(&key)
    }

    /// Retoma, en la versión final, las lecturas en espera o cortadas, un paso cada vez: no hace
    /// nada con la cola en pausa, sin proveedor confirmado o con lo pendiente retenido. Devuelve
    /// si hizo trabajo (quien la llama vuelve enseguida) o no. Un error de una lectura queda en
    /// su marca y no impide las demás ni se repite.
    pub fn read_tick(&mut self) -> AppResult<bool> {
        if self.refine_mode != RefineMode::Automatic || self.paused || self.ai_wait.is_some() {
            return Ok(false);
        }
        for (source, value) in self.store.read_marks()? {
            let Ok(mark) = serde_json::from_value::<Mark>(value) else {
                continue;
            };
            let Some(account) = (mark.kind == "account").then_some(mark.target.clone()) else {
                continue;
            };
            if mark.error.is_some() || mark.removed || mark.stopped {
                continue;
            }
            let worked = match mark.phase.as_str() {
                "waiting" => self
                    .begin_read(Target::Account(account), None, None, true)
                    .map(|_| true),
                "enqueuing" => self.read_step(&source).map(|s| s == ReadStep::Continue),
                _ => continue,
            };
            match worked {
                Ok(true) => return Ok(true),
                Ok(false) => {}
                Err(error) => self.note_read_error(&source, &error),
            }
        }
        Ok(false)
    }

    /// Lecturas en espera o cortadas, hasta el final, en este hilo (pruebas y llamadas sin
    /// ventana). Devuelve el estado de las lecturas de cuentas que están en curso o terminadas.
    pub fn start_waiting_reads(&mut self) -> AppResult<Vec<InitialReadStatus>> {
        let before: HashSet<String> = self
            .store
            .read_marks()?
            .into_iter()
            .filter(|(_, v)| v["phase"] == "queued")
            .map(|(k, _)| k)
            .collect();
        let mut worked = false;
        while self.read_tick()? {
            worked = true;
        }
        if !worked {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        for (source, value) in self.store.read_marks()? {
            let phase = value["phase"].as_str().unwrap_or("");
            if source.starts_with("account:")
                && matches!(phase, "enqueuing" | "queued")
                && !before.contains(&source)
            {
                out.push(self.initial_read_status(&source)?);
            }
        }
        Ok(out)
    }

    /// Deja constancia de un error interno en la marca para no reintentarlo cada pocos segundos.
    fn note_read_error(&mut self, source: &str, error: &AppError) {
        self.note_initial_read_error(source, &error.to_string());
    }

    /// Lo mismo, desde quien lleva la lectura paso a paso fuera de esta capa.
    pub fn note_initial_read_error(&mut self, source: &str, message: &str) {
        if let Ok(Some(mut mark)) = self.load_mark(source) {
            mark.error = Some(ReadIssue {
                code: "internal".into(),
                message: message.to_owned(),
            });
            let _ = self.save_mark(source, &mark);
        }
    }

    // ---- Un paso ----

    /// Hace **una** unidad de trabajo de la lectura: recorrer una página del buzón, encolar un
    /// mensaje con sus adjuntos o un archivo de la carpeta. Quien la llama puede soltar el
    /// cerrojo de `App` entre un paso y el siguiente.
    pub fn read_step(&mut self, source: &str) -> AppResult<ReadStep> {
        let Some(mut mark) = self.load_mark(source)?.filter(|m| m.phase == "enqueuing") else {
            return Ok(ReadStep::Done);
        };
        if self.hold_reason(&mark).is_some() {
            self.reads_active.remove(source);
            return Ok(ReadStep::Paused);
        }
        self.reads_active.insert(source.to_owned());
        let stepped = match mark.kind.as_str() {
            "folder" => self.step_folder(source, &mut mark),
            _ => self.step_account(source, &mut mark),
        };
        let step = match stepped {
            Ok(step) => step,
            Err(error) => {
                self.reads_active.remove(source);
                return Err(error);
            }
        };
        if step != ReadStep::Continue {
            self.reads_active.remove(source);
        }
        if step == ReadStep::Done {
            self.finalize_unclassified(source)?;
            mark.phase = "queued".into();
            mark.pending.clear();
            mark.pending_files.clear();
            mark.error = None;
        }
        self.save_mark(source, &mark)?;
        if step == ReadStep::Done {
            self.report_read_if_finished(source)?;
        }
        Ok(step)
    }

    /// Tareas de correo de la lectura (mensajes con tarea, si es de la lectura) y adjuntos ya
    /// importados, más los documentos de esta fuente.
    fn read_index(&self, source: &str) -> AppResult<ReadIndex> {
        let mut keys = vec![source.to_owned()];
        if let Some(mark) = self.load_mark(source)? {
            keys.extend(mark.aliases);
        }
        let mut index = ReadIndex::default();
        for task in self.store.all_tasks()? {
            let input: Value = serde_json::from_str(&task.input_ref).unwrap_or(Value::Null);
            match task.kind.as_str() {
                EMAIL => {
                    if let Some(id) = input["message_id"].as_str() {
                        let (a, inner) = split_id(id);
                        index
                            .messages
                            .insert(join_id(a, inner), (task.id, is_initial_read(&input)));
                    }
                }
                INVOICE => {
                    if let Some(k) = input["origin"]["attachment_key"].as_str() {
                        index.attachments.insert(k.to_owned());
                    }
                    if let Some(h) = input["origin"]["sha256"].as_str() {
                        index.hashes.insert(h.to_owned());
                    }
                    let mine = is_initial_read(&input)
                        && input["read_source"]
                            .as_str()
                            .is_some_and(|s| keys.iter().any(|k| k == s));
                    if mine && input.get("part").is_none() {
                        index.documents += 1;
                    }
                }
                _ => {}
            }
            if task.kind == EMAIL
                && is_initial_read(&input)
                && input["read_source"]
                    .as_str()
                    .is_some_and(|s| keys.iter().any(|k| k == s))
            {
                index.emails += 1;
            }
        }
        Ok(index)
    }

    fn step_account(&mut self, source: &str, mark: &mut Mark) -> AppResult<ReadStep> {
        let account = mark.target.clone();
        let volume = mark.volume;
        // Fecha de corte fijada al empezar: una reanudación no mueve el límite de los 7 días.
        let today = mark.cutoff_day.unwrap_or_else(today_day);
        // 1. Recorrer una página del buzón buscando mensajes anteriores a la fecha de corte.
        if !mark.scan_done {
            let index = self.read_index(source)?;
            let (page, next) = match self.mail.list_page(&account, mark.cursor.as_deref()) {
                Ok(found) => found,
                // Un cursor caducado (el proveedor ya no lo reconoce) no deja la lectura fallando
                // para siempre: se reinicia el recorrido, sin duplicar nada porque lo ya
                // encolado se reconoce.
                Err(e)
                    if mark.cursor.is_some()
                        && matches!(e.code.as_str(), "invalid_input" | "not_found")
                        && mark.cursor_resets < 3 =>
                {
                    mark.cursor_resets += 1;
                    mark.cursor = None;
                    mark.scanned = 0;
                    mark.recent_left_out = 0;
                    return Ok(ReadStep::Continue);
                }
                Err(e) => {
                    mark.error = Some(ReadIssue {
                        code: e.code,
                        message: e.message,
                    });
                    return Ok(ReadStep::Failed);
                }
            };
            mark.error = None;
            for message in &page {
                mark.scanned += 1;
                if is_recent(message.date.as_deref(), today) {
                    mark.recent_left_out += 1;
                    continue;
                }
                match index.messages.get(&message.id) {
                    // Ya tiene una tarea normal (no se toca).
                    Some((_, false)) => {}
                    // De una pasada anterior: solo faltan sus adjuntos.
                    Some((_, true)) => {
                        if !mark.pending.iter().any(|m| m.id == message.id) {
                            mark.pending.push(stripped(message));
                        }
                    }
                    None => {
                        if index.emails + mark.pending.len() < volume
                            && !mark.pending.iter().any(|m| m.id == message.id)
                        {
                            mark.pending.push(stripped(message));
                        }
                    }
                }
            }
            mark.listed = mark.scanned;
            let selected = mark.pending.len() + index.emails;
            mark.cursor = next;
            if mark.cursor.is_none() || selected >= volume || mark.scanned >= SCAN_MAX {
                mark.scan_done = true;
                mark.scan_capped = mark.cursor.is_some() && selected < volume;
            }
            return Ok(ReadStep::Continue);
        }
        // 2. Encolar un mensaje (sin clasificar, sin cuerpo) y sus adjuntos.
        if mark.pending.is_empty() {
            return Ok(ReadStep::Done);
        }
        let message = mark.pending.remove(0);
        let index = self.read_index(source)?;
        // Los recuentos de omitidos solo se suman la primera vez que se procesa un mensaje: un
        // reintento o una reanudación no los infla (pasan al Historial).
        let first_time = !index.messages.contains_key(&message.id);
        let task_id = match index.messages.get(&message.id) {
            Some((_, false)) => {
                mark.known += 1;
                return Ok(ReadStep::Continue);
            }
            Some((id, true)) => *id,
            None => self.enqueue_unclassified(&account, source, &message)?,
        };
        let mut imported = index.attachments;
        let mut report = AttachmentFetch::default();
        let mut budget = volume.saturating_sub(index.documents);
        let mut over = 0;
        self.import_attachments(
            task_id,
            &message,
            &mut imported,
            &mut report,
            Some((&mut budget, &mut over)),
        )?;
        // Fallos que este mensaje tenía listados antes de este recorrido.
        let prior = mark
            .attachment_failures
            .iter()
            .filter(|f| f.message_id.as_deref() == Some(message.id.as_str()))
            .count();
        if first_time {
            mark.over_cap += over;
            mark.skipped_attachments += report.skipped.len();
        } else {
            // Un adjunto que fallaba y ahora no cabe en el tope no se resolvió: pasa a omitido
            // por tope en vez de desaparecer al borrar su fallo (no se suma `over` entero:
            // contaría dos veces los que ya estaban contados).
            mark.over_cap += prior.saturating_sub(report.imported + report.failed.len());
        }
        mark.imported_attachments += report.imported;
        // Los fallos de este mensaje se sustituyen por los de ahora: se conservan hasta que se
        // resuelvan y nunca desaparecen por volver a recorrer.
        mark.attachment_failures
            .retain(|f| f.message_id.as_deref() != Some(message.id.as_str()));
        mark.failed_messages.retain(|m| m.id != message.id);
        if !report.failed.is_empty() {
            mark.failed_messages.push(stripped(&message));
        }
        for failed in report.failed {
            if mark.attachment_failures.len() < MAX_LISTED_FAILURES {
                mark.attachment_failures.push(NamedReadIssue {
                    name: failed.name,
                    code: failed.code,
                    message: failed.message,
                    message_id: Some(message.id.clone()),
                });
            }
        }
        Ok(ReadStep::Continue)
    }

    /// Crea la tarea de un correo de la lectura y la cierra enseguida como **sin clasificar**: no
    /// se envía a ningún proveedor y de su cuerpo no se guarda nada (solo remitente, asunto y
    /// fecha, que sirven de origen de sus adjuntos).
    fn enqueue_unclassified(
        &mut self,
        account: &str,
        source: &str,
        message: &Message,
    ) -> AppResult<i64> {
        let origin = self.mail.origin(account);
        let input = json!({
            "message_id": message.id,
            "account": account,
            "origin": origin,
            "message": {
                "id": message.id,
                "subject": message.subject,
                "from": message.from,
                "date": message.date,
            },
            "initial_read": true,
            "read_source": source,
        });
        let task = self.store.enqueue(EMAIL, &input.to_string())?;
        self.store.claim(task.id)?;
        let result = json!({
            "type": EMAIL,
            "version": 1,
            "initial_read": true,
            "unclassified": true,
            "origin": origin,
            "message_id": message.id,
        });
        self.store.complete(task.id, &result)?;
        Ok(task.id)
    }

    /// Cierra como sin clasificar los correos de la lectura que un corte dejó a medias.
    fn finalize_unclassified(&mut self, source: &str) -> AppResult<()> {
        for task in self.source_tasks(source)? {
            if task.kind != EMAIL {
                continue;
            }
            let input: Value = serde_json::from_str(&task.input_ref).unwrap_or(Value::Null);
            let current = match task.state {
                TaskState::Queued => self.store.claim(task.id)?,
                TaskState::Running => task,
                _ => continue,
            };
            let result = json!({
                "type": EMAIL,
                "version": 1,
                "initial_read": true,
                "unclassified": true,
                "origin": input["origin"],
                "message_id": input["message_id"],
            });
            self.store.complete(current.id, &result)?;
        }
        Ok(())
    }

    fn step_folder(&mut self, source: &str, mark: &mut Mark) -> AppResult<ReadStep> {
        if !mark.scan_done {
            let path = PathBuf::from(&mark.target);
            match list_folder(&path) {
                Ok(files) => {
                    mark.listed = files.len();
                    mark.pending_files = files;
                    mark.scan_done = true;
                    mark.error = None;
                }
                Err(issue) => {
                    mark.error = Some(issue);
                    return Ok(ReadStep::Failed);
                }
            }
        }
        if mark.pending_files.is_empty() {
            return Ok(ReadStep::Done);
        }
        let index = self.read_index(source)?;
        let name = mark.pending_files.remove(0);
        if index.documents >= mark.volume {
            // Tope alcanzado: el resto no se lee y se cuenta.
            mark.over_cap += 1 + mark.pending_files.len();
            mark.pending_files.clear();
            return Ok(ReadStep::Done);
        }
        let file = Path::new(&mark.target).join(&name);
        // Un reintento de este archivo sustituye a su fallo anterior.
        mark.attachment_failures.retain(|f| f.name != name);
        let fail = |mark: &mut Mark, code: &str, message: String| {
            if mark.attachment_failures.len() < MAX_LISTED_FAILURES {
                mark.attachment_failures.push(NamedReadIssue {
                    name: name.clone(),
                    code: code.to_owned(),
                    message,
                    message_id: None,
                });
            }
        };
        let too_big = fs::metadata(&file)
            .map(|m| m.len() > MAX_DOCUMENT_BYTES as u64)
            .unwrap_or(false);
        if too_big {
            fail(
                mark,
                "invalid_input",
                format!(
                    "supera el tamaño máximo de {} MB",
                    MAX_DOCUMENT_BYTES / 1_000_000
                ),
            );
            return Ok(ReadStep::Continue);
        }
        let bytes = match fs::read(&file) {
            Ok(bytes) => bytes,
            Err(e) => {
                fail(mark, "unavailable", format!("no se pudo leer: {e}"));
                return Ok(ReadStep::Continue);
            }
        };
        if index.hashes.contains(&hex(&Sha256::digest(&bytes))) {
            mark.known += 1;
            return Ok(ReadStep::Continue);
        }
        let extension = match check_document(&name, &bytes) {
            Ok(extension) => extension,
            Err(AppError::Invalid(reason)) => {
                fail(mark, "invalid_input", reason);
                return Ok(ReadStep::Continue);
            }
            Err(other) => return Err(other),
        };
        let origin = json!({
            "kind": "file",
            "name": name,
            "original_path": file.display().to_string(),
        });
        self.store_document(&name, &extension, &bytes, origin, Some(source))?;
        Ok(ReadStep::Continue)
    }

    // ---- Historial ----

    /// Si la lectura terminó (nada en cola ni en curso) y aún no se ha resumido, escribe su
    /// entrada en el Historial.
    pub(crate) fn after_initial_task(&mut self, task: &Task) {
        let input: Value = serde_json::from_str(&task.input_ref).unwrap_or(Value::Null);
        if let Some(source) = input["read_source"].as_str() {
            // La marca pudo migrar de clave (por identificador a dirección) con la tarea en curso.
            let source = self
                .canonical_source(source)
                .unwrap_or_else(|_| source.to_owned());
            let _ = self.report_read_if_finished(&source);
        }
    }

    /// La clave vigente de una fuente: la misma, o la que la sustituyó al migrar la marca.
    fn canonical_source(&self, source: &str) -> AppResult<String> {
        if self.load_mark(source)?.is_some_and(|m| !m.phase.is_empty()) {
            return Ok(source.to_owned());
        }
        for (key, value) in self.store.read_marks()? {
            if let Ok(mark) = serde_json::from_value::<Mark>(value) {
                if !mark.phase.is_empty() && mark.aliases.iter().any(|a| a == source) {
                    return Ok(key);
                }
            }
        }
        Ok(source.to_owned())
    }

    /// Las claves de lectura de una cuenta: la de su dirección (migrando antes una marca guardada
    /// por identificador) y cualquier marca que apunte a su identificador.
    pub(crate) fn account_read_sources(&mut self, id: &str) -> AppResult<Vec<String>> {
        let mut keys: Vec<String> = Vec::new();
        if let Some(key) = self.resolve_account_source(id)? {
            keys.push(key);
        }
        for (key, value) in self.store.read_marks()? {
            if let Ok(mark) = serde_json::from_value::<Mark>(value) {
                if mark.kind == "account"
                    && mark.target == id
                    && !mark.phase.is_empty()
                    && !keys.contains(&key)
                {
                    keys.push(key);
                }
            }
        }
        Ok(keys)
    }

    /// Al volver a conectar una cuenta, su lectura deja de constar como desconectada.
    pub(crate) fn clear_removed_read(&mut self, id: &str) -> AppResult<()> {
        for key in self.account_read_sources(id)? {
            if let Some(mut mark) = self.load_mark(&key)?.filter(|m| m.removed) {
                mark.removed = false;
                mark.target = id.to_owned();
                self.save_mark(&key, &mark)?;
            }
        }
        self.refresh_read_gate();
        Ok(())
    }

    /// Actualiza lo que las tareas en curso comprueban justo antes de llamar al proveedor.
    pub(crate) fn refresh_read_gate(&self) {
        if let (Ok(held), Ok(mut gate)) = (self.held_read_sources(), self.read_gate.lock()) {
            *gate = held;
        }
    }

    fn report_read_if_finished(&mut self, source: &str) -> AppResult<()> {
        let Some(mark) = self.load_mark(source)?.filter(|m| m.phase == "queued") else {
            return Ok(());
        };
        let _ = mark;
        // Con la última tarea ya cerrada se resume aunque el destino haya cambiado o se haya
        // detenido mientras tanto: no se pierde la entrada del Historial.
        let status = self.initial_read_status(source)?;
        if status.tasks.queued + status.tasks.running > 0 {
            return Ok(());
        }
        let event = if status.state == "done" {
            "done"
        } else {
            "done_with_errors"
        };
        self.report_read(source, event)
    }

    /// UNA entrada agregada en el Historial por lectura terminada, detenida o cancelada: fuente,
    /// proveedores, recuentos y si actuó el respaldo (D102, D133). Nunca una por tarea.
    fn report_read(&mut self, source: &str, event: &str) -> AppResult<()> {
        let Some(mut mark) = self.load_mark(source)? else {
            return Ok(());
        };
        if mark.audited.as_deref() == Some(event) {
            return Ok(());
        }
        let status = self.initial_read_status(source)?;
        let verb = match event {
            "done" => "terminada",
            "done_with_errors" => "terminada con errores",
            "stopped" => "detenida",
            _ => "cancelada al desconectar la cuenta",
        };
        let providers = if status.by_provider.is_empty() {
            "a ningún proveedor".to_owned()
        } else {
            status
                .by_provider
                .iter()
                .map(|p| {
                    let host = p
                        .host
                        .as_deref()
                        .map_or("en este equipo".to_owned(), |h| format!("servidor {h}"));
                    format!("{} {}, {}: {}", p.provider, p.model, host, p.documents)
                })
                .collect::<Vec<_>>()
                .join("; ")
        };
        let mut detail = format!(
            "Primera lectura de «{}» {verb}. Documentos enviados {providers}; {} omitidos por no parecer factura; {} sin leer por el tope; {} correos sin clasificar (no se enviaron); {} recientes dejados al flujo normal; {} con error",
            mark.label,
            status.documents_skipped,
            status.over_cap,
            status.emails_unclassified,
            status.recent_left_out,
            status.tasks.failed,
        );
        if status.fallback_used > 0 {
            detail.push_str(&format!(
                "; atendidos por la IA local de respaldo: {}",
                status.fallback_used
            ));
        }
        if status.actions == 0 {
            detail.push_str(". No se envió ningún correo ni se registró nada.");
        } else {
            detail.push('.');
        }
        self.store
            .record_note("initial_read", event, &Value::String(detail))?;
        mark.audited = Some(event.to_owned());
        self.save_mark(source, &mark)?;
        Ok(())
    }
}

/// Lo ya conocido antes de un paso: mensajes con tarea (y si es de la primera lectura), adjuntos
/// importados, huellas de documentos y cuántos correos y documentos tiene ya esta fuente.
#[derive(Default)]
struct ReadIndex {
    messages: HashMap<String, (i64, bool)>,
    attachments: HashSet<String>,
    hashes: HashSet<String>,
    emails: usize,
    documents: usize,
}

/// Archivos admitidos de una carpeta (sin subcarpetas), por nombre.
fn list_folder(path: &Path) -> Result<Vec<String>, ReadIssue> {
    let entries = fs::read_dir(path).map_err(|e| ReadIssue {
        code: "unavailable".into(),
        message: format!("no se pudo abrir la carpeta: {e}"),
    })?;
    let mut files: Vec<String> = entries
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter_map(|e| e.file_name().to_str().map(str::to_owned))
        .filter(|n| supported_document(n))
        .collect();
    files.sort();
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_are_read_from_the_prefix_and_the_edge_is_inclusive() {
        let today = day_of("2026-11-09").unwrap();
        assert_eq!(day_of("2026-11-09T10:00:00Z"), Some(today));
        assert_eq!(day_of("2026-11-10"), Some(today + 1));
        assert_eq!(day_of("1970-01-01"), Some(0));
        // Siete días: todavía reciente; ocho: se lee.
        assert!(is_recent(Some("2026-11-02"), today));
        assert!(!is_recent(Some("2026-11-01"), today));
        assert!(is_recent(Some("2026-11-02T23:59:59Z"), today));
        // Futura, ausente o ilegible: reciente, flujo normal.
        assert!(is_recent(Some("2027-01-01"), today));
        for bad in [
            None,
            Some(""),
            Some("ayer"),
            Some("2026-13-40"),
            Some("09/11/2026"),
        ] {
            assert!(is_recent(bad, today), "{bad:?}");
        }
    }

    #[test]
    fn keys_use_the_provider_and_the_normalized_address() {
        assert_eq!(
            account_key("google", "  Ana@Ejemplo.TEST "),
            account_key("google", "ana@ejemplo.test")
        );
        assert_ne!(
            account_key("google", "ana@ejemplo.test"),
            account_key("microsoft", "ana@ejemplo.test")
        );
        assert_ne!(
            account_key("google", "ana@ejemplo.test"),
            account_id_key("google", "google-1")
        );
    }
}
