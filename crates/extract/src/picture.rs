//! Fotos y escaneos sueltos (JPG, PNG, TIFF, BMP, WebP) mediante OCR local (D67).
//!
//! La imagen se decodifica aquí y no en Tesseract para aplicar la orientación EXIF: las fotos
//! de móvil suelen guardarse giradas con esa etiqueta, que Tesseract ignora. Se pasa a escala
//! de grises, se reduce si es desmesurada y se amplía si es demasiado pequeña para leer.

use std::path::Path;

use image::imageops::FilterType;
use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader, Limits};

use crate::{visible_chars, ExtractError, Extraction, Method, Ocr, Page};

/// Extensiones de imagen que se procesan con OCR.
pub const IMAGE_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "tif", "tiff", "bmp", "webp"];

/// Lado mayor a partir del cual se reduce la imagen: una foto de 50 megapíxeles no mejora el
/// OCR y multiplica su tiempo. 6000 px cubre un A4 a unos 500 ppp.
const MAX_SIDE: u32 = 6000;
/// Lado mayor por debajo del cual se amplía: con letra de pocos píxeles Tesseract falla.
const MIN_SIDE: u32 = 1500;
/// Memoria máxima que puede reservar el decodificador (una imagen de 100 megapíxeles en RGBA
/// ocupa 400 MB): un archivo manipulado no puede agotar la memoria del equipo.
const MAX_DECODE_BYTES: u64 = 512 * 1024 * 1024;

pub(crate) fn extract(ocr: &dyn Ocr, path: &Path) -> Result<Extraction, ExtractError> {
    let image = load(path)?;
    let scratch = tempfile::tempdir().map_err(|e| {
        ExtractError::OcrUnavailable(format!("sin carpeta temporal ({})", e.kind()))
    })?;
    let prepared = scratch.path().join("imagen.png");
    prepare(image)
        .save(&prepared)
        .map_err(|e| ExtractError::OcrFailed {
            page: 1,
            message: format!("no se pudo preparar la imagen ({e})"),
        })?;
    let text = ocr.recognize(&prepared, 1)?;
    let chars = visible_chars(&text);
    if chars == 0 {
        return Err(ExtractError::Empty);
    }
    Ok(Extraction {
        text,
        pages: vec![Page {
            number: 1,
            method: Method::Ocr,
            chars,
            offset: 0,
        }],
    })
}

/// Decodifica según el contenido (no la extensión) y aplica la orientación EXIF.
fn load(path: &Path) -> Result<DynamicImage, ExtractError> {
    let unreadable = |e: &dyn std::fmt::Display| ExtractError::Unreadable(format!("imagen: {e}"));
    let mut reader = ImageReader::open(path)
        .map_err(|e| ExtractError::Unreadable(e.kind().to_string()))?
        .with_guessed_format()
        .map_err(|e| ExtractError::Unreadable(e.kind().to_string()))?;
    if reader.format().is_none() {
        return Err(ExtractError::Unsupported(
            "el archivo no es una imagen reconocible (HEIC no está admitido: expórtela como JPG)"
                .into(),
        ));
    }
    if reader.format() == Some(ImageFormat::Jpeg) {
        let bytes =
            std::fs::read(path).map_err(|e| ExtractError::Unreadable(e.kind().to_string()))?;
        if !jpeg_is_complete(&bytes) {
            return Err(ExtractError::Unreadable(
                "la imagen JPEG está cortada (¿descarga incompleta?); se usaría solo una parte"
                    .into(),
            ));
        }
    }
    let mut limits = Limits::default();
    limits.max_alloc = Some(MAX_DECODE_BYTES);
    reader.limits(limits);
    let mut decoder = reader.into_decoder().map_err(|e| unreadable(&e))?;
    let orientation = decoder.orientation().map_err(|e| unreadable(&e))?;
    let mut image = DynamicImage::from_decoder(decoder).map_err(|e| unreadable(&e))?;
    image.apply_orientation(orientation);
    Ok(image)
}

/// Un JPEG completo termina su último barrido (`FF DA`) con el marcador de fin (`FF D9`).
/// Dentro de los datos comprimidos un `FF` siempre va seguido de `00` o de un reinicio
/// (`D0`–`D7`), así que `FF D9` tras el último barrido solo puede ser el fin. El decodificador
/// acepta un archivo cortado y rellena lo que falta; aquí se rechaza.
fn jpeg_is_complete(bytes: &[u8]) -> bool {
    let last_scan = bytes.windows(2).rposition(|w| w == [0xFF, 0xDA]);
    match last_scan {
        Some(start) => bytes[start..].windows(2).any(|w| w == [0xFF, 0xD9]),
        None => false,
    }
}

fn prepare(image: DynamicImage) -> DynamicImage {
    let gray = DynamicImage::ImageLuma8(image.into_luma8());
    let side = gray.width().max(gray.height());
    if side > MAX_SIDE {
        gray.resize(MAX_SIDE, MAX_SIDE, FilterType::Lanczos3)
    } else if side > 0 && side < MIN_SIDE {
        let factor = MIN_SIDE.div_ceil(side).min(4);
        gray.resize(side * factor, side * factor, FilterType::CatmullRom)
    } else {
        gray
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_images_grow_large_ones_shrink_and_proportions_stay() {
        let small = prepare(DynamicImage::new_rgb8(400, 300));
        assert_eq!((small.width(), small.height()), (1600, 1200));
        let large = prepare(DynamicImage::new_rgb8(8000, 400));
        assert_eq!((large.width(), large.height()), (6000, 300));
        let normal = prepare(DynamicImage::new_rgb8(1600, 2000));
        assert_eq!((normal.width(), normal.height()), (1600, 2000));
        assert!(matches!(normal, DynamicImage::ImageLuma8(_)));
    }

    #[test]
    fn truncated_jpeg_is_detected() {
        let complete = [0xFF, 0xD8, 0xFF, 0xDA, 0x12, 0xFF, 0x00, 0x34, 0xFF, 0xD9];
        assert!(jpeg_is_complete(&complete));
        assert!(!jpeg_is_complete(&complete[..8]));
        // Una miniatura EXIF con su propio fin antes del barrido principal no cuenta.
        let thumb_then_cut = [0xFF, 0xD8, 0xFF, 0xD9, 0xFF, 0xDA, 0x12, 0x34];
        assert!(!jpeg_is_complete(&thumb_then_cut));
        assert!(!jpeg_is_complete(b"no es jpeg"));
    }
}
