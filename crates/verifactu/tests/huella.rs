//! Vectores oficiales de la huella (especificación AEAT v0.1.2, apartado 6) y verificación
//! de cadenas.

mod common;

use common::*;
use nestadmin_verifactu::huella::{
    cadena_alta, cadena_anulacion, sha256_hex, CamposAlta, CamposAnulacion,
};
use nestadmin_verifactu::*;

const H1: &str = "3C464DAF61ACB827C65FDA19F352A4E3BDC2C640E9E9FC4CC058073F38F12F60";
const H2: &str = "F7B94CFD8924EDFF273501B01EE5153E4CE8F259766F88CF6ACB8935802A2B97";
const H3: &str = "177547C0D57AC74748561D054A9CEC14B4C4EA23D1BEFD6F2E69E3A388F90C68";

#[test]
fn vectores_oficiales_por_campos() {
    let c1 = cadena_alta(&CamposAlta {
        id_emisor_factura: "89890001K",
        num_serie_factura: "12345678/G33",
        fecha_expedicion_factura: "01-01-2024",
        tipo_factura: "F1",
        cuota_total: "12.35",
        importe_total: "123.45",
        huella_anterior: "",
        fecha_hora_huso_gen_registro: "2024-01-01T19:20:30+01:00",
    });
    assert_eq!(
        c1,
        "IDEmisorFactura=89890001K&NumSerieFactura=12345678/G33&FechaExpedicionFactura=01-01-2024&TipoFactura=F1&CuotaTotal=12.35&ImporteTotal=123.45&Huella=&FechaHoraHusoGenRegistro=2024-01-01T19:20:30+01:00"
    );
    assert_eq!(sha256_hex(&c1), H1);

    let c2 = cadena_alta(&CamposAlta {
        id_emisor_factura: "89890001K",
        num_serie_factura: "12345679/G34",
        fecha_expedicion_factura: "01-01-2024",
        tipo_factura: "F1",
        cuota_total: "12.35",
        importe_total: "123.45",
        huella_anterior: H1,
        fecha_hora_huso_gen_registro: "2024-01-01T19:20:35+01:00",
    });
    assert_eq!(sha256_hex(&c2), H2);

    let c3 = cadena_anulacion(&CamposAnulacion {
        id_emisor_factura_anulada: "89890001K",
        num_serie_factura_anulada: "12345679/G34",
        fecha_expedicion_factura_anulada: "01-01-2024",
        huella_anterior: H2,
        fecha_hora_huso_gen_registro: "2024-01-01T19:20:40+01:00",
    });
    assert_eq!(
        c3,
        format!("IDEmisorFacturaAnulada=89890001K&NumSerieFacturaAnulada=12345679/G34&FechaExpedicionFacturaAnulada=01-01-2024&Huella={H2}&FechaHoraHusoGenRegistro=2024-01-01T19:20:40+01:00")
    );
    assert_eq!(sha256_hex(&c3), H3);
}

#[test]
fn espacios_al_inicio_y_al_final_no_cuentan() {
    let base = CamposAlta {
        id_emisor_factura: "89890001K",
        num_serie_factura: "12345678/G33",
        fecha_expedicion_factura: "01-01-2024",
        tipo_factura: "F1",
        cuota_total: "12.35",
        importe_total: "123.45",
        huella_anterior: "",
        fecha_hora_huso_gen_registro: "2024-01-01T19:20:30+01:00",
    };
    let con_espacios = CamposAlta {
        num_serie_factura: "  12345678/G33 ",
        ..base
    };
    assert_eq!(sha256_hex(&cadena_alta(&con_espacios)), H1);
}

/// Los mismos vectores con los tipos del crate: importes, fechas y huso con su formato.
#[test]
fn vectores_oficiales_con_tipos() {
    let mut r1 = alta(
        "12345678/G33",
        Encadenamiento::PrimerRegistro,
        "2024-01-01T19:20:30+01:00",
    );
    r1.cuota_total = eur(1_235);
    r1.importe_total = eur(12_345);
    r1.sellar().unwrap();
    assert_eq!(r1.huella, H1);
    let r1 = RegistroFacturacion::Alta(r1);

    let mut r2 = alta(
        "12345679/G34",
        r1.siguiente_encadenamiento(),
        "2024-01-01T19:20:35+01:00",
    );
    r2.cuota_total = eur(1_235);
    r2.importe_total = eur(12_345);
    r2.sellar().unwrap();
    assert_eq!(r2.huella, H2);
    let r2 = RegistroFacturacion::Alta(r2);

    let mut r3 = anulacion(
        r2.id_factura().clone(),
        r2.siguiente_encadenamiento(),
        "2024-01-01T19:20:40+01:00",
    );
    r3.sellar().unwrap();
    assert_eq!(r3.huella, H3);

    let cadena = vec![r1, r2, RegistroFacturacion::Anulacion(r3)];
    verificar_cadena(&cadena, None).unwrap();
}

#[test]
fn cadena_valida_y_por_tramos() {
    let c = cadena();
    verificar_cadena(&c, None).unwrap();
    // Un tramo intermedio se verifica dando el eslabón del último registro ya comprobado.
    verificar_cadena(&c[1..], Some(&c[0].siguiente_encadenamiento())).unwrap();
    assert_eq!(
        verificar_cadena(&c[1..], None),
        Err(FalloCadena::FaltaInicio)
    );
    assert_eq!(verificar_cadena(&[], None), Err(FalloCadena::Vacia));
}

#[test]
fn detecta_registro_alterado() {
    let mut c = cadena();
    if let RegistroFacturacion::Alta(a) = &mut c[1] {
        a.importe_total = eur(12_101);
    }
    assert_eq!(
        verificar_cadena(&c, None),
        Err(FalloCadena::HuellaAlterada { indice: 1 })
    );
}

#[test]
fn detecta_eslabon_roto() {
    let mut c = cadena();
    // Se rehace el registro 2 apuntando a otra huella: su propia huella es coherente, pero
    // ya no enlaza con el registro 1.
    if let RegistroFacturacion::Alta(a) = &mut c[1] {
        a.encadenamiento = Encadenamiento::RegistroAnterior {
            id_factura: a.id_factura.clone(),
            huella: H1.to_owned(),
        };
        a.sellar().unwrap();
    }
    assert_eq!(
        verificar_cadena(&c, None),
        Err(FalloCadena::EslabonRoto { indice: 1 })
    );

    // Eliminar un registro también rompe la cadena.
    let c = cadena();
    let sin_el_segundo = vec![c[0].clone(), c[2].clone()];
    assert_eq!(
        verificar_cadena(&sin_el_segundo, None),
        Err(FalloCadena::EslabonRoto { indice: 1 })
    );
}

#[test]
fn primer_registro_solo_al_principio() {
    let c = cadena();
    let mut otro = alta(
        "B-1",
        Encadenamiento::PrimerRegistro,
        "2024-01-01T11:00:00+01:00",
    );
    otro.sellar().unwrap();
    let c2 = vec![c[0].clone(), RegistroFacturacion::Alta(otro)];
    assert_eq!(
        verificar_cadena(&c2, None),
        Err(FalloCadena::PrimerRegistroInesperado { indice: 1 })
    );
}

#[test]
fn comprobacion_previa_a_generar() {
    let c = cadena();
    let ultimo = &c[2]; // 10:10:00+01:00
    comprobar_antes_de_generar(ultimo, fh("2024-01-01T10:10:00+01:00")).unwrap();
    // Un minuto antes se admite; más de un minuto, no (Orden HAC/1177/2024, art. 7.i).
    comprobar_antes_de_generar(ultimo, fh("2024-01-01T10:09:00+01:00")).unwrap();
    assert!(matches!(
        comprobar_antes_de_generar(ultimo, fh("2024-01-01T10:08:59+01:00")),
        Err(Error::RelojAtrasado { .. })
    ));
    // Mismo instante expresado en otro huso.
    comprobar_antes_de_generar(ultimo, fh("2024-01-01T09:10:00+00:00")).unwrap();

    let mut alterado = c[2].clone();
    if let RegistroFacturacion::Anulacion(a) = &mut alterado {
        a.huella = H1.to_owned();
    }
    assert!(matches!(
        comprobar_antes_de_generar(&alterado, fh("2024-01-01T11:00:00+01:00")),
        Err(Error::Cadena(FalloCadena::HuellaAlterada { .. }))
    ));
}

#[test]
fn validaciones_de_formato() {
    let mut r = alta(
        "A-1",
        Encadenamiento::PrimerRegistro,
        "2024-01-01T10:00:00+01:00",
    );
    r.tipo_factura = TipoFactura::R1;
    assert!(matches!(r.sellar(), Err(Error::Campo { campo, .. }) if campo == "TipoRectificativa"));
    r.tipo_rectificativa = Some(TipoRectificativa::Diferencias);
    r.sellar().unwrap();

    let mut r = alta(
        "A-1",
        Encadenamiento::PrimerRegistro,
        "2024-01-01T10:00:00+01:00",
    );
    r.desglose.clear();
    assert!(r.sellar().is_err());

    let mut r = alta(
        "Serie-ñ",
        Encadenamiento::PrimerRegistro,
        "2024-01-01T10:00:00+01:00",
    );
    assert!(r.sellar().is_err(), "numserie fuera de ASCII imprimible");

    let mut r = alta(
        "A-1",
        Encadenamiento::RegistroAnterior {
            id_factura: IdFactura::new(NIF_EMISOR, "A-0", fecha("01-01-2024")),
            huella: "abc".to_owned(),
        },
        "2024-01-01T10:00:00+01:00",
    );
    assert!(r.sellar().is_err(), "huella anterior mal formada");
}
