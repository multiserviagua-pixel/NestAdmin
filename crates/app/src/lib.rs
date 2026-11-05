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
    ActionRequest, AuditEntry, Authorization, ConnectorError, CoreError, Rule, Store, Task,
    TaskError, TaskState,
};
use nestadmin_documents::{process_invoice, DuplicateRegistry, Evidenced, Invoice, RawInvoice};
use nestadmin_extract::{Extraction, Extractor};
use nestadmin_mail::{OAuthConfig, Tokens};
use nestadmin_simulators::{AdminSimulator, MailSimulator, Message, NewDraft};
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

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

/// Trabajo de una tarea: una solicitud lista o un documento del que extraer el texto.
enum Prepared {
    Request(GenerateRequest),
    Document(PathBuf),
}

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
    outcome: Result<GenerateResult, TaskError>,
    /// Texto y páginas del documento (método de cada una: texto, PDF u OCR).
    extraction: Option<Extraction>,
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
        let mut extracted = None;
        let outcome = prepared.and_then(|prepared| {
            let request = match prepared {
                Prepared::Request(request) => request,
                Prepared::Document(path) => {
                    let extraction = extractor
                        .extract(&path)
                        .map_err(|e| task_error(e.code(), e.to_string(), e.recoverable()))?;
                    let request = extraction_request(&task.id.to_string(), &extraction.text);
                    extracted = Some(extraction);
                    request
                }
            };
            provider.generate(&request).map_err(|e| TaskError {
                code: e.code().to_owned(),
                message: e.to_string(),
                recoverable: true,
            })
        });
        Completed {
            task,
            outcome,
            extraction: extracted,
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

    /// Encola un documento de texto.
    pub fn import_document(&mut self, name: &str, text: &str) -> AppResult<Task> {
        if text.trim().is_empty() {
            return Err(AppError::Invalid("el documento está vacío".into()));
        }
        let origin = json!({ "kind": "file", "name": name, "original_path": null });
        self.store_document(name, "txt", text.as_bytes(), origin)
    }

    /// Encola un archivo `.txt` o `.pdf` del que no se conoce la ruta original.
    pub fn import_file(&mut self, name: &str, bytes: &[u8]) -> AppResult<Task> {
        self.import_file_from(name, None, bytes)
    }

    /// Lee un archivo del disco y lo encola anotando su ruta original como origen.
    pub fn import_path(&mut self, path: &Path) -> AppResult<Task> {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| AppError::Invalid("el archivo no tiene un nombre válido".into()))?
            .to_owned();
        let size = fs::metadata(path)?.len();
        if size > MAX_DOCUMENT_BYTES as u64 {
            return Err(AppError::Invalid(format!(
                "«{name}» supera el tamaño máximo de {} MB",
                MAX_DOCUMENT_BYTES / 1_000_000
            )));
        }
        let bytes = fs::read(path)?;
        self.import_file_from(&name, Some(path), &bytes)
    }

    /// Encola un archivo `.txt` o `.pdf`. Se guarda una copia en los datos locales junto con
    /// su origen (nombre, ruta original si se conoce, fecha de importación y SHA-256); la
    /// extracción (con OCR si hace falta) se hace al procesar la tarea.
    pub fn import_file_from(
        &mut self,
        name: &str,
        original: Option<&Path>,
        bytes: &[u8],
    ) -> AppResult<Task> {
        let extension = check_document(name, bytes)?;
        let origin = json!({
            "kind": "file",
            "name": name,
            // Ruta tal como la dio el sistema; `null` si no se conoce (texto pegado o un
            // selector que no la facilita).
            "original_path": original.map(|p| p.display().to_string()),
        });
        self.store_document(name, &extension, bytes, origin)
    }

    /// Encola un adjunto de un correo ya recibido (`message_task`, la tarea de ese correo).
    /// El origen copia la cuenta, el remitente y el asunto guardados en esa tarea, de modo que
    /// la factura dice de qué buzón y de qué mensaje salió (D49).
    pub fn import_attachment(
        &mut self,
        message_task: i64,
        name: &str,
        bytes: &[u8],
    ) -> AppResult<Task> {
        let task = self.store.task(message_task)?;
        if task.kind != EMAIL {
            return Err(AppError::Invalid(format!(
                "la tarea {message_task} no es un correo"
            )));
        }
        let extension = check_document(name, bytes)?;
        let input: Value = serde_json::from_str(&task.input_ref).unwrap_or(Value::Null);
        let message = &input["message"];
        let account = match input.get("origin").filter(|o| !o.is_null()) {
            Some(origin) => origin.clone(),
            None => {
                let id = input["message_id"].as_str().unwrap_or("");
                serde_json::to_value(self.mail.origin(split_id(id).0)).unwrap_or_default()
            }
        };
        let origin = json!({
            "kind": "email",
            "account": account,
            "message_id": input["message_id"],
            "from": message["from"],
            "subject": message["subject"],
            "attachment": name,
        });
        self.store_document(name, &extension, bytes, origin)
    }

    /// Guarda la copia y encola la tarea. `origin` dice de dónde vino; aquí se añaden la
    /// fecha de importación, el tamaño y la huella SHA-256 del contenido.
    fn store_document(
        &mut self,
        name: &str,
        extension: &str,
        bytes: &[u8],
        mut origin: Value,
    ) -> AppResult<Task> {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).ok();
        let stamp = now.map(|d| d.as_nanos()).unwrap_or(0);
        let path = self.documents.join(format!("{stamp}.{extension}"));
        fs::write(&path, bytes)?;
        origin["imported_at"] = json!(now.map(|d| d.as_secs()));
        origin["sha256"] = json!(hex(&Sha256::digest(bytes)));
        origin["size"] = json!(bytes.len());
        let input = json!({ "document": name, "path": path, "origin": origin });
        Ok(self.store.enqueue(INVOICE, &input.to_string())?)
    }

    /// Copia guardada del documento de una tarea, para abrirla con el visor del sistema. Solo
    /// se devuelve si está dentro de la carpeta de documentos de NestAdmin y sigue existiendo.
    pub fn document_file(&mut self, task_id: i64) -> AppResult<PathBuf> {
        let task = self.store.task(task_id)?;
        if task.kind != INVOICE {
            return Err(AppError::Invalid(format!(
                "la tarea {task_id} no tiene documento"
            )));
        }
        let input: Value = serde_json::from_str(&task.input_ref).unwrap_or(Value::Null);
        let missing = || {
            AppError::Invalid(format!(
                "la copia guardada del documento de la tarea {task_id} ya no está"
            ))
        };
        let path = input["path"]
            .as_str()
            .map(PathBuf::from)
            .ok_or_else(missing)?;
        let folder = fs::canonicalize(&self.documents)?;
        let file = fs::canonicalize(&path).map_err(|_| missing())?;
        if !file.starts_with(&folder) || !file.is_file() {
            return Err(AppError::Invalid(
                "el documento no está en la carpeta de NestAdmin".into(),
            ));
        }
        Ok(file)
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
                                "origin": self.mail.origin(&account),
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
            Ok(generated) => generated,
            Err(error) => return Ok(self.store.fail(task.id, &error)?),
        };
        let built = match task.kind.as_str() {
            INVOICE => self.invoice_result(&task, &generated, completed.extraction.as_ref()),
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
        extraction: Option<&Extraction>,
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
        let pages = extraction.and_then(|e| serde_json::to_value(&e.pages).ok());
        let sources: serde_json::Map<String, Value> = extraction
            .map(|extraction| {
                fields
                    .iter()
                    .filter_map(|(name, value)| {
                        let source = field_source(extraction, name, value.as_str()?)?;
                        Some((name.clone(), source))
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(json!({
            "type": INVOICE,
            "version": 1,
            "document": input["document"],
            "origin": input["origin"],
            "pages": pages,
            "fields": fields,
            "sources": sources,
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
        // Tareas anteriores a D48 no guardaron el origen: se toma el de la cuenta actual.
        let origin = match input.get("origin").filter(|o| !o.is_null()) {
            Some(origin) => origin.clone(),
            None => {
                serde_json::to_value(self.mail.origin(split_id(&message.id).0)).unwrap_or_default()
            }
        };
        Ok(json!({
            "type": EMAIL,
            "version": 1,
            "origin": origin,
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

    /// Cambia el nombre visible de una cuenta; vacío vuelve al predeterminado. Las tareas ya
    /// encoladas conservan el nombre con que llegaron; la ventana muestra el actual.
    pub fn rename_mail_account(&mut self, id: &str, label: &str) -> AppResult<AccountView> {
        Ok(self.mail.rename(id, label)?)
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

/// Comprueba nombre, tamaño y contenido de un documento; devuelve su extensión.
fn check_document(name: &str, bytes: &[u8]) -> AppResult<String> {
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
    Ok(extension)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Formas en que un valor normalizado puede aparecer escrito en el documento: el importe
/// `1234.50` como `1234,50`, `1.234,50` o `1,234.50`; la fecha `2026-10-05` como `05/10/2026`.
fn written_forms(field: &str, value: &str) -> Vec<String> {
    let mut forms = vec![value.to_owned()];
    match field {
        "subtotal" | "tax" | "total" => {
            if let Some((units, cents)) = value.split_once('.') {
                let (sign, digits) = match units.strip_prefix('-') {
                    Some(rest) => ("-", rest),
                    None => ("", units),
                };
                let grouped = |separator: char| {
                    let mut out = String::new();
                    for (i, c) in digits.chars().enumerate() {
                        if i > 0 && (digits.len() - i) % 3 == 0 {
                            out.push(separator);
                        }
                        out.push(c);
                    }
                    out
                };
                forms.push(format!("{units},{cents}"));
                forms.push(format!("{sign}{},{cents}", grouped('.')));
                forms.push(format!("{sign}{}.{cents}", grouped(',')));
            }
        }
        "date" => {
            let parts: Vec<&str> = value.split('-').collect();
            if let [year, month, day] = parts[..] {
                for separator in ['/', '-', '.'] {
                    forms.push(format!("{day}{separator}{month}{separator}{year}"));
                }
                let (d, m) = (day.trim_start_matches('0'), month.trim_start_matches('0'));
                forms.push(format!("{d}/{m}/{year}"));
            }
        }
        _ => {}
    }
    let mut unique = Vec::new();
    for form in forms {
        if !unique.contains(&form) {
            unique.push(form);
        }
    }
    unique
}

/// Dónde aparece un campo en el texto extraído: páginas y la línea de la primera aparición.
/// `None` si no se encuentra literalmente (el modelo pudo reescribirlo): nunca se adivina.
fn field_source(extraction: &Extraction, field: &str, value: &str) -> Option<Value> {
    for form in written_forms(field, value) {
        let pages = extraction.pages_containing(&form);
        if pages.is_empty() {
            continue;
        }
        let text = &extraction.text;
        let at = text.find(&form)?;
        let start = text[..at].rfind('\n').map_or(0, |i| i + 1);
        let end = text[at..].find('\n').map_or(text.len(), |i| at + i);
        let line: String = text[start..end].trim().chars().take(160).collect();
        return Some(json!({ "pages": pages, "text": form, "line": line }));
    }
    None
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

#[cfg(test)]
mod tests {
    use super::*;
    use nestadmin_extract::{Method, Page};

    fn page(number: usize, offset: usize) -> Page {
        Page {
            number,
            method: Method::PdfText,
            chars: 1,
            offset,
        }
    }

    #[test]
    fn amounts_and_dates_are_found_as_written_with_their_page() {
        let first = "Proveedor Alfa SL\nFecha: 05/10/2026";
        let text = format!("{first}\n\nBase 1.234,50\nTotal: 1.493,75 EUR");
        let extraction = Extraction {
            pages: vec![page(1, 0), page(2, first.len() + 2)],
            text,
        };
        let total = field_source(&extraction, "total", "1493.75").unwrap();
        assert_eq!(total["pages"], json!([2]));
        assert_eq!(total["line"], "Total: 1.493,75 EUR");
        let date = field_source(&extraction, "date", "2026-10-05").unwrap();
        assert_eq!(date["pages"], json!([1]));
        assert_eq!(date["text"], "05/10/2026");
        assert!(
            field_source(&extraction, "tax", "259.25").is_none(),
            "lo que no aparece en el texto no tiene página"
        );
    }
}
