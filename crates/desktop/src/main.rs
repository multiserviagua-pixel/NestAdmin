//! Ventana de NestAdmin (R01, flujo 5). Cerrar oculta la ventana y el trabajo sigue en un
//! hilo propio; el icono del área de notificación permite restaurarla o salir por completo.
//! La lógica vive en `nestadmin-app`; aquí solo se exponen comandos a la ventana principal.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod mail_clients;
mod probe;
mod settings;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::Duration;

use nestadmin_ai::{CredentialStore, ProviderChoice, Secret};
use nestadmin_app::accounts::{
    AccountView, FetchReport, MailAccountsView, MailEnv, MailProviderKind,
};
use nestadmin_app::initial_read::{
    FolderPreview, InitialReadOverview, InitialReadStatus, ReadExpectation, ReadStep,
};
use nestadmin_app::panel::{MergePreview, NewRule, RuleView, Simulation};
use nestadmin_app::preferences::PreferenceChange;
use nestadmin_app::refine::{
    ConsentView, RefineExpectation, RefineMode, RefinePreview, RefineTrigger, RefinementView,
};
use nestadmin_app::rules::RuleFinding;
use nestadmin_app::suggest::{RuleSeed, SeenInvoice, Suggestions, ThresholdSettings};
use nestadmin_app::{AiWait, App, AppPaths, LocalFallback, ProviderInfo};
use nestadmin_core::{AuditEntry, Rule, RuleStats, SenderPreference, Task};
use nestadmin_extract::{Extractor, Tesseract};
use nestadmin_mail::PendingAuthorization;
use nestadmin_setup::{Cancel, Report, Setup, SetupError, Startup, Status, WindowsSystem};
use serde::Serialize;
use serde_json::Value;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Builder, Emitter, Manager, Runtime, State, Window, WindowEvent};
use tauri_plugin_dialog::DialogExt;

use crate::settings::{credential_store, Settings};

/// Evento que indica a la ventana que vuelva a leer tareas e historial.
const CHANGED: &str = "nestadmin://changed";
/// Progreso de cada paso de preparación.
const SETUP: &str = "nestadmin://setup";
/// Final de una preparación, con su informe o su error.
const SETUP_DONE: &str = "nestadmin://setup-done";
/// Resultado de la comprobación al abrir (D39).
const STARTUP: &str = "nestadmin://startup";
/// Dirección de inicio de sesión de correo, por si el navegador no se abrió solo.
const MAIL_SIGN_IN: &str = "nestadmin://mail-sign-in";
/// Tiempo máximo para completar el inicio de sesión en el navegador.
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(300);
/// Argumento con el que el instalador lanza solo la preparación (D36).
const PREPARE_ARG: &str = "--preparar";

struct Shared {
    app: Mutex<App>,
    settings: Mutex<Settings>,
    credentials: Arc<dyn CredentialStore>,
    data_dir: PathBuf,
    /// Comprobación de la IA local para el respaldo (D101): comprueba, arranca si estaba
    /// detenida, y nunca descarga ni instala nada.
    local_setup: Arc<Setup>,
    /// Inicio de sesión de correo en curso; solo uno a la vez.
    sign_in: Mutex<Option<SignIn>>,
    /// Motivo por el que no se pudo leer `correo-oauth.json`, si lo hay.
    mail_clients_error: Option<String>,
    wake: Mutex<Sender<()>>,
    stopping: AtomicBool,
}

/// Preparación de componentes locales; existe en los dos modos de arranque.
struct Preparation {
    setup: Arc<Setup>,
    /// Modo `--preparar`: sin base de datos ni bandeja; el código de salida informa al instalador.
    prepare_only: bool,
    /// Cancelación de la preparación en curso; solo una a la vez.
    running: Mutex<Option<Cancel>>,
    /// Último resultado de la comprobación al abrir, por si la ventana aún no escuchaba.
    startup: Mutex<Option<Startup>>,
}

impl Shared {
    fn app(&self) -> Result<MutexGuard<'_, App>, String> {
        self.app
            .lock()
            .map_err(|_| "estado interno no disponible".to_owned())
    }

    fn wake(&self) {
        if let Ok(sender) = self.wake.lock() {
            let _ = sender.send(());
        }
    }
}

struct SignIn {
    redirect_uri: String,
    cancelled: bool,
}

type SharedState = Arc<Shared>;
type CmdResult<T> = Result<T, String>;

fn changed<R: Runtime>(handle: &AppHandle<R>) {
    let _ = handle.emit(CHANGED, ());
}

fn text<E: ToString>(error: E) -> String {
    error.to_string()
}

#[tauri::command]
fn tasks(shared: State<'_, SharedState>) -> CmdResult<Vec<Task>> {
    shared.app()?.tasks().map_err(text)
}

/// Recibe el archivo como cuerpo binario de la llamada y su nombre, codificado con
/// `encodeURIComponent`, en la cabecera `x-nestadmin-name`.
#[tauri::command]
fn import_file<R: Runtime>(
    shared: State<'_, SharedState>,
    handle: AppHandle<R>,
    request: tauri::ipc::Request<'_>,
) -> CmdResult<Task> {
    let tauri::ipc::InvokeBody::Raw(bytes) = request.body() else {
        return Err("el archivo debe enviarse como datos binarios".into());
    };
    let name = request
        .headers()
        .get("x-nestadmin-name")
        .and_then(|v| v.to_str().ok())
        .and_then(percent_decode)
        .ok_or("falta el nombre del archivo")?;
    let task = shared.app()?.import_file(&name, bytes).map_err(text)?;
    shared.wake();
    changed(&handle);
    Ok(task)
}

/// Resultado de importar desde el selector: lo importado y, aparte, cada archivo rechazado.
#[derive(Serialize)]
struct ImportReport {
    imported: Vec<Task>,
    errors: Vec<String>,
}

/// Abre el selector de archivos del sistema y encola lo elegido leyéndolo del disco, de modo
/// que el origen guarda la ruta original (D49). Cancelar el selector no importa nada.
#[tauri::command]
async fn import_dialog<R: Runtime>(
    shared: State<'_, SharedState>,
    handle: AppHandle<R>,
) -> CmdResult<ImportReport> {
    let shared = Arc::clone(shared.inner());
    let worker = Arc::clone(&shared);
    let dialog = handle.clone();
    let report = tauri::async_runtime::spawn_blocking(move || {
        let picked = dialog
            .dialog()
            .file()
            .set_title("Importar documentos")
            .add_filter(
                "Facturas (PDF, imagen o texto)",
                &[
                    "pdf", "jpg", "jpeg", "png", "tif", "tiff", "bmp", "webp", "txt",
                ],
            )
            .blocking_pick_files()
            .unwrap_or_default();
        let mut report = ImportReport {
            imported: Vec::new(),
            errors: Vec::new(),
        };
        for file in picked {
            let path = match file.into_path() {
                Ok(path) => path,
                Err(e) => {
                    report.errors.push(format!("ruta no válida: {e}"));
                    continue;
                }
            };
            match worker.app()?.import_path(&path) {
                Ok(task) => report.imported.push(task),
                Err(e) => report.errors.push(e.to_string()),
            }
        }
        Ok::<_, String>(report)
    })
    .await
    .map_err(text)??;
    if !report.imported.is_empty() {
        shared.wake();
        changed(&handle);
    }
    Ok(report)
}

/// Abre con el visor del sistema la copia guardada del documento de una tarea. Solo se abren
/// archivos de la carpeta de documentos de NestAdmin, nunca una ruta que envíe la ventana.
#[tauri::command]
fn open_original(shared: State<'_, SharedState>, id: i64) -> CmdResult<()> {
    let path = shared.app()?.document_file(id).map_err(text)?;
    open_file(&path)
}

/// Abre un archivo local con la aplicación predeterminada sin pasar por un intérprete.
fn open_file(path: &std::path::Path) -> CmdResult<()> {
    #[cfg(windows)]
    {
        std::process::Command::new("explorer")
            .arg(path)
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("no se pudo abrir el visor del sistema: {e}"))
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        Err("abrir el original solo está disponible en Windows".into())
    }
}

/// Decodifica `%XX` (UTF-8); `None` si la secuencia no es válida.
fn percent_decode(text: &str) -> Option<String> {
    let mut bytes = Vec::with_capacity(text.len());
    let mut input = text.bytes();
    while let Some(byte) = input.next() {
        if byte == b'%' {
            let hex = [input.next()?, input.next()?];
            bytes.push(u8::from_str_radix(std::str::from_utf8(&hex).ok()?, 16).ok()?);
        } else {
            bytes.push(byte);
        }
    }
    String::from_utf8(bytes).ok()
}

/// Consulta las cuentas. Puede tardar si un proveedor responde despacio, así que se hace
/// fuera del hilo de la ventana. El informe dice qué cuenta falló y por qué.
#[tauri::command]
async fn fetch_mail<R: Runtime>(
    shared: State<'_, SharedState>,
    handle: AppHandle<R>,
) -> CmdResult<FetchReport> {
    let shared = Arc::clone(shared.inner());
    let worker = Arc::clone(&shared);
    let report =
        tauri::async_runtime::spawn_blocking(move || worker.app()?.fetch_mail().map_err(text))
            .await
            .map_err(text)??;
    shared.wake();
    changed(&handle);
    Ok(report)
}

#[derive(Serialize)]
struct MailView {
    #[serde(flatten)]
    accounts: MailAccountsView,
    /// `correo-oauth.json` existe pero no se pudo leer.
    clients_error: Option<String>,
    signing_in: bool,
}

#[tauri::command]
fn mail_accounts(shared: State<'_, SharedState>) -> CmdResult<MailView> {
    Ok(MailView {
        accounts: shared.app()?.mail_accounts(),
        clients_error: shared.mail_clients_error.clone(),
        signing_in: shared.sign_in.lock().map_err(text)?.is_some(),
    })
}

#[tauri::command]
fn set_mail_limit(shared: State<'_, SharedState>, limit: usize) -> CmdResult<()> {
    shared.app()?.set_mail_limit(limit).map_err(text)?;
    let mut settings = shared.settings.lock().map_err(text)?;
    let updated = Settings {
        mail_limit: limit,
        ..settings.clone()
    };
    updated.save(&shared.data_dir).map_err(text)?;
    *settings = updated;
    Ok(())
}

/// Inicia sesión en Google o Microsoft con OAuth y PKCE (D31): abre el navegador del sistema
/// y espera la redirección en 127.0.0.1. Con `account`, renueva la sesión de esa cuenta; sin
/// ella, añade una cuenta nueva. Los tokens van directos al almacén de credenciales.
#[tauri::command]
async fn mail_sign_in<R: Runtime>(
    shared: State<'_, SharedState>,
    handle: AppHandle<R>,
    provider: MailProviderKind,
    label: String,
    account: Option<String>,
    expect: Option<ReadExpectation>,
) -> CmdResult<AccountView> {
    let shared = Arc::clone(shared.inner());
    let emitter = handle.clone();
    let view = tauri::async_runtime::spawn_blocking(move || {
        sign_in(
            &shared,
            &emitter,
            provider,
            &label,
            account.as_deref(),
            expect.as_ref(),
        )
    })
    .await
    .map_err(text)??;
    changed(&handle);
    Ok(view)
}

fn sign_in<R: Runtime>(
    shared: &Shared,
    handle: &AppHandle<R>,
    provider: MailProviderKind,
    label: &str,
    account: Option<&str>,
    expect: Option<&ReadExpectation>,
) -> CmdResult<AccountView> {
    let config = shared.app()?.mail_oauth(provider).map_err(text)?;
    let pending = {
        let mut running = shared.sign_in.lock().map_err(text)?;
        if running.is_some() {
            return Err("ya hay un inicio de sesión en curso".into());
        }
        let pending = PendingAuthorization::begin(&config).map_err(text)?;
        *running = Some(SignIn {
            redirect_uri: pending.redirect_uri().to_owned(),
            cancelled: false,
        });
        pending
    };
    let _ = handle.emit(MAIL_SIGN_IN, pending.url());
    open_browser(pending.url());
    let result = pending.finish(&config, SIGN_IN_TIMEOUT);
    let cancelled = shared
        .sign_in
        .lock()
        .map_err(text)?
        .take()
        .is_some_and(|s| s.cancelled);
    let tokens = match result {
        Ok(tokens) => tokens,
        Err(_) if cancelled => return Err("inicio de sesión cancelado".into()),
        Err(error) => return Err(text(error)),
    };
    // El cerrojo solo se retiene para anotar la cuenta y la lectura; el listado y las descargas
    // van después, paso a paso, sin él.
    let (view, source) = {
        let mut app = shared.app()?;
        let view = match account {
            Some(id) => app.reconnect_mail_account(id, &tokens),
            None => app.add_mail_account(provider, label, &tokens),
        }
        .map_err(text)?;
        // Versión final: al conectar una cuenta nueva empieza su primera lectura (E5, D133) solo
        // si lo mostrado antes de conectar sigue siendo cierto (`expect`); si no, o si no se
        // mostró nada, queda pendiente con un botón. Un fallo no impide conectar la cuenta.
        let source = if account.is_none() {
            match app.request_initial_read(&view.id, expect) {
                Ok(status) if status.state == "reading" => Some(status.source),
                Ok(_) => None,
                Err(error) => {
                    eprintln!("no se pudo empezar la primera lectura: {error}");
                    None
                }
            }
        } else {
            None
        };
        (view, source)
    };
    if let Some(source) = source {
        drive_read(shared, handle, &source);
    }
    Ok(view)
}

/// Lleva una primera lectura hasta el final paso a paso. El cerrojo de la aplicación se **retiene
/// durante cada paso** (una página del listado, un mensaje con las descargas de sus adjuntos o un
/// archivo) y se suelta **entre** un paso y el siguiente: la ventana y la cola no esperan a toda la
/// lectura, pero sí mientras dura un paso.
fn drive_read<R: Runtime>(shared: &Shared, handle: &AppHandle<R>, source: &str) {
    let mut steps = 0u32;
    loop {
        if shared.stopping.load(Ordering::SeqCst) {
            break;
        }
        let step = match shared.app() {
            Ok(mut app) => app.read_step(source),
            Err(_) => return,
        };
        match step {
            Ok(ReadStep::Continue) => {
                steps += 1;
                if steps.is_multiple_of(5) {
                    shared.wake();
                    changed(handle);
                }
            }
            Ok(_) => break,
            Err(error) => {
                eprintln!("no se pudo seguir con la primera lectura: {error}");
                if let Ok(mut app) = shared.app() {
                    app.note_initial_read_error(source, &error.to_string());
                }
                break;
            }
        }
    }
    shared.wake();
    changed(handle);
}

/// Cancela el inicio de sesión en curso: la espera local recibe una redirección que no
/// corresponde a la solicitud y termina sin canjear ningún código.
#[tauri::command]
fn mail_sign_in_cancel(shared: State<'_, SharedState>) -> CmdResult<()> {
    let redirect = {
        let mut running = shared.sign_in.lock().map_err(text)?;
        match running.as_mut() {
            Some(sign_in) => {
                sign_in.cancelled = true;
                sign_in.redirect_uri.clone()
            }
            None => return Ok(()),
        }
    };
    let address = redirect
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .to_owned();
    if let Ok(mut stream) = std::net::TcpStream::connect(address) {
        use std::io::Write;
        let _ = stream.write_all(b"GET /?error=cancelled HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n");
    }
    Ok(())
}

/// Abre la dirección en el navegador predeterminado sin pasar por un intérprete de órdenes.
fn open_browser(url: &str) {
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("rundll32")
            .args(["url.dll,FileProtocolHandler", url])
            .spawn();
    }
    #[cfg(not(windows))]
    let _ = url;
}

/// Desconecta una cuenta: borra sus tokens del almacén y la quita de la lista.
#[tauri::command]
fn remove_mail_account<R: Runtime>(
    shared: State<'_, SharedState>,
    handle: AppHandle<R>,
    id: String,
) -> CmdResult<()> {
    shared.app()?.remove_mail_account(&id).map_err(text)?;
    changed(&handle);
    Ok(())
}

/// Cambia el nombre visible de una cuenta (el identificador no cambia).
#[tauri::command]
fn rename_mail_account<R: Runtime>(
    shared: State<'_, SharedState>,
    handle: AppHandle<R>,
    id: String,
    label: String,
) -> CmdResult<AccountView> {
    let view = shared
        .app()?
        .rename_mail_account(&id, &label)
        .map_err(text)?;
    changed(&handle);
    Ok(view)
}

#[tauri::command]
fn add_simulated_mail_account<R: Runtime>(
    shared: State<'_, SharedState>,
    handle: AppHandle<R>,
) -> CmdResult<AccountView> {
    let view = shared.app()?.add_simulated_mail_account().map_err(text)?;
    changed(&handle);
    Ok(view)
}

#[tauri::command]
fn approve<R: Runtime>(
    shared: State<'_, SharedState>,
    handle: AppHandle<R>,
    id: i64,
    version: i64,
) -> CmdResult<Task> {
    let task = shared.app()?.approve(id, version).map_err(text);
    changed(&handle);
    task
}

/// El usuario confirma que el intento anterior no se envió; prepara una versión nueva (D50).
#[tauri::command]
fn confirm_not_sent<R: Runtime>(
    shared: State<'_, SharedState>,
    handle: AppHandle<R>,
    id: i64,
) -> CmdResult<Task> {
    let task = shared.app()?.confirm_not_sent(id).map_err(text);
    changed(&handle);
    task
}

#[tauri::command]
fn edit_draft<R: Runtime>(
    shared: State<'_, SharedState>,
    handle: AppHandle<R>,
    id: i64,
    body: String,
) -> CmdResult<Task> {
    let task = shared.app()?.edit_draft(id, &body).map_err(text)?;
    changed(&handle);
    Ok(task)
}

#[tauri::command]
fn reject<R: Runtime>(
    shared: State<'_, SharedState>,
    handle: AppHandle<R>,
    id: i64,
) -> CmdResult<Task> {
    let task = shared.app()?.reject(id).map_err(text)?;
    changed(&handle);
    Ok(task)
}

/// Archivar (D159): cierra un correo en revisión sin enviar nada ni contar como rechazo.
#[tauri::command]
fn archive<R: Runtime>(
    shared: State<'_, SharedState>,
    handle: AppHandle<R>,
    id: i64,
) -> CmdResult<Task> {
    let task = shared.app()?.archive(id).map_err(text)?;
    changed(&handle);
    Ok(task)
}

/// Preferencias por remitente (D161). Solo lectura.
#[tauri::command]
fn sender_preferences(shared: State<'_, SharedState>) -> CmdResult<Vec<SenderPreference>> {
    shared.app()?.sender_preferences().map_err(text)
}

/// Fija «Siempre a Informativos» o «Siempre a Pendientes» para una dirección o un dominio
/// (D161). El backend normaliza la clave, rechaza los dominios de correo público y deja el
/// cambio en el historial; avisa si contradice una regla activa.
#[tauri::command]
fn set_sender_preference<R: Runtime>(
    shared: State<'_, SharedState>,
    handle: AppHandle<R>,
    scope: String,
    key: String,
    route: String,
) -> CmdResult<PreferenceChange> {
    let change = shared
        .app()?
        .set_sender_preference(&scope, &key, &route)
        .map_err(text)?;
    changed(&handle);
    Ok(change)
}

/// Quita una preferencia por remitente (D161).
#[tauri::command]
fn remove_sender_preference<R: Runtime>(
    shared: State<'_, SharedState>,
    handle: AppHandle<R>,
    scope: String,
    key: String,
) -> CmdResult<bool> {
    let removed = shared
        .app()?
        .remove_sender_preference(&scope, &key)
        .map_err(text)?;
    changed(&handle);
    Ok(removed)
}

/// Redactar respuesta (D159): crea la tarea que solo redacta y la pone en marcha.
#[tauri::command]
fn redraft<R: Runtime>(
    shared: State<'_, SharedState>,
    handle: AppHandle<R>,
    id: i64,
) -> CmdResult<Task> {
    let task = shared.app()?.redraft(id).map_err(text)?;
    shared.wake();
    changed(&handle);
    Ok(task)
}

#[tauri::command]
fn retry<R: Runtime>(
    shared: State<'_, SharedState>,
    handle: AppHandle<R>,
    id: i64,
) -> CmdResult<Task> {
    let task = shared.app()?.retry(id).map_err(text)?;
    shared.wake();
    changed(&handle);
    Ok(task)
}

#[tauri::command]
fn rules(shared: State<'_, SharedState>) -> CmdResult<Vec<Rule>> {
    shared.app()?.rules().map_err(text)
}

/// Todas las reglas con nombre, archivado, validación avanzada y contadores.
#[tauri::command]
fn rule_views(shared: State<'_, SharedState>) -> CmdResult<Vec<RuleView>> {
    shared.app()?.rule_views().map_err(text)
}

/// `id` puede faltar (se genera del nombre). `acknowledge_any_sender` no tiene valor por
/// defecto: es la validación avanzada de una regla de envío sin remitente (D126).
#[tauri::command]
fn create_rule<R: Runtime>(
    shared: State<'_, SharedState>,
    handle: AppHandle<R>,
    id: Option<String>,
    name: Option<String>,
    action_kind: String,
    conditions: Value,
    acknowledge_any_sender: bool,
) -> CmdResult<RuleView> {
    let view = shared
        .app()?
        .create_rule_full(NewRule {
            id: id.as_deref(),
            name: name.as_deref(),
            action_kind: &action_kind,
            conditions: &conditions,
            allow_any_sender: acknowledge_any_sender,
        })
        .map_err(text)?;
    changed(&handle);
    Ok(view)
}

#[tauri::command]
fn rename_rule<R: Runtime>(
    shared: State<'_, SharedState>,
    handle: AppHandle<R>,
    id: String,
    name: Option<String>,
) -> CmdResult<RuleView> {
    let view = shared
        .app()?
        .rename_rule(&id, name.as_deref())
        .map_err(text)?;
    changed(&handle);
    Ok(view)
}

#[tauri::command]
fn set_rule_any_sender<R: Runtime>(
    shared: State<'_, SharedState>,
    handle: AppHandle<R>,
    id: String,
    acknowledge: bool,
) -> CmdResult<RuleView> {
    let view = shared
        .app()?
        .set_rule_any_sender(&id, acknowledge)
        .map_err(text)?;
    changed(&handle);
    Ok(view)
}

#[tauri::command]
fn archive_rule<R: Runtime>(
    shared: State<'_, SharedState>,
    handle: AppHandle<R>,
    id: String,
) -> CmdResult<RuleView> {
    let view = shared.app()?.archive_rule(&id).map_err(text)?;
    changed(&handle);
    Ok(view)
}

#[tauri::command]
fn unarchive_rule<R: Runtime>(
    shared: State<'_, SharedState>,
    handle: AppHandle<R>,
    id: String,
) -> CmdResult<RuleView> {
    let view = shared.app()?.unarchive_rule(&id).map_err(text)?;
    changed(&handle);
    Ok(view)
}

#[tauri::command]
fn rule_stats(shared: State<'_, SharedState>) -> CmdResult<Vec<RuleStats>> {
    shared.app()?.rule_stats().map_err(text)
}

/// Solo lectura: no escribe nada ni llama a la IA. `assume_any_sender_ack` es solo una
/// hipótesis (D126): nunca guarda la marca. `exclude_rule_id` quita esa regla de la
/// comparación (probar una regla guardada sin que coincida consigo misma).
#[tauri::command]
fn simulate_rule(
    shared: State<'_, SharedState>,
    action_kind: String,
    conditions: Value,
    limit: Option<usize>,
    assume_any_sender_ack: Option<bool>,
    exclude_rule_id: Option<String>,
) -> CmdResult<Simulation> {
    shared
        .app()?
        .simulate_rule(
            &action_kind,
            &conditions,
            limit,
            assume_any_sender_ack.unwrap_or(false),
            exclude_rule_id.as_deref(),
        )
        .map_err(text)
}

/// Contradicciones entre las reglas (D130). Solo lectura: no llama a la IA.
#[tauri::command]
fn rule_findings(shared: State<'_, SharedState>) -> CmdResult<Vec<RuleFinding>> {
    shared.app()?.rule_findings().map_err(text)
}

/// Sugerencias de reglas calculadas por código (D132). Solo lectura: no llama a la IA.
#[tauri::command]
fn rule_suggestions(shared: State<'_, SharedState>) -> CmdResult<Suggestions> {
    shared.app()?.rule_suggestions().map_err(text)
}

/// Estado del afinado de las sugerencias con IA (D134). Solo lectura: no llama al proveedor.
#[tauri::command]
fn suggestion_refinement(shared: State<'_, SharedState>) -> CmdResult<RefinementView> {
    shared.app()?.refinement_view().map_err(text)
}

/// A qué proveedor y qué datos se enviarían al afinar (D134). Solo lectura.
#[tauri::command]
fn refinement_preview(shared: State<'_, SharedState>) -> CmdResult<RefinePreview> {
    shared.app()?.refinement_preview().map_err(text)
}

/// Activa el afinado automático para el proveedor y modelo que la persona vio en el aviso
/// (D134). Queda en el historial; si el proveedor o el modelo cambiaron, se rechaza.
#[tauri::command]
fn grant_refine_consent<R: Runtime>(
    shared: State<'_, SharedState>,
    handle: AppHandle<R>,
    provider: String,
    model: String,
    host: String,
) -> CmdResult<ConsentView> {
    let consent = shared
        .app()?
        .grant_refine_consent(&provider, &model, &host)
        .map_err(text)?;
    changed(&handle);
    Ok(consent)
}

/// Retira el consentimiento del afinado automático (D134).
#[tauri::command]
fn withdraw_refine_consent<R: Runtime>(
    shared: State<'_, SharedState>,
    handle: AppHandle<R>,
) -> CmdResult<ConsentView> {
    let consent = shared.app()?.withdraw_refine_consent().map_err(text)?;
    changed(&handle);
    Ok(consent)
}

/// Afina las sugerencias con la IA cuando la persona lo pide, tras ver la vista previa (D134). La llamada al
/// modelo se hace sin el cerrojo de la aplicación y fuera del hilo de la ventana.
#[tauri::command]
async fn refine_suggestions<R: Runtime>(
    shared: State<'_, SharedState>,
    handle: AppHandle<R>,
    expect: RefineExpectation,
) -> CmdResult<RefinementView> {
    let shared = Arc::clone(shared.inner());
    tauri::async_runtime::spawn_blocking(move || {
        // `expect` es lo que la persona vio en la vista previa: si algo cambió, no se envía.
        let job = shared
            .app()?
            .begin_refinement_with(RefineTrigger::Demand, Some(&expect))
            .map_err(text)?;
        let Some(job) = job else {
            return shared.app()?.refinement_view().map_err(text);
        };
        // La ventana ve «afinando» mientras dura la llamada.
        changed(&handle);
        let completed = job.run();
        let view = shared.app()?.finish_refinement(completed).map_err(text);
        changed(&handle);
        view
    })
    .await
    .map_err(text)?
}

/// Afinado por lotes de la versión final (D134): si se acumularon señales válidas nuevas,
/// lanza una sola llamada en un hilo aparte, para no frenar la cola de tareas.
fn auto_refine<R: Runtime>(shared: &SharedState, handle: &AppHandle<R>) {
    let job = match shared.app() {
        Ok(mut app) if app.refine_mode() == RefineMode::Automatic => {
            app.begin_refinement(RefineTrigger::Auto)
        }
        _ => return,
    };
    let Ok(Some(job)) = job else { return };
    changed(handle);
    let (shared, handle) = (Arc::clone(shared), handle.clone());
    let spawned = thread::Builder::new()
        .name("nestadmin-afinado".into())
        .spawn(move || {
            let completed = job.run();
            if let Ok(mut app) = shared.app() {
                if let Err(error) = app.finish_refinement(completed) {
                    eprintln!("no se pudo guardar el afinado: {error}");
                }
            }
            changed(&handle);
        });
    if let Err(error) = spawned {
        eprintln!("no se pudo iniciar el afinado: {error}");
    }
}

// ---- Primera lectura (E5, D133) ----

/// Lo que la pantalla muestra antes y después de leer: modo, volumen, proveedor, qué se envía y el
/// estado de cada cuenta y carpeta. Solo lectura.
#[tauri::command]
fn initial_read_overview(shared: State<'_, SharedState>) -> CmdResult<InitialReadOverview> {
    shared.app()?.initial_read_overview().map_err(text)
}

/// Una carpeta antes de leerla (cuántos archivos admitidos tiene y si ya se leyó).
#[tauri::command]
fn initial_read_folder_preview(
    shared: State<'_, SharedState>,
    path: String,
) -> CmdResult<FolderPreview> {
    shared
        .app()?
        .initial_read_folder_preview(&PathBuf::from(path))
        .map_err(text)
}

/// Abre el selector de carpetas del sistema; `None` si se cancela.
#[tauri::command]
async fn pick_read_folder<R: Runtime>(handle: AppHandle<R>) -> CmdResult<Option<String>> {
    tauri::async_runtime::spawn_blocking(move || {
        let picked = handle
            .dialog()
            .file()
            .set_title("Carpeta de archivos para la primera lectura")
            .blocking_pick_folder();
        picked
            .map(|folder| folder.into_path().map(|p| p.display().to_string()))
            .transpose()
            .map_err(text)
    })
    .await
    .map_err(text)?
}

/// Pide la lectura bajo demanda de una cuenta (o, en la versión final, la que quedó pendiente),
/// con el volumen y el aviso que la persona vio. El cerrojo solo se toma para anotarla y, después,
/// en cada paso: el buzón puede tardar sin bloquear la ventana.
#[tauri::command]
async fn start_initial_read<R: Runtime>(
    shared: State<'_, SharedState>,
    handle: AppHandle<R>,
    account: String,
    volume: Option<usize>,
    expect: Option<ReadExpectation>,
) -> CmdResult<InitialReadStatus> {
    let shared = Arc::clone(shared.inner());
    let emitter = handle.clone();
    let worker = Arc::clone(&shared);
    let status = tauri::async_runtime::spawn_blocking(move || {
        let begun = worker
            .app()?
            .begin_initial_read(&account, volume, expect.as_ref())
            .map_err(text)?;
        drive_read(&worker, &emitter, &begun.source);
        worker
            .app()?
            .initial_read_status(&begun.source)
            .map_err(text)
    })
    .await
    .map_err(text)??;
    shared.wake();
    changed(&handle);
    Ok(status)
}

/// Lo mismo con una carpeta de archivos (solo en las versiones de prueba).
#[tauri::command]
async fn start_initial_read_folder<R: Runtime>(
    shared: State<'_, SharedState>,
    handle: AppHandle<R>,
    path: String,
    volume: Option<usize>,
    expect: Option<ReadExpectation>,
) -> CmdResult<InitialReadStatus> {
    let shared = Arc::clone(shared.inner());
    let emitter = handle.clone();
    let worker = Arc::clone(&shared);
    let status = tauri::async_runtime::spawn_blocking(move || {
        let begun = worker
            .app()?
            .begin_initial_read_folder(&PathBuf::from(path), volume, expect.as_ref())
            .map_err(text)?;
        drive_read(&worker, &emitter, &begun.source);
        worker
            .app()?
            .initial_read_status(&begun.source)
            .map_err(text)
    })
    .await
    .map_err(text)??;
    shared.wake();
    changed(&handle);
    Ok(status)
}

/// Detiene una lectura: no sale nada más y se puede continuar.
#[tauri::command]
fn stop_initial_read<R: Runtime>(
    shared: State<'_, SharedState>,
    handle: AppHandle<R>,
    source: String,
) -> CmdResult<InitialReadStatus> {
    let status = shared.app()?.stop_initial_read(&source).map_err(text)?;
    changed(&handle);
    Ok(status)
}

/// Continúa una lectura detenida o retenida por un cambio de destino, con el destino que la
/// persona vio ahora.
#[tauri::command]
async fn continue_initial_read<R: Runtime>(
    shared: State<'_, SharedState>,
    handle: AppHandle<R>,
    source: String,
    expect: ReadExpectation,
) -> CmdResult<InitialReadStatus> {
    let shared = Arc::clone(shared.inner());
    let emitter = handle.clone();
    let worker = Arc::clone(&shared);
    let status = tauri::async_runtime::spawn_blocking(move || {
        worker
            .app()?
            .continue_initial_read(&source, &expect)
            .map_err(text)?;
        drive_read(&worker, &emitter, &source);
        worker.app()?.initial_read_status(&source).map_err(text)
    })
    .await
    .map_err(text)??;
    shared.wake();
    changed(&handle);
    Ok(status)
}

/// Reintenta las tareas fallidas de una lectura. Devuelve cuántas volvieron a la cola.
#[tauri::command]
fn retry_initial_read<R: Runtime>(
    shared: State<'_, SharedState>,
    handle: AppHandle<R>,
    source: String,
) -> CmdResult<usize> {
    let retried = shared.app()?.retry_initial_read(&source).map_err(text)?;
    // Si quedaban adjuntos por descargar, la lectura vuelve a avanzar paso a paso.
    let state = Arc::clone(shared.inner());
    let emitter = handle.clone();
    thread::spawn(move || drive_read(&state, &emitter, &source));
    changed(&handle);
    Ok(retried)
}

/// Crea la regla de una sugerencia, siempre inactiva (D132). Repetirlo no duplica.
#[tauri::command]
fn accept_suggestion<R: Runtime>(
    shared: State<'_, SharedState>,
    handle: AppHandle<R>,
    fingerprint: String,
    shown: String,
    name: Option<String>,
    model_limit: Option<String>,
    user_limit: Option<String>,
) -> CmdResult<RuleView> {
    let mut app = shared.app()?;
    // Facturas solo vistas (D133): la regla solo se crea con el límite que escribe la persona.
    let view = match user_limit.as_deref() {
        Some(limit) => {
            app.accept_suggestion_with_user_limit(&fingerprint, &shown, name.as_deref(), limit)
        }
        None => app.accept_suggestion_with_limit(
            &fingerprint,
            &shown,
            name.as_deref(),
            model_limit.as_deref(),
        ),
    }
    .map_err(text)?;
    drop(app);
    changed(&handle);
    Ok(view)
}

/// Las facturas que solo se leyeron en la serie de una sugerencia (D133). Solo lectura.
#[tauri::command]
fn seen_invoices(
    shared: State<'_, SharedState>,
    fingerprint: String,
) -> CmdResult<Vec<SeenInvoice>> {
    shared.app()?.seen_invoices(&fingerprint).map_err(text)
}

/// Descarta una sugerencia; solo reaparece con el doble de evidencia nueva (D132).
#[tauri::command]
fn discard_suggestion<R: Runtime>(
    shared: State<'_, SharedState>,
    handle: AppHandle<R>,
    fingerprint: String,
) -> CmdResult<()> {
    shared
        .app()?
        .discard_suggestion(&fingerprint)
        .map_err(text)?;
    changed(&handle);
    Ok(())
}

/// Umbrales de evidencia para sugerir y sus límites.
#[tauri::command]
fn suggestion_thresholds(shared: State<'_, SharedState>) -> CmdResult<ThresholdSettings> {
    shared.app()?.suggestion_thresholds().map_err(text)
}

/// Cambia un umbral («invoice» o «reply»); el backend rechaza lo que no esté entre el mínimo
/// y el máximo y deja el cambio en el historial.
#[tauri::command]
fn set_suggestion_threshold<R: Runtime>(
    shared: State<'_, SharedState>,
    handle: AppHandle<R>,
    which: String,
    value: i64,
) -> CmdResult<ThresholdSettings> {
    let settings = shared
        .app()?
        .set_suggestion_threshold(&which, value)
        .map_err(text)?;
    changed(&handle);
    Ok(settings)
}

/// Datos para precargar una regla desde una revisión. Solo lectura.
#[tauri::command]
fn rule_seed_for_task(shared: State<'_, SharedState>, task_id: i64) -> CmdResult<RuleSeed> {
    shared.app()?.rule_seed_for_task(task_id).map_err(text)
}

/// Contradicciones que tendría una regla archivada al restaurarla (D128, D130). Solo lectura.
#[tauri::command]
fn preview_restore(shared: State<'_, SharedState>, id: String) -> CmdResult<Vec<RuleFinding>> {
    shared.app()?.preview_restore(&id).map_err(text)
}

/// Revisa una unificación (D131) sin cambiar nada: antes, opciones y, con la elección hecha,
/// el plan con su simulación.
#[tauri::command]
fn preview_merge(
    shared: State<'_, SharedState>,
    ids: Vec<String>,
    chosen: Option<String>,
) -> CmdResult<MergePreview> {
    shared
        .app()?
        .preview_merge(&ids, chosen.as_deref())
        .map_err(text)
}

/// Unifica reglas en una sola transacción (D131): la regla nueva queda inactiva y las
/// originales archivadas. `plan_fingerprint` es la del plan que el usuario revisó.
#[tauri::command]
fn merge_rules<R: Runtime>(
    shared: State<'_, SharedState>,
    handle: AppHandle<R>,
    ids: Vec<String>,
    chosen: Option<String>,
    name: Option<String>,
    plan_fingerprint: String,
) -> CmdResult<RuleView> {
    let view = shared
        .app()?
        .merge_rules(&ids, chosen.as_deref(), name.as_deref(), &plan_fingerprint)
        .map_err(text)?;
    changed(&handle);
    Ok(view)
}

/// Guarda de una vez condiciones, nombre y validación avanzada (D126): sin guardados parciales.
#[tauri::command]
fn update_rule_with_ack<R: Runtime>(
    shared: State<'_, SharedState>,
    handle: AppHandle<R>,
    id: String,
    name: Option<String>,
    conditions: Value,
    acknowledge_any_sender: bool,
) -> CmdResult<RuleView> {
    let view = shared
        .app()?
        .update_rule_with_ack(&id, name.as_deref(), &conditions, acknowledge_any_sender)
        .map_err(text)?;
    changed(&handle);
    Ok(view)
}

#[tauri::command]
fn update_rule<R: Runtime>(
    shared: State<'_, SharedState>,
    handle: AppHandle<R>,
    id: String,
    conditions: Value,
) -> CmdResult<Rule> {
    let rule = shared.app()?.update_rule(&id, &conditions).map_err(text)?;
    changed(&handle);
    Ok(rule)
}

#[tauri::command]
fn set_rule_active<R: Runtime>(
    shared: State<'_, SharedState>,
    handle: AppHandle<R>,
    id: String,
    active: bool,
) -> CmdResult<Rule> {
    let rule = shared.app()?.set_rule_active(&id, active).map_err(text)?;
    changed(&handle);
    Ok(rule)
}

#[tauri::command]
fn audit_log(shared: State<'_, SharedState>) -> CmdResult<Vec<AuditEntry>> {
    shared.app()?.audit_log().map_err(text)
}

#[derive(Serialize)]
struct SettingsView {
    /// `None`: el usuario debe elegir proveedor (D100).
    choice: Option<ProviderChoice>,
    active: ProviderInfo,
    /// Por qué las tareas con IA esperan en la cola, si esperan (D101).
    ai_wait: Option<AiWait>,
    ai_wait_message: Option<String>,
    /// Con una espera de D146 (404, 402): causa enumerada (`model_not_found`, `route_not_found`,
    /// `other`) y estado HTTP, para que la interfaz ofrezca la salida que corresponde.
    ai_wait_cause: Option<&'static str>,
    ai_wait_status: Option<u16>,
    /// Respaldo automático del externo elegido a la IA local (D101).
    local_fallback: bool,
    /// Las tareas van directas a la IA local por un `unavailable` reciente del externo (D101).
    fallback_period_active: bool,
    /// Destino de la elección actual ya confirmado (siempre `true` con la IA local).
    consented: bool,
    /// Oferta única de pasar al proveedor externo a quien tenía la IA local (D100).
    offer_external: bool,
    /// Modelo prefijado para Ollama cloud (D108): la interfaz lo toma de aquí.
    default_cloud_model: &'static str,
    /// Modelo y dirección de la IA local, para el botón explícito «Usar la IA local» (D146).
    default_local_model: &'static str,
    default_local_url: &'static str,
    data_dir: PathBuf,
}

#[tauri::command]
fn settings(shared: State<'_, SharedState>) -> CmdResult<SettingsView> {
    let (choice, local_fallback, consented, offer_external) = {
        let settings = shared.settings.lock().map_err(text)?;
        let consented = settings
            .provider
            .as_ref()
            .is_some_and(|c| c.is_local() || settings.has_consent(c));
        let offer_external = !settings.external_offer_shown
            && settings
                .provider
                .as_ref()
                .is_some_and(ProviderChoice::is_local);
        (
            settings.provider.clone(),
            settings.local_fallback,
            consented,
            offer_external,
        )
    };
    // Con el compatible con OpenAI, el texto de la espera cita la dirección configurada.
    let address = match &choice {
        Some(ProviderChoice::OpenAiCompatible { base_url, .. }) => Some(base_url.clone()),
        _ => None,
    };
    let (active, ai_wait, ai_wait_message, fallback_period_active) = {
        let app = shared.app()?;
        (
            app.provider(),
            app.ai_wait(),
            app.ai_wait_message(address.as_deref()),
            app.fallback_period_active(),
        )
    };
    Ok(SettingsView {
        choice,
        active,
        ai_wait,
        ai_wait_message,
        ai_wait_cause: ai_wait.and_then(AiWait::cause),
        ai_wait_status: ai_wait.and_then(AiWait::status),
        local_fallback,
        fallback_period_active,
        consented,
        offer_external,
        default_cloud_model: nestadmin_ai::ollama::DEFAULT_CLOUD_MODEL,
        default_local_model: nestadmin_ai::ollama::DEFAULT_MODEL,
        default_local_url: nestadmin_ai::ollama::DEFAULT_URL,
        data_dir: shared.data_dir.clone(),
    })
}

/// Vuelve a calcular el proveedor activo a partir de la configuración y las credenciales
/// (D100, D101): si falta elegir, la clave o confirmar el destino, la aplicación queda con un
/// proveedor que no puede enviar y las tareas con IA esperan en la cola; si no, se construye
/// el elegido y se retoman. Se llama al arrancar y tras cada cambio de proveedor o de clave.
fn refresh_provider(shared: &Shared) -> CmdResult<()> {
    let (provider, wait, fallback) = {
        let settings = shared.settings.lock().map_err(text)?;
        let (provider, wait) = settings::resolve(&settings, shared.credentials.as_ref());
        // Respaldo solo del externo elegido (y utilizable) a la IA local, y solo si está
        // activado (D101). Con la IA local elegida o en espera no hay ninguno.
        let external = settings.provider.as_ref().is_some_and(|c| !c.is_local());
        let fallback =
            (external && wait.is_none() && settings.local_fallback).then(|| local_fallback(shared));
        (provider, wait, fallback)
    };
    {
        let mut app = shared.app()?;
        app.select_provider(provider);
        app.set_ai_wait(wait);
        app.set_local_fallback(fallback);
    }
    shared.wake();
    Ok(())
}

/// Respaldo local (D101): si Ollama está preparado (arrancándolo si estaba detenido) devuelve el
/// proveedor local con su selección inicial; si falta algo, `None`, y no se descarga nada.
fn local_fallback(shared: &Shared) -> LocalFallback {
    let setup = Arc::clone(&shared.local_setup);
    let credentials = Arc::clone(&shared.credentials);
    Arc::new(move || match setup.startup(true) {
        Startup::Ready | Startup::Started => {
            settings::build(&local_choice(), credentials.as_ref()).ok()
        }
        _ => None,
    })
}

fn local_choice() -> ProviderChoice {
    ProviderChoice::Ollama {
        base_url: nestadmin_ai::ollama::DEFAULT_URL.to_owned(),
        model: nestadmin_ai::ollama::DEFAULT_MODEL.to_owned(),
    }
}

/// Activa o desactiva el respaldo automático del proveedor externo a la IA local (D101).
#[tauri::command]
fn set_local_fallback(shared: State<'_, SharedState>, enabled: bool) -> CmdResult<()> {
    {
        let mut current = shared.settings.lock().map_err(text)?;
        let updated = Settings {
            local_fallback: enabled,
            ..current.clone()
        };
        updated.save(&shared.data_dir).map_err(text)?;
        *current = updated;
    }
    refresh_provider(&shared)
}

/// Cambio explícito de proveedor. Con uno externo hace falta la confirmación del destino
/// (`consent`, que se registra) o una anterior para ese mismo destino (D100). Si el elegido
/// no se puede preparar (por ejemplo, sin clave), se informa y se mantiene la selección
/// anterior.
#[tauri::command]
fn select_provider(
    shared: State<'_, SharedState>,
    choice: ProviderChoice,
    consent: Option<bool>,
) -> CmdResult<ProviderInfo> {
    let consent = consent.unwrap_or(false);
    {
        let mut current = shared.settings.lock().map_err(text)?;
        let mut updated = current.clone();
        if !choice.is_local() {
            if choice.destination().is_none() {
                // URL no válida: el error explícito del adaptador, sin leer ninguna clave.
                settings::build(&choice, shared.credentials.as_ref()).map_err(text)?;
            }
            if consent {
                updated.record_consent(&choice);
            } else if !updated.has_consent(&choice) {
                return Err("Confirme antes el destino de los datos".into());
            }
        }
        settings::build(&choice, shared.credentials.as_ref()).map_err(text)?;
        updated.provider = Some(choice);
        updated.save(&shared.data_dir).map_err(text)?;
        *current = updated;
    }
    refresh_provider(&shared)?;
    Ok(shared.app()?.provider())
}

/// Elimina la clave del proveedor `choice` (la del host en el compatible con OpenAI) y vuelve
/// a calcular el proveedor activo. Las claves nuevas solo se guardan al listar los modelos.
#[tauri::command]
fn delete_provider_key(shared: State<'_, SharedState>, choice: ProviderChoice) -> CmdResult<()> {
    if let Some(name) = choice.credential_name() {
        shared.credentials.delete(&name).map_err(text)?;
    }
    lift_wait_if_same_credential(&shared, &choice);
    refresh_provider(&shared)
}

/// Si hay clave guardada para `choice`; una URL compatible no válida no tiene ninguna.
#[tauri::command]
fn provider_key_saved(shared: State<'_, SharedState>, choice: ProviderChoice) -> CmdResult<bool> {
    let Some(name) = choice.credential_name() else {
        return Ok(false);
    };
    Ok(shared.credentials.get(&name).map_err(text)?.is_some())
}

/// Marca la oferta única de D100 como mostrada.
#[tauri::command]
fn dismiss_external_offer(shared: State<'_, SharedState>) -> CmdResult<()> {
    let mut current = shared.settings.lock().map_err(text)?;
    let updated = Settings {
        external_offer_shown: true,
        ..current.clone()
    };
    updated.save(&shared.data_dir).map_err(text)?;
    *current = updated;
    Ok(())
}

/// Plazo de la lista de modelos y de la prueba sintética.
const PROVIDER_CALL_TIMEOUT: Duration = Duration::from_secs(60);

/// Correo fijo y sintético de la prueba del proveedor (D122): no contiene datos del usuario.
const SYNTHETIC_TEST_MAIL: &str = "Asunto: Solicitud de copia de factura
De: cliente@ejemplo.invalid

Buenos días. Les agradecería que me enviaran una copia de la factura F-2026-001 del mes pasado. Gracias.";

/// Modelos que ofrece el proveedor (D122), tal cual los devuelve, con el recomendado primero.
/// Con `key` vacía o ausente se usa la guardada del proveedor (la del host en el compatible);
/// una `key` nueva se guarda solo si la lista responde, y entonces se vuelve a calcular el
/// proveedor activo. La clave no se devuelve, ni se registra ni aparece en ningún error. No
/// se envía contenido del usuario.
#[tauri::command]
async fn list_models(
    shared: State<'_, SharedState>,
    choice: ProviderChoice,
    key: Option<String>,
) -> CmdResult<Vec<String>> {
    let shared = Arc::clone(shared.inner());
    tauri::async_runtime::spawn_blocking(move || list_models_for(&shared, &choice, key))
        .await
        .map_err(text)?
}

fn list_models_for(
    shared: &SharedState,
    choice: &ProviderChoice,
    key: Option<String>,
) -> CmdResult<Vec<String>> {
    // Una URL compatible no válida falla aquí, antes de leer ninguna clave.
    if let ProviderChoice::OpenAiCompatible { base_url, .. } = choice {
        nestadmin_ai::openai_compatible::parse_base_url(base_url).map_err(text)?;
    }
    let name = choice.credential_name();
    let provided = key
        .map(|k| k.trim().to_owned())
        .filter(|k| !k.is_empty())
        .map(Secret::new);
    let saved;
    let used = match (&provided, &name) {
        (Some(key), _) => Some(key),
        (None, Some(name)) => {
            saved = shared.credentials.get(name).map_err(text)?;
            saved.as_ref()
        }
        (None, None) => None,
    };
    let models =
        nestadmin_ai::models::list_models(choice, used, PROVIDER_CALL_TIMEOUT).map_err(text)?;
    if let (Some(key), Some(name)) = (&provided, &name) {
        shared.credentials.set(name, key).map_err(text)?;
        lift_wait_if_same_credential(shared, choice);
        refresh_provider(shared)?;
    }
    Ok(models)
}

/// Prueba sintética del proveedor `choice` con su clave guardada (D122): genera con un texto
/// fijo y devuelve `Ok` o el texto del error. Con un proveedor externo exige el destino
/// confirmado antes de leer la clave o construir ningún cliente. No usa el bloqueo de la
/// aplicación: una respuesta lenta no detiene el resto.
#[tauri::command]
async fn test_provider<R: Runtime>(
    shared: State<'_, SharedState>,
    handle: AppHandle<R>,
    choice: ProviderChoice,
) -> CmdResult<()> {
    let shared = Arc::clone(shared.inner());
    tauri::async_runtime::spawn_blocking(move || {
        let outcome = probe_provider(&shared, &choice);
        if settle_probe(&shared, &choice, &outcome) {
            changed(&handle);
        }
        outcome.map_err(|e| e.to_string())
    })
    .await
    .map_err(text)?
}

/// Cambiar o borrar la clave del proveedor vigente puede significar otra cuenta: una espera de
/// D146 ya no vale. Solo si la credencial es la de la elección vigente; la clave de otro proveedor
/// no toca la espera.
fn lift_wait_if_same_credential(shared: &SharedState, choice: &ProviderChoice) -> bool {
    let same = choice.credential_name().is_some_and(|name| {
        shared
            .settings
            .lock()
            .map(|s| {
                s.provider
                    .as_ref()
                    .and_then(ProviderChoice::credential_name)
                    == Some(name)
            })
            .unwrap_or(false)
    });
    if !same {
        return false;
    }
    let lifted = shared
        .app()
        .map(|mut app| app.lift_provider_wait())
        .unwrap_or(false);
    if lifted {
        shared.wake();
    }
    lifted
}

/// Por qué una prueba sintética no dio un resultado del servicio.
#[derive(Debug)]
enum ProbeError {
    /// El destino externo no está confirmado: no se leyó ninguna clave ni se construyó ningún cliente.
    NoConsent,
    /// No se pudo construir el proveedor (por ejemplo, sin clave): tampoco salió nada.
    Unprepared(nestadmin_ai::AiError),
    /// El servicio respondió (o falló) al generar o al leer la respuesta.
    Ai(nestadmin_ai::AiError),
}

impl std::fmt::Display for ProbeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoConsent => f.write_str("Confirme antes el destino de los datos"),
            Self::Unprepared(e) | Self::Ai(e) => write!(f, "{e}"),
        }
    }
}

/// Qué hace con la espera de D146 el resultado de una prueba de la elección `choice`, solo si es
/// la vigente: un éxito, o una respuesta que no cumple el formato (`invalid_response`, que ya
/// falla por su cuenta en cada tarea, D105), demuestran que el servicio responde y levantan la
/// espera; un 401 la convierte en clave inválida; cualquier otro error la deja como está.
/// Devuelve si algo cambió.
fn settle_probe(
    shared: &SharedState,
    choice: &ProviderChoice,
    outcome: &Result<(), ProbeError>,
) -> bool {
    use nestadmin_ai::AiError;
    match outcome {
        Ok(()) | Err(ProbeError::Ai(AiError::InvalidResponse(_))) => {
            lift_wait_if_current(shared, choice)
        }
        Err(ProbeError::Ai(AiError::Unauthorized)) => {
            let current = shared
                .settings
                .lock()
                .map(|s| s.provider.as_ref() == Some(choice))
                .unwrap_or(false);
            let Ok(mut app) = shared.app() else {
                return false;
            };
            if current && app.ai_wait().is_some_and(AiWait::is_provider_response) {
                app.set_ai_wait(Some(AiWait::InvalidKey));
                return true;
            }
            false
        }
        Err(_) => false,
    }
}

/// Prueba y resuelve la espera: el sondeo. Devuelve si algo cambió.
fn run_probe(shared: &SharedState, choice: &ProviderChoice) -> bool {
    let outcome = probe_provider(shared, choice);
    settle_probe(shared, choice, &outcome)
}

/// Una prueba sintética con éxito demuestra que el proveedor responde con esa elección: si es la
/// vigente, levanta la espera de D146 (404, 402) y despierta la cola (D146).
fn lift_wait_if_current(shared: &SharedState, choice: &ProviderChoice) -> bool {
    let current = shared
        .settings
        .lock()
        .map(|s| s.provider.as_ref() == Some(choice))
        .unwrap_or(false);
    if !current {
        return false;
    }
    let lifted = shared
        .app()
        .map(|mut app| app.lift_provider_wait())
        .unwrap_or(false);
    if lifted {
        shared.wake();
    }
    lifted
}

fn probe_provider(shared: &SharedState, choice: &ProviderChoice) -> Result<(), ProbeError> {
    let consented = shared
        .settings
        .lock()
        .map(|s| choice.is_local() || s.has_consent(choice))
        .unwrap_or(false);
    if !consented {
        return Err(ProbeError::NoConsent);
    }
    let provider =
        nestadmin_ai::build_provider(choice, shared.credentials.as_ref(), PROVIDER_CALL_TIMEOUT)
            .map_err(ProbeError::Unprepared)?;
    // La misma solicitud que el correo real: un modelo que no devuelve el JSON estricto falla
    // aquí y no más tarde en una tarea (D105).
    let result = provider
        .generate(&nestadmin_ai::tasks::email_request(
            "prueba-sintetica",
            SYNTHETIC_TEST_MAIL,
        ))
        .map_err(ProbeError::Ai)?;
    nestadmin_ai::tasks::parse_email(&result.output)
        .map(|_| ())
        .map_err(ProbeError::Ai)
}

#[derive(Serialize)]
struct LaunchView {
    prepare_only: bool,
    preparing: bool,
    startup: Option<Startup>,
}

/// Modo de arranque y estado de la preparación, para que la ventana sepa qué mostrar.
#[tauri::command]
fn launch_mode(preparation: State<'_, Arc<Preparation>>) -> CmdResult<LaunchView> {
    Ok(LaunchView {
        prepare_only: preparation.prepare_only,
        preparing: preparation.running.lock().map_err(text)?.is_some(),
        startup: preparation.startup.lock().map_err(text)?.clone(),
    })
}

/// Comprobación rápida (R08), sin descargas, con la memoria del equipo (D46). Puede tardar
/// unos segundos si Ollama no responde, así que se ejecuta fuera del hilo de la ventana.
#[tauri::command]
async fn setup_check(preparation: State<'_, Arc<Preparation>>) -> CmdResult<Status> {
    let setup = Arc::clone(&preparation.setup);
    tauri::async_runtime::spawn_blocking(move || setup.status())
        .await
        .map_err(text)
}

#[derive(Clone, Serialize)]
struct SetupDone {
    report: Option<Report>,
    error: Option<SetupError>,
}

/// Inicia la preparación en segundo plano (D37). El progreso llega como `nestadmin://setup`
/// y el final como `nestadmin://setup-done`; un fallo deja la preparación lista para
/// reintentar y los pasos ya hechos se saltan.
#[tauri::command]
fn setup_prepare<R: Runtime>(
    preparation: State<'_, Arc<Preparation>>,
    handle: AppHandle<R>,
) -> CmdResult<()> {
    start_preparation(&preparation, handle)
}

/// Cancela la preparación en curso; termina con `cancelled` en el paso activo.
#[tauri::command]
fn setup_cancel(preparation: State<'_, Arc<Preparation>>) -> CmdResult<()> {
    if let Some(cancel) = preparation.running.lock().map_err(text)?.as_ref() {
        cancel.cancel();
    }
    Ok(())
}

/// Cierra el modo `--preparar` sin éxito: el instalador lo indica y la aplicación ofrecerá
/// el asistente al abrirse.
#[tauri::command]
fn setup_abandon<R: Runtime>(preparation: State<'_, Arc<Preparation>>, handle: AppHandle<R>) {
    if let Ok(running) = preparation.running.lock() {
        if let Some(cancel) = running.as_ref() {
            cancel.cancel();
        }
    }
    if preparation.prepare_only {
        handle.exit(1);
    }
}

fn start_preparation<R: Runtime>(
    preparation: &Arc<Preparation>,
    handle: AppHandle<R>,
) -> CmdResult<()> {
    let cancel = {
        let mut running = preparation.running.lock().map_err(text)?;
        if running.is_some() {
            return Err("la preparación ya está en curso".into());
        }
        let cancel = Cancel::default();
        *running = Some(cancel.clone());
        cancel
    };
    let preparation_ref = Arc::clone(preparation);
    let spawned = thread::Builder::new()
        .name("nestadmin-preparacion".into())
        .spawn(move || {
            let preparation = preparation_ref;
            let result = preparation.setup.prepare(
                &mut |event| {
                    let _ = handle.emit(SETUP, event);
                },
                &cancel,
            );
            if let Ok(mut running) = preparation.running.lock() {
                *running = None;
            }
            let ok = result.is_ok();
            let (report, error) = match result {
                Ok(report) => (Some(report), None),
                Err(error) => (None, Some(error)),
            };
            if ok {
                if let Ok(mut startup) = preparation.startup.lock() {
                    *startup = Some(Startup::Ready);
                }
            }
            let _ = handle.emit(SETUP_DONE, SetupDone { report, error });
            // Modo instalador: tras mostrar el éxito un momento, termina con código 0.
            if ok && preparation.prepare_only {
                thread::sleep(Duration::from_secs(3));
                handle.exit(0);
            }
        });
    if let Err(error) = spawned {
        *preparation.running.lock().map_err(text)? = None;
        return Err(text(error));
    }
    Ok(())
}

/// Comprobación al abrir (D39), en un hilo aparte: arranca un Ollama detenido y, si falta
/// algo, avisa a la ventana para que muestre el asistente. Nunca descarga ni cambia de
/// proveedor.
fn startup_check<R: Runtime>(
    preparation: Arc<Preparation>,
    handle: AppHandle<R>,
    local_provider: bool,
) {
    let spawned = thread::Builder::new()
        .name("nestadmin-comprobacion".into())
        .spawn(move || {
            let outcome = preparation.setup.startup(local_provider);
            if let Ok(mut startup) = preparation.startup.lock() {
                *startup = Some(outcome.clone());
            }
            let _ = handle.emit(STARTUP, outcome);
        });
    if let Err(error) = spawned {
        eprintln!("no se pudo iniciar la comprobación al abrir: {error}");
    }
}

/// Versión final: retoma, un paso cada vez, las primeras lecturas que esperaban al proveedor o que
/// se cortaron (E5, D133). Devuelve si hizo trabajo. Sin proveedor confirmado, con la cola en
/// pausa o con lo pendiente retenido no hace nada ni envía nada; un error de una lectura queda en
/// su marca y no se repite cada pocos segundos.
fn resume_initial_reads<R: Runtime>(shared: &SharedState, handle: &AppHandle<R>) -> bool {
    let worked = match shared.app() {
        Ok(mut app) if app.refine_mode() == RefineMode::Automatic => app.read_tick(),
        _ => return false,
    };
    match worked {
        Ok(true) => {
            changed(handle);
            true
        }
        Ok(false) => false,
        Err(error) => {
            eprintln!("no se pudo retomar la primera lectura: {error}");
            false
        }
    }
}

/// Procesa la cola sin bloquear la ventana: la llamada al modelo se hace sin el cerrojo.
fn worker<R: Runtime>(shared: SharedState, handle: AppHandle<R>, wake: Receiver<()>) {
    let mut probe = probe::WaitProbe::default();
    let probing = Arc::new(AtomicBool::new(false));
    while !shared.stopping.load(Ordering::SeqCst) {
        let job = match shared.app() {
            Ok(mut app) => app.begin_next(),
            Err(_) => return,
        };
        match job {
            Ok(Some(job)) => {
                changed(&handle);
                let completed = job.run();
                if let Ok(mut app) = shared.app() {
                    if let Err(error) = app.finish(completed) {
                        eprintln!("no se pudo guardar el resultado: {error}");
                    }
                }
                changed(&handle);
            }
            Ok(None) => {
                if resume_initial_reads(&shared, &handle) {
                    continue;
                }
                auto_refine(&shared, &handle);
                probe_provider_wait(&shared, &handle, &mut probe, &probing);
                let _ = wake.recv_timeout(Duration::from_secs(5));
            }
            Err(error) => {
                eprintln!("cola no disponible: {error}");
                let _ = wake.recv_timeout(Duration::from_secs(5));
            }
        }
    }
}

/// Sondeo sintético de la espera de D146 (404, 402): con la cola ociosa, cada
/// [`probe::PROBE_EVERY`] y solo mientras dure la espera, repite la prueba sintética de D122 con el
/// proveedor elegido. Sin clave o sin el destino confirmado `probe_provider` falla antes de
/// construir ningún cliente, así que no sale nada. No envía contenido del usuario: es el mismo
/// correo fijo y sintético de la prueba. Si responde, levanta la espera y la cola sigue.
/// La elección que toca sondear ahora, si toca: hay una espera de D146, pasaron los 15 minutos y
/// hay un proveedor elegido.
fn probe_target(
    shared: &SharedState,
    probe: &mut probe::WaitProbe,
    now: std::time::Instant,
) -> Option<ProviderChoice> {
    let waiting = shared
        .app()
        .map(|app| app.ai_wait().is_some_and(AiWait::is_provider_response))
        .unwrap_or(false);
    if !probe.due(waiting, now) {
        return None;
    }
    shared.settings.lock().ok().and_then(|s| s.provider.clone())
}

fn probe_provider_wait<R: Runtime>(
    shared: &SharedState,
    handle: &AppHandle<R>,
    probe: &mut probe::WaitProbe,
    probing: &Arc<AtomicBool>,
) {
    let Some(choice) = probe_target(shared, probe, std::time::Instant::now()) else {
        return;
    };
    if probing.swap(true, Ordering::SeqCst) {
        return;
    }
    let (shared, handle, probing_flag) = (Arc::clone(shared), handle.clone(), Arc::clone(probing));
    let spawned = thread::Builder::new()
        .name("nestadmin-sondeo".into())
        .spawn(move || {
            if run_probe(&shared, &choice) {
                changed(&handle);
            }
            probing_flag.store(false, Ordering::SeqCst);
        });
    if let Err(error) = spawned {
        probing.store(false, Ordering::SeqCst);
        eprintln!("no se pudo iniciar el sondeo: {error}");
    }
}

fn show_main<R: Runtime>(handle: &AppHandle<R>) {
    if let Some(window) = handle.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

/// Salida completa: no se toman tareas nuevas; una tarea en curso se recupera al reabrir.
fn quit<R: Runtime>(handle: &AppHandle<R>) {
    stop_worker(handle);
    handle.exit(0);
}

/// Deja de tomar tareas nuevas y despierta la cola para que su hilo termine.
fn stop_worker<R: Runtime>(handle: &AppHandle<R>) {
    if let Some(shared) = handle.try_state::<SharedState>() {
        shared.stopping.store(true, Ordering::SeqCst);
        shared.wake();
    }
}

fn data_dir<R: Runtime>(handle: &AppHandle<R>) -> PathBuf {
    std::env::var_os("NESTADMIN_DATA_DIR")
        .map(PathBuf::from)
        .or_else(nestadmin_app::default_data_dir)
        .or_else(|| handle.path().app_local_data_dir().ok())
        .unwrap_or_else(|| PathBuf::from("NestAdmin"))
}

/// Ubica un componente local: la variable de entorno, junto al ejecutable (instalación) o,
/// en compilaciones de desarrollo, `vendor/` del repositorio (`scripts/setup-ocr.ps1`).
fn component_dir(variable: &str, installed: &str, vendor: &str) -> PathBuf {
    if let Some(dir) = std::env::var_os(variable) {
        return PathBuf::from(dir);
    }
    let beside_exe = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(installed)));
    match beside_exe {
        Some(dir) if dir.exists() || !cfg!(debug_assertions) => dir,
        _ => PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../vendor")
            .join(vendor),
    }
}

/// Tesseract viaja en el paquete (D38); en desarrollo sin `vendor/tesseract`, se usa el
/// instalado en Program Files.
fn tesseract_exe() -> PathBuf {
    if let Some(exe) = std::env::var_os("NESTADMIN_TESSERACT") {
        return PathBuf::from(exe);
    }
    let packaged =
        component_dir("NESTADMIN_TESSERACT_DIR", "tesseract", "tesseract").join("tesseract.exe");
    if packaged.is_file() || !cfg!(debug_assertions) {
        packaged
    } else {
        Tesseract::default_executable()
    }
}

fn pdfium_dir() -> PathBuf {
    component_dir("NESTADMIN_PDFIUM_DIR", "pdfium", "pdfium/bin")
}

fn tessdata_dir() -> PathBuf {
    component_dir("NESTADMIN_TESSDATA", "tessdata", "tessdata")
}

/// Extracción local: PDFium para PDF y Tesseract para páginas escaneadas. Si falta algún
/// componente, las tareas afectadas fallan con el motivo; no se envía nada fuera.
fn extractor() -> Extractor {
    Extractor::new(
        pdfium_dir(),
        Box::new(Tesseract::new(tesseract_exe(), tessdata_dir())),
    )
}

/// Preparación de producción (D37, D38): componentes del paquete y el sistema real.
fn windows_setup(data_dir: &std::path::Path) -> Setup {
    let config = nestadmin_setup::Config::windows(
        tesseract_exe(),
        tessdata_dir(),
        pdfium_dir(),
        data_dir.join("descargas"),
    );
    Setup::new(config, Box::new(WindowsSystem))
}

fn preparation(setup: Setup, prepare_only: bool) -> Arc<Preparation> {
    Arc::new(Preparation {
        setup: Arc::new(setup),
        prepare_only,
        running: Mutex::new(None),
        startup: Mutex::new(None),
    })
}

/// Modo `--preparar` (D36): solo el asistente. Sin base de datos, cola ni bandeja; cerrar la
/// ventana sin terminar devuelve 1 al instalador.
fn setup_prepare_only<R: Runtime>(handle: &AppHandle<R>) -> Result<(), Box<dyn std::error::Error>> {
    let data_dir = data_dir(handle);
    std::fs::create_dir_all(&data_dir)?;
    handle.manage(preparation(windows_setup(&data_dir), true));
    if let Some(window) = handle.get_webview_window("main") {
        let _ = window.set_title("Preparación de NestAdmin");
        let _ = window.set_size(tauri::LogicalSize::new(720.0, 600.0));
        let _ = window.center();
    }
    Ok(())
}

/// Lo que cambia entre la aplicación instalada y las pruebas de recorrido (D53): carpeta de
/// datos, almacén de credenciales, preparación, extracción y bandeja. Todo lo demás —comandos,
/// configuración, cola y proveedor— es el mismo código en los dos casos.
struct Deps {
    data_dir: PathBuf,
    credentials: Arc<dyn CredentialStore>,
    setup: Setup,
    extractor: Extractor,
    /// Icono del área de notificación; las pruebas no lo crean para no tocar la bandeja.
    tray: bool,
}

impl Deps {
    fn installed<R: Runtime>(handle: &AppHandle<R>) -> Self {
        let data_dir = data_dir(handle);
        Self {
            credentials: credential_store(),
            setup: windows_setup(&data_dir),
            extractor: extractor(),
            tray: true,
            data_dir,
        }
    }
}

/// Abre los datos, arranca la cola y, si se pide, el icono del área de notificación.
/// Devuelve el hilo de la cola, que termina tras `stop_worker`.
fn start<R: Runtime>(
    handle: &AppHandle<R>,
    deps: Deps,
) -> Result<thread::JoinHandle<()>, Box<dyn std::error::Error>> {
    let Deps {
        data_dir,
        credentials,
        setup,
        extractor,
        tray,
    } = deps;
    std::fs::create_dir_all(&data_dir)?;
    let settings = Settings::load(&data_dir);
    let local_provider = settings
        .provider
        .as_ref()
        .is_some_and(ProviderChoice::is_local);
    let (provider, ai_wait) = settings::resolve(&settings, credentials.as_ref());
    let (profiles, mail_clients_error) = mail_clients::load(&data_dir);
    let mail = MailEnv {
        credentials: Arc::clone(&credentials),
        profiles,
    };
    let mut app = App::open_with_mail(
        &AppPaths::new(&data_dir),
        provider,
        Arc::new(extractor),
        mail,
    )?;
    app.set_ai_wait(ai_wait);
    if let Err(error) = app.set_mail_limit(settings.mail_limit) {
        eprintln!("límite de correo no válido en la configuración: {error}");
    }
    let (sender, receiver) = mpsc::channel();
    let preparation = preparation(setup, false);
    handle.manage(Arc::clone(&preparation));
    let local_setup = Arc::clone(&preparation.setup);
    startup_check(preparation, handle.clone(), local_provider);
    let shared = Arc::new(Shared {
        app: Mutex::new(app),
        settings: Mutex::new(settings),
        credentials,
        data_dir,
        local_setup,
        sign_in: Mutex::new(None),
        mail_clients_error,
        wake: Mutex::new(sender),
        stopping: AtomicBool::new(false),
    });
    handle.manage(Arc::clone(&shared));
    if let Err(error) = refresh_provider(&shared) {
        eprintln!("no se pudo preparar el proveedor de IA: {error}");
    }
    let worker_handle = handle.clone();
    let worker = thread::Builder::new()
        .name("nestadmin-cola".into())
        .spawn(move || worker(shared, worker_handle, receiver))?;
    if tray {
        build_tray(handle)?;
    }
    Ok(worker)
}

fn build_tray<R: Runtime>(handle: &AppHandle<R>) -> tauri::Result<()> {
    let show = MenuItem::with_id(handle, "show", "Mostrar NestAdmin", true, None::<&str>)?;
    let exit = MenuItem::with_id(
        handle,
        "quit",
        "Salir de NestAdmin por completo",
        true,
        None::<&str>,
    )?;
    let menu = Menu::with_items(handle, &[&show, &exit])?;
    let mut tray = TrayIconBuilder::with_id("nestadmin")
        .tooltip("NestAdmin")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|handle, event| match event.id().as_ref() {
            "show" => show_main(handle),
            "quit" => quit(handle),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main(tray.app_handle());
            }
        });
    if let Some(icon) = handle.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(handle)?;
    Ok(())
}

/// Cerrar la ventana principal la oculta y el trabajo sigue; en el modo `--preparar`,
/// cerrar el asistente equivale a cancelarlo.
fn on_window_event<R: Runtime>(window: &Window<R>, event: &WindowEvent, prepare_only: bool) {
    if let WindowEvent::CloseRequested { api, .. } = event {
        if prepare_only {
            if let Some(preparation) = window.try_state::<Arc<Preparation>>() {
                if let Ok(running) = preparation.running.lock() {
                    if let Some(cancel) = running.as_ref() {
                        cancel.cancel();
                    }
                }
            }
            window.app_handle().exit(1);
        } else {
            api.prevent_close();
            let _ = window.hide();
        }
    }
}

/// Eventos de ventana y comandos que se exponen a la interfaz; las pruebas de recorrido usan
/// esta misma función con el runtime simulado de Tauri.
fn configure<R: Runtime>(builder: Builder<R>, prepare_only: bool) -> Builder<R> {
    builder
        .plugin(tauri_plugin_dialog::init())
        .on_window_event(move |window, event| on_window_event(window, event, prepare_only))
        .invoke_handler(tauri::generate_handler![
            tasks,
            import_file,
            import_dialog,
            open_original,
            fetch_mail,
            mail_accounts,
            set_mail_limit,
            mail_sign_in,
            mail_sign_in_cancel,
            remove_mail_account,
            rename_mail_account,
            add_simulated_mail_account,
            approve,
            confirm_not_sent,
            edit_draft,
            reject,
            archive,
            sender_preferences,
            set_sender_preference,
            remove_sender_preference,
            redraft,
            retry,
            rules,
            rule_views,
            create_rule,
            rename_rule,
            update_rule,
            update_rule_with_ack,
            set_rule_active,
            set_rule_any_sender,
            archive_rule,
            unarchive_rule,
            rule_stats,
            simulate_rule,
            rule_findings,
            rule_suggestions,
            suggestion_refinement,
            refinement_preview,
            refine_suggestions,
            initial_read_overview,
            initial_read_folder_preview,
            pick_read_folder,
            start_initial_read,
            start_initial_read_folder,
            retry_initial_read,
            stop_initial_read,
            continue_initial_read,
            seen_invoices,
            grant_refine_consent,
            withdraw_refine_consent,
            accept_suggestion,
            discard_suggestion,
            suggestion_thresholds,
            set_suggestion_threshold,
            rule_seed_for_task,
            preview_merge,
            preview_restore,
            merge_rules,
            audit_log,
            settings,
            select_provider,
            delete_provider_key,
            provider_key_saved,
            dismiss_external_offer,
            list_models,
            test_provider,
            set_local_fallback,
            launch_mode,
            setup_check,
            setup_prepare,
            setup_cancel,
            setup_abandon,
        ])
}

fn main() {
    let prepare_only = std::env::args().skip(1).any(|arg| arg == PREPARE_ARG);
    let mut builder = tauri::Builder::default();
    if !prepare_only {
        // Primero: una segunda ejecución solo muestra la ventana existente.
        builder = builder.plugin(tauri_plugin_single_instance::init(|handle, _args, _cwd| {
            show_main(handle)
        }));
    }
    configure(builder, prepare_only)
        .setup(move |app| {
            if prepare_only {
                setup_prepare_only(app.handle())?;
            } else {
                start(app.handle(), Deps::installed(app.handle()))?;
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("no se pudo iniciar NestAdmin");
}

#[cfg(test)]
mod espera;
#[cfg(test)]
mod recorrido;
