//! Transporte real: TCP, TLS mutuo con `rustls` (backend `ring`, nunca `aws-lc-rs`) y un único
//! `POST` HTTP/1.1 propio sobre `rustls::StreamOwned` (D279).
//!
//! - El SNI y la verificación del nombre del servidor están siempre activos; no hay ninguna
//!   opción para desactivarlos.
//! - Caída a TLS 1.2 (D279): si el servidor pide certificado y la clave no puede firmar con
//!   ninguno de los esquemas que ofrece (TLS 1.3 solo admite PSS para RSA; una tarjeta puede
//!   limitarse a PKCS#1 v1.5), el saludo se repite una vez con un `ClientConfig` limitado a TLS
//!   1.2 y el informe lo indica (`needs_tls12`). Ocurre antes de escribir el cuerpo.
//! - **Frontera «no escrito»:** el cuerpo se escribe en el socket solo cuando el saludo ha
//!   terminado. En TLS 1.3 el saludo del cliente termina antes de que el servidor valide su
//!   certificado; por eso, tras el saludo, se escucha una breve gracia ([`POST_HANDSHAKE_GRACE`])
//!   por si llega la alerta de rechazo, que sigue siendo «no enviado». Una alerta de
//!   certificado posterior (el servidor la manda más tarde) también se clasifica como no
//!   enviado, porque ningún servidor responde a una petición con una alerta de ese tipo.
//! - La resolución DNS usa la del sistema y no tiene límite propio; el resto (conexión,
//!   saludo, escritura y lectura) comparte un plazo total.

use std::io::{self, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rustls::client::ResolvesClientCert;
use rustls::pki_types::ServerName;
use rustls::sign::CertifiedKey;
use rustls::{
    AlertDescription, ClientConfig, ClientConnection, RootCertStore, SignatureScheme, StreamOwned,
};

use super::http::{build_request, read_response, ReadFailure};
use super::{
    AeatEnvironment, ClientCredentials, Endpoint, NoResponseReason, NotSentReason, SendOutcome,
    SoapTransport, TlsPreference, TransportInitError, TransportReport,
};

/// Tamaño máximo del cuerpo de la respuesta.
pub const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
/// Escucha tras el saludo TLS 1.3 por si llega el rechazo del certificado.
const POST_HANDSHAKE_GRACE: Duration = Duration::from_millis(100);

/// Tiempos límite, coherentes con los de la remisión actual (`CONNECT_TIMEOUT` 3 s y
/// `REQUEST_TIMEOUT` 10 s de `crates/app/src/remission.rs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timeouts {
    /// Límite de cada conexión TCP.
    pub connect: Duration,
    /// Plazo total desde que empieza la conexión hasta leer la respuesta.
    pub request: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        Self {
            connect: Duration::from_secs(3),
            request: Duration::from_secs(10),
        }
    }
}

/// Destino inyectado solo para pruebas (característica `test-support`): una dirección local, el
/// nombre que debe presentar el servidor y la raíz de confianza sintética (DER).
#[cfg(feature = "test-support")]
#[derive(Debug, Clone)]
pub struct TestTarget {
    pub addr: std::net::SocketAddr,
    pub server_name: String,
    pub roots_der: Vec<Vec<u8>>,
}

enum Target {
    /// Solo se construye con `aeat-red` (D285).
    #[cfg_attr(not(feature = "aeat-red"), allow(dead_code))]
    Aeat {
        environment: AeatEnvironment,
        roots: Arc<RootCertStore>,
    },
    #[cfg(feature = "test-support")]
    Test {
        target: TestTarget,
        roots: Arc<RootCertStore>,
    },
}

pub struct MutualTlsTransport {
    target: Target,
    timeouts: Timeouts,
    max_response: usize,
}

impl std::fmt::Debug for MutualTlsTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MutualTlsTransport")
            .field("timeouts", &self.timeouts)
            .finish_non_exhaustive()
    }
}

impl MutualTlsTransport {
    /// Transporte hacia la AEAT real del entorno dado, con las raíces de `webpki-roots`. Sin la
    /// característica `aeat-red` devuelve `network_disabled` y no abre ningún socket (D285).
    #[cfg(feature = "aeat-red")]
    pub fn aeat(environment: AeatEnvironment) -> Result<Self, TransportInitError> {
        let roots = RootCertStore {
            roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
        };
        Ok(Self {
            target: Target::Aeat {
                environment,
                roots: Arc::new(roots),
            },
            timeouts: Timeouts::default(),
            max_response: MAX_RESPONSE_BYTES,
        })
    }

    #[cfg(not(feature = "aeat-red"))]
    pub fn aeat(_environment: AeatEnvironment) -> Result<Self, TransportInitError> {
        Err(TransportInitError::NetworkDisabled)
    }

    /// Transporte de pruebas contra un destino inyectado (no se expone al escritorio).
    #[cfg(feature = "test-support")]
    pub fn for_test(target: TestTarget) -> Self {
        let mut roots = RootCertStore::empty();
        for der in &target.roots_der {
            let _ = roots.add(rustls::pki_types::CertificateDer::from(der.clone()));
        }
        Self {
            target: Target::Test {
                target,
                roots: Arc::new(roots),
            },
            timeouts: Timeouts::default(),
            max_response: MAX_RESPONSE_BYTES,
        }
    }

    pub fn with_timeouts(mut self, timeouts: Timeouts) -> Self {
        self.timeouts = timeouts;
        self
    }

    pub fn with_max_response_bytes(mut self, max: usize) -> Self {
        self.max_response = max;
        self
    }

    fn roots(&self) -> &Arc<RootCertStore> {
        match &self.target {
            Target::Aeat { roots, .. } => roots,
            #[cfg(feature = "test-support")]
            Target::Test { roots, .. } => roots,
        }
    }

    /// Nombre del servidor para el SNI, la verificación y la cabecera `Host`.
    fn server_name(&self, endpoint: Endpoint) -> String {
        match &self.target {
            Target::Aeat { .. } => endpoint.host().to_string(),
            #[cfg(feature = "test-support")]
            Target::Test { target, .. } => target.server_name.clone(),
        }
    }

    fn connect(&self, endpoint: Endpoint, deadline: Instant) -> Result<TcpStream, NotSentReason> {
        let addrs: Vec<std::net::SocketAddr> = match &self.target {
            Target::Aeat { .. } => (endpoint.host(), 443)
                .to_socket_addrs()
                .map_err(|_| NotSentReason::Dns)?
                .collect(),
            #[cfg(feature = "test-support")]
            Target::Test { target, .. } => vec![target.addr],
        };
        if addrs.is_empty() {
            return Err(NotSentReason::Dns);
        }
        let mut timed_out = false;
        for addr in addrs {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(NotSentReason::ConnectTimeout);
            }
            match TcpStream::connect_timeout(&addr, remaining.min(self.timeouts.connect)) {
                Ok(sock) => {
                    let _ = sock.set_nodelay(true);
                    return Ok(sock);
                }
                Err(e) => timed_out = is_timeout(&e),
            }
        }
        Err(if timed_out {
            NotSentReason::ConnectTimeout
        } else {
            NotSentReason::Connect
        })
    }

    fn config(
        &self,
        credentials: &ClientCredentials,
        only_tls12: bool,
        unsupported: &Arc<AtomicBool>,
    ) -> Option<Arc<ClientConfig>> {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let versions: &[&rustls::SupportedProtocolVersion] = if only_tls12 {
            &[&rustls::version::TLS12]
        } else {
            &[&rustls::version::TLS13, &rustls::version::TLS12]
        };
        let resolver = Resolver {
            key: (!credentials.chain.is_empty()).then(|| {
                Arc::new(CertifiedKey::new(
                    credentials.chain.clone(),
                    credentials.key.clone(),
                ))
            }),
            unsupported: unsupported.clone(),
        };
        let config = ClientConfig::builder_with_provider(provider)
            .with_protocol_versions(versions)
            .ok()?
            .with_root_certificates(self.roots().clone())
            .with_client_cert_resolver(Arc::new(resolver));
        Some(Arc::new(config))
    }

    fn attempt(
        &self,
        endpoint: Endpoint,
        body: &[u8],
        credentials: &ClientCredentials,
        only_tls12: bool,
        deadline: Instant,
    ) -> Attempt {
        let not_sent = |reason| Attempt::Done {
            outcome: SendOutcome::NotSent(reason),
            handshake_ok: false,
        };
        let empty = credentials.chain.is_empty();
        let host = self.server_name(endpoint);
        let Ok(name) = ServerName::try_from(host.clone()) else {
            return not_sent(NotSentReason::ServerCertificate);
        };
        let sock = match self.connect(endpoint, deadline) {
            Ok(s) => s,
            Err(reason) => return not_sent(reason),
        };
        let unsupported = Arc::new(AtomicBool::new(false));
        let Some(config) = self.config(credentials, only_tls12, &unsupported) else {
            return not_sent(NotSentReason::TlsHandshake);
        };
        let Ok(mut conn) = ClientConnection::new(config, name) else {
            return not_sent(NotSentReason::TlsHandshake);
        };
        let mut sock = TimedStream { sock, deadline };

        // Saludo TLS completo antes de escribir nada.
        let handshake = (|| -> io::Result<()> {
            while conn.is_handshaking() {
                conn.complete_io(&mut sock)?;
            }
            Ok(())
        })();
        if let Err(e) = handshake {
            let reason = handshake_reason(&e, empty);
            if unsupported.load(Ordering::SeqCst) && reason != NotSentReason::ServerCertificate {
                return Attempt::KeyCannotSign;
            }
            return not_sent(reason);
        }
        if unsupported.load(Ordering::SeqCst) {
            // El servidor pidió certificado y no se pudo firmar: no se envió ninguno.
            return Attempt::KeyCannotSign;
        }

        // TLS 1.3: el cliente termina el saludo antes de que el servidor valide su certificado.
        if conn.protocol_version() == Some(rustls::ProtocolVersion::TLSv1_3) {
            sock.deadline = deadline.min(Instant::now() + POST_HANDSHAKE_GRACE);
            let probed = probe(&mut conn, &mut sock, empty);
            sock.deadline = deadline;
            if let Err(reason) = probed {
                return Attempt::Done {
                    outcome: SendOutcome::NotSent(reason),
                    handshake_ok: true,
                };
            }
        }

        let mut stream = StreamOwned::new(conn, sock);
        let request = build_request(&host, endpoint.path(), body);
        if let Err(e) = stream.write_all(&request).and_then(|()| stream.flush()) {
            return Attempt::Done {
                outcome: write_outcome(&e, empty),
                handshake_ok: true,
            };
        }
        let outcome = match read_response(&mut stream, self.max_response) {
            Ok((status, body)) => SendOutcome::Response { status, body },
            Err(ReadFailure::Eof) => {
                SendOutcome::SentNoResponse(NoResponseReason::ConnectionClosed)
            }
            Err(ReadFailure::TooLarge) => {
                SendOutcome::SentNoResponse(NoResponseReason::ResponseTooLarge)
            }
            Err(ReadFailure::Invalid) => {
                SendOutcome::SentNoResponse(NoResponseReason::ResponseInvalid)
            }
            Err(ReadFailure::Io(e)) => read_error_outcome(&e, empty),
        };
        // Cierre ordenado, sin importar si falla.
        stream.conn.send_close_notify();
        let _ = stream.conn.write_tls(&mut stream.sock);
        Attempt::Done {
            outcome,
            handshake_ok: true,
        }
    }
}

enum Attempt {
    Done {
        outcome: SendOutcome,
        handshake_ok: bool,
    },
    /// El servidor pidió certificado y la clave no puede firmar con lo que se ofreció.
    KeyCannotSign,
}

impl SoapTransport for MutualTlsTransport {
    fn send(
        &self,
        endpoint: Endpoint,
        body: &[u8],
        credentials: &ClientCredentials,
        tls: TlsPreference,
    ) -> TransportReport {
        #[allow(irrefutable_let_patterns)]
        if let Target::Aeat { environment, .. } = &self.target {
            if *environment != endpoint.environment {
                return TransportReport {
                    outcome: SendOutcome::NotSent(NotSentReason::WrongEnvironment),
                    needs_tls12: false,
                };
            }
        }
        let deadline = Instant::now() + self.timeouts.request;
        let mut only_tls12 = tls == TlsPreference::Tls12;
        loop {
            match self.attempt(endpoint, body, credentials, only_tls12, deadline) {
                Attempt::Done {
                    outcome,
                    handshake_ok,
                } => {
                    return TransportReport {
                        outcome,
                        needs_tls12: only_tls12 && handshake_ok,
                    }
                }
                // Una sola repetición, con TLS 1.2 (D279).
                Attempt::KeyCannotSign if !only_tls12 => only_tls12 = true,
                Attempt::KeyCannotSign => {
                    return TransportReport {
                        outcome: SendOutcome::NotSent(NotSentReason::KeyUnsupported),
                        needs_tls12: false,
                    }
                }
            }
        }
    }
}

/// Entrega el certificado si la clave puede firmar con algún esquema del servidor; si no, lo
/// anota y no ofrece ninguno (el servidor que lo exija rechaza el saludo).
#[derive(Debug)]
struct Resolver {
    key: Option<Arc<CertifiedKey>>,
    unsupported: Arc<AtomicBool>,
}

impl ResolvesClientCert for Resolver {
    fn resolve(
        &self,
        _root_hint_subjects: &[&[u8]],
        sigschemes: &[SignatureScheme],
    ) -> Option<Arc<CertifiedKey>> {
        let key = self.key.as_ref()?;
        if key.key.choose_scheme(sigschemes).is_none() {
            self.unsupported.store(true, Ordering::SeqCst);
            return None;
        }
        Some(key.clone())
    }

    fn has_certs(&self) -> bool {
        self.key.is_some()
    }
}

/// Socket con un plazo absoluto: cada operación usa lo que queda.
struct TimedStream {
    sock: TcpStream,
    deadline: Instant,
}

impl TimedStream {
    fn remaining(&self) -> io::Result<Duration> {
        let left = self.deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            Err(io::Error::from(io::ErrorKind::TimedOut))
        } else {
            Ok(left)
        }
    }
}

impl io::Read for TimedStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.sock.set_read_timeout(Some(self.remaining()?))?;
        self.sock.read(buf)
    }
}

impl io::Write for TimedStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.sock.set_write_timeout(Some(self.remaining()?))?;
        self.sock.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.sock.flush()
    }
}

fn is_timeout(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
    )
}

fn tls_error_of(e: &io::Error) -> Option<&rustls::Error> {
    e.get_ref()?.downcast_ref::<rustls::Error>()
}

/// Traduce un error TLS a un motivo de «no enviado».
fn reason_from_tls_error(e: &rustls::Error, empty: bool) -> NotSentReason {
    match e {
        rustls::Error::InvalidCertificate(_) => NotSentReason::ServerCertificate,
        rustls::Error::AlertReceived(alert) => alert_reason(*alert, empty),
        _ => NotSentReason::TlsHandshake,
    }
}

/// Alertas con las que un servidor rechaza el certificado del cliente. Sin certificado
/// entregado (`empty`), cualquiera de ellas y `handshake_failure` significan «falta».
fn alert_reason(alert: AlertDescription, empty: bool) -> NotSentReason {
    use AlertDescription as A;
    match alert {
        A::CertificateRequired | A::NoCertificate => NotSentReason::CertificateMissing,
        A::BadCertificate
        | A::UnsupportedCertificate
        | A::CertificateRevoked
        | A::CertificateExpired
        | A::CertificateUnknown
        | A::UnknownCA
        | A::AccessDenied
        | A::DecryptError => {
            if empty {
                NotSentReason::CertificateMissing
            } else {
                NotSentReason::CertificateRejected
            }
        }
        A::HandshakeFailure if empty => NotSentReason::CertificateMissing,
        _ => NotSentReason::TlsHandshake,
    }
}

fn is_certificate_alert(e: &io::Error) -> Option<AlertDescription> {
    match tls_error_of(e) {
        Some(rustls::Error::AlertReceived(a)) => {
            use AlertDescription as A;
            matches!(
                a,
                A::CertificateRequired
                    | A::NoCertificate
                    | A::BadCertificate
                    | A::UnsupportedCertificate
                    | A::CertificateRevoked
                    | A::CertificateExpired
                    | A::CertificateUnknown
                    | A::UnknownCA
                    | A::AccessDenied
            )
            .then_some(*a)
        }
        _ => None,
    }
}

fn handshake_reason(e: &io::Error, empty: bool) -> NotSentReason {
    if is_timeout(e) {
        return NotSentReason::HandshakeTimeout;
    }
    match tls_error_of(e) {
        Some(t) => reason_from_tls_error(t, empty),
        None => NotSentReason::TlsHandshake,
    }
}

/// Escucha lo que el servidor mande justo tras el saludo TLS 1.3 (tiques de sesión o una alerta
/// de rechazo) hasta que se agota la gracia.
fn probe(
    conn: &mut ClientConnection,
    sock: &mut TimedStream,
    empty: bool,
) -> Result<(), NotSentReason> {
    loop {
        match conn.read_tls(sock) {
            Ok(0) => return Err(NotSentReason::TlsHandshake),
            Ok(_) => {
                if let Err(e) = conn.process_new_packets() {
                    return Err(reason_from_tls_error(&e, empty));
                }
            }
            Err(e) if is_timeout(&e) => return Ok(()),
            Err(e) => return Err(handshake_reason(&e, empty)),
        }
    }
}

/// Error al escribir la petición. Una alerta de certificado del servidor sigue siendo «no
/// enviado»: el servidor rechazó el saludo y nunca vio la petición.
fn write_outcome(e: &io::Error, empty: bool) -> SendOutcome {
    if let Some(alert) = is_certificate_alert(e) {
        return SendOutcome::NotSent(alert_reason(alert, empty));
    }
    if is_timeout(e) {
        SendOutcome::SentNoResponse(NoResponseReason::WriteTimeout)
    } else {
        SendOutcome::SentNoResponse(NoResponseReason::WriteFailed)
    }
}

fn read_error_outcome(e: &io::Error, empty: bool) -> SendOutcome {
    if let Some(alert) = is_certificate_alert(e) {
        return SendOutcome::NotSent(alert_reason(alert, empty));
    }
    if is_timeout(e) {
        SendOutcome::SentNoResponse(NoResponseReason::ReadTimeout)
    } else {
        SendOutcome::SentNoResponse(NoResponseReason::ConnectionClosed)
    }
}
