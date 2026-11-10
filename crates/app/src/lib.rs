//! Capa de aplicación de NestAdmin (flujos 1–5 de requisitos.md). Une la cola durable, la
//! IA, la validación de facturas, los simuladores y las reglas, sin depender de la ventana:
//! la interfaz Tauri solo llama a esta capa.
//!
//! El trabajo con IA se divide en tres pasos para que la llamada al modelo, que puede tardar
//! decenas de segundos, no bloquee el resto de la aplicación: `begin_next` toma la tarea y
//! prepara la solicitud, `Job::run` llama al proveedor sin acceso a `App`, y `finish` valida,
//! aplica reglas y deja la tarea completada, en revisión o fallida.

pub mod accounts;
pub mod assistant;
pub mod initial_read;
pub mod learn;
pub mod learn_consent;
pub mod learn_continuous;
pub mod learn_evidence;
pub mod learn_rules;
pub mod learn_state;
pub mod panel;
pub mod pipelines;
pub mod preferences;
pub mod profile;
pub mod refine;
pub mod rules;
pub mod search;
pub mod suggest;

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use nestadmin_ai::tasks::{
    draft_flags_allowing, draft_leaks_internals, draft_request_with_habit, extraction_request,
    parse_draft, parse_extraction, parse_other_invoices, parse_triage, triage_request,
};
use nestadmin_ai::{
    AiError, GenerateRequest, GenerateResult, ModelUnavailableCause, Provider, ProviderKind,
};
use nestadmin_core::mail_signals::{route_of_message, Route, Signal, TriageDecision};
use nestadmin_core::{
    ActionRecord, ActionRequest, AuditEntry, Authorization, Connector, ConnectorError, CoreError,
    Rule, SearchDoc, Store, Task, TaskError, TaskState,
};
use nestadmin_documents::{
    looks_like_invoice, process_invoice, DuplicateRegistry, Evidenced, Invoice, Issue, RawInvoice,
    RecoveryReason,
};
use nestadmin_extract::{Extraction, Extractor};
use nestadmin_mail::{OAuthConfig, Tokens};
use nestadmin_simulators::{AdminSimulator, MailSimulator, Message, NewDraft};
use serde::{Serialize, Serializer};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::accounts::{
    split_id, AccountFetch, AccountView, Accounts, AttachmentFetch, AttachmentIssue, FetchReport,
    MailAccountsView, MailEnv, MailIssue, MailProviderKind,
};
use crate::rules::{
    evaluate_result_in_run, Evaluation, RuleDecision, REGISTER_INVOICE, SEND_DRAFT,
};

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
    /// Una factura con importe dudoso (D120) no se aprueba sin que la persona confirme antes los
    /// importes con el documento (D186).
    #[error("hay importes que no se leen sin duda: confirme antes los importes con el documento")]
    AmountAckRequired,
    /// Una propuesta del asistente que no se puede confirmar (D185): `code` es `not_found`,
    /// `expired`, `rejected` o `failed`. La versión cambiada usa `StaleVersion`.
    #[error("{message}")]
    Proposal { code: &'static str, message: String },
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

/// Por qué las tareas con IA esperan en la cola sin gastar intentos (D100, D101, D146). Mientras
/// haya una espera, nada se envía a ningún proveedor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiWait {
    /// No hay proveedor elegido (por ejemplo, una configuración antigua con Groq).
    NeedsChoice,
    /// El proveedor externo elegido no tiene clave guardada.
    NeedsKey,
    /// El destino de los datos no está confirmado.
    NeedsConsent,
    /// El proveedor rechazó la clave (`unauthorized`): revocada o incorrecta.
    InvalidKey,
    /// Al generar, el proveedor respondió 404 o 410 (o 400 con `model_not_found`): el modelo o la
    /// dirección no sirven (D146). Solo en memoria, como `InvalidKey`; no activa el respaldo.
    ModelUnavailable {
        cause: ModelUnavailableCause,
        status: u16,
    },
    /// El proveedor respondió 402: la cuenta no tiene crédito (D146). Solo en memoria.
    NoCredit,
}

impl Serialize for AiWait {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.code())
    }
}

impl AiWait {
    /// Código estable para la interfaz y los registros.
    pub fn code(self) -> &'static str {
        match self {
            Self::NeedsChoice => "needs_choice",
            Self::NeedsKey => "needs_key",
            Self::NeedsConsent => "needs_consent",
            Self::InvalidKey => "invalid_key",
            Self::ModelUnavailable { .. } => "model_unavailable",
            Self::NoCredit => "no_credit",
        }
    }

    /// Esperas que nacen de la respuesta del proveedor (D146) y no de la configuración: no las
    /// levanta una nueva resolución del proveedor si este no ha cambiado.
    pub fn is_provider_response(self) -> bool {
        matches!(self, Self::ModelUnavailable { .. } | Self::NoCredit)
    }

    /// Causa enumerada de una espera de D146, para la interfaz.
    pub fn cause(self) -> Option<&'static str> {
        match self {
            Self::ModelUnavailable { cause, .. } => Some(match cause {
                ModelUnavailableCause::ModelNotFound => "model_not_found",
                ModelUnavailableCause::RouteNotFound => "route_not_found",
                ModelUnavailableCause::Other => "other",
            }),
            _ => None,
        }
    }

    /// Estado HTTP que originó una espera de D146.
    pub fn status(self) -> Option<u16> {
        match self {
            Self::ModelUnavailable { status, .. } => Some(status),
            Self::NoCredit => Some(402),
            _ => None,
        }
    }

    /// Texto sin datos del proveedor; para las esperas de D146, `describe` da el completo.
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
            Self::ModelUnavailable { .. } => {
                "El proveedor de IA no responde con la configuración elegida. Las tareas esperan sin gastar intentos."
            }
            Self::NoCredit => {
                "El proveedor de IA indica que la cuenta no tiene crédito. Las tareas esperan sin gastar intentos."
            }
        }
    }

    /// Texto completo de la espera (D146). `address` es la dirección configurada y solo cuenta con
    /// el compatible con OpenAI: con una dirección fija (Ollama cloud, Claude) un 404 de ruta no
    /// permite hablar de dirección. Las esperas anteriores a D146 usan `message`.
    pub fn describe(self, kind: ProviderKind, model: &str, address: Option<&str>) -> String {
        const WAITING: &str = "Las tareas esperan sin gastar intentos.";
        let provider = provider_label(kind);
        match self {
            Self::ModelUnavailable { cause, status } => match cause {
                ModelUnavailableCause::ModelNotFound => format!(
                    "{provider} no encuentra el modelo «{model}» (error {status}). {WAITING}"
                ),
                ModelUnavailableCause::RouteNotFound if kind != ProviderKind::OpenAiCompatible => {
                    format!(
                        "{provider} respondió con un error {status} sin indicar la causa. {WAITING} Pruebe de nuevo o elija otro modelo."
                    )
                }
                ModelUnavailableCause::RouteNotFound => {
                    let mut text = format!(
                        "{provider} no reconoce la dirección del servicio (error {status}). {WAITING} Compruebe la dirección en Configuración."
                    );
                    if let Some(address) = address {
                        text.push_str(&format!(" Dirección configurada: {address}."));
                    }
                    text
                }
                ModelUnavailableCause::Other => format!(
                    "{provider} respondió con un error {status} usando el modelo «{model}». No consta la causa exacta; puede que el modelo no esté disponible para su cuenta. {WAITING}"
                ),
            },
            Self::NoCredit => {
                format!("{provider} indica que la cuenta no tiene crédito (error 402). {WAITING}")
            }
            other => other.message().to_owned(),
        }
    }
}

/// Nombre del proveedor en los textos que ve la persona (D146).
pub fn provider_label(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::Ollama => "Ollama (IA local)",
        ProviderKind::OllamaCloud => "Ollama cloud",
        ProviderKind::OpenAiCompatible => "El servicio compatible con OpenAI",
        ProviderKind::Anthropic => "Claude · Anthropic",
    }
}

/// Motivo de la tarea cuando el proveedor rechaza esa petición (D146).
fn request_rejected_text(provider: &str, status: u16) -> String {
    format!(
        "{provider} rechazó esta tarea (error {status}). Las demás siguen en marcha. Puede reintentarla o elegir otro modelo."
    )
}

/// La espera global que implica un error del proveedor elegido, si implica alguna (D101, D146).
fn wait_for(error: &AiError) -> Option<AiWait> {
    match error {
        AiError::Unauthorized => Some(AiWait::InvalidKey),
        AiError::ModelUnavailable { cause, status } => Some(AiWait::ModelUnavailable {
            cause: *cause,
            status: *status,
        }),
        AiError::NoCredit => Some(AiWait::NoCredit),
        _ => None,
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
    /// La IA local que puede usar el asistente del chat sin consentimiento o sin IA externa
    /// utilizable (D183): solo si ya está en marcha, sin arrancar nada, y solo si el respaldo local
    /// está activado. Independiente de `fallback` y de `learning.local_ai`, que pueden arrancarla.
    assistant_local: Option<LocalFallback>,
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
    /// Primeras lecturas que están avanzando en este proceso (D133): una marca en fase de encolar
    /// que no esté aquí se cortó (se cerró la aplicación) y se muestra como interrumpida.
    reads_active: HashSet<String>,
    /// Fuentes de la primera lectura cuyas tareas no deben salir ahora (detenidas, canceladas o con
    /// el destino cambiado). Se comparte con las tareas en curso, que lo comprueban justo antes de
    /// llamar al proveedor (D133).
    read_gate: Arc<Mutex<HashSet<String>>>,
    /// Estado en memoria de la fase «Aprender a responder» (E4, D175): reanudaciones hechas,
    /// trabajos en curso y lo recogido. No se guarda nada de esto.
    learning: learn_state::LearningRuntime,
    /// Estado en memoria del aprendizaje continuo (D178): pasadas en curso, puerta de tiempo,
    /// orden de comprobación y recuentos. No se guarda nada de esto.
    continuous: learn_continuous::ContinuousRuntime,
    /// Estado en memoria del asistente del chat (D182): consentimiento vigente, cancelación y
    /// turno exclusivo. No se guarda nada de esto.
    assistant: assistant::AssistantRuntime,
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
    Document(PathBuf),
    /// Una factura más de un documento con varias (D68): sus campos ya los dio la IA al
    /// procesar la primera; solo se vuelve a leer el documento para localizar cada campo.
    Split(PathBuf, GenerateResult),
    Resume(Value),
    /// Un correo nuevo (D158): primero se clasifica y, solo si hace falta respuesta, se redacta.
    /// `text` es el asunto y el cuerpo; nada más sale del equipo.
    Email {
        id: String,
        text: String,
    },
    /// «Redactar respuesta» (D159): solo la redacción, sin clasificar de nuevo.
    Redraft {
        id: String,
        text: String,
        /// Categoría del correo de origen, para la línea de respuesta habitual (D171).
        category: Option<String>,
    },
}

/// Lo que dan las dos llamadas de un correo (D158).
struct EmailGenerated {
    /// Respuesta de la última llamada hecha (la redacción si la hubo): proveedor, modelo y latencia.
    meta: GenerateResult,
    /// Categoría de la clasificación; `None` en «Redactar», que no clasifica.
    category: Option<String>,
    needs_reply: bool,
    /// Texto del borrador, ya leído; `None` si no hacía falta respuesta.
    draft: Option<String>,
    /// Huella del perfil del negocio que llevó la redacción (D163); `None` si no llevó ninguno.
    profile_fp: Option<String>,
    /// Texto de ese perfil, solo en memoria: sirve para no marcar sus contactos en el borrador y
    /// nunca se guarda en el resultado.
    profile_text: Option<String>,
}

/// Lo que devuelve el trabajo de una tarea.
enum Outcome {
    Generated(GenerateResult),
    Email(EmailGenerated),
    Resumed(Value),
    /// Primera lectura: el documento no parece una factura según la comprobación local y no se
    /// envía a ningún proveedor (E5, D133). Lleva el código del motivo.
    Skipped(&'static str),
    /// La lectura se detuvo, se desconectó o cambió de destino mientras la tarea estaba en curso:
    /// no se llamó al proveedor y la tarea vuelve a la cola retenida (D133).
    Held,
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

/// Una tarea de la primera lectura (E5, D133): su entrada lleva `initial_read: true`. Solo
/// clasifica y alimenta las sugerencias; nunca crea acciones ni pendientes.
pub(crate) fn is_initial_read(input: &Value) -> bool {
    input["initial_read"].as_bool() == Some(true)
}

/// Lo mismo, a partir del texto de la entrada de una tarea.
pub(crate) fn task_is_initial_read(task: &Task) -> bool {
    serde_json::from_str::<Value>(&task.input_ref)
        .map(|input| is_initial_read(&input))
        .unwrap_or(false)
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

/// Motivo de revisión de un correo que la IA no cree que requiera respuesta (D158).
const NO_REPLY_REASON: &str =
    "probablemente no requiere respuesta; puede archivarlo o pedir una respuesta";
/// Motivo de revisión de un rebote (D157).
const BOUNCE_REASON: &str =
    "posible fallo de entrega: avisa de que un correo no llegó; archívelo o pida una respuesta";

/// Motivo de revisión de una tarea cuyo intento anterior no se confirmó.
const RESUMED_REASON: &str =
    "el intento anterior no se confirmó; se conserva el contenido aprobado, sin pedir otro a la IA";
/// Motivo de revisión cuando el borrador anterior ya no existe y no consta enviado.
const MISSING_REASON: &str = "el borrador del intento anterior ya no existe y no consta enviado; confirme que no se envió antes de preparar otro";

/// Tarea tomada de la cola con su trabajo preparado.
pub struct Job {
    pub task: Task,
    provider: Arc<dyn Provider>,
    /// Servidor al que saldrían los datos de esta tarea (`None` con la IA local).
    destination: Option<String>,
    /// Compuerta de la primera lectura (D133): se comprueba justo antes de llamar al proveedor.
    gate: Arc<Mutex<HashSet<String>>>,
    fallback: Option<LocalFallback>,
    fallback_until: Arc<Mutex<Option<Instant>>>,
    extractor: Arc<Extractor>,
    /// Perfil del negocio para la redacción de esta tarea, ya comprobado el consentimiento (D163).
    profile: Option<profile::ProfileUse>,
    /// Recuentos de `reply_style` (categoría, tipo, n) para la línea de respuesta habitual de la
    /// redacción (D171). Solo enums y cifras; vacío fuera de la redacción.
    reply_style: Vec<(String, String, i64)>,
    prepared: Result<Prepared, TaskError>,
}

/// Respuesta del proveedor (o el motivo del fallo) lista para `finish`.
pub struct Completed {
    task: Task,
    outcome: Result<Outcome, TaskError>,
    /// Texto y páginas del documento (método de cada una: texto, PDF u OCR).
    extraction: Option<Extraction>,
    /// Espera que impone el error del proveedor (`unauthorized`, 404, 402). Se marca aquí y no por
    /// el código del error, porque los conectores de correo también usan `unauthorized` y esos
    /// fallos siguen siendo fallos.
    wait: Option<AiWait>,
    /// Proveedor con el que se pidió: si ya no es el activo, la espera no se vuelve a poner.
    used: Arc<dyn Provider>,
    /// `{from, reason}` si la respuesta la dio el respaldo local (D101).
    fallback: Option<Value>,
    /// Servidor al que se envió la petición; `None` con la IA local o si la atendió el respaldo.
    destination: Option<String>,
}

/// Resultado de pedir la generación, con el respaldo local si procede.
struct Attempt {
    result: Result<GenerateResult, TaskError>,
    /// `{from, reason}` si respondió el respaldo local.
    fallback: Option<Value>,
    /// Espera que impone el error del proveedor elegido (y no hubo respaldo).
    wait: Option<AiWait>,
}

/// Errores del externo que activan el respaldo local (D101). `unauthorized` e
/// `invalid_response` no: el primero pide corregir la clave y el segundo no es un fallo de
/// disponibilidad. Tampoco `model_unavailable`, `no_credit` ni `request_rejected` (D146): son
/// configuración o cuenta, y el respaldo nunca pasa a otro proveedor externo.
fn triggers_fallback(error: &AiError) -> bool {
    matches!(error.code(), "unavailable" | "timeout" | "rate_limited")
}

fn failure(provider: &dyn Provider, error: &AiError) -> TaskError {
    // El rechazo de una petición (D146) se explica con el texto de la persona, no con el código.
    let message = match error {
        AiError::RequestRejected { status, .. } => {
            request_rejected_text(provider_label(provider.kind()), *status)
        }
        other => other.to_string(),
    };
    TaskError {
        code: error.code().to_owned(),
        message,
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
    generate_with_fallback_split(provider, fallback, until, request, request)
}

/// Como `generate_with_fallback`, con una petición para el proveedor elegido y otra para la IA
/// local: la redacción lleva el perfil del negocio a la IA local sin consentimiento extra (no sale
/// del equipo) y al externo solo con él (D163).
fn generate_with_fallback_split(
    provider: &dyn Provider,
    fallback: Option<&LocalFallback>,
    until: &Mutex<Option<Instant>>,
    request: &GenerateRequest,
    local_request: &GenerateRequest,
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
                match local.generate(local_request) {
                    Ok(result) => {
                        return Attempt {
                            result: Ok(result),
                            fallback: Some(fallback_note(from, "unavailable")),
                            wait: None,
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
            wait: None,
        },
        Err(error) => {
            let Some(local) = fallback.filter(|_| triggers_fallback(&error)) else {
                return Attempt {
                    result: Err(failure(provider, &error)),
                    fallback: None,
                    wait: wait_for(&error),
                };
            };
            let outcome = match local_failure {
                Some(previous) => Err(previous),
                None => match local_provider(local) {
                    Some(local) => local.generate(local_request),
                    // Sin IA local preparada: el error del externo, como sin respaldo.
                    None => {
                        return Attempt {
                            result: Err(failure(provider, &error)),
                            fallback: None,
                            wait: None,
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
                        wait: None,
                    }
                }
                Err(local_error) => Attempt {
                    result: Err(TaskError {
                        code: error.code().to_owned(),
                        message: format!("{error}; respaldo local: {local_error}"),
                        recoverable: true,
                    }),
                    fallback: None,
                    wait: None,
                },
            }
        }
    }
}

/// Pide al proveedor local una generación que no puede ir a ningún otro: la redacción de una
/// tarea cuya clasificación ya usó el respaldo local (D101, D158). Nunca vuelve al proveedor
/// externo ni pasa a otro; `note` es el aviso de respaldo de la clasificación.
fn generate_local_only(
    fallback: Option<&LocalFallback>,
    note: &Value,
    request: &GenerateRequest,
) -> Attempt {
    let local = fallback
        .and_then(|local| local())
        .filter(|p| p.kind() == ProviderKind::Ollama);
    match local {
        Some(local) => match local.generate(request) {
            Ok(result) => Attempt {
                result: Ok(result),
                fallback: Some(note.clone()),
                wait: None,
            },
            Err(error) => Attempt {
                result: Err(failure(&*local, &error)),
                fallback: None,
                wait: None,
            },
        },
        None => Attempt {
            result: Err(task_error(
                "unavailable",
                "la IA local ya no está preparada y la redacción no se envía a ningún proveedor externo",
                true,
            )),
            fallback: None,
            wait: None,
        },
    }
}

impl Job {
    /// Extrae el texto si hace falta (PDF u OCR, localmente) y llama al proveedor elegido.
    /// Un error se devuelve tal cual: no hay otro proveedor ni extracción parcial.
    pub fn run(self) -> Completed {
        let Job {
            task,
            provider,
            destination,
            gate,
            fallback,
            fallback_until,
            extractor,
            profile,
            reply_style,
            prepared,
        } = self;
        let read_source: Option<String> = serde_json::from_str::<Value>(&task.input_ref)
            .ok()
            .filter(is_initial_read)
            .and_then(|i| i["read_source"].as_str().map(str::to_owned));
        let mut extracted = None;
        let mut wait = None;
        let mut used_fallback = None;
        let outcome = prepared.and_then(|prepared| {
            let request = match prepared {
                Prepared::Resume(result) => return Ok(Outcome::Resumed(result)),
                Prepared::Split(path, generated) => {
                    extracted = extractor.extract(&path).ok();
                    return Ok(Outcome::Generated(generated));
                }
                Prepared::Email { id, text } => {
                    let first = generate_with_fallback(
                        &*provider,
                        fallback.as_ref(),
                        &fallback_until,
                        &triage_request(&id, &text),
                    );
                    wait = first.wait;
                    used_fallback = first.fallback.clone();
                    let triage = first.result?;
                    let (category, needs_reply) = parse_triage(&triage.output)
                        .map_err(|e| task_error(e.code(), e.to_string(), true))?;
                    if !needs_reply {
                        return Ok(Outcome::Email(EmailGenerated {
                            meta: triage,
                            category: Some(category),
                            needs_reply,
                            draft: None,
                            profile_fp: None,
                            profile_text: None,
                        }));
                    }
                    let habit = learn_evidence::dominant_reply_kind(&reply_style, &category);
                    let (request, local_request) = draft_requests(
                        &format!("{id}/draft"),
                        &text,
                        profile.as_ref(),
                        habit.as_deref(),
                    );
                    // Si la clasificación la atendió el respaldo local (o el periodo de respaldo
                    // sigue abierto), la redacción se queda en local: el mismo correo nunca vuelve
                    // al proveedor externo (D101).
                    let second = match &used_fallback {
                        Some(note) => generate_local_only(fallback.as_ref(), note, &local_request),
                        None => generate_with_fallback_split(
                            &*provider,
                            fallback.as_ref(),
                            &fallback_until,
                            &request,
                            &local_request,
                        ),
                    };
                    wait = second.wait;
                    if used_fallback.is_none() {
                        used_fallback = second.fallback;
                    }
                    let drafted = second.result?;
                    let draft = parse_draft(&drafted.output)
                        .map_err(|e| task_error(e.code(), e.to_string(), true))?;
                    // La redacción salió por la IA local si hubo respaldo (en la clasificación o
                    // en ella); si no, por el proveedor elegido.
                    let (profile_fp, profile_text) =
                        profile_sent(profile.as_ref(), used_fallback.is_some());
                    return Ok(Outcome::Email(EmailGenerated {
                        meta: drafted,
                        category: Some(category),
                        needs_reply,
                        draft: Some(draft),
                        profile_fp,
                        profile_text,
                    }));
                }
                Prepared::Redraft { id, text, category } => {
                    let habit = category
                        .as_deref()
                        .and_then(|c| learn_evidence::dominant_reply_kind(&reply_style, c));
                    let (request, local_request) = draft_requests(
                        &format!("{id}/draft"),
                        &text,
                        profile.as_ref(),
                        habit.as_deref(),
                    );
                    let attempt = generate_with_fallback_split(
                        &*provider,
                        fallback.as_ref(),
                        &fallback_until,
                        &request,
                        &local_request,
                    );
                    wait = attempt.wait;
                    used_fallback = attempt.fallback;
                    let drafted = attempt.result?;
                    let draft = parse_draft(&drafted.output)
                        .map_err(|e| task_error(e.code(), e.to_string(), true))?;
                    let (profile_fp, profile_text) =
                        profile_sent(profile.as_ref(), used_fallback.is_some());
                    return Ok(Outcome::Email(EmailGenerated {
                        meta: drafted,
                        category: None,
                        needs_reply: true,
                        draft: Some(draft),
                        profile_fp,
                        profile_text,
                    }));
                }
                Prepared::Document(path) => {
                    let extraction = extractor
                        .extract(&path)
                        .map_err(|e| task_error(e.code(), e.to_string(), e.recoverable()))?;
                    // Primera lectura: solo sale lo que parece una factura según el texto ya
                    // extraído en el equipo, sin IA (E5, D133). Si la extracción falló, ya se
                    // devolvió el error de arriba y no sale nada.
                    let initial = serde_json::from_str::<Value>(&task.input_ref)
                        .is_ok_and(|i| is_initial_read(&i));
                    if initial {
                        if let Err(why) = looks_like_invoice(&extraction.text) {
                            extracted = Some(extraction);
                            return Ok(Outcome::Skipped(why.code()));
                        }
                    }
                    let request = extraction_request(&task.id.to_string(), &extraction.text);
                    extracted = Some(extraction);
                    request
                }
            };
            // Última comprobación antes de que salga nada: la lectura pudo detenerse, desconectarse
            // o cambiar de destino durante el OCR.
            if let Some(source) = &read_source {
                if gate.lock().map_or(true, |held| held.contains(source)) {
                    return Ok(Outcome::Held);
                }
            }
            let attempt =
                generate_with_fallback(&*provider, fallback.as_ref(), &fallback_until, &request);
            wait = attempt.wait;
            used_fallback = attempt.fallback;
            attempt.result.map(Outcome::Generated)
        });
        let destination = if used_fallback.is_some() {
            None
        } else {
            destination
        };
        Completed {
            task,
            outcome,
            extraction: extracted,
            wait,
            used: provider,
            fallback: used_fallback,
            destination,
        }
    }
}

/// Peticiones de redacción para el proveedor elegido y para la IA local (D163). El perfil va al
/// elegido si es local o hay consentimiento (`primary_ok`), y a la IA local siempre: no sale del
/// equipo. Sin perfil, ambas son iguales y no llevan nada.
fn draft_requests(
    id: &str,
    text: &str,
    profile: Option<&profile::ProfileUse>,
    habit: Option<&str>,
) -> (GenerateRequest, GenerateRequest) {
    let primary = profile.filter(|p| p.primary_ok).map(|p| p.text.as_str());
    let local = profile.map(|p| p.text.as_str());
    (
        draft_request_with_habit(id, text, primary, habit),
        draft_request_with_habit(id, text, local, habit),
    )
}

/// Huella y texto del perfil que de verdad llevó la redacción: el de la IA local si ella la
/// atendió, el del proveedor elegido si no (y solo si pudo recibirlo).
fn profile_sent(
    profile: Option<&profile::ProfileUse>,
    served_locally: bool,
) -> (Option<String>, Option<String>) {
    match profile.filter(|p| served_locally || p.primary_ok) {
        Some(p) => (Some(p.fp.clone()), Some(p.text.clone())),
        None => (None, None),
    }
}

/// Asunto de la respuesta: «Re: …» sin repetir el prefijo si ya lo trae.
fn reply_subject(subject: &str) -> String {
    let trimmed = subject.trim_start();
    if trimmed
        .get(..3)
        .is_some_and(|p| p.eq_ignore_ascii_case("re:"))
    {
        trimmed.to_owned()
    } else {
        format!("Re: {subject}")
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
            assistant_local: None,
            fallback_until: Arc::new(Mutex::new(None)),
            refining: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            auto_skip: None,
            retry_obsolete: None,
            key_fixed: false,
            refine_mode: refine::RefineMode::of_build(),
            reads_active: HashSet::new(),
            learning: learn_state::LearningRuntime::default(),
            continuous: learn_continuous::ContinuousRuntime::default(),
            assistant: assistant::AssistantRuntime::default(),
            read_gate: Arc::new(Mutex::new(HashSet::new())),
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
                self.complete_with(task.id, result, done.version, receipt, None)?;
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
        doc: Option<&SearchDoc>,
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
        self.complete_indexing(task_id, &result, doc)
    }

    /// Deja la tarea en revisión con el contenido dado, desde `running` o desde la revisión.
    fn back_to_review(
        &mut self,
        task: &Task,
        result: &Value,
        doc: Option<&SearchDoc>,
    ) -> AppResult<Task> {
        let current = self.store.task(task.id)?;
        match current.state {
            TaskState::AwaitingReview => Ok(self.store.revise(task.id, result)?),
            _ => self.review_indexing(task.id, result, doc),
        }
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
                return self.complete_with(task.id, result, version, receipt, None);
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
        self.back_to_review(&task, &result, None)
    }

    /// Vuelve a cargar en orden las facturas ya procesadas para seguir detectando duplicados.
    fn rebuild_registry(&mut self) -> AppResult<()> {
        let mut tasks = self.store.all_tasks()?;
        tasks.reverse();
        for task in tasks.iter().filter(|t| t.kind == INVOICE) {
            if let Some(fields) = task
                .result
                .as_ref()
                .map(|r| &r["fields"])
                .filter(|f| f.is_object())
            {
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
        // Una espera nacida de la respuesta del proveedor (D146) deja de valer si cambia el
        // proveedor, el modelo o el destino; una nueva resolución del mismo no la levanta.
        let same = self.provider.kind() == provider.kind()
            && self.provider.model() == provider.model()
            && self.provider.destination() == provider.destination()
            && self.provider.configured_url() == provider.configured_url();
        if !same {
            self.lift_provider_wait();
        }
        self.provider = provider;
        self.key_fixed = true;
        // Lo que está en cola o en curso de una primera lectura no sigue hacia el destino nuevo.
        self.refresh_read_gate();
        // El consentimiento del afinado automático es de un destino concreto (D134).
        self.drop_consent_if_destination_changed();
        self.drop_profile_consent_if_destination_changed();
        self.drop_learning_consent_if_destination_changed();
        self.drop_assistant_consent_if_destination_changed(!same);
        self.expire_fallback_period();
    }

    /// Fija o quita la espera de IA (D101). Con una espera, las tareas que necesitan el
    /// proveedor se quedan en `queued` sin gastar intentos; al quitarla se retoman.
    ///
    /// Quitarla (`None`) no levanta una espera de D146 (404, 402): esas dependen de lo que
    /// respondió el proveedor y solo las quitan un cambio de proveedor o de modelo
    /// (`select_provider`), una prueba con éxito o el sondeo (`lift_provider_wait`).
    pub fn set_ai_wait(&mut self, wait: Option<AiWait>) {
        if wait.is_none() && self.ai_wait.is_some_and(AiWait::is_provider_response) {
            return;
        }
        if wait.is_none() && self.ai_wait == Some(AiWait::InvalidKey) {
            self.key_fixed = true;
        }
        self.ai_wait = wait;
    }

    /// Levanta la espera de D146 (modelo no disponible o sin crédito) y devuelve si había una.
    /// La usan el cambio de modelo o de clave, la prueba sintética con éxito y el sondeo.
    pub fn lift_provider_wait(&mut self) -> bool {
        if self.ai_wait.is_some_and(AiWait::is_provider_response) {
            self.ai_wait = None;
            // Como al corregir una clave: un fallo de afinado por esta causa ya no vale (D134).
            self.key_fixed = true;
            return true;
        }
        false
    }

    /// Texto de la espera actual, con el proveedor y el modelo de ahora. `address` es la dirección
    /// configurada; solo se cita con el compatible con OpenAI.
    pub fn ai_wait_message(&self, address: Option<&str>) -> Option<String> {
        let wait = self.ai_wait?;
        let kind = self.provider.kind();
        let address = address
            .map(str::to_owned)
            .or_else(|| self.provider.configured_url())
            .filter(|_| kind == ProviderKind::OpenAiCompatible);
        Some(wait.describe(kind, self.provider.model(), address.as_deref()))
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

    /// Fija la IA local del asistente del chat (D183): un resolvedor que NO arranca nada y que
    /// devuelve la IA local solo si ya está en marcha. Quien lo llama lo da solo si el proveedor
    /// elegido es la IA local o el respaldo local está activado; con `None` el asistente no usa
    /// ninguna IA local fuera del caso en que esta es el proveedor elegido.
    pub fn set_assistant_local(&mut self, local: Option<LocalFallback>) {
        self.assistant_local = local;
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
        self.store_document(name, "txt", text.as_bytes(), origin, None)
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
        self.store_document(name, &extension, bytes, origin, None)
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
        self.import_attachment_keyed(message_task, name, bytes, "", "", None)
    }

    fn import_attachment_keyed(
        &mut self,
        message_task: i64,
        name: &str,
        bytes: &[u8],
        attachment_key: &str,
        attachment_id: &str,
        step: Option<i64>,
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
        // Un adjunto de un correo de la primera lectura lo es también: se hereda la marca y la
        // fuente, de modo que ni la primera lectura ni una consulta posterior del buzón lo
        // convierten en un pendiente (E5).
        let read =
            is_initial_read(&input).then(|| input["read_source"].as_str().unwrap_or("").to_owned());
        self.store_document_for(name, &extension, bytes, origin, read.as_deref(), step)
    }

    /// Guarda la copia y encola la tarea. `origin` dice de dónde vino; aquí se añaden la
    /// fecha de importación, el tamaño y la huella SHA-256 del contenido.
    fn store_document(
        &mut self,
        name: &str,
        extension: &str,
        bytes: &[u8],
        origin: Value,
        initial_read: Option<&str>,
    ) -> AppResult<Task> {
        self.store_document_for(name, extension, bytes, origin, initial_read, None)
    }

    /// Como `store_document`; con `step`, la tarea es la de ese paso de un pipeline y se crea
    /// junto con su vínculo en una transacción (D148): repetir la descarga no la duplica.
    fn store_document_for(
        &mut self,
        name: &str,
        extension: &str,
        bytes: &[u8],
        mut origin: Value,
        initial_read: Option<&str>,
        step: Option<i64>,
    ) -> AppResult<Task> {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).ok();
        let stamp = now.map(|d| d.as_nanos()).unwrap_or(0);
        let path = self.documents.join(format!("{stamp}.{extension}"));
        fs::write(&path, bytes)?;
        origin["imported_at"] = json!(now.map(|d| d.as_secs()));
        origin["sha256"] = json!(hex(&Sha256::digest(bytes)));
        origin["size"] = json!(bytes.len());
        let mut input = json!({ "document": name, "path": path, "origin": origin });
        if let Some(source) = initial_read {
            input["initial_read"] = json!(true);
            input["read_source"] = json!(source);
        }
        Ok(match step {
            Some(step_id) => self
                .store
                .enqueue_for_step(step_id, INVOICE, &input.to_string())?,
            None => self.store.enqueue(INVOICE, &input.to_string())?,
        })
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
        self.locate_document(task_id, &input)
    }

    /// Ruta de la copia guardada según la entrada de una tarea, comprobada: dentro de la carpeta
    /// de documentos y existente. La usan `document_file` y el indexado (D179).
    fn locate_document(&self, task_id: i64, input: &Value) -> AppResult<PathBuf> {
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
        let mut read_tasks: HashSet<i64> = HashSet::new();
        let mut imported: HashSet<String> = HashSet::new();
        for task in self.store.all_tasks()? {
            let input: Value = serde_json::from_str(&task.input_ref).unwrap_or(Value::Null);
            match task.kind.as_str() {
                // Una redacción pedida (D159) comparte el mensaje con su origen: no cuenta como
                // «ya tiene tarea», para que el mensaje siga ligado a la tarea original.
                EMAIL if input.get("redraft_of").is_some() => {}
                EMAIL => {
                    if let Some(id) = input["message_id"].as_str() {
                        let (account, inner) = split_id(id);
                        known.insert(accounts::join_id(account, inner), task.id);
                        if is_initial_read(&input) {
                            read_tasks.insert(task.id);
                        }
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
        // Plantillas activas (D149): solo se miran al encolar un correo nuevo.
        let templates = self.active_task_templates()?;
        // Preferencias por remitente (D161), leídas una vez por consulta.
        let preference_index =
            preferences::PreferenceIndex::from_rows(&self.store.sender_preferences()?);
        let mut report = FetchReport {
            added: 0,
            limit: self.mail.limit(),
            accounts: Vec::new(),
        };
        for (account, outcome) in self.mail.read_all() {
            let fetch = match outcome {
                Ok(messages) => {
                    let mut added = 0;
                    let mut informative = 0;
                    let mut attachments = AttachmentFetch::default();
                    for message in &messages {
                        let task_id = match known.get(&message.id) {
                            Some(id) => *id,
                            None => {
                                let input_value = json!({
                                    "message_id": message.id,
                                    "account": account,
                                    "origin": self.mail.origin(&account),
                                    "message": message,
                                });
                                let origin = input_value["origin"].clone();
                                let (_, found) = route_of_message(
                                    message.from.as_deref(),
                                    message.signals.as_deref(),
                                );
                                // Precedencia de D161: rebote, hilo propio (solo metadatos,
                                // D162), preferencia, señal fuerte, IA. Un fallo al mirar el
                                // hilo es «no se sabe» y no oculta nada.
                                let (decision, scope) = preferences::decide_new_mail(
                                    &preference_index,
                                    message.from.as_deref(),
                                    &found,
                                    || {
                                        let key = message.thread_key.as_deref()?;
                                        self.mail.last_message_is_ours(&account, key).ok().flatten()
                                    },
                                );
                                let route = decision.route;
                                let input = input_value.to_string();
                                // La ruta local (D157) se decide aquí, sin IA y de una vez: el
                                // informativo y el rebote nacen ya cerrados o en revisión, en una
                                // sola transacción, así que un corte no puede dejarlos en la cola.
                                let task = match route {
                                    Route::Informativo => {
                                        let result = triaged_result(
                                            message, origin, &decision, scope, &found, false,
                                        );
                                        let doc = search::email_doc(&input_value, &result);
                                        self.enqueue_born_indexing(
                                            &input,
                                            &result,
                                            "correo informativo",
                                            false,
                                            doc.as_ref(),
                                        )?
                                    }
                                    Route::Rebote => {
                                        let result = triaged_result(
                                            message, origin, &decision, scope, &found, true,
                                        );
                                        let doc = search::email_doc(&input_value, &result);
                                        self.enqueue_born_indexing(
                                            &input,
                                            &result,
                                            "posible fallo de entrega",
                                            true,
                                            doc.as_ref(),
                                        )?
                                    }
                                    // Solo un correo que va a la IA puede abrir una ejecución
                                    // de plantilla (D149): el informativo y el rebote no tienen
                                    // respuesta que retener y sus adjuntos siguen el flujo normal.
                                    Route::Ia => {
                                        self.enqueue_new_mail(&input, message, &templates)?
                                    }
                                };
                                // Los contadores, solo con la tarea ya guardada.
                                if route == Route::Informativo {
                                    informative += 1;
                                } else {
                                    added += 1;
                                }
                                known.insert(message.id.clone(), task.id);
                                task.id
                            }
                        };
                        // Los adjuntos de un correo de la primera lectura son cosa de esa lectura
                        // (con su tope y su comprobación): una consulta no los importa (D133).
                        if read_tasks.contains(&task_id) {
                            continue;
                        }
                        self.import_attachments(
                            task_id,
                            message,
                            &mut imported,
                            &mut attachments,
                            None,
                        )?;
                    }
                    AccountFetch {
                        account,
                        read: messages.len(),
                        added,
                        informative,
                        error: None,
                        attachments,
                    }
                }
                Err(error) => AccountFetch {
                    account,
                    read: 0,
                    added: 0,
                    informative: 0,
                    error: Some(MailIssue::from(&error)),
                    attachments: AttachmentFetch::default(),
                },
            };
            report.added += fetch.added;
            // Una consulta correcta adelanta la siguiente pasada de aprendizaje continuo (D178).
            if fetch.error.is_none() {
                self.continuous_advance(&fetch.account);
            }
            self.mail.record_fetch(fetch.clone());
            report.accounts.push(fetch);
        }
        Ok(report)
    }

    /// Importa como documentos los adjuntos admitidos de un mensaje que aún no se importaron.
    /// La clave de un adjunto es el mensaje, su nombre y su tamaño, que no cambian entre
    /// consultas (el identificador de Gmail sí puede cambiar). Un fallo de descarga queda en
    /// el informe y se reintenta en la consulta siguiente; nunca se pierde en silencio.
    ///
    /// Con `cap = Some((presupuesto, omitidos))` (primera lectura, D133) no se importa más de los
    /// documentos que quedan en el presupuesto; los que sobran se cuentan en `omitidos`.
    fn import_attachments(
        &mut self,
        message_task: i64,
        message: &Message,
        imported: &mut HashSet<String>,
        report: &mut AttachmentFetch,
        mut cap: Option<(&mut usize, &mut usize)>,
    ) -> AppResult<()> {
        // Pasos de pipeline que esperan su adjunto (D149). La primera lectura nunca tiene.
        let rejected: HashSet<String> = if cap.is_none() {
            self.store
                .rejected_attachment_keys(message_task)?
                .into_iter()
                .collect()
        } else {
            HashSet::new()
        };
        let mut steps: HashMap<String, i64> = if cap.is_none() {
            self.store
                .pending_attachment_steps(message_task)?
                .into_iter()
                .filter_map(|s| Some((s.attachment_key?, s.id)))
                .collect()
        } else {
            HashMap::new()
        };
        for attachment in &message.attachments {
            let name = attachment_name(attachment);
            let key = attachment_key(&message.id, &name, attachment.size);
            // Rechazado de forma definitiva: no se vuelve a descargar (D150).
            if imported.contains(&key) || rejected.contains(&key) {
                continue;
            }
            let step = steps.get(&key).copied();
            if !supported_document(&name) {
                report.skipped.push(name);
                continue;
            }
            if let Some((budget, over)) = cap.as_mut() {
                if **budget == 0 {
                    **over += 1;
                    continue;
                }
            }
            let fail = |code: &str, message: String| AttachmentIssue {
                name: name.clone(),
                code: code.to_owned(),
                message,
            };
            if let Some(reason) = attachment_rejection(attachment.size) {
                report.failed.push(fail("invalid_input", reason));
                if let Some(step) = step {
                    self.store
                        .record_step_error(step, nestadmin_core::ATTACHMENT_REJECTED)?;
                }
                continue;
            }
            let bytes = match self.mail.fetch_attachment(&message.id, &attachment.id) {
                Ok(bytes) => bytes,
                Err(e) => {
                    // El paso queda sin tarea y con un código corto, nunca contenido; el
                    // reintento de la consulta siguiente lo enlaza sin duplicar.
                    if let Some(step) = step {
                        self.store.record_step_error(step, &e.code)?;
                    }
                    report.failed.push(fail(&e.code, e.message));
                    continue;
                }
            };
            match self.import_attachment_keyed(
                message_task,
                &name,
                &bytes,
                &key,
                &attachment.id,
                step,
            ) {
                Ok(_) => {
                    steps.remove(&key);
                    imported.insert(key);
                    report.imported += 1;
                    if let Some((budget, _)) = cap.as_mut() {
                        **budget = budget.saturating_sub(1);
                    }
                }
                Err(AppError::Invalid(reason)) => {
                    if let Some(step) = step {
                        self.store
                            .record_step_error(step, nestadmin_core::ATTACHMENT_REJECTED)?;
                    }
                    report.failed.push(fail("invalid_input", reason));
                }
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
        self.refresh_read_gate();
        let claimed = if self.ai_wait.is_none() {
            // Las tareas de la primera lectura esperan a las normales: una lectura de cientos
            // de mensajes no retrasa el correo nuevo (E5).
            self.claim_normal_first()?
        } else {
            // Sin IA utilizable solo avanzan las tareas que no la necesitan: reanudar una
            // acción ya intentada (D50) o una factura ya separada (D68).
            let mut found = None;
            for queued in self.store.ready_tasks()? {
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
        let initial = is_initial_read(&input);
        // Una tarea de la primera lectura no tiene acción que reanudar (E5).
        let attempted = if initial {
            None
        } else {
            self.attempted_result(&task)?
        };
        if let Some(result) = attempted {
            return Ok(Some(Job {
                task,
                provider: Arc::clone(&self.provider),
                destination: self.provider.destination(),
                gate: Arc::clone(&self.read_gate),
                fallback: self.fallback.clone(),
                fallback_until: Arc::clone(&self.fallback_until),
                extractor: Arc::clone(&self.extractor),
                profile: None,
                reply_style: Vec::new(),
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
                        from,
                        to,
                        date,
                        ..
                    }) => {
                        // Remitente, destinatarios, fecha y asunto saneados, y el cuerpo (D102).
                        let text = nestadmin_ai::tasks::email_text(
                            subject.as_deref(),
                            from.as_deref(),
                            to.as_deref(),
                            date.as_deref(),
                            &body,
                        );
                        if initial {
                            // La primera lectura no envía ningún correo a un proveedor (D133):
                            // `claim_normal_first` no los toma, y si uno llegara aquí falla.
                            Err(task_error(
                                "invalid_input",
                                "la primera lectura no envía correos a ningún proveedor",
                                false,
                            ))
                        } else if let Some(origin) = input["redraft_of"].as_i64() {
                            // «Redactar respuesta» (D159): solo la redacción; no se clasifica otra vez.
                            // La categoría es la del correo de origen (si consta).
                            let category = self
                                .store
                                .task_by_id(origin)
                                .ok()
                                .flatten()
                                .and_then(|t| t.result)
                                .and_then(|r| r["category"].as_str().map(str::to_owned));
                            Ok(Prepared::Redraft {
                                id: id.clone(),
                                text,
                                category,
                            })
                        } else {
                            Ok(Prepared::Email { id: id.clone(), text })
                        }
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
            destination: self.provider.destination(),
            gate: Arc::clone(&self.read_gate),
            fallback: self.fallback.clone(),
            fallback_until: Arc::clone(&self.fallback_until),
            extractor: Arc::clone(&self.extractor),
            // Solo la redacción de un correo lleva el perfil (D163); se decide al tomar la tarea,
            // con el consentimiento de ese momento.
            profile: match &prepared {
                Ok(Prepared::Email { .. } | Prepared::Redraft { .. }) => self.profile_for_draft(),
                _ => None,
            },
            // Solo la redacción lleva la línea de respuesta habitual (D171). Un fallo de lectura
            // equivale a no tener datos: la línea no aparece.
            reply_style: match &prepared {
                Ok(Prepared::Email { .. } | Prepared::Redraft { .. }) => {
                    self.store.reply_style_rows().unwrap_or_default()
                }
                _ => Vec::new(),
            },
            prepared,
        }))
    }

    /// Valida la respuesta, aplica reglas y deja la tarea en su estado siguiente. Al cerrar la
    /// última tarea de una primera lectura deja su entrada agregada en el Historial (D133).
    pub fn finish(&mut self, completed: Completed) -> AppResult<Task> {
        let initial = task_is_initial_read(&completed.task);
        let done = self.finish_task(completed)?;
        if initial {
            self.after_initial_task(&done);
        }
        Ok(done)
    }

    fn finish_task(&mut self, completed: Completed) -> AppResult<Task> {
        let task = completed.task;
        let destination = completed.destination;
        let fallback = completed.fallback;
        if let Some(wait) = completed.wait {
            // La clave no sirve, el modelo no existe o no hay crédito: la tarea vuelve a la cola
            // sin gastar intento ni fallar y la ventana lo explica (D101, D146). Si el proveedor
            // ya cambió mientras la llamada seguía en curso, la respuesta es del anterior y no se
            // vuelve a poner la espera; la tarea igualmente vuelve a la cola.
            if Arc::ptr_eq(&completed.used, &self.provider) {
                self.ai_wait = Some(wait);
            }
            let used = &completed.used;
            let reason = wait.describe(used.kind(), used.model(), used.configured_url().as_deref());
            return Ok(self.store.release(task.id, &reason)?);
        }
        let (generated, email) = match completed.outcome {
            Ok(Outcome::Generated(generated)) => (Some(generated), None),
            Ok(Outcome::Email(email)) => (None, Some(email)),
            Ok(Outcome::Resumed(mut result)) => {
                if task_is_initial_read(&task) {
                    // Nunca se reanuda una acción en la primera lectura: se completa sin ella.
                    if let Some(fields) = result.as_object_mut() {
                        fields.remove("action");
                    }
                    return Ok(self.store.complete(task.id, &result)?);
                }
                return self.resume(task, result);
            }
            Ok(Outcome::Held) => {
                return Ok(self
                    .store
                    .release(task.id, "lectura retenida: no se envió nada")?)
            }
            Ok(Outcome::Skipped(code)) => {
                return self.finish_skipped(&task, code, completed.extraction.as_ref())
            }
            Err(error) => return Ok(self.store.fail(task.id, &error)?),
        };
        let initial = task_is_initial_read(&task);
        if let Some(note) = fallback.as_ref().filter(|_| !initial) {
            // Solo el proveedor de origen y el motivo: ni contenido ni secretos (D101). En la
            // primera lectura no se escribe en el Historial: el respaldo consta en el resultado.
            self.store
                .record_event(task.id, "ai_fallback", "used", note)?;
        }
        let built = match (task.kind.as_str(), &generated, &email) {
            (INVOICE, Some(generated), _) => {
                self.invoice_result(&task, generated, completed.extraction.as_ref())
            }
            (_, _, Some(email)) => self.email_result(&task, email),
            _ => Err(task_error(
                "invalid_response",
                "la respuesta del proveedor no corresponde a la tarea",
                true,
            )),
        };
        let mut result = match built {
            Ok(result) => result,
            Err(error) => return Ok(self.store.fail(task.id, &error)?),
        };
        if let Some(note) = fallback {
            result["fallback"] = note;
        }
        if let (INVOICE, Some(generated)) = (task.kind.as_str(), &generated) {
            if let Err(error) = self.enqueue_other_invoices(&task, generated, &mut result) {
                return Ok(self.store.fail(task.id, &error)?);
            }
        }
        // La fila del índice (D179) se calcula aquí, donde el texto extraído está disponible;
        // si no se puede calcular, el resultado se guarda igual y la recoge la pasada de fondo.
        let doc = search::doc_on_finish(&task, &result, completed.extraction.as_ref());
        if initial {
            return self.finish_initial_read(&task, result, destination, doc.as_ref());
        }
        let kind = action_kind(&task);
        // Un importe dudoso o una factura separada de un documento con varias nunca se
        // registran por regla (D120, D68): `evaluate_result` los resuelve antes de las reglas.
        let (rules, any_sender) = self.rules_for_evaluation()?;
        // Si se continuó sin un paso anterior del pipeline, la tarea siempre se revisa (D150).
        // Si no se puede consultar, se supone omitido (más restrictivo) y no se pierde el resultado.
        let step_waived = match self.store.has_waived_prerequisite(task.id) {
            Ok(waived) => waived,
            Err(error) => {
                result["evidence_error"] =
                    json!(format!("no se pudo comprobar el pipeline: {error}"));
                true
            }
        };
        let evaluation = evaluate_result_in_run(&rules, &any_sender, kind, &result, step_waived);
        // Una regla aprendida aceptada en lote revisa sus primeros envíos (D174). Va después de la
        // evaluación completa: los bloqueos de `send_draft_blocker` ya tuvieron prioridad y solo una
        // decisión de automatizar puede pasar a Rodaje.
        let evaluation = self.apply_rodaje(evaluation, &mut result);
        // Solo ids, códigos e importes o nombres que ya están en el resultado (D125).
        result["evaluation"] = json!(evaluation.explanation);
        // La evidencia (D129) no puede impedir ni duplicar la acción: si no se guarda, la
        // acción sigue su curso y el fallo consta en el resultado de la tarea. Una redacción
        // pedida a mano no es una decisión de las reglas y no cuenta en sus estadísticas.
        if result.get("redraft_of").is_none() {
            if let Err(error) =
                self.record_evidence(&task, &result, kind, &rules, &any_sender, &evaluation)
            {
                result["evidence_error"] = json!(format!("no se guardó la evaluación: {error}"));
            }
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
                self.execute(task, result, authorization, doc.as_ref())
            }
            RuleDecision::Review(reason) => {
                result["review_reason"] = json!(reason);
                if result["needs_reply"].as_bool() == Some(false) {
                    // La IA no oculta correos: queda en revisión con el aviso (D158).
                    result["review_reason"] = json!(NO_REPLY_REASON);
                }
                self.review_indexing(task.id, &result, doc.as_ref())
            }
        }
    }

    /// Rodaje de las reglas aprendidas (D174): si la regla que automatizaría tiene envíos de rodaje
    /// pendientes, la tarea va a revisión con el motivo `Rodaje`. Una regla sin rodaje (la mayoría)
    /// no cambia nada. Si no se puede leer el contador, falla cerrado: va a revisión.
    fn apply_rodaje(&self, evaluation: Evaluation, result: &mut Value) -> Evaluation {
        let Some((rule_id, _)) = evaluation.automating_rule() else {
            return evaluation;
        };
        match self.store.rodaje_state(rule_id) {
            Ok(Some(state)) if state.active() => evaluation.into_rodaje(&state),
            Ok(_) => evaluation,
            Err(error) => {
                result["evidence_error"] = json!(format!(
                    "no se pudo comprobar el rodaje de la regla: {error}"
                ));
                evaluation.into_rodaje(&nestadmin_core::RodajeState::closed())
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
        // Una factura cancelada mientras corría no crea partes nuevas: nadie las pidió ya.
        let running = self
            .store
            .task_by_id(task.id)
            .map_err(|e| task_error("unavailable", e.to_string(), true))?
            .is_some_and(|t| t.state == TaskState::Running);
        if !running {
            return Ok(());
        }
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
            self.enqueue_part(task, index, of, &part_input.to_string())
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
        // Las facturas parecidas que solo se leyeron en la primera lectura no se registraron
        // nunca ni la persona las ve en Tareas: se dice (E5, D133).
        let duplicates_initial_read: Vec<String> = report
            .duplicates
            .iter()
            .filter_map(|d| {
                let id = d.existing_id.parse::<i64>().ok()?;
                let existing = self.store.task_by_id(id).ok().flatten()?;
                task_is_initial_read(&existing).then(|| d.existing_id.clone())
            })
            .collect();
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
            "duplicates_initial_read": duplicates_initial_read,
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

    /// Cierra una tarea de la primera lectura (E5, D133): se completa sin `action`, sin evaluar
    /// reglas, sin crear acciones ni pendientes y sin escribir en el Historial. Una regla activa
    /// que coincidiría no actúa. Una factura deja una observación («vista», no aprobada) que
    /// solo alimenta las sugerencias de facturas. Reprocesar la tarea no añade nada más: la
    /// observación es única por tarea y versión.
    fn finish_initial_read(
        &mut self,
        task: &Task,
        mut result: Value,
        destination: Option<String>,
        doc: Option<&SearchDoc>,
    ) -> AppResult<Task> {
        if let Some(fields) = result.as_object_mut() {
            fields.remove("action");
        }
        result["initial_read"] = json!(true);
        // A qué servidor se enviaron los datos de esta tarea (`null`: la IA local), para el
        // recuento por proveedor del final de la lectura.
        result["host"] = json!(destination);
        if task.kind == INVOICE {
            let draft = panel::signal_for(task, &result, "observed");
            if let Err(error) = self.record_signal("observed", draft) {
                result["evidence_error"] = json!(format!("no se guardó la observación: {error}"));
            }
        }
        self.complete_indexing(task.id, &result, doc)
    }

    /// Cierra un documento de la primera lectura que no parece una factura: no se envió a ningún
    /// proveedor y queda como omitido, sin campos ni observación (D133).
    fn finish_skipped(
        &mut self,
        task: &Task,
        code: &str,
        extraction: Option<&Extraction>,
    ) -> AppResult<Task> {
        let input: Value = serde_json::from_str(&task.input_ref).unwrap_or(Value::Null);
        // Lo que no parece una factura no se queda copiado en la carpeta de documentos (D133).
        // `copy_removed` refleja el borrado real: si la copia no existía ya o se borró, `true`;
        // si el borrado falló, `false` con el motivo (la tarea no se queda con un dato falso).
        let mut copy_removed = true;
        let mut copy_error = None;
        if let Some(path) = input["path"].as_str().map(PathBuf::from) {
            if path.starts_with(&self.documents) {
                match fs::remove_file(&path) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => {
                        copy_removed = false;
                        copy_error = Some(e.to_string());
                    }
                }
            }
        }
        let result = json!({
            "type": INVOICE,
            "version": 1,
            "initial_read": true,
            "copy_removed": copy_removed,
            "copy_remove_error": copy_error,
            "document": input["document"],
            "origin": input["origin"],
            "pages": extraction.and_then(|e| serde_json::to_value(&e.pages).ok()),
            "skipped": { "reason": code },
        });
        Ok(self.store.complete(task.id, &result)?)
    }

    /// Toma la tarea en cola más antigua que no sea de la primera lectura; si solo quedan de
    /// esas, la más antigua de ellas que se pueda enviar: no los correos (no se envían), ni las
    /// de una lectura detenida o cuyo destino cambió desde que se mostró (D133).
    fn claim_normal_first(&mut self) -> AppResult<Option<Task>> {
        let held = self.held_read_sources()?;
        let queued: Vec<Task> = self
            .store
            .ready_tasks()?
            .into_iter()
            .filter(|t| {
                let Ok(input) = serde_json::from_str::<Value>(&t.input_ref) else {
                    return true;
                };
                !is_initial_read(&input)
                    || (t.kind != EMAIL
                        && !input["read_source"]
                            .as_str()
                            .is_some_and(|s| held.contains(s)))
            })
            .collect();
        let next = queued
            .iter()
            .find(|t| !task_is_initial_read(t))
            .or_else(|| queued.first());
        match next {
            Some(task) => Ok(Some(self.store.claim(task.id)?)),
            None => Ok(None),
        }
    }

    /// Resultado de un correo a partir de las dos llamadas (D158). Sin respuesta necesaria no hay
    /// borrador ni `action`; un borrador que filtra instrucciones o marcadores es respuesta
    /// inválida (reintentable) y uno con enlaces, direcciones, teléfonos o IBAN lleva
    /// `draft_flags` (D160).
    fn email_result(&mut self, task: &Task, mail: &EmailGenerated) -> Result<Value, TaskError> {
        let input: Value = serde_json::from_str(&task.input_ref).unwrap_or(Value::Null);
        let message = self
            .task_message(&input)
            .map_err(|e| task_error(&e.code, e.message, true))?
            .ok_or_else(|| task_error("not_found", "el mensaje ya no existe", false))?;
        // Tareas anteriores a D48 no guardaron el origen: se toma el de la cuenta actual.
        let origin = match input.get("origin").filter(|o| !o.is_null()) {
            Some(origin) => origin.clone(),
            None => {
                serde_json::to_value(self.mail.origin(split_id(&message.id).0)).unwrap_or_default()
            }
        };
        // «Redactar» conserva la categoría con la que se clasificó el correo de origen.
        let category = mail
            .category
            .clone()
            .or_else(|| input["category"].as_str().map(str::to_owned));
        let mut result = json!({
            "type": EMAIL,
            "version": 1,
            "origin": origin,
            "message": message,
            "needs_reply": mail.needs_reply,
            "provider": mail.meta.provider,
            "model": mail.meta.model,
            "latency_ms": mail.meta.latency_ms,
        });
        if let Some(category) = category {
            result["category"] = json!(category);
        }
        if let Some(source) = input["redraft_of"].as_i64() {
            result["redraft_of"] = json!(source);
        }
        let Some(body) = &mail.draft else {
            return Ok(result);
        };
        if draft_leaks_internals(body) {
            return Err(task_error(
                "invalid_response",
                "el borrador repite instrucciones, marcadores o nombres internos; se descarta",
                true,
            ));
        }
        // Sin remitente no hay a quién responder: el borrador se muestra, pero no se puede enviar.
        let to: Vec<String> = message.from.iter().cloned().collect();
        let subject = message.subject.as_ref().map(|s| reply_subject(s));
        let automatic: Vec<&str> = to
            .iter()
            .flat_map(|address| nestadmin_core::mail_signals::address_signals(Some(address)))
            .map(|s| s.code())
            .collect();
        result["draft"] = json!({ "to": to, "subject": subject, "body": body });
        // Los contactos del perfil que llevó la redacción no se marcan (D160, D163); el resto sí.
        // El asunto lo escribe el remitente y sale con la respuesta: se examina con el cuerpo.
        let outgoing = match &subject {
            Some(subject) => format!("{subject}\n{body}"),
            None => body.to_owned(),
        };
        result["draft_flags"] = json!(draft_flags_allowing(
            &outgoing,
            mail.profile_text.as_deref()
        ));
        if let Some(fp) = &mail.profile_fp {
            // Solo la huella: el texto del perfil no se guarda en ningún resultado.
            result["profile_fp"] = json!(fp);
        }
        if !automatic.is_empty() {
            // Aviso para la revisión: el destinatario parece una dirección automática.
            result["recipient_signals"] = json!(automatic);
        }
        result["action"] = json!({ "kind": SEND_DRAFT });
        Ok(result)
    }

    /// Ejecuta la acción de la tarea una sola vez y la completa; un fallo del conector deja
    /// la tarea fallida y visible, nunca como éxito.
    fn execute(
        &mut self,
        task: Task,
        mut result: Value,
        authorization: Authorization,
        doc: Option<&SearchDoc>,
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
                return self.complete_with(task.id, result, version, receipt, doc);
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
                return self.back_to_review(&task, &result, doc);
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
                self.complete_indexing(task.id, &result, doc)
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
    /// Envoltorio de `approve_reviewed` sin la confirmación de importes (D186): una factura con
    /// importe dudoso solo se aprueba por `approve_reviewed(…, true)`.
    pub fn approve(&mut self, task_id: i64, version: i64) -> AppResult<Task> {
        self.approve_reviewed(task_id, version, false)
    }

    /// Aprueba la versión que el usuario ha visto. `amounts_checked` es la confirmación de que la
    /// persona comprobó los importes con el documento; sin ella, una factura con importe dudoso
    /// (D120) se rechaza con `AmountAckRequired` sin ejecutar nada (D186). Orden de comprobaciones:
    /// estado, versión, borrador e importes.
    pub fn approve_reviewed(
        &mut self,
        task_id: i64,
        version: i64,
        amounts_checked: bool,
    ) -> AppResult<Task> {
        let (task, result) = self.reviewable(task_id)?;
        let current = result["version"].as_i64().unwrap_or(1);
        if current != version {
            return Err(AppError::StaleVersion {
                approved: version,
                current,
            });
        }
        // Sin borrador no hay nada que enviar: se rechaza antes de anotar señal, intento o acción.
        if task.kind == EMAIL
            && result["draft"]["body"]
                .as_str()
                .is_none_or(|body| body.trim().is_empty())
        {
            return Err(AppError::Invalid(
                "el correo no tiene un borrador que aprobar; archívelo o pida una respuesta".into(),
            ));
        }
        if !amounts_checked && task.kind == INVOICE && result["amount_review"].as_str().is_some() {
            return Err(AppError::AmountAckRequired);
        }
        // La aprobación es la decisión de la persona: se anota antes de ejecutar para que un
        // fallo al anotarla conste en el resultado sin impedir ni repetir la acción.
        let mut result = result;
        let signal = panel::signal_for(&task, &result, "approved");
        if let Err(error) = self.record_signal("approved", signal) {
            result["evidence_error"] = json!(format!("no se guardó la señal: {error}"));
        }
        let task_id = task.id;
        // Si la tarea estaba en revisión por el Rodaje de una regla aprendida, aprobarla sin editar
        // y que se envíe baja el contador de esa regla (D174), en la misma transacción que la señal.
        let rodaje = rodaje_rule(&result);
        let done = self.execute(task, result, Authorization::Approval, None)?;
        // La aprobación solo vale como señal si la acción de esta versión quedó confirmada o
        // reconciliada; si falló o volvió a revisión, la señal queda anulada (D129).
        let confirmed = done.state == TaskState::Completed
            && done.result.as_ref().is_some_and(|r| {
                r["version"].as_i64() == Some(version) && r.get("sent_version").is_none()
            });
        if let Err(error) =
            self.store
                .settle_approval(task_id, version, confirmed, rodaje.as_deref())
        {
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
        if !result["draft"].is_object() {
            return Err(AppError::Invalid(
                "esta tarea no tiene borrador; pida una respuesta".into(),
            ));
        }
        result["draft"]["body"] = json!(body);
        // Las marcas (D160) son las del texto nuevo, no las del borrador anterior.
        result["draft_flags"] = json!(self.outgoing_flags(&result, body));
        // Marca que una persona cambió el texto: la aprobación posterior ya no es «sin
        // cambios» y no sirve de evidencia para sugerir una regla (D129).
        result["edited"] = json!(true);
        result["version"] = json!(result["version"].as_i64().unwrap_or(1) + 1);
        let signal = panel::signal_for(&task, &result, "edited");
        if let Err(error) = self.record_signal("edited", signal) {
            result["evidence_error"] = json!(format!("no se guardó la señal: {error}"));
        }
        // Editar un envío de rodaje reinicia el rodaje de su regla (D174).
        if let Some(rule) = rodaje_rule(&result) {
            if let Err(error) = self.store.rodaje_reset(&rule) {
                result["evidence_error"] = json!(format!("no se reinició el rodaje: {error}"));
            }
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
            return self.complete_with(task.id, result, done.version, receipt, None);
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
                    return self.complete_with(task.id, result, action.version, receipt, None);
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

    /// Archivar (D159): cierra un correo pendiente de revisión sin enviar nada. No cuenta como
    /// rechazo (no escribe la señal `rejected`, D129: no es una decisión sobre un borrador) y deja
    /// un evento del historial sin contenido, en la misma transacción que el cierre. Una tarea que
    /// ya intentó una acción no se archiva: se confirma o se descarta.
    pub fn archive(&mut self, task_id: i64) -> AppResult<Task> {
        let (task, mut result) = self.reviewable(task_id)?;
        if task.kind != EMAIL {
            return Err(AppError::Invalid("solo se archivan correos".into()));
        }
        if result.get("attempt").is_some() || !self.store.task_actions(task.id)?.is_empty() {
            return Err(AppError::Invalid(
                "la tarea ya intentó enviar una respuesta: confírmela o descártela".into(),
            ));
        }
        archived_result(&mut result);
        Ok(self.store.complete_noting(
            task_id,
            &result,
            "archivada",
            ("archive", "archived", &json!({})),
        )?)
    }

    /// Redactar respuesta (D159): crea una tarea nueva que solo redacta (no clasifica otra vez y
    /// conserva la categoría) a partir de un informativo, un rebote o un correo sin respuesta
    /// necesaria. La de origen, si estaba en revisión, se archiva en la misma transacción. Pedirla
    /// dos veces devuelve la misma tarea: un índice único impide el duplicado. La nueva siempre
    /// acaba en revisión.
    pub fn redraft(&mut self, task_id: i64) -> AppResult<Task> {
        let source = self.store.task(task_id)?;
        if source.kind != EMAIL {
            return Err(AppError::Invalid(
                "solo se redactan respuestas a correos".into(),
            ));
        }
        if let Some(existing) = self.store.redraft_of(task_id)? {
            return Ok(existing);
        }
        let result = source.result.clone().unwrap_or(Value::Null);
        // Una tarea cerrada porque la persona ya respondió desde su programa de correo (D178) no
        // admite otra respuesta redactada.
        if result["closed"]["reason"] == "replied_outside" {
            return Err(AppError::Invalid(
                "Este correo ya lo respondió usted desde su programa de correo: no se puede pedir otra respuesta."
                    .into(),
            ));
        }
        if result.get("redraft_of").is_some_and(|v| !v.is_null()) {
            return Err(AppError::Invalid(
                "esta tarea ya es una respuesta redactada a mano".into(),
            ));
        }
        let route = result["triage"]["route"].as_str();
        let no_answer = route == Some("rebote") || result["needs_reply"].as_bool() == Some(false);
        let eligible = match source.state {
            TaskState::Completed => {
                route == Some("informativo") || (result["archived"] == json!(true) && no_answer)
            }
            TaskState::AwaitingReview => no_answer,
            _ => false,
        };
        if !eligible {
            return Err(AppError::Invalid(
                "esta tarea no admite pedir una respuesta: solo un informativo, un rebote o un correo sin respuesta necesaria".into(),
            ));
        }
        let mut input: Value = serde_json::from_str(&source.input_ref).unwrap_or(Value::Null);
        let Some(fields) = input.as_object_mut() else {
            return Err(AppError::Invalid(
                "la entrada de la tarea no se puede leer".into(),
            ));
        };
        fields.remove("initial_read");
        fields.remove("read_source");
        fields.insert("redraft_of".into(), json!(task_id));
        if let Some(category) = result["category"].as_str() {
            fields.insert("category".into(), json!(category));
        }
        if fields.get("message").is_none_or(Value::is_null) {
            fields.insert("message".into(), result["message"].clone());
        }
        let archive = (source.state == TaskState::AwaitingReview).then(|| {
            let mut archived = result.clone();
            archived_result(&mut archived);
            archived
        });
        let (task, _) =
            self.store
                .enqueue_redraft(task_id, &input.to_string(), archive.as_ref())?;
        Ok(task)
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
            // Rechazar un envío de rodaje también reinicia el rodaje de su regla (D174).
            if let Some(rule) = rodaje_rule(&result) {
                if let Err(error) = self.store.rodaje_reset(&rule) {
                    let note = json!({ "kind": "rodaje", "error": error.to_string() });
                    let _ = self
                        .store
                        .record_event(task_id, "signal_error", "unsettled", &note);
                }
            }
        }
        Ok(self.store.cancel(task_id)?)
    }

    pub fn retry(&mut self, task_id: i64) -> AppResult<Task> {
        Ok(self.store.retry(task_id)?)
    }

    /// Las tareas que ve la persona. Las de la primera lectura (E5) no están: se ven aparte,
    /// con `initial_read_status`, para no ensuciar Pendientes, Tareas ni el recuento de trabajo.
    pub fn tasks(&self) -> AppResult<Vec<Task>> {
        Ok(self
            .store
            .all_tasks()?
            .into_iter()
            .filter(|t| !task_is_initial_read(t))
            .collect())
    }

    /// Deja en el historial un cambio de configuración, sin tarea (D186). El llamador decide si
    /// hubo un cambio real y garantiza que `detail` no lleva secretos ni contenido.
    pub fn record_setting(&mut self, action: &str, status: &str, detail: &Value) -> AppResult<()> {
        Ok(self.store.record_note(action, status, detail)?)
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
    /// Una regla activa solo sigue activa si lo nuevo no amplía lo anterior; si no, queda
    /// inactiva y la vista lo indica en `deactivated_by_edit` (D186c).
    pub fn update_rule(&mut self, id: &str, conditions: &Value) -> AppResult<panel::RuleView> {
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
        // Editar borra la validación avanzada: la versión nueva no la tiene.
        let (_, deactivated) =
            self.store
                .update_rule_guarded(id, conditions, &|old, old_ack| {
                    panel::edit_keeps_active(old, old_ack, conditions, false)
                })?;
        let mut view = self.rule_view(id)?;
        view.deactivated_by_edit = deactivated;
        Ok(view)
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
        let view = self.mail.add(provider, label, tokens)?;
        self.clear_removed_read(&view.id)?;
        Ok(view)
    }

    /// Guarda tokens nuevos para una cuenta existente.
    pub fn reconnect_mail_account(&mut self, id: &str, tokens: &Tokens) -> AppResult<AccountView> {
        let view = self.mail.reconnect(id, tokens)?;
        self.clear_removed_read(&view.id)?;
        Ok(view)
    }

    /// Cambia el nombre visible de una cuenta; vacío vuelve al predeterminado. Las tareas ya
    /// encoladas conservan el nombre con que llegaron; la ventana muestra el actual.
    pub fn rename_mail_account(&mut self, id: &str, label: &str) -> AppResult<AccountView> {
        Ok(self.mail.rename(id, label)?)
    }

    /// Vuelve a añadir la cuenta simulada si se había quitado.
    pub fn add_simulated_mail_account(&mut self) -> AppResult<AccountView> {
        let view = self.mail.add_simulator()?;
        self.clear_removed_read(&view.id)?;
        Ok(view)
    }

    /// Desconecta una cuenta: borra sus tokens y la quita de la lista. Las tareas ya
    /// encoladas de esa cuenta siguen visibles; enviar desde ellas falla con el motivo. Las
    /// tareas pendientes de su primera lectura se cancelan.
    pub fn remove_mail_account(&mut self, id: &str) -> AppResult<()> {
        // Se resuelve antes de quitarla (migra una marca guardada por identificador si ya consta la
        // dirección) y se buscan también las marcas que apunten a este identificador.
        let sources = self.account_read_sources(id)?;
        self.mail.remove(id)?;
        // Las tareas pendientes de su primera lectura no siguen enviando datos (D133); lo ya
        // leído sigue visible.
        for source in sources {
            // Una recogida de «aprender» en curso de ese buzón se descarta (solo está en memoria).
            self.learning.collecting.remove(&source);
            self.cancel_initial_read(&source)?;
        }
        Ok(())
    }
}

/// Regla aprendida cuyo rodaje mandó la tarea a revisión (D174): solo si la evaluación guardada
/// tiene el motivo `rodaje`. Cualquier otra revisión (un bloqueo, otra regla) no cuenta.
fn rodaje_rule(result: &Value) -> Option<String> {
    let evaluation = &result["evaluation"];
    (evaluation["code"] == "rodaje")
        .then(|| evaluation["facts"]["rule_id"].as_str().map(str::to_owned))
        .flatten()
}

/// Marca un resultado como archivado (D159): sin `action` ni intento de recuperación.
fn archived_result(result: &mut Value) {
    if let Some(fields) = result.as_object_mut() {
        fields.remove("action");
        fields.remove("recovery");
    }
    result["archived"] = json!(true);
}

/// Resultado de un correo que la ruta local resolvió sin IA (D157): `triage` dice por qué y
/// ninguna `action`. Un informativo no tiene borrador ni nada que revisar; un rebote queda en
/// revisión con `review_reason` y `needs_reply = false`, para que la ventana lo muestre como
/// posible fallo de entrega. Solo códigos de señal, nunca cabeceras.
fn triaged_result(
    message: &Message,
    origin: Value,
    decision: &TriageDecision,
    preference_scope: Option<nestadmin_core::PreferenceScope>,
    signals: &[Signal],
    review: bool,
) -> Value {
    let mut triage = json!({
        "route": decision.route.as_str(),
        "codes": nestadmin_core::mail_signals::codes(signals),
        "source": decision.source.as_str(),
    });
    if decision.already_replied {
        triage["already_replied"] = json!(true);
    }
    if let Some(scope) = preference_scope {
        triage["preference_scope"] = json!(scope.as_str());
    }
    let mut result = json!({
        "type": EMAIL,
        "version": 1,
        "origin": origin,
        "message": message,
        "triage": triage,
    });
    if review {
        result["needs_reply"] = json!(false);
        result["review_reason"] = json!(BOUNCE_REASON);
    }
    result
}

/// Un correo resuelto por la ruta local (informativo o rebote) o una redacción pedida a mano:
/// no cuentan como decisiones sobre borradores de la IA para la simulación, las sugerencias ni
/// las estadísticas (D159).
pub(crate) fn task_is_triaged_or_redraft(task: &Task) -> bool {
    let Some(result) = task.result.as_ref() else {
        return serde_json::from_str::<Value>(&task.input_ref)
            .is_ok_and(|input| input.get("redraft_of").is_some());
    };
    matches!(
        result["triage"]["route"].as_str(),
        Some("informativo" | "rebote")
    ) || result.get("redraft_of").is_some_and(|v| !v.is_null())
}

fn action_kind(task: &Task) -> &'static str {
    if task.kind == INVOICE {
        REGISTER_INVOICE
    } else {
        SEND_DRAFT
    }
}

/// Valida las condiciones de una regla (D123, D126, D127). `allow_no_sender`: la validación
/// avanzada permite una regla de envío sin remitente. Es `pub` (oculta) solo para que las pruebas de
/// la aceptación en lote (D173) puedan envolverla.
#[doc(hidden)]
pub fn check_rule(action_kind: &str, conditions: &Value, allow_no_sender: bool) -> AppResult<()> {
    check_rule_inner(action_kind, conditions, allow_no_sender, false)
}

/// Como `check_rule`. `allow_review_only` deja pasar una categoría de solo revisión (D177): solo
/// la simulación de una regla antigua que ya la tiene, para poder enseñar que todo iría a revisión.
pub(crate) fn check_rule_inner(
    action_kind: &str,
    conditions: &Value,
    allow_no_sender: bool,
    allow_review_only: bool,
) -> AppResult<()> {
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
        if !allow_review_only
            && nestadmin_ai::tasks::REVIEW_ONLY_CATEGORIES.contains(&text("category").unwrap_or(""))
        {
            return Err(AppError::Invalid(crate::rules::REVIEW_ONLY_MESSAGE.into()));
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

/// Un adjunto cuyo tamaño ya dice que nunca podrá importarse: sobre el máximo o vacío. Lo usan la
/// importación y los pasos de pipeline, de modo que no divergen; el contenido bajado se comprueba
/// además con `check_document`.
fn attachment_rejection(size: Option<u64>) -> Option<String> {
    match size {
        Some(s) if s > MAX_DOCUMENT_BYTES as u64 => Some(format!(
            "supera el tamaño máximo de {} MB",
            MAX_DOCUMENT_BYTES / 1_000_000
        )),
        Some(0) => Some("está vacío".to_owned()),
        _ => None,
    }
}

/// Clave de un adjunto: el mensaje, su nombre y su tamaño, que no cambian entre consultas (el
/// identificador de Gmail sí puede cambiar). La usan la importación y los pasos de pipeline.
pub(crate) fn attachment_key(message_id: &str, name: &str, size: Option<u64>) -> String {
    format!(
        "{message_id}#{name}#{}",
        size.map(|s| s.to_string()).unwrap_or_default()
    )
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
                wait: None,
                used: Arc::new(Silent),
                fallback: None,
                destination: None,
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
