//! Calibración del OCR con facturas sintéticas (D65–D67). Genera facturas inventadas, las
//! convierte en escaneos o fotos degradados (baja resolución, ruido, inclinación, desenfoque,
//! JPEG, foto de móvil con orientación EXIF) y las pasa por el extractor con el Tesseract
//! empaquetado en `vendor/tesseract` (el mismo que copia el instalador, D38). Informa de la
//! confianza media, los campos encontrados y el tiempo por condición, y cómo se comporta cada
//! umbral de confianza y de caracteres. Solo usa datos sintéticos; no guarda nada en el
//! repositorio.
//!
//! ```text
//! powershell -File scripts/setup-ocr.ps1
//! cargo run --release -p nestadmin-extract --example calibrar_ocr
//! ```
//!
//! Opcional: `-- --imagen <ruta>` mide además una imagen local y solo imprime métricas
//! agregadas (confianza, caracteres, tiempo y si aparecen campos con forma de número de
//! factura, fecha, importe y NIF), nunca su texto.

use std::f64::consts::PI;
use std::path::{Path, PathBuf};
use std::time::Instant;

use image::codecs::jpeg::JpegEncoder;
use image::{GrayImage, ImageEncoder, Luma};
use nestadmin_extract::{
    Extraction, Extractor, Method, Tesseract, DEFAULT_MIN_CHARS, DEFAULT_MIN_CONFIDENCE,
};
use pdfium_render::prelude::*;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Generador determinista (xorshift64*): las mismas facturas y el mismo ruido en cada
/// ejecución.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    fn gauss(&mut self) -> f64 {
        let u = self.unit().max(1e-12);
        let v = self.unit();
        (-2.0 * u.ln()).sqrt() * (2.0 * PI * v).cos()
    }
}

/// Importe en formato español: 1.234,56.
fn eur(cents: u64) -> String {
    let units = (cents / 100).to_string();
    let mut grouped = String::new();
    for (i, c) in units.chars().enumerate() {
        if i > 0 && (units.len() - i).is_multiple_of(3) {
            grouped.push('.');
        }
        grouped.push(c);
    }
    format!("{grouped},{:02}", cents % 100)
}

struct Invoice {
    lines: Vec<String>,
    /// Valores que el OCR debe recuperar: número, proveedor, fecha, base, IVA y total.
    fields: Vec<String>,
    font: usize,
    size: f32,
}

const SUPPLIERS: [&str; 8] = [
    "Alba Solar SL",
    "Talleres Brisa SA",
    "Papelería Ondina SL",
    "Limpiezas Cierzo SCoop",
    "Frutas Valdemora SL",
    "Copistería Mirador SL",
    "Fontanería Arroyo Claro SL",
    "Mensajería Pinar SA",
];
const CONCEPTS: [&str; 6] = [
    "Mantenimiento mensual de instalación",
    "Material de oficina variado",
    "Servicio de limpieza de portal",
    "Reparación de bomba de agua",
    "Envío urgente de documentación",
    "Revisión anual de extintores",
];

fn invoice(rng: &mut Rng, index: usize) -> Invoice {
    let number = format!("F-2026-{:04}", 100 + rng.below(9000));
    let supplier = SUPPLIERS[index % SUPPLIERS.len()].to_owned();
    let date = format!("{:02}/{:02}/2026", 1 + rng.below(28), 1 + rng.below(12));
    let mut base = 0;
    let mut concept_lines = Vec::new();
    for _ in 0..(2 + rng.below(3)) {
        let amount = 1_000 + rng.below(250_000);
        base += amount;
        let concept = CONCEPTS[rng.below(CONCEPTS.len() as u64) as usize];
        concept_lines.push(format!("{concept}    {} EUR", eur(amount)));
    }
    let vat = (base * 21 + 50) / 100;
    let total = base + vat;
    let mut lines = vec![
        supplier.clone(),
        "Calle Inventada 12, 28000 Ciudad Ficticia".to_owned(),
        String::new(),
        format!("FACTURA N.º {number}"),
        format!("Fecha: {date}"),
        "Cliente: Comunidad de Propietarios Ejemplo".to_owned(),
        String::new(),
        "Concepto                                   Importe".to_owned(),
    ];
    lines.extend(concept_lines);
    lines.extend([
        String::new(),
        format!("Base imponible: {} EUR", eur(base)),
        format!("IVA 21 %: {} EUR", eur(vat)),
        format!("TOTAL: {} EUR", eur(total)),
        String::new(),
        "Forma de pago: transferencia a 30 días".to_owned(),
    ]);
    Invoice {
        lines,
        fields: vec![number, supplier, date, eur(base), eur(vat), eur(total)],
        font: index % 3,
        size: [10.0, 11.0, 12.0, 14.0][rng.below(4) as usize],
    }
}

fn text_page(document: &mut PdfDocument<'_>, invoice: &Invoice) {
    let font = match invoice.font {
        0 => document.fonts_mut().helvetica(),
        1 => document.fonts_mut().times_roman(),
        _ => document.fonts_mut().courier(),
    };
    let mut page = document
        .pages_mut()
        .create_page_at_end(PdfPagePaperSize::a4())
        .unwrap();
    let step = invoice.size * 1.6;
    for (i, line) in invoice.lines.iter().enumerate() {
        if line.is_empty() {
            continue;
        }
        page.objects_mut()
            .create_text_object(
                PdfPoints::new(64.0),
                PdfPoints::new(780.0 - step * i as f32),
                line,
                font,
                PdfPoints::new(invoice.size),
            )
            .unwrap();
    }
}

/// Página de factura rasterizada en gris a `dpi`.
fn raster(pdfium: &Pdfium, invoice: &Invoice, dpi: f32) -> GrayImage {
    let mut document = pdfium.create_new_pdf().unwrap();
    text_page(&mut document, invoice);
    document
        .pages()
        .first()
        .unwrap()
        .render_with_config(&PdfRenderConfig::new().scale_page_by_factor(dpi / 72.0))
        .unwrap()
        .as_image()
        .unwrap()
        .into_luma8()
}

fn add_noise(image: &mut GrayImage, rng: &mut Rng, sigma: f64) {
    for pixel in image.pixels_mut() {
        let value = pixel.0[0] as f64 + rng.gauss() * sigma;
        pixel.0[0] = value.clamp(0.0, 255.0) as u8;
    }
}

/// Rotación bilineal alrededor del centro; fondo `fill`.
fn rotate(image: &GrayImage, degrees: f64, fill: u8) -> GrayImage {
    let (w, h) = image.dimensions();
    let (sin, cos) = (degrees * PI / 180.0).sin_cos();
    let (cx, cy) = (w as f64 / 2.0, h as f64 / 2.0);
    GrayImage::from_fn(w, h, |x, y| {
        let dx = x as f64 - cx;
        let dy = y as f64 - cy;
        let sx = cos * dx + sin * dy + cx;
        let sy = -sin * dx + cos * dy + cy;
        if sx < 0.0 || sy < 0.0 || sx >= (w - 1) as f64 || sy >= (h - 1) as f64 {
            return Luma([fill]);
        }
        let (x0, y0) = (sx.floor() as u32, sy.floor() as u32);
        let (fx, fy) = (sx - x0 as f64, sy - y0 as f64);
        let p = |x, y| image.get_pixel(x, y).0[0] as f64;
        let top = p(x0, y0) * (1.0 - fx) + p(x0 + 1, y0) * fx;
        let bottom = p(x0, y0 + 1) * (1.0 - fx) + p(x0 + 1, y0 + 1) * fx;
        Luma([(top * (1.0 - fy) + bottom * fy).round() as u8])
    })
}

fn scale(image: &GrayImage, factor: f64) -> GrayImage {
    let (w, h) = image.dimensions();
    image::imageops::resize(
        image,
        ((w as f64 * factor) as u32).max(1),
        ((h as f64 * factor) as u32).max(1),
        image::imageops::FilterType::Triangle,
    )
}

/// EXIF mínimo (TIFF little-endian) con solo la etiqueta de orientación.
fn exif_orientation(value: u16) -> Vec<u8> {
    let mut exif = vec![0x49, 0x49, 0x2A, 0x00, 0x08, 0x00, 0x00, 0x00, 0x01, 0x00];
    exif.extend([0x12, 0x01, 0x03, 0x00, 0x01, 0x00, 0x00, 0x00]);
    exif.extend(value.to_le_bytes());
    exif.extend([0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
    exif
}

fn save_jpeg(image: &GrayImage, path: &Path, quality: u8, orientation: Option<u16>) {
    let mut bytes = Vec::new();
    let mut encoder = JpegEncoder::new_with_quality(&mut bytes, quality);
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

/// Foto de móvil simulada: página algo inclinada sobre una mesa oscura, iluminación desigual,
/// desenfoque, ruido de sensor y JPEG. El sensor guarda la imagen girada y la orientación va
/// en EXIF (6 = girar 90° a la derecha para verla), como hacen los móviles en vertical.
fn phone_photo(page: &GrayImage, rng: &mut Rng, long_side: u32, quality: u8) -> (GrayImage, u8) {
    let (canvas_w, canvas_h) = (long_side * 3 / 4, long_side);
    let fit = (canvas_h as f64 * 0.86) / page.height() as f64;
    let page = scale(page, fit);
    let mut canvas = GrayImage::from_pixel(canvas_w, canvas_h, Luma([70]));
    let ox = (canvas_w - page.width().min(canvas_w)) / 2;
    let oy = (canvas_h - page.height().min(canvas_h)) / 2;
    image::imageops::overlay(&mut canvas, &page, ox as i64, oy as i64);
    let mut canvas = rotate(&canvas, 2.5, 70);
    let (w, h) = canvas.dimensions();
    for (x, y, pixel) in canvas.enumerate_pixels_mut() {
        let light = 0.62 + 0.38 * (x as f64 / w as f64) * (1.0 - 0.3 * y as f64 / h as f64);
        pixel.0[0] = (pixel.0[0] as f64 * light) as u8;
    }
    let mut canvas = image::imageops::blur(&canvas, 1.1);
    add_noise(&mut canvas, rng, 7.0);
    // El sensor está en horizontal: los píxeles se guardan girados 90° a la izquierda.
    (image::imageops::rotate270(&canvas), quality)
}

struct Sample {
    condition: &'static str,
    confidence: Option<f32>,
    fields_found: usize,
    fields_total: usize,
    seconds: f64,
}

fn squash(text: &str) -> String {
    text.chars().filter(|c| !c.is_whitespace()).collect()
}

fn measure(
    extractor: &Extractor,
    path: &Path,
    invoice: &Invoice,
    condition: &'static str,
) -> Sample {
    let start = Instant::now();
    let result = extractor.extract(path);
    let seconds = start.elapsed().as_secs_f64();
    let (text, confidence) = match result {
        Ok(Extraction { text, pages }) => (text, pages[0].confidence),
        Err(_) => (String::new(), None),
    };
    let haystack = squash(&text);
    let fields_found = invoice
        .fields
        .iter()
        .filter(|f| haystack.contains(&squash(f)))
        .count();
    Sample {
        condition,
        confidence,
        fields_found,
        fields_total: invoice.fields.len(),
        seconds,
    }
}

/// Condición de la imagen y cómo construirla a partir de la página a 300 ppp.
type Build = fn(&GrayImage, &mut Rng, &Path) -> PathBuf;

fn png(image: &GrayImage, dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(format!("{name}.png"));
    image.save(&path).unwrap();
    path
}

const CONDITIONS: [(&str, Build); 14] = [
    ("limpia 300 ppp", |p, _, d| png(p, d, "a")),
    ("200 ppp", |p, _, d| png(&scale(p, 200.0 / 300.0), d, "b")),
    ("150 ppp", |p, _, d| png(&scale(p, 0.5), d, "c")),
    ("100 ppp", |p, _, d| png(&scale(p, 1.0 / 3.0), d, "d")),
    ("75 ppp", |p, _, d| png(&scale(p, 0.25), d, "e")),
    ("ruido σ=20", |p, r, d| {
        let mut i = p.clone();
        add_noise(&mut i, r, 20.0);
        png(&i, d, "f")
    }),
    ("ruido σ=45", |p, r, d| {
        let mut i = p.clone();
        add_noise(&mut i, r, 45.0);
        png(&i, d, "g")
    }),
    ("giro 1,5°", |p, _, d| png(&rotate(p, 1.5, 255), d, "h")),
    ("giro 4°", |p, _, d| png(&rotate(p, 4.0, 255), d, "i")),
    ("desenfoque + JPEG q35", |p, _, d| {
        let path = d.join("j.jpg");
        save_jpeg(&image::imageops::blur(p, 1.6), &path, 35, None);
        path
    }),
    ("150 ppp + ruido + giro 2°", |p, r, d| {
        let mut i = rotate(&scale(p, 0.5), 2.0, 255);
        add_noise(&mut i, r, 25.0);
        png(&i, d, "k")
    }),
    ("móvil 12 Mpx, EXIF 6", |p, r, d| {
        let (photo, q) = phone_photo(p, r, 4032, 88);
        let path = d.join("l.jpg");
        save_jpeg(&photo, &path, q, Some(6));
        path
    }),
    ("móvil 2 Mpx, EXIF 6, q60", |p, r, d| {
        let (photo, q) = phone_photo(p, r, 1600, 60);
        let path = d.join("m.jpg");
        save_jpeg(&photo, &path, q, Some(6));
        path
    }),
    ("móvil 12 Mpx sin EXIF (girada)", |p, r, d| {
        let (photo, q) = phone_photo(p, r, 4032, 88);
        let path = d.join("n.jpg");
        save_jpeg(&photo, &path, q, None);
        path
    }),
];

fn pdfium() -> Pdfium {
    let dir = root().join("vendor/pdfium/bin");
    match Pdfium::bind_to_library(Pdfium::pdfium_platform_library_name_at_path(&dir)) {
        Ok(bindings) => Pdfium::new(bindings),
        Err(PdfiumError::PdfiumLibraryBindingsAlreadyInitialized) => Pdfium::default(),
        Err(e) => panic!("PDFium no disponible; ejecute scripts/setup-ocr.ps1 ({e})"),
    }
}

fn mean(values: impl Iterator<Item = f64>) -> f64 {
    let (sum, n) = values.fold((0.0, 0), |(s, n), v| (s + v, n + 1));
    if n == 0 {
        f64::NAN
    } else {
        sum / n as f64
    }
}

/// Métricas agregadas de una imagen local: nunca imprime su texto.
fn measure_local(extractor: &Extractor, path: &Path) {
    println!("\n## Imagen local (solo métricas agregadas)\n");
    if let Ok(mut decoder) = image::ImageReader::open(path)
        .and_then(|r| r.with_guessed_format())
        .map_err(image::ImageError::IoError)
        .and_then(|r| r.into_decoder())
    {
        use image::ImageDecoder;
        let original = decoder.dimensions();
        let orientation = decoder.orientation().ok();
        let prepared = nestadmin_extract::photo::prepare(path).map(|i| i.dimensions());
        println!(
            "- Archivo: {} KB; píxeles {}×{}; orientación EXIF {:?}; al OCR {:?}",
            std::fs::metadata(path).map_or(0, |m| m.len() / 1024),
            original.0,
            original.1,
            orientation,
            prepared.ok()
        );
    }
    // Confianza con la imagen girada otros 90°, 180° y 270°: indica si la orientación
    // elegida es la buena.
    if let Ok(upright) = nestadmin_extract::photo::prepare(path) {
        let dir = tempfile::tempdir().unwrap();
        for (degrees, image) in [
            (90, image::imageops::rotate90(&upright)),
            (180, image::imageops::rotate180(&upright)),
            (270, image::imageops::rotate270(&upright)),
        ] {
            let png = dir.path().join(format!("giro-{degrees}.png"));
            image.save(&png).unwrap();
            match extractor.ocr().recognize_page(&png, 1) {
                Ok(r) => println!(
                    "- Girada {degrees}° más: confianza {:?}; caracteres {}; {}",
                    r.confidence,
                    nestadmin_extract::visible_chars(&r.text),
                    field_shapes(&r.text)
                ),
                Err(e) => println!("- Girada {degrees}° más: {}", e.code()),
            }
        }
    }
    let start = Instant::now();
    let result = extractor.extract(path);
    let seconds = start.elapsed().as_secs_f64();
    match result {
        Err(e) => println!("- Error: {} ({})", e.code(), seconds),
        Ok(extraction) => {
            let page = &extraction.pages[0];
            println!(
                "- Tiempo: {seconds:.1} s; caracteres: {}; confianza media: {}; baja confianza: {}",
                page.chars,
                page.confidence.map_or("—".into(), |c| format!("{c:.1}")),
                if page.low_confidence { "sí" } else { "no" },
            );
            println!("- {}", field_shapes(&extraction.text));
        }
    }
}

/// Si el texto contiene campos con forma de factura, sin mostrar ningún valor.
fn field_shapes(text: &str) -> String {
    let text = text.to_lowercase();
    let has = |pred: &dyn Fn(&str) -> bool| text.split_whitespace().any(pred);
    let date = has(&|w| {
        let parts: Vec<&str> = w
            .trim_matches(|c: char| !c.is_ascii_digit())
            .split(['/', '-', '.'])
            .collect();
        parts.len() == 3
            && parts
                .iter()
                .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
    });
    let amount = has(&|w| {
        let w = w.trim_matches(|c: char| !c.is_ascii_digit());
        w.len() >= 4
            && w.chars()
                .all(|c| c.is_ascii_digit() || c == ',' || c == '.')
            && w.chars().rev().nth(2).is_some_and(|c| c == ',' || c == '.')
    });
    let nif = has(&|w| {
        let w: String = w.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
        w.len() == 9
            && w.chars().filter(|c| c.is_ascii_digit()).count() >= 7
            && w.chars().any(|c| c.is_ascii_alphabetic())
    });
    format!(
        "palabra «factura»: {}; «total»: {}; fecha: {}; importe con decimales: {}; NIF/CIF: {}",
        yes(text.contains("factura")),
        yes(text.contains("total")),
        yes(date),
        yes(amount),
        yes(nif)
    )
}

fn yes(value: bool) -> &'static str {
    if value {
        "sí"
    } else {
        "no"
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let local = args
        .iter()
        .position(|a| a == "--imagen")
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from);
    let count: usize = args
        .iter()
        .position(|a| a == "--facturas")
        .and_then(|i| args.get(i + 1))
        .and_then(|n| n.parse().ok())
        .unwrap_or(8);

    let packaged = Tesseract::packaged_executable(&root().join("vendor"));
    assert!(
        packaged.is_file(),
        "falta {}; ejecute scripts/setup-ocr.ps1",
        packaged.display()
    );
    let extractor = Extractor::new(
        root().join("vendor/pdfium/bin"),
        Box::new(Tesseract::new(&packaged, root().join("vendor/tessdata"))),
    );
    println!("Tesseract: {}", packaged.display());

    if let Some(path) = local {
        measure_local(&extractor, &path);
        return;
    }

    let pdfium = pdfium();
    let dir = tempfile::tempdir().unwrap();
    let mut rng = Rng(0x00C0_FFEE_2026_1105);
    let invoices: Vec<Invoice> = (0..count).map(|i| invoice(&mut rng, i)).collect();

    // Umbral de caracteres (D65): páginas con capa de texto frente a sellos y marcas de
    // agua que algunos escáneres añaden a una página escaneada.
    println!("\n## Caracteres visibles de la capa de texto\n");
    let mut text_chars = Vec::new();
    for (i, inv) in invoices.iter().enumerate() {
        let mut document = pdfium.create_new_pdf().unwrap();
        text_page(&mut document, inv);
        let path = dir.path().join(format!("texto-{i}.pdf"));
        document.save_to_file(&path).unwrap();
        let extraction = extractor.extract(&path).unwrap();
        text_chars.push(extraction.pages[0].chars);
    }
    println!(
        "- Facturas con texto: mínimo {} y máximo {} caracteres",
        text_chars.iter().min().unwrap(),
        text_chars.iter().max().unwrap()
    );
    let stamps = [
        "Página 1 de 1",
        "Escaneado el 05/11/2026 09:14",
        "Digitalizado con EscanerMovil Gratis",
        "Documento escaneado - copia auténtica - pág. 1/1",
    ];
    let scanned = raster(&pdfium, &invoices[0], 200.0);
    for stamp in stamps {
        let mut document = pdfium.create_new_pdf().unwrap();
        let font = document.fonts_mut().helvetica();
        let mut page = document
            .pages_mut()
            .create_page_at_end(PdfPagePaperSize::a4())
            .unwrap();
        let picture = image::DynamicImage::ImageLuma8(scanned.clone()).into_rgb8();
        let width = page.width();
        page.objects_mut()
            .create_image_object(
                PdfPoints::ZERO,
                PdfPoints::ZERO,
                &image::DynamicImage::ImageRgb8(picture),
                Some(width),
                None,
            )
            .unwrap();
        page.objects_mut()
            .create_text_object(
                PdfPoints::new(40.0),
                PdfPoints::new(20.0),
                stamp,
                font,
                PdfPoints::new(7.0),
            )
            .unwrap();
        let path = dir.path().join("sello.pdf");
        document.save_to_file(&path).unwrap();
        let chars = nestadmin_extract::visible_chars(stamp);
        let with_20 = Extractor::new(
            root().join("vendor/pdfium/bin"),
            Box::new(Tesseract::new(&packaged, root().join("vendor/tessdata"))),
        )
        .with_min_chars(20)
        .extract(&path)
        .unwrap();
        let with_default = extractor.extract(&path).unwrap();
        let method = |e: &Extraction| match e.pages[0].method {
            Method::Ocr => "OCR",
            _ => "capa de texto",
        };
        println!(
            "- Escaneada con sello de {chars} caracteres: umbral 20 → {}; umbral {DEFAULT_MIN_CHARS} → {}",
            method(&with_20),
            method(&with_default)
        );
    }

    println!("\n## OCR por condición ({count} facturas sintéticas por condición)\n");
    println!("| Condición | Confianza media | Mínima | Campos encontrados | Facturas completas | Tiempo medio |");
    println!("| --- | --- | --- | --- | --- | --- |");
    let mut samples = Vec::new();
    for (name, build) in CONDITIONS {
        let mut rows = Vec::new();
        for inv in &invoices {
            let page = raster(&pdfium, inv, 300.0);
            let path = build(&page, &mut rng, dir.path());
            rows.push(measure(&extractor, &path, inv, name));
        }
        let confidences: Vec<f64> = rows
            .iter()
            .map(|s| s.confidence.unwrap_or(0.0) as f64)
            .collect();
        let found: usize = rows.iter().map(|s| s.fields_found).sum();
        let total: usize = rows.iter().map(|s| s.fields_total).sum();
        let complete = rows
            .iter()
            .filter(|s| s.fields_found == s.fields_total)
            .count();
        println!(
            "| {name} | {:.1} | {:.1} | {found}/{total} ({:.0} %) | {complete}/{} | {:.1} s |",
            mean(confidences.iter().copied()),
            confidences.iter().copied().fold(f64::INFINITY, f64::min),
            100.0 * found as f64 / total as f64,
            rows.len(),
            mean(rows.iter().map(|s| s.seconds)),
        );
        samples.extend(rows);
    }

    println!("\n## Umbral de confianza\n");
    println!("Una página es «incompleta» si le falta al menos un campo de los seis.\n");
    println!("| Umbral | Marcadas | Incompletas marcadas | Incompletas sin marcar | Completas marcadas |");
    println!("| --- | --- | --- | --- | --- |");
    let incomplete = |s: &Sample| s.fields_found < s.fields_total;
    let all_incomplete = samples.iter().filter(|s| incomplete(s)).count();
    for threshold in [50.0, 60.0, 70.0, 75.0, 80.0, 85.0, 90.0] {
        let flagged = |s: &Sample| s.confidence.unwrap_or(0.0) < threshold;
        let marked = samples.iter().filter(|s| flagged(s)).count();
        let caught = samples
            .iter()
            .filter(|s| flagged(s) && incomplete(s))
            .count();
        let false_alarm = samples
            .iter()
            .filter(|s| flagged(s) && !incomplete(s))
            .count();
        println!(
            "| {threshold:.0} | {marked}/{} | {caught}/{all_incomplete} | {} | {false_alarm} |",
            samples.len(),
            all_incomplete - caught,
        );
    }
    println!("\nIncompletas sin marcar con el umbral por defecto ({DEFAULT_MIN_CONFIDENCE:.0}):\n");
    for s in samples
        .iter()
        .filter(|s| incomplete(s) && s.confidence.unwrap_or(0.0) >= DEFAULT_MIN_CONFIDENCE)
    {
        println!(
            "- {}: confianza {:.1}, {} de {} campos",
            s.condition,
            s.confidence.unwrap_or(0.0),
            s.fields_found,
            s.fields_total
        );
    }
}
