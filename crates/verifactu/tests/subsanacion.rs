//! Subsanación (D253) y simulador sembrado (E5): campos del registro y del XML, y respuestas
//! del simulador local. Todo en 127.0.0.1 con datos sintéticos.

mod common;

use common::*;
use nestadmin_verifactu::simulador::{ConfigSimulador, SimuladorAeat};
use nestadmin_verifactu::*;

fn enviar(sim: &SimuladorAeat, regs: Vec<RegistroFacturacion>) -> RespuestaEnvio {
    let (_, cuerpo) = post(&sim.url(), &envio(regs).a_soap().unwrap()).expect("respuesta");
    match ResultadoEnvio::desde_xml(&cuerpo).unwrap() {
        ResultadoEnvio::Respuesta(r) => r,
        ResultadoEnvio::Fallo(f) => panic!("fallo inesperado: {f:?}"),
    }
}

fn simulador() -> SimuladorAeat {
    SimuladorAeat::iniciar(ConfigSimulador {
        tiempo_espera_envio: 0,
        ..ConfigSimulador::default()
    })
    .unwrap()
}

/// Alta sellada de subsanación (`Subsanacion=S`, `RechazoPrevio=X`) de la serie dada.
fn alta_subsanacion(serie: &str, enc: Encadenamiento) -> RegistroFacturacion {
    let mut a = alta(serie, enc, "2024-01-01T11:00:00+01:00");
    a.marcar_subsanacion_de_rechazado();
    a.sellar().unwrap();
    RegistroFacturacion::Alta(a)
}

#[test]
fn el_alta_de_subsanacion_lleva_los_campos_en_el_xml() {
    let r = alta_subsanacion("A-9", Encadenamiento::PrimerRegistro);
    let xml = envio(vec![r]).a_xml().unwrap();
    assert!(xml.contains(":Subsanacion>S</"), "{xml}");
    assert!(xml.contains(":RechazoPrevio>X</"), "{xml}");
    // Un alta normal no los lleva.
    let mut normal = alta(
        "A-8",
        Encadenamiento::PrimerRegistro,
        "2024-01-01T10:00:00+01:00",
    );
    normal.sellar().unwrap();
    let xml = envio(vec![RegistroFacturacion::Alta(normal)])
        .a_xml()
        .unwrap();
    assert!(!xml.contains("Subsanacion") && !xml.contains("RechazoPrevio"));
}

#[test]
fn la_anulacion_de_subsanacion_lleva_los_campos_en_el_xml() {
    let id = IdFactura::new(NIF_EMISOR, "A-9", fecha("01-01-2024"));
    let mut a = anulacion(
        id,
        Encadenamiento::PrimerRegistro,
        "2024-01-01T11:00:00+01:00",
    );
    a.marcar_subsanacion(true);
    a.sellar().unwrap();
    let xml = envio(vec![RegistroFacturacion::Anulacion(a)])
        .a_xml()
        .unwrap();
    assert!(xml.contains(":SinRegistroPrevio>S</"), "{xml}");
    assert!(xml.contains(":RechazoPrevio>S</"), "{xml}");
}

#[test]
fn rechazo_previo_sin_subsanacion_no_valida() {
    let mut a = alta(
        "A-7",
        Encadenamiento::PrimerRegistro,
        "2024-01-01T10:00:00+01:00",
    );
    a.rechazo_previo = Some(RechazoPrevio::X);
    assert!(matches!(a.validar(), Err(Error::Campo { campo, .. }) if campo == "RechazoPrevio"));
}

#[test]
fn el_simulador_acepta_la_subsanacion_con_rechazo_previo_x_sin_registro() {
    let sim = simulador();
    let r = enviar(
        &sim,
        vec![alta_subsanacion("A-9", Encadenamiento::PrimerRegistro)],
    );
    assert_eq!(r.lineas[0].estado, EstadoRegistro::Correcto);
    assert_eq!(r.lineas[0].codigo_error, None);
    // Ahora consta: un alta normal repetida es un duplicado.
    let mut otra = alta(
        "A-9",
        Encadenamiento::PrimerRegistro,
        "2024-01-01T12:00:00+01:00",
    );
    otra.sellar().unwrap();
    let r = enviar(&sim, vec![RegistroFacturacion::Alta(otra)]);
    assert_eq!(r.lineas[0].codigo_error, Some(3000));
}

#[test]
fn la_subsanacion_sin_rechazo_previo_x_y_sin_registro_sigue_dando_3002() {
    let sim = simulador();
    let mut a = alta(
        "A-5",
        Encadenamiento::PrimerRegistro,
        "2024-01-01T11:00:00+01:00",
    );
    a.subsanacion = true;
    a.rechazo_previo = Some(RechazoPrevio::S);
    a.sellar().unwrap();
    let r = enviar(&sim, vec![RegistroFacturacion::Alta(a)]);
    assert_eq!(r.lineas[0].estado, EstadoRegistro::Incorrecto);
    assert_eq!(r.lineas[0].codigo_error, Some(3002));
}

/// Una anulación reenviada recibe 3001 («ya dado de baja»), no un duplicado 3000: la
/// aplicación decide si es suya (ajuste 2).
#[test]
fn la_anulacion_reenviada_recibe_3001_sin_registro_duplicado() {
    let sim = simulador();
    let c = cadena();
    enviar(&sim, c.clone());
    let r = enviar(&sim, vec![c[2].clone()]);
    assert_eq!(r.lineas[0].estado, EstadoRegistro::Incorrecto);
    assert_eq!(r.lineas[0].codigo_error, Some(3001));
    assert!(r.lineas[0].duplicado.is_none());
}

#[test]
fn simulador_sembrado_con_registros_aceptados() {
    let sim = simulador();
    let c = cadena();
    // Se siembra lo que ya estaba aceptado antes de «reabrir»: dos altas y una anulación.
    sim.sembrar(&c);
    assert_eq!(sim.estado().envios_procesados, 0);
    // La anulación vuelve a darse de baja: 3001, no 3002 (falso rechazo por memoria perdida).
    let r = enviar(&sim, vec![c[2].clone()]);
    assert_eq!(r.lineas[0].codigo_error, Some(3001));
    // Un alta sembrada, reenviada, es un duplicado correcto.
    let r = enviar(&sim, vec![c[0].clone()]);
    assert_eq!(r.lineas[0].codigo_error, Some(3000));
    assert_eq!(r.lineas[0].duplicado.as_ref().unwrap().estado, "Correcta");
}

#[test]
fn sembrar_una_anulacion_sin_alta_la_deja_anulada() {
    let sim = simulador();
    let c = cadena();
    sim.sembrar(&c[2..]);
    let r = enviar(&sim, vec![c[2].clone()]);
    assert_eq!(r.lineas[0].codigo_error, Some(3001));
}
