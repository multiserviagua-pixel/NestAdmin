//! Datos sintéticos y utilidades comunes. El NIF 89890001K es el de los ejemplos oficiales de
//! la AEAT; el resto de NIF y nombres son inventados. Ninguna prueba sale a la red.

#![allow(dead_code)]

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use nestadmin_verifactu::*;

pub const NIF_EMISOR: &str = "89890001K";
pub const NIF_CLIENTE: &str = "B00000034";
pub const NIF_PRODUCTOR: &str = "B00000000";

pub fn fecha(s: &str) -> Fecha {
    s.parse().unwrap()
}

pub fn fh(s: &str) -> FechaHoraHuso {
    s.parse().unwrap()
}

pub fn eur(c: i64) -> Importe {
    Importe::centimos(c).unwrap()
}

pub fn sistema() -> SistemaInformatico {
    SistemaInformatico {
        productor: Persona::con_nif("Productor Sintetico SL", NIF_PRODUCTOR),
        nombre_sistema: "NestAdmin".to_owned(),
        id_sistema: "NA".to_owned(),
        version: "0.1.0".to_owned(),
        numero_instalacion: "INST-0001".to_owned(),
        solo_verifactu: true,
        multi_ot: false,
        indicador_multiples_ot: false,
    }
}

/// Alta F1 de 100,00 € + 21 % de IVA, sin sellar.
pub fn alta(serie: &str, enc: Encadenamiento, momento: &str) -> RegistroAlta {
    RegistroAlta {
        id_factura: IdFactura::new(NIF_EMISOR, serie, fecha("01-01-2024")),
        ref_externa: None,
        nombre_razon_emisor: "Emisor Sintetico SL".to_owned(),
        subsanacion: false,
        rechazo_previo: None,
        tipo_factura: TipoFactura::F1,
        tipo_rectificativa: None,
        facturas_rectificadas: vec![],
        facturas_sustituidas: vec![],
        importe_rectificacion: None,
        fecha_operacion: None,
        descripcion_operacion: "Servicios de prueba".to_owned(),
        factura_simplificada_art7273: None,
        sin_identif_destinatario_art61d: None,
        macrodato: None,
        emitida_por_tercero_o_destinatario: None,
        tercero: None,
        destinatarios: vec![Persona::con_nif("Cliente Sintetico SA", NIF_CLIENTE)],
        cupon: None,
        desglose: vec![DetalleDesglose::iva_general(
            eur(10_000),
            Porcentaje::centesimas(2100).unwrap(),
            eur(2_100),
        )],
        cuota_total: eur(2_100),
        importe_total: eur(12_100),
        encadenamiento: enc,
        sistema_informatico: sistema(),
        fecha_hora_huso_gen_registro: fh(momento),
        huella: String::new(),
    }
}

pub fn anulacion(id: IdFactura, enc: Encadenamiento, momento: &str) -> RegistroAnulacion {
    RegistroAnulacion {
        id_factura: id,
        ref_externa: None,
        sin_registro_previo: false,
        rechazo_previo: false,
        generado_por: None,
        generador: None,
        encadenamiento: enc,
        sistema_informatico: sistema(),
        fecha_hora_huso_gen_registro: fh(momento),
        huella: String::new(),
    }
}

/// Cadena sellada: alta A-1, alta A-2 y anulación de A-2.
pub fn cadena() -> Vec<RegistroFacturacion> {
    let mut r1 = alta(
        "A-1",
        Encadenamiento::PrimerRegistro,
        "2024-01-01T10:00:00+01:00",
    );
    r1.sellar().unwrap();
    let r1 = RegistroFacturacion::Alta(r1);
    let mut r2 = alta(
        "A-2",
        r1.siguiente_encadenamiento(),
        "2024-01-01T10:05:00+01:00",
    );
    r2.sellar().unwrap();
    let r2 = RegistroFacturacion::Alta(r2);
    let mut r3 = anulacion(
        r2.id_factura().clone(),
        r2.siguiente_encadenamiento(),
        "2024-01-01T10:10:00+01:00",
    );
    r3.sellar().unwrap();
    vec![r1, r2, RegistroFacturacion::Anulacion(r3)]
}

pub fn envio(registros: Vec<RegistroFacturacion>) -> Envio {
    Envio {
        cabecera: Cabecera::new(PersonaEs::new("Emisor Sintetico SL", NIF_EMISOR)),
        registros,
    }
}

/// `POST` HTTP/1.1 mínimo a una URL `http://127.0.0.1:puerto/...`. Devuelve (estado,
/// cuerpo) o `None` si el servidor cierra sin responder.
pub fn post(url: &str, cuerpo: &str) -> Option<(u16, String)> {
    let resto = url.strip_prefix("http://").expect("solo http local");
    let (host, ruta) = resto.split_once('/').unwrap_or((resto, ""));
    let mut s = TcpStream::connect(host).expect("conexión al simulador");
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    write!(
        s,
        "POST /{ruta} HTTP/1.1\r\nHost: {host}\r\nContent-Type: text/xml; charset=utf-8\r\nSOAPAction: \"\"\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{cuerpo}",
        cuerpo.len()
    )
    .unwrap();
    let mut resp = String::new();
    if let Err(e) = s.read_to_string(&mut resp) {
        eprintln!("lectura de la respuesta: {e}");
        return None;
    }
    if resp.is_empty() {
        return None;
    }
    let (cab, cuerpo) = resp.split_once("\r\n\r\n").expect("respuesta HTTP");
    let estado = cab.split_whitespace().nth(1).unwrap().parse().unwrap();
    Some((estado, cuerpo.to_owned()))
}

pub fn dir_xsd() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("xsd")
}

/// Valida un documento contra los XSD oficiales con `System.Xml` de .NET (Windows
/// PowerShell). Los XSD no se versionan: `tests/xsd/obtener.ps1` los descarga.
pub fn validar_xsd(xml: &str) -> Result<(), String> {
    let dir = dir_xsd();
    for f in [
        "SuministroLR.xsd",
        "SuministroInformacion.xsd",
        "RespuestaSuministro.xsd",
        "xmldsig-core-schema.xsd",
    ] {
        assert!(
            dir.join(f).exists(),
            "falta tests/xsd/{f}; ejecute `powershell -File crates/verifactu/tests/xsd/obtener.ps1`"
        );
    }
    let tmp = tempfile::tempdir().unwrap();
    let doc = tmp.path().join("doc.xml");
    std::fs::write(&doc, xml).unwrap();
    let out = Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ])
        .arg(dir.join("validar.ps1"))
        .arg(&dir)
        .arg(&doc)
        .output()
        .expect("powershell");
    let texto =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    if out.status.success() && texto.contains("VALIDO") {
        Ok(())
    } else {
        Err(texto)
    }
}
