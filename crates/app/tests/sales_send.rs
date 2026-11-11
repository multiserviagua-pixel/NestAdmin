//! Envío de documentos de venta por correo con revisión (E4b, D232–D241; D197): la tarea
//! `send_sales_document`, su aprobación única, la recuperación sin doble envío (D50, D240), la
//! revisión nueva tras aprobar (D234), la anulación (D235), la plantilla y la cuenta (D236, D237)
//! y la regla por cliente al emitir (D238). Con el simulador de correo, un conector de prueba que
//! pierde la respuesta o no responde y PDFium real; solo datos sintéticos, sin cuentas reales ni
//! IA. Los efectos se miran en el estado del simulador.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::Arc;

use nestadmin_ai::{AiError, GenerateRequest, GenerateResult, Provider, ProviderKind};
use nestadmin_app::rules::SEND_SALES_DOCUMENT;
use nestadmin_app::sales::SalesClock;
use nestadmin_app::{App, AppPaths};
use nestadmin_core::sales::{Company, CustomerData, DraftInput, DraftLine};
use nestadmin_core::{Connector, ConnectorError, Task, TaskState};
use nestadmin_extract::image::{DynamicImage, RgbImage};
use nestadmin_extract::pdfium_render::prelude::*;
use nestadmin_extract::{ExtractError, Extractor, Ocr};
use nestadmin_sales::{
    DocumentKind, LineInput, RectificationKind, RectificationMethod, Status, TaxRate,
};
use nestadmin_simulators::{
    MailConnector, MailSimulator, MessagePage, NewDraft, SendReceipt, MAX_OUTGOING_ATTACHMENT_BYTES,
};
use nestadmin_verifactu::{Fecha, FechaHoraHuso};
use rust_decimal::Decimal;
use serde_json::{json, Value};

struct NoAi;

impl Provider for NoAi {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Ollama
    }
    fn model(&self) -> &str {
        "simulado"
    }
    fn generate(&self, _request: &GenerateRequest) -> Result<GenerateResult, AiError> {
        panic!("el envío de documentos no usa IA");
    }
}

struct NoOcr;

impl Ocr for NoOcr {
    fn recognize(&self, _image: &Path, page: usize) -> Result<String, ExtractError> {
        Err(ExtractError::OcrUnavailable(format!("página {page}")))
    }
}

struct FixedClock(AtomicI64);

impl SalesClock for FixedClock {
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

fn extractor() -> Extractor {
    Extractor::new(pdfium_dir(), Box::new(NoOcr))
}

fn open(dir: &Path) -> App {
    let mut app = App::open(&AppPaths::new(dir), Arc::new(NoAi), Arc::new(extractor())).unwrap();
    let base = FechaHoraHuso::new(Fecha::new(2026, 3, 1).unwrap(), 10, 0, 0, 60)
        .unwrap()
        .unix();
    app.set_sales_clock(Arc::new(FixedClock(AtomicI64::new(base))));
    app.set_verifactu_producer("Productor Demo SL", "B00000034")
        .unwrap();
    app
}

fn d(text: &str) -> Decimal {
    text.parse().unwrap()
}

fn line(description: &str) -> DraftLine {
    LineInput {
        description: description.into(),
        quantity: d("2"),
        unit_price: d("40"),
        discount: Decimal::ZERO,
        tax_rate: TaxRate::TwentyOne,
        exemption: None,
    }
    .into()
}

const CLIENT_EMAIL: &str = "cliente@example.test";

fn customer_data(email: Option<&str>) -> CustomerData {
    CustomerData {
        name: "Cliente Uno SA".into(),
        tax_id: Some("A58818501".into()),
        address: Some("Avenida Inventada 2, 08000 Barcelona".into()),
        email: email.map(str::to_owned),
    }
}

/// Empresa y un cliente con correo; devuelve el cliente.
fn setup(app: &mut App) -> i64 {
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
    app.create_customer(&customer_data(Some(CLIENT_EMAIL)))
        .unwrap()
}

fn issue(app: &mut App, kind: DocumentKind, customer: i64) -> i64 {
    let mut input = DraftInput::new(kind);
    input.customer_id = Some(customer);
    input.lines = vec![line("Revisión anual")];
    let id = app.create_sales_draft(&input).unwrap();
    app.issue_sales_document(id, None).unwrap();
    id
}

fn status(app: &App, id: i64) -> Status {
    app.sales_document(id).unwrap().status
}

fn code(error: &nestadmin_app::AppError) -> String {
    error.code().to_owned()
}

fn sent(app: &App) -> Vec<SendReceipt> {
    app.simulators().0.sent().unwrap()
}

fn result_of(app: &App, task: i64) -> Value {
    app.tasks()
        .unwrap()
        .into_iter()
        .find(|t| t.id == task)
        .unwrap()
        .result
        .unwrap()
}

fn task_of(app: &App, task: i64) -> Task {
    app.tasks()
        .unwrap()
        .into_iter()
        .find(|t| t.id == task)
        .unwrap()
}

fn process_all(app: &mut App) {
    while app.process_next().unwrap().is_some() {}
}

/// Un PDF de membrete de más de 3 MB con una imagen de ruido (no comprime) en el margen superior.
fn heavy_letterhead() -> Vec<u8> {
    let pdfium = extractor().pdfium().unwrap();
    let mut document = pdfium.create_new_pdf().unwrap();
    let mut page = document
        .pages_mut()
        .create_page_at_end(PdfPagePaperSize::a4())
        .unwrap();
    let (pw, ph) = (page.width().value, page.height().value);
    let mut state = 0x2545_F491_4F6C_DD1Du64;
    let mut noise = RgbImage::new(1300, 900);
    for pixel in noise.pixels_mut() {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        pixel.0 = [state as u8, (state >> 8) as u8, (state >> 16) as u8];
    }
    page.objects_mut()
        .create_image_object(
            PdfPoints::new(0.0),
            PdfPoints::new(ph - 60.0),
            &DynamicImage::ImageRgb8(noise),
            Some(PdfPoints::new(pw)),
            Some(PdfPoints::new(60.0)),
        )
        .unwrap();
    document.save_to_bytes().unwrap()
}

// ---- Conector de prueba: pierde la respuesta del envío o no responde a las consultas ----

#[derive(Default)]
struct Flags {
    /// El envío se hace y la respuesta se pierde.
    drop_send: AtomicBool,
    /// El envío falla sin efecto.
    fail_send: AtomicBool,
    lookup_unavailable: AtomicBool,
    lookup_not_found: AtomicBool,
}

struct Lossy {
    sim: Arc<MailSimulator>,
    flags: Arc<Flags>,
}

fn failure(code: &str) -> ConnectorError {
    ConnectorError {
        code: code.into(),
        message: "fallo simulado".into(),
    }
}

impl Connector for Lossy {
    fn lookup(&self, key: &str) -> Result<Option<Value>, ConnectorError> {
        if self.flags.lookup_not_found.load(Ordering::SeqCst) {
            return Err(failure("not_found"));
        }
        if self.flags.lookup_unavailable.load(Ordering::SeqCst) {
            return Err(failure("unavailable"));
        }
        self.sim.lookup(key)
    }
    fn execute(&self, key: &str, payload: &Value) -> Result<Value, ConnectorError> {
        if self.flags.fail_send.load(Ordering::SeqCst) {
            return Err(failure("unavailable"));
        }
        let receipt = self.sim.execute(key, payload)?;
        if self.flags.drop_send.load(Ordering::SeqCst) {
            return Err(failure("unavailable"));
        }
        Ok(receipt)
    }
}

impl MailConnector for Lossy {
    fn list_messages(&self, cursor: Option<&str>) -> Result<MessagePage, ConnectorError> {
        self.sim.list_messages(cursor)
    }
    fn create_draft(
        &self,
        message_id: &str,
        draft: &NewDraft,
        key: &str,
    ) -> Result<String, ConnectorError> {
        self.sim.create_draft(message_id, draft, key)
    }
    fn create_new_draft(&self, draft: &NewDraft, key: &str) -> Result<String, ConnectorError> {
        self.sim.create_new_draft(draft, key)
    }
    fn fetch_attachment(
        &self,
        message_id: &str,
        attachment: &str,
    ) -> Result<Vec<u8>, ConnectorError> {
        self.sim.fetch_attachment(message_id, attachment)
    }
    fn send_draft(&self, draft_id: &str, key: &str) -> Result<SendReceipt, ConnectorError> {
        self.sim.send_draft(draft_id, key)
    }
}

/// App cuya única cuenta es «lossy», con su propio simulador y sus interruptores.
fn lossy_app(dir: &Path) -> (App, i64, Arc<Flags>, Arc<MailSimulator>) {
    let mut app = open(dir);
    let customer = setup(&mut app);
    let sim = Arc::new(MailSimulator::in_memory());
    let flags = Arc::new(Flags::default());
    app.insert_mail_connector_for_tests(
        "lossy",
        Box::new(Lossy {
            sim: Arc::clone(&sim),
            flags: Arc::clone(&flags),
        }),
    );
    app.remove_mail_account("simulador").unwrap();
    (app, customer, flags, sim)
}

// ---------------------------------------------------------------------------------------------

#[test]
fn the_five_document_kinds_are_prepared_reviewed_and_sent_once_each() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = open(dir.path());
    let customer = setup(&mut app);
    let quote = issue(&mut app, DocumentKind::Quote, customer);
    let note = issue(&mut app, DocumentKind::DeliveryNote, customer);
    let invoice = issue(&mut app, DocumentKind::Invoice, customer);
    let simplified = issue(&mut app, DocumentKind::SimplifiedInvoice, customer);
    let original = issue(&mut app, DocumentKind::Invoice, customer);
    let corrective = app
        .create_corrective_draft(
            original,
            RectificationKind::R1,
            RectificationMethod::Substitution,
        )
        .unwrap();
    app.issue_sales_document(corrective, None).unwrap();

    let cases = [
        (
            quote,
            "Presupuesto P-2026-0001",
            "el presupuesto P-2026-0001",
        ),
        (note, "Albarán A-2026-0001", "el albarán A-2026-0001"),
        (invoice, "Factura F-2026-0001", "la factura F-2026-0001"),
        (
            simplified,
            "Factura simplificada FS-2026-0001",
            "la factura simplificada FS-2026-0001",
        ),
        (
            corrective,
            "Factura rectificativa FR-2026-0001",
            "la factura rectificativa FR-2026-0001",
        ),
    ];
    for (n, (id, subject, phrase)) in cases.into_iter().enumerate() {
        let task = app.prepare_sales_send(id).unwrap();
        assert_eq!(task.kind, SEND_SALES_DOCUMENT);
        // Nace en revisión: preparar no envía nada.
        assert_eq!(task.state, TaskState::AwaitingReview);
        assert_eq!(sent(&app).len(), n);
        let result = task.result.clone().unwrap();
        assert_eq!(result["draft"]["to"], json!([CLIENT_EMAIL]));
        assert_eq!(
            result["draft"]["subject"],
            format!("{subject} — Taller Demo SL")
        );
        let body = result["draft"]["body"].as_str().unwrap();
        assert!(
            body.starts_with("Hola, Cliente Uno SA:\n\nLe adjuntamos "),
            "{body}"
        );
        assert!(body.contains(phrase), "{body}");
        assert!(body.contains(" del 01/03/2026."), "{body}");
        assert!(body.ends_with("Un saludo,\nTaller Demo SL"), "{body}");
        assert!(!body.contains("None"));
        let meta = &result["draft"]["attachments"][0];
        assert_eq!(meta["mime"], "application/pdf");
        assert!(meta["size"].as_u64().unwrap() > 0);
        assert_eq!(meta["sha256"].as_str().unwrap().len(), 64);
        assert_eq!(result["account"]["id"], "simulador");
        assert_eq!(result["customer_email_at_prepare"], CLIENT_EMAIL);
        assert_eq!(result["warnings"]["recipient_differs"], false);

        let done = app.approve(task.id, 1).unwrap();
        assert_eq!(done.state, TaskState::Completed, "{subject}");
        assert_eq!(sent(&app).len(), n + 1, "{subject}: un solo envío");
        // El documento pasa a enviado y consta una fila con el adjunto y quién lo autorizó.
        assert_eq!(status(&app, id), Status::Sent, "{subject}");
        let rows = app.sales_document_sends(id).unwrap();
        assert_eq!(rows.len(), 1, "{subject}");
        assert_eq!(rows[0].recipient, CLIENT_EMAIL);
        assert_eq!(rows[0].authorization, "aprobación");
        assert_eq!(rows[0].attachment_sha256, meta["sha256"].as_str().unwrap());
        assert_eq!(
            rows[0].attachment_size as u64,
            meta["size"].as_u64().unwrap()
        );
        assert!(rows[0].attachment_name.ends_with(".pdf"));
        assert_eq!(rows[0].account, "simulador");
    }
    // El simulador recibió cada borrador con su adjunto (solo metadatos guardados).
    let drafts = app.simulators().0.drafts().unwrap();
    assert_eq!(drafts.len(), 5);
    assert!(drafts.iter().all(|d| d.standalone
        && d.content.attachments.len() == 1
        && d.content.attachments[0].content.is_empty()));
    // Esta tarea no deja señales de aprendizaje (no contamina D129).
    let suggestions = app.rule_suggestions().unwrap();
    assert_eq!((suggestions.observed, suggestions.decisions), (0, 0));
}

#[test]
fn a_sent_document_can_be_resent_without_changing_its_status() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = open(dir.path());
    let customer = setup(&mut app);
    let quote = issue(&mut app, DocumentKind::Quote, customer);
    let first = app.prepare_sales_send(quote).unwrap();
    app.approve(first.id, 1).unwrap();
    assert_eq!(status(&app, quote), Status::Sent);

    let second = app.prepare_sales_send(quote).unwrap();
    app.approve(second.id, 1).unwrap();
    assert_eq!(status(&app, quote), Status::Sent);
    assert_eq!(app.sales_document_sends(quote).unwrap().len(), 2);
    assert_eq!(sent(&app).len(), 2);

    // Un albarán entregado también se reenvía y sigue entregado.
    let note = issue(&mut app, DocumentKind::DeliveryNote, customer);
    app.transition_sales_document(note, Status::Delivered)
        .unwrap();
    let task = app.prepare_sales_send(note).unwrap();
    app.approve(task.id, 1).unwrap();
    assert_eq!(status(&app, note), Status::Delivered);
    assert_eq!(app.sales_document_sends(note).unwrap().len(), 1);
}

#[test]
fn states_that_cannot_be_sent_are_refused_with_their_code_and_create_no_task() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = open(dir.path());
    let customer = setup(&mut app);

    let mut input = DraftInput::new(DocumentKind::Quote);
    input.customer_id = Some(customer);
    input.lines = vec![line("Borrador")];
    let draft = app.create_sales_draft(&input).unwrap();
    let accepted = issue(&mut app, DocumentKind::Quote, customer);
    app.transition_sales_document(accepted, Status::Accepted)
        .unwrap();
    let rejected = issue(&mut app, DocumentKind::Quote, customer);
    app.transition_sales_document(rejected, Status::Rejected)
        .unwrap();
    let voided = issue(&mut app, DocumentKind::Invoice, customer);
    app.void_sales_invoice(voided).unwrap();
    // Una revisión nueva sustituye a la anterior: la antigua ya no se envía.
    let old = issue(&mut app, DocumentKind::Quote, customer);
    let revision = app.revise_sales_document(old).unwrap();
    app.issue_sales_document(revision, None).unwrap();

    for (id, expected) in [
        (draft, "status_not_sendable"),
        (accepted, "status_not_sendable"),
        (rejected, "status_not_sendable"),
        (voided, "document_voided"),
        (old, "document_revised"),
    ] {
        let error = app.prepare_sales_send(id).unwrap_err();
        assert_eq!(code(&error), expected, "{id}: {error}");
    }
    assert!(app.prepare_sales_send(9999).is_err());
    assert!(app
        .tasks()
        .unwrap()
        .iter()
        .all(|t| t.kind != SEND_SALES_DOCUMENT));
    assert!(sent(&app).is_empty());
}

#[test]
fn the_content_is_validated_before_any_attempt_and_a_missing_recipient_is_explicit() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = open(dir.path());
    setup(&mut app);
    let bare = app
        .create_customer(&CustomerData {
            name: "Sin Correo SL".into(),
            tax_id: Some("B12345674".into()),
            address: Some("Calle Falsa 3, 28001 Madrid".into()),
            email: None,
        })
        .unwrap();
    let id = issue(&mut app, DocumentKind::Quote, bare);
    let task = app.prepare_sales_send(id).unwrap();
    let result = task.result.clone().unwrap();
    assert_eq!(result["draft"]["to"], json!([]));
    assert!(result["customer_email_at_prepare"].is_null());
    assert_eq!(result["checks"]["recipient"], "missing");

    // Aprobar sin destinatario: error explícito, sin intento ni acción ni correo.
    let error = app.approve(task.id, 1).unwrap_err();
    assert_eq!(code(&error), "recipient_missing");
    assert!(sent(&app).is_empty());
    assert!(task_of(&app, task.id)
        .result
        .unwrap()
        .get("attempt")
        .is_none());
    assert_eq!(task_of(&app, task.id).state, TaskState::AwaitingReview);

    let bad = |app: &mut App, to: &str, subject: &str, body: &str, account: &str| {
        code(
            &app.edit_sales_send(task.id, to, subject, body, account)
                .unwrap_err(),
        )
    };
    assert_eq!(
        bad(&mut app, "", "Asunto", "Texto", "simulador"),
        "recipient_missing"
    );
    for to in [
        "no es una dirección",
        "a@ejemplo.test, b@ejemplo.test",
        "Nombre <a@ejemplo.test>",
        "a@ejemplo.test\r\nBcc: x@ejemplo.test",
        "a@@ejemplo.test",
    ] {
        assert_eq!(
            bad(&mut app, to, "Asunto", "Texto", "simulador"),
            "recipient_invalid",
            "{to:?}"
        );
    }
    assert_eq!(
        bad(&mut app, "a@ejemplo.test", "", "Texto", "simulador"),
        "subject_invalid"
    );
    assert_eq!(
        bad(
            &mut app,
            "a@ejemplo.test",
            "Asunto\r\nBcc: x@ejemplo.test",
            "Texto",
            "simulador"
        ),
        "subject_invalid"
    );
    assert_eq!(
        bad(
            &mut app,
            "a@ejemplo.test",
            &"x".repeat(201),
            "Texto",
            "simulador"
        ),
        "subject_invalid"
    );
    assert_eq!(
        bad(&mut app, "a@ejemplo.test", "Asunto", "  ", "simulador"),
        "body_empty"
    );
    assert_eq!(
        bad(&mut app, "a@ejemplo.test", "Asunto", "Texto", ""),
        "account_required"
    );
    assert_eq!(
        bad(&mut app, "a@ejemplo.test", "Asunto", "Texto", "no-existe"),
        "account_unavailable"
    );

    // Con un destinatario válido hay una versión nueva que se aprueba aparte.
    let edited = app
        .edit_sales_send(
            task.id,
            "contabilidad@ejemplo.test",
            "Presupuesto",
            "Texto",
            "simulador",
        )
        .unwrap();
    let result = edited.result.unwrap();
    assert_eq!(result["version"], 2);
    assert_eq!(result["edited"], true);
    assert_eq!(result["draft"]["to"], json!(["contabilidad@ejemplo.test"]));
    assert_eq!(result["draft"]["body"], "Texto");
    assert_eq!(
        result["warnings"]["recipient_differs"], false,
        "sin correo del cliente no hay con qué comparar"
    );
    let stale = app.approve(task.id, 1).unwrap_err();
    assert_eq!(code(&stale), "stale_version");
    assert!(sent(&app).is_empty());
    let done = app.approve(task.id, 2).unwrap();
    assert_eq!(done.state, TaskState::Completed);
    assert_eq!(sent(&app).len(), 1);
    assert_eq!(
        app.sales_document_sends(id).unwrap()[0].recipient,
        "contabilidad@ejemplo.test"
    );
}

#[test]
fn a_pdf_over_3_mb_is_refused_with_a_clear_error_and_nothing_is_queued_or_sent() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = open(dir.path());
    let customer = setup(&mut app);
    app.set_sales_letterhead(&heavy_letterhead()).unwrap();
    let id = issue(&mut app, DocumentKind::Quote, customer);
    let bytes = app
        .render_sales_pdf(&app.sales_document(id).unwrap())
        .unwrap();
    assert!(
        bytes.len() as u64 > MAX_OUTGOING_ATTACHMENT_BYTES,
        "el PDF de prueba debe pasar de 3 MB: {}",
        bytes.len()
    );
    let error = app.prepare_sales_send(id).unwrap_err();
    assert_eq!(code(&error), "attachment_too_large");
    assert!(error.to_string().contains("3 MB"), "{error}");
    assert!(app
        .tasks()
        .unwrap()
        .iter()
        .all(|t| t.kind != SEND_SALES_DOCUMENT));
    assert!(sent(&app).is_empty());
    assert_eq!(status(&app, id), Status::Issued);
}

#[test]
fn approving_twice_sends_once() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = open(dir.path());
    let customer = setup(&mut app);
    let id = issue(&mut app, DocumentKind::Invoice, customer);
    let task = app.prepare_sales_send(id).unwrap();
    // Solo hay una tarea abierta por documento.
    assert_eq!(
        code(&app.prepare_sales_send(id).unwrap_err()),
        "send_in_progress"
    );
    app.approve(task.id, 1).unwrap();
    assert!(
        app.approve(task.id, 1).is_err(),
        "la tarea ya no está en revisión"
    );
    assert_eq!(sent(&app).len(), 1);
    assert_eq!(app.sales_document_sends(id).unwrap().len(), 1);
    // Ni siquiera reabriendo la aplicación sobre los mismos datos.
    drop(app);
    let app = open(dir.path());
    assert_eq!(sent(&app).len(), 1);
    assert_eq!(app.sales_document_sends(id).unwrap().len(), 1);
    assert_eq!(status(&app, id), Status::Sent);
}

#[test]
fn a_cut_between_the_attempt_and_the_receipt_never_duplicates_and_never_says_not_sent() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, customer, flags, sim) = lossy_app(dir.path());
    let id = issue(&mut app, DocumentKind::Invoice, customer);
    let task = app.prepare_sales_send(id).unwrap();
    assert_eq!(result_of(&app, task.id)["account"]["id"], "lossy");

    // El correo sale y la respuesta se pierde: la tarea falla con el intento anotado.
    flags.drop_send.store(true, Ordering::SeqCst);
    let failed = app.approve(task.id, 1).unwrap();
    assert_eq!(failed.state, TaskState::Failed);
    assert_eq!(sim.sent().unwrap().len(), 1);
    assert_eq!(status(&app, id), Status::Issued, "sin recibo no hay `sent`");
    assert!(app.sales_document_sends(id).unwrap().is_empty());
    assert_eq!(
        result_of(&app, task.id)["attempt_accounts"]["1"],
        "lossy",
        "consta la cuenta que lo intentó"
    );

    // La cuenta no responde: no se sabe si salió, y nunca se dice «no enviado» ni se reenvía.
    flags.drop_send.store(false, Ordering::SeqCst);
    flags.lookup_unavailable.store(true, Ordering::SeqCst);
    app.retry(task.id).unwrap();
    process_all(&mut app);
    let review = task_of(&app, task.id);
    assert_eq!(review.state, TaskState::AwaitingReview);
    let result = review.result.unwrap();
    assert_eq!(result["recovery"]["error"], "unavailable");
    assert_eq!(result["recovery"]["needs_confirmation"], false);
    let blocked = app.approve(task.id, 1).unwrap();
    assert_eq!(blocked.state, TaskState::Failed);
    assert_eq!(blocked.error.unwrap().code, "unavailable");
    assert_eq!(sim.sent().unwrap().len(), 1, "sigue habiendo un solo envío");
    // «Confirmo que no se envió» tampoco vale con la cuenta sin responder.
    app.retry(task.id).unwrap();
    process_all(&mut app);
    let error = app.confirm_not_sent(task.id).unwrap_err();
    assert_eq!(code(&error), "unavailable");

    // Con la cuenta de vuelta se encuentra el recibo y se completa sin enviar otra vez.
    flags.lookup_unavailable.store(false, Ordering::SeqCst);
    let done = app.confirm_not_sent(task.id).unwrap();
    assert_eq!(done.state, TaskState::Completed);
    assert_eq!(sim.sent().unwrap().len(), 1);
    assert_eq!(status(&app, id), Status::Sent);
    let rows = app.sales_document_sends(id).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].account, "lossy");
    assert_eq!(result_of(&app, task.id)["recovered"]["version"], 1);
}

#[test]
fn a_cut_leaves_the_receipt_to_be_found_when_the_app_reopens() {
    let dir = tempfile::tempdir().unwrap();
    let id;
    let task_id;
    {
        let (mut app, customer, flags, _sim) = lossy_app(dir.path());
        id = issue(&mut app, DocumentKind::Quote, customer);
        let task = app.prepare_sales_send(id).unwrap();
        task_id = task.id;
        flags.drop_send.store(true, Ordering::SeqCst);
        assert_eq!(app.approve(task.id, 1).unwrap().state, TaskState::Failed);
    }
    // Reabrir descarta la cuenta de prueba (solo vive en memoria; también su entrada en la lista de
    // cuentas): el envío no se puede comprobar y, sin la cuenta, no se afirma nada ni se envía
    // otra vez.
    std::fs::remove_file(dir.path().join("correo").join("cuentas.json")).unwrap();
    let mut app = open(dir.path());
    assert_eq!(status(&app, id), Status::Issued);
    app.retry(task_id).unwrap();
    process_all(&mut app);
    let result = result_of(&app, task_id);
    assert_eq!(result["recovery"]["error"], "unavailable");
    assert_eq!(result["recovery"]["needs_confirmation"], false);
    assert!(sent(&app).is_empty());
    assert!(app.sales_document_sends(id).unwrap().is_empty());
    let error = app.confirm_not_sent(task_id).unwrap_err();
    assert_eq!(code(&error), "unavailable");
}

#[test]
fn a_disconnected_account_gives_unavailable() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, customer, flags, sim) = lossy_app(dir.path());
    let id = issue(&mut app, DocumentKind::Quote, customer);
    let task = app.prepare_sales_send(id).unwrap();
    // Aprobar con la cuenta desconectada: error y nada intentado.
    app.remove_mail_account("lossy").unwrap();
    let error = app.approve(task.id, 1).unwrap_err();
    assert_eq!(code(&error), "account_unavailable");
    assert!(task_of(&app, task.id)
        .result
        .unwrap()
        .get("attempt")
        .is_none());
    assert!(sim.sent().unwrap().is_empty());
    // Y no se puede preparar un envío nuevo sin ninguna cuenta capaz de enviar.
    let none = app.prepare_sales_send(id).unwrap_err();
    assert_eq!(code(&none), "send_in_progress", "ya hay uno abierto");
    app.reject(task.id).unwrap();
    assert_eq!(
        code(&app.prepare_sales_send(id).unwrap_err()),
        "no_mail_account"
    );
    let _ = flags;
}

#[test]
fn a_send_that_fails_before_leaving_is_confirmed_unsent_with_a_new_version() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, customer, flags, sim) = lossy_app(dir.path());
    let id = issue(&mut app, DocumentKind::Quote, customer);
    let task = app.prepare_sales_send(id).unwrap();
    flags.fail_send.store(true, Ordering::SeqCst);
    assert_eq!(app.approve(task.id, 1).unwrap().state, TaskState::Failed);
    assert!(sim.sent().unwrap().is_empty());

    // El borrador del intento ya no existe para el conector: pide confirmar que no se envió.
    flags.fail_send.store(false, Ordering::SeqCst);
    flags.lookup_not_found.store(true, Ordering::SeqCst);
    app.retry(task.id).unwrap();
    process_all(&mut app);
    let review = task_of(&app, task.id);
    assert_eq!(review.state, TaskState::AwaitingReview);
    let result = review.result.unwrap();
    assert_eq!(result["recovery"]["error"], "not_found");
    assert_eq!(result["recovery"]["needs_confirmation"], true);
    // Aprobarlo tal cual no envía: falla visible y sigue exigiendo la confirmación.
    let again = app.approve(task.id, 1).unwrap();
    assert_eq!(again.state, TaskState::Failed);
    assert!(sim.sent().unwrap().is_empty());
    app.retry(task.id).unwrap();
    process_all(&mut app);

    // «Confirmo que no se envió»: versión nueva sin enviar nada; luego se aprueba aparte.
    let confirmed = app.confirm_not_sent(task.id).unwrap();
    assert_eq!(confirmed.state, TaskState::AwaitingReview);
    assert_eq!(confirmed.result.unwrap()["version"], 2);
    assert!(sim.sent().unwrap().is_empty(), "confirmar no envía");
    flags.lookup_not_found.store(false, Ordering::SeqCst);
    let done = app.approve(task.id, 2).unwrap();
    assert_eq!(done.state, TaskState::Completed);
    assert_eq!(sim.sent().unwrap().len(), 1);
    assert_eq!(app.sales_document_sends(id).unwrap().len(), 1);
    assert_eq!(app.sales_document_sends(id).unwrap()[0].version, 2);
}

#[test]
fn a_new_revision_after_the_review_requires_a_new_approval() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = open(dir.path());
    let customer = setup(&mut app);
    let quote = issue(&mut app, DocumentKind::Quote, customer);
    let task = app.prepare_sales_send(quote).unwrap();
    let before = task.result.unwrap();
    assert_eq!(before["document"]["revision"], 1);

    // Mientras la tarea espera, el presupuesto se revisa y la revisión se emite.
    let revision = app.revise_sales_document(quote).unwrap();
    app.issue_sales_document(revision, None).unwrap();

    // Aprobar la versión vista no envía el documento antiguo: vuelve a revisión con la vigente.
    let back = app.approve(task.id, 1).unwrap();
    assert_eq!(back.state, TaskState::AwaitingReview);
    assert!(sent(&app).is_empty());
    let result = back.result.unwrap();
    assert_eq!(result["version"], 2);
    assert_eq!(result["document"]["id"], revision);
    assert_eq!(result["document"]["revision"], 2);
    assert!(result["review_reason"]
        .as_str()
        .unwrap()
        .contains("revisión nueva"));
    assert!(result["draft"]["subject"]
        .as_str()
        .unwrap()
        .contains("(revisión 2)"));
    assert_ne!(
        result["draft"]["attachments"][0]["sha256"], before["draft"]["attachments"][0]["sha256"],
        "PDF y SHA-256 nuevos"
    );
    assert_eq!(code(&app.approve(task.id, 1).unwrap_err()), "stale_version");
    assert!(sent(&app).is_empty());

    let done = app.approve(task.id, 2).unwrap();
    assert_eq!(done.state, TaskState::Completed);
    assert_eq!(sent(&app).len(), 1);
    assert_eq!(status(&app, revision), Status::Sent);
    assert_eq!(app.sales_document_sends(revision).unwrap().len(), 1);
    assert!(app.sales_document_sends(quote).unwrap().is_empty());
}

#[test]
fn voiding_with_a_pending_attempt_is_blocked_and_with_only_a_review_it_cancels_the_task() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, customer, flags, sim) = lossy_app(dir.path());

    // Solo en revisión: anular la factura cancela el envío.
    let quiet = issue(&mut app, DocumentKind::Invoice, customer);
    let review = app.prepare_sales_send(quiet).unwrap();
    app.void_sales_invoice(quiet).unwrap();
    assert_eq!(status(&app, quiet), Status::Voided);
    assert_eq!(task_of(&app, review.id).state, TaskState::Cancelled);

    // Con un intento que pudo salir: no se anula ni se descarta el envío.
    let id = issue(&mut app, DocumentKind::Invoice, customer);
    let task = app.prepare_sales_send(id).unwrap();
    flags.drop_send.store(true, Ordering::SeqCst);
    assert_eq!(app.approve(task.id, 1).unwrap().state, TaskState::Failed);
    let error = app.void_sales_invoice(id).unwrap_err();
    assert_eq!(code(&error), "send_in_progress");
    assert_eq!(status(&app, id), Status::Issued);
    assert_eq!(code(&app.reject(task.id).unwrap_err()), "send_in_progress");
    assert_eq!(task_of(&app, task.id).state, TaskState::Failed);
    assert_eq!(sim.sent().unwrap().len(), 1);

    // Recuperado el recibo, la factura queda enviada y ya no se anula (se rectifica, D82).
    flags.drop_send.store(false, Ordering::SeqCst);
    app.retry(task.id).unwrap();
    process_all(&mut app);
    assert_eq!(task_of(&app, task.id).state, TaskState::Completed);
    assert_eq!(status(&app, id), Status::Sent);
    assert!(app.void_sales_invoice(id).is_err());
}

#[test]
fn a_pdf_that_changed_since_the_review_goes_back_to_review_with_pdf_changed() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = open(dir.path());
    let customer = setup(&mut app);
    let id = issue(&mut app, DocumentKind::Quote, customer);
    let task = app.prepare_sales_send(id).unwrap();
    let real = task.result.unwrap()["draft"]["attachments"][0]["sha256"]
        .as_str()
        .unwrap()
        .to_owned();

    // Lo que la persona vio ya no es lo que se genera: se altera el SHA-256 guardado.
    let conn = rusqlite::Connection::open(dir.path().join("nestadmin.db")).unwrap();
    conn.execute(
        "UPDATE tasks SET result = json_set(result, '$.draft.attachments[0].sha256', ?2) \
         WHERE id = ?1",
        rusqlite::params![task.id, "0".repeat(64)],
    )
    .unwrap();
    drop(conn);

    let back = app.approve(task.id, 1).unwrap();
    assert_eq!(back.state, TaskState::AwaitingReview);
    assert!(sent(&app).is_empty());
    let result = back.result.unwrap();
    assert_eq!(result["version"], 2);
    assert!(result["review_reason"].as_str().unwrap().contains("PDF"));
    assert_eq!(result["pdf_changed"]["to"], real);
    assert_eq!(result["draft"]["attachments"][0]["sha256"], real);
    assert!(result["attempt"].is_null(), "no se intentó nada");
    // Aprobar la versión nueva sí envía, con el PDF que se ve.
    let done = app.approve(task.id, 2).unwrap();
    assert_eq!(done.state, TaskState::Completed);
    assert_eq!(
        app.sales_document_sends(id).unwrap()[0].attachment_sha256,
        real
    );
}

fn rule_conditions(customer: i64, recipient: &str) -> Value {
    json!({ "customer_id": customer, "recipient": recipient, "account": "simulador" })
}

fn activate(app: &mut App, id: &str, conditions: &Value) {
    app.create_rule(id, SEND_SALES_DOCUMENT, conditions)
        .unwrap();
    app.set_rule_active(id, true).unwrap();
}

#[test]
fn the_rule_sends_on_issue_only_with_every_condition_and_only_once() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = open(dir.path());
    let customer = setup(&mut app);

    // Las tres condiciones son obligatorias: no hay «cualquier cliente, destinatario o cuenta».
    for missing in ["customer_id", "recipient", "account"] {
        let mut conditions = rule_conditions(customer, CLIENT_EMAIL);
        conditions.as_object_mut().unwrap().remove(missing);
        assert!(
            app.create_rule("incompleta", SEND_SALES_DOCUMENT, &conditions)
                .is_err(),
            "{missing}"
        );
    }
    assert!(app
        .create_rule(
            "otra",
            SEND_SALES_DOCUMENT,
            &json!({ "customer_id": customer, "recipient": CLIENT_EMAIL, "account": "simulador", "x": 1 })
        )
        .is_err());
    // Una regla creada nace inactiva: sin activar, emitir no envía nada.
    app.create_rule(
        "envio-cliente",
        SEND_SALES_DOCUMENT,
        &rule_conditions(customer, CLIENT_EMAIL),
    )
    .unwrap();
    let inactive = issue(&mut app, DocumentKind::Quote, customer);
    assert!(sent(&app).is_empty());
    assert!(app.sales_send_task(inactive).unwrap().is_none());
    assert_eq!(status(&app, inactive), Status::Issued);

    app.set_rule_active("envio-cliente", true).unwrap();
    let id = issue(&mut app, DocumentKind::Quote, customer);
    assert_eq!(sent(&app).len(), 1, "se envió al emitir");
    assert_eq!(status(&app, id), Status::Sent);
    let rows = app.sales_document_sends(id).unwrap();
    assert_eq!(rows.len(), 1);
    assert!(rows[0].authorization.starts_with("regla envio-cliente v"));
    let label = rows[0].authorization.clone();
    assert_eq!(rows[0].recipient, CLIENT_EMAIL);
    // Una sola tarea de envío, completada por la regla; sin señales de aprendizaje.
    let tasks: Vec<Task> = app
        .tasks()
        .unwrap()
        .into_iter()
        .filter(|t| t.kind == SEND_SALES_DOCUMENT && t.input_ref.ends_with(&id.to_string()))
        .collect();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].state, TaskState::Completed);
    assert_eq!(
        tasks[0].result.as_ref().unwrap()["authorization"]["type"],
        "rule"
    );
    assert!(app
        .audit_log()
        .unwrap()
        .iter()
        .any(|e| e.action == SEND_SALES_DOCUMENT
            && e.authorization.as_deref() == Some(label.as_str())));
    let suggestions = app.rule_suggestions().unwrap();
    assert_eq!((suggestions.observed, suggestions.decisions), (0, 0));

    // Reabrir no vuelve a enviar ni a encolar nada.
    drop(app);
    let app = open(dir.path());
    assert_eq!(sent(&app).len(), 1);
    assert_eq!(app.sales_document_sends(id).unwrap().len(), 1);
}

#[test]
fn the_rule_does_not_send_if_the_customer_email_changed_or_is_missing() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = open(dir.path());
    let customer = setup(&mut app);
    activate(
        &mut app,
        "envio-cliente",
        &rule_conditions(customer, CLIENT_EMAIL),
    );

    // El correo del cliente cambió desde que se escribió la regla.
    app.update_customer(customer, &customer_data(Some("nuevo@example.test")))
        .unwrap();
    let id = issue(&mut app, DocumentKind::Quote, customer);
    assert!(sent(&app).is_empty());
    let task = app.sales_send_task(id).unwrap().expect("queda en revisión");
    assert_eq!(task.state, TaskState::AwaitingReview);
    let result = task.result.unwrap();
    assert_eq!(result["evaluation"]["code"], "recipient_changed");
    assert_eq!(result["draft"]["to"], json!(["nuevo@example.test"]));
    assert_eq!(status(&app, id), Status::Issued);

    // Sin correo del cliente: tampoco, y la tarea explica que falta.
    app.update_customer(customer, &customer_data(None)).unwrap();
    let other = issue(&mut app, DocumentKind::Quote, customer);
    assert!(sent(&app).is_empty());
    let result = app.sales_send_task(other).unwrap().unwrap().result.unwrap();
    assert_eq!(result["evaluation"]["code"], "recipient_changed");
    assert_eq!(result["checks"]["recipient"], "missing");
    assert_eq!(status(&app, other), Status::Issued);

    // Otro cliente, aunque exista la regla, no dispara nada.
    let stranger = app
        .create_customer(&CustomerData {
            name: "Otro Cliente SL".into(),
            tax_id: Some("B12345674".into()),
            address: Some("Calle Falsa 3, 28001 Madrid".into()),
            email: Some(CLIENT_EMAIL.into()),
        })
        .unwrap();
    let unrelated = issue(&mut app, DocumentKind::Quote, stranger);
    assert!(app.sales_send_task(unrelated).unwrap().is_none());
    assert!(sent(&app).is_empty());

    // La persona puede aprobar la tarea que quedó en revisión; la regla no lo hace por ella.
    app.update_customer(customer, &customer_data(Some("nuevo@example.test")))
        .unwrap();
    let done = app
        .approve(app.sales_send_task(id).unwrap().unwrap().id, 1)
        .unwrap();
    assert_eq!(done.state, TaskState::Completed);
    assert_eq!(
        app.sales_document_sends(id).unwrap()[0].authorization,
        "aprobación"
    );
}

#[test]
fn the_rule_with_document_kinds_ignores_other_kinds_and_a_blocked_send_stays_in_review() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = open(dir.path());
    let customer = setup(&mut app);
    let mut conditions = rule_conditions(customer, CLIENT_EMAIL);
    conditions["document_kinds"] = json!(["invoice"]);
    activate(&mut app, "solo-facturas", &conditions);

    let quote = issue(&mut app, DocumentKind::Quote, customer);
    assert!(sent(&app).is_empty());
    let task = app.sales_send_task(quote).unwrap().unwrap();
    assert_eq!(
        task.result.unwrap()["evaluation"]["code"],
        "no_active_rule_matches"
    );

    let invoice = issue(&mut app, DocumentKind::Invoice, customer);
    assert_eq!(sent(&app).len(), 1);
    assert_eq!(status(&app, invoice), Status::Sent);

    // Una regla con una cuenta que no es la elegida no automatiza.
    let mut conditions = rule_conditions(customer, CLIENT_EMAIL);
    conditions["account"] = json!("otra-cuenta");
    app.update_rule("solo-facturas", &conditions).unwrap();
    let second = issue(&mut app, DocumentKind::Invoice, customer);
    assert_eq!(sent(&app).len(), 1);
    assert_eq!(status(&app, second), Status::Issued);
    assert!(app.sales_send_task(second).unwrap().is_some());
}

#[test]
fn rules_of_this_kind_are_simulated_read_only_and_never_open_the_default() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = open(dir.path());
    let customer = setup(&mut app);
    let id = issue(&mut app, DocumentKind::Quote, customer);
    app.prepare_sales_send(id).unwrap();
    let tasks_before = app.tasks().unwrap().len();
    let sim = app
        .simulate_rule(
            SEND_SALES_DOCUMENT,
            &rule_conditions(customer, CLIENT_EMAIL),
            None,
            false,
            None,
        )
        .unwrap();
    assert_eq!(sim.considered, 1);
    assert_eq!(sim.would_automate, 1);
    // Solo lectura: no creó, envió ni cambió nada.
    assert_eq!(app.tasks().unwrap().len(), tasks_before);
    assert!(sent(&app).is_empty());
    assert!(app.rules().unwrap().is_empty());
    // Una acción desconocida no se evalúa ni se crea.
    assert!(app.create_rule("x", "send_everything", &json!({})).is_err());
}
