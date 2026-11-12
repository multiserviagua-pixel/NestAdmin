//! Envío de la remisión Verifactu con certificado (T18b-D1, D278–D283, migración 17): configuración
//! por base, intentos sin envío, clasificación de los `Fault`, incidencias y paro por
//! autorización.
//!
//! Todo es de solo inserción, como la migración 16: el valor vigente de la configuración es la
//! última fila; un intento sin envío se ANULA con una fila aparte, nunca se borra; una incidencia
//! está abierta si su última fila es `open`; la remisión está parada por autorización si la
//! última fila de `verifactu_authorization_halts` es `halt`. Cada base es la de un obligado
//! (D273), así que nada de esto lleva identificador de obligado.
//!
//! Este módulo solo guarda y consulta: no llama a ninguna red ni toca el certificado. El
//! certificado entra solo como el JSON de `CertificateSource` (huella pública o referencia
//! opaca), que el núcleo trata como texto opaco.

use nestadmin_verifactu::{FechaHoraHuso, PersonaEs};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};

use super::verifactu::VerifactuRecordRow;
use super::verifactu_remission::{
    audit_remission, coded, ensure_batch_open, VerifactuEnvironment, MIN_RETRY_WAIT_SECONDS,
    OBLIGATION_AUTHORIZATION,
};
use super::{SalesResult, SalesStoreError};
use crate::Store;

/// Códigos de `Fault` de autorización (D281): el certificado no está autorizado para el obligado
/// o el `Representante` no es válido. La remisión de la base se para y no se reintenta.
pub const AUTHORIZATION_FAULT_CODES: [u32; 5] = [4112, 4105, 4117, 4123, 4124];
/// Códigos de `Fault` técnicos (D281): se reintentan con espera creciente y nunca paran.
pub const TECHNICAL_FAULT_CODES: [u32; 4] = [4108, 4110, 4134, 4141];
/// Espera máxima entre reintentos tras fallos técnicos seguidos: como mucho una hora (art. 16.4
/// de la Orden HAC/1177/2024).
pub const MAX_RETRY_WAIT_SECONDS: u32 = 3600;
/// Fallos seguidos que esperan la espera mínima (60 s) antes de que la espera empiece a crecer.
pub const FAST_RETRIES: u32 = 5;
/// Código de la tarea y del estado cuando la remisión está parada por autorización (D281).
pub const CODE_AUTHORIZATION_REJECTED: &str = "authorization_rejected";

/// Clase de un `Fault` SOAP (D281, enmienda de D259).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaultClass {
    /// 4112, 4105, 4117, 4123, 4124: se para la remisión de la base hasta que el usuario cambie
    /// el certificado o la configuración, o declare que lo arregló.
    Authorization,
    /// 4108, 4110, 4134, 4141: se reintenta con espera creciente hasta una hora; nunca para.
    Technical,
    /// El resto (esquema u otro código, o sin código): para con aviso tras
    /// [`MAX_AUTOMATIC_FAULTS`](super::MAX_AUTOMATIC_FAULTS) seguidos, como D259.
    Schema,
}

impl FaultClass {
    pub fn of(code: Option<u32>) -> Self {
        match code {
            Some(c) if AUTHORIZATION_FAULT_CODES.contains(&c) => FaultClass::Authorization,
            Some(c) if TECHNICAL_FAULT_CODES.contains(&c) => FaultClass::Technical,
            _ => FaultClass::Schema,
        }
    }
}

/// Quién pregunta por la espera: cambia qué fallos la alargan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitMode {
    /// El tick de la remisión automática: respeta la espera del último lote y la de los fallos
    /// seguidos (sin respuesta, sin envío y `Fault` técnicos).
    Automatic,
    /// «Remitir ahora»: como el automático, pero los intentos sin envío no la alargan: nada
    /// llegó a la AEAT (no hay a quién proteger) y el usuario acaba de arreglar su certificado.
    Manual,
    /// La primera pasada tras arrancar (D281): solo respeta la espera que fijó la AEAT en su
    /// último lote respondido; los fallos anteriores no cuentan, porque la red o el certificado
    /// pueden haber cambiado.
    Startup,
}

/// Espera, en segundos, tras `consecutive_failures` fallos técnicos seguidos: 60 s los
/// [`FAST_RETRIES`] primeros y después el doble cada vez, sin pasar de [`MAX_RETRY_WAIT_SECONDS`].
pub fn retry_wait_seconds(consecutive_failures: u32) -> u32 {
    if consecutive_failures <= FAST_RETRIES {
        return MIN_RETRY_WAIT_SECONDS;
    }
    let doublings = (consecutive_failures - FAST_RETRIES).min(16);
    MIN_RETRY_WAIT_SECONDS
        .saturating_mul(1u32 << doublings)
        .min(MAX_RETRY_WAIT_SECONDS)
}

/// Motivos de un intento sin envío: solo `a-z`, `0-9` y `_`, hasta 64 caracteres. Son códigos
/// estables, nunca texto del sistema ni datos del certificado.
fn valid_reason(code: &str) -> bool {
    !code.is_empty()
        && code.len() <= 64
        && code
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

fn check_reason(code: &str) -> SalesResult<()> {
    if valid_reason(code) {
        Ok(())
    } else {
        Err(SalesStoreError::invalid(
            "reason_code",
            "el motivo debe ser un código estable (a-z, 0-9 y _)",
        ))
    }
}

fn id_list(codes: &[u32]) -> String {
    codes
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

// ---------------------------------------------------------------------------------------------
// Configuración de la remisión (D278, D283)
// ---------------------------------------------------------------------------------------------

/// Calidad con la que remite quien lo hace (D278): consta en el historial y no se deduce.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SenderCapacity {
    /// El propio obligado, con su certificado.
    Taxpayer,
    /// Representante legal del obligado.
    Representative,
    /// Apoderado.
    Attorney,
    /// Colaborador social (la gestoría con su certificado).
    Collaborator,
}

impl SenderCapacity {
    pub fn as_str(self) -> &'static str {
        match self {
            SenderCapacity::Taxpayer => "taxpayer",
            SenderCapacity::Representative => "representative",
            SenderCapacity::Attorney => "attorney",
            SenderCapacity::Collaborator => "collaborator",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "taxpayer" => Some(SenderCapacity::Taxpayer),
            "representative" => Some(SenderCapacity::Representative),
            "attorney" => Some(SenderCapacity::Attorney),
            "collaborator" => Some(SenderCapacity::Collaborator),
            _ => None,
        }
    }
}

/// Configuración vigente de la remisión de esta base (la última fila, o los valores por defecto:
/// sin certificado, remite el propio obligado y sin representante).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemissionSettings {
    /// 0 si nunca se configuró.
    pub id: i64,
    /// JSON de `CertificateSource` (sin secretos); `None` si no hay certificado vinculado.
    pub certificate_source: Option<String>,
    pub sender_capacity: SenderCapacity,
    /// Nombre y NIF de quien remite con su propio certificado en nombre del obligado (D283).
    /// Solo si se configuró expresamente.
    pub representative: Option<PersonaEs>,
    pub changed_at: i64,
}

impl RemissionSettings {
    fn unset() -> Self {
        Self {
            id: 0,
            certificate_source: None,
            sender_capacity: SenderCapacity::Taxpayer,
            representative: None,
            changed_at: 0,
        }
    }
}

fn load_settings(conn: &Connection) -> SalesResult<RemissionSettings> {
    #[allow(clippy::type_complexity)]
    let row: Option<(
        i64,
        Option<String>,
        String,
        Option<String>,
        Option<String>,
        i64,
    )> = conn
        .query_row(
            "SELECT id, certificate_source, sender_capacity, representative_nif, \
             representative_name, changed_at FROM verifactu_remission_settings \
             ORDER BY id DESC LIMIT 1",
            [],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                ))
            },
        )
        .optional()?;
    let Some((id, source, capacity, nif, name, changed_at)) = row else {
        return Ok(RemissionSettings::unset());
    };
    let sender_capacity = SenderCapacity::parse(&capacity).ok_or_else(|| {
        SalesStoreError::Corrupt("calidad del remitente desconocida en la configuración".into())
    })?;
    let representative = match (nif, name) {
        (Some(nif), Some(name)) => Some(PersonaEs::new(&name, &nif)),
        _ => None,
    };
    Ok(RemissionSettings {
        id,
        certificate_source: source,
        sender_capacity,
        representative,
        changed_at,
    })
}

// ---------------------------------------------------------------------------------------------
// Incidencias (D282)
// ---------------------------------------------------------------------------------------------

/// Una incidencia técnica de remisión: de su apertura al siguiente envío con respuesta.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifactuIncident {
    /// Milisegundos Unix.
    pub opened_at: i64,
    /// `None` mientras siga abierta.
    pub closed_at: Option<i64>,
    /// Código estable del fallo que la abrió.
    pub reason_code: String,
}

fn load_incidents(conn: &Connection) -> SalesResult<Vec<VerifactuIncident>> {
    let mut stmt =
        conn.prepare("SELECT event, reason_code, at FROM verifactu_incidents ORDER BY id")?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, i64>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut incidents: Vec<VerifactuIncident> = Vec::new();
    for (event, reason, at) in rows {
        if event == "open" {
            incidents.push(VerifactuIncident {
                opened_at: at,
                closed_at: None,
                reason_code: reason.unwrap_or_default(),
            });
        } else if let Some(last) = incidents.last_mut() {
            if last.closed_at.is_none() {
                last.closed_at = Some(at);
            }
        }
    }
    Ok(incidents)
}

fn incident_is_open(conn: &Connection) -> SalesResult<bool> {
    let last: Option<String> = conn
        .query_row(
            "SELECT event FROM verifactu_incidents ORDER BY id DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .optional()?;
    Ok(last.as_deref() == Some("open"))
}

/// Abre una incidencia si no hay una abierta (idempotente). Devuelve si la abrió.
pub(super) fn open_incident(
    conn: &Connection,
    reason_code: &str,
    batch_id: Option<i64>,
    now_ms: i64,
) -> SalesResult<bool> {
    check_reason(reason_code)?;
    if incident_is_open(conn)? {
        return Ok(false);
    }
    conn.execute(
        "INSERT INTO verifactu_incidents (event, reason_code, batch_id, at) \
         VALUES ('open', ?1, ?2, ?3)",
        params![reason_code, batch_id, now_ms],
    )?;
    Ok(true)
}

/// Cierra la incidencia abierta, si la hay, con un envío con respuesta (idempotente).
pub(super) fn close_incident(conn: &Connection, batch_id: i64, now_ms: i64) -> SalesResult<bool> {
    if !incident_is_open(conn)? {
        return Ok(false);
    }
    conn.execute(
        "INSERT INTO verifactu_incidents (event, reason_code, batch_id, at) \
         VALUES ('close', NULL, ?1, ?2)",
        params![batch_id, now_ms],
    )?;
    Ok(true)
}

/// `true` si alguno de los registros se generó durante una incidencia (abierta o ya cerrada):
/// su fecha cae entre una apertura y su cierre (D282). Se deduce por el momento, sin marcar los
/// registros, que son inmutables.
pub(super) fn generated_during_incident(
    conn: &Connection,
    rows: &[VerifactuRecordRow],
) -> SalesResult<bool> {
    let incidents = load_incidents(conn)?;
    if incidents.is_empty() {
        return Ok(false);
    }
    for row in rows {
        let generated: FechaHoraHuso = row.generated_at.parse().map_err(|_| {
            SalesStoreError::Corrupt(format!(
                "fecha de generación ilegible en el registro Verifactu {}",
                row.id
            ))
        })?;
        let at = generated.unix().saturating_mul(1000);
        if incidents
            .iter()
            .any(|i| at >= i.opened_at && i.closed_at.is_none_or(|closed| at < closed))
        {
            return Ok(true);
        }
    }
    Ok(false)
}

// ---------------------------------------------------------------------------------------------
// Paro por autorización (D281)
// ---------------------------------------------------------------------------------------------

/// La remisión de esta base está parada por un `Fault` de autorización.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuthorizationHalt {
    /// Código del `Fault` (4112, 4105, 4117, 4123 o 4124).
    pub error_code: u32,
    pub batch_id: i64,
    /// Milisegundos Unix.
    pub at: i64,
}

fn load_halt(conn: &Connection) -> SalesResult<Option<AuthorizationHalt>> {
    let last: Option<(String, Option<i64>, Option<i64>, i64)> = conn
        .query_row(
            "SELECT event, batch_id, error_code, at FROM verifactu_authorization_halts \
             ORDER BY id DESC LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?;
    Ok(match last {
        Some((event, Some(batch_id), Some(code), at)) if event == "halt" => {
            Some(AuthorizationHalt {
                error_code: u32::try_from(code).unwrap_or(0),
                batch_id,
                at,
            })
        }
        _ => None,
    })
}

/// Para la remisión por un `Fault` de autorización, en la misma transacción que cierra el lote.
pub(super) fn halt_authorization(
    conn: &Connection,
    batch_id: i64,
    code: u32,
    now_ms: i64,
) -> SalesResult<()> {
    if load_halt(conn)?.is_some() {
        return Ok(());
    }
    conn.execute(
        "INSERT INTO verifactu_authorization_halts (event, batch_id, error_code, cause, at) \
         VALUES ('halt', ?1, ?2, NULL, ?3)",
        params![batch_id, i64::from(code), now_ms],
    )?;
    let environment: String = conn.query_row(
        "SELECT environment FROM verifactu_batches WHERE id = ?1",
        [batch_id],
        |r| r.get(0),
    )?;
    audit_remission(
        conn,
        "verifactu.authorization_halt",
        &format!("{OBLIGATION_AUTHORIZATION} {environment}"),
        serde_json::json!({ "batch_id": batch_id, "fault_code": code }),
    )
}

/// Reanuda la remisión parada, si lo estaba, con la causa dada (la configuración cambió o el
/// usuario declaró que lo arregló).
fn resume_authorization(
    conn: &Connection,
    cause: &str,
    authorization: &str,
    now_ms: i64,
) -> SalesResult<bool> {
    let Some(halt) = load_halt(conn)? else {
        return Ok(false);
    };
    conn.execute(
        "INSERT INTO verifactu_authorization_halts (event, batch_id, error_code, cause, at) \
         VALUES ('resume', NULL, NULL, ?1, ?2)",
        params![cause, now_ms],
    )?;
    audit_remission(
        conn,
        "verifactu.authorization_resume",
        authorization,
        serde_json::json!({ "cause": cause, "fault_code": halt.error_code }),
    )?;
    Ok(true)
}

// ---------------------------------------------------------------------------------------------
// Espera tras fallos seguidos (D251, D281)
// ---------------------------------------------------------------------------------------------

/// Fallos seguidos desde el último lote respondido y el momento del último. Cuenta, de los lotes
/// posteriores a ese: los intentos que cuentan de un lote abierto o cerrado por un `Fault`
/// técnico, los intentos anulados y los que ni se anotaron (si `include_unsent`).
fn failure_streak(
    conn: &Connection,
    environment: VerifactuEnvironment,
    include_unsent: bool,
) -> SalesResult<(u32, Option<i64>)> {
    let env = environment.as_str();
    let last_ok: i64 = conn.query_row(
        "SELECT COALESCE(MAX(r.batch_id), 0) FROM verifactu_batch_results r JOIN \
         verifactu_batches b ON b.id = r.batch_id WHERE r.outcome = 'responded' \
         AND b.environment = ?1",
        [env],
        |r| r.get(0),
    )?;
    let technical = id_list(&TECHNICAL_FAULT_CODES);
    let (attempts, attempts_at): (i64, Option<i64>) = conn.query_row(
        &format!(
            "SELECT count(*), max(a.attempted_at) FROM verifactu_batch_attempts a JOIN \
             verifactu_batches b ON b.id = a.batch_id WHERE b.environment = ?1 AND b.id > ?2 \
             AND NOT EXISTS (SELECT 1 FROM verifactu_unsent_attempts u WHERE u.attempt_id = a.id) \
             AND (NOT EXISTS (SELECT 1 FROM verifactu_batch_results r WHERE r.batch_id = b.id) \
                  OR EXISTS (SELECT 1 FROM verifactu_batch_results r WHERE r.batch_id = b.id \
                             AND r.outcome = 'fault' AND r.error_code IN ({technical})))"
        ),
        params![env, last_ok],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let fault_at: Option<i64> = conn.query_row(
        &format!(
            "SELECT max(r.finished_at) FROM verifactu_batch_results r JOIN verifactu_batches b \
             ON b.id = r.batch_id WHERE b.environment = ?1 AND b.id > ?2 AND r.outcome = 'fault' \
             AND r.error_code IN ({technical})"
        ),
        params![env, last_ok],
        |r| r.get(0),
    )?;
    let (mut streak, mut last_at) = (attempts, attempts_at.max(fault_at));
    if include_unsent {
        let (unsent, unsent_at): (i64, Option<i64>) = conn.query_row(
            "SELECT count(*), max(u.attempted_at) FROM verifactu_unsent_attempts u JOIN \
             verifactu_batches b ON b.id = u.batch_id WHERE b.environment = ?1 AND b.id > ?2",
            params![env, last_ok],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        streak += unsent;
        last_at = last_at.max(unsent_at);
    }
    Ok((u32::try_from(streak).unwrap_or(u32::MAX), last_at))
}

/// Momento a partir del cual se puede reintentar tras los fallos seguidos, o `None` si no
/// cuentan o no hay ninguno (ver [`WaitMode`]).
pub(super) fn retry_failure_wait(
    conn: &Connection,
    environment: VerifactuEnvironment,
    mode: WaitMode,
) -> SalesResult<Option<i64>> {
    if mode == WaitMode::Startup {
        return Ok(None);
    }
    let (streak, last_at) = failure_streak(conn, environment, mode == WaitMode::Automatic)?;
    Ok(match (streak, last_at) {
        (0, _) | (_, None) => None,
        (n, Some(at)) => Some(at + i64::from(retry_wait_seconds(n)) * 1000),
    })
}

/// Último intento sin envío anotado, para mostrarlo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifactuUnsentAttempt {
    pub batch_id: i64,
    /// Código estable del motivo (`certificate_needs_user`, `connect_failed`, ...).
    pub reason_code: String,
    /// Milisegundos Unix.
    pub attempted_at: i64,
    /// `true` si anuló un intento que ya estaba anotado; `false` si falló antes de anotarlo.
    pub voided_attempt: bool,
}

impl Store {
    // --- Configuración ---

    /// Configuración vigente de la remisión de esta base (D278, D283).
    pub fn verifactu_remission_settings(&self) -> SalesResult<RemissionSettings> {
        load_settings(&self.conn)
    }

    /// Guarda una configuración nueva y devuelve si cambió algo (una fila nueva en el historial;
    /// la misma configuración no escribe nada). Cambiarla reanuda la remisión parada por
    /// autorización (D281). `certificate_source` es el JSON de `CertificateSource`: sin
    /// secretos. El representante se configura expresamente y nunca se deduce (D283): `None`
    /// quita el anterior. En el historial constan la calidad y si hay certificado y
    /// representante, nunca sus valores.
    pub fn set_verifactu_remission_settings(
        &mut self,
        certificate_source: Option<&str>,
        sender_capacity: SenderCapacity,
        representative: Option<&PersonaEs>,
        authorization: &str,
        now_ms: i64,
    ) -> SalesResult<bool> {
        let authorization = authorization.trim();
        if authorization.is_empty() {
            return Err(SalesStoreError::invalid(
                "authorization",
                "falta la autorización del cambio de configuración",
            ));
        }
        if let Some(source) = certificate_source {
            let parsed: serde_json::Value = serde_json::from_str(source).map_err(|_| {
                SalesStoreError::invalid("certificate_source", "no es un JSON válido")
            })?;
            if !parsed.is_object() || source.len() > 1024 {
                return Err(SalesStoreError::invalid(
                    "certificate_source",
                    "debe ser un objeto JSON corto",
                ));
            }
        }
        if let Some(person) = representative {
            person
                .validar("Representante")
                .map_err(|e| SalesStoreError::invalid("representative", e.to_string()))?;
        }
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = load_settings(&tx)?;
        let unchanged = current.id != 0
            && current.certificate_source.as_deref() == certificate_source
            && current.sender_capacity == sender_capacity
            && current.representative.as_ref() == representative;
        if unchanged {
            return Ok(false);
        }
        tx.execute(
            "INSERT INTO verifactu_remission_settings (certificate_source, sender_capacity, \
             representative_nif, representative_name, authorization, changed_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                certificate_source,
                sender_capacity.as_str(),
                representative.map(|p| p.nif.as_str()),
                representative.map(|p| p.nombre_razon.as_str()),
                authorization,
                now_ms,
            ],
        )?;
        audit_remission(
            &tx,
            "verifactu.remission_settings",
            authorization,
            serde_json::json!({
                "sender_capacity": sender_capacity.as_str(),
                "certificate": certificate_source.is_some(),
                "representative": representative.is_some(),
            }),
        )?;
        resume_authorization(&tx, "settings_change", authorization, now_ms)?;
        tx.commit()?;
        Ok(true)
    }

    /// ¿Este certificado (por su huella SHA-256 en hexadecimal) solo negocia TLS 1.2? (D279)
    pub fn verifactu_tls12_preferred(&self, fingerprint: &str) -> SalesResult<bool> {
        let n: i64 = self.conn.query_row(
            "SELECT count(*) FROM verifactu_tls12_preferences WHERE fingerprint = ?1",
            [fingerprint.to_ascii_lowercase()],
            |r| r.get(0),
        )?;
        Ok(n > 0)
    }

    /// Recuerda que ese certificado solo negoció con TLS 1.2. Devuelve si es nuevo.
    pub fn remember_verifactu_tls12(
        &mut self,
        fingerprint: &str,
        now_ms: i64,
    ) -> SalesResult<bool> {
        let fingerprint = fingerprint.to_ascii_lowercase();
        if fingerprint.len() != 64 || !fingerprint.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(SalesStoreError::invalid(
                "fingerprint",
                "la huella debe ser un SHA-256 en hexadecimal",
            ));
        }
        let rows = self.conn.execute(
            "INSERT OR IGNORE INTO verifactu_tls12_preferences (fingerprint, learned_at) \
             VALUES (?1, ?2)",
            params![fingerprint, now_ms],
        )?;
        Ok(rows > 0)
    }

    // --- Intentos sin envío (D280) ---

    /// Anota que el intento falló ANTES de anotarlo (el certificado o la clave no estaban
    /// disponibles): no hay intento, no cuenta para considerar propio un duplicado 3000 (D50,
    /// D252) y abre una incidencia (D282). Falla si el lote no existe o ya está cerrado.
    pub fn note_verifactu_unsent_attempt(
        &mut self,
        batch_id: i64,
        reason_code: &str,
        now_ms: i64,
    ) -> SalesResult<()> {
        check_reason(reason_code)?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        ensure_batch_open(&tx, batch_id)?;
        tx.execute(
            "INSERT INTO verifactu_unsent_attempts (batch_id, attempt_id, reason_code, \
             attempted_at) VALUES (?1, NULL, ?2, ?3)",
            params![batch_id, reason_code, now_ms],
        )?;
        open_incident(&tx, reason_code, Some(batch_id), now_ms)?;
        tx.commit()?;
        Ok(())
    }

    /// Anula un intento ya anotado porque el transporte dijo que el cuerpo no llegó a
    /// escribirse (D280): el intento sigue en `verifactu_batch_attempts` (solo inserción) pero
    /// deja de contar. Cada intento se anula una sola vez (`conflict`). Abre una incidencia.
    pub fn void_verifactu_attempt(
        &mut self,
        attempt_id: i64,
        reason_code: &str,
        now_ms: i64,
    ) -> SalesResult<()> {
        check_reason(reason_code)?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let batch_id: i64 = tx
            .query_row(
                "SELECT batch_id FROM verifactu_batch_attempts WHERE id = ?1",
                [attempt_id],
                |r| r.get(0),
            )
            .optional()?
            .ok_or_else(|| SalesStoreError::not_found("intento de envío", attempt_id))?;
        ensure_batch_open(&tx, batch_id)?;
        let voided: i64 = tx.query_row(
            "SELECT count(*) FROM verifactu_unsent_attempts WHERE attempt_id = ?1",
            [attempt_id],
            |r| r.get(0),
        )?;
        if voided > 0 {
            return Err(SalesStoreError::Conflict(format!(
                "el intento {attempt_id} ya está anulado"
            )));
        }
        tx.execute(
            "INSERT INTO verifactu_unsent_attempts (batch_id, attempt_id, reason_code, \
             attempted_at) VALUES (?1, ?2, ?3, ?4)",
            params![batch_id, attempt_id, reason_code, now_ms],
        )?;
        open_incident(&tx, reason_code, Some(batch_id), now_ms)?;
        tx.commit()?;
        Ok(())
    }

    /// El último intento sin envío anotado, si lo hay.
    pub fn verifactu_last_unsent_attempt(&self) -> SalesResult<Option<VerifactuUnsentAttempt>> {
        Ok(self
            .conn
            .query_row(
                "SELECT batch_id, reason_code, attempted_at, attempt_id IS NOT NULL \
                 FROM verifactu_unsent_attempts ORDER BY id DESC LIMIT 1",
                [],
                |r| {
                    Ok(VerifactuUnsentAttempt {
                        batch_id: r.get(0)?,
                        reason_code: r.get(1)?,
                        attempted_at: r.get(2)?,
                        voided_attempt: r.get(3)?,
                    })
                },
            )
            .optional()?)
    }

    // --- Incidencias (D282) ---

    /// Abre una incidencia por un fallo técnico de remisión (sin respuesta, no enviado): si ya
    /// hay una abierta no hace nada. Devuelve si la abrió.
    pub fn open_verifactu_incident(
        &mut self,
        batch_id: Option<i64>,
        reason_code: &str,
        now_ms: i64,
    ) -> SalesResult<bool> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let opened = open_incident(&tx, reason_code, batch_id, now_ms)?;
        tx.commit()?;
        Ok(opened)
    }

    /// ¿Hay una incidencia abierta?
    pub fn verifactu_incident_open(&self) -> SalesResult<bool> {
        incident_is_open(&self.conn)
    }

    /// Las incidencias, de la más antigua a la más reciente.
    pub fn verifactu_incidents(&self) -> SalesResult<Vec<VerifactuIncident>> {
        load_incidents(&self.conn)
    }

    // --- Paro por autorización (D281) ---

    /// El paro por `Fault` de autorización de esta base, si lo hay.
    pub fn verifactu_authorization_halt(&self) -> SalesResult<Option<AuthorizationHalt>> {
        load_halt(&self.conn)
    }

    /// El usuario declara que ya arregló la autorización (nuevo certificado, apoderamiento o
    /// colaboración dados de alta) y reanuda la remisión. Devuelve si estaba parada. Consta en
    /// el historial con `authorization`.
    pub fn resume_verifactu_authorization(
        &mut self,
        authorization: &str,
        now_ms: i64,
    ) -> SalesResult<bool> {
        let authorization = authorization.trim();
        if authorization.is_empty() {
            return Err(coded(
                "invalid_input",
                "falta la declaración de que la autorización está arreglada",
            ));
        }
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let resumed = resume_authorization(&tx, "declaration", authorization, now_ms)?;
        tx.commit()?;
        Ok(resumed)
    }

    /// Fallos seguidos que alargan la espera (D281): ver [`retry_wait_seconds`].
    pub fn verifactu_failure_streak(
        &self,
        environment: VerifactuEnvironment,
        mode: WaitMode,
    ) -> SalesResult<u32> {
        if mode == WaitMode::Startup {
            return Ok(0);
        }
        Ok(failure_streak(&self.conn, environment, mode == WaitMode::Automatic)?.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn faults_are_classified_by_code() {
        for code in AUTHORIZATION_FAULT_CODES {
            assert_eq!(FaultClass::of(Some(code)), FaultClass::Authorization);
        }
        for code in TECHNICAL_FAULT_CODES {
            assert_eq!(FaultClass::of(Some(code)), FaultClass::Technical);
        }
        for code in [Some(4102), Some(4104), Some(4116), Some(9999), None] {
            assert_eq!(FaultClass::of(code), FaultClass::Schema);
        }
    }

    #[test]
    fn the_retry_wait_grows_and_never_passes_one_hour() {
        let waits: Vec<u32> = (0..=14).map(retry_wait_seconds).collect();
        assert_eq!(&waits[..6], &[60; 6]);
        assert_eq!(&waits[6..11], &[120, 240, 480, 960, 1920]);
        assert!(waits[11..].iter().all(|w| *w == MAX_RETRY_WAIT_SECONDS));
        assert_eq!(retry_wait_seconds(u32::MAX), MAX_RETRY_WAIT_SECONDS);
        assert!(waits.windows(2).all(|w| w[0] <= w[1]));
    }

    #[test]
    fn reasons_are_stable_codes() {
        assert!(valid_reason("certificate_needs_user"));
        assert!(!valid_reason(""));
        assert!(!valid_reason("Con Mayúsculas"));
        assert!(!valid_reason(&"a".repeat(65)));
    }
}
