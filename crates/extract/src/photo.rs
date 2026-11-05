//! Imágenes de una página (foto de móvil o escaneo en JPG/PNG): se endereza según la
//! orientación EXIF, se pasa a gris, se lleva a una resolución útil para Tesseract y se
//! reconoce como una página escaneada (D62).

use std::path::Path;

use image::imageops::FilterType;
use image::{DynamicImage, GrayImage, ImageDecoder, ImageReader};

use crate::{visible_chars, ExtractError, Extraction, Ocr, Page};

/// Lado mayor máximo que se entrega al OCR. Una foto de móvil de 12–50 Mpx se reduce a este
/// tamaño (equivale a un A4 a 300 ppp): el OCR tarda menos y no pierde calidad (D62).
pub const MAX_LONG_SIDE: u32 = 3508;
/// Por debajo de este lado mayor la imagen se amplía (un A4 a unos 150 ppp): con poca
/// resolución Tesseract pierde caracteres (D62).
pub const MIN_LONG_SIDE: u32 = 1754;
/// Ampliación máxima de una imagen pequeña.
const MAX_UPSCALE: u32 = 3;

/// Abre la imagen, aplica la orientación EXIF y la normaliza para el OCR.
pub fn prepare(path: &Path) -> Result<GrayImage, ExtractError> {
    let unreadable = |e: image::ImageError| ExtractError::Unreadable(format!("imagen: {e}"));
    let reader = ImageReader::open(path)
        .map_err(|e| ExtractError::Unreadable(e.kind().to_string()))?
        .with_guessed_format()
        .map_err(|e| ExtractError::Unreadable(e.kind().to_string()))?;
    let mut decoder = reader.into_decoder().map_err(unreadable)?;
    // Un EXIF dañado no impide leer la imagen: se trata como sin rotación.
    let orientation = decoder
        .orientation()
        .unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut image = DynamicImage::from_decoder(decoder).map_err(unreadable)?;
    image.apply_orientation(orientation);
    Ok(normalize(image.into_luma8()))
}

/// Ajusta el tamaño: reduce las imágenes enormes y amplía las pequeñas.
pub fn normalize(image: GrayImage) -> GrayImage {
    let (width, height) = image.dimensions();
    let long = width.max(height);
    if long == 0 {
        return image;
    }
    let scale = if long > MAX_LONG_SIDE {
        MAX_LONG_SIDE as f64 / long as f64
    } else if long < MIN_LONG_SIDE {
        (MIN_LONG_SIDE as f64 / long as f64)
            .ceil()
            .min(MAX_UPSCALE as f64)
    } else {
        return image;
    };
    let target = |side: u32| ((side as f64 * scale).round() as u32).max(1);
    let filter = if scale < 1.0 {
        FilterType::Triangle
    } else {
        FilterType::CatmullRom
    };
    image::imageops::resize(&image, target(width), target(height), filter)
}

pub(crate) fn extract(
    ocr: &dyn Ocr,
    min_confidence: f32,
    path: &Path,
) -> Result<Extraction, ExtractError> {
    let image = prepare(path)?;
    let scratch = tempfile::tempdir().map_err(|e| {
        ExtractError::OcrUnavailable(format!("sin carpeta temporal ({})", e.kind()))
    })?;
    let png = scratch.path().join("pagina-1.png");
    image
        .save_with_format(&png, image::ImageFormat::Png)
        .map_err(|e| ExtractError::OcrFailed {
            page: 1,
            message: format!("no se pudo guardar la imagen ({e})"),
        })?;
    let recognized = ocr.recognize_page(&png, 1)?;
    let chars = visible_chars(&recognized.text);
    if chars == 0 {
        return Err(ExtractError::OcrFailed {
            page: 1,
            message: "no se reconoció texto".into(),
        });
    }
    Ok(Extraction {
        text: recognized.text.trim_end().to_owned(),
        pages: vec![Page::ocr(1, chars, recognized.confidence, min_confidence)],
    })
}
