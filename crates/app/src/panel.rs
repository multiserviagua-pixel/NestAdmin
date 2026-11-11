//! Panel de reglas (D124, D126, D128, D129): nombre e identificador, archivado, validación
//! avanzada sin remitente, evidencia de cada evaluación, señales de aprendizaje, contadores
//! y simulación de solo lectura.

use std::collections::{BTreeMap, HashMap, HashSet};

use nestadmin_core::{
    is_learned_origin, CoreError, EvaluationRecord, Rule, RuleStats, SignalRecord, Task,
};
use serde::Serialize;
use serde_json::{json, Value};

use crate::rules::{
    analyze, evaluate_result_in_run, evaluate_with, merge_before, merge_options, merge_plan,
    merge_plan_in_runs, rules_fingerprint, Evaluation, FindingKind, Marks, MergeError, MergeOption,
    MergePlan, PlanRule, ReasonCode, RuleDecision, RuleFinding,
};
use crate::{
    action_kind, check_rule, check_rule_id, check_rule_inner, App, AppError, AppResult, EMAIL,
    INVOICE, REGISTER_INVOICE, SEND_DRAFT, SEND_SALES_DOCUMENT,
};

/// Número de tareas que mira la simulación si no se indica otro.
pub const DEFAULT_SIMULATION_LIMIT: usize = 200;
/// Máximo que admite la simulación.
pub const MAX_SIMULATION_LIMIT: usize = 1000;
/// Identificador de la regla hipotética: no puede coincidir con uno real (`check_rule_id`).
const SIMULATED: &str = "(simulada)";
const MAX_NAME_CHARS: usize = 80;
const MAX_SLUG_CHARS: usize = 40;

/// Datos de una regla nueva. El identificador puede faltar: entonces se genera a partir del
/// nombre (D124). `allow_any_sender` es la validación avanzada de una regla de envío sin
/// remitente (D126) y no tiene valor por defecto: quien crea decide cada vez.
#[derive(Debug, Clone)]
pub struct NewRule<'a> {
    pub id: Option<&'a str>,
    pub name: Option<&'a str>,
    pub action_kind: &'a str,
    pub conditions: &'a Value,
    pub allow_any_sender: bool,
}

/// Una regla con lo que el panel muestra de ella. `rule` se serializa en el mismo nivel, de
/// modo que la vista es un `Rule` con campos añadidos.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RuleView {
    #[serde(flatten)]
    pub rule: Rule,
    /// Nombre visible; si la regla no tiene, su identificador.
    pub name: String,
    pub archived_at: Option<i64>,
    pub any_sender_ack_at: Option<i64>,
    /// De dónde sale la regla: al unificar, las reglas de origen y la opción elegida (D131).
    pub origin: Option<Value>,
    pub stats: RuleStats,
}

/// Qué haría la regla hipotética con una tarea.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SimulationOutcome {
    /// La regla la automatizaría.
    Automate,
    /// La regla coincide pero la tarea iría a revisión (ver `reason_code`).
    Review,
    /// La regla coincide, pero otra regla activa también: iría a revisión.
    Overlap,
    /// La regla no coincide con la tarea.
    NoMatch,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SimulationItem {
    pub task_id: i64,
    /// Nombre del documento o asunto del correo, tal como consta en el resultado.
    pub label: Option<String>,
    pub outcome: SimulationOutcome,
    pub reason_code: Option<ReasonCode>,
    pub explanation: String,
    /// Otras reglas activas que también coinciden.
    pub overlapping_rules: Vec<String>,
}

/// Regla activa que dejaría de actuar en estas tareas por coincidencia múltiple (D17).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BlockedRule {
    pub rule_id: String,
    pub task_ids: Vec<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Simulation {
    pub action_kind: String,
    pub limit: usize,
    /// Tareas del tipo que se miraron (las últimas, con resultado).
    pub considered: usize,
    /// Había más tareas del tipo que el límite: el resto no se miró.
    pub truncated: bool,
    /// Tareas del tipo dentro del límite que no tienen resultado (fallidas o sin procesar).
    pub skipped_without_result: usize,
    /// Facturas sin la comprobación local de moneda (tareas anteriores a D127): no se vuelve
    /// a leer su texto y, para una regla de registro que coincida, van a revisión.
    pub without_currency_check: usize,
    /// Correos sin la comprobación de señales de correo automático (tareas anteriores a D157, o
    /// un proveedor sin cabeceras): con cualquier regla de envío irían a revisión (D158).
    pub without_signal_check: usize,
    pub would_automate: usize,
    /// Tareas que coinciden pero irían a revisión, por código de motivo.
    pub would_review: BTreeMap<String, usize>,
    pub overlaps: usize,
    pub no_match: usize,
    pub would_block_other: Vec<BlockedRule>,
    /// Envíos de rodaje que le quedan a la regla que se prueba (`exclude_rule_id`) si es una regla
    /// aprendida aceptada en lote (D174), o `None`. La simulación no modela el rodaje: las
    /// `would_automate` cuentan lo que la regla haría sin él; con `Some(n)` las `n` primeras (más
    /// las de las direcciones que falten, en una regla por dominio) irían a revisión.
    pub rodaje_left: Option<i64>,
    /// Contradicciones que la regla crearía con las demás al activarse, con su explicación
    /// (D130). Una regla nueva sin guardar aparece como `(simulada)`.
    pub findings: Vec<RuleFinding>,
    pub items: Vec<SimulationItem>,
    /// Lo mismo para las facturas que solo se leyeron en la primera lectura (D133), aparte: no
    /// desplazan a las tareas reales del tope y no se listan por tarea (no son visibles).
    pub from_read: Option<Box<Simulation>>,
}

impl Simulation {
    fn empty(action_kind: &str, limit: usize) -> Self {
        Self {
            action_kind: action_kind.into(),
            limit,
            considered: 0,
            truncated: false,
            skipped_without_result: 0,
            without_currency_check: 0,
            without_signal_check: 0,
            would_automate: 0,
            would_review: BTreeMap::new(),
            overlaps: 0,
            no_match: 0,
            would_block_other: Vec::new(),
            rodaje_left: None,
            findings: Vec::new(),
            items: Vec::new(),
            from_read: None,
        }
    }
}

/// Lo que la interfaz necesita para revisar una unificación: las reglas de hoy, las
/// opciones y, cuando ya se puede, el plan completo (D131). Solo lectura.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MergePreview {
    pub action_kind: String,
    pub before: Vec<PlanRule>,
    pub options: Vec<MergeOption>,
    /// Las reglas tienen condiciones distintas y falta elegir.
    pub needs_choice: bool,
    pub chosen: Option<String>,
    /// `None` mientras falte la elección.
    pub plan: Option<MergePlan>,
    pub suggested_name: String,
}

impl App {
    /// Reglas que se evalúan (las no archivadas) y las que tienen la validación avanzada
    /// sin remitente (D126, D128).
    pub(crate) fn rules_for_evaluation(&self) -> AppResult<(Vec<Rule>, HashSet<String>)> {
        let metas = self.store.rule_metas()?;
        let live = |id: &str| metas.get(id).is_none_or(|m| m.archived_at.is_none());
        let rules = self
            .store
            .rules()?
            .into_iter()
            .filter(|r| live(&r.id))
            .collect();
        let any_sender = metas
            .values()
            .filter(|m| m.archived_at.is_none() && m.allow_any_sender_ack_at.is_some())
            .map(|m| m.rule_id.clone())
            .collect();
        Ok((rules, any_sender))
    }

    /// Guarda la evaluación de una tarea sin contenido del correo: ids de regla, estado y
    /// motivos (D129). Repetirla con las mismas reglas y versión no añade nada.
    pub(crate) fn record_evidence(
        &mut self,
        task: &Task,
        result: &Value,
        kind: &str,
        rules: &[Rule],
        any_sender: &HashSet<String>,
        evaluation: &Evaluation,
    ) -> Result<bool, CoreError> {
        let outcome = match evaluation.decision {
            RuleDecision::Automate { .. } => "automated",
            RuleDecision::Review(_) => "review",
        };
        let reason = serde_json::to_value(evaluation.explanation.code)?;
        let candidates: Vec<Value> = evaluation
            .explanation
            .candidates
            .iter()
            .map(|c| {
                json!({
                    "id": c.id,
                    "version": c.version,
                    "active": c.active,
                    "matches": c.matches,
                })
            })
            .collect();
        let checks = json!({ "candidates": candidates });
        let fingerprint = rules_fingerprint(rules, any_sender, kind);
        self.store.record_evaluation(&EvaluationRecord {
            task_id: task.id,
            content_version: result["version"].as_i64().unwrap_or(1),
            action_kind: kind,
            outcome,
            reason_code: reason.as_str().unwrap_or("unknown"),
            rules_fp: &fingerprint,
            checks: &checks,
        })
    }

    /// Guarda una señal. Un fallo se devuelve para que quien llama lo deje en
    /// `result.evidence_error`: la ausencia de datos es explícita y la acción no se ve afectada.
    pub(crate) fn record_signal(
        &mut self,
        kind: &str,
        draft: Option<SignalDraft>,
    ) -> Result<bool, CoreError> {
        let Some(d) = draft else { return Ok(false) };
        // Una aprobación no cuenta hasta que la acción conste ejecutada (D129).
        let state = if kind == "approved" {
            "pending"
        } else {
            "valid"
        };
        self.store.record_signal(&SignalRecord {
            state,
            kind,
            action_kind: d.action_kind,
            subject_key: &d.subject_key,
            edit_state: d.edit_state,
            task_id: d.task_id,
            content_version: d.content_version,
            source_key: d.source_key.as_deref(),
        })
    }

    /// Crea una regla inactiva. Sin identificador, lo genera a partir del nombre con un
    /// sufijo si ya existe (D124). Una regla de envío sin remitente solo se crea con
    /// `allow_any_sender` (D126).
    pub fn create_rule_full(&mut self, new: NewRule<'_>) -> AppResult<RuleView> {
        let name = clean_name(new.name)?;
        let id = match new.id.map(str::trim).filter(|id| !id.is_empty()) {
            Some(id) => {
                check_rule_id(id)?;
                id.to_owned()
            }
            None => {
                let name = name.as_deref().ok_or_else(|| {
                    AppError::Invalid("indique un nombre o un identificador para la regla".into())
                })?;
                self.unique_rule_id(name)?
            }
        };
        if new.allow_any_sender
            && (new.action_kind != SEND_DRAFT
                || ["sender", "sender_domain"]
                    .iter()
                    .any(|k| new.conditions.get(k).is_some()))
        {
            return Err(AppError::Invalid(
                "la validación avanzada solo se aplica a reglas de envío sin remitente".into(),
            ));
        }
        check_rule(new.action_kind, new.conditions, new.allow_any_sender)?;
        let rule = self.store.create_rule_named(
            &id,
            new.action_kind,
            new.conditions,
            name.as_deref(),
            new.allow_any_sender,
        )?;
        self.rule_view(&rule.id)
    }

    pub(crate) fn unique_rule_id(&self, name: &str) -> AppResult<String> {
        self.unique_rule_id_avoiding(name, &HashSet::new())
    }

    /// Como `unique_rule_id`, evitando además los identificadores `reserved` (reglas de un lote que
    /// aún no se han escrito, D173).
    pub(crate) fn unique_rule_id_avoiding(
        &self,
        name: &str,
        reserved: &HashSet<String>,
    ) -> AppResult<String> {
        let base = slugify(name);
        let mut candidate = base.clone();
        let mut n = 1;
        while self.store.rule_id_taken(&candidate)? || reserved.contains(&candidate) {
            n += 1;
            candidate = format!("{base}-{n}");
        }
        Ok(candidate)
    }

    /// Cambia el nombre visible de una regla; vacío lo borra y se vuelve a mostrar el id.
    pub fn rename_rule(&mut self, id: &str, name: Option<&str>) -> AppResult<RuleView> {
        let name = clean_name(name)?;
        self.store.set_rule_name(id, name.as_deref())?;
        self.rule_view(id)
    }

    /// Guarda de una vez las condiciones, el nombre y la validación avanzada de una regla
    /// (D126): una sola transacción, sin guardados parciales. `name`: `None` lo deja como está;
    /// `Some("")` lo borra. `acknowledge_any_sender` da la validación a las condiciones
    /// nuevas y solo vale para una regla de envío sin remitente; sin ella, una regla de envío
    /// sin remitente se rechaza.
    pub fn update_rule_with_ack(
        &mut self,
        id: &str,
        name: Option<&str>,
        conditions: &Value,
        acknowledge_any_sender: bool,
    ) -> AppResult<RuleView> {
        let current = self
            .store
            .rule(id)?
            .ok_or_else(|| AppError::Invalid(format!("regla {id} inexistente")))?;
        let meta = self.store.rule_meta(id)?;
        if meta.as_ref().is_some_and(|m| m.archived_at.is_some()) {
            return Err(CoreError::RuleArchived(id.to_owned()).into());
        }
        if acknowledge_any_sender
            && (current.action_kind != SEND_DRAFT
                || ["sender", "sender_domain"]
                    .iter()
                    .any(|k| conditions.get(k).is_some()))
        {
            return Err(AppError::Invalid(
                "la validación avanzada solo se aplica a reglas de envío sin remitente".into(),
            ));
        }
        check_rule(&current.action_kind, conditions, acknowledge_any_sender)?;
        let name_change = match name {
            None => None,
            Some(raw) => {
                let cleaned = clean_name(Some(raw))?;
                let stored = meta.and_then(|m| m.name);
                (cleaned != stored).then_some(cleaned)
            }
        };
        self.store.update_rule_full(
            id,
            conditions,
            name_change.as_ref().map(|n| n.as_deref()),
            acknowledge_any_sender,
        )?;
        self.rule_view(id)
    }

    /// Valida o retira la opción avanzada de una regla de envío sin remitente (D126). Queda
    /// en el historial con su motivo. Sin la marca, la regla no automatiza.
    pub fn set_rule_any_sender(&mut self, id: &str, acknowledge: bool) -> AppResult<RuleView> {
        let rule = self
            .store
            .rule(id)?
            .ok_or_else(|| AppError::Invalid(format!("regla {id} inexistente")))?;
        if acknowledge {
            if rule.action_kind != SEND_DRAFT {
                return Err(AppError::Invalid(
                    "la opción avanzada solo existe en reglas de envío".into(),
                ));
            }
            if ["sender", "sender_domain"]
                .iter()
                .any(|k| rule.conditions.get(k).is_some())
            {
                return Err(AppError::Invalid(
                    "la regla ya indica remitente; no necesita la opción avanzada".into(),
                ));
            }
            check_rule(&rule.action_kind, &rule.conditions, true)?;
        }
        let reason = if acknowledge {
            "validada la opción avanzada: la regla contesta a cualquier remitente de la categoría"
        } else {
            "retirada la opción avanzada: la regla vuelve a exigir remitente"
        };
        self.store.set_any_sender_ack(id, acknowledge, reason)?;
        self.rule_view(id)
    }

    /// Archiva una regla: queda inactiva y fuera de la lista, y su historial se conserva
    /// (D128). No se activa ni se edita hasta desarchivarla.
    pub fn archive_rule(&mut self, id: &str) -> AppResult<RuleView> {
        self.store.archive_rule(id)?;
        self.rule_view(id)
    }

    /// Devuelve una regla archivada, inactiva.
    pub fn unarchive_rule(&mut self, id: &str) -> AppResult<RuleView> {
        self.store.unarchive_rule(id)?;
        self.rule_view(id)
    }

    /// Contadores de cada regla: aplicada, última vez, pasó a revisión y sin clasificar.
    pub fn rule_stats(&self) -> AppResult<Vec<RuleStats>> {
        Ok(self.store.rule_stats()?)
    }

    /// Todas las reglas (también las archivadas) con nombre, archivado, validación avanzada
    /// y contadores.
    pub fn rule_views(&self) -> AppResult<Vec<RuleView>> {
        let metas = self.store.rule_metas()?;
        let mut stats: HashMap<String, RuleStats> = self
            .store
            .rule_stats()?
            .into_iter()
            .map(|s| (s.rule_id.clone(), s))
            .collect();
        Ok(self
            .store
            .rules()?
            .into_iter()
            .map(|rule| {
                let meta = metas.get(&rule.id);
                RuleView {
                    name: meta
                        .and_then(|m| m.name.clone())
                        .unwrap_or_else(|| rule.id.clone()),
                    archived_at: meta.and_then(|m| m.archived_at),
                    any_sender_ack_at: meta.and_then(|m| m.allow_any_sender_ack_at),
                    origin: meta.and_then(|m| m.origin.clone()),
                    stats: stats.remove(&rule.id).unwrap_or(RuleStats {
                        rule_id: rule.id.clone(),
                        applied: 0,
                        last_applied_at: None,
                        reviewed: 0,
                        unclassified: 0,
                    }),
                    rule,
                }
            })
            .collect())
    }

    pub(crate) fn rule_view(&self, id: &str) -> AppResult<RuleView> {
        self.rule_views()?
            .into_iter()
            .find(|v| v.rule.id == id)
            .ok_or_else(|| AppError::Invalid(format!("regla {id} inexistente")))
    }

    /// Simula una regla hipotética, activa, contra las últimas `limit` tareas guardadas del
    /// tipo (200 por defecto) y las reglas existentes. Solo lectura: no escribe nada, no
    /// crea acciones ni llama a la IA. Dice cuántas se automatizarían, cuántas irían a
    /// revisión y por qué, cuáles solaparían con otra regla y qué reglas activas dejarían
    /// de actuar por coincidencia múltiple (D17).
    ///
    /// `assume_any_sender_ack` es solo una hipótesis para una regla de envío sin remitente:
    /// simula qué haría si tuviera la validación avanzada (D126). Nunca escribe la marca; sin
    /// ella la regla sin remitente no automatiza y la simulación lo dice.
    ///
    /// `exclude_rule_id` quita esa regla de las existentes para la prueba: sirve para probar
    /// una regla ya guardada (o editada) sin que coincida consigo misma.
    pub fn simulate_rule(
        &self,
        action_kind: &str,
        conditions: &Value,
        limit: Option<usize>,
        assume_any_sender_ack: bool,
        exclude_rule_id: Option<&str>,
    ) -> AppResult<Simulation> {
        let limit = limit.unwrap_or(DEFAULT_SIMULATION_LIMIT);
        if limit == 0 || limit > MAX_SIMULATION_LIMIT {
            return Err(AppError::Invalid(format!(
                "el límite de la simulación debe estar entre 1 y {MAX_SIMULATION_LIMIT}"
            )));
        }
        // Se valida como una regla real; el remitente ausente se simula tal cual (sin la
        // validación avanzada), no se rechaza. Una regla antigua con una categoría de solo
        // revisión (D177) tampoco: se simula y todo lo que cubre va a revisión.
        check_rule_inner(action_kind, conditions, true, true)?;
        let task_kind = match action_kind {
            REGISTER_INVOICE => INVOICE,
            SEND_SALES_DOCUMENT => SEND_SALES_DOCUMENT,
            _ => EMAIL,
        };
        let (mut rules, mut any_sender) = self.rules_for_evaluation()?;
        if let Some(excluded) = exclude_rule_id {
            rules.retain(|r| r.id != excluded);
            any_sender.remove(excluded);
        }
        if assume_any_sender_ack {
            any_sender.insert(SIMULATED.to_owned());
        }
        let mut with_hypothetical = rules.clone();
        with_hypothetical.push(Rule {
            id: SIMULATED.into(),
            version: 1,
            action_kind: action_kind.into(),
            conditions: conditions.clone(),
            active: true,
            created_at: 0,
        });
        let of_kind: Vec<Task> = self
            .store
            .all_tasks()?
            .into_iter()
            // Un correo de la primera lectura no tiene remitente ni borrador: no dice nada sobre
            // una regla de envío (D133). Sus facturas se simulan aparte, sin desplazar las reales.
            // Tampoco un informativo, un rebote o una redacción a mano (D159): no son decisiones
            // sobre un borrador de la IA y no dirían nada de una regla de envío.
            .filter(|t| {
                t.kind == task_kind
                    && !(t.kind == EMAIL
                        && (crate::task_is_initial_read(t) || crate::task_is_triaged_or_redraft(t)))
            })
            .collect();
        let (real, read): (Vec<Task>, Vec<Task>) = of_kind
            .into_iter()
            .partition(|t| !crate::task_is_initial_read(t));
        let truncated = real.len() > limit;
        let rodaje_left = match exclude_rule_id {
            Some(id) => self.store.rodaje_left(id)?,
            None => None,
        };
        let mut sim = Simulation {
            rodaje_left,
            action_kind: action_kind.into(),
            limit,
            considered: 0,
            truncated,
            skipped_without_result: 0,
            without_currency_check: 0,
            without_signal_check: 0,
            would_automate: 0,
            would_review: BTreeMap::new(),
            overlaps: 0,
            no_match: 0,
            would_block_other: Vec::new(),
            findings: Vec::new(),
            items: Vec::new(),
            from_read: None,
        };
        let run = |tasks: Vec<Task>,
                   sim: &mut Simulation,
                   blocked: &mut BTreeMap<String, Vec<i64>>| {
            for task in tasks.into_iter().take(limit) {
                let Some(result) = task.result.as_ref().filter(|r| r.is_object()) else {
                    sim.skipped_without_result += 1;
                    continue;
                };
                sim.considered += 1;
                if action_kind == REGISTER_INVOICE
                    && crate::rules::foreign_currencies(result).is_none()
                {
                    sim.without_currency_check += 1;
                }
                if action_kind == SEND_DRAFT && !result["message"]["signals"].is_array() {
                    sim.without_signal_check += 1;
                }
                let label = if task_kind == INVOICE {
                    result["document"].as_str()
                } else if task_kind == SEND_SALES_DOCUMENT {
                    result["document"]["number"].as_str()
                } else {
                    result["message"]["subject"].as_str()
                }
                .map(str::to_owned);
                // Una respuesta con un paso omitido siempre se revisa (D150): no cuenta como
                // automatizable. Ante un fallo al consultarlo se supone omitido (cierra en falso).
                let waived = self.store.has_waived_prerequisite(task.id).unwrap_or(true);
                let before =
                    evaluate_result_in_run(&rules, &any_sender, action_kind, result, waived);
                let after = evaluate_result_in_run(
                    &with_hypothetical,
                    &any_sender,
                    action_kind,
                    result,
                    waived,
                );
                // Si la regla coincide con los datos se mira sin los atajos previos a las reglas
                // (importe dudoso, factura separada), que mandan la tarea a revisión aunque
                // coincida cualquier regla.
                let raw = evaluate_with(&with_hypothetical, &any_sender, action_kind, result);
                let mine = raw
                    .explanation
                    .candidates
                    .iter()
                    .find(|c| c.id == SIMULATED);
                let code = after.explanation.code;
                let shortcut = matches!(
                    code,
                    ReasonCode::AmountReview
                        | ReasonCode::SplitInvoice
                        | ReasonCode::PipelineStepWaived
                );
                let item = match mine {
                    None => review_item(sim, task.id, label, &after),
                    Some(check) if check.matches && shortcut => {
                        review_item(sim, task.id, label, &after)
                    }
                    Some(check) if check.matches => match (&after.decision, code) {
                        (RuleDecision::Automate { rule_id, .. }, _) if rule_id == SIMULATED => {
                            sim.would_automate += 1;
                            SimulationItem {
                                task_id: task.id,
                                label,
                                outcome: SimulationOutcome::Automate,
                                reason_code: Some(ReasonCode::Automated),
                                explanation: "la regla la automatizaría".into(),
                                overlapping_rules: Vec::new(),
                            }
                        }
                        (_, ReasonCode::SeveralRulesMatch) => {
                            let others: Vec<String> = after.explanation.facts["rule_ids"]
                                .as_array()
                                .into_iter()
                                .flatten()
                                .filter_map(Value::as_str)
                                .filter(|id| *id != SIMULATED)
                                .map(str::to_owned)
                                .collect();
                            if let RuleDecision::Automate { rule_id, .. } = &before.decision {
                                if others.contains(rule_id) {
                                    blocked.entry(rule_id.clone()).or_default().push(task.id);
                                }
                            }
                            sim.overlaps += 1;
                            SimulationItem {
                            task_id: task.id,
                            label,
                            outcome: SimulationOutcome::Overlap,
                            reason_code: Some(ReasonCode::SeveralRulesMatch),
                            explanation: format!(
                                "también coincide {}; con dos reglas la decisión no es inequívoca y iría a revisión",
                                others.join(", ")
                            ),
                            overlapping_rules: others,
                        }
                        }
                        _ => review_item(sim, task.id, label, &after),
                    },
                    Some(check) if check.sender_required => {
                        *sim.would_review
                            .entry(reason_key(ReasonCode::SenderRequired))
                            .or_default() += 1;
                        SimulationItem {
                            task_id: task.id,
                            label,
                            outcome: SimulationOutcome::Review,
                            reason_code: Some(ReasonCode::SenderRequired),
                            explanation: check.mismatch.clone().unwrap_or_default(),
                            overlapping_rules: Vec::new(),
                        }
                    }
                    Some(check) => {
                        sim.no_match += 1;
                        SimulationItem {
                            task_id: task.id,
                            label,
                            outcome: SimulationOutcome::NoMatch,
                            reason_code: None,
                            explanation: check.mismatch.clone().unwrap_or_default(),
                            overlapping_rules: Vec::new(),
                        }
                    }
                };
                sim.items.push(item);
            }
        };
        let mut blocked: BTreeMap<String, Vec<i64>> = BTreeMap::new();
        run(real, &mut sim, &mut blocked);
        sim.would_block_other = blocked
            .into_iter()
            .map(|(rule_id, task_ids)| BlockedRule { rule_id, task_ids })
            .collect();
        sim.findings = activation_findings(
            &with_hypothetical,
            &any_sender,
            exclude_rule_id.and_then(|id| self.store.rule(id).ok().flatten()),
        );
        if !read.is_empty() {
            let mut from_read = Simulation {
                truncated: read.len() > limit,
                findings: Vec::new(),
                would_block_other: Vec::new(),
                ..Simulation::empty(action_kind, limit)
            };
            let mut unused = BTreeMap::new();
            run(read, &mut from_read, &mut unused);
            from_read.items.clear();
            sim.from_read = Some(Box::new(from_read));
        }
        Ok(sim)
    }

    /// Reglas y las marcas que necesita el análisis (D130): archivadas y con la validación
    /// avanzada.
    pub(crate) fn analysis_inputs(&self) -> AppResult<(Vec<Rule>, Marks)> {
        let metas = self.store.rule_metas()?;
        let marks = Marks {
            any_sender: metas
                .values()
                .filter(|m| m.archived_at.is_none() && m.allow_any_sender_ack_at.is_some())
                .map(|m| m.rule_id.clone())
                .collect(),
            archived: metas
                .values()
                .filter(|m| m.archived_at.is_some())
                .map(|m| m.rule_id.clone())
                .collect(),
        };
        Ok((self.store.rules()?, marks))
    }

    /// Contradicciones entre las reglas guardadas, de más a menos graves (D130). Solo
    /// lectura: no llama a la IA ni cambia nada. Cada choque lleva el recuento de tareas
    /// que ya coincidieron con las dos reglas, según `rule_evaluations` (la última
    /// evaluación de cada tarea).
    pub fn rule_findings(&self) -> AppResult<Vec<RuleFinding>> {
        let (rules, marks) = self.analysis_inputs()?;
        let mut findings = analyze(&rules, &marks);
        self.fill_affected(&mut findings)?;
        Ok(findings)
    }

    /// Recuento de tareas afectadas de cada choque: la última evaluación de cada tarea, solo
    /// si las dos reglas estaban activas y coincidieron, con las condiciones que tienen hoy
    /// (las evaluaciones de versiones con otras condiciones ya editadas no cuentan).
    fn fill_affected(&self, findings: &mut [RuleFinding]) -> AppResult<()> {
        let mut latest: HashMap<i64, Value> = HashMap::new();
        for row in self.store.rule_evaluations()? {
            latest.insert(row.task_id, row.checks);
        }
        for finding in findings.iter_mut() {
            if !matches!(finding.kind, FindingKind::Duplicate | FindingKind::Overlap) {
                continue;
            }
            // Versiones de cada regla cuyas condiciones son las vigentes.
            let mut current_versions: Vec<(String, HashSet<i64>)> = Vec::new();
            for fr in &finding.rules {
                let history = self.store.rule_history(&fr.id)?;
                let now = history.iter().find(|r| r.version == fr.version);
                let versions = history
                    .iter()
                    .filter(|r| now.is_some_and(|n| n.conditions == r.conditions))
                    .map(|r| r.version)
                    .collect();
                current_versions.push((fr.id.clone(), versions));
            }
            let both = |checks: &Value| {
                current_versions.iter().all(|(id, versions)| {
                    checks["candidates"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .any(|c| {
                            c["id"].as_str() == Some(id.as_str())
                                && c["matches"] == json!(true)
                                // El choque solo ocurre si las dos estaban activas.
                                && c["active"] == json!(true)
                                && c["version"].as_i64().is_some_and(|v| versions.contains(&v))
                        })
                })
            };
            finding.affected_tasks = Some(latest.values().filter(|c| both(c)).count());
        }
        Ok(())
    }

    /// Contradicciones que tendría una regla archivada si se restaurara (D128): la regla
    /// vuelve inactiva, así que el choque solo ocurriría al activarla. Solo lectura.
    pub fn preview_restore(&self, id: &str) -> AppResult<Vec<RuleFinding>> {
        let (mut rules, mut marks) = self.analysis_inputs()?;
        if !marks.archived.remove(id) {
            return Err(AppError::Invalid(format!(
                "la regla {id} no está archivada"
            )));
        }
        for rule in rules.iter_mut().filter(|r| r.id == id) {
            rule.active = false;
        }
        let mut findings: Vec<RuleFinding> = analyze(&rules, &marks)
            .into_iter()
            .filter(|f| f.rules.iter().any(|r| r.id == id))
            .collect();
        self.fill_affected(&mut findings)?;
        Ok(findings)
    }

    /// Revisa una unificación sin cambiar nada: las reglas de hoy, las opciones y, con la
    /// elección hecha (o si no hace falta), el plan con su simulación (D131). Sin elección,
    /// `plan` es `None` y `needs_choice` lo dice.
    pub fn preview_merge(&self, ids: &[String], chosen: Option<&str>) -> AppResult<MergePreview> {
        let (rules, marks) = self.analysis_inputs()?;
        let invalid = |e: MergeError| AppError::Invalid(e.to_string());
        let before = merge_before(&rules, &marks, ids).map_err(invalid)?;
        let options = merge_options(&rules, &marks, ids).map_err(invalid)?;
        let tasks: Vec<Task> = self
            .store
            .all_tasks()?
            .into_iter()
            // Lo que solo se leyó (D133) no es una tarea que la persona vea: no entra en el plan.
            // Tampoco un informativo, un rebote o una redacción a mano (D159).
            .filter(|t| !crate::task_is_initial_read(t) && !crate::task_is_triaged_or_redraft(t))
            .collect();
        // Una respuesta con un paso omitido siempre se revisa (D150). Ante un fallo al consultarlo
        // se supone omitida (cierra en falso), como en `finish_task`.
        let step_waived: HashSet<i64> = tasks
            .iter()
            .filter(|t| t.kind == EMAIL)
            .filter(|t| self.store.has_waived_prerequisite(t.id).unwrap_or(true))
            .map(|t| t.id)
            .collect();
        let plan = match merge_plan_in_runs(&rules, &marks, ids, chosen, &tasks, &step_waived) {
            Ok(plan) => Some(plan),
            Err(MergeError::ChoiceRequired(_)) => None,
            Err(e) => return Err(invalid(e)),
        };
        let action_kind = rules
            .iter()
            .find(|r| r.id == before[0].id)
            .map(|r| r.action_kind.clone())
            .unwrap_or_default();
        let metas = self.store.rule_metas()?;
        let names: Vec<String> = before
            .iter()
            .map(|r| {
                metas
                    .get(&r.id)
                    .and_then(|m| m.name.clone())
                    .unwrap_or_else(|| r.id.clone())
            })
            .collect();
        Ok(MergePreview {
            action_kind,
            needs_choice: options.len() > 1,
            chosen: plan.as_ref().map(|p| p.chosen.clone()),
            plan,
            options,
            before,
            suggested_name: suggested_merge_name(&names),
        })
    }

    /// Unifica reglas (D131) en una sola transacción: vuelve a calcular el plan y, si las
    /// reglas cambiaron desde que se revisó (`plan_fingerprint`), no hace nada; archiva las
    /// originales, crea una regla **inactiva** (sin heredar la validación avanzada) que las
    /// cita en su origen y lo audita con ids y versiones. Nunca activa nada.
    pub fn merge_rules(
        &mut self,
        ids: &[String],
        chosen: Option<&str>,
        name: Option<&str>,
        plan_fingerprint: &str,
    ) -> AppResult<RuleView> {
        let (rules, marks) = self.analysis_inputs()?;
        let changed = || AppError::Invalid("las reglas han cambiado; revise de nuevo".into());
        let plan = merge_plan(&rules, &marks, ids, chosen, &[]).map_err(|e| match e {
            // La opción revisada ya no existe: alguien cambió las reglas.
            MergeError::UnknownChoice(_) | MergeError::ChoiceRequired(_) => changed(),
            other => AppError::Invalid(other.to_string()),
        })?;
        if plan.fingerprint != plan_fingerprint {
            return Err(changed());
        }
        check_rule(&plan.action_kind, &plan.after.conditions, true)?;
        let metas = self.store.rule_metas()?;
        let names: Vec<String> = plan
            .before
            .iter()
            .map(|r| {
                metas
                    .get(&r.id)
                    .and_then(|m| m.name.clone())
                    .unwrap_or_else(|| r.id.clone())
            })
            .collect();
        let name = match clean_name(name)? {
            Some(name) => name,
            None => suggested_merge_name(&names),
        };
        let new_id = self.unique_rule_id(&name)?;
        let sources: Vec<(String, i64)> = plan
            .before
            .iter()
            .map(|r| (r.id.clone(), r.version))
            .collect();
        let mut origin = json!({
            "kind": "merge",
            "sources": plan.before.iter().map(|r| json!({
                "id": r.id,
                "version": r.version,
                "active": r.active,
                "any_sender": r.any_sender,
            })).collect::<Vec<_>>(),
            "option": plan.chosen,
        });
        // Si alguna original era aprendida, la unificada sigue siéndolo y conserva el rodaje más
        // exigente (D174): no se pierde el freno de los primeros envíos al unificar.
        if plan.before.iter().any(|r| {
            metas
                .get(&r.id)
                .and_then(|m| m.origin.as_ref())
                .is_some_and(is_learned_origin)
        }) {
            origin["evidence_origin"] = json!("learned");
            let ids: Vec<&str> = plan.before.iter().map(|r| r.id.as_str()).collect();
            if let Some(rodaje) = self.merged_rodaje(&ids, &plan.after.conditions)? {
                origin["rodaje"] = rodaje;
            }
        }
        let rule = self.store.merge_rules(
            &sources,
            &new_id,
            &plan.action_kind,
            &plan.after.conditions,
            Some(&name),
            &origin,
        )?;
        self.rule_view(&rule.id)
    }
}

impl App {
    /// Rodaje de una regla unificada (D174): el `left` más alto de las originales que lo tengan y,
    /// si la unificada es por dominio, la unión de las huellas de direcciones ya cubiertas. Con la
    /// cobertura incompleta, el rodaje no queda en 0. `None` si ninguna original tenía rodaje.
    /// Una original con el origen ilegible aporta el rodaje completo (falla cerrado).
    fn merged_rodaje(&self, ids: &[&str], conditions: &Value) -> AppResult<Option<Value>> {
        let mut left: Option<i64> = None;
        let mut hashes: Vec<String> = Vec::new();
        for id in ids {
            if let Some(state) = self.store.rodaje_state(id)? {
                left = Some(left.map_or(state.left, |l| l.max(state.left)));
                for hash in state.address_hashes {
                    if !hashes.contains(&hash) {
                        hashes.push(hash);
                    }
                }
            }
        }
        let Some(mut left) = left else {
            return Ok(None);
        };
        let min = if conditions.get("sender_domain").is_some() {
            crate::suggest::DOMAIN_MIN_ADDRESSES
        } else {
            0
        };
        if left == 0 && hashes.len() < min {
            left = 1;
        }
        let mut rodaje = json!({ "left": left, "of": nestadmin_core::RODAJE_SENDS });
        if min > 0 {
            rodaje["min_addresses"] = json!(min);
            rodaje["address_hashes"] = json!(hashes);
        }
        Ok(Some(rodaje))
    }
}

/// Nombre propuesto para la regla unificada: «Unificada: A + B», dentro del límite.
fn suggested_merge_name(names: &[String]) -> String {
    let full = format!("Unificada: {}", names.join(" + "));
    full.chars()
        .take(MAX_NAME_CHARS)
        .collect::<String>()
        .trim_end()
        .to_owned()
}

/// Contradicciones que crearía activar la regla hipotética `(simulada)` con las demás. Si la
/// simulación es de una regla ya guardada (`saved`), su id real sustituye al hipotético.
fn activation_findings(
    rules: &[Rule],
    any_sender: &HashSet<String>,
    saved: Option<Rule>,
) -> Vec<RuleFinding> {
    let marks = Marks {
        any_sender: any_sender.clone(),
        archived: HashSet::new(),
    };
    let mut findings: Vec<RuleFinding> = analyze(rules, &marks)
        .into_iter()
        .filter(|f| f.kind != FindingKind::Defective && f.rules.iter().any(|r| r.id == SIMULATED))
        .collect();
    if let Some(saved) = saved {
        for finding in &mut findings {
            finding.key = finding.key.replace(SIMULATED, &saved.id);
            for fr in &mut finding.rules {
                if fr.id == SIMULATED {
                    fr.id = saved.id.clone();
                    fr.version = saved.version;
                }
            }
            replace_id(&mut finding.effect.data, &saved.id);
        }
    }
    findings
}

fn replace_id(value: &mut Value, to: &str) {
    match value {
        Value::String(s) if s == SIMULATED => *s = to.to_owned(),
        Value::Array(items) => items.iter_mut().for_each(|v| replace_id(v, to)),
        Value::Object(map) => map.values_mut().for_each(|v| replace_id(v, to)),
        _ => {}
    }
}

/// Lo que identifica una señal de aprendizaje de esta tarea, o `None` si no es señal:
/// las partes de una factura separada de un documento con varias (D68) no lo son.
///
/// `signal` es la clase de señal (`approved`, `edited` o `rejected`). Tampoco es señal una
/// tarea cuya decisión tomó una regla, ni la aprobación de un reintento de una regla (la
/// señal es lo que las personas deciden por su cuenta). Pero un **rechazo** humano sí lo es
/// aunque la tarea lleve `authorization` de regla (una regla lo intentó, el conector falló y
/// `retry` la devolvió a revisión) o `rule_attempt` (si antes aprobó un reintento): `reject`
/// solo crea la señal desde `AwaitingReview`, así que es una decisión de la persona sobre lo
/// que la regla no llegó a hacer, y corta la serie (D132). Una ejecución por regla (tarea
/// completada) sigue sin ser evidencia ni aprobación.
pub(crate) fn signal_for(task: &Task, result: &Value, signal: &str) -> Option<SignalDraft> {
    // El envío de un documento de venta no enseña qué reglas sugerir: ni su aprobación ni su
    // rechazo dejan señal (D232, D129).
    if task.kind == SEND_SALES_DOCUMENT {
        return None;
    }
    let by_rule = signal != "rejected"
        && (result["authorization"]["type"] == "rule" || result["rule_attempt"]["type"] == "rule");
    if result["part"]["of"].as_u64().is_some_and(|n| n > 1) || by_rule {
        return None;
    }
    // Un correo resuelto por la ruta local, sin respuesta necesaria o redactado a mano no es una
    // decisión sobre un borrador de la IA: no enseña qué reglas sugerir (D159).
    if result.get("triage").is_some()
        || result["needs_reply"].as_bool() == Some(false)
        || result.get("redraft_of").is_some_and(|v| !v.is_null())
    {
        return None;
    }
    let kind = action_kind(task);
    let (subject_key, source_key) = if kind == REGISTER_INVOICE {
        let supplier = result["fields"]["supplier"].as_str().unwrap_or("");
        (
            supplier.trim().to_ascii_lowercase(),
            result["origin"]["sha256"].as_str().map(str::to_owned),
        )
    } else {
        let category = result["category"].as_str().unwrap_or("");
        let sender = result["message"]["from"].as_str().unwrap_or("");
        (
            format!("{}|{}", category.trim(), sender.trim().to_ascii_lowercase()),
            None,
        )
    };
    let version = result["version"].as_i64().unwrap_or(1);
    // `confirm_not_sent` también sube la versión: sin la marca, una versión mayor que 1
    // no permite saber si alguien editó el texto.
    let edit_state = match result["edited"].as_bool() {
        Some(true) => "edited",
        Some(false) => "unedited",
        None if version > 1 => "unknown",
        None => "unedited",
    };
    Some(SignalDraft {
        task_id: task.id,
        action_kind: kind,
        subject_key,
        edit_state,
        content_version: version,
        source_key,
    })
}

/// Señal de aprendizaje por guardar (D129).
#[derive(Debug, Clone)]
pub(crate) struct SignalDraft {
    task_id: i64,
    action_kind: &'static str,
    subject_key: String,
    edit_state: &'static str,
    content_version: i64,
    source_key: Option<String>,
}

fn reason_key(code: ReasonCode) -> String {
    serde_json::to_value(code)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

/// Tarea que iría a revisión por el motivo de `after` (incidencias, destinatario, atajos).
fn review_item(
    sim: &mut Simulation,
    task_id: i64,
    label: Option<String>,
    after: &Evaluation,
) -> SimulationItem {
    let code = after.explanation.code;
    *sim.would_review.entry(reason_key(code)).or_default() += 1;
    SimulationItem {
        task_id,
        label,
        outcome: SimulationOutcome::Review,
        reason_code: Some(code),
        explanation: after.explanation.summary(),
        overlapping_rules: Vec::new(),
    }
}

/// Nombre visible: recortado, sin controles, de 80 caracteres como mucho. Vacío es «sin nombre».
fn clean_name(name: Option<&str>) -> AppResult<Option<String>> {
    let Some(name) = name.map(str::trim).filter(|n| !n.is_empty()) else {
        return Ok(None);
    };
    if name.chars().any(char::is_control) {
        return Err(AppError::Invalid(
            "el nombre de la regla no admite caracteres de control".into(),
        ));
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(AppError::Invalid(format!(
            "el nombre de la regla admite como mucho {MAX_NAME_CHARS} caracteres"
        )));
    }
    Ok(Some(name.to_owned()))
}

/// Identificador legible a partir de un nombre: minúsculas, sin acentos, cifras y guiones.
pub(crate) fn slugify(name: &str) -> String {
    let mut slug = String::new();
    for c in name.chars().flat_map(char::to_lowercase) {
        let c = match c {
            'á' | 'à' | 'ä' | 'â' => 'a',
            'é' | 'è' | 'ë' | 'ê' => 'e',
            'í' | 'ì' | 'ï' | 'î' => 'i',
            'ó' | 'ò' | 'ö' | 'ô' => 'o',
            'ú' | 'ù' | 'ü' | 'û' => 'u',
            'ñ' => 'n',
            'ç' => 'c',
            other => other,
        };
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            slug.push(c);
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug: String = slug
        .trim_matches('-')
        .chars()
        .take(MAX_SLUG_CHARS)
        .collect();
    let slug = slug.trim_matches('-').to_owned();
    if slug.is_empty() {
        "regla".into()
    } else {
        slug
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_are_valid_rule_ids() {
        for (name, expected) in [
            ("Facturas de Alfa SL", "facturas-de-alfa-sl"),
            ("  Pedidos — Año 2026 ", "pedidos-ano-2026"),
            ("¡¡¡", "regla"),
            ("Ñandú y Cía.", "nandu-y-cia"),
        ] {
            assert_eq!(slugify(name), expected);
            assert!(check_rule_id(&slugify(name)).is_ok());
        }
        assert!(slugify(&"a".repeat(100)).len() <= MAX_SLUG_CHARS);
        assert!(check_rule_id(SIMULATED).is_err(), "no choca con un id real");
    }

    fn task(kind: &str) -> Task {
        Task {
            id: 9,
            kind: kind.into(),
            input_ref: "{}".into(),
            state: nestadmin_core::TaskState::AwaitingReview,
            attempts: 1,
            result: None,
            error: None,
            created_at: 0,
            updated_at: 0,
        }
    }

    #[test]
    fn signals_normalize_the_subject_and_classify_the_edit_state() {
        let invoice = json!({
            "version": 1,
            "fields": { "supplier": "  Alfa SL " },
            "origin": { "sha256": "abc" },
        });
        let s = signal_for(&task(INVOICE), &invoice, "approved").unwrap();
        assert_eq!(
            (s.action_kind, s.subject_key.as_str(), s.edit_state),
            (REGISTER_INVOICE, "alfa sl", "unedited")
        );
        assert_eq!(s.source_key.as_deref(), Some("abc"));

        let mut mail = json!({
            "version": 1,
            "category": "solicitud_factura",
            "message": { "from": "Pedidos@Alfa.TEST" },
        });
        let s = signal_for(&task(EMAIL), &mail, "approved").unwrap();
        assert_eq!(s.subject_key, "solicitud_factura|pedidos@alfa.test");
        assert_eq!((s.edit_state, s.source_key), ("unedited", None));
        // Una versión mayor sin la marca (`confirm_not_sent` también la sube): no se sabe.
        mail["version"] = json!(2);
        assert_eq!(
            signal_for(&task(EMAIL), &mail, "approved")
                .unwrap()
                .edit_state,
            "unknown"
        );
        mail["edited"] = json!(true);
        assert_eq!(
            signal_for(&task(EMAIL), &mail, "approved")
                .unwrap()
                .edit_state,
            "edited"
        );
        mail["edited"] = json!(false);
        assert_eq!(
            signal_for(&task(EMAIL), &mail, "approved")
                .unwrap()
                .edit_state,
            "unedited"
        );
        // Sin remitente ni proveedor la clave queda vacía, nunca inventada.
        let empty = signal_for(&task(EMAIL), &json!({}), "approved").unwrap();
        assert_eq!(empty.subject_key, "|");
        assert_eq!(
            signal_for(&task(INVOICE), &json!({}), "approved")
                .unwrap()
                .subject_key,
            ""
        );
    }

    #[test]
    fn split_invoices_and_rule_executions_are_not_signals() {
        let mut result = json!({ "version": 1, "fields": { "supplier": "Alfa SL" } });
        result["part"] = json!({ "k": 2, "of": 3 });
        assert!(signal_for(&task(INVOICE), &result, "approved").is_none());
        result["part"] = json!({ "k": 1, "of": 1 });
        assert!(signal_for(&task(INVOICE), &result, "approved").is_some());
        result["authorization"] = json!({ "type": "rule", "rule_id": "a", "rule_version": 1 });
        assert!(signal_for(&task(INVOICE), &result, "approved").is_none());
        // Tras aprobar un intento de regla que no se ejecutó, `authorization` es la
        // aprobación y el intento consta en `rule_attempt`: sigue sin ser señal.
        let attempt = result["authorization"].take();
        result["authorization"] = json!({ "type": "approval" });
        result["rule_attempt"] = attempt;
        assert!(signal_for(&task(INVOICE), &result, "approved").is_none());
        // Pero rechazarla sí es una decisión de la persona y deja señal.
        assert!(signal_for(&task(INVOICE), &result, "rejected").is_some());
        // También con la autorización de regla aún en el resultado (la regla lo intentó, el
        // conector falló y `retry` la devolvió a revisión sin `rule_attempt`).
        result["authorization"] = json!({ "type": "rule", "rule_id": "a", "rule_version": 1 });
        result["rule_attempt"] = Value::Null;
        assert!(signal_for(&task(INVOICE), &result, "rejected").is_some());
        assert!(signal_for(&task(INVOICE), &result, "approved").is_none());
        result["authorization"] = json!({ "type": "approval" });
        result["rule_attempt"] = json!({ "type": "rule", "rule_id": "a", "rule_version": 1 });
        // Una parte de factura separada no lo es ni al rechazarla.
        result["part"] = json!({ "index": 2, "of": 3 });
        assert!(signal_for(&task(INVOICE), &result, "rejected").is_none());
    }

    #[test]
    fn names_are_trimmed_and_bounded() {
        assert_eq!(clean_name(None).unwrap(), None);
        assert_eq!(clean_name(Some("   ")).unwrap(), None);
        assert_eq!(clean_name(Some(" Alfa ")).unwrap().as_deref(), Some("Alfa"));
        assert!(clean_name(Some(&"x".repeat(81))).is_err());
        assert!(clean_name(Some("a\nb")).is_err());
    }
}
