//! Capa de aplicación de NestAdmin (flujos 1–5 de requisitos.md). Une la cola durable, la
//! IA, la validación de facturas, los simuladores y las reglas, sin depender de la ventana:
//! la interfaz Tauri solo llama a esta capa.
//!
//! El trabajo con IA se divide en tres pasos para que la llamada al modelo, que puede tardar
//! decenas de segundos, no bloquee el resto de la aplicación: `begin_next` toma la tarea y
//! prepara la solicitud, `Job::run` llama al proveedor sin acceso a `App`, y `finish` valida,
//! aplica reglas y deja la tarea completada, en revisión o fallida.

pub mod accounts;
pub mod panel;
pub mod refine;
pub mod rules;
pub mod suggest;

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use nestadmin_ai::tasks::{
    email_request, extraction_request, parse_email, parse_extraction, parse_other_invoices,
};
use nestadmin_ai::{AiError, GenerateRequest, GenerateResult, Provider, ProviderKind};
use nestadmin_core::{
    ActionRecord, ActionRequest, AuditEntry, Authorization, Connector, ConnectorError, CoreError,
    Rule, Store, Task, TaskError, TaskState,
};
use nestadmin_documents::{
    process_invoice, DuplicateRegistry, Evidenced, Invoice, Issue, RawInvoice, RecoveryReason,
};
use nestadmin_extract::{Extraction, Extractor};
use nestadmin_mail::{OAuthConfig, Tokens};
use nestadmin_simulators::{AdminSimulator, MailSimulator, Message, NewDraft};
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::accounts::{
    split_id, AccountFetch, AccountView, Accounts, AttachmentFetch, AttachmentIssue, FetchReport,
    MailAccountsView, MailEnv, MailIssue, MailProviderKind,
};
use crate::rules::{evaluate_result, RuleDecision, REGISTER_INVOICE, SEND_DRAFT};

pub const INVOICE: &str = "invoice";
pub const EMAIL: &str = "email";
/// Límite de tamaño de un documento importado.
const MAX_DOCUMENT_BYTES: usize = 50_000_000;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error(transparent)]
    Core(#[from] CoreError),
    #[error("conector: {0}")]
    Connector(#[from] ConnectorError),
    #[error("archivo: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Invalid(String),
    #[error("el contenido cambió (versión {current}, se aprobó la {approved}); revíselo de nuevo")]
    StaleVersion { approved: i64, current: i64 },
}

/// La interfaz recibe el error como texto; nunca contiene credenciales.
impl Serialize for AppError {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

pub type AppResult<T> = Result<T, AppError>;

/// Ubicación de los datos locales; nunca dentro del repositorio ni junto a evaluaciones.
#[derive(Debug, Clone)]
pub struct AppPaths {
    pub data_dir: PathBuf,
}

impl AppPaths {
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            data_dir: data_dir.into(),
        }
    }
    fn db(&self) -> PathBuf {
        self.data_dir.join("nestadmin.db")
    }
    fn mail(&self) -> PathBuf {
        self.data_dir.join("simulador-correo.json")
    }
    /// Lista de cuentas y registro local de borradores de cada cuenta real.
    fn mail_accounts(&self) -> PathBuf {
        self.data_dir.join("correo")
    }
    fn admin(&self) -> PathBuf {
        self.data_dir.join("simulador-administracion.json")
    }
    fn documents(&self) -> PathBuf {
        self.data_dir.join("documentos")
    }
}

/// Proveedor activo, para mostrarlo en la interfaz.
#[derive(Debug, Clone, Serialize)]
pub struct ProviderInfo {
    pub provider: ProviderKind,
    pub model: String,
}

/// Por qué las tareas con IA esperan en la cola sin gastar intentos (D100, D101). Mientras
/// haya una espera, nada se envía a ningún proveedor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AiWait {
    /// No hay proveedor elegido (por ejemplo, una configuración antigua con Groq).
    NeedsChoice,
    /// El proveedor externo elegido no tiene clave guardada.
    NeedsKey,
    /// El destino de los datos no está confirmado.
    NeedsConsent,
    /// El proveedor rechazó la clave (`unauthorized`): revocada o incorrecta.
    InvalidKey,
}

impl AiWait {
    pub fn message(self) -> &'static str {
        match self {
            Self::NeedsChoice => "Falta configurar la IA: elija un proveedor en Configuración",
            Self::NeedsKey => {
                "Falta configurar la IA: guarde la clave del proveedor en Configuración"
            }
            Self::NeedsConsent => {
                "Falta configurar la IA: confirme el destino de los datos en Configuración"
            }
            Self::InvalidKey => {
                "La clave de IA no es válida o fue revocada: corríjala en Configuración"
            }
        }
    }
}

/// Proveedor local de respaldo (D101): `None` si no está preparado. Nunca descarga ni instala
/// nada; quien lo construye decide si la IA local está lista y si el respaldo está permitido.
pub type LocalFallback = Arc<dyn Fn() -> Option<Arc<dyn Provider>> + Send + Sync>;

/// Tras un `unavailable` del externo, las tareas siguientes van directas al local este tiempo
/// (D101). `timeout` y `rate_limited` no abren periodo.
const FALLBACK_PERIOD: Duration = Duration::from_secs(10 * 60);

pub struct App {
    store: Store,
    mail: Accounts,
    admin: AdminSimulator,
    provider: Arc<dyn Provider>,
    documents: PathBuf,
    registry: DuplicateRegistry,
    extractor: Arc<Extractor>,
    /// Mientras sea `true`, no empieza ninguna tarea nueva (actualización pendiente, D63).
    paused: bool,
    /// Si es `Some`, las tareas que necesitan la IA esperan en la cola (D101).
    ai_wait: Option<AiWait>,
    /// Respaldo del proveedor externo elegido al local (D101); `None` desactiva el respaldo.
    fallback: Option<LocalFallback>,
    /// Hasta cuándo las tareas van directas al local tras un `unavailable` del externo.
    fallback_until: Arc<Mutex<Option<Instant>>>,
    /// Hay un afinado de sugerencias con IA en curso (D134): no se lanza otro.
    refining: Arc<std::sync::atomic::AtomicBool>,
    /// Mayor id de señal con el que el lote automático ya comprobó que no había nada que afinar.
    auto_skip: Option<i64>,
    /// Un reintento vencido de otros datos que ya no aplica, descartado con este id de señal.
    retry_obsolete: Option<i64>,
    /// En esta sesión se corrigió la clave de IA o cambió el proveedor tras un `unauthorized`.
    key_fixed: bool,
    /// Cómo se afinan las sugerencias según el canal (D134): bajo demanda o por lotes.
    refine_mode: refine::RefineMode,
}

/// Si se puede instalar una actualización sin cortar trabajo (actualizaciones.md, paso 5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UpdateReadiness {
    /// No hay ninguna tarea ejecutándose: instalar no corta nada.
    pub ready: bool,
    /// Tareas ejecutándose (la IA, el OCR o un envío en curso).
    pub running: usize,
    /// Tareas en cola: se conservan y se procesan tras actualizar (R02).
    pub queued: usize,
    /// Tareas pendientes de revisión: se conservan.
    pub awaiting_review: usize,
    /// Si la cola está en pausa: no empieza ninguna tarea nueva.
    pub paused: bool,
}

/// Trabajo de una tarea: una solicitud lista, un documento del que extraer el texto o, si ya
/// se intentó ejecutar su acción, el resultado guardado entonces (D50), sin volver a la IA.
enum Prepared {
    Request(GenerateRequest),
    Document(PathBuf),
    /// Una factura más de un documento con varias (D68): sus campos ya los dio la IA al
    /// procesar la primera; solo se vuelve a leer el documento para localizar cada campo.
    Split(PathBuf, GenerateResult),
    Resume(Value),
}

/// Lo que devuelve el trabajo de una tarea.
enum Outcome {
    Generated(GenerateResult),
    Resumed(Value),
}

/// Estado de los intentos anteriores de la acción de una tarea (D50).
enum Previous {
    /// No queda ningún intento sin cerrar: se puede ejecutar.
    Clear,
    /// Una versión consta realizada, con la autorización que figura en su acción (D129).
    Done {
        version: i64,
        receipt: Value,
        authorization: String,
    },
    /// Un intento no se pudo comprobar: el conector falló o el borrador ya no existe.
    Unknown { version: i64, error: ConnectorError },
}

/// Campos ya extraídos de una factura separada de un documento con varias (D68).
fn split_preset(input: &Value) -> Option<GenerateResult> {
    let preset = input.get("preset")?;
    Some(GenerateResult {
        request_id: String::new(),
        provider: serde_json::from_value(preset["provider"].clone()).ok()?,
        model: preset["model"].as_str().unwrap_or("").to_owned(),
        output: preset["output"].clone(),
        latency_ms: 0,
        usage: Default::default(),
    })
}

/// Una factura separada cuyo `preset` no se puede leer, por ejemplo porque se extrajo con un
/// proveedor que ya no existe (Groq, retirado en D100). No se procesa con el proveedor actual
/// como si nada: se falla de forma explícita y recuperable con otro intento del documento.
fn split_preset_unreadable(input: &Value) -> bool {
    input.get("preset").is_some() && split_preset(input).is_none()
}

/// Motivo de revisión de una tarea cuyo intento anterior no se confirmó.
const RESUMED_REASON: &str =
    "el intento anterior no se confirmó; se conserva el contenido aprobado, sin pedir otro a la IA";
/// Motivo de revisión cuando el borrador anterior ya no existe y no consta enviado.
const MISSING_REASON: &str = "el borrador del intento anterior ya no existe y no consta enviado; confirme que no se envió antes de preparar otro";

/// Tarea tomada de la cola con su trabajo preparado.
pub struct Job {
    pub task: Task,
    provider: Arc<dyn Provider>,
    fallback: Option<LocalFallback>,
    fallback_until: Arc<Mutex<Option<Instant>>>,
    extractor: Arc<Extractor>,
    prepared: Result<Prepared, TaskError>,
}

/// Respuesta del proveedor (o el motivo del fallo) lista para `finish`.
pub struct Completed {
    task: Task,
    outcome: Result<Outcome, TaskError>,
    /// Texto y páginas del documento (método de cada una: texto, PDF u OCR).
    extraction: Option<Extraction>,
    /// El proveedor devolvió `unauthorized`. Se marca aquí y no por el código del error, porque
    /// los conectores de correo también usan `unauthorized` y esos fallos siguen siendo fallos.
    ai_unauthorized: bool,
    /// `{from, reason}` si la respuesta la dio el respaldo local (D101).
    fallback: Option<Value>,
}

/// Resultado de pedir la generación, con el respaldo local si procede.
struct Attempt {
    result: Result<GenerateResult, TaskError>,
    /// `{from, reason}` si respondió el respaldo local.
    fallback: Option<Value>,
    /// El proveedor elegido devolvió `unauthorized` (y no hubo respaldo).
    unauthorized: bool,
}

/// Errores del externo que activan el respaldo local (D101). `unauthorized` e
/// `invalid_response` no: el primero pide corregir la clave y el segundo no es un fallo de
/// disponibilidad.
fn triggers_fallback(error: &AiError) -> bool {
    matches!(error.code(), "unavailable" | "timeout" | "rate_limited")
}

fn failure(error: &AiError) -> TaskError {
    TaskError {
        code: error.code().to_owned(),
        message: error.to_string(),
        recoverable: true,
    }
}

fn fallback_note(from: ProviderKind, reason: &str) -> Value {
    json!({ "from": from, "reason": reason })
}

/// Genera con el proveedor elegido. Solo si es externo y hay respaldo: ante `unavailable`,
/// `timeout` o `rate_limited` repite UNA vez con el local; durante el periodo abierto por un
/// `unavailable` va directo al local. El único camino de respaldo es externo a local: con la
/// IA local elegida no hay ninguno, ni existe código que llame a otro proveedor externo.
fn generate_with_fallback(
    provider: &dyn Provider,
    fallback: Option<&LocalFallback>,
    until: &Mutex<Option<Instant>>,
    request: &GenerateRequest,
) -> Attempt {
    let from = provider.kind();
    let fallback = fallback.filter(|_| from != ProviderKind::Ollama);
    // El respaldo es siempre la IA local: lo que no sea Ollama se descarta, de modo que ningún
    // camino lleva de un externo a otro.
    let local_provider =
        |local: &LocalFallback| local().filter(|p| p.kind() == ProviderKind::Ollama);
    let mut local_failure: Option<AiError> = None;
    if let Some(local) = fallback {
        let active = until
            .lock()
            .map(|u| u.is_some_and(|end| Instant::now() < end))
            .unwrap_or(false);
        if active {
            if let Some(local) = local_provider(local) {
                match local.generate(request) {
                    Ok(result) => {
                        return Attempt {
                            result: Ok(result),
                            fallback: Some(fallback_note(from, "unavailable")),
                            unauthorized: false,
                        }
                    }
                    Err(error) => local_failure = Some(error),
                }
            }
        }
    }
    match provider.generate(request) {
        Ok(result) => Attempt {
            result: Ok(result),
            fallback: None,
            unauthorized: false,
        },
        Err(error) => {
            let Some(local) = fallback.filter(|_| triggers_fallback(&error)) else {
                return Attempt {
                    result: Err(failure(&error)),
                    fallback: None,
                    unauthorized: error == AiError::Unauthorized,
                };
            };
            let outcome = match local_failure {
                Some(previous) => Err(previous),
                None => match local_provider(local) {
                    Some(local) => local.generate(request),
                    // Sin IA local preparada: el error del externo, como sin respaldo.
                    None => {
                        return Attempt {
                            result: Err(failure(&error)),
                            fallback: None,
                            unauthorized: false,
                        }
                    }
                },
            };
            match outcome {
                Ok(result) => {
                    if error.code() == "unavailable" {
                        if let Ok(mut until) = until.lock() {
                            *until = Some(Instant::now() + FALLBACK_PERIOD);
                        }
                    }
                    Attempt {
                        result: Ok(result),
                        fallback: Some(fallback_note(from, error.code())),
                        unauthorized: false,
                    }
                }
                Err(local_error) => Attempt {
                    result: Err(TaskError {
                        code: error.code().to_owned(),
                        message: format!("{error}; respaldo local: {local_error}"),
                        recoverable: true,
                    }),
                    fallback: None,
                    unauthorized: false,
                },
            }
        }
    }
}

impl Job {
    /// Extrae el texto si hace falta (PDF u OCR, localmente) y llama al proveedor elegido.
    /// Un error se devuelve tal cual: no hay otro proveedor ni extracción parcial.
    pub fn run(self) -> Completed {
        let Job {
            task,
            provider,
            fallback,
            fallback_until,
            extractor,
            prepared,
        } = self;
        let mut extracted = None;
        let mut ai_unauthorized = false;
        let mut used_fallback = None;
        let outcome = prepared.and_then(|prepared| {
            let request = match prepared {
                Prepared::Resume(result) => return Ok(Outcome::Resumed(result)),
                Prepared::Split(path, generated) => {
                    extracted = extractor.extract(&path).ok();
                    return Ok(Outcome::Generated(generated));
                }
                Prepared::Request(request) => request,
                Prepared::Document(path) => {
                    let extraction = extractor
                        .extract(&path)
                        .map_err(|e| task_error(e.code(), e.to_string(), e.recoverable()))?;
                    let request = extraction_request(&task.id.to_string(), &extraction.text);
                    extracted = Some(extraction);
                    request
                }
            };
            let attempt =
                generate_with_fallback(&*provider, fallback.as_ref(), &fallback_until, &request);
            ai_unauthorized = attempt.unauthorized;
            used_fallback = attempt.fallback;
            attempt.result.map(Outcome::Generated)
        });
        Completed {
            task,
            outcome,
            extraction: extracted,
            ai_unauthorized,
            fallback: used_fallback,
        }
    }
}

fn task_error(code: &str, message: impl Into<String>, recoverable: bool) -> TaskError {
    TaskError {
        code: code.to_owned(),
        message: message.into(),
        recoverable,
    }
}

impl App {
    /// Abre los datos locales solo con el correo simulado y sin proveedores reales.
    pub fn open(
        paths: &AppPaths,
        provider: Arc<dyn Provider>,
        extractor: Arc<Extractor>,
    ) -> AppResult<Self> {
        Self::open_with_mail(paths, provider, extractor, MailEnv::simulator_only())
    }

    /// Abre los datos locales y recupera el trabajo interrumpido: las tareas en curso
    /// vuelven a la cola y las acciones pendientes se reconcilian con los conectores (cada
    /// envío, con la cuenta que lo hizo).
    pub fn open_with_mail(
        paths: &AppPaths,
        provider: Arc<dyn Provider>,
        extractor: Arc<Extractor>,
        mail: MailEnv,
    ) -> AppResult<Self> {
        fs::create_dir_all(paths.documents())?;
        let simulator = MailSimulator::open(&paths.mail())?;
        let mut app = Self {
            store: Store::open(&paths.db())?,
            mail: Accounts::open(&paths.mail_accounts(), simulator, mail)?,
            admin: AdminSimulator::open(&paths.admin())?,
            provider,
            documents: paths.documents(),
            registry: DuplicateRegistry::new(),
            extractor,
            paused: false,
            ai_wait: None,
            fallback: None,
            fallback_until: Arc::new(Mutex::new(None)),
            refining: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            auto_skip: None,
            retry_obsolete: None,
            key_fixed: false,
            refine_mode: refine::RefineMode::of_build(),
        };
        app.store.recover()?;
        app.store.reconcile_actions(SEND_DRAFT, &app.mail)?;
        app.store.reconcile_actions(REGISTER_INVOICE, &app.admin)?;
        app.complete_confirmed_reviews()?;
        app.rebuild_registry()?;
        Ok(app)
    }

    /// Una tarea que seguía en revisión cuando se cortó su acción y cuya acción consta ya
    /// confirmada (por ejemplo, al reconciliar al abrir) se completa: no se vuelve a pedir
    /// aprobación de algo que ya ocurrió.
    fn complete_confirmed_reviews(&mut self) -> AppResult<()> {
        for task in self.store.tasks_in(TaskState::AwaitingReview)? {
            let kind = action_kind(&task);
            let done = self
                .store
                .task_actions(task.id)?
                .into_iter()
                .rfind(|a| a.kind == kind && a.status == "confirmed");
            if let Some(done) = done {
                let result = task.result.clone().unwrap_or(Value::Null);
                let receipt = done.receipt.unwrap_or(Value::Null);
                self.complete_with(task.id, result, done.version, receipt)?;
            }
        }
        Ok(())
    }

    /// Resultado guardado de una tarea cuya acción ya se intentó (D50): se reutiliza al
    /// reprocesarla. También cubre tareas anteriores a D50 con acciones registradas.
    fn attempted_result(&self, task: &Task) -> AppResult<Option<Value>> {
        let Some(result) = task.result.as_ref().filter(|r| r.get("action").is_some()) else {
            return Ok(None);
        };
        if result.get("attempt").is_some() || !self.store.task_actions(task.id)?.is_empty() {
            return Ok(Some(result.clone()));
        }
        Ok(None)
    }

    /// Comprueba con el conector los intentos anteriores de la acción de una tarea. La versión
    /// `executing` se excluye porque `perform_action` ya la consulta antes de repetirla. Con
    /// `supersede`, los intentos que el conector no ve realizados se cierran como sustituidos
    /// con esa autorización; sin ella se dejan como están.
    fn check_previous(
        &mut self,
        task: &Task,
        executing: Option<i64>,
        supersede: Option<&Authorization>,
    ) -> AppResult<Previous> {
        let kind = action_kind(task);
        let actions: Vec<ActionRecord> = self
            .store
            .task_actions(task.id)?
            .into_iter()
            .filter(|a| a.kind == kind)
            .collect();
        if let Some(done) = actions.iter().rev().find(|a| a.status == "confirmed") {
            return Ok(Previous::Done {
                version: done.version,
                receipt: done.receipt.clone().unwrap_or(Value::Null),
                authorization: done.authorization.clone(),
            });
        }
        let mut unsent = Vec::new();
        let mut unknown = None;
        for action in actions
            .iter()
            .filter(|a| a.is_unsettled() && Some(a.version) != executing)
        {
            let found = match kind {
                REGISTER_INVOICE => self.admin.lookup(&action.idempotency_key),
                _ => self.mail.lookup(&action.idempotency_key),
            };
            match found {
                Ok(Some(receipt)) => {
                    self.store.reconcile_action(task.id, action, &receipt)?;
                    return Ok(Previous::Done {
                        version: action.version,
                        receipt,
                        authorization: action.authorization.clone(),
                    });
                }
                Ok(None) => unsent.push(action),
                Err(error) => {
                    unknown.get_or_insert((action.version, error));
                }
            }
        }
        if let Some((version, error)) = unknown {
            return Ok(Previous::Unknown { version, error });
        }
        if let Some(authorization) = supersede {
            for action in unsent {
                self.store.supersede_action(
                    task.id,
                    action,
                    authorization,
                    "el conector no lo ve realizado; lo sustituye otra versión",
                )?;
            }
        }
        Ok(Previous::Clear)
    }

    /// Completa la tarea con el recibo de una versión ya realizada. Si no es la versión que
    /// se muestra, se recupera el contenido de ese intento y se indica cuál salió.
    fn complete_with(
        &mut self,
        task_id: i64,
        mut result: Value,
        version: i64,
        receipt: Value,
    ) -> AppResult<Task> {
        if result["version"].as_i64() != Some(version) {
            if result["attempt"]["version"].as_i64() == Some(version)
                && !result["attempt"]["draft"].is_null()
            {
                result["draft"] = result["attempt"]["draft"].clone();
                result["version"] = json!(version);
            } else {
                result["sent_version"] = json!(version);
            }
        }
        result["receipt"] = receipt;
        // Para la ventana: se completó con un intento anterior, no con una ejecución nueva.
        result["recovered"] = json!({ "version": version });
        if let Some(fields) = result.as_object_mut() {
            fields.remove("recovery");
        }
        Ok(self.store.complete(task_id, &result)?)
    }

    /// Deja la tarea en revisión con el contenido dado, desde `running` o desde la revisión.
    fn back_to_review(&mut self, task: &Task, result: &Value) -> AppResult<Task> {
        let current = self.store.task(task.id)?;
        Ok(match current.state {
            TaskState::AwaitingReview => self.store.revise(task.id, result)?,
            _ => self.store.await_review(task.id, result)?,
        })
    }

    /// Reproceso de una tarea cuya acción ya se intentó: si consta realizada se completa; si
    /// no, vuelve a revisión con el mismo contenido y versión, explicando lo ocurrido. Nunca se
    /// ejecuta nada sin una nueva aprobación.
    fn resume(&mut self, task: Task, mut result: Value) -> AppResult<Task> {
        let attempted = result["attempt"]["version"]
            .as_i64()
            .or_else(|| result["version"].as_i64())
            .unwrap_or(1);
        let (error, missing) = match self.check_previous(&task, None, None)? {
            Previous::Done {
                version,
                receipt,
                authorization: done_by,
            } => {
                // Misma regla que en `execute` (D129): si lo envió la regla, consta la regla.
                if done_by.starts_with("regla ") && result["rule_attempt"]["type"] == "rule" {
                    result["authorization"] = result["rule_attempt"].clone();
                }
                return self.complete_with(task.id, result, version, receipt);
            }
            Previous::Unknown { error, .. } => {
                let missing = error.code == nestadmin_simulators::codes::NOT_FOUND;
                (Some(error.code), missing)
            }
            Previous::Clear => (task.error.as_ref().map(|e| e.code.clone()), false),
        };
        result["review_reason"] = json!(if missing {
            MISSING_REASON
        } else {
            RESUMED_REASON
        });
        result["recovery"] = json!({
            "version": attempted,
            "error": error,
            "needs_confirmation": missing,
        });
        self.back_to_review(&task, &result)
    }

    /// Vuelve a cargar en orden las facturas ya procesadas para seguir detectando duplicados.
    fn rebuild_registry(&mut self) -> AppResult<()> {
        let mut tasks = self.store.all_tasks()?;
        tasks.reverse();
        for task in tasks.iter().filter(|t| t.kind == INVOICE) {
            if let Some(fields) = task.result.as_ref().map(|r| &r["fields"]) {
                let invoice = Invoice::from_raw(&raw_from_fields(fields));
                self.registry.check_and_add(task.id.to_string(), &invoice);
            }
        }
        Ok(())
    }

    pub fn provider(&self) -> ProviderInfo {
        ProviderInfo {
            provider: self.provider.kind(),
            model: self.provider.model().to_owned(),
        }
    }

    /// Cambio explícito de proveedor; afecta a las tareas que empiecen después.
    pub fn select_provider(&mut self, provider: Arc<dyn Provider>) {
        self.provider = provider;
        self.key_fixed = true;
        // El consentimiento del afinado automático es de un destino concreto (D134).
        self.drop_consent_if_destination_changed();
        self.expire_fallback_period();
    }

    /// Fija o quita la espera de IA (D101). Con una espera, las tareas que necesitan el
    /// proveedor se quedan en `queued` sin gastar intentos; al quitarla se retoman.
    pub fn set_ai_wait(&mut self, wait: Option<AiWait>) {
        if wait.is_none() && self.ai_wait == Some(AiWait::InvalidKey) {
            self.key_fixed = true;
        }
        self.ai_wait = wait;
    }

    pub fn ai_wait(&self) -> Option<AiWait> {
        self.ai_wait
    }

    /// Activa o desactiva el respaldo del proveedor externo al local (D101). El respaldo solo
    /// se usa si el proveedor activo es externo; con `None` no hay ninguno.
    pub fn set_local_fallback(&mut self, fallback: Option<LocalFallback>) {
        self.fallback = fallback;
        self.expire_fallback_period();
    }

    /// Si las tareas van directas al local por un `unavailable` reciente del externo.
    pub fn fallback_period_active(&self) -> bool {
        self.fallback_until
            .lock()
            .map(|u| u.is_some_and(|end| Instant::now() < end))
            .unwrap_or(false)
    }

    /// Cierra el periodo de respaldo (también al cambiar de proveedor). Las pruebas lo usan
    /// para simular que pasaron los 10 minutos.
    #[doc(hidden)]
    pub fn expire_fallback_period(&self) {
        if let Ok(mut until) = self.fallback_until.lock() {
            *until = None;
        }
    }

    /// Encola un documento de texto.
    pub fn import_document(&mut self, name: &str, text: &str) -> AppResult<Task> {
        if text.trim().is_empty() {
            return Err(AppError::Invalid("el documento está vacío".into()));
        }
        let origin = json!({ "kind": "file", "name": name, "original_path": null });
        self.store_document(name, "txt", text.as_bytes(), origin)
    }

    /// Encola un archivo `.txt` o `.pdf` del que no se conoce la ruta original.
    pub fn import_file(&mut self, name: &str, bytes: &[u8]) -> AppResult<Task> {
        self.import_file_from(name, None, bytes)
    }

    /// Lee un archivo del disco y lo encola anotando su ruta original como origen.
    pub fn import_path(&mut self, path: &Path) -> AppResult<Task> {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| AppError::Invalid("el archivo no tiene un nombre válido".into()))?
            .to_owned();
        let size = fs::metadata(path)?.len();
        if size > MAX_DOCUMENT_BYTES as u64 {
            return Err(AppError::Invalid(format!(
                "«{name}» supera el tamaño máximo de {} MB",
                MAX_DOCUMENT_BYTES / 1_000_000
            )));
        }
        let bytes = fs::read(path)?;
        self.import_file_from(&name, Some(path), &bytes)
    }

    /// Encola un archivo `.txt` o `.pdf`. Se guarda una copia en los datos locales junto con
    /// su origen (nombre, ruta original si se conoce, fecha de importación y SHA-256); la
    /// extracción (con OCR si hace falta) se hace al procesar la tarea.
    pub fn import_file_from(
        &mut self,
        name: &str,
        original: Option<&Path>,
        bytes: &[u8],
    ) -> AppResult<Task> {
        let extension = check_document(name, bytes)?;
        let origin = json!({
            "kind": "file",
            "name": name,
            // Ruta tal como la dio el sistema; `null` si no se conoce (texto pegado o un
            // selector que no la facilita).
            "original_path": original.map(|p| p.display().to_string()),
        });
        self.store_document(name, &extension, bytes, origin)
    }

    /// Encola un adjunto de un correo ya recibido (`message_task`, la tarea de ese correo).
    /// El origen copia la cuenta, el remitente y el asunto guardados en esa tarea, de modo que
    /// la factura dice de qué buzón y de qué mensaje salió (D49).
    pub fn import_attachment(
        &mut self,
        message_task: i64,
        name: &str,
        bytes: &[u8],
    ) -> AppResult<Task> {
        self.import_attachment_keyed(message_task, name, bytes, "", "")
    }

    fn import_attachment_keyed(
        &mut self,
        message_task: i64,
        name: &str,
        bytes: &[u8],
        attachment_key: &str,
        attachment_id: &str,
    ) -> AppResult<Task> {
        let task = self.store.task(message_task)?;
        if task.kind != EMAIL {
            return Err(AppError::Invalid(format!(
                "la tarea {message_task} no es un correo"
            )));
        }
        let extension = check_document(name, bytes)?;
        let input: Value = serde_json::from_str(&task.input_ref).unwrap_or(Value::Null);
        let message = &input["message"];
        let account = match input.get("origin").filter(|o| !o.is_null()) {
            Some(origin) => origin.clone(),
            None => {
                let id = input["message_id"].as_str().unwrap_or("");
                serde_json::to_value(self.mail.origin(split_id(id).0)).unwrap_or_default()
            }
        };
        let origin = json!({
            "kind": "email",
            "account": account,
            "message_id": input["message_id"],
            "from": message["from"],
            "subject": message["subject"],
            "attachment": name,
        });
        let mut origin = origin;
        if !attachment_key.is_empty() {
            origin["attachment_key"] = json!(attachment_key);
            origin["attachment_id"] = json!(attachment_id);
        }
        self.store_document(name, &extension, bytes, origin)
    }

    /// Guarda la copia y encola la tarea. `origin` dice de dónde vino; aquí se añaden la
    /// fecha de importación, el tamaño y la huella SHA-256 del contenido.
    fn store_document(
        &mut self,
        name: &str,
        extension: &str,
        bytes: &[u8],
        mut origin: Value,
    ) -> AppResult<Task> {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).ok();
        let stamp = now.map(|d| d.as_nanos()).unwrap_or(0);
        let path = self.documents.join(format!("{stamp}.{extension}"));
        fs::write(&path, bytes)?;
        origin["imported_at"] = json!(now.map(|d| d.as_secs()));
        origin["sha256"] = json!(hex(&Sha256::digest(bytes)));
        origin["size"] = json!(bytes.len());
        let input = json!({ "document": name, "path": path, "origin": origin });
        Ok(self.store.enqueue(INVOICE, &input.to_string())?)
    }

    /// Copia guardada del documento de una tarea, para abrirla con el visor del sistema. Solo
    /// se devuelve si está dentro de la carpeta de documentos de NestAdmin y sigue existiendo.
    pub fn document_file(&mut self, task_id: i64) -> AppResult<PathBuf> {
        let task = self.store.task(task_id)?;
        if task.kind != INVOICE {
            return Err(AppError::Invalid(format!(
                "la tarea {task_id} no tiene documento"
            )));
        }
        let input: Value = serde_json::from_str(&task.input_ref).unwrap_or(Value::Null);
        let missing = || {
            AppError::Invalid(format!(
                "la copia guardada del documento de la tarea {task_id} ya no está"
            ))
        };
        let path = input["path"]
            .as_str()
            .map(PathBuf::from)
            .ok_or_else(missing)?;
        let folder = fs::canonicalize(&self.documents)?;
        let file = fs::canonicalize(&path).map_err(|_| missing())?;
        if !file.starts_with(&folder) || !file.is_file() {
            return Err(AppError::Invalid(
                "el documento no está en la carpeta de NestAdmin".into(),
            ));
        }
        Ok(file)
    }

    /// Consulta cada cuenta (como mucho el límite de mensajes por cuenta) y encola los
    /// mensajes que aún no tienen tarea. El mensaje se guarda en la tarea para no volver a
    /// pedirlo al proveedor. El fallo de una cuenta queda en el informe y no detiene a las
    /// demás.
    pub fn fetch_mail(&mut self) -> AppResult<FetchReport> {
        // Mensajes con tarea (ID con cuenta → tarea) y adjuntos ya importados.
        let mut known: HashMap<String, i64> = HashMap::new();
        let mut imported: HashSet<String> = HashSet::new();
        for task in self.store.all_tasks()? {
            let input: Value = serde_json::from_str(&task.input_ref).unwrap_or(Value::Null);
            match task.kind.as_str() {
                EMAIL => {
                    if let Some(id) = input["message_id"].as_str() {
                        let (account, inner) = split_id(id);
                        known.insert(accounts::join_id(account, inner), task.id);
                    }
                }
                INVOICE => {
                    if let Some(key) = input["origin"]["attachment_key"].as_str() {
                        imported.insert(key.to_owned());
                    }
                }
                _ => {}
            }
        }
        let mut report = FetchReport {
            added: 0,
            limit: self.mail.limit(),
            accounts: Vec::new(),
        };
        for (account, outcome) in self.mail.read_all() {
            let fetch = match outcome {
                Ok(messages) => {
                    let mut added = 0;
                    let mut attachments = AttachmentFetch::default();
                    for message in &messages {
                        let task_id = match known.get(&message.id) {
                            Some(id) => *id,
                            None => {
                                let input = json!({
                                    "message_id": message.id,
                                    "account": account,
                                    "origin": self.mail.origin(&account),
                                    "message": message,
                                });
                                let task = self.store.enqueue(EMAIL, &input.to_string())?;
                                known.insert(message.id.clone(), task.id);
                                added += 1;
                                task.id
                            }
                        };
                        self.import_attachments(task_id, message, &mut imported, &mut attachments)?;
                    }
                    AccountFetch {
                        account,
                        read: messages.len(),
                        added,
                        error: None,
                        attachments,
                    }
                }
                Err(error) => AccountFetch {
                    account,
                    read: 0,
                    added: 0,
                    error: Some(MailIssue::from(&error)),
                    attachments: AttachmentFetch::default(),
                },
            };
            report.added += fetch.added;
            self.mail.record_fetch(fetch.clone());
            report.accounts.push(fetch);
        }
        Ok(report)
    }

    /// Importa como documentos los adjuntos admitidos de un mensaje que aún no se importaron.
    /// La clave de un adjunto es el mensaje, su nombre y su tamaño, que no cambian entre
    /// consultas (el identificador de Gmail sí puede cambiar). Un fallo de descarga queda en
    /// el informe y se reintenta en la consulta siguiente; nunca se pierde en silencio.
    fn import_attachments(
        &mut self,
        message_task: i64,
        message: &Message,
        imported: &mut HashSet<String>,
        report: &mut AttachmentFetch,
    ) -> AppResult<()> {
        for attachment in &message.attachments {
            let name = attachment_name(attachment);
            let key = format!(
                "{}#{}#{}",
                message.id,
                name,
                attachment.size.map(|s| s.to_string()).unwrap_or_default()
            );
            if imported.contains(&key) {
                continue;
            }
            if !supported_document(&name) {
                report.skipped.push(name);
                continue;
            }
            let fail = |code: &str, message: String| AttachmentIssue {
                name: name.clone(),
                code: code.to_owned(),
                message,
            };
            if attachment
                .size
                .is_some_and(|s| s > MAX_DOCUMENT_BYTES as u64)
            {
                report.failed.push(fail(
                    "invalid_input",
                    format!(
                        "supera el tamaño máximo de {} MB",
                        MAX_DOCUMENT_BYTES / 1_000_000
                    ),
                ));
                continue;
            }
            let bytes = match self.mail.fetch_attachment(&message.id, &attachment.id) {
                Ok(bytes) => bytes,
                Err(e) => {
                    report.failed.push(fail(&e.code, e.message));
                    continue;
                }
            };
            match self.import_attachment_keyed(message_task, &name, &bytes, &key, &attachment.id) {
                Ok(_) => {
                    imported.insert(key);
                    report.imported += 1;
                }
                Err(AppError::Invalid(reason)) => report.failed.push(fail("invalid_input", reason)),
                Err(other) => return Err(other),
            }
        }
        Ok(())
    }

    /// Mensaje de una tarea de correo: el guardado al encolarla o, en tareas anteriores a las
    /// cuentas, el que devuelva su cuenta.
    fn task_message(&self, input: &Value) -> Result<Option<Message>, ConnectorError> {
        if let Ok(message) = serde_json::from_value::<Message>(input["message"].clone()) {
            return Ok(Some(message));
        }
        self.mail
            .find_message(input["message_id"].as_str().unwrap_or(""))
    }

    /// Toma la siguiente tarea y prepara su solicitud. `None` si la cola está vacía.
    pub fn begin_next(&mut self) -> AppResult<Option<Job>> {
        if self.paused {
            return Ok(None);
        }
        let claimed = if self.ai_wait.is_none() {
            self.store.claim_next()?
        } else {
            // Sin IA utilizable solo avanzan las tareas que no la necesitan: reanudar una
            // acción ya intentada (D50) o una factura ya separada (D68).
            let mut found = None;
            for queued in self.store.tasks_in(TaskState::Queued)? {
                let input: Value = serde_json::from_str(&queued.input_ref).unwrap_or(Value::Null);
                if self.attempted_result(&queued)?.is_some() || split_preset(&input).is_some() {
                    found = Some(queued.id);
                    break;
                }
            }
            match found {
                Some(id) => Some(self.store.claim(id)?),
                None => None,
            }
        };
        let Some(task) = claimed else {
            return Ok(None);
        };
        let input: Value = serde_json::from_str(&task.input_ref).unwrap_or(Value::Null);
        let id = task.id.to_string();
        if let Some(result) = self.attempted_result(&task)? {
            return Ok(Some(Job {
                task,
                provider: Arc::clone(&self.provider),
                fallback: self.fallback.clone(),
                fallback_until: Arc::clone(&self.fallback_until),
                extractor: Arc::clone(&self.extractor),
                prepared: Ok(Prepared::Resume(result)),
            }));
        }
        let prepared = match task.kind.as_str() {
            INVOICE if split_preset_unreadable(&input) => Err(task_error(
                "preset_unreadable",
                "esta factura separada se extrajo con un proveedor de IA que ya no existe (Groq, retirado); descarte la tarea y vuelva a importar el documento",
                false,
            )),
            INVOICE => match input["path"].as_str() {
                Some(path) => match split_preset(&input) {
                    Some(generated) => Ok(Prepared::Split(PathBuf::from(path), generated)),
                    None => Ok(Prepared::Document(PathBuf::from(path))),
                },
                None => Err(task_error(
                    "document_unreadable",
                    "la tarea no indica el documento",
                    false,
                )),
            },
            EMAIL => match self.task_message(&input) {
                Err(e) => Err(task_error(&e.code, e.message, true)),
                Ok(found) => match found {
                    Some(Message {
                        body: Some(body),
                        subject,
                        ..
                    }) => {
                        let text = match subject {
                            Some(subject) => format!("Asunto: {subject}\n\n{body}"),
                            None => body,
                        };
                        Ok(Prepared::Request(email_request(&id, &text)))
                    }
                    Some(_) => Err(task_error(
                        "empty_message",
                        "el mensaje no tiene cuerpo",
                        false,
                    )),
                    None => Err(task_error("not_found", "el mensaje ya no existe", false)),
                },
            },
            other => Err(task_error(
                "invalid_input",
                format!("tipo de tarea desconocido: {other}"),
                false,
            )),
        };
        Ok(Some(Job {
            task,
            provider: Arc::clone(&self.provider),
            fallback: self.fallback.clone(),
            fallback_until: Arc::clone(&self.fallback_until),
            extractor: Arc::clone(&self.extractor),
            prepared,
        }))
    }

    /// Valida la respuesta, aplica reglas y deja la tarea en su estado siguiente.
    pub fn finish(&mut self, completed: Completed) -> AppResult<Task> {
        let task = completed.task;
        let fallback = completed.fallback;
        if completed.ai_unauthorized {
            // La clave no sirve: la tarea vuelve a la cola sin gastar intento ni fallar y la
            // ventana pide corregir la clave (D101).
            self.ai_wait = Some(AiWait::InvalidKey);
            return Ok(self.store.release(task.id, AiWait::InvalidKey.message())?);
        }
        let generated = match completed.outcome {
            Ok(Outcome::Generated(generated)) => generated,
            Ok(Outcome::Resumed(result)) => return self.resume(task, result),
            Err(error) => return Ok(self.store.fail(task.id, &error)?),
        };
        if let Some(note) = &fallback {
            // Solo el proveedor de origen y el motivo: ni contenido ni secretos (D101).
            self.store
                .record_event(task.id, "ai_fallback", "used", note)?;
        }
        let built = match task.kind.as_str() {
            INVOICE => self.invoice_result(&task, &generated, completed.extraction.as_ref()),
            _ => self.email_result(&task, &generated),
        };
        let mut result = match built {
            Ok(result) => result,
            Err(error) => return Ok(self.store.fail(task.id, &error)?),
        };
        if let Some(note) = fallback {
            result["fallback"] = note;
        }
        if task.kind == INVOICE {
            if let Err(error) = self.enqueue_other_invoices(&task, &generated, &mut result) {
                return Ok(self.store.fail(task.id, &error)?);
            }
        }
        let kind = action_kind(&task);
        // Un importe dudoso o una factura separada de un documento con varias nunca se
        // registran por regla (D120, D68): `evaluate_result` los resuelve antes de las reglas.
        let (rules, any_sender) = self.rules_for_evaluation()?;
        let evaluation = evaluate_result(&rules, &any_sender, kind, &result);
        // Solo ids, códigos e importes o nombres que ya están en el resultado (D125).
        result["evaluation"] = json!(evaluation.explanation);
        // La evidencia (D129) no puede impedir ni duplicar la acción: si no se guarda, la
        // acción sigue su curso y el fallo consta en el resultado de la tarea.
        if let Err(error) =
            self.record_evidence(&task, &result, kind, &rules, &any_sender, &evaluation)
        {
            result["evidence_error"] = json!(format!("no se guardó la evaluación: {error}"));
        }
        let decision = evaluation.decision;
        match decision {
            RuleDecision::Automate {
                rule_id,
                rule_version,
            } => {
                let authorization = Authorization::Rule {
                    rule_id,
                    rule_version,
                };
                self.execute(task, result, authorization)
            }
            RuleDecision::Review(reason) => {
                result["review_reason"] = json!(reason);
                Ok(self.store.await_review(task.id, &result)?)
            }
        }
    }

    /// Si la IA vio más facturas en el documento, encola una tarea por cada una con el mismo
    /// documento y origen, y marca `part` («factura k de n») en el resultado. Al reprocesar
    /// la tarea (recuperación tras un corte) no se encolan otra vez las que ya existen.
    fn enqueue_other_invoices(
        &mut self,
        task: &Task,
        generated: &GenerateResult,
        result: &mut Value,
    ) -> Result<(), TaskError> {
        let input: Value = serde_json::from_str(&task.input_ref).unwrap_or(Value::Null);
        if input.get("part").is_some() {
            result["part"] = input["part"].clone();
            return Ok(());
        }
        let others = parse_other_invoices(&generated.output)
            .map_err(|e| task_error(e.code(), e.to_string(), true))?;
        if others.is_empty() {
            return Ok(());
        }
        let of = others.len() + 1;
        result["part"] = json!({ "index": 1, "of": of });
        let existing: HashSet<u64> = self
            .store
            .all_tasks()
            .map_err(|e| task_error("unavailable", e.to_string(), true))?
            .into_iter()
            .filter_map(|t| {
                let i: Value = serde_json::from_str(&t.input_ref).ok()?;
                (i["part"]["split_from"].as_i64() == Some(task.id))
                    .then(|| i["part"]["index"].as_u64())
                    .flatten()
            })
            .collect();
        for (offset, output) in others.into_iter().enumerate() {
            let index = offset as u64 + 2;
            if existing.contains(&index) {
                continue;
            }
            let mut part_input = input.clone();
            part_input["part"] = json!({ "index": index, "of": of, "split_from": task.id });
            part_input["preset"] = json!({
                "output": output,
                "provider": generated.provider,
                "model": generated.model,
            });
            self.store
                .enqueue(INVOICE, &part_input.to_string())
                .map_err(|e| task_error("unavailable", e.to_string(), true))?;
        }
        Ok(())
    }

    /// Detiene o reanuda el inicio de tareas nuevas. Las que ya se ejecutan terminan; las
    /// que esperan en la cola no se pierden. La pausa no se guarda: tras reiniciar (por
    /// ejemplo, al instalar la versión nueva) la cola sigue sola.
    pub fn set_queue_paused(&mut self, paused: bool) {
        self.paused = paused;
    }

    /// Estado de la cola para decidir si se instala una actualización: solo se instala sin
    /// tareas ejecutándose (actualizaciones.md, paso 5). Quien instala pausa la cola, espera a
    /// que `ready` sea `true` y entonces cierra la aplicación como en una salida completa.
    pub fn update_readiness(&self) -> AppResult<UpdateReadiness> {
        let tasks = self.store.all_tasks()?;
        let count = |state: TaskState| tasks.iter().filter(|t| t.state == state).count();
        let running = count(TaskState::Running);
        Ok(UpdateReadiness {
            ready: running == 0,
            running,
            queued: count(TaskState::Queued),
            awaiting_review: count(TaskState::AwaitingReview),
            paused: self.paused,
        })
    }

    /// Procesa una tarea completa en el hilo actual. `None` si no había trabajo.
    pub fn process_next(&mut self) -> AppResult<Option<Task>> {
        match self.begin_next()? {
            Some(job) => self.finish(job.run()).map(Some),
            None => Ok(None),
        }
    }

    fn invoice_result(
        &mut self,
        task: &Task,
        generated: &GenerateResult,
        extraction: Option<&Extraction>,
    ) -> Result<Value, TaskError> {
        let raw = parse_extraction(&generated.output)
            .map_err(|e| task_error(e.code(), e.to_string(), true))?;
        let invoice = Invoice::from_raw(&raw);
        let report = process_invoice(&mut self.registry, task.id.to_string(), &invoice);
        let mut fields: serde_json::Map<String, Value> = invoice
            .contract_fields()
            .into_iter()
            .map(|(name, value)| (name.to_owned(), value.map_or(Value::Null, Value::String)))
            .collect();
        let currency = invoice.currency.as_ref().map(|c| c.value.clone());
        fields.insert(
            "currency".into(),
            currency.clone().map_or(Value::Null, Value::String),
        );
        let field = |name: &str| fields[name].as_str().map(str::to_owned);
        let payload = json!({
            "supplier": field("supplier"),
            "number": field("invoice_number"),
            "date": field("date"),
            "currency": currency,
            "base": field("subtotal"),
            "taxes": field("tax"),
            "total": field("total"),
        });
        let input: Value = serde_json::from_str(&task.input_ref).unwrap_or(Value::Null);
        // El usuario conoce el nombre; la copia guardada permite abrir el original importado.
        let document = match (input["document"].as_str(), input["path"].as_str()) {
            (Some(name), Some(path)) => format!("{name}», copia en «{path}"),
            (Some(name), None) => name.to_owned(),
            (None, Some(path)) => path.to_owned(),
            (None, None) => "documento sin nombre".to_owned(),
        };
        let amount_review = amount_review(&document, &report, &fields);
        let pages = extraction.and_then(|e| serde_json::to_value(&e.pages).ok());
        // Moneda buscada en el texto local ya extraído (D127). Sin extracción no consta, y
        // las reglas lo tratan como fallo cerrado.
        // `ocr`: alguna página se leyó con OCR; sin `€` legible la moneda no se puede
        // comprobar (D127). Solo un booleano, nunca texto.
        let currency_evidence = extraction.and_then(|e| {
            let mut evidence =
                serde_json::to_value(nestadmin_documents::currency_evidence(&e.text)).ok()?;
            evidence["ocr"] = json!(e
                .pages
                .iter()
                .any(|p| p.method == nestadmin_extract::Method::Ocr));
            Some(evidence)
        });
        let sources: serde_json::Map<String, Value> = extraction
            .map(|extraction| {
                fields
                    .iter()
                    .filter_map(|(name, value)| {
                        let source = field_source(extraction, name, value.as_str()?)?;
                        Some((name.clone(), source))
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(json!({
            "type": INVOICE,
            "version": 1,
            "document": input["document"],
            "document_path": input["path"],
            "origin": input["origin"],
            "pages": pages,
            "fields": fields,
            "sources": sources,
            "issues": report.issues.iter().map(|i| i.as_str()).collect::<Vec<_>>(),
            "duplicates": report.duplicates,
            "unreadable": report.unreadable,
            "recovered": report.recovered,
            "amount_review": amount_review,
            "currency_evidence": currency_evidence,
            "provider": generated.provider,
            "model": generated.model,
            "latency_ms": generated.latency_ms,
            "action": { "kind": REGISTER_INVOICE, "payload": payload },
        }))
    }

    fn email_result(
        &mut self,
        task: &Task,
        generated: &GenerateResult,
    ) -> Result<Value, TaskError> {
        let (category, body) = parse_email(&generated.output)
            .map_err(|e| task_error(e.code(), e.to_string(), true))?;
        let input: Value = serde_json::from_str(&task.input_ref).unwrap_or(Value::Null);
        let message = self
            .task_message(&input)
            .map_err(|e| task_error(&e.code, e.message, true))?
            .ok_or_else(|| task_error("not_found", "el mensaje ya no existe", false))?;
        // Sin remitente no hay a quién responder: el borrador se muestra, pero no se puede enviar.
        let to: Vec<String> = message.from.iter().cloned().collect();
        let subject = message.subject.as_ref().map(|s| format!("Re: {s}"));
        // Tareas anteriores a D48 no guardaron el origen: se toma el de la cuenta actual.
        let origin = match input.get("origin").filter(|o| !o.is_null()) {
            Some(origin) => origin.clone(),
            None => {
                serde_json::to_value(self.mail.origin(split_id(&message.id).0)).unwrap_or_default()
            }
        };
        Ok(json!({
            "type": EMAIL,
            "version": 1,
            "origin": origin,
            "message": message,
            "category": category,
            "draft": { "to": to, "subject": subject, "body": body },
            "provider": generated.provider,
            "model": generated.model,
            "latency_ms": generated.latency_ms,
            "action": { "kind": SEND_DRAFT },
        }))
    }

    /// Ejecuta la acción de la tarea una sola vez y la completa; un fallo del conector deja
    /// la tarea fallida y visible, nunca como éxito.
    fn execute(
        &mut self,
        task: Task,
        mut result: Value,
        authorization: Authorization,
    ) -> AppResult<Task> {
        let version = result["version"].as_i64().unwrap_or(1);
        // Quien autoriza lo que se ejecuta ahora es quien consta en el resultado. Si una
        // regla lo intentó y no llegó a ejecutarse, el intento pasa a `rule_attempt` (sigue
        // contando como decisión de regla para las señales, D129) y `authorization` pasa a
        // ser la aprobación de la persona.
        if matches!(authorization, Authorization::Approval)
            && result["authorization"]["type"] == "rule"
        {
            result["rule_attempt"] = result["authorization"].clone();
        }
        result["authorization"] = serde_json::to_value(&authorization).unwrap_or_default();
        // Antes de ejecutar otra versión, los intentos anteriores deben constar como no
        // realizados: si alguno salió, la tarea se completa con él y no se ejecuta otro.
        match self.check_previous(&task, Some(version), Some(&authorization))? {
            Previous::Clear => {}
            Previous::Done {
                version,
                receipt,
                authorization: done_by,
            } => {
                // Si el envío lo había hecho la regla y aquí solo se reconcilia, la
                // autorización que consta es la de la regla, no la aprobación (D129).
                if done_by.starts_with("regla ") && result["rule_attempt"]["type"] == "rule" {
                    result["authorization"] = result["rule_attempt"].clone();
                }
                return self.complete_with(task.id, result, version, receipt);
            }
            Previous::Unknown { version, error }
                if error.code == nestadmin_simulators::codes::NOT_FOUND =>
            {
                result["review_reason"] = json!(MISSING_REASON);
                result["recovery"] = json!({
                    "version": version,
                    "error": error.code,
                    "needs_confirmation": true,
                });
                return self.back_to_review(&task, &result);
            }
            Previous::Unknown { error, .. } => {
                return Ok(self.store.fail(
                    task.id,
                    &task_error(
                        &error.code,
                        format!("no se pudo comprobar el intento anterior: {error}"),
                        true,
                    ),
                )?)
            }
        }
        // Se guarda lo que se va a ejecutar antes de llamar a ningún conector (D50).
        let draft = match task.kind.as_str() {
            INVOICE => Value::Null,
            _ => result["draft"].clone(),
        };
        result["attempt"] = json!({ "version": version, "draft": draft });
        if let Some(fields) = result.as_object_mut() {
            fields.remove("recovery");
        }
        self.store.record_attempt(task.id, &result)?;
        let outcome = match task.kind.as_str() {
            INVOICE => {
                let request = ActionRequest {
                    task_id: task.id,
                    kind: REGISTER_INVOICE.into(),
                    version,
                    payload: result["action"]["payload"].clone(),
                    authorization,
                };
                self.store.perform_action(&request, &self.admin)
            }
            _ => self.send_reply(&task, &result, version, authorization),
        };
        match outcome {
            Ok(done) => {
                // Si lo ejecutó la regla y aquí solo se reconcilia, la autorización que consta
                // en la acción es la de la regla; el resultado debe decir lo mismo (D129).
                if matches!(done, nestadmin_core::ActionOutcome::Reconciled(_))
                    && result["rule_attempt"]["type"] == "rule"
                {
                    let kind = action_kind(&task);
                    let by_rule = self.store.task_actions(task.id)?.iter().any(|a| {
                        a.kind == kind
                            && a.version == version
                            && a.authorization.starts_with("regla ")
                    });
                    if by_rule {
                        result["authorization"] = result["rule_attempt"].clone();
                    }
                }
                result["receipt"] = done.receipt().clone();
                Ok(self.store.complete(task.id, &result)?)
            }
            Err(error) => {
                let code = match &error {
                    CoreError::Connector(e) => e.code.clone(),
                    _ => "action_failed".to_owned(),
                };
                Ok(self
                    .store
                    .fail(task.id, &task_error(&code, error.to_string(), true))?)
            }
        }
    }

    fn send_reply(
        &mut self,
        task: &Task,
        result: &Value,
        version: i64,
        authorization: Authorization,
    ) -> Result<nestadmin_core::ActionOutcome, CoreError> {
        let draft: NewDraft = serde_json::from_value(result["draft"].clone())?;
        let message_id = result["message"]["id"].as_str().unwrap_or("");
        // El borrador se crea con una clave propia de esta versión, así que repetirlo tras
        // un corte devuelve el mismo borrador y la misma carga del envío.
        let draft_id = self.mail.create_draft(
            message_id,
            &draft,
            &format!("task-{}/create_draft/v{version}", task.id),
        )?;
        let request = ActionRequest {
            task_id: task.id,
            kind: SEND_DRAFT.into(),
            version,
            payload: json!({ "draft_id": draft_id }),
            authorization,
        };
        self.store.perform_action(&request, &self.mail)
    }

    fn reviewable(&mut self, task_id: i64) -> AppResult<(Task, Value)> {
        let task = self.store.task(task_id)?;
        if task.state != TaskState::AwaitingReview {
            return Err(AppError::Invalid(format!(
                "la tarea {task_id} no está pendiente de revisión"
            )));
        }
        let result = task.result.clone().unwrap_or(Value::Null);
        Ok((task, result))
    }

    /// Aprueba la versión que el usuario ha visto. Si el contenido cambió, exige revisarlo.
    pub fn approve(&mut self, task_id: i64, version: i64) -> AppResult<Task> {
        let (task, result) = self.reviewable(task_id)?;
        let current = result["version"].as_i64().unwrap_or(1);
        if current != version {
            return Err(AppError::StaleVersion {
                approved: version,
                current,
            });
        }
        // La aprobación es la decisión de la persona: se anota antes de ejecutar para que un
        // fallo al anotarla conste en el resultado sin impedir ni repetir la acción.
        let mut result = result;
        let signal = panel::signal_for(&task, &result, "approved");
        if let Err(error) = self.record_signal("approved", signal) {
            result["evidence_error"] = json!(format!("no se guardó la señal: {error}"));
        }
        let task_id = task.id;
        let done = self.execute(task, result, Authorization::Approval)?;
        // La aprobación solo vale como señal si la acción de esta versión quedó confirmada o
        // reconciliada; si falló o volvió a revisión, la señal queda anulada (D129).
        let confirmed = done.state == TaskState::Completed
            && done.result.as_ref().is_some_and(|r| {
                r["version"].as_i64() == Some(version) && r.get("sent_version").is_none()
            });
        if let Err(error) = self.store.settle_signal(task_id, version, confirmed) {
            let note = json!({ "kind": "approved", "error": error.to_string() });
            let _ = self
                .store
                .record_event(task_id, "signal_error", "unsettled", &note);
        }
        Ok(done)
    }

    /// Edita el borrador de una respuesta; crea una versión nueva que requiere aprobación.
    pub fn edit_draft(&mut self, task_id: i64, body: &str) -> AppResult<Task> {
        let (task, mut result) = self.reviewable(task_id)?;
        if task.kind != EMAIL {
            return Err(AppError::Invalid(
                "solo se editan borradores de correo".into(),
            ));
        }
        if body.trim().is_empty() {
            return Err(AppError::Invalid("el borrador está vacío".into()));
        }
        result["draft"]["body"] = json!(body);
        // Marca que una persona cambió el texto: la aprobación posterior ya no es «sin
        // cambios» y no sirve de evidencia para sugerir una regla (D129).
        result["edited"] = json!(true);
        result["version"] = json!(result["version"].as_i64().unwrap_or(1) + 1);
        let signal = panel::signal_for(&task, &result, "edited");
        if let Err(error) = self.record_signal("edited", signal) {
            result["evidence_error"] = json!(format!("no se guardó la señal: {error}"));
        }
        Ok(self.store.revise(task_id, &result)?)
    }

    /// El usuario confirma que el intento anterior no se envió (o registró), por ejemplo
    /// porque borró el borrador. Antes se pregunta al conector: si consta realizado, la tarea
    /// se completa con ese recibo. Si no, los intentos se cierran como sustituidos y el mismo
    /// contenido pasa a una versión nueva, con claves nuevas, que requiere aprobación (D50).
    pub fn confirm_not_sent(&mut self, task_id: i64) -> AppResult<Task> {
        let (task, mut result) = self.reviewable(task_id)?;
        let kind = action_kind(&task);
        let actions: Vec<ActionRecord> = self
            .store
            .task_actions(task.id)?
            .into_iter()
            .filter(|a| a.kind == kind)
            .collect();
        if result.get("attempt").is_none() && actions.is_empty() {
            return Err(AppError::Invalid(format!(
                "la tarea {task_id} no tiene ningún intento anterior"
            )));
        }
        if let Some(done) = actions.iter().rev().find(|a| a.status == "confirmed") {
            let receipt = done.receipt.clone().unwrap_or(Value::Null);
            return self.complete_with(task.id, result, done.version, receipt);
        }
        let mut unsent = Vec::new();
        for action in actions.iter().filter(|a| a.is_unsettled()) {
            let found = match kind {
                REGISTER_INVOICE => self.admin.lookup(&action.idempotency_key),
                _ => self.mail.lookup(&action.idempotency_key),
            };
            match found {
                Ok(Some(receipt)) => {
                    self.store.reconcile_action(task.id, action, &receipt)?;
                    return self.complete_with(task.id, result, action.version, receipt);
                }
                Ok(None) => unsent.push(action),
                Err(e) if e.code == nestadmin_simulators::codes::NOT_FOUND => unsent.push(action),
                Err(e) => return Err(AppError::Connector(e)),
            }
        }
        for action in unsent {
            self.store.supersede_action(
                task.id,
                action,
                &Authorization::Approval,
                "el usuario confirmó que no se realizó",
            )?;
        }
        let version = result["version"].as_i64().unwrap_or(1);
        result["version"] = json!(version + 1);
        result["review_reason"] =
            json!("versión nueva tras confirmar que el intento anterior no se realizó");
        if let Some(fields) = result.as_object_mut() {
            fields.remove("recovery");
        }
        Ok(self.store.revise(task_id, &result)?)
    }

    /// Descarta la tarea sin ejecutar ninguna acción.
    pub fn reject(&mut self, task_id: i64) -> AppResult<Task> {
        let before = self.store.task(task_id)?;
        // Solo cuenta como rechazo lo que estaba pendiente de revisión. Si la señal no se
        // guarda, el fallo queda en el resultado antes de cancelar.
        if before.state == TaskState::AwaitingReview {
            let mut result = before.result.clone().unwrap_or(Value::Null);
            let signal = panel::signal_for(&before, &result, "rejected");
            if let Err(error) = self.record_signal("rejected", signal) {
                result["evidence_error"] = json!(format!("no se guardó la señal: {error}"));
                self.store.revise(task_id, &result)?;
            }
        }
        Ok(self.store.cancel(task_id)?)
    }

    pub fn retry(&mut self, task_id: i64) -> AppResult<Task> {
        Ok(self.store.retry(task_id)?)
    }

    pub fn tasks(&self) -> AppResult<Vec<Task>> {
        Ok(self.store.all_tasks()?)
    }

    pub fn audit_log(&self) -> AppResult<Vec<AuditEntry>> {
        Ok(self.store.audit_log()?)
    }

    pub fn rules(&self) -> AppResult<Vec<Rule>> {
        Ok(self.store.rules()?)
    }

    /// Crea una regla de tipo `action_kind` con identificador propio. Una regla de envío exige
    /// remitente (D126): para una sin remitente, `create_rule_full` con la validación avanzada.
    pub fn create_rule(
        &mut self,
        id: &str,
        action_kind: &str,
        conditions: &Value,
    ) -> AppResult<Rule> {
        let view = self.create_rule_full(panel::NewRule {
            id: Some(id),
            name: None,
            action_kind,
            conditions,
            allow_any_sender: false,
        })?;
        Ok(view.rule)
    }

    /// Cambia las condiciones de una regla. Una regla archivada no se edita (D128). Cambiar
    /// las condiciones borra la validación avanzada sin remitente (D126): si la regla no
    /// indicaba remitente, deja de automatizar hasta validarla otra vez.
    pub fn update_rule(&mut self, id: &str, conditions: &Value) -> AppResult<Rule> {
        let current = self
            .store
            .rule(id)?
            .ok_or_else(|| AppError::Invalid(format!("regla {id} inexistente")))?;
        let meta = self.store.rule_meta(id)?;
        if meta.as_ref().is_some_and(|m| m.archived_at.is_some()) {
            return Err(CoreError::RuleArchived(id.to_owned()).into());
        }
        let validated = meta.is_some_and(|m| m.allow_any_sender_ack_at.is_some());
        check_rule(&current.action_kind, conditions, validated)?;
        Ok(self.store.update_rule(id, conditions)?)
    }

    /// Activar revalida las condiciones guardadas y no admite una regla archivada;
    /// desactivar es siempre posible.
    pub fn set_rule_active(&mut self, id: &str, active: bool) -> AppResult<Rule> {
        if active {
            if let Some(current) = self.store.rule(id)? {
                let meta = self.store.rule_meta(id)?;
                if meta.as_ref().is_some_and(|m| m.archived_at.is_some()) {
                    return Err(CoreError::RuleArchived(id.to_owned()).into());
                }
                let validated = meta.is_some_and(|m| m.allow_any_sender_ack_at.is_some());
                check_rule(&current.action_kind, &current.conditions, validated).map_err(|e| {
                    match e {
                        AppError::Invalid(why) => {
                            AppError::Invalid(format!("la regla {id} no se puede activar: {why}"))
                        }
                        other => other,
                    }
                })?;
            }
        }
        Ok(self.store.set_rule_active(id, active)?)
    }

    /// Para pruebas y demostraciones: efectos registrados en los simuladores.
    pub fn simulators(&self) -> (&MailSimulator, &AdminSimulator) {
        (self.mail.simulator(), &self.admin)
    }

    /// Cuentas de correo, su estado y el límite de mensajes por consulta.
    pub fn mail_accounts(&self) -> MailAccountsView {
        self.mail.view()
    }

    /// Mensajes leídos como mucho por cuenta en cada consulta (1 a 500).
    pub fn set_mail_limit(&mut self, limit: usize) -> AppResult<()> {
        self.mail.set_limit(limit).map_err(AppError::Invalid)
    }

    /// Cliente OAuth para iniciar sesión; error si esta instalación no lo tiene configurado.
    pub fn mail_oauth(&self, provider: MailProviderKind) -> AppResult<OAuthConfig> {
        Ok(self.mail.oauth(provider)?)
    }

    /// Añade una cuenta con los tokens obtenidos al iniciar sesión.
    pub fn add_mail_account(
        &mut self,
        provider: MailProviderKind,
        label: &str,
        tokens: &Tokens,
    ) -> AppResult<AccountView> {
        Ok(self.mail.add(provider, label, tokens)?)
    }

    /// Guarda tokens nuevos para una cuenta existente.
    pub fn reconnect_mail_account(&mut self, id: &str, tokens: &Tokens) -> AppResult<AccountView> {
        Ok(self.mail.reconnect(id, tokens)?)
    }

    /// Cambia el nombre visible de una cuenta; vacío vuelve al predeterminado. Las tareas ya
    /// encoladas conservan el nombre con que llegaron; la ventana muestra el actual.
    pub fn rename_mail_account(&mut self, id: &str, label: &str) -> AppResult<AccountView> {
        Ok(self.mail.rename(id, label)?)
    }

    /// Vuelve a añadir la cuenta simulada si se había quitado.
    pub fn add_simulated_mail_account(&mut self) -> AppResult<AccountView> {
        Ok(self.mail.add_simulator()?)
    }

    /// Desconecta una cuenta: borra sus tokens y la quita de la lista. Las tareas ya
    /// encoladas de esa cuenta siguen visibles; enviar desde ellas falla con el motivo.
    pub fn remove_mail_account(&mut self, id: &str) -> AppResult<()> {
        Ok(self.mail.remove(id)?)
    }
}

fn action_kind(task: &Task) -> &'static str {
    if task.kind == INVOICE {
        REGISTER_INVOICE
    } else {
        SEND_DRAFT
    }
}

/// Valida las condiciones de una regla (D123, D126, D127). `allow_no_sender`: la validación
/// avanzada permite una regla de envío sin remitente.
fn check_rule(action_kind: &str, conditions: &Value, allow_no_sender: bool) -> AppResult<()> {
    let Some(map) = conditions.as_object() else {
        return Err(AppError::Invalid(
            "las condiciones de la regla deben ser un objeto".into(),
        ));
    };
    let allowed: &[&str] = match action_kind {
        REGISTER_INVOICE => &["supplier", "max_total", "currency"],
        SEND_DRAFT => &["category", "sender", "sender_domain"],
        other => {
            return Err(AppError::Invalid(format!(
                "tipo de acción desconocido: {other}"
            )))
        }
    };
    if let Some(unknown) = map.keys().find(|k| !allowed.contains(&k.as_str())) {
        return Err(AppError::Invalid(format!(
            "condición desconocida en la regla: {unknown}"
        )));
    }
    let text = |key: &str| {
        conditions
            .get(key)
            .and_then(Value::as_str)
            .filter(|v| !v.trim().is_empty())
    };
    if action_kind == SEND_DRAFT {
        if !nestadmin_ai::tasks::CATEGORIES.contains(&text("category").unwrap_or("")) {
            return Err(AppError::Invalid(
                "la regla de envío necesita una categoría válida".into(),
            ));
        }
        return check_sender(map, allow_no_sender);
    }
    if text("supplier").is_none() {
        return Err(AppError::Invalid(
            "la regla de registro necesita un proveedor".into(),
        ));
    }
    if let Some(currency) = conditions.get("currency") {
        if currency.as_str() != Some("EUR") {
            return Err(AppError::Invalid(
                "Hoy la aplicación solo registra facturas en euros: la moneda de la regla solo puede ser \"EUR\"".into(),
            ));
        }
    }
    match conditions.get("max_total") {
        None => Ok(()),
        Some(Value::String(limit)) => nestadmin_documents::parse_amount("max_total", limit)
            .map(|_| ())
            .map_err(|_| max_total_error()),
        Some(_) => Err(max_total_error()),
    }
}

/// Remitente de una regla de envío: dirección exacta o dominio, nunca los dos (D126).
fn check_sender(map: &serde_json::Map<String, Value>, allow_no_sender: bool) -> AppResult<()> {
    let invalid = |what: &str| AppError::Invalid(what.to_owned());
    match (map.get("sender"), map.get("sender_domain")) {
        (Some(_), Some(_)) => Err(invalid(
            "indique la dirección del remitente o su dominio, no las dos cosas",
        )),
        (Some(value), None) => match value.as_str() {
            Some(address) if rules::is_email_address(address.trim()) => Ok(()),
            _ => Err(invalid(
                "el remitente debe ser una dirección de correo, p. ej. \"pedidos@proveedor.example\"",
            )),
        },
        (None, Some(value)) => match value.as_str() {
            Some(domain) if rules::is_domain(domain.trim()) => Ok(()),
            _ => Err(invalid(
                "el dominio del remitente debe ser como \"proveedor.example\", sin @ ni subdominios implícitos",
            )),
        },
        (None, None) if allow_no_sender => Ok(()),
        (None, None) => Err(invalid(
            "la regla de envío necesita un remitente (dirección o dominio); sin él solo puede \
             guardarse con la validación avanzada de «cualquier remitente»",
        )),
    }
}

fn max_total_error() -> AppError {
    AppError::Invalid(
        "el importe máximo debe ser un texto con dos decimales, p. ej. \"500.00\"".into(),
    )
}

/// Identificador de regla: minúsculas, cifras y guiones, no vacío tras `trim`.
fn check_rule_id(id: &str) -> AppResult<()> {
    let id = id.trim();
    if id.is_empty()
        || !id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return Err(AppError::Invalid(
            "el identificador de la regla solo admite minúsculas, cifras y guiones".into(),
        ));
    }
    Ok(())
}

/// Comprueba nombre, tamaño y contenido de un documento; devuelve su extensión.
/// Nombre con el que se guarda un adjunto: el suyo o, si no lo trae, uno deducido del tipo.
fn attachment_name(attachment: &nestadmin_simulators::Attachment) -> String {
    if let Some(name) = attachment
        .name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
    {
        // Sin rutas: solo el último componente, por si el remitente puso barras.
        return name.rsplit(['/', '\\']).next().unwrap_or(name).to_owned();
    }
    let extension = match attachment
        .mime
        .as_deref()
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("application/pdf") => "pdf",
        Some("text/plain") => "txt",
        Some("image/jpeg") => "jpg",
        Some("image/png") => "png",
        Some("image/tiff") => "tif",
        _ => "bin",
    };
    format!(
        "adjunto-{}.{extension}",
        attachment.id.chars().take(12).collect::<String>()
    )
}

/// Texto y PDF; además, las imágenes que el extractor pasa por OCR.
fn is_document_extension(extension: &str) -> bool {
    matches!(extension, "txt" | "pdf") || nestadmin_extract::IMAGE_EXTENSIONS.contains(&extension)
}

fn document_extension(name: &str) -> String {
    Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default()
}

fn supported_document(name: &str) -> bool {
    is_document_extension(&document_extension(name))
}

fn check_document(name: &str, bytes: &[u8]) -> AppResult<String> {
    let extension = document_extension(name);
    if !is_document_extension(&extension) {
        return Err(AppError::Invalid(format!(
            "«{name}»: solo se admiten texto (.txt), PDF e imágenes (.jpg, .png, .tif, .bmp, .webp)"
        )));
    }
    if bytes.is_empty() {
        return Err(AppError::Invalid(format!("«{name}» está vacío")));
    }
    if bytes.len() > MAX_DOCUMENT_BYTES {
        return Err(AppError::Invalid(format!(
            "«{name}» supera el tamaño máximo de {} MB",
            MAX_DOCUMENT_BYTES / 1_000_000
        )));
    }
    Ok(extension)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Formas en que un valor normalizado puede aparecer escrito en el documento: el importe
/// `1234.50` como `1234,50`, `1.234,50` o `1,234.50`; la fecha `2026-10-05` como `05/10/2026`.
fn written_forms(field: &str, value: &str) -> Vec<String> {
    let mut forms = vec![value.to_owned()];
    match field {
        "subtotal" | "tax" | "total" => {
            if let Some((units, cents)) = value.split_once('.') {
                let (sign, digits) = match units.strip_prefix('-') {
                    Some(rest) => ("-", rest),
                    None => ("", units),
                };
                let grouped = |separator: char| {
                    let mut out = String::new();
                    for (i, c) in digits.chars().enumerate() {
                        if i > 0 && (digits.len() - i) % 3 == 0 {
                            out.push(separator);
                        }
                        out.push(c);
                    }
                    out
                };
                forms.push(format!("{units},{cents}"));
                forms.push(format!("{sign}{},{cents}", grouped('.')));
                forms.push(format!("{sign}{}.{cents}", grouped(',')));
            }
        }
        "date" => {
            let parts: Vec<&str> = value.split('-').collect();
            if let [year, month, day] = parts[..] {
                for separator in ['/', '-', '.'] {
                    forms.push(format!("{day}{separator}{month}{separator}{year}"));
                }
                let (d, m) = (day.trim_start_matches('0'), month.trim_start_matches('0'));
                forms.push(format!("{d}/{m}/{year}"));
            }
        }
        _ => {}
    }
    let mut unique = Vec::new();
    for form in forms {
        if !unique.contains(&form) {
            unique.push(form);
        }
    }
    unique
}

/// Dónde aparece un campo en el texto extraído: páginas y la línea de la primera aparición.
/// `None` si no se encuentra literalmente (el modelo pudo reescribirlo): nunca se adivina.
fn field_source(extraction: &Extraction, field: &str, value: &str) -> Option<Value> {
    for form in written_forms(field, value) {
        let pages = extraction.pages_containing(&form);
        if pages.is_empty() {
            continue;
        }
        let text = &extraction.text;
        let at = text.find(&form)?;
        let start = text[..at].rfind('\n').map_or(0, |i| i + 1);
        let end = text[at..].find('\n').map_or(text.len(), |i| at + i);
        let line: String = text[start..end].trim().chars().take(160).collect();
        return Some(json!({ "pages": pages, "text": form, "line": line }));
    }
    None
}

fn raw_from_fields(fields: &Value) -> RawInvoice {
    let field = |name: &str| fields[name].as_str().map(|v| Evidenced::new(v.to_owned()));
    RawInvoice {
        supplier: field("supplier"),
        invoice_number: field("invoice_number"),
        date: field("date"),
        currency: field("currency"),
        subtotal: field("subtotal"),
        tax: field("tax"),
        total: field("total"),
    }
}

/// Nombre del importe tal como lo ve el usuario.
fn amount_label(field: &str) -> &str {
    match field {
        "subtotal" => "Base imponible",
        "tax" => "IVA",
        "total" => "Total",
        other => other,
    }
}

/// Mensaje que pide revisar los importes cuando es probable que estén mal (D120): importes
/// recuperados como propuesta o una suma que no cuadra. Lleva la ubicación del archivo y
/// los importes. `None` si los importes se leyeron sin duda y cuadran.
fn amount_review(
    document: &str,
    report: &nestadmin_documents::ValidationReport,
    fields: &serde_json::Map<String, Value>,
) -> Option<String> {
    let mut parts: Vec<String> = report
        .recovered
        .iter()
        .map(|r| {
            let why = match r.reason {
                RecoveryReason::NoDecimals => "sin decimales",
                RecoveryReason::OneDecimal => "con un solo decimal",
                RecoveryReason::AmbiguousSeparator => "el punto o la coma podría ser de miles",
            };
            format!(
                "{} leído como «{}», se propone {} ({why})",
                amount_label(&r.field),
                r.literal,
                r.value
            )
        })
        .collect();
    if report.issues.contains(&Issue::TotalMismatch) {
        let amount = |key: &str| fields[key].as_str().unwrap_or("?").to_owned();
        parts.push(format!(
            "base {} + IVA {} no coincide con el total {}",
            amount("subtotal"),
            amount("tax"),
            amount("total")
        ));
    }
    (!parts.is_empty()).then(|| {
        format!(
            "Revise los importes en «{document}»: {}. Compruébelos con el documento; solo se registra con su aprobación.",
            parts.join("; ")
        )
    })
}

/// Directorio de datos por defecto en Windows (D03).
pub fn default_data_dir() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|base| Path::new(&base).join("NestAdmin"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use nestadmin_extract::{Method, Page};

    struct NoOcr;
    impl nestadmin_extract::Ocr for NoOcr {
        fn recognize(
            &self,
            _: &Path,
            page: usize,
        ) -> Result<String, nestadmin_extract::ExtractError> {
            Err(nestadmin_extract::ExtractError::OcrUnavailable(format!(
                "{page}"
            )))
        }
    }

    struct Silent;

    impl Provider for Silent {
        fn kind(&self) -> ProviderKind {
            ProviderKind::Ollama
        }
        fn model(&self) -> &str {
            "simulado"
        }
        fn generate(&self, _: &GenerateRequest) -> Result<GenerateResult, nestadmin_ai::AiError> {
            unreachable!("la prueba no llama al proveedor")
        }
    }

    /// El código `unauthorized` también lo usan los conectores de correo: un fallo así sigue
    /// siendo un fallo de la tarea y no activa la espera de la IA (solo lo hace el
    /// `unauthorized` que devuelve el proveedor, marcado en `Completed`).
    #[test]
    fn a_connector_unauthorized_still_fails_the_task_and_does_not_make_the_ai_wait() {
        let dir = tempfile::tempdir().unwrap();
        let extractor = Extractor::new(dir.path().join("sin-pdfium"), Box::new(NoOcr));
        let mut app = App::open(
            &AppPaths::new(dir.path()),
            Arc::new(Silent),
            Arc::new(extractor),
        )
        .unwrap();
        app.import_document("a.txt", "texto sintético").unwrap();
        let job = app.begin_next().unwrap().unwrap();
        let error = task_error("unauthorized", "sesión de correo caducada", true);
        let failed = app
            .finish(Completed {
                task: job.task,
                outcome: Err(error),
                extraction: None,
                ai_unauthorized: false,
                fallback: None,
            })
            .unwrap();
        assert_eq!(failed.state, TaskState::Failed);
        assert_eq!(failed.error.unwrap().code, "unauthorized");
        assert_eq!(failed.attempts, 1);
        assert_eq!(app.ai_wait(), None);
    }

    /// Una factura separada, aún en cola, cuyo `preset` se extrajo con Groq (retirado en D100)
    /// no se procesa con el proveedor actual como si nada: falla de forma explícita, sin gastar
    /// una llamada a la IA. Un `preset` de un proveedor vigente sigue reutilizándose (D68).
    #[test]
    fn a_split_preset_from_the_retired_groq_fails_explicitly_and_a_current_one_is_reused() {
        fn input(provider: &str) -> String {
            json!({
                "path": "documento.txt",
                "preset": { "provider": provider, "model": "m", "output": { "invoice_number": "F-9" } },
            })
            .to_string()
        }
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::open(
            &AppPaths::new(dir.path()),
            Arc::new(Silent),
            Arc::new(Extractor::new(
                dir.path().join("sin-pdfium"),
                Box::new(NoOcr),
            )),
        )
        .unwrap();
        let current: Value = serde_json::from_str(&input("ollama_cloud")).unwrap();
        let retired: Value = serde_json::from_str(&input("groq")).unwrap();
        assert!(split_preset(&current).is_some());
        assert!(!split_preset_unreadable(&current));
        assert!(split_preset(&retired).is_none());
        assert!(split_preset_unreadable(&retired));
        assert!(!split_preset_unreadable(&json!({ "path": "x.txt" })));

        app.store.enqueue(INVOICE, &input("groq")).unwrap();
        let job = app.begin_next().unwrap().unwrap();
        let error = job.prepared.err().expect("falla de forma explícita");
        assert_eq!(error.code, "preset_unreadable");
        assert!(!error.recoverable);

        app.store.enqueue(INVOICE, &input("ollama_cloud")).unwrap();
        let job = app.begin_next().unwrap().unwrap();
        assert!(matches!(job.prepared, Ok(Prepared::Split(..))));
    }

    fn page(number: usize, offset: usize) -> Page {
        Page {
            number,
            method: Method::PdfText,
            chars: 1,
            offset,
            confidence: None,
            low_confidence: false,
        }
    }

    #[test]
    fn amounts_and_dates_are_found_as_written_with_their_page() {
        let first = "Proveedor Alfa SL\nFecha: 05/10/2026";
        let text = format!("{first}\n\nBase 1.234,50\nTotal: 1.493,75 EUR");
        let extraction = Extraction {
            pages: vec![page(1, 0), page(2, first.len() + 2)],
            text,
        };
        let total = field_source(&extraction, "total", "1493.75").unwrap();
        assert_eq!(total["pages"], json!([2]));
        assert_eq!(total["line"], "Total: 1.493,75 EUR");
        let date = field_source(&extraction, "date", "2026-10-05").unwrap();
        assert_eq!(date["pages"], json!([1]));
        assert_eq!(date["text"], "05/10/2026");
        assert!(
            field_source(&extraction, "tax", "259.25").is_none(),
            "lo que no aparece en el texto no tiene página"
        );
    }
}
