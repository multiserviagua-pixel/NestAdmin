//! PDF y OCR con PDFium y Tesseract reales y documentos sintéticos generados en la prueba.
//! Necesitan `scripts/setup-ocr.ps1`, así que no se ejecutan por defecto:
//! `cargo test -p nestadmin-extract --test pdf -- --ignored`

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use nestadmin_extract::{ExtractError, Extractor, Method, Ocr, Tesseract};
use pdfium_render::prelude::*;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn pdfium_dir() -> PathBuf {
    root().join("vendor/pdfium/bin")
}

fn tesseract() -> Box<dyn Ocr> {
    Box::new(Tesseract::new(
        Tesseract::packaged_executable(&root().join("vendor")),
        root().join("vendor/tessdata"),
    ))
}

/// Las pruebas comparten el PDFium enlazado por el extractor (uno por proceso).
fn pdfium() -> Pdfium {
    match Pdfium::bind_to_library(Pdfium::pdfium_platform_library_name_at_path(&pdfium_dir())) {
        Ok(bindings) => Pdfium::new(bindings),
        Err(PdfiumError::PdfiumLibraryBindingsAlreadyInitialized) => Pdfium::default(),
        Err(e) => panic!("PDFium no disponible; ejecute scripts/setup-ocr.ps1 ({e})"),
    }
}

static PDFIUM_LOCK: Mutex<()> = Mutex::new(());

const LINES: [&str; 5] = [
    "FACTURA F-2026-077",
    "Proveedor: Alba Solar SL",
    "Fecha: 2026-01-10",
    "Base: 100.00 EUR   IVA: 21.00 EUR",
    "Total: 121.00 EUR",
];

/// Página A4 con texto real (capa de texto).
fn add_text_page(document: &mut PdfDocument<'_>) {
    let font = document.fonts_mut().helvetica();
    let mut page = document
        .pages_mut()
        .create_page_at_end(PdfPagePaperSize::a4())
        .unwrap();
    for (i, line) in LINES.iter().enumerate() {
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
}

/// Página "escaneada": la imagen de una página de texto, sin capa de texto.
fn add_scanned_page(pdfium: &Pdfium, document: &mut PdfDocument<'_>) {
    let mut source = pdfium.create_new_pdf().unwrap();
    add_text_page(&mut source);
    let image = source
        .pages()
        .first()
        .unwrap()
        .render_with_config(&PdfRenderConfig::new().scale_page_by_factor(200.0 / 72.0))
        .unwrap()
        .as_image()
        .unwrap();
    let mut page = document
        .pages_mut()
        .create_page_at_end(PdfPagePaperSize::a4())
        .unwrap();
    let width = page.width();
    page.objects_mut()
        .create_image_object(PdfPoints::ZERO, PdfPoints::ZERO, &image, Some(width), None)
        .unwrap();
}

fn save(document: &PdfDocument<'_>, dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    document.save_to_file(&path).unwrap();
    path
}

fn normalized(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[test]
#[ignore = "requiere PDFium y Tesseract (scripts/setup-ocr.ps1)"]
fn text_scanned_and_mixed_pdfs_use_the_right_method() {
    let _guard = PDFIUM_LOCK.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let pdfium = pdfium();
    let extractor = Extractor::new(pdfium_dir(), tesseract());

    let mut text = pdfium.create_new_pdf().unwrap();
    add_text_page(&mut text);
    let text = extractor
        .extract(&save(&text, dir.path(), "texto.pdf"))
        .unwrap();
    assert_eq!(text.pages[0].method, Method::PdfText);
    assert!(normalized(&text.text).contains("Proveedor: Alba Solar SL"));
    assert!(normalized(&text.text).contains("Total: 121.00 EUR"));

    let mut scanned = pdfium.create_new_pdf().unwrap();
    add_scanned_page(&pdfium, &mut scanned);
    let scanned = extractor
        .extract(&save(&scanned, dir.path(), "escaneado.pdf"))
        .unwrap();
    println!("OCR:\n{}", scanned.text);
    assert_eq!(scanned.pages[0].method, Method::Ocr);
    let confidence = scanned.pages[0]
        .confidence
        .expect("Tesseract informa la confianza");
    assert!(!scanned.pages[0].low_confidence, "confianza {confidence}");
    let ocr = normalized(&scanned.text);
    for expected in ["F-2026-077", "Alba Solar SL", "2026-01-10", "121.00"] {
        assert!(
            ocr.contains(expected),
            "falta «{expected}» en el OCR: {ocr}"
        );
    }

    let mut mixed = pdfium.create_new_pdf().unwrap();
    add_text_page(&mut mixed);
    add_scanned_page(&pdfium, &mut mixed);
    let mixed = extractor
        .extract(&save(&mixed, dir.path(), "mixto.pdf"))
        .unwrap();
    let methods: Vec<_> = mixed.pages.iter().map(|p| p.method).collect();
    assert_eq!(methods, [Method::PdfText, Method::Ocr]);
    // Cada página sabe dónde empieza su texto: el mismo dato aparece en las dos (D49).
    assert_eq!(mixed.pages[0].offset, 0);
    assert!(mixed.pages[1].offset > 0);
    assert_eq!(mixed.pages_containing("F-2026-077"), [1, 2]);
}

struct FailingOcr(Option<&'static str>);

impl Ocr for FailingOcr {
    fn recognize(&self, _image: &Path, page: usize) -> Result<String, ExtractError> {
        match self.0 {
            Some(text) => Ok(text.to_owned()),
            None => Err(ExtractError::OcrFailed {
                page,
                message: "fallo simulado".into(),
            }),
        }
    }
}

#[test]
#[ignore = "requiere PDFium (scripts/setup-ocr.ps1)"]
fn ocr_failure_never_returns_a_partial_extraction() {
    let _guard = PDFIUM_LOCK.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let pdfium = pdfium();
    let mut mixed = pdfium.create_new_pdf().unwrap();
    add_text_page(&mut mixed);
    add_scanned_page(&pdfium, &mut mixed);
    let path = save(&mixed, dir.path(), "mixto.pdf");

    let failing = Extractor::new(pdfium_dir(), Box::new(FailingOcr(None)));
    let error = failing.extract(&path).unwrap_err();
    assert_eq!(
        error,
        ExtractError::OcrFailed {
            page: 2,
            message: "fallo simulado".into()
        }
    );
    assert!(error.recoverable());

    let blank = Extractor::new(pdfium_dir(), Box::new(FailingOcr(Some("  \n"))));
    let error = blank.extract(&path).unwrap_err();
    assert_eq!(error.code(), "ocr_failed");
    assert!(error.to_string().contains("página 2"));
}

#[test]
#[ignore = "requiere PDFium (scripts/setup-ocr.ps1)"]
fn damaged_and_blank_pdfs_are_explicit_errors() {
    let _guard = PDFIUM_LOCK.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let pdfium = pdfium();
    let extractor = Extractor::new(pdfium_dir(), Box::new(FailingOcr(Some(""))));

    let damaged = dir.path().join("danado.pdf");
    std::fs::write(&damaged, b"%PDF-1.7\nesto no es un PDF").unwrap();
    assert_eq!(
        extractor.extract(&damaged).unwrap_err().code(),
        "document_unreadable"
    );

    let mut blank = pdfium.create_new_pdf().unwrap();
    blank
        .pages_mut()
        .create_page_at_end(PdfPagePaperSize::a4())
        .unwrap();
    let error = extractor
        .extract(&save(&blank, dir.path(), "blanco.pdf"))
        .unwrap_err();
    assert_eq!(error.code(), "empty_document");
}

/// Foto de móvil sintética de la factura de prueba: página sobre fondo oscuro, píxeles
/// guardados girados como en el sensor y orientación EXIF 6, JPEG de calidad media.
fn phone_photo_jpeg(pdfium: &Pdfium, path: &Path) {
    use image::{codecs::jpeg::JpegEncoder, GrayImage, ImageEncoder, Luma};
    let mut source = pdfium.create_new_pdf().unwrap();
    add_text_page(&mut source);
    let page = source
        .pages()
        .first()
        .unwrap()
        .render_with_config(&PdfRenderConfig::new().scale_page_by_factor(300.0 / 72.0))
        .unwrap()
        .as_image()
        .unwrap()
        .into_luma8();
    let mut canvas = GrayImage::from_pixel(page.width() + 400, page.height() + 300, Luma([70]));
    image::imageops::overlay(&mut canvas, &page, 200, 150);
    let sensor = image::imageops::rotate270(&image::imageops::blur(&canvas, 0.8));
    let exif = [
        0x49, 0x49, 0x2A, 0x00, 0x08, 0x00, 0x00, 0x00, 0x01, 0x00, 0x12, 0x01, 0x03, 0x00, 0x01,
        0x00, 0x00, 0x00, 0x06, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    ];
    let mut bytes = Vec::new();
    let mut encoder = JpegEncoder::new_with_quality(&mut bytes, 80);
    encoder.set_exif_metadata(exif.to_vec()).unwrap();
    encoder
        .write_image(
            sensor.as_raw(),
            sensor.width(),
            sensor.height(),
            image::ExtendedColorType::L8,
        )
        .unwrap();
    std::fs::write(path, bytes).unwrap();
}

/// D38: la copia de `vendor/tesseract` (la que lleva el instalador) funciona sola desde una
/// carpeta de instalación, con un `PATH` sin ningún otro Tesseract, y `Tesseract::locate` la
/// elige en una aplicación instalada. Reconoce una foto de móvil sintética con EXIF.
#[test]
#[ignore = "requiere PDFium y el Tesseract empaquetado (scripts/setup-ocr.ps1)"]
fn packaged_tesseract_reads_a_phone_photo_from_an_install_folder() {
    let _guard = PDFIUM_LOCK.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let app = dir.path().join("NestAdmin");
    let target = app.join("tesseract");
    std::fs::create_dir_all(&target).unwrap();
    for entry in std::fs::read_dir(root().join("vendor/tesseract")).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(entry.path(), target.join(entry.file_name())).unwrap();
    }
    let exe = Tesseract::locate(Some(&app), &dir.path().join("sin-vendor"), false);
    assert_eq!(exe, target.join("tesseract.exe"));

    let photo = dir.path().join("foto.jpg");
    phone_photo_jpeg(&pdfium(), &photo);
    let extractor = Extractor::new(
        pdfium_dir(),
        Box::new(Tesseract::new(&exe, root().join("vendor/tessdata"))),
    );
    let system32 = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
    let saved = std::env::var_os("PATH");
    std::env::set_var("PATH", format!(r"{system32}\System32"));
    let result = extractor.extract(&photo);
    match saved {
        Some(path) => std::env::set_var("PATH", path),
        None => std::env::remove_var("PATH"),
    }
    let extraction = result.unwrap();
    let page = &extraction.pages[0];
    println!("confianza {:?}", page.confidence);
    assert_eq!(page.method, Method::Ocr);
    assert!(!page.low_confidence, "confianza {:?}", page.confidence);
    let ocr = normalized(&extraction.text);
    for expected in ["F-2026-077", "Alba Solar SL", "2026-01-10", "121.00"] {
        assert!(ocr.contains(expected), "falta «{expected}»: {ocr}");
    }
}

/// D65: algunos escáneres añaden a la página escaneada un sello de texto (fecha, «página 1
/// de 1», el nombre de la aplicación). Con el umbral anterior de 20 caracteres esa página se
/// tomaba como texto y no se hacía OCR; con el actual se reconoce la factura.
#[test]
#[ignore = "requiere PDFium y el Tesseract empaquetado (scripts/setup-ocr.ps1)"]
fn a_scanned_page_with_a_short_text_stamp_still_goes_through_ocr() {
    let _guard = PDFIUM_LOCK.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let pdfium = pdfium();
    let mut document = pdfium.create_new_pdf().unwrap();
    add_scanned_page(&pdfium, &mut document);
    let font = document.fonts_mut().helvetica();
    let stamp = "Digitalizado con EscanerMovil Gratis";
    document
        .pages_mut()
        .first()
        .unwrap()
        .objects_mut()
        .create_text_object(
            PdfPoints::new(40.0),
            PdfPoints::new(20.0),
            stamp,
            font,
            PdfPoints::new(7.0),
        )
        .unwrap();
    let path = save(&document, dir.path(), "sello.pdf");
    assert!(nestadmin_extract::visible_chars(stamp) > 20);

    let before = Extractor::new(pdfium_dir(), tesseract()).with_min_chars(20);
    assert_eq!(
        before.extract(&path).unwrap().pages[0].method,
        Method::PdfText
    );

    let extraction = Extractor::new(pdfium_dir(), tesseract())
        .extract(&path)
        .unwrap();
    assert_eq!(extraction.pages[0].method, Method::Ocr);
    let ocr = normalized(&extraction.text);
    for expected in ["F-2026-077", "Alba Solar SL", "121.00"] {
        assert!(ocr.contains(expected), "falta «{expected}»: {ocr}");
    }
}
