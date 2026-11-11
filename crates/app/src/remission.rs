//! Remisión de los registros Verifactu al simulador local de la AEAT (etapa E5, encargo B,
//! D247-D260): entorno vigente, simulador en proceso, transporte SOAP, «bomba» de remisión con
//! su tarea `submit_verifactu`, recuperación al abrir y estado para la interfaz.
//!
//! La remisión es la obligación de la modalidad VERI*FACTU, no una acción discrecional (D250,
//! excepción de R07). Garantías, escritas y probadas en `tests/sales_remission.rs`:
//!
//! * Solo se envía el `record_json` guardado: este módulo no genera ni cambia ningún registro
//!   (la subsanación, que la pide el usuario, crea uno nuevo en core).
//! * Nunca bloquea ni devuelve error a la emisión: la bomba corre después de la confirmación y
//!   sus fallos solo se anotan en el historial.
//! * La tarea `submit_verifactu` tiene camino propio: no lleva `action`, no pasa por `blockers`
//!   ni por las reglas de envío, y `begin_next` no la toma (la ejecuta la bomba).
//! * El historial deja la autorización `obligacion_verifactu <entorno>` al abrir y cerrar cada
//!   lote (lo escribe core, `verifactu_remission`).
//!
//! Un lote sin respuesta nunca se cierra (D252): un fallo de red, un 5xx, una respuesta ilegible
//! o un tiempo agotado dejan el lote abierto y la siguiente pasada lo reenvía idéntico. Solo una
//! respuesta entendida o un `Fault` SOAP lo cierran.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use nestadmin_core::sales::{
    BatchClosure, BatchOutcome, RecordEnvironment, SalesStoreError, VerifactuBatch,
    VerifactuBatchSummary, VerifactuEnvironment, VerifactuKind, VerifactuRecordState,
    VerifactuStatus, VerifactuSubsanation, CODE_BATCH_OPEN, CODE_BATCH_REJECTED,
    CODE_ENVIRONMENT_BUSY, CODE_RECORD_INVALID, MAX_AUTOMATIC_FAULTS,
};
use nestadmin_core::{Task, TaskState};
use nestadmin_verifactu::simulador::{ConfigSimulador, ErrorForzado, SimuladorAeat};
use nestadmin_verifactu::xml::{Cabecera, PersonaEs};
use nestadmin_verifactu::{FalloSoap, RegistroFacturacion, ResultadoEnvio};
use serde::Serialize;
use serde_json::{json, Value};

use crate::sales::clock_error;
use crate::{task_error, App, AppError, AppResult};

/// Tipo de las tareas de remisión (visibles en Tareas, D250).
pub const SUBMIT_VERIFACTU: &str = "submit_verifactu";
/// Elegir `preproduccion` o `produccion` (D247): llegan con T18b.
pub const CODE_ENVIRONMENT_UNAVAILABLE: &str = "environment_unavailable";
/// Códigos estables de los errores del transporte (D255). Ninguno cierra el lote.
pub const TRANSPORT_UNAVAILABLE: &str = "unavailable";
pub const TRANSPORT_INVALID_RESPONSE: &str = "invalid_response";
pub const TRANSPORT_RATE_LIMITED: &str = "rate_limited";
pub const TRANSPORT_UNAUTHORIZED: &str = "unauthorized";
/// Código de la tarea cuyo lote se cerró con un `Fault` SOAP (D259).
pub const CODE_SOAP_FAULT: &str = "verifactu_fault";
/// Variable que fuerza respuestas del simulador (solo compilaciones de depuración y entorno
/// `simulador`, D254).
pub const SIM_VARIABLE: &str = "NESTADMIN_VERIFACTU_SIM";

const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);

// ---------------------------------------------------------------------------------------------
// Transporte (D255)
// ---------------------------------------------------------------------------------------------

/// Error del transporte: nada de esto cierra el lote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransportError {
    /// `unavailable`, `invalid_response`, `rate_limited` o `unauthorized`.
    pub code: &'static str,
    /// Texto sin datos del certificado ni del contenido enviado.
    pub message: String,
}

fn transport_error(code: &'static str, message: impl Into<String>) -> TransportError {
    TransportError {
        code,
        message: message.into(),
    }
}

/// `POST` del sobre SOAP y clasificación de lo que vuelva: una respuesta entendida o un `Fault`
/// SOAP son resultado; el resto es un error del transporte (el lote sigue abierto).
fn post_soap(url: &str, body: &str) -> Result<ResultadoEnvio, TransportError> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(REQUEST_TIMEOUT))
        .timeout_connect(Some(CONNECT_TIMEOUT))
        .http_status_as_error(false)
        .proxy(None)
        .build()
        .into();
    let mut response = agent
        .post(url)
        .header("Content-Type", "text/xml; charset=utf-8")
        .header("SOAPAction", "")
        .send(body)
        .map_err(|e| transport_error(TRANSPORT_UNAVAILABLE, format!("sin conexión: {e}")))?;
    let status = response.status().as_u16();
    let text = response
        .body_mut()
        .read_to_string()
        .map_err(|e| transport_error(TRANSPORT_UNAVAILABLE, format!("respuesta cortada: {e}")))?;
    classify_response(status, &text)
}

fn classify_response(status: u16, text: &str) -> Result<ResultadoEnvio, TransportError> {
    match status {
        200..=299 => ResultadoEnvio::desde_xml(text).map_err(|e| {
            transport_error(
                TRANSPORT_INVALID_RESPONSE,
                format!("la respuesta no se entiende: {e}"),
            )
        }),
        401 | 403 => Err(transport_error(
            TRANSPORT_UNAUTHORIZED,
            format!("el servicio no autoriza el envío (HTTP {status})"),
        )),
        429 => Err(transport_error(
            TRANSPORT_RATE_LIMITED,
            "el servicio pide esperar antes de otro envío (HTTP 429)",
        )),
        // Un `Fault` SOAP viaja con HTTP 500: es un resultado (cierra el lote). Un 5xx sin
        // `Fault` entendible es no disponibilidad y el lote sigue abierto (ajuste 4).
        500..=599 => match ResultadoEnvio::desde_xml(text) {
            Ok(fault @ ResultadoEnvio::Fallo(_)) => Ok(fault),
            _ => Err(transport_error(
                TRANSPORT_UNAVAILABLE,
                format!("servicio no disponible (HTTP {status})"),
            )),
        },
        _ => Err(transport_error(
            TRANSPORT_INVALID_RESPONSE,
            format!("respuesta inesperada del servicio (HTTP {status})"),
        )),
    }
}

// ---------------------------------------------------------------------------------------------
// Modos que fuerzan respuestas del simulador (solo depuración, D254, ajuste 12)
// ---------------------------------------------------------------------------------------------

/// Respuesta que se fuerza al simulador en el siguiente envío. Solo existe en compilación de
/// depuración y con el entorno `simulador`; en otro caso se ignora.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SimMode {
    /// El siguiente envío se rechaza (el primer registro del lote, con un error forzado).
    RejectNext,
    /// Sin red: el envío falla sin llegar a conectar (modo persistente hasta quitarlo).
    NoNetwork,
    /// El simulador procesa el siguiente envío pero cierra la conexión sin responder.
    LoseNextResponse,
    /// El siguiente envío recibe un `Fault` SOAP de cabecera (4104).
    SchemaFault,
    /// El primer registro del siguiente envío se acepta con errores (2000).
    AcceptWithErrors,
}

impl SimMode {
    /// Valor de `NESTADMIN_VERIFACTU_SIM`: `rechazar-siguiente`, `sin-red`, `perder-respuesta`,
    /// `fault-esquema` o `aceptado-con-errores`.
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim() {
            "rechazar-siguiente" => Some(Self::RejectNext),
            "sin-red" => Some(Self::NoNetwork),
            "perder-respuesta" => Some(Self::LoseNextResponse),
            "fault-esquema" => Some(Self::SchemaFault),
            "aceptado-con-errores" => Some(Self::AcceptWithErrors),
            _ => None,
        }
    }
}

/// Lo que hay que deshacer en el simulador tras la llamada.
#[cfg_attr(not(debug_assertions), allow(dead_code))]
enum Cleanup {
    ForcedError(String),
    Censo(Option<Vec<String>>),
}

/// Estado en memoria de la remisión (no se guarda nada de esto).
pub(crate) struct RemissionRuntime {
    /// Exclusión entre el tick y «Remitir ahora»: `true` mientras una pasada está en curso.
    busy: Arc<Mutex<bool>>,
    /// Una emisión, anulación o subsanación pidió remitir: lo atiende el worker (D250).
    requested: Arc<std::sync::atomic::AtomicBool>,
    simulator: Option<Arc<SimuladorAeat>>,
    #[cfg(debug_assertions)]
    sim_mode: Option<SimMode>,
    /// Servidor de pruebas que sustituye al simulador (solo depuración).
    #[cfg(debug_assertions)]
    endpoint: Option<String>,
}

impl RemissionRuntime {
    pub(crate) fn from_env() -> Self {
        Self {
            busy: Arc::new(Mutex::new(false)),
            requested: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            simulator: None,
            #[cfg(debug_assertions)]
            sim_mode: std::env::var(SIM_VARIABLE)
                .ok()
                .and_then(|v| SimMode::parse(&v)),
            #[cfg(debug_assertions)]
            endpoint: None,
        }
    }
}

/// Concesión de la exclusión de la bomba: se libera al soltarla (también si el trabajo se
/// abandona o falla).
struct PumpLease(Arc<Mutex<bool>>);

impl PumpLease {
    fn acquire(busy: &Arc<Mutex<bool>>) -> Option<Self> {
        let mut flag = busy.lock().unwrap_or_else(|e| e.into_inner());
        if *flag {
            return None;
        }
        *flag = true;
        Some(Self(Arc::clone(busy)))
    }
}

impl Drop for PumpLease {
    fn drop(&mut self) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = false;
    }
}

// ---------------------------------------------------------------------------------------------
// Tipos que ve la interfaz (encargo C)
// ---------------------------------------------------------------------------------------------

/// Resultado de una pasada de la bomba.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum PumpReport {
    /// Nada pendiente ni lote abierto.
    Idle,
    /// Otra pasada (el tick o «Remitir ahora») está en curso.
    Busy,
    /// Hay trabajo pero no ha pasado la espera (D251).
    Waiting {
        /// Segundos que faltan según el reloj de la aplicación.
        seconds: u64,
        /// Instante permitido (milisegundos Unix).
        until: i64,
    },
    /// Tres `Fault` seguidos: la remisión automática está parada hasta «Remitir ahora» (D259).
    Halted,
    /// El entorno vigente no tiene transporte (solo `simulador` en E5, D247).
    EnvironmentUnavailable { environment: String },
    /// La AEAT respondió y el lote se cerró.
    Sent {
        batch_id: i64,
        accepted: u32,
        accepted_with_errors: u32,
        rejected: u32,
        unexpected_duplicates: u32,
        /// Registros sin línea en la respuesta: siguen pendientes.
        missing: u32,
        wait_seconds: u32,
    },
    /// `Fault` SOAP: el lote se cerró y sus registros vuelven a pendientes (D259).
    Fault {
        batch_id: i64,
        code: Option<u32>,
        description: String,
    },
    /// Sin respuesta (red, 5xx, respuesta ilegible): el lote sigue abierto y se reenviará.
    NoResponse {
        batch_id: i64,
        code: String,
        message: String,
    },
}

/// Un registro Verifactu con su estado derivado, para mostrarlo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VerifactuRecordView {
    pub record_id: i64,
    pub document_id: i64,
    /// `alta` o `anulacion`.
    pub kind: String,
    pub invoice_number: String,
    /// `DD-MM-AAAA`.
    pub issue_date: String,
    /// `pending`, `sending`, `accepted`, `accepted_with_errors`, `rejected`, `subsanated` o
    /// `unexpected_duplicate`.
    pub status: String,
    /// Código y descripción de la AEAT del último resultado que explica el estado.
    pub aeat_code: Option<u32>,
    pub aeat_description: Option<String>,
    /// Aceptado porque la AEAT ya lo tenía de un intento anterior de nuestro lote.
    pub duplicate: bool,
    pub batch_id: Option<i64>,
    /// Registro que lo subsana (`subsanated`).
    pub subsanation_record_id: Option<i64>,
    /// Entorno guardado con el registro (`simulador`, `produccion_heredado`, ...).
    pub environment: String,
}

/// Resumen del último lote.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VerifactuBatchView {
    pub id: i64,
    pub environment: String,
    pub record_count: u32,
    pub started_at: i64,
    pub attempts: u32,
    /// `None` mientras el lote está abierto (sin respuesta).
    pub outcome: Option<String>,
    pub send_state: Option<String>,
    pub error_code: Option<u32>,
    pub error_description: Option<String>,
    pub wait_seconds: Option<u32>,
    pub csv: Option<String>,
    pub finished_at: Option<i64>,
}

/// Estado de la remisión para la ventana (D250-D252, D256, D259).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VerifactuStatusView {
    /// Entorno vigente: `simulador`, `preproduccion` o `produccion`.
    pub environment: String,
    /// Entornos que se pueden elegir hoy (solo `simulador`, D247).
    pub selectable_environments: Vec<String>,
    /// Aviso fijo mientras el entorno es el simulador (D256).
    pub environment_notice: Option<String>,
    pub records: Vec<VerifactuRecordView>,
    pub last_batch: Option<VerifactuBatchView>,
    /// Instante a partir del cual se puede enviar el siguiente lote (milisegundos Unix).
    pub next_send_at: Option<i64>,
    /// Segundos que faltan para ese instante según el reloj de la aplicación (0 si ya pasó).
    pub wait_remaining_seconds: Option<u64>,
    /// La remisión automática está parada por `Fault` seguidos (D259).
    pub halted: bool,
    pub consecutive_faults: u32,
    /// Registros pendientes de remitir del entorno vigente.
    pub pending: u32,
    /// Registros en un lote abierto.
    pub sending: u32,
    /// Registros rechazados sin subsanar.
    pub rejected: u32,
    /// Duplicados que la AEAT tenía y no eran nuestros (visibles, no aceptados).
    pub unexpected_duplicates: u32,
}

// ---------------------------------------------------------------------------------------------
// Trabajo de una pasada (se parte para no tener el cerrojo de la aplicación durante la red)
// ---------------------------------------------------------------------------------------------

/// Trabajo de red de una pasada ya preparado: se ejecuta sin tocar la aplicación.
pub struct VerifactuJob {
    lease: PumpLease,
    task_id: i64,
    batch_id: i64,
    url: String,
    body: String,
    started_ms: i64,
    no_network: bool,
    simulator: Option<Arc<SimuladorAeat>>,
    cleanup: Vec<Cleanup>,
}

/// Lo que devolvió la llamada, listo para guardarse con [`App::verifactu_pump_finish`].
pub struct VerifactuSent {
    lease: PumpLease,
    task_id: i64,
    batch_id: i64,
    started_ms: i64,
    outcome: Result<ResultadoEnvio, TransportError>,
}

impl VerifactuJob {
    /// Hace la llamada (sin la aplicación) y deshace lo que se forzó en el simulador.
    pub fn run(self) -> VerifactuSent {
        let outcome = if self.no_network {
            Err(transport_error(
                TRANSPORT_UNAVAILABLE,
                "sin red (simulado)".to_owned(),
            ))
        } else {
            post_soap(&self.url, &self.body)
        };
        if let Some(sim) = &self.simulator {
            for undo in &self.cleanup {
                match undo {
                    Cleanup::ForcedError(serie) => {
                        sim.estado().config.errores.remove(serie);
                    }
                    Cleanup::Censo(previous) => {
                        sim.estado().config.censo = previous.clone();
                    }
                }
            }
        }
        VerifactuSent {
            lease: self.lease,
            task_id: self.task_id,
            batch_id: self.batch_id,
            started_ms: self.started_ms,
            outcome,
        }
    }
}

/// Resultado de preparar una pasada.
pub enum PumpStart {
    /// No hay nada que enviar ahora (y por qué).
    Idle(PumpReport),
    /// Hay una llamada que hacer.
    Job(VerifactuJob),
}

fn kind_text(kind: VerifactuKind) -> &'static str {
    match kind {
        VerifactuKind::Alta => "alta",
        VerifactuKind::Anulacion => "anulacion",
    }
}

fn sales_error(code: &str, message: impl Into<String>) -> AppError {
    AppError::Sales(SalesStoreError::External {
        code: code.to_owned(),
        message: message.into(),
    })
}

fn batch_view(summary: &VerifactuBatchSummary) -> VerifactuBatchView {
    let result = summary.result.as_ref();
    VerifactuBatchView {
        id: summary.id,
        environment: summary.environment.as_str().to_owned(),
        record_count: summary.record_count,
        started_at: summary.started_at,
        attempts: summary.attempts,
        outcome: result.map(|r| match r.outcome {
            BatchOutcome::Responded => "responded".to_owned(),
            BatchOutcome::Fault => "fault".to_owned(),
        }),
        send_state: result.and_then(|r| r.send_state.clone()),
        error_code: result.and_then(|r| r.error_code),
        error_description: result.and_then(|r| r.error_description.clone()),
        wait_seconds: result.map(|r| r.wait_seconds),
        csv: result.and_then(|r| r.csv.clone()),
        finished_at: result.map(|r| r.finished_at),
    }
}

impl App {
    // --- Entorno (D247, D248, D256) ---

    /// Entorno de remisión vigente; sin cambios guardados, `simulador`.
    pub fn verifactu_environment(&self) -> AppResult<VerifactuEnvironment> {
        Ok(self.store.verifactu_environment()?)
    }

    /// Entorno guardado con un registro (el del QR, D248). Una fila ausente es un error.
    pub fn verifactu_record_environment(&self, record_id: i64) -> AppResult<RecordEnvironment> {
        Ok(self.store.verifactu_record_environment(record_id)?)
    }

    /// Cambia el entorno vigente y dice si cambió. En E5 solo `simulador` se puede elegir:
    /// `preproduccion` y `produccion` se rechazan con `environment_unavailable` (D247), o con
    /// `verifactu_environment_busy` si además hay registros por remitir (D256: la razón más
    /// útil, y nada cambia en ninguno de los dos casos). Elegir el entorno vigente no hace nada.
    pub fn set_verifactu_environment(
        &mut self,
        environment: VerifactuEnvironment,
        authorization: &str,
    ) -> AppResult<bool> {
        if environment != VerifactuEnvironment::Simulador {
            let status = self.verifactu_status()?;
            let busy = status.pending + status.sending + status.rejected;
            if busy > 0 {
                return Err(sales_error(
                    CODE_ENVIRONMENT_BUSY,
                    format!(
                        "no se cambia de entorno con registros por remitir ({} pendientes, {} \
                         en envío, {} rechazados sin subsanar)",
                        status.pending, status.sending, status.rejected
                    ),
                ));
            }
            return Err(sales_error(
                CODE_ENVIRONMENT_UNAVAILABLE,
                format!(
                    "el entorno «{}» no está disponible todavía: la remisión real a la AEAT \
                     (certificado y declaración responsable) llega más adelante; hoy solo \
                     funciona el simulador",
                    environment.as_str()
                ),
            ));
        }
        Ok(self
            .store
            .set_verifactu_environment(environment, authorization)?)
    }

    // --- Simulador en proceso (D254) ---

    /// Arranca el simulador la primera vez que se usa (127.0.0.1, puerto libre) y lo siembra con
    /// los registros aceptados guardados, para no dar falsos rechazos de anulación tras reabrir.
    fn ensure_simulator(&mut self) -> AppResult<Arc<SimuladorAeat>> {
        if let Some(sim) = &self.remission.simulator {
            return Ok(Arc::clone(sim));
        }
        let accepted = self
            .store
            .verifactu_accepted_records(VerifactuEnvironment::Simulador)?;
        let sim = SimuladorAeat::iniciar(ConfigSimulador::default())?;
        sim.sembrar(&accepted);
        let sim = Arc::new(sim);
        self.remission.simulator = Some(Arc::clone(&sim));
        Ok(sim)
    }

    /// El simulador en proceso, para preparar y comprobar escenarios en las pruebas.
    #[cfg(debug_assertions)]
    pub fn verifactu_simulator(&mut self) -> AppResult<Arc<SimuladorAeat>> {
        if self.store.verifactu_environment()? != VerifactuEnvironment::Simulador {
            return Err(sales_error(
                CODE_ENVIRONMENT_UNAVAILABLE,
                "el simulador solo existe en el entorno simulador",
            ));
        }
        self.ensure_simulator()
    }

    /// Fuerza la respuesta del simulador en el siguiente envío (pruebas y `tauri dev`).
    #[cfg(debug_assertions)]
    pub fn set_verifactu_sim_mode(&mut self, mode: Option<SimMode>) {
        self.remission.sim_mode = mode;
    }

    /// Apunta el envío a otro servidor (uno de pruebas que contesta lo que el simulador no
    /// sabe: una respuesta parcial, un 429, un cuerpo ilegible); `None` vuelve al simulador.
    /// Solo existe en compilación de depuración.
    #[cfg(debug_assertions)]
    pub fn set_verifactu_endpoint(&mut self, url: Option<String>) {
        self.remission.endpoint = url;
    }

    /// Prepara lo que hay que forzar en el simulador para este envío y qué deshacer después.
    /// Devuelve `(sin_red, deshacer)`.
    #[cfg(debug_assertions)]
    fn arm_simulator(
        &mut self,
        sim: &SimuladorAeat,
        batch: &VerifactuBatch,
    ) -> (bool, Vec<Cleanup>) {
        let first_serie = batch
            .envio
            .registros
            .first()
            .map(|r| r.id_factura().num_serie.clone());
        let mut undo = Vec::new();
        let mut no_network = false;
        match self.remission.sim_mode {
            None => {}
            Some(SimMode::NoNetwork) => no_network = true,
            Some(mode) => {
                // Los demás modos valen para un solo envío.
                self.remission.sim_mode = None;
                match mode {
                    SimMode::RejectNext | SimMode::AcceptWithErrors => {
                        if let Some(serie) = first_serie {
                            let accepted = mode == SimMode::AcceptWithErrors;
                            sim.estado().config.errores.insert(
                                serie.clone(),
                                ErrorForzado {
                                    codigo: if accepted { 2000 } else { 1100 },
                                    descripcion: if accepted {
                                        "Aceptado con errores (simulado)".to_owned()
                                    } else {
                                        "Rechazo simulado".to_owned()
                                    },
                                    aceptado: accepted,
                                },
                            );
                            undo.push(Cleanup::ForcedError(serie));
                        }
                    }
                    SimMode::LoseNextResponse => {
                        sim.estado().perder_siguiente_respuesta = true;
                    }
                    SimMode::SchemaFault => {
                        let mut state = sim.estado();
                        undo.push(Cleanup::Censo(state.config.censo.clone()));
                        state.config.censo = Some(Vec::new());
                    }
                    SimMode::NoNetwork => {}
                }
            }
        }
        (no_network, undo)
    }

    #[cfg(not(debug_assertions))]
    fn arm_simulator(
        &mut self,
        _sim: &SimuladorAeat,
        _batch: &VerifactuBatch,
    ) -> (bool, Vec<Cleanup>) {
        (false, Vec::new())
    }

    // --- Reloj ---

    /// Milisegundos Unix según el reloj de la aplicación (el inyectable de las pruebas).
    fn remission_now_ms(&self) -> AppResult<i64> {
        let now = self.sales_clock.try_now().map_err(clock_error)?;
        Ok(now.unix().saturating_mul(1000))
    }

    // --- Bomba ---

    /// Una pasada completa de la remisión: reenvía de forma idéntica un lote abierto sin
    /// resultado o, si hay pendientes y ya pasó la espera, abre uno nuevo y lo envía. Con
    /// `manual` (el botón «Remitir ahora») se salta el paro por `Fault` seguidos, pero no la
    /// espera (D251). Se llama tras emitir y anular, en el tick periódico y con el botón.
    pub fn verifactu_pump(&mut self, manual: bool) -> AppResult<PumpReport> {
        match self.verifactu_pump_begin(manual)? {
            PumpStart::Idle(report) => Ok(report),
            PumpStart::Job(job) => self.verifactu_pump_finish(job.run()),
        }
    }

    /// Tras confirmar una emisión, anulación o subsanación solo se deja una petición de
    /// remisión (un indicador atómico) y se vuelve: no hay red ni espera en el camino de la
    /// emisión (D250, garantía 2). La remisión la hace el worker en su tick, o quien llame a
    /// `verifactu_pump` de forma explícita.
    pub(crate) fn pump_after_write(&mut self) {
        self.remission
            .requested
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    /// Si una emisión, anulación o subsanación pidió remitir desde la última vez; lo borra. El
    /// worker lo consulta para despertar ya en lugar de esperar al siguiente tick.
    pub fn verifactu_pump_requested(&self) -> bool {
        self.remission
            .requested
            .swap(false, std::sync::atomic::Ordering::SeqCst)
    }

    /// Primera mitad de la pasada: decide, abre o recupera el lote, crea la tarea y anota el
    /// intento. No hace ninguna llamada de red; devuelve el trabajo para hacerla sin tener la
    /// aplicación (el escritorio suelta su cerrojo durante la llamada).
    pub fn verifactu_pump_begin(&mut self, manual: bool) -> AppResult<PumpStart> {
        let Some(lease) = PumpLease::acquire(&self.remission.busy) else {
            return Ok(PumpStart::Idle(PumpReport::Busy));
        };
        let environment = self.store.verifactu_environment()?;
        if environment != VerifactuEnvironment::Simulador {
            return Ok(PumpStart::Idle(PumpReport::EnvironmentUnavailable {
                environment: environment.as_str().to_owned(),
            }));
        }
        let now = self.remission_now_ms()?;
        self.settle_submit_tasks();
        let open = self.store.unfinished_verifactu_batch()?;
        let faults = self.store.verifactu_consecutive_faults(environment)?;
        if !manual && faults >= MAX_AUTOMATIC_FAULTS {
            return Ok(PumpStart::Idle(PumpReport::Halted));
        }
        let has_pending = open.is_some()
            || !self
                .store
                .verifactu_pending_records(environment)?
                .is_empty();
        if !has_pending {
            return Ok(PumpStart::Idle(PumpReport::Idle));
        }
        if let Some(until) = self.store.verifactu_next_send_at(environment)? {
            if now < until {
                let seconds = u64::try_from((until - now + 999) / 1000).unwrap_or(0);
                return Ok(PumpStart::Idle(PumpReport::Waiting { seconds, until }));
            }
        }
        let batch = match open {
            Some(batch) => batch,
            None => {
                let header = self.remission_header(environment)?;
                match self
                    .store
                    .open_verifactu_batch(environment, &header, now, manual)
                {
                    Ok(Some(batch)) => batch,
                    Ok(None) => return Ok(PumpStart::Idle(PumpReport::Idle)),
                    Err(e) if e.code() == CODE_BATCH_REJECTED => {
                        return Ok(PumpStart::Idle(PumpReport::Halted))
                    }
                    // Otra conexión abrió uno entre medias: esa pasada lo envía.
                    Err(e) if e.code() == CODE_BATCH_OPEN => {
                        return Ok(PumpStart::Idle(PumpReport::Busy))
                    }
                    Err(e) => return Err(e.into()),
                }
            }
        };
        let body = batch
            .envio
            .a_soap()
            .map_err(|e| sales_error(CODE_RECORD_INVALID, format!("el lote no es válido: {e}")))?;
        let simulator = self.ensure_simulator()?;
        let task = self.claim_submit_task(&batch)?;
        // El intento se anota ANTES de la llamada (D252).
        if let Err(error) = self.store.begin_verifactu_attempt(batch.id, now) {
            let _ = self.store.fail(
                task.id,
                &task_error(error.code(), "no se pudo anotar el intento", true),
            );
            return Err(error.into());
        }
        #[cfg(debug_assertions)]
        let overridden = self.remission.endpoint.clone();
        #[cfg(not(debug_assertions))]
        let overridden: Option<String> = None;
        let (no_network, cleanup) = match overridden {
            Some(_) => (false, Vec::new()),
            None => self.arm_simulator(&simulator, &batch),
        };
        let url = overridden.unwrap_or_else(|| simulator.url());
        Ok(PumpStart::Job(VerifactuJob {
            lease,
            task_id: task.id,
            batch_id: batch.id,
            url,
            body,
            started_ms: now,
            no_network,
            simulator: Some(simulator),
            cleanup,
        }))
    }

    /// Segunda mitad: guarda lo que devolvió la llamada. Con respuesta o `Fault` cierra el lote
    /// (una sola vez, D252) y completa o falla la tarea; sin respuesta deja el lote abierto y la
    /// tarea fallida y reintentable, con el motivo visible.
    pub fn verifactu_pump_finish(&mut self, sent: VerifactuSent) -> AppResult<PumpReport> {
        let VerifactuSent {
            lease: _lease,
            task_id,
            batch_id,
            started_ms,
            outcome,
        } = sent;
        // Sin hora fiable se usa la del inicio de la pasada: no se inventa otra.
        let now = self.remission_now_ms().unwrap_or(started_ms);
        match outcome {
            Err(error) => {
                let note = json!({ "batch_id": batch_id, "code": error.code });
                let _ = self
                    .store
                    .record_note("verifactu.transport", "error", &note);
                let _ = self.store.fail(
                    task_id,
                    &task_error(error.code, error.message.clone(), true),
                );
                Ok(PumpReport::NoResponse {
                    batch_id,
                    code: error.code.to_owned(),
                    message: error.message,
                })
            }
            Ok(result) => {
                let closure = match self.store.close_verifactu_batch(batch_id, &result, now) {
                    Ok(closure) => closure,
                    Err(error) => {
                        let _ = self.store.fail(
                            task_id,
                            &task_error(error.code(), "no se pudo guardar el resultado", true),
                        );
                        return Err(error.into());
                    }
                };
                self.finish_submit_task(task_id, &closure, &result)
            }
        }
    }

    fn finish_submit_task(
        &mut self,
        task_id: i64,
        closure: &BatchClosure,
        result: &ResultadoEnvio,
    ) -> AppResult<PumpReport> {
        match (closure.outcome, result) {
            (BatchOutcome::Fault, ResultadoEnvio::Fallo(FalloSoap { faultstring, .. })) => {
                let description = faultstring.clone();
                self.store.fail(
                    task_id,
                    &task_error(CODE_SOAP_FAULT, description.clone(), false),
                )?;
                Ok(PumpReport::Fault {
                    batch_id: closure.batch_id,
                    code: closure.fault_code,
                    description,
                })
            }
            _ => {
                let summary = json!({
                    "type": SUBMIT_VERIFACTU,
                    "version": 1,
                    "batch_id": closure.batch_id,
                    "accepted": closure.accepted,
                    "accepted_with_errors": closure.accepted_with_errors,
                    "rejected": closure.rejected,
                    "unexpected_duplicates": closure.unexpected_duplicates,
                    "missing_record_ids": closure.missing_record_ids,
                    "unmatched_lines": closure.unmatched_lines,
                    "wait_seconds": closure.wait_seconds,
                    "csv": closure.csv,
                });
                self.store.complete(task_id, &summary)?;
                Ok(PumpReport::Sent {
                    batch_id: closure.batch_id,
                    accepted: closure.accepted,
                    accepted_with_errors: closure.accepted_with_errors,
                    rejected: closure.rejected,
                    unexpected_duplicates: closure.unexpected_duplicates,
                    missing: u32::try_from(closure.missing_record_ids.len()).unwrap_or(u32::MAX),
                    wait_seconds: closure.wait_seconds,
                })
            }
        }
    }

    /// Cabecera del envío: el obligado a la emisión es el emisor de los registros pendientes
    /// (NIF) con su razón social. Falla cerrado si faltan datos o si hay emisores distintos.
    fn remission_header(&self, environment: VerifactuEnvironment) -> AppResult<Cabecera> {
        let pending = self.store.verifactu_pending_records(environment)?;
        let first = pending
            .first()
            .ok_or_else(|| sales_error(CODE_RECORD_INVALID, "no hay registros pendientes"))?;
        let nif = first.issuer_tax_id.clone();
        if pending.iter().any(|r| r.issuer_tax_id != nif) {
            return Err(sales_error(
                CODE_RECORD_INVALID,
                "hay registros pendientes de más de un emisor: no se remiten juntos",
            ));
        }
        // La razón social consta en cualquier alta del emisor; sin ella, la de la empresa si su
        // NIF es el mismo. Sin ninguna, no se inventa.
        let name = self
            .store
            .verifactu_records()?
            .into_iter()
            .find_map(|r| match r.record {
                RegistroFacturacion::Alta(a) if a.id_factura.nif_emisor == nif => {
                    Some(a.nombre_razon_emisor)
                }
                _ => None,
            })
            .or_else(|| {
                self.store
                    .sales_company()
                    .ok()
                    .flatten()
                    .filter(|c| c.tax_id.as_deref() == Some(nif.as_str()))
                    .and_then(|c| c.name)
            })
            .ok_or_else(|| {
                sales_error(
                    CODE_RECORD_INVALID,
                    "no consta la razón social del emisor para la cabecera del envío",
                )
            })?;
        Ok(Cabecera::new(PersonaEs::new(&name, &nif)))
    }

    // --- Tarea `submit_verifactu` ---

    fn submit_tasks_of(&self, batch_id: i64, states: &[TaskState]) -> AppResult<Vec<Task>> {
        let mut found = Vec::new();
        for state in states {
            for task in self.store.tasks_in(*state)? {
                if task.kind == SUBMIT_VERIFACTU
                    && task_batch_id(&task).is_some_and(|id| id == batch_id)
                {
                    found.push(task);
                }
            }
        }
        found.sort_by_key(|t| t.id);
        Ok(found)
    }

    /// La tarea de remisión del lote, ya en curso. Si el lote se reenvía, se reutiliza la
    /// anterior (fallida, en cola o en curso tras una interrupción) en lugar de crear otra por
    /// cada intento; sin ninguna reutilizable se crea una.
    fn claim_submit_task(&mut self, batch: &VerifactuBatch) -> AppResult<Task> {
        let reusable = self.submit_tasks_of(
            batch.id,
            &[TaskState::Running, TaskState::Queued, TaskState::Failed],
        )?;
        let task = match reusable.last() {
            Some(task) => match task.state {
                TaskState::Failed => {
                    self.store.retry(task.id)?;
                    task.clone()
                }
                TaskState::Running => {
                    // Quedó en curso sin que nadie la lleve (la pasada se abandonó): vuelve a
                    // la cola sin gastar intento.
                    self.store
                        .release(task.id, "recuperada: pasada de remisión abandonada")?;
                    task.clone()
                }
                _ => task.clone(),
            },
            None => {
                let input = json!({
                    "batch_id": batch.id,
                    "idempotency_key": batch.idempotency_key,
                    "environment": batch.environment.as_str(),
                    "records": batch.record_ids.len(),
                });
                self.store.enqueue(SUBMIT_VERIFACTU, &input.to_string())?
            }
        };
        Ok(self.store.claim(task.id)?)
    }

    /// Cierra las tareas de remisión que quedaron en cola con su lote ya cerrado (un corte entre
    /// guardar el resultado y completar la tarea, D50): se completan sin repetir ningún envío.
    /// Un fallo se anota, no se propaga.
    fn settle_submit_tasks(&mut self) {
        if let Err(error) = self.try_settle_submit_tasks() {
            let note = json!({ "code": error.code() });
            let _ = self.store.record_note("verifactu.recover", "error", &note);
        }
    }

    fn try_settle_submit_tasks(&mut self) -> AppResult<()> {
        let open = self.store.unfinished_verifactu_batch()?.map(|b| b.id);
        for task in self.store.tasks_in(TaskState::Queued)? {
            if task.kind != SUBMIT_VERIFACTU {
                continue;
            }
            let Some(batch_id) = task_batch_id(&task) else {
                continue;
            };
            if Some(batch_id) == open {
                continue;
            }
            // El lote ya está cerrado: su resultado consta en `verifactu_batch_results`.
            self.store.claim(task.id)?;
            let summary = json!({
                "type": SUBMIT_VERIFACTU,
                "version": 1,
                "batch_id": batch_id,
                "recovered": true,
            });
            self.store.complete(task.id, &summary)?;
        }
        Ok(())
    }

    /// Al abrir la aplicación: las tareas en curso ya volvieron a la cola (`recover`); las de
    /// remisión cuyo lote se cerró se completan, y las de un lote abierto esperan a la bomba,
    /// que lo reenvía idéntico. No hay llamada de red aquí ni se crea ningún registro.
    pub(crate) fn recover_verifactu(&mut self) {
        self.settle_submit_tasks();
    }

    // --- Subsanación (D253) ---

    /// Subsana un registro rechazado: crea un registro nuevo vinculado, encadenado y pendiente
    /// de remitir (la AEAT recibe `Subsanacion = S` y `RechazoPrevio = X` en un alta). Es
    /// idempotente: una segunda llamada devuelve el mismo registro con `created = false`.
    pub fn verifactu_subsanate(&mut self, record_id: i64) -> AppResult<VerifactuSubsanation> {
        let ctx = self.verifactu_context()?;
        let created = self.store.create_verifactu_subsanation(record_id, &ctx)?;
        if created.created {
            self.pump_after_write();
        }
        Ok(created)
    }

    // --- Estado para la interfaz ---

    /// Estado de la remisión: entorno, estado de cada registro con el código y la descripción
    /// de la AEAT, último lote, siguiente envío permitido, paro por `Fault` y pendientes.
    pub fn verifactu_status(&self) -> AppResult<VerifactuStatusView> {
        let environment = self.store.verifactu_environment()?;
        let rows: HashMap<i64, _> = self
            .store
            .verifactu_records()?
            .into_iter()
            .map(|r| (r.id, r))
            .collect();
        let mut records = Vec::with_capacity(rows.len());
        let (mut sending, mut rejected, mut unexpected) = (0u32, 0u32, 0u32);
        for state in self.store.verifactu_record_states()? {
            let VerifactuRecordState {
                record_id,
                status,
                aeat_code,
                aeat_description,
                duplicate,
                batch_id,
                subsanation_record_id,
            } = state;
            let Some(row) = rows.get(&record_id) else {
                continue;
            };
            let stored = self.store.verifactu_record_environment(record_id)?;
            let in_environment = match environment {
                VerifactuEnvironment::Produccion => matches!(
                    stored,
                    RecordEnvironment::Produccion | RecordEnvironment::ProduccionHeredado
                ),
                VerifactuEnvironment::Preproduccion => stored == RecordEnvironment::Preproduccion,
                VerifactuEnvironment::Simulador => stored == RecordEnvironment::Simulador,
            };
            if in_environment {
                match status {
                    VerifactuStatus::Sending => sending += 1,
                    VerifactuStatus::Rejected => rejected += 1,
                    VerifactuStatus::UnexpectedDuplicate => unexpected += 1,
                    _ => {}
                }
            }
            records.push(VerifactuRecordView {
                record_id,
                document_id: row.document_id,
                kind: kind_text(row.kind).to_owned(),
                invoice_number: row.invoice_number.clone(),
                issue_date: row.issue_date.clone(),
                status: status.as_str().to_owned(),
                aeat_code,
                aeat_description,
                duplicate,
                batch_id,
                subsanation_record_id,
                environment: stored.as_str().to_owned(),
            });
        }
        let pending = u32::try_from(self.store.verifactu_pending_records(environment)?.len())
            .unwrap_or(u32::MAX);
        let next_send_at = self.store.verifactu_next_send_at(environment)?;
        let wait_remaining_seconds = match (next_send_at, self.remission_now_ms()) {
            (Some(at), Ok(now)) => Some(u64::try_from((at - now + 999) / 1000).unwrap_or(0)),
            _ => None,
        };
        let faults = self.store.verifactu_consecutive_faults(environment)?;
        Ok(VerifactuStatusView {
            environment: environment.as_str().to_owned(),
            selectable_environments: vec![VerifactuEnvironment::Simulador.as_str().to_owned()],
            environment_notice: (environment == VerifactuEnvironment::Simulador).then(|| {
                "Entorno de pruebas: nada de esto llega a la AEAT y las facturas emitidas aquí \
                 llevan para siempre el QR de pruebas."
                    .to_owned()
            }),
            records,
            last_batch: self
                .store
                .verifactu_last_batch(environment)?
                .as_ref()
                .map(batch_view),
            next_send_at,
            wait_remaining_seconds,
            halted: faults >= MAX_AUTOMATIC_FAULTS,
            consecutive_faults: faults,
            pending,
            sending,
            rejected,
            unexpected_duplicates: unexpected,
        })
    }
}

/// Lote al que se refiere una tarea de remisión (`input_ref`).
fn task_batch_id(task: &Task) -> Option<i64> {
    serde_json::from_str::<Value>(&task.input_ref)
        .ok()?
        .get("batch_id")?
        .as_i64()
}

#[cfg(test)]
mod tests {
    use super::*;

    const FAULT: &str = "<?xml version=\"1.0\"?><env:Envelope xmlns:env=\"http://schemas.xmlsoap.org/soap/envelope/\"><env:Body><env:Fault><faultcode>env:Client</faultcode><faultstring>Codigo[4102].Falta un campo</faultstring></env:Fault></env:Body></env:Envelope>";

    #[test]
    fn http_errors_are_classified_and_none_closes_the_batch() {
        let code = |status, body| classify_response(status, body).unwrap_err().code;
        assert_eq!(code(503, "caído"), TRANSPORT_UNAVAILABLE);
        assert_eq!(code(500, "no es XML"), TRANSPORT_UNAVAILABLE);
        assert_eq!(code(200, "no es XML"), TRANSPORT_INVALID_RESPONSE);
        assert_eq!(code(404, ""), TRANSPORT_INVALID_RESPONSE);
        assert_eq!(code(401, ""), TRANSPORT_UNAUTHORIZED);
        assert_eq!(code(403, ""), TRANSPORT_UNAUTHORIZED);
        assert_eq!(code(429, ""), TRANSPORT_RATE_LIMITED);
    }

    #[test]
    fn a_soap_fault_is_a_result_not_a_transport_error() {
        for status in [200, 500] {
            match classify_response(status, FAULT) {
                Ok(ResultadoEnvio::Fallo(f)) => assert_eq!(f.codigo, Some(4102)),
                other => panic!("HTTP {status}: {other:?}"),
            }
        }
    }

    #[test]
    fn the_simulator_variable_names_parse_and_unknown_ones_are_ignored() {
        assert_eq!(
            SimMode::parse("rechazar-siguiente"),
            Some(SimMode::RejectNext)
        );
        assert_eq!(SimMode::parse("sin-red"), Some(SimMode::NoNetwork));
        assert_eq!(
            SimMode::parse(" perder-respuesta "),
            Some(SimMode::LoseNextResponse)
        );
        assert_eq!(SimMode::parse("fault-esquema"), Some(SimMode::SchemaFault));
        assert_eq!(
            SimMode::parse("aceptado-con-errores"),
            Some(SimMode::AcceptWithErrors)
        );
        assert_eq!(SimMode::parse("cualquier-cosa"), None);
    }

    #[test]
    fn the_pump_lease_is_exclusive_and_released_on_drop() {
        let busy = Arc::new(Mutex::new(false));
        let first = PumpLease::acquire(&busy).unwrap();
        assert!(PumpLease::acquire(&busy).is_none());
        drop(first);
        assert!(PumpLease::acquire(&busy).is_some());
    }
}
