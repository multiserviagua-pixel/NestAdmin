//! Simulador local de la AEAT: aceptación, rechazo, duplicados, tiempo de espera y fallos de
//! red. Todo en 127.0.0.1 con datos sintéticos.

mod common;

use common::*;
use nestadmin_verifactu::simulador::{ConfigSimulador, ErrorForzado, SimuladorAeat};
use nestadmin_verifactu::*;

fn enviar(sim: &SimuladorAeat, e: &Envio) -> (u16, ResultadoEnvio) {
    let (codigo, cuerpo) = post(&sim.url(), &e.a_soap().unwrap()).expect("respuesta");
    (codigo, ResultadoEnvio::desde_xml(&cuerpo).unwrap())
}

fn respuesta(r: ResultadoEnvio) -> RespuestaEnvio {
    match r {
        ResultadoEnvio::Respuesta(r) => r,
        ResultadoEnvio::Fallo(f) => panic!("fallo inesperado: {f:?}"),
    }
}

fn simulador(espera: u32) -> SimuladorAeat {
    SimuladorAeat::iniciar(ConfigSimulador {
        tiempo_espera_envio: espera,
        ..ConfigSimulador::default()
    })
    .unwrap()
}

#[test]
fn acepta_una_cadena_y_devuelve_el_tiempo_de_espera() {
    let sim = simulador(75);
    assert!(sim.direccion().ip().is_loopback());
    let (codigo, r) = enviar(&sim, &envio(cadena()));
    assert_eq!(codigo, 200);
    let r = respuesta(r);
    assert_eq!(r.estado_envio, EstadoEnvio::Correcto);
    assert_eq!(r.tiempo_espera_envio, 75);
    assert!(r.csv.is_some());
    assert_eq!(r.lineas.len(), 3);
    assert_eq!(r.lineas[2].tipo_operacion, "Anulacion");
    let est = sim.estado();
    assert_eq!(est.envios_procesados, 1);
    let a1 = &est.registros[&(
        NIF_EMISOR.to_owned(),
        "A-1".to_owned(),
        "01-01-2024".to_owned(),
    )];
    assert_eq!(a1.estado, "Correcta");
    let a2 = &est.registros[&(
        NIF_EMISOR.to_owned(),
        "A-2".to_owned(),
        "01-01-2024".to_owned(),
    )];
    assert_eq!(a2.estado, "Anulada");
}

/// Si se pierde la respuesta y se reintenta, el simulador rechaza el alta por duplicada (3000)
/// e informa del registro ya guardado: el reintento no duplica el efecto.
#[test]
fn reintento_tras_respuesta_perdida_detecta_duplicado() {
    let sim = simulador(60);
    let c = cadena();
    let e = envio(c[..2].to_vec());
    sim.estado().perder_siguiente_respuesta = true;
    assert!(
        post(&sim.url(), &e.a_soap().unwrap()).is_none(),
        "respuesta perdida"
    );
    assert_eq!(sim.estado().registros.len(), 2, "el envío sí llegó");

    let r = respuesta(enviar(&sim, &e).1);
    assert_eq!(r.estado_envio, EstadoEnvio::Incorrecto);
    for l in &r.lineas {
        assert_eq!(l.estado, EstadoRegistro::Incorrecto);
        assert_eq!(l.codigo_error, Some(3000));
        let d = l.duplicado.as_ref().unwrap();
        assert_eq!(d.estado, "Correcta");
        assert!(d.id_peticion.starts_with("SIM"));
    }
    assert_eq!(sim.estado().registros.len(), 2);
}

#[test]
fn huella_incorrecta_se_acepta_con_errores() {
    let sim = simulador(60);
    let mut c = cadena();
    if let RegistroFacturacion::Alta(a) = &mut c[1] {
        a.importe_total = eur(99_999); // la huella ya no corresponde
    }
    let r = respuesta(enviar(&sim, &envio(c)).1);
    assert_eq!(r.estado_envio, EstadoEnvio::ParcialmenteCorrecto);
    assert_eq!(r.lineas[0].estado, EstadoRegistro::Correcto);
    assert_eq!(r.lineas[1].estado, EstadoRegistro::AceptadoConErrores);
    assert_eq!(r.lineas[1].codigo_error, Some(2000));
}

#[test]
fn errores_de_cabecera_y_limites_son_fault() {
    let sim = SimuladorAeat::iniciar(ConfigSimulador {
        censo: Some(vec!["B00000034".to_owned()]),
        ..ConfigSimulador::default()
    })
    .unwrap();
    let (codigo, r) = enviar(&sim, &envio(cadena()));
    assert_eq!(codigo, 500);
    match r {
        ResultadoEnvio::Fallo(f) => assert_eq!(f.codigo, Some(4104)),
        otro => panic!("{otro:?}"),
    }
    assert!(
        sim.estado().registros.is_empty(),
        "un Fault no registra nada"
    );

    // XML no parseable y más de 1000 registros (se construye a mano: `Envio` no lo permite).
    let (_, cuerpo) = post(&sim.url(), "<roto>").unwrap();
    assert!(
        matches!(ResultadoEnvio::desde_xml(&cuerpo).unwrap(), ResultadoEnvio::Fallo(f) if f.codigo == Some(4103))
    );
    let mut censado = envio(cadena()[..1].to_vec());
    censado.cabecera.obligado_emision.nif = "B00000034".to_owned();
    let soap = censado.a_soap().unwrap();
    let ini = soap.find("<sum:RegistroFactura>").unwrap();
    let fin = soap.find("</sum:RegFactuSistemaFacturacion>").unwrap();
    let uno = &soap[ini..fin];
    let enorme = format!("{}{}{}", &soap[..ini], uno.repeat(1001), &soap[fin..]);
    let (_, cuerpo) = post(&sim.url(), &enorme).unwrap();
    assert!(
        matches!(ResultadoEnvio::desde_xml(&cuerpo).unwrap(), ResultadoEnvio::Fallo(f) if f.codigo == Some(4114))
    );
}

#[test]
fn anulaciones() {
    let sim = simulador(0);
    let c = cadena();
    // Anular algo que no existe: 3002.
    let r = respuesta(enviar(&sim, &envio(vec![c[2].clone()])).1);
    assert_eq!(r.lineas[0].codigo_error, Some(3002));
    // Alta, anulación y segunda anulación: 3001.
    respuesta(enviar(&sim, &envio(c.clone())).1);
    let r = respuesta(enviar(&sim, &envio(vec![c[2].clone()])).1);
    assert_eq!(r.lineas[0].estado, EstadoRegistro::Incorrecto);
    assert_eq!(r.lineas[0].codigo_error, Some(3001));
}

#[test]
fn errores_forzados_y_fallo_http() {
    let mut cfg = ConfigSimulador::default();
    cfg.errores.insert(
        "A-1".to_owned(),
        ErrorForzado {
            codigo: 1100,
            descripcion: "Valor o tipo incorrecto del campo.".to_owned(),
            aceptado: false,
        },
    );
    let sim = SimuladorAeat::iniciar(cfg).unwrap();
    sim.estado().fallo_http_siguiente = Some(503);
    let (codigo, cuerpo) = post(&sim.url(), &envio(cadena()).a_soap().unwrap()).unwrap();
    assert_eq!(codigo, 503);
    assert!(
        ResultadoEnvio::desde_xml(&cuerpo).is_err(),
        "un 503 no es una respuesta de la AEAT"
    );
    assert!(sim.estado().registros.is_empty());

    let r = respuesta(enviar(&sim, &envio(cadena()[..2].to_vec())).1);
    assert_eq!(r.estado_envio, EstadoEnvio::ParcialmenteCorrecto);
    assert_eq!(r.lineas[0].estado, EstadoRegistro::Incorrecto);
    assert_eq!(r.lineas[0].codigo_error, Some(1100));
    assert_eq!(r.lineas[1].estado, EstadoRegistro::Correcto);
}

#[test]
fn cuenta_envios_anticipados() {
    let sim = simulador(3600);
    let c = cadena();
    respuesta(enviar(&sim, &envio(c[..1].to_vec())).1);
    respuesta(enviar(&sim, &envio(c[1..2].to_vec())).1);
    let e = sim.estado();
    assert_eq!(e.envios_procesados, 2);
    assert_eq!(e.envios_anticipados, 1);
}
