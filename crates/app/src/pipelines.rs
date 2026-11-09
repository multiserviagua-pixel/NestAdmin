//! Plantillas de pipeline en marcha (R25, D149, D150): catálogo cerrado, encaje con un correo
//! nuevo, apertura de la ejecución, vista de bloqueos y «Continuar sin ese paso».
//!
//! **Una plantilla nunca autoriza nada.** Este módulo no importa `Authorization` ni llama a
//! `execute` ni a `approve`: solo decide qué tareas se crean y en qué orden. Terminar un paso
//! solo permite que empiece el siguiente, que pasa por `finish`, `evaluate_result` y su propia
//! regla o revisión (R07). Tampoco toca `GenerateRequest` ni `generate_with_fallback`: lo que se
//! envía a la IA es idéntico con y sin plantilla (D102) y el respaldo no cambia (D101).

use std::collections::{HashMap, HashSet};

use nestadmin_core::{CoreError, NewStep, PipelineRun, PipelineStep, TaskState, TaskTemplate};

use crate::initial_read::day_of;
use nestadmin_simulators::Message;
use serde::Serialize;
use serde_json::{json, Map, Value};

use crate::rules::{is_domain, is_email_address, sender_matches};
use crate::{
    attachment_key, attachment_name, supported_document, App, AppError, AppResult, Task, EMAIL,
    INVOICE,
};

/// Paso de etapa 1: una factura por adjunto admitido del correo, más sus partes (D68).
pub const STEP_ATTACHMENT_INVOICES: &str = "attachment_invoices";
/// Paso de etapa 2: la tarea del propio correo, que es el sujeto de la ejecución.
pub const STEP_REPLY: &str = "reply";
const STAGE_ATTACHMENTS: i64 = 1;
const STAGE_REPLY: i64 = 2;
const REPLY_LABEL: &str = "Responder al correo";
/// Longitud máxima del nombre de una plantilla y de la nota de «Continuar sin ese paso».
const NAME_MAX_CHARS: usize = 80;
const NOTE_MAX_CHARS: usize = 200;
/// Ejecuciones que devuelve `pipeline_overview` como mucho (las más recientes).
const OVERVIEW_RUNS: usize = 500;

fn invalid(text: impl Into<String>) -> AppError {
    AppError::Invalid(text.into())
}

// ---- Validación al guardar ----

/// Comprueba los pasos contra el catálogo cerrado de la versión 1: exactamente
/// `attachment_invoices` y después `reply`, cada uno solo con su `kind`. Cualquier otro tipo, un
/// campo más (por ejemplo, una acción o una automatización) o otro orden se rechaza.
pub fn validate_steps(steps: &Value) -> AppResult<Value> {
    let list = steps
        .as_array()
        .ok_or_else(|| invalid("los pasos de la plantilla deben ser una lista"))?;
    let mut kinds = Vec::new();
    for step in list {
        let object = step
            .as_object()
            .ok_or_else(|| invalid("cada paso de la plantilla debe ser un objeto con su tipo"))?;
        let kind = object
            .get("kind")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("a un paso de la plantilla le falta el tipo"))?;
        if !matches!(kind, STEP_ATTACHMENT_INVOICES | STEP_REPLY) {
            return Err(invalid(format!(
                "tipo de paso desconocido: «{kind}»; solo se admiten «{STEP_ATTACHMENT_INVOICES}» y «{STEP_REPLY}»"
            )));
        }
        if object.len() != 1 {
            return Err(invalid(
                "un paso de la plantilla solo lleva su tipo: no admite acciones ni automatizaciones",
            ));
        }
        kinds.push(kind);
    }
    if kinds != [STEP_ATTACHMENT_INVOICES, STEP_REPLY] {
        return Err(invalid(format!(
            "la plantilla necesita exactamente los pasos «{STEP_ATTACHMENT_INVOICES}» y después «{STEP_REPLY}»"
        )));
    }
    Ok(json!([{ "kind": STEP_ATTACHMENT_INVOICES }, { "kind": STEP_REPLY }]))
}

/// Condición de remitente de una plantilla, ya normalizada (sin mayúsculas ni espacios).
#[derive(Debug, Clone, PartialEq, Eq)]
enum Sender {
    Any,
    Domain(String),
    Address(String),
}

impl Sender {
    /// Más concreta, mayor: dirección, dominio, cualquiera.
    fn rank(&self) -> u8 {
        match self {
            Sender::Any => 0,
            Sender::Domain(_) => 1,
            Sender::Address(_) => 2,
        }
    }

    fn of(conditions: &Value) -> Sender {
        let text = |key: &str| {
            conditions
                .get(key)
                .and_then(Value::as_str)
                .map(|v| v.trim().to_ascii_lowercase())
        };
        match (text("sender"), text("sender_domain")) {
            (Some(address), _) => Sender::Address(address),
            (None, Some(domain)) => Sender::Domain(domain),
            (None, None) => Sender::Any,
        }
    }

    fn matches(&self, from: Option<&str>) -> bool {
        match self {
            Sender::Any => true,
            Sender::Address(wanted) => sender_matches("sender", wanted, from).is_ok(),
            Sender::Domain(wanted) => sender_matches("sender_domain", wanted, from).is_ok(),
        }
    }
}

/// Valida y normaliza la condición: `{}`, o una sola de `sender` (dirección simple) y
/// `sender_domain` (dominio), con las mismas funciones que las reglas de envío (D126).
pub fn validate_conditions(conditions: &Value) -> AppResult<Value> {
    let object = conditions
        .as_object()
        .ok_or_else(|| invalid("las condiciones de la plantilla deben ser un objeto"))?;
    if let Some(unknown) = object
        .keys()
        .find(|k| !matches!(k.as_str(), "sender" | "sender_domain"))
    {
        return Err(invalid(format!(
            "condición desconocida: «{unknown}»; solo se admite el remitente o su dominio"
        )));
    }
    if object.len() > 1 {
        return Err(invalid(
            "indique una dirección o un dominio de remitente, no los dos",
        ));
    }
    let mut out = Map::new();
    for (key, value) in object {
        let text = value
            .as_str()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .ok_or_else(|| invalid("el remitente de la plantilla debe ser un texto"))?;
        let valid = if key == "sender" {
            is_email_address(text)
        } else {
            is_domain(text)
        };
        if !valid {
            return Err(invalid(if key == "sender" {
                format!("«{text}» no es una dirección de correo simple")
            } else {
                format!("«{text}» no es un dominio válido")
            }));
        }
        out.insert(key.clone(), json!(text.to_ascii_lowercase()));
    }
    Ok(Value::Object(out))
}

/// La plantilla activa que encaja con un correo con adjuntos admitidos: gana la más concreta
/// (dirección, luego dominio, luego cualquiera). Con un remitente ausente o que no es una
/// dirección simple solo encaja una plantilla sin condición de remitente. `templates` ya está
/// filtrada a activas no archivadas; el empate (no debería darse: no se activan dos con la
/// misma condición) lo gana el identificador menor.
pub fn best_template<'a>(
    templates: &'a [TaskTemplate],
    from: Option<&str>,
) -> Option<&'a TaskTemplate> {
    templates
        .iter()
        .filter(|t| t.active && !t.archived)
        .filter_map(|t| {
            let sender = Sender::of(&t.conditions);
            sender.matches(from).then_some((sender.rank(), t))
        })
        .max_by(|(a_rank, a), (b_rank, b)| a_rank.cmp(b_rank).then_with(|| b.id.cmp(&a.id)))
        .map(|(_, t)| t)
}

/// Pasos del pipeline para un correo: un paso por adjunto admitido (sin repetir clave) y el
/// paso de la respuesta, que es la tarea del propio correo. `None` si no hay adjuntos admitidos.
fn run_steps(message: &Message) -> Option<Vec<NewStep>> {
    let mut seen = HashSet::new();
    let mut steps: Vec<NewStep> = message
        .attachments
        .iter()
        .filter_map(|attachment| {
            let name = attachment_name(attachment);
            if !supported_document(&name) {
                return None;
            }
            let key = attachment_key(&message.id, &name, attachment.size);
            seen.insert(key.clone()).then(|| NewStep {
                stage: STAGE_ATTACHMENTS,
                kind: STEP_ATTACHMENT_INVOICES.into(),
                label: name,
                attachment_key: Some(key),
                subject: false,
            })
        })
        .collect();
    if steps.is_empty() {
        return None;
    }
    steps.push(NewStep {
        stage: STAGE_REPLY,
        kind: STEP_REPLY.into(),
        label: REPLY_LABEL.into(),
        attachment_key: None,
        subject: true,
    });
    Some(steps)
}

// ---- Vista de las ejecuciones ----

/// Por qué una tarea espera, por paso que la retiene.
#[derive(Debug, Clone, Serialize)]
pub struct BlockerView {
    pub step_id: i64,
    pub task_id: Option<i64>,
    pub task_state: Option<TaskState>,
    /// `waiting` (el paso avanza), `failed`, `cancelled`, `attachment_missing` (sin tarea; se
    /// reintenta) o `attachment_rejected` (el adjunto nunca podrá importarse: no se resolverá
    /// solo y la salida es «Continuar sin ese paso»).
    pub code: &'static str,
    /// Código corto del último fallo de la descarga, si lo hubo.
    pub error_code: Option<String>,
}

/// Una tarea retenida por su pipeline. `reason` es `waiting` («Esperando») si todos los pasos
/// que la retienen avanzan, o `blocked` («Bloqueada») si alguno falló, se canceló o falta su
/// adjunto. Los textos los genera la interfaz.
#[derive(Debug, Clone, Serialize)]
pub struct BlockView {
    pub reason: &'static str,
    pub by: Vec<BlockerView>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StepView {
    pub id: i64,
    pub stage: i64,
    pub kind: String,
    pub label: String,
    pub task_id: Option<i64>,
    pub task_state: Option<TaskState>,
    pub waived: bool,
    pub waived_note: Option<String>,
    pub error_code: Option<String>,
    /// «Continuar sin ese paso» es válido ahora para este paso.
    pub waivable: bool,
    /// El adjunto llegó después de «Continuar sin ese paso»: se importó igual para no perder la
    /// factura, y la revisión forzada de la respuesta se conserva (D150).
    pub linked_after_waive: bool,
    /// Solo si la tarea del paso está en cola y algo la retiene.
    pub blocked: Option<BlockView>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RunView {
    pub id: i64,
    pub template_id: String,
    pub template_version: i64,
    pub template_name: Option<String>,
    pub subject_task_id: i64,
    pub created_at: i64,
    pub steps: Vec<StepView>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PipelineOverview {
    pub runs: Vec<RunView>,
}

/// ¿Está el paso atascado (sin tarea, fallida o cancelada)?
fn stuck(state: Option<TaskState>) -> bool {
    matches!(
        state,
        None | Some(TaskState::Failed) | Some(TaskState::Cancelled)
    )
}

impl App {
    // ---- Plantillas ----

    /// Versión vigente de cada plantilla, archivadas incluidas.
    pub fn task_templates(&self) -> AppResult<Vec<TaskTemplate>> {
        Ok(self.store.task_templates()?)
    }

    /// Plantillas activas con el día (UTC, días desde 1970) desde el que lo están de forma
    /// continua: la versión que pasó a activa tras la última inactiva (D149).
    pub(crate) fn active_task_templates(&self) -> AppResult<Vec<(TaskTemplate, i64)>> {
        let mut out = Vec::new();
        for template in self.store.task_templates()? {
            if !(template.active && !template.archived) {
                continue;
            }
            let history = self.store.task_template_history(&template.id)?;
            let since = nestadmin_core::active_since(&history).unwrap_or(template.created_at);
            out.push((template, since.div_euclid(86_400_000)));
        }
        Ok(out)
    }

    /// Guarda una plantilla: nueva (inactiva, con identificador del nombre si no se da) o una
    /// versión nueva de una existente, que conserva su estado activo. Las ejecuciones en curso
    /// conservan sus pasos. Rechaza un paso fuera del catálogo y, si la plantilla queda activa,
    /// otra activa con la misma condición.
    pub fn save_task_template(
        &mut self,
        id: Option<&str>,
        name: &str,
        conditions: &Value,
        steps: &Value,
    ) -> AppResult<TaskTemplate> {
        let name = name.trim();
        if name.is_empty() {
            return Err(invalid("indique un nombre para la plantilla"));
        }
        if name.chars().count() > NAME_MAX_CHARS {
            return Err(invalid(format!(
                "el nombre de la plantilla admite como mucho {NAME_MAX_CHARS} caracteres"
            )));
        }
        let steps = validate_steps(steps)?;
        let conditions = validate_conditions(conditions)?;
        let existing = self.store.task_templates()?;
        let id = match id.map(str::trim).filter(|i| !i.is_empty()) {
            Some(id) => {
                if !id
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
                {
                    return Err(invalid(
                        "el identificador de la plantilla solo admite minúsculas, cifras y guiones",
                    ));
                }
                id.to_owned()
            }
            None => {
                let base = crate::panel::slugify(name);
                let base = if base.is_empty() {
                    "plantilla".to_owned()
                } else {
                    base
                };
                let mut candidate = base.clone();
                let mut n = 2;
                while existing.iter().any(|t| t.id == candidate) {
                    candidate = format!("{base}-{n}");
                    n += 1;
                }
                candidate
            }
        };
        let current = existing.iter().find(|t| t.id == id);
        if current.is_some_and(|t| t.active) {
            self.check_condition_free(&id, &conditions, &existing)?;
        }
        Ok(self
            .store
            .save_task_template(&id, name, &conditions, &steps, false)?)
    }

    /// Activa o desactiva una plantilla. Activar rechaza si otra plantilla activa tiene la misma
    /// condición. Una plantilla nunca autoriza nada: activarla solo ordena tareas nuevas.
    pub fn set_task_template_active(&mut self, id: &str, active: bool) -> AppResult<TaskTemplate> {
        if active {
            let existing = self.store.task_templates()?;
            let template = existing
                .iter()
                .find(|t| t.id == id)
                .ok_or_else(|| invalid(format!("la plantilla {id} no existe")))?;
            self.check_condition_free(id, &template.conditions, &existing)?;
        }
        Ok(self.store.set_task_template_active(id, active)?)
    }

    /// Archiva (o desarchiva) una plantilla sin borrar versiones; queda inactiva.
    pub fn archive_task_template(&mut self, id: &str, archived: bool) -> AppResult<TaskTemplate> {
        Ok(self.store.set_task_template_archived(id, archived)?)
    }

    fn check_condition_free(
        &self,
        id: &str,
        conditions: &Value,
        existing: &[TaskTemplate],
    ) -> AppResult<()> {
        let wanted = Sender::of(conditions);
        match existing
            .iter()
            .find(|t| t.id != id && t.active && !t.archived && Sender::of(&t.conditions) == wanted)
        {
            Some(other) => Err(invalid(format!(
                "la plantilla «{}» ya está activa con la misma condición; desactívela antes",
                other.name
            ))),
            None => Ok(()),
        }
    }

    // ---- Alta de tareas ----

    /// Encola un correo nuevo. Si encaja una plantilla activa (y solo entonces), crea en una
    /// transacción el correo, la ejecución y un paso por adjunto admitido; los adjuntos se
    /// importan después y enlazan su paso. Sin plantilla, es un `enqueue` normal. Nunca se
    /// llama para un correo que ya tenía tarea, y una plantilla solo se aplica a un correo
    /// fechado desde el día en que pasó a activa (D149): sin fecha legible no hay ejecución.
    pub(crate) fn enqueue_new_mail(
        &mut self,
        input: &str,
        message: &Message,
        templates: &[(TaskTemplate, i64)],
    ) -> AppResult<Task> {
        let day = message.date.as_deref().and_then(day_of);
        let eligible: Vec<TaskTemplate> = match day {
            Some(day) => templates
                .iter()
                .filter(|(_, since)| *since <= day)
                .map(|(t, _)| t.clone())
                .collect(),
            None => Vec::new(),
        };
        let template = best_template(&eligible, message.from.as_deref());
        let (Some(template), Some(steps)) = (template, run_steps(message)) else {
            return Ok(self.store.enqueue(EMAIL, input)?);
        };
        let steps_json = serde_json::to_string(&template.steps).map_err(CoreError::from)?;
        Ok(self.store.enqueue_with_run(
            EMAIL,
            input,
            &template.id,
            template.version,
            &steps_json,
            &steps,
        )?)
    }

    /// Encola una parte de factura separada (D68). Si la factura madre es un paso de una
    /// ejecución, la parte es otro paso de la misma etapa: la respuesta no se desbloquea hasta
    /// que terminen todas. Sin ejecución, o si la ejecución ya no admite pasos nuevos (una etapa
    /// posterior ya empezó, por ejemplo tras «Continuar sin ese paso»), es un `enqueue` normal:
    /// la parte conserva `split_from`, no se duplica y D68 la manda a revisión. Solo un fallo
    /// del almacenamiento se propaga; el resultado de la factura madre no se pierde por la
    /// ejecución.
    pub(crate) fn enqueue_part(
        &mut self,
        parent: &Task,
        index: u64,
        of: usize,
        input: &str,
    ) -> AppResult<Task> {
        let step = match self.store.pipeline_step_of_task(parent.id) {
            Ok(step) => step,
            Err(CoreError::Storage(e)) => return Err(CoreError::Storage(e).into()),
            Err(_) => None,
        };
        if let Some(step) = step {
            let part = NewStep {
                stage: step.stage,
                kind: step.kind.clone(),
                label: format!("{} (factura {index} de {of})", step.label),
                attachment_key: None,
                subject: false,
            };
            match self
                .store
                .add_step_with_task(step.run_id, &part, INVOICE, input)
            {
                Ok(task) => return Ok(task),
                Err(CoreError::Storage(e)) => return Err(CoreError::Storage(e).into()),
                // PipelineStageTooLate u otro rechazo de la ejecución: la parte sigue fuera.
                Err(_) => {}
            }
        }
        Ok(self.store.enqueue(INVOICE, input)?)
    }

    // ---- Vista y salida ----

    /// Ejecuciones con sus pasos, la plantilla y versión que las abrió y, por tarea retenida,
    /// qué la retiene y con qué código: las 500 más recientes y, siempre, las cuyo correo sigue en
    /// cola. Sin contenido de correos ni documentos más allá de lo que ya recibe la interfaz.
    pub fn pipeline_overview(&self) -> AppResult<PipelineOverview> {
        let runs = self.store.pipeline_runs_for_overview(OVERVIEW_RUNS)?;
        Ok(PipelineOverview {
            runs: self.run_views(runs)?,
        })
    }

    /// Vistas de varias ejecuciones con tres consultas (ejecuciones, pasos con el estado de su
    /// tarea y versiones de plantilla), sin leer ni deserializar la entrada ni el resultado de
    /// ninguna tarea. Los bloqueos se calculan en memoria con la misma condición que `READY_SQL`.
    fn run_views(&self, runs: Vec<PipelineRun>) -> AppResult<Vec<RunView>> {
        let names: HashMap<(String, i64), String> = self
            .store
            .task_template_versions()?
            .into_iter()
            .map(|t| ((t.id, t.version), t.name))
            .collect();
        let ids: Vec<i64> = runs.iter().map(|r| r.id).collect();
        let mut by_run: HashMap<i64, Vec<(PipelineStep, Option<TaskState>)>> = HashMap::new();
        for (step, state) in self.store.pipeline_steps_with_state(&ids)? {
            by_run.entry(step.run_id).or_default().push((step, state));
        }
        Ok(runs
            .into_iter()
            .map(|run| {
                let steps = by_run.remove(&run.id).unwrap_or_default();
                RunView {
                    id: run.id,
                    template_name: names
                        .get(&(run.template_id.clone(), run.template_version))
                        .cloned(),
                    template_id: run.template_id,
                    template_version: run.template_version,
                    subject_task_id: run.subject_task_id,
                    created_at: run.created_at,
                    steps: step_views(&steps),
                }
            })
            .collect())
    }

    /// «Continuar sin ese paso» (D150): omite un paso atascado (su tarea falló, se canceló o falta
    /// su adjunto) a petición de la persona, con una nota opcional, y deja constancia en el
    /// historial sin contenido. No crea, aprueba ni envía nada: la respuesta pasa de «Bloqueada»
    /// a lista y, al terminar, **siempre va a revisión**, aunque coincida una regla. Repetirlo
    /// sobre un paso ya omitido no hace nada y devuelve la ejecución. Devuelve la ejecución
    /// actualizada, consultada directamente. Si un adjunto llega después, se importa igual para
    /// no perder la factura (`linked_after_waive`) y la revisión forzada se conserva.
    pub fn waive_pipeline_step(&mut self, step_id: i64, note: &str) -> AppResult<RunView> {
        self.waive_step(step_id, note).map(|(view, _)| view)
    }

    /// Como `waive_pipeline_step`, y dice si omitió el paso ahora (`true`) o ya estaba omitido.
    pub fn waive_step(&mut self, step_id: i64, note: &str) -> AppResult<(RunView, bool)> {
        let note = note.trim();
        if note.chars().count() > NOTE_MAX_CHARS {
            return Err(invalid(format!(
                "la nota admite como mucho {NOTE_MAX_CHARS} caracteres"
            )));
        }
        let run_id = self
            .store
            .pipeline_step(step_id)?
            .ok_or(CoreError::PipelineStepNotFound(step_id))?
            .run_id;
        let newly = self
            .store
            .waive_pipeline_step(step_id, (!note.is_empty()).then_some(note))?;
        let run = self
            .store
            .pipeline_run(run_id)?
            .ok_or(CoreError::PipelineRunNotFound(run_id))?;
        let view = self
            .run_views(vec![run])?
            .pop()
            .ok_or(CoreError::PipelineRunNotFound(run_id))?;
        Ok((view, newly))
    }
}

/// Vista de los pasos de una ejecución, en memoria: bloqueos, si se puede omitir cada paso y qué
/// pasos llegaron tras omitirse.
fn step_views(steps: &[(PipelineStep, Option<TaskState>)]) -> Vec<StepView> {
    steps
        .iter()
        .map(|(step, state)| {
            let retains = steps
                .iter()
                .any(|(later, ls)| later.stage > step.stage && *ls == Some(TaskState::Queued));
            let blocked = (*state == Some(TaskState::Queued))
                .then(|| {
                    let by: Vec<BlockerView> = steps
                        .iter()
                        .filter(|(p, ps)| {
                            p.stage < step.stage
                                && p.waived_at.is_none()
                                && *ps != Some(TaskState::Completed)
                        })
                        .map(|(p, ps)| BlockerView {
                            step_id: p.id,
                            task_id: p.task_id,
                            task_state: *ps,
                            code: match ps {
                                None if p.last_error.as_deref()
                                    == Some(nestadmin_core::ATTACHMENT_REJECTED) =>
                                {
                                    "attachment_rejected"
                                }
                                None => "attachment_missing",
                                Some(TaskState::Failed) => "failed",
                                Some(TaskState::Cancelled) => "cancelled",
                                Some(_) => "waiting",
                            },
                            error_code: p.last_error.clone(),
                        })
                        .collect();
                    (!by.is_empty()).then(|| BlockView {
                        reason: if by.iter().any(|b| b.code != "waiting") {
                            "blocked"
                        } else {
                            "waiting"
                        },
                        by,
                    })
                })
                .flatten();
            StepView {
                id: step.id,
                stage: step.stage,
                kind: step.kind.clone(),
                label: step.label.clone(),
                task_id: step.task_id,
                task_state: *state,
                waived: step.waived_at.is_some(),
                waived_note: step.waived_by_note.clone(),
                error_code: step.last_error.clone(),
                waivable: step.waived_at.is_none() && stuck(*state) && retains,
                linked_after_waive: step.waived_at.is_some() && step.task_id.is_some(),
                blocked,
            }
        })
        .collect()
}
