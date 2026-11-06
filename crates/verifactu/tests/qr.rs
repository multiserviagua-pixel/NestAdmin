//! URL de cotejo y QR (especificación del QR v0.5.0).

mod common;

use common::*;
use nestadmin_verifactu::qr::{LEYENDA_VERIFACTU, TEXTO_SUPERIOR};
use nestadmin_verifactu::*;

#[test]
fn url_del_ejemplo_oficial() {
    // Apartado 4: numserie «12345678&G33» se codifica como 12345678%26G33. El ejemplo usa
    // «241.4»; aquí el importe va con dos decimales, igual que ImporteTotal en el XML.
    let url = url_cotejo(
        Entorno::Pruebas,
        "89890001K",
        "12345678&G33",
        fecha("01-01-2024"),
        eur(24_140),
    );
    assert_eq!(
        url,
        "https://prewww2.aeat.es/wlpl/TIKE-CONT/ValidarQR?nif=89890001K&numserie=12345678%26G33&fecha=01-01-2024&importe=241.40"
    );
    let prod = url_cotejo(
        Entorno::Produccion,
        "89890001K",
        "12345678-G33",
        fecha("01-09-2024"),
        eur(24_140),
    );
    assert_eq!(
        prod,
        "https://www2.agenciatributaria.gob.es/wlpl/TIKE-CONT/ValidarQR?nif=89890001K&numserie=12345678-G33&fecha=01-09-2024&importe=241.40"
    );
    let raro = url_cotejo(
        Entorno::Pruebas,
        "89890001K",
        "A 1/2?=+#",
        fecha("01-01-2024"),
        eur(-500),
    );
    assert!(raro.contains("numserie=A%201%2F2%3F%3D%2B%23&"), "{raro}");
    assert!(raro.ends_with("importe=-5.00"));
    assert!(!raro.contains("formato="), "el QR nunca lleva formato=json");
}

#[test]
fn urls_del_servicio_de_remision() {
    assert_eq!(
        Entorno::Pruebas.url_verifactu_soap(false),
        "https://prewww1.aeat.es/wlpl/TIKE-CONT/ws/SistemaFacturacion/VerifactuSOAP"
    );
    assert!(Entorno::Produccion
        .url_verifactu_soap(true)
        .starts_with("https://www10.agenciatributaria.gob.es/"));
}

#[test]
fn qr_de_un_registro() {
    let mut r = alta(
        "A-1",
        Encadenamiento::PrimerRegistro,
        "2024-01-01T10:00:00+01:00",
    );
    r.sellar().unwrap();
    let q = CodigoQr::de_registro(Entorno::Pruebas, &r).unwrap();
    assert_eq!(
        q.url(),
        "https://prewww2.aeat.es/wlpl/TIKE-CONT/ValidarQR?nif=89890001K&numserie=A-1&fecha=01-01-2024&importe=121.00"
    );
    let n = q.ancho_modulos();
    assert!(
        n >= 21 && (n - 17).is_multiple_of(4),
        "lado de un QR estándar: {n}"
    );
    // Patrones de localización en tres esquinas.
    for (x, y) in [(0, 0), (n - 7, 0), (0, n - 7)] {
        assert!(q.modulo(x, y) && q.modulo(x + 6, y + 6) && q.modulo(x + 3, y + 3));
        assert!(!q.modulo(x + 1, y + 1));
    }
}

#[test]
fn svg_con_textos() {
    let q = CodigoQr::new("https://prewww2.aeat.es/wlpl/TIKE-CONT/ValidarQR?nif=89890001K&numserie=A-1&fecha=01-01-2024&importe=1.00").unwrap();
    let svg = q.svg();
    assert!(svg.starts_with("<svg xmlns=\"http://www.w3.org/2000/svg\""));
    assert!(svg.contains(&format!(">{TEXTO_SUPERIOR}</text>")));
    assert!(svg.contains(&format!(">{LEYENDA_VERIFACTU}</text>")));
    assert!(
        svg.find(TEXTO_SUPERIOR) < svg.find("<path"),
        "el texto precede al QR"
    );
    assert!(
        svg.find("<path") < svg.find(LEYENDA_VERIFACTU),
        "la leyenda va debajo"
    );
    assert!(
        svg.contains("width=\"47mm\""),
        "35 mm de símbolo + 2 x 6 mm de margen"
    );
}

#[test]
fn png_con_densidad_y_leyenda() {
    let q = CodigoQr::new("https://prewww2.aeat.es/wlpl/TIKE-CONT/ValidarQR?nif=89890001K&numserie=A-1&fecha=01-01-2024&importe=1.00").unwrap();
    let datos = q.png(8).unwrap();
    assert_eq!(&datos[..8], b"\x89PNG\r\n\x1a\n");
    let dec = png::Decoder::new(std::io::Cursor::new(&datos));
    let mut lector = dec.read_info().unwrap();
    let info = lector.info().clone();
    let n = q.ancho_modulos();
    assert!(info.width as usize >= (n + 8) * 8);
    let dims = info.pixel_dims.unwrap();
    assert_eq!(dims.unit, png::Unit::Meter);
    // El símbolo (n x 8 px) mide 35 mm.
    let mm = (n * 8) as f32 / dims.xppu as f32 * 1000.0;
    assert!((mm - 35.0).abs() < 0.1, "{mm} mm");

    let mut buf = vec![0; lector.output_buffer_size().unwrap()];
    lector.next_frame(&mut buf).unwrap();
    let w = info.width as usize;
    let negros_en_filas = |desde: usize, hasta: usize| {
        (desde..hasta)
            .map(|y| buf[y * w..(y + 1) * w].iter().filter(|&&p| p == 0).count())
            .sum::<usize>()
    };
    // Margen superior en blanco, texto encima y leyenda debajo del símbolo.
    assert_eq!(negros_en_filas(0, 32), 0);
    assert!(negros_en_filas(32, 60) > 0, "texto «QR tributario:»");
    let alto = info.height as usize;
    assert!(
        negros_en_filas(alto - 60, alto - 32) > 0,
        "leyenda VERI*FACTU"
    );
    assert_eq!(negros_en_filas(alto - 32, alto), 0);
}
