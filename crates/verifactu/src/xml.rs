//! XML del envío `RegFactuSistemaFacturacion` (`SuministroLR.xsd`) y su sobre SOAP 1.1.

use std::io::Cursor;

use quick_xml::events::{BytesDecl, BytesEnd, BytesStart, BytesText, Event};
use quick_xml::Writer;
use serde::{Deserialize, Serialize};

use crate::registro::{
    Calificacion, DetalleDesglose, Encadenamiento, IdFactura, Identificacion, Persona,
    RegistroAlta, RegistroAnulacion, RegistroFacturacion, SistemaInformatico, ID_VERSION,
    TIPO_HUELLA_SHA256,
};
use crate::tipos::{validar_nif, validar_texto, Fecha};
use crate::{Error, MAX_REGISTROS_POR_ENVIO};

pub const NS_SOAP: &str = "http://schemas.xmlsoap.org/soap/envelope/";
pub const NS_SUMINISTRO_LR: &str = "https://www2.agenciatributaria.gob.es/static_files/common/internet/dep/aplicaciones/es/aeat/tike/cont/ws/SuministroLR.xsd";
pub const NS_SUMINISTRO_INFORMACION: &str = "https://www2.agenciatributaria.gob.es/static_files/common/internet/dep/aplicaciones/es/aeat/tike/cont/ws/SuministroInformacion.xsd";
pub const NS_RESPUESTA_SUMINISTRO: &str = "https://www2.agenciatributaria.gob.es/static_files/common/internet/dep/aplicaciones/es/aeat/tike/cont/ws/RespuestaSuministro.xsd";

/// Persona con NIF español (`PersonaFisicaJuridicaESType`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersonaEs {
    pub nombre_razon: String,
    pub nif: String,
}

impl PersonaEs {
    pub fn new(nombre_razon: &str, nif: &str) -> Self {
        Self {
            nombre_razon: nombre_razon.to_owned(),
            nif: nif.to_owned(),
        }
    }

    fn validar(&self, campo: &str) -> Result<(), Error> {
        validar_texto(
            &self.nombre_razon,
            &format!("{campo}/NombreRazon"),
            120,
            true,
        )?;
        validar_nif(&self.nif, &format!("{campo}/NIF"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RemisionVoluntaria {
    pub fecha_fin_verifactu: Option<Fecha>,
    pub incidencia: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemisionRequerimiento {
    pub ref_requerimiento: String,
    pub fin_requerimiento: Option<bool>,
}

/// Cabecera del envío (`CabeceraType`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cabecera {
    pub obligado_emision: PersonaEs,
    pub representante: Option<PersonaEs>,
    pub remision_voluntaria: Option<RemisionVoluntaria>,
    pub remision_requerimiento: Option<RemisionRequerimiento>,
}

impl Cabecera {
    pub fn new(obligado_emision: PersonaEs) -> Self {
        Self {
            obligado_emision,
            representante: None,
            remision_voluntaria: None,
            remision_requerimiento: None,
        }
    }
}

/// Un envío: cabecera y de 1 a 1000 registros de alta o anulación.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Envio {
    pub cabecera: Cabecera,
    pub registros: Vec<RegistroFacturacion>,
}

impl Envio {
    pub fn validar(&self) -> Result<(), Error> {
        let n = self.registros.len();
        if n == 0 || n > MAX_REGISTROS_POR_ENVIO {
            return Err(Error::NumeroRegistros(n));
        }
        self.cabecera
            .obligado_emision
            .validar("Cabecera/ObligadoEmision")?;
        if let Some(r) = &self.cabecera.representante {
            r.validar("Cabecera/Representante")?;
        }
        if let Some(r) = &self.cabecera.remision_requerimiento {
            validar_texto(&r.ref_requerimiento, "Cabecera/RefRequerimiento", 18, true)?;
        }
        for (i, r) in self.registros.iter().enumerate() {
            r.validar().map_err(|e| match e {
                Error::Campo { campo, motivo } => Error::Campo {
                    campo: format!("RegistroFactura[{i}]/{campo}"),
                    motivo,
                },
                otro => otro,
            })?;
            if !crate::huella::es_huella(r.huella()) {
                return Err(Error::Campo {
                    campo: format!("RegistroFactura[{i}]/Huella"),
                    motivo: "registro sin sellar".to_owned(),
                });
            }
        }
        Ok(())
    }

    /// Documento con `RegFactuSistemaFacturacion` como raíz (el que valida el XSD).
    pub fn a_xml(&self) -> Result<String, Error> {
        self.validar()?;
        escribir(|w| {
            w.write_event(Event::Decl(BytesDecl::new("1.0", Some("UTF-8"), None)))?;
            self.escribir_cuerpo(w, true)
        })
    }

    /// Sobre SOAP 1.1 listo para `POST` al servicio `VerifactuSOAP`.
    pub fn a_soap(&self) -> Result<String, Error> {
        self.validar()?;
        escribir(|w| {
            w.write_event(Event::Decl(BytesDecl::new("1.0", Some("UTF-8"), None)))?;
            let mut env = BytesStart::new("soapenv:Envelope");
            env.push_attribute(("xmlns:soapenv", NS_SOAP));
            env.push_attribute(("xmlns:sum", NS_SUMINISTRO_LR));
            env.push_attribute(("xmlns:sum1", NS_SUMINISTRO_INFORMACION));
            w.write_event(Event::Start(env))?;
            w.write_event(Event::Empty(BytesStart::new("soapenv:Header")))?;
            abrir(w, "soapenv:Body")?;
            self.escribir_cuerpo(w, false)?;
            cerrar(w, "soapenv:Body")?;
            cerrar(w, "soapenv:Envelope")
        })
    }

    fn escribir_cuerpo(&self, w: &mut W, declarar_ns: bool) -> std::io::Result<()> {
        let mut raiz = BytesStart::new("sum:RegFactuSistemaFacturacion");
        if declarar_ns {
            raiz.push_attribute(("xmlns:sum", NS_SUMINISTRO_LR));
            raiz.push_attribute(("xmlns:sum1", NS_SUMINISTRO_INFORMACION));
        }
        w.write_event(Event::Start(raiz))?;
        escribir_cabecera(w, &self.cabecera)?;
        for r in &self.registros {
            abrir(w, "sum:RegistroFactura")?;
            match r {
                RegistroFacturacion::Alta(a) => escribir_alta(w, a)?,
                RegistroFacturacion::Anulacion(a) => escribir_anulacion(w, a)?,
            }
            cerrar(w, "sum:RegistroFactura")?;
        }
        cerrar(w, "sum:RegFactuSistemaFacturacion")
    }
}

type W = Writer<Cursor<Vec<u8>>>;

fn escribir(f: impl FnOnce(&mut W) -> std::io::Result<()>) -> Result<String, Error> {
    let mut w = Writer::new_with_indent(Cursor::new(Vec::new()), b' ', 2);
    f(&mut w).map_err(|e| Error::Xml(e.to_string()))?;
    String::from_utf8(w.into_inner().into_inner()).map_err(|e| Error::Xml(e.to_string()))
}

fn abrir(w: &mut W, n: &str) -> std::io::Result<()> {
    w.write_event(Event::Start(BytesStart::new(n)))
}

fn cerrar(w: &mut W, n: &str) -> std::io::Result<()> {
    w.write_event(Event::End(BytesEnd::new(n)))
}

/// Elemento `sum1:<n>` con texto escapado.
fn campo(w: &mut W, n: &str, texto: &str) -> std::io::Result<()> {
    let nombre = format!("sum1:{n}");
    abrir(w, &nombre)?;
    w.write_event(Event::Text(BytesText::new(texto)))?;
    cerrar(w, &nombre)
}

fn campo_opt<T: ToString>(w: &mut W, n: &str, v: &Option<T>) -> std::io::Result<()> {
    match v {
        Some(v) => campo(w, n, &v.to_string()),
        None => Ok(()),
    }
}

fn sn(v: bool) -> &'static str {
    if v {
        "S"
    } else {
        "N"
    }
}

fn bloque(
    w: &mut W,
    n: &str,
    f: impl FnOnce(&mut W) -> std::io::Result<()>,
) -> std::io::Result<()> {
    let nombre = format!("sum1:{n}");
    abrir(w, &nombre)?;
    f(w)?;
    cerrar(w, &nombre)
}

fn escribir_persona_es(w: &mut W, n: &str, p: &PersonaEs) -> std::io::Result<()> {
    bloque(w, n, |w| {
        campo(w, "NombreRazon", &p.nombre_razon)?;
        campo(w, "NIF", &p.nif)
    })
}

fn escribir_identificacion(w: &mut W, id: &Identificacion) -> std::io::Result<()> {
    match id {
        Identificacion::Nif(n) => campo(w, "NIF", n),
        Identificacion::Otro {
            codigo_pais,
            id_type,
            id,
        } => bloque(w, "IDOtro", |w| {
            campo_opt(w, "CodigoPais", codigo_pais)?;
            campo(w, "IDType", id_type)?;
            campo(w, "ID", id)
        }),
    }
}

fn escribir_persona(w: &mut W, n: &str, p: &Persona) -> std::io::Result<()> {
    bloque(w, n, |w| {
        campo(w, "NombreRazon", &p.nombre_razon)?;
        escribir_identificacion(w, &p.identificacion)
    })
}

fn escribir_cabecera(w: &mut W, c: &Cabecera) -> std::io::Result<()> {
    abrir(w, "sum:Cabecera")?;
    escribir_persona_es(w, "ObligadoEmision", &c.obligado_emision)?;
    if let Some(r) = &c.representante {
        escribir_persona_es(w, "Representante", r)?;
    }
    if let Some(r) = &c.remision_voluntaria {
        bloque(w, "RemisionVoluntaria", |w| {
            campo_opt(w, "FechaFinVeriFactu", &r.fecha_fin_verifactu)?;
            campo_opt(w, "Incidencia", &r.incidencia.map(sn))
        })?;
    }
    if let Some(r) = &c.remision_requerimiento {
        bloque(w, "RemisionRequerimiento", |w| {
            campo(w, "RefRequerimiento", &r.ref_requerimiento)?;
            campo_opt(w, "FinRequerimiento", &r.fin_requerimiento.map(sn))
        })?;
    }
    cerrar(w, "sum:Cabecera")
}

fn escribir_id_factura(w: &mut W, n: &str, id: &IdFactura) -> std::io::Result<()> {
    bloque(w, n, |w| {
        campo(w, "IDEmisorFactura", &id.nif_emisor)?;
        campo(w, "NumSerieFactura", &id.num_serie)?;
        campo(
            w,
            "FechaExpedicionFactura",
            &id.fecha_expedicion.to_string(),
        )
    })
}

fn escribir_encadenamiento(w: &mut W, e: &Encadenamiento) -> std::io::Result<()> {
    bloque(w, "Encadenamiento", |w| match e {
        Encadenamiento::PrimerRegistro => campo(w, "PrimerRegistro", "S"),
        Encadenamiento::RegistroAnterior { id_factura, huella } => {
            bloque(w, "RegistroAnterior", |w| {
                campo(w, "IDEmisorFactura", &id_factura.nif_emisor)?;
                campo(w, "NumSerieFactura", &id_factura.num_serie)?;
                campo(
                    w,
                    "FechaExpedicionFactura",
                    &id_factura.fecha_expedicion.to_string(),
                )?;
                campo(w, "Huella", huella)
            })
        }
    })
}

fn escribir_sistema(w: &mut W, s: &SistemaInformatico) -> std::io::Result<()> {
    bloque(w, "SistemaInformatico", |w| {
        campo(w, "NombreRazon", &s.productor.nombre_razon)?;
        escribir_identificacion(w, &s.productor.identificacion)?;
        campo(w, "NombreSistemaInformatico", &s.nombre_sistema)?;
        campo(w, "IdSistemaInformatico", &s.id_sistema)?;
        campo(w, "Version", &s.version)?;
        campo(w, "NumeroInstalacion", &s.numero_instalacion)?;
        campo(w, "TipoUsoPosibleSoloVerifactu", sn(s.solo_verifactu))?;
        campo(w, "TipoUsoPosibleMultiOT", sn(s.multi_ot))?;
        campo(w, "IndicadorMultiplesOT", sn(s.indicador_multiples_ot))
    })
}

fn escribir_detalle(w: &mut W, d: &DetalleDesglose) -> std::io::Result<()> {
    bloque(w, "DetalleDesglose", |w| {
        campo_opt(w, "Impuesto", &d.impuesto.map(|i| i.as_str()))?;
        campo_opt(w, "ClaveRegimen", &d.clave_regimen)?;
        match d.calificacion {
            Calificacion::S1 => campo(w, "CalificacionOperacion", "S1")?,
            Calificacion::S2 => campo(w, "CalificacionOperacion", "S2")?,
            Calificacion::N1 => campo(w, "CalificacionOperacion", "N1")?,
            Calificacion::N2 => campo(w, "CalificacionOperacion", "N2")?,
            Calificacion::Exenta(n) => campo(w, "OperacionExenta", &format!("E{n}"))?,
        }
        campo_opt(w, "TipoImpositivo", &d.tipo_impositivo)?;
        campo(
            w,
            "BaseImponibleOimporteNoSujeto",
            &d.base_imponible.to_string(),
        )?;
        campo_opt(w, "BaseImponibleACoste", &d.base_imponible_a_coste)?;
        campo_opt(w, "CuotaRepercutida", &d.cuota_repercutida)?;
        campo_opt(w, "TipoRecargoEquivalencia", &d.tipo_recargo_equivalencia)?;
        campo_opt(w, "CuotaRecargoEquivalencia", &d.cuota_recargo_equivalencia)
    })
}

fn escribir_alta(w: &mut W, r: &RegistroAlta) -> std::io::Result<()> {
    bloque(w, "RegistroAlta", |w| {
        campo(w, "IDVersion", ID_VERSION)?;
        escribir_id_factura(w, "IDFactura", &r.id_factura)?;
        campo_opt(w, "RefExterna", &r.ref_externa)?;
        campo(w, "NombreRazonEmisor", &r.nombre_razon_emisor)?;
        if r.subsanacion {
            campo(w, "Subsanacion", "S")?;
        }
        campo_opt(w, "RechazoPrevio", &r.rechazo_previo.map(|x| x.as_str()))?;
        campo(w, "TipoFactura", r.tipo_factura.as_str())?;
        campo_opt(
            w,
            "TipoRectificativa",
            &r.tipo_rectificativa.map(|x| x.as_str()),
        )?;
        if !r.facturas_rectificadas.is_empty() {
            bloque(w, "FacturasRectificadas", |w| {
                for f in &r.facturas_rectificadas {
                    escribir_id_factura(w, "IDFacturaRectificada", f)?;
                }
                Ok(())
            })?;
        }
        if !r.facturas_sustituidas.is_empty() {
            bloque(w, "FacturasSustituidas", |w| {
                for f in &r.facturas_sustituidas {
                    escribir_id_factura(w, "IDFacturaSustituida", f)?;
                }
                Ok(())
            })?;
        }
        if let Some(i) = &r.importe_rectificacion {
            bloque(w, "ImporteRectificacion", |w| {
                campo(w, "BaseRectificada", &i.base_rectificada.to_string())?;
                campo(w, "CuotaRectificada", &i.cuota_rectificada.to_string())?;
                campo_opt(w, "CuotaRecargoRectificado", &i.cuota_recargo_rectificado)
            })?;
        }
        campo_opt(w, "FechaOperacion", &r.fecha_operacion)?;
        campo(w, "DescripcionOperacion", &r.descripcion_operacion)?;
        campo_opt(
            w,
            "FacturaSimplificadaArt7273",
            &r.factura_simplificada_art7273.map(sn),
        )?;
        campo_opt(
            w,
            "FacturaSinIdentifDestinatarioArt61d",
            &r.sin_identif_destinatario_art61d.map(sn),
        )?;
        campo_opt(w, "Macrodato", &r.macrodato.map(sn))?;
        campo_opt(
            w,
            "EmitidaPorTerceroODestinatario",
            &r.emitida_por_tercero_o_destinatario,
        )?;
        if let Some(t) = &r.tercero {
            escribir_persona(w, "Tercero", t)?;
        }
        if !r.destinatarios.is_empty() {
            bloque(w, "Destinatarios", |w| {
                for d in &r.destinatarios {
                    escribir_persona(w, "IDDestinatario", d)?;
                }
                Ok(())
            })?;
        }
        campo_opt(w, "Cupon", &r.cupon.map(sn))?;
        bloque(w, "Desglose", |w| {
            for d in &r.desglose {
                escribir_detalle(w, d)?;
            }
            Ok(())
        })?;
        campo(w, "CuotaTotal", &r.cuota_total.to_string())?;
        campo(w, "ImporteTotal", &r.importe_total.to_string())?;
        escribir_encadenamiento(w, &r.encadenamiento)?;
        escribir_sistema(w, &r.sistema_informatico)?;
        campo(
            w,
            "FechaHoraHusoGenRegistro",
            &r.fecha_hora_huso_gen_registro.to_string(),
        )?;
        campo(w, "TipoHuella", TIPO_HUELLA_SHA256)?;
        campo(w, "Huella", &r.huella)
    })
}

fn escribir_anulacion(w: &mut W, r: &RegistroAnulacion) -> std::io::Result<()> {
    bloque(w, "RegistroAnulacion", |w| {
        campo(w, "IDVersion", ID_VERSION)?;
        bloque(w, "IDFactura", |w| {
            campo(w, "IDEmisorFacturaAnulada", &r.id_factura.nif_emisor)?;
            campo(w, "NumSerieFacturaAnulada", &r.id_factura.num_serie)?;
            campo(
                w,
                "FechaExpedicionFacturaAnulada",
                &r.id_factura.fecha_expedicion.to_string(),
            )
        })?;
        campo_opt(w, "RefExterna", &r.ref_externa)?;
        if r.sin_registro_previo {
            campo(w, "SinRegistroPrevio", "S")?;
        }
        if r.rechazo_previo {
            campo(w, "RechazoPrevio", "S")?;
        }
        campo_opt(w, "GeneradoPor", &r.generado_por.map(|g| g.as_str()))?;
        if let Some(g) = &r.generador {
            escribir_persona(w, "Generador", g)?;
        }
        escribir_encadenamiento(w, &r.encadenamiento)?;
        escribir_sistema(w, &r.sistema_informatico)?;
        campo(
            w,
            "FechaHoraHusoGenRegistro",
            &r.fecha_hora_huso_gen_registro.to_string(),
        )?;
        campo(w, "TipoHuella", TIPO_HUELLA_SHA256)?;
        campo(w, "Huella", &r.huella)
    })
}
