//! QR tributario de la factura (especificación del QR, versión 0.5.0 de 10/12/2025).
//!
//! - Contenido: URL de cotejo con `nif`, `numserie`, `fecha` e `importe`, valores codificados
//!   en UTF-8 con «URL encoding» (apartados 4 a 6). El parámetro `formato=json` no puede ir
//!   nunca en el QR.
//! - ISO/IEC 18004, nivel de corrección M, entre 30x30 y 40x40 mm, con al menos 2 mm en
//!   blanco alrededor (se recomiendan 6).
//! - Encima, el texto «QR tributario:»; debajo, en sistemas VERI*FACTU, «VERI*FACTU».

use qrcode::{Color, EcLevel, QrCode};

use crate::registro::RegistroAlta;
use crate::tipos::{Fecha, Importe};
use crate::Error;

pub const TEXTO_SUPERIOR: &str = "QR tributario:";
pub const LEYENDA_VERIFACTU: &str = "VERI*FACTU";

/// Lado del símbolo en milímetros que se usa por defecto (rango admitido: 30-40).
pub const LADO_MM: f32 = 35.0;
/// Margen en blanco alrededor del símbolo, en milímetros (el recomendado).
pub const MARGEN_MM: f32 = 6.0;

/// Entorno de la AEAT: servicio de remisión y servicio de cotejo del QR.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Entorno {
    /// Portal de Pruebas Externas.
    Pruebas,
    Produccion,
}

impl Entorno {
    /// URL del servicio `VerifactuSOAP` de remisión voluntaria (`SistemaFacturacion.wsdl`).
    /// `certificado_sello` elige el acceso para certificado de sello (`www10`/`prewww10`).
    pub fn url_verifactu_soap(&self, certificado_sello: bool) -> &'static str {
        match (self, certificado_sello) {
            (Self::Produccion, false) => {
                "https://www1.agenciatributaria.gob.es/wlpl/TIKE-CONT/ws/SistemaFacturacion/VerifactuSOAP"
            }
            (Self::Produccion, true) => {
                "https://www10.agenciatributaria.gob.es/wlpl/TIKE-CONT/ws/SistemaFacturacion/VerifactuSOAP"
            }
            (Self::Pruebas, false) => {
                "https://prewww1.aeat.es/wlpl/TIKE-CONT/ws/SistemaFacturacion/VerifactuSOAP"
            }
            (Self::Pruebas, true) => {
                "https://prewww10.aeat.es/wlpl/TIKE-CONT/ws/SistemaFacturacion/VerifactuSOAP"
            }
        }
    }

    /// URL base de `ValidarQR` (sistemas que emiten facturas verificables).
    pub fn base_validar_qr(&self) -> &'static str {
        match self {
            Self::Pruebas => "https://prewww2.aeat.es/wlpl/TIKE-CONT/ValidarQR",
            Self::Produccion => "https://www2.agenciatributaria.gob.es/wlpl/TIKE-CONT/ValidarQR",
        }
    }
}

/// Codificación de un valor de parámetro: UTF-8 y `%XX` para todo salvo los caracteres no
/// reservados de RFC 3986 (`A-Z a-z 0-9 - . _ ~`). El espacio va como `%20`.
fn codificar(valor: &str) -> String {
    let mut s = String::with_capacity(valor.len());
    for b in valor.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            s.push(b as char);
        } else {
            s.push_str(&format!("%{b:02X}"));
        }
    }
    s
}

/// URL de cotejo de una factura VERI*FACTU.
pub fn url_cotejo(
    entorno: Entorno,
    nif: &str,
    num_serie: &str,
    fecha: Fecha,
    importe_total: Importe,
) -> String {
    format!(
        "{}?nif={}&numserie={}&fecha={}&importe={}",
        entorno.base_validar_qr(),
        codificar(nif.trim()),
        codificar(num_serie.trim()),
        fecha,
        codificar(&importe_total.to_string())
    )
}

/// QR listo para imprimir con su texto superior y la leyenda VERI*FACTU.
#[derive(Debug, Clone)]
pub struct CodigoQr {
    url: String,
    ancho: usize,
    modulos: Vec<bool>,
}

impl CodigoQr {
    pub fn new(url: &str) -> Result<Self, Error> {
        let code = QrCode::with_error_correction_level(url.as_bytes(), EcLevel::M)
            .map_err(|e| Error::Qr(e.to_string()))?;
        let ancho = code.width();
        let modulos = code
            .to_colors()
            .into_iter()
            .map(|c| c == Color::Dark)
            .collect();
        Ok(Self {
            url: url.to_owned(),
            ancho,
            modulos,
        })
    }

    pub fn de_registro(entorno: Entorno, r: &RegistroAlta) -> Result<Self, Error> {
        Self::new(&url_cotejo(
            entorno,
            &r.id_factura.nif_emisor,
            &r.id_factura.num_serie,
            r.id_factura.fecha_expedicion,
            r.importe_total,
        ))
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    /// Módulos por lado del símbolo (sin zona en blanco).
    pub fn ancho_modulos(&self) -> usize {
        self.ancho
    }

    /// `true` si el módulo (x, y) es oscuro.
    pub fn modulo(&self, x: usize, y: usize) -> bool {
        self.modulos[y * self.ancho + x]
    }

    /// SVG con medidas en milímetros: símbolo de [`LADO_MM`], margen de [`MARGEN_MM`], texto
    /// superior y leyenda centrados.
    pub fn svg(&self) -> String {
        let n = self.ancho as f32;
        let m = LADO_MM / n;
        let texto_mm = 3.5_f32;
        let ancho = LADO_MM + 2.0 * MARGEN_MM;
        let y_qr = MARGEN_MM + texto_mm + 1.5;
        let alto = y_qr + LADO_MM + 1.5 + texto_mm + MARGEN_MM;
        let mut d = String::new();
        for y in 0..self.ancho {
            let mut x = 0;
            while x < self.ancho {
                if self.modulo(x, y) {
                    let inicio = x;
                    while x < self.ancho && self.modulo(x, y) {
                        x += 1;
                    }
                    d.push_str(&format!("M{inicio} {y}h{}v1h-{}z", x - inicio, x - inicio));
                } else {
                    x += 1;
                }
            }
        }
        format!(
            concat!(
                "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}mm\" height=\"{h}mm\" ",
                "viewBox=\"0 0 {w} {h}\">",
                "<rect width=\"{w}\" height=\"{h}\" fill=\"#fff\"/>",
                "<text x=\"{cx}\" y=\"{ty}\" font-family=\"Arial, Helvetica, sans-serif\" ",
                "font-size=\"{fs}\" text-anchor=\"middle\" fill=\"#000\">{sup}</text>",
                "<path transform=\"translate({mx} {qy}) scale({m})\" fill=\"#000\" ",
                "shape-rendering=\"crispEdges\" d=\"{d}\"/>",
                "<text x=\"{cx}\" y=\"{ly}\" font-family=\"Arial, Helvetica, sans-serif\" ",
                "font-size=\"{fs}\" text-anchor=\"middle\" fill=\"#000\">{ley}</text></svg>"
            ),
            w = ancho,
            h = alto,
            cx = ancho / 2.0,
            ty = MARGEN_MM + texto_mm,
            fs = texto_mm,
            sup = TEXTO_SUPERIOR,
            mx = MARGEN_MM,
            qy = y_qr,
            m = m,
            d = d,
            ly = y_qr + LADO_MM + 1.5 + texto_mm,
            ley = LEYENDA_VERIFACTU,
        )
    }

    /// PNG en escala de grises de 8 bits. `px_modulo` es el lado de cada módulo en píxeles;
    /// la densidad (`pHYs`) se fija para que el símbolo mida [`LADO_MM`] al imprimirlo.
    pub fn png(&self, px_modulo: u32) -> Result<Vec<u8>, Error> {
        let px_modulo = px_modulo.max(1) as usize;
        let lado = self.ancho * px_modulo;
        let margen = 4 * px_modulo; // zona en blanco de 4 módulos (ISO/IEC 18004)
        let escala = (px_modulo / 2).max(1);
        let alto_texto = 7 * escala;
        let sep = 2 * px_modulo;
        let ancho_txt = |t: &str| t.chars().count() * 6 * escala;
        let ancho = (lado + 2 * margen)
            .max(ancho_txt(TEXTO_SUPERIOR) + 2 * margen)
            .max(ancho_txt(LEYENDA_VERIFACTU) + 2 * margen);
        let alto = margen + alto_texto + sep + lado + sep + alto_texto + margen;
        let mut px = vec![255u8; ancho * alto];

        let y_sup = margen;
        let y_qr = y_sup + alto_texto + sep;
        let y_ley = y_qr + lado + sep;
        let x_qr = (ancho - lado) / 2;
        for y in 0..self.ancho {
            for x in 0..self.ancho {
                if self.modulo(x, y) {
                    for dy in 0..px_modulo {
                        let fila = (y_qr + y * px_modulo + dy) * ancho;
                        let col = x_qr + x * px_modulo;
                        px[fila + col..fila + col + px_modulo].fill(0);
                    }
                }
            }
        }
        for (texto, y0) in [(TEXTO_SUPERIOR, y_sup), (LEYENDA_VERIFACTU, y_ley)] {
            let x0 = (ancho - ancho_txt(texto)) / 2;
            dibujar_texto(&mut px, ancho, x0, y0, escala, texto)?;
        }

        let mut out = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut out, ancho as u32, alto as u32);
            enc.set_color(png::ColorType::Grayscale);
            enc.set_depth(png::BitDepth::Eight);
            let ppm = (lado as f32 / (LADO_MM / 1000.0)).round() as u32;
            enc.set_pixel_dims(Some(png::PixelDimensions {
                xppu: ppm,
                yppu: ppm,
                unit: png::Unit::Meter,
            }));
            let mut w = enc.write_header().map_err(|e| Error::Qr(e.to_string()))?;
            w.write_image_data(&px)
                .map_err(|e| Error::Qr(e.to_string()))?;
        }
        Ok(out)
    }
}

/// Glifos de 5x7 para los textos fijos del QR (no hace falta una fuente del sistema).
fn glifo(c: char) -> Option<[&'static str; 7]> {
    Some(match c {
        ' ' => [
            ".....", ".....", ".....", ".....", ".....", ".....", ".....",
        ],
        ':' => [
            ".....", "..#..", "..#..", ".....", "..#..", "..#..", ".....",
        ],
        '*' => [
            ".....", "..#..", "#.#.#", ".###.", "#.#.#", "..#..", ".....",
        ],
        'A' => [
            ".###.", "#...#", "#...#", "#####", "#...#", "#...#", "#...#",
        ],
        'C' => [
            ".###.", "#...#", "#....", "#....", "#....", "#...#", ".###.",
        ],
        'E' => [
            "#####", "#....", "#....", "####.", "#....", "#....", "#####",
        ],
        'F' => [
            "#####", "#....", "#....", "####.", "#....", "#....", "#....",
        ],
        'I' => [
            ".###.", "..#..", "..#..", "..#..", "..#..", "..#..", ".###.",
        ],
        'Q' => [
            ".###.", "#...#", "#...#", "#...#", "#.#.#", "#..#.", ".##.#",
        ],
        'R' => [
            "####.", "#...#", "#...#", "####.", "#.#..", "#..#.", "#...#",
        ],
        'T' => [
            "#####", "..#..", "..#..", "..#..", "..#..", "..#..", "..#..",
        ],
        'U' => [
            "#...#", "#...#", "#...#", "#...#", "#...#", "#...#", ".###.",
        ],
        'V' => [
            "#...#", "#...#", "#...#", "#...#", "#...#", ".#.#.", "..#..",
        ],
        'a' => [
            ".....", ".....", ".###.", "....#", ".####", "#...#", ".####",
        ],
        'b' => [
            "#....", "#....", "#.##.", "##..#", "#...#", "#...#", "####.",
        ],
        'i' => [
            "..#..", ".....", ".##..", "..#..", "..#..", "..#..", ".###.",
        ],
        'o' => [
            ".....", ".....", ".###.", "#...#", "#...#", "#...#", ".###.",
        ],
        'r' => [
            ".....", ".....", "#.##.", "##..#", "#....", "#....", "#....",
        ],
        't' => [
            ".#...", ".#...", "###..", ".#...", ".#...", ".#..#", "..##.",
        ],
        'u' => [
            ".....", ".....", "#...#", "#...#", "#...#", "#..##", ".##.#",
        ],
        _ => return None,
    })
}

fn dibujar_texto(
    px: &mut [u8],
    ancho: usize,
    x0: usize,
    y0: usize,
    escala: usize,
    texto: &str,
) -> Result<(), Error> {
    for (i, c) in texto.chars().enumerate() {
        let g = glifo(c).ok_or_else(|| Error::Qr(format!("sin glifo para «{c}»")))?;
        for (fy, fila) in g.iter().enumerate() {
            for (fx, b) in fila.bytes().enumerate() {
                if b != b'#' {
                    continue;
                }
                for dy in 0..escala {
                    let base = (y0 + fy * escala + dy) * ancho + x0 + (i * 6 + fx) * escala;
                    px[base..base + escala].fill(0);
                }
            }
        }
    }
    Ok(())
}
