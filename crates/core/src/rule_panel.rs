//! Datos del panel de reglas (D124, D126, D128, D129): nombre y archivado, validación
//! avanzada sin remitente, evaluaciones, señales y contadores. Ninguna tabla guarda cuerpos ni
//! textos de correo: solo ids, códigos y claves ya normalizadas.

use std::collections::{HashMap, HashSet};

use rusqlite::{params, OptionalExtension};
use serde::Serialize;
use serde_json::Value;

use crate::error::{CoreError, Result};
use crate::rules::{audit_any_sender, current_rule, insert_rule_version, is_archived, live_rule};
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
    pub task_id: i64,
    pub content_version: i64,
    pub source_key: Option<String>,
    /// `valid`, `pending` o `void`. Las sugerencias solo usan las `valid`.
    pub state: String,
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
    /// Quien llama comprueba las condiciones con `check_rule`.
    pub fn update_rule_full(
        &mut self,
        id: &str,
        conditions: &Value,
        name: Option<Option<&str>>,
        any_sender_ack: bool,
    ) -> Result<Rule> {
        let tx = self.conn.transaction()?;
        let current = live_rule(&tx, id)?;
        let rule = insert_rule_version(
            &tx,
            id,
            current.version + 1,
            &current.action_kind,
            conditions,
            current.active,
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
        Ok(rule)
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
        self.conn.execute(
            "UPDATE rule_signals SET state = ?3
             WHERE kind = 'approved' AND task_id = ?1 AND content_version = ?2",
            params![
                task_id,
                content_version,
                if valid { "valid" } else { "void" }
            ],
        )?;
        Ok(())
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
            "SELECT fingerprint, status, evidence_json, decided_at FROM rule_suggestions ORDER BY fingerprint",
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
        let prior: Option<(String, String)> = tx
            .query_row(
                "SELECT status, evidence_json FROM rule_suggestions WHERE fingerprint = ?1",
                [fingerprint],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if let Some((status, stored)) = prior {
            if status == "accepted" {
                let stored: Value = serde_json::from_str(&stored).unwrap_or(Value::Null);
                if let Some(rule) = stored["rule_id"]
                    .as_str()
                    .map(|id| current_rule(&tx, id))
                    .transpose()?
                    .flatten()
                {
                    return Ok((rule, false));
                }
            }
        }
        if current_rule(&tx, new_id)?.is_some() {
            return Err(CoreError::IdempotencyConflict(format!("regla {new_id}")));
        }
        let rule = insert_rule_version(&tx, new_id, 1, action_kind, conditions, false)?;
        tx.execute(
            "INSERT INTO rule_meta (rule_id, name, origin_json) VALUES (?1, ?2, ?3)",
            params![new_id, name, serde_json::to_string(origin)?],
        )?;
        let mut stored = evidence.clone();
        stored["rule_id"] = Value::String(new_id.to_owned());
        let now = now_ms();
        tx.execute(
            "INSERT INTO rule_suggestions (fingerprint, status, evidence_json, decided_at)
             VALUES (?1, 'accepted', ?2, ?3)
             ON CONFLICT(fingerprint) DO UPDATE SET status = 'accepted',
                 evidence_json = excluded.evidence_json, decided_at = excluded.decided_at",
            params![fingerprint, serde_json::to_string(&stored)?, now],
        )?;
        tx.execute(
            "INSERT INTO audit_log (at, task_id, action, status, authorization, detail) VALUES (?1, NULL, 'rule_suggestion', 'accepted', NULL, ?2)",
            params![
                now,
                format!("{new_id} v1 (inactiva) creada desde la sugerencia {fingerprint}")
            ],
        )?;
        tx.commit()?;
        Ok((rule, true))
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
