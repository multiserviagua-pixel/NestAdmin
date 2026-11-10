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
use nestadmin_ai::tasks::{draft_flags, draft_leaks_internals};
use nestadmin_core::actions::idempotency_key;
use nestadmin_core::{NewProposal, ProposalRow, Task, TaskState, CHAT_ORIGIN};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use super::tools::ToolOutput;
use crate::refine::now_ms;
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
}

impl Effect {
    fn as_str(self) -> &'static str {
        match self {
            Self::SendMail => "send_mail",
            Self::RegisterInvoice => "register_invoice",
            Self::NoEffect => "none",
        }
    }

    fn parse(text: &str) -> Self {
        match text {
            "send_mail" => Self::SendMail,
            "register_invoice" => Self::RegisterInvoice,
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

/// Lo que devuelve confirmar: se guarda tal cual y se devuelve igual si se confirma otra vez.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProposalReceipt {
    pub proposal_id: i64,
    pub kind: String,
    pub task_id: Option<i64>,
    /// Estado de la tarea tras la acción, si la hay.
    pub task_state: Option<String>,
    pub effect: Effect,
    pub confirmed_at: i64,
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
fn email_summary(task: &Task) -> Value {
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
        "draft_flags": r["draft_flags"],
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

fn rejected(code: &'static str) -> Result<Built, &'static str> {
    Err(code)
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

    /// Valida y compone una propuesta con datos del equipo. Los códigos de rechazo son cerrados.
    fn build_proposal(
        &self,
        kind: Kind,
        args: &Map<String, Value>,
        known: &HashSet<String>,
    ) -> Result<Built, &'static str> {
        if kind.on_task() {
            let task = self.task_of(args, known)?;
            return self.build_task_proposal(kind, args, task);
        }
        match kind {
            Kind::FetchMail => {
                let view = self.mail_accounts();
                Ok(Built {
                    task_id: None,
                    seen_version: None,
                    args: json!({}),
                    effect: Effect::NoEffect,
                    summary: json!({
                        "type": "fetch_mail",
                        "accounts": view.accounts.len(),
                        "limit": view.limit,
                    }),
                })
            }
            Kind::MailLimit => {
                let limit = args
                    .get("limit")
                    .and_then(Value::as_i64)
                    .filter(|n| (1..=500).contains(n))
                    .ok_or("bad_value")?;
                Ok(Built {
                    task_id: None,
                    seen_version: None,
                    args: json!({ "limit": limit }),
                    effect: Effect::NoEffect,
                    summary: json!({
                        "type": "mail_limit",
                        "current": self.mail_accounts().limit,
                        "new": limit,
                    }),
                })
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
                let label = args
                    .get("label")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|l| !l.is_empty() && l.chars().count() <= 60)
                    .filter(|l| !l.chars().any(char::is_control))
                    .ok_or("bad_value")?;
                let view = self.mail_accounts();
                let account = view.accounts.get(index - 1).ok_or("account_not_found")?;
                Ok(Built {
                    task_id: None,
                    seen_version: None,
                    args: json!({
                        "account_id": account.id,
                        "account_ref": format!("acct:{index}"),
                        "label": label,
                    }),
                    effect: Effect::NoEffect,
                    summary: json!({
                        "type": "rename_account",
                        "account": format!("acct:{index}"),
                        "provider": serde_json::to_value(account.provider).unwrap_or(Value::Null),
                        "current_label": account.label,
                        "new_label": label,
                    }),
                })
            }
            Kind::SenderPreference => {
                let get = |k: &str| args.get(k).and_then(Value::as_str).unwrap_or_default();
                let (scope, key, route) =
                    crate::preferences::validated(get("scope"), get("key"), get("route"))
                        .map_err(|_| "bad_value")?;
                let current = self
                    .store
                    .sender_preferences()
                    .map_err(|_| "unavailable")?
                    .into_iter()
                    .find(|p| p.scope == scope.as_str() && p.key == key)
                    .map(|p| p.route);
                Ok(Built {
                    task_id: None,
                    seen_version: None,
                    args: json!({
                        "scope": scope.as_str(),
                        "key": key,
                        "route": route.as_str(),
                    }),
                    effect: Effect::NoEffect,
                    summary: json!({
                        "type": "sender_preference",
                        "scope": scope.as_str(),
                        "key": key,
                        "current": current,
                        "new": route.as_str(),
                    }),
                })
            }
            _ => rejected("unknown_tool"),
        }
    }

    /// Las seis propuestas que actúan sobre una tarea: mismas condiciones que valida el método de
    /// `App` correspondiente y la pantalla de revisión.
    fn build_task_proposal(
        &self,
        kind: Kind,
        args: &Map<String, Value>,
        task: Task,
    ) -> Result<Built, &'static str> {
        let r = task.result.clone().unwrap_or(Value::Null);
        let reviewable = task.state == TaskState::AwaitingReview;
        let (effect, args_json, summary) = match kind {
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
                let summary = if task.kind == INVOICE {
                    invoice_summary(&task)
                } else {
                    email_summary(&task)
                };
                (effect_of(&task), json!({}), summary)
            }
            Kind::Reject => {
                if !reviewable && task.state != TaskState::Failed {
                    return Err("wrong_state");
                }
                (
                    Effect::NoEffect,
                    json!({}),
                    json!({ "type": "reject", "task": task_block(&task) }),
                )
            }
            Kind::Retry => {
                if task.state != TaskState::Failed {
                    return Err("wrong_state");
                }
                (
                    Effect::NoEffect,
                    json!({}),
                    json!({
                        "type": "retry",
                        "task": task_block(&task),
                        "error_code": task.error.as_ref().map(|e| e.code.clone()),
                    }),
                )
            }
            Kind::Archive => {
                if task.kind != EMAIL {
                    return Err("not_an_email");
                }
                if !reviewable {
                    return Err("wrong_state");
                }
                if self.has_attempt(&task)? {
                    return Err("already_attempted");
                }
                (
                    Effect::NoEffect,
                    json!({}),
                    json!({ "type": "archive", "task": task_block(&task) }),
                )
            }
            Kind::ConfirmNotSent => {
                if !reviewable {
                    return Err("wrong_state");
                }
                if !self.has_attempt(&task)? {
                    return Err("no_attempt");
                }
                (
                    Effect::NoEffect,
                    json!({}),
                    json!({
                        "type": "confirm_not_sent",
                        "task": task_block(&task),
                        "recovery": recovery_code(&r),
                    }),
                )
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
                let body = args
                    .get("body")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|b| !b.is_empty())
                    .ok_or("bad_value")?;
                if body.chars().count() > nestadmin_ai::assistant::MAX_DRAFT_CHARS {
                    return Err("bad_value");
                }
                // El borrador es el único texto del modelo que llega a una tarjeta: pasa las
                // mismas comprobaciones que uno redactado por la IA (D160).
                if draft_leaks_internals(body) {
                    return Err("draft_leak");
                }
                if !draft_flags(body).is_empty() {
                    return Err("draft_flagged");
                }
                if body == current.trim() {
                    return Err("no_change");
                }
                (
                    Effect::NoEffect,
                    json!({ "body": body }),
                    json!({
                        "type": "edit_draft",
                        "version": version_of(&task),
                        "task": task_block(&task),
                        "to": r["draft"]["to"],
                        "subject": r["draft"]["subject"],
                        "current_body": current,
                        "new_body": body,
                    }),
                )
            }
            _ => return Err("unknown_tool"),
        };
        Ok(Built {
            task_id: Some(task.id),
            seen_version: Some(version_of(&task)),
            args: args_json,
            effect,
            summary,
        })
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

    /// Recalcula `expired` y `stale` de una propuesta pendiente (y los guarda).
    fn refreshed(&self, row: ProposalRow) -> AppResult<ProposalRow> {
        if row.status != "pending" {
            return Ok(row);
        }
        let new_status = if now_ms() >= row.expires_at {
            Some("expired")
        } else if let (Some(task_id), Some(seen)) = (row.task_id, row.seen_version) {
            (self.current_version(task_id)? != Some(seen)).then_some("stale")
        } else {
            None
        };
        if let Some(status) = new_status {
            self.settle_status(&row, &["pending"], status, None);
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
    /// Descartar dos veces no hace nada; una ya confirmada no se descarta.
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
            "executing" => {
                return Err(AppError::Proposal {
                    code: "executing",
                    message: "la propuesta se está ejecutando".into(),
                })
            }
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
    /// - `pending`: comprueba la versión vista (si cambió, `stale` y `StaleVersion`), pasa a
    ///   `executing`, ejecuta y pasa a `confirmed`. `AmountAckRequired` la deja `pending`; una
    ///   precondición que ya no se cumple (`Invalid`) la deja `failed`; un fallo del conector, la
    ///   base de datos o el disco, cuyo efecto no se sabe, la deja `executing` para reconciliarla.
    /// - `executing` (un corte entre ejecutar y anotar): averigua si la acción ya se hizo y, si es
    ///   así, la da por confirmada sin repetirla; si no, la reintenta por el camino idempotente.
    pub fn proposal_confirm(
        &mut self,
        id: i64,
        amounts_checked: bool,
    ) -> AppResult<ProposalReceipt> {
        // El origen vale solo durante la ejecución y se restablece siempre, también con error.
        self.store.set_audit_origin(Some(CHAT_ORIGIN))?;
        let result = self.confirm_proposal(id, amounts_checked);
        if self.store.set_audit_origin(None).is_err() {
            let _ = self.store.set_audit_origin(None);
        }
        result
    }

    fn confirm_proposal(&mut self, id: i64, amounts_checked: bool) -> AppResult<ProposalReceipt> {
        let row = self.store.proposal(id)?.ok_or_else(not_found)?;
        let seen = row.seen_version.unwrap_or_default();
        let stale = |current: i64| AppError::StaleVersion {
            approved: seen,
            current,
        };
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
            "stale" => {
                let current = match row.task_id {
                    Some(task_id) => self.current_version(task_id)?.unwrap_or_default(),
                    None => seen,
                };
                return Err(stale(current));
            }
            _ => {}
        }
        let executing = row.status == "executing";
        if executing {
            // Un corte entre ejecutar y anotar: no se vuelve a ejecutar a ciegas.
            if let Some(result) = self.proposal_done(&row)? {
                return Ok(self.finish_confirmed(&row, result, true));
            }
        } else if now_ms() >= row.expires_at {
            self.settle_status(&row, &["pending"], "expired", None);
            return Err(expired());
        }
        if let Some(task_id) = row.task_id {
            let current = self.current_version(task_id)?.unwrap_or_default();
            if current != seen {
                self.settle_status(&row, &["pending", "executing"], "stale", None);
                return Err(stale(current));
            }
        }
        if !executing
            && !self
                .store
                .proposal_transition(row.id, &["pending"], "executing", None)?
        {
            return Err(AppError::Proposal {
                code: "busy",
                message: "la propuesta ya se está confirmando".into(),
            });
        }
        match self.run_proposal(&row, amounts_checked) {
            Ok(result) => Ok(self.finish_confirmed(&row, result, false)),
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
                    // La precondición ya no se cumple (estado, borrador…): no se ejecutó nada.
                    AppError::Invalid(_) => {
                        self.settle_status(&row, &["executing"], "failed", Some("invalid"));
                    }
                    AppError::Connector(e) if e.code == nestadmin_simulators::codes::NOT_FOUND => {
                        self.settle_status(&row, &["executing"], "failed", Some("not_found"));
                    }
                    // Conector, base de datos o disco: no se sabe si llegó a hacerse. Queda
                    // `executing` y la siguiente confirmación lo comprueba antes de repetir.
                    _ => {}
                }
                Err(error)
            }
        }
    }

    /// Guarda el recibo, marca `confirmed`, lo deja en el historial y da por obsoletas las demás
    /// propuestas pendientes de la misma tarea (la tarea ha cambiado).
    fn finish_confirmed(
        &self,
        row: &ProposalRow,
        result: Value,
        reconciled: bool,
    ) -> ProposalReceipt {
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
            reconciled,
            result,
        };
        let stored = serde_json::to_value(&receipt).unwrap_or(Value::Null);
        if self
            .store
            .proposal_transition(
                row.id,
                &["executing", "pending"],
                "confirmed",
                Some(&stored),
            )
            .unwrap_or(false)
        {
            self.audit_proposal(row, "confirmed", reconciled.then_some("reconciled"));
        }
        if let Some(task_id) = row.task_id {
            if let Ok(open) = self.store.proposals_open_for_task(task_id) {
                for sibling in open.iter().filter(|s| s.id != row.id) {
                    self.settle_status(sibling, &["pending"], "stale", None);
                }
            }
        }
        receipt
    }

    /// Ejecuta la acción propuesta con el mismo método de `App` que la pantalla.
    fn run_proposal(&mut self, row: &ProposalRow, amounts_checked: bool) -> AppResult<Value> {
        let kind = Kind::parse(&row.kind)
            .ok_or_else(|| AppError::Invalid("tipo de propuesta desconocido".into()))?;
        let task_id = row.task_id.unwrap_or_default();
        let seen = row.seen_version.unwrap_or(1);
        let arg = |k: &str| row.args[k].as_str().unwrap_or_default().to_owned();
        match kind {
            Kind::Approve => {
                self.approve_reviewed(task_id, seen, amounts_checked)?;
                Ok(json!({ "version": seen }))
            }
            Kind::EditDraft => {
                let task = self.edit_draft(task_id, &arg("body"))?;
                Ok(json!({ "new_version": version_of(&task) }))
            }
            Kind::Reject => {
                self.reject(task_id)?;
                Ok(json!({}))
            }
            Kind::Retry => {
                self.retry(task_id)?;
                Ok(json!({}))
            }
            Kind::Archive => {
                self.archive(task_id)?;
                Ok(json!({}))
            }
            Kind::ConfirmNotSent => {
                let task = self.confirm_not_sent(task_id)?;
                Ok(json!({ "new_version": version_of(&task) }))
            }
            Kind::FetchMail => {
                let report = self.fetch_mail()?;
                Ok(json!({ "added": report.added, "limit": report.limit }))
            }
            Kind::MailLimit => {
                let limit = usize::try_from(row.args["limit"].as_i64().unwrap_or_default())
                    .map_err(|_| AppError::Invalid("límite no válido".into()))?;
                self.set_mail_limit(limit)?;
                Ok(json!({ "limit": limit }))
            }
            Kind::RenameAccount => {
                let view = self.rename_mail_account(&arg("account_id"), &arg("label"))?;
                Ok(json!({ "label": view.label }))
            }
            Kind::SenderPreference => {
                let change =
                    self.set_sender_preference(&arg("scope"), &arg("key"), &arg("route"))?;
                Ok(json!({
                    "changed": change.changed,
                    "route": change.preference.route,
                    "warnings": change.warnings.len(),
                }))
            }
        }
    }

    /// ¿La acción de una propuesta `executing` ya se hizo? Devuelve sus hechos si es así, sin
    /// ejecutar nada.
    fn proposal_done(&mut self, row: &ProposalRow) -> AppResult<Option<Value>> {
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
            Kind::Retry => task.and_then(|t| (t.state != TaskState::Failed).then(|| json!({}))),
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
