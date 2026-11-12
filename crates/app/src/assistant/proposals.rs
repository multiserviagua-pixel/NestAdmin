//! Propuestas del asistente (D185, R34, E3): el asistente PREPARA una acción, la aplicación la
//! guarda con los datos reales del equipo y solo el botón de la tarjeta puede confirmarla.
//!
//! Dos mitades que no se tocan:
//! - Crear (`tool_propose`, herramientas de nivel `Propose`): valida con datos del equipo, nunca
//!   del modelo (la referencia `doc:N` o `acct:N` tiene que haberla devuelto una herramienta de
//!   esta conversación; la versión vista la lee la aplicación), guarda la propuesta `pending` y
//!   devuelve al modelo solo su identificador. No ejecuta nada.
//! - Confirmar o descartar (`proposal_confirm`, `proposal_dismiss`): comandos de la ventana que no
//!   figuran entre las herramientas. Confirmar ejecuta EL MISMO método de `App` que la pantalla,
//!   con el origen `chat` en el historial; repetirlo devuelve el recibo original.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use nestadmin_ai::assistant::{account_index, doc_id, rule_index, suggestion_index};
use nestadmin_ai::refine::is_format_char;
use nestadmin_ai::tasks::{draft_flags_allowing, draft_leaks_internals};
use nestadmin_core::actions::idempotency_key;
use nestadmin_core::{
    is_learned_origin, CoreError, NewProposal, ProposalRow, Task, TaskState, CHAT_ORIGIN,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use super::tools::ToolOutput;
use crate::panel::{edit_keeps_active, NewRule, Simulation, SIMULATED};
use crate::refine::now_ms;
use crate::rules::{REGISTER_INVOICE, SEND_DRAFT};
use crate::{
    action_kind, check_rule, task_is_initial_read, App, AppError, AppResult, EMAIL, INVOICE,
};

/// Datos de otros documentos que enumera como mucho la tarjeta de un borrador (D266).
pub const MAX_OTHER_SHOWN: usize = 5;
/// Caracteres mínimos de un dato de otro documento para contar (evita falsos positivos).
pub const MIN_OTHER_CHARS: usize = 4;

/// Texto en minúsculas y sin tildes, para comparar nombres.
fn fold(text: &str) -> String {
    nestadmin_ai::assistant::plain(text)
}

/// ¿Está `needle` en `haystack` como palabra entera (sin letras ni cifras pegadas por los lados)?
/// Los dos van ya sin tildes ni mayúsculas.
fn contains_word(haystack: &str, needle: &str) -> bool {
    haystack.match_indices(needle).any(|(start, found)| {
        let before = haystack[..start].chars().next_back();
        let after = haystack[start + found.len()..].chars().next();
        !before.is_some_and(char::is_alphanumeric) && !after.is_some_and(char::is_alphanumeric)
    })
}

/// Propuestas nuevas como mucho por turno.
pub const MAX_PER_TURN: usize = 3;
/// Propuestas pendientes (sin caducar) como mucho por conversación.
pub const MAX_PENDING: usize = 10;
/// Vida de una propuesta: 24 horas.
pub const TTL_MS: i64 = 24 * 60 * 60 * 1000;

/// Estado en memoria del turno en curso que necesita crear propuestas. Lo fija `begin` y lo vacía
/// `finish`; no se guarda nada de esto.
#[derive(Debug, Default)]
pub(crate) struct TurnCtx {
    pub(crate) active: bool,
    pub(crate) conversation_id: Option<i64>,
    /// Referencias (`doc:N`, `acct:N`) que las herramientas devolvieron en esta conversación.
    pub(crate) known: HashSet<String>,
    /// A qué apunta cada `sug:N` (`{"fp", "shown"}`) y `rule:N` (`{"rule_id"}`) de la conversación.
    /// Se guarda con el turno (como `known`) para que valga entre turnos; nunca sale hacia el
    /// modelo.
    pub(crate) targets: BTreeMap<String, Value>,
    /// Propuestas de este turno (nuevas o reutilizadas), sin repetir.
    pub(crate) created: Vec<i64>,
}

impl TurnCtx {
    /// La referencia `prefix` + N de lo identificado por `key` en `data[key_field]`: la misma de
    /// siempre si ya se dio en la conversación (con el destino puesto al día) o la siguiente libre.
    pub(crate) fn target_ref(
        &mut self,
        prefix: &str,
        key_field: &str,
        key: &str,
        data: Value,
    ) -> String {
        let existing = self
            .targets
            .iter()
            .find(|(r, v)| r.starts_with(prefix) && v[key_field].as_str() == Some(key))
            .map(|(r, _)| r.clone());
        let reference = existing.unwrap_or_else(|| {
            let next = self
                .targets
                .keys()
                .filter_map(|r| r.strip_prefix(prefix)?.parse::<usize>().ok())
                .max()
                .unwrap_or(0)
                + 1;
            format!("{prefix}{next}")
        });
        self.targets.insert(reference.clone(), data);
        reference
    }
}

/// Si confirmar la propuesta envía un correo, registra una factura o no tiene efecto externo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Effect {
    SendMail,
    RegisterInvoice,
    #[serde(rename = "none")]
    NoEffect,
    /// No actúa por sí misma, pero el correo nuevo o la tarea reintentada pasan por las reglas
    /// activas, que pueden enviar o registrar sin revisión (R07).
    MayTriggerRules,
    /// Crea una regla INACTIVA: no actúa hasta que la persona la active en el panel (D191).
    RuleInactive,
    /// Cambia las condiciones de una regla; nunca la activa y solo sigue activa si se estrecha.
    RuleEdited,
    /// Importa los archivos que la persona elija en el diálogo del sistema; lo importado pasa por
    /// las reglas activas.
    Import,
}

impl Effect {
    fn as_str(self) -> &'static str {
        match self {
            Self::SendMail => "send_mail",
            Self::RegisterInvoice => "register_invoice",
            Self::NoEffect => "none",
            Self::MayTriggerRules => "may_trigger_rules",
            Self::RuleInactive => "rule_inactive",
            Self::RuleEdited => "rule_edited",
            Self::Import => "import",
        }
    }

    fn parse(text: &str) -> Self {
        match text {
            "send_mail" => Self::SendMail,
            "register_invoice" => Self::RegisterInvoice,
            "may_trigger_rules" => Self::MayTriggerRules,
            "rule_inactive" => Self::RuleInactive,
            "rule_edited" => Self::RuleEdited,
            "import" => Self::Import,
            _ => Self::NoEffect,
        }
    }
}

/// Los quince tipos de propuesta.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Approve,
    Reject,
    Retry,
    Archive,
    ConfirmNotSent,
    EditDraft,
    FetchMail,
    MailLimit,
    RenameAccount,
    SenderPreference,
    AcceptSuggestion,
    DiscardSuggestion,
    CreateRule,
    EditRule,
    Import,
}

impl Kind {
    fn from_tool(tool: &str) -> Option<Self> {
        Self::parse(tool.strip_prefix("propose_")?)
    }

    fn parse(code: &str) -> Option<Self> {
        Some(match code {
            "approve" => Self::Approve,
            "reject" => Self::Reject,
            "retry" => Self::Retry,
            "archive" => Self::Archive,
            "confirm_not_sent" => Self::ConfirmNotSent,
            "edit_draft" => Self::EditDraft,
            "fetch_mail" => Self::FetchMail,
            "mail_limit" => Self::MailLimit,
            "rename_account" => Self::RenameAccount,
            "sender_preference" => Self::SenderPreference,
            "accept_suggestion" => Self::AcceptSuggestion,
            "discard_suggestion" => Self::DiscardSuggestion,
            "create_rule" => Self::CreateRule,
            "edit_rule" => Self::EditRule,
            "import" => Self::Import,
            _ => return None,
        })
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Approve => "approve",
            Self::Reject => "reject",
            Self::Retry => "retry",
            Self::Archive => "archive",
            Self::ConfirmNotSent => "confirm_not_sent",
            Self::EditDraft => "edit_draft",
            Self::FetchMail => "fetch_mail",
            Self::MailLimit => "mail_limit",
            Self::RenameAccount => "rename_account",
            Self::SenderPreference => "sender_preference",
            Self::AcceptSuggestion => "accept_suggestion",
            Self::DiscardSuggestion => "discard_suggestion",
            Self::CreateRule => "create_rule",
            Self::EditRule => "edit_rule",
            Self::Import => "import",
        }
    }

    /// Las de reglas y sugerencias: su huella sale de `summary.basis`, no de todo el resumen (los
    /// casos que habría cubierto cambian con cada tarea y no hacen obsoleta la propuesta).
    fn on_rules(self) -> bool {
        matches!(
            self,
            Self::AcceptSuggestion | Self::DiscardSuggestion | Self::CreateRule | Self::EditRule
        )
    }

    /// Las que actúan sobre una tarea (llevan versión vista).
    fn on_task(self) -> bool {
        matches!(
            self,
            Self::Approve
                | Self::Reject
                | Self::Retry
                | Self::Archive
                | Self::ConfirmNotSent
                | Self::EditDraft
        )
    }
}

/// La tarjeta de una propuesta, tal como llega a la ventana. La compone la aplicación con los datos
/// reales del equipo; el único texto del modelo que lleva es el del borrador de `edit_draft` (y el
/// valor ya validado de un nombre de cuenta o una clave de remitente).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProposalCard {
    pub id: i64,
    pub kind: String,
    /// `pending`, `executing`, `confirmed`, `rejected`, `expired`, `stale` o `failed`.
    pub status: String,
    pub task_id: Option<i64>,
    /// Versión de la tarea que se vio al crearla (la lee la aplicación).
    pub seen_version: Option<i64>,
    pub created_at: i64,
    pub expires_at: i64,
    pub effect: Effect,
    /// Lo mismo que muestra la pantalla antes de aprobar, según el tipo (campo `type`).
    pub summary: Value,
    /// Recibo original, si ya se confirmó.
    pub receipt: Option<ProposalReceipt>,
}

/// Resultado real de la acción: `done` solo si la acción con efecto consta realizada.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Done,
    NotDone,
}

/// Lo que devuelve confirmar: se guarda tal cual y se devuelve igual si se confirma otra vez.
/// Si `outcome` es `not_done` la propuesta queda `failed` y `reason` dice por qué (código cerrado:
/// `action_failed` si la tarea quedó fallida, `returned_to_review` si volvió a revisión,
/// `not_completed` en otro caso).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProposalReceipt {
    pub proposal_id: i64,
    pub kind: String,
    pub task_id: Option<i64>,
    /// Estado de la tarea tras la acción, si la hay.
    pub task_state: Option<String>,
    pub effect: Effect,
    pub confirmed_at: i64,
    /// Lo que pasó de verdad con la acción.
    pub outcome: Outcome,
    /// Motivo cerrado cuando `outcome` es `not_done`.
    pub reason: Option<String>,
    /// `true` si tras un corte se comprobó que la acción ya estaba hecha y no se ejecutó de nuevo.
    pub reconciled: bool,
    /// La regla creada o editada (aceptar, crear o editar), para abrirla en el panel de reglas.
    #[serde(default)]
    pub rule_id: Option<String>,
    /// Hechos de la acción según el tipo (versión, límite, nombre, recuento…); sin secretos.
    pub result: Value,
}

/// Lo que valida la creación de una propuesta.
struct Built {
    task_id: Option<i64>,
    seen_version: Option<i64>,
    args: Value,
    effect: Effect,
    summary: Value,
    fp: String,
}

/// Lo que dejó la ejecución de la acción.
struct Run {
    result: Value,
    outcome: Outcome,
    reason: Option<&'static str>,
}

impl Run {
    fn done(result: Value) -> Self {
        Self {
            result,
            outcome: Outcome::Done,
            reason: None,
        }
    }
}

/// Huella de lo que muestra la tarjeta y de lo que determina la acción: tipo, estado, versión y
/// hora de la tarea (si la hay), efecto y resumen. El JSON es canónico (claves ordenadas), de modo
/// que la misma situación da siempre la misma huella.
fn fingerprint(kind: Kind, task: Option<&Task>, effect: Effect, summary: &Value) -> String {
    let task = task.map(|t| {
        json!({
            "state": t.state.as_str(),
            "version": version_of(t),
            "updated_at": t.updated_at,
        })
    });
    // Reglas y sugerencias: solo lo que las identifica (`basis`, un resumen criptográfico).
    let shown = match summary.get("basis") {
        Some(basis) if kind.on_rules() => basis.clone(),
        _ => summary.clone(),
    };
    let canonical = json!({
        "kind": kind.as_str(),
        "task": task,
        "effect": effect.as_str(),
        "summary": shown,
    })
    .to_string();
    Sha256::digest(canonical.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// ¿Lleva el texto caracteres de formato invisibles o bidireccionales?
fn has_invisible(text: &str) -> bool {
    text.chars().any(is_format_char)
}

/// Tramos de IBAN de un texto: (inicio, fin, forma compacta en mayúsculas). Mismo criterio que
/// el detector de `draft_flags`.
fn iban_spans(text: &str) -> Vec<(usize, usize, String)> {
    let mut tokens: Vec<(usize, usize)> = Vec::new();
    let mut start = None;
    for (i, c) in text.char_indices() {
        if c.is_alphanumeric() {
            start.get_or_insert(i);
        } else if let Some(s) = start.take() {
            tokens.push((s, i));
        }
    }
    if let Some(s) = start {
        tokens.push((s, text.len()));
    }
    let upper: Vec<String> = tokens
        .iter()
        .map(|&(s, e)| text[s..e].to_uppercase())
        .collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        let b = upper[i].as_bytes();
        let head = b.len() >= 4
            && b[..2].iter().all(u8::is_ascii_uppercase)
            && b[2..4].iter().all(u8::is_ascii_digit);
        if head {
            let mut total = b.len();
            let mut j = i + 1;
            while j < tokens.len() && upper[j].len() <= 4 {
                total += upper[j].len();
                j += 1;
            }
            if total >= 15 {
                out.push((tokens[i].0, tokens[j - 1].1, upper[i..j].concat()));
                i = j;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// ¿Introduce `new` un contacto, un IBAN, una URL o un identificador que no esté en el perfil
/// del negocio ni en el borrador actual? Lo que ya estaba en el borrador (una firma, un teléfono)
/// puede conservarse.
fn introduces_new_contact(new: &str, current: &str, profile: Option<&str>) -> bool {
    let known: HashSet<String> = iban_spans(current).into_iter().map(|s| s.2).collect();
    let mut cleaned = new.to_owned();
    for (start, end, compact) in iban_spans(new).into_iter().rev() {
        if known.contains(&compact) {
            cleaned.replace_range(start..end, " ");
        }
    }
    let allowed = match profile {
        Some(p) => format!(
            "{p}
{current}"
        ),
        None => current.to_owned(),
    };
    !draft_flags_allowing(&cleaned, Some(&allowed)).is_empty()
}

static SEQ: AtomicU64 = AtomicU64::new(0);

fn version_of(task: &Task) -> i64 {
    task.result
        .as_ref()
        .and_then(|r| r["version"].as_i64())
        .unwrap_or(1)
}

fn effect_of(task: &Task) -> Effect {
    if task.kind == INVOICE {
        Effect::RegisterInvoice
    } else {
        Effect::SendMail
    }
}

fn text(value: &Value) -> Option<String> {
    value.as_str().map(str::to_owned)
}

/// Título corto de la tarea, de datos del equipo.
fn title_of(task: &Task) -> String {
    let r = task.result.clone().unwrap_or(Value::Null);
    if task.kind == INVOICE {
        let supplier = r["fields"]["supplier"].as_str().unwrap_or("").trim();
        let number = r["fields"]["invoice_number"].as_str().unwrap_or("").trim();
        let joined = format!("{supplier} {number}");
        let joined = joined.trim();
        if joined.is_empty() {
            "Factura".to_owned()
        } else {
            joined.to_owned()
        }
    } else {
        r["message"]["subject"]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("(sin asunto)")
            .to_owned()
    }
}

fn task_block(task: &Task) -> Value {
    json!({
        "id": task.id,
        "kind": task.kind,
        "title": title_of(task),
        "state": task.state.as_str(),
        "version": version_of(task),
    })
}

/// Código de la incidencia de recuperación, como la muestra la revisión.
fn recovery_code(r: &Value) -> Option<&'static str> {
    if r["recovery"]["needs_confirmation"] == json!(true) {
        Some("needs_confirmation")
    } else if !r["recovery"].is_null()
        || (r["attempt"]["version"].as_i64().is_some()
            && r["attempt"]["version"].as_i64() == Some(r["version"].as_i64().unwrap_or(1)))
    {
        Some("unconfirmed_attempt")
    } else {
        None
    }
}

/// Nombre del documento de una factura (sin rutas).
fn document_name(task: &Task, r: &Value) -> Option<String> {
    let from_input: Value = serde_json::from_str(&task.input_ref).unwrap_or(Value::Null);
    let origin = if r["origin"].is_object() {
        &r["origin"]
    } else {
        &from_input["origin"]
    };
    text(&origin["name"])
        .or_else(|| text(&origin["attachment"]))
        .or_else(|| text(&from_input["document"]))
}

/// Resumen de la aprobación de un correo: lo mismo que la pantalla de revisión antes de enviar.
/// `draft_flags` se recalcula con el cuerpo actual del borrador.
fn email_summary(task: &Task, draft_flags: &[&str], other: &[Value]) -> Value {
    let r = task.result.clone().unwrap_or(Value::Null);
    let mut issues: Vec<&str> = Vec::new();
    if let Some(code) = recovery_code(&r) {
        issues.push(code);
    }
    if r["draft"]["to"]
        .as_array()
        .is_none_or(std::vec::Vec::is_empty)
    {
        issues.push("no_recipient");
    }
    json!({
        "type": "email",
        "version": version_of(task),
        "task": task_block(task),
        "from": r["message"]["from"],
        "original_subject": r["message"]["subject"],
        "to": r["draft"]["to"],
        "subject": r["draft"]["subject"],
        "body": r["draft"]["body"],
        "category": r["category"],
        "issues": issues,
        "draft_flags": draft_flags,
        // D266 (ampliado a aprobar): datos de otros documentos que el borrador guardado ya lleva.
        "other_documents": other.iter().take(MAX_OTHER_SHOWN).collect::<Vec<_>>(),
        "other_documents_total": other.len(),
        "recipient_signals": r["recipient_signals"],
        "review_reason": r["review_reason"],
    })
}

/// Resumen de la aprobación de una factura: documento, datos, incidencias, duplicados y el aviso
/// de importe dudoso, como la revisión.
fn invoice_summary(task: &Task) -> Value {
    let r = task.result.clone().unwrap_or(Value::Null);
    let fields = &r["fields"];
    let duplicates: Vec<Value> = r["duplicates"]
        .as_array()
        .map(|d| d.iter().map(|x| x["existing_id"].clone()).collect())
        .unwrap_or_default();
    json!({
        "type": "invoice",
        "version": version_of(task),
        "task": task_block(task),
        "document": document_name(task, &r),
        "supplier": fields["supplier"],
        "invoice_number": fields["invoice_number"],
        "date": fields["date"],
        "subtotal": fields["subtotal"],
        "tax": fields["tax"],
        "total": fields["total"],
        "currency": fields["currency"],
        "issues": r["issues"],
        "duplicates": duplicates,
        "amount_review": r["amount_review"],
        "recovered": r["recovered"],
        "unreadable": r["unreadable"],
        "recovery": recovery_code(&r),
        "review_reason": r["review_reason"],
    })
}

/// `may_trigger_rules` si hay reglas activas que envían o registran; si no, sin efecto.
fn rules_effect(send: usize, register: usize) -> Effect {
    if send + register > 0 {
        Effect::MayTriggerRules
    } else {
        Effect::NoEffect
    }
}

impl App {
    // ---- Crear (herramientas de nivel «propone») ----

    /// Ejecuta una herramienta `propose_*`: valida con datos del equipo, guarda la propuesta
    /// `pending` y devuelve al modelo solo `{"proposal": "prop:N", "status": "created"}` o un
    /// código de rechazo. No ejecuta nada.
    pub(crate) fn tool_propose(&self, name: &str, args: &Map<String, Value>) -> ToolOutput {
        let reply = |value: Value| ToolOutput {
            value,
            refs: Vec::new(),
            navigate: None,
            withheld: 0,
        };
        let failure = |code: &str| reply(json!({ "error": code }));
        let Some(kind) = Kind::from_tool(name) else {
            return failure("unknown_tool");
        };
        let (conversation_id, known, targets, created) = {
            let Ok(ctx) = self.assistant.turn.lock() else {
                return failure("unavailable");
            };
            if !ctx.active {
                return failure("no_turn");
            }
            (
                ctx.conversation_id,
                ctx.known.clone(),
                ctx.targets.clone(),
                ctx.created.clone(),
            )
        };
        let built = match self.build_proposal(kind, args, &known, &targets) {
            Ok(built) => built,
            // Hallado en `tauri dev` (T37d): el modelo local propone sobre una referencia que no
            // pidió antes y, con solo el código, se rinde. La aplicación le dice qué llamar.
            Err("ref_not_returned") => {
                let next = match kind {
                    Kind::EditRule => {
                        "Llame primero a list_rules y repita con la referencia rule:N que devuelva."
                    }
                    Kind::AcceptSuggestion | Kind::DiscardSuggestion => {
                        "Llame primero a list_suggestions y repita con la referencia sug:N que devuelva."
                    }
                    _ => "Llame primero a search o list_tasks y repita con la referencia que devuelva.",
                };
                return reply(json!({ "error": "ref_not_returned", "next": next }));
            }
            Err(code) => return failure(code),
        };
        match self.save_proposal(kind, &built, conversation_id, &created) {
            Ok(id) => {
                if let Ok(mut ctx) = self.assistant.turn.lock() {
                    if !ctx.created.contains(&id) {
                        ctx.created.push(id);
                    }
                }
                reply(json!({ "proposal": format!("prop:{id}"), "status": "created" }))
            }
            Err(code) => failure(code),
        }
    }

    /// Guarda la propuesta (o devuelve la idéntica que sigue pendiente) respetando los topes.
    fn save_proposal(
        &self,
        kind: Kind,
        built: &Built,
        conversation_id: Option<i64>,
        created: &[i64],
    ) -> Result<i64, &'static str> {
        let unavailable = |_| "unavailable";
        if let Some(existing) = self
            .store
            .proposal_identical_pending(
                conversation_id,
                kind.as_str(),
                built.task_id,
                built.seen_version,
                &built.fp,
                &built.args,
            )
            .map_err(unavailable)?
        {
            return Ok(existing);
        }
        if created.len() >= MAX_PER_TURN {
            return Err("too_many_this_turn");
        }
        if self
            .store
            .proposals_pending_count(conversation_id)
            .map_err(unavailable)?
            >= MAX_PENDING
        {
            return Err("too_many_pending");
        }
        let card = json!({ "effect": built.effect.as_str(), "summary": built.summary });
        let key = format!(
            "prop/{}/{}/{}-{}",
            kind.as_str(),
            built.task_id.unwrap_or(0),
            now_ms(),
            SEQ.fetch_add(1, Ordering::SeqCst)
        );
        let id = self
            .store
            .proposal_insert(&NewProposal {
                conversation_id,
                kind: kind.as_str(),
                task_id: built.task_id,
                seen_version: built.seen_version,
                seen_fp: &built.fp,
                args: &built.args,
                card: &card,
                idem_key: &key,
                expires_at: now_ms() + TTL_MS,
            })
            .map_err(unavailable)?;
        // Sin texto del modelo ni de documentos: tipo, tarea y efecto.
        self.store
            .proposal_audit(
                built.task_id,
                "created",
                &json!({ "kind": kind.as_str(), "task_id": built.task_id, "effect": built.effect.as_str() }),
            )
            .map_err(unavailable)?;
        Ok(id)
    }

    /// La tarea a la que se refiere la propuesta, validada contra lo que las herramientas
    /// devolvieron en la conversación y contra el equipo.
    fn task_of(
        &self,
        args: &Map<String, Value>,
        known: &HashSet<String>,
    ) -> Result<Task, &'static str> {
        let id = args
            .get("ref")
            .and_then(Value::as_str)
            .and_then(doc_id)
            .ok_or("bad_ref")?;
        // La defensa frente a un documento que nombre otra tarea: solo vale una referencia que una
        // herramienta devolvió en esta conversación.
        if !known.contains(&format!("doc:{id}")) {
            return Err("ref_not_returned");
        }
        let task = self
            .store
            .task_by_id(id)
            .map_err(|_| "unavailable")?
            .ok_or("not_found")?;
        if task_is_initial_read(&task) || (task.kind != INVOICE && task.kind != EMAIL) {
            return Err("not_found");
        }
        Ok(task)
    }

    fn has_attempt(&self, task: &Task) -> Result<bool, &'static str> {
        let result = task.result.clone().unwrap_or(Value::Null);
        Ok(result.get("attempt").is_some()
            || !self
                .store
                .task_actions(task.id)
                .map_err(|_| "unavailable")?
                .is_empty())
    }

    /// Marcas del texto que saldría (asunto y cuerpo) como las calcula la redacción: los contactos
    /// del perfil del negocio no se marcan, pero solo si el perfil iba en la petición que produjo
    /// el borrador (`profile_fp`).
    pub(crate) fn outgoing_flags(&self, result: &Value, body: &str) -> Vec<&'static str> {
        let profile = result["profile_fp"]
            .is_string()
            .then(|| self.local_profile_text())
            .flatten();
        let outgoing = match result["draft"]["subject"].as_str() {
            Some(subject) => format!("{subject}\n{body}"),
            None => body.to_owned(),
        };
        draft_flags_allowing(&outgoing, profile.as_deref())
    }

    /// Texto del perfil del negocio guardado en este equipo, si lo hay.
    fn local_profile_text(&self) -> Option<String> {
        let (profile, _) = self.store.business_profile().ok().flatten()?;
        let text = crate::profile::compose(&profile);
        (!text.trim().is_empty()).then_some(text)
    }

    /// Datos de OTROS documentos que aparecen en el texto de un borrador propuesto (D266): números
    /// de factura, nombres de archivo y nombres de proveedor o remitente de otras tareas del
    /// índice local que no están en la tarea del correo ni en su hilo. Sin IA. Cada elemento es
    /// `{"kind": "invoice_number" | "file_name" | "party", "value": <texto del índice>}`; los
    /// términos de menos de `MIN_OTHER_CHARS` caracteres no cuentan y un importe suelto tampoco.
    fn other_documents_in(&self, task: &Task, body: &str) -> Result<Vec<Value>, &'static str> {
        let identities = self
            .store
            .index_identities(Some(task.id))
            .map_err(|_| "unavailable")?;
        // Lo que es del correo y de su hilo: su resultado SIN el borrador (el borrador es justo lo
        // que se examina, y el actual no puede dar por buenos sus propios datos).
        let mut own_result = task.result.clone().unwrap_or(Value::Null);
        if let Some(map) = own_result.as_object_mut() {
            map.remove("draft");
            map.remove("drafts");
        }
        let own = fold(&format!("{own_result} {}", task.input_ref));
        let text = fold(body);
        let mut found: Vec<Value> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        for identity in identities {
            let mut candidates: Vec<(&str, String)> = Vec::new();
            if identity.kind == nestadmin_core::SearchKind::Invoice {
                candidates.push(("invoice_number", identity.number.clone()));
                candidates.push(("file_name", identity.title.clone()));
            }
            // Un remitente puede venir como «Nombre <dirección>»: se mira entero, el nombre y la
            // dirección por separado.
            candidates.push(("party", identity.party.clone()));
            if let Some((name, rest)) = identity.party.split_once('<') {
                candidates.push(("party", name.to_owned()));
                candidates.push(("party", rest.trim_end_matches('>').to_owned()));
            }
            for (kind, value) in candidates {
                let value = value.trim();
                let key = fold(value);
                if key.chars().count() < MIN_OTHER_CHARS
                    || !contains_word(&text, &key)
                    || contains_word(&own, &key)
                    || !seen.insert(key)
                {
                    continue;
                }
                found.push(json!({ "kind": kind, "value": value }));
            }
        }
        Ok(found)
    }

    /// Recuentos de reglas activas que envían y que registran: lo que pueden hacer sin revisión.
    fn active_rule_counts(&self) -> Result<(usize, usize), &'static str> {
        let rules = self.store.rules().map_err(|_| "unavailable")?;
        let count = |kind: &str| {
            rules
                .iter()
                .filter(|r| r.active && r.action_kind == kind)
                .count()
        };
        Ok((count(SEND_DRAFT), count(REGISTER_INVOICE)))
    }

    /// Efecto y resumen de una propuesta con los datos ACTUALES del equipo. Es lo que se muestra al
    /// crearla, al leerla y al confirmarla; la huella se calcula sobre ello.
    fn compose(
        &self,
        kind: Kind,
        args: &Value,
        task: Option<&Task>,
    ) -> Result<(Effect, Value), &'static str> {
        if kind.on_task() {
            let task = task.ok_or("not_found")?;
            return self.compose_task(kind, args, task);
        }
        match kind {
            Kind::FetchMail => {
                let view = self.mail_accounts();
                let (send, register) = self.active_rule_counts()?;
                Ok((
                    rules_effect(send, register),
                    json!({
                        "type": "fetch_mail",
                        "accounts": view.accounts.len(),
                        "limit": view.limit,
                        "active_send_rules": send,
                        "active_register_rules": register,
                    }),
                ))
            }
            Kind::MailLimit => Ok((
                Effect::NoEffect,
                json!({
                    "type": "mail_limit",
                    "current": self.mail_accounts().limit,
                    "new": args["limit"],
                }),
            )),
            Kind::RenameAccount => {
                let id = args["account_id"].as_str().unwrap_or_default();
                let view = self.mail_accounts();
                let (index, account) = view
                    .accounts
                    .iter()
                    .enumerate()
                    .find(|(_, a)| a.id == id)
                    .ok_or("account_not_found")?;
                Ok((
                    Effect::NoEffect,
                    json!({
                        "type": "rename_account",
                        "account": format!("acct:{}", index + 1),
                        "provider": serde_json::to_value(account.provider).unwrap_or(Value::Null),
                        "current_label": account.label,
                        "new_label": args["label"],
                    }),
                ))
            }
            Kind::SenderPreference => {
                let (scope, key) = (
                    args["scope"].as_str().unwrap_or_default(),
                    args["key"].as_str().unwrap_or_default(),
                );
                let current = self
                    .store
                    .sender_preferences()
                    .map_err(|_| "unavailable")?
                    .into_iter()
                    .find(|p| p.scope == scope && p.key == key)
                    .map(|p| p.route);
                Ok((
                    Effect::NoEffect,
                    json!({
                        "type": "sender_preference",
                        "scope": scope,
                        "key": key,
                        "current": current,
                        "new": args["route"],
                    }),
                ))
            }
            Kind::AcceptSuggestion | Kind::DiscardSuggestion => self.compose_suggestion(kind, args),
            Kind::CreateRule => self.compose_create_rule(args),
            Kind::EditRule => self.compose_edit_rule(args),
            Kind::Import => {
                let (_, register) = self.active_rule_counts()?;
                Ok((
                    Effect::Import,
                    json!({ "type": "import", "active_register_rules": register }),
                ))
            }
            _ => Err("unknown_tool"),
        }
    }

    /// Cuántos casos anteriores habría cubierto una regla y los solapamientos que crearía, a partir
    /// de la simulación (solo lectura).
    fn coverage_of(sim: &Simulation) -> Value {
        let review: usize = sim.would_review.values().sum();
        json!({
            "considered": sim.considered,
            "covered": sim.would_automate + sim.overlaps + review,
            "would_automate": sim.would_automate,
        })
    }

    fn overlaps_of(&self, sim: &Simulation, own: Option<&str>) -> Result<Value, &'static str> {
        let names: HashMap<String, String> = self
            .rule_views()
            .map_err(|_| "unavailable")?
            .into_iter()
            .map(|v| (v.rule.id.clone(), v.name.clone()))
            .collect();
        let items: Vec<Value> = sim
            .findings
            .iter()
            .filter_map(|f| {
                let others: Vec<Value> = f
                    .rules
                    .iter()
                    .filter(|r| r.id != SIMULATED && Some(r.id.as_str()) != own)
                    .map(|r| {
                        json!({
                            "id": r.id,
                            "name": names.get(&r.id).cloned().unwrap_or_else(|| r.id.clone()),
                            "active": r.active,
                        })
                    })
                    .collect();
                (!others.is_empty()).then(|| {
                    json!({
                        "kind": serde_json::to_value(f.kind).unwrap_or(Value::Null),
                        "rules": others,
                    })
                })
            })
            .collect();
        Ok(Value::Array(items))
    }

    fn simulate_for_card(
        &self,
        action_kind: &str,
        conditions: &Value,
        exclude: Option<&str>,
    ) -> Result<Simulation, &'static str> {
        self.simulate_rule(action_kind, conditions, None, false, exclude)
            .map_err(|e| match e {
                AppError::Invalid(_) => "invalid_conditions",
                _ => "unavailable",
            })
    }

    /// Aceptar o descartar una sugerencia: la sugerencia tiene que seguir vigente y ser la que se
    /// mostró (misma huella y mismo `shown`).
    fn compose_suggestion(
        &self,
        kind: Kind,
        args: &Value,
    ) -> Result<(Effect, Value), &'static str> {
        let fp = args["fingerprint"].as_str().unwrap_or_default();
        let shown = args["shown"].as_str().unwrap_or_default();
        let all = self.rule_suggestions().map_err(|_| "unavailable")?;
        let Some(c) = all.candidates.iter().find(|c| c.fingerprint == fp) else {
            let rows = self
                .store
                .rule_suggestion_rows()
                .map_err(|_| "unavailable")?;
            return Err(
                match rows
                    .iter()
                    .find(|r| r.fingerprint == fp)
                    .map(|r| r.status.as_str())
                {
                    Some("accepted") => "already_accepted",
                    Some("discarded") => "already_discarded",
                    _ => "suggestion_gone",
                },
            );
        };
        if c.shown != shown {
            return Err("suggestion_changed");
        }
        let accept = kind == Kind::AcceptSuggestion;
        if accept && c.evidence.needs_limit {
            return Err("needs_limit");
        }
        let sim = self.simulate_for_card(&c.action_kind, &c.conditions, None)?;
        let basis = basis_of(&json!([kind.as_str(), fp, shown]));
        Ok((
            if accept {
                Effect::RuleInactive
            } else {
                Effect::NoEffect
            },
            json!({
                "type": kind.as_str(),
                "action": c.action_kind,
                "name": c.name,
                "conditions": c.conditions,
                "evidence": {
                    "count": c.evidence.count,
                    "threshold": c.evidence.threshold,
                    "approved": c.evidence.approved,
                    "seen": c.evidence.seen,
                    "learned": c.evidence.learned,
                    "origin": c.evidence.origin,
                },
                "coverage": Self::coverage_of(&sim),
                "overlaps": self.overlaps_of(&sim, None)?,
                "stays_inactive": accept,
                "basis": basis,
            }),
        ))
    }

    fn compose_create_rule(&self, args: &Value) -> Result<(Effect, Value), &'static str> {
        let action = args["action"].as_str().unwrap_or_default();
        let conditions = &args["conditions"];
        check_rule(action, conditions, false).map_err(|_| "invalid_conditions")?;
        let sim = self.simulate_for_card(action, conditions, None)?;
        let basis = basis_of(&json!(["create_rule", action, conditions, args["name"]]));
        Ok((
            Effect::RuleInactive,
            json!({
                "type": "create_rule",
                "action": action,
                "name": args["name"],
                "conditions": conditions,
                "coverage": Self::coverage_of(&sim),
                "overlaps": self.overlaps_of(&sim, None)?,
                "stays_inactive": true,
                "basis": basis,
            }),
        ))
    }

    fn compose_edit_rule(&self, args: &Value) -> Result<(Effect, Value), &'static str> {
        let id = args["rule_id"].as_str().unwrap_or_default();
        let conditions = &args["conditions"];
        let rule = self
            .store
            .rule(id)
            .map_err(|_| "unavailable")?
            .ok_or("rule_not_found")?;
        let meta = self.store.rule_meta(id).map_err(|_| "unavailable")?;
        if meta.as_ref().is_some_and(|m| m.archived_at.is_some()) {
            return Err("rule_archived");
        }
        if meta
            .as_ref()
            .is_some_and(|m| m.allow_any_sender_ack_at.is_some())
        {
            return Err("advanced_validation");
        }
        if meta
            .as_ref()
            .and_then(|m| m.origin.as_ref())
            .is_some_and(is_learned_origin)
        {
            return Err("learned_rule");
        }
        check_rule(&rule.action_kind, conditions, false).map_err(|_| "invalid_conditions")?;
        if rule.conditions == *conditions {
            return Err("no_change");
        }
        let stays_active = rule.active && edit_keeps_active(&rule, false, conditions, false);
        let sim = self.simulate_for_card(&rule.action_kind, conditions, Some(id))?;
        let name = meta.and_then(|m| m.name).unwrap_or_else(|| rule.id.clone());
        let basis = basis_of(&json!([
            "edit_rule",
            rule.id,
            rule.version,
            rule.conditions,
            rule.active,
            conditions
        ]));
        Ok((
            Effect::RuleEdited,
            json!({
                "type": "edit_rule",
                "rule_id": rule.id,
                "rule_version": rule.version,
                "name": name,
                "action": rule.action_kind,
                "before": rule.conditions,
                "after": conditions,
                "was_active": rule.active,
                "stays_active": stays_active,
                "coverage": Self::coverage_of(&sim),
                "overlaps": self.overlaps_of(&sim, Some(id))?,
                "basis": basis,
            }),
        ))
    }

    /// Efecto y resumen de las seis propuestas que actúan sobre una tarea.
    fn compose_task(
        &self,
        kind: Kind,
        args: &Value,
        task: &Task,
    ) -> Result<(Effect, Value), &'static str> {
        let r = task.result.clone().unwrap_or(Value::Null);
        Ok(match kind {
            Kind::Approve => {
                if task.kind == INVOICE {
                    (effect_of(task), invoice_summary(task))
                } else {
                    let body = r["draft"]["body"].as_str().unwrap_or_default();
                    let flags = self.outgoing_flags(&r, body);
                    let other = self.other_documents_in(task, body)?;
                    (effect_of(task), email_summary(task, &flags, &other))
                }
            }
            Kind::Reject => (
                Effect::NoEffect,
                json!({ "type": "reject", "task": task_block(task) }),
            ),
            Kind::Retry => {
                let (send, register) = self.active_rule_counts()?;
                (
                    rules_effect(send, register),
                    json!({
                        "type": "retry",
                        "task": task_block(task),
                        "error_code": task.error.as_ref().map(|e| e.code.clone()),
                        "active_send_rules": send,
                        "active_register_rules": register,
                    }),
                )
            }
            Kind::Archive => (
                Effect::NoEffect,
                json!({ "type": "archive", "task": task_block(task) }),
            ),
            Kind::ConfirmNotSent => (
                Effect::NoEffect,
                json!({
                    "type": "confirm_not_sent",
                    "task": task_block(task),
                    "recovery": recovery_code(&r),
                }),
            ),
            Kind::EditDraft => {
                let new_body = args["body"].as_str().unwrap_or_default();
                let found = self.other_documents_in(task, new_body)?;
                let shown: Vec<&Value> = found.iter().take(MAX_OTHER_SHOWN).collect();
                (
                    Effect::NoEffect,
                    json!({
                        "type": "edit_draft",
                        "version": version_of(task),
                        "task": task_block(task),
                        "to": r["draft"]["to"],
                        "subject": r["draft"]["subject"],
                        "current_body": r["draft"]["body"],
                        "new_body": new_body,
                        "draft_flags": self.outgoing_flags(&r, new_body),
                        "other_documents": shown,
                        "other_documents_total": found.len(),
                    }),
                )
            }
            _ => return Err("unknown_tool"),
        })
    }

    /// Valida y compone una propuesta con datos del equipo. Los códigos de rechazo son cerrados.
    fn build_proposal(
        &self,
        kind: Kind,
        args: &Map<String, Value>,
        known: &HashSet<String>,
        targets: &BTreeMap<String, Value>,
    ) -> Result<Built, &'static str> {
        let (task, args_json) = if kind.on_task() {
            let task = self.task_of(args, known)?;
            let args_json = self.validate_task_proposal(kind, args, &task)?;
            (Some(task), args_json)
        } else {
            (
                None,
                self.validate_other_proposal(kind, args, known, targets)?,
            )
        };
        let (effect, summary) = self.compose(kind, &args_json, task.as_ref())?;
        let fp = fingerprint(kind, task.as_ref(), effect, &summary);
        // Editar una regla guarda su versión vista (la lee la aplicación, nunca el modelo).
        let seen_version = match kind {
            Kind::EditRule => summary["rule_version"].as_i64(),
            _ => task.as_ref().map(version_of),
        };
        Ok(Built {
            task_id: task.as_ref().map(|t| t.id),
            seen_version,
            args: args_json,
            effect,
            summary,
            fp,
        })
    }

    /// Argumentos validados de las cuatro propuestas que no actúan sobre una tarea.
    fn validate_other_proposal(
        &self,
        kind: Kind,
        args: &Map<String, Value>,
        known: &HashSet<String>,
        targets: &BTreeMap<String, Value>,
    ) -> Result<Value, &'static str> {
        match kind {
            Kind::FetchMail | Kind::Import => Ok(json!({})),
            Kind::AcceptSuggestion | Kind::DiscardSuggestion => {
                let reference = args
                    .get("suggestion")
                    .and_then(Value::as_str)
                    .filter(|r| suggestion_index(r).is_some())
                    .ok_or("bad_ref")?;
                // Solo vale una referencia que `list_suggestions` devolvió en esta conversación.
                if !known.contains(reference) {
                    return Err("ref_not_returned");
                }
                let target = targets.get(reference).ok_or("ref_not_returned")?;
                Ok(json!({
                    "suggestion": reference,
                    "fingerprint": target["fp"],
                    "shown": target["shown"],
                }))
            }
            Kind::CreateRule => {
                let action = args
                    .get("action")
                    .and_then(Value::as_str)
                    .ok_or("bad_value")?;
                // D265: el modelo no da el nombre; el catálogo ya rechaza `name` y aquí se vuelve a
                // cerrar por si algo llamara a esta función sin pasar por él.
                if args.contains_key("name") {
                    return Err("bad_value");
                }
                let conditions = rule_conditions(action, args)?;
                let name = default_rule_name(action, &conditions);
                Ok(json!({ "action": action, "name": name, "conditions": conditions }))
            }
            Kind::EditRule => {
                let reference = args
                    .get("rule")
                    .and_then(Value::as_str)
                    .filter(|r| rule_index(r).is_some())
                    .ok_or("bad_ref")?;
                if !known.contains(reference) {
                    return Err("ref_not_returned");
                }
                let id = targets
                    .get(reference)
                    .and_then(|t| t["rule_id"].as_str())
                    .ok_or("ref_not_returned")?;
                let rule = self
                    .store
                    .rule(id)
                    .map_err(|_| "unavailable")?
                    .ok_or("rule_not_found")?;
                let conditions = rule_conditions(&rule.action_kind, args)?;
                Ok(json!({ "rule_id": id, "rule_ref": reference, "conditions": conditions }))
            }
            Kind::MailLimit => {
                let limit = args
                    .get("limit")
                    .and_then(Value::as_i64)
                    .filter(|n| (1..=500).contains(n))
                    .ok_or("bad_value")?;
                Ok(json!({ "limit": limit }))
            }
            Kind::RenameAccount => {
                let reference = args
                    .get("account")
                    .and_then(Value::as_str)
                    .ok_or("bad_ref")?;
                let index = account_index(reference).ok_or("bad_ref")?;
                if !known.contains(&format!("acct:{index}")) {
                    return Err("ref_not_returned");
                }
                let raw = args
                    .get("label")
                    .and_then(Value::as_str)
                    .ok_or("bad_value")?;
                if has_invisible(raw) {
                    return Err("invisible_chars");
                }
                let label = Some(raw.trim())
                    .filter(|l| !l.is_empty() && l.chars().count() <= 60)
                    .filter(|l| !l.chars().any(char::is_control) && !carries_web_markup(l))
                    .ok_or("bad_value")?;
                let view = self.mail_accounts();
                let account = view.accounts.get(index - 1).ok_or("account_not_found")?;
                Ok(json!({
                    "account_id": account.id,
                    "account_ref": format!("acct:{index}"),
                    "label": label,
                }))
            }
            Kind::SenderPreference => {
                let get = |k: &str| args.get(k).and_then(Value::as_str).unwrap_or_default();
                if [get("scope"), get("key"), get("route")]
                    .iter()
                    .any(|t| has_invisible(t))
                {
                    return Err("invisible_chars");
                }
                let (scope, key, route) =
                    crate::preferences::validated(get("scope"), get("key"), get("route"))
                        .map_err(|_| "bad_value")?;
                Ok(json!({
                    "scope": scope.as_str(),
                    "key": key,
                    "route": route.as_str(),
                }))
            }
            _ => Err("unknown_tool"),
        }
    }

    /// Argumentos validados de las seis propuestas que actúan sobre una tarea: mismas condiciones
    /// que valida el método de `App` correspondiente y la pantalla de revisión.
    fn validate_task_proposal(
        &self,
        kind: Kind,
        args: &Map<String, Value>,
        task: &Task,
    ) -> Result<Value, &'static str> {
        let r = task.result.clone().unwrap_or(Value::Null);
        let reviewable = task.state == TaskState::AwaitingReview;
        match kind {
            Kind::Approve => {
                if !reviewable {
                    return Err("wrong_state");
                }
                if task.kind == EMAIL {
                    if r["draft"]["body"]
                        .as_str()
                        .is_none_or(|b| b.trim().is_empty())
                    {
                        return Err("no_draft");
                    }
                    if r["draft"]["to"]
                        .as_array()
                        .is_none_or(std::vec::Vec::is_empty)
                    {
                        return Err("no_recipient");
                    }
                }
                if recovery_code(&r) == Some("needs_confirmation") {
                    return Err("needs_confirmation");
                }
                Ok(json!({}))
            }
            Kind::Reject => {
                if !reviewable && task.state != TaskState::Failed {
                    return Err("wrong_state");
                }
                Ok(json!({}))
            }
            Kind::Retry => {
                if task.state != TaskState::Failed {
                    return Err("wrong_state");
                }
                // Los intentos hasta ahora: tras un corte, un intento posterior prueba que se hizo.
                Ok(json!({ "seen_attempts": task.attempts }))
            }
            Kind::Archive => {
                if task.kind != EMAIL {
                    return Err("not_an_email");
                }
                if !reviewable {
                    return Err("wrong_state");
                }
                if self.has_attempt(task)? {
                    return Err("already_attempted");
                }
                Ok(json!({}))
            }
            Kind::ConfirmNotSent => {
                if !reviewable {
                    return Err("wrong_state");
                }
                if !self.has_attempt(task)? {
                    return Err("no_attempt");
                }
                Ok(json!({}))
            }
            Kind::EditDraft => {
                if task.kind != EMAIL {
                    return Err("not_an_email");
                }
                if !reviewable {
                    return Err("wrong_state");
                }
                let Some(current) = r["draft"]["body"].as_str() else {
                    return Err("no_draft");
                };
                let raw = args
                    .get("body")
                    .and_then(Value::as_str)
                    .ok_or("bad_value")?;
                // Caracteres de formato o bidireccionales: la tarjeta se leería distinto de lo
                // que se guarda.
                if has_invisible(raw) {
                    return Err("invisible_chars");
                }
                let body = Some(raw.trim())
                    .filter(|b| !b.is_empty())
                    .ok_or("bad_value")?;
                if body.chars().count() > nestadmin_ai::assistant::MAX_DRAFT_CHARS {
                    return Err("bad_value");
                }
                // El borrador es el único texto del modelo que llega a una tarjeta (D160): lo que
                // filtra instrucciones se rechaza siempre; un contacto, IBAN o enlace solo si es
                // NUEVO (ni del perfil del negocio ni del borrador actual). Las demás marcas se
                // muestran en la tarjeta.
                if draft_leaks_internals(body) {
                    return Err("draft_leak");
                }
                if introduces_new_contact(body, current, self.local_profile_text().as_deref()) {
                    return Err("draft_flagged");
                }
                if body == current.trim() {
                    return Err("no_change");
                }
                Ok(json!({ "body": body }))
            }
            _ => Err("unknown_tool"),
        }
    }

    // ---- Tarjetas ----

    fn card_of(&self, row: &ProposalRow) -> ProposalCard {
        ProposalCard {
            id: row.id,
            kind: row.kind.clone(),
            status: row.status.clone(),
            task_id: row.task_id,
            seen_version: row.seen_version,
            created_at: row.created_at,
            expires_at: row.expires_at,
            effect: Effect::parse(row.card["effect"].as_str().unwrap_or("none")),
            summary: row.card["summary"].clone(),
            receipt: row
                .receipt
                .clone()
                .and_then(|r| serde_json::from_value(r).ok()),
        }
    }

    /// Versión actual de la tarea de una propuesta; `None` si ya no existe.
    fn current_version(&self, task_id: i64) -> AppResult<Option<i64>> {
        Ok(self.store.task_by_id(task_id)?.map(|t| version_of(&t)))
    }

    /// Efecto, resumen y huella de la propuesta con los datos ACTUALES; `None` si ya no se puede
    /// componer (la tarea o la cuenta ya no existen).
    fn current_view(&self, row: &ProposalRow) -> AppResult<Option<(Effect, Value, String)>> {
        let Some(kind) = Kind::parse(&row.kind) else {
            return Ok(None);
        };
        let task = match row.task_id {
            Some(id) => match self.store.task_by_id(id)? {
                Some(task) => Some(task),
                None => return Ok(None),
            },
            None => None,
        };
        match self.compose(kind, &row.args, task.as_ref()) {
            Ok((effect, summary)) => {
                let fp = fingerprint(kind, task.as_ref(), effect, &summary);
                Ok(Some((effect, summary, fp)))
            }
            Err("unavailable") => Err(AppError::Invalid(
                "no se pudo comprobar el estado actual de la propuesta".into(),
            )),
            Err(_) => Ok(None),
        }
    }

    /// Error de una propuesta obsoleta: con versión distinta, el de siempre; si la versión es la
    /// misma pero cambió otra cosa (estado, hora, importes…), un código propio.
    fn stale_error(&self, row: &ProposalRow) -> AppResult<AppError> {
        let seen = row.seen_version.unwrap_or_default();
        if let Some(task_id) = row.task_id {
            let current = self.current_version(task_id)?.unwrap_or_default();
            if current != seen {
                return Ok(AppError::StaleVersion {
                    approved: seen,
                    current,
                });
            }
        }
        Ok(AppError::Proposal {
            code: "stale",
            message: "lo que mostraba la tarjeta ya no es lo que hay; pídala de nuevo".into(),
        })
    }

    fn audit_proposal(&self, row: &ProposalRow, status: &str, code: Option<&str>) {
        let effect = row.card["effect"].as_str().unwrap_or("none");
        // Sin texto del modelo ni de documentos.
        let _ = self.store.proposal_audit(
            row.task_id,
            status,
            &json!({ "kind": row.kind, "task_id": row.task_id, "effect": effect, "code": code }),
        );
    }

    /// Mueve una propuesta a un estado final si sigue en uno de `from`, y lo deja en el historial.
    fn settle_status(
        &self,
        row: &ProposalRow,
        from: &[&str],
        to: &str,
        code: Option<&str>,
    ) -> bool {
        let changed = self
            .store
            .proposal_transition(row.id, from, to, None)
            .unwrap_or(false);
        if changed {
            self.audit_proposal(row, to, code);
        }
        changed
    }

    /// Recalcula `expired` y `stale` (y los guarda). Una propuesta `pending` solo sigue así si la
    /// huella de lo que hay ahora coincide con la que se mostró; entonces su tarjeta lleva el
    /// resumen ACTUAL recompuesto. Una `executing` caduca como las demás si no consta como hecha.
    fn refreshed(&self, mut row: ProposalRow) -> AppResult<ProposalRow> {
        let new_status = match row.status.as_str() {
            "pending" => {
                if now_ms() >= row.expires_at {
                    Some("expired")
                } else {
                    match self.current_view(&row)? {
                        Some((effect, summary, fp)) if row.seen_fp.as_deref() == Some(&fp) => {
                            row.card = json!({ "effect": effect.as_str(), "summary": summary });
                            None
                        }
                        _ => Some("stale"),
                    }
                }
            }
            "executing" if now_ms() >= row.expires_at && self.proposal_done(&row)?.is_none() => {
                Some("expired")
            }
            _ => None,
        };
        if let Some(status) = new_status {
            self.settle_status(&row, &[row.status.as_str()], status, None);
            return Ok(self.store.proposal(row.id)?.unwrap_or(row));
        }
        Ok(row)
    }

    /// Las tarjetas de una conversación con su estado actual (recalcula `stale` y `expired`).
    pub fn proposals(&self, conversation_id: i64) -> AppResult<Vec<ProposalCard>> {
        let mut cards = Vec::new();
        for row in self.store.proposals_of(conversation_id)? {
            cards.push(self.card_of(&self.refreshed(row)?));
        }
        Ok(cards)
    }

    /// Las tarjetas de estas propuestas, en el orden pedido, con su estado actual.
    pub(crate) fn proposal_cards(&self, ids: &[i64]) -> AppResult<Vec<ProposalCard>> {
        let mut cards = Vec::new();
        for row in self.store.proposals_by_ids(ids)? {
            cards.push(self.card_of(&self.refreshed(row)?));
        }
        Ok(cards)
    }

    // ---- Descartar ----

    /// La persona descarta la tarjeta: la propuesta pasa a `rejected` y ya no se puede confirmar.
    /// Descartar dos veces no hace nada; una ya confirmada no se descarta. Una `executing` se
    /// resuelve antes: si la acción consta como hecha pasa a `confirmed` (y se devuelve así, sin
    /// repetir nada); si no, queda `rejected`.
    pub fn proposal_dismiss(&mut self, id: i64) -> AppResult<ProposalCard> {
        // Con el diálogo de importar abierto no se descarta: el resultado de lo que se importe tiene
        // que constar en esta propuesta.
        if self
            .assistant
            .importing
            .lock()
            .map_or(true, |set| set.contains(&id))
        {
            return Err(AppError::Proposal {
                code: "busy",
                message: "la propuesta se está confirmando: espere a que termine".into(),
            });
        }
        let row = self.store.proposal(id)?.ok_or_else(not_found)?;
        match row.status.as_str() {
            "rejected" => {}
            "confirmed" => {
                return Err(AppError::Proposal {
                    code: "confirmed",
                    message: "la propuesta ya se confirmó y no se puede descartar".into(),
                })
            }
            "executing" => match self.proposal_done(&row)? {
                Some(result) => {
                    self.record_outcome(&row, Run::done(result), true);
                }
                None => {
                    self.settle_status(&row, &["executing"], "rejected", Some("not_done"));
                }
            },
            _ => {
                self.settle_status(
                    &row,
                    &["pending", "expired", "stale", "failed"],
                    "rejected",
                    None,
                );
            }
        }
        let row = self.store.proposal(id)?.ok_or_else(not_found)?;
        Ok(self.card_of(&row))
    }

    // ---- Confirmar (solo desde el botón de la tarjeta) ----

    /// Confirma una propuesta: ejecuta el mismo método de `App` que la pantalla, con origen `chat`
    /// en el historial, y guarda el recibo.
    ///
    /// - `confirmed`: devuelve el recibo original sin ejecutar nada.
    /// - `rejected`, `expired` (o pasada la caducidad), `stale` y `failed`: error con su código.
    /// - `pending`: compara la huella con lo que hay ahora (si difiere, `stale` y no se ejecuta),
    ///   pasa a `executing`, ejecuta y anota el resultado REAL: `confirmed` si la acción consta
    ///   hecha y `failed` (con recibo `not_done`) si no. `AmountAckRequired` la deja `pending`; una
    ///   precondición que ya no se cumple (estado, tarea inexistente…) la deja `failed`; un fallo
    ///   del conector, la base de datos o el disco, cuyo efecto no se sabe, la deja `executing`.
    /// - `executing` (un corte entre ejecutar y anotar): averigua si la acción ya se hizo y, si es
    ///   así, la da por confirmada sin repetirla; si no, la reintenta por el camino idempotente o
    ///   la deja `failed` si la precondición ya no se cumple.
    pub fn proposal_confirm(
        &mut self,
        id: i64,
        amounts_checked: bool,
        other_documents_checked: bool,
    ) -> AppResult<ProposalReceipt> {
        self.with_chat_origin(|app| {
            app.confirm_proposal(id, amounts_checked, other_documents_checked)
        })
    }

    /// Ejecuta `f` con el origen `chat` en el historial. El origen se restablece SIEMPRE al salir:
    /// por retorno, por error o por pánico (guardián RAII); si no se puede restablecer, es un error
    /// explícito y no se continúa en silencio. No lo usa nada más que la confirmación (y las
    /// pruebas).
    #[doc(hidden)]
    pub fn with_chat_origin<R>(
        &mut self,
        f: impl FnOnce(&mut Self) -> AppResult<R>,
    ) -> AppResult<R> {
        self.store.set_audit_origin(Some(CHAT_ORIGIN))?;
        let scope = OriginScope(self);
        let result = f(scope.0);
        // El restablecimiento explícito permite informar; el `Drop` cubre el pánico.
        let reset = scope.0.store.clear_audit_origin();
        match (result, reset) {
            (result, Ok(())) => result,
            (Ok(_), Err(_)) => Err(AppError::Proposal {
                code: "origin_not_reset",
                message: "no se pudo restablecer el origen del historial".into(),
            }),
            (Err(error), Err(_)) => Err(error),
        }
    }

    fn confirm_proposal(
        &mut self,
        id: i64,
        amounts_checked: bool,
        other_documents_checked: bool,
    ) -> AppResult<ProposalReceipt> {
        let row = self.store.proposal(id)?.ok_or_else(not_found)?;
        match row.status.as_str() {
            "confirmed" => {
                return row
                    .receipt
                    .clone()
                    .and_then(|r| serde_json::from_value(r).ok())
                    .ok_or_else(|| AppError::Proposal {
                        code: "failed",
                        message: "la propuesta consta confirmada pero falta su recibo".into(),
                    })
            }
            "rejected" => {
                return Err(AppError::Proposal {
                    code: "rejected",
                    message: "la propuesta se descartó y ya no se puede confirmar".into(),
                })
            }
            "expired" => return Err(expired()),
            "failed" => {
                return Err(AppError::Proposal {
                    code: "failed",
                    message: "la propuesta falló y no se puede confirmar; pídala de nuevo".into(),
                })
            }
            "stale" => return Err(self.stale_error(&row)?),
            _ => {}
        }
        // Importar necesita el diálogo del sistema, que solo abre la ventana: el flujo es
        // `proposal_import_begin` / `proposal_import_finish`. Una `executing` no se repite sola.
        if row.kind == Kind::Import.as_str() {
            if row.status == "executing" {
                return Err(self.import_interrupted(&row));
            }
            return Err(AppError::Proposal {
                code: "import_needs_dialog",
                message: "importar exige elegir los archivos en el diálogo del sistema".into(),
            });
        }
        let executing = row.status == "executing";
        if executing {
            // Un corte entre ejecutar y anotar: no se vuelve a ejecutar a ciegas.
            if let Some(result) = self.proposal_done(&row)? {
                return Ok(self.record_outcome(&row, Run::done(result), true));
            }
            if now_ms() >= row.expires_at {
                self.settle_status(&row, &["executing"], "expired", None);
                return Err(expired());
            }
            // Una regla o sugerencia cambiada desde entonces (por el panel) ya no es esta.
            if Kind::parse(&row.kind).is_some_and(Kind::on_rules) {
                let same = self
                    .current_view(&row)?
                    .is_some_and(|(_, _, fp)| row.seen_fp.as_deref() == Some(&fp));
                if !same {
                    self.settle_status(&row, &["executing"], "stale", None);
                    return Err(self.stale_error(&row)?);
                }
            }
            // La acción no consta hecha: si la tarea cambió de versión, ya no es esta.
            if let Some(task_id) = row.task_id {
                if self.current_version(task_id)? != Some(row.seen_version.unwrap_or_default()) {
                    self.settle_status(&row, &["executing"], "stale", None);
                    return Err(self.stale_error(&row)?);
                }
            }
        } else {
            if now_ms() >= row.expires_at {
                self.settle_status(&row, &["pending"], "expired", None);
                return Err(expired());
            }
            // Lo que hay ahora tiene que ser lo que mostró la tarjeta: estado, versión, hora de la
            // tarea y resumen. Si no, la tarjeta ya no describe el efecto y no se ejecuta.
            let view = self.current_view(&row)?;
            let same = view
                .as_ref()
                .is_some_and(|(_, _, fp)| row.seen_fp.as_deref() == Some(fp));
            if !same {
                self.settle_status(&row, &["pending"], "stale", None);
                return Err(self.stale_error(&row)?);
            }
            // D266: un borrador con datos de otros documentos exige la casilla de «lo he
            // revisado». La propuesta sigue pendiente para volver a pulsar con ella marcada.
            let other_documents = view
                .as_ref()
                .and_then(|(_, summary, _)| summary["other_documents_total"].as_u64())
                .unwrap_or(0);
            let sends_text = row.kind == Kind::EditDraft.as_str()
                || (row.kind == Kind::Approve.as_str() && row.task_id.is_some());
            if sends_text && other_documents > 0 && !other_documents_checked {
                return Err(AppError::Proposal {
                    code: "other_documents_ack_required",
                    message: "el borrador contiene datos de otros documentos: confirme antes que ha revisado que deben ir en la respuesta".into(),
                });
            }
            if !self
                .store
                .proposal_transition(row.id, &["pending"], "executing", None)?
            {
                return Err(AppError::Proposal {
                    code: "busy",
                    message: "la propuesta ya se está confirmando".into(),
                });
            }
        }
        match self.run_proposal(&row, amounts_checked) {
            Ok(run) => Ok(self.record_outcome(&row, run, false)),
            Err(error) => {
                match &error {
                    // Falta confirmar los importes: la persona puede marcarlo y volver a pulsar.
                    AppError::AmountAckRequired => {
                        let _ =
                            self.store
                                .proposal_transition(row.id, &["executing"], "pending", None);
                    }
                    AppError::StaleVersion { .. } => {
                        self.settle_status(&row, &["executing"], "stale", None);
                    }
                    // La precondición ya no se cumple (estado, borrador, tarea inexistente…): la
                    // acción no se hizo y no se puede hacer.
                    AppError::Invalid(_)
                    | AppError::Core(
                        CoreError::InvalidTransition { .. } | CoreError::TaskNotFound(_),
                    ) => {
                        self.settle_status(&row, &["executing"], "failed", Some("invalid"));
                    }
                    AppError::Connector(e) if e.code == nestadmin_simulators::codes::NOT_FOUND => {
                        self.settle_status(&row, &["executing"], "failed", Some("not_found"));
                    }
                    // Conector, base de datos o disco: no se sabe si llegó a hacerse. Queda
                    // `executing` y la siguiente confirmación (o descartarla) lo comprueba.
                    _ => {}
                }
                Err(error)
            }
        }
    }

    /// Guarda el recibo con el resultado real y lo deja en el historial. Si la acción se hizo, marca
    /// `confirmed` y da por obsoletas las demás propuestas pendientes de la misma tarea; si no,
    /// marca `failed` con el motivo y no toca las demás (se invalidan solas si la tarea cambió).
    fn record_outcome(&self, row: &ProposalRow, run: Run, reconciled: bool) -> ProposalReceipt {
        let task_state = row
            .task_id
            .and_then(|id| self.store.task_by_id(id).ok().flatten())
            .map(|t| t.state.as_str().to_owned());
        let receipt = ProposalReceipt {
            proposal_id: row.id,
            kind: row.kind.clone(),
            task_id: row.task_id,
            task_state,
            effect: Effect::parse(row.card["effect"].as_str().unwrap_or("none")),
            confirmed_at: now_ms(),
            outcome: run.outcome,
            reason: run.reason.map(str::to_owned),
            reconciled,
            rule_id: run.result["rule_id"].as_str().map(str::to_owned),
            result: run.result,
        };
        let stored = serde_json::to_value(&receipt).unwrap_or(Value::Null);
        let done = run.outcome == Outcome::Done;
        let target = if done { "confirmed" } else { "failed" };
        if self
            .store
            .proposal_transition(row.id, &["executing", "pending"], target, Some(&stored))
            .unwrap_or(false)
        {
            let code = if done {
                reconciled.then_some("reconciled")
            } else {
                run.reason
            };
            self.audit_proposal(row, target, code);
        }
        if done {
            if let Some(task_id) = row.task_id {
                if let Ok(open) = self.store.proposals_open_for_task(task_id) {
                    for sibling in open.iter().filter(|s| s.id != row.id) {
                        self.settle_status(sibling, &["pending"], "stale", None);
                    }
                }
            }
        }
        receipt
    }

    /// Ejecuta la acción propuesta con el mismo método de `App` que la pantalla.
    fn run_proposal(&mut self, row: &ProposalRow, amounts_checked: bool) -> AppResult<Run> {
        let kind = Kind::parse(&row.kind)
            .ok_or_else(|| AppError::Invalid("tipo de propuesta desconocido".into()))?;
        let task_id = row.task_id.unwrap_or_default();
        let seen = row.seen_version.unwrap_or(1);
        let arg = |k: &str| row.args[k].as_str().unwrap_or_default().to_owned();
        match kind {
            Kind::Approve => {
                let task = self.approve_reviewed(task_id, seen, amounts_checked)?;
                // Aprobar no basta: solo vale si la acción con efecto consta realizada.
                let result = json!({ "version": seen });
                Ok(match task.state {
                    TaskState::Completed => Run::done(result),
                    state => Run {
                        result,
                        outcome: Outcome::NotDone,
                        reason: Some(match state {
                            TaskState::Failed => "action_failed",
                            TaskState::AwaitingReview => "returned_to_review",
                            _ => "not_completed",
                        }),
                    },
                })
            }
            Kind::EditDraft => {
                let task = self.edit_draft(task_id, &arg("body"))?;
                Ok(Run::done(json!({ "new_version": version_of(&task) })))
            }
            Kind::Reject => {
                self.reject(task_id)?;
                Ok(Run::done(json!({})))
            }
            Kind::Retry => {
                self.retry(task_id)?;
                Ok(Run::done(json!({})))
            }
            Kind::Archive => {
                self.archive(task_id)?;
                Ok(Run::done(json!({})))
            }
            Kind::ConfirmNotSent => {
                let task = self.confirm_not_sent(task_id)?;
                Ok(Run::done(json!({ "new_version": version_of(&task) })))
            }
            Kind::FetchMail => {
                let report = self.fetch_mail()?;
                Ok(Run::done(
                    json!({ "added": report.added, "limit": report.limit }),
                ))
            }
            Kind::MailLimit => {
                let limit = usize::try_from(row.args["limit"].as_i64().unwrap_or_default())
                    .map_err(|_| AppError::Invalid("límite no válido".into()))?;
                self.set_mail_limit(limit)?;
                Ok(Run::done(json!({ "limit": limit })))
            }
            Kind::RenameAccount => {
                let view = self.rename_mail_account(&arg("account_id"), &arg("label"))?;
                Ok(Run::done(json!({ "label": view.label })))
            }
            Kind::SenderPreference => {
                let change =
                    self.set_sender_preference(&arg("scope"), &arg("key"), &arg("route"))?;
                Ok(Run::done(json!({
                    "changed": change.changed,
                    "route": change.preference.route,
                    "warnings": change.warnings.len(),
                })))
            }
            Kind::AcceptSuggestion => {
                let view = self.accept_suggestion(&arg("fingerprint"), &arg("shown"), None)?;
                Ok(Run::done(rule_facts(&view)))
            }
            Kind::DiscardSuggestion => {
                self.discard_suggestion(&arg("fingerprint"))?;
                Ok(Run::done(json!({})))
            }
            Kind::CreateRule => {
                let id = chat_rule_id(&row.idem_key);
                let action = arg("action");
                let conditions = row.args["conditions"].clone();
                // Recuperarse de un corte: la regla de esta propuesta ya existe, no se crea otra.
                // Solo cuenta si es la que esta propuesta escribió; otra regla con ese
                // identificador (aunque sea igual) no se da por hecha.
                if self.store.rule(&id)?.is_some() {
                    if let Some(facts) = self.created_by_proposal(row)? {
                        return Ok(Run::done(facts));
                    }
                    return Err(AppError::Invalid(
                        "el identificador de la regla de esta propuesta ya lo usa otra regla"
                            .into(),
                    ));
                }
                let view = self.create_rule_full(NewRule {
                    id: Some(&id),
                    name: row.args["name"].as_str(),
                    action_kind: &action,
                    conditions: &conditions,
                    allow_any_sender: false,
                })?;
                Ok(Run::done(rule_facts(&view)))
            }
            Kind::EditRule => {
                let view = self.update_rule(&arg("rule_id"), &row.args["conditions"])?;
                let mut facts = rule_facts(&view);
                facts["new_version"] = json!(view.rule.version);
                facts["deactivated_by_edit"] = json!(view.deactivated_by_edit);
                Ok(Run::done(facts))
            }
            // Lo hace `proposal_import_finish` tras el diálogo; aquí no se llega nunca.
            Kind::Import => Err(AppError::Proposal {
                code: "import_needs_dialog",
                message: "importar exige elegir los archivos en el diálogo del sistema".into(),
            }),
        }
    }

    /// ¿La acción de una propuesta `executing` ya se hizo? Devuelve sus hechos si es así, sin
    /// ejecutar nada.
    fn proposal_done(&self, row: &ProposalRow) -> AppResult<Option<Value>> {
        let Some(kind) = Kind::parse(&row.kind) else {
            return Ok(None);
        };
        let seen = row.seen_version.unwrap_or(1);
        let task = match row.task_id {
            Some(id) => self.store.task_by_id(id)?,
            None => None,
        };
        Ok(match kind {
            Kind::Approve => {
                // La tabla de acciones con su clave `task-{id}/{kind}/v{version}`.
                task.and_then(|t| {
                    let key = idempotency_key(t.id, action_kind(&t), seen);
                    (self.store.action_status(&key).ok().flatten().as_deref() == Some("confirmed"))
                        .then(|| json!({ "version": seen }))
                })
            }
            Kind::EditDraft => task.and_then(|t| {
                let r = t.result.clone().unwrap_or(Value::Null);
                (version_of(&t) > seen && r["draft"]["body"].as_str() == row.args["body"].as_str())
                    .then(|| json!({ "new_version": version_of(&t) }))
            }),
            Kind::Reject => task.and_then(|t| (t.state == TaskState::Cancelled).then(|| json!({}))),
            // Reintentada: otra vez en cola o en curso, o con un intento posterior al de la
            // propuesta. Una tarea cancelada o aún fallida no cuenta.
            Kind::Retry => task.and_then(|t| {
                let seen_attempts = row.args["seen_attempts"].as_i64().unwrap_or(i64::MAX);
                (matches!(t.state, TaskState::Queued | TaskState::Running)
                    || t.attempts > seen_attempts)
                    .then(|| json!({}))
            }),
            Kind::Archive => task.and_then(|t| {
                let r = t.result.clone().unwrap_or(Value::Null);
                (t.state == TaskState::Completed && r["archived"] == json!(true)).then(|| json!({}))
            }),
            Kind::ConfirmNotSent => task.and_then(|t| {
                (t.state == TaskState::Completed || version_of(&t) > seen)
                    .then(|| json!({ "new_version": version_of(&t) }))
            }),
            Kind::MailLimit => {
                let limit = row.args["limit"].as_u64().unwrap_or_default();
                (u64::try_from(self.mail_accounts().limit).ok() == Some(limit))
                    .then(|| json!({ "limit": limit }))
            }
            Kind::RenameAccount => {
                let id = row.args["account_id"].as_str().unwrap_or_default();
                let label = row.args["label"].as_str().unwrap_or_default();
                self.mail_accounts()
                    .accounts
                    .iter()
                    .find(|a| a.id == id && a.label == label)
                    .map(|a| json!({ "label": a.label }))
            }
            Kind::SenderPreference => {
                let (scope, key, route) = (
                    row.args["scope"].as_str().unwrap_or_default(),
                    row.args["key"].as_str().unwrap_or_default(),
                    row.args["route"].as_str().unwrap_or_default(),
                );
                self.store
                    .sender_preferences()?
                    .into_iter()
                    .any(|p| p.scope == scope && p.key == key && p.route == route)
                    .then(|| json!({ "changed": false, "route": route, "warnings": 0 }))
            }
            // Consultar el correo no deja un estado que comprobar: es idempotente por mensaje.
            Kind::FetchMail => None,
            // Importar no es idempotente por contenido y no deja una marca de esta propuesta.
            Kind::Import => None,
            // Aceptada, y con el origen `chat` en el historial: una sugerencia aceptada por el panel
            // no se atribuye al chat.
            Kind::AcceptSuggestion => {
                let fp = row.args["fingerprint"].as_str().unwrap_or_default();
                if !self.chat_audited("rule_suggestion", "accepted", fp)? {
                    return Ok(None);
                }
                self.store
                    .rule_suggestion_rows()?
                    .into_iter()
                    .find(|r| r.fingerprint == fp && r.status == "accepted")
                    .and_then(|r| r.evidence["rule_id"].as_str().map(str::to_owned))
                    .and_then(|id| self.store.rule(&id).ok().flatten())
                    .map(|rule| json!({ "rule_id": rule.id, "active": rule.active }))
            }
            Kind::DiscardSuggestion => {
                let fp = row.args["fingerprint"].as_str().unwrap_or_default();
                if !self.chat_audited("rule_suggestion", "discarded", fp)? {
                    return Ok(None);
                }
                self.store
                    .rule_suggestion_rows()?
                    .into_iter()
                    .any(|r| r.fingerprint == fp && r.status == "discarded")
                    .then(|| json!({}))
            }
            Kind::CreateRule => self.created_by_proposal(row)?,
            // La regla está en la versión siguiente a la vista, con las condiciones propuestas, y
            // esa versión la escribió el chat: una igual hecha por el panel no cuenta.
            Kind::EditRule => {
                let id = row.args["rule_id"].as_str().unwrap_or_default();
                match self.store.rule(id)? {
                    Some(rule)
                        if rule.version == seen + 1
                            && rule.conditions == row.args["conditions"]
                            && self.chat_audited(
                                "rule",
                                "",
                                &format!("{id} v{} (", rule.version),
                            )? =>
                    {
                        Some(json!({
                            "rule_id": rule.id,
                            "active": rule.active,
                            "new_version": rule.version,
                        }))
                    }
                    _ => None,
                }
            }
        })
    }
}

impl App {
    // ---- Importar (el diálogo lo abre la ventana, no este crate) ----

    /// Tipo de una propuesta, para que la ventana sepa si confirmarla exige el diálogo del sistema.
    pub fn proposal_kind(&self, id: i64) -> AppResult<Option<String>> {
        Ok(self.store.proposal(id)?.map(|row| row.kind))
    }

    /// Una propuesta de importar encontrada en `executing` y que no está en curso en esta sesión:
    /// no se repite sola (repetirla exige elegir de nuevo los archivos) y pasa a `failed`.
    fn import_interrupted(&self, row: &ProposalRow) -> AppError {
        if self
            .assistant
            .importing
            .lock()
            .map_or(true, |set| set.contains(&row.id))
        {
            return AppError::Proposal {
                code: "busy",
                message: "la propuesta ya se está confirmando".into(),
            };
        }
        self.settle_status(row, &["executing"], "failed", Some("import_interrupted"));
        AppError::Proposal {
            code: "failed",
            message: "la importación quedó a medias y no se repite sola: revise las tareas y, si falta algo, pida la propuesta de nuevo".into(),
        }
    }

    /// Al arrancar: una importación que quedó `executing` tras un corte pasa a `failed`.
    pub(crate) fn fail_interrupted_imports(&self) -> AppResult<usize> {
        let rows = self
            .store
            .proposals_of_kind_in(Kind::Import.as_str(), "executing")?;
        let mut failed = 0;
        for row in rows {
            if self.settle_status(&row, &["executing"], "failed", Some("import_interrupted")) {
                failed += 1;
            }
        }
        Ok(failed)
    }

    /// ¿Hay un evento del historial con origen `chat`, de esta acción y (si no es vacío) este
    /// estado, cuyo detalle empieza o contiene `needle`? Es lo que atribuye al chat una acción
    /// que se encuentra ya hecha tras un corte.
    fn chat_audited(&self, action: &str, status: &str, needle: &str) -> AppResult<bool> {
        Ok(self.store.audit_log()?.iter().any(|e| {
            e.action == action
                && e.origin.as_deref() == Some(CHAT_ORIGIN)
                && (status.is_empty() || e.status == status)
                && e.detail.as_deref().is_some_and(|d| d.contains(needle))
        }))
    }

    /// Los hechos de la regla que ESTA propuesta creó, si consta: tiene el identificador derivado
    /// de la propuesta, la misma acción, condiciones y nombre, y su creación está en el historial
    /// con origen `chat`.
    fn created_by_proposal(&self, row: &ProposalRow) -> AppResult<Option<Value>> {
        let id = chat_rule_id(&row.idem_key);
        let Some(rule) = self.store.rule(&id)? else {
            return Ok(None);
        };
        let name = self.store.rule_meta(&id)?.and_then(|m| m.name);
        let same = rule.version == 1
            && rule.action_kind == row.args["action"].as_str().unwrap_or_default()
            && rule.conditions == row.args["conditions"]
            && name.as_deref() == row.args["name"].as_str();
        if same && self.chat_audited("rule", "", &format!("{id} v1 ("))? {
            return Ok(Some(json!({ "rule_id": id, "active": rule.active })));
        }
        Ok(None)
    }

    /// Primer paso de confirmar una importación: valida que siga pendiente y la pasa a
    /// `executing`. Devuelve el recibo si ya estaba confirmada (no se abre nada) o el guardián de la
    /// importación en curso, que la ventana conserva mientras el diálogo está abierto y se importa:
    /// al soltarse, pase lo que pase, la propuesta deja de constar en curso. Nada se importa aquí.
    pub fn proposal_import_begin(&mut self, id: i64) -> AppResult<ImportStart> {
        let row = self.store.proposal(id)?.ok_or_else(not_found)?;
        if row.kind != Kind::Import.as_str() {
            return Err(AppError::Proposal {
                code: "not_import",
                message: "la propuesta no es de importar".into(),
            });
        }
        match row.status.as_str() {
            "confirmed" => {
                return row
                    .receipt
                    .clone()
                    .and_then(|r| serde_json::from_value(r).ok())
                    .map(|receipt| ImportStart::Done(Box::new(receipt)))
                    .ok_or_else(|| AppError::Proposal {
                        code: "failed",
                        message: "la propuesta consta confirmada pero falta su recibo".into(),
                    })
            }
            "rejected" => {
                return Err(AppError::Proposal {
                    code: "rejected",
                    message: "la propuesta se descartó y ya no se puede confirmar".into(),
                })
            }
            "expired" => return Err(expired()),
            "failed" => {
                return Err(AppError::Proposal {
                    code: "failed",
                    message: "la propuesta falló y no se puede confirmar; pídala de nuevo".into(),
                })
            }
            "stale" => return Err(self.stale_error(&row)?),
            "executing" => return Err(self.import_interrupted(&row)),
            _ => {}
        }
        if now_ms() >= row.expires_at {
            self.settle_status(&row, &["pending"], "expired", None);
            return Err(expired());
        }
        let same = self
            .current_view(&row)?
            .is_some_and(|(_, _, fp)| row.seen_fp.as_deref() == Some(&fp));
        if !same {
            self.settle_status(&row, &["pending"], "stale", None);
            return Err(self.stale_error(&row)?);
        }
        if !self
            .store
            .proposal_transition(row.id, &["pending"], "executing", None)?
        {
            return Err(AppError::Proposal {
                code: "busy",
                message: "la propuesta ya se está confirmando".into(),
            });
        }
        let guard = ImportGuard::new(&self.assistant.importing, row.id);
        Ok(ImportStart::Started(guard))
    }

    /// La persona cerró el diálogo sin elegir nada: la propuesta vuelve a `pending`, sin efectos.
    pub fn proposal_import_cancel(&mut self, guard: &ImportGuard) -> AppResult<()> {
        self.proposal_import_abandon(guard.id, 0, 0)
    }

    /// La importación no llegó a su recibo (cierre del diálogo, error o pánico del hilo): sin nada
    /// importado, la propuesta vuelve a `pending`; con algo importado no se puede repetir, así que
    /// pasa a `failed` y el resultado consta en el historial. Solo actúa sobre una `executing`.
    pub fn proposal_import_abandon(
        &mut self,
        id: i64,
        imported: usize,
        errors: usize,
    ) -> AppResult<()> {
        let Some(row) = self.store.proposal(id)? else {
            return Ok(());
        };
        if row.kind != Kind::Import.as_str() || row.status != "executing" {
            return Ok(());
        }
        if imported == 0 {
            self.store
                .proposal_transition(id, &["executing"], "pending", None)?;
        } else {
            self.record_orphan_import(id, imported, errors, "interrupted");
            self.settle_status(&row, &["executing"], "failed", Some("import_interrupted"));
        }
        Ok(())
    }

    /// Deja en el historial (origen `chat`) lo que se importó cuando la propuesta ya no está en
    /// condiciones de recibirlo: los archivos importados no deben quedar sin constar.
    fn record_orphan_import(&self, id: i64, imported: usize, errors: usize, why: &str) {
        let _ = self.store.proposal_audit(
            None,
            "import_unattached",
            &json!({
                "kind": "import",
                "proposal": id,
                "imported": imported,
                "errors": errors,
                "code": why,
            }),
        );
    }

    /// Último paso: anota lo que el camino de la pantalla importó (`imported` tareas, `errors`
    /// archivos rechazados) como recibo. Sin ninguna importada es `not_done` y la propuesta queda
    /// `failed`.
    pub fn proposal_import_finish(
        &mut self,
        guard: &ImportGuard,
        imported: usize,
        errors: usize,
    ) -> AppResult<ProposalReceipt> {
        let id = guard.id;
        let row = self.store.proposal(id)?;
        let Some(row) = row.filter(|r| r.kind == Kind::Import.as_str() && r.status == "executing")
        else {
            // La propuesta ya no está en condiciones de recibir el resultado: lo importado consta
            // igualmente en el historial.
            self.record_orphan_import(id, imported, errors, "proposal_not_executing");
            return Err(AppError::Proposal {
                code: "not_started",
                message: "la importación de esta propuesta ya no está en curso; lo importado consta en el historial".into(),
            });
        };
        let result = json!({ "imported": imported, "errors": errors });
        let run = if imported > 0 {
            Run::done(result)
        } else {
            Run {
                result,
                outcome: Outcome::NotDone,
                reason: Some("nothing_imported"),
            }
        };
        Ok(self.record_outcome(&row, run, false))
    }
}

/// Identificador de la regla que crea una propuesta, derivado de su clave de idempotencia (única y
/// no reutilizable): `chat-` y 12 cifras hexadecimales. No depende del número de la propuesta, que
/// una persona puede imaginar, y no choca con el nombre que una regla del panel tenga.
pub fn chat_rule_id(idem_key: &str) -> String {
    let digest = Sha256::digest(idem_key.as_bytes());
    let hex: String = digest.iter().take(6).map(|b| format!("{b:02x}")).collect();
    format!("chat-{hex}")
}

/// El resultado de `proposal_import_begin`.
pub enum ImportStart {
    /// Ya estaba confirmada: su recibo original, sin abrir nada.
    Done(Box<ProposalReceipt>),
    /// Puede abrirse el diálogo; el guardián conserva «en curso» hasta soltarse.
    Started(ImportGuard),
}

/// Marca una propuesta de importar como en curso en esta sesión y la quita al soltarse, pase lo que
/// pase (retorno, error o pánico del hilo): un id nunca se queda atascado.
pub struct ImportGuard {
    set: Arc<Mutex<HashSet<i64>>>,
    id: i64,
}

impl ImportGuard {
    fn new(set: &Arc<Mutex<HashSet<i64>>>, id: i64) -> Self {
        set.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(id);
        Self {
            set: Arc::clone(set),
            id,
        }
    }

    pub fn id(&self) -> i64 {
        self.id
    }
}

impl Drop for ImportGuard {
    fn drop(&mut self) {
        self.set
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&self.id);
    }
}

/// Resumen criptográfico de lo que identifica una propuesta de reglas (nunca la huella en claro).
fn basis_of(parts: &Value) -> String {
    Sha256::digest(parts.to_string().as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Hechos de una regla creada o editada para el recibo.
fn rule_facts(view: &crate::panel::RuleView) -> Value {
    json!({ "rule_id": view.rule.id, "active": view.rule.active })
}

/// Condiciones de una regla a partir de los argumentos de la herramienta, con códigos cerrados.
/// Solo las claves de D123 de la acción; nunca «cualquier remitente» (D191).
fn rule_conditions(action: &str, args: &Map<String, Value>) -> Result<Value, &'static str> {
    const KEYS: [&str; 5] = [
        "supplier",
        "max_total",
        "category",
        "sender",
        "sender_domain",
    ];
    let get = |key: &str| args.get(key).and_then(Value::as_str);
    for key in KEYS {
        if let Some(value) = get(key) {
            if has_invisible(value) {
                return Err("invisible_chars");
            }
            if value.chars().any(char::is_control) || carries_web_markup(value) {
                return Err("bad_value");
            }
        }
    }
    // Una dirección o un dominio con letras de otros alfabetos puede leerse igual que otro y no ser
    // el mismo («е» cirílica): desde el chat solo ASCII; una dirección internacional se hace en el
    // panel de reglas.
    if ["sender", "sender_domain"]
        .iter()
        .any(|k| get(k).is_some_and(|v| !v.is_ascii()))
    {
        return Err("non_ascii_sender");
    }
    let present = |keys: &[&str]| keys.iter().any(|k| get(k).is_some());
    let conditions = match action {
        crate::rules::REGISTER_INVOICE => {
            if present(&["category", "sender", "sender_domain"]) {
                return Err("unknown_condition");
            }
            let supplier = get("supplier").ok_or("missing_condition")?;
            let mut map = Map::new();
            map.insert("supplier".into(), json!(supplier.trim()));
            if let Some(limit) = get("max_total") {
                // Un límite de cero no cubriría ninguna factura.
                if nestadmin_ai::assistant::amount_minor(limit) == Some(0) {
                    return Err("bad_value");
                }
                map.insert("max_total".into(), json!(limit));
            }
            Value::Object(map)
        }
        crate::rules::SEND_DRAFT => {
            if present(&["supplier", "max_total"]) {
                return Err("unknown_condition");
            }
            let category = get("category").ok_or("missing_condition")?;
            if nestadmin_ai::tasks::REVIEW_ONLY_CATEGORIES.contains(&category) {
                return Err("review_only_category");
            }
            if !present(&["sender", "sender_domain"]) {
                // «Cualquier remitente» solo existe en el panel (D126).
                return Err("sender_required");
            }
            let mut map = Map::new();
            map.insert("category".into(), json!(category));
            for key in ["sender", "sender_domain"] {
                if let Some(value) = get(key) {
                    map.insert(key.into(), json!(value.trim()));
                }
            }
            Value::Object(map)
        }
        _ => return Err("bad_value"),
    };
    check_rule(action, &conditions, false).map_err(|_| "invalid_conditions")?;
    Ok(conditions)
}

/// ¿Lleva el texto una dirección web, una sintaxis de enlace o una etiqueta (D264)? Los valores que
/// el modelo escribe y la tarjeta muestra (proveedor, nombre de cuenta) no pueden llevarlas.
fn carries_web_markup(text: &str) -> bool {
    nestadmin_ai::assistant::strip_links(text).1 > 0
}

/// Nombre de la regla, siempre compuesto por la aplicación con sus condiciones (D265).
fn default_rule_name(action: &str, conditions: &Value) -> String {
    let text = |key: &str| conditions[key].as_str().unwrap_or_default();
    let name = if action == crate::rules::REGISTER_INVOICE {
        format!("Facturas de {}", text("supplier"))
    } else {
        let who = if text("sender").is_empty() {
            text("sender_domain")
        } else {
            text("sender")
        };
        format!("Respuestas ({}) a {who}", text("category"))
    };
    name.chars().take(80).collect()
}

/// Guardián del origen `chat`: al salir del ámbito (retorno, error o pánico) lo restablece.
struct OriginScope<'a>(&'a mut App);

impl Drop for OriginScope<'_> {
    fn drop(&mut self) {
        let _ = self.0.store.clear_audit_origin();
    }
}

fn not_found() -> AppError {
    AppError::Proposal {
        code: "not_found",
        message: "la propuesta no existe".into(),
    }
}

fn expired() -> AppError {
    AppError::Proposal {
        code: "expired",
        message: "la propuesta caducó; pídala de nuevo".into(),
    }
}
