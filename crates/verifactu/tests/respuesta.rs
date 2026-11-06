//! Parseo de respuestas de la AEAT con datos sintéticos.

use nestadmin_verifactu::*;

const NS: &str = r#"xmlns:env="http://schemas.xmlsoap.org/soap/envelope/" xmlns:tikR="https://www2.agenciatributaria.gob.es/static_files/common/internet/dep/aplicaciones/es/aeat/tike/cont/ws/RespuestaSuministro.xsd" xmlns:tik="https://www2.agenciatributaria.gob.es/static_files/common/internet/dep/aplicaciones/es/aeat/tike/cont/ws/SuministroInformacion.xsd""#;

fn sobre(cuerpo: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><env:Envelope {NS}><env:Body><tikR:RespuestaRegFactuSistemaFacturacion>{cuerpo}</tikR:RespuestaRegFactuSistemaFacturacion></env:Body></env:Envelope>"
    )
}

const CABECERA: &str = "<tikR:Cabecera><tik:ObligadoEmision><tik:NombreRazon>Emisor Sintetico SL</tik:NombreRazon><tik:NIF>89890001K</tik:NIF></tik:ObligadoEmision></tikR:Cabecera>";

fn linea(serie: &str, estado: &str, extra: &str) -> String {
    format!(
        "<tikR:RespuestaLinea><tikR:IDFactura><tik:IDEmisorFactura>89890001K</tik:IDEmisorFactura><tik:NumSerieFactura>{serie}</tik:NumSerieFactura><tik:FechaExpedicionFactura>01-01-2024</tik:FechaExpedicionFactura></tikR:IDFactura><tikR:Operacion><tik:TipoOperacion>Alta</tik:TipoOperacion></tikR:Operacion><tikR:EstadoRegistro>{estado}</tikR:EstadoRegistro>{extra}</tikR:RespuestaLinea>"
    )
}

fn respuesta(xml: &str) -> RespuestaEnvio {
    match ResultadoEnvio::desde_xml(xml).unwrap() {
        ResultadoEnvio::Respuesta(r) => r,
        otro => panic!("se esperaba respuesta: {otro:?}"),
    }
}

#[test]
fn correcto() {
    let xml = sobre(&format!(
        "<tikR:CSV>CSVSINTETICO01</tikR:CSV><tikR:DatosPresentacion><tik:NIFPresentador>89890001K</tik:NIFPresentador><tik:TimestampPresentacion>2024-01-01T19:20:31+01:00</tik:TimestampPresentacion></tikR:DatosPresentacion>{CABECERA}<tikR:TiempoEsperaEnvio>60</tikR:TiempoEsperaEnvio><tikR:EstadoEnvio>Correcto</tikR:EstadoEnvio>{}",
        linea("A-1", "Correcto", "")
    ));
    let r = respuesta(&xml);
    assert_eq!(r.estado_envio, EstadoEnvio::Correcto);
    assert_eq!(r.csv.as_deref(), Some("CSVSINTETICO01"));
    assert_eq!(r.nif_presentador.as_deref(), Some("89890001K"));
    assert_eq!(r.tiempo_espera_envio, 60);
    assert_eq!(r.lineas.len(), 1);
    let l = &r.lineas[0];
    assert_eq!(l.id_factura.num_serie, "A-1");
    assert_eq!(l.id_factura.fecha_expedicion.to_string(), "01-01-2024");
    assert_eq!(l.tipo_operacion, "Alta");
    assert_eq!(l.estado, EstadoRegistro::Correcto);
    assert!(l.estado.aceptado());
    assert_eq!(l.codigo_error, None);
}

#[test]
fn parcialmente_correcto_con_errores_y_duplicado() {
    let xml = sobre(&format!(
        "<tikR:CSV>CSVSINTETICO02</tikR:CSV>{CABECERA}<tikR:TiempoEsperaEnvio>120</tikR:TiempoEsperaEnvio><tikR:EstadoEnvio>ParcialmenteCorrecto</tikR:EstadoEnvio>{}{}{}",
        linea("A-1", "AceptadoConErrores", "<tikR:CodigoErrorRegistro>2000</tikR:CodigoErrorRegistro><tikR:DescripcionErrorRegistro>El c&#225;lculo de la huella suministrada es incorrecta.</tikR:DescripcionErrorRegistro>"),
        linea("A&amp;2", "Incorrecto", "<tikR:CodigoErrorRegistro>3000</tikR:CodigoErrorRegistro><tikR:DescripcionErrorRegistro>Registro de facturación duplicado.</tikR:DescripcionErrorRegistro><tikR:RegistroDuplicado><tik:IdPeticionRegistroDuplicado>SIM00000000000000001</tik:IdPeticionRegistroDuplicado><tik:EstadoRegistroDuplicado>Correcta</tik:EstadoRegistroDuplicado></tikR:RegistroDuplicado>"),
        linea("A-3", "Correcto", ""),
    ));
    let r = respuesta(&xml);
    assert_eq!(r.estado_envio, EstadoEnvio::ParcialmenteCorrecto);
    assert_eq!(r.tiempo_espera_envio, 120);
    assert_eq!(r.lineas[0].estado, EstadoRegistro::AceptadoConErrores);
    assert_eq!(r.lineas[0].codigo_error, Some(2000));
    assert_eq!(
        r.lineas[0].descripcion_error.as_deref(),
        Some("El cálculo de la huella suministrada es incorrecta.")
    );
    let d = &r.lineas[1];
    assert_eq!(d.id_factura.num_serie, "A&2");
    assert_eq!(d.estado, EstadoRegistro::Incorrecto);
    assert!(!d.estado.aceptado());
    assert_eq!(d.codigo_error, Some(3000));
    let dup = d.duplicado.as_ref().unwrap();
    assert_eq!(dup.id_peticion, "SIM00000000000000001");
    assert_eq!(dup.estado, "Correcta");
}

#[test]
fn incorrecto_sin_csv() {
    let xml = sobre(&format!(
        "{CABECERA}<tikR:TiempoEsperaEnvio>60</tikR:TiempoEsperaEnvio><tikR:EstadoEnvio>Incorrecto</tikR:EstadoEnvio>{}",
        linea("A-1", "Incorrecto", "<tikR:CodigoErrorRegistro>1100</tikR:CodigoErrorRegistro>")
    ));
    let r = respuesta(&xml);
    assert_eq!(r.estado_envio, EstadoEnvio::Incorrecto);
    assert_eq!(r.csv, None);
    assert_eq!(r.timestamp_presentacion, None);
}

#[test]
fn fallo_soap_con_codigo() {
    let xml = r#"<env:Envelope xmlns:env="http://schemas.xmlsoap.org/soap/envelope/"><env:Body><env:Fault><faultcode>env:Client</faultcode><faultstring>Codigo[4104].Error en la cabecera: el valor del campo NIF del bloque ObligadoEmision no está identificado.</faultstring><detail><callstack>sintetico</callstack></detail></env:Fault></env:Body></env:Envelope>"#;
    match ResultadoEnvio::desde_xml(xml).unwrap() {
        ResultadoEnvio::Fallo(f) => {
            assert_eq!(f.codigo, Some(4104));
            assert_eq!(f.faultcode, "env:Client");
            assert!(f.faultstring.contains("ObligadoEmision"));
        }
        otro => panic!("{otro:?}"),
    }
    let sin_codigo = xml.replace("Codigo[4104].", "");
    match ResultadoEnvio::desde_xml(&sin_codigo).unwrap() {
        ResultadoEnvio::Fallo(f) => assert_eq!(f.codigo, None),
        otro => panic!("{otro:?}"),
    }
}

#[test]
fn tiempo_de_espera() {
    let base = |te: &str| {
        sobre(&format!(
            "{CABECERA}{te}<tikR:EstadoEnvio>Correcto</tikR:EstadoEnvio>"
        ))
    };
    // Vacío (lo admite `Tipo6Type`): se usa el inicial de 60 s.
    let r = respuesta(&base("<tikR:TiempoEsperaEnvio></tikR:TiempoEsperaEnvio>"));
    assert_eq!(r.tiempo_espera_envio, TIEMPO_ESPERA_INICIAL_S);
    // Ausente: la falta de datos es un error explícito.
    assert_eq!(
        ResultadoEnvio::desde_xml(&base("")),
        Err(Error::RespuestaIncompleta("TiempoEsperaEnvio".to_owned()))
    );
    assert!(
        ResultadoEnvio::desde_xml(&base("<tikR:TiempoEsperaEnvio>x</tikR:TiempoEsperaEnvio>"))
            .is_err()
    );
}

#[test]
fn respuestas_mal_formadas() {
    assert!(ResultadoEnvio::desde_xml("").is_err());
    assert!(ResultadoEnvio::desde_xml("<html>Servicio no disponible</html>").is_err());
    assert!(ResultadoEnvio::desde_xml("<a><b></a>").is_err());
    let con_doctype = format!(
        "<!DOCTYPE x [<!ENTITY e \"x\">]>{}",
        sobre(&format!("{CABECERA}<tikR:TiempoEsperaEnvio>60</tikR:TiempoEsperaEnvio><tikR:EstadoEnvio>Correcto</tikR:EstadoEnvio>"))
    );
    assert!(matches!(
        ResultadoEnvio::desde_xml(&con_doctype),
        Err(Error::Xml(_))
    ));
    let estado_raro = sobre(&format!("{CABECERA}<tikR:TiempoEsperaEnvio>60</tikR:TiempoEsperaEnvio><tikR:EstadoEnvio>Otro</tikR:EstadoEnvio>"));
    assert!(ResultadoEnvio::desde_xml(&estado_raro).is_err());
}
