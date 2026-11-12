//! Remisión de los registros Verifactu al simulador local de la AEAT (etapa E5, encargo B,
//! D247-D260; D197): bomba, tarea `submit_verifactu`, control de flujo, recuperación al abrir,
//! subsanación, entorno y QR. Contra el simulador real en proceso (127.0.0.1) y, para lo que el
//! simulador no hace (respuesta parcial, 429, cuerpo ilegible), un servidor de pruebas. La
//! espera de 60 s se avanza con el reloj inyectable de la aplicación. Solo datos sintéticos.
//!
//! Los modos que fuerzan respuestas solo existen en compilación de depuración (ajuste 12).

#![cfg(debug_assertions)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use nestadmin_ai::{AiError, GenerateRequest, GenerateResult, Provider, ProviderKind};
use nestadmin_app::remission::{
    PumpReport, PumpStart, SimMode, VerifactuRecordView, SUBMIT_VERIFACTU,
};
use nestadmin_app::sales::SalesClock;
use nestadmin_app::sales_pdf::{render, PdfSource};
use nestadmin_app::{App, AppPaths};
use nestadmin_core::sales::{
    ChainStatus, Company, CustomerData, DraftInput, VerifactuEnvironment, VerifactuKind,
};
use nestadmin_core::{Task, TaskState};
use nestadmin_extract::{ExtractError, Extractor, Ocr};
use nestadmin_sales::{DocumentKind, Product, TaxRate};
use nestadmin_verifactu::{
    CodigoQr, Entorno, Fecha, FechaHoraHuso, IdFactura, RegistroFacturacion,
};
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
    /// 2026-03-01 10:00:00 +01:00.
    fn new() -> Arc<Self> {
        let base = FechaHoraHuso::new(Fecha::new(2026, 3, 1).unwrap(), 10, 0, 0, 60)
            .unwrap()
            .unix();
        Arc::new(Self(AtomicI64::new(base)))
    }
    fn advance(&self, seconds: i64) {
        self.0.fetch_add(seconds, Ordering::SeqCst);
    }
    fn now_ms(&self) -> i64 {
        self.0.load(Ordering::SeqCst) * 1000
    }
}

impl SalesClock for TestClock {
    fn now(&self) -> FechaHoraHuso {
        FechaHoraHuso::desde_unix(self.0.load(Ordering::SeqCst), 60).unwrap()
    }
}

fn pdfium_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("NESTADMIN_PDFIUM_DIR") {
        return PathBuf::from(dir);
    }
    let mut dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    loop {
        let candidate = dir.join("vendor/pdfium/bin");
        if candidate.join("pdfium.dll").exists() {
            return candidate;
        }
        if !dir.pop() {
            panic!(
                "PDFium no disponible: ejecute scripts/setup-ocr.ps1 o defina NESTADMIN_PDFIUM_DIR"
            );
        }
    }
}

fn open_app(dir: &Path, clock: &Arc<TestClock>, pdf: bool) -> App {
    let pdfium = if pdf {
        pdfium_dir()
    } else {
        dir.join("sin-pdfium")
    };
    let extractor = Extractor::new(pdfium, Box::new(NoOcr));
    let mut app = App::open(&AppPaths::new(dir), Arc::new(NoAi), Arc::new(extractor)).unwrap();
    app.set_sales_clock(clock.clone());
    app.set_verifactu_producer("Productor Demo SL", "B00000034")
        .unwrap();
    app
}

fn d(text: &str) -> Decimal {
    text.parse().unwrap()
}

struct Harness {
    dir: tempfile::TempDir,
    clock: Arc<TestClock>,
    app: App,
    customer: i64,
    product: i64,
}

impl Harness {
    fn new() -> Self {
        Self::with_pdf(false)
    }

    fn with_pdf(pdf: bool) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let clock = TestClock::new();
        let mut app = open_app(dir.path(), &clock, pdf);
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

    /// Emite una factura (la emisión llama a la bomba al terminar) y devuelve su documento.
    fn emit(&mut self) -> i64 {
        let mut input = DraftInput::new(DocumentKind::Invoice);
        input.customer_id = Some(self.customer);
        let id = self.app.create_sales_draft(&input).unwrap();
        self.app
            .add_sales_line_from_product(id, self.product, d("1"), Decimal::ZERO)
            .unwrap();
        self.app.issue_sales_document(id, None).unwrap();
        // La emisión solo deja la petición; aquí la bomba se ejecuta de forma explícita.
        self.pump();
        id
    }

    /// Pasada explícita de la bomba (lo que haría el worker en su tick); se ignora el resultado.
    fn pump(&mut self) {
        let _ = self.app.verifactu_pump(false);
    }

    /// Avanza el reloj de la aplicación pasada la espera de 60 s.
    fn wait(&self) {
        self.clock.advance(61);
    }

    /// Cierra la aplicación y la abre de nuevo sobre los mismos datos (corte y reapertura).
    fn reopen(&mut self) {
        // El simulador guarda un archivo de estado propio: la aplicación anterior se cierra antes
        // de abrir la nueva. Una aplicación de relleno ocupa el sitio mientras tanto.
        let filler_dir = tempfile::tempdir().unwrap();
        let filler = open_app(filler_dir.path(), &self.clock, false);
        drop(std::mem::replace(&mut self.app, filler));
        self.app = open_app(self.dir.path(), &self.clock, false);
    }

    fn raw(&self) -> rusqlite::Connection {
        rusqlite::Connection::open(self.dir.path().join("nestadmin.db")).unwrap()
    }

    fn count(&self, sql: &str) -> i64 {
        self.raw().query_row(sql, [], |r| r.get(0)).unwrap()
    }

    fn view(&self, record_id: i64) -> VerifactuRecordView {
        self.app
            .verifactu_status()
            .unwrap()
            .records
            .into_iter()
            .find(|r| r.record_id == record_id)
            .unwrap()
    }

    /// Ids de los registros del documento, en orden de cadena, con su clase.
    fn records_of(&self, document: i64) -> Vec<(i64, VerifactuKind)> {
        self.app
            .verifactu_records()
            .unwrap()
            .into_iter()
            .filter(|r| r.document_id == document)
            .map(|r| (r.id, r.kind))
            .collect()
    }

    fn alta(&self, document: i64) -> i64 {
        self.records_of(document)
            .into_iter()
            .find(|(_, k)| *k == VerifactuKind::Alta)
            .unwrap()
            .0
    }

    fn anulacion(&self, document: i64) -> i64 {
        self.records_of(document)
            .into_iter()
            .find(|(_, k)| *k == VerifactuKind::Anulacion)
            .unwrap()
            .0
    }

    fn id_factura(&self, record_id: i64) -> IdFactura {
        self.app
            .verifactu_records()
            .unwrap()
            .into_iter()
            .find(|r| r.id == record_id)
            .unwrap()
            .record
            .id_factura()
            .clone()
    }

    fn submit_tasks(&self) -> Vec<Task> {
        self.app
            .tasks()
            .unwrap()
            .into_iter()
            .filter(|t| t.kind == SUBMIT_VERIFACTU)
            .collect()
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

fn sent_counts(report: &PumpReport) -> (u32, u32, u32) {
    match report {
        PumpReport::Sent {
            accepted,
            accepted_with_errors,
            rejected,
            ..
        } => (*accepted, *accepted_with_errors, *rejected),
        other => panic!("se esperaba un envío cerrado, y fue {other:?}"),
    }
}

// --- Servidor de pruebas (lo que el simulador no hace) ---

struct TestServer {
    url: String,
    hits: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    addr: std::net::SocketAddr,
    handle: Option<JoinHandle<()>>,
}

/// Responde a cada petición con la siguiente respuesta de la lista (la última se repite).
fn serve(responses: Vec<(u16, String)>) -> TestServer {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let addr = listener.local_addr().unwrap();
    let hits = Arc::new(AtomicUsize::new(0));
    let stop = Arc::new(AtomicBool::new(false));
    let (h, s) = (hits.clone(), stop.clone());
    let responses = Arc::new(Mutex::new(responses));
    let handle = thread::spawn(move || {
        for connection in listener.incoming() {
            if s.load(Ordering::SeqCst) {
                break;
            }
            let Ok(mut stream) = connection else { continue };
            let index = h.fetch_add(1, Ordering::SeqCst);
            read_request(&mut stream);
            let list = responses.lock().unwrap();
            let (code, body) = list[index.min(list.len() - 1)].clone();
            let _ = write!(
                stream,
                "HTTP/1.1 {code} X\r\nContent-Type: text/xml; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.flush();
            let _ = stream.shutdown(std::net::Shutdown::Write);
        }
    });
    TestServer {
        url: format!("http://{addr}/servicio"),
        hits,
        stop,
        addr,
        handle: Some(handle),
    }
}

fn read_request(stream: &mut TcpStream) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" || line == "\n" {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            if name.trim().eq_ignore_ascii_case("content-length") {
                length = value.trim().parse().unwrap_or(0);
            }
        }
    }
    let mut body = vec![0u8; length];
    let _ = reader.read_exact(&mut body);
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.addr);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

/// Respuesta de la AEAT con las líneas dadas: (factura, operación, estado del registro).
fn response_xml(state: &str, lines: &[(&IdFactura, &str, &str)]) -> String {
    let body: String = lines
        .iter()
        .map(|(id, operation, status)| {
            format!(
                "<tikR:RespuestaLinea><tikR:IDFactura><tik:IDEmisorFactura>{}</tik:IDEmisorFactura><tik:NumSerieFactura>{}</tik:NumSerieFactura><tik:FechaExpedicionFactura>{}</tik:FechaExpedicionFactura></tikR:IDFactura><tikR:Operacion><tik:TipoOperacion>{operation}</tik:TipoOperacion></tikR:Operacion><tikR:EstadoRegistro>{status}</tikR:EstadoRegistro></tikR:RespuestaLinea>",
                id.nif_emisor, id.num_serie, id.fecha_expedicion
            )
        })
        .collect();
    format!(
        "<env:Envelope xmlns:env=\"http://schemas.xmlsoap.org/soap/envelope/\"><env:Body><tikR:RespuestaRegFactuSistemaFacturacion xmlns:tikR=\"urn:r\" xmlns:tik=\"urn:t\"><tikR:CSV>CSVPRUEBA</tikR:CSV><tikR:TiempoEsperaEnvio>60</tikR:TiempoEsperaEnvio><tikR:EstadoEnvio>{state}</tikR:EstadoEnvio>{body}</tikR:RespuestaRegFactuSistemaFacturacion></env:Body></env:Envelope>"
    )
}

// --- Pruebas ---

#[test]
fn an_accepted_batch_closes_the_task_and_leaves_the_obligation_in_the_history() {
    let mut h = Harness::new();
    let doc = h.emit();
    let alta = h.alta(doc);
    let view = h.view(alta);
    assert_eq!(view.status, "accepted");
    assert!(!view.duplicate);
    assert_eq!(view.environment, "simulador");

    let status = h.app.verifactu_status().unwrap();
    assert_eq!(status.environment, "simulador");
    assert_eq!(status.selectable_environments, vec!["simulador"]);
    assert_eq!((status.pending, status.sending, status.rejected), (0, 0, 0));
    assert!(!status.halted);
    let batch = status.last_batch.unwrap();
    assert_eq!(batch.record_count, 1);
    assert_eq!(batch.outcome.as_deref(), Some("responded"));
    assert_eq!(batch.attempts, 1);
    assert!(batch.csv.is_some());
    // Siguiente envío permitido: fin del lote más los 60 s de la AEAT.
    assert_eq!(status.next_send_at, Some(h.clock.now_ms() + 60_000));
    assert_eq!(status.wait_remaining_seconds, Some(60));

    // La tarea es visible en Tareas, completada, y no lleva `action`: no pasa por reglas.
    let tasks = h.submit_tasks();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].state, TaskState::Completed);
    let result = tasks[0].result.as_ref().unwrap();
    assert_eq!(result["accepted"], 1);
    assert!(result.get("action").is_none());

    // El historial deja la autorización de la obligación al abrir y al cerrar el lote.
    let audit = h.app.audit_log().unwrap();
    for action in ["verifactu.batch_open", "verifactu.batch_close"] {
        assert!(
            audit.iter().any(|e| e.action == action
                && e.authorization.as_deref() == Some("obligacion_verifactu simulador")),
            "falta {action} con la autorización"
        );
    }

    // Lo que recibió el simulador es el registro guardado, una sola vez.
    assert_eq!(h.requests().len(), 1);
    // Sin pendientes no hay nada que enviar.
    assert_eq!(h.app.verifactu_pump(false).unwrap(), PumpReport::Idle);
    assert!(matches!(
        h.app.verifactu_chain_status().unwrap(),
        ChainStatus::Ok { .. }
    ));
}

#[test]
fn accepted_with_errors_is_visible_and_is_not_subsanated() {
    let mut h = Harness::new();
    h.app
        .set_verifactu_sim_mode(Some(SimMode::AcceptWithErrors));
    let doc = h.emit();
    let view = h.view(h.alta(doc));
    assert_eq!(view.status, "accepted_with_errors");
    assert_eq!(view.aeat_code, Some(2000));
    assert!(view.aeat_description.unwrap().contains("simulado"));
    let error = h.app.verifactu_subsanate(h.alta(doc)).unwrap_err();
    assert_eq!(error.code(), "conflict");
}

#[test]
fn a_rejection_is_subsanated_once_and_the_subsanation_is_accepted_with_rechazo_previo() {
    let mut h = Harness::new();
    h.app.set_verifactu_sim_mode(Some(SimMode::RejectNext));
    let doc = h.emit();
    let rejected = h.alta(doc);
    let view = h.view(rejected);
    assert_eq!(view.status, "rejected");
    assert_eq!(view.aeat_code, Some(1100));
    assert_eq!(view.aeat_description.as_deref(), Some("Rechazo simulado"));
    assert_eq!(h.app.verifactu_status().unwrap().rejected, 1);

    // Doble clic: un solo registro de subsanación.
    let first = h.app.verifactu_subsanate(rejected).unwrap();
    let second = h.app.verifactu_subsanate(rejected).unwrap();
    assert!(first.created && !second.created);
    assert_eq!(first.record_id, second.record_id);
    assert_eq!(h.records_of(doc).len(), 2);
    let status = h.app.verifactu_status().unwrap();
    assert_eq!(h.view(rejected).status, "subsanated");
    assert_eq!(
        h.view(rejected).subsanation_record_id,
        Some(first.record_id)
    );
    assert_eq!(h.view(first.record_id).status, "pending");
    // La remisión respeta la espera del lote anterior: nada salió todavía.
    assert!(matches!(
        h.app.verifactu_pump(false).unwrap(),
        PumpReport::Waiting { .. }
    ));
    assert_eq!(status.pending, 1);

    h.wait();
    let report = h.app.verifactu_pump(false).unwrap();
    assert_eq!(sent_counts(&report), (1, 0, 0));
    assert_eq!(h.view(first.record_id).status, "accepted");
    // El simulador recibió la subsanación con Subsanacion=S y RechazoPrevio=X.
    let last = h.requests().pop().unwrap();
    assert!(last.contains("Subsanacion>S</"), "{last}");
    assert!(last.contains("RechazoPrevio>X</"), "{last}");
    // Lo rechazado queda como estaba: la cadena sigue íntegra.
    assert!(matches!(
        h.app.verifactu_chain_status().unwrap(),
        ChainStatus::Ok { .. }
    ));
}

#[test]
fn without_network_the_issue_does_not_wait_for_the_pump_and_the_record_stays_sending() {
    let mut h = Harness::new();
    h.app.set_verifactu_sim_mode(Some(SimMode::NoNetwork));
    let doc = h.emit(); // no devuelve error aunque la remisión falle
    let alta = h.alta(doc);
    assert!(h.app.sales_document(doc).unwrap().number.is_some());
    assert_eq!(h.view(alta).status, "sending");
    let status = h.app.verifactu_status().unwrap();
    let batch = status.last_batch.unwrap();
    assert_eq!(batch.outcome, None, "el lote sin respuesta no se cierra");
    assert_eq!(batch.attempts, 1);
    assert_eq!(status.next_send_at, Some(h.clock.now_ms() + 60_000));
    // La tarea queda fallida y reintentable, con el motivo.
    let task = h.submit_tasks().pop().unwrap();
    assert_eq!(task.state, TaskState::Failed);
    assert_eq!(task.error.as_ref().unwrap().code, "unavailable");

    // Dentro de la espera no hay otro intento, y otra emisión no espera a nada.
    let doc2 = h.emit();
    assert_eq!(h.view(h.alta(doc2)).status, "pending");
    match h.app.verifactu_pump(false).unwrap() {
        PumpReport::Waiting { seconds, .. } => assert_eq!(seconds, 60),
        other => panic!("{other:?}"),
    }
    assert_eq!(
        h.app
            .verifactu_status()
            .unwrap()
            .last_batch
            .unwrap()
            .attempts,
        1
    );

    // Pasada la espera, vuelve la red: se reenvía el MISMO lote y la misma tarea.
    h.app.set_verifactu_sim_mode(None);
    h.wait();
    let report = h.app.verifactu_pump(false).unwrap();
    assert_eq!(sent_counts(&report), (1, 0, 0));
    let batch = h.app.verifactu_status().unwrap().last_batch.unwrap();
    assert_eq!((batch.id, batch.attempts, batch.record_count), (1, 2, 1));
    assert_eq!(h.view(alta).status, "accepted");
    assert_eq!(h.view(h.alta(doc2)).status, "pending");
    assert_eq!(
        h.submit_tasks().len(),
        1,
        "una tarea por lote, no por intento"
    );
    assert_eq!(h.submit_tasks()[0].state, TaskState::Completed);
    // La primera vez no llegó nada al simulador; la segunda, sí.
    assert_eq!(h.requests().len(), 1);
}

#[test]
fn a_lost_response_is_resent_identical_and_the_own_duplicate_counts_as_accepted() {
    let mut h = Harness::new();
    h.app
        .set_verifactu_sim_mode(Some(SimMode::LoseNextResponse));
    let doc = h.emit();
    let alta = h.alta(doc);
    assert_eq!(h.view(alta).status, "sending");
    assert_eq!(
        h.app
            .verifactu_simulator()
            .unwrap()
            .estado()
            .envios_procesados,
        1
    );
    let task = h.submit_tasks().pop().unwrap();
    assert_eq!(task.error.unwrap().code, "unavailable");

    h.wait();
    let report = h.app.verifactu_pump(false).unwrap();
    assert_eq!(sent_counts(&report), (1, 0, 0));
    let view = h.view(alta);
    assert_eq!(view.status, "accepted");
    assert!(view.duplicate, "la AEAT ya lo tenía: duplicado propio");
    // Mismo contenido byte a byte, y ningún registro nuevo.
    let requests = h.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0], requests[1]);
    assert_eq!(h.records_of(doc).len(), 1);
    assert_eq!(h.count("SELECT count(*) FROM verifactu_batches"), 1);
}

#[test]
fn a_resent_void_answered_3001_counts_as_accepted() {
    let mut h = Harness::new();
    let doc = h.emit();
    h.wait();
    h.app
        .set_verifactu_sim_mode(Some(SimMode::LoseNextResponse));
    h.app.void_sales_invoice(doc).unwrap();
    h.pump();
    let void = h.anulacion(doc);
    // La baja salió al anular (la espera ya había pasado) y se perdió la respuesta.
    assert_eq!(h.view(void).status, "sending");
    h.wait();
    let report = h.app.verifactu_pump(false).unwrap();
    assert_eq!(sent_counts(&report), (1, 0, 0));
    let view = h.view(void);
    assert_eq!(view.status, "accepted");
    assert!(view.duplicate);
    assert_eq!(view.aeat_code, Some(3001));
}

#[test]
fn a_duplicate_the_aeat_had_from_elsewhere_is_unexpected_and_not_accepted() {
    let mut h = Harness::new();
    let first = h.emit();
    let second = h.emit(); // dentro de la espera: pendiente
    let pending = h.alta(second);
    assert_eq!(h.view(pending).status, "pending");
    // La AEAT (simulada) ya tiene ese registro sin que nuestro lote lo hubiera enviado.
    let record = h
        .app
        .verifactu_records()
        .unwrap()
        .into_iter()
        .find(|r| r.id == pending)
        .unwrap()
        .record;
    h.app.verifactu_simulator().unwrap().sembrar(&[record]);
    h.wait();
    let report = h.app.verifactu_pump(false).unwrap();
    match report {
        PumpReport::Sent {
            accepted,
            unexpected_duplicates,
            ..
        } => assert_eq!((accepted, unexpected_duplicates), (0, 1)),
        other => panic!("{other:?}"),
    }
    let view = h.view(pending);
    assert_eq!(view.status, "unexpected_duplicate");
    assert_eq!(view.aeat_code, Some(3000));
    assert_eq!(h.app.verifactu_status().unwrap().unexpected_duplicates, 1);
    assert_eq!(h.view(h.alta(first)).status, "accepted");
}

#[test]
fn a_schema_fault_closes_the_batch_and_three_in_a_row_stop_the_pump_until_the_user_asks() {
    let mut h = Harness::new();
    h.app.set_verifactu_sim_mode(Some(SimMode::SchemaFault));
    let doc = h.emit();
    let alta = h.alta(doc);
    let batch = h.app.verifactu_status().unwrap().last_batch.unwrap();
    assert_eq!(batch.outcome.as_deref(), Some("fault"));
    assert_eq!(batch.error_code, Some(4104));
    assert_eq!(h.view(alta).status, "pending", "vuelve a pendientes");
    for _ in 0..2 {
        h.wait();
        h.app.set_verifactu_sim_mode(Some(SimMode::SchemaFault));
        match h.app.verifactu_pump(false).unwrap() {
            PumpReport::Fault { code, .. } => assert_eq!(code, Some(4104)),
            other => panic!("{other:?}"),
        }
    }
    let status = h.app.verifactu_status().unwrap();
    assert!(status.halted);
    assert_eq!(status.consecutive_faults, 3);
    // Parada: ni con la espera pasada se reintenta sola.
    h.wait();
    assert_eq!(h.app.verifactu_pump(false).unwrap(), PumpReport::Halted);
    assert_eq!(h.requests().len(), 3);
    // Las tareas de los lotes con `Fault` quedan fallidas y visibles.
    let failed: Vec<_> = h
        .submit_tasks()
        .into_iter()
        .filter(|t| t.state == TaskState::Failed)
        .collect();
    assert_eq!(failed.len(), 3);
    assert!(failed
        .iter()
        .all(|t| t.error.as_ref().unwrap().code == "verifactu_fault"));
    // «Remitir ahora» lo salta y, si el envío es bueno, la remisión sigue.
    let report = h.app.verifactu_pump(true).unwrap();
    assert_eq!(sent_counts(&report), (1, 0, 0));
    assert!(!h.app.verifactu_status().unwrap().halted);
    assert_eq!(h.view(alta).status, "accepted");
}

#[test]
fn a_partial_response_leaves_the_missing_record_pending() {
    let mut h = Harness::new();
    h.emit();
    let second = h.emit();
    let third = h.emit();
    let (a, b) = (h.alta(second), h.alta(third));
    let id_a = h.id_factura(a);
    h.wait();
    // Respuesta con una sola línea: falta la del tercero.
    let server = serve(vec![(
        200,
        response_xml("ParcialmenteCorrecto", &[(&id_a, "Alta", "Correcto")]),
    )]);
    h.app.set_verifactu_endpoint(Some(server.url.clone()));
    let report = h.app.verifactu_pump(false).unwrap();
    match report {
        PumpReport::Sent {
            accepted, missing, ..
        } => assert_eq!((accepted, missing), (1, 1)),
        other => panic!("{other:?}"),
    }
    assert_eq!(h.view(a).status, "accepted");
    assert_eq!(h.view(b).status, "pending", "nunca aceptado sin su línea");
    let result = h
        .submit_tasks()
        .into_iter()
        .max_by_key(|t| t.id)
        .unwrap()
        .result
        .unwrap();
    assert_eq!(result["missing_record_ids"], serde_json::json!([b]));
    // Después se envía de verdad.
    h.app.set_verifactu_endpoint(None);
    h.wait();
    let report = h.app.verifactu_pump(false).unwrap();
    assert_eq!(sent_counts(&report), (1, 0, 0));
    assert_eq!(h.view(b).status, "accepted");
    assert_eq!(server.hits.load(Ordering::SeqCst), 1);
}

#[test]
fn transport_errors_are_classified_and_none_closes_the_batch() {
    let mut h = Harness::new();
    let server = serve(vec![
        (429, String::new()),
        (401, String::new()),
        (200, "esto no es XML".into()),
        (503, "caído".into()),
    ]);
    h.app.set_verifactu_endpoint(Some(server.url.clone()));
    let doc = h.emit();
    let alta = h.alta(doc);
    let mut codes = Vec::new();
    let first = h.app.verifactu_status().unwrap();
    assert_eq!(first.last_batch.unwrap().outcome, None);
    codes.push(h.submit_tasks()[0].error.as_ref().unwrap().code.clone());
    for _ in 0..3 {
        h.wait();
        match h.app.verifactu_pump(false).unwrap() {
            PumpReport::NoResponse { code, .. } => codes.push(code),
            other => panic!("{other:?}"),
        }
        assert_eq!(h.view(alta).status, "sending");
    }
    assert_eq!(
        codes,
        [
            "rate_limited",
            "unauthorized",
            "invalid_response",
            "unavailable"
        ]
    );
    assert_eq!(server.hits.load(Ordering::SeqCst), 4);
    // Ninguno cerró el lote: con el servicio de vuelta se reenvía el mismo.
    h.app.set_verifactu_endpoint(None);
    h.wait();
    let report = h.app.verifactu_pump(false).unwrap();
    assert_eq!(sent_counts(&report), (1, 0, 0));
    let batch = h.app.verifactu_status().unwrap().last_batch.unwrap();
    assert_eq!((batch.id, batch.attempts), (1, 5));
    assert_eq!(h.submit_tasks().len(), 1);
}

/// Inserta `count` altas pendientes encadenadas tras la última (sin recalcular huellas: el
/// simulador las acepta con errores, 2000), como hace la prueba de core.
fn bulk_pending(h: &Harness, count: u64) {
    let template = h.app.verifactu_records().unwrap().pop().unwrap();
    let conn = h.raw();
    let mut previous_id = template.id;
    let mut previous_hash = template.hash.clone();
    let mut previous = template.record.clone();
    for i in 0..count {
        let RegistroFacturacion::Alta(mut alta) = previous.clone() else {
            unreachable!()
        };
        alta.id_factura.num_serie = format!("BULK-{i:04}");
        alta.encadenamiento = previous.siguiente_encadenamiento();
        alta.huella = format!("{:064X}", i + 1);
        let record = RegistroFacturacion::Alta(alta);
        let id = record.id_factura().clone();
        conn.execute(
            "INSERT INTO verifactu_records (kind, document_id, issuer_tax_id, invoice_number, \
             issue_date, invoice_type, total, tax_total, previous_id, previous_hash, \
             generated_at, hash, record_json, created_at) \
             VALUES ('alta', ?1, ?2, ?3, ?4, 'F1', '121.00', '21.00', ?5, ?6, ?7, ?8, ?9, 0)",
            rusqlite::params![
                template.document_id,
                id.nif_emisor,
                id.num_serie,
                id.fecha_expedicion.to_string(),
                previous_id,
                previous_hash,
                template.generated_at,
                record.huella(),
                serde_json::to_string(&record).unwrap(),
            ],
        )
        .unwrap();
        previous_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO verifactu_record_environments (record_id, environment) \
             VALUES (?1, 'simulador')",
            [previous_id],
        )
        .unwrap();
        previous_hash = record.huella().to_owned();
        previous = record;
    }
}

#[test]
fn more_than_1000_records_go_in_two_batches_respecting_the_wait() {
    let mut h = Harness::new();
    h.emit();
    bulk_pending(&h, 1001);
    assert_eq!(h.app.verifactu_status().unwrap().pending, 1001);
    h.wait();
    let report = h.app.verifactu_pump(false).unwrap();
    let (accepted, with_errors, rejected) = sent_counts(&report);
    assert_eq!(accepted + with_errors, 1000);
    assert_eq!(rejected, 0);
    assert_eq!(h.app.verifactu_status().unwrap().pending, 1);
    // Sin pasar la espera, el segundo lote no sale.
    assert!(matches!(
        h.app.verifactu_pump(false).unwrap(),
        PumpReport::Waiting { .. }
    ));
    h.wait();
    let report = h.app.verifactu_pump(false).unwrap();
    assert_eq!(sent_counts(&report).0 + sent_counts(&report).1, 1);
    let status = h.app.verifactu_status().unwrap();
    assert_eq!((status.pending, status.sending), (0, 0));
    assert_eq!(h.count("SELECT count(*) FROM verifactu_batches"), 3);
    assert_eq!(h.requests().len(), 3);
}

#[test]
fn the_alta_and_the_void_of_the_same_invoice_go_in_different_batches() {
    let mut h = Harness::new();
    h.emit();
    let doc = h.emit(); // pendiente (dentro de la espera)
    h.app.void_sales_invoice(doc).unwrap();
    h.pump();
    let (alta, void) = (h.alta(doc), h.anulacion(doc));
    assert_eq!(h.view(alta).status, "pending");
    assert_eq!(h.view(void).status, "pending");
    h.wait();
    let report = h.app.verifactu_pump(false).unwrap();
    assert_eq!(sent_counts(&report), (1, 0, 0));
    assert_eq!(
        h.app
            .verifactu_status()
            .unwrap()
            .last_batch
            .unwrap()
            .record_count,
        1
    );
    assert_eq!(h.view(alta).status, "accepted");
    assert_eq!(h.view(void).status, "pending");
    h.wait();
    let report = h.app.verifactu_pump(false).unwrap();
    assert_eq!(sent_counts(&report), (1, 0, 0));
    assert_eq!(h.view(void).status, "accepted");
}

#[test]
fn the_environment_cannot_change_with_records_to_send_and_only_the_simulator_is_selectable() {
    let mut h = Harness::new();
    h.emit();
    h.emit(); // pendiente
    for environment in [
        VerifactuEnvironment::Preproduccion,
        VerifactuEnvironment::Produccion,
    ] {
        let error = h
            .app
            .set_verifactu_environment(environment, "autorizo")
            .unwrap_err();
        assert_eq!(error.code(), "verifactu_environment_busy");
    }
    // Elegir el vigente no hace nada, con o sin pendientes.
    assert!(!h
        .app
        .set_verifactu_environment(VerifactuEnvironment::Simulador, "autorizo")
        .unwrap());
    h.wait();
    h.app.verifactu_pump(false).unwrap();
    // Sin nada por remitir, preproducción y producción siguen sin estar disponibles (D247).
    for environment in [
        VerifactuEnvironment::Preproduccion,
        VerifactuEnvironment::Produccion,
    ] {
        let error = h
            .app
            .set_verifactu_environment(environment, "autorizo")
            .unwrap_err();
        assert_eq!(error.code(), "environment_unavailable");
    }
    assert_eq!(
        h.app.verifactu_environment().unwrap(),
        VerifactuEnvironment::Simulador
    );
    assert_eq!(
        h.count("SELECT count(*) FROM verifactu_environment_changes"),
        0
    );
}

#[test]
fn the_simulator_is_seeded_with_the_accepted_records_after_reopening() {
    let mut h = Harness::new();
    let doc = h.emit();
    assert_eq!(h.view(h.alta(doc)).status, "accepted");
    h.reopen();
    h.app.void_sales_invoice(doc).unwrap();
    h.pump();
    h.wait();
    let report = h.app.verifactu_pump(false).unwrap();
    // Sin sembrar, el simulador nuevo no conocería el alta y rechazaría la baja con 3002.
    assert_eq!(sent_counts(&report), (1, 0, 0), "{report:?}");
    assert_eq!(h.view(h.anulacion(doc)).status, "accepted");
    assert_eq!(h.requests().len(), 1);
}

#[test]
fn a_cut_halfway_leaves_nothing_duplicated_when_reopening() {
    let mut h = Harness::new();
    h.app.set_verifactu_sim_mode(Some(SimMode::NoNetwork));
    let doc = h.emit();
    h.app.set_verifactu_sim_mode(None);
    h.wait();
    // La pasada llega al simulador y se corta antes de guardar la respuesta.
    let job = match h.app.verifactu_pump_begin(false).unwrap() {
        PumpStart::Job(job) => job,
        PumpStart::Idle(report) => panic!("{report:?}"),
    };
    let sent = job.run();
    assert_eq!(
        h.app
            .verifactu_simulator()
            .unwrap()
            .estado()
            .envios_procesados,
        1
    );
    drop(sent);
    let task = h.submit_tasks().pop().unwrap();
    assert_eq!(task.state, TaskState::Running);
    assert_eq!(h.view(h.alta(doc)).status, "sending");

    // Al reabrir: la tarea vuelve a la cola (sin que la cola la tome) y el lote sigue abierto.
    h.reopen();
    let tasks = h.submit_tasks();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].state, TaskState::Queued);
    assert!(h.app.process_next().unwrap().is_none());
    assert_eq!(h.view(h.alta(doc)).status, "sending");
    // La bomba reenvía el mismo lote: la espera del último intento aún no ha pasado.
    assert!(matches!(
        h.app.verifactu_pump(false).unwrap(),
        PumpReport::Waiting { .. }
    ));
    h.wait();
    let report = h.app.verifactu_pump(false).unwrap();
    assert_eq!(sent_counts(&report), (1, 0, 0));
    assert_eq!(h.view(h.alta(doc)).status, "accepted");
    // Nada duplicado: un registro, un lote, una tarea (la misma), cadena íntegra.
    assert_eq!(h.records_of(doc).len(), 1);
    assert_eq!(h.count("SELECT count(*) FROM verifactu_batches"), 1);
    let tasks = h.submit_tasks();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].id, task.id);
    assert_eq!(tasks[0].state, TaskState::Completed);
    assert!(matches!(
        h.app.verifactu_chain_status().unwrap(),
        ChainStatus::Ok { .. }
    ));

    // Otro corte: el lote ya constaba cerrado y la tarea quedó en cola. Al reabrir se completa
    // sin ningún envío ni lote nuevo.
    h.raw()
        .execute(
            "UPDATE tasks SET state = 'queued' WHERE kind = 'submit_verifactu'",
            [],
        )
        .unwrap();
    h.reopen();
    let tasks = h.submit_tasks();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].state, TaskState::Completed);
    assert_eq!(tasks[0].result.as_ref().unwrap()["recovered"], true);
    assert_eq!(h.count("SELECT count(*) FROM verifactu_batches"), 1);
    assert_eq!(h.app.verifactu_pump(false).unwrap(), PumpReport::Idle);
}

#[test]
fn only_one_pass_runs_at_a_time() {
    let mut h = Harness::new();
    h.app.set_verifactu_sim_mode(Some(SimMode::NoNetwork));
    h.emit();
    h.app.set_verifactu_sim_mode(None);
    h.wait();
    let job = match h.app.verifactu_pump_begin(false).unwrap() {
        PumpStart::Job(job) => job,
        PumpStart::Idle(report) => panic!("{report:?}"),
    };
    // Mientras la pasada está en curso, el tick y el botón no abren otra.
    assert_eq!(h.app.verifactu_pump(false).unwrap(), PumpReport::Busy);
    assert_eq!(h.app.verifactu_pump(true).unwrap(), PumpReport::Busy);
    let report = h.app.verifactu_pump_finish(job.run()).unwrap();
    assert_eq!(sent_counts(&report), (1, 0, 0));
    assert_eq!(h.app.verifactu_pump(true).unwrap(), PumpReport::Idle);
}

fn pdf_with(app: &App, document: i64) -> Vec<u8> {
    let doc = app.sales_document(document).unwrap();
    app.render_sales_pdf(&doc).unwrap()
}

/// PDF tal como lo hacía la versión anterior: siempre con el QR de un entorno fijo.
fn pdf_with_fixed_qr(app: &App, document: i64, environment: Entorno) -> Vec<u8> {
    let doc = app.sales_document(document).unwrap();
    let alta = app
        .verifactu_records()
        .unwrap()
        .into_iter()
        .find_map(|r| match r.record {
            RegistroFacturacion::Alta(a) if r.document_id == document => Some(a),
            _ => None,
        })
        .unwrap();
    let extractor = Extractor::new(pdfium_dir(), Box::new(NoOcr));
    render(
        extractor.pdfium().unwrap(),
        &PdfSource {
            document: &doc,
            rectified_number: None,
            logo: None,
            letterhead: None,
            qr: Some(CodigoQr::de_registro(environment, &alta).unwrap()),
        },
    )
    .unwrap()
}

#[test]
fn new_records_carry_the_test_qr_and_inherited_production_records_keep_their_bytes() {
    let mut h = Harness::with_pdf(true);
    let doc = h.emit();
    let alta = h.alta(doc);
    assert_eq!(
        h.app.verifactu_record_environment(alta).unwrap().as_str(),
        "simulador"
    );

    // Un registro nuevo del simulador lleva el QR de pruebas, byte a byte, y es estable.
    let simulator_pdf = pdf_with(&h.app, doc);
    assert_eq!(simulator_pdf, pdf_with(&h.app, doc));
    assert_eq!(
        simulator_pdf,
        pdf_with_fixed_qr(&h.app, doc, Entorno::Pruebas)
    );
    let production_pdf = pdf_with_fixed_qr(&h.app, doc, Entorno::Produccion);
    assert_ne!(
        simulator_pdf, production_pdf,
        "el QR de pruebas es distinto"
    );

    // La subsanación crea otra alta de la misma factura: el QR sigue siendo el de la primera.
    // (Se fuerza el rechazo del primer registro para poder subsanarlo.)
    let mut h2 = Harness::with_pdf(true);
    h2.app.set_verifactu_sim_mode(Some(SimMode::RejectNext));
    let rejected_doc = h2.emit();
    let before = pdf_with(&h2.app, rejected_doc);
    let rejected = h2.alta(rejected_doc);
    h2.app.verifactu_subsanate(rejected).unwrap();
    assert_eq!(h2.records_of(rejected_doc).len(), 2);
    assert_eq!(before, pdf_with(&h2.app, rejected_doc));

    // Un registro anterior a la migración 16 (`produccion_heredado`) conserva el QR de
    // producción de siempre (D226): se simula en la base, sin triggers, y se reabre.
    let Harness {
        dir, clock, app, ..
    } = h;
    drop(app);
    {
        let conn = rusqlite::Connection::open(dir.path().join("nestadmin.db")).unwrap();
        conn.execute_batch("DROP TRIGGER verifactu_record_environments_no_update;")
            .unwrap();
        conn.execute(
            "UPDATE verifactu_record_environments SET environment = 'produccion_heredado' \
             WHERE record_id = ?1",
            [alta],
        )
        .unwrap();
    }
    let app = open_app(dir.path(), &clock, true);
    assert_eq!(
        app.verifactu_record_environment(alta).unwrap().as_str(),
        "produccion_heredado"
    );
    let legacy_pdf = pdf_with(&app, doc);
    assert_eq!(legacy_pdf, production_pdf, "mismos bytes que antes (D226)");
    drop(app);

    // Sin fila de entorno es un error explícito, no un «producción» silencioso.
    {
        let conn = rusqlite::Connection::open(dir.path().join("nestadmin.db")).unwrap();
        conn.execute_batch("DROP TRIGGER verifactu_record_environments_no_delete;")
            .unwrap();
        conn.execute(
            "DELETE FROM verifactu_record_environments WHERE record_id = ?1",
            [alta],
        )
        .unwrap();
    }
    let app = open_app(dir.path(), &clock, true);
    let doc_row = app.sales_document(doc).unwrap();
    let error = app.render_sales_pdf(&doc_row).unwrap_err();
    assert_eq!(error.code(), "storage");
}

/// Servidor que acepta conexiones y nunca responde: una llamada real esperaría su tiempo.
fn silent_endpoint() -> (TcpListener, String) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let url = format!("http://{}/servicio", listener.local_addr().unwrap());
    (listener, url)
}

#[test]
fn issuing_voiding_and_subsanating_return_without_touching_the_network() {
    let mut h = Harness::new();
    let limit = std::time::Duration::from_secs(2);
    let (_silent, url) = silent_endpoint();
    let check = |name: &str, h: &mut Harness, f: &dyn Fn(&mut Harness)| {
        let started = std::time::Instant::now();
        f(h);
        assert!(started.elapsed() < limit, "{name} esperó a la red");
        assert!(h.app.verifactu_pump_requested(), "{name} no pidió remitir");
        assert!(!h.app.verifactu_pump_requested(), "la petición se consume");
    };
    // Con un endpoint que no responde.
    h.app.set_verifactu_endpoint(Some(url));
    let issue = |h: &mut Harness| {
        let mut input = DraftInput::new(DocumentKind::Invoice);
        input.customer_id = Some(h.customer);
        let id = h.app.create_sales_draft(&input).unwrap();
        h.app
            .add_sales_line_from_product(id, h.product, d("1"), Decimal::ZERO)
            .unwrap();
        h.app.issue_sales_document(id, None).unwrap();
        id
    };
    check("emitir", &mut h, &|h| {
        let _ = issue(h);
    });
    // Con el modo sin red.
    h.app.set_verifactu_endpoint(None);
    h.app.set_verifactu_sim_mode(Some(SimMode::NoNetwork));
    let started = std::time::Instant::now();
    let doc = issue(&mut h);
    assert!(started.elapsed() < limit);
    assert!(h.app.verifactu_pump_requested());
    // Nada salió: ni lote ni intento, y los registros están pendientes.
    assert_eq!(h.count("SELECT count(*) FROM verifactu_batches"), 0);
    assert_eq!(h.view(h.alta(doc)).status, "pending");
    check("anular", &mut h, &|h| {
        h.app.void_sales_invoice(doc).unwrap();
    });
    assert_eq!(h.count("SELECT count(*) FROM verifactu_batch_attempts"), 0);
}

// --- Revisión de E5: paro con lote abierto, respuestas parciales, emisores y avisos ---

impl Harness {
    /// Como `emit`, pero sin pasada de la bomba: el registro queda pendiente.
    fn emit_quiet(&mut self) -> i64 {
        let mut input = DraftInput::new(DocumentKind::Invoice);
        input.customer_id = Some(self.customer);
        let id = self.app.create_sales_draft(&input).unwrap();
        self.app
            .add_sales_line_from_product(id, self.product, d("1"), Decimal::ZERO)
            .unwrap();
        self.app.issue_sales_document(id, None).unwrap();
        id
    }

    fn record(&self, record_id: i64) -> RegistroFacturacion {
        self.app
            .verifactu_records()
            .unwrap()
            .into_iter()
            .find(|r| r.id == record_id)
            .unwrap()
            .record
    }
}

fn company(name: &str, tax_id: &str) -> Company {
    Company {
        name: Some(name.into()),
        tax_id: Some(tax_id.into()),
        address: Some("Calle Ficticia 9, 28000 Madrid".into()),
        email: None,
        iban: None,
        logo: None,
        logo_mime: None,
    }
}

#[test]
fn a_halted_pump_still_resends_the_open_batch() {
    let mut h = Harness::new();
    h.app.set_verifactu_sim_mode(Some(SimMode::SchemaFault));
    let doc = h.emit();
    let alta = h.alta(doc);
    for _ in 0..2 {
        h.wait();
        h.app.set_verifactu_sim_mode(Some(SimMode::SchemaFault));
        assert!(matches!(
            h.app.verifactu_pump(false).unwrap(),
            PumpReport::Fault { .. }
        ));
    }
    assert!(h.app.verifactu_status().unwrap().halted);
    h.wait();
    assert_eq!(h.app.verifactu_pump(false).unwrap(), PumpReport::Halted);
    // «Remitir ahora» abre un lote y pierde la respuesta: el lote queda abierto.
    h.app
        .set_verifactu_sim_mode(Some(SimMode::LoseNextResponse));
    match h.app.verifactu_pump(true).unwrap() {
        PumpReport::NoResponse { .. } => {}
        other => panic!("{other:?}"),
    }
    assert_eq!(h.view(alta).status, "sending");
    assert!(h.app.verifactu_status().unwrap().halted);
    // El tick automático reenvía el lote abierto aunque la remisión esté parada.
    h.wait();
    let report = h.app.verifactu_pump(false).unwrap();
    assert_eq!(sent_counts(&report), (1, 0, 0));
    assert_eq!(h.view(alta).status, "accepted");
    assert!(!h.app.verifactu_status().unwrap().halted);
    assert_eq!(h.count("SELECT count(*) FROM verifactu_batches"), 4);
}

#[test]
fn a_record_missing_from_a_partial_response_that_returns_as_3000_is_our_own_duplicate() {
    let mut h = Harness::new();
    h.emit();
    let second = h.emit_quiet();
    let third = h.emit_quiet();
    let (b, c) = (h.alta(second), h.alta(third));
    let id_b = h.id_factura(b);
    h.wait();
    let server = serve(vec![(
        200,
        response_xml("ParcialmenteCorrecto", &[(&id_b, "Alta", "Correcto")]),
    )]);
    h.app.set_verifactu_endpoint(Some(server.url.clone()));
    match h.app.verifactu_pump(false).unwrap() {
        PumpReport::Sent {
            accepted, missing, ..
        } => assert_eq!((accepted, missing), (1, 1)),
        other => panic!("{other:?}"),
    }
    assert_eq!(h.view(c).status, "pending");
    // La AEAT sí lo había recibido aunque la línea no llegara: en el lote siguiente es 3000.
    h.app.set_verifactu_endpoint(None);
    let record = h.record(c);
    h.app.verifactu_simulator().unwrap().sembrar(&[record]);
    h.wait();
    let report = h.app.verifactu_pump(false).unwrap();
    assert_eq!(sent_counts(&report), (1, 0, 0));
    let view = h.view(c);
    assert_eq!(view.status, "accepted");
    assert!(view.duplicate);
    assert_eq!(view.aeat_code, Some(3000));
    assert_eq!(h.app.verifactu_status().unwrap().unexpected_duplicates, 0);
}

#[test]
fn a_void_missing_from_a_partial_response_that_returns_as_3001_is_accepted() {
    let mut h = Harness::new();
    let first = h.emit();
    h.wait();
    h.app.void_sales_invoice(first).unwrap();
    let other = h.emit_quiet();
    let (void, alta) = (h.anulacion(first), h.alta(other));
    let id_alta = h.id_factura(alta);
    let server = serve(vec![(
        200,
        response_xml("ParcialmenteCorrecto", &[(&id_alta, "Alta", "Correcto")]),
    )]);
    h.app.set_verifactu_endpoint(Some(server.url.clone()));
    match h.app.verifactu_pump(false).unwrap() {
        PumpReport::Sent {
            accepted, missing, ..
        } => assert_eq!((accepted, missing), (1, 1)),
        other => panic!("{other:?}"),
    }
    assert_eq!(h.view(void).status, "pending");
    h.app.set_verifactu_endpoint(None);
    let record = h.record(void);
    h.app.verifactu_simulator().unwrap().sembrar(&[record]);
    h.wait();
    let report = h.app.verifactu_pump(false).unwrap();
    assert_eq!(sent_counts(&report), (1, 0, 0));
    let view = h.view(void);
    assert_eq!(view.status, "accepted");
    assert!(view.duplicate);
    assert_eq!(view.aeat_code, Some(3001));
}

/// Inserta una alta pendiente de otro emisor tras la última (sin recalcular la huella: el
/// simulador la acepta con errores, 2000). Devuelve su id.
fn insert_other_issuer(h: &Harness, nif: &str, name: &str) -> i64 {
    let template = h.app.verifactu_records().unwrap().pop().unwrap();
    let RegistroFacturacion::Alta(mut alta) = template.record.clone() else {
        unreachable!()
    };
    alta.id_factura.nif_emisor = nif.to_owned();
    alta.id_factura.num_serie = "OTRO-0001".to_owned();
    alta.nombre_razon_emisor = name.to_owned();
    alta.encadenamiento = template.record.siguiente_encadenamiento();
    alta.huella = format!("{:064X}", 7);
    let record = RegistroFacturacion::Alta(alta);
    let id = record.id_factura().clone();
    let conn = h.raw();
    conn.execute(
        "INSERT INTO verifactu_records (kind, document_id, issuer_tax_id, invoice_number, \
         issue_date, invoice_type, total, tax_total, previous_id, previous_hash, \
         generated_at, hash, record_json, created_at) \
         VALUES ('alta', ?1, ?2, ?3, ?4, 'F1', '121.00', '21.00', ?5, ?6, ?7, ?8, ?9, 0)",
        rusqlite::params![
            template.document_id,
            id.nif_emisor,
            id.num_serie,
            id.fecha_expedicion.to_string(),
            template.id,
            template.hash,
            template.generated_at,
            record.huella(),
            serde_json::to_string(&record).unwrap(),
        ],
    )
    .unwrap();
    let record_id = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO verifactu_record_environments (record_id, environment) \
         VALUES (?1, 'simulador')",
        [record_id],
    )
    .unwrap();
    record_id
}

#[test]
fn pending_records_of_two_issuers_go_in_separate_batches_and_nothing_blocks() {
    let mut h = Harness::new();
    let first = h.emit_quiet();
    let a = h.alta(first);
    let b = insert_other_issuer(&h, "B00000034", "Otra Empresa Demo SL");
    assert_ne!(h.id_factura(a).nif_emisor, h.id_factura(b).nif_emisor);
    // Primer lote: solo el primer emisor.
    let report = h.app.verifactu_pump(false).unwrap();
    assert_eq!(sent_counts(&report), (1, 0, 0));
    assert_eq!(h.view(a).status, "accepted");
    assert_eq!(h.view(b).status, "pending");
    let status = h.app.verifactu_status().unwrap();
    assert!(status.last_error.is_none(), "{:?}", status.last_error);
    assert_eq!(status.pending, 1);
    assert_eq!(status.last_batch.unwrap().record_count, 1);
    // Segundo lote, tras la espera: el otro emisor, con su razón social.
    h.wait();
    let report = h.app.verifactu_pump(false).unwrap();
    let (accepted, with_errors, rejected) = sent_counts(&report);
    assert_eq!((accepted + with_errors, rejected), (1, 0));
    assert_eq!(h.view(b).status, "accepted_with_errors");
    let requests = h.requests();
    assert_eq!(requests.len(), 2);
    assert!(requests[1].contains("Otra Empresa Demo SL"));
    assert!(!requests[0].contains("Otra Empresa Demo SL"));
    assert_eq!(h.count("SELECT count(*) FROM verifactu_batches"), 2);
}

#[test]
fn the_header_uses_the_current_company_name_when_it_changed() {
    let mut h = Harness::new();
    h.emit_quiet();
    h.app
        .set_sales_company(&company("Taller Renombrado SL", "B12345674"))
        .unwrap();
    let report = h.app.verifactu_pump(false).unwrap();
    assert_eq!(sent_counts(&report), (1, 0, 0));
    let request = h.requests().pop().unwrap();
    assert!(request.contains("Taller Renombrado SL"), "{request}");
}

#[test]
fn the_last_error_is_visible_and_cleared_by_a_good_send() {
    let mut h = Harness::new();
    let server = serve(vec![
        (401, String::new()),
        (200, response_xml("Incorrecto", &[])),
    ]);
    h.app.set_verifactu_endpoint(Some(server.url.clone()));
    let doc = h.emit();
    let alta = h.alta(doc);
    let error = h.app.verifactu_status().unwrap().last_error.unwrap();
    assert_eq!(error.code, "unauthorized");
    assert!(error.message.contains("401"));
    assert_eq!(error.occurrences, 1);
    // Una respuesta `Incorrecto` sin ninguna línea no deja nada resuelto: también se ve.
    h.wait();
    match h.app.verifactu_pump(false).unwrap() {
        PumpReport::Sent { missing, .. } => assert_eq!(missing, 1),
        other => panic!("{other:?}"),
    }
    let error = h.app.verifactu_status().unwrap().last_error.unwrap();
    assert_eq!(error.code, "empty_response");
    assert_eq!(h.view(alta).status, "pending");
    // Un envío normal borra el error.
    h.app.set_verifactu_endpoint(None);
    h.wait();
    let report = h.app.verifactu_pump(false).unwrap();
    assert_eq!(sent_counts(&report), (1, 0, 0));
    assert!(h.app.verifactu_status().unwrap().last_error.is_none());
    // Y repetir el mismo código seguido lo cuenta.
    let doc2 = h.emit_quiet();
    let id = h.alta(doc2);
    let bad = serve(vec![(401, String::new())]);
    h.app.set_verifactu_endpoint(Some(bad.url.clone()));
    for _ in 0..3 {
        h.wait();
        let _ = h.app.verifactu_pump(false).unwrap();
    }
    let error = h.app.verifactu_status().unwrap().last_error.unwrap();
    assert_eq!(
        (error.code.as_str(), error.occurrences),
        ("unauthorized", 3)
    );
    assert_eq!(h.view(id).status, "sending");
}

#[test]
fn a_task_closed_by_fault_stays_failed_when_it_is_retried_or_recovered() {
    let mut h = Harness::new();
    h.app.set_verifactu_sim_mode(Some(SimMode::SchemaFault));
    let doc = h.emit();
    let alta = h.alta(doc);
    let task = h.submit_tasks().pop().unwrap();
    assert_eq!(task.state, TaskState::Failed);
    assert_eq!(task.error.as_ref().unwrap().code, "verifactu_fault");
    // «Reintentar» la deja en cola con su lote ya cerrado por el Fault: no se da por buena.
    h.app.retry(task.id).unwrap();
    assert!(matches!(
        h.app.verifactu_pump(false).unwrap(),
        PumpReport::Waiting { .. }
    ));
    let task = h
        .submit_tasks()
        .into_iter()
        .find(|t| t.id == task.id)
        .unwrap();
    assert_eq!(task.state, TaskState::Failed);
    assert_eq!(task.error.as_ref().unwrap().code, "verifactu_fault");
    assert!(task.result.is_none(), "no se completa con `recovered`");
    // La recuperación al abrir hace lo mismo con una tarea que quedó en cola.
    h.app.retry(task.id).unwrap();
    h.reopen();
    let task = h
        .submit_tasks()
        .into_iter()
        .find(|t| t.id == task.id)
        .unwrap();
    assert_eq!(task.state, TaskState::Failed);
    assert!(task.result.is_none());
    assert_eq!(h.view(alta).status, "pending");
}

#[test]
fn a_pending_record_of_inherited_production_is_not_shown_as_pending() {
    let mut h = Harness::new();
    let doc = h.emit_quiet();
    let alta = h.alta(doc);
    let conn = h.raw();
    conn.execute_batch("DROP TRIGGER verifactu_record_environments_no_update;")
        .unwrap();
    conn.execute(
        "UPDATE verifactu_record_environments SET environment = 'produccion_heredado' \
         WHERE record_id = ?1",
        [alta],
    )
    .unwrap();
    drop(conn);
    let status = h.app.verifactu_status().unwrap();
    assert_eq!(status.pending, 0, "la cabecera no lo cuenta");
    let view = status.records.iter().find(|r| r.record_id == alta).unwrap();
    assert_eq!(view.status, "other_environment", "la tabla tampoco");
    assert_eq!(view.environment, "produccion_heredado");
    assert_eq!(h.app.verifactu_pump(false).unwrap(), PumpReport::Idle);
    assert!(h.requests().is_empty());
}

#[test]
fn the_simulator_keeps_accepted_with_errors_when_seeded_after_reopening() {
    let mut h = Harness::new();
    h.app
        .set_verifactu_sim_mode(Some(SimMode::AcceptWithErrors));
    let doc = h.emit();
    let alta = h.alta(doc);
    assert_eq!(h.view(alta).status, "accepted_with_errors");
    h.reopen();
    let id = h.id_factura(alta);
    let key = (
        id.nif_emisor.clone(),
        id.num_serie.clone(),
        id.fecha_expedicion.to_string(),
    );
    let sim = h.app.verifactu_simulator().unwrap();
    let state = sim.estado();
    let stored = state.registros.get(&key).expect("sembrado");
    assert_eq!(stored.estado, "AceptadaConErrores");
}
