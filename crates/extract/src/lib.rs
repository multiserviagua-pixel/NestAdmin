//! Extracción local de texto (R04, D09, D10): texto plano, PDF con capa de texto mediante
//! PDFium, y páginas escaneadas e imágenes (fotos de factura) mediante OCR con Tesseract como
//! proceso local. Nada sale del equipo. Si una página no se puede leer, el documento entero falla con el motivo: nunca se
//! entrega una extracción incompleta como si fuera completa.

mod ocr;
mod pdf;
pub mod photo;

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub use ocr::{mean_confidence, Ocr, OcrText, Tesseract};

/// Una página con menos caracteres visibles que este umbral se considera escaneada (D10,
/// calibrado en D65).
pub const DEFAULT_MIN_CHARS: usize = 60;
/// Por debajo de esta confianza media de Tesseract (0–100) la página se marca como OCR de
/// baja calidad (D66).
pub const DEFAULT_MIN_CONFIDENCE: f32 = 85.0;
/// Resolución con la que se rasteriza una página para el OCR.
pub const OCR_DPI: f32 = 300.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Method {
    PlainText,
    PdfText,
    Ocr,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Page {
    pub number: usize,
    pub method: Method,
    /// Caracteres visibles (sin espacios) obtenidos en la página.
    pub chars: usize,
    /// Confianza media del OCR (0–100); solo en páginas con OCR.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f32>,
    /// OCR por debajo del umbral de confianza: el texto puede tener errores (D66).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub low_confidence: bool,
    /// Posición (en bytes) en que empieza el texto de la página dentro de `Extraction::text`.
    /// Sirve para saber de qué página salió cada campo extraído (D49).
    #[serde(default)]
    pub offset: usize,
}

impl Extraction {
    /// Páginas en que aparece `needle` en el texto extraído, en orden y sin repetir.
    pub fn pages_containing(&self, needle: &str) -> Vec<usize> {
        if needle.trim().is_empty() {
            return Vec::new();
        }
        let mut found = Vec::new();
        for (index, _) in self.text.match_indices(needle) {
            let page = self
                .pages
                .iter()
                .rev()
                .find(|p| p.offset <= index)
                .map(|p| p.number);
            if let Some(page) = page {
                if !found.contains(&page) {
                    found.push(page);
                }
            }
        }
        found
    }
}

impl Page {
    pub(crate) fn text(number: usize, method: Method, chars: usize) -> Self {
        Self {
            number,
            method,
            chars,
            confidence: None,
            low_confidence: false,
            offset: 0,
        }
    }

    pub(crate) fn ocr(number: usize, chars: usize, confidence: Option<f32>, min: f32) -> Self {
        Self {
            number,
            method: Method::Ocr,
            chars,
            confidence,
            low_confidence: confidence.is_some_and(|c| c < min),
            offset: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Extraction {
    pub text: String,
    pub pages: Vec<Page>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ExtractError {
    #[error("formato no admitido: {0}")]
    Unsupported(String),
    #[error("documento ilegible: {0}")]
    Unreadable(String),
    #[error("el documento no contiene texto")]
    Empty,
    #[error("PDFium no disponible: {0}")]
    PdfUnavailable(String),
    #[error("OCR no disponible: {0}")]
    OcrUnavailable(String),
    #[error("falló el OCR de la página {page}: {message}")]
    OcrFailed { page: usize, message: String },
}

impl ExtractError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Unsupported(_) => "unsupported_format",
            Self::Unreadable(_) => "document_unreadable",
            Self::Empty => "empty_document",
            Self::PdfUnavailable(_) => "pdf_unavailable",
            Self::OcrUnavailable(_) => "ocr_unavailable",
            Self::OcrFailed { .. } => "ocr_failed",
        }
    }

    /// Los fallos de componentes locales se pueden reintentar tras corregirlos; un
    /// documento dañado o vacío no cambia al reintentar.
    pub fn recoverable(&self) -> bool {
        matches!(
            self,
            Self::PdfUnavailable(_) | Self::OcrUnavailable(_) | Self::OcrFailed { .. }
        )
    }
}

/// Extensiones de imagen admitidas (fotos o escaneos de una sola página).
pub const IMAGE_EXTENSIONS: [&str; 3] = ["jpg", "jpeg", "png"];

pub fn visible_chars(text: &str) -> usize {
    text.chars().filter(|c| !c.is_whitespace()).count()
}

/// Extractor configurado con la ubicación de PDFium y el motor de OCR.
pub struct Extractor {
    pdfium_dir: PathBuf,
    ocr: Box<dyn Ocr>,
    min_chars: usize,
    min_confidence: f32,
}

impl Extractor {
    pub fn new(pdfium_dir: impl Into<PathBuf>, ocr: Box<dyn Ocr>) -> Self {
        Self {
            pdfium_dir: pdfium_dir.into(),
            ocr,
            min_chars: DEFAULT_MIN_CHARS,
            min_confidence: DEFAULT_MIN_CONFIDENCE,
        }
    }

    pub fn with_min_chars(mut self, min_chars: usize) -> Self {
        self.min_chars = min_chars;
        self
    }

    pub fn with_min_confidence(mut self, min_confidence: f32) -> Self {
        self.min_confidence = min_confidence;
        self
    }

    /// Motor de OCR configurado.
    pub fn ocr(&self) -> &dyn Ocr {
        self.ocr.as_ref()
    }

    /// Extrae el texto según la extensión del archivo: `.txt`, `.pdf` o imagen (`.jpg`,
    /// `.jpeg`, `.png`).
    pub fn extract(&self, path: &Path) -> Result<Extraction, ExtractError> {
        let extension = path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .unwrap_or_default();
        match extension.as_str() {
            "txt" => plain_text(path),
            "pdf" => pdf::extract(
                &self.pdfium_dir,
                self.ocr.as_ref(),
                self.min_chars,
                self.min_confidence,
                path,
            ),
            ext if IMAGE_EXTENSIONS.contains(&ext) => {
                photo::extract(self.ocr.as_ref(), self.min_confidence, path)
            }
            "" => Err(ExtractError::Unsupported("archivo sin extensión".into())),
            other => Err(ExtractError::Unsupported(format!(".{other}"))),
        }
    }
}

fn plain_text(path: &Path) -> Result<Extraction, ExtractError> {
    let bytes = std::fs::read(path).map_err(|e| ExtractError::Unreadable(e.kind().to_string()))?;
    let text = String::from_utf8(bytes)
        .map_err(|_| ExtractError::Unreadable("el texto no está en UTF-8".into()))?;
    let chars = visible_chars(&text);
    if chars == 0 {
        return Err(ExtractError::Empty);
    }
    Ok(Extraction {
        text,
        pages: vec![Page::text(1, Method::PlainText, chars)],
    })
}
