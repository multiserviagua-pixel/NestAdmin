//! Aceptar y activar en lote las reglas aprendidas en la primera carga (D173) y desactivarlas
//! todas (D174). El rodaje de los primeros envíos vive en `finish`, `approve`, `edit_draft` y
//! `reject` (`lib.rs`) y en `rule_panel.rs`; aquí solo se marca al aceptar.
//!
//! Nada de lo que sale de aquí lleva contenido de correos: ids, nombres de regla (derivados de la
//! dirección del remitente o su dominio, que ya son las condiciones de la regla), recuentos y
//! códigos.

use std::collections::HashSet;

use nestadmin_core::{is_learned_origin, BatchRule, CoreError, RODAJE_SENDS};
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::suggest::{stale_message, Candidate, DOMAIN_MIN_ADDRESSES};
use crate::{check_rule, App, AppError, AppResult, SEND_DRAFT};

/// El rodaje con el que nace toda regla de origen aprendido (D174, D178): [`RODAJE_SENDS`]
/// envíos. Una regla por dominio no sale del rodaje hasta aprobar a varias direcciones distintas
/// (solo se guardan sus huellas). Lo comparten el lote y la aceptación de una tarjeta.
pub(crate) fn learned_rodaje(conditions: &Value) -> Value {
    let mut rodaje = json!({ "left": RODAJE_SENDS, "of": RODAJE_SENDS });
    if conditions.get("sender_domain").is_some() {
        rodaje["min_addresses"] = json!(DOMAIN_MIN_ADDRESSES);
        rodaje["address_hashes"] = json!([]);
    }
    rodaje
}

/// Una regla de la tanda aprendida, con la evidencia que la sostiene. Solo recuentos y la dirección
/// o el dominio de la propia regla; nunca texto de correos.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LearnedBatchItem {
    /// Identificador de la sugerencia (su huella): es el que se pasa a `accept_and_activate_learned`.
    pub id: String,
    /// Huella de las condiciones tal como se muestran.
    pub shown: String,
    pub name: String,
    pub category: Option<String>,
    /// Dirección exacta o dominio al que respondería la regla.
    pub subject: String,
    pub conditions: Value,
    /// Pares aprendidos que la sostienen, y el umbral que se exigió.
    pub count: u32,
    pub threshold: u32,
    pub learned: u32,
    /// Direcciones distintas (más de una solo en las reglas por dominio).
    pub addresses: u32,
}

/// Las sugerencias aprendidas pendientes, con una huella estable de todas ellas (D173).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LearnedBatch {
    /// Cubre los ids y las huellas de cada sugerencia: si algo cambia, la aceptación se rechaza.
    pub batch_fp: String,
    pub items: Vec<LearnedBatchItem>,
    /// Envíos de rodaje que tendrá cada regla al aceptarla (D174).
    pub rodaje_sends: i64,
}

/// Qué pasó con una regla de la aceptación.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LearnedAcceptedRule {
    /// El id de la tanda (huella de la sugerencia).
    pub id: String,
    pub rule_id: String,
    pub name: String,
    /// `activated` (aceptada y activada ahora) o `already_active` (ya lo estaba: nada cambió).
    pub status: &'static str,
    /// Envíos de rodaje que le quedan (`None`: sin rodaje).
    pub rodaje_left: Option<i64>,
}

/// La regla por la que se deshizo todo, y por qué, para que la persona pueda desmarcarla.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LearnedAcceptFailure {
    /// La regla que falló; `None` si falló la transacción misma (`batch_write_failed`).
    pub id: Option<String>,
    pub name: Option<String>,
    /// `invalid_rule` (no pasa `check_rule`), `not_valid` (la sugerencia ya no es válida),
    /// `rule_exists`, `rule_archived`, `write_failed` (una regla) o `batch_write_failed` (abrir,
    /// agregar la auditoría o confirmar la transacción: sin regla asociada).
    pub code: &'static str,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LearnedAcceptOutcome {
    /// `true`: todas las reglas pedidas quedaron activas (ahora o ya lo estaban). `false`: no se
    /// escribió nada y `failed` dice por qué.
    pub applied: bool,
    pub rules: Vec<LearnedAcceptedRule>,
    pub activated: usize,
    pub already_active: usize,
    pub failed: Option<LearnedAcceptFailure>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LearnedDeactivateOutcome {
    /// Reglas que estaban activas y ahora no.
    pub deactivated: Vec<String>,
    /// Reglas aprendidas que existen (activas o no).
    pub learned_total: usize,
}

fn batch_stale_message() -> String {
    "La tanda de reglas aprendidas ha cambiado desde que la vio: revise la lista de nuevo antes de aceptar."
        .into()
}

fn batch_of(candidates: &[Candidate]) -> LearnedBatch {
    let items: Vec<LearnedBatchItem> = candidates
        .iter()
        .filter(|c| {
            c.action_kind == SEND_DRAFT
                && c.evidence.origin == "learned"
                && !c.evidence.needs_limit
                // Una sugerencia descartada que reaparece solo se acepta con su tarjeta (D173).
                && c.evidence.reappeared.is_none()
        })
        .map(|c| LearnedBatchItem {
            id: c.fingerprint.clone(),
            shown: c.shown.clone(),
            name: c.name.clone(),
            category: c.category.clone(),
            subject: c.subject.clone(),
            conditions: c.conditions.clone(),
            count: c.evidence.count,
            threshold: c.evidence.threshold,
            learned: c.evidence.learned,
            addresses: c.evidence.addresses,
        })
        .collect();
    let mut parts: Vec<String> = items
        .iter()
        .map(|i| {
            format!(
                "{}|{}|{}|{}|{}",
                i.id, i.shown, i.count, i.learned, i.addresses
            )
        })
        .collect();
    parts.sort();
    let digest = Sha256::digest(parts.join("\n").as_bytes());
    LearnedBatch {
        batch_fp: digest.iter().take(8).map(|b| format!("{b:02x}")).collect(),
        items,
        rodaje_sends: RODAJE_SENDS,
    }
}

impl App {
    /// Las sugerencias aprendidas pendientes (origen `learned`) con su evidencia y la huella de la
    /// tanda. Solo lectura: no escribe nada, no llama a la IA ni contacta a ningún proveedor.
    pub fn learned_suggestions_batch(&self) -> AppResult<LearnedBatch> {
        Ok(batch_of(&self.rule_suggestions()?.candidates))
    }

    /// Acepta y activa de una vez las reglas aprendidas que la persona dejó marcadas (D173,
    /// excepción explícita a D131 y D132: un clic, con evidencia por regla).
    ///
    /// - Rechaza una `batch_fp` caducada (la tanda cambió) y los ids que no pertenezcan a ella.
    /// - Cada regla pasa por `check_rule` y luego se acepta y se activa en **una sola transacción**
    ///   del almacén: todo o nada. Si una falla se deshace todo y la respuesta dice cuál y por qué.
    /// - Repetirlo no duplica nada: las ya aceptadas y activas se devuelven como `already_active`.
    /// - Cada regla nace con `origin.batch` y un rodaje de [`RODAJE_SENDS`] envíos (D174).
    /// - Deja una auditoría por regla y una entrada agregada, sin contenido.
    /// - La compuerta `send_draft_blocker` no cambia: activar no la salta.
    pub fn accept_and_activate_learned(
        &mut self,
        batch_fp: &str,
        ids: &[String],
    ) -> AppResult<LearnedAcceptOutcome> {
        self.accept_and_activate_learned_with(batch_fp, ids, &check_rule)
    }

    /// Como [`App::accept_and_activate_learned`] con la comprobación de cada regla a elegir; existe
    /// para probar que un fallo de `check_rule` deshace todo. La interfaz usa la normal.
    #[doc(hidden)]
    pub fn accept_and_activate_learned_with(
        &mut self,
        batch_fp: &str,
        ids: &[String],
        check: &dyn Fn(&str, &Value, bool) -> AppResult<()>,
    ) -> AppResult<LearnedAcceptOutcome> {
        let mut wanted: Vec<&str> = Vec::new();
        for id in ids.iter().map(|id| id.trim()).filter(|id| !id.is_empty()) {
            if !wanted.contains(&id) {
                wanted.push(id);
            }
        }
        if wanted.is_empty() {
            return Err(AppError::Invalid(
                "Deje marcada al menos una regla para aceptar.".into(),
            ));
        }

        // Las que ya se aceptaron y están activas no necesitan nada: repetir la llamada, o un
        // reintento tras un corte, devuelve lo mismo sin escribir.
        let rows = self.store.rule_suggestion_rows()?;
        let metas = self.store.rule_metas()?;
        let mut done: Vec<LearnedAcceptedRule> = Vec::new();
        let mut pending: Vec<&str> = Vec::new();
        for id in &wanted {
            let accepted = rows
                .iter()
                .find(|r| r.fingerprint == *id && r.status == "accepted")
                .and_then(|r| r.evidence["rule_id"].as_str());
            let live = match accepted {
                Some(rule_id) => match (self.store.rule(rule_id)?, metas.get(rule_id)) {
                    (Some(rule), Some(meta))
                        if rule.active
                            && meta.archived_at.is_none()
                            && meta.origin.as_ref().is_some_and(is_learned_origin) =>
                    {
                        Some(LearnedAcceptedRule {
                            id: (*id).to_owned(),
                            rule_id: rule.id.clone(),
                            name: meta.name.clone().unwrap_or_else(|| rule.id.clone()),
                            status: "already_active",
                            rodaje_left: self.store.rodaje_left(&rule.id)?,
                        })
                    }
                    _ => None,
                },
                None => None,
            };
            match live {
                Some(rule) => done.push(rule),
                None => pending.push(id),
            }
        }
        if pending.is_empty() {
            return Ok(outcome(done));
        }

        // Algo falta por aceptar: la tanda que la persona vio debe seguir siendo la de ahora.
        let all = self.rule_suggestions()?;
        let batch = batch_of(&all.candidates);
        if batch.batch_fp != batch_fp {
            return Err(AppError::Invalid(batch_stale_message()));
        }
        if pending
            .iter()
            .any(|id| !batch.items.iter().any(|i| i.id == **id))
        {
            return Err(AppError::Invalid(stale_message()));
        }

        // Todo se comprueba antes de escribir; la escritura es una sola transacción.
        struct Prepared {
            id: String,
            rule_id: String,
            plan: crate::suggest::AcceptPlan,
        }
        let mut reserved: HashSet<String> = HashSet::new();
        let mut prepared: Vec<Prepared> = Vec::new();
        for id in &pending {
            let Some(item) = batch.items.iter().find(|i| i.id == **id) else {
                return Err(AppError::Invalid(stale_message()));
            };
            let fail = |code: &'static str, message: String| LearnedAcceptOutcome {
                applied: false,
                rules: Vec::new(),
                activated: 0,
                already_active: 0,
                failed: Some(LearnedAcceptFailure {
                    id: Some(item.id.clone()),
                    name: Some(item.name.clone()),
                    code,
                    message,
                }),
            };
            let mut plan = match self.accept_plan(&all, id, &item.shown, None, None, None) {
                Ok(plan) => plan,
                Err(error) => return Ok(fail("not_valid", error.to_string())),
            };
            if let Err(error) = check(&plan.action_kind, &plan.conditions, false) {
                return Ok(fail("invalid_rule", error.to_string()));
            }
            // El rodaje ya lo pone `accept_plan` a toda regla de origen aprendido (D178).
            plan.origin["batch"] = json!(true);
            let rule_id = self.unique_rule_id_avoiding(&plan.name, &reserved)?;
            reserved.insert(rule_id.clone());
            prepared.push(Prepared {
                id: (*id).to_owned(),
                rule_id,
                plan,
            });
        }
        let rules: Vec<BatchRule<'_>> = prepared
            .iter()
            .map(|p| BatchRule {
                fingerprint: &p.id,
                new_id: &p.rule_id,
                action_kind: &p.plan.action_kind,
                conditions: &p.plan.conditions,
                name: Some(&p.plan.name),
                origin: &p.plan.origin,
                evidence: &p.plan.evidence,
            })
            .collect();
        let detail = format!(
            "{} reglas aprendidas aceptadas y activadas en lote (rodaje de {RODAJE_SENDS} envíos cada una)",
            prepared.len()
        );
        let applied = match self.store.accept_and_activate_batch(&rules, &detail) {
            Ok(applied) => applied,
            Err((index, error)) => {
                // Sin índice falló la transacción misma, no una regla.
                let item = index.map(|i| &prepared[i]);
                let code = match (&error, item) {
                    (_, None) => "batch_write_failed",
                    (CoreError::IdempotencyConflict(_), _) => "rule_exists",
                    (CoreError::RuleArchived(_), _) => "rule_archived",
                    _ => "write_failed",
                };
                return Ok(LearnedAcceptOutcome {
                    applied: false,
                    rules: Vec::new(),
                    activated: 0,
                    already_active: 0,
                    failed: Some(LearnedAcceptFailure {
                        id: item.map(|i| i.id.clone()),
                        name: item.map(|i| i.plan.name.clone()),
                        code,
                        message: error.to_string(),
                    }),
                });
            }
        };
        for (item, applied) in prepared.iter().zip(applied) {
            done.push(LearnedAcceptedRule {
                id: item.id.clone(),
                rule_id: applied.rule.id.clone(),
                name: item.plan.name.clone(),
                status: if applied.activated || applied.created {
                    "activated"
                } else {
                    "already_active"
                },
                // Todo quedó activado y confirmado: un fallo al leer el contador no lo convierte en
                // error, solo se omite el dato.
                rodaje_left: self.store.rodaje_left(&applied.rule.id).ok().flatten(),
            });
        }
        // En el orden en que la persona las marcó.
        done.sort_by_key(|r| wanted.iter().position(|id| *id == r.id));
        Ok(outcome(done))
    }

    /// «Desactivar las aprendidas» (D174): desactiva en una transacción todas las reglas de origen
    /// `learned` que estén activas, con una auditoría por regla y una entrada agregada. Idempotente:
    /// sin reglas activas de ese origen no escribe nada. No toca las reglas de otro origen.
    pub fn deactivate_learned_rules(&mut self) -> AppResult<LearnedDeactivateOutcome> {
        let off = self.store.deactivate_learned_rules()?;
        let learned_total = self
            .store
            .rule_metas()?
            .values()
            .filter(|m| m.origin.as_ref().is_some_and(is_learned_origin))
            .count();
        Ok(LearnedDeactivateOutcome {
            deactivated: off.into_iter().map(|r| r.id).collect(),
            learned_total,
        })
    }
}

fn outcome(rules: Vec<LearnedAcceptedRule>) -> LearnedAcceptOutcome {
    let activated = rules.iter().filter(|r| r.status == "activated").count();
    LearnedAcceptOutcome {
        applied: true,
        already_active: rules.len() - activated,
        activated,
        rules,
        failed: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::suggest::{Evidence, Reappearance};

    fn candidate(fingerprint: &str, reappeared: Option<Reappearance>) -> Candidate {
        Candidate {
            fingerprint: fingerprint.into(),
            shown: format!("shown-{fingerprint}"),
            action_kind: SEND_DRAFT.into(),
            name: format!("Responder a {fingerprint}"),
            conditions: json!({ "category": "solicitud_factura", "sender": format!("{fingerprint}@alfa.test") }),
            subject: format!("{fingerprint}@alfa.test"),
            category: Some("solicitud_factura".into()),
            evidence: Evidence {
                count: 3,
                threshold: 3,
                earlier_breaks: 0,
                last_at: 0,
                task_ids: Vec::new(),
                amounts: Vec::new(),
                limit: None,
                needs_limit: false,
                origin: "learned",
                learned: 3,
                addresses: 1,
                approved: 0,
                seen: 0,
                amount_min: None,
                amount_second: None,
                limit_outlier: false,
                reappeared,
            },
            overlaps: Vec::new(),
        }
    }

    #[test]
    fn a_reappeared_discarded_suggestion_is_left_out_of_the_batch() {
        let reappeared = Reappearance {
            at_discard: 3,
            new_since: 6,
            needed_new: 6,
        };
        let all = [candidate("ana", None), candidate("luis", Some(reappeared))];
        let batch = batch_of(&all);
        assert_eq!(
            batch
                .items
                .iter()
                .map(|i| i.id.as_str())
                .collect::<Vec<_>>(),
            ["ana"]
        );
        // La huella de la tanda no cubre a la excluida: solo se acepta con su tarjeta.
        assert_ne!(batch.batch_fp, batch_of(&all[1..]).batch_fp);
        assert_eq!(batch.batch_fp, batch_of(&all[..1]).batch_fp);
    }
}
