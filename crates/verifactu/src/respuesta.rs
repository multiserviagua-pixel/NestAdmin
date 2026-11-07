//! Respuesta del servicio (`RespuestaSuministro.xsd`) o `Fault` SOAP.
//!
//! - Error de esquema o de cabecera: `Fault` SOAP, con el código en `faultstring` como
//!   `Codigo[4104].Descripción` (servicio web 1.0.3, apartado 5.1).
//! - En otro caso, `RespuestaRegFactuSistemaFacturacion` con `EstadoEnvio` global
//!   (`Correcto`, `ParcialmenteCorrecto`, `Incorrecto`), `TiempoEsperaEnvio` y una
//!   `RespuestaLinea` por registro (`Correcto`, `AceptadoConErrores`, `Incorrecto`).

use serde::{Deserialize, Serialize};

use crate::dom::{self, Nodo};
use crate::registro::IdFactura;
use crate::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EstadoEnvio {
    Correcto,
    ParcialmenteCorrecto,
    Incorrecto,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EstadoRegistro {
    Correcto,
    AceptadoConErrores,
    Incorrecto,
}

impl EstadoRegistro {
    /// El registro consta en la AEAT (aunque sea con errores).
    pub fn aceptado(&self) -> bool {
        !matches!(self, Self::Incorrecto)
    }
}

/// Datos del registro ya existente cuando un alta se rechaza por duplicada (código 3000).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistroDuplicado {
    pub id_peticion: String,
    /// `Correcta`, `AceptadaConErrores` o `Anulada`.
    pub estado: String,
    pub codigo_error: Option<u32>,
    pub descripcion_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RespuestaLinea {
    pub id_factura: IdFactura,
    /// `Alta` o `Anulacion`.
    pub tipo_operacion: String,
    pub ref_externa: Option<String>,
    pub estado: EstadoRegistro,
    pub codigo_error: Option<u32>,
    pub descripcion_error: Option<String>,
    pub duplicado: Option<RegistroDuplicado>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RespuestaEnvio {
    /// Código seguro de verificación del envío. La AEAT no permite recuperarlo después:
    /// hay que guardarlo al recibirlo.
    pub csv: Option<String>,
    pub nif_presentador: Option<String>,
    /// `TimestampPresentacion` tal como llega (huso de los servidores de la AEAT).
    pub timestamp_presentacion: Option<String>,
    /// Segundos que hay que esperar antes del siguiente envío (salvo que se acumulen 1000
    /// registros antes).
    pub tiempo_espera_envio: u32,
    pub estado_envio: EstadoEnvio,
    pub lineas: Vec<RespuestaLinea>,
}

/// `Fault` SOAP. `codigo` es el número de `Codigo[NNNN]` si `faultstring` lo trae.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FalloSoap {
    pub faultcode: String,
    pub faultstring: String,
    pub codigo: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResultadoEnvio {
    Respuesta(RespuestaEnvio),
    Fallo(FalloSoap),
}

impl ResultadoEnvio {
    /// Lee un sobre SOAP o el elemento `RespuestaRegFactuSistemaFacturacion` suelto.
    pub fn desde_xml(xml: &str) -> Result<Self, Error> {
        let raiz = dom::parsear(xml)?;
        if let Some(f) = raiz.buscar("Fault") {
            let faultcode = f.texto_de("faultcode").unwrap_or_default();
            let faultstring = f.texto_de("faultstring").unwrap_or_default();
            let codigo = codigo_en_texto(&faultstring);
            return Ok(Self::Fallo(FalloSoap {
                faultcode,
                faultstring,
                codigo,
            }));
        }
        let r = raiz
            .buscar("RespuestaRegFactuSistemaFacturacion")
            .ok_or_else(|| {
                Error::RespuestaIncompleta("RespuestaRegFactuSistemaFacturacion".into())
            })?;
        Ok(Self::Respuesta(leer_respuesta(r)?))
    }
}

fn obligatorio(n: &Nodo, campo: &str) -> Result<String, Error> {
    n.texto_de(campo)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| Error::RespuestaIncompleta(campo.to_owned()))
}

fn numero(s: Option<String>, campo: &str) -> Result<Option<u32>, Error> {
    match s {
        None => Ok(None),
        Some(s) if s.is_empty() => Ok(None),
        Some(s) => s
            .parse()
            .map(Some)
            .map_err(|_| Error::Formato(format!("{campo} «{s}» no es numérico"))),
    }
}

fn leer_respuesta(r: &Nodo) -> Result<RespuestaEnvio, Error> {
    let estado_envio = match obligatorio(r, "EstadoEnvio")?.as_str() {
        "Correcto" => EstadoEnvio::Correcto,
        "ParcialmenteCorrecto" => EstadoEnvio::ParcialmenteCorrecto,
        "Incorrecto" => EstadoEnvio::Incorrecto,
        otro => return Err(Error::Formato(format!("EstadoEnvio «{otro}»"))),
    };
    // El elemento es obligatorio; su tipo (`Tipo6Type`, `\d{0,4}`) admite el valor vacío, que
    // se trata como el tiempo inicial de 60 s en lugar de como «sin espera».
    let te = r
        .texto_de("TiempoEsperaEnvio")
        .ok_or_else(|| Error::RespuestaIncompleta("TiempoEsperaEnvio".into()))?;
    let tiempo_espera_envio =
        numero(Some(te), "TiempoEsperaEnvio")?.unwrap_or(crate::TIEMPO_ESPERA_INICIAL_S);
    let datos = r.hijo("DatosPresentacion");
    let mut lineas = Vec::new();
    for l in r.hijos("RespuestaLinea") {
        lineas.push(leer_linea(l)?);
    }
    Ok(RespuestaEnvio {
        csv: r.texto_de("CSV").filter(|s| !s.is_empty()),
        nif_presentador: datos.and_then(|d| d.texto_de("NIFPresentador")),
        timestamp_presentacion: datos.and_then(|d| d.texto_de("TimestampPresentacion")),
        tiempo_espera_envio,
        estado_envio,
        lineas,
    })
}

fn leer_linea(l: &Nodo) -> Result<RespuestaLinea, Error> {
    let id = l
        .hijo("IDFactura")
        .ok_or_else(|| Error::RespuestaIncompleta("RespuestaLinea/IDFactura".into()))?;
    let id_factura = IdFactura {
        nif_emisor: obligatorio(id, "IDEmisorFactura")?,
        num_serie: obligatorio(id, "NumSerieFactura")?,
        fecha_expedicion: obligatorio(id, "FechaExpedicionFactura")?.parse()?,
    };
    let tipo_operacion = l
        .hijo("Operacion")
        .and_then(|o| o.texto_de("TipoOperacion"))
        .ok_or_else(|| Error::RespuestaIncompleta("RespuestaLinea/Operacion".into()))?;
    let estado = match obligatorio(l, "EstadoRegistro")?.as_str() {
        "Correcto" => EstadoRegistro::Correcto,
        "AceptadoConErrores" => EstadoRegistro::AceptadoConErrores,
        "Incorrecto" => EstadoRegistro::Incorrecto,
        otro => return Err(Error::Formato(format!("EstadoRegistro «{otro}»"))),
    };
    let duplicado = match l.hijo("RegistroDuplicado") {
        None => None,
        Some(d) => Some(RegistroDuplicado {
            id_peticion: obligatorio(d, "IdPeticionRegistroDuplicado")?,
            estado: obligatorio(d, "EstadoRegistroDuplicado")?,
            codigo_error: numero(d.texto_de("CodigoErrorRegistro"), "CodigoErrorRegistro")?,
            descripcion_error: d.texto_de("DescripcionErrorRegistro"),
        }),
    };
    Ok(RespuestaLinea {
        id_factura,
        tipo_operacion,
        ref_externa: l.texto_de("RefExterna"),
        estado,
        codigo_error: numero(l.texto_de("CodigoErrorRegistro"), "CodigoErrorRegistro")?,
        descripcion_error: l.texto_de("DescripcionErrorRegistro"),
        duplicado,
    })
}

fn codigo_en_texto(s: &str) -> Option<u32> {
    let i = s.find("Codigo[")? + "Codigo[".len();
    let fin = s[i..].find(']')? + i;
    s[i..fin].trim().parse().ok()
}
