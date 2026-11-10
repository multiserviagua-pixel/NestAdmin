//! Datos del panel de reglas (D124, D126, D128, D129): nombre y archivado, validación
//! avanzada sin remitente, evaluaciones, señales y contadores. Ninguna tabla guarda cuerpos ni
//! textos de correo: solo ids, códigos y claves ya normalizadas.

use std::collections::{HashMap, HashSet};

use rusqlite::{params, OptionalExtension};
use serde::Serialize;
use serde_json::Value;

use crate::error::{CoreError, Result};
use crate::rules::{
    audit_any_sender, current_rule, decide_active, insert_rule_version, insert_rule_version_noting,
    is_archived, live_rule, DEACTIVATED_BY_ANY_SENDER, DEACTIVATED_BY_EDIT,
};
use crate::store::{now_ms, Store};
use crate::Rule;

/// Datos de una regla que no son sus condiciones.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RuleMeta {
    pub rule_id: String,
    pub name: Option<String>,
    pub archived_at: Option<i64>,
    /// Marca de la validación avanzada: la regla de envío puede contestar a cualquier
    /// remitente (D126). Se borra al cambiar las condiciones.
    pub allow_any_sender_ack_at: Option<i64>,
    /// De dónde sale la regla (D131): al unificar, las reglas de origen y la opción elegida.
    /// Solo ids, versiones y claves; nunca contenido de correos.
    pub origin: Option<Value>,
}

/// Una evaluación que se guarda (D129). `checks` no lleva contenido del correo.
#[derive(Debug, Clone)]
pub struct EvaluationRecord<'a> {
    pub task_id: i64,
    pub content_version: i64,
    pub action_kind: &'a str,
    /// `automated` o `review`.
    pub outcome: &'a str,
    pub reason_code: &'a str,
    pub rules_fp: &'a str,
    pub checks: &'a Value,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EvaluationRow {
    pub task_id: i64,
    pub content_version: i64,
    pub action_kind: String,
    pub outcome: String,
    pub reason_code: String,
    pub rules_fp: String,
    pub checks: Value,
    pub at: i64,
}

/// Una señal de aprendizaje (D129): `approved`, `edited` o `rejected`.
#[derive(Debug, Clone)]
pub struct SignalRecord<'a> {
    pub kind: &'a str,
    pub action_kind: &'a str,
    pub subject_key: &'a str,
    /// `unedited`, `edited` o `unknown`.
    pub edit_state: &'a str,
    pub task_id: i64,
    pub content_version: i64,
    pub source_key: Option<&'a str>,
    /// `valid`, o `pending` para una aprobación que aún no consta ejecutada (D129).
    pub state: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SignalRow {
    /// Orden de llegada: creciente y sin reutilizar (las filas no se borran).
    pub id: i64,
    pub at: i64,
    pub kind: String,
    pub action_kind: String,
    pub subject_key: String,
    pub edit_state: String,
    /// `None` solo en las señales `learned`: nacen del aprendizaje y no de una tarea (D172).
    pub task_id: Option<i64>,
    pub content_version: i64,
    pub source_key: Option<String>,
    /// `valid`, `pending` o `void`. Las sugerencias solo usan las `valid`.
    pub state: String,
}

/// Una señal `learned` por guardar (D172): un correo recibido que la persona ya contestó antes de
/// instalar NestAdmin. Solo la clave de señal y la del mensaje; nunca texto.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LearnedSignal {
    /// `categoría|remitente`, la misma clave que las aprobaciones de correo.
    pub subject_key: String,
    /// Fuente y id del mensaje recibido: única por correo.
    pub source_key: String,
}

/// Lo que se guarda al terminar de aprender, en la misma transacción que el resultado (D171,
/// D172): señales `learned` y, por cada par que pedía respuesta, el enum `(categoría, reply_kind)`
/// que alimenta `reply_style`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LearnedEvidence {
    pub signals: Vec<LearnedSignal>,
    pub reply_style: Vec<(String, String)>,
}

/// Una decisión sobre una sugerencia (D132): descartada o aceptada, con la evidencia que
/// había entonces. Solo cifras, ids y claves; nunca contenido de correos.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SuggestionRow {
    pub fingerprint: String,
    /// `discarded` o `accepted`.
    pub status: String,
    pub evidence: Value,
    pub decided_at: Option<i64>,
}

/// Huella reservada de la fila que guarda el último afinado con IA (D134). No es una huella
/// de condiciones (esas son hexadecimales), de modo que no puede coincidir con una sugerencia.
pub const REFINEMENT_KEY: &str = "(afinado)";

/// Prefijo de las claves de `rule_settings` que guardan las marcas de primera lectura.
pub const READ_MARK_PREFIX: &str = "initial_read:";

/// Prefijo de las marcas de la fase «Aprender a responder» (D167, D175): estado, huella,
/// ids y recuentos. Nunca cuerpos de correo.
pub const LEARN_MARK_PREFIX: &str = "first_learning:";

/// Prefijo de lo aprendido (estilo y enums por mensaje), clave hermana de la marca. Sin textos
/// de correo (D175).
pub const LEARN_RESULT_PREFIX: &str = "first_learning_result:";

/// Contadores de una regla.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RuleStats {
    pub rule_id: String,
    /// Acciones confirmadas o reconciliadas que autorizó esta regla.
    pub applied: u64,
    pub last_applied_at: Option<i64>,
    /// Tareas que esta regla habría automatizado pero pasaron a revisión (por ejemplo,
    /// porque otra regla también coincidía o por una incidencia).
    pub reviewed: u64,
    /// Acciones antiguas que citan la regla en un formato que no se puede leer con certeza.
    pub unclassified: u64,
}

impl Store {
    pub fn rule_meta(&self, id: &str) -> Result<Option<RuleMeta>> {
        Ok(self
            .conn
            .query_row(
                "SELECT rule_id, name, archived_at, allow_any_sender_ack_at, origin_json FROM rule_meta WHERE rule_id = ?1",
                [id],
                meta_from_row,
            )
            .optional()?)
    }

    pub fn rule_metas(&self) -> Result<HashMap<String, RuleMeta>> {
        let mut stmt = self
            .conn
            .prepare("SELECT rule_id, name, archived_at, allow_any_sender_ack_at, origin_json FROM rule_meta")?;
        let rows = stmt.query_map([], meta_from_row)?;
        let mut map = HashMap::new();
        for meta in rows {
            let meta = meta?;
            map.insert(meta.rule_id.clone(), meta);
        }
        Ok(map)
    }

    /// Existe alguna versión de la regla (también archivada): un id no se reutiliza.
    pub fn rule_id_taken(&self, id: &str) -> Result<bool> {
        Ok(current_rule(&self.conn, id)?.is_some())
    }

    /// Cambia el nombre visible; `None` lo borra y se vuelve a mostrar el identificador.
    pub fn set_rule_name(&mut self, id: &str, name: Option<&str>) -> Result<()> {
        let tx = self.conn.transaction()?;
        current_rule(&tx, id)?.ok_or_else(|| CoreError::RuleNotFound(id.to_owned()))?;
        write_name(&tx, id, name)?;
        tx.commit()?;
        Ok(())
    }

    /// Guarda en una sola transacción las condiciones nuevas, el nombre (si `name` es
    /// `Some`; `Some(None)` lo borra) y la validación avanzada sin remitente (D126). Cambiar
    /// las condiciones borra la validación anterior; con `any_sender_ack` se da de nuevo a las
    /// condiciones nuevas. Todo consta en el historial y, si algo falla, no se guarda nada.
    /// Una regla activa solo sigue activa si `keep_active(anterior, tenía_validación)` lo
    /// permite (D186c); si no, la versión nueva nace inactiva en la misma transacción. Devuelve
    /// la regla y si quedó desactivada por la edición. Quien llama comprueba las condiciones con
    /// `check_rule`.
    pub fn update_rule_full(
        &mut self,
        id: &str,
        conditions: &Value,
        name: Option<Option<&str>>,
        any_sender_ack: bool,
        keep_active: &dyn Fn(&Rule, bool) -> bool,
    ) -> Result<(Rule, bool)> {
        let tx = self.conn.transaction()?;
        let current = live_rule(&tx, id)?;
        let (active, deactivated) = decide_active(&tx, &current, keep_active)?;
        let rule = insert_rule_version_noting(
            &tx,
            id,
            current.version + 1,
            &current.action_kind,
            conditions,
            active,
            deactivated.then_some(DEACTIVATED_BY_EDIT),
        )?;
        let had_ack = tx.execute(
            "UPDATE rule_meta SET allow_any_sender_ack_at = NULL
             WHERE rule_id = ?1 AND allow_any_sender_ack_at IS NOT NULL",
            [id],
        )?;
        if had_ack > 0 {
            audit_any_sender(&tx, &rule, false, "retirada: las condiciones cambiaron")?;
        }
        if let Some(name) = name {
            write_name(&tx, id, name)?;
        }
        if any_sender_ack {
            tx.execute(
                "INSERT INTO rule_meta (rule_id, allow_any_sender_ack_at) VALUES (?1, ?2)
                 ON CONFLICT(rule_id) DO UPDATE SET allow_any_sender_ack_at = excluded.allow_any_sender_ack_at",
                params![id, now_ms()],
            )?;
            audit_any_sender(&tx, &rule, true, "validada al guardar la regla")?;
        }
        tx.commit()?;
        Ok((rule, deactivated))
    }

    /// Archiva una regla: la deja inactiva (versión nueva si estaba activa) y la marca. No
    /// hay borrado físico: el historial conserva todas las versiones (D128).
    pub fn archive_rule(&mut self, id: &str) -> Result<Rule> {
        let tx = self.conn.transaction()?;
        let rule = archive_in(&tx, id)?;
        tx.commit()?;
        Ok(rule)
    }

    /// Unifica reglas en una sola transacción (D131): archiva las originales, crea la regla
    /// nueva **inactiva**, sin la validación avanzada de ninguna, con su origen, y lo audita
    /// con ids y versiones. `sources` lleva cada original con la versión que se revisó: si
    /// alguna cambió o está archivada, no se toca nada. Si algo falla a mitad, tampoco.
    /// Nunca reactiva nada. Quien llama comprueba las condiciones con `check_rule`.
    pub fn merge_rules(
        &mut self,
        sources: &[(String, i64)],
        new_id: &str,
        action_kind: &str,
        conditions: &Value,
        name: Option<&str>,
        origin: &Value,
    ) -> Result<Rule> {
        let tx = self.conn.transaction()?;
        for (id, version) in sources {
            let current = live_rule(&tx, id)?;
            if current.version != *version {
                return Err(CoreError::RuleChanged(id.clone()));
            }
        }
        if current_rule(&tx, new_id)?.is_some() {
            return Err(CoreError::IdempotencyConflict(format!("regla {new_id}")));
        }
        for (id, _) in sources {
            archive_in(&tx, id)?;
        }
        let rule = insert_rule_version(&tx, new_id, 1, action_kind, conditions, false)?;
        tx.execute(
            "INSERT INTO rule_meta (rule_id, name, origin_json) VALUES (?1, ?2, ?3)",
            params![new_id, name, serde_json::to_string(origin)?],
        )?;
        let from = sources
            .iter()
            .map(|(id, version)| format!("{id} v{version}"))
            .collect::<Vec<_>>()
            .join(", ");
        tx.execute(
            "INSERT INTO audit_log (at, task_id, action, status, authorization, detail) VALUES (?1, NULL, 'rule_merge', 'merged', NULL, ?2)",
            params![
                now_ms(),
                format!("{new_id} v1 (inactiva) sustituye a {from}; originales archivadas")
            ],
        )?;
        tx.commit()?;
        Ok(rule)
    }

    /// Devuelve una regla archivada a la lista, inactiva.
    pub fn unarchive_rule(&mut self, id: &str) -> Result<Rule> {
        let tx = self.conn.transaction()?;
        let current =
            current_rule(&tx, id)?.ok_or_else(|| CoreError::RuleNotFound(id.to_owned()))?;
        if !is_archived(&tx, id)? {
            return Ok(current);
        }
        tx.execute(
            "UPDATE rule_meta SET archived_at = NULL WHERE rule_id = ?1",
            [id],
        )?;
        tx.execute(
            "INSERT INTO audit_log (at, task_id, action, status, authorization, detail) VALUES (?1, NULL, 'rule_archive', 'unarchived', NULL, ?2)",
            params![now_ms(), format!("{id} v{} (inactiva)", current.version)],
        )?;
        tx.commit()?;
        Ok(current)
    }

    /// Guarda o retira la validación avanzada sin remitente (D126), con su motivo en el
    /// historial. Quien llama comprueba que la regla es de envío y no indica remitente.
    pub fn set_any_sender_ack(
        &mut self,
        id: &str,
        acknowledge: bool,
        reason: &str,
    ) -> Result<Rule> {
        let tx = self.conn.transaction()?;
        let rule = live_rule(&tx, id)?;
        tx.execute(
            "INSERT INTO rule_meta (rule_id, allow_any_sender_ack_at) VALUES (?1, ?2)
             ON CONFLICT(rule_id) DO UPDATE SET allow_any_sender_ack_at = excluded.allow_any_sender_ack_at",
            params![id, acknowledge.then(now_ms)],
        )?;
        audit_any_sender(&tx, &rule, acknowledge, reason)?;
        tx.commit()?;
        Ok(rule)
    }

    /// Como `set_any_sender_ack`, pero dar la validación avanzada a una regla ACTIVA que no la
    /// tenía la deja inactiva en la misma transacción (D186c): `keep_active(anterior,
    /// tenía_validación)` decide, con fallo cerrado. Retirarla no cambia el estado. La versión nueva
    /// y la validación constan en el historial. Devuelve la regla y si quedó desactivada.
    pub fn set_any_sender_ack_guarded(
        &mut self,
        id: &str,
        acknowledge: bool,
        reason: &str,
        keep_active: &dyn Fn(&Rule, bool) -> bool,
    ) -> Result<(Rule, bool)> {
        let tx = self.conn.transaction()?;
        let current = live_rule(&tx, id)?;
        let (_, deactivated) = if acknowledge {
            decide_active(&tx, &current, keep_active)?
        } else {
            (current.active, false)
        };
        tx.execute(
            "INSERT INTO rule_meta (rule_id, allow_any_sender_ack_at) VALUES (?1, ?2)
             ON CONFLICT(rule_id) DO UPDATE SET allow_any_sender_ack_at = excluded.allow_any_sender_ack_at",
            params![id, acknowledge.then(now_ms)],
        )?;
        let rule = if deactivated {
            insert_rule_version_noting(
                &tx,
                id,
                current.version + 1,
                &current.action_kind,
                &current.conditions,
                false,
                Some(DEACTIVATED_BY_ANY_SENDER),
            )?
        } else {
            current
        };
        audit_any_sender(&tx, &rule, acknowledge, reason)?;
        tx.commit()?;
        Ok((rule, deactivated))
    }

    /// Guarda una evaluación; `UNIQUE` la hace idempotente al reprocesar. Con la misma clave,
    /// si la tarea todavía no tiene ninguna acción y el resultado cambió (un corte entre la
    /// evidencia y el intento hizo que al reprocesar la IA leyera otra cosa), la fila se
    /// actualiza: la primera evaluación quedó obsoleta. Con una acción ya registrada, la
    /// evaluación guardada es la que la autorizó y no se toca. `true` si se guardó o cambió.
    pub fn record_evaluation(&mut self, e: &EvaluationRecord<'_>) -> Result<bool> {
        let changed = self.conn.execute(
            "INSERT INTO rule_evaluations
             (task_id, content_version, action_kind, outcome, reason_code, rules_fp, checks_json, at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT (task_id, content_version, rules_fp) DO UPDATE SET
                 action_kind = excluded.action_kind,
                 outcome = excluded.outcome,
                 reason_code = excluded.reason_code,
                 checks_json = excluded.checks_json,
                 at = excluded.at
             WHERE NOT EXISTS (SELECT 1 FROM actions WHERE task_id = excluded.task_id)
               AND (rule_evaluations.outcome IS NOT excluded.outcome
                    OR rule_evaluations.reason_code IS NOT excluded.reason_code
                    OR rule_evaluations.checks_json IS NOT excluded.checks_json)",
            params![
                e.task_id,
                e.content_version,
                e.action_kind,
                e.outcome,
                e.reason_code,
                e.rules_fp,
                serde_json::to_string(e.checks)?,
                now_ms()
            ],
        )?;
        Ok(changed == 1)
    }

    /// Evaluaciones guardadas, de la más antigua a la más reciente.
    pub fn rule_evaluations(&self) -> Result<Vec<EvaluationRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT task_id, content_version, action_kind, outcome, reason_code, rules_fp, checks_json, at
             FROM rule_evaluations ORDER BY id",
        )?;
        let rows = stmt.query_map([], |r| {
            let checks: String = r.get(6)?;
            Ok(EvaluationRow {
                task_id: r.get(0)?,
                content_version: r.get(1)?,
                action_kind: r.get(2)?,
                outcome: r.get(3)?,
                reason_code: r.get(4)?,
                rules_fp: r.get(5)?,
                checks: serde_json::from_str(&checks).unwrap_or(Value::Null),
                at: r.get(7)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Guarda una señal; `UNIQUE` la hace idempotente. `true` si es nueva.
    pub fn record_signal(&mut self, s: &SignalRecord<'_>) -> Result<bool> {
        let changed = self.conn.execute(
            "INSERT OR IGNORE INTO rule_signals
             (kind, action_kind, subject_key, edit_state, task_id, content_version, source_key, at, state)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                s.kind,
                s.action_kind,
                s.subject_key,
                s.edit_state,
                s.task_id,
                s.content_version,
                s.source_key,
                now_ms(),
                s.state
            ],
        )?;
        Ok(changed == 1)
    }

    /// Resuelve la señal `approved` de una versión: `valid` si la acción quedó confirmada o
    /// reconciliada, `void` si no llegó a ejecutarse. Sin esa fila no hace nada.
    pub fn settle_signal(&mut self, task_id: i64, content_version: i64, valid: bool) -> Result<()> {
        self.settle_approval(task_id, content_version, valid, None)
            .map(|_| ())
    }

    /// Como `settle_signal`; con `rodaje_rule` (la regla aprendida que mandó la tarea a revisión por
    /// el Rodaje, D174), en la **misma transacción** baja su contador si esta aprobación pasa a
    /// `valid` ahora (no antes) y se hizo **sin editar**. Así un reintento no cuenta dos veces y una
    /// aprobación que no llegó a enviarse no cuenta. Devuelve los envíos de rodaje que quedan.
    pub fn settle_approval(
        &mut self,
        task_id: i64,
        content_version: i64,
        valid: bool,
        rodaje_rule: Option<&str>,
    ) -> Result<Option<i64>> {
        let tx = self.conn.transaction()?;
        let before: Option<(String, String, String)> = tx
            .query_row(
                "SELECT state, edit_state, subject_key FROM rule_signals
                 WHERE kind = 'approved' AND task_id = ?1 AND content_version = ?2",
                params![task_id, content_version],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        tx.execute(
            "UPDATE rule_signals SET state = ?3
             WHERE kind = 'approved' AND task_id = ?1 AND content_version = ?2",
            params![
                task_id,
                content_version,
                if valid { "valid" } else { "void" }
            ],
        )?;
        let mut left = None;
        if let (true, Some((state, edit, subject_key)), Some(rule)) = (valid, before, rodaje_rule) {
            if state != "valid" && edit == "unedited" {
                // Un origen ilegible no se reescribe: el rodaje sigue completo (falla cerrado).
                if let Some(now) = rodaje_of(&tx, rule)?.filter(|n| n.readable && n.active()) {
                    let mut hashes = now.address_hashes.clone();
                    // En una regla por dominio se anota la dirección (solo su huella) de cada
                    // aprobación que cuenta: el rodaje no acaba hasta cubrir `min_addresses`.
                    if now.min_addresses > 1 {
                        if let Some((_, address)) = subject_key.split_once('|') {
                            let hash = address_hash(address);
                            if !address.trim().is_empty() && !hashes.contains(&hash) {
                                hashes.push(hash);
                            }
                        }
                    }
                    let mut next = (now.left - 1).max(0);
                    if next == 0 && hashes.len() < now.min_addresses {
                        next = 1;
                    }
                    set_rodaje(
                        &tx,
                        rule,
                        next,
                        Some(&hashes),
                        "envío de rodaje aprobado sin editar",
                    )?;
                    left = Some(next);
                }
            }
        }
        tx.commit()?;
        Ok(left)
    }

    pub fn rule_signals(&self) -> Result<Vec<SignalRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, at, kind, action_kind, subject_key, edit_state, task_id, content_version, source_key, state
             FROM rule_signals ORDER BY id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(SignalRow {
                id: r.get(0)?,
                at: r.get(1)?,
                kind: r.get(2)?,
                action_kind: r.get(3)?,
                subject_key: r.get(4)?,
                edit_state: r.get(5)?,
                task_id: r.get(6)?,
                content_version: r.get(7)?,
                source_key: r.get(8)?,
                state: r.get(9)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Decisiones guardadas sobre sugerencias, por huella. Solo lectura.
    pub fn rule_suggestion_rows(&self) -> Result<Vec<SuggestionRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT fingerprint, status, evidence_json, decided_at FROM rule_suggestions
             WHERE status IN ('discarded', 'accepted') ORDER BY fingerprint",
        )?;
        let rows = stmt.query_map([], |r| {
            let evidence: String = r.get(2)?;
            Ok(SuggestionRow {
                fingerprint: r.get(0)?,
                status: r.get(1)?,
                evidence: serde_json::from_str(&evidence).unwrap_or(Value::Null),
                decided_at: r.get(3)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Último afinado con IA guardado (D134), o `None` si nunca hubo uno. Solo lectura. El
    /// contenido lo valida quien lo escribió: aquí solo hay códigos, huellas y cifras.
    pub fn rule_refinement(&self) -> Result<Option<Value>> {
        let stored: Option<Option<String>> = self
            .conn
            .query_row(
                "SELECT model_json FROM rule_suggestions WHERE fingerprint = ?1 AND status = 'refinement'",
                [REFINEMENT_KEY],
                |r| r.get(0),
            )
            .optional()?;
        Ok(stored
            .flatten()
            .and_then(|json| serde_json::from_str(&json).ok()))
    }

    /// Guarda el resultado del último afinado con IA (D134) y lo audita, en una transacción.
    /// Una sola fila reservada: un nuevo afinado sustituye al anterior, que ya no sería
    /// vigente. No toca las decisiones sobre sugerencias. `outcome` (`ok` o `failed`) y
    /// `detail` son códigos, nunca contenido.
    pub fn save_refinement(&mut self, model: &Value, outcome: &str, detail: &str) -> Result<()> {
        let tx = self.conn.transaction()?;
        let now = now_ms();
        tx.execute(
            "INSERT INTO rule_suggestions (fingerprint, status, evidence_json, model_json, decided_at)
             VALUES (?1, 'refinement', '{}', ?2, ?3)
             ON CONFLICT(fingerprint) DO UPDATE SET model_json = excluded.model_json,
                 decided_at = excluded.decided_at",
            params![REFINEMENT_KEY, serde_json::to_string(model)?, now],
        )?;
        tx.execute(
            "INSERT INTO audit_log (at, task_id, action, status, authorization, detail) VALUES (?1, NULL, 'rule_refinement', ?2, NULL, ?3)",
            params![now, outcome, detail],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Descarta una sugerencia (D132): guarda la huella con la evidencia que había y lo audita,
    /// en una transacción. Descartar otra vez actualiza la referencia con la que se mide la
    /// evidencia nueva. Una sugerencia ya aceptada no se descarta.
    pub fn discard_suggestion(
        &mut self,
        fingerprint: &str,
        evidence: &Value,
        detail: &str,
    ) -> Result<()> {
        let tx = self.conn.transaction()?;
        let status: Option<String> = tx
            .query_row(
                "SELECT status FROM rule_suggestions WHERE fingerprint = ?1",
                [fingerprint],
                |r| r.get(0),
            )
            .optional()?;
        if status.as_deref() == Some("accepted") {
            return Err(CoreError::IdempotencyConflict(format!(
                "sugerencia {fingerprint} ya aceptada"
            )));
        }
        let now = now_ms();
        tx.execute(
            "INSERT INTO rule_suggestions (fingerprint, status, evidence_json, decided_at)
             VALUES (?1, 'discarded', ?2, ?3)
             ON CONFLICT(fingerprint) DO UPDATE SET status = 'discarded',
                 evidence_json = excluded.evidence_json, decided_at = excluded.decided_at",
            params![fingerprint, serde_json::to_string(evidence)?, now],
        )?;
        tx.execute(
            "INSERT INTO audit_log (at, task_id, action, status, authorization, detail) VALUES (?1, NULL, 'rule_suggestion', 'discarded', NULL, ?2)",
            params![now, detail],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Acepta una sugerencia (D132) en una sola transacción: crea la regla **inactiva** con su
    /// origen, marca la sugerencia como aceptada y lo audita. Es idempotente por huella: si ya
    /// estaba aceptada devuelve la regla que se creó entonces (`false`: no se creó nada).
    /// Nunca activa nada. Quien llama comprueba las condiciones con `check_rule`.
    #[allow(clippy::too_many_arguments)]
    pub fn accept_suggestion(
        &mut self,
        fingerprint: &str,
        new_id: &str,
        action_kind: &str,
        conditions: &Value,
        name: Option<&str>,
        origin: &Value,
        evidence: &Value,
    ) -> Result<(Rule, bool)> {
        let tx = self.conn.transaction()?;
        let accepted = accept_in_tx(
            &tx,
            &BatchRule {
                fingerprint,
                new_id,
                action_kind,
                conditions,
                name,
                origin,
                evidence,
            },
        )?;
        tx.commit()?;
        Ok(accepted)
    }

    /// Acepta y activa varias sugerencias aprendidas (D173) en **una sola transacción**: para cada
    /// regla, la misma aceptación que `accept_suggestion` (idempotente por huella) y su activación
    /// como una versión nueva. Todo o nada: si una falla (por ejemplo, un identificador ya usado o
    /// una regla archivada) se deshace todo y se devuelve el índice de la que falló; si falla la
    /// transacción misma (abrirla, el agregado de auditoría o confirmarla) no hay regla a la que
    /// atribuirlo y el índice es `None`. Deja una
    /// entrada de auditoría por regla y, si cambió algo, una agregada con `detail`; ninguna lleva
    /// contenido. Quien llama ya comprobó cada regla con `check_rule`.
    pub fn accept_and_activate_batch(
        &mut self,
        rules: &[BatchRule<'_>],
        detail: &str,
    ) -> std::result::Result<Vec<BatchApplied>, (Option<usize>, CoreError)> {
        let tx = self.conn.transaction().map_err(|e| (None, e.into()))?;
        let mut applied = Vec::with_capacity(rules.len());
        for (index, rule) in rules.iter().enumerate() {
            applied.push(apply_batch_rule(&tx, rule).map_err(|e| (Some(index), e))?);
        }
        if applied.iter().any(|a| a.created || a.activated) {
            tx.execute(
                "INSERT INTO audit_log (at, task_id, action, status, authorization, detail) VALUES (?1, NULL, 'rule_learned_batch', 'activated_all', NULL, ?2)",
                params![now_ms(), detail],
            )
            .map_err(|e| (None, e.into()))?;
        }
        tx.commit().map_err(|e| (None, e.into()))?;
        Ok(applied)
    }

    /// Desactiva todas las reglas de origen `learned` que estén activas (D174), en una transacción,
    /// con una entrada de auditoría por regla y una agregada. Idempotente: sin reglas activas de ese
    /// origen no escribe nada. Devuelve las versiones inactivas nuevas.
    pub fn deactivate_learned_rules(&mut self) -> Result<Vec<Rule>> {
        let tx = self.conn.transaction()?;
        let ids: Vec<String> = {
            let mut stmt = tx.prepare(
                "SELECT rule_id, origin_json FROM rule_meta WHERE origin_json IS NOT NULL ORDER BY rule_id",
            )?;
            let rows =
                stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
            let mut ids = Vec::new();
            for row in rows {
                let (id, origin) = row?;
                let origin: Value = serde_json::from_str(&origin).unwrap_or(Value::Null);
                if is_learned_origin(&origin) {
                    ids.push(id);
                }
            }
            ids
        };
        let mut off = Vec::new();
        for id in ids {
            let Some(current) = current_rule(&tx, &id)?.filter(|r| r.active) else {
                continue;
            };
            let rule = insert_rule_version(
                &tx,
                &id,
                current.version + 1,
                &current.action_kind,
                &current.conditions,
                false,
            )?;
            tx.execute(
                "INSERT INTO audit_log (at, task_id, action, status, authorization, detail) VALUES (?1, NULL, 'rule_learned_batch', 'deactivated', NULL, ?2)",
                params![
                    now_ms(),
                    format!("{id} v{} desactivada con las aprendidas", rule.version)
                ],
            )?;
            off.push(rule);
        }
        if !off.is_empty() {
            tx.execute(
                "INSERT INTO audit_log (at, task_id, action, status, authorization, detail) VALUES (?1, NULL, 'rule_learned_batch', 'deactivated_all', NULL, ?2)",
                params![
                    now_ms(),
                    format!("{} reglas aprendidas desactivadas", off.len())
                ],
            )?;
        }
        tx.commit()?;
        Ok(off)
    }

    /// Envíos de rodaje que le quedan a una regla (D174), o `None` si no tiene rodaje (no se aceptó
    /// por el lote aprendido). `Some(0)`: el rodaje terminó.
    pub fn rodaje_left(&self, id: &str) -> Result<Option<i64>> {
        Ok(rodaje_of(&self.conn, id)?.map(|state| state.left))
    }

    /// Estado completo del rodaje de una regla (D174), o `None` si no tiene. Un origen que existe
    /// pero no se puede leer devuelve el rodaje completo (falla cerrado).
    pub fn rodaje_state(&self, id: &str) -> Result<Option<RodajeState>> {
        rodaje_of(&self.conn, id)
    }

    /// Reinicia el rodaje de una regla que aún lo tiene a medias (D174): una edición o un rechazo
    /// de un envío de rodaje vuelven a exigir los envíos completos. Un rodaje ya terminado (0) o
    /// intacto (completo) no cambia. `true` si cambió algo.
    pub fn rodaje_reset(&mut self, id: &str) -> Result<bool> {
        let tx = self.conn.transaction()?;
        let changed = match rodaje_of(&tx, id)? {
            Some(state) if state.readable && state.left > 0 && state.left < state.of => {
                // La serie vuelve a empezar: también la cobertura de direcciones.
                let cleared: Vec<String> = Vec::new();
                set_rodaje(
                    &tx,
                    id,
                    state.of,
                    Some(&cleared),
                    "reiniciado por una edición o un rechazo",
                )?;
                true
            }
            _ => false,
        };
        tx.commit()?;
        Ok(changed)
    }

    /// Valor guardado de un ajuste del panel (`rule_settings`), si existe.
    pub fn rule_setting(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT value FROM rule_settings WHERE key = ?1",
                [key],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// Guarda un ajuste y lo deja en el historial, en una transacción. Quien llama valida el
    /// valor. `detail` describe el cambio (valor anterior y nuevo).
    pub fn set_rule_setting(&mut self, key: &str, value: &str, detail: &str) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO rule_settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        tx.execute(
            "INSERT INTO audit_log (at, task_id, action, status, authorization, detail) VALUES (?1, NULL, 'rule_setting', 'changed', NULL, ?2)",
            params![now_ms(), detail],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Borra un ajuste y lo deja en el historial, en una transacción. Devuelve si existía; si
    /// no existía no escribe nada.
    pub fn delete_rule_setting(&mut self, key: &str, detail: &str) -> Result<bool> {
        let tx = self.conn.transaction()?;
        let removed = tx.execute("DELETE FROM rule_settings WHERE key = ?1", [key])? > 0;
        if removed {
            tx.execute(
                "INSERT INTO audit_log (at, task_id, action, status, authorization, detail) VALUES (?1, NULL, 'rule_setting', 'removed', NULL, ?2)",
                params![now_ms(), detail],
            )?;
        }
        tx.commit()?;
        Ok(removed)
    }

    /// Marca de la primera lectura de una fuente (E5, D133), o `None` si nunca se empezó. Vive
    /// en `rule_settings` bajo `initial_read:<fuente>`, sin entrada en el historial: la lectura
    /// no ensucia el Historial. Solo ids, cifras y códigos; nunca contenido.
    pub fn read_mark(&self, source: &str) -> Result<Option<Value>> {
        Ok(self
            .rule_setting(&format!("{READ_MARK_PREFIX}{source}"))?
            .and_then(|text| serde_json::from_str(&text).ok()))
    }

    /// Guarda (o sustituye) la marca de una fuente. Idempotente; sin entrada en el historial.
    pub fn set_read_mark(&mut self, source: &str, mark: &Value) -> Result<()> {
        self.conn.execute(
            "INSERT INTO rule_settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![format!("{READ_MARK_PREFIX}{source}"), mark.to_string()],
        )?;
        Ok(())
    }

    /// Marca de la fase de aprendizaje de una fuente (D175), o `None` si nunca se empezó. Vive en
    /// `rule_settings` bajo `first_learning:<fuente>`, sin entrada en el historial.
    pub fn learning_mark(&self, source: &str) -> Result<Option<Value>> {
        Ok(self
            .rule_setting(&format!("{LEARN_MARK_PREFIX}{source}"))?
            .and_then(|text| serde_json::from_str(&text).ok()))
    }

    /// Lo aprendido de una fuente (`first_learning_result:<fuente>`), si ya terminó.
    pub fn learning_result(&self, source: &str) -> Result<Option<Value>> {
        Ok(self
            .rule_setting(&format!("{LEARN_RESULT_PREFIX}{source}"))?
            .and_then(|text| serde_json::from_str(&text).ok()))
    }

    /// Guarda en **una** transacción la marca de aprendizaje, lo aprendido (si lo hay) y, si se
    /// pide, una entrada agregada del historial `(acción, estado, detalle)`. Sustituir una marca no
    /// escribe nada en el historial; solo la nota explícita. Idempotente.
    ///
    /// Con `evidence`, en esa misma transacción: las señales `learned` (únicas por correo recibido:
    /// repetirlo no duplica filas) y los recuentos de `reply_style`, que solo se suman la primera
    /// vez que se guarda el resultado de esa fuente (no hay resultado previo).
    pub fn save_learning(
        &mut self,
        source: &str,
        mark: &Value,
        result: Option<&Value>,
        note: Option<(&str, &str, &Value)>,
        evidence: Option<&LearnedEvidence>,
    ) -> Result<()> {
        let tx = self.conn.transaction()?;
        let upsert = "INSERT INTO rule_settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value";
        tx.execute(
            upsert,
            params![format!("{LEARN_MARK_PREFIX}{source}"), mark.to_string()],
        )?;
        let result_key = format!("{LEARN_RESULT_PREFIX}{source}");
        let first_result = tx
            .query_row(
                "SELECT 1 FROM rule_settings WHERE key = ?1",
                [&result_key],
                |_| Ok(()),
            )
            .optional()?
            .is_none();
        if let Some(result) = result {
            tx.execute(upsert, params![result_key, result.to_string()])?;
        }
        if let Some(evidence) = evidence {
            let now = now_ms();
            for s in &evidence.signals {
                // Única también por fuente + id interno del mensaje: la fuente o el id de la cuenta
                // pueden haber cambiado desde que el aprendizaje continuo contó ese correo (D178).
                crate::continuous::insert_learned(&tx, &s.subject_key, &s.source_key, now)?;
            }
            if first_result {
                for (category, kind) in &evidence.reply_style {
                    tx.execute(
                        "INSERT INTO reply_style (category, reply_kind, n) VALUES (?1, ?2, 1)
                         ON CONFLICT(category, reply_kind) DO UPDATE SET n = n + 1",
                        params![category, kind],
                    )?;
                }
            }
        }
        if let Some((action, status, detail)) = note {
            tx.execute(
                "INSERT INTO audit_log (at, task_id, action, status, authorization, detail) VALUES (?1, NULL, ?2, ?3, NULL, ?4)",
                params![now_ms(), action, status, detail.to_string()],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Recuentos de `reply_style`: `(categoría, reply_kind, n)`, en orden estable.
    pub fn reply_style_rows(&self) -> Result<Vec<(String, String, i64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT category, reply_kind, n FROM reply_style ORDER BY category, reply_kind",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Todas las marcas de aprendizaje, por fuente, en orden estable.
    pub fn learning_marks(&self) -> Result<Vec<(String, Value)>> {
        let mut stmt = self.conn.prepare(
            "SELECT key, value FROM rule_settings WHERE substr(key, 1, ?1) = ?2 ORDER BY key",
        )?;
        let rows = stmt.query_map(
            params![LEARN_MARK_PREFIX.len() as i64, LEARN_MARK_PREFIX],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
        )?;
        let mut out = Vec::new();
        for row in rows {
            let (key, value) = row?;
            if let (Some(source), Ok(mark)) = (
                key.strip_prefix(LEARN_MARK_PREFIX),
                serde_json::from_str::<Value>(&value),
            ) {
                out.push((source.to_owned(), mark));
            }
        }
        Ok(out)
    }

    /// Todas las marcas de primera lectura, por fuente, en orden estable.
    pub fn read_marks(&self) -> Result<Vec<(String, Value)>> {
        let mut stmt = self.conn.prepare(
            "SELECT key, value FROM rule_settings WHERE substr(key, 1, ?1) = ?2 ORDER BY key",
        )?;
        let rows = stmt.query_map(
            params![READ_MARK_PREFIX.len() as i64, READ_MARK_PREFIX],
            |r| {
                let key: String = r.get(0)?;
                let value: String = r.get(1)?;
                Ok((key, value))
            },
        )?;
        let mut out = Vec::new();
        for row in rows {
            let (key, value) = row?;
            if let (Some(source), Ok(mark)) = (
                key.strip_prefix(READ_MARK_PREFIX),
                serde_json::from_str::<Value>(&value),
            ) {
                out.push((source.to_owned(), mark));
            }
        }
        Ok(out)
    }

    /// Autorización y estado de la última entrada del historial que confirmó o reconcilió la
    /// acción (`confirmed` o `reconciled`). `None` si no consta.
    fn last_execution_authorization(
        &self,
        task_id: i64,
        key: &str,
    ) -> Result<Option<(String, String)>> {
        Ok(self
            .conn
            .query_row(
                "SELECT authorization, status FROM audit_log
                 WHERE task_id = ?1 AND detail = ?2 AND status IN ('confirmed', 'reconciled')
                 ORDER BY id DESC LIMIT 1",
                params![task_id, key],
                |r| {
                    Ok((
                        r.get::<_, Option<String>>(0)?.unwrap_or_default(),
                        r.get(1)?,
                    ))
                },
            )
            .optional()?)
    }

    /// Contadores de todas las reglas conocidas. `applied` cuenta acciones `confirmed`
    /// (incluidas las reconciliadas) por `rule_id`; las filas anteriores a la migración no
    /// lo tienen y se leen con un análisis estricto de «regla X vN», solo para esta consulta.
    pub fn rule_stats(&self) -> Result<Vec<RuleStats>> {
        let ids: Vec<String> = {
            let mut stmt = self
                .conn
                .prepare("SELECT DISTINCT id FROM rules ORDER BY id")?;
            let rows = stmt.query_map([], |r| r.get(0))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        let known: HashSet<&str> = ids.iter().map(String::as_str).collect();
        let mut stats: HashMap<String, RuleStats> = ids
            .iter()
            .map(|id| {
                (
                    id.clone(),
                    RuleStats {
                        rule_id: id.clone(),
                        applied: 0,
                        last_applied_at: None,
                        reviewed: 0,
                        unclassified: 0,
                    },
                )
            })
            .collect();

        let mut stmt = self.conn.prepare(
            "SELECT rule_id, authorization, status, updated_at, idempotency_key, task_id FROM actions
             WHERE rule_id IS NOT NULL OR authorization LIKE 'regla %'",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, Option<String>>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, i64>(5)?,
            ))
        })?;
        for row in rows {
            let (rule_id, authorization, status, updated_at, key, task_id) = row?;
            let applied = status == "confirmed";
            let mut unclassified = None;
            let attributed = match rule_id {
                Some(id) => Some(id),
                None => match parse_rule_label(&authorization)
                    .map(|(id, _)| id)
                    .filter(|id| known.contains(id.as_str()))
                {
                    // Fila anterior a `rule_id`: la etiqueta de `actions` pudo quedar de un
                    // intento de regla que luego ejecutó una persona. Se cree solo si la
                    // última entrada del historial de esa acción la confirma (D129).
                    Some(id) if applied => {
                        match self.last_execution_authorization(task_id, &key)? {
                            Some((a, _)) if a == authorization => Some(id),
                            // Ejecutó una persona al aprobar: no es de la regla.
                            Some((a, s)) if a == "aprobación" && s == "confirmed" => None,
                            // Sin entrada, o `reconciled` con otra autorización: no se distingue.
                            _ => {
                                unclassified = Some(id);
                                None
                            }
                        }
                    }
                    Some(id) => Some(id),
                    None => None,
                },
            };
            match (attributed, unclassified) {
                (Some(id), _) => {
                    if let Some(entry) = stats.get_mut(&id) {
                        if applied {
                            entry.applied += 1;
                            entry.last_applied_at = Some(
                                entry
                                    .last_applied_at
                                    .map_or(updated_at, |t| t.max(updated_at)),
                            );
                        }
                    }
                }
                (None, Some(id)) => {
                    if let Some(entry) = stats.get_mut(&id) {
                        entry.unclassified += 1;
                    }
                }
                (None, None) => {
                    // Quien la ejecutó fue una persona, o cita una regla pero no en un
                    // formato que se pueda leer con certeza.
                    if parse_rule_label(&authorization).is_none() {
                        if let Some(id) = ids
                            .iter()
                            .filter(|id| authorization.starts_with(&format!("regla {id}")))
                            .max_by_key(|id| id.len())
                        {
                            if let Some(entry) = stats.get_mut(id) {
                                entry.unclassified += 1;
                            }
                        }
                    }
                }
            }
        }
        drop(stmt);

        // Tareas que una regla activa habría automatizado y fueron a revisión.
        let mut seen: HashSet<(i64, i64, String)> = HashSet::new();
        let mut stmt = self.conn.prepare(
            "SELECT task_id, content_version, checks_json FROM rule_evaluations WHERE outcome = 'review'",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?;
        for row in rows {
            let (task_id, version, checks) = row?;
            let checks: Value = serde_json::from_str(&checks).unwrap_or(Value::Null);
            for check in checks["candidates"].as_array().into_iter().flatten() {
                let (Some(id), true, true) = (
                    check["id"].as_str(),
                    check["active"].as_bool().unwrap_or(false),
                    check["matches"].as_bool().unwrap_or(false),
                ) else {
                    continue;
                };
                if let Some(entry) = stats.get_mut(id) {
                    if seen.insert((task_id, version, id.to_owned())) {
                        entry.reviewed += 1;
                    }
                }
            }
        }
        let mut out: Vec<RuleStats> = stats.into_values().collect();
        out.sort_by(|a, b| a.rule_id.cmp(&b.rule_id));
        Ok(out)
    }
}

/// Archiva una regla dentro de una transacción: inactiva (versión nueva si estaba activa),
/// marcada, sin validación avanzada y con su entrada en el historial.
fn archive_in(tx: &rusqlite::Transaction<'_>, id: &str) -> Result<Rule> {
    let current = current_rule(tx, id)?.ok_or_else(|| CoreError::RuleNotFound(id.to_owned()))?;
    if is_archived(tx, id)? {
        return Ok(current);
    }
    let rule = if current.active {
        insert_rule_version(
            tx,
            id,
            current.version + 1,
            &current.action_kind,
            &current.conditions,
            false,
        )?
    } else {
        current
    };
    tx.execute(
        "INSERT INTO rule_meta (rule_id, archived_at) VALUES (?1, ?2)
         ON CONFLICT(rule_id) DO UPDATE SET archived_at = excluded.archived_at",
        params![id, now_ms()],
    )?;
    // La validación avanzada no sobrevive al archivado: desarchivar no debe reactivar el
    // «cualquier remitente» sin que alguien lo vuelva a validar (D126).
    let had_ack = tx.execute(
        "UPDATE rule_meta SET allow_any_sender_ack_at = NULL
         WHERE rule_id = ?1 AND allow_any_sender_ack_at IS NOT NULL",
        [id],
    )?;
    if had_ack > 0 {
        audit_any_sender(tx, &rule, false, "retirada: la regla se archivó")?;
    }
    tx.execute(
        "INSERT INTO audit_log (at, task_id, action, status, authorization, detail) VALUES (?1, NULL, 'rule_archive', 'archived', NULL, ?2)",
        params![now_ms(), format!("{id} v{}", rule.version)],
    )?;
    Ok(rule)
}

/// Escribe el nombre visible de una regla y lo deja en el historial.
fn write_name(tx: &rusqlite::Transaction<'_>, id: &str, name: Option<&str>) -> Result<()> {
    tx.execute(
        "INSERT INTO rule_meta (rule_id, name) VALUES (?1, ?2)
         ON CONFLICT(rule_id) DO UPDATE SET name = excluded.name",
        params![id, name],
    )?;
    tx.execute(
        "INSERT INTO audit_log (at, task_id, action, status, authorization, detail) VALUES (?1, NULL, 'rule_rename', 'renamed', NULL, ?2)",
        params![now_ms(), format!("{id}: {}", name.unwrap_or("(sin nombre)"))],
    )?;
    Ok(())
}

/// Análisis estricto de la autorización «regla X vN» de las filas anteriores a `rule_id`.
/// Lo que no encaje exactamente devuelve `None`.
pub fn parse_rule_label(label: &str) -> Option<(String, i64)> {
    let rest = label.strip_prefix("regla ")?;
    let (id, version) = rest.rsplit_once(" v")?;
    if id.is_empty() || id.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return None;
    }
    if version.is_empty() || !version.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some((id.to_owned(), version.parse().ok()?))
}

/// Envíos de rodaje de una regla aprendida aceptada en lote (D174).
pub const RODAJE_SENDS: i64 = 3;

/// Una regla de un lote que se acepta (D173). Solo ids, condiciones y cifras.
#[derive(Debug, Clone)]
pub struct BatchRule<'a> {
    pub fingerprint: &'a str,
    pub new_id: &'a str,
    pub action_kind: &'a str,
    pub conditions: &'a Value,
    pub name: Option<&'a str>,
    pub origin: &'a Value,
    pub evidence: &'a Value,
}

/// Lo que pasó con una regla del lote: se creó ahora o ya existía, y si se activó ahora.
#[derive(Debug, Clone, PartialEq)]
pub struct BatchApplied {
    pub rule: Rule,
    pub created: bool,
    pub activated: bool,
}

/// El origen de la regla es una sugerencia con evidencia aprendida en la primera carga (D172).
pub fn is_learned_origin(origin: &Value) -> bool {
    origin["evidence_origin"] == "learned"
}

/// Acepta una sugerencia dentro de una transacción (lógica común de `accept_suggestion` y del lote).
fn accept_in_tx(tx: &rusqlite::Transaction<'_>, r: &BatchRule<'_>) -> Result<(Rule, bool)> {
    let prior: Option<(String, String)> = tx
        .query_row(
            "SELECT status, evidence_json FROM rule_suggestions WHERE fingerprint = ?1",
            [r.fingerprint],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    if let Some((status, stored)) = prior {
        if status == "accepted" {
            let stored: Value = serde_json::from_str(&stored).unwrap_or(Value::Null);
            if let Some(rule) = stored["rule_id"]
                .as_str()
                .map(|id| current_rule(tx, id))
                .transpose()?
                .flatten()
            {
                return Ok((rule, false));
            }
        }
    }
    if current_rule(tx, r.new_id)?.is_some() {
        return Err(CoreError::IdempotencyConflict(format!(
            "regla {}",
            r.new_id
        )));
    }
    let rule = insert_rule_version(tx, r.new_id, 1, r.action_kind, r.conditions, false)?;
    tx.execute(
        "INSERT INTO rule_meta (rule_id, name, origin_json) VALUES (?1, ?2, ?3)",
        params![r.new_id, r.name, serde_json::to_string(r.origin)?],
    )?;
    let mut stored = r.evidence.clone();
    stored["rule_id"] = Value::String(r.new_id.to_owned());
    let now = now_ms();
    tx.execute(
        "INSERT INTO rule_suggestions (fingerprint, status, evidence_json, decided_at)
         VALUES (?1, 'accepted', ?2, ?3)
         ON CONFLICT(fingerprint) DO UPDATE SET status = 'accepted',
             evidence_json = excluded.evidence_json, decided_at = excluded.decided_at",
        params![r.fingerprint, serde_json::to_string(&stored)?, now],
    )?;
    tx.execute(
        "INSERT INTO audit_log (at, task_id, action, status, authorization, detail) VALUES (?1, NULL, 'rule_suggestion', 'accepted', NULL, ?2)",
        params![
            now,
            format!(
                "{} v1 (inactiva) creada desde la sugerencia {}",
                r.new_id, r.fingerprint
            )
        ],
    )?;
    Ok((rule, true))
}

/// Acepta (si falta) y activa una regla del lote dentro de la transacción del lote.
fn apply_batch_rule(tx: &rusqlite::Transaction<'_>, r: &BatchRule<'_>) -> Result<BatchApplied> {
    let (rule, created) = accept_in_tx(tx, r)?;
    if is_archived(tx, &rule.id)? {
        return Err(CoreError::RuleArchived(rule.id));
    }
    let (rule, activated) = if rule.active {
        (rule, false)
    } else {
        let active = insert_rule_version(
            tx,
            &rule.id,
            rule.version + 1,
            &rule.action_kind,
            &rule.conditions,
            true,
        )?;
        tx.execute(
            "INSERT INTO audit_log (at, task_id, action, status, authorization, detail) VALUES (?1, NULL, 'rule_learned_batch', 'activated', NULL, ?2)",
            params![
                now_ms(),
                format!(
                    "{} v{} aceptada y activada en lote desde la sugerencia {}",
                    active.id, active.version, r.fingerprint
                )
            ],
        )?;
        (active, true)
    };
    Ok(BatchApplied {
        rule,
        created,
        activated,
    })
}

/// Estado del rodaje de una regla aprendida (D174), leído de `origin.rodaje`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RodajeState {
    /// Aprobaciones sin editar que faltan.
    pub left: i64,
    pub of: i64,
    /// Direcciones distintas que se exigen entre las aprobaciones (reglas por dominio; 0 si no).
    pub min_addresses: usize,
    /// Huellas (nunca la dirección) de las direcciones ya cubiertas por aprobaciones contadas.
    pub address_hashes: Vec<String>,
    /// `false`: el origen existe pero no se pudo leer; el estado es el del rodaje completo.
    pub readable: bool,
}

impl RodajeState {
    /// Rodaje completo y sin cobertura: lo que se supone cuando no se puede leer el contador.
    pub fn closed() -> Self {
        Self {
            left: RODAJE_SENDS,
            of: RODAJE_SENDS,
            min_addresses: 0,
            address_hashes: Vec::new(),
            readable: false,
        }
    }

    /// Direcciones distintas cubiertas.
    pub fn addresses(&self) -> usize {
        self.address_hashes.len()
    }

    /// El rodaje sigue activo: faltan aprobaciones o, en una regla por dominio, direcciones.
    pub fn active(&self) -> bool {
        self.left > 0 || self.addresses() < self.min_addresses
    }
}

/// Huella de una dirección (minúsculas, sin espacios): lo único que se guarda de ella.
fn address_hash(address: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(address.trim().to_ascii_lowercase().as_bytes())
        .iter()
        .take(8)
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Estado del rodaje leído del origen de la regla. `None` si el origen es NULL o no lleva la marca.
/// Con un origen que existe pero no se puede leer, falla cerrado: rodaje completo.
fn rodaje_of(conn: &rusqlite::Connection, id: &str) -> Result<Option<RodajeState>> {
    let origin: Option<Option<String>> = conn
        .query_row(
            "SELECT origin_json FROM rule_meta WHERE rule_id = ?1",
            [id],
            |r| r.get(0),
        )
        .optional()?;
    let Some(Some(text)) = origin else {
        return Ok(None);
    };
    let origin: Value = match serde_json::from_str(&text) {
        Ok(origin @ Value::Object(_)) => origin,
        _ => return Ok(Some(RodajeState::closed())),
    };
    let rodaje = &origin["rodaje"];
    if rodaje.is_null() {
        return Ok(None);
    }
    // Con marca de rodaje pero un contador ilegible, falla cerrado: el rodaje sigue completo.
    Ok(Some(RodajeState {
        left: rodaje["left"]
            .as_i64()
            .map_or(RODAJE_SENDS, |left| left.clamp(0, RODAJE_SENDS)),
        of: rodaje["of"].as_i64().unwrap_or(RODAJE_SENDS),
        min_addresses: rodaje["min_addresses"]
            .as_u64()
            .map_or(0, |n| usize::try_from(n).unwrap_or(usize::MAX)),
        address_hashes: rodaje["address_hashes"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default(),
        readable: true,
    }))
}

/// Cambia el contador de rodaje del origen de la regla (y, si se pasan, las huellas de direcciones
/// cubiertas) y lo deja en el historial, sin contenido. Un origen ilegible no se reescribe.
fn set_rodaje(
    tx: &rusqlite::Transaction<'_>,
    id: &str,
    left: i64,
    hashes: Option<&[String]>,
    why: &str,
) -> Result<()> {
    let text: Option<String> = tx.query_row(
        "SELECT origin_json FROM rule_meta WHERE rule_id = ?1",
        [id],
        |r| r.get(0),
    )?;
    let mut origin: Value = serde_json::from_str(text.as_deref().unwrap_or("null"))?;
    origin["rodaje"]["left"] = Value::from(left);
    if let Some(hashes) = hashes {
        origin["rodaje"]["address_hashes"] = Value::from(hashes.to_vec());
    }
    tx.execute(
        "UPDATE rule_meta SET origin_json = ?2 WHERE rule_id = ?1",
        params![id, serde_json::to_string(&origin)?],
    )?;
    let covered = origin["rodaje"]["min_addresses"]
        .as_u64()
        .filter(|n| *n > 1)
        .map(|n| {
            format!(
                "; direcciones cubiertas {} de {n}",
                origin["rodaje"]["address_hashes"]
                    .as_array()
                    .map_or(0, Vec::len)
            )
        })
        .unwrap_or_default();
    tx.execute(
        "INSERT INTO audit_log (at, task_id, action, status, authorization, detail) VALUES (?1, NULL, 'rule_rodaje', ?2, NULL, ?3)",
        params![
            now_ms(),
            if left == 0 { "finished" } else { "counted" },
            format!("{id}: {why}; faltan {left} envíos de rodaje{covered}")
        ],
    )?;
    Ok(())
}

fn meta_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RuleMeta> {
    Ok(RuleMeta {
        rule_id: row.get(0)?,
        name: row.get(1)?,
        archived_at: row.get(2)?,
        allow_any_sender_ack_at: row.get(3)?,
        origin: row
            .get::<_, Option<String>>(4)?
            .and_then(|text| serde_json::from_str(&text).ok()),
    })
}

#[cfg(test)]
mod tests {
    use super::parse_rule_label;

    #[test]
    fn only_the_exact_label_is_read() {
        assert_eq!(parse_rule_label("regla alfa v3"), Some(("alfa".into(), 3)));
        assert_eq!(
            parse_rule_label("regla a-b-1 v12"),
            Some(("a-b-1".into(), 12))
        );
        for bad in [
            "aprobación",
            "regla alfa",
            "regla alfa v",
            "regla alfa vx",
            "regla  v3",
            "regla dos palabras v3",
            "Regla alfa v3",
            "regla alfa v3 extra",
        ] {
            assert_eq!(parse_rule_label(bad), None, "{bad}");
        }
    }
}
