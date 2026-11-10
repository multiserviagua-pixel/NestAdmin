//! Facturación de venta en la capa de aplicación (fase 11, E2; D197): reloj inyectable, identidad
//! del sistema informático Verifactu, número de instalación y flujo presupuesto -> factura
//! registrada. Prueba independiente de las demás. Solo datos sintéticos (NIF de prueba) y un
//! proveedor de IA simulado que nunca se llama: la venta no usa IA ni red.

use std::path::Path;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

use nestadmin_ai::{AiError, GenerateRequest, GenerateResult, Provider, ProviderKind};
use nestadmin_app::sales::{SalesClock, CODE_NOT_CONFIGURED, SIF_VERSION};
use nestadmin_app::{App, AppPaths};
use nestadmin_core::sales::{
    ChainStatus, Company, CustomerData, DraftInput, VerifactuKind, CODE_CLOCK_BEHIND,
};
use nestadmin_documents::IsoDate;
use nestadmin_extract::{ExtractError, Extractor, Ocr};
use nestadmin_sales::{CollectionState, DocumentKind, PaymentMethod, Product, Status, TaxRate};
use nestadmin_verifactu::{Fecha, FechaHoraHuso, RegistroFacturacion};
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
        Err(AiError::Unavailable("la venta no usa IA".into()))
    }
}

struct NoOcr;

impl Ocr for NoOcr {
    fn recognize(&self, _image: &Path, page: usize) -> Result<String, ExtractError> {
        Err(ExtractError::OcrUnavailable(format!("página {page}")))
    }
}

/// Reloj de pruebas: segundos Unix desplazables, siempre con huso +01:00.
struct FixedClock(AtomicI64);

impl FixedClock {
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
}

impl SalesClock for FixedClock {
    fn now(&self) -> FechaHoraHuso {
        FechaHoraHuso::desde_unix(self.0.load(Ordering::SeqCst), 60).unwrap()
    }
}

fn open_at(dir: &Path) -> App {
    let extractor = Extractor::new(dir.join("sin-pdfium"), Box::new(NoOcr));
    App::open(&AppPaths::new(dir), Arc::new(NoAi), Arc::new(extractor)).unwrap()
}

/// App con el reloj fijo y, si se pide, el productor sintético configurado.
fn app(dir: &tempfile::TempDir, producer: bool) -> (App, Arc<FixedClock>) {
    let mut app = open_at(dir.path());
    let clock = FixedClock::new();
    app.set_sales_clock(clock.clone());
    if producer {
        app.set_verifactu_producer("Productor Demo SL", "B00000034")
            .unwrap();
    }
    (app, clock)
}

fn d(text: &str) -> Decimal {
    text.parse().unwrap()
}

fn date(text: &str) -> IsoDate {
    IsoDate::parse("test", text).unwrap()
}

/// Empresa, cliente y producto (80 EUR al 21 %); devuelve (cliente, producto).
fn setup(app: &mut App) -> (i64, i64) {
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
    (customer, product)
}

/// Borrador de `kind` con una unidad del producto.
fn draft(app: &mut App, kind: DocumentKind, customer: i64, product: i64) -> i64 {
    let mut input = DraftInput::new(kind);
    input.customer_id = Some(customer);
    let id = app.create_sales_draft(&input).unwrap();
    app.add_sales_line_from_product(id, product, d("1"), Decimal::ZERO)
        .unwrap();
    id
}

fn raw(dir: &tempfile::TempDir) -> rusqlite::Connection {
    rusqlite::Connection::open(dir.path().join("nestadmin.db")).unwrap()
}

fn installation_in_db(dir: &tempfile::TempDir) -> Option<String> {
    raw(dir)
        .query_row("SELECT number FROM installation", [], |r| r.get(0))
        .ok()
}

fn last_invoice_number(dir: &tempfile::TempDir) -> Option<i64> {
    raw(dir)
        .query_row("SELECT last FROM series WHERE kind = 'invoice'", [], |r| {
            r.get(0)
        })
        .ok()
}

#[test]
fn quote_to_registered_invoice_end_to_end() {
    let dir = tempfile::tempdir().unwrap();
    // Sin productor: el presupuesto se emite igual.
    let (mut app, _clock) = app(&dir, false);
    let (customer, product) = setup(&mut app);

    let quote = draft(&mut app, DocumentKind::Quote, customer, product);
    let issued = app.issue_sales_document(quote, None).unwrap();
    assert_eq!(issued.status, Status::Issued);
    assert!(issued.number.is_some());
    assert!(app.verifactu_records().unwrap().is_empty());
    assert_eq!(app.verifactu_chain_status().unwrap(), ChainStatus::Empty);
    assert_eq!(
        installation_in_db(&dir),
        None,
        "no se genera sin necesitarlo"
    );

    app.transition_sales_document(quote, Status::Accepted)
        .unwrap();
    let invoice = app
        .convert_sales_documents(&[quote], DocumentKind::Invoice)
        .unwrap();
    assert_eq!(app.sales_document(quote).unwrap().status, Status::Converted);
    assert_eq!(app.sales_links(invoice).unwrap().len(), 1);

    app.set_verifactu_producer("Productor Demo SL", "B00000034")
        .unwrap();
    let invoice_doc = app.issue_sales_document(invoice, None).unwrap();
    assert_eq!(invoice_doc.number.as_deref(), Some("F-2026-0001"));

    let records = app.verifactu_records().unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].kind, VerifactuKind::Alta);
    assert_eq!(records[0].document_id, invoice);
    assert_eq!(
        app.verifactu_chain_status().unwrap(),
        ChainStatus::Ok { records: 1 }
    );
    let RegistroFacturacion::Alta(alta) = &records[0].record else {
        panic!("se esperaba un alta");
    };
    let system = &alta.sistema_informatico;
    assert_eq!(system.numero_instalacion, installation_in_db(&dir).unwrap());
    assert_eq!(system.version, SIF_VERSION);
    assert_eq!(system.nombre_sistema, "NestAdmin");
    assert_eq!(system.id_sistema, "NA");
    assert!(system.solo_verifactu && !system.multi_ot && !system.indicador_multiples_ot);
    assert_eq!(system.productor.nombre_razon, "Productor Demo SL");
    assert_eq!(
        alta.fecha_hora_huso_gen_registro.to_string(),
        "2026-03-01T10:00:00+01:00"
    );
}

#[test]
fn an_invoice_without_producer_fails_and_leaves_everything_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _clock) = app(&dir, false);
    let (customer, product) = setup(&mut app);
    let invoice = draft(&mut app, DocumentKind::Invoice, customer, product);

    let err = app.issue_sales_document(invoice, None).unwrap_err();
    assert_eq!(err.code(), CODE_NOT_CONFIGURED);
    let doc = app.sales_document(invoice).unwrap();
    assert_eq!(doc.status, Status::Draft);
    assert_eq!(doc.number, None);
    assert_eq!(
        last_invoice_number(&dir),
        None,
        "la serie no consumió número"
    );
    assert!(app.verifactu_records().unwrap().is_empty());
    assert_eq!(installation_in_db(&dir), None);

    // Anular tampoco funciona sin productor.
    let err = app.void_sales_invoice(invoice).unwrap_err();
    assert_eq!(err.code(), CODE_NOT_CONFIGURED);

    // Con el productor configurado se emite y toma el primer número.
    app.set_verifactu_producer("Productor Demo SL", "B00000034")
        .unwrap();
    let issued = app.issue_sales_document(invoice, None).unwrap();
    assert_eq!(issued.number.as_deref(), Some("F-2026-0001"));
}

#[test]
fn an_invalid_producer_is_rejected_and_not_kept() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _clock) = app(&dir, false);
    let (customer, product) = setup(&mut app);
    for (name, nif) in [("", "B00000034"), ("Productor Demo SL", "123"), ("  ", " ")] {
        let err = app.set_verifactu_producer(name, nif).unwrap_err();
        assert_eq!(err.code(), "invalid_input", "{name:?} {nif:?}");
    }
    let invoice = draft(&mut app, DocumentKind::Invoice, customer, product);
    assert_eq!(
        app.issue_sales_document(invoice, None).unwrap_err().code(),
        CODE_NOT_CONFIGURED
    );
}

#[test]
fn the_default_issue_date_is_today_and_a_future_date_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, clock) = app(&dir, true);
    let (customer, product) = setup(&mut app);
    assert_eq!(app.sales_today(), date("2026-03-01"));

    let future = draft(&mut app, DocumentKind::Invoice, customer, product);
    let err = app
        .issue_sales_document(future, Some(date("2026-03-02")))
        .unwrap_err();
    assert_eq!(err.code(), "invalid_input");
    assert_eq!(app.sales_document(future).unwrap().status, Status::Draft);
    assert_eq!(last_invoice_number(&dir), None);

    let issued = app.issue_sales_document(future, None).unwrap();
    assert_eq!(issued.issue_date, Some(date("2026-03-01")));

    // Al día siguiente, «hoy» avanza con el reloj.
    clock.advance(86_400);
    assert_eq!(app.sales_today(), date("2026-03-02"));
    let next = draft(&mut app, DocumentKind::Invoice, customer, product);
    assert_eq!(
        app.issue_sales_document(next, None).unwrap().issue_date,
        Some(date("2026-03-02"))
    );
    // Una fecha anterior a la última de la serie la rechaza la serie (conflict), no se ajusta.
    let older = draft(&mut app, DocumentKind::Invoice, customer, product);
    let err = app
        .issue_sales_document(older, Some(date("2026-03-01")))
        .unwrap_err();
    assert_eq!(err.code(), "conflict");
    assert_eq!(app.sales_document(older).unwrap().status, Status::Draft);
}

#[test]
fn the_installation_number_is_stable_across_issues_and_reopening_and_immutable() {
    let dir = tempfile::tempdir().unwrap();
    let first_number = {
        let (mut app, clock) = app(&dir, true);
        let (customer, product) = setup(&mut app);
        let first = draft(&mut app, DocumentKind::Invoice, customer, product);
        app.issue_sales_document(first, None).unwrap();
        clock.advance(60);
        let second = draft(&mut app, DocumentKind::Invoice, customer, product);
        app.issue_sales_document(second, None).unwrap();
        let records = app.verifactu_records().unwrap();
        let numbers: Vec<String> = records
            .iter()
            .map(|r| match &r.record {
                RegistroFacturacion::Alta(a) => a.sistema_informatico.numero_instalacion.clone(),
                RegistroFacturacion::Anulacion(a) => {
                    a.sistema_informatico.numero_instalacion.clone()
                }
            })
            .collect();
        assert_eq!(numbers.len(), 2);
        assert_eq!(numbers[0], numbers[1]);
        assert_eq!(numbers[0].len(), 16);
        assert_eq!(Some(numbers[0].clone()), installation_in_db(&dir));
        numbers[0].clone()
    };

    // Tras reabrir la App (otro proceso lógico) el número es el mismo.
    let (mut app, clock) = app(&dir, true);
    clock.advance(3_600);
    let customer = app.customers(false).unwrap()[0].id;
    let product = app.products(false).unwrap()[0].id as i64;
    let third = draft(&mut app, DocumentKind::Invoice, customer, product);
    app.issue_sales_document(third, None).unwrap();
    let records = app.verifactu_records().unwrap();
    assert_eq!(records.len(), 3);
    let RegistroFacturacion::Alta(last) = &records[2].record else {
        panic!("se esperaba un alta");
    };
    assert_eq!(last.sistema_informatico.numero_instalacion, first_number);
    assert_eq!(installation_in_db(&dir), Some(first_number.clone()));

    // La tabla no admite modificar ni borrar.
    let conn = raw(&dir);
    assert!(conn
        .execute("UPDATE installation SET number = 'AAAAAAAAAAAAAAAA'", [])
        .is_err());
    assert!(conn.execute("DELETE FROM installation", []).is_err());
    assert_eq!(installation_in_db(&dir), Some(first_number));
}

#[test]
fn a_clock_behind_the_last_record_fails_without_consuming_a_number() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, clock) = app(&dir, true);
    let (customer, product) = setup(&mut app);
    clock.advance(1_000);
    let first = draft(&mut app, DocumentKind::Invoice, customer, product);
    app.issue_sales_document(first, None).unwrap();
    let second = draft(&mut app, DocumentKind::Invoice, customer, product);

    // El reloj retrocede 61 segundos respecto al último registro.
    clock.advance(-61);
    let err = app.issue_sales_document(second, None).unwrap_err();
    assert_eq!(err.code(), CODE_CLOCK_BEHIND);
    assert_eq!(app.sales_document(second).unwrap().status, Status::Draft);
    assert_eq!(app.sales_document(second).unwrap().number, None);
    assert_eq!(last_invoice_number(&dir), Some(1));
    assert_eq!(app.verifactu_records().unwrap().len(), 1);

    // Recuperado el reloj, toma el número siguiente.
    clock.advance(61);
    assert_eq!(
        app.issue_sales_document(second, None)
            .unwrap()
            .number
            .as_deref(),
        Some("F-2026-0002")
    );
}

#[test]
fn voiding_chains_a_cancellation_record() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, clock) = app(&dir, true);
    let (customer, product) = setup(&mut app);
    let invoice = draft(&mut app, DocumentKind::Invoice, customer, product);
    app.issue_sales_document(invoice, None).unwrap();
    clock.advance(120);
    let voided = app.void_sales_invoice(invoice).unwrap();
    assert_eq!(voided.status, Status::Voided);

    let records = app.verifactu_records().unwrap();
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].kind, VerifactuKind::Alta);
    assert_eq!(records[1].kind, VerifactuKind::Anulacion);
    assert_eq!(records[1].previous_id, Some(records[0].id));
    assert_eq!(
        records[1].previous_hash.as_deref(),
        Some(records[0].hash.as_str())
    );
    assert_eq!(
        app.verifactu_chain_status().unwrap(),
        ChainStatus::Ok { records: 2 }
    );
}

#[test]
fn partial_and_full_payments_derive_the_collection_state() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _clock) = app(&dir, true);
    let (customer, product) = setup(&mut app);
    let invoice = draft(&mut app, DocumentKind::Invoice, customer, product);
    app.issue_sales_document(invoice, None).unwrap();
    // 80,00 + 21 % = 96,80.
    let summary = app.sales_collection(invoice).unwrap();
    assert_eq!(summary.state, CollectionState::Pending);
    assert_eq!(summary.payable, d("96.80"));

    let (first, summary) = app
        .add_sales_payment(invoice, date("2026-03-01"), d("30.00"), PaymentMethod::Cash)
        .unwrap();
    assert_eq!(summary.state, CollectionState::Partial);
    assert_eq!(summary.outstanding, d("66.80"));

    // Más de lo pendiente se rechaza.
    let err = app
        .add_sales_payment(invoice, date("2026-03-01"), d("70.00"), PaymentMethod::Cash)
        .unwrap_err();
    assert_ne!(err.code(), "storage");
    assert_eq!(app.sales_payments(invoice).unwrap().len(), 1);

    let (_, summary) = app
        .add_sales_payment(
            invoice,
            date("2026-03-01"),
            d("66.80"),
            PaymentMethod::Transfer,
        )
        .unwrap();
    assert_eq!(summary.state, CollectionState::Paid);
    assert_eq!(
        app.sales_collection(invoice).unwrap().state,
        CollectionState::Paid
    );

    let summary = app.remove_sales_payment(first).unwrap();
    assert_eq!(summary.state, CollectionState::Partial);
    // Cobrar no genera registros Verifactu.
    assert_eq!(app.verifactu_records().unwrap().len(), 1);
}

#[test]
fn app_error_code_for_sales_is_the_core_code() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _clock) = app(&dir, true);
    let missing = app.sales_document(9_999).unwrap_err();
    assert_eq!(missing.code(), "not_found");
    // Mismo texto que antes: la serialización de AppError sigue siendo el texto.
    assert_eq!(
        serde_json::to_string(&missing).unwrap(),
        serde_json::to_string(&missing.to_string()).unwrap()
    );
    let (customer, product) = setup(&mut app);
    // Una factura sin líneas no se emite: error de datos, no de almacenamiento.
    let mut input = DraftInput::new(DocumentKind::Invoice);
    input.customer_id = Some(customer);
    let empty = app.create_sales_draft(&input).unwrap();
    let err = app.issue_sales_document(empty, None).unwrap_err();
    assert_eq!(err.code(), "invalid_input");
    let _ = product;
}
