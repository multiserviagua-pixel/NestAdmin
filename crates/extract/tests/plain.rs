//! Casos que no necesitan PDFium ni Tesseract: se ejecutan siempre, también en CI.

use std::path::Path;

use nestadmin_extract::{ExtractError, Extraction, Extractor, Method, Ocr, Page, Tesseract};

struct NoOcr;

impl Ocr for NoOcr {
    fn recognize(&self, _image: &Path, page: usize) -> Result<String, ExtractError> {
        Err(ExtractError::OcrFailed {
            page,
            message: "no usado".into(),
        })
    }
}

fn extractor(dir: &Path) -> Extractor {
    // Carpeta sin pdfium.dll: este archivo de pruebas nunca enlaza PDFium.
    Extractor::new(dir.join("sin-pdfium"), Box::new(NoOcr))
}

#[test]
fn plain_text_is_read_as_one_page() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("factura.TXT");
    std::fs::write(&path, "Factura F-1. Total: 121.00 EUR.").unwrap();
    let extraction = extractor(dir.path()).extract(&path).unwrap();
    assert_eq!(extraction.text, "Factura F-1. Total: 121.00 EUR.");
    assert_eq!(extraction.pages.len(), 1);
    assert_eq!(extraction.pages[0].method, Method::PlainText);
    assert_eq!(extraction.pages[0].chars, 27);
}

#[test]
fn empty_invalid_and_unsupported_files_are_explicit_errors() {
    let dir = tempfile::tempdir().unwrap();
    let extractor = extractor(dir.path());
    let write = |name: &str, bytes: &[u8]| {
        let path = dir.path().join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    };
    let cases = [
        (write("vacio.txt", b"  \n\t"), "empty_document", false),
        (
            write("latin1.txt", b"Factura n\xba 1"),
            "document_unreadable",
            false,
        ),
        (write("hoja.xlsx", b"PK"), "unsupported_format", false),
        (
            write("sin-extension", b"texto"),
            "unsupported_format",
            false,
        ),
        (
            dir.path().join("no-existe.txt"),
            "document_unreadable",
            false,
        ),
    ];
    for (path, code, recoverable) in cases {
        let error = extractor.extract(&path).unwrap_err();
        assert_eq!(error.code(), code, "{}", path.display());
        assert_eq!(error.recoverable(), recoverable);
    }
}

#[test]
fn missing_pdfium_is_reported_and_recoverable() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("documento.pdf");
    std::fs::write(&path, b"%PDF-1.4").unwrap();
    let error = extractor(dir.path()).extract(&path).unwrap_err();
    assert_eq!(error.code(), "pdf_unavailable");
    assert!(error.recoverable());
}

#[test]
fn tesseract_without_language_data_or_binary_is_unavailable() {
    let dir = tempfile::tempdir().unwrap();
    let image = dir.path().join("pagina.png");
    let error = Tesseract::new("tesseract", dir.path())
        .recognize(&image, 1)
        .unwrap_err();
    assert_eq!(error.code(), "ocr_unavailable");
    assert!(error.to_string().contains("spa"));

    for language in ["spa", "eng"] {
        std::fs::write(dir.path().join(format!("{language}.traineddata")), b"").unwrap();
    }
    let error = Tesseract::new(dir.path().join("no-existe.exe"), dir.path())
        .recognize(&image, 1)
        .unwrap_err();
    assert_eq!(error.code(), "ocr_unavailable");
    assert!(error.recoverable());
}

#[test]
fn each_found_text_names_its_page() {
    let first = "Proveedor Alfa SL
Fecha 05/10/2026";
    let page = |number, offset| Page {
        number,
        method: Method::PdfText,
        chars: 1,
        confidence: None,
        low_confidence: false,
        offset,
    };
    let extraction = Extraction {
        text: format!(
            "{first}

Total 121,00
Proveedor Alfa SL"
        ),
        pages: vec![page(1, 0), page(2, first.len() + 2)],
    };
    assert_eq!(extraction.pages_containing("Total 121,00"), [2]);
    assert_eq!(extraction.pages_containing("Alfa SL"), [1, 2]);
    assert!(extraction.pages_containing("No aparece").is_empty());
    assert!(extraction.pages_containing("  ").is_empty());
}
