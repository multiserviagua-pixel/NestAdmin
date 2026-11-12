//! Remisión de los registros Verifactu (etapa E5, encargo B, D247-D260, y T18b-D1, D278-D283):
//! entorno vigente, simulador en proceso, transporte SOAP detrás de `SoapTransport`, «bomba» de
//! remisión con su tarea `submit_verifactu`, certificado de remisión, intentos sin envío,
//! clasificación de los `Fault`, incidencias, recuperación al abrir y estado para la interfaz.
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
//!
//! Orden de cada pasada para un lote (D280): resolver la configuración; si el entorno lo exige
//! (`preproduccion` y `produccion`), obtener el certificado en silencio y sin el cerrojo de la
//! aplicación (`CredentialJob`); anotar el intento (`begin_verifactu_attempt`); llamar al
//! transporte. Si el certificado falla, no se anota ningún intento. Si el transporte dice que el
//! cuerpo no llegó a escribirse (`NotSent`), el intento se ANULA con una fila aparte
//! (`void_verifactu_attempt`; nunca se borra) y no cuenta para considerar propio un duplicado
//! 3000 (D50, D252). Un `Fault` de autorización (D281) para la remisión hasta que el usuario
//! cambia la configuración o declara que lo arregló; uno técnico no para nunca.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use nestadmin_certs::{
    AeatEnvironment, CertError, CertificateKind, CertificateSource, ClientCredentials, Endpoint,
    MutualTlsTransport, NoResponseReason, NotSentReason, SendOutcome, SoapTransport, TlsPreference,
    TransportReport, MAX_RESPONSE_BYTES,
};
use nestadmin_core::sales::{
    BatchClosure, BatchOutcome, FaultClass, RecordEnvironment, SalesStoreError, SenderCapacity,
    VerifactuBatch, VerifactuBatchSummary, VerifactuEnvironment, VerifactuKind,
    VerifactuRecordState, VerifactuStatus, VerifactuSubsanation, WaitMode,
    CODE_AUTHORIZATION_REJECTED, CODE_BATCH_OPEN, CODE_BATCH_REJECTED, CODE_ENVIRONMENT_BUSY,
    CODE_RECORD_INVALID, MAX_AUTOMATIC_FAULTS,
};
use nestadmin_core::{Task, TaskState};
#[cfg(debug_assertions)]
use nestadmin_verifactu::simulador::ErrorForzado;
use nestadmin_verifactu::simulador::{ConfigSimulador, SimuladorAeat};
use nestadmin_verifactu::xml::{Cabecera, PersonaEs};
use nestadmin_verifactu::{FalloSoap, FechaHoraHuso, RegistroFacturacion, ResultadoEnvio};
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
/// Código del último error cuando una respuesta no trae líneas para los registros enviados.
pub const CODE_EMPTY_RESPONSE: &str = "empty_response";
/// Estado que se muestra de un registro pendiente o en envío que es de otro entorno.
pub const OTHER_ENVIRONMENT: &str = "other_environment";
/// Variable que fuerza respuestas del simulador (solo compilaciones de depuración y entorno
/// `simulador`, D254).
pub const SIM_VARIABLE: &str = "NESTADMIN_VERIFACTU_SIM";
/// Código del intento sin envío cuando la compilación no incluye la red real de la AEAT
/// (`aeat-red`, D285).
pub const CODE_NETWORK_DISABLED: &str = "network_disabled";
/// Código del motivo cuando el certificado pide la intervención del usuario (D278): el registro
/// queda pendiente hasta «Remitir ahora».
pub const CODE_CERTIFICATE_NEEDS_USER: &str = "certificate_needs_user";
/// Declaración con la que el usuario reanuda la remisión parada por autorización (D281).
pub const AUTHORIZATION_DECLARATION: &str = "usuario_declara_autorizacion";
/// Tiempo que se espera a que un proveedor de claves conteste en modo silencioso (D280).
const SILENT_ACQUIRE_TIMEOUT: Duration = Duration::from_secs(5);

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

/// Credenciales de remisión ya obtenidas: la cadena y la clave de firma del TLS mutuo, el tipo de
/// certificado (para elegir el endpoint, D279) y su huella (para recordar si solo negocia
/// TLS 1.2). Su `Debug` no muestra nada del certificado.
pub struct SenderCredentials {
    pub credentials: ClientCredentials,
    pub kind: CertificateKind,
    /// Huella SHA-256 del certificado, en hexadecimal.
    pub fingerprint: String,
}

impl std::fmt::Debug for SenderCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SenderCredentials")
            .field("kind", &self.kind)
            .finish_non_exhaustive()
    }
}

/// De dónde sale el certificado de remisión (D278, D280): el escritorio inyecta el de Windows
/// (almacén o PFX); las pruebas, uno sintético. Sin proveedor, o sin certificado vinculado, el
/// resultado es `certificate_missing`.
pub trait CertificateProvider: Send + Sync {
    /// Obtiene la clave de firma. Con `interactive = false` nunca muestra una ventana ni pide un
    /// PIN: si hace falta, devuelve [`CertError::NeedsUser`]. Con `interactive = true` (solo
    /// «Remitir ahora») Windows puede mostrar su diálogo. `now_ms` es el reloj de la aplicación.
    fn acquire(
        &self,
        source: &CertificateSource,
        interactive: bool,
        now_ms: i64,
    ) -> Result<SenderCredentials, CertError>;
}

/// Proveedor de Windows: almacén de certificados (CNG) o PFX custodiado con DPAPI (D278).
#[cfg(windows)]
pub struct WindowsCertificateProvider {
    store_dir: std::path::PathBuf,
    entropy: Vec<u8>,
}

#[cfg(windows)]
impl WindowsCertificateProvider {
    /// `store_dir`: la carpeta de datos del usuario donde `import_pfx` guarda los PFX;
    /// `entropy`: la entropía por instalación con la que se cifraron.
    pub fn new(store_dir: std::path::PathBuf, entropy: Vec<u8>) -> Self {
        Self { store_dir, entropy }
    }
}

#[cfg(windows)]
impl CertificateProvider for WindowsCertificateProvider {
    fn acquire(
        &self,
        source: &CertificateSource,
        interactive: bool,
        now_ms: i64,
    ) -> Result<SenderCredentials, CertError> {
        use nestadmin_certs::AcquireMode;
        let signer = if interactive {
            nestadmin_certs::acquire(
                source,
                &self.store_dir,
                &self.entropy,
                AcquireMode::Interactive,
                now_ms,
            )?
        } else {
            nestadmin_certs::acquire_with_timeout(
                source,
                &self.store_dir,
                &self.entropy,
                AcquireMode::Silent,
                now_ms,
                SILENT_ACQUIRE_TIMEOUT,
            )?
        };
        Ok(SenderCredentials {
            credentials: ClientCredentials::from(&signer),
            kind: signer.info.kind,
            fingerprint: signer.info.fingerprint.clone(),
        })
    }
}

fn not_sent(reason: NotSentReason) -> TransportReport {
    TransportReport {
        outcome: SendOutcome::NotSent(reason),
        needs_tls12: false,
    }
}

fn no_response(reason: NoResponseReason) -> TransportReport {
    TransportReport {
        outcome: SendOutcome::SentNoResponse(reason),
        needs_tls12: false,
    }
}

/// Transporte del entorno `simulador` (D254): un `POST` HTTP/1.1 plano contra el simulador local
/// en 127.0.0.1, detrás del mismo trait que el TLS mutuo. Distingue sin ambigüedad lo que nunca
/// se escribió (`NotSent`: DNS, conexión) de lo que pudo llegar sin respuesta completa
/// (`SentNoResponse`), igual que `MutualTlsTransport`. Ignora el endpoint de la AEAT y las
/// credenciales: solo habla con la URL que se le dio, que es siempre local.
#[derive(Debug, Clone)]
pub struct SimulatorTransport {
    url: String,
}

impl SimulatorTransport {
    pub fn new(url: impl Into<String>) -> Self {
        Self { url: url.into() }
    }

    /// Un único `POST` del cuerpo, con tiempo de conexión de 3 s y plazo total de 10 s.
    pub fn post(&self, body: &[u8]) -> TransportReport {
        let Some((authority, path)) = split_http_url(&self.url) else {
            return not_sent(NotSentReason::Connect);
        };
        let addr = match authority.to_socket_addrs() {
            Ok(mut addrs) => match addrs.next() {
                Some(addr) => addr,
                None => return not_sent(NotSentReason::Dns),
            },
            Err(_) => return not_sent(NotSentReason::Dns),
        };
        let started = Instant::now();
        let mut stream = match TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT) {
            Ok(stream) => stream,
            Err(error) if is_timeout(&error) => return not_sent(NotSentReason::ConnectTimeout),
            Err(_) => return not_sent(NotSentReason::Connect),
        };
        let _ = stream.set_nodelay(true);
        let remaining = || REQUEST_TIMEOUT.saturating_sub(started.elapsed());
        let _ = stream.set_write_timeout(Some(remaining().max(Duration::from_millis(1))));
        let mut request = format!(
            "POST {path} HTTP/1.1\r\nHost: {authority}\r\nContent-Type: text/xml; charset=utf-8\r\n\
             SOAPAction: \"\"\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .into_bytes();
        request.extend_from_slice(body);
        if let Err(error) = stream.write_all(&request).and_then(|_| stream.flush()) {
            return no_response(if is_timeout(&error) {
                NoResponseReason::WriteTimeout
            } else {
                NoResponseReason::WriteFailed
            });
        }
        read_plain_response(&mut stream, &remaining)
    }
}

impl SoapTransport for SimulatorTransport {
    fn send(
        &self,
        _endpoint: Endpoint,
        body: &[u8],
        _credentials: &ClientCredentials,
        _tls: TlsPreference,
    ) -> TransportReport {
        self.post(body)
    }
}

fn is_timeout(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
    )
}

/// `http://host:puerto/ruta` -> (`host:puerto`, `/ruta`). Solo HTTP plano: el simulador es local.
fn split_http_url(url: &str) -> Option<(&str, String)> {
    let rest = url.strip_prefix("http://")?;
    let (authority, path) = match rest.split_once('/') {
        Some((authority, path)) => (authority, format!("/{path}")),
        None => (rest, "/".to_owned()),
    };
    (!authority.is_empty()).then_some((authority, path))
}

/// Lee la respuesta HTTP/1.1 (`Content-Length` o hasta el cierre) con el plazo que quede.
fn read_plain_response(
    stream: &mut TcpStream,
    remaining: &dyn Fn() -> Duration,
) -> TransportReport {
    let mut buffer: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 8192];
    // (fin de la cabecera, estado, longitud del cuerpo si se declaró)
    let mut head: Option<(usize, u16, Option<usize>)> = None;
    loop {
        if head.is_none() {
            if let Some(end) = buffer.windows(4).position(|w| w == b"\r\n\r\n") {
                let text = String::from_utf8_lossy(&buffer[..end]).into_owned();
                let mut lines = text.lines();
                let status = lines
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1))
                    .and_then(|code| code.parse::<u16>().ok());
                let Some(status) = status else {
                    return no_response(NoResponseReason::ResponseInvalid);
                };
                let mut length = None;
                for line in lines {
                    if let Some((name, value)) = line.split_once(':') {
                        let name = name.trim();
                        if name.eq_ignore_ascii_case("content-length") {
                            length = value.trim().parse::<usize>().ok();
                        } else if name.eq_ignore_ascii_case("transfer-encoding") {
                            return no_response(NoResponseReason::ResponseInvalid);
                        }
                    }
                }
                if length.is_some_and(|l| l > MAX_RESPONSE_BYTES) {
                    return no_response(NoResponseReason::ResponseTooLarge);
                }
                head = Some((end + 4, status, length));
            }
        }
        if let Some((start, status, Some(length))) = head {
            if buffer.len() >= start + length {
                let body = buffer[start..start + length].to_vec();
                return TransportReport {
                    outcome: SendOutcome::Response { status, body },
                    needs_tls12: false,
                };
            }
        }
        if buffer.len() > MAX_RESPONSE_BYTES + 16 * 1024 {
            return no_response(NoResponseReason::ResponseTooLarge);
        }
        let left = remaining();
        if left.is_zero() {
            return no_response(NoResponseReason::ReadTimeout);
        }
        let _ = stream.set_read_timeout(Some(left));
        match stream.read(&mut chunk) {
            Ok(0) => {
                return match head {
                    // Sin `Content-Length`, el cierre marca el fin del cuerpo.
                    Some((start, status, None)) => TransportReport {
                        outcome: SendOutcome::Response {
                            status,
                            body: buffer[start..].to_vec(),
                        },
                        needs_tls12: false,
                    },
                    _ => no_response(NoResponseReason::ConnectionClosed),
                };
            }
            Ok(n) => buffer.extend_from_slice(&chunk[..n]),
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) if is_timeout(&error) => return no_response(NoResponseReason::ReadTimeout),
            Err(_) => return no_response(NoResponseReason::ConnectionClosed),
        }
    }
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
    /// El siguiente envío recibe un `Fault` de autorización (4112, D281): la remisión se para.
    AuthorizationFault,
    /// El siguiente envío recibe un `Fault` técnico (4108, D281): nunca para; se reintenta con
    /// espera creciente.
    TechnicalFault,
}

impl SimMode {
    /// Valor de `NESTADMIN_VERIFACTU_SIM`: `rechazar-siguiente`, `sin-red`, `perder-respuesta`,
    /// `fault-esquema`, `aceptado-con-errores`, `fault-autorizacion` o `fault-tecnico`.
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim() {
            "rechazar-siguiente" => Some(Self::RejectNext),
            "sin-red" => Some(Self::NoNetwork),
            "perder-respuesta" => Some(Self::LoseNextResponse),
            "fault-esquema" => Some(Self::SchemaFault),
            "aceptado-con-errores" => Some(Self::AcceptWithErrors),
            "fault-autorizacion" => Some(Self::AuthorizationFault),
            "fault-tecnico" => Some(Self::TechnicalFault),
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
    /// Último error al preparar o enviar una pasada (en memoria): lo ve la interfaz.
    last_error: Option<RemissionIssue>,
    /// De dónde sale el certificado de remisión (D278): lo inyecta el escritorio. Sin él, la
    /// remisión a la AEAT da `certificate_missing`.
    certificates: Option<Arc<dyn CertificateProvider>>,
    /// La última pasada terminó en `certificate_needs_user`: el registro espera a «Remitir
    /// ahora» (el worker nunca abre una ventana, D280).
    needs_certificate: bool,
    /// Lo último que impidió abrir un lote sin llegar a abrirlo (sin transporte, proveedor o
    /// certificado vinculado): no hay lote al que anotarlo, así que solo vive en memoria.
    unbatched: Option<VerifactuUnsentView>,
    /// Aún no hubo ninguna pasada desde que se abrió la aplicación: la primera reintenta
    /// enseguida, sin esperar la espera que dejaron los fallos anteriores (D281).
    startup_pending: bool,
    #[cfg(debug_assertions)]
    sim_mode: Option<SimMode>,
    /// Servidor de pruebas que sustituye al simulador (solo depuración).
    #[cfg(debug_assertions)]
    endpoint: Option<String>,
    /// Transporte de pruebas que sustituye al TLS mutuo en preproducción y producción (solo
    /// depuración): los endpoints de la AEAT son constantes y nada los cambia (D279).
    #[cfg(debug_assertions)]
    transport: Option<Arc<dyn SoapTransport>>,
}

impl RemissionRuntime {
    pub(crate) fn from_env() -> Self {
        Self {
            busy: Arc::new(Mutex::new(false)),
            requested: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            simulator: None,
            last_error: None,
            certificates: None,
            needs_certificate: false,
            unbatched: None,
            startup_pending: true,
            #[cfg(debug_assertions)]
            sim_mode: std::env::var(SIM_VARIABLE)
                .ok()
                .and_then(|v| SimMode::parse(&v)),
            #[cfg(debug_assertions)]
            endpoint: None,
            #[cfg(debug_assertions)]
            transport: None,
        }
    }
}

/// Último error de la remisión (preparación, transporte, `Fault` o respuesta vacía).
#[derive(Debug, Clone)]
struct RemissionIssue {
    code: String,
    message: String,
    at: i64,
    /// Veces seguidas que ocurrió el mismo código desde el último envío bien resuelto.
    occurrences: u32,
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
    /// Tres `Fault` de esquema seguidos: la remisión automática está parada hasta «Remitir
    /// ahora» (D259, D281).
    Halted,
    /// La AEAT respondió con un `Fault` de autorización (4112, 4105, 4117, 4123 o 4124): la
    /// remisión de esta base está parada y no se reintenta hasta que el usuario cambie el
    /// certificado o la configuración, o declare que lo arregló (D281).
    AuthorizationRejected { code: u32 },
    /// El certificado pide la intervención del usuario (PIN, tarjeta, confirmación): no se anotó
    /// ningún intento y el registro queda pendiente hasta «Remitir ahora» (D278, D280).
    NeedsUser { batch_id: i64 },
    /// El cuerpo no llegó a escribirse (certificado ausente, caducado o rechazado, sin red, saludo
    /// TLS rechazado, `network_disabled`...): no cuenta como intento de envío (D280). `code` es
    /// el motivo estable.
    NotSent {
        batch_id: i64,
        code: String,
        message: String,
    },
    /// El entorno vigente no tiene transporte.
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
    /// Intentos que cuentan como envío (D252, D280).
    pub attempts: u32,
    /// Intentos sin envío: el cuerpo no llegó a escribirse y no cuentan (D280).
    pub unsent_attempts: u32,
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
    /// Último error de preparación o de transporte (código estable y texto), hasta el siguiente
    /// envío bien resuelto o hasta que no quede nada pendiente. Solo se guarda en memoria.
    pub last_error: Option<VerifactuIssueView>,
    /// Momento (milisegundos Unix) en que se generó el registro más antiguo del entorno vigente
    /// que aún no obtuvo respuesta (pendiente o en un lote abierto sin enviar) y su antigüedad en
    /// segundos según el reloj de la aplicación (D281).
    pub oldest_pending_at: Option<i64>,
    pub oldest_pending_age_seconds: Option<u64>,
    /// Hay una incidencia técnica de remisión abierta (D282): los registros generados ahora irán
    /// en un lote con `Incidencia = S`.
    pub incident_open: bool,
    /// El último intento sin envío y su motivo (D280).
    pub last_unsent: Option<VerifactuUnsentView>,
    /// La remisión de esta base está parada por un `Fault` de autorización y no se reintenta
    /// hasta que el usuario cambie el certificado o la configuración, o declare que lo arregló
    /// (D281). `authorization_code` es el código de la AEAT.
    pub authorization_rejected: bool,
    pub authorization_code: Option<u32>,
    /// El certificado pide la intervención del usuario: «Remitir ahora» abre el diálogo (D280).
    pub needs_certificate: bool,
}

/// Un intento sin envío tal como lo ve la interfaz (D280).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VerifactuUnsentView {
    pub batch_id: i64,
    /// Código estable del motivo (`certificate_needs_user`, `connect_failed`...).
    pub code: String,
    /// Milisegundos Unix.
    pub at: i64,
}

/// Configuración de la remisión de esta base (D278, D283).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VerifactuRemissionSettingsView {
    /// Origen del certificado (huella pública o referencia opaca), si hay uno vinculado.
    pub certificate: Option<CertificateSource>,
    /// `taxpayer`, `representative`, `attorney` o `collaborator`.
    pub sender_capacity: String,
    /// Representante configurado expresamente (D283): `(nombre, NIF)`.
    pub representative: Option<VerifactuRepresentativeView>,
    pub changed_at: i64,
}

/// Quien remite con su propio certificado en nombre del obligado (`Cabecera/Representante`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VerifactuRepresentativeView {
    pub name: String,
    pub nif: String,
}

/// Error de la remisión tal como lo ve la interfaz.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VerifactuIssueView {
    pub code: String,
    pub message: String,
    /// Milisegundos Unix.
    pub at: i64,
    /// Veces seguidas que se repitió el mismo error.
    pub occurrences: u32,
}

// ---------------------------------------------------------------------------------------------
// Trabajo de una pasada (se parte para no tener el cerrojo de la aplicación durante la red)
// ---------------------------------------------------------------------------------------------

/// Cómo sale el cuerpo de esta pasada.
enum Wire {
    /// El simulador local (D254), con HTTP plano en 127.0.0.1.
    Simulator {
        transport: SimulatorTransport,
        /// Modo de depuración `sin-red`: no se llega a conectar.
        no_network: bool,
    },
    /// TLS mutuo hacia la AEAT, o el transporte de pruebas que lo sustituye (D279).
    Remote {
        transport: Arc<dyn SoapTransport>,
        credentials: ClientCredentials,
        endpoint: Endpoint,
        tls: TlsPreference,
        fingerprint: String,
    },
}

/// Trabajo de red de una pasada ya preparado, con el intento ya anotado (D252): se ejecuta sin
/// tocar la aplicación.
pub struct VerifactuJob {
    lease: PumpLease,
    task_id: i64,
    batch_id: i64,
    /// Intento anotado antes de la llamada: se anula si el cuerpo no llegó a escribirse (D280).
    attempt_id: i64,
    /// Identificador de la configuración de remisión con la que se preparó la pasada (D281).
    settings_id: i64,
    body: String,
    started_ms: i64,
    wire: Wire,
    simulator: Option<Arc<SimuladorAeat>>,
    cleanup: Vec<Cleanup>,
}

/// Lo que devolvió la llamada, listo para guardarse con [`App::verifactu_pump_finish`].
pub struct VerifactuSent {
    lease: PumpLease,
    task_id: i64,
    batch_id: i64,
    attempt_id: i64,
    settings_id: i64,
    started_ms: i64,
    report: TransportReport,
    /// Huella del certificado usado, para recordar si solo negoció TLS 1.2 (D279).
    fingerprint: Option<String>,
}

impl VerifactuJob {
    /// Hace la llamada (sin la aplicación) y deshace lo que se forzó en el simulador.
    pub fn run(self) -> VerifactuSent {
        let (report, fingerprint) = match &self.wire {
            Wire::Simulator {
                no_network: true, ..
            } => (not_sent(NotSentReason::Connect), None),
            Wire::Simulator { transport, .. } => (transport.post(self.body.as_bytes()), None),
            Wire::Remote {
                transport,
                credentials,
                endpoint,
                tls,
                fingerprint,
            } => (
                transport.send(*endpoint, self.body.as_bytes(), credentials, *tls),
                Some(fingerprint.clone()),
            ),
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
            attempt_id: self.attempt_id,
            settings_id: self.settings_id,
            started_ms: self.started_ms,
            report,
            fingerprint,
        }
    }
}

/// Primera fase de una pasada con certificado (`preproduccion` y `produccion`): obtener la clave
/// de firma ANTES de anotar el intento (D280). Puede tardar (una tarjeta, un proveedor en la
/// nube), así que se hace sin el cerrojo de la aplicación.
pub struct CredentialJob {
    lease: PumpLease,
    task_id: i64,
    batch_id: i64,
    body: String,
    started_ms: i64,
    /// «Remitir ahora»: si el modo silencioso da `NeedsUser`, se reintenta con diálogo.
    manual: bool,
    source: CertificateSource,
    provider: Arc<dyn CertificateProvider>,
    transport: Arc<dyn SoapTransport>,
    aeat: AeatEnvironment,
    settings_id: i64,
}

/// Lo que dio la obtención del certificado, listo para [`App::verifactu_pump_attempt`].
pub struct CredentialsFetched {
    lease: PumpLease,
    task_id: i64,
    batch_id: i64,
    body: String,
    started_ms: i64,
    transport: Arc<dyn SoapTransport>,
    aeat: AeatEnvironment,
    settings_id: i64,
    result: Result<SenderCredentials, CertError>,
}

impl CredentialJob {
    /// Obtiene el certificado en silencio (sin ventana ni PIN). Solo con «Remitir ahora», y solo
    /// si hace falta al usuario, vuelve a pedirlo con el diálogo de Windows: el worker nunca
    /// abre una ventana (D280).
    pub fn run(self) -> CredentialsFetched {
        let mut result = self.provider.acquire(&self.source, false, self.started_ms);
        if self.manual && matches!(result, Err(CertError::NeedsUser)) {
            result = self.provider.acquire(&self.source, true, self.started_ms);
        }
        CredentialsFetched {
            lease: self.lease,
            task_id: self.task_id,
            batch_id: self.batch_id,
            body: self.body,
            started_ms: self.started_ms,
            transport: self.transport,
            aeat: self.aeat,
            settings_id: self.settings_id,
            result,
        }
    }
}

/// Lo que hace falta para enviar con certificado (ver `App::remote_prereqs`).
struct RemotePrereqs {
    aeat: AeatEnvironment,
    transport: Arc<dyn SoapTransport>,
    source: CertificateSource,
    provider: Arc<dyn CertificateProvider>,
    settings_id: i64,
}

/// Resultado de preparar una pasada.
pub enum PumpStart {
    /// No hay nada que enviar ahora (y por qué).
    Idle(PumpReport),
    /// Hay que obtener el certificado (sin el cerrojo de la aplicación) y después anotar el
    /// intento con [`App::verifactu_pump_attempt`].
    Credentials(CredentialJob),
    /// Hay una llamada que hacer: el intento ya está anotado.
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
        unsent_attempts: summary.unsent_attempts,
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
            .verifactu_accepted_seed(VerifactuEnvironment::Simulador)?;
        let sim = SimuladorAeat::iniciar(ConfigSimulador::default())?;
        sim.sembrar_con_estado(&accepted);
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
                    SimMode::AuthorizationFault => {
                        sim.estado().fallo_soap_siguiente = Some(4112);
                    }
                    SimMode::TechnicalFault => {
                        sim.estado().fallo_soap_siguiente = Some(4108);
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
    /// `manual` (el botón «Remitir ahora») se salta el paro por `Fault` de esquema seguidos,
    /// pero no la espera por fallos que sí llegaron a la AEAT (D251) ni el paro por autorización
    /// (D281: ese lo levanta [`App::verifactu_declare_authorization_fixed`]). Se llama tras emitir
    /// y anular, en el tick periódico y con el botón.
    pub fn verifactu_pump(&mut self, manual: bool) -> AppResult<PumpReport> {
        let mut start = self.verifactu_pump_begin(manual)?;
        loop {
            match start {
                PumpStart::Idle(report) => return Ok(report),
                PumpStart::Credentials(job) => {
                    let fetched = job.run();
                    start = self.verifactu_pump_attempt(fetched)?;
                }
                PumpStart::Job(job) => return self.verifactu_pump_finish(job.run()),
            }
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

    /// Primera fase de la pasada: decide, abre o recupera el lote y crea la tarea. En el
    /// simulador anota ya el intento y devuelve el trabajo de red; con certificado devuelve
    /// primero la obtención de la clave (`PumpStart::Credentials`), que el llamador hace sin la
    /// aplicación y entrega a [`App::verifactu_pump_attempt`]. Ninguna fase hace llamadas de red
    /// con el cerrojo de la aplicación tomado.
    pub fn verifactu_pump_begin(&mut self, manual: bool) -> AppResult<PumpStart> {
        let started = self.pump_begin_inner(manual);
        match &started {
            Err(error) => {
                let (code, message) = (error.code().to_owned(), error.to_string());
                self.note_issue(&code, message);
            }
            Ok(PumpStart::Idle(PumpReport::Idle)) => self.remission.last_error = None,
            Ok(_) => {}
        }
        started
    }

    /// Segunda fase de la pasada con certificado: con la clave obtenida, anota el intento
    /// ANTES de la llamada (D252) y devuelve el trabajo de red. Si el certificado falló, no se
    /// anota ningún intento (D280): queda un intento sin envío con el motivo, la tarea fallida y
    /// reintentable y el lote abierto.
    pub fn verifactu_pump_attempt(&mut self, fetched: CredentialsFetched) -> AppResult<PumpStart> {
        let started = self.attempt_inner(fetched);
        if let Err(error) = &started {
            let (code, message) = (error.code().to_owned(), error.to_string());
            self.note_issue(&code, message);
        }
        started
    }

    /// Anota el último error de la remisión; el mismo código seguido suma ocurrencias.
    fn note_issue(&mut self, code: &str, message: String) {
        let at = self.remission_now_ms().unwrap_or(0);
        let occurrences = match &self.remission.last_error {
            Some(previous) if previous.code == code => previous.occurrences.saturating_add(1),
            _ => 1,
        };
        self.remission.last_error = Some(RemissionIssue {
            code: code.to_owned(),
            message,
            at,
            occurrences,
        });
    }

    fn pump_begin_inner(&mut self, manual: bool) -> AppResult<PumpStart> {
        let Some(lease) = PumpLease::acquire(&self.remission.busy) else {
            return Ok(PumpStart::Idle(PumpReport::Busy));
        };
        let environment = self.store.verifactu_environment()?;
        let now = self.remission_now_ms()?;
        // La primera pasada tras abrir la aplicación reintenta enseguida (D281).
        let startup = std::mem::replace(&mut self.remission.startup_pending, false);
        self.settle_submit_tasks();
        let open = self.store.unfinished_verifactu_batch()?;
        // Un `Fault` de autorización para la remisión de esta base: no se reintenta hasta que el
        // usuario cambie el certificado o la configuración, o declare que lo arregló (D281).
        if let Some(halt) = self.store.verifactu_authorization_halt()? {
            return Ok(PumpStart::Idle(PumpReport::AuthorizationRejected {
                code: halt.error_code,
            }));
        }
        let faults = self.store.verifactu_consecutive_faults(environment)?;
        // El paro frena abrir lotes nuevos, no reenviar el abierto: un lote sin respuesta se
        // reenvía idéntico aunque haya tres `Fault` seguidos (D252, D259).
        if open.is_none() && !manual && faults >= MAX_AUTOMATIC_FAULTS {
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
        let mode = if manual {
            WaitMode::Manual
        } else if startup {
            WaitMode::Startup
        } else {
            WaitMode::Automatic
        };
        if let Some(until) = self.store.verifactu_next_send_at_mode(environment, mode)? {
            if now < until {
                let seconds = u64::try_from((until - now + 999) / 1000).unwrap_or(0);
                return Ok(PumpStart::Idle(PumpReport::Waiting { seconds, until }));
            }
        }
        let batch = match open {
            Some(batch) => batch,
            None => {
                // Sin lote abierto se comprueban antes el transporte, el proveedor y el
                // certificado vinculado: si falta algo no se abre el lote, y así un
                // `Representante` configurado después entra en el primero (D283).
                if environment != VerifactuEnvironment::Simulador {
                    if let Err(code) = self.remote_prereqs(environment)? {
                        return Ok(PumpStart::Idle(self.note_unbatched(code, now)));
                    }
                }
                self.remission.unbatched = None;
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
        let simulator = match environment {
            VerifactuEnvironment::Simulador => Some(self.ensure_simulator()?),
            _ => None,
        };
        let task = self.claim_submit_task(&batch)?;
        let Some(simulator) = simulator else {
            return self.credentials_phase(lease, task.id, &batch, body, now, manual, environment);
        };
        // El intento se anota ANTES de la llamada (D252).
        let settings_id = match self.store.verifactu_remission_settings() {
            Ok(settings) => settings.id,
            Err(error) => return Err(self.fail_task_with(task.id, error)),
        };
        let ticket = match self.store.begin_verifactu_attempt_ticket(batch.id, now) {
            Ok(ticket) => ticket,
            Err(error) => return Err(self.fail_task_with(task.id, error)),
        };
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
            attempt_id: ticket.attempt_id,
            settings_id,
            body,
            started_ms: now,
            wire: Wire::Simulator {
                transport: SimulatorTransport::new(url),
                no_network,
            },
            simulator: Some(simulator),
            cleanup,
        }))
    }

    /// Falla la tarea de remisión con el error de almacenamiento y lo devuelve.
    fn fail_task_with(&mut self, task_id: i64, error: SalesStoreError) -> AppError {
        let _ = self.store.fail(
            task_id,
            &task_error(error.code(), "no se pudo anotar el intento", true),
        );
        error.into()
    }

    /// Prepara la pasada de un entorno con certificado (D280): el transporte, la configuración y
    /// la obtención de la clave, antes de anotar ningún intento.
    #[allow(clippy::too_many_arguments)]
    fn credentials_phase(
        &mut self,
        lease: PumpLease,
        task_id: i64,
        batch: &VerifactuBatch,
        body: String,
        now: i64,
        manual: bool,
        environment: VerifactuEnvironment,
    ) -> AppResult<PumpStart> {
        let prereqs = match self.remote_prereqs(environment) {
            Ok(Ok(prereqs)) => prereqs,
            Ok(Err(code)) => {
                let report = self.finish_unsent(task_id, batch.id, code, None, now);
                return Ok(PumpStart::Idle(report));
            }
            Err(error) => return Err(self.fail_task_with_app(task_id, error)),
        };
        Ok(PumpStart::Credentials(CredentialJob {
            lease,
            task_id,
            batch_id: batch.id,
            body,
            started_ms: now,
            manual,
            source: prereqs.source,
            provider: prereqs.provider,
            transport: prereqs.transport,
            aeat: prereqs.aeat,
            settings_id: prereqs.settings_id,
        }))
    }

    fn fail_task_with_app(&mut self, task_id: i64, error: AppError) -> AppError {
        let _ = self.store.fail(
            task_id,
            &task_error(error.code(), "no se pudo preparar el envío", true),
        );
        error
    }

    /// Lo que hace falta para enviar con certificado: el transporte (sin la red real compilada,
    /// `network_disabled`), la configuración vigente, el certificado vinculado y quien lo
    /// proporciona. `Ok(Err(código))` dice qué falta, con el código del motivo sin envío.
    fn remote_prereqs(
        &self,
        environment: VerifactuEnvironment,
    ) -> AppResult<Result<RemotePrereqs, &'static str>> {
        let aeat = match environment {
            VerifactuEnvironment::Produccion => AeatEnvironment::Production,
            _ => AeatEnvironment::Preproduction,
        };
        let transport = match self.remote_transport(aeat) {
            Ok(transport) => transport,
            Err(code) => return Ok(Err(code)),
        };
        let settings = self.store.verifactu_remission_settings()?;
        let source = match settings.certificate_source.as_deref() {
            None => return Ok(Err(CertError::Missing.code())),
            Some(json) => match serde_json::from_str::<CertificateSource>(json) {
                Ok(source) => source,
                Err(_) => return Ok(Err(CertError::Invalid.code())),
            },
        };
        let Some(provider) = self.remission.certificates.clone() else {
            return Ok(Err(CertError::Missing.code()));
        };
        Ok(Ok(RemotePrereqs {
            aeat,
            transport,
            source,
            provider,
            settings_id: settings.id,
        }))
    }

    /// Falta algo para enviar y ni siquiera hay lote: no se abre ninguno (D283). Queda como
    /// último error y como último intento sin envío en memoria, sin lote (`batch_id` 0).
    fn note_unbatched(&mut self, code: &'static str, now: i64) -> PumpReport {
        let message = unsent_message(code);
        self.remission.unbatched = Some(VerifactuUnsentView {
            batch_id: 0,
            code: code.to_owned(),
            at: now,
        });
        self.remission.needs_certificate = false;
        self.note_issue(code, message.clone());
        PumpReport::NotSent {
            batch_id: 0,
            code: code.to_owned(),
            message,
        }
    }

    /// El transporte hacia la AEAT: el TLS mutuo (solo con la característica `aeat-red`, D285) o,
    /// en depuración, el de pruebas. `Err` lleva el código del motivo (`network_disabled`).
    fn remote_transport(
        &self,
        aeat: AeatEnvironment,
    ) -> Result<Arc<dyn SoapTransport>, &'static str> {
        #[cfg(debug_assertions)]
        if let Some(transport) = &self.remission.transport {
            return Ok(Arc::clone(transport));
        }
        MutualTlsTransport::aeat(aeat)
            .map(|transport| Arc::new(transport) as Arc<dyn SoapTransport>)
            .map_err(|error| error.code())
    }

    fn attempt_inner(&mut self, fetched: CredentialsFetched) -> AppResult<PumpStart> {
        let CredentialsFetched {
            lease,
            task_id,
            batch_id,
            body,
            started_ms,
            transport,
            aeat,
            settings_id,
            result,
        } = fetched;
        let now = self.remission_now_ms().unwrap_or(started_ms);
        // La configuración cambió mientras se obtenía la clave (otro certificado, otro
        // representante): no se envía con lo anterior; se anota y se pide otra pasada.
        let current = match self.store.verifactu_remission_settings() {
            Ok(settings) => settings.id,
            Err(error) => return Err(self.fail_task_with(task_id, error)),
        };
        if current != settings_id {
            let report = self.finish_unsent(task_id, batch_id, "settings_changed", None, now);
            self.pump_after_write();
            drop(lease);
            return Ok(PumpStart::Idle(report));
        }
        let credentials = match result {
            Ok(credentials) => credentials,
            Err(error) => {
                // La clave no estaba disponible: no se anota ningún intento (D280).
                let report = self.finish_unsent(task_id, batch_id, error.code(), None, now);
                drop(lease);
                return Ok(PumpStart::Idle(report));
            }
        };
        let tls = if self
            .store
            .verifactu_tls12_preferred(&credentials.fingerprint)
            .unwrap_or(false)
        {
            TlsPreference::Tls12
        } else {
            TlsPreference::Auto
        };
        let endpoint = Endpoint::for_certificate(aeat, credentials.kind);
        // El intento se anota ANTES de la llamada (D252), con la clave ya en la mano.
        let ticket = match self.store.begin_verifactu_attempt_ticket(batch_id, now) {
            Ok(ticket) => ticket,
            Err(error) => return Err(self.fail_task_with(task_id, error)),
        };
        Ok(PumpStart::Job(VerifactuJob {
            lease,
            task_id,
            batch_id,
            attempt_id: ticket.attempt_id,
            settings_id,
            body,
            started_ms: now,
            wire: Wire::Remote {
                transport,
                credentials: credentials.credentials,
                endpoint,
                tls,
                fingerprint: credentials.fingerprint,
            },
            simulator: None,
            cleanup: Vec::new(),
        }))
    }

    /// El cuerpo no llegó a escribirse: no cuenta como intento de envío (D280). Si el intento ya
    /// estaba anotado se anula (una fila aparte; nunca se borra) y, si no, se anota el motivo
    /// sin intento. El lote sigue abierto, la tarea queda fallida y reintentable y se abre una
    /// incidencia (D282).
    fn finish_unsent(
        &mut self,
        task_id: i64,
        batch_id: i64,
        code: &str,
        voided_attempt: Option<i64>,
        now: i64,
    ) -> PumpReport {
        let noted = match voided_attempt {
            Some(attempt_id) => self.store.void_verifactu_attempt(attempt_id, code, now),
            None => self
                .store
                .note_verifactu_unsent_attempt(batch_id, code, now),
        };
        if let Err(error) = noted {
            let note = json!({ "batch_id": batch_id, "code": error.code() });
            let _ = self.store.record_note("verifactu.unsent", "error", &note);
        }
        let message = unsent_message(code);
        let note = json!({ "batch_id": batch_id, "code": code });
        let _ = self.store.record_note("verifactu.unsent", "error", &note);
        let _ = self
            .store
            .fail(task_id, &task_error(code, message.clone(), true));
        self.note_issue(code, message.clone());
        self.remission.needs_certificate = code == CODE_CERTIFICATE_NEEDS_USER;
        if code == CODE_CERTIFICATE_NEEDS_USER {
            PumpReport::NeedsUser { batch_id }
        } else {
            PumpReport::NotSent {
                batch_id,
                code: code.to_owned(),
                message,
            }
        }
    }

    /// Sin respuesta entendida (el cuerpo pudo llegar): el lote sigue abierto, la tarea queda
    /// fallida y reintentable con el motivo visible y se abre una incidencia (D282).
    fn finish_no_response(
        &mut self,
        task_id: i64,
        batch_id: i64,
        error: TransportError,
        reason: &str,
        now: i64,
    ) -> PumpReport {
        let note = json!({ "batch_id": batch_id, "code": error.code });
        let _ = self
            .store
            .record_note("verifactu.transport", "error", &note);
        let _ = self.store.fail(
            task_id,
            &task_error(error.code, error.message.clone(), true),
        );
        // Solo la red, el TLS y los fallos del servicio abren una incidencia (D282): un 401, un
        // 429 o una respuesta ilegible no son una incidencia técnica de remisión.
        if error.code == TRANSPORT_UNAVAILABLE {
            let _ = self
                .store
                .open_verifactu_incident(Some(batch_id), reason, now);
        }
        self.note_issue(error.code, error.message.clone());
        PumpReport::NoResponse {
            batch_id,
            code: error.code.to_owned(),
            message: error.message,
        }
    }

    /// Tercera fase: guarda lo que devolvió la llamada. Con respuesta o `Fault` cierra el lote
    /// (una sola vez, D252) y completa o falla la tarea; sin respuesta deja el lote abierto y la
    /// tarea fallida y reintentable, con el motivo visible; si el cuerpo no llegó a escribirse,
    /// anula el intento (D280).
    pub fn verifactu_pump_finish(&mut self, sent: VerifactuSent) -> AppResult<PumpReport> {
        let VerifactuSent {
            lease: _lease,
            task_id,
            batch_id,
            attempt_id,
            settings_id,
            started_ms,
            report,
            fingerprint,
        } = sent;
        // Sin hora fiable se usa la del inicio de la pasada: no se inventa otra.
        let now = self.remission_now_ms().unwrap_or(started_ms);
        self.remission.needs_certificate = false;
        if report.needs_tls12 {
            if let Some(fingerprint) = &fingerprint {
                if let Err(error) = self.store.remember_verifactu_tls12(fingerprint, now) {
                    let note = json!({ "code": error.code() });
                    let _ = self.store.record_note("verifactu.tls12", "error", &note);
                }
            }
        }
        let outcome = match report.outcome {
            SendOutcome::NotSent(reason) => {
                return Ok(self.finish_unsent(
                    task_id,
                    batch_id,
                    reason.code(),
                    Some(attempt_id),
                    now,
                ));
            }
            SendOutcome::SentNoResponse(reason) => {
                let error = transport_error(
                    TRANSPORT_UNAVAILABLE,
                    format!("sin respuesta completa del servicio ({})", reason.code()),
                );
                return Ok(self.finish_no_response(task_id, batch_id, error, reason.code(), now));
            }
            SendOutcome::Response { status, body } => {
                classify_response(status, &String::from_utf8_lossy(&body))
            }
        };
        match outcome {
            Err(error) => {
                let reason = error.code;
                Ok(self.finish_no_response(task_id, batch_id, error, reason, now))
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
                // Un `Fault` de autorización de un envío con la configuración anterior no vale
                // para la nueva: no se para (D281).
                if let ResultadoEnvio::Fallo(fault) = &result {
                    let stale = FaultClass::of(fault.codigo) == FaultClass::Authorization
                        && self
                            .store
                            .verifactu_remission_settings()
                            .map(|current| current.id != settings_id)
                            .unwrap_or(false);
                    if stale {
                        let _ = self
                            .store
                            .resume_verifactu_authorization_after_settings_change(
                                "configuracion_cambiada_durante_envio",
                                now,
                            );
                        let _ = self.store.fail(
                            task_id,
                            &task_error(CODE_SOAP_FAULT, fault.faultstring.clone(), true),
                        );
                        self.note_issue(CODE_SOAP_FAULT, fault.faultstring.clone());
                        self.pump_after_write();
                        return Ok(PumpReport::Fault {
                            batch_id,
                            code: fault.codigo,
                            description: fault.faultstring.clone(),
                        });
                    }
                }
                self.note_result_issue(&closure, &result);
                self.finish_submit_task(task_id, &closure, &result)
            }
        }
    }

    /// Un `Fault` o una respuesta sin ninguna línea para los registros enviados no dejan nada
    /// resuelto y se repetirían sin fin: se anotan como último error para que se vean. Una
    /// respuesta normal borra el error anterior.
    fn note_result_issue(&mut self, closure: &BatchClosure, result: &ResultadoEnvio) {
        match result {
            ResultadoEnvio::Fallo(f) => {
                let code = if FaultClass::of(f.codigo) == FaultClass::Authorization {
                    CODE_AUTHORIZATION_REJECTED
                } else {
                    CODE_SOAP_FAULT
                };
                self.note_issue(code, f.faultstring.clone());
            }
            ResultadoEnvio::Respuesta(r)
                if closure.accepted
                    + closure.accepted_with_errors
                    + closure.rejected
                    + closure.unexpected_duplicates
                    == 0
                    && !closure.missing_record_ids.is_empty() =>
            {
                self.note_issue(
                    CODE_EMPTY_RESPONSE,
                    format!(
                        "la respuesta ({:?}) no trae ninguna línea para los registros enviados; \
                         siguen pendientes",
                        r.estado_envio
                    ),
                );
            }
            ResultadoEnvio::Respuesta(_) => self.remission.last_error = None,
        }
    }

    fn finish_submit_task(
        &mut self,
        task_id: i64,
        closure: &BatchClosure,
        result: &ResultadoEnvio,
    ) -> AppResult<PumpReport> {
        match (closure.outcome, result) {
            (
                BatchOutcome::Fault,
                ResultadoEnvio::Fallo(FalloSoap {
                    faultstring,
                    codigo,
                    ..
                }),
            ) => {
                let description = faultstring.clone();
                if FaultClass::of(*codigo) == FaultClass::Authorization {
                    // La remisión de esta base queda parada (lo guardó el cierre del lote, D281).
                    self.store.fail(
                        task_id,
                        &task_error(CODE_AUTHORIZATION_REJECTED, description, false),
                    )?;
                    return Ok(PumpReport::AuthorizationRejected {
                        code: codigo.unwrap_or(0),
                    });
                }
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

    /// Cabecera del envío: el obligado a la emisión es el emisor del primer registro pendiente
    /// (NIF) con su razón social; el lote se corta ante otro emisor (`open_verifactu_batch`).
    /// Falla cerrado si falta la razón social.
    fn remission_header(&self, environment: VerifactuEnvironment) -> AppResult<Cabecera> {
        let pending = self.store.verifactu_pending_records(environment)?;
        let first = pending
            .first()
            .ok_or_else(|| sales_error(CODE_RECORD_INVALID, "no hay registros pendientes"))?;
        let nif = first.issuer_tax_id.clone();
        // La razón social vigente: la de la empresa si su NIF es el mismo; si no, la de la alta
        // más reciente de ese NIF (no la más antigua, que pudo cambiar). Sin ninguna, no se
        // inventa.
        let name = self
            .store
            .sales_company()
            .ok()
            .flatten()
            .filter(|c| c.tax_id.as_deref() == Some(nif.as_str()))
            .and_then(|c| c.name)
            .or_else(|| {
                self.store
                    .verifactu_records()
                    .ok()?
                    .into_iter()
                    .rev()
                    .find_map(|r| match r.record {
                        RegistroFacturacion::Alta(a) if a.id_factura.nif_emisor == nif => {
                            Some(a.nombre_razon_emisor)
                        }
                        _ => None,
                    })
            })
            .ok_or_else(|| {
                sales_error(
                    CODE_RECORD_INVALID,
                    "no consta la razón social del emisor para la cabecera del envío",
                )
            })?;
        let mut header = Cabecera::new(PersonaEs::new(&name, &nif));
        // El `Representante` se configura expresamente y nunca se deduce (D283).
        header.representante = self.store.verifactu_remission_settings()?.representative;
        Ok(header)
    }

    // --- Certificado y configuración de la remisión (D278, D283) ---

    /// Inyecta de dónde sale el certificado de remisión (el escritorio pone el de Windows, las
    /// pruebas uno sintético). Sin proveedor, la remisión a la AEAT da `certificate_missing`.
    pub fn set_certificate_provider(&mut self, provider: Arc<dyn CertificateProvider>) {
        self.remission.certificates = Some(provider);
    }

    /// Transporte que sustituye al TLS mutuo en preproducción y producción (solo depuración:
    /// los endpoints de la AEAT son constantes y nada los cambia, D279). `None` lo quita.
    #[cfg(debug_assertions)]
    pub fn set_verifactu_transport(&mut self, transport: Option<Arc<dyn SoapTransport>>) {
        self.remission.transport = transport;
    }

    /// Configuración vigente de la remisión de esta base: origen del certificado, calidad de
    /// quien remite y representante, si se configuró (D278, D283).
    pub fn verifactu_remission_settings(&self) -> AppResult<VerifactuRemissionSettingsView> {
        let settings = self.store.verifactu_remission_settings()?;
        let certificate = match settings.certificate_source.as_deref() {
            None => None,
            Some(json) => Some(
                serde_json::from_str::<CertificateSource>(json).map_err(|_| {
                    AppError::Sales(SalesStoreError::Corrupt(
                        "el origen del certificado guardado no es legible".into(),
                    ))
                })?,
            ),
        };
        Ok(VerifactuRemissionSettingsView {
            certificate,
            sender_capacity: settings.sender_capacity.as_str().to_owned(),
            representative: settings
                .representative
                .map(|person| VerifactuRepresentativeView {
                    name: person.nombre_razon,
                    nif: person.nif,
                }),
            changed_at: settings.changed_at,
        })
    }

    /// Cambia la configuración de la remisión (una fila nueva del historial) y devuelve si
    /// cambió algo. Cambiarla reanuda la remisión parada por autorización (D281) y pide una
    /// pasada. El representante se configura expresamente y nunca se deduce (D283); `None` lo
    /// quita. En el historial constan la calidad y si hay certificado y representante, nunca sus
    /// valores.
    pub fn set_verifactu_remission_settings(
        &mut self,
        certificate: Option<&CertificateSource>,
        sender_capacity: SenderCapacity,
        representative: Option<&PersonaEs>,
        authorization: &str,
    ) -> AppResult<bool> {
        let now = self.remission_now_ms()?;
        let json = certificate
            .map(serde_json::to_string)
            .transpose()
            .map_err(|_| {
                AppError::Sales(SalesStoreError::invalid(
                    "certificate",
                    "el origen del certificado no se puede guardar",
                ))
            })?;
        let changed = self.store.set_verifactu_remission_settings(
            json.as_deref(),
            sender_capacity,
            representative,
            authorization,
            now,
        )?;
        if changed {
            self.pump_after_write();
        }
        Ok(changed)
    }

    /// El usuario declara que ya arregló la autorización (nuevo certificado, apoderamiento o
    /// colaboración dados de alta) y la remisión parada por un `Fault` de autorización se
    /// reanuda (D281). Devuelve si estaba parada. Consta en el historial.
    pub fn verifactu_declare_authorization_fixed(&mut self) -> AppResult<bool> {
        let now = self.remission_now_ms()?;
        let resumed = self
            .store
            .resume_verifactu_authorization(AUTHORIZATION_DECLARATION, now)?;
        if resumed {
            self.pump_after_write();
        }
        Ok(resumed)
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
            let closed_by_fault = self
                .store
                .verifactu_batch_summary(batch_id)?
                .and_then(|b| b.result)
                .is_some_and(|r| r.outcome == BatchOutcome::Fault);
            if closed_by_fault {
                // Un `Fault` no se da por bueno al recuperar: la tarea queda fallida (D259).
                self.store.fail(
                    task.id,
                    &task_error(CODE_SOAP_FAULT, "el lote se cerró con un Fault SOAP", false),
                )?;
                continue;
            }
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
            // Un registro pendiente o en envío de otro entorno (p. ej. producción heredada) no
            // se remite desde este: no cuenta como pendiente y se muestra aparte.
            let status_text = if !in_environment
                && matches!(status, VerifactuStatus::Pending | VerifactuStatus::Sending)
            {
                OTHER_ENVIRONMENT.to_owned()
            } else {
                status.as_str().to_owned()
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
                status: status_text,
                aeat_code,
                aeat_description,
                duplicate,
                batch_id,
                subsanation_record_id,
                environment: stored.as_str().to_owned(),
            });
        }
        let pending_rows = self.store.verifactu_pending_records(environment)?;
        let pending = u32::try_from(pending_rows.len()).unwrap_or(u32::MAX);
        // El registro sin respuesta más antiguo (pendiente o en un lote abierto que aún no obtuvo
        // respuesta), por la hora con la que se generó (D281).
        let in_open_batch = self.store.unfinished_verifactu_batch()?.map(|b| {
            b.envio
                .registros
                .iter()
                .map(|r| r.fecha_hora_huso_gen_registro().unix())
                .min()
        });
        let oldest_pending_at = pending_rows
            .iter()
            .filter_map(|row| row.generated_at.parse::<FechaHoraHuso>().ok())
            .map(|at| at.unix())
            .chain(in_open_batch.flatten())
            .min()
            .map(|seconds| seconds.saturating_mul(1000));
        let oldest_pending_age_seconds = match (oldest_pending_at, self.remission_now_ms()) {
            (Some(at), Ok(now)) => Some(u64::try_from((now - at).max(0) / 1000).unwrap_or(0)),
            _ => None,
        };
        let halt = self.store.verifactu_authorization_halt()?;
        let stored_unsent =
            self.store
                .verifactu_last_unsent_attempt()?
                .map(|unsent| VerifactuUnsentView {
                    batch_id: unsent.batch_id,
                    code: unsent.reason_code,
                    at: unsent.attempted_at,
                });
        // El último sin lote (solo en memoria) manda si es más reciente que el guardado.
        let last_unsent = match (stored_unsent, self.remission.unbatched.clone()) {
            (Some(stored), Some(unbatched)) if stored.at > unbatched.at => Some(stored),
            (_, Some(unbatched)) => Some(unbatched),
            (stored, None) => stored,
        };
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
            last_error: self
                .remission
                .last_error
                .as_ref()
                .map(|e| VerifactuIssueView {
                    code: e.code.clone(),
                    message: e.message.clone(),
                    at: e.at,
                    occurrences: e.occurrences,
                }),
            oldest_pending_at,
            oldest_pending_age_seconds,
            incident_open: self.store.verifactu_incident_open()?,
            last_unsent,
            authorization_rejected: halt.is_some(),
            authorization_code: halt.map(|h| h.error_code),
            needs_certificate: self.remission.needs_certificate,
        })
    }
}

/// Texto fijo de un intento sin envío, sin NIF, nombre ni texto del sistema (D278, D280).
fn unsent_message(code: &str) -> String {
    match code {
        "certificate_missing" => "No hay un certificado vinculado para remitir a la AEAT.",
        "certificate_expired" => "El certificado de remisión está caducado o aún no es válido.",
        "certificate_needs_user" => {
            "El certificado necesita su intervención (PIN, tarjeta o confirmación): pulse «Remitir ahora»."
        }
        "certificate_key_unsupported" => "La clave del certificado no es admitida por el servicio.",
        "certificate_rejected" => "El certificado no es apto o fue rechazado.",
        "certificate_invalid" => "El certificado vinculado no es un X.509 válido.",
        "network_disabled" => {
            "Esta compilación no incluye la red real de la AEAT: el registro queda pendiente."
        }
        "dns_failed" | "connect_failed" | "connect_timeout" => {
            "No se pudo conectar con el servicio: el registro queda pendiente."
        }
        "server_certificate_invalid" => "El certificado del servidor no es de confianza.",
        "tls_handshake_failed" | "tls_timeout" => {
            "No se pudo establecer la conexión segura: el registro queda pendiente."
        }
        "settings_changed" => {
            "La configuración de remisión cambió durante el envío: se vuelve a intentar con la nueva."
        }
        "wrong_environment" => "El transporte no corresponde al entorno de la remisión.",
        _ => "El envío no llegó a salir: el registro queda pendiente.",
    }
    .to_owned()
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
        assert_eq!(
            SimMode::parse("fault-autorizacion"),
            Some(SimMode::AuthorizationFault)
        );
        assert_eq!(
            SimMode::parse("fault-tecnico"),
            Some(SimMode::TechnicalFault)
        );
        assert_eq!(SimMode::parse("cualquier-cosa"), None);
    }

    #[test]
    fn http_urls_split_into_authority_and_path() {
        assert_eq!(
            split_http_url("http://127.0.0.1:8080/a/b"),
            Some(("127.0.0.1:8080", "/a/b".to_owned()))
        );
        assert_eq!(
            split_http_url("http://localhost:1"),
            Some(("localhost:1", "/".to_owned()))
        );
        assert_eq!(split_http_url("https://127.0.0.1/x"), None);
        assert_eq!(split_http_url("http:///x"), None);
    }

    #[test]
    fn unsent_messages_never_carry_system_text() {
        for code in [
            "certificate_missing",
            "certificate_needs_user",
            "network_disabled",
            "connect_failed",
            "tls_handshake_failed",
            "algo_desconocido",
        ] {
            let text = unsent_message(code);
            assert!(!text.is_empty() && !text.contains(code), "{code}: {text}");
        }
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
