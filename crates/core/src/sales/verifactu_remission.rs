//! Remisión de los registros Verifactu a la AEAT: entorno, lotes, resultados y subsanación
//! (etapa E5, D247–D259, migración 16).
//!
//! Este módulo solo guarda y consulta; no llama a ninguna red. Quien remite (la capa de
//! aplicación, con el simulador local en E5) usa esta API así:
//!
//! 1. [`Store::unfinished_verifactu_batch`]: si hay un lote abierto (sin resultado), se reenvía
//!    idéntico; si no, [`Store::open_verifactu_batch`] compone uno con los registros pendientes
//!    del entorno, en orden de cadena.
//! 2. [`Store::begin_verifactu_attempt`] ANTES de cada llamada (D252).
//! 3. Con una respuesta entendida o un `Fault` SOAP, [`Store::close_verifactu_batch`]. Un fallo
//!    de red, un 5xx o una respuesta ilegible NO cierran el lote: queda abierto y se reenvía.
//!
//! Todo es de solo inserción: el estado de cada registro ([`VerifactuStatus`]) se deriva de lo
//! guardado y nunca se actualiza. Los registros que se remiten son siempre los guardados en
//! `verifactu_records`, sin cambios (R07, D250): aquí no se genera ni se modifica ninguno,
//! salvo la subsanación, que la pide el usuario y crea un registro nuevo.

use std::collections::{HashMap, HashSet};

use nestadmin_verifactu::{
    Cabecera, Entorno, Envio, EstadoRegistro, RegistroFacturacion, ResultadoEnvio,
    MAX_REGISTROS_POR_ENVIO, TIEMPO_ESPERA_INICIAL_S,
};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};

use super::verifactu::{
    insert_record, next_link, record_error, rows_matching, VerifactuContext, VerifactuKind,
    VerifactuRecordRow,
};
use super::verifactu_sending::{
    close_incident, generated_during_incident, halt_authorization, open_incident,
    retry_failure_wait, FaultClass, WaitMode,
};
use super::{SalesResult, SalesStoreError};
use crate::store::now_ms;
use crate::Store;

/// Autorización que consta en el historial de la remisión (R07, D250): no es una acción
/// discrecional del usuario sino la obligación de la modalidad VERI*FACTU; va seguida del
/// entorno (`obligacion_verifactu simulador`).
pub const OBLIGATION_AUTHORIZATION: &str = "obligacion_verifactu";
/// Código: ya hay un lote abierto; hay que reenviarlo, no abrir otro (D252).
pub const CODE_BATCH_OPEN: &str = "verifactu_batch_open";
/// Código: demasiados `Fault` seguidos; la remisión automática se para hasta acción del
/// usuario (D259).
pub const CODE_BATCH_REJECTED: &str = "verifactu_batch_rejected";
/// Código: no se cambia de entorno con registros pendientes, en envío o rechazados sin
/// subsanar (D256).
pub const CODE_ENVIRONMENT_BUSY: &str = "verifactu_environment_busy";
/// Código: el entorno vigente no es el que llevaba quien llamó (cambió entre la lectura y la
/// escritura); no se genera ni se remite nada.
pub const CODE_ENVIRONMENT_CHANGED: &str = "verifactu_environment_changed";
/// `Fault` seguidos tras los cuales la remisión automática se detiene (D259).
pub const MAX_AUTOMATIC_FAULTS: u32 = 3;
/// Espera mínima en segundos tras un fallo sin respuesta o un `Fault` (D251).
pub const MIN_RETRY_WAIT_SECONDS: u32 = TIEMPO_ESPERA_INICIAL_S;

// --- Entornos ---

/// Entorno de remisión vigente (D85, D247). En E5 solo `Simulador` es elegible desde la
/// aplicación; el almacenamiento admite los tres.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VerifactuEnvironment {
    Simulador,
    Preproduccion,
    Produccion,
}

impl VerifactuEnvironment {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Simulador => "simulador",
            Self::Preproduccion => "preproduccion",
            Self::Produccion => "produccion",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "simulador" => Some(Self::Simulador),
            "preproduccion" => Some(Self::Preproduccion),
            "produccion" => Some(Self::Produccion),
            _ => None,
        }
    }

    fn from_stored(text: &str) -> SalesResult<Self> {
        Self::parse(text)
            .ok_or_else(|| SalesStoreError::Corrupt(format!("entorno de Verifactu {text}")))
    }

    /// Entornos guardados por registro que cuentan como este (los registros heredados cuentan
    /// como producción, D258).
    fn record_environments(self) -> (&'static str, &'static str) {
        match self {
            Self::Produccion => ("produccion", "produccion_heredado"),
            other => (other.as_str(), other.as_str()),
        }
    }
}

/// Entorno guardado con un registro (D248). `ProduccionHeredado` es el de los registros
/// anteriores a la migración 16 (D258).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RecordEnvironment {
    Simulador,
    Preproduccion,
    Produccion,
    ProduccionHeredado,
}

impl RecordEnvironment {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Simulador => "simulador",
            Self::Preproduccion => "preproduccion",
            Self::Produccion => "produccion",
            Self::ProduccionHeredado => "produccion_heredado",
        }
    }

    fn from_stored(text: &str) -> SalesResult<Self> {
        match text {
            "simulador" => Ok(Self::Simulador),
            "preproduccion" => Ok(Self::Preproduccion),
            "produccion" => Ok(Self::Produccion),
            "produccion_heredado" => Ok(Self::ProduccionHeredado),
            other => Err(SalesStoreError::Corrupt(format!(
                "entorno de un registro Verifactu {other}"
            ))),
        }
    }

    /// Entorno del QR (D248): simulador y preproducción llevan el QR de pruebas; producción y
    /// los registros heredados, el de producción.
    pub fn qr_entorno(self) -> Entorno {
        match self {
            Self::Simulador | Self::Preproduccion => Entorno::Pruebas,
            Self::Produccion | Self::ProduccionHeredado => Entorno::Produccion,
        }
    }
}

impl From<VerifactuEnvironment> for RecordEnvironment {
    fn from(e: VerifactuEnvironment) -> Self {
        match e {
            VerifactuEnvironment::Simulador => Self::Simulador,
            VerifactuEnvironment::Preproduccion => Self::Preproduccion,
            VerifactuEnvironment::Produccion => Self::Produccion,
        }
    }
}

pub(super) fn coded(code: &str, message: impl Into<String>) -> SalesStoreError {
    SalesStoreError::External {
        code: code.to_owned(),
        message: message.into(),
    }
}

/// Entorno vigente: el último cambio; sin cambios, `simulador` (D248).
fn current_environment(conn: &Connection) -> SalesResult<VerifactuEnvironment> {
    let text: Option<String> = conn
        .query_row(
            "SELECT environment FROM verifactu_environment_changes ORDER BY id DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .optional()?;
    match text {
        None => Ok(VerifactuEnvironment::Simulador),
        Some(t) => VerifactuEnvironment::from_stored(&t),
    }
}

/// Comprueba que el entorno que llevaba quien llamó sigue siendo el vigente.
pub(super) fn ensure_context_environment(
    conn: &Connection,
    expected: VerifactuEnvironment,
) -> SalesResult<()> {
    let current = current_environment(conn)?;
    if current != expected {
        return Err(coded(
            CODE_ENVIRONMENT_CHANGED,
            format!(
                "el entorno de Verifactu cambió a {} mientras se generaba el registro; reintente",
                current.as_str()
            ),
        ));
    }
    Ok(())
}

pub(super) fn insert_record_environment(
    conn: &Connection,
    record_id: i64,
    environment: VerifactuEnvironment,
) -> SalesResult<()> {
    conn.execute(
        "INSERT INTO verifactu_record_environments (record_id, environment) VALUES (?1, ?2)",
        params![record_id, environment.as_str()],
    )?;
    Ok(())
}

/// Un registro sin fila de entorno es un error (D258), no un «producción» silencioso.
fn ensure_all_records_have_environment(conn: &Connection) -> SalesResult<()> {
    let missing: i64 = conn.query_row(
        "SELECT count(*) FROM verifactu_records r WHERE NOT EXISTS \
         (SELECT 1 FROM verifactu_record_environments e WHERE e.record_id = r.id)",
        [],
        |r| r.get(0),
    )?;
    if missing > 0 {
        return Err(SalesStoreError::Corrupt(format!(
            "{missing} registro(s) Verifactu sin entorno guardado"
        )));
    }
    Ok(())
}

// --- Estado derivado ---

/// Estado de un registro, derivado de los resultados guardados. Prioridad (ajuste 6, D249):
/// `Accepted`/`AcceptedWithErrors` (definitivo) > `Sending` (en un lote abierto) > `Subsanated`
/// (tiene vínculo) > `Rejected` > `UnexpectedDuplicate` > `Pending`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VerifactuStatus {
    Pending,
    Sending,
    Accepted,
    AcceptedWithErrors,
    Rejected,
    Subsanated,
    UnexpectedDuplicate,
}

impl VerifactuStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Sending => "sending",
            Self::Accepted => "accepted",
            Self::AcceptedWithErrors => "accepted_with_errors",
            Self::Rejected => "rejected",
            Self::Subsanated => "subsanated",
            Self::UnexpectedDuplicate => "unexpected_duplicate",
        }
    }
}

/// Estado derivado de un registro con lo que la interfaz muestra.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifactuRecordState {
    pub record_id: i64,
    pub status: VerifactuStatus,
    /// Código de la AEAT del último resultado que explica el estado (rechazo, aceptación con
    /// errores, duplicado).
    pub aeat_code: Option<u32>,
    pub aeat_description: Option<String>,
    /// Aceptado porque la AEAT ya lo tenía de un intento anterior de este lote (D252).
    pub duplicate: bool,
    /// Lote abierto (`Sending`) o del último resultado.
    pub batch_id: Option<i64>,
    /// Registro que lo subsana (`Subsanated`).
    pub subsanation_record_id: Option<i64>,
}

struct ResultRow {
    status: String,
    duplicate: bool,
    code: Option<u32>,
    description: Option<String>,
    batch_id: i64,
}

fn derive_state(conn: &Connection, record_id: i64) -> SalesResult<VerifactuRecordState> {
    let exists: i64 = conn.query_row(
        "SELECT count(*) FROM verifactu_records WHERE id = ?1",
        [record_id],
        |r| r.get(0),
    )?;
    if exists == 0 {
        return Err(SalesStoreError::not_found("registro Verifactu", record_id));
    }
    let mut stmt = conn.prepare(
        "SELECT status, duplicate, aeat_code, aeat_description, batch_id \
         FROM verifactu_record_results WHERE record_id = ?1 ORDER BY id",
    )?;
    let rows = stmt
        .query_map([record_id], |r| {
            Ok(ResultRow {
                status: r.get(0)?,
                duplicate: r.get::<_, i64>(1)? != 0,
                code: r
                    .get::<_, Option<i64>>(2)?
                    .and_then(|c| u32::try_from(c).ok()),
                description: r.get(3)?,
                batch_id: r.get(4)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let open_batch: Option<i64> = conn
        .query_row(
            "SELECT br.batch_id FROM verifactu_batch_records br WHERE br.record_id = ?1 AND NOT \
             EXISTS (SELECT 1 FROM verifactu_batch_results x WHERE x.batch_id = br.batch_id) \
             LIMIT 1",
            [record_id],
            |r| r.get(0),
        )
        .optional()?;
    let subsanation: Option<i64> = conn
        .query_row(
            "SELECT subsanation_record_id FROM verifactu_subsanations \
             WHERE rejected_record_id = ?1",
            [record_id],
            |r| r.get(0),
        )
        .optional()?;
    let last = |wanted: &[&str]| {
        rows.iter()
            .rev()
            .find(|r| wanted.contains(&r.status.as_str()))
    };
    let (status, row) = if let Some(r) = last(&["accepted", "accepted_with_errors"]) {
        let s = if r.status == "accepted" {
            VerifactuStatus::Accepted
        } else {
            VerifactuStatus::AcceptedWithErrors
        };
        (s, Some(r))
    } else if open_batch.is_some() {
        (VerifactuStatus::Sending, None)
    } else if subsanation.is_some() {
        (VerifactuStatus::Subsanated, last(&["rejected"]))
    } else if let Some(r) = last(&["rejected"]) {
        (VerifactuStatus::Rejected, Some(r))
    } else if let Some(r) = last(&["unexpected_duplicate"]) {
        (VerifactuStatus::UnexpectedDuplicate, Some(r))
    } else {
        (VerifactuStatus::Pending, None)
    };
    Ok(VerifactuRecordState {
        record_id,
        status,
        aeat_code: row.and_then(|r| r.code),
        aeat_description: row.and_then(|r| r.description.clone()),
        duplicate: row.map(|r| r.duplicate).unwrap_or(false),
        batch_id: open_batch.or_else(|| row.map(|r| r.batch_id)),
        subsanation_record_id: subsanation,
    })
}

/// ¿Consta (o va a constar) en la AEAT algún alta de la factura? Pendiente, en envío,
/// aceptada o duplicada cuentan; solo las rechazadas no. Decide `SinRegistroPrevio` de la
/// anulación (D253).
pub(super) fn registered_alta_exists(conn: &Connection, document_id: i64) -> SalesResult<bool> {
    let ids: Vec<i64> = {
        let mut stmt = conn.prepare(
            "SELECT id FROM verifactu_records WHERE document_id = ?1 AND kind = 'alta' \
             ORDER BY id",
        )?;
        let rows = stmt
            .query_map([document_id], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<i64>>>()?;
        rows
    };
    for id in ids {
        match derive_state(conn, id)?.status {
            VerifactuStatus::Rejected | VerifactuStatus::Subsanated => {}
            _ => return Ok(true),
        }
    }
    Ok(false)
}

// --- Pendientes y lotes ---

const PENDING_TAIL: &str = "WHERE id IN (SELECT record_id FROM verifactu_record_environments \
     WHERE environment IN (?1, ?2)) \
     AND NOT EXISTS (SELECT 1 FROM verifactu_record_results x WHERE x.record_id = \
     verifactu_records.id) \
     AND NOT EXISTS (SELECT 1 FROM verifactu_batch_records br WHERE br.record_id = \
     verifactu_records.id AND NOT EXISTS (SELECT 1 FROM verifactu_batch_results z \
     WHERE z.batch_id = br.batch_id)) \
     ORDER BY id";

fn pending_rows(
    conn: &Connection,
    environment: VerifactuEnvironment,
) -> SalesResult<Vec<VerifactuRecordRow>> {
    ensure_all_records_have_environment(conn)?;
    let (a, b) = environment.record_environments();
    rows_matching(conn, PENDING_TAIL, params![a, b])
}

/// Un lote de remisión tal como se guardó: con él se reconstruyen los mismos bytes del envío.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifactuBatch {
    pub id: i64,
    pub environment: VerifactuEnvironment,
    pub idempotency_key: String,
    /// Registros del lote en orden de cadena.
    pub record_ids: Vec<i64>,
    /// Cabecera y registros del envío, tal como se mandan (`a_soap()` da siempre los mismos
    /// bytes: los registros son inmutables).
    pub envio: Envio,
    /// Intentos de envío anotados (ver [`Store::begin_verifactu_attempt`]).
    pub attempts: u32,
    pub started_at: i64,
}

/// Cómo terminó un lote.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchOutcome {
    /// Respuesta de la AEAT entendida (con líneas por registro).
    Responded,
    /// `Fault` SOAP: error de esquema o de cabecera (D259).
    Fault,
}

/// Resultado guardado de un lote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifactuBatchResult {
    pub outcome: BatchOutcome,
    /// `Correcto`, `ParcialmenteCorrecto` o `Incorrecto` (solo con respuesta).
    pub send_state: Option<String>,
    pub error_code: Option<u32>,
    pub error_description: Option<String>,
    pub wait_seconds: u32,
    pub csv: Option<String>,
    pub finished_at: i64,
}

/// Resumen de un lote para mostrarlo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifactuBatchSummary {
    pub id: i64,
    pub environment: VerifactuEnvironment,
    pub record_count: u32,
    pub started_at: i64,
    /// Intentos que cuentan como envío (D252, D280).
    pub attempts: u32,
    /// Intentos sin envío (el cuerpo no llegó a escribirse): no cuentan (D280).
    pub unsent_attempts: u32,
    /// `None` mientras el lote está abierto.
    pub result: Option<VerifactuBatchResult>,
}

/// Lo que dejó el cierre de un lote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchClosure {
    pub batch_id: i64,
    pub outcome: BatchOutcome,
    pub accepted: u32,
    pub accepted_with_errors: u32,
    pub rejected: u32,
    pub unexpected_duplicates: u32,
    /// Registros del lote sin línea en la respuesta: siguen pendientes (explícito, nunca
    /// aceptados).
    pub missing_record_ids: Vec<i64>,
    /// Líneas de la respuesta que no corresponden a ningún registro del lote (o repetidas).
    pub unmatched_lines: u32,
    pub wait_seconds: u32,
    pub csv: Option<String>,
    pub fault_code: Option<u32>,
}

fn id_key(row: &VerifactuRecordRow) -> (String, String, String) {
    let id = row.record.id_factura();
    (
        id.nif_emisor.clone(),
        id.num_serie.clone(),
        id.fecha_expedicion.to_string(),
    )
}

fn open_batch_ids(conn: &Connection) -> SalesResult<Vec<i64>> {
    let mut stmt = conn.prepare(
        "SELECT b.id FROM verifactu_batches b WHERE NOT EXISTS \
         (SELECT 1 FROM verifactu_batch_results r WHERE r.batch_id = b.id) ORDER BY b.id",
    )?;
    let ids = stmt
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<Vec<i64>>>()?;
    Ok(ids)
}

/// Intentos que cuentan como envío: los anotados menos los que el transporte anuló por no haber
/// escrito el cuerpo (`verifactu_unsent_attempts`, D280).
pub(super) fn attempts_of(conn: &Connection, batch_id: i64) -> SalesResult<u32> {
    let n: i64 = conn.query_row(
        "SELECT count(*) FROM verifactu_batch_attempts a WHERE a.batch_id = ?1 AND NOT EXISTS          (SELECT 1 FROM verifactu_unsent_attempts u WHERE u.attempt_id = a.id)",
        [batch_id],
        |r| r.get(0),
    )?;
    Ok(u32::try_from(n).unwrap_or(u32::MAX))
}

fn unsent_of(conn: &Connection, batch_id: i64) -> SalesResult<u32> {
    let n: i64 = conn.query_row(
        "SELECT count(*) FROM verifactu_unsent_attempts WHERE batch_id = ?1",
        [batch_id],
        |r| r.get(0),
    )?;
    Ok(u32::try_from(n).unwrap_or(u32::MAX))
}

/// Intento anotado antes de la llamada, con su identificador por si hay que anularlo (D280).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VerifactuAttemptTicket {
    pub attempt_id: i64,
    /// Número de intento que cuenta tras anotarlo.
    pub counted: u32,
}

fn load_batch(conn: &Connection, batch_id: i64) -> SalesResult<VerifactuBatch> {
    let (environment, key, header_json, started_at): (String, String, String, i64) = conn
        .query_row(
            "SELECT environment, idempotency_key, header_json, started_at \
             FROM verifactu_batches WHERE id = ?1",
            [batch_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?
        .ok_or_else(|| SalesStoreError::not_found("lote Verifactu", batch_id))?;
    let cabecera: Cabecera = serde_json::from_str(&header_json)
        .map_err(|_| SalesStoreError::Corrupt(format!("cabecera del lote {batch_id}")))?;
    let record_ids: Vec<i64> = {
        let mut stmt = conn.prepare(
            "SELECT record_id FROM verifactu_batch_records WHERE batch_id = ?1 ORDER BY position",
        )?;
        let ids = stmt
            .query_map([batch_id], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<i64>>>()?;
        ids
    };
    let mut registros = Vec::with_capacity(record_ids.len());
    for id in &record_ids {
        let rows = rows_matching(conn, "WHERE id = ?1", [id])?;
        let row = rows
            .into_iter()
            .next()
            .ok_or_else(|| SalesStoreError::not_found("registro Verifactu", *id))?;
        registros.push(row.record);
    }
    Ok(VerifactuBatch {
        id: batch_id,
        environment: VerifactuEnvironment::from_stored(&environment)?,
        idempotency_key: key,
        record_ids,
        envio: Envio {
            cabecera,
            registros,
        },
        attempts: attempts_of(conn, batch_id)?,
        started_at,
    })
}

pub(super) fn audit_remission(
    conn: &Connection,
    action: &str,
    authorization: &str,
    detail: serde_json::Value,
) -> SalesResult<()> {
    conn.execute(
        "INSERT INTO audit_log (at, task_id, action, status, authorization, detail) \
         VALUES (?1, NULL, ?2, 'ok', ?3, ?4)",
        params![now_ms(), action, authorization, detail.to_string()],
    )?;
    Ok(())
}

/// `Fault` de esquema seguidos desde el último lote respondido (D259, D281). Los `Fault`
/// técnicos no cuentan ni cortan la racha (nunca paran la remisión) y uno de autorización la
/// reinicia: su paro es otro (`verifactu_authorization_halts`).
fn consecutive_faults(conn: &Connection, environment: VerifactuEnvironment) -> SalesResult<u32> {
    let mut stmt = conn.prepare(
        "SELECT r.outcome, r.error_code FROM verifactu_batch_results r JOIN verifactu_batches b \
         ON b.id = r.batch_id WHERE b.environment = ?1 ORDER BY r.batch_id DESC",
    )?;
    let rows = stmt
        .query_map([environment.as_str()], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Option<i64>>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut count = 0u32;
    for (outcome, code) in rows {
        if outcome != "fault" {
            break;
        }
        match FaultClass::of(code.and_then(|c| u32::try_from(c).ok())) {
            FaultClass::Schema => count += 1,
            FaultClass::Technical => {}
            FaultClass::Authorization => break,
        }
    }
    Ok(count)
}

/// Resultado de una línea de respuesta ya clasificado.
struct Classified {
    status: &'static str,
    duplicate: bool,
}

/// Clasifica una línea de la respuesta (D252, ajustes 2 y 3). `resent`: el lote ya se había
/// enviado antes sin recibir resultado (más de un intento), así que la AEAT puede tener lo que
/// ahora declara duplicado o ya dado de baja.
fn classify(
    line: &nestadmin_verifactu::RespuestaLinea,
    kind: VerifactuKind,
    resent: bool,
) -> Classified {
    let plain = |status| Classified {
        status,
        duplicate: false,
    };
    match line.estado {
        EstadoRegistro::Correcto => plain("accepted"),
        EstadoRegistro::AceptadoConErrores => plain("accepted_with_errors"),
        EstadoRegistro::Incorrecto => {
            if line.codigo_error == Some(3000) || line.duplicado.is_some() {
                // El duplicado no trae huella: solo cuenta como nuestro si ese registro estuvo
                // en un lote de esta base sin resultado. Una alta que la AEAT tiene «Anulada»
                // tampoco es aceptada sin más.
                match (&line.duplicado, kind, resent) {
                    (Some(d), VerifactuKind::Alta, true) if d.estado == "Correcta" => Classified {
                        status: "accepted",
                        duplicate: true,
                    },
                    (Some(d), VerifactuKind::Alta, true) if d.estado == "AceptadaConErrores" => {
                        Classified {
                            status: "accepted_with_errors",
                            duplicate: true,
                        }
                    }
                    _ => plain("unexpected_duplicate"),
                }
            } else if kind == VerifactuKind::Anulacion && line.codigo_error == Some(3001) && resent
            {
                // Una baja reenviada recibe «ya dado de baja» (3001), no 3000 (ajuste 2).
                Classified {
                    status: "accepted",
                    duplicate: true,
                }
            } else {
                plain("rejected")
            }
        }
    }
}

/// Subsanación creada (o ya existente) de un registro rechazado.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifactuSubsanation {
    pub rejected_record_id: i64,
    /// Registro nuevo que lo subsana.
    pub record_id: i64,
    /// `false` si ya existía (doble llamada = un solo registro).
    pub created: bool,
}

impl Store {
    // --- Entorno ---

    /// Entorno vigente (D248): el último cambio; sin cambios, `simulador`.
    pub fn verifactu_environment(&self) -> SalesResult<VerifactuEnvironment> {
        current_environment(&self.conn)
    }

    /// Entorno guardado con un registro. Una fila ausente es un error (D258).
    pub fn verifactu_record_environment(&self, record_id: i64) -> SalesResult<RecordEnvironment> {
        let text: Option<String> = self
            .conn
            .query_row(
                "SELECT environment FROM verifactu_record_environments WHERE record_id = ?1",
                [record_id],
                |r| r.get(0),
            )
            .optional()?;
        match text {
            Some(t) => RecordEnvironment::from_stored(&t),
            None => Err(SalesStoreError::Corrupt(format!(
                "el registro Verifactu {record_id} no tiene entorno guardado"
            ))),
        }
    }

    /// Cambia el entorno vigente y devuelve si cambió (D256). Prohibido con registros
    /// pendientes del entorno actual, un lote en envío o registros rechazados sin subsanar
    /// (`verifactu_environment_busy`). `authorization` deja constancia en el historial
    /// (D85: ajuste y confirmación explícitos). Si el entorno ya es el pedido, no hace nada.
    pub fn set_verifactu_environment(
        &mut self,
        environment: VerifactuEnvironment,
        authorization: &str,
    ) -> SalesResult<bool> {
        let authorization = authorization.trim();
        if authorization.is_empty() {
            return Err(SalesStoreError::invalid(
                "authorization",
                "falta la autorización del cambio de entorno",
            ));
        }
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = current_environment(&tx)?;
        if current == environment {
            return Ok(false);
        }
        let pending = pending_rows(&tx, current)?.len();
        let sending = open_batch_ids(&tx)?.len();
        let (a, b) = current.record_environments();
        let rejected_candidates: Vec<i64> = {
            let mut stmt = tx.prepare(
                "SELECT DISTINCT x.record_id FROM verifactu_record_results x \
                 JOIN verifactu_record_environments e ON e.record_id = x.record_id \
                 WHERE x.status = 'rejected' AND e.environment IN (?1, ?2) ORDER BY x.record_id",
            )?;
            let ids = stmt
                .query_map(params![a, b], |r| r.get(0))?
                .collect::<rusqlite::Result<Vec<i64>>>()?;
            ids
        };
        let mut rejected = 0usize;
        for id in rejected_candidates {
            if derive_state(&tx, id)?.status == VerifactuStatus::Rejected {
                rejected += 1;
            }
        }
        if pending + sending + rejected > 0 {
            return Err(coded(
                CODE_ENVIRONMENT_BUSY,
                format!(
                    "no se cambia de entorno con registros por remitir ({pending} pendientes, \
                     {sending} lotes en envío, {rejected} rechazados sin subsanar)"
                ),
            ));
        }
        tx.execute(
            "INSERT INTO verifactu_environment_changes (environment, authorization, changed_at) \
             VALUES (?1, ?2, ?3)",
            params![environment.as_str(), authorization, now_ms()],
        )?;
        audit_remission(
            &tx,
            "verifactu.environment",
            authorization,
            serde_json::json!({ "from": current.as_str(), "to": environment.as_str() }),
        )?;
        tx.commit()?;
        Ok(true)
    }

    // --- Estado derivado ---

    /// Estado derivado de un registro.
    pub fn verifactu_record_state(&self, record_id: i64) -> SalesResult<VerifactuRecordState> {
        derive_state(&self.conn, record_id)
    }

    /// Estado derivado de todos los registros, en orden de cadena.
    pub fn verifactu_record_states(&self) -> SalesResult<Vec<VerifactuRecordState>> {
        let ids: Vec<i64> = {
            let mut stmt = self
                .conn
                .prepare("SELECT id FROM verifactu_records ORDER BY id")?;
            let ids = stmt
                .query_map([], |r| r.get(0))?
                .collect::<rusqlite::Result<Vec<i64>>>()?;
            ids
        };
        ids.into_iter()
            .map(|id| derive_state(&self.conn, id))
            .collect()
    }

    /// Registros pendientes del entorno, en orden de cadena. Los heredados (D258) cuentan como
    /// producción.
    pub fn verifactu_pending_records(
        &self,
        environment: VerifactuEnvironment,
    ) -> SalesResult<Vec<VerifactuRecordRow>> {
        pending_rows(&self.conn, environment)
    }

    /// Registros aceptados (con o sin errores) del entorno, en orden de cadena: lo que hay que
    /// sembrar en el simulador al arrancarlo para que no dé falsos rechazos de anulación tras
    /// reabrir la aplicación (ajuste 10).
    pub fn verifactu_accepted_records(
        &self,
        environment: VerifactuEnvironment,
    ) -> SalesResult<Vec<RegistroFacturacion>> {
        ensure_all_records_have_environment(&self.conn)?;
        let (a, b) = environment.record_environments();
        let rows = rows_matching(
            &self.conn,
            "WHERE id IN (SELECT record_id FROM verifactu_record_environments \
             WHERE environment IN (?1, ?2)) \
             AND EXISTS (SELECT 1 FROM verifactu_record_results x WHERE x.record_id = \
             verifactu_records.id AND x.status IN ('accepted', 'accepted_with_errors')) \
             ORDER BY id",
            params![a, b],
        )?;
        Ok(rows.into_iter().map(|r| r.record).collect())
    }

    /// Como [`Store::verifactu_accepted_records`], con si el último resultado del registro fue
    /// «aceptado con errores» (el simulador lo conserva al sembrarlo).
    pub fn verifactu_accepted_seed(
        &self,
        environment: VerifactuEnvironment,
    ) -> SalesResult<Vec<(RegistroFacturacion, bool)>> {
        ensure_all_records_have_environment(&self.conn)?;
        let (a, b) = environment.record_environments();
        let rows = rows_matching(
            &self.conn,
            "WHERE id IN (SELECT record_id FROM verifactu_record_environments              WHERE environment IN (?1, ?2))              AND EXISTS (SELECT 1 FROM verifactu_record_results x WHERE x.record_id =              verifactu_records.id AND x.status IN ('accepted', 'accepted_with_errors'))              ORDER BY id",
            params![a, b],
        )?;
        let mut seed = Vec::with_capacity(rows.len());
        for row in rows {
            let last: Option<String> = self
                .conn
                .query_row(
                    "SELECT status FROM verifactu_record_results WHERE record_id = ?1                      AND status IN ('accepted', 'accepted_with_errors')                      ORDER BY rowid DESC LIMIT 1",
                    [row.id],
                    |r| r.get(0),
                )
                .optional()?;
            seed.push((row.record, last.as_deref() == Some("accepted_with_errors")));
        }
        Ok(seed)
    }

    // --- Lotes ---

    /// El lote abierto (sin resultado), si lo hay, para reenviarlo idéntico (D252).
    pub fn unfinished_verifactu_batch(&self) -> SalesResult<Option<VerifactuBatch>> {
        let ids = open_batch_ids(&self.conn)?;
        match ids.as_slice() {
            [] => Ok(None),
            [id] => Ok(Some(load_batch(&self.conn, *id)?)),
            _ => Err(SalesStoreError::Corrupt(
                "hay más de un lote de Verifactu abierto".into(),
            )),
        }
    }

    /// Abre un lote con los registros pendientes del entorno, en orden de cadena, o `None` si
    /// no hay ninguno. La transacción es inmediata y comprueba que no haya otro lote abierto
    /// (`verifactu_batch_open`): nunca hay dos a la vez (ajuste 4). Composición (ajuste 7): a lo
    /// sumo 1000 registros y nunca dos con la misma factura (el lote se corta antes del
    /// segundo), de modo que el alta y la anulación de una factura van en lotes distintos.
    /// `manual`: la pide el usuario; sin ella, tras [`MAX_AUTOMATIC_FAULTS`] `Fault` seguidos no
    /// se abre ninguno (`verifactu_batch_rejected`, D259).
    pub fn open_verifactu_batch(
        &mut self,
        environment: VerifactuEnvironment,
        header: &Cabecera,
        now_ms: i64,
        manual: bool,
    ) -> SalesResult<Option<VerifactuBatch>> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        ensure_context_environment(&tx, environment)?;
        if !open_batch_ids(&tx)?.is_empty() {
            return Err(coded(
                CODE_BATCH_OPEN,
                "hay un lote de Verifactu sin cerrar: se reenvía el mismo, no se abre otro",
            ));
        }
        if !manual && consecutive_faults(&tx, environment)? >= MAX_AUTOMATIC_FAULTS {
            return Err(coded(
                CODE_BATCH_REJECTED,
                format!(
                    "la AEAT rechazó {MAX_AUTOMATIC_FAULTS} envíos seguidos por error de esquema \
                     o de cabecera; la remisión automática se detiene hasta que el usuario la \
                     reintente"
                ),
            ));
        }
        let pending = pending_rows(&tx, environment)?;
        let mut seen = HashSet::new();
        let mut chosen: Vec<VerifactuRecordRow> = Vec::new();
        for row in pending {
            // El lote se corta ante otro emisor (la cabecera lleva uno solo) igual que ante una
            // factura repetida: los de otro NIF van en el lote siguiente.
            let other_issuer = chosen
                .first()
                .is_some_and(|first| first.issuer_tax_id != row.issuer_tax_id);
            if chosen.len() >= MAX_REGISTROS_POR_ENVIO || other_issuer || !seen.insert(id_key(&row))
            {
                break;
            }
            chosen.push(row);
        }
        if chosen.is_empty() {
            return Ok(None);
        }
        // `Incidencia = S` se decide ahora, al abrir el lote (D282): si alguno de sus registros
        // se generó durante una incidencia abierta. Un lote ya abierto se reenvía con los
        // mismos bytes y no se recalcula nada.
        let mut header = header.clone();
        if generated_during_incident(&tx, &chosen)? {
            let mut voluntary = header.remision_voluntaria.take().unwrap_or_default();
            voluntary.incidencia = Some(true);
            header.remision_voluntaria = Some(voluntary);
        }
        let header = &header;
        let envio = Envio {
            cabecera: header.clone(),
            registros: chosen.iter().map(|r| r.record.clone()).collect(),
        };
        envio.validar().map_err(record_error)?;
        let next_id: i64 = tx.query_row(
            "SELECT COALESCE(MAX(id), 0) + 1 FROM verifactu_batches",
            [],
            |r| r.get(0),
        )?;
        let key = format!("verifactu/{}/lote-{next_id}", environment.as_str());
        let authorization = format!("{OBLIGATION_AUTHORIZATION} {}", environment.as_str());
        tx.execute(
            "INSERT INTO verifactu_batches (id, environment, idempotency_key, header_json, \
             record_count, authorization, started_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                next_id,
                environment.as_str(),
                key,
                serde_json::to_string(header)?,
                chosen.len() as i64,
                authorization,
                now_ms,
            ],
        )?;
        for (position, row) in chosen.iter().enumerate() {
            tx.execute(
                "INSERT INTO verifactu_batch_records (batch_id, position, record_id) \
                 VALUES (?1, ?2, ?3)",
                params![next_id, position as i64, row.id],
            )?;
        }
        audit_remission(
            &tx,
            "verifactu.batch_open",
            &authorization,
            serde_json::json!({
                "batch_id": next_id,
                "records": chosen.len(),
                "first_record": chosen.first().map(|r| r.id),
                "last_record": chosen.last().map(|r| r.id),
            }),
        )?;
        let batch = load_batch(&tx, next_id)?;
        tx.commit()?;
        Ok(Some(batch))
    }

    /// Anota un intento de envío del lote, ANTES de la llamada (D252): sin esa anotación, un
    /// corte durante el envío no dejaría rastro de que el lote pudo llegar a la AEAT. Devuelve
    /// el número de intento que cuenta (1 el primero). Falla si el lote no existe o ya está
    /// cerrado. Si después el transporte dice que el cuerpo no llegó a escribirse, quien llama
    /// lo anula con [`Store::void_verifactu_attempt`] (D280).
    pub fn begin_verifactu_attempt(&mut self, batch_id: i64, now_ms: i64) -> SalesResult<u32> {
        Ok(self
            .begin_verifactu_attempt_ticket(batch_id, now_ms)?
            .counted)
    }

    /// Como [`Store::begin_verifactu_attempt`], pero devuelve también el identificador del
    /// intento anotado, para poder anularlo (D280).
    pub fn begin_verifactu_attempt_ticket(
        &mut self,
        batch_id: i64,
        now_ms: i64,
    ) -> SalesResult<VerifactuAttemptTicket> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        ensure_batch_open(&tx, batch_id)?;
        tx.execute(
            "INSERT INTO verifactu_batch_attempts (batch_id, attempted_at) VALUES (?1, ?2)",
            params![batch_id, now_ms],
        )?;
        let attempt_id = tx.last_insert_rowid();
        let counted = attempts_of(&tx, batch_id)?;
        tx.commit()?;
        Ok(VerifactuAttemptTicket {
            attempt_id,
            counted,
        })
    }

    /// Cierra el lote con una respuesta entendida o un `Fault` SOAP y guarda el resultado de
    /// cada registro (D249). Solo cierra un lote abierto con al menos un intento anotado
    /// (cerrarlo dos veces no repite ningún efecto: la segunda da `conflict`). Un duplicado (3000)
    /// cuenta como aceptado solo si el lote ya se había enviado antes sin resultado; en otro
    /// caso queda como `unexpected_duplicate`. Una baja con 3001 en un lote reenviado cuenta
    /// como aceptada (ajuste 2). Un registro sin línea en la respuesta queda pendiente.
    pub fn close_verifactu_batch(
        &mut self,
        batch_id: i64,
        result: &ResultadoEnvio,
        now_ms: i64,
    ) -> SalesResult<BatchClosure> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        ensure_batch_open(&tx, batch_id)?;
        let attempts = attempts_of(&tx, batch_id)?;
        if attempts == 0 {
            return Err(SalesStoreError::Conflict(format!(
                "el lote {batch_id} no tiene ningún intento de envío anotado"
            )));
        }
        let batch = load_batch(&tx, batch_id)?;
        let mut closure = BatchClosure {
            batch_id,
            outcome: BatchOutcome::Fault,
            accepted: 0,
            accepted_with_errors: 0,
            rejected: 0,
            unexpected_duplicates: 0,
            missing_record_ids: Vec::new(),
            unmatched_lines: 0,
            wait_seconds: MIN_RETRY_WAIT_SECONDS,
            csv: None,
            fault_code: None,
        };
        match result {
            ResultadoEnvio::Fallo(f) => {
                tx.execute(
                    "INSERT INTO verifactu_batch_results (batch_id, outcome, send_state, \
                     error_code, error_description, wait_seconds, csv, finished_at) \
                     VALUES (?1, 'fault', NULL, ?2, ?3, ?4, NULL, ?5)",
                    params![
                        batch_id,
                        f.codigo.map(i64::from),
                        f.faultstring,
                        MIN_RETRY_WAIT_SECONDS,
                        now_ms
                    ],
                )?;
                closure.fault_code = f.codigo;
                closure.missing_record_ids = batch.record_ids.clone();
                match FaultClass::of(f.codigo) {
                    // El certificado no está autorizado o el `Representante` no vale: la
                    // remisión de esta base se para hasta que el usuario actúe (D281).
                    FaultClass::Authorization => {
                        halt_authorization(&tx, batch_id, f.codigo.unwrap_or(0), now_ms)?;
                    }
                    // Técnico: nunca para; se anota la incidencia (D282).
                    FaultClass::Technical => {
                        open_incident(
                            &tx,
                            &format!("fault_{}", f.codigo.unwrap_or(0)),
                            Some(batch_id),
                            now_ms,
                        )?;
                    }
                    FaultClass::Schema => {}
                }
            }
            ResultadoEnvio::Respuesta(r) => {
                closure.outcome = BatchOutcome::Responded;
                // Un envío con respuesta cierra la incidencia abierta (D282).
                close_incident(&tx, batch_id, now_ms)?;
                closure.wait_seconds = r.tiempo_espera_envio;
                closure.csv = r.csv.clone();
                let send_state = format!("{:?}", r.estado_envio);
                tx.execute(
                    "INSERT INTO verifactu_batch_results (batch_id, outcome, send_state, \
                     error_code, error_description, wait_seconds, csv, finished_at) \
                     VALUES (?1, 'responded', ?2, NULL, NULL, ?3, ?4, ?5)",
                    params![batch_id, send_state, r.tiempo_espera_envio, r.csv, now_ms],
                )?;
                // Registros del lote por (factura, operación): únicos por la composición.
                let mut by_key: HashMap<((String, String, String), VerifactuKind), i64> =
                    HashMap::new();
                for id in &batch.record_ids {
                    let row = rows_matching(&tx, "WHERE id = ?1", [id])?
                        .into_iter()
                        .next()
                        .ok_or_else(|| SalesStoreError::not_found("registro Verifactu", *id))?;
                    by_key.insert((id_key(&row), row.kind), row.id);
                }
                // Un lote reenviado sin resultado (dos o más intentos) pudo llegar a la AEAT.
                let batch_resent = attempts >= 2;
                let mut answered = HashSet::new();
                for line in &r.lineas {
                    let kind = match line.tipo_operacion.as_str() {
                        "Alta" => VerifactuKind::Alta,
                        "Anulacion" => VerifactuKind::Anulacion,
                        _ => {
                            closure.unmatched_lines += 1;
                            continue;
                        }
                    };
                    let key = (
                        (
                            line.id_factura.nif_emisor.clone(),
                            line.id_factura.num_serie.clone(),
                            line.id_factura.fecha_expedicion.to_string(),
                        ),
                        kind,
                    );
                    let Some(&record_id) = by_key.get(&key) else {
                        closure.unmatched_lines += 1;
                        continue;
                    };
                    if !answered.insert(record_id) {
                        closure.unmatched_lines += 1;
                        continue;
                    }
                    let resent = batch_resent || was_sent_before(&tx, batch_id, record_id)?;
                    let class = classify(line, kind, resent);
                    match class.status {
                        "accepted" => closure.accepted += 1,
                        "accepted_with_errors" => closure.accepted_with_errors += 1,
                        "unexpected_duplicate" => closure.unexpected_duplicates += 1,
                        _ => closure.rejected += 1,
                    }
                    let mut description = line.descripcion_error.clone();
                    if class.status == "unexpected_duplicate" {
                        if let Some(d) = &line.duplicado {
                            description = Some(format!(
                                "{} (la AEAT lo tiene como {})",
                                description.unwrap_or_default().trim(),
                                d.estado
                            ));
                        }
                    }
                    tx.execute(
                        "INSERT INTO verifactu_record_results (batch_id, record_id, status, \
                         duplicate, aeat_code, aeat_description) \
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                        params![
                            batch_id,
                            record_id,
                            class.status,
                            i64::from(class.duplicate),
                            line.codigo_error.map(i64::from),
                            description,
                        ],
                    )?;
                }
                closure.missing_record_ids = batch
                    .record_ids
                    .iter()
                    .copied()
                    .filter(|id| !answered.contains(id))
                    .collect();
            }
        }
        let authorization = format!("{OBLIGATION_AUTHORIZATION} {}", batch.environment.as_str());
        audit_remission(
            &tx,
            "verifactu.batch_close",
            &authorization,
            serde_json::json!({
                "batch_id": batch_id,
                "outcome": match closure.outcome {
                    BatchOutcome::Responded => "responded",
                    BatchOutcome::Fault => "fault",
                },
                "accepted": closure.accepted,
                "accepted_with_errors": closure.accepted_with_errors,
                "rejected": closure.rejected,
                "unexpected_duplicates": closure.unexpected_duplicates,
                "missing": closure.missing_record_ids.len(),
                "fault_code": closure.fault_code,
            }),
        )?;
        tx.commit()?;
        Ok(closure)
    }

    /// `Fault` cerrados seguidos del entorno, contados desde el último lote respondido. A
    /// [`MAX_AUTOMATIC_FAULTS`] la remisión automática se detiene (`batch_rejected`, D259).
    pub fn verifactu_consecutive_faults(
        &self,
        environment: VerifactuEnvironment,
    ) -> SalesResult<u32> {
        consecutive_faults(&self.conn, environment)
    }

    /// Momento (milisegundos Unix) a partir del cual se puede enviar el siguiente lote, o
    /// `None` si no hay restricción (D251, D281): el fin del último lote cerrado más el tiempo
    /// de espera de la AEAT y, tras fallos seguidos (sin respuesta, sin envío o `Fault`
    /// técnico), el último fallo más una espera creciente de 60 s a una hora como mucho; el
    /// mayor de los dos. Es la espera de la remisión automática.
    pub fn verifactu_next_send_at(
        &self,
        environment: VerifactuEnvironment,
    ) -> SalesResult<Option<i64>> {
        self.verifactu_next_send_at_mode(environment, WaitMode::Automatic)
    }

    /// Como [`Store::verifactu_next_send_at`], según quién pregunta ([`WaitMode`]).
    pub fn verifactu_next_send_at_mode(
        &self,
        environment: VerifactuEnvironment,
        mode: WaitMode,
    ) -> SalesResult<Option<i64>> {
        let last_result: Option<(i64, i64)> = self
            .conn
            .query_row(
                "SELECT r.finished_at, r.wait_seconds FROM verifactu_batch_results r \
                 JOIN verifactu_batches b ON b.id = r.batch_id WHERE b.environment = ?1 \
                 ORDER BY r.batch_id DESC LIMIT 1",
                [environment.as_str()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let from_result = last_result.map(|(at, wait)| at + wait * 1000);
        let from_failure = retry_failure_wait(&self.conn, environment, mode)?;
        Ok(match (from_result, from_failure) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        })
    }

    /// Resumen del último lote del entorno (abierto o cerrado), para mostrarlo.
    pub fn verifactu_last_batch(
        &self,
        environment: VerifactuEnvironment,
    ) -> SalesResult<Option<VerifactuBatchSummary>> {
        let id: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM verifactu_batches WHERE environment = ?1 ORDER BY id DESC LIMIT 1",
                [environment.as_str()],
                |r| r.get(0),
            )
            .optional()?;
        let Some(id) = id else { return Ok(None) };
        self.verifactu_batch_summary(id)
    }

    /// Resumen de un lote concreto (abierto o cerrado), o `None` si no existe.
    pub fn verifactu_batch_summary(&self, id: i64) -> SalesResult<Option<VerifactuBatchSummary>> {
        let head: Option<(i64, i64, String)> = self
            .conn
            .query_row(
                "SELECT record_count, started_at, environment FROM verifactu_batches                  WHERE id = ?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let Some((record_count, started_at, environment)) = head else {
            return Ok(None);
        };
        let environment = VerifactuEnvironment::from_stored(&environment)?;
        #[allow(clippy::type_complexity)]
        let result: Option<(
            String,
            Option<String>,
            Option<i64>,
            Option<String>,
            i64,
            Option<String>,
            i64,
        )> = self
            .conn
            .query_row(
                "SELECT outcome, send_state, error_code, error_description, wait_seconds, csv, \
                 finished_at FROM verifactu_batch_results WHERE batch_id = ?1",
                [id],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                    ))
                },
            )
            .optional()?;
        let result = result.map(
            |(outcome, send_state, code, description, wait, csv, finished_at)| {
                VerifactuBatchResult {
                    outcome: if outcome == "fault" {
                        BatchOutcome::Fault
                    } else {
                        BatchOutcome::Responded
                    },
                    send_state,
                    error_code: code.and_then(|c| u32::try_from(c).ok()),
                    error_description: description,
                    wait_seconds: u32::try_from(wait).unwrap_or(u32::MAX),
                    csv,
                    finished_at,
                }
            },
        );
        Ok(Some(VerifactuBatchSummary {
            id,
            environment,
            record_count: u32::try_from(record_count).unwrap_or(0),
            started_at,
            attempts: attempts_of(&self.conn, id)?,
            unsent_attempts: unsent_of(&self.conn, id)?,
            result,
        }))
    }

    // --- Subsanación ---

    /// Crea la subsanación de un registro rechazado (D253), idempotente: una segunda llamada
    /// devuelve el mismo registro con `created = false`.
    ///
    /// * Alta rechazada: alta nueva de la misma factura con `Subsanacion = S` y
    ///   `RechazoPrevio = X`, con los mismos datos (corregirlos queda fuera de E5: la vía es
    ///   una rectificativa). Si la factura ya tiene una anulación, no se subsana el alta.
    /// * Anulación rechazada: anulación nueva con `RechazoPrevio = S` y `SinRegistroPrevio`
    ///   según conste o no un alta en la AEAT.
    ///
    /// Se encadena como cualquier otro registro (cadena íntegra y reloj, D195) y queda
    /// pendiente de remitir; el rechazado no cambia. Solo se subsana un registro `rejected`
    /// (no uno aceptado con errores, en envío o pendiente) del entorno vigente.
    pub fn create_verifactu_subsanation(
        &mut self,
        rejected_record_id: i64,
        ctx: &VerifactuContext,
    ) -> SalesResult<VerifactuSubsanation> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing: Option<i64> = tx
            .query_row(
                "SELECT subsanation_record_id FROM verifactu_subsanations \
                 WHERE rejected_record_id = ?1",
                [rejected_record_id],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(record_id) = existing {
            return Ok(VerifactuSubsanation {
                rejected_record_id,
                record_id,
                created: false,
            });
        }
        let row = rows_matching(&tx, "WHERE id = ?1", [rejected_record_id])?
            .into_iter()
            .next()
            .ok_or_else(|| SalesStoreError::not_found("registro Verifactu", rejected_record_id))?;
        let state = derive_state(&tx, rejected_record_id)?;
        if state.status != VerifactuStatus::Rejected {
            return Err(SalesStoreError::Conflict(format!(
                "el registro {rejected_record_id} está «{}»: solo se subsana uno rechazado",
                state.status.as_str()
            )));
        }
        ensure_context_environment(&tx, ctx.environment)?;
        let stored: RecordEnvironment = {
            let text: String = tx.query_row(
                "SELECT environment FROM verifactu_record_environments WHERE record_id = ?1",
                [rejected_record_id],
                |r| r.get(0),
            )?;
            RecordEnvironment::from_stored(&text)?
        };
        if stored != RecordEnvironment::from(ctx.environment) {
            return Err(SalesStoreError::Conflict(format!(
                "el registro {rejected_record_id} es del entorno {} y el vigente es {}",
                stored.as_str(),
                ctx.environment.as_str()
            )));
        }
        let (chain, previous) = next_link(&tx, ctx)?;
        let record = match row.record {
            RegistroFacturacion::Alta(mut alta) => {
                let voided: i64 = tx.query_row(
                    "SELECT count(*) FROM verifactu_records \
                     WHERE document_id = ?1 AND kind = 'anulacion'",
                    [row.document_id],
                    |r| r.get(0),
                )?;
                if voided > 0 {
                    return Err(SalesStoreError::Conflict(format!(
                        "la factura {} ya tiene una anulación Verifactu: no se subsana el alta",
                        row.document_id
                    )));
                }
                alta.marcar_subsanacion_de_rechazado();
                alta.encadenamiento = chain;
                alta.sistema_informatico = ctx.system.clone();
                alta.fecha_hora_huso_gen_registro = ctx.now;
                alta.sellar().map_err(super::verifactu::from_vf)?;
                RegistroFacturacion::Alta(alta)
            }
            RegistroFacturacion::Anulacion(mut anulacion) => {
                let sin_previo = !registered_alta_exists(&tx, row.document_id)?;
                anulacion.marcar_subsanacion(sin_previo);
                anulacion.encadenamiento = chain;
                anulacion.sistema_informatico = ctx.system.clone();
                anulacion.fecha_hora_huso_gen_registro = ctx.now;
                anulacion.sellar().map_err(super::verifactu::from_vf)?;
                RegistroFacturacion::Anulacion(anulacion)
            }
        };
        let record_id = insert_record(&tx, row.document_id, &record, previous, ctx.environment)?;
        tx.execute(
            "INSERT INTO verifactu_subsanations (rejected_record_id, subsanation_record_id, \
             created_at) VALUES (?1, ?2, ?3)",
            params![rejected_record_id, record_id, now_ms()],
        )?;
        tx.commit()?;
        Ok(VerifactuSubsanation {
            rejected_record_id,
            record_id,
            created: true,
        })
    }
}

/// El registro estuvo en un lote anterior con respuesta, con intento anotado y sin línea para él
/// (una respuesta parcial lo dejó fuera): la AEAT pudo recibirlo, así que un 3000 o un 3001 en
/// el lote actual puede ser suyo. Un lote cerrado por `Fault` no cuenta: la AEAT rechazó el
/// mensaje entero y no guardó nada.
fn was_sent_before(conn: &Connection, batch_id: i64, record_id: i64) -> SalesResult<bool> {
    let n: i64 = conn.query_row(
        "SELECT count(*) FROM verifactu_batch_records br          JOIN verifactu_batch_results z ON z.batch_id = br.batch_id AND z.outcome = 'responded'          WHERE br.record_id = ?1 AND br.batch_id <> ?2          AND EXISTS (SELECT 1 FROM verifactu_batch_attempts a WHERE a.batch_id = br.batch_id AND NOT EXISTS (SELECT 1 FROM verifactu_unsent_attempts u WHERE u.attempt_id = a.id))          AND NOT EXISTS (SELECT 1 FROM verifactu_record_results x                          WHERE x.batch_id = br.batch_id AND x.record_id = br.record_id)",
        params![record_id, batch_id],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

pub(super) fn ensure_batch_open(conn: &Connection, batch_id: i64) -> SalesResult<()> {
    let exists: i64 = conn.query_row(
        "SELECT count(*) FROM verifactu_batches WHERE id = ?1",
        [batch_id],
        |r| r.get(0),
    )?;
    if exists == 0 {
        return Err(SalesStoreError::not_found("lote Verifactu", batch_id));
    }
    let closed: i64 = conn.query_row(
        "SELECT count(*) FROM verifactu_batch_results WHERE batch_id = ?1",
        [batch_id],
        |r| r.get(0),
    )?;
    if closed > 0 {
        return Err(SalesStoreError::Conflict(format!(
            "el lote {batch_id} ya está cerrado"
        )));
    }
    Ok(())
}
