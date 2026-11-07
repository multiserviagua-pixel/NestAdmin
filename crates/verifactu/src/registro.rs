//! Registros de facturación de alta y de anulación (`RegistroFacturacionAltaType` y
//! `RegistroFacturacionAnulacionType` de `SuministroInformacion.xsd`).

use serde::{Deserialize, Serialize};

use crate::huella;
use crate::tipos::{validar_nif, validar_texto, Fecha, FechaHoraHuso, Importe, Porcentaje};
use crate::Error;

/// Versión del diseño de registro (`VersionType`).
pub const ID_VERSION: &str = "1.0";
/// Tipo de huella (`TipoHuellaType`, lista L12): `01` = SHA-256.
pub const TIPO_HUELLA_SHA256: &str = "01";

/// Identificación de una factura: NIF del emisor, serie y número, fecha de expedición.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct IdFactura {
    pub nif_emisor: String,
    pub num_serie: String,
    pub fecha_expedicion: Fecha,
}

impl IdFactura {
    pub fn new(nif_emisor: &str, num_serie: &str, fecha_expedicion: Fecha) -> Self {
        Self {
            nif_emisor: nif_emisor.to_owned(),
            num_serie: num_serie.to_owned(),
            fecha_expedicion,
        }
    }

    pub(crate) fn validar(&self, campo: &str) -> Result<(), Error> {
        validar_nif(&self.nif_emisor, &format!("{campo}/IDEmisorFactura"))?;
        validar_texto(
            &self.num_serie,
            &format!("{campo}/NumSerieFactura"),
            60,
            true,
        )?;
        // La URL del QR solo admite ASCII imprimible (32-126) en `numserie`.
        if !self.num_serie.bytes().all(|c| (32..=126).contains(&c)) {
            return Err(Error::Campo {
                campo: format!("{campo}/NumSerieFactura"),
                motivo: "solo admite caracteres ASCII imprimibles".to_owned(),
            });
        }
        Ok(())
    }
}

/// Tipo de factura (`ClaveTipoFacturaType`). Descripciones del propio XSD.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TipoFactura {
    /// Factura (art. 6, 7.2 y 7.3 del RD 1619/2012).
    F1,
    /// Factura simplificada y facturas sin identificación del destinatario (art. 6.1.d).
    F2,
    /// Factura emitida en sustitución de facturas simplificadas facturadas y declaradas.
    F3,
    /// Rectificativa (art. 80.1 y 80.2 LIVA y error fundado en derecho).
    R1,
    /// Rectificativa (art. 80.3 LIVA).
    R2,
    /// Rectificativa (art. 80.4 LIVA).
    R3,
    /// Rectificativa (resto).
    R4,
    /// Rectificativa en facturas simplificadas.
    R5,
}

impl TipoFactura {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::F1 => "F1",
            Self::F2 => "F2",
            Self::F3 => "F3",
            Self::R1 => "R1",
            Self::R2 => "R2",
            Self::R3 => "R3",
            Self::R4 => "R4",
            Self::R5 => "R5",
        }
    }

    pub fn es_rectificativa(&self) -> bool {
        matches!(self, Self::R1 | Self::R2 | Self::R3 | Self::R4 | Self::R5)
    }

    pub fn desde_str(s: &str) -> Option<Self> {
        Some(match s {
            "F1" => Self::F1,
            "F2" => Self::F2,
            "F3" => Self::F3,
            "R1" => Self::R1,
            "R2" => Self::R2,
            "R3" => Self::R3,
            "R4" => Self::R4,
            "R5" => Self::R5,
            _ => return None,
        })
    }
}

/// Rectificativa sustitutiva (`S`) o incremental, por diferencias (`I`)
/// (`ClaveTipoRectificativaType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TipoRectificativa {
    Sustitucion,
    Diferencias,
}

impl TipoRectificativa {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Sustitucion => "S",
            Self::Diferencias => "I",
        }
    }
}

/// `RechazoPrevio` del alta (`RechazoPrevioType`, lista L17): `N`, `S` o `X`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RechazoPrevio {
    N,
    S,
    X,
}

impl RechazoPrevio {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::N => "N",
            Self::S => "S",
            Self::X => "X",
        }
    }
}

/// Identificación de una persona física o jurídica.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Identificacion {
    Nif(String),
    /// `IDOtro`: código de país ISO 3166-1 alfa-2 opcional, tipo de identificador
    /// (`PersonaFisicaJuridicaIDTypeType`: `02` NIF-IVA, `03` pasaporte, `04` documento del
    /// país de residencia, `05` certificado de residencia, `06` otro documento probatorio,
    /// `07` no censado) e identificador.
    Otro {
        codigo_pais: Option<String>,
        id_type: String,
        id: String,
    },
}

/// `PersonaFisicaJuridicaType`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Persona {
    pub nombre_razon: String,
    pub identificacion: Identificacion,
}

impl Persona {
    pub fn con_nif(nombre_razon: &str, nif: &str) -> Self {
        Self {
            nombre_razon: nombre_razon.to_owned(),
            identificacion: Identificacion::Nif(nif.to_owned()),
        }
    }

    pub(crate) fn validar(&self, campo: &str) -> Result<(), Error> {
        validar_texto(
            &self.nombre_razon,
            &format!("{campo}/NombreRazon"),
            120,
            true,
        )?;
        match &self.identificacion {
            Identificacion::Nif(n) => validar_nif(n, &format!("{campo}/NIF")),
            Identificacion::Otro {
                codigo_pais,
                id_type,
                id,
            } => {
                if let Some(p) = codigo_pais {
                    if p.len() != 2 || !p.bytes().all(|c| c.is_ascii_uppercase()) {
                        return Err(Error::Campo {
                            campo: format!("{campo}/IDOtro/CodigoPais"),
                            motivo: "debe ser un código ISO 3166-1 alfa-2".to_owned(),
                        });
                    }
                }
                if !["02", "03", "04", "05", "06", "07"].contains(&id_type.as_str()) {
                    return Err(Error::Campo {
                        campo: format!("{campo}/IDOtro/IDType"),
                        motivo: "valor fuera de PersonaFisicaJuridicaIDTypeType".to_owned(),
                    });
                }
                validar_texto(id, &format!("{campo}/IDOtro/ID"), 20, true)
            }
        }
    }
}

/// Impuesto del detalle (`ImpuestoType`): IVA, IPSI de Ceuta y Melilla, IGIC u otros.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Impuesto {
    Iva,
    Ipsi,
    Igic,
    Otros,
}

impl Impuesto {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Iva => "01",
            Self::Ipsi => "02",
            Self::Igic => "03",
            Self::Otros => "05",
        }
    }
}

/// Calificación de la operación (`CalificacionOperacionType`) o causa de exención
/// (`OperacionExentaType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Calificacion {
    /// Sujeta y no exenta sin inversión del sujeto pasivo.
    S1,
    /// Sujeta y no exenta con inversión del sujeto pasivo.
    S2,
    /// No sujeta (art. 7, 14 y otros).
    N1,
    /// No sujeta por reglas de localización.
    N2,
    /// Exenta; el número es la causa (`E1` a `E8`).
    Exenta(u8),
}

/// Línea de `Desglose/DetalleDesglose` (`DetalleType`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DetalleDesglose {
    pub impuesto: Option<Impuesto>,
    /// Clave de régimen (`IdOperacionesTrascendenciaTributariaType`), p. ej. `01` régimen
    /// general.
    pub clave_regimen: Option<String>,
    pub calificacion: Calificacion,
    pub tipo_impositivo: Option<Porcentaje>,
    pub base_imponible: Importe,
    pub base_imponible_a_coste: Option<Importe>,
    pub cuota_repercutida: Option<Importe>,
    pub tipo_recargo_equivalencia: Option<Porcentaje>,
    pub cuota_recargo_equivalencia: Option<Importe>,
}

impl DetalleDesglose {
    /// Línea sujeta y no exenta en régimen general (`01`, `S1`) de IVA.
    pub fn iva_general(base: Importe, tipo: Porcentaje, cuota: Importe) -> Self {
        Self {
            impuesto: Some(Impuesto::Iva),
            clave_regimen: Some("01".to_owned()),
            calificacion: Calificacion::S1,
            tipo_impositivo: Some(tipo),
            base_imponible: base,
            base_imponible_a_coste: None,
            cuota_repercutida: Some(cuota),
            tipo_recargo_equivalencia: None,
            cuota_recargo_equivalencia: None,
        }
    }

    fn validar(&self, i: usize) -> Result<(), Error> {
        const CLAVES: [&str; 18] = [
            "01", "02", "03", "04", "05", "06", "07", "08", "09", "10", "11", "14", "15", "17",
            "18", "19", "20", "21",
        ];
        if let Some(c) = &self.clave_regimen {
            if !CLAVES.contains(&c.as_str()) {
                return Err(Error::Campo {
                    campo: format!("Desglose[{i}]/ClaveRegimen"),
                    motivo: format!("«{c}» fuera de la lista"),
                });
            }
        }
        if let Calificacion::Exenta(n) = self.calificacion {
            if !(1..=8).contains(&n) {
                return Err(Error::Campo {
                    campo: format!("Desglose[{i}]/OperacionExenta"),
                    motivo: format!("E{n} fuera de OperacionExentaType"),
                });
            }
        }
        Ok(())
    }
}

/// `ImporteRectificacion` de una rectificativa por sustitución.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImporteRectificacion {
    pub base_rectificada: Importe,
    pub cuota_rectificada: Importe,
    pub cuota_recargo_rectificado: Option<Importe>,
}

/// Datos del productor del sistema informático (`SistemaInformaticoType`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SistemaInformatico {
    /// Productor del software: nombre y NIF (o `IDOtro`).
    pub productor: Persona,
    pub nombre_sistema: String,
    /// Identificador de dos caracteres que asigna el productor.
    pub id_sistema: String,
    pub version: String,
    pub numero_instalacion: String,
    pub solo_verifactu: bool,
    pub multi_ot: bool,
    pub indicador_multiples_ot: bool,
}

impl SistemaInformatico {
    pub(crate) fn validar(&self) -> Result<(), Error> {
        self.productor.validar("SistemaInformatico")?;
        validar_texto(
            &self.nombre_sistema,
            "SistemaInformatico/NombreSistemaInformatico",
            30,
            true,
        )?;
        validar_texto(
            &self.id_sistema,
            "SistemaInformatico/IdSistemaInformatico",
            2,
            true,
        )?;
        validar_texto(&self.version, "SistemaInformatico/Version", 50, true)?;
        validar_texto(
            &self.numero_instalacion,
            "SistemaInformatico/NumeroInstalacion",
            100,
            true,
        )
    }
}

/// Encadenamiento con el registro inmediatamente anterior del mismo sistema.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Encadenamiento {
    /// `PrimerRegistro = S`: no hay registro anterior y la huella anterior va vacía.
    PrimerRegistro,
    /// `RegistroAnterior`: identificación de la factura del registro anterior y su huella.
    RegistroAnterior {
        id_factura: IdFactura,
        huella: String,
    },
}

impl Encadenamiento {
    /// Huella anterior tal como entra en la cadena de la huella (vacía si es el primero).
    pub fn huella_anterior(&self) -> &str {
        match self {
            Self::PrimerRegistro => "",
            Self::RegistroAnterior { huella, .. } => huella,
        }
    }

    fn validar(&self) -> Result<(), Error> {
        if let Self::RegistroAnterior { id_factura, huella } = self {
            id_factura.validar("Encadenamiento/RegistroAnterior")?;
            if !huella::es_huella(huella) {
                return Err(Error::Campo {
                    campo: "Encadenamiento/RegistroAnterior/Huella".to_owned(),
                    motivo: "debe ser SHA-256 en hexadecimal mayúsculas (64 caracteres)".to_owned(),
                });
            }
        }
        Ok(())
    }
}

/// Registro de facturación de alta.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistroAlta {
    pub id_factura: IdFactura,
    pub ref_externa: Option<String>,
    pub nombre_razon_emisor: String,
    pub subsanacion: bool,
    pub rechazo_previo: Option<RechazoPrevio>,
    pub tipo_factura: TipoFactura,
    pub tipo_rectificativa: Option<TipoRectificativa>,
    pub facturas_rectificadas: Vec<IdFactura>,
    pub facturas_sustituidas: Vec<IdFactura>,
    pub importe_rectificacion: Option<ImporteRectificacion>,
    pub fecha_operacion: Option<Fecha>,
    pub descripcion_operacion: String,
    pub factura_simplificada_art7273: Option<bool>,
    pub sin_identif_destinatario_art61d: Option<bool>,
    pub macrodato: Option<bool>,
    /// `EmitidaPorTerceroODestinatario`: `Some('T')` tercero o `Some('D')` destinatario.
    pub emitida_por_tercero_o_destinatario: Option<char>,
    pub tercero: Option<Persona>,
    pub destinatarios: Vec<Persona>,
    pub cupon: Option<bool>,
    pub desglose: Vec<DetalleDesglose>,
    pub cuota_total: Importe,
    pub importe_total: Importe,
    pub encadenamiento: Encadenamiento,
    pub sistema_informatico: SistemaInformatico,
    pub fecha_hora_huso_gen_registro: FechaHoraHuso,
    /// Huella SHA-256 de este registro. La rellena [`RegistroAlta::sellar`].
    pub huella: String,
}

impl RegistroAlta {
    /// Cadena de entrada de la huella (especificación de huella, apartado 3.a).
    pub fn cadena_huella(&self) -> String {
        huella::cadena_alta(&huella::CamposAlta {
            id_emisor_factura: &self.id_factura.nif_emisor,
            num_serie_factura: &self.id_factura.num_serie,
            fecha_expedicion_factura: &self.id_factura.fecha_expedicion.to_string(),
            tipo_factura: self.tipo_factura.as_str(),
            cuota_total: &self.cuota_total.to_string(),
            importe_total: &self.importe_total.to_string(),
            huella_anterior: self.encadenamiento.huella_anterior(),
            fecha_hora_huso_gen_registro: &self.fecha_hora_huso_gen_registro.to_string(),
        })
    }

    pub fn calcular_huella(&self) -> String {
        huella::sha256_hex(&self.cadena_huella())
    }

    /// Valida el registro y escribe su huella.
    pub fn sellar(&mut self) -> Result<(), Error> {
        self.validar()?;
        self.huella = self.calcular_huella();
        Ok(())
    }

    /// Validaciones de formato y cardinalidad del XSD que el tipo no garantiza por sí solo.
    /// Las reglas de negocio y de censo las aplica la AEAT.
    pub fn validar(&self) -> Result<(), Error> {
        self.id_factura.validar("IDFactura")?;
        if let Some(r) = &self.ref_externa {
            validar_texto(r, "RefExterna", 60, false)?;
        }
        validar_texto(&self.nombre_razon_emisor, "NombreRazonEmisor", 120, true)?;
        validar_texto(
            &self.descripcion_operacion,
            "DescripcionOperacion",
            500,
            true,
        )?;
        if self.tipo_factura.es_rectificativa() != self.tipo_rectificativa.is_some() {
            return Err(Error::Campo {
                campo: "TipoRectificativa".to_owned(),
                motivo: "obligatorio en R1-R5 y prohibido en F1-F3".to_owned(),
            });
        }
        if self.facturas_rectificadas.len() > 1000
            || self.facturas_sustituidas.len() > 1000
            || self.destinatarios.len() > 1000
        {
            return Err(Error::Campo {
                campo: "FacturasRectificadas/FacturasSustituidas/Destinatarios".to_owned(),
                motivo: "admiten como máximo 1000 elementos".to_owned(),
            });
        }
        for (i, f) in self.facturas_rectificadas.iter().enumerate() {
            f.validar(&format!("FacturasRectificadas[{i}]"))?;
        }
        for (i, f) in self.facturas_sustituidas.iter().enumerate() {
            f.validar(&format!("FacturasSustituidas[{i}]"))?;
        }
        if let Some(c) = self.emitida_por_tercero_o_destinatario {
            if c != 'T' && c != 'D' {
                return Err(Error::Campo {
                    campo: "EmitidaPorTerceroODestinatario".to_owned(),
                    motivo: "debe ser T o D".to_owned(),
                });
            }
        }
        if let Some(t) = &self.tercero {
            t.validar("Tercero")?;
        }
        for (i, d) in self.destinatarios.iter().enumerate() {
            d.validar(&format!("Destinatarios[{i}]"))?;
        }
        if self.desglose.is_empty() || self.desglose.len() > 12 {
            return Err(Error::Campo {
                campo: "Desglose".to_owned(),
                motivo: "debe tener entre 1 y 12 líneas".to_owned(),
            });
        }
        for (i, d) in self.desglose.iter().enumerate() {
            d.validar(i)?;
        }
        self.encadenamiento.validar()?;
        self.sistema_informatico.validar()
    }
}

/// `GeneradoPor` de la anulación (`GeneradoPorType`): expedidor, destinatario o tercero.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum GeneradoPor {
    Expedidor,
    Destinatario,
    Tercero,
}

impl GeneradoPor {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Expedidor => "E",
            Self::Destinatario => "D",
            Self::Tercero => "T",
        }
    }
}

/// Registro de facturación de anulación.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistroAnulacion {
    /// Factura anulada (`IDEmisorFacturaAnulada`, `NumSerieFacturaAnulada`,
    /// `FechaExpedicionFacturaAnulada`).
    pub id_factura: IdFactura,
    pub ref_externa: Option<String>,
    pub sin_registro_previo: bool,
    pub rechazo_previo: bool,
    pub generado_por: Option<GeneradoPor>,
    pub generador: Option<Persona>,
    pub encadenamiento: Encadenamiento,
    pub sistema_informatico: SistemaInformatico,
    pub fecha_hora_huso_gen_registro: FechaHoraHuso,
    pub huella: String,
}

impl RegistroAnulacion {
    /// Cadena de entrada de la huella (especificación de huella, apartado 3.b).
    pub fn cadena_huella(&self) -> String {
        huella::cadena_anulacion(&huella::CamposAnulacion {
            id_emisor_factura_anulada: &self.id_factura.nif_emisor,
            num_serie_factura_anulada: &self.id_factura.num_serie,
            fecha_expedicion_factura_anulada: &self.id_factura.fecha_expedicion.to_string(),
            huella_anterior: self.encadenamiento.huella_anterior(),
            fecha_hora_huso_gen_registro: &self.fecha_hora_huso_gen_registro.to_string(),
        })
    }

    pub fn calcular_huella(&self) -> String {
        huella::sha256_hex(&self.cadena_huella())
    }

    pub fn sellar(&mut self) -> Result<(), Error> {
        self.validar()?;
        self.huella = self.calcular_huella();
        Ok(())
    }

    pub fn validar(&self) -> Result<(), Error> {
        self.id_factura.validar("IDFactura")?;
        if let Some(r) = &self.ref_externa {
            validar_texto(r, "RefExterna", 60, false)?;
        }
        if let Some(g) = &self.generador {
            g.validar("Generador")?;
        }
        self.encadenamiento.validar()?;
        self.sistema_informatico.validar()
    }
}

/// Un registro de la cadena: alta o anulación.
// El alta ocupa más que la anulación; no se encaja en `Box` para que el tipo se construya y
// se compare de forma directa. Un envío tiene como mucho 1000 registros.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RegistroFacturacion {
    Alta(RegistroAlta),
    Anulacion(RegistroAnulacion),
}

impl RegistroFacturacion {
    pub fn id_factura(&self) -> &IdFactura {
        match self {
            Self::Alta(r) => &r.id_factura,
            Self::Anulacion(r) => &r.id_factura,
        }
    }

    pub fn huella(&self) -> &str {
        match self {
            Self::Alta(r) => &r.huella,
            Self::Anulacion(r) => &r.huella,
        }
    }

    pub fn encadenamiento(&self) -> &Encadenamiento {
        match self {
            Self::Alta(r) => &r.encadenamiento,
            Self::Anulacion(r) => &r.encadenamiento,
        }
    }

    pub fn fecha_hora_huso_gen_registro(&self) -> FechaHoraHuso {
        match self {
            Self::Alta(r) => r.fecha_hora_huso_gen_registro,
            Self::Anulacion(r) => r.fecha_hora_huso_gen_registro,
        }
    }

    pub fn calcular_huella(&self) -> String {
        match self {
            Self::Alta(r) => r.calcular_huella(),
            Self::Anulacion(r) => r.calcular_huella(),
        }
    }

    pub fn validar(&self) -> Result<(), Error> {
        match self {
            Self::Alta(r) => r.validar(),
            Self::Anulacion(r) => r.validar(),
        }
    }

    /// Encadenamiento que debe llevar el registro siguiente a este.
    pub fn siguiente_encadenamiento(&self) -> Encadenamiento {
        Encadenamiento::RegistroAnterior {
            id_factura: self.id_factura().clone(),
            huella: self.huella().to_owned(),
        }
    }
}
