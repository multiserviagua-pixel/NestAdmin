//! Capa de aplicación de NestAdmin (flujos 1–5 de requisitos.md). Une la cola durable, la
//! IA, la validación de facturas, los simuladores y las reglas, sin depender de la ventana:
//! la interfaz Tauri solo llama a esta capa.
//!
//! El trabajo con IA se divide en tres pasos para que la llamada al modelo, que puede tardar
//! decenas de segundos, no bloquee el resto de la aplicación: `begin_next` toma la tarea y
//! prepara la solicitud, `Job::run` llama al proveedor sin acceso a `App`, y `finish` valida,
//! aplica reglas y deja la tarea completada, en revisión o fallida.

pub mod rules;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use nestadmin_ai::tasks::{email_request, extraction_request, parse_email, parse_extraction};
use nestadmin_ai::{GenerateRequest, GenerateResult, Provider, ProviderKind};
use nestadmin_core::{
    ActionRequest, AuditEntry, Authorization, ConnectorError, CoreError, Rule, Store, Task,
    TaskError, TaskState,
};
use nestadmin_documents::{process_invoice, DuplicateRegistry, Evidenced, Invoice, RawInvoice};
use nestadmin_simulators::{AdminSimulator, MailConnector, MailSimulator, Message, NewDraft};
use serde::Serialize;
use serde_json::{json, Value};

use crate::rules::{decide, RuleDecision, REGISTER_INVOICE, SEND_DRAFT};

pub const INVOICE: &str = "invoice";
pub const EMAIL: &str = "email";

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error(transparent)]
    Core(#[from] CoreError),
    #[error("conector: {0}")]
    Connector(#[from] ConnectorError),
    #[error("archivo: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Invalid(String),
    #[error("el contenido cambió (versión {current}, se aprobó la {approved}); revíselo de nuevo")]
    StaleVersion { approved: i64, current: i64 },
}

/// La interfaz recibe el error como texto; nunca contiene credenciales.
impl Serialize for AppError {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

pub type AppResult<T> = Result<T, AppError>;

/// Ubicación de los datos locales; nunca dentro del repositorio ni junto a evaluaciones.
#[derive(Debug, Clone)]
pub struct AppPaths {
    pub data_dir: PathBuf,
}

impl AppPaths {
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            data_dir: data_dir.into(),
        }
    }
    fn db(&self) -> PathBuf {
        self.data_dir.join("nestadmin.db")
    }
    fn mail(&self) -> PathBuf {
        self.data_dir.join("simulador-correo.json")
    }
    fn admin(&self) -> PathBuf {
        self.data_dir.join("simulador-administracion.json")
    }
    fn documents(&self) -> PathBuf {
        self.data_dir.join("documentos")
    }
}

/// Proveedor activo, para mostrarlo en la interfaz.
#[derive(Debug, Clone, Serialize)]
pub struct ProviderInfo {
    pub provider: ProviderKind,
    pub model: String,
}

pub struct App {
    store: Store,
    mail: MailSimulator,
    admin: AdminSimulator,
    provider: Arc<dyn Provider>,
    documents: PathBuf,
    registry: DuplicateRegistry,
}

/// Tarea tomada de la cola con su solicitud preparada.
pub struct Job {
    pub task: Task,
    provider: Arc<dyn Provider>,
    prepared: Result<GenerateRequest, TaskError>,
}

/// Respuesta del proveedor (o el motivo del fallo) lista para `finish`.
pub struct Completed {
    task: Task,
    outcome: Result<GenerateResult, TaskError>,
}

impl Job {
    /// Llama al proveedor elegido. Un error se devuelve tal cual: no hay otro proveedor.
    pub fn run(self) -> Completed {
        let outcome = self.prepared.and_then(|request| {
            self.provider.generate(&request).map_err(|e| TaskError {
                code: e.code().to_owned(),
                message: e.to_string(),
                recoverable: true,
            })
        });
        Completed {
            task: self.task,
            outcome,
        }
    }
}

fn task_error(code: &str, message: impl Into<String>, recoverable: bool) -> TaskError {
    TaskError {
        code: code.to_owned(),
        message: message.into(),
        recoverable,
    }
}

impl App {
    /// Abre los datos locales y recupera el trabajo interrumpido: las tareas en curso
    /// vuelven a la cola y las acciones pendientes se reconcilian con los simuladores.
    pub fn open(paths: &AppPaths, provider: Arc<dyn Provider>) -> AppResult<Self> {
        fs::create_dir_all(paths.documents())?;
        let mut app = Self {
            store: Store::open(&paths.db())?,
            mail: MailSimulator::open(&paths.mail())?,
            admin: AdminSimulator::open(&paths.admin())?,
            provider,
            documents: paths.documents(),
            registry: DuplicateRegistry::new(),
        };
        app.store.recover()?;
        app.store.reconcile_actions(SEND_DRAFT, &app.mail)?;
        app.store.reconcile_actions(REGISTER_INVOICE, &app.admin)?;
        app.rebuild_registry()?;
        Ok(app)
    }

    /// Vuelve a cargar en orden las facturas ya procesadas para seguir detectando duplicados.
    fn rebuild_registry(&mut self) -> AppResult<()> {
        let mut tasks = self.store.all_tasks()?;
        tasks.reverse();
        for task in tasks.iter().filter(|t| t.kind == INVOICE) {
            if let Some(fields) = task.result.as_ref().map(|r| &r["fields"]) {
                let invoice = Invoice::from_raw(&raw_from_fields(fields));
                self.registry.check_and_add(task.id.to_string(), &invoice);
            }
        }
        Ok(())
    }

    pub fn provider(&self) -> ProviderInfo {
        ProviderInfo {
            provider: self.provider.kind(),
            model: self.provider.model().to_owned(),
        }
    }

    /// Cambio explícito de proveedor; afecta a las tareas que empiecen después.
    pub fn select_provider(&mut self, provider: Arc<dyn Provider>) {
        self.provider = provider;
    }

    /// Guarda una copia del documento en los datos locales y lo encola.
    pub fn import_document(&mut self, name: &str, text: &str) -> AppResult<Task> {
        if text.trim().is_empty() {
            return Err(AppError::Invalid("el documento está vacío".into()));
        }
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let path = self.documents.join(format!("{stamp}.txt"));
        fs::write(&path, text)?;
        let input = json!({ "document": name, "path": path });
        Ok(self.store.enqueue(INVOICE, &input.to_string())?)
    }

    /// Consulta el simulador de correo y encola los mensajes que aún no tienen tarea.
    pub fn fetch_mail(&mut self) -> AppResult<usize> {
        let known: Vec<String> = self
            .store
            .all_tasks()?
            .into_iter()
            .filter(|t| t.kind == EMAIL)
            .map(|t| t.input_ref)
            .collect();
        let mut added = 0;
        for message in self.messages()? {
            let input = json!({ "message_id": message.id }).to_string();
            if !known.contains(&input) {
                self.store.enqueue(EMAIL, &input)?;
                added += 1;
            }
        }
        Ok(added)
    }

    fn messages(&self) -> AppResult<Vec<Message>> {
        let mut all = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let page = self.mail.list_messages(cursor.as_deref())?;
            all.extend(page.messages);
            match page.next_cursor {
                Some(next) => cursor = Some(next),
                None => return Ok(all),
            }
        }
    }

    fn message(&self, id: &str) -> AppResult<Option<Message>> {
        Ok(self.messages()?.into_iter().find(|m| m.id == id))
    }

    /// Toma la siguiente tarea y prepara su solicitud. `None` si la cola está vacía.
    pub fn begin_next(&mut self) -> AppResult<Option<Job>> {
        let Some(task) = self.store.claim_next()? else {
            return Ok(None);
        };
        let input: Value = serde_json::from_str(&task.input_ref).unwrap_or(Value::Null);
        let id = task.id.to_string();
        let prepared = match task.kind.as_str() {
            INVOICE => match input["path"].as_str().map(fs::read_to_string) {
                Some(Ok(text)) => Ok(extraction_request(&id, &text)),
                _ => Err(task_error(
                    "document_unreadable",
                    "no se pudo leer el documento",
                    false,
                )),
            },
            EMAIL => match self.message(input["message_id"].as_str().unwrap_or(""))? {
                Some(Message {
                    body: Some(body),
                    subject,
                    ..
                }) => {
                    let text = match subject {
                        Some(subject) => format!("Asunto: {subject}\n\n{body}"),
                        None => body,
                    };
                    Ok(email_request(&id, &text))
                }
                Some(_) => Err(task_error(
                    "empty_message",
                    "el mensaje no tiene cuerpo",
                    false,
                )),
                None => Err(task_error("not_found", "el mensaje ya no existe", false)),
            },
            other => Err(task_error(
                "invalid_input",
                format!("tipo de tarea desconocido: {other}"),
                false,
            )),
        };
        Ok(Some(Job {
            task,
            provider: Arc::clone(&self.provider),
            prepared,
        }))
    }

    /// Valida la respuesta, aplica reglas y deja la tarea en su estado siguiente.
    pub fn finish(&mut self, completed: Completed) -> AppResult<Task> {
        let task = completed.task;
        let generated = match completed.outcome {
            Ok(generated) => generated,
            Err(error) => return Ok(self.store.fail(task.id, &error)?),
        };
        let built = match task.kind.as_str() {
            INVOICE => self.invoice_result(&task, &generated),
            _ => self.email_result(&task, &generated),
        };
        let mut result = match built {
            Ok(result) => result,
            Err(error) => return Ok(self.store.fail(task.id, &error)?),
        };
        let kind = action_kind(&task);
        match decide(&self.store.rules()?, kind, &result) {
            RuleDecision::Automate {
                rule_id,
                rule_version,
            } => {
                let authorization = Authorization::Rule {
                    rule_id,
                    rule_version,
                };
                result["authorization"] = serde_json::to_value(&authorization).unwrap_or_default();
                self.execute(task, result, authorization)
            }
            RuleDecision::Review(reason) => {
                result["review_reason"] = json!(reason);
                Ok(self.store.await_review(task.id, &result)?)
            }
        }
    }

    /// Procesa una tarea completa en el hilo actual. `None` si no había trabajo.
    pub fn process_next(&mut self) -> AppResult<Option<Task>> {
        match self.begin_next()? {
            Some(job) => self.finish(job.run()).map(Some),
            None => Ok(None),
        }
    }

    fn invoice_result(
        &mut self,
        task: &Task,
        generated: &GenerateResult,
    ) -> Result<Value, TaskError> {
        let raw = parse_extraction(&generated.output)
            .map_err(|e| task_error(e.code(), e.to_string(), true))?;
        let invoice = Invoice::from_raw(&raw);
        let report = process_invoice(&mut self.registry, task.id.to_string(), &invoice);
        let mut fields: serde_json::Map<String, Value> = invoice
            .contract_fields()
            .into_iter()
            .map(|(name, value)| (name.to_owned(), value.map_or(Value::Null, Value::String)))
            .collect();
        let currency = invoice.currency.as_ref().map(|c| c.value.clone());
        fields.insert(
            "currency".into(),
            currency.clone().map_or(Value::Null, Value::String),
        );
        let field = |name: &str| fields[name].as_str().map(str::to_owned);
        let payload = json!({
            "supplier": field("supplier"),
            "number": field("invoice_number"),
            "date": field("date"),
            "currency": currency,
            "base": field("subtotal"),
            "taxes": field("tax"),
            "total": field("total"),
        });
        let input: Value = serde_json::from_str(&task.input_ref).unwrap_or(Value::Null);
        Ok(json!({
            "type": INVOICE,
            "version": 1,
            "document": input["document"],
            "fields": fields,
            "issues": report.issues.iter().map(|i| i.as_str()).collect::<Vec<_>>(),
            "duplicates": report.duplicates,
            "unreadable": report.unreadable,
            "provider": generated.provider,
            "model": generated.model,
            "latency_ms": generated.latency_ms,
            "action": { "kind": REGISTER_INVOICE, "payload": payload },
        }))
    }

    fn email_result(
        &mut self,
        task: &Task,
        generated: &GenerateResult,
    ) -> Result<Value, TaskError> {
        let (category, body) = parse_email(&generated.output)
            .map_err(|e| task_error(e.code(), e.to_string(), true))?;
        let input: Value = serde_json::from_str(&task.input_ref).unwrap_or(Value::Null);
        let message = self
            .message(input["message_id"].as_str().unwrap_or(""))
            .map_err(|e| task_error("unavailable", e.to_string(), true))?
            .ok_or_else(|| task_error("not_found", "el mensaje ya no existe", false))?;
        // Sin remitente no hay a quién responder: el borrador se muestra, pero no se puede enviar.
        let to: Vec<String> = message.from.iter().cloned().collect();
        let subject = message.subject.as_ref().map(|s| format!("Re: {s}"));
        Ok(json!({
            "type": EMAIL,
            "version": 1,
            "message": message,
            "category": category,
            "draft": { "to": to, "subject": subject, "body": body },
            "provider": generated.provider,
            "model": generated.model,
            "latency_ms": generated.latency_ms,
            "action": { "kind": SEND_DRAFT },
        }))
    }

    /// Ejecuta la acción de la tarea una sola vez y la completa; un fallo del conector deja
    /// la tarea fallida y visible, nunca como éxito.
    fn execute(
        &mut self,
        task: Task,
        mut result: Value,
        authorization: Authorization,
    ) -> AppResult<Task> {
        let version = result["version"].as_i64().unwrap_or(1);
        let outcome = match task.kind.as_str() {
            INVOICE => {
                let request = ActionRequest {
                    task_id: task.id,
                    kind: REGISTER_INVOICE.into(),
                    version,
                    payload: result["action"]["payload"].clone(),
                    authorization,
                };
                self.store.perform_action(&request, &self.admin)
            }
            _ => self.send_reply(&task, &result, version, authorization),
        };
        match outcome {
            Ok(done) => {
                result["receipt"] = done.receipt().clone();
                Ok(self.store.complete(task.id, &result)?)
            }
            Err(error) => {
                let code = match &error {
                    CoreError::Connector(e) => e.code.clone(),
                    _ => "action_failed".to_owned(),
                };
                Ok(self
                    .store
                    .fail(task.id, &task_error(&code, error.to_string(), true))?)
            }
        }
    }

    fn send_reply(
        &mut self,
        task: &Task,
        result: &Value,
        version: i64,
        authorization: Authorization,
    ) -> Result<nestadmin_core::ActionOutcome, CoreError> {
        let draft: NewDraft = serde_json::from_value(result["draft"].clone())?;
        let message_id = result["message"]["id"].as_str().unwrap_or("");
        // El borrador se crea con una clave propia de esta versión, así que repetirlo tras
        // un corte devuelve el mismo borrador y la misma carga del envío.
        let draft_id = self.mail.create_draft(
            message_id,
            &draft,
            &format!("task-{}/create_draft/v{version}", task.id),
        )?;
        let request = ActionRequest {
            task_id: task.id,
            kind: SEND_DRAFT.into(),
            version,
            payload: json!({ "draft_id": draft_id }),
            authorization,
        };
        self.store.perform_action(&request, &self.mail)
    }

    fn reviewable(&mut self, task_id: i64) -> AppResult<(Task, Value)> {
        let task = self.store.task(task_id)?;
        if task.state != TaskState::AwaitingReview {
            return Err(AppError::Invalid(format!(
                "la tarea {task_id} no está pendiente de revisión"
            )));
        }
        let result = task.result.clone().unwrap_or(Value::Null);
        Ok((task, result))
    }

    /// Aprueba la versión que el usuario ha visto. Si el contenido cambió, exige revisarlo.
    pub fn approve(&mut self, task_id: i64, version: i64) -> AppResult<Task> {
        let (task, result) = self.reviewable(task_id)?;
        let current = result["version"].as_i64().unwrap_or(1);
        if current != version {
            return Err(AppError::StaleVersion {
                approved: version,
                current,
            });
        }
        self.execute(task, result, Authorization::Approval)
    }

    /// Edita el borrador de una respuesta; crea una versión nueva que requiere aprobación.
    pub fn edit_draft(&mut self, task_id: i64, body: &str) -> AppResult<Task> {
        let (task, mut result) = self.reviewable(task_id)?;
        if task.kind != EMAIL {
            return Err(AppError::Invalid(
                "solo se editan borradores de correo".into(),
            ));
        }
        if body.trim().is_empty() {
            return Err(AppError::Invalid("el borrador está vacío".into()));
        }
        result["draft"]["body"] = json!(body);
        result["version"] = json!(result["version"].as_i64().unwrap_or(1) + 1);
        Ok(self.store.revise(task_id, &result)?)
    }

    /// Descarta la tarea sin ejecutar ninguna acción.
    pub fn reject(&mut self, task_id: i64) -> AppResult<Task> {
        Ok(self.store.cancel(task_id)?)
    }

    pub fn retry(&mut self, task_id: i64) -> AppResult<Task> {
        Ok(self.store.retry(task_id)?)
    }

    pub fn tasks(&self) -> AppResult<Vec<Task>> {
        Ok(self.store.all_tasks()?)
    }

    pub fn audit_log(&self) -> AppResult<Vec<AuditEntry>> {
        Ok(self.store.audit_log()?)
    }

    pub fn rules(&self) -> AppResult<Vec<Rule>> {
        Ok(self.store.rules()?)
    }

    pub fn create_rule(
        &mut self,
        id: &str,
        action_kind: &str,
        conditions: &Value,
    ) -> AppResult<Rule> {
        check_rule(action_kind, conditions)?;
        Ok(self.store.create_rule(id.trim(), action_kind, conditions)?)
    }

    pub fn update_rule(&mut self, id: &str, conditions: &Value) -> AppResult<Rule> {
        let current = self
            .store
            .rule(id)?
            .ok_or_else(|| AppError::Invalid(format!("regla {id} inexistente")))?;
        check_rule(&current.action_kind, conditions)?;
        Ok(self.store.update_rule(id, conditions)?)
    }

    pub fn set_rule_active(&mut self, id: &str, active: bool) -> AppResult<Rule> {
        Ok(self.store.set_rule_active(id, active)?)
    }

    /// Para pruebas y demostraciones: efectos registrados en los simuladores.
    pub fn simulators(&self) -> (&MailSimulator, &AdminSimulator) {
        (&self.mail, &self.admin)
    }
}

fn action_kind(task: &Task) -> &'static str {
    if task.kind == INVOICE {
        REGISTER_INVOICE
    } else {
        SEND_DRAFT
    }
}

fn check_rule(action_kind: &str, conditions: &Value) -> AppResult<()> {
    let text = |key: &str| {
        conditions
            .get(key)
            .and_then(Value::as_str)
            .filter(|v| !v.trim().is_empty())
    };
    match action_kind {
        REGISTER_INVOICE if text("supplier").is_none() => Err(AppError::Invalid(
            "la regla de registro necesita un proveedor".into(),
        )),
        REGISTER_INVOICE => match text("max_total") {
            Some(limit) => nestadmin_documents::parse_amount("max_total", limit)
                .map(|_| ())
                .map_err(|_| {
                    AppError::Invalid(
                        "el importe máximo debe tener dos decimales, p. ej. 500.00".into(),
                    )
                }),
            None => Ok(()),
        },
        SEND_DRAFT
            if !nestadmin_ai::tasks::CATEGORIES.contains(&text("category").unwrap_or("")) =>
        {
            Err(AppError::Invalid(
                "la regla de envío necesita una categoría válida".into(),
            ))
        }
        SEND_DRAFT => Ok(()),
        other => Err(AppError::Invalid(format!(
            "tipo de acción desconocido: {other}"
        ))),
    }
}

fn raw_from_fields(fields: &Value) -> RawInvoice {
    let field = |name: &str| fields[name].as_str().map(|v| Evidenced::new(v.to_owned()));
    RawInvoice {
        supplier: field("supplier"),
        invoice_number: field("invoice_number"),
        date: field("date"),
        currency: field("currency"),
        subtotal: field("subtotal"),
        tax: field("tax"),
        total: field("total"),
    }
}

/// Directorio de datos por defecto en Windows (D03).
pub fn default_data_dir() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|base| Path::new(&base).join("NestAdmin"))
}
