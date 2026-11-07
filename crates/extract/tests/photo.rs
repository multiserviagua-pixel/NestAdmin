//! Confianza del OCR en imágenes y ubicación de Tesseract (fase 9e), sin PDFium ni
//! Tesseract: se ejecutan siempre, también en CI. Todas las imágenes son sintéticas. La
//! orientación, el tamaño y los errores de imagen se prueban en `images.rs` (D67).

use std::path::Path;
use std::sync::Mutex;

use image::codecs::jpeg::JpegEncoder;
use image::{GrayImage, ImageEncoder, Luma};
use nestadmin_extract::{
    ExtractError, Extractor, Method, Ocr, OcrText, Tesseract, DEFAULT_MIN_CONFIDENCE,
};

/// OCR simulado: devuelve un texto y una confianza fijos y recuerda el tamaño de la imagen
/// que recibió.
struct FixedOcr {
    text: &'static str,
    confidence: Option<f32>,
    seen: Mutex<Option<(u32, u32)>>,
}

impl FixedOcr {
    fn new(text: &'static str, confidence: Option<f32>) -> Self {
        Self {
            text,
            confidence,
            seen: Mutex::new(None),
        }
    }
}

impl Ocr for FixedOcr {
    fn recognize(&self, image: &Path, page: usize) -> Result<String, ExtractError> {
        self.recognize_page(image, page).map(|r| r.text)
    }

    fn recognize_page(&self, image: &Path, _page: usize) -> Result<OcrText, ExtractError> {
        *self.seen.lock().unwrap() = Some(image::image_dimensions(image).unwrap());
        Ok(OcrText {
            text: self.text.to_owned(),
            confidence: self.confidence,
        })
    }
}

/// EXIF mínimo (TIFF little-endian) con solo la etiqueta de orientación.
fn exif_orientation(value: u16) -> Vec<u8> {
    let mut exif = vec![0x49, 0x49, 0x2A, 0x00, 0x08, 0x00, 0x00, 0x00, 0x01, 0x00];
    exif.extend([0x12, 0x01, 0x03, 0x00, 0x01, 0x00, 0x00, 0x00]);
    exif.extend(value.to_le_bytes());
    exif.extend([0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
    exif
}

fn jpeg(image: &GrayImage, path: &Path, orientation: Option<u16>) {
    let mut bytes = Vec::new();
    let mut encoder = JpegEncoder::new_with_quality(&mut bytes, 95);
    if let Some(value) = orientation {
        encoder.set_exif_metadata(exif_orientation(value)).unwrap();
    }
    encoder
        .write_image(
            image.as_raw(),
            image.width(),
            image.height(),
            image::ExtendedColorType::L8,
        )
        .unwrap();
    std::fs::write(path, bytes).unwrap();
}

/// Foto «de sensor» apaisada de 1800×1350 con una marca negra arriba a la izquierda. Con
/// EXIF 6 el móvil indica que hay que girarla 90° a la derecha: vertical de 1350×1800 con la
/// marca arriba a la derecha.
fn sensor_photo() -> GrayImage {
    GrayImage::from_fn(1800, 1350, |x, y| {
        if x < 200 && y < 200 {
            Luma([0])
        } else {
            Luma([255])
        }
    })
}

#[test]
fn image_files_go_through_ocr_with_confidence() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Factura.JPEG");
    jpeg(&sensor_photo(), &path, Some(6));

    let ocr = FixedOcr::new("FACTURA F-1\nTotal: 121,00 EUR\n", Some(91.5));
    let extractor = Extractor::new(dir.path().join("sin-pdfium"), Box::new(ocr));
    let extraction = extractor.extract(&path).unwrap();
    assert_eq!(extraction.text.trim_end(), "FACTURA F-1\nTotal: 121,00 EUR");
    let page = &extraction.pages[0];
    assert_eq!(page.method, Method::Ocr);
    assert_eq!(page.confidence, Some(91.5));
    assert!(!page.low_confidence);
    let json = serde_json::to_value(&extraction.pages).unwrap();
    assert_eq!(json[0]["method"], "ocr");
    assert_eq!(json[0]["confidence"], 91.5);
    assert!(json[0].get("low_confidence").is_none());

    // Por debajo del umbral, la página queda marcada.
    let low = Extractor::new(
        dir.path().join("sin-pdfium"),
        Box::new(FixedOcr::new(
            "FACTURA F-1",
            Some(DEFAULT_MIN_CONFIDENCE - 0.1),
        )),
    );
    let page = &low.extract(&path).unwrap().pages[0];
    assert!(page.low_confidence);
    assert_eq!(serde_json::to_value(page).unwrap()["low_confidence"], true);

    // Sin confianza informada no se marca nada.
    let unknown = Extractor::new(
        dir.path().join("sin-pdfium"),
        Box::new(FixedOcr::new("FACTURA F-1", None)),
    );
    assert!(!unknown.extract(&path).unwrap().pages[0].low_confidence);
}

#[test]
fn installed_app_only_uses_the_packaged_tesseract() {
    let dir = tempfile::tempdir().unwrap();
    let app = dir.path().join("NestAdmin");
    let vendor = dir.path().join("vendor");
    let packaged = app.join("tesseract").join("tesseract.exe");
    let vendored = vendor.join("tesseract").join("tesseract.exe");

    // Instalada: siempre la copia junto al ejecutable, aunque falte; nunca Program Files ni
    // el PATH.
    assert_eq!(Tesseract::locate(Some(&app), &vendor, false), packaged);
    std::fs::create_dir_all(vendored.parent().unwrap()).unwrap();
    std::fs::write(&vendored, b"").unwrap();
    assert_eq!(Tesseract::locate(Some(&app), &vendor, false), packaged);
    assert_ne!(
        Tesseract::locate(None, &vendor, false),
        Tesseract::default_executable()
    );

    // Desarrollo: empaquetada, si no vendor/, si no Program Files.
    assert_eq!(Tesseract::locate(Some(&app), &vendor, true), vendored);
    std::fs::create_dir_all(packaged.parent().unwrap()).unwrap();
    std::fs::write(&packaged, b"").unwrap();
    assert_eq!(Tesseract::locate(Some(&app), &vendor, true), packaged);
    let empty = dir.path().join("nada");
    assert_eq!(
        Tesseract::locate(Some(&empty), &empty, true),
        Tesseract::default_executable()
    );

    // Con la copia empaquetada ausente, el OCR falla de forma explícita y reintentable.
    for language in ["spa", "eng"] {
        std::fs::write(dir.path().join(format!("{language}.traineddata")), b"").unwrap();
    }
    let missing = Tesseract::locate(Some(&empty), &empty, false);
    let error = Tesseract::new(missing, dir.path())
        .recognize(&dir.path().join("x.png"), 1)
        .unwrap_err();
    assert_eq!(error.code(), "ocr_unavailable");
}
