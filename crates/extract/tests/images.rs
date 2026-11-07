//! Fotos de facturas con Tesseract real (D67) sobre imágenes sintéticas de `fixtures/`
//! (generadas con `fixtures/generar.py`). Necesitan Tesseract, así que no se ejecutan por
//! defecto: `cargo test -p nestadmin-extract --test images -- --ignored`. En Windows usan
//! `scripts/setup-ocr.ps1`; en otro sistema, `NESTADMIN_TESSERACT` y `NESTADMIN_TESSDATA`.

use std::path::{Path, PathBuf};

use nestadmin_extract::{ExtractError, Extractor, Method, Ocr, Tesseract};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn extractor() -> Extractor {
    let executable = std::env::var_os("NESTADMIN_TESSERACT")
        .map(PathBuf::from)
        .unwrap_or_else(Tesseract::default_executable);
    let tessdata = std::env::var_os("NESTADMIN_TESSDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| root().join("vendor/tessdata"));
    Extractor::new(
        root().join("vendor/pdfium/bin"),
        Box::new(Tesseract::new(executable, tessdata)),
    )
}

fn normalized(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[test]
#[ignore = "requiere Tesseract (scripts/setup-ocr.ps1 o NESTADMIN_TESSERACT)"]
fn photo_stored_rotated_is_read_upright_thanks_to_exif() {
    let extraction = extractor()
        .extract(&fixture("factura-girada-exif.jpg"))
        .unwrap();
    assert_eq!(extraction.pages.len(), 1);
    assert_eq!(extraction.pages[0].method, Method::Ocr);
    let text = normalized(&extraction.text);
    for expected in ["F-2026-077", "Alba Solar", "100.00", "21.00", "121.00"] {
        assert!(text.contains(expected), "falta «{expected}» en: {text}");
    }
}

#[test]
#[ignore = "requiere Tesseract (scripts/setup-ocr.ps1 o NESTADMIN_TESSERACT)"]
fn image_with_two_invoices_keeps_both_in_the_text() {
    let extraction = extractor().extract(&fixture("dos-facturas.jpg")).unwrap();
    let text = normalized(&extraction.text);
    for expected in [
        "RA-0412",
        "Río Azul",
        "242.00",
        "TN-88",
        "Talleres Norte",
        "60.50",
    ] {
        assert!(text.contains(expected), "falta «{expected}» en: {text}");
    }
}

/// Sin Tesseract: comprueba la decodificación y los errores sin OCR real.
struct EchoOcr;

impl Ocr for EchoOcr {
    fn recognize(&self, image: &Path, _page: usize) -> Result<String, ExtractError> {
        let size = image::image_dimensions(image).unwrap();
        Ok(format!("{}x{}", size.0, size.1))
    }
}

#[test]
fn exif_orientation_is_applied_before_ocr() {
    let extractor = Extractor::new(root().join("sin-pdfium"), Box::new(EchoOcr));
    // La factura mide 1240 × 1000 y se guardó girada (1000 × 1240) con orientación EXIF 6:
    // el OCR la recibe derecha, más ancha que alta (y ampliada al doble por ser pequeña).
    let text = extractor
        .extract(&fixture("factura-girada-exif.jpg"))
        .unwrap()
        .text;
    assert_eq!(text, "2480x2000");
}

#[test]
fn files_that_are_not_images_are_explicit_errors() {
    let dir = tempfile::tempdir().unwrap();
    let extractor = Extractor::new(root().join("sin-pdfium"), Box::new(EchoOcr));
    let fake = dir.path().join("foto.jpg");
    std::fs::write(&fake, b"esto no es una imagen").unwrap();
    assert_eq!(
        extractor.extract(&fake).unwrap_err().code(),
        "document_unreadable"
    );
    let heic = dir.path().join("IMG_0001.HEIC");
    std::fs::write(&heic, b"x").unwrap();
    let error = extractor.extract(&heic).unwrap_err();
    assert_eq!(error.code(), "unsupported_format");
    assert!(error.to_string().contains("JPG"), "{error}");
    let truncated = dir.path().join("cortada.jpg");
    let bytes = std::fs::read(fixture("dos-facturas.jpg")).unwrap();
    std::fs::write(&truncated, &bytes[..2000]).unwrap();
    assert_eq!(
        extractor.extract(&truncated).unwrap_err().code(),
        "document_unreadable"
    );
}
