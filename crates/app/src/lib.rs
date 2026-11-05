//! Capa de aplicación de NestAdmin (flujos 1–5 de requisitos.md). Une la cola durable, la
//! IA, la validación de facturas, los simuladores y las reglas, sin depender de la ventana:
//! la interfaz Tauri solo llama a esta capa.
//!
//! El trabajo con IA se divide en tres pasos para que la llamada al modelo, que puede tardar
//! decenas de segundos, no bloquee el resto de la aplicación: `begin_next` toma la tarea y
//! prepara la solicitud, `Job::run` llama al proveedor sin acceso a `App`, y `finish` valida,
//! aplica reglas y deja la tarea completada, en revisión o fallida.

pub mod accounts;
pub mod rules;

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use nestadmin_ai::tasks::{email_request, extraction_request, parse_email, parse_extraction};
use nestadmin_ai::{GenerateRequest, GenerateResult, Provider, ProviderKind};
use nestadmin_core::{
    ActionRecord, ActionRequest, AuditEntry, Authorization, Connector, ConnectorError, CoreError,
    Rule, Store, Task, TaskError, TaskState,
};
use nestadmin_documents::{process_invoice, DuplicateRegistry, Evidenced, Invoice, RawInvoice};
use nestadmin_extract::Extractor;
use nestadmin_mail::{OAuthConfig, Tokens};
use nestadmin_simulators::{AdminSimulator, MailSimulator, Message, NewDraft};
use serde::Serialize;
use serde_json::{json, Value};

use crate::accounts::{
    split_id, AccountFetch, AccountView, Accounts, FetchReport, MailAccountsView, MailEnv,
    MailIssue, MailProviderKind,
};
use crate::rules::{decide, RuleDecision, REGISTER_INVOICE, SEND_DRAFT};

pub const INVOICE: &str = "invoice";
pub const EMAIL: &str = "email";
/// Límite de tamaño de un documento importado.
const MAX_DOCUMENT_BYTES: usize = 50_000_000;

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
    /// Lista de cuentas y registro local de borradores de cada cuenta real.
    fn mail_accounts(&self) -> PathBuf {
        self.data_dir.join("correo")
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
    mail: Accounts,
    admin: AdminSimulator,
    provider: Arc<dyn Provider>,
    documents: PathBuf,
    registry: DuplicateRegistry,
    extractor: Arc<Extractor>,
}

/// Trabajo de una tarea: una solicitud lista, un documento del que extraer el texto o, si ya
/// se intentó ejecutar su acción, el resultado guardado entonces (D50), sin volver a la IA.
enum Prepared {
    Request(GenerateRequest),
    Document(PathBuf),
    Resume(Value),
}

/// Lo que devuelve el trabajo de una tarea.
enum Outcome {
    Generated(GenerateResult),
    Resumed(Value),
}

/// Estado de los intentos anteriores de la acción de una tarea (D50).
enum Previous {
    /// No queda ningún intento sin cerrar: se puede ejecutar.
    Clear,
    /// Una versión consta realizada.
    Done { version: i64, receipt: Value },
    /// Un intento no se pudo comprobar: el conector falló o el borrador ya no existe.
    Unknown { version: i64, error: ConnectorError },
}

/// Motivo de revisión de una tarea cuyo intento anterior no se confirmó.
const RESUMED_REASON: &str =
    "el intento anterior no se confirmó; se conserva el contenido aprobado, sin pedir otro a la IA";
/// Motivo de revisión cuando el borrador anterior ya no existe y no consta enviado.
const MISSING_REASON: &str = "el borrador del intento anterior ya no existe y no consta enviado; confirme que no se envió antes de preparar otro";

/// Tarea tomada de la cola con su trabajo preparado.
pub struct Job {
    pub task: Task,
    provider: Arc<dyn Provider>,
    extractor: Arc<Extractor>,
    prepared: Result<Prepared, TaskError>,
}

/// Respuesta del proveedor (o el motivo del fallo) lista para `finish`.
pub struct Completed {
    task: Task,
    outcome: Result<Outcome, TaskError>,
    /// Páginas del documento y método de cada una (texto, PDF u OCR).
    pages: Option<Value>,
}

impl Job {
    /// Extrae el texto si hace falta (PDF u OCR, localmente) y llama al proveedor elegido.
    /// Un error se devuelve tal cual: no hay otro proveedor ni extracción parcial.
    pub fn run(self) -> Completed {
        let Job {
            task,
            provider,
            extractor,
            prepared,
        } = self;
        let mut pages = None;
        let outcome = prepared.and_then(|prepared| {
            let request = match prepared {
                Prepared::Resume(result) => return Ok(Outcome::Resumed(result)),
                Prepared::Request(request) => request,
                Prepared::Document(path) => {
                    let extraction = extractor
                        .extract(&path)
                        .map_err(|e| task_error(e.code(), e.to_string(), e.recoverable()))?;
                    pages = serde_json::to_value(&extraction.pages).ok();
                    extraction_request(&task.id.to_string(), &extraction.text)
                }
            };
            provider
                .generate(&request)
                .map(Outcome::Generated)
                .map_err(|e| TaskError {
                    code: e.code().to_owned(),
                    message: e.to_string(),
                    recoverable: true,
                })
        });
        Completed {
            task,
            outcome,
            pages,
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
    /// Abre los datos locales solo con el correo simulado y sin proveedores reales.
    pub fn open(
        paths: &AppPaths,
        provider: Arc<dyn Provider>,
        extractor: Arc<Extractor>,
    ) -> AppResult<Self> {
        Self::open_with_mail(paths, provider, extractor, MailEnv::simulator_only())
    }

    /// Abre los datos locales y recupera el trabajo interrumpido: las tareas en curso
    /// vuelven a la cola y las acciones pendientes se reconcilian con los conectores (cada
    /// envío, con la cuenta que lo hizo).
    pub fn open_with_mail(
        paths: &AppPaths,
        provider: Arc<dyn Provider>,
        extractor: Arc<Extractor>,
        mail: MailEnv,
    ) -> AppResult<Self> {
        fs::create_dir_all(paths.documents())?;
        let simulator = MailSimulator::open(&paths.mail())?;
        let mut app = Self {
            store: Store::open(&paths.db())?,
            mail: Accounts::open(&paths.mail_accounts(), simulator, mail)?,
            admin: AdminSimulator::open(&paths.admin())?,
            provider,
            documents: paths.documents(),
            registry: DuplicateRegistry::new(),
            extractor,
        };
        app.store.recover()?;
        app.store.reconcile_actions(SEND_DRAFT, &app.mail)?;
        app.store.reconcile_actions(REGISTER_INVOICE, &app.admin)?;
        app.complete_confirmed_reviews()?;
        app.rebuild_registry()?;
        Ok(app)
    }

    /// Una tarea que seguía en revisión cuando se cortó su acción y cuya acción consta ya
    /// confirmada (por ejemplo, al reconciliar al abrir) se completa: no se vuelve a pedir
    /// aprobación de algo que ya ocurrió.
    fn complete_confirmed_reviews(&mut self) -> AppResult<()> {
        for task in self.store.tasks_in(TaskState::AwaitingReview)? {
            let kind = action_kind(&task);
            let done = self
                .store
                .task_actions(task.id)?
                .into_iter()
                .rfind(|a| a.kind == kind && a.status == "confirmed");
            if let Some(done) = done {
                let result = task.result.clone().unwrap_or(Value::Null);
                let receipt = done.receipt.unwrap_or(Value::Null);
                self.complete_with(task.id, result, done.version, receipt)?;
            }
        }
        Ok(())
    }

    /// Resultado guardado de una tarea cuya acción ya se intentó (D50): se reutiliza al
    /// reprocesarla. También cubre tareas anteriores a D50 con acciones registradas.
    fn attempted_result(&self, task: &Task) -> AppResult<Option<Value>> {
        let Some(result) = task.result.as_ref().filter(|r| r.get("action").is_some()) else {
            return Ok(None);
        };
        if result.get("attempt").is_some() || !self.store.task_actions(task.id)?.is_empty() {
            return Ok(Some(result.clone()));
        }
        Ok(None)
    }

    /// Comprueba con el conector los intentos anteriores de la acción de una tarea. La versión
    /// `executing` se excluye porque `perform_action` ya la consulta antes de repetirla. Con
    /// `supersede`, los intentos que el conector no ve realizados se cierran como sustituidos
    /// con esa autorización; sin ella se dejan como están.
    fn check_previous(
        &mut self,
        task: &Task,
        executing: Option<i64>,
        supersede: Option<&Authorization>,
    ) -> AppResult<Previous> {
        let kind = action_kind(task);
        let actions: Vec<ActionRecord> = self
            .store
            .task_actions(task.id)?
            .into_iter()
            .filter(|a| a.kind == kind)
            .collect();
        if let Some(done) = actions.iter().rev().find(|a| a.status == "confirmed") {
            return Ok(Previous::Done {
                version: done.version,
                receipt: done.receipt.clone().unwrap_or(Value::Null),
            });
        }
        let mut unsent = Vec::new();
        let mut unknown = None;
        for action in actions
            .iter()
            .filter(|a| a.is_unsettled() && Some(a.version) != executing)
        {
            let found = match kind {
                REGISTER_INVOICE => self.admin.lookup(&action.idempotency_key),
                _ => self.mail.lookup(&action.idempotency_key),
            };
            match found {
                Ok(Some(receipt)) => {
                    self.store.reconcile_action(task.id, action, &receipt)?;
                    return Ok(Previous::Done {
                        version: action.version,
                        receipt,
                    });
                }
                Ok(None) => unsent.push(action),
                Err(error) => {
                    unknown.get_or_insert((action.version, error));
                }
            }
        }
        if let Some((version, error)) = unknown {
            return Ok(Previous::Unknown { version, error });
        }
        if let Some(authorization) = supersede {
            for action in unsent {
                self.store.supersede_action(
                    task.id,
                    action,
                    authorization,
                    "el conector no lo ve realizado; lo sustituye otra versión",
                )?;
            }
        }
        Ok(Previous::Clear)
    }

    /// Completa la tarea con el recibo de una versión ya realizada. Si no es la versión que
    /// se muestra, se recupera el contenido de ese intento y se indica cuál salió.
    fn complete_with(
        &mut self,
        task_id: i64,
        mut result: Value,
        version: i64,
        receipt: Value,
    ) -> AppResult<Task> {
        if result["version"].as_i64() != Some(version) {
            if result["attempt"]["version"].as_i64() == Some(version)
                && !result["attempt"]["draft"].is_null()
            {
                result["draft"] = result["attempt"]["draft"].clone();
                result["version"] = json!(version);
            } else {
                result["sent_version"] = json!(version);
            }
        }
        result["receipt"] = receipt;
        // Para la ventana: se completó con un intento anterior, no con una ejecución nueva.
        result["recovered"] = json!({ "version": version });
        if let Some(fields) = result.as_object_mut() {
            fields.remove("recovery");
        }
        Ok(self.store.complete(task_id, &result)?)
    }

    /// Deja la tarea en revisión con el contenido dado, desde `running` o desde la revisión.
    fn back_to_review(&mut self, task: &Task, result: &Value) -> AppResult<Task> {
        let current = self.store.task(task.id)?;
        Ok(match current.state {
            TaskState::AwaitingReview => self.store.revise(task.id, result)?,
            _ => self.store.await_review(task.id, result)?,
        })
    }

    /// Reproceso de una tarea cuya acción ya se intentó: si consta realizada se completa; si
    /// no, vuelve a revisión con el mismo contenido y versión, explicando lo ocurrido. Nunca se
    /// ejecuta nada sin una nueva aprobación.
    fn resume(&mut self, task: Task, mut result: Value) -> AppResult<Task> {
        let attempted = result["attempt"]["version"]
            .as_i64()
            .or_else(|| result["version"].as_i64())
            .unwrap_or(1);
        let (error, missing) = match self.check_previous(&task, None, None)? {
            Previous::Done { version, receipt } => {
                return self.complete_with(task.id, result, version, receipt)
            }
            Previous::Unknown { error, .. } => {
                let missing = error.code == nestadmin_simulators::codes::NOT_FOUND;
                (Some(error.code), missing)
            }
            Previous::Clear => (task.error.as_ref().map(|e| e.code.clone()), false),
        };
        result["review_reason"] = json!(if missing {
            MISSING_REASON
        } else {
            RESUMED_REASON
        });
        result["recovery"] = json!({
            "version": attempted,
            "error": error,
            "needs_confirmation": missing,
        });
        self.back_to_review(&task, &result)
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

    /// Encola un documento de texto.
    pub fn import_document(&mut self, name: &str, text: &str) -> AppResult<Task> {
        if text.trim().is_empty() {
            return Err(AppError::Invalid("el documento está vacío".into()));
        }
        self.store_document(name, "txt", text.as_bytes())
    }

    /// Encola un archivo `.txt` o `.pdf`. Se guarda una copia en los datos locales; la
    /// extracción (con OCR si hace falta) se hace al procesar la tarea.
    pub fn import_file(&mut self, name: &str, bytes: &[u8]) -> AppResult<Task> {
        let extension = Path::new(name)
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .unwrap_or_default();
        if extension != "txt" && extension != "pdf" {
            return Err(AppError::Invalid(format!(
                "«{name}»: solo se admiten archivos .txt y .pdf"
            )));
        }
        if bytes.is_empty() {
            return Err(AppError::Invalid(format!("«{name}» está vacío")));
        }
        if bytes.len() > MAX_DOCUMENT_BYTES {
            return Err(AppError::Invalid(format!(
                "«{name}» supera el tamaño máximo de {} MB",
                MAX_DOCUMENT_BYTES / 1_000_000
            )));
        }
        self.store_document(name, &extension, bytes)
    }

    fn store_document(&mut self, name: &str, extension: &str, bytes: &[u8]) -> AppResult<Task> {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let path = self.documents.join(format!("{stamp}.{extension}"));
        fs::write(&path, bytes)?;
        let input = json!({ "document": name, "path": path });
        Ok(self.store.enqueue(INVOICE, &input.to_string())?)
    }

    /// Consulta cada cuenta (como mucho el límite de mensajes por cuenta) y encola los
    /// mensajes que aún no tienen tarea. El mensaje se guarda en la tarea para no volver a
    /// pedirlo al proveedor. El fallo de una cuenta queda en el informe y no detiene a las
    /// demás.
    pub fn fetch_mail(&mut self) -> AppResult<FetchReport> {
        let mut known: HashSet<String> = HashSet::new();
        for task in self
            .store
            .all_tasks()?
            .into_iter()
            .filter(|t| t.kind == EMAIL)
        {
            let input: Value = serde_json::from_str(&task.input_ref).unwrap_or(Value::Null);
            if let Some(id) = input["message_id"].as_str() {
                let (account, inner) = split_id(id);
                known.insert(accounts::join_id(account, inner));
            }
        }
        let mut report = FetchReport {
            added: 0,
            limit: self.mail.limit(),
            accounts: Vec::new(),
        };
        for (account, outcome) in self.mail.read_all() {
            let fetch = match outcome {
                Ok(messages) => {
                    let mut added = 0;
                    for message in &messages {
                        if known.insert(message.id.clone()) {
                            let input = json!({
                                "message_id": message.id,
                                "account": account,
                                "message": message,
                            });
                            self.store.enqueue(EMAIL, &input.to_string())?;
                            added += 1;
                        }
                    }
                    AccountFetch {
                        account,
                        read: messages.len(),
                        added,
                        error: None,
                    }
                }
                Err(error) => AccountFetch {
                    account,
                    read: 0,
                    added: 0,
                    error: Some(MailIssue::from(&error)),
                },
            };
            report.added += fetch.added;
            self.mail.record_fetch(fetch.clone());
            report.accounts.push(fetch);
        }
        Ok(report)
    }

    /// Mensaje de una tarea de correo: el guardado al encolarla o, en tareas anteriores a las
    /// cuentas, el que devuelva su cuenta.
    fn task_message(&self, input: &Value) -> Result<Option<Message>, ConnectorError> {
        if let Ok(message) = serde_json::from_value::<Message>(input["message"].clone()) {
            return Ok(Some(message));
        }
        self.mail
            .find_message(input["message_id"].as_str().unwrap_or(""))
    }

    /// Toma la siguiente tarea y prepara su solicitud. `None` si la cola está vacía.
    pub fn begin_next(&mut self) -> AppResult<Option<Job>> {
        let Some(task) = self.store.claim_next()? else {
            return Ok(None);
        };
        let input: Value = serde_json::from_str(&task.input_ref).unwrap_or(Value::Null);
        let id = task.id.to_string();
        if let Some(result) = self.attempted_result(&task)? {
            return Ok(Some(Job {
                task,
                provider: Arc::clone(&self.provider),
                extractor: Arc::clone(&self.extractor),
                prepared: Ok(Prepared::Resume(result)),
            }));
        }
        let prepared = match task.kind.as_str() {
            INVOICE => match input["path"].as_str() {
                Some(path) => Ok(Prepared::Document(PathBuf::from(path))),
                None => Err(task_error(
                    "document_unreadable",
                    "la tarea no indica el documento",
                    false,
                )),
            },
            EMAIL => match self.task_message(&input) {
                Err(e) => Err(task_error(&e.code, e.message, true)),
                Ok(found) => match found {
                    Some(Message {
                        body: Some(body),
                        subject,
                        ..
                    }) => {
                        let text = match subject {
                            Some(subject) => format!("Asunto: {subject}\n\n{body}"),
                            None => body,
                        };
                        Ok(Prepared::Request(email_request(&id, &text)))
                    }
                    Some(_) => Err(task_error(
                        "empty_message",
                        "el mensaje no tiene cuerpo",
                        false,
                    )),
                    None => Err(task_error("not_found", "el mensaje ya no existe", false)),
                },
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
            extractor: Arc::clone(&self.extractor),
            prepared,
        }))
    }

    /// Valida la respuesta, aplica reglas y deja la tarea en su estado siguiente.
    pub fn finish(&mut self, completed: Completed) -> AppResult<Task> {
        let task = completed.task;
        let generated = match completed.outcome {
            Ok(Outcome::Generated(generated)) => generated,
            Ok(Outcome::Resumed(result)) => return self.resume(task, result),
            Err(error) => return Ok(self.store.fail(task.id, &error)?),
        };
        let built = match task.kind.as_str() {
            INVOICE => self.invoice_result(&task, &generated, completed.pages),
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
        pages: Option<Value>,
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
            "pages": pages,
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
            .task_message(&input)
            .map_err(|e| task_error(&e.code, e.message, true))?
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
        // Antes de ejecutar otra versión, los intentos anteriores deben constar como no
        // realizados: si alguno salió, la tarea se completa con él y no se ejecuta otro.
        match self.check_previous(&task, Some(version), Some(&authorization))? {
            Previous::Clear => {}
            Previous::Done { version, receipt } => {
                return self.complete_with(task.id, result, version, receipt)
            }
            Previous::Unknown { version, error }
                if error.code == nestadmin_simulators::codes::NOT_FOUND =>
            {
                result["review_reason"] = json!(MISSING_REASON);
                result["recovery"] = json!({
                    "version": version,
                    "error": error.code,
                    "needs_confirmation": true,
                });
                return self.back_to_review(&task, &result);
            }
            Previous::Unknown { error, .. } => {
                return Ok(self.store.fail(
                    task.id,
                    &task_error(
                        &error.code,
                        format!("no se pudo comprobar el intento anterior: {error}"),
                        true,
                    ),
                )?)
            }
        }
        // Se guarda lo que se va a ejecutar antes de llamar a ningún conector (D50).
        let draft = match task.kind.as_str() {
            INVOICE => Value::Null,
            _ => result["draft"].clone(),
        };
        result["attempt"] = json!({ "version": version, "draft": draft });
        if let Some(fields) = result.as_object_mut() {
            fields.remove("recovery");
        }
        self.store.record_attempt(task.id, &result)?;
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

    /// El usuario confirma que el intento anterior no se envió (o registró), por ejemplo
    /// porque borró el borrador. Antes se pregunta al conector: si consta realizado, la tarea
    /// se completa con ese recibo. Si no, los intentos se cierran como sustituidos y el mismo
    /// contenido pasa a una versión nueva, con claves nuevas, que requiere aprobación (D50).
    pub fn confirm_not_sent(&mut self, task_id: i64) -> AppResult<Task> {
        let (task, mut result) = self.reviewable(task_id)?;
        let kind = action_kind(&task);
        let actions: Vec<ActionRecord> = self
            .store
            .task_actions(task.id)?
            .into_iter()
            .filter(|a| a.kind == kind)
            .collect();
        if result.get("attempt").is_none() && actions.is_empty() {
            return Err(AppError::Invalid(format!(
                "la tarea {task_id} no tiene ningún intento anterior"
            )));
        }
        if let Some(done) = actions.iter().rev().find(|a| a.status == "confirmed") {
            let receipt = done.receipt.clone().unwrap_or(Value::Null);
            return self.complete_with(task.id, result, done.version, receipt);
        }
        let mut unsent = Vec::new();
        for action in actions.iter().filter(|a| a.is_unsettled()) {
            let found = match kind {
                REGISTER_INVOICE => self.admin.lookup(&action.idempotency_key),
                _ => self.mail.lookup(&action.idempotency_key),
            };
            match found {
                Ok(Some(receipt)) => {
                    self.store.reconcile_action(task.id, action, &receipt)?;
                    return self.complete_with(task.id, result, action.version, receipt);
                }
                Ok(None) => unsent.push(action),
                Err(e) if e.code == nestadmin_simulators::codes::NOT_FOUND => unsent.push(action),
                Err(e) => return Err(AppError::Connector(e)),
            }
        }
        for action in unsent {
            self.store.supersede_action(
                task.id,
                action,
                &Authorization::Approval,
                "el usuario confirmó que no se realizó",
            )?;
        }
        let version = result["version"].as_i64().unwrap_or(1);
        result["version"] = json!(version + 1);
        result["review_reason"] =
            json!("versión nueva tras confirmar que el intento anterior no se realizó");
        if let Some(fields) = result.as_object_mut() {
            fields.remove("recovery");
        }
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
        (self.mail.simulator(), &self.admin)
    }

    /// Cuentas de correo, su estado y el límite de mensajes por consulta.
    pub fn mail_accounts(&self) -> MailAccountsView {
        self.mail.view()
    }

    /// Mensajes leídos como mucho por cuenta en cada consulta (1 a 500).
    pub fn set_mail_limit(&mut self, limit: usize) -> AppResult<()> {
        self.mail.set_limit(limit).map_err(AppError::Invalid)
    }

    /// Cliente OAuth para iniciar sesión; error si esta instalación no lo tiene configurado.
    pub fn mail_oauth(&self, provider: MailProviderKind) -> AppResult<OAuthConfig> {
        Ok(self.mail.oauth(provider)?)
    }

    /// Añade una cuenta con los tokens obtenidos al iniciar sesión.
    pub fn add_mail_account(
        &mut self,
        provider: MailProviderKind,
        label: &str,
        tokens: &Tokens,
    ) -> AppResult<AccountView> {
        Ok(self.mail.add(provider, label, tokens)?)
    }

    /// Guarda tokens nuevos para una cuenta existente.
    pub fn reconnect_mail_account(&mut self, id: &str, tokens: &Tokens) -> AppResult<AccountView> {
        Ok(self.mail.reconnect(id, tokens)?)
    }

    /// Vuelve a añadir la cuenta simulada si se había quitado.
    pub fn add_simulated_mail_account(&mut self) -> AppResult<AccountView> {
        Ok(self.mail.add_simulator()?)
    }

    /// Desconecta una cuenta: borra sus tokens y la quita de la lista. Las tareas ya
    /// encoladas de esa cuenta siguen visibles; enviar desde ellas falla con el motivo.
    pub fn remove_mail_account(&mut self, id: &str) -> AppResult<()> {
        Ok(self.mail.remove(id)?)
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
