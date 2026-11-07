//! PDF mediante PDFium (D09): texto de cada página y, si no alcanza el umbral, rasterizado y
//! OCR de esa página.

use std::path::Path;
use std::sync::OnceLock;

use pdfium_render::prelude::*;

use crate::{visible_chars, ExtractError, Extraction, Method, Ocr, Page, OCR_DPI};

/// PDFium solo se puede enlazar una vez por proceso; el resultado se reutiliza.
static PDFIUM: OnceLock<Result<Pdfium, String>> = OnceLock::new();

fn pdfium(dir: &Path) -> Result<&'static Pdfium, ExtractError> {
    PDFIUM
        .get_or_init(|| {
            match Pdfium::bind_to_library(Pdfium::pdfium_platform_library_name_at_path(dir)) {
                Ok(bindings) => Ok(Pdfium::new(bindings)),
                // Otro componente del proceso ya lo enlazó: se reutiliza.
                Err(PdfiumError::PdfiumLibraryBindingsAlreadyInitialized) => Ok(Pdfium::default()),
                Err(e) => Err(format!(
                    "no se pudo cargar pdfium.dll desde {} ({e})",
                    dir.display()
                )),
            }
        })
        .as_ref()
        .map_err(|e| ExtractError::PdfUnavailable(e.clone()))
}

pub(crate) fn extract(
    pdfium_dir: &Path,
    ocr: &dyn Ocr,
    min_chars: usize,
    min_confidence: f32,
    path: &Path,
) -> Result<Extraction, ExtractError> {
    let pdfium = pdfium(pdfium_dir)?;
    let document = pdfium.load_pdf_from_file(path, None).map_err(load_error)?;
    let scratch = tempfile::tempdir().map_err(|e| {
        ExtractError::OcrUnavailable(format!("sin carpeta temporal ({})", e.kind()))
    })?;
    let mut text = String::new();
    let mut pages = Vec::new();
    for (index, page) in document.pages().iter().enumerate() {
        let number = index + 1;
        let page_text = page
            .text()
            .map(|t| t.all())
            .map_err(|e| ExtractError::Unreadable(format!("página {number}: {e}")))?;
        let (page_text, page_info) = if visible_chars(&page_text) >= min_chars {
            let chars = visible_chars(&page_text);
            (page_text, Page::text(number, Method::PdfText, chars))
        } else {
            let image = scratch.path().join(format!("pagina-{number}.png"));
            page.render_with_config(&PdfRenderConfig::new().scale_page_by_factor(OCR_DPI / 72.0))
                .and_then(|bitmap| bitmap.as_image())
                .map_err(|e| ExtractError::OcrFailed {
                    page: number,
                    message: format!("no se pudo rasterizar ({e})"),
                })?
                .into_luma8()
                .save_with_format(&image, image::ImageFormat::Png)
                .map_err(|e| ExtractError::OcrFailed {
                    page: number,
                    message: format!("no se pudo guardar la imagen ({e})"),
                })?;
            let recognized = ocr.recognize_page(&image, number)?;
            let chars = visible_chars(&recognized.text);
            let info = Page::ocr(number, chars, recognized.confidence, min_confidence);
            (recognized.text, info)
        };
        if !text.is_empty() {
            text.push_str("\n\n");
        }
        let offset = text.len();
        text.push_str(page_text.trim_end());
        pages.push(Page {
            offset,
            ..page_info
        });
    }
    if pages.is_empty() || visible_chars(&text) == 0 {
        return Err(ExtractError::Empty);
    }
    // Una página escaneada de la que el OCR no obtiene nada deja el documento incompleto.
    if let Some(page) = pages
        .iter()
        .find(|p| p.method == Method::Ocr && p.chars == 0)
    {
        return Err(ExtractError::OcrFailed {
            page: page.number,
            message: "no se reconoció texto".into(),
        });
    }
    Ok(Extraction { text, pages })
}

fn load_error(error: PdfiumError) -> ExtractError {
    match error {
        PdfiumError::PdfiumLibraryInternalError(PdfiumInternalError::PasswordError)
        | PdfiumError::PdfiumLibraryInternalError(PdfiumInternalError::SecurityError) => {
            ExtractError::Unreadable("PDF protegido con contraseña".into())
        }
        PdfiumError::PdfiumLibraryInternalError(PdfiumInternalError::FileError) => {
            ExtractError::Unreadable("no se pudo abrir el archivo".into())
        }
        other => ExtractError::Unreadable(format!("PDF dañado o no válido ({other})")),
    }
}
