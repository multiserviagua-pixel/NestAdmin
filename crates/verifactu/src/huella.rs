//! Huella («hash») de los registros y verificación de la cadena.
//!
//! Especificación AEAT «Detalle de las especificaciones técnicas para generación de la
//! huella o hash de los registros de facturación», versión 0.1.2 (27/08/2024):
//!
//! - Campos en orden fijo, `nombre=valor` unidos por `&`, valores sin espacios al inicio ni
//!   al final; un campo ausente se escribe como `nombre=`.
//! - La cadena se codifica en UTF-8 y se aplica SHA-256 (lista L12, `TipoHuella = 01`).
//! - Salida en hexadecimal y mayúsculas, 64 caracteres.

use sha2::{Digest, Sha256};

use crate::registro::{Encadenamiento, RegistroFacturacion};
use crate::Error;

/// Campos de la huella de un registro de alta, ya con el texto que lleva el XML.
#[derive(Debug, Clone, Copy)]
pub struct CamposAlta<'a> {
    pub id_emisor_factura: &'a str,
    pub num_serie_factura: &'a str,
    pub fecha_expedicion_factura: &'a str,
    pub tipo_factura: &'a str,
    pub cuota_total: &'a str,
    pub importe_total: &'a str,
    pub huella_anterior: &'a str,
    pub fecha_hora_huso_gen_registro: &'a str,
}

/// Campos de la huella de un registro de anulación.
#[derive(Debug, Clone, Copy)]
pub struct CamposAnulacion<'a> {
    pub id_emisor_factura_anulada: &'a str,
    pub num_serie_factura_anulada: &'a str,
    pub fecha_expedicion_factura_anulada: &'a str,
    pub huella_anterior: &'a str,
    pub fecha_hora_huso_gen_registro: &'a str,
}

fn unir(campos: &[(&str, &str)]) -> String {
    campos
        .iter()
        .map(|(n, v)| format!("{n}={}", v.trim()))
        .collect::<Vec<_>>()
        .join("&")
}

pub fn cadena_alta(c: &CamposAlta<'_>) -> String {
    unir(&[
        ("IDEmisorFactura", c.id_emisor_factura),
        ("NumSerieFactura", c.num_serie_factura),
        ("FechaExpedicionFactura", c.fecha_expedicion_factura),
        ("TipoFactura", c.tipo_factura),
        ("CuotaTotal", c.cuota_total),
        ("ImporteTotal", c.importe_total),
        ("Huella", c.huella_anterior),
        ("FechaHoraHusoGenRegistro", c.fecha_hora_huso_gen_registro),
    ])
}

pub fn cadena_anulacion(c: &CamposAnulacion<'_>) -> String {
    unir(&[
        ("IDEmisorFacturaAnulada", c.id_emisor_factura_anulada),
        ("NumSerieFacturaAnulada", c.num_serie_factura_anulada),
        (
            "FechaExpedicionFacturaAnulada",
            c.fecha_expedicion_factura_anulada,
        ),
        ("Huella", c.huella_anterior),
        ("FechaHoraHusoGenRegistro", c.fecha_hora_huso_gen_registro),
    ])
}

/// SHA-256 de la cadena en UTF-8, en hexadecimal y mayúsculas.
pub fn sha256_hex(cadena: &str) -> String {
    let d = Sha256::digest(cadena.as_bytes());
    d.iter().map(|b| format!("{b:02X}")).collect()
}

/// `true` si el texto tiene la forma de una huella: 64 dígitos hexadecimales en mayúsculas.
pub fn es_huella(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|c| c.is_ascii_digit() || (b'A'..=b'F').contains(&c))
}

/// Fallo de la verificación de una cadena. `indice` es la posición en el slice verificado.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FalloCadena {
    #[error("la cadena está vacía")]
    Vacia,
    #[error(
        "registro {indice}: la huella guardada no coincide con la recalculada (registro alterado)"
    )]
    HuellaAlterada { indice: usize },
    #[error("registro {indice}: el encadenamiento no apunta al registro anterior (eslabón roto)")]
    EslabonRoto { indice: usize },
    #[error("registro {indice}: se declara primer registro pero no lo es")]
    PrimerRegistroInesperado { indice: usize },
    #[error("registro 0: no es el primer registro y no se indicó el eslabón anterior")]
    FaltaInicio,
}

/// Verifica una cadena de registros consecutivos de un mismo sistema.
///
/// `anterior` es el encadenamiento esperado del primer registro del slice: `None` si el slice
/// empieza en el primer registro del sistema (debe llevar `PrimerRegistro`), o el
/// [`RegistroFacturacion::siguiente_encadenamiento`] del último registro ya verificado.
/// Comprueba, para cada registro, que su huella es la recalculada y que su encadenamiento
/// identifica al registro anterior con su huella.
pub fn verificar_cadena(
    registros: &[RegistroFacturacion],
    anterior: Option<&Encadenamiento>,
) -> Result<(), FalloCadena> {
    if registros.is_empty() {
        return Err(FalloCadena::Vacia);
    }
    for (i, r) in registros.iter().enumerate() {
        if r.calcular_huella() != r.huella() {
            return Err(FalloCadena::HuellaAlterada { indice: i });
        }
        let esperado = if i == 0 {
            anterior.cloned()
        } else {
            Some(registros[i - 1].siguiente_encadenamiento())
        };
        match (esperado, r.encadenamiento()) {
            (None, Encadenamiento::PrimerRegistro) => {}
            (None, Encadenamiento::RegistroAnterior { .. }) => {
                return Err(FalloCadena::FaltaInicio)
            }
            (Some(_), Encadenamiento::PrimerRegistro) => {
                return Err(FalloCadena::PrimerRegistroInesperado { indice: i })
            }
            (Some(e), real) => {
                if &e != real {
                    return Err(FalloCadena::EslabonRoto { indice: i });
                }
            }
        }
    }
    Ok(())
}

/// Comprobación previa a generar un registro (Orden HAC/1177/2024, art. 7.i): el último
/// registro está bien formado y su fecha-hora no es posterior en más de un minuto a la que se
/// usará para el nuevo registro. El encadenamiento del último con el penúltimo se comprueba
/// con [`verificar_cadena`].
pub fn comprobar_antes_de_generar(
    ultimo: &RegistroFacturacion,
    nueva_fecha_hora: crate::FechaHoraHuso,
) -> Result<(), Error> {
    if ultimo.calcular_huella() != ultimo.huella() {
        return Err(Error::Cadena(FalloCadena::HuellaAlterada { indice: 0 }));
    }
    let previa = ultimo.fecha_hora_huso_gen_registro();
    if previa.unix() > nueva_fecha_hora.unix() + 60 {
        return Err(Error::RelojAtrasado {
            ultimo: previa.to_string(),
            nuevo: nueva_fecha_hora.to_string(),
        });
    }
    Ok(())
}
