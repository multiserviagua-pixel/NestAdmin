//! Obtención del texto de un documento local. Por ahora solo texto plano UTF-8; el PDF
//! (PDFium, D09) y el OCR (Tesseract, D10) devuelven un error explícito hasta integrarse.

use std::path::Path;

use crate::error::{DocumentError, Result};

/// Formato reconocido por la extensión del archivo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentFormat {
    PlainText,
    Pdf,
}

impl DocumentFormat {
    /// `None` si la extensión no corresponde a ningún formato previsto.
    pub fn from_path(path: &Path) -> Option<Self> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        match ext.as_str() {
            "txt" => Some(Self::PlainText),
            "pdf" => Some(Self::Pdf),
            _ => None,
        }
    }
}

/// Texto obtenido y su procedencia, para no presentar como completo lo que no lo es (R04).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractedText {
    pub format: DocumentFormat,
    pub text: String,
}

/// Fuente de texto de documentos; permite sustituir la implementación local en pruebas.
pub trait DocumentText {
    fn extract_text(&self, path: &Path) -> Result<ExtractedText>;
}

/// Implementación local, sin red.
#[derive(Debug, Default, Clone, Copy)]
pub struct LocalDocumentText;

impl DocumentText for LocalDocumentText {
    fn extract_text(&self, path: &Path) -> Result<ExtractedText> {
        match DocumentFormat::from_path(path) {
            Some(DocumentFormat::PlainText) => {
                let bytes = std::fs::read(path)?;
                let text = String::from_utf8(bytes).map_err(|_| DocumentError::InvalidUtf8)?;
                // Se quita la marca de orden de bytes que añaden algunos editores de Windows.
                let text = text.strip_prefix('\u{feff}').unwrap_or(&text).to_owned();
                if text.trim().is_empty() {
                    return Err(DocumentError::EmptyText);
                }
                Ok(ExtractedText {
                    format: DocumentFormat::PlainText,
                    text,
                })
            }
            Some(DocumentFormat::Pdf) => Err(DocumentError::UnsupportedFormat {
                format: "pdf".to_owned(),
            }),
            None => Err(DocumentError::UnsupportedFormat {
                format: path
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(str::to_ascii_lowercase)
                    .unwrap_or_else(|| "desconocido".to_owned()),
            }),
        }
    }
}
