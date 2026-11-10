//! Correcciones de la capa de aplicación de venta tras la revisión (D213-D219): reloj sin
//! reserva a 1970 y productor normalizado y validado. Prueba independiente (D197). Solo datos
//! sintéticos.

use std::path::Path;
use std::sync::Arc;

use nestadmin_ai::{AiError, GenerateRequest, GenerateResult, Provider, ProviderKind};
use nestadmin_app::sales::{LocalClock, SalesClock, CODE_CLOCK_UNAVAILABLE};
use nestadmin_app::{App, AppPaths};
use nestadmin_core::sales::{Company, CustomerData, DraftInput};
use nestadmin_extract::{ExtractError, Extractor, Ocr};
use nestadmin_sales::{DocumentKind, Product, TaxRate};
use nestadmin_verifactu::{FechaHoraHuso, Identificacion};
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

/// Reloj que no puede dar una hora representable.
struct BrokenClock;

impl SalesClock for BrokenClock {
    fn now(&self) -> FechaHoraHuso {
        unreachable!("la aplicación usa try_now")
    }
    fn try_now(&self) -> Result<FechaHoraHuso, String> {
        Err("hora no representable".into())
    }
}

fn open_at(dir: &Path) -> App {
    let extractor = Extractor::new(dir.join("sin-pdfium"), Box::new(NoOcr));
    App::open(&AppPaths::new(dir), Arc::new(NoAi), Arc::new(extractor)).unwrap()
}

fn d(text: &str) -> Decimal {
    text.parse().unwrap()
}

fn invoice_draft(app: &mut App) -> i64 {
    app.set_sales_company(&Company {
        name: Some("Taller Demo SL".into()),
        tax_id: Some("B12345674".into()),
        address: Some("Calle Ficticia 1, 28000 Madrid".into()),
        email: None,
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
    let mut input = DraftInput::new(DocumentKind::Invoice);
    input.customer_id = Some(customer);
    let id = app.create_sales_draft(&input).unwrap();
    app.add_sales_line_from_product(id, product, d("1"), Decimal::ZERO)
        .unwrap();
    id
}

#[test]
fn the_local_clock_gives_a_real_time_never_the_epoch() {
    let now = LocalClock.try_now().unwrap();
    assert!(now.fecha().anio() >= 2026, "{}", now.fecha().anio());
    assert!(LocalClock.try_today().unwrap().year >= 2026);
}

#[test]
fn an_unrepresentable_clock_fails_explicitly_and_consumes_no_number() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = open_at(dir.path());
    app.set_verifactu_producer("Productor Demo SL", "B00000034")
        .unwrap();
    let id = invoice_draft(&mut app);
    app.set_sales_clock(Arc::new(BrokenClock));
    assert_eq!(
        app.sales_today().unwrap_err().code(),
        CODE_CLOCK_UNAVAILABLE
    );
    let err = app.issue_sales_document(id, None).unwrap_err();
    assert_eq!(err.code(), CODE_CLOCK_UNAVAILABLE);
    let doc = app.sales_document(id).unwrap();
    assert_eq!(doc.number, None);
    assert!(app.verifactu_records().unwrap().is_empty());
}

#[test]
fn the_producer_nif_is_normalized_to_uppercase_before_it_is_recorded() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = open_at(dir.path());
    app.set_verifactu_producer("Productor Demo SL", " b-0000003-4 ")
        .unwrap();
    let id = invoice_draft(&mut app);
    app.issue_sales_document(id, None).unwrap();
    let rows = app.verifactu_records().unwrap();
    let producer = match &rows[0].record {
        nestadmin_verifactu::RegistroFacturacion::Alta(a) => {
            a.sistema_informatico.productor.clone()
        }
        other => panic!("se esperaba un alta, hay {other:?}"),
    };
    match producer.identificacion {
        Identificacion::Nif(nif) => assert_eq!(nif, "B00000034"),
        other => panic!("se esperaba un NIF, hay {other:?}"),
    }
}

#[test]
fn a_producer_nif_with_a_wrong_control_digit_or_format_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = open_at(dir.path());
    for bad in ["B00000035", "12345", "PONER-AQUI", ""] {
        let err = app
            .set_verifactu_producer("Productor Demo SL", bad)
            .unwrap_err();
        assert_eq!(err.code(), "invalid_input", "{bad}");
    }
    assert!(!app.has_verifactu_producer());
}
