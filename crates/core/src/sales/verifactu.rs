//! Registros Verifactu de la facturación de venta (fase 11, migración 12).
//!
//! Conecta la emisión y la anulación de `documents.rs` con `nestadmin-verifactu`: dentro de la
//! MISMA transacción que numera, emite o anula, traduce la entrada del registro (D83, D113) a
//! un `RegistroAlta` o `RegistroAnulacion`, lo encadena con el último registro guardado, lo
//! sella y lo inserta en `verifactu_records`. Si cualquier paso falla (traducción, cadena rota,
//! reloj atrasado, inserción), la transacción se deshace: la factura sigue en borrador y la
//! serie no consume número.
//!
//! Este módulo no tiene reloj ni datos propios del sistema informático: los aporta quien llama
//! en [`VerifactuContext`]. No remite nada a la AEAT; solo deja los registros listos y
//! verificables. Los mensajes de error llevan identificadores y nombres de campo, nunca
//! secretos.

use nestadmin_documents::IsoDate;
use nestadmin_sales::{
    InvoiceRecordInput, InvoiceTypeCode, RectificationMethod, TaxBreakdown, TaxQualification,
};
use nestadmin_verifactu::{
    comprobar_antes_de_generar, verificar_cadena, Calificacion, DetalleDesglose, Encadenamiento,
    Error as VfError, FalloCadena, Fecha, FechaHoraHuso, IdFactura, Identificacion, Importe,
    ImporteRectificacion, Impuesto, Persona, Porcentaje, RegistroAlta, RegistroAnulacion,
    RegistroFacturacion, SistemaInformatico, TipoFactura, TipoRectificativa,
};
use rusqlite::{params, Connection, OptionalExtension};
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;

use super::documents::SalesDocument;
use super::master::load_company;
use super::{SalesResult, SalesStoreError};
use crate::store::now_ms;
use crate::Store;

/// Código del error cuando la cadena guardada no es íntegra (registro alterado o ilegible).
pub const CODE_CHAIN_BROKEN: &str = "verifactu_chain_broken";
/// Código del error cuando el reloj está más de un minuto por detrás del último registro.
pub const CODE_CLOCK_BEHIND: &str = "verifactu_clock_behind";
/// Código del error cuando la factura no se puede traducir a un registro.
pub const CODE_RECORD_INVALID: &str = "verifactu_record_invalid";

/// Datos que aporta quien llama: el sistema informático (productor, versión, instalación) y el
/// instante de generación del registro. Nada se inventa ni se lee de un reloj interno.
#[derive(Debug, Clone)]
pub struct VerifactuContext {
    pub system: SistemaInformatico,
    pub now: FechaHoraHuso,
}

/// Clase de un registro guardado.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifactuKind {
    Alta,
    Anulacion,
}

impl VerifactuKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Alta => "alta",
            Self::Anulacion => "anulacion",
        }
    }

    fn from_text(text: &str) -> SalesResult<Self> {
        match text {
            "alta" => Ok(Self::Alta),
            "anulacion" => Ok(Self::Anulacion),
            other => Err(SalesStoreError::Corrupt(format!(
                "clase de registro Verifactu {other}"
            ))),
        }
    }
}

/// Fila de `verifactu_records` con el registro ya reconstruido.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifactuRecordRow {
    pub id: i64,
    pub kind: VerifactuKind,
    pub document_id: i64,
    pub issuer_tax_id: String,
    pub invoice_number: String,
    /// `DD-MM-AAAA`, como va en el registro.
    pub issue_date: String,
    pub invoice_type: Option<String>,
    pub total: Option<String>,
    pub tax_total: Option<String>,
    pub previous_id: Option<i64>,
    pub previous_hash: Option<String>,
    /// `AAAA-MM-DDThh:mm:ss±hh:mm`.
    pub generated_at: String,
    pub hash: String,
    /// Registro completo (`record_json` deserializado).
    pub record: RegistroFacturacion,
    pub created_at: i64,
}

/// Estado de la cadena guardada. Una cadena rota no es un error: es un estado que la aplicación
/// muestra.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChainStatus {
    Empty,
    Ok { records: usize },
    Broken { at_id: i64, reason: String },
}

fn record_error(why: impl std::fmt::Display) -> SalesStoreError {
    SalesStoreError::External {
        code: CODE_RECORD_INVALID.to_owned(),
        message: format!("no se puede generar el registro Verifactu: {why}"),
    }
}

fn chain_error(why: impl std::fmt::Display) -> SalesStoreError {
    SalesStoreError::External {
        code: CODE_CHAIN_BROKEN.to_owned(),
        message: format!("cadena de registros Verifactu: {why}"),
    }
}

fn from_vf(e: VfError) -> SalesStoreError {
    match e {
        VfError::RelojAtrasado { .. } => SalesStoreError::External {
            code: CODE_CLOCK_BEHIND.to_owned(),
            message: e.to_string(),
        },
        VfError::Cadena(_) => chain_error(e),
        other => record_error(other),
    }
}

// --- Traducción de la entrada del registro ---

/// Céntimos exactos de un decimal; `None` si tiene más de dos decimales.
fn exact_hundredths(value: Decimal) -> Option<Decimal> {
    value
        .checked_mul(Decimal::ONE_HUNDRED)
        .filter(|c| c.fract().is_zero())
}

fn importe(field: &str, value: Decimal) -> SalesResult<Importe> {
    let cents = exact_hundredths(value)
        .and_then(|c| c.trunc().to_i64())
        .ok_or_else(|| record_error(format!("{field} no es un importe exacto a dos decimales")))?;
    Importe::centimos(cents).map_err(from_vf)
}

fn porcentaje(field: &str, value: Decimal) -> SalesResult<Porcentaje> {
    let hundredths = exact_hundredths(value)
        .and_then(|c| c.trunc().to_u32())
        .ok_or_else(|| record_error(format!("{field} no es un porcentaje exacto")))?;
    Porcentaje::centesimas(hundredths).map_err(from_vf)
}

fn fecha(date: IsoDate) -> SalesResult<Fecha> {
    Fecha::new(date.year, date.month, date.day).map_err(from_vf)
}

fn invoice_type(code: InvoiceTypeCode) -> TipoFactura {
    match code {
        InvoiceTypeCode::F1 => TipoFactura::F1,
        InvoiceTypeCode::F2 => TipoFactura::F2,
        InvoiceTypeCode::R1 => TipoFactura::R1,
        InvoiceTypeCode::R2 => TipoFactura::R2,
        InvoiceTypeCode::R3 => TipoFactura::R3,
        InvoiceTypeCode::R4 => TipoFactura::R4,
        InvoiceTypeCode::R5 => TipoFactura::R5,
    }
}

/// Línea del desglose. La correspondencia con la AEAT es uno a uno (D113): `S1` y `S2` llevan
/// tipo y cuota; `N1`, `N2` y `E1` a `E6` solo la base, y exigen el 0 %. Un grupo sin
/// calificación es `S1` y exige un tipo gravado. Cualquier otra combinación es un error.
fn detail(index: usize, group: &TaxBreakdown) -> SalesResult<DetalleDesglose> {
    let rate = group.tax_rate.percent();
    let at = |what: &str| format!("desglose {}: {what}", index + 1);
    let qualification = group.qualification.unwrap_or(TaxQualification::S1);
    let (calificacion, taxed) = match qualification {
        TaxQualification::S1 => (Calificacion::S1, true),
        TaxQualification::S2 => (Calificacion::S2, true),
        TaxQualification::N1 => (Calificacion::N1, false),
        TaxQualification::N2 => (Calificacion::N2, false),
        TaxQualification::E1 => (Calificacion::Exenta(1), false),
        TaxQualification::E2 => (Calificacion::Exenta(2), false),
        TaxQualification::E3 => (Calificacion::Exenta(3), false),
        TaxQualification::E4 => (Calificacion::Exenta(4), false),
        TaxQualification::E5 => (Calificacion::Exenta(5), false),
        TaxQualification::E6 => (Calificacion::Exenta(6), false),
    };
    if qualification == TaxQualification::S1 && rate.is_zero() {
        return Err(record_error(at("S1 exige un tipo gravado")));
    }
    if !taxed && !rate.is_zero() {
        return Err(record_error(at(&format!(
            "{} exige el tipo del 0 %",
            qualification.as_str()
        ))));
    }
    let base = importe(&at("base"), group.base)?;
    let (tipo, cuota) = if taxed {
        (
            Some(porcentaje(&at("tipo"), rate)?),
            Some(importe(&at("cuota"), group.tax)?),
        )
    } else {
        if !group.tax.is_zero() {
            return Err(record_error(at("una operación sin cuota lleva cuota")));
        }
        (None, None)
    };
    Ok(DetalleDesglose {
        impuesto: Some(Impuesto::Iva),
        clave_regimen: Some("01".to_owned()),
        calificacion,
        tipo_impositivo: tipo,
        base_imponible: base,
        base_imponible_a_coste: None,
        cuota_repercutida: cuota,
        tipo_recargo_equivalencia: None,
        cuota_recargo_equivalencia: None,
    })
}

/// Descripciones del desglose unidas con «; », sin repetidas y recortadas a 500 caracteres.
fn operation_description(breakdown: &[TaxBreakdown]) -> String {
    let mut parts: Vec<String> = Vec::new();
    for text in breakdown.iter().flat_map(|b| b.descriptions.iter()) {
        let clean: String = text
            .chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect::<String>()
            .trim()
            .to_owned();
        if !clean.is_empty() && !parts.contains(&clean) {
            parts.push(clean);
        }
    }
    if parts.is_empty() {
        return "Venta".to_owned();
    }
    let cut: String = parts.join("; ").chars().take(500).collect();
    cut.trim_end().to_owned()
}

/// Traduce la entrada de una factura emitida a un alta sin sellar (huella vacía).
fn build_alta(
    issuer_name: &str,
    input: &InvoiceRecordInput,
    chain: Encadenamiento,
    ctx: &VerifactuContext,
) -> SalesResult<RegistroAlta> {
    let id_factura = IdFactura::new(
        &input.issuer_tax_id,
        &input.number.to_string(),
        fecha(input.issue_date)?,
    );
    let tipo_factura = invoice_type(input.invoice_type);
    if tipo_factura.es_rectificativa() != input.rectifies.is_some() {
        return Err(record_error(
            "el tipo de factura y los datos de rectificación no concuerdan",
        ));
    }
    let (tipo_rectificativa, facturas_rectificadas, importe_rectificacion) = match &input.rectifies
    {
        None => (None, Vec::new(), None),
        Some(rect) => {
            let rectified = IdFactura::new(
                &input.issuer_tax_id,
                &rect.number.to_string(),
                fecha(rect.issue_date)?,
            );
            match rect.method {
                RectificationMethod::Substitution => {
                    let (Some(base), Some(tax)) = (rect.base, rect.tax) else {
                        return Err(record_error(
                            "la rectificativa por sustitución no lleva base y cuota rectificadas",
                        ));
                    };
                    (
                        Some(TipoRectificativa::Sustitucion),
                        vec![rectified],
                        Some(ImporteRectificacion {
                            base_rectificada: importe("base rectificada", base)?,
                            cuota_rectificada: importe("cuota rectificada", tax)?,
                            cuota_recargo_rectificado: None,
                        }),
                    )
                }
                RectificationMethod::Differences => {
                    (Some(TipoRectificativa::Diferencias), vec![rectified], None)
                }
            }
        }
    };
    let destinatarios = match (&input.recipient_name, &input.recipient_tax_id) {
        (Some(name), Some(nif)) => vec![Persona {
            nombre_razon: name.clone(),
            identificacion: Identificacion::Nif(nif.clone()),
        }],
        (None, None) => Vec::new(),
        _ => {
            return Err(record_error(
                "el destinatario necesita nombre y NIF, o ninguno de los dos",
            ))
        }
    };
    let desglose = input
        .breakdown
        .iter()
        .enumerate()
        .map(|(i, g)| detail(i, g))
        .collect::<SalesResult<Vec<_>>>()?;
    Ok(RegistroAlta {
        id_factura,
        ref_externa: None,
        nombre_razon_emisor: issuer_name.to_owned(),
        subsanacion: false,
        rechazo_previo: None,
        tipo_factura,
        tipo_rectificativa,
        facturas_rectificadas,
        facturas_sustituidas: Vec::new(),
        importe_rectificacion,
        fecha_operacion: None,
        descripcion_operacion: operation_description(&input.breakdown),
        factura_simplificada_art7273: None,
        sin_identif_destinatario_art61d: None,
        macrodato: None,
        emitida_por_tercero_o_destinatario: None,
        tercero: None,
        destinatarios,
        cupon: None,
        desglose,
        cuota_total: importe("cuota total", input.tax_total)?,
        importe_total: importe("importe total", input.total)?,
        encadenamiento: chain,
        sistema_informatico: ctx.system.clone(),
        fecha_hora_huso_gen_registro: ctx.now,
        huella: String::new(),
    })
}

// --- Lectura y escritura de la cadena ---

fn parse_record(id: i64, json: &str) -> SalesResult<RegistroFacturacion> {
    serde_json::from_str(json)
        .map_err(|_| chain_error(format!("el registro {id} no se puede leer")))
}

/// Encadenamiento del registro nuevo tras comprobar el último (mayor id): su huella recalculada
/// desde su JSON coincide con la guardada y con la columna `hash`, y su fecha-hora no supera en
/// más de un minuto a la del nuevo registro. Devuelve también id y huella del último, para
/// `previous_id` y `previous_hash`.
fn next_link(
    conn: &Connection,
    ctx: &VerifactuContext,
) -> SalesResult<(Encadenamiento, Option<(i64, String)>)> {
    let head: Option<(i64, String, String)> = conn
        .query_row(
            "SELECT id, hash, record_json FROM verifactu_records ORDER BY id DESC LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let Some((id, hash, json)) = head else {
        return Ok((Encadenamiento::PrimerRegistro, None));
    };
    // Antes de generar se verifica la cadena entera (D83): una manipulación de cualquier
    // registro anterior bloquea la emisión, no solo la del último.
    if let ChainStatus::Broken { at_id, reason } = chain_status(conn)? {
        return Err(chain_error(format!(
            "cadena rota en el registro {at_id}: {reason}"
        )));
    }
    let last = parse_record(id, &json)?;
    comprobar_antes_de_generar(&last, ctx.now).map_err(from_vf)?;
    Ok((last.siguiente_encadenamiento(), Some((id, hash))))
}

fn insert_record(
    conn: &Connection,
    document_id: i64,
    record: &RegistroFacturacion,
    previous: Option<(i64, String)>,
) -> SalesResult<()> {
    let (kind, invoice_type, total, tax_total) = match record {
        RegistroFacturacion::Alta(a) => (
            VerifactuKind::Alta,
            Some(a.tipo_factura.as_str().to_owned()),
            Some(a.importe_total.to_string()),
            Some(a.cuota_total.to_string()),
        ),
        RegistroFacturacion::Anulacion(_) => (VerifactuKind::Anulacion, None, None, None),
    };
    let id = record.id_factura();
    let (previous_id, previous_hash) = match previous {
        Some((id, hash)) => (Some(id), Some(hash)),
        None => (None, None),
    };
    conn.execute(
        "INSERT INTO verifactu_records (kind, document_id, issuer_tax_id, invoice_number, \
         issue_date, invoice_type, total, tax_total, previous_id, previous_hash, generated_at, \
         hash, record_json, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        params![
            kind.as_str(),
            document_id,
            id.nif_emisor,
            id.num_serie,
            id.fecha_expedicion.to_string(),
            invoice_type,
            total,
            tax_total,
            previous_id,
            previous_hash,
            record.fecha_hora_huso_gen_registro().to_string(),
            record.huella(),
            serde_json::to_string(record)?,
            now_ms(),
        ],
    )?;
    Ok(())
}

/// Genera, sella e inserta el alta de una factura recién emitida (dentro de su transacción).
fn write_alta(
    conn: &Connection,
    document_id: i64,
    input: &InvoiceRecordInput,
    ctx: &VerifactuContext,
) -> SalesResult<()> {
    let issuer_name = load_company(conn)?
        .and_then(|c| c.name)
        .ok_or_else(|| record_error("la empresa emisora no tiene nombre"))?;
    let (chain, previous) = next_link(conn, ctx)?;
    let mut alta = build_alta(&issuer_name, input, chain, ctx)?;
    alta.sellar().map_err(from_vf)?;
    insert_record(
        conn,
        document_id,
        &RegistroFacturacion::Alta(alta),
        previous,
    )
}

/// Misma normalización que `record_input` aplica al NIF del emisor.
fn normalize_tax_id(id: &str) -> String {
    id.chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .collect::<String>()
        .to_uppercase()
}

/// Genera, sella e inserta la anulación de una factura recién anulada.
fn write_anulacion(
    conn: &Connection,
    doc: &SalesDocument,
    ctx: &VerifactuContext,
) -> SalesResult<()> {
    let number = doc
        .number
        .as_deref()
        .ok_or_else(|| record_error("la factura anulada no tiene número"))?;
    let issue_date = doc
        .issue_date
        .ok_or_else(|| record_error("la factura anulada no tiene fecha de expedición"))?;
    let tax_id = doc
        .issuer
        .as_ref()
        .and_then(|i| i.party.tax_id.as_deref())
        .ok_or_else(|| record_error("la factura anulada no tiene NIF de emisor"))?;
    let (chain, previous) = next_link(conn, ctx)?;
    let mut anulacion = RegistroAnulacion {
        id_factura: IdFactura::new(&normalize_tax_id(tax_id), number, fecha(issue_date)?),
        ref_externa: None,
        sin_registro_previo: false,
        rechazo_previo: false,
        generado_por: None,
        generador: None,
        encadenamiento: chain,
        sistema_informatico: ctx.system.clone(),
        fecha_hora_huso_gen_registro: ctx.now,
        huella: String::new(),
    };
    anulacion.sellar().map_err(from_vf)?;
    insert_record(
        conn,
        doc.id,
        &RegistroFacturacion::Anulacion(anulacion),
        previous,
    )
}

/// Fila tal como está guardada, antes de interpretar `kind` y `record_json`.
struct RawRow {
    id: i64,
    kind: String,
    document_id: i64,
    issuer_tax_id: String,
    invoice_number: String,
    issue_date: String,
    invoice_type: Option<String>,
    total: Option<String>,
    tax_total: Option<String>,
    previous_id: Option<i64>,
    previous_hash: Option<String>,
    generated_at: String,
    hash: String,
    json: String,
    created_at: i64,
}

impl RawRow {
    fn into_row(self) -> SalesResult<VerifactuRecordRow> {
        let record = serde_json::from_str(&self.json).map_err(|_| {
            SalesStoreError::Corrupt(format!("registro Verifactu {} ilegible", self.id))
        })?;
        Ok(VerifactuRecordRow {
            id: self.id,
            kind: VerifactuKind::from_text(&self.kind)?,
            document_id: self.document_id,
            issuer_tax_id: self.issuer_tax_id,
            invoice_number: self.invoice_number,
            issue_date: self.issue_date,
            invoice_type: self.invoice_type,
            total: self.total,
            tax_total: self.tax_total,
            previous_id: self.previous_id,
            previous_hash: self.previous_hash,
            generated_at: self.generated_at,
            hash: self.hash,
            record,
            created_at: self.created_at,
        })
    }
}

fn raw_rows(conn: &Connection) -> SalesResult<Vec<RawRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, kind, document_id, issuer_tax_id, invoice_number, issue_date, invoice_type, \
         total, tax_total, previous_id, previous_hash, generated_at, hash, record_json, \
         created_at FROM verifactu_records ORDER BY id",
    )?;
    let rows = stmt
        .query_map([], |r| {
            Ok(RawRow {
                id: r.get(0)?,
                kind: r.get(1)?,
                document_id: r.get(2)?,
                issuer_tax_id: r.get(3)?,
                invoice_number: r.get(4)?,
                issue_date: r.get(5)?,
                invoice_type: r.get(6)?,
                total: r.get(7)?,
                tax_total: r.get(8)?,
                previous_id: r.get(9)?,
                previous_hash: r.get(10)?,
                generated_at: r.get(11)?,
                hash: r.get(12)?,
                json: r.get(13)?,
                created_at: r.get(14)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

impl Store {
    /// Emite el documento. Si es una factura, genera en la misma transacción su registro de
    /// alta Verifactu, encadenado con el último; un presupuesto o un albarán no genera ninguno.
    /// Si el registro no se puede generar (traducción, cadena rota, reloj atrasado) no se emite
    /// ni se consume número.
    pub fn issue_sales_invoice_registered(
        &mut self,
        id: i64,
        issue_date: IsoDate,
        today: IsoDate,
        ctx: &VerifactuContext,
    ) -> SalesResult<SalesDocument> {
        self.issue_sales_document_with(id, issue_date, today, |tx, record| match record {
            Some(input) => write_alta(tx, id, input, ctx),
            None => Ok(()),
        })
    }

    /// Anula una factura emitida y genera en la misma transacción su registro de anulación.
    pub fn void_sales_invoice_registered(
        &mut self,
        id: i64,
        ctx: &VerifactuContext,
    ) -> SalesResult<SalesDocument> {
        self.void_sales_invoice_with(id, |tx, doc| {
            if doc.kind.is_invoice() {
                write_anulacion(tx, doc, ctx)
            } else {
                Ok(())
            }
        })
    }

    /// Registros guardados, en orden de generación.
    pub fn verifactu_records(&self) -> SalesResult<Vec<VerifactuRecordRow>> {
        raw_rows(&self.conn)?
            .into_iter()
            .map(RawRow::into_row)
            .collect()
    }

    /// Reconstruye todos los registros y verifica la cadena: huellas recalculadas, eslabones y
    /// coherencia de las columnas `hash` y `previous_id`. Una cadena rota se devuelve como
    /// estado, no como error.
    pub fn verify_verifactu_chain(&self) -> SalesResult<ChainStatus> {
        chain_status(&self.conn)
    }
}

/// Estado de la cadena completa; lo usan la verificación pública y la comprobación previa a
/// cada registro nuevo.
fn chain_status(conn: &Connection) -> SalesResult<ChainStatus> {
    let raw = raw_rows(conn)?;
    if raw.is_empty() {
        return Ok(ChainStatus::Empty);
    }
    let mut records = Vec::with_capacity(raw.len());
    let mut previous_id: Option<i64> = None;
    for row in &raw {
        let broken = |reason: &str| {
            Ok(ChainStatus::Broken {
                at_id: row.id,
                reason: reason.to_owned(),
            })
        };
        let Ok(record) = serde_json::from_str::<RegistroFacturacion>(&row.json) else {
            return broken("el registro no se puede leer");
        };
        if record.huella() != row.hash {
            return broken("la huella guardada no coincide con la del registro");
        }
        if row.previous_id != previous_id {
            return broken("el registro anterior enlazado no es el precedente");
        }
        previous_id = Some(row.id);
        records.push(record);
    }
    match verificar_cadena(&records, None) {
        Ok(()) => Ok(ChainStatus::Ok {
            records: records.len(),
        }),
        Err(e) => {
            let index = match &e {
                FalloCadena::HuellaAlterada { indice }
                | FalloCadena::EslabonRoto { indice }
                | FalloCadena::PrimerRegistroInesperado { indice } => *indice,
                FalloCadena::Vacia | FalloCadena::FaltaInicio => 0,
            };
            Ok(ChainStatus::Broken {
                at_id: raw[index.min(raw.len() - 1)].id,
                reason: e.to_string(),
            })
        }
    }
}
