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

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};

use nestadmin_ai::assistant::{account_index, doc_id};
use nestadmin_ai::refine::is_format_char;
use nestadmin_ai::tasks::{draft_flags_allowing, draft_leaks_internals};
use nestadmin_core::actions::idempotency_key;
use nestadmin_core::{CoreError, NewProposal, ProposalRow, Task, TaskState, CHAT_ORIGIN};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use super::tools::ToolOutput;
use crate::refine::now_ms;
use crate::rules::{REGISTER_INVOICE, SEND_DRAFT};
use crate::{action_kind, task_is_initial_read, App, AppError, AppResult, EMAIL, INVOICE};

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
    /// Propuestas de este turno (nuevas o reutilizadas), sin repetir.
    pub(crate) created: Vec<i64>,
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
}

impl Effect {
    fn as_str(self) -> &'static str {
        match self {
            Self::SendMail => "send_mail",
            Self::RegisterInvoice => "register_invoice",
            Self::NoEffect => "none",
            Self::MayTriggerRules => "may_trigger_rules",
        }
    }

    fn parse(text: &str) -> Self {
        match text {
            "send_mail" => Self::SendMail,
            "register_invoice" => Self::RegisterInvoice,
            "may_trigger_rules" => Self::MayTriggerRules,
            _ => Self::NoEffect,
        }
    }
}

/// Los diez tipos de propuesta.
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
        }
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
    let canonical = json!({
        "kind": kind.as_str(),
        "task": task,
        "effect": effect.as_str(),
        "summary": summary,
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
fn email_summary(task: &Task, draft_flags: &[&str]) -> Value {
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
        let (conversation_id, known, created) = {
            let Ok(ctx) = self.assistant.turn.lock() else {
                return failure("unavailable");
            };
            if !ctx.active {
                return failure("no_turn");
            }
            (ctx.conversation_id, ctx.known.clone(), ctx.created.clone())
        };
        let built = match self.build_proposal(kind, args, &known) {
            Ok(built) => built,
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
            _ => Err("unknown_tool"),
        }
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
                    (effect_of(task), email_summary(task, &flags))
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
    ) -> Result<Built, &'static str> {
        let (task, args_json) = if kind.on_task() {
            let task = self.task_of(args, known)?;
            let args_json = self.validate_task_proposal(kind, args, &task)?;
            (Some(task), args_json)
        } else {
            (None, self.validate_other_proposal(kind, args, known)?)
        };
        let (effect, summary) = self.compose(kind, &args_json, task.as_ref())?;
        let fp = fingerprint(kind, task.as_ref(), effect, &summary);
        Ok(Built {
            task_id: task.as_ref().map(|t| t.id),
            seen_version: task.as_ref().map(version_of),
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
    ) -> Result<Value, &'static str> {
        match kind {
            Kind::FetchMail => Ok(json!({})),
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
                    .filter(|l| !l.chars().any(char::is_control))
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
    ) -> AppResult<ProposalReceipt> {
        self.with_chat_origin(|app| app.confirm_proposal(id, amounts_checked))
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

    fn confirm_proposal(&mut self, id: i64, amounts_checked: bool) -> AppResult<ProposalReceipt> {
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
        })
    }
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
