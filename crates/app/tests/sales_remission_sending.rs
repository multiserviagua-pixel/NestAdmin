//! Envío de la remisión Verifactu con certificado (T18b-D1, D278–D283; D197): orden de la bomba
//! (certificado antes de anotar el intento), intentos sin envío que no cuentan (D50, D252,
//! D280), clasificación de los `Fault` (D281), espera creciente hasta una hora, `Incidencia` y
//! `Representante`. El transporte y el certificado son sintéticos (un guion de respuestas y claves
//! de `rcgen`) y el simulador de la AEAT corre en 127.0.0.1: nada contacta con la AEAT. La espera
//! se avanza con el reloj inyectable de la aplicación. Solo datos sintéticos.

#![cfg(debug_assertions)]

use std::collections::VecDeque;
use std::path::Path;
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use nestadmin_ai::{AiError, GenerateRequest, GenerateResult, Provider, ProviderKind};
use nestadmin_app::remission::{
    CertificateProvider, PumpReport, SenderCredentials, SimMode, SimulatorTransport,
    VerifactuStatusView, AUTHORIZATION_DECLARATION, SUBMIT_VERIFACTU,
};
use nestadmin_app::sales::SalesClock;
use nestadmin_app::{App, AppPaths};
use nestadmin_certs::{
    AeatEnvironment, CertError, CertificateKind, CertificateSource, Channel, ClientCredentials,
    Endpoint, NoResponseReason, NotSentReason, SendOutcome, SoapTransport, TlsPreference,
    TransportReport,
};
use nestadmin_core::sales::{
    retry_wait_seconds, Company, CustomerData, DraftInput, SenderCapacity, VerifactuStatus,
    MAX_RETRY_WAIT_SECONDS,
};
use nestadmin_core::TaskState;
use nestadmin_extract::{ExtractError, Extractor, Ocr};
use nestadmin_sales::{DocumentKind, Product, TaxRate};
use nestadmin_verifactu::simulador::{ConfigSimulador, SimuladorAeat};
use nestadmin_verifactu::{Fecha, FechaHoraHuso, PersonaEs};
use rust_decimal::Decimal;

struct NoAi;

impl Provider for NoAi {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Ollama
    }
    fn model(&self) -> &str {
        "simulado"
    }
    fn generate(&self, _request: &GenerateRequest) -> Result<GenerateResult, AiError> {
        Err(AiError::Unavailable("la remisión no usa IA".into()))
    }
}

struct NoOcr;

impl Ocr for NoOcr {
    fn recognize(&self, _image: &Path, page: usize) -> Result<String, ExtractError> {
        Err(ExtractError::OcrUnavailable(format!("página {page}")))
    }
}

/// Reloj de pruebas: segundos Unix desplazables (huso +01:00). Es el que gobierna la espera.
struct TestClock(AtomicI64);

impl TestClock {
    fn new() -> Arc<Self> {
        let base = FechaHoraHuso::new(Fecha::new(2026, 3, 1).unwrap(), 10, 0, 0, 60)
            .unwrap()
            .unix();
        Arc::new(Self(AtomicI64::new(base)))
    }
    fn advance(&self, seconds: i64) {
        self.0.fetch_add(seconds, Ordering::SeqCst);
    }
}

impl SalesClock for TestClock {
    fn now(&self) -> FechaHoraHuso {
        FechaHoraHuso::desde_unix(self.0.load(Ordering::SeqCst), 60).unwrap()
    }
}

fn d(text: &str) -> Decimal {
    text.parse().unwrap()
}

// --- Certificado y transporte sintéticos ---

/// Credenciales con una clave `rcgen` sin ningún valor: nada de un certificado real.
fn synthetic_credentials(kind: CertificateKind, fingerprint: &str) -> SenderCredentials {
    use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};
    let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
    let cert = rcgen::CertificateParams::new(vec!["remision.example.invalid".to_owned()])
        .unwrap()
        .self_signed(&key)
        .unwrap();
    let der = PrivateKeyDer::from(PrivatePkcs8KeyDer::from(key.serialize_der()));
    let signing = rustls::crypto::ring::sign::any_supported_type(&der).unwrap();
    SenderCredentials {
        credentials: ClientCredentials::new(vec![cert.der().to_vec()], signing),
        kind,
        fingerprint: fingerprint.to_owned(),
    }
}

/// Proveedor con guion: cada llamada saca el siguiente resultado (vacío: `Ok`).
struct FakeCerts {
    script: Mutex<VecDeque<Result<(), CertError>>>,
    kind: CertificateKind,
    silent_calls: AtomicUsize,
    interactive_calls: AtomicUsize,
}

impl FakeCerts {
    fn new(script: Vec<Result<(), CertError>>) -> Arc<Self> {
        Arc::new(Self {
            script: Mutex::new(script.into()),
            kind: CertificateKind::PersonaFisica,
            silent_calls: AtomicUsize::new(0),
            interactive_calls: AtomicUsize::new(0),
        })
    }
    fn with_kind(kind: CertificateKind) -> Arc<Self> {
        Arc::new(Self {
            script: Mutex::new(VecDeque::new()),
            kind,
            silent_calls: AtomicUsize::new(0),
            interactive_calls: AtomicUsize::new(0),
        })
    }
    fn calls(&self) -> (usize, usize) {
        (
            self.silent_calls.load(Ordering::SeqCst),
            self.interactive_calls.load(Ordering::SeqCst),
        )
    }
}

impl CertificateProvider for FakeCerts {
    fn acquire(
        &self,
        _source: &CertificateSource,
        interactive: bool,
        _now_ms: i64,
    ) -> Result<SenderCredentials, CertError> {
        if interactive {
            self.interactive_calls.fetch_add(1, Ordering::SeqCst);
        } else {
            self.silent_calls.fetch_add(1, Ordering::SeqCst);
        }
        let next = self.script.lock().unwrap().pop_front().unwrap_or(Ok(()));
        next.map(|()| synthetic_credentials(self.kind, &"ab".repeat(32)))
    }
}

/// Qué hace el transporte de pruebas en cada envío.
#[derive(Clone)]
enum Step {
    NotSent(NotSentReason),
    NoResponse(NoResponseReason),
    /// Lo envía al simulador de la AEAT (HTTP plano en 127.0.0.1) y devuelve su respuesta.
    Simulator,
    /// Como `Simulator`, pero indica que el saludo solo funcionó con TLS 1.2.
    SimulatorTls12,
}

#[derive(Debug, Clone)]
struct Call {
    endpoint: Endpoint,
    tls: TlsPreference,
    body: String,
}

/// Transporte con guion que sustituye al TLS mutuo (solo depuración).
struct ScriptedTransport {
    steps: Mutex<VecDeque<Step>>,
    calls: Mutex<Vec<Call>>,
    sim_url: String,
}

impl ScriptedTransport {
    fn new(sim: &SimuladorAeat, steps: Vec<Step>) -> Arc<Self> {
        Arc::new(Self {
            steps: Mutex::new(steps.into()),
            calls: Mutex::new(Vec::new()),
            sim_url: sim.url(),
        })
    }
    fn calls(&self) -> Vec<Call> {
        self.calls.lock().unwrap().clone()
    }
}

impl SoapTransport for ScriptedTransport {
    fn send(
        &self,
        endpoint: Endpoint,
        body: &[u8],
        _credentials: &ClientCredentials,
        tls: TlsPreference,
    ) -> TransportReport {
        self.calls.lock().unwrap().push(Call {
            endpoint,
            tls,
            body: String::from_utf8_lossy(body).into_owned(),
        });
        let step = self
            .steps
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Step::Simulator);
        let report = |outcome| TransportReport {
            outcome,
            needs_tls12: false,
        };
        match step {
            Step::NotSent(reason) => report(SendOutcome::NotSent(reason)),
            Step::NoResponse(reason) => report(SendOutcome::SentNoResponse(reason)),
            Step::Simulator => SimulatorTransport::new(self.sim_url.clone()).post(body),
            Step::SimulatorTls12 => {
                let mut sent = SimulatorTransport::new(self.sim_url.clone()).post(body);
                sent.needs_tls12 = true;
                sent
            }
        }
    }
}

// --- Banco de pruebas ---

struct Harness {
    dir: tempfile::TempDir,
    clock: Arc<TestClock>,
    app: App,
    customer: i64,
    product: i64,
}

fn open_app(dir: &Path, clock: &Arc<TestClock>) -> App {
    let extractor = Extractor::new(dir.join("sin-pdfium"), Box::new(NoOcr));
    let mut app = App::open(&AppPaths::new(dir), Arc::new(NoAi), Arc::new(extractor)).unwrap();
    app.set_sales_clock(clock.clone());
    app.set_verifactu_producer("Productor Demo SL", "B00000034")
        .unwrap();
    app
}

impl Harness {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let clock = TestClock::new();
        let mut app = open_app(dir.path(), &clock);
        app.set_sales_company(&Company {
            name: Some("Taller Demo SL".into()),
            tax_id: Some("B12345674".into()),
            address: Some("Calle Ficticia 1, 28000 Madrid".into()),
            email: Some("demo@example.test".into()),
            iban: None,
            logo: None,
            logo_mime: None,
        })
        .unwrap();
        let customer = app
            .create_customer(&CustomerData {
                name: "Cliente Uno SA".into(),
                tax_id: Some("12345678Z".into()),
                address: Some("Avenida Inventada 2, 08000 Barcelona".into()),
                email: None,
            })
            .unwrap();
        let product = app
            .create_product(&Product {
                id: 0,
                description: "Revisión anual".into(),
                unit_price: d("80"),
                tax_rate: TaxRate::TwentyOne,
                exemption: None,
                retired: false,
            })
            .unwrap();
        Self {
            dir,
            clock,
            app,
            customer,
            product,
        }
    }

    /// Un entorno de remisión con certificado: la configuración de D278 y el entorno vigente se
    /// fijan directamente en la base (la aplicación solo deja elegir el simulador hasta D2).
    fn with_environment(environment: &str) -> Self {
        let h = Self::new();
        h.raw()
            .execute(
                "INSERT INTO verifactu_environment_changes (environment, authorization, \
                 changed_at) VALUES (?1, 'prueba', 0)",
                [environment],
            )
            .unwrap();
        h
    }

    /// Un entorno de pruebas con certificado, transporte con guion y simulador propio.
    fn certified(
        steps: Vec<Step>,
        certs: Arc<FakeCerts>,
    ) -> (Self, Arc<ScriptedTransport>, Arc<SimuladorAeat>) {
        let mut h = Self::with_environment("preproduccion");
        let sim = Arc::new(SimuladorAeat::iniciar(ConfigSimulador::default()).unwrap());
        let transport = ScriptedTransport::new(&sim, steps);
        h.app.set_verifactu_transport(Some(transport.clone()));
        h.app.set_certificate_provider(certs);
        h.app
            .set_verifactu_remission_settings(
                Some(&CertificateSource::Pfx {
                    reference: "00112233445566778899aabbccddeeff".into(),
                }),
                SenderCapacity::Taxpayer,
                None,
                "usuario_configuracion",
            )
            .unwrap();
        (h, transport, sim)
    }

    fn emit(&mut self) -> i64 {
        let mut input = DraftInput::new(DocumentKind::Invoice);
        input.customer_id = Some(self.customer);
        let id = self.app.create_sales_draft(&input).unwrap();
        self.app
            .add_sales_line_from_product(id, self.product, d("1"), Decimal::ZERO)
            .unwrap();
        self.app.issue_sales_document(id, None).unwrap();
        id
    }

    fn pump(&mut self) -> PumpReport {
        self.app.verifactu_pump(false).unwrap()
    }

    fn pump_now(&mut self) -> PumpReport {
        self.app.verifactu_pump(true).unwrap()
    }

    /// Avanza el reloj pasada la espera de 60 s.
    fn wait(&self) {
        self.clock.advance(61);
    }

    fn raw(&self) -> rusqlite::Connection {
        rusqlite::Connection::open(self.dir.path().join("nestadmin.db")).unwrap()
    }

    fn count(&self, sql: &str) -> i64 {
        self.raw().query_row(sql, [], |r| r.get(0)).unwrap()
    }

    fn status(&self) -> VerifactuStatusView {
        self.app.verifactu_status().unwrap()
    }

    fn view_status(&self, record: i64) -> String {
        self.status()
            .records
            .into_iter()
            .find(|r| r.record_id == record)
            .unwrap()
            .status
    }

    fn first_record(&self) -> i64 {
        self.app.verifactu_records().unwrap()[0].id
    }

    fn task_error_code(&self) -> String {
        self.app
            .tasks()
            .unwrap()
            .into_iter()
            .rfind(|t| t.kind == SUBMIT_VERIFACTU)
            .unwrap()
            .error
            .unwrap()
            .code
    }

    fn requests(&mut self) -> Vec<String> {
        self.app
            .verifactu_simulator()
            .unwrap()
            .estado()
            .peticiones
            .clone()
    }
}

fn batch_counts(h: &Harness) -> (i64, i64, i64) {
    (
        h.count("SELECT count(*) FROM verifactu_batch_attempts"),
        h.count("SELECT count(*) FROM verifactu_unsent_attempts"),
        h.count("SELECT count(*) FROM verifactu_unsent_attempts WHERE attempt_id IS NOT NULL"),
    )
}

// --- Certificado antes de `begin` (D280) ---

#[test]
fn a_certificate_that_needs_the_user_leaves_no_attempt_and_a_foreign_3000_is_not_ours() {
    let certs = FakeCerts::new(vec![Err(CertError::NeedsUser), Ok(())]);
    let (mut h, transport, sim) = Harness::certified(vec![Step::Simulator], certs.clone());
    h.emit();
    let alta = h.first_record();

    // El worker (no manual) obtiene la clave en silencio: pide al usuario y no anota nada.
    assert_eq!(
        h.pump(),
        PumpReport::NeedsUser { batch_id: 1 },
        "necesita su certificado"
    );
    assert_eq!(certs.calls(), (1, 0), "el worker nunca abre un diálogo");
    assert!(transport.calls().is_empty(), "no se llamó al transporte");
    assert_eq!(
        batch_counts(&h),
        (0, 1, 0),
        "ningún intento, un intento sin envío"
    );
    let status = h.status();
    assert!(status.needs_certificate);
    assert!(status.incident_open);
    let unsent = status.last_unsent.unwrap();
    assert_eq!(unsent.code, "certificate_needs_user");
    assert_eq!(unsent.batch_id, 1);
    assert_eq!(status.last_batch.unwrap().attempts, 0);
    assert_eq!(h.view_status(alta), "sending", "el lote sigue abierto");
    assert_eq!(h.task_error_code(), "certificate_needs_user");

    // Mientras tanto, otro remitente envió este mismo registro a la AEAT (simula un 3000).
    let record = h.app.verifactu_records().unwrap()[0].record.clone();
    sim.sembrar(&[record]);

    // «Remitir ahora» no espera por un intento sin envío: ahora el certificado sí está.
    let report = h.pump_now();
    assert!(
        matches!(
            report,
            PumpReport::Sent {
                unexpected_duplicates: 1,
                accepted: 0,
                ..
            }
        ),
        "{report:?}"
    );
    // Un solo intento real: el duplicado no es propio, no se da por aceptado (D50, D280).
    assert_eq!(
        h.view_status(alta),
        VerifactuStatus::UnexpectedDuplicate.as_str()
    );
    assert_eq!(h.status().last_batch.unwrap().attempts, 1);
    assert!(!h.status().needs_certificate);
    assert!(
        !h.status().incident_open,
        "el envío con respuesta cierra la incidencia"
    );
    assert_eq!(transport.calls().len(), 1);
}

#[test]
fn a_missing_or_unconfigured_certificate_is_an_unsent_attempt_not_an_error() {
    // Sin proveedor de certificados (con un transporte de pruebas: sin él, `network_disabled`
    // llega antes que el certificado).
    let mut h = Harness::with_environment("produccion");
    let sim = SimuladorAeat::iniciar(ConfigSimulador::default()).unwrap();
    h.app
        .set_verifactu_transport(Some(ScriptedTransport::new(&sim, vec![])));
    h.emit();
    match h.pump() {
        PumpReport::NotSent { code, batch_id, .. } => {
            assert_eq!((code.as_str(), batch_id), ("certificate_missing", 1));
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(batch_counts(&h), (0, 1, 0));
    assert_eq!(h.task_error_code(), "certificate_missing");

    // Con proveedor pero sin certificado vinculado en la configuración.
    let certs = FakeCerts::new(vec![]);
    h.app.set_certificate_provider(certs.clone());
    h.wait();
    assert!(matches!(h.pump(), PumpReport::NotSent { .. }));
    assert_eq!(
        certs.calls(),
        (0, 0),
        "sin certificado vinculado no se pide nada"
    );

    // Caducado, clave no admitida y rechazado: cada uno con su código, sin intento.
    h.app
        .set_verifactu_remission_settings(
            Some(&CertificateSource::Store {
                thumbprint_sha256: "cd".repeat(32),
            }),
            SenderCapacity::Taxpayer,
            None,
            "usuario_configuracion",
        )
        .unwrap();
    let certs = FakeCerts::new(vec![
        Err(CertError::Expired),
        Err(CertError::KeyUnsupported),
        Err(CertError::Rejected),
    ]);
    h.app.set_certificate_provider(certs);
    for expected in [
        "certificate_expired",
        "certificate_key_unsupported",
        "certificate_rejected",
    ] {
        h.wait();
        match h.pump() {
            PumpReport::NotSent { code, .. } => assert_eq!(code, expected),
            other => panic!("{other:?}"),
        }
    }
    assert_eq!(batch_counts(&h).0, 0, "ninguno anotó un intento");
    assert_eq!(h.count("SELECT count(*) FROM verifactu_batches"), 1);
    assert_eq!(h.count("SELECT count(*) FROM verifactu_unsent_attempts"), 5);
}

#[test]
fn manual_remission_asks_for_the_dialog_only_when_the_silent_attempt_needs_the_user() {
    // La primera vez, en silencio y con diálogo piden al usuario; la segunda, con diálogo ya sí.
    let certs = FakeCerts::new(vec![
        Err(CertError::NeedsUser),
        Err(CertError::NeedsUser),
        Err(CertError::NeedsUser),
        Err(CertError::NeedsUser),
        Ok(()),
    ]);
    let (mut h, transport, _sim) = Harness::certified(vec![Step::Simulator], certs.clone());
    h.emit();
    // El worker: una llamada en silencio, ninguna interactiva.
    assert!(matches!(h.pump(), PumpReport::NeedsUser { .. }));
    assert_eq!(certs.calls(), (1, 0));
    // «Remitir ahora»: en silencio, y como pide al usuario, de nuevo con el diálogo; que el
    // usuario cancela (sigue `NeedsUser`).
    assert!(matches!(h.pump_now(), PumpReport::NeedsUser { .. }));
    assert_eq!(certs.calls(), (2, 1));
    // Otra vez: en silencio sigue pidiendo, con diálogo el usuario ya puso su PIN.
    let report = h.pump_now();
    assert!(
        matches!(report, PumpReport::Sent { accepted: 1, .. }),
        "{report:?}"
    );
    assert_eq!(certs.calls(), (3, 2));
    assert_eq!(transport.calls().len(), 1);
    assert_eq!(batch_counts(&h), (1, 2, 0));
}

// --- NotSent y SentNoResponse del transporte (D280, D252) ---

#[test]
fn not_sent_does_not_count_as_an_attempt_and_sent_no_response_does() {
    let certs = FakeCerts::new(vec![]);
    let (mut h, transport, sim) = Harness::certified(
        vec![
            Step::NotSent(NotSentReason::Connect),
            Step::NoResponse(NoResponseReason::ConnectionClosed),
            Step::Simulator,
            Step::Simulator,
        ],
        certs.clone(),
    );
    h.emit();
    let alta = h.first_record();

    // 1. El cuerpo no llegó a escribirse: el intento anotado se anula (una fila aparte).
    match h.pump() {
        PumpReport::NotSent { code, batch_id, .. } => {
            assert_eq!((code.as_str(), batch_id), ("connect_failed", 1));
        }
        other => panic!("{other:?}"),
    }
    let summary = h.status().last_batch.unwrap();
    assert_eq!((summary.attempts, summary.unsent_attempts), (0, 1));
    assert_eq!(
        batch_counts(&h),
        (1, 1, 1),
        "el intento sigue anotado, anulado"
    );
    assert_eq!(h.task_error_code(), "connect_failed");
    assert_eq!(h.view_status(alta), "sending");

    // 2. El cuerpo pudo llegar y no hubo respuesta: cuenta como intento, y el lote se reenvía.
    h.wait();
    assert!(matches!(h.pump(), PumpReport::NoResponse { code, .. } if code == "unavailable"));
    let summary = h.status().last_batch.unwrap();
    assert_eq!((summary.attempts, summary.unsent_attempts), (1, 1));
    assert_eq!(h.task_error_code(), "unavailable");

    // 3. Otro intento: la AEAT no tenía el registro (la respuesta a (2) no llegó a procesarse en
    // este guion), así que lo acepta; el lote es el mismo y con los mismos bytes.
    h.wait();
    let report = h.pump();
    assert!(
        matches!(report, PumpReport::Sent { accepted: 1, .. }),
        "{report:?}"
    );
    let calls = transport.calls();
    assert_eq!(calls.len(), 3);
    assert!(
        calls.iter().all(|c| c.body == calls[0].body),
        "mismos bytes"
    );
    assert_eq!(h.count("SELECT count(*) FROM verifactu_batches"), 1);
    assert_eq!(h.view_status(alta), "accepted");
    assert_eq!(sim.estado().envios_procesados, 1);
    // Una sola tarea para todo el lote, y completada.
    let tasks: Vec<_> = h
        .app
        .tasks()
        .unwrap()
        .into_iter()
        .filter(|t| t.kind == SUBMIT_VERIFACTU)
        .collect();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].state, TaskState::Completed);
}

#[test]
fn a_sent_attempt_whose_response_was_lost_makes_the_own_duplicate_accepted() {
    let certs = FakeCerts::new(vec![]);
    let (mut h, _transport, sim) = Harness::certified(
        vec![
            Step::NotSent(NotSentReason::TlsHandshake),
            Step::Simulator,
            Step::Simulator,
        ],
        certs,
    );
    h.emit();
    let alta = h.first_record();
    assert!(matches!(h.pump(), PumpReport::NotSent { .. }));
    // Segundo intento: la AEAT lo procesa y se pierde la respuesta (cuerpo enviado, sin respuesta).
    sim.estado().perder_siguiente_respuesta = true;
    h.wait();
    assert!(matches!(h.pump(), PumpReport::NoResponse { .. }));
    assert_eq!(sim.estado().envios_procesados, 1);
    // Tercer intento: 3000, y como el intento sin respuesta SÍ cuenta, el duplicado es propio.
    h.wait();
    let report = h.pump();
    assert!(
        matches!(report, PumpReport::Sent { accepted: 1, .. }),
        "{report:?}"
    );
    let view = h
        .status()
        .records
        .into_iter()
        .find(|r| r.record_id == alta)
        .unwrap();
    assert_eq!((view.status.as_str(), view.duplicate), ("accepted", true));
    assert_eq!(h.status().last_batch.unwrap().attempts, 2);
}

#[test]
fn the_tls12_preference_is_remembered_by_fingerprint_and_the_endpoint_follows_the_certificate_kind()
{
    let certs = FakeCerts::with_kind(CertificateKind::Sello);
    let (mut h, transport, _sim) = Harness::certified(vec![Step::SimulatorTls12], certs);
    h.emit();
    assert!(matches!(h.pump(), PumpReport::Sent { .. }));
    h.wait();
    h.emit();
    assert!(matches!(h.pump(), PumpReport::Sent { .. }));
    let calls = transport.calls();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].tls, TlsPreference::Auto);
    assert_eq!(calls[1].tls, TlsPreference::Tls12, "se recuerda por huella");
    // El sello va por su canal y el entorno es el de pruebas de la AEAT (D279).
    assert_eq!(calls[0].endpoint.channel, Channel::Seal);
    assert_eq!(
        calls[0].endpoint.environment,
        AeatEnvironment::Preproduction
    );
    assert_eq!(
        h.count("SELECT count(*) FROM verifactu_tls12_preferences"),
        1
    );
}

// --- Fault de autorización (D281) ---

#[test]
fn an_authorization_fault_stops_the_remission_until_the_user_declares_or_changes_the_settings() {
    let mut h = Harness::new();
    h.app
        .set_verifactu_sim_mode(Some(SimMode::AuthorizationFault));
    h.emit();
    let alta = h.first_record();
    assert_eq!(h.pump(), PumpReport::AuthorizationRejected { code: 4112 });
    let status = h.status();
    assert!(status.authorization_rejected);
    assert_eq!(status.authorization_code, Some(4112));
    assert!(!status.halted, "no es el paro por esquema");
    assert_eq!(status.consecutive_faults, 0);
    assert_eq!(h.view_status(alta), "pending");
    assert_eq!(h.task_error_code(), "authorization_rejected");
    assert_eq!(status.last_error.unwrap().code, "authorization_rejected");
    let requests = h.requests().len();
    assert_eq!(requests, 1, "el Fault llegó una vez");

    // No se reintenta: ni el tick ni «Remitir ahora» (que no levanta este paro por sí solo).
    for _ in 0..3 {
        h.clock.advance(7200);
        assert_eq!(h.pump(), PumpReport::AuthorizationRejected { code: 4112 });
        assert_eq!(
            h.pump_now(),
            PumpReport::AuthorizationRejected { code: 4112 }
        );
    }
    assert_eq!(h.requests().len(), requests, "nada se reintentó");
    assert_eq!(h.count("SELECT count(*) FROM verifactu_batches"), 1);
    assert_eq!(h.view_status(alta), "pending");

    // «Remitir ahora» declarando que ya lo arregló: se reanuda y queda anotado.
    assert!(h.app.verifactu_declare_authorization_fixed().unwrap());
    assert!(!h.app.verifactu_declare_authorization_fixed().unwrap());
    assert!(!h.status().authorization_rejected);
    let report = h.pump_now();
    assert!(
        matches!(report, PumpReport::Sent { accepted: 1, .. }),
        "{report:?}"
    );
    assert_eq!(h.view_status(alta), "accepted");
    let resumed: i64 = h.count(&format!(
        "SELECT count(*) FROM audit_log WHERE action = 'verifactu.authorization_resume' \
         AND authorization = '{AUTHORIZATION_DECLARATION}'"
    ));
    assert_eq!(resumed, 1, "la declaración consta en el historial");
    assert_eq!(
        h.count("SELECT count(*) FROM audit_log WHERE action = 'verifactu.authorization_halt'"),
        1
    );

    // Otro Fault de autorización: cambiar la configuración de remisión también la reanuda.
    h.app
        .set_verifactu_sim_mode(Some(SimMode::AuthorizationFault));
    h.wait();
    h.emit();
    assert_eq!(h.pump(), PumpReport::AuthorizationRejected { code: 4112 });
    let changed = h
        .app
        .set_verifactu_remission_settings(
            None,
            SenderCapacity::Attorney,
            None,
            "usuario_configuracion",
        )
        .unwrap();
    assert!(changed);
    assert!(!h.status().authorization_rejected);
    h.wait();
    let report = h.pump();
    assert!(
        matches!(report, PumpReport::Sent { accepted: 1, .. }),
        "{report:?}"
    );
}

// --- Fault técnico (D281) ---

#[test]
fn a_technical_fault_never_stops_and_the_wait_grows_up_to_one_hour() {
    let mut h = Harness::new();
    h.emit();
    let alta = h.first_record();
    let mut waits = Vec::new();
    for n in 1..=14u32 {
        h.app.set_verifactu_sim_mode(Some(SimMode::TechnicalFault));
        let report = h.pump();
        assert!(
            matches!(
                report,
                PumpReport::Fault {
                    code: Some(4108),
                    ..
                }
            ),
            "fallo {n}: {report:?}"
        );
        let status = h.status();
        assert!(
            !status.halted && !status.authorization_rejected,
            "nunca para"
        );
        assert_eq!(h.view_status(alta), "pending", "vuelve a pendientes");
        // Dentro de la espera no se envía; la espera crece con cada fallo hasta una hora.
        let expected = u64::from(retry_wait_seconds(n));
        assert_eq!(status.wait_remaining_seconds, Some(expected), "fallo {n}");
        match h.pump() {
            PumpReport::Waiting { seconds, .. } => assert_eq!(seconds, expected),
            other => panic!("{other:?}"),
        }
        waits.push(expected);
        h.clock.advance(i64::try_from(expected).unwrap());
    }
    assert_eq!(&waits[..5], &[60; 5]);
    assert!(waits.windows(2).all(|w| w[0] <= w[1]), "{waits:?}");
    assert_eq!(*waits.last().unwrap(), u64::from(MAX_RETRY_WAIT_SECONDS));
    assert!(waits
        .iter()
        .all(|w| *w <= u64::from(MAX_RETRY_WAIT_SECONDS)));
    assert!(
        h.status().incident_open,
        "el fallo técnico abre una incidencia"
    );

    // Cuando el servicio vuelve, el mismo registro se acepta: nunca hubo paro.
    h.app.set_verifactu_sim_mode(None);
    let report = h.pump();
    assert!(
        matches!(report, PumpReport::Sent { accepted: 1, .. }),
        "{report:?}"
    );
    assert_eq!(h.view_status(alta), "accepted");
    assert!(
        !h.status().incident_open,
        "el envío con respuesta cierra la incidencia"
    );
}

#[test]
fn a_schema_fault_stops_with_a_notice_after_three_and_manual_remission_goes_through() {
    let mut h = Harness::new();
    h.emit();
    for n in 1..=3u32 {
        h.app.set_verifactu_sim_mode(Some(SimMode::SchemaFault));
        assert!(matches!(h.pump(), PumpReport::Fault { .. }), "{n}");
        h.wait();
    }
    assert_eq!(h.pump(), PumpReport::Halted);
    let status = h.status();
    assert!(status.halted && !status.authorization_rejected);
    assert_eq!(status.consecutive_faults, 3);
    assert_eq!(status.last_error.unwrap().code, "verifactu_fault");
    // «Remitir ahora» se salta el paro por esquema (pero no el de autorización).
    let report = h.pump_now();
    assert!(
        matches!(report, PumpReport::Sent { accepted: 1, .. }),
        "{report:?}"
    );
}

// --- Incidencia (D282) ---

#[test]
fn incidencia_is_decided_when_a_batch_is_opened_and_an_open_batch_keeps_its_bytes() {
    let mut h = Harness::new();
    h.app
        .set_verifactu_sim_mode(Some(SimMode::LoseNextResponse));
    h.emit();
    let first = h.first_record();
    assert!(matches!(h.pump(), PumpReport::NoResponse { .. }));
    assert!(h.status().incident_open);
    // Una factura emitida durante la incidencia queda fuera del lote ya abierto.
    h.clock.advance(5);
    h.emit();
    let second = h.app.verifactu_records().unwrap()[1].id;
    h.wait();
    // El lote abierto se reenvía con los MISMOS bytes, aunque haya una incidencia (D252, D282).
    let report = h.pump();
    assert!(
        matches!(report, PumpReport::Sent { accepted: 1, .. }),
        "{report:?}"
    );
    assert!(!h.status().incident_open);
    assert_eq!(h.view_status(first), "accepted");
    let requests = h.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0], requests[1], "mismos bytes");
    assert!(!requests[0].contains("Incidencia"));

    // El lote siguiente lleva la factura generada durante la incidencia: `Incidencia = S`.
    h.wait();
    assert!(matches!(h.pump(), PumpReport::Sent { .. }));
    assert_eq!(h.view_status(second), "accepted");
    let requests = h.requests();
    assert_eq!(requests.len(), 3);
    assert!(requests[2].contains("Incidencia>S<"), "{}", requests[2]);

    // Una posterior a la incidencia, no.
    h.wait();
    h.emit();
    assert!(matches!(h.pump(), PumpReport::Sent { .. }));
    let requests = h.requests();
    assert_eq!(requests.len(), 4);
    assert!(!requests[3].contains("Incidencia"));
}

#[test]
fn a_technical_fault_marks_the_next_batch_only_if_a_record_was_generated_during_the_incident() {
    let mut h = Harness::new();
    h.app.set_verifactu_sim_mode(Some(SimMode::TechnicalFault));
    h.emit();
    assert!(matches!(h.pump(), PumpReport::Fault { .. }));
    // El registro del primer lote es anterior a la incidencia; este es posterior.
    h.clock.advance(5);
    h.emit();
    h.wait();
    assert!(matches!(h.pump(), PumpReport::Sent { accepted: 2, .. }));
    let requests = h.requests();
    assert_eq!(requests.len(), 2, "el Fault y el lote reenviado");
    assert!(
        !requests[0].contains("Incidencia"),
        "antes de la incidencia"
    );
    assert!(requests[1].contains("Incidencia>S<"), "{}", requests[1]);
}

// --- Representante (D283) ---

#[test]
fn representante_goes_in_the_header_only_when_configured_and_never_from_the_capacity() {
    let mut h = Harness::new();
    // Sin configurar, no aparece; que quien remite sea un colaborador no lo deduce.
    h.app
        .set_verifactu_remission_settings(
            None,
            SenderCapacity::Collaborator,
            None,
            "usuario_configuracion",
        )
        .unwrap();
    h.emit();
    assert!(matches!(h.pump(), PumpReport::Sent { .. }));
    let requests = h.requests();
    assert!(!requests[0].contains("Representante"), "{}", requests[0]);

    // Configurado expresamente: va en `Cabecera/Representante`, tras `ObligadoEmision`.
    h.app
        .set_verifactu_remission_settings(
            None,
            SenderCapacity::Collaborator,
            Some(&PersonaEs::new("Gestoría Demo SL", "B00000034")),
            "usuario_configuracion",
        )
        .unwrap();
    let view = h.app.verifactu_remission_settings().unwrap();
    assert_eq!(view.representative.unwrap().nif, "B00000034");
    assert_eq!(view.sender_capacity, "collaborator");
    h.wait();
    h.emit();
    assert!(matches!(h.pump(), PumpReport::Sent { .. }));
    let requests = h.requests();
    let body = &requests[1];
    let obligado = body.find("ObligadoEmision").unwrap();
    let representante = body.find("Representante").unwrap();
    assert!(obligado < representante, "orden del XSD");
    assert!(body.contains("Gestoría Demo SL") && body.contains("B00000034"));
    // El lote ya enviado conserva su cabecera: el primero sigue sin `Representante`.
    assert!(!requests[0].contains("Representante"));
    // Quitarlo es un cambio expreso.
    h.app
        .set_verifactu_remission_settings(
            None,
            SenderCapacity::Taxpayer,
            None,
            "usuario_configuracion",
        )
        .unwrap();
    h.wait();
    h.emit();
    assert!(matches!(h.pump(), PumpReport::Sent { .. }));
    assert!(!h.requests()[2].contains("Representante"));
}

#[test]
fn an_invalid_representante_is_refused_when_configured() {
    let mut h = Harness::new();
    let error = h
        .app
        .set_verifactu_remission_settings(
            None,
            SenderCapacity::Collaborator,
            Some(&PersonaEs::new("Gestoría Demo SL", "123")),
            "usuario_configuracion",
        )
        .unwrap_err();
    assert_eq!(error.code(), "invalid_input");
    assert!(h
        .app
        .verifactu_remission_settings()
        .unwrap()
        .representative
        .is_none());
}

// --- Sin red real (D285) y la emisión no espera (D250) ---

#[cfg(not(feature = "aeat-red"))]
#[test]
fn preproduction_and_production_without_the_real_network_are_unsent_attempts_and_issuing_never_waits(
) {
    for environment in ["preproduccion", "produccion"] {
        let mut h = Harness::with_environment(environment);
        let certs = FakeCerts::new(vec![]);
        h.app.set_certificate_provider(certs.clone());
        h.app
            .set_verifactu_remission_settings(
                Some(&CertificateSource::Pfx {
                    reference: "00112233445566778899aabbccddeeff".into(),
                }),
                SenderCapacity::Taxpayer,
                None,
                "usuario_configuracion",
            )
            .unwrap();
        // Emitir no espera a nada: solo deja la petición de remitir (D250).
        let started = std::time::Instant::now();
        h.emit();
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
        assert!(h.app.verifactu_pump_requested());
        assert_eq!(h.count("SELECT count(*) FROM verifactu_batches"), 0);
        // La pasada: sin la característica `aeat-red`, `network_disabled`, sin tocar el certificado.
        match h.pump() {
            PumpReport::NotSent { code, message, .. } => {
                assert_eq!(code, "network_disabled", "{environment}");
                assert!(message.contains("red real"), "aviso claro: {message}");
            }
            other => panic!("{environment}: {other:?}"),
        }
        assert_eq!(certs.calls(), (0, 0), "no se obtuvo ninguna clave");
        assert_eq!(
            batch_counts(&h),
            (0, 1, 0),
            "intento sin envío, sin intento"
        );
        let status = h.status();
        assert_eq!(status.last_unsent.unwrap().code, "network_disabled");
        assert_eq!(
            status.pending + status.sending,
            1,
            "{environment}: sigue pendiente"
        );
        assert_eq!(status.last_error.unwrap().code, "network_disabled");
        // Emitir sigue sin esperar.
        let started = std::time::Instant::now();
        h.emit();
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
    }
}

// --- Estado para la interfaz (D281) ---

#[test]
fn the_status_shows_the_age_of_the_oldest_unanswered_record_and_the_open_incident() {
    let mut h = Harness::new();
    h.app.set_verifactu_sim_mode(Some(SimMode::NoNetwork));
    assert_eq!(h.status().oldest_pending_age_seconds, None);
    h.emit();
    let first = h.status().oldest_pending_at.expect("hay un pendiente");
    assert_eq!(h.status().oldest_pending_age_seconds, Some(0));
    h.clock.advance(300);
    h.emit();
    let status = h.status();
    assert_eq!(status.oldest_pending_at, Some(first), "el más antiguo");
    assert_eq!(status.oldest_pending_age_seconds, Some(300));
    assert!(!status.incident_open, "todavía no se intentó remitir");

    // Sin red el lote queda abierto y sin enviar: sus registros siguen sin remitir y cuentan.
    assert!(matches!(h.pump(), PumpReport::NotSent { .. }));
    h.clock.advance(60);
    let status = h.status();
    assert_eq!(status.oldest_pending_at, Some(first));
    assert_eq!(status.oldest_pending_age_seconds, Some(360));
    assert!(status.incident_open, "sin red abre la incidencia");
    assert_eq!(status.last_unsent.unwrap().code, "connect_failed");
    assert!(!status.needs_certificate && !status.authorization_rejected);

    // Con la red de vuelta y el envío resuelto ya no hay nada pendiente ni incidencia.
    h.app.set_verifactu_sim_mode(None);
    h.wait();
    assert!(matches!(h.pump(), PumpReport::Sent { .. }));
    let status = h.status();
    assert_eq!(status.oldest_pending_age_seconds, None);
    assert!(!status.incident_open);
}
