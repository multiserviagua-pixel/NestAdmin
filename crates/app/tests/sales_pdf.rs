//! PDF de los documentos de venta emitidos y membrete de fondo (E4a, D76, D222, D223; D197):
//! contenido, QR de Verifactu, paginación, determinismo, membrete y su validación. Usa PDFium
//! real (busca `vendor/pdfium/bin` hacia arriba desde el crate o `NESTADMIN_PDFIUM_DIR`; se
//! instala con `scripts/setup-ocr.ps1`) y solo datos sintéticos. Sin PDFium falla con el motivo,
//! no se salta en silencio.

use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

use nestadmin_ai::{AiError, GenerateRequest, GenerateResult, Provider, ProviderKind};
use nestadmin_app::sales::{SalesClock, CODE_PDF_FAILED};
use nestadmin_app::sales_pdf::{render, PdfSource};
use nestadmin_app::{App, AppPaths};
use nestadmin_core::sales::{
    Company, CustomerData, DraftInput, DraftLine, LetterheadMargins, SalesDocument,
};
use nestadmin_extract::image::{DynamicImage, ImageFormat};
use nestadmin_extract::pdfium_render::prelude::*;
use nestadmin_extract::{ExtractError, Extractor, Ocr};
use nestadmin_sales::{
    DocumentKind, LineInput, RectificationKind, RectificationMethod, Status, TaxRate,
};
use nestadmin_verifactu::{CodigoQr, Entorno, Fecha, FechaHoraHuso, RegistroFacturacion};
use rust_decimal::Decimal;

const MM: f32 = 72.0 / 25.4;

struct NoAi;

impl Provider for NoAi {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Ollama
    }
    fn model(&self) -> &str {
        "simulado"
    }
    fn generate(&self, _request: &GenerateRequest) -> Result<GenerateResult, AiError> {
        Err(AiError::Unavailable("el PDF no usa IA".into()))
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

/// Carpeta de PDFium: `NESTADMIN_PDFIUM_DIR` o la primera `vendor/pdfium/bin` hacia arriba.
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

fn app(dir: &tempfile::TempDir) -> App {
    let mut app = App::open(
        &AppPaths::new(dir.path()),
        Arc::new(NoAi),
        Arc::new(extractor()),
    )
    .unwrap();
    // 2026-03-01 10:00:00 +01:00.
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

fn png() -> Vec<u8> {
    let mut out = Cursor::new(Vec::new());
    DynamicImage::new_rgb8(60, 24)
        .write_to(&mut out, ImageFormat::Png)
        .unwrap();
    out.into_inner()
}

fn line(description: &str, quantity: &str, price: &str) -> DraftLine {
    LineInput {
        description: description.into(),
        quantity: d(quantity),
        unit_price: d(price),
        discount: Decimal::ZERO,
        tax_rate: TaxRate::TwentyOne,
        exemption: None,
    }
    .into()
}

/// Empresa (con logotipo e IBAN sintéticos) y un cliente; devuelve el cliente.
fn setup(app: &mut App) -> i64 {
    app.set_sales_company(&Company {
        name: Some("Taller Demo SL".into()),
        tax_id: Some("B12345674".into()),
        address: Some("Calle Ficticia 1, 28000 Madrid".into()),
        email: Some("demo@example.test".into()),
        iban: Some("ES9121000418450200051332".into()),
        logo: Some(png()),
        logo_mime: Some("image/png".into()),
    })
    .unwrap();
    app.create_customer(&CustomerData {
        name: "Cliente Uno SA".into(),
        tax_id: Some("A58818501".into()),
        address: Some("Avenida Inventada 2, 08000 Barcelona".into()),
        email: None,
    })
    .unwrap()
}

fn issue(app: &mut App, kind: DocumentKind, customer: Option<i64>, lines: Vec<DraftLine>) -> i64 {
    let mut input = DraftInput::new(kind);
    input.customer_id = customer;
    input.lines = lines;
    let id = app.create_sales_draft(&input).unwrap();
    app.issue_sales_document(id, None).unwrap();
    id
}

fn doc(app: &App, id: i64) -> SalesDocument {
    app.sales_document(id).unwrap()
}

/// Texto de cada página del PDF.
fn texts(bytes: &[u8]) -> Vec<String> {
    let pdfium = extractor().pdfium().unwrap();
    let document = pdfium.load_pdf_from_byte_slice(bytes, None).unwrap();
    document
        .pages()
        .iter()
        .map(|p| p.text().unwrap().all())
        .collect()
}

fn file_texts(path: &Path) -> Vec<String> {
    texts(&std::fs::read(path).unwrap())
}

/// Número de imágenes de la página `index`.
fn image_count(bytes: &[u8], index: i32) -> usize {
    let pdfium = extractor().pdfium().unwrap();
    let document = pdfium.load_pdf_from_byte_slice(bytes, None).unwrap();
    let page = document.pages().get(index).unwrap();
    let count = page
        .objects()
        .iter()
        .filter(|o| o.as_image_object().is_some())
        .count();
    count
}

/// PDF sintético de membrete: `pages` páginas del tamaño dado, cada una con su texto.
fn make_letterhead(pages: usize, landscape: bool) -> Vec<u8> {
    let pdfium = extractor().pdfium().unwrap();
    let mut document = pdfium.create_new_pdf().unwrap();
    let font = document.fonts_mut().helvetica();
    for i in 0..pages {
        let size = if landscape {
            PdfPagePaperSize::a4().landscape()
        } else {
            PdfPagePaperSize::a4()
        };
        let mut page = document.pages_mut().create_page_at_end(size).unwrap();
        let (pw, ph) = (page.width().value, page.height().value);
        // Banda superior y texto reconocible del membrete, en el margen superior.
        page.objects_mut()
            .create_path_object_rect(
                PdfRect::new(
                    PdfPoints::new(ph - 20.0),
                    PdfPoints::new(0.0),
                    PdfPoints::new(ph),
                    PdfPoints::new(pw),
                ),
                None,
                None,
                Some(PdfColor::new(200, 220, 255, 255)),
            )
            .unwrap();
        page.objects_mut()
            .create_text_object(
                PdfPoints::new(30.0),
                PdfPoints::new(ph - 15.0),
                format!("MEMBRETE SINTETICO PAGINA {}", i + 1),
                font,
                PdfPoints::new(10.0),
            )
            .unwrap();
    }
    document.save_to_bytes().unwrap()
}

fn many_lines(n: usize) -> Vec<DraftLine> {
    (1..=n)
        .map(|i| line(&format!("Concepto sintético número {i}"), "1", "10"))
        .collect()
}

// ---------------------------------------------------------------------------------------------

#[test]
fn invoice_pdf_has_the_issued_data_qr_logo_and_legend() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app(&dir);
    let customer = setup(&mut app);
    let id = issue(
        &mut app,
        DocumentKind::Invoice,
        Some(customer),
        vec![line(
            "Revisión anual con ñ, áéíóú y 10 € – “comillas”",
            "2",
            "40",
        )],
    );

    let path = app.sales_document_pdf(id).unwrap();
    assert!(
        path.ends_with("documentos/ventas/F-2026-0001.pdf")
            || path.ends_with("documentos\\ventas\\F-2026-0001.pdf")
    );
    assert!(path.starts_with(dir.path()));
    let pages = file_texts(&path);
    assert_eq!(pages.len(), 1);
    let text = &pages[0];
    for expected in [
        "FACTURA Nº F-2026-0001",
        "Taller Demo SL",
        "NIF: B12345674",
        "demo@example.test",
        "CLIENTE",
        "Cliente Uno SA",
        "NIF: A58818501",
        "Fecha de expedición: 01/03/2026",
        "Revisión anual con ñ, áéíóú y 10 € – “comillas”",
        "80,00",
        "Base imponible",
        "16,80 €",
        "TOTAL",
        "96,80 €",
        "IBAN: ES91 2100 0418 4502 0005 1332",
        "QR tributario:",
        "VERI*FACTU",
        "Página 1 de 1",
    ] {
        assert!(text.contains(expected), "falta «{expected}» en:\n{text}");
    }

    // Logotipo (sin membrete) y QR: dos imágenes en la primera página.
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(image_count(&bytes, 0), 2);

    // El QR de la imagen reproduce los módulos de la URL de cotejo del registro de alta.
    let alta = app
        .verifactu_records()
        .unwrap()
        .into_iter()
        .find_map(|r| match r.record {
            RegistroFacturacion::Alta(a) if r.document_id == id => Some(a),
            _ => None,
        })
        .unwrap();
    let qr = CodigoQr::de_registro(Entorno::Produccion, &alta).unwrap();
    assert!(qr.url().contains("nif=B12345674"));
    assert!(qr.url().contains("numserie=F-2026-0001"));
    assert!(qr.url().contains("fecha=01-03-2026"));
    assert!(qr.url().contains("importe=96.80"));
    let pdfium = extractor().pdfium().unwrap();
    let document = pdfium.load_pdf_from_byte_slice(&bytes, None).unwrap();
    let page = document.pages().get(0).unwrap();
    let side = qr.ancho_modulos() as u32 * 10;
    let image = page
        .objects()
        .iter()
        .filter_map(|o| o.as_image_object().and_then(|i| i.get_raw_image().ok()))
        .find(|i| i.width() == side)
        .expect("la imagen del QR");
    let luma = image.to_luma8();
    for y in 0..qr.ancho_modulos() {
        for x in 0..qr.ancho_modulos() {
            let pixel = luma.get_pixel(x as u32 * 10 + 5, y as u32 * 10 + 5).0[0];
            assert_eq!(pixel < 128, qr.modulo(x, y), "módulo ({x}, {y})");
        }
    }
}

#[test]
fn every_document_kind_has_its_pdf() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app(&dir);
    let customer = setup(&mut app);

    let quote = issue(
        &mut app,
        DocumentKind::Quote,
        Some(customer),
        vec![line("Trabajo", "1", "100")],
    );
    let note = issue(
        &mut app,
        DocumentKind::DeliveryNote,
        Some(customer),
        vec![line("Material", "3", "5")],
    );
    let simplified = issue(
        &mut app,
        DocumentKind::SimplifiedInvoice,
        None,
        vec![line("Venta mostrador", "1", "10")],
    );
    let invoice = issue(
        &mut app,
        DocumentKind::Invoice,
        Some(customer),
        vec![line("Servicio", "1", "100")],
    );

    // Rectificativa por diferencias: importe negativo sobre la factura.
    let corrective = app
        .create_corrective_draft(
            invoice,
            RectificationKind::R4,
            RectificationMethod::Differences,
        )
        .unwrap();
    let mut input = DraftInput::new(DocumentKind::CorrectiveInvoice);
    input.customer_id = Some(customer);
    input.lines = vec![line("Descuento posterior", "-1", "20")];
    app.update_sales_draft(corrective, &input).unwrap();
    app.issue_sales_document(corrective, None).unwrap();

    let cases = [
        (quote, "PRESUPUESTO Nº P-2026-0001", false),
        (note, "ALBARÁN Nº A-2026-0001", false),
        (simplified, "FACTURA SIMPLIFICADA Nº FS-2026-0001", true),
        (invoice, "FACTURA Nº F-2026-0001", true),
        (corrective, "FACTURA RECTIFICATIVA Nº FR-2026-0001", true),
    ];
    for (id, head, is_invoice) in cases {
        let path = app.sales_document_pdf(id).unwrap();
        let text = file_texts(&path).join("\n");
        assert!(text.contains(head), "falta «{head}»:\n{text}");
        assert_eq!(text.contains("VERI*FACTU"), is_invoice, "{head}");
        assert_eq!(text.contains("QR tributario:"), is_invoice, "{head}");
        let bytes = std::fs::read(&path).unwrap();
        // Logotipo siempre; QR solo en facturas.
        assert_eq!(
            image_count(&bytes, 0),
            if is_invoice { 2 } else { 1 },
            "{head}"
        );
    }
    let simplified_text = file_texts(&app.sales_document_pdf(simplified).unwrap()).join("\n");
    assert!(simplified_text.contains("Sin destinatario identificado"));
    let corrective_text = file_texts(&app.sales_document_pdf(corrective).unwrap()).join("\n");
    for expected in [
        "Rectifica a la factura: F-2026-0001",
        "Tipo de rectificación: R4",
        "Método: por diferencias",
        "-24,20 €",
    ] {
        assert!(
            corrective_text.contains(expected),
            "falta «{expected}»:\n{corrective_text}"
        );
    }
}

#[test]
fn many_lines_paginate_and_keep_totals_together() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app(&dir);
    let customer = setup(&mut app);
    let id = issue(
        &mut app,
        DocumentKind::Invoice,
        Some(customer),
        many_lines(70),
    );

    let pages = file_texts(&app.sales_document_pdf(id).unwrap());
    assert!(
        pages.len() >= 2,
        "70 líneas caben en {} página(s)",
        pages.len()
    );
    let all = pages.join("\n");
    for i in 1..=70 {
        assert!(
            all.contains(&format!("Concepto sintético número {i}")),
            "línea {i}"
        );
    }
    for (i, page) in pages.iter().enumerate() {
        assert!(
            page.contains(&format!("Página {} de {}", i + 1, pages.len())),
            "pie de la página {}",
            i + 1
        );
    }
    // QR y leyenda solo en la primera; encabezado de continuación en las demás; los totales
    // (70 x 10 + 21 % = 847,00) en la última.
    assert!(pages[0].contains("VERI*FACTU"));
    assert!(!pages[1].contains("VERI*FACTU"));
    assert!(pages[1].contains("FACTURA F-2026-0001 (continuación)"));
    assert!(pages.last().unwrap().contains("847,00 €"));
    assert!(pages.last().unwrap().contains("DESGLOSE DE IVA"));
}

#[test]
fn regenerating_gives_the_same_content() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app(&dir);
    let customer = setup(&mut app);
    let id = issue(
        &mut app,
        DocumentKind::Invoice,
        Some(customer),
        many_lines(30),
    );
    let document = doc(&app, id);

    let first = app.render_sales_pdf(&document).unwrap();
    let second = app.render_sales_pdf(&document).unwrap();
    assert_eq!(texts(&first), texts(&second), "mismo texto por página");
    assert_eq!(image_count(&first, 0), image_count(&second, 0));
    // PDFium estampa la hora de creación y un identificador aleatorio; `stabilize` los fija
    // (D226) y el archivo entero sale igual byte a byte, con la fecha de expedición de fecha de
    // creación.
    assert_eq!(first, second, "mismos bytes");
    let raw = String::from_utf8_lossy(&first);
    assert!(
        raw.contains("/CreationDate(D:20260301000000"),
        "fecha de creación fija"
    );
    // Otro documento con el mismo contenido tiene otro identificador (depende del contenido).
    let other = issue(
        &mut app,
        DocumentKind::Invoice,
        Some(customer),
        many_lines(30),
    );
    assert_ne!(first, app.render_sales_pdf(&doc(&app, other)).unwrap());

    // Borrar el archivo y pedirlo de nuevo regenera el mismo contenido en la misma ruta.
    let path = app.sales_document_pdf(id).unwrap();
    let before = file_texts(&path);
    std::fs::remove_file(&path).unwrap();
    assert_eq!(app.sales_document_pdf(id).unwrap(), path);
    assert_eq!(file_texts(&path), before);
    // Sin borrarlo, no se reescribe.
    let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
    app.sales_document_pdf(id).unwrap();
    assert_eq!(
        std::fs::metadata(&path).unwrap().modified().unwrap(),
        modified
    );
    assert!(!path.with_extension("pdf.part").exists());
}

#[test]
fn a_draft_has_no_pdf() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app(&dir);
    let customer = setup(&mut app);
    let mut input = DraftInput::new(DocumentKind::Invoice);
    input.customer_id = Some(customer);
    input.lines = vec![line("Servicio", "1", "10")];
    let id = app.create_sales_draft(&input).unwrap();
    let err = app.sales_document_pdf(id).unwrap_err();
    assert_eq!(err.code(), "conflict");
    assert!(err.to_string().contains("emitido"));
    assert!(!dir.path().join("documentos/ventas").exists());
}

#[test]
fn letterhead_is_the_background_and_content_stays_inside_the_margins() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app(&dir);
    let customer = setup(&mut app);
    app.set_sales_letterhead(&make_letterhead(2, false))
        .unwrap();
    let margins = LetterheadMargins {
        top_mm: 30,
        right_mm: 25,
        bottom_mm: 20,
        left_mm: 22,
    };
    app.set_sales_letterhead_margins(margins).unwrap();
    let id = issue(
        &mut app,
        DocumentKind::Invoice,
        Some(customer),
        many_lines(60),
    );

    let path = app.sales_document_pdf(id).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    let pages = texts(&bytes);
    assert!(pages.len() >= 2);
    // Fondo: primera página del membrete en la primera hoja y la segunda en las siguientes.
    assert!(pages[0].contains("MEMBRETE SINTETICO PAGINA 1"));
    assert!(pages[1].contains("MEMBRETE SINTETICO PAGINA 2"));
    for page in &pages[1..] {
        assert!(!page.contains("MEMBRETE SINTETICO PAGINA 1"));
    }
    // El contenido y el QR están encima.
    assert!(pages[0].contains("FACTURA Nº F-2026-0001"));
    assert!(pages[0].contains("VERI*FACTU"));
    assert!(pages[0].contains("Cliente Uno SA"));
    assert!(pages[0].contains("Concepto sintético número 1"));
    assert!(pages[1].contains("(continuación)"));
    // Con membrete no se dibuja el logotipo: solo el QR.
    assert_eq!(image_count(&bytes, 0), 1);
    assert_eq!(image_count(&bytes, 1), 0);

    // Todo lo que escribe NestAdmin cae dentro del área útil (el texto del membrete no cuenta).
    let pdfium = extractor().pdfium().unwrap();
    let document = pdfium.load_pdf_from_byte_slice(&bytes, None).unwrap();
    for (index, page) in document.pages().iter().enumerate() {
        let (w, h) = (page.width().value, page.height().value);
        for object in page.objects().iter() {
            let Some(text) = object.as_text_object() else {
                continue;
            };
            if text.text().starts_with("MEMBRETE") {
                continue;
            }
            let b = object.bounds().unwrap();
            let (left, right) = (b.left().value, b.right().value);
            let (top, bottom) = (h - b.top().value, h - b.bottom().value);
            assert!(
                left >= f32::from(margins.left_mm) * MM - 1.0
                    && right <= w - f32::from(margins.right_mm) * MM + 1.0
                    && top >= f32::from(margins.top_mm) * MM - 4.0
                    && bottom <= h - f32::from(margins.bottom_mm) * MM + 3.0,
                "página {}: «{}» en ({left:.0}, {top:.0})-({right:.0}, {bottom:.0})",
                index + 1,
                text.text()
            );
        }
    }
}

#[test]
fn a_single_page_letterhead_repeats_on_every_sheet() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app(&dir);
    let customer = setup(&mut app);
    app.set_sales_letterhead(&make_letterhead(1, false))
        .unwrap();
    let id = issue(
        &mut app,
        DocumentKind::Quote,
        Some(customer),
        many_lines(60),
    );
    let pages = file_texts(&app.sales_document_pdf(id).unwrap());
    assert!(pages.len() >= 2);
    for (i, page) in pages.iter().enumerate() {
        assert!(
            page.contains("MEMBRETE SINTETICO PAGINA 1"),
            "hoja {}",
            i + 1
        );
    }
}

#[test]
fn changing_the_letterhead_does_not_alter_issued_documents() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app(&dir);
    let customer = setup(&mut app);

    // Anterior al membrete: diseño normal con logotipo.
    let before = issue(
        &mut app,
        DocumentKind::Invoice,
        Some(customer),
        vec![line("Antes", "1", "10")],
    );
    let before_bytes = app.render_sales_pdf(&doc(&app, before)).unwrap();
    assert_eq!(image_count(&before_bytes, 0), 2);
    assert!(!texts(&before_bytes)[0].contains("MEMBRETE"));
    assert!(doc(&app, before)
        .issuer
        .unwrap()
        .letterhead_sha256
        .is_none());

    // Con el membrete A y sus márgenes.
    app.set_sales_letterhead(&make_letterhead(1, false))
        .unwrap();
    let with_a = issue(
        &mut app,
        DocumentKind::Invoice,
        Some(customer),
        vec![line("Con A", "1", "10")],
    );
    let a_hash = doc(&app, with_a).issuer.unwrap().letterhead_sha256.unwrap();
    let a_text = texts(&app.render_sales_pdf(&doc(&app, with_a)).unwrap());
    assert!(a_text[0].contains("MEMBRETE SINTETICO PAGINA 1"));
    assert_eq!(
        doc(&app, with_a).issuer.unwrap().letterhead_margins,
        Some(LetterheadMargins::default())
    );

    // Otro membrete (dos páginas, otros márgenes) y después ninguno.
    app.set_sales_letterhead(&make_letterhead(2, false))
        .unwrap();
    app.set_sales_letterhead_margins(LetterheadMargins {
        top_mm: 50,
        right_mm: 10,
        bottom_mm: 10,
        left_mm: 10,
    })
    .unwrap();
    let with_b = issue(
        &mut app,
        DocumentKind::Invoice,
        Some(customer),
        vec![line("Con B", "1", "10")],
    );
    assert_ne!(
        doc(&app, with_b).issuer.unwrap().letterhead_sha256.unwrap(),
        a_hash
    );
    app.remove_sales_letterhead().unwrap();
    assert!(app.sales_letterhead().unwrap().is_none());
    let without = issue(
        &mut app,
        DocumentKind::Invoice,
        Some(customer),
        vec![line("Sin", "1", "10")],
    );
    let without_bytes = app.render_sales_pdf(&doc(&app, without)).unwrap();
    assert!(!texts(&without_bytes)[0].contains("MEMBRETE"));
    assert_eq!(image_count(&without_bytes, 0), 2, "con logotipo");

    // Los ya emitidos siguen igual que antes de los cambios.
    assert_eq!(
        texts(&app.render_sales_pdf(&doc(&app, with_a)).unwrap()),
        a_text
    );
    assert_eq!(
        texts(&app.render_sales_pdf(&doc(&app, before)).unwrap()),
        texts(&before_bytes)
    );
    // Y los contenidos antiguos siguen guardados aunque ya no sean los vigentes.
    assert!(app.sales_letterhead_by_hash(&a_hash).unwrap().is_some());
}

#[test]
fn invalid_letterheads_are_rejected_with_the_reason() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app(&dir);
    setup(&mut app);

    let rejected = |app: &mut App, bytes: &[u8], reason: &str| {
        let err = app.set_sales_letterhead(bytes).unwrap_err();
        assert_eq!(err.code(), "invalid_input", "{err}");
        assert!(
            err.to_string().contains(reason),
            "«{err}» no dice «{reason}»"
        );
        assert!(
            app.sales_letterhead().unwrap().is_none(),
            "no se guardó nada"
        );
    };
    rejected(&mut app, b"", "vacío");
    rejected(&mut app, b"esto no es un PDF", "no es un PDF válido");
    rejected(&mut app, b"%PDF-1.7\nroto", "no es un PDF válido");
    rejected(
        &mut app,
        &make_letterhead(3, false),
        "una o dos páginas y este tiene 3",
    );
    rejected(&mut app, &make_letterhead(1, true), "A4 vertical");
    let mut big = b"%PDF-1.4\n".to_vec();
    big.resize(5 * 1024 * 1024 + 1, b' ');
    rejected(&mut app, &big, "5 MB");

    // Uno válido se acepta y lo guardado coincide.
    let good = make_letterhead(2, false);
    app.set_sales_letterhead(&good).unwrap();
    assert_eq!(app.sales_letterhead().unwrap().unwrap().data, good);
}

#[test]
fn margins_are_validated_and_survive_removing_the_letterhead() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app(&dir);
    setup(&mut app);
    assert_eq!(
        app.sales_letterhead_margins().unwrap(),
        LetterheadMargins::default()
    );

    for bad in [
        LetterheadMargins {
            top_mm: 101,
            ..Default::default()
        },
        LetterheadMargins {
            left_mm: 100,
            right_mm: 100,
            ..Default::default()
        },
        LetterheadMargins {
            top_mm: 100,
            bottom_mm: 100,
            ..Default::default()
        },
    ] {
        let err = app.set_sales_letterhead_margins(bad).unwrap_err();
        assert_eq!(err.code(), "invalid_input", "{bad:?}");
    }
    let custom = LetterheadMargins {
        top_mm: 40,
        right_mm: 15,
        bottom_mm: 30,
        left_mm: 18,
    };
    app.set_sales_letterhead_margins(custom).unwrap();
    app.set_sales_letterhead(&make_letterhead(1, false))
        .unwrap();
    assert_eq!(app.sales_letterhead().unwrap().unwrap().margins, custom);
    app.remove_sales_letterhead().unwrap();
    assert_eq!(app.sales_letterhead_margins().unwrap(), custom);
    // Los datos de la empresa no se tocan.
    assert_eq!(
        app.sales_company().unwrap().unwrap().name.as_deref(),
        Some("Taller Demo SL")
    );
}

#[test]
fn text_outside_the_font_is_replaced_not_fatal() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app(&dir);
    let customer = setup(&mut app);
    let id = issue(
        &mut app,
        DocumentKind::Quote,
        Some(customer),
        vec![line("Café 日本 y una_palabra_larguísima_sin_espacios_que_no_cabe_en_la_columna_de_descripcion_de_la_tabla", "1", "10")],
    );
    let text = file_texts(&app.sales_document_pdf(id).unwrap()).join("\n");
    assert!(text.contains("Café ?? y"));
    assert!(text.contains("TOTAL"));
}

#[test]
fn an_unreadable_logo_is_left_out_and_a_missing_pdfium_is_explicit() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app(&dir);
    let customer = setup(&mut app);
    app.set_sales_company(&Company {
        name: Some("Taller Demo SL".into()),
        tax_id: Some("B12345674".into()),
        address: None,
        email: None,
        iban: None,
        logo: Some(b"logotipo sintetico".to_vec()),
        logo_mime: Some("image/png".into()),
    })
    .unwrap();
    let id = issue(
        &mut app,
        DocumentKind::Quote,
        Some(customer),
        vec![line("Trabajo", "1", "10")],
    );
    let bytes = app.render_sales_pdf(&doc(&app, id)).unwrap();
    assert_eq!(image_count(&bytes, 0), 0, "el logotipo ilegible se omite");
    assert!(texts(&bytes)[0].contains("PRESUPUESTO"));

    // Sin PDFium: `pdf_unavailable` con el motivo, y no queda archivo a medias.
    let other = tempfile::tempdir().unwrap();
    let mut broken = App::open(
        &AppPaths::new(other.path()),
        Arc::new(NoAi),
        Arc::new(Extractor::new(
            other.path().join("sin-pdfium"),
            Box::new(NoOcr),
        )),
    )
    .unwrap();
    let customer = setup(&mut broken);
    let id = issue(
        &mut broken,
        DocumentKind::Quote,
        Some(customer),
        vec![line("Trabajo", "1", "10")],
    );
    // PDFium se enlaza una sola vez por proceso: si ya está enlazado, la carpeta no importa. Por
    // eso solo se comprueba el contrato del código de error cuando falla.
    match broken.sales_document_pdf(id) {
        Ok(path) => assert!(path.exists()),
        Err(e) => {
            assert!(
                ["pdf_unavailable", CODE_PDF_FAILED].contains(&e.code()),
                "{e}"
            );
            assert!(!other
                .path()
                .join("documentos/ventas/P-2026-0001.pdf.part")
                .exists());
        }
    }
}

#[test]
fn render_rejects_a_document_without_totals() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app(&dir);
    let customer = setup(&mut app);
    let mut input = DraftInput::new(DocumentKind::Quote);
    input.customer_id = Some(customer);
    let id = app.create_sales_draft(&input).unwrap();
    let mut document = doc(&app, id);
    document.totals = None;
    document.status = Status::Issued;
    let err = render(
        extractor().pdfium().unwrap(),
        &PdfSource {
            document: &document,
            rectified_number: None,
            logo: None,
            letterhead: None,
            qr: None,
        },
    )
    .unwrap_err();
    assert!(err.0.contains("totales"));
}

/// Factura con importes grandes, descuento, dos tipos de IVA y retención.
fn big_amounts_invoice(app: &mut App, customer: i64) -> i64 {
    let mut big = LineInput {
        description: "Suministro sintético de importe elevado".into(),
        quantity: d("1234.56"),
        unit_price: d("98765.43"),
        discount: d("0.15"),
        tax_rate: TaxRate::TwentyOne,
        exemption: None,
    };
    let first = DraftLine::from(big.clone());
    big.tax_rate = TaxRate::Ten;
    big.description = "Segunda línea al diez por ciento".into();
    let second = DraftLine::from(big);
    let mut input = DraftInput::new(DocumentKind::Invoice);
    input.customer_id = Some(customer);
    input.withholding_percent = Some(d("15"));
    input.lines = vec![first, second, line("Concepto corriente", "1", "10")];
    let id = app.create_sales_draft(&input).unwrap();
    app.issue_sales_document(id, None).unwrap();
    id
}

/// Cajas (texto, izquierda, derecha, línea base, ancho de página) de los textos, sin el membrete.
fn text_boxes(bytes: &[u8]) -> Vec<(String, f32, f32, f32, f32)> {
    let pdfium = extractor().pdfium().unwrap();
    let document = pdfium.load_pdf_from_byte_slice(bytes, None).unwrap();
    let mut out = Vec::new();
    for page in document.pages().iter() {
        let h = page.height().value;
        for object in page.objects().iter() {
            let Some(text) = object.as_text_object() else {
                continue;
            };
            if text.text().starts_with("MEMBRETE") {
                continue;
            }
            let b = object.bounds().unwrap();
            out.push((
                text.text().trim().to_owned(),
                b.left().value,
                b.right().value,
                h - b.bottom().value,
                page.width().value,
            ));
        }
    }
    out
}

#[test]
fn narrow_margins_with_big_amounts_never_overlap_numbers() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app(&dir);
    let customer = setup(&mut app);
    app.set_sales_letterhead(&make_letterhead(1, false))
        .unwrap();
    let margins = LetterheadMargins {
        top_mm: 30,
        right_mm: 55,
        bottom_mm: 20,
        left_mm: 55,
    };
    app.set_sales_letterhead_margins(margins).unwrap();
    let id = big_amounts_invoice(&mut app, customer);
    let bytes = std::fs::read(app.sales_document_pdf(id).unwrap()).unwrap();

    let boxes = text_boxes(&bytes);
    let all: Vec<&str> = boxes.iter().map(|b| b.0.as_str()).collect();
    assert!(all.contains(&"1.234,56"), "{all:?}");
    assert!(all.contains(&"98.765,43"), "{all:?}");
    // Ninguna pareja de textos de la misma línea se pisa ni sale del área útil.
    let (min_x, max_x) = (
        f32::from(margins.left_mm) * MM - 1.0,
        boxes[0].4 - f32::from(margins.right_mm) * MM + 1.0,
    );
    for (i, a) in boxes.iter().enumerate() {
        assert!(
            a.1 >= min_x && a.2 <= max_x,
            "«{}» fuera del área útil",
            a.0
        );
        for b in &boxes[i + 1..] {
            if (a.3 - b.3).abs() < 1.0 {
                assert!(
                    a.2 <= b.1 || b.2 <= a.1,
                    "«{}» ({:.1}-{:.1}) se pisa con «{}» ({:.1}-{:.1})",
                    a.0,
                    a.1,
                    a.2,
                    b.0,
                    b.1,
                    b.2
                );
            }
        }
    }
}

#[test]
fn normal_margins_and_amounts_keep_the_proportional_columns() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app(&dir);
    let customer = setup(&mut app);
    let id = issue(
        &mut app,
        DocumentKind::Invoice,
        Some(customer),
        vec![line("Servicio sintético", "2", "125.50")],
    );
    let bytes = std::fs::read(app.sales_document_pdf(id).unwrap()).unwrap();
    let boxes = text_boxes(&bytes);
    let page_w = boxes[0].4;
    let (x0, x1) = (18.0 * MM, page_w - 18.0 * MM);
    let w = x1 - x0;
    let right_of = |text: &str| boxes.iter().find(|b| b.0 == text).unwrap().2;
    for (head, edge) in [
        ("Cant.", 0.52),
        ("Precio", 0.66),
        ("Dto.", 0.75),
        ("IVA", 0.84),
        ("Importe €", 1.0),
    ] {
        let got = right_of(head);
        let want = x0 + w * edge - 4.0;
        assert!((got - want).abs() < 1.5, "{head}: {got:.1} y no {want:.1}");
    }
    let total_left = boxes.iter().find(|b| b.0 == "TOTAL").unwrap().1;
    assert!((total_left - (x1 - w * 0.38)).abs() < 1.5);
}
