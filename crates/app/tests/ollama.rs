//! Flujo completo contra el Ollama local real, con datos sintéticos. No se ejecuta por
//! defecto porque necesita Ollama con `qwen3:8b` y, para el PDF escaneado, PDFium y
//! Tesseract (`scripts/setup-ocr.ps1`):
//! `cargo test -p nestadmin-app --test ollama -- --ignored --nocapture --test-threads 1`

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use nestadmin_ai::ollama::{OllamaProvider, DEFAULT_MODEL, DEFAULT_URL};
use nestadmin_app::{App, AppPaths};
use nestadmin_core::TaskState;
use nestadmin_extract::{Extractor, Tesseract};
use pdfium_render::prelude::*;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn open(dir: &Path) -> App {
    let provider =
        OllamaProvider::new(DEFAULT_URL, DEFAULT_MODEL, Duration::from_secs(300)).unwrap();
    let extractor = Extractor::new(
        root().join("vendor/pdfium/bin"),
        Box::new(Tesseract::new(
            Tesseract::default_executable(),
            root().join("vendor/tessdata"),
        )),
    );
    App::open(&AppPaths::new(dir), Arc::new(provider), Arc::new(extractor)).unwrap()
}

#[test]
#[ignore = "requiere Ollama local con qwen3:8b"]
fn document_and_mail_flow_with_local_ollama() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = open(dir.path());

    app.import_document(
        "sintetica.txt",
        "Factura F-901. Proveedor: Alba Solar SL. Fecha: 2026-01-10. Base: 100.00 EUR. IVA: 21.00 EUR. Total: 121.00 EUR.",
    )
    .unwrap();
    let invoice = app.process_next().unwrap().unwrap();
    let result = invoice.result.clone().unwrap_or_default();
    println!("factura: {:?} {}", invoice.state, result["fields"]);
    assert_eq!(
        invoice.state,
        TaskState::AwaitingReview,
        "{:?}",
        invoice.error
    );
    assert_eq!(result["fields"]["total"], "121.00");
    assert_eq!(result["provider"], "ollama");

    let before = app.simulators().1.record_count().unwrap();
    assert_eq!(
        app.approve(invoice.id, 1).unwrap().state,
        TaskState::Completed
    );
    assert_eq!(app.simulators().1.record_count().unwrap(), before + 1);

    app.fetch_mail().unwrap();
    let email = app.process_next().unwrap().unwrap();
    let result = email.result.clone().unwrap_or_default();
    println!(
        "correo: {:?} {} / {}",
        email.state, result["category"], result["draft"]["body"]
    );
    assert_eq!(email.state, TaskState::AwaitingReview, "{:?}", email.error);
    assert!(app.simulators().0.sent().unwrap().is_empty());
}

/// PDF sintético sin capa de texto: la imagen de una factura, como un escaneo.
fn scanned_invoice_pdf() -> Vec<u8> {
    let pdfium = match Pdfium::bind_to_library(Pdfium::pdfium_platform_library_name_at_path(
        &root().join("vendor/pdfium/bin"),
    )) {
        Ok(bindings) => Pdfium::new(bindings),
        Err(PdfiumError::PdfiumLibraryBindingsAlreadyInitialized) => Pdfium::default(),
        Err(e) => panic!("PDFium no disponible; ejecute scripts/setup-ocr.ps1 ({e})"),
    };
    let mut source = pdfium.create_new_pdf().unwrap();
    let font = source.fonts_mut().helvetica();
    let mut page = source
        .pages_mut()
        .create_page_at_end(PdfPagePaperSize::a4())
        .unwrap();
    let lines = [
        "FACTURA F-2026-078",
        "Proveedor: Norte Papeleria SL",
        "Fecha: 2026-02-03",
        "Base imponible: 200.00 EUR",
        "IVA 21%: 42.00 EUR",
        "Total: 242.00 EUR",
    ];
    for (i, line) in lines.iter().enumerate() {
        page.objects_mut()
            .create_text_object(
                PdfPoints::new(72.0),
                PdfPoints::new(760.0 - 28.0 * i as f32),
                line,
                font,
                PdfPoints::new(16.0),
            )
            .unwrap();
    }
    let image = page
        .render_with_config(&PdfRenderConfig::new().scale_page_by_factor(200.0 / 72.0))
        .unwrap()
        .as_image()
        .unwrap();
    let mut scanned = pdfium.create_new_pdf().unwrap();
    let mut page = scanned
        .pages_mut()
        .create_page_at_end(PdfPagePaperSize::a4())
        .unwrap();
    let width = page.width();
    page.objects_mut()
        .create_image_object(PdfPoints::ZERO, PdfPoints::ZERO, &image, Some(width), None)
        .unwrap();
    scanned.save_to_bytes().unwrap()
}

#[test]
#[ignore = "requiere Ollama local, PDFium y Tesseract"]
fn scanned_pdf_goes_through_local_ocr_and_ollama() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = open(dir.path());
    app.import_file("escaneada.pdf", &scanned_invoice_pdf())
        .unwrap();
    let task = app.process_next().unwrap().unwrap();
    let result = task.result.clone().unwrap_or_default();
    println!(
        "pdf escaneado: {:?} {} páginas={} ilegibles={}",
        task.state, result["fields"], result["pages"], result["unreadable"]
    );
    assert_eq!(task.state, TaskState::AwaitingReview, "{:?}", task.error);
    assert_eq!(result["pages"][0]["method"], "ocr");
    assert_eq!(result["fields"]["total"], "242.00");
    assert_eq!(result["fields"]["subtotal"], "200.00");
    assert_eq!(result["issues"], serde_json::json!([]));
}
