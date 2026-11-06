//! XML del envío y de la respuesta. La validación contra los XSD oficiales está marcada
//! `#[ignore]` porque los XSD de la AEAT no se versionan (su aviso legal no autoriza la
//! redistribución): descárguelos con `tests/xsd/obtener.ps1` y ejecute
//! `cargo test -p nestadmin-verifactu -- --ignored`.

mod common;

use common::*;
use nestadmin_verifactu::simulador::{ConfigSimulador, SimuladorAeat};
use nestadmin_verifactu::*;

/// Envío con todas las variantes que se usan: alta F1, rectificativa R1 por diferencias con
/// IDOtro y exenta, F2 sin destinatario y anulación.
fn envio_completo() -> Envio {
    let mut c = cadena();
    let mut r = alta(
        "R-1",
        c[2].siguiente_encadenamiento(),
        "2024-01-01T10:15:00+01:00",
    );
    r.tipo_factura = TipoFactura::R1;
    r.tipo_rectificativa = Some(TipoRectificativa::Diferencias);
    r.facturas_rectificadas = vec![IdFactura::new(NIF_EMISOR, "A-1", fecha("01-01-2024"))];
    r.fecha_operacion = Some(fecha("31-12-2023"));
    r.ref_externa = Some("pedido <7> & co".to_owned());
    r.destinatarios = vec![Persona {
        nombre_razon: "Cliente Extranjero".to_owned(),
        identificacion: Identificacion::Otro {
            codigo_pais: Some("FR".to_owned()),
            id_type: "02".to_owned(),
            id: "FR00000000000".to_owned(),
        },
    }];
    r.desglose.push(DetalleDesglose {
        impuesto: Some(Impuesto::Iva),
        clave_regimen: Some("01".to_owned()),
        calificacion: Calificacion::Exenta(5),
        tipo_impositivo: None,
        base_imponible: eur(-1_000),
        base_imponible_a_coste: None,
        cuota_repercutida: None,
        tipo_recargo_equivalencia: None,
        cuota_recargo_equivalencia: None,
    });
    r.cuota_total = eur(-2_100);
    r.importe_total = eur(-12_100);
    r.sellar().unwrap();
    let r = RegistroFacturacion::Alta(r);
    let mut s = alta(
        "T-1",
        r.siguiente_encadenamiento(),
        "2024-01-01T10:20:00+01:00",
    );
    s.tipo_factura = TipoFactura::F2;
    s.destinatarios.clear();
    s.macrodato = Some(false);
    s.sellar().unwrap();
    c.push(r);
    c.push(RegistroFacturacion::Alta(s));
    let mut e = envio(c);
    e.cabecera.remision_voluntaria = Some(RemisionVoluntaria {
        fecha_fin_verifactu: None,
        incidencia: Some(false),
    });
    e
}

#[test]
fn estructura_del_envio() {
    let e = envio_completo();
    let xml = e.a_xml().unwrap();
    assert!(xml.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>"));
    assert!(xml.contains("<sum:RegFactuSistemaFacturacion xmlns:sum=\""));
    assert_eq!(xml.matches("<sum:RegistroFactura>").count(), 5);
    assert_eq!(xml.matches("<sum1:RegistroAlta>").count(), 4);
    assert_eq!(xml.matches("<sum1:RegistroAnulacion>").count(), 1);
    assert!(xml.contains("<sum1:PrimerRegistro>S</sum1:PrimerRegistro>"));
    assert!(xml.contains("<sum1:ImporteTotal>121.00</sum1:ImporteTotal>"));
    assert!(xml.contains("<sum1:ImporteTotal>-121.00</sum1:ImporteTotal>"));
    assert!(xml.contains(
        "<sum1:FechaHoraHusoGenRegistro>2024-01-01T10:00:00+01:00</sum1:FechaHoraHusoGenRegistro>"
    ));
    assert!(xml.contains("<sum1:TipoHuella>01</sum1:TipoHuella>"));
    assert!(xml.contains("<sum1:OperacionExenta>E5</sum1:OperacionExenta>"));
    assert!(xml.contains("pedido &lt;7&gt; &amp; co"), "texto escapado");
    // El orden de la secuencia del XSD: Huella es el último campo antes de la firma.
    let alta =
        &xml[xml.find("<sum1:RegistroAlta>").unwrap()..xml.find("</sum1:RegistroAlta>").unwrap()];
    let pos = |t: &str| alta.find(t).unwrap_or_else(|| panic!("falta {t}"));
    assert!(pos("<sum1:IDVersion>") < pos("<sum1:IDFactura>"));
    assert!(pos("<sum1:Desglose>") < pos("<sum1:CuotaTotal>"));
    assert!(pos("<sum1:Encadenamiento>") < pos("<sum1:SistemaInformatico>"));
    assert!(pos("<sum1:TipoHuella>") < pos("<sum1:Huella>"));

    let soap = e.a_soap().unwrap();
    assert!(soap
        .contains("<soapenv:Envelope xmlns:soapenv=\"http://schemas.xmlsoap.org/soap/envelope/\""));
    assert!(soap.contains("<soapenv:Header/>"));
}

#[test]
fn limites_del_envio() {
    assert_eq!(envio(vec![]).a_xml(), Err(Error::NumeroRegistros(0)));
    let c = cadena();
    let muchos = envio(vec![c[0].clone(); MAX_REGISTROS_POR_ENVIO + 1]);
    assert_eq!(muchos.a_xml(), Err(Error::NumeroRegistros(1001)));
    assert!(envio(vec![c[0].clone(); MAX_REGISTROS_POR_ENVIO])
        .a_xml()
        .is_ok());

    let sin_sellar = RegistroFacturacion::Alta(alta(
        "A-9",
        Encadenamiento::PrimerRegistro,
        "2024-01-01T10:00:00+01:00",
    ));
    assert!(
        matches!(envio(vec![sin_sellar]).a_xml(), Err(Error::Campo { campo, .. }) if campo.ends_with("Huella"))
    );
}

#[test]
#[ignore = "requiere los XSD oficiales en tests/xsd (obtener.ps1) y Windows PowerShell"]
fn envio_valido_contra_xsd() {
    validar_xsd(&envio_completo().a_xml().unwrap()).unwrap();
    validar_xsd(&envio(cadena()).a_xml().unwrap()).unwrap();
}

#[test]
#[ignore = "requiere los XSD oficiales en tests/xsd (obtener.ps1) y Windows PowerShell"]
fn xsd_detecta_un_envio_invalido() {
    let xml = envio(cadena())
        .a_xml()
        .unwrap()
        .replace("<sum1:TipoFactura>F1<", "<sum1:TipoFactura>F9<");
    let e = validar_xsd(&xml).unwrap_err();
    assert!(e.contains("TipoFactura") && e.contains("F9"), "{e}");
}

#[test]
#[ignore = "requiere los XSD oficiales en tests/xsd (obtener.ps1) y Windows PowerShell"]
fn respuesta_del_simulador_valida_contra_xsd() {
    let mut cfg = ConfigSimulador::default();
    cfg.errores.insert(
        "A-2".to_owned(),
        simulador::ErrorForzado {
            codigo: 1100,
            descripcion: "Valor o tipo incorrecto del campo.".to_owned(),
            aceptado: false,
        },
    );
    let sim = SimuladorAeat::iniciar(cfg).unwrap();
    let e = envio(cadena()).a_soap().unwrap();
    post(&sim.url(), &e).unwrap();
    let (_, cuerpo) = post(&sim.url(), &e).unwrap(); // con duplicados
    let ini = cuerpo
        .find("<tikR:RespuestaRegFactuSistemaFacturacion")
        .unwrap();
    let fin = cuerpo.find("</env:Body>").unwrap();
    let doc = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>{}",
        &cuerpo[ini..fin]
    );
    validar_xsd(&doc).unwrap();
}
