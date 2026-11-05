//! Ventana de NestAdmin (R01, flujo 5). Cerrar oculta la ventana y el trabajo sigue en un
//! hilo propio; el icono del área de notificación permite restaurarla o salir por completo.
//! La lógica vive en `nestadmin-app`; aquí solo se exponen comandos a la ventana principal.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod mail_clients;
mod settings;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::Duration;

use nestadmin_ai::credentials::GROQ_API_KEY;
use nestadmin_ai::{CredentialStore, ProviderChoice, Secret};
use nestadmin_app::accounts::{
    AccountView, FetchReport, MailAccountsView, MailEnv, MailProviderKind,
};
use nestadmin_app::{App, AppPaths, ProviderInfo};
use nestadmin_core::{AuditEntry, Rule, Task};
use nestadmin_extract::{Extractor, Tesseract};
use nestadmin_mail::PendingAuthorization;
use nestadmin_setup::{Cancel, Report, Setup, SetupError, Startup, Status, WindowsSystem};
use serde::Serialize;
use serde_json::Value;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Builder, Emitter, Manager, Runtime, State, Window, WindowEvent};
use tauri_plugin_dialog::DialogExt;

use crate::settings::{credential_store, provider_for, Settings};

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
            .add_filter("Facturas en PDF o texto", &["pdf", "txt"])
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
) -> CmdResult<AccountView> {
    let shared = Arc::clone(shared.inner());
    let emitter = handle.clone();
    let view = tauri::async_runtime::spawn_blocking(move || {
        sign_in(&shared, &emitter, provider, &label, account.as_deref())
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
    let mut app = shared.app()?;
    match account {
        Some(id) => app.reconnect_mail_account(id, &tokens),
        None => app.add_mail_account(provider, label, &tokens),
    }
    .map_err(text)
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

#[tauri::command]
fn create_rule<R: Runtime>(
    shared: State<'_, SharedState>,
    handle: AppHandle<R>,
    id: String,
    action_kind: String,
    conditions: Value,
) -> CmdResult<Rule> {
    let rule = shared
        .app()?
        .create_rule(&id, &action_kind, &conditions)
        .map_err(text)?;
    changed(&handle);
    Ok(rule)
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
    choice: ProviderChoice,
    active: ProviderInfo,
    groq_key_saved: bool,
    data_dir: PathBuf,
}

#[tauri::command]
fn settings(shared: State<'_, SharedState>) -> CmdResult<SettingsView> {
    let choice = shared.settings.lock().map_err(text)?.provider.clone();
    let groq_key_saved = shared
        .credentials
        .get(GROQ_API_KEY)
        .map_err(text)?
        .is_some();
    Ok(SettingsView {
        choice,
        active: shared.app()?.provider(),
        groq_key_saved,
        data_dir: shared.data_dir.clone(),
    })
}

/// Cambio explícito de proveedor. Si el elegido no se puede preparar (por ejemplo, Groq sin
/// clave), se informa y se mantiene la selección anterior.
#[tauri::command]
fn select_provider(
    shared: State<'_, SharedState>,
    choice: ProviderChoice,
) -> CmdResult<ProviderInfo> {
    let provider = settings::build(&choice, shared.credentials.as_ref()).map_err(text)?;
    let mut current = shared.settings.lock().map_err(text)?;
    let updated = Settings {
        provider: choice,
        ..current.clone()
    };
    updated.save(&shared.data_dir).map_err(text)?;
    *current = updated;
    let mut app = shared.app()?;
    app.select_provider(provider);
    Ok(app.provider())
}

/// La clave va directa al Administrador de credenciales de Windows; nunca se devuelve.
#[tauri::command]
fn save_groq_key(shared: State<'_, SharedState>, key: String) -> CmdResult<()> {
    let key = key.trim();
    if key.is_empty() {
        return Err("la clave está vacía".into());
    }
    shared
        .credentials
        .set(GROQ_API_KEY, &Secret::new(key))
        .map_err(text)
}

#[tauri::command]
fn delete_groq_key(shared: State<'_, SharedState>) -> CmdResult<()> {
    shared.credentials.delete(GROQ_API_KEY).map_err(text)
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

/// Procesa la cola sin bloquear la ventana: la llamada al modelo se hace sin el cerrojo.
fn worker<R: Runtime>(shared: SharedState, handle: AppHandle<R>, wake: Receiver<()>) {
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
                let _ = wake.recv_timeout(Duration::from_secs(5));
            }
            Err(error) => {
                eprintln!("cola no disponible: {error}");
                let _ = wake.recv_timeout(Duration::from_secs(5));
            }
        }
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
    let local_provider = matches!(settings.provider, ProviderChoice::Ollama { .. });
    let provider = provider_for(&settings.provider, credentials.as_ref());
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
    if let Err(error) = app.set_mail_limit(settings.mail_limit) {
        eprintln!("límite de correo no válido en la configuración: {error}");
    }
    let (sender, receiver) = mpsc::channel();
    let preparation = preparation(setup, false);
    handle.manage(Arc::clone(&preparation));
    startup_check(preparation, handle.clone(), local_provider);
    let shared = Arc::new(Shared {
        app: Mutex::new(app),
        settings: Mutex::new(settings),
        credentials,
        data_dir,
        sign_in: Mutex::new(None),
        mail_clients_error,
        wake: Mutex::new(sender),
        stopping: AtomicBool::new(false),
    });
    handle.manage(Arc::clone(&shared));
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
            retry,
            rules,
            create_rule,
            update_rule,
            set_rule_active,
            audit_log,
            settings,
            select_provider,
            save_groq_key,
            delete_groq_key,
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
mod recorrido;
