//! PDF de los documentos de venta emitidos (E4a, D76, D222, D223): se dibuja con PDFium a partir
//! de los datos copiados al emitir, sin tocar la base ni la red.
//!
//! * Una sola hoja A4 vertical por página, con Helvetica. Los textos se miden con la propia
//!   PDFium (anchura de cada carácter a 1000 pt), de modo que el ajuste de línea y la alineación
//!   a la derecha no dependen de una tabla escrita a mano.
//! * Con membrete (D222), la primera página del membrete es el fondo de la primera hoja y la
//!   segunda (o la primera, si solo hay una) el de las siguientes; el contenido se coloca dentro
//!   del área útil que definen los márgenes y no se dibuja el logotipo.
//! * En las facturas, el QR de cotejo y la leyenda «VERI*FACTU» (D83) van en lo alto de la primera
//!   hoja, sobre un recuadro blanco que se dibuja encima del membrete para que se pueda leer.
//! * Determinista: el contenido solo depende de los datos de entrada, sin fechas ni azar. Lo que
//!   PDFium escribe por su cuenta (identificador del archivo, fechas de creación) no lo controla
//!   esta capa; ver D226.

use std::collections::HashMap;

use nestadmin_core::sales::{LetterheadMargins, SalesDocument};
use nestadmin_documents::IsoDate;
use nestadmin_extract::image::{self, DynamicImage, GrayImage, Luma};
use nestadmin_extract::pdfium_render::prelude::*;
use nestadmin_sales::{
    DocumentKind, RectificationKind, RectificationMethod, TaxQualification, Totals,
};
use nestadmin_verifactu::qr::{LADO_MM, LEYENDA_VERIFACTU, TEXTO_SUPERIOR};
use nestadmin_verifactu::CodigoQr;
use rust_decimal::{Decimal, RoundingStrategy};

/// Tamaño de una hoja A4 vertical, en puntos.
const PAGE_W: f32 = 595.276;
const PAGE_H: f32 = 841.89;
/// Tolerancia al comprobar que una página del membrete es A4 (puntos).
const A4_TOLERANCE: f32 = 3.0;
const MM: f32 = 72.0 / 25.4;
/// Márgenes sin membrete (mm).
const DEFAULT_MARGIN_MM: f32 = 18.0;
const DEFAULT_TOP_MM: f32 = 15.0;
/// Espacio reservado al pie de cada página para «Página n de m».
const FOOTER_H: f32 = 18.0;
const LINE_H: f32 = 11.5;
/// Píxeles por módulo del QR en el PDF.
const QR_PX_PER_MODULE: u32 = 10;

/// Error de la generación: el motivo, listo para mostrarse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PdfError(pub String);

impl std::fmt::Display for PdfError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

fn pdf_err(context: &str, e: impl std::fmt::Display) -> PdfError {
    PdfError(format!("{context}: {e}"))
}

/// Datos de entrada del PDF de un documento emitido.
pub struct PdfSource<'a> {
    pub document: &'a SalesDocument,
    /// Número de la factura rectificada, en una rectificativa.
    pub rectified_number: Option<String>,
    /// Logotipo (PNG o JPEG) citado por el documento; no se usa con membrete.
    pub logo: Option<Vec<u8>>,
    /// PDF del membrete citado por el documento.
    pub letterhead: Option<Vec<u8>>,
    /// QR de cotejo de la factura, construido desde su registro de alta.
    pub qr: Option<CodigoQr>,
}

// ---------------------------------------------------------------------------------------------
// Membrete: comprobación al abrirlo
// ---------------------------------------------------------------------------------------------

/// Comprueba con PDFium que el PDF sirve de membrete: se abre sin contraseña, tiene una o dos
/// páginas y todas son A4 vertical sin girar. Devuelve el número de páginas o el motivo del
/// rechazo, en español y listo para mostrar.
pub fn check_letterhead(pdfium: &Pdfium, bytes: &[u8]) -> Result<u16, String> {
    if bytes.is_empty() {
        return Err("el archivo está vacío".into());
    }
    if bytes.len() > nestadmin_core::sales::MAX_LETTERHEAD_BYTES {
        return Err("el PDF supera los 5 MB".into());
    }
    let document = pdfium
        .load_pdf_from_byte_slice(bytes, None)
        .map_err(|e| match e {
            PdfiumError::PdfiumLibraryInternalError(PdfiumInternalError::PasswordError)
            | PdfiumError::PdfiumLibraryInternalError(PdfiumInternalError::SecurityError) => {
                "el PDF está protegido con contraseña".to_owned()
            }
            _ => "el archivo no es un PDF válido o está dañado".to_owned(),
        })?;
    let pages = document.pages().len();
    if !(1..=2).contains(&pages) {
        return Err(format!(
            "el membrete debe tener una o dos páginas y este tiene {pages}"
        ));
    }
    for (index, page) in document.pages().iter().enumerate() {
        let (w, h) = (page.width().value, page.height().value);
        let rotated = !matches!(page.rotation(), Ok(PdfPageRenderRotation::None));
        if rotated || (w - PAGE_W).abs() > A4_TOLERANCE || (h - PAGE_H).abs() > A4_TOLERANCE {
            return Err(format!(
                "todas las páginas deben ser A4 vertical (210 x 297 mm); la página {} mide \
                 {:.0} x {:.0} mm{}",
                index + 1,
                w / MM,
                h / MM,
                if rotated { " y está girada" } else { "" }
            ));
        }
    }
    Ok(pages as u16)
}

// ---------------------------------------------------------------------------------------------
// Formato
// ---------------------------------------------------------------------------------------------

/// `1234.5` -> `1.234,50` (miles con punto, decimales con coma), entre `min` y `max` decimales.
fn fmt_decimal(value: Decimal, min: u32, max: u32) -> String {
    let mut rounded = value.round_dp_with_strategy(max, RoundingStrategy::MidpointAwayFromZero);
    rounded.rescale(max);
    let text = rounded.abs().to_string();
    let (int, frac) = text.split_once('.').unwrap_or((&text, ""));
    let mut frac = frac.to_owned();
    while frac.len() > min as usize && frac.ends_with('0') {
        frac.pop();
    }
    let mut grouped = String::new();
    for (i, ch) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            grouped.push('.');
        }
        grouped.push(ch);
    }
    let sign = if rounded.is_sign_negative() && !rounded.is_zero() {
        "-"
    } else {
        ""
    };
    if frac.is_empty() {
        format!("{sign}{grouped}")
    } else {
        format!("{sign}{grouped},{frac}")
    }
}

fn money(value: Decimal) -> String {
    format!("{} €", fmt_decimal(value, 2, 2))
}

fn fmt_date(date: IsoDate) -> String {
    format!("{:02}/{:02}/{:04}", date.day, date.month, date.year)
}

fn fmt_iban(iban: &str) -> String {
    let compact: String = iban.chars().filter(|c| !c.is_whitespace()).collect();
    compact
        .as_bytes()
        .chunks(4)
        .map(|c| String::from_utf8_lossy(c).into_owned())
        .collect::<Vec<_>>()
        .join(" ")
}

fn title(kind: DocumentKind) -> &'static str {
    match kind {
        DocumentKind::Quote => "PRESUPUESTO",
        DocumentKind::DeliveryNote => "ALBARÁN",
        DocumentKind::Invoice => "FACTURA",
        DocumentKind::SimplifiedInvoice => "FACTURA SIMPLIFICADA",
        DocumentKind::CorrectiveInvoice => "FACTURA RECTIFICATIVA",
    }
}

fn qualification_label(q: TaxQualification) -> Option<&'static str> {
    match q {
        TaxQualification::S1 => None,
        TaxQualification::S2 => Some("Inversión del sujeto pasivo"),
        TaxQualification::N1 => Some("No sujeta (arts. 7 y 14)"),
        TaxQualification::N2 => Some("No sujeta por reglas de localización"),
        TaxQualification::E1 => Some("Exenta (art. 20)"),
        TaxQualification::E2 => Some("Exenta (art. 21)"),
        TaxQualification::E3 => Some("Exenta (art. 22)"),
        TaxQualification::E4 => Some("Exenta (arts. 23 y 24)"),
        TaxQualification::E5 => Some("Exenta (art. 25)"),
        TaxQualification::E6 => Some("Exenta por otros motivos"),
    }
}

fn rectification_label(kind: RectificationKind) -> &'static str {
    match kind {
        RectificationKind::R1 => "R1 (error fundado en derecho y art. 80 Uno, Dos y Seis)",
        RectificationKind::R2 => "R2 (art. 80 Tres)",
        RectificationKind::R3 => "R3 (art. 80 Cuatro)",
        RectificationKind::R4 => "R4 (resto de causas)",
        RectificationKind::R5 => "R5 (factura simplificada)",
    }
}

/// Solo los caracteres que la fuente estándar escribe bien (ASCII, Latin-1 y unos pocos signos de
/// WinAnsi); el resto se sustituye por `?` y los saltos de línea por espacios.
fn allowed(c: char) -> bool {
    (' '..='~').contains(&c)
        || ('\u{A0}'..='\u{FF}').contains(&c)
        || matches!(c, '€' | '–' | '—' | '‘' | '’' | '“' | '”' | '•' | '…' | '™')
}

fn clean_text(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_whitespace() {
                ' '
            } else if allowed(c) {
                c
            } else {
                '?'
            }
        })
        .collect::<String>()
        .split(' ')
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

// ---------------------------------------------------------------------------------------------
// Medida de texto
// ---------------------------------------------------------------------------------------------

/// Anchura de cada carácter por punto de cuerpo, medida con PDFium.
struct Widths {
    regular: HashMap<char, f32>,
    bold: HashMap<char, f32>,
}

impl Widths {
    fn measure(pdfium: &Pdfium) -> Result<Self, PdfError> {
        let mut document = pdfium
            .create_new_pdf()
            .map_err(|e| pdf_err("no se pudo crear el PDF", e))?;
        let regular_font = document.fonts_mut().helvetica();
        let bold_font = document.fonts_mut().helvetica_bold();
        let mut page = document
            .pages_mut()
            .create_page_at_end(PdfPagePaperSize::a4())
            .map_err(|e| pdf_err("no se pudo crear la página", e))?;
        let mut width_of = |font: PdfFontToken, text: &str| -> Result<f32, PdfError> {
            let object = page
                .objects_mut()
                .create_text_object(
                    PdfPoints::ZERO,
                    PdfPoints::ZERO,
                    text,
                    font,
                    PdfPoints::new(1000.0),
                )
                .map_err(|e| pdf_err("no se pudo medir el texto", e))?;
            Ok(object
                .width()
                .map_err(|e| pdf_err("no se pudo medir el texto", e))?
                .value)
        };
        let mut table = |font: PdfFontToken| -> Result<HashMap<char, f32>, PdfError> {
            // Se mide entre dos barras para que el espacio y los signos con poco trazo cuenten
            // por su avance y no por su tinta.
            let bars = width_of(font, "||")?;
            let mut map = HashMap::new();
            for c in (' '..='~')
                .chain('\u{A0}'..='\u{FF}')
                .chain(['€', '–', '—', '‘', '’', '“', '”', '•', '…', '™'])
            {
                let w = width_of(font, &format!("|{c}|"))? - bars;
                map.insert(c, w.max(0.0) / 1000.0);
            }
            Ok(map)
        };
        Ok(Self {
            regular: table(regular_font)?,
            bold: table(bold_font)?,
        })
    }

    fn of(&self, bold: bool, text: &str, size: f32) -> f32 {
        let table = if bold { &self.bold } else { &self.regular };
        text.chars()
            .map(|c| table.get(&c).copied().unwrap_or(0.5))
            .sum::<f32>()
            * size
    }

    /// Divide el texto en líneas de como mucho `width` puntos; una palabra más larga que la línea
    /// se parte por caracteres.
    fn wrap(&self, text: &str, width: f32, size: f32, bold: bool) -> Vec<String> {
        let text = clean_text(text);
        if text.is_empty() {
            return Vec::new();
        }
        let mut lines = Vec::new();
        let mut line = String::new();
        for word in text.split(' ') {
            let candidate = if line.is_empty() {
                word.to_owned()
            } else {
                format!("{line} {word}")
            };
            if self.of(bold, &candidate, size) <= width {
                line = candidate;
                continue;
            }
            if !line.is_empty() {
                lines.push(std::mem::take(&mut line));
            }
            let mut rest = word.to_owned();
            while self.of(bold, &rest, size) > width && rest.chars().count() > 1 {
                let mut cut = String::new();
                for ch in rest.chars() {
                    let mut next = cut.clone();
                    next.push(ch);
                    if self.of(bold, &next, size) > width && !cut.is_empty() {
                        break;
                    }
                    cut = next;
                }
                rest = rest[cut.len()..].to_owned();
                lines.push(cut);
            }
            line = rest;
        }
        if !line.is_empty() {
            lines.push(line);
        }
        lines
    }
}

// ---------------------------------------------------------------------------------------------
// Composición
// ---------------------------------------------------------------------------------------------

/// Orden de dibujo sobre una página; `y` se mide desde el borde superior.
enum Cmd {
    Text {
        x: f32,
        y: f32,
        size: f32,
        bold: bool,
        gray: u8,
        text: String,
    },
    Rect {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        gray: u8,
    },
    Image {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        image: usize,
    },
}

struct Layout<'w> {
    widths: &'w Widths,
    pages: Vec<Vec<Cmd>>,
    images: Vec<DynamicImage>,
    x0: f32,
    x1: f32,
    top: f32,
    /// Límite inferior del contenido (sin el pie).
    limit: f32,
    /// Borde inferior del área útil, donde va el pie de página.
    bottom: f32,
    y: f32,
    running_title: String,
}

impl<'w> Layout<'w> {
    fn width(&self) -> f32 {
        self.x1 - self.x0
    }

    fn page(&mut self) -> &mut Vec<Cmd> {
        self.pages.last_mut().expect("siempre hay una página")
    }

    fn text(&mut self, x: f32, y: f32, size: f32, bold: bool, gray: u8, text: &str) {
        let text = clean_text(text);
        if text.is_empty() {
            return;
        }
        self.page().push(Cmd::Text {
            x,
            y,
            size,
            bold,
            gray,
            text,
        });
    }

    fn text_right(&mut self, right: f32, y: f32, size: f32, bold: bool, gray: u8, text: &str) {
        let text = clean_text(text);
        let x = right - self.widths.of(bold, &text, size);
        self.text(x, y, size, bold, gray, &text);
    }

    fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, gray: u8) {
        self.page().push(Cmd::Rect { x, y, w, h, gray });
    }

    /// Empieza una página nueva (con un encabezado corto si no es la primera).
    fn new_page(&mut self) {
        self.pages.push(Vec::new());
        self.y = self.top;
        if self.pages.len() > 1 {
            let heading = format!("{} (continuación)", self.running_title);
            let (x, y) = (self.x0, self.y + 9.0);
            self.text(x, y, 9.0, true, 90, &heading);
            self.y += 20.0;
        }
    }

    /// Si no caben `height` puntos en la página actual, empieza otra. `true` si lo hizo.
    fn ensure(&mut self, height: f32) -> bool {
        if self.y + height > self.limit && self.y > self.top + 30.0 {
            self.new_page();
            true
        } else {
            false
        }
    }
}

struct Margins {
    left: f32,
    right: f32,
    top: f32,
    bottom: f32,
}

fn margins_of(letterhead: Option<&LetterheadMargins>) -> Margins {
    match letterhead {
        Some(m) => Margins {
            left: f32::from(m.left_mm) * MM,
            right: f32::from(m.right_mm) * MM,
            top: f32::from(m.top_mm) * MM,
            bottom: f32::from(m.bottom_mm) * MM,
        },
        None => Margins {
            left: DEFAULT_MARGIN_MM * MM,
            right: DEFAULT_MARGIN_MM * MM,
            top: DEFAULT_TOP_MM * MM,
            bottom: DEFAULT_MARGIN_MM * MM,
        },
    }
}

fn qr_image(qr: &CodigoQr) -> DynamicImage {
    let n = qr.ancho_modulos() as u32;
    let side = n * QR_PX_PER_MODULE;
    DynamicImage::ImageLuma8(GrayImage::from_fn(side, side, |x, y| {
        let dark = qr.modulo(
            (x / QR_PX_PER_MODULE) as usize,
            (y / QR_PX_PER_MODULE) as usize,
        );
        Luma([if dark { 0 } else { 255 }])
    }))
}

impl Layout<'_> {
    /// Escribe un texto ajustado a `width` desde `y` (borde superior del bloque) y devuelve el
    /// borde inferior.
    #[allow(clippy::too_many_arguments)]
    fn block(
        &mut self,
        x: f32,
        mut y: f32,
        width: f32,
        size: f32,
        bold: bool,
        gray: u8,
        text: &str,
    ) -> f32 {
        for line in self.widths.wrap(text, width, size, bold) {
            y += size * 1.25;
            self.text(x, y, size, bold, gray, &line);
        }
        y
    }
}

/// Columnas de la tabla de líneas: bordes derechos de cada una.
struct Columns {
    desc_w: f32,
    qty: f32,
    price: f32,
    discount: f32,
    tax: f32,
    amount: f32,
}

fn columns(l: &Layout<'_>) -> Columns {
    let w = l.width();
    Columns {
        desc_w: w * 0.43,
        qty: l.x0 + w * 0.52,
        price: l.x0 + w * 0.66,
        discount: l.x0 + w * 0.75,
        tax: l.x0 + w * 0.84,
        amount: l.x1,
    }
}

fn table_header(l: &mut Layout<'_>, c: &Columns) {
    let (x0, w, y) = (l.x0, l.width(), l.y);
    l.rect(x0, y, w, 16.0, 232);
    let base = y + 11.0;
    l.text(x0 + 4.0, base, 8.5, true, 0, "Descripción");
    l.text_right(c.qty - 4.0, base, 8.5, true, 0, "Cant.");
    l.text_right(c.price - 4.0, base, 8.5, true, 0, "Precio");
    l.text_right(c.discount - 4.0, base, 8.5, true, 0, "Dto.");
    l.text_right(c.tax - 4.0, base, 8.5, true, 0, "IVA");
    l.text_right(c.amount - 4.0, base, 8.5, true, 0, "Importe €");
    l.y += 16.0;
}

/// Compone todas las páginas del documento sobre el área útil que le corresponde.
fn compose<'w>(src: &PdfSource<'_>, widths: &'w Widths) -> Result<Layout<'w>, PdfError> {
    let doc = src.document;
    let totals: &Totals = doc
        .totals
        .as_ref()
        .ok_or_else(|| PdfError("el documento no tiene totales fijados".into()))?;
    let issuer = doc.issuer.as_ref();
    let with_letterhead = src.letterhead.is_some();
    let lm = with_letterhead.then(|| {
        issuer
            .and_then(|i| i.letterhead_margins)
            .unwrap_or_default()
    });
    let m = margins_of(lm.as_ref());
    let number = doc.number.clone().unwrap_or_default();
    let mut head = format!("{} Nº {number}", title(doc.kind));
    if doc.revision > 1 {
        head.push_str(&format!(" (revisión {})", doc.revision));
    }
    let mut l = Layout {
        widths,
        pages: Vec::new(),
        images: Vec::new(),
        x0: m.left,
        x1: PAGE_W - m.right,
        top: m.top,
        limit: PAGE_H - m.bottom - FOOTER_H,
        bottom: PAGE_H - m.bottom,
        y: 0.0,
        running_title: format!("{} {number}", title(doc.kind)),
    };
    l.new_page();

    // --- Cabecera: emisor (y logotipo sin membrete) a la izquierda, QR a la derecha ---
    let show_qr = doc.kind.is_invoice() && src.qr.is_some();
    let qr_pad = 3.0 * MM;
    let qr_side = LADO_MM * MM;
    let qr_w = qr_side + 2.0 * qr_pad;
    let qr_h = qr_pad + 12.0 + qr_side + 15.0 + qr_pad;
    let left_w = if show_qr {
        l.width() - qr_w - 14.0
    } else {
        l.width()
    };
    let top_y = l.y;
    let x0 = l.x0;
    let mut y = top_y;
    if !with_letterhead {
        if let Some(img) = src
            .logo
            .as_deref()
            .and_then(|bytes| image::load_from_memory(bytes).ok())
        {
            let (iw, ih) = (img.width() as f32, img.height() as f32);
            let scale = (left_w.min(180.0) / iw).min(56.0 / ih);
            let (w, h) = (iw * scale, ih * scale);
            l.images.push(img);
            let index = l.images.len() - 1;
            l.page().push(Cmd::Image {
                x: x0,
                y,
                w,
                h,
                image: index,
            });
            y += h + 6.0;
        }
    }
    if let Some(i) = issuer {
        if let Some(name) = &i.party.name {
            y = l.block(x0, y, left_w, 11.0, true, 0, name);
        }
        if let Some(nif) = &i.party.tax_id {
            y = l.block(x0, y, left_w, 9.0, false, 0, &format!("NIF: {nif}"));
        }
        if let Some(address) = &i.party.address {
            y = l.block(x0, y, left_w, 9.0, false, 0, address);
        }
        if let Some(email) = &i.email {
            y = l.block(x0, y, left_w, 9.0, false, 0, email);
        }
    }
    if let (true, Some(qr)) = (show_qr, src.qr.as_ref()) {
        let bx = l.x1 - qr_w;
        // Recuadro blanco encima del membrete: el QR y su leyenda siempre se leen.
        l.rect(bx, top_y, qr_w, qr_h, 255);
        let cx = bx + qr_w / 2.0;
        let label_x = cx - widths.of(false, TEXTO_SUPERIOR, 8.0) / 2.0;
        l.text(label_x, top_y + qr_pad + 8.0, 8.0, false, 0, TEXTO_SUPERIOR);
        let qy = top_y + qr_pad + 12.0;
        l.images.push(qr_image(qr));
        let index = l.images.len() - 1;
        l.page().push(Cmd::Image {
            x: bx + qr_pad,
            y: qy,
            w: qr_side,
            h: qr_side,
            image: index,
        });
        let legend_x = cx - widths.of(true, LEYENDA_VERIFACTU, 9.0) / 2.0;
        l.text(
            legend_x,
            qy + qr_side + 12.0,
            9.0,
            true,
            0,
            LEYENDA_VERIFACTU,
        );
        y = y.max(top_y + qr_h);
    }
    y += 14.0;
    l.text(x0, y + 15.0, 15.0, true, 0, &head);
    y += 28.0;

    // --- Cliente a la izquierda y datos del documento a la derecha ---
    let col_w = (l.width() - 20.0) / 2.0;
    let xr = x0 + col_w + 20.0;
    let mut yl = l.block(x0, y, col_w, 8.0, true, 110, "CLIENTE");
    match &doc.recipient {
        Some(p) => {
            if let Some(name) = &p.name {
                yl = l.block(x0, yl, col_w, 10.0, true, 0, name);
            }
            if let Some(nif) = &p.tax_id {
                yl = l.block(x0, yl, col_w, 9.0, false, 0, &format!("NIF: {nif}"));
            }
            if let Some(address) = &p.address {
                yl = l.block(x0, yl, col_w, 9.0, false, 0, address);
            }
        }
        None => {
            yl = l.block(
                x0,
                yl,
                col_w,
                9.0,
                false,
                0,
                "Sin destinatario identificado",
            )
        }
    }
    let mut yr = l.block(xr, y, col_w, 8.0, true, 110, "DATOS DEL DOCUMENTO");
    if let Some(date) = doc.issue_date {
        let text = format!("Fecha de expedición: {}", fmt_date(date));
        yr = l.block(xr, yr, col_w, 9.0, false, 0, &text);
    }
    if let Some(date) = doc.operation_date.filter(|d| Some(*d) != doc.issue_date) {
        let text = format!("Fecha de operación: {}", fmt_date(date));
        yr = l.block(xr, yr, col_w, 9.0, false, 0, &text);
    }
    if let Some(date) = doc.due_date {
        let text = format!("Vencimiento: {}", fmt_date(date));
        yr = l.block(xr, yr, col_w, 9.0, false, 0, &text);
    }
    if let Some(rectified) = &src.rectified_number {
        let text = format!("Rectifica a la factura: {rectified}");
        yr = l.block(xr, yr, col_w, 9.0, true, 0, &text);
    }
    if let Some(r) = &doc.rectification {
        let text = format!("Tipo de rectificación: {}", rectification_label(r.kind));
        yr = l.block(xr, yr, col_w, 9.0, false, 0, &text);
        let method = match r.method {
            RectificationMethod::Substitution => "por sustitución",
            RectificationMethod::Differences => "por diferencias",
        };
        yr = l.block(xr, yr, col_w, 9.0, false, 0, &format!("Método: {method}"));
        if let (RectificationMethod::Substitution, Some(base), Some(tax)) =
            (r.method, r.rectified_base, r.rectified_tax)
        {
            let text = format!(
                "Base rectificada: {}; cuota rectificada: {}",
                money(base),
                money(tax)
            );
            yr = l.block(xr, yr, col_w, 9.0, false, 0, &text);
        }
    }
    l.y = yl.max(yr) + 14.0;

    // --- Líneas ---
    let c = columns(&l);
    l.ensure(16.0 + 30.0);
    table_header(&mut l, &c);
    for (index, line) in doc.lines.iter().enumerate() {
        let input = &line.input;
        let desc = widths.wrap(&input.description, c.desc_w - 8.0, 9.0, false);
        let mut notes = Vec::new();
        if let Some(e) = &input.exemption {
            if let Some(label) = qualification_label(e.qualification) {
                notes.extend(widths.wrap(label, c.desc_w - 8.0, 8.0, false));
            }
            if let Some(text) = &e.description {
                notes.extend(widths.wrap(text, c.desc_w - 8.0, 8.0, false));
            }
        }
        let height = desc.len().max(1) as f32 * LINE_H + notes.len() as f32 * 10.0 + 6.0;
        if l.ensure(height) {
            table_header(&mut l, &c);
        }
        let top = l.y;
        let first = top + 3.0 + 9.0;
        let amount = line
            .amount
            .or_else(|| totals.line_amounts.get(index).copied())
            .unwrap_or_default();
        for (i, text) in desc.iter().enumerate() {
            l.text(x0 + 4.0, first + i as f32 * LINE_H, 9.0, false, 0, text);
        }
        for (i, text) in notes.iter().enumerate() {
            let y = first + desc.len().max(1) as f32 * LINE_H - 1.0 + i as f32 * 10.0;
            l.text(x0 + 4.0, y, 8.0, false, 100, text);
        }
        l.text_right(
            c.qty - 4.0,
            first,
            9.0,
            false,
            0,
            &fmt_decimal(input.quantity, 0, 3),
        );
        l.text_right(
            c.price - 4.0,
            first,
            9.0,
            false,
            0,
            &fmt_decimal(input.unit_price, 2, 4),
        );
        if !input.discount.is_zero() {
            let pct = format!(
                "{} %",
                fmt_decimal(input.discount * Decimal::from(100), 0, 2)
            );
            l.text_right(c.discount - 4.0, first, 9.0, false, 0, &pct);
        }
        let iva = format!("{} %", fmt_decimal(input.tax_rate.percent(), 0, 2));
        l.text_right(c.tax - 4.0, first, 9.0, false, 0, &iva);
        l.text_right(
            c.amount - 4.0,
            first,
            9.0,
            false,
            0,
            &fmt_decimal(amount, 2, 2),
        );
        l.y += height;
        let y = l.y;
        l.rect(x0, y, l.width(), 0.4, 215);
    }
    l.y += 12.0;

    // --- Desglose de IVA (izquierda) y totales (derecha) ---
    let w = l.width();
    let bw = w * 0.56;
    let (base_r, type_r, tax_r) = (x0 + bw * 0.38, x0 + bw * 0.62, x0 + bw);
    let mut breakdown_h = 16.0;
    let mut notes_per_row = Vec::new();
    for b in &totals.breakdown {
        let mut notes = Vec::new();
        if let Some(label) = b.qualification.and_then(qualification_label) {
            notes.push(label.to_owned());
        }
        breakdown_h += 12.0 + notes.len() as f32 * 9.5;
        notes_per_row.push(notes);
    }
    let has_withholding = !totals.withholding.is_zero();
    let mut total_rows = vec![
        ("Base imponible".to_owned(), money(totals.base_total), false),
        ("Cuotas de IVA".to_owned(), money(totals.tax_total), false),
    ];
    if has_withholding {
        let label = match doc.withholding_percent {
            Some(p) => format!("Retención ({} %)", fmt_decimal(p, 0, 2)),
            None => "Retención".to_owned(),
        };
        total_rows.push((label, format!("-{}", money(totals.withholding)), false));
    }
    total_rows.push(("TOTAL".to_owned(), money(totals.total), true));
    if has_withholding {
        total_rows.push(("Importe a pagar".to_owned(), money(totals.amount_due), true));
    }
    let totals_h = total_rows
        .iter()
        .map(|(_, _, bold)| if *bold { 16.0 } else { 13.0 })
        .sum::<f32>();
    if l.ensure(breakdown_h.max(totals_h) + 6.0) {
        // Página nueva: sin encabezado de tabla, solo el bloque de totales.
    }
    let by = l.y;
    l.text(x0, by + 9.0, 8.0, true, 110, "DESGLOSE DE IVA");
    l.text_right(base_r, by + 22.0, 8.5, true, 0, "Base");
    l.text_right(type_r, by + 22.0, 8.5, true, 0, "Tipo");
    l.text_right(tax_r, by + 22.0, 8.5, true, 0, "Cuota");
    let mut yy = by + 22.0;
    for (b, notes) in totals.breakdown.iter().zip(&notes_per_row) {
        yy += 12.0;
        l.text_right(base_r, yy, 9.0, false, 0, &fmt_decimal(b.base, 2, 2));
        let rate = format!("{} %", fmt_decimal(b.tax_rate.percent(), 0, 2));
        l.text_right(type_r, yy, 9.0, false, 0, &rate);
        l.text_right(tax_r, yy, 9.0, false, 0, &fmt_decimal(b.tax, 2, 2));
        for note in notes {
            yy += 9.5;
            l.text(x0, yy, 7.5, false, 100, note);
        }
    }
    let tx0 = l.x1 - w * 0.38;
    let mut ty = by;
    for (label, value, bold) in &total_rows {
        let step = if *bold { 16.0 } else { 13.0 };
        let size = if *bold { 11.0 } else { 9.0 };
        ty += step;
        l.text(tx0, ty - 3.0, size, *bold, 0, label);
        l.text_right(l.x1, ty - 3.0, size, *bold, 0, value);
    }
    l.y = by + breakdown_h.max(totals_h) + 14.0;

    // --- Pago ---
    if let Some(iban) = issuer.and_then(|i| i.iban.as_deref()) {
        l.ensure(30.0);
        let y = l.y;
        let text = format!(
            "Forma de pago: transferencia bancaria. IBAN: {}",
            fmt_iban(iban)
        );
        l.block(x0, y, l.width(), 9.0, false, 0, &text);
    }

    // --- Pie de página ---
    let count = l.pages.len();
    let centre = (l.x0 + l.x1) / 2.0;
    let footer_y = l.bottom;
    for (i, page) in l.pages.iter_mut().enumerate() {
        let text = format!("Página {} de {count}", i + 1);
        let x = centre - widths.of(false, &text, 8.0) / 2.0;
        page.push(Cmd::Text {
            x,
            y: footer_y,
            size: 8.0,
            bold: false,
            gray: 100,
            text,
        });
    }
    Ok(l)
}

/// Genera el PDF del documento. Determinista: los mismos datos dan el mismo contenido.
pub fn render(pdfium: &Pdfium, src: &PdfSource<'_>) -> Result<Vec<u8>, PdfError> {
    let widths = Widths::measure(pdfium)?;
    let layout = compose(src, &widths)?;
    let mut out = pdfium
        .create_new_pdf()
        .map_err(|e| pdf_err("no se pudo crear el PDF", e))?;
    let regular = out.fonts_mut().helvetica();
    let bold = out.fonts_mut().helvetica_bold();
    let background = match &src.letterhead {
        Some(bytes) => Some(
            pdfium
                .load_pdf_from_byte_slice(bytes, None)
                .map_err(|e| pdf_err("no se pudo abrir el membrete", e))?,
        ),
        None => None,
    };
    let background_pages = background.as_ref().map_or(0, |d| d.pages().len());
    for (i, cmds) in layout.pages.iter().enumerate() {
        let index = i as i32;
        match &background {
            Some(bg) => {
                // Primera hoja: primera página del membrete; las siguientes, la segunda (o la
                // primera si solo hay una).
                let source = if i == 0 {
                    0
                } else {
                    1.min(background_pages.saturating_sub(1))
                };
                out.pages_mut()
                    .copy_page_from_document(bg, source, index)
                    .map_err(|e| pdf_err("no se pudo copiar el membrete", e))?;
            }
            None => {
                out.pages_mut()
                    .create_page_at_end(PdfPagePaperSize::a4())
                    .map_err(|e| pdf_err("no se pudo crear la página", e))?;
            }
        }
        let mut page = out
            .pages()
            .get(index)
            .map_err(|e| pdf_err("no se pudo leer la página", e))?;
        let page_h = page.height().value;
        for cmd in cmds {
            match cmd {
                Cmd::Text {
                    x,
                    y,
                    size,
                    bold: is_bold,
                    gray,
                    text,
                } => {
                    let mut object = page
                        .objects_mut()
                        .create_text_object(
                            PdfPoints::new(*x),
                            PdfPoints::new(page_h - y),
                            text,
                            if *is_bold { bold } else { regular },
                            PdfPoints::new(*size),
                        )
                        .map_err(|e| pdf_err("no se pudo escribir el texto", e))?;
                    if *gray != 0 {
                        object
                            .set_fill_color(PdfColor::new(*gray, *gray, *gray, 255))
                            .map_err(|e| pdf_err("no se pudo colorear el texto", e))?;
                    }
                }
                Cmd::Rect { x, y, w, h, gray } => {
                    let rect = PdfRect::new(
                        PdfPoints::new(page_h - y - h),
                        PdfPoints::new(*x),
                        PdfPoints::new(page_h - y),
                        PdfPoints::new(x + w),
                    );
                    page.objects_mut()
                        .create_path_object_rect(
                            rect,
                            None,
                            None,
                            Some(PdfColor::new(*gray, *gray, *gray, 255)),
                        )
                        .map_err(|e| pdf_err("no se pudo dibujar", e))?;
                }
                Cmd::Image { x, y, w, h, image } => {
                    page.objects_mut()
                        .create_image_object(
                            PdfPoints::new(*x),
                            PdfPoints::new(page_h - y - h),
                            &layout.images[*image],
                            Some(PdfPoints::new(*w)),
                            Some(PdfPoints::new(*h)),
                        )
                        .map_err(|e| pdf_err("no se pudo insertar la imagen", e))?;
                }
            }
        }
    }
    let mut bytes = out
        .save_to_bytes()
        .map_err(|e| pdf_err("no se pudo guardar el PDF", e))?;
    stabilize(&mut bytes, src.document.issue_date);
    Ok(bytes)
}

/// PDFium estampa en cada PDF la hora de creación (`/CreationDate`) y un identificador aleatorio
/// (`/ID` del trailer). Se sustituyen, con la misma longitud para no mover ningún desplazamiento
/// de la tabla de referencias cruzadas, por la fecha de expedición a las 00:00:00 y por un
/// resumen SHA-256 del propio contenido: regenerar un documento da los mismos bytes (D226). Si
/// una versión futura de PDFium cambiara esos campos, no se toca nada y la prueba de
/// determinismo lo detecta.
fn stabilize(bytes: &mut [u8], issue_date: Option<IsoDate>) {
    use sha2::{Digest, Sha256};

    const DATE_KEY: &[u8] = b"/CreationDate(D:";
    const ID_KEY: &[u8] = b"/ID[<";
    let find = |bytes: &[u8], key: &[u8]| {
        bytes
            .windows(key.len())
            .rposition(|w| w == key)
            .map(|i| i + key.len())
    };
    if let Some(at) = find(bytes, DATE_KEY).filter(|at| bytes.len() >= at + 14) {
        let stamp = match issue_date {
            Some(d) => format!("{:04}{:02}{:02}000000", d.year, d.month, d.day),
            None => "19700101000000".to_owned(),
        };
        bytes[at..at + 14].copy_from_slice(stamp.as_bytes());
    }
    // `/ID[<32 hex><32 hex>]`: se calcula el resumen con ambas cadenas a cero.
    let Some(at) = find(bytes, ID_KEY).filter(|at| bytes.len() >= at + 32 + 2 + 32) else {
        return;
    };
    let second = at + 32 + 2;
    let well_formed = bytes[at..at + 32].iter().all(u8::is_ascii_hexdigit)
        && &bytes[at + 32..second] == b"><"
        && bytes[second..second + 32].iter().all(u8::is_ascii_hexdigit);
    if !well_formed {
        return;
    }
    bytes[at..at + 32].fill(b'0');
    bytes[second..second + 32].fill(b'0');
    let digest = Sha256::digest(&*bytes);
    let hex: String = digest[..16].iter().map(|b| format!("{b:02X}")).collect();
    bytes[at..at + 32].copy_from_slice(hex.as_bytes());
    bytes[second..second + 32].copy_from_slice(hex.as_bytes());
}
