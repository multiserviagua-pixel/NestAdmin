//! Ventana de NestAdmin (R01, flujo 5). Cerrar oculta la ventana y el trabajo sigue en un
//! hilo propio; el icono del área de notificación permite restaurarla o salir por completo.
//! La lógica vive en `nestadmin-app`; aquí solo se exponen comandos a la ventana principal.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod settings;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::Duration;

use nestadmin_ai::credentials::GROQ_API_KEY;
use nestadmin_ai::{CredentialStore, ProviderChoice, Secret};
use nestadmin_app::{App, AppPaths, ProviderInfo};
use nestadmin_core::{AuditEntry, Rule, Task};
use nestadmin_extract::{Extractor, Tesseract};
use nestadmin_setup::{Cancel, Check, Report, Setup, SetupError, Startup, WindowsSystem};
use serde::Serialize;
use serde_json::Value;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, State, WindowEvent};

use crate::settings::{credential_store, provider_for, Settings};

/// Evento que indica a la ventana que vuelva a leer tareas e historial.
const CHANGED: &str = "nestadmin://changed";
/// Progreso de cada paso de preparación.
const SETUP: &str = "nestadmin://setup";
/// Final de una preparación, con su informe o su error.
const SETUP_DONE: &str = "nestadmin://setup-done";
/// Resultado de la comprobación al abrir (D39).
const STARTUP: &str = "nestadmin://startup";
/// Argumento con el que el instalador lanza solo la preparación (D36).
const PREPARE_ARG: &str = "--preparar";

struct Shared {
    app: Mutex<App>,
    settings: Mutex<Settings>,
    credentials: Box<dyn CredentialStore>,
    data_dir: PathBuf,
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

type SharedState = Arc<Shared>;
type CmdResult<T> = Result<T, String>;

fn changed(handle: &AppHandle) {
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
fn import_file(
    shared: State<'_, SharedState>,
    handle: AppHandle,
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

#[tauri::command]
fn fetch_mail(shared: State<'_, SharedState>, handle: AppHandle) -> CmdResult<usize> {
    let added = shared.app()?.fetch_mail().map_err(text)?;
    shared.wake();
    changed(&handle);
    Ok(added)
}

#[tauri::command]
fn approve(
    shared: State<'_, SharedState>,
    handle: AppHandle,
    id: i64,
    version: i64,
) -> CmdResult<Task> {
    let task = shared.app()?.approve(id, version).map_err(text);
    changed(&handle);
    task
}

#[tauri::command]
fn edit_draft(
    shared: State<'_, SharedState>,
    handle: AppHandle,
    id: i64,
    body: String,
) -> CmdResult<Task> {
    let task = shared.app()?.edit_draft(id, &body).map_err(text)?;
    changed(&handle);
    Ok(task)
}

#[tauri::command]
fn reject(shared: State<'_, SharedState>, handle: AppHandle, id: i64) -> CmdResult<Task> {
    let task = shared.app()?.reject(id).map_err(text)?;
    changed(&handle);
    Ok(task)
}

#[tauri::command]
fn retry(shared: State<'_, SharedState>, handle: AppHandle, id: i64) -> CmdResult<Task> {
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
fn create_rule(
    shared: State<'_, SharedState>,
    handle: AppHandle,
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
fn update_rule(
    shared: State<'_, SharedState>,
    handle: AppHandle,
    id: String,
    conditions: Value,
) -> CmdResult<Rule> {
    let rule = shared.app()?.update_rule(&id, &conditions).map_err(text)?;
    changed(&handle);
    Ok(rule)
}

#[tauri::command]
fn set_rule_active(
    shared: State<'_, SharedState>,
    handle: AppHandle,
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
    let updated = Settings { provider: choice };
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

/// Comprobación rápida (R08), sin descargas. Puede tardar unos segundos si Ollama no
/// responde, así que se ejecuta fuera del hilo de la ventana.
#[tauri::command]
async fn setup_check(preparation: State<'_, Arc<Preparation>>) -> CmdResult<Check> {
    let setup = Arc::clone(&preparation.setup);
    tauri::async_runtime::spawn_blocking(move || setup.check())
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
fn setup_prepare(preparation: State<'_, Arc<Preparation>>, handle: AppHandle) -> CmdResult<()> {
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
fn setup_abandon(preparation: State<'_, Arc<Preparation>>, handle: AppHandle) {
    if let Ok(running) = preparation.running.lock() {
        if let Some(cancel) = running.as_ref() {
            cancel.cancel();
        }
    }
    if preparation.prepare_only {
        handle.exit(1);
    }
}

fn start_preparation(preparation: &Arc<Preparation>, handle: AppHandle) -> CmdResult<()> {
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
fn startup_check(preparation: Arc<Preparation>, handle: AppHandle, local_provider: bool) {
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
fn worker(shared: SharedState, handle: AppHandle, wake: Receiver<()>) {
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

fn show_main(handle: &AppHandle) {
    if let Some(window) = handle.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

/// Salida completa: no se toman tareas nuevas; una tarea en curso se recupera al reabrir.
fn quit(handle: &AppHandle) {
    if let Some(shared) = handle.try_state::<SharedState>() {
        shared.stopping.store(true, Ordering::SeqCst);
        shared.wake();
    }
    handle.exit(0);
}

fn data_dir(handle: &AppHandle) -> PathBuf {
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

fn preparation(data_dir: &std::path::Path, prepare_only: bool) -> Arc<Preparation> {
    let config = nestadmin_setup::Config::windows(
        tesseract_exe(),
        tessdata_dir(),
        pdfium_dir(),
        data_dir.join("descargas"),
    );
    Arc::new(Preparation {
        setup: Arc::new(Setup::new(config, Box::new(WindowsSystem))),
        prepare_only,
        running: Mutex::new(None),
        startup: Mutex::new(None),
    })
}

/// Modo `--preparar` (D36): solo el asistente. Sin base de datos, cola ni bandeja; cerrar la
/// ventana sin terminar devuelve 1 al instalador.
fn setup_prepare_only(handle: &AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    let data_dir = data_dir(handle);
    std::fs::create_dir_all(&data_dir)?;
    handle.manage(preparation(&data_dir, true));
    if let Some(window) = handle.get_webview_window("main") {
        let _ = window.set_title("Preparación de NestAdmin");
        let _ = window.set_size(tauri::LogicalSize::new(720.0, 600.0));
        let _ = window.center();
    }
    Ok(())
}

fn setup(handle: &AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    let data_dir = data_dir(handle);
    std::fs::create_dir_all(&data_dir)?;
    let credentials = credential_store();
    let settings = Settings::load(&data_dir);
    let local_provider = matches!(settings.provider, ProviderChoice::Ollama { .. });
    let provider = provider_for(&settings.provider, credentials.as_ref());
    let app = App::open(&AppPaths::new(&data_dir), provider, Arc::new(extractor()))?;
    let (sender, receiver) = mpsc::channel();
    let preparation = preparation(&data_dir, false);
    handle.manage(Arc::clone(&preparation));
    startup_check(preparation, handle.clone(), local_provider);
    let shared = Arc::new(Shared {
        app: Mutex::new(app),
        settings: Mutex::new(settings),
        credentials,
        data_dir,
        wake: Mutex::new(sender),
        stopping: AtomicBool::new(false),
    });
    handle.manage(Arc::clone(&shared));
    let worker_handle = handle.clone();
    thread::Builder::new()
        .name("nestadmin-cola".into())
        .spawn(move || worker(shared, worker_handle, receiver))?;

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

fn main() {
    let prepare_only = std::env::args().skip(1).any(|arg| arg == PREPARE_ARG);
    let mut builder = tauri::Builder::default();
    if !prepare_only {
        // Primero: una segunda ejecución solo muestra la ventana existente.
        builder = builder.plugin(tauri_plugin_single_instance::init(|handle, _args, _cwd| {
            show_main(handle)
        }));
    }
    builder
        .setup(move |app| {
            if prepare_only {
                setup_prepare_only(app.handle())?;
            } else {
                setup(app.handle())?;
            }
            Ok(())
        })
        .on_window_event(move |window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                if prepare_only {
                    // Cerrar el asistente del instalador equivale a cancelarlo.
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
        })
        .invoke_handler(tauri::generate_handler![
            tasks,
            import_file,
            fetch_mail,
            approve,
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
        .run(tauri::generate_context!())
        .expect("no se pudo iniciar NestAdmin");
}
