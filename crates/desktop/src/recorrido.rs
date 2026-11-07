//! Recorridos de extremo a extremo desde la ventana (T02, T06, T07 y parte de T01; D53).
//!
//! Cada prueba arranca la aplicación de escritorio real —`configure` registra los mismos
//! comandos y eventos de ventana que `main`, y `start` abre los datos, la cola y su hilo— con el
//! runtime simulado de Tauri en lugar de WebView2. La «interfaz» es una ventana `main` que llama
//! a los comandos por IPC con los mismos nombres y argumentos que `ui/src/api.ts`, incluido el
//! cuerpo binario de `import_file`. El proveedor es el adaptador real de Ollama, elegido en
//! `configuracion.json`, contra un Ollama simulado en 127.0.0.1; el correo y la administración
//! son los simuladores. Los datos van a un directorio temporal (en el proceso hijo, mediante
//! `NESTADMIN_DATA_DIR`); nunca se usa `%LOCALAPPDATA%\NestAdmin`, la bandeja del sistema, el
//! Administrador de credenciales ni el Ollama del equipo.
//!
//! Las interrupciones reales se hacen con un proceso hijo —este mismo ejecutable de pruebas— que
//! se termina con `Child::kill` (`TerminateProcess` en Windows) en mitad del trabajo.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::AtomicUsize;
use std::time::Instant;

use nestadmin_ai::MemoryCredentialStore;
use nestadmin_extract::{ExtractError, Ocr};
use nestadmin_setup::System;
use serde_json::json;
use tauri::ipc::{CallbackFn, InvokeBody};
use tauri::test::{get_ipc_response, mock_builder, MockRuntime, INVOKE_KEY};
use tauri::webview::InvokeRequest;
use tauri::{WebviewUrl, WebviewWindow, WebviewWindowBuilder};

use super::*;

/// Variable con la que el proceso padre pide al hijo un escenario.
const CHILD: &str = "NESTADMIN_E2E_HIJO";
/// Dirección del Ollama simulado para el proceso hijo.
const CHILD_AI: &str = "NESTADMIN_E2E_IA";
/// Plazo máximo de cualquier espera; los pasos normales tardan milisegundos.
const WAIT: Duration = Duration::from_secs(30);

// ---- Ollama simulado en 127.0.0.1 ----

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    /// Devuelve el JSON incluido en el documento o un borrador fijo.
    Answer,
    /// Responde 500, como un Ollama sin el modelo cargado.
    Fail,
    /// Recibe la petición y no responde: el proceso que la hizo se termina antes.
    Hang,
}

struct FakeOllama {
    url: String,
    mode: Arc<Mutex<Mode>>,
    /// Peticiones a `/api/chat`; las demás rutas (comprobación al abrir) no cuentan.
    chats: Arc<AtomicUsize>,
}

impl FakeOllama {
    fn start(mode: Mode) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let mode = Arc::new(Mutex::new(mode));
        let chats = Arc::new(AtomicUsize::new(0));
        let (m, c) = (Arc::clone(&mode), Arc::clone(&chats));
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let (m, c) = (Arc::clone(&m), Arc::clone(&c));
                thread::spawn(move || serve(stream, &m, &c));
            }
        });
        Self { url, mode, chats }
    }

    fn set(&self, mode: Mode) {
        *self.mode.lock().unwrap() = mode;
    }

    fn chats(&self) -> usize {
        self.chats.load(Ordering::SeqCst)
    }
}

fn serve(mut stream: TcpStream, mode: &Mutex<Mode>, chats: &AtomicUsize) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).unwrap_or(0) == 0 {
        return;
    }
    let mut length = 0;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
            break;
        }
        if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
            length = v.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0; length];
    if reader.read_exact(&mut body).is_err() {
        return;
    }
    let (status, reply) = if request_line.starts_with("POST /api/chat ") {
        chats.fetch_add(1, Ordering::SeqCst);
        let request: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
        let current = *mode.lock().unwrap();
        match current {
            Mode::Answer => (200, chat_reply(&request)),
            Mode::Fail => (500, json!({ "error": "model not loaded" })),
            Mode::Hang => {
                thread::sleep(Duration::from_secs(120));
                return;
            }
        }
    } else {
        (404, json!({ "error": "not found" }))
    };
    let text = reply.to_string();
    let _ = write!(
        stream,
        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
        text.len()
    );
}

/// Extracción: el documento sintético lleva su propio JSON y se devuelve tal cual. Correo: una
/// clasificación y un borrador fijos.
fn chat_reply(request: &Value) -> Value {
    let extraction = request
        .pointer("/format/properties/invoice_number")
        .is_some();
    let content = if extraction {
        let text = request
            .pointer("/messages/1/content")
            .and_then(Value::as_str)
            .unwrap_or("");
        match (text.find('{'), text.rfind('}')) {
            (Some(a), Some(b)) => text[a..=b].to_owned(),
            _ => "{}".to_owned(),
        }
    } else {
        json!({ "category": "solicitud_factura", "draft": "Adjuntamos la factura solicitada." })
            .to_string()
    };
    json!({
        "model": "qwen3:8b",
        "message": { "role": "assistant", "content": content },
        "done": true,
    })
}

// ---- Dobles de lo que no debe tocarse en el equipo ----

/// Sistema sin Ollama ni Tesseract: la comprobación al abrir no arranca ni instala nada.
struct NoSystem;

impl System for NoSystem {
    fn ollama_executable(&self) -> Option<PathBuf> {
        None
    }
    fn verify_signature(&self, _file: &Path, _publisher: &str) -> Result<(), String> {
        Err("no disponible en pruebas".into())
    }
    fn install_ollama(&self, _installer: &Path) -> Result<(), String> {
        Err("no disponible en pruebas".into())
    }
    fn start_ollama(&self, _executable: &Path) -> Result<(), String> {
        Err("no disponible en pruebas".into())
    }
    fn tesseract_version(&self, _executable: &Path, _tessdata: &Path) -> Result<String, String> {
        Err("no disponible en pruebas".into())
    }
    fn memory(&self) -> Option<(u64, u64)> {
        None
    }
}

/// Los recorridos importan texto; PDF y OCR se prueban en T04.
struct NoOcr;

impl Ocr for NoOcr {
    fn recognize(&self, _image: &Path, page: usize) -> Result<String, ExtractError> {
        Err(ExtractError::OcrUnavailable(format!("página {page}")))
    }
}

fn test_deps(data_dir: &Path, ai_url: &str) -> Deps {
    let mut config = nestadmin_setup::Config::windows(
        data_dir.join("sin-tesseract.exe"),
        data_dir.join("sin-tessdata"),
        data_dir.join("sin-pdfium"),
        data_dir.join("descargas"),
    );
    config.ollama_url = ai_url.to_owned();
    Deps {
        data_dir: data_dir.to_owned(),
        credentials: Arc::new(MemoryCredentialStore::default()),
        setup: Setup::new(config, Box::new(NoSystem)),
        extractor: Extractor::new(data_dir.join("sin-pdfium"), Box::new(NoOcr)),
        tray: false,
    }
}

/// Selección local con la dirección del Ollama simulado, como la guardaría Configuración.
fn select_local(data_dir: &Path, ai_url: &str) {
    std::fs::create_dir_all(data_dir).unwrap();
    let settings = Settings {
        provider: ProviderChoice::Ollama {
            base_url: ai_url.to_owned(),
            model: "qwen3:8b".to_owned(),
        },
        ..Settings::default()
    };
    settings.save(data_dir).unwrap();
}

// ---- La aplicación con su ventana ----

struct Session {
    app: tauri::App<MockRuntime>,
    window: WebviewWindow<MockRuntime>,
    worker: Option<thread::JoinHandle<()>>,
}

impl Session {
    /// Abre la aplicación sobre `data_dir`, como al ejecutar `nestadmin.exe`.
    fn open(deps: Deps) -> Self {
        let app = configure(mock_builder(), false)
            .build(tauri::generate_context!())
            .expect("la aplicación se construye");
        let worker = start(app.handle(), deps).expect("la aplicación arranca");
        let window = WebviewWindowBuilder::new(&app, "main", WebviewUrl::default())
            .build()
            .expect("ventana principal");
        Self {
            app,
            window,
            worker: Some(worker),
        }
    }

    fn request(
        &self,
        cmd: &str,
        body: InvokeBody,
        headers: tauri::http::HeaderMap,
    ) -> Result<Value, Value> {
        let url = if cfg!(windows) {
            "http://tauri.localhost"
        } else {
            "tauri://localhost"
        };
        get_ipc_response(
            &self.window,
            InvokeRequest {
                cmd: cmd.into(),
                callback: CallbackFn(0),
                error: CallbackFn(1),
                url: url.parse().unwrap(),
                body,
                headers,
                invoke_key: INVOKE_KEY.to_owned(),
            },
        )
        .map(|body| body.deserialize::<Value>().unwrap())
    }

    /// Un comando con argumentos JSON, como `invoke(cmd, args)` en la interfaz.
    fn call(&self, cmd: &str, args: Value) -> Result<Value, Value> {
        self.request(cmd, InvokeBody::Json(args), Default::default())
    }

    fn ok(&self, cmd: &str, args: Value) -> Value {
        self.call(cmd, args)
            .unwrap_or_else(|e| panic!("{cmd} falló: {e}"))
    }

    /// `api.importFile`: el archivo como cuerpo binario y el nombre en una cabecera.
    fn import(&self, name: &str, bytes: &[u8]) -> Value {
        let mut headers = tauri::http::HeaderMap::new();
        let encoded: String = name
            .bytes()
            .map(|b| match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' => {
                    (b as char).to_string()
                }
                _ => format!("%{b:02X}"),
            })
            .collect();
        headers.insert("x-nestadmin-name", encoded.parse().unwrap());
        self.request("import_file", InvokeBody::Raw(bytes.to_vec()), headers)
            .unwrap_or_else(|e| panic!("import_file falló: {e}"))
    }

    fn tasks(&self) -> Vec<Value> {
        let tasks = self.ok("tasks", json!({}));
        tasks.as_array().cloned().unwrap_or_default()
    }

    fn task(&self, id: i64) -> Value {
        self.tasks()
            .into_iter()
            .find(|t| t["id"] == id)
            .unwrap_or_else(|| panic!("no existe la tarea {id}"))
    }

    /// Espera, como la ventana al recibir `nestadmin://changed`, a que la tarea llegue a `state`.
    fn wait_state(&self, id: i64, state: &str) -> Value {
        let started = Instant::now();
        loop {
            let task = self.task(id);
            if task["state"] == state {
                return task;
            }
            assert!(
                started.elapsed() < WAIT,
                "la tarea {id} no llegó a {state}: {task}"
            );
            thread::sleep(Duration::from_millis(25));
        }
    }

    fn audit(&self) -> Vec<Value> {
        self.ok("audit_log", json!({}))
            .as_array()
            .cloned()
            .unwrap_or_default()
    }

    /// Facturas registradas en el simulador por estos recorridos, sin contar las sintéticas
    /// con las que el simulador se crea.
    fn registered(&self) -> usize {
        let shared = self.app.state::<SharedState>();
        let app = shared.app().unwrap();
        app.simulators().1.record_count().unwrap() - seeded_records()
    }

    fn sent(&self) -> usize {
        let shared = self.app.state::<SharedState>();
        let app = shared.app().unwrap();
        app.simulators().0.sent().unwrap().len()
    }

    /// Cuerpo del borrador creado para el mensaje, si existe.
    fn draft_body(&self, message_id: &str) -> Option<String> {
        let shared = self.app.state::<SharedState>();
        let app = shared.app().unwrap();
        let drafts = app.simulators().0.drafts().unwrap();
        drafts
            .into_iter()
            .find(|d| d.message_id.ends_with(message_id))
            .map(|d| d.content.body)
    }

    /// «Salir de NestAdmin por completo» sin terminar el proceso de pruebas: la cola deja de
    /// tomar tareas, su hilo termina y se liberan la base y los simuladores.
    fn close(mut self) {
        stop_worker(self.app.handle());
        if let Some(worker) = self.worker.take() {
            worker.join().unwrap();
        }
        let _ = self.window.destroy();
    }
}

/// Registros con los que nace el simulador de administración en una carpeta vacía.
fn seeded_records() -> usize {
    static SEEDED: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *SEEDED.get_or_init(|| {
        let dir = tempfile::tempdir().unwrap();
        let extractor = Extractor::new(dir.path().join("sin-pdfium"), Box::new(NoOcr));
        let app = App::open(
            &AppPaths::new(dir.path()),
            provider_for(
                &ProviderChoice::default(),
                &MemoryCredentialStore::default(),
            ),
            Arc::new(extractor),
        )
        .unwrap();
        let count = app.simulators().1.record_count().unwrap();
        count
    })
}

fn id(task: &Value) -> i64 {
    task["id"].as_i64().unwrap()
}

fn version(task: &Value) -> i64 {
    task["result"]["version"].as_i64().unwrap()
}

/// Factura sintética: el Ollama simulado devuelve el JSON que contiene.
fn invoice(supplier: &str, number: &str, day: u32, total: &str) -> Vec<u8> {
    format!(
        "FACTURA {number}\nProveedor: {supplier}\n{}",
        json!({
            "supplier": supplier,
            "invoice_number": number,
            "date": format!("2026-10-{day:02}"),
            "subtotal": "100.00",
            "tax": "21.00",
            "total": total,
        })
    )
    .into_bytes()
}

fn data_dir_for(dir: &tempfile::TempDir) -> PathBuf {
    dir.path().join("datos")
}

// ---- Recorridos ----

/// Cada comando que invoca `ui/src/api.ts` está registrado en `configure`, y al revés. Los
/// recorridos llaman a los comandos con los nombres y argumentos de `api.ts`; esta prueba evita
/// que la interfaz y los comandos se separen sin que nadie lo note.
#[test]
fn la_interfaz_solo_llama_a_comandos_registrados() {
    let api = include_str!("../../../ui/src/api.ts");
    let called: std::collections::BTreeSet<&str> = api
        .split("invoke")
        .skip(1)
        .filter_map(|rest| {
            let rest = match rest.strip_prefix('<') {
                Some(generic) => &generic[generic.find(">(")? + 1..],
                None => rest,
            };
            let name = rest.strip_prefix("(\"")?;
            Some(&name[..name.find('"')?])
        })
        .collect();
    let source = include_str!("main.rs");
    let list = &source[source.find("generate_handler![").unwrap()..];
    let registered: std::collections::BTreeSet<&str> = list
        ["generate_handler![".len()..list.find(']').unwrap()]
        .split(',')
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .collect();
    assert!(called.len() >= 25, "no se leyeron las llamadas: {called:?}");
    assert_eq!(called, registered);
}

/// Flujo 1 y R07: importar dos facturas, ver sus incidencias, descartar una, aprobar la otra y
/// comprobar que se registra una sola vez y queda en el historial.
#[test]
fn documento_importado_se_revisa_y_se_registra_una_sola_vez() {
    let dir = tempfile::tempdir().unwrap();
    let data = data_dir_for(&dir);
    let ai = FakeOllama::start(Mode::Answer);
    select_local(&data, &ai.url);
    let s = Session::open(test_deps(&data, &ai.url));

    let launch = s.ok("launch_mode", json!({}));
    assert_eq!(launch["prepare_only"], false);
    let settings = s.ok("settings", json!({}));
    assert_eq!(settings["choice"]["provider"], "ollama");
    assert_eq!(settings["active"]["provider"], "ollama");
    assert_eq!(settings["groq_key_saved"], false);

    let clean = s.import(
        "Factura Alfa F-1.txt",
        &invoice("Alfa SL", "F-1", 1, "121.00"),
    );
    let wrong = s.import("factura-beta.txt", &invoice("Beta SA", "B-7", 2, "125.00"));
    assert_eq!(clean["state"], "queued");

    let clean = s.wait_state(id(&clean), "awaiting_review");
    let wrong = s.wait_state(id(&wrong), "awaiting_review");
    assert_eq!(clean["result"]["issues"], json!([]));
    assert_eq!(clean["result"]["fields"]["total"], "121.00");
    assert_eq!(clean["result"]["provider"], "ollama");
    // Origen del documento (D49): archivo importado por la ventana, con tamaño y huella.
    let origin = &clean["result"]["origin"];
    assert_eq!(origin["kind"], "file", "{origin}");
    assert_eq!(
        origin["sha256"].as_str().map(str::len),
        Some(64),
        "{origin}"
    );
    assert!(origin["size"].as_u64().is_some_and(|n| n > 0), "{origin}");
    assert_eq!(
        clean["result"]["review_reason"],
        "ninguna regla activa coincide"
    );
    assert!(
        wrong["result"]["issues"]
            .as_array()
            .unwrap()
            .contains(&json!("total_mismatch")),
        "{wrong}"
    );
    assert_eq!(ai.chats(), 2, "una llamada al modelo por documento");
    assert_eq!(s.registered(), 0, "sin aprobación no se registra nada");

    // Descartar la factura con incidencias no ejecuta nada.
    let rejected = s.ok("reject", json!({ "id": id(&wrong) }));
    assert_eq!(rejected["state"], "cancelled");
    assert!(s
        .call("approve", json!({ "id": id(&wrong), "version": 1 }))
        .is_err());

    // Aprobar la versión vista registra una vez; repetir la aprobación es un error.
    let done = s.ok(
        "approve",
        json!({ "id": id(&clean), "version": version(&clean) }),
    );
    assert_eq!(done["state"], "completed");
    assert!(done["result"]["receipt"].is_object(), "{done}");
    let again = s.call(
        "approve",
        json!({ "id": id(&clean), "version": version(&clean) }),
    );
    assert!(again.is_err());
    assert_eq!(s.registered(), 1);

    let audit = s.audit();
    let confirmed: Vec<_> = audit
        .iter()
        .filter(|e| {
            e["task_id"] == id(&clean)
                && e["action"] == "register_invoice"
                && e["status"] == "confirmed"
        })
        .collect();
    assert_eq!(confirmed.len(), 1, "{audit:?}");
    assert!(confirmed[0]["authorization"]
        .as_str()
        .unwrap_or("")
        .contains("aprobación"));
    assert!(
        audit
            .iter()
            .all(|e| e["task_id"] != id(&wrong) || e["action"] != "register_invoice"),
        "la factura descartada no tiene acciones: {audit:?}"
    );
    s.close();
}

/// Flujo 2, R06 y R07: consultar el correo simulado, revisar el borrador, editarlo (versión
/// nueva), comprobar que la versión anterior ya no se puede aprobar y enviar una sola vez.
#[test]
fn correo_simulado_borrador_editado_se_envia_una_sola_vez() {
    let dir = tempfile::tempdir().unwrap();
    let data = data_dir_for(&dir);
    let ai = FakeOllama::start(Mode::Answer);
    select_local(&data, &ai.url);
    let s = Session::open(test_deps(&data, &ai.url));

    let accounts = s.ok("mail_accounts", json!({}));
    assert_eq!(accounts["signing_in"], false);
    let report = s.ok("fetch_mail", json!({}));
    let first = report["accounts"]
        .as_array()
        .and_then(|a| a.first())
        .cloned()
        .unwrap_or_else(|| panic!("informe sin cuentas: {report}"));
    assert_eq!(first["account"], "simulador", "{report}");
    assert!(first["error"].is_null(), "{report}");
    let added = first["added"].as_u64().unwrap();
    assert!(added > 0, "{report}");

    // Consultar otra vez no duplica tareas.
    let again = s.ok("fetch_mail", json!({}));
    assert_eq!(again["accounts"][0]["added"], 0, "{again}");
    let emails: Vec<Value> = s
        .tasks()
        .into_iter()
        .filter(|t| t["kind"] == "email")
        .collect();
    assert_eq!(emails.len() as u64, added);

    // El mensaje con remitente queda en revisión con su borrador.
    let target = emails
        .iter()
        .map(|t| s.wait_state(id(t), "awaiting_review"))
        .find(|t| {
            t["result"]["message"]["id"]
                .as_str()
                .is_some_and(|id| id.ends_with("M-0002"))
        })
        .expect("mensaje M-0002 en revisión");
    assert_eq!(
        target["result"]["draft"]["to"],
        json!(["cliente.beta@ejemplo.test"])
    );
    // Origen del correo (D48): la cuenta simulada que lo recibió.
    assert_eq!(
        target["result"]["origin"]["account"], "simulador",
        "{target}"
    );
    assert_eq!(s.sent(), 0);

    let edited = s.ok(
        "edit_draft",
        json!({ "id": id(&target), "body": "Confirmamos la entrega del pedido 2231 el lunes." }),
    );
    assert_eq!(version(&edited), 2);
    let stale = s.call("approve", json!({ "id": id(&target), "version": 1 }));
    assert!(stale.is_err(), "aprobar la versión 1 debe rechazarse");
    assert_eq!(s.sent(), 0, "nada enviado con la versión anterior");

    let sent = s.ok("approve", json!({ "id": id(&target), "version": 2 }));
    assert_eq!(sent["state"], "completed");
    assert_eq!(s.sent(), 1);
    assert_eq!(
        s.draft_body("M-0002").as_deref(),
        Some("Confirmamos la entrega del pedido 2231 el lunes.")
    );
    assert!(s
        .call("approve", json!({ "id": id(&target), "version": 2 }))
        .is_err());
    assert_eq!(s.sent(), 1);
    s.close();
}

/// R02, R07 y D50: el envío aprobado falla porque el correo no está disponible; nada se marca
/// como enviado. «Reintentar» devuelve la tarea a revisión con el mismo texto y versión, sin
/// volver a llamar a la IA. «No se envió» (`confirm_not_sent`) prepara una versión nueva, sin
/// enviar nada, y aprobarla envía una sola vez.
#[test]
fn envio_fallido_se_confirma_como_no_enviado_y_se_envia_una_vez() {
    let dir = tempfile::tempdir().unwrap();
    let data = data_dir_for(&dir);
    let ai = FakeOllama::start(Mode::Answer);
    select_local(&data, &ai.url);
    let s = Session::open(test_deps(&data, &ai.url));

    s.ok("fetch_mail", json!({}));
    let target = s
        .tasks()
        .into_iter()
        .filter(|t| t["kind"] == "email")
        .map(|t| s.wait_state(id(&t), "awaiting_review"))
        .find(|t| {
            t["result"]["message"]["id"]
                .as_str()
                .is_some_and(|id| id.ends_with("M-0002"))
        })
        .expect("mensaje M-0002 en revisión");
    let chats = ai.chats();
    // Sin ningún intento anterior no hay nada que confirmar.
    assert!(s
        .call("confirm_not_sent", json!({ "id": id(&target) }))
        .is_err());

    let set_failure = |failure| {
        let shared = s.app.state::<SharedState>();
        let app = shared.app().unwrap();
        app.simulators().0.set_failure(failure);
    };
    set_failure(Some(nestadmin_simulators::FailureMode::Unavailable));
    let failed = s.ok("approve", json!({ "id": id(&target), "version": 1 }));
    assert_eq!(failed["state"], "failed", "{failed}");
    assert_eq!(failed["error"]["code"], "unavailable");
    set_failure(None);
    assert_eq!(s.sent(), 0, "un fallo nunca cuenta como envío");

    s.ok("retry", json!({ "id": id(&target) }));
    let review = s.wait_state(id(&target), "awaiting_review");
    assert_eq!(version(&review), 1, "{review}");
    assert!(review["result"]["recovery"].is_object(), "{review}");
    assert_eq!(ai.chats(), chats, "reprocesar no vuelve a llamar a la IA");

    let confirmed = s.ok("confirm_not_sent", json!({ "id": id(&target) }));
    assert_eq!(confirmed["state"], "awaiting_review", "{confirmed}");
    assert_eq!(version(&confirmed), 2);
    assert!(confirmed["result"].get("recovery").is_none(), "{confirmed}");
    assert_eq!(s.sent(), 0, "confirmar no envía nada");
    assert!(s
        .call("approve", json!({ "id": id(&target), "version": 1 }))
        .is_err());

    let done = s.ok("approve", json!({ "id": id(&target), "version": 2 }));
    assert_eq!(done["state"], "completed", "{done}");
    assert_eq!(s.sent(), 1);
    assert_eq!(ai.chats(), chats);
    s.close();
}

/// R07: una regla expresa creada desde la ventana empieza inactiva; activa, automatiza la
/// factura limpia que coincide; desactivada, la siguiente vuelve a revisión. Cada cambio queda
/// como versión nueva y la acción automatizada guarda la regla y su versión.
#[test]
fn regla_expresa_se_activa_y_desactiva_desde_la_ventana() {
    let dir = tempfile::tempdir().unwrap();
    let data = data_dir_for(&dir);
    let ai = FakeOllama::start(Mode::Answer);
    select_local(&data, &ai.url);
    let s = Session::open(test_deps(&data, &ai.url));

    // Mismos argumentos que `api.createRule` (camelCase).
    let rule = s.ok(
        "create_rule",
        json!({
            "id": "alfa-hasta-500",
            "actionKind": "register_invoice",
            "conditions": { "supplier": "Alfa SL", "max_total": "500.00" },
        }),
    );
    assert_eq!(rule["active"], false);
    let invalid = s.call(
        "create_rule",
        json!({ "id": "sin-proveedor", "actionKind": "register_invoice", "conditions": {} }),
    );
    assert!(invalid.is_err());

    let inactive = s.import("f1.txt", &invoice("Alfa SL", "F-1", 1, "121.00"));
    let inactive = s.wait_state(id(&inactive), "awaiting_review");
    assert_eq!(s.registered(), 0);

    let active = s.ok(
        "set_rule_active",
        json!({ "id": "alfa-hasta-500", "active": true }),
    );
    assert_eq!(active["active"], true);
    let rule_version = active["version"].as_i64().unwrap();
    assert!(rule_version > rule["version"].as_i64().unwrap());

    let automated = s.import("f2.txt", &invoice("Alfa SL", "F-2", 2, "121.00"));
    let automated = s.wait_state(id(&automated), "completed");
    assert_eq!(
        automated["result"]["authorization"]["rule_id"],
        "alfa-hasta-500"
    );
    assert_eq!(
        automated["result"]["authorization"]["rule_version"],
        rule_version
    );
    assert_eq!(s.registered(), 1);

    // Con la regla activa, otro proveedor o una incidencia siguen en revisión.
    let other = s.import("f3.txt", &invoice("Gamma SL", "G-3", 3, "121.00"));
    let blocked = s.import("f4.txt", &invoice("Alfa SL", "F-4", 4, "130.00"));
    s.wait_state(id(&other), "awaiting_review");
    s.wait_state(id(&blocked), "awaiting_review");
    assert_eq!(s.registered(), 1);

    let off = s.ok(
        "set_rule_active",
        json!({ "id": "alfa-hasta-500", "active": false }),
    );
    assert_eq!(off["active"], false);
    let after = s.import("f5.txt", &invoice("Alfa SL", "F-5", 5, "121.00"));
    s.wait_state(id(&after), "awaiting_review");
    assert_eq!(s.registered(), 1);

    let rules = s.ok("rules", json!({}));
    let listed = rules.as_array().unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0]["active"], false);
    assert!(listed[0]["version"].as_i64().unwrap() > rule_version);

    let audit = s.audit();
    let by_rule: Vec<_> = audit
        .iter()
        .filter(|e| e["task_id"] == id(&automated) && e["status"] == "confirmed")
        .collect();
    assert_eq!(by_rule.len(), 1, "{audit:?}");
    let authorization = by_rule[0]["authorization"].as_str().unwrap_or("");
    assert!(
        authorization.contains("alfa-hasta-500"),
        "historial sin la regla: {authorization}"
    );
    assert!(
        audit
            .iter()
            .any(|e| e["action"].as_str().unwrap_or("").contains("rule")),
        "los cambios de regla están en el historial: {audit:?}"
    );
    // La factura que esperó con la regla inactiva sigue pendiente: activar no la ejecuta.
    assert_eq!(s.task(id(&inactive))["state"], "awaiting_review");
    s.close();
}

/// R02, R03 y flujo 3: un fallo del modelo local se ve como tarea fallida con su código, no
/// cambia de proveedor ni se reintenta solo; el reintento explícito la procesa. Elegir Groq sin
/// clave se rechaza y la selección local se mantiene.
#[test]
fn fallo_del_modelo_es_visible_y_el_reintento_es_explicito() {
    let dir = tempfile::tempdir().unwrap();
    let data = data_dir_for(&dir);
    let ai = FakeOllama::start(Mode::Fail);
    select_local(&data, &ai.url);
    let s = Session::open(test_deps(&data, &ai.url));

    let task = s.import("f1.txt", &invoice("Alfa SL", "F-1", 1, "121.00"));
    let failed = s.wait_state(id(&task), "failed");
    assert!(
        failed["error"]["code"]
            .as_str()
            .is_some_and(|c| !c.is_empty()),
        "{failed}"
    );
    assert!(failed["result"].is_null());
    thread::sleep(Duration::from_millis(300));
    assert_eq!(ai.chats(), 1, "sin reintentos automáticos");
    assert_eq!(s.task(id(&task))["state"], "failed");

    let groq = s.call(
        "select_provider",
        json!({ "choice": { "provider": "groq", "model": "llama-3.3-70b-versatile" } }),
    );
    assert!(groq.is_err(), "Groq sin clave no se puede elegir");
    let settings = s.ok("settings", json!({}));
    assert_eq!(settings["choice"]["provider"], "ollama");
    assert_eq!(settings["active"]["provider"], "ollama");

    ai.set(Mode::Answer);
    let retried = s.ok("retry", json!({ "id": id(&task) }));
    assert_eq!(retried["state"], "queued");
    let review = s.wait_state(id(&task), "awaiting_review");
    assert_eq!(review["attempts"], 2);
    assert_eq!(ai.chats(), 2);
    s.close();
}

// ---- Interrupciones y salidas reales del proceso ----
//
// Cada reapertura se hace en otro proceso, como al volver a abrir NestAdmin: así se comprueba
// también que la base y los simuladores quedan libres cuando el proceso anterior termina o muere.
// El proceso de la prueba solo abre los datos al final, para verificar.

/// Escenarios del proceso hijo. Lee la carpeta de datos de `NESTADMIN_DATA_DIR` con la misma
/// función que la aplicación y deja en `<escenario>.listo` el resultado (JSON) al llegar al
/// punto pedido. Después espera a que el padre lo termine, salvo `salir`, que termina solo.
#[test]
#[ignore = "solo lo lanzan las pruebas de interrupción como proceso hijo"]
fn proceso_hijo() {
    let Ok(scenario) = std::env::var(CHILD) else {
        return;
    };
    let ai = std::env::var(CHILD_AI).unwrap();
    let probe = configure(mock_builder(), false)
        .build(tauri::generate_context!())
        .unwrap();
    let data = data_dir(probe.handle());
    drop(probe);
    let s = Session::open(test_deps(&data, &ai));
    let pending = || {
        s.tasks()
            .into_iter()
            .find(|t| t["state"] != "completed")
            .expect("tarea pendiente")
    };
    let outcome = match scenario.as_str() {
        // Importa y el modelo no responde: la tarea queda `running` cuando llega el corte.
        "importar" => s.import("corte.txt", &invoice("Alfa SL", "F-9", 9, "121.00")),
        // Reabre tras el corte y espera a que la tarea vuelva a estar en revisión.
        "reabrir" => s.wait_state(id(&pending()), "awaiting_review"),
        // Aprueba la factura pendiente y espera a que conste registrada.
        "aprobar" => {
            let task = pending();
            s.ok(
                "approve",
                json!({ "id": id(&task), "version": version(&task) }),
            )
        }
        // Registra una factura, sale por completo con otra recién importada y termina.
        "salir" => {
            let done = s.import("f1.txt", &invoice("Alfa SL", "F-1", 1, "121.00"));
            let done = s.wait_state(id(&done), "awaiting_review");
            s.ok("approve", json!({ "id": id(&done), "version": 1 }));
            stop_worker(s.app.handle());
            thread::sleep(Duration::from_millis(200));
            let queued = s.import("f2.txt", &invoice("Alfa SL", "F-2", 2, "121.00"));
            thread::sleep(Duration::from_millis(500));
            assert_eq!(s.task(id(&queued))["state"], "queued");
            let outcome = json!({ "done": id(&done), "queued": id(&queued) });
            std::fs::write(data.join("salir.listo"), outcome.to_string()).unwrap();
            s.close();
            return;
        }
        other => panic!("escenario desconocido: {other}"),
    };
    std::fs::write(data.join(format!("{scenario}.listo")), outcome.to_string()).unwrap();
    loop {
        thread::sleep(Duration::from_secs(1));
    }
}

fn spawn_child(scenario: &str, data: &Path, ai: &str) -> Child {
    Command::new(std::env::current_exe().unwrap())
        .args([
            "recorrido::proceso_hijo",
            "--exact",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CHILD, scenario)
        .env(CHILD_AI, ai)
        .env("NESTADMIN_DATA_DIR", data)
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap()
}

/// Espera la marca del escenario y devuelve su resultado.
fn wait_marker(scenario: &str, data: &Path, child: &mut Child) -> Value {
    let marker = data.join(format!("{scenario}.listo"));
    let started = Instant::now();
    loop {
        if let Ok(text) = std::fs::read_to_string(&marker) {
            if let Ok(value) = serde_json::from_str(&text) {
                return value;
            }
        }
        if let Some(status) = child.try_wait().unwrap() {
            panic!("el proceso hijo terminó antes de «{scenario}»: {status}");
        }
        assert!(started.elapsed() < WAIT, "el hijo no llegó a «{scenario}»");
        thread::sleep(Duration::from_millis(25));
    }
}

/// Termina el proceso sin darle opción a cerrar nada (`TerminateProcess` en Windows).
fn kill(mut child: Child) {
    child.kill().unwrap();
    child.wait().unwrap();
}

/// R01 (parte automatizable) y R02: «Salir por completo» detiene la cola y termina el proceso;
/// lo que quedó en cola se conserva y se procesa al volver a abrir, sin repetir el registro.
#[test]
fn salir_por_completo_conserva_la_cola_y_reabrir_la_retoma() {
    let dir = tempfile::tempdir().unwrap();
    let data = data_dir_for(&dir);
    let ai = FakeOllama::start(Mode::Answer);
    select_local(&data, &ai.url);

    let mut child = spawn_child("salir", &data, &ai.url);
    let outcome = wait_marker("salir", &data, &mut child);
    let status = child.wait().unwrap();
    assert!(
        status.success(),
        "la salida termina el proceso con éxito: {status}"
    );
    assert_eq!(
        ai.chats(),
        1,
        "tras salir no se llamó al modelo por la tarea en cola"
    );

    let s = Session::open(test_deps(&data, &ai.url));
    let queued = outcome["queued"].as_i64().unwrap();
    let done = outcome["done"].as_i64().unwrap();
    s.wait_state(queued, "awaiting_review");
    assert_eq!(s.task(done)["state"], "completed");
    assert_eq!(s.registered(), 1, "reabrir no repite el registro");
    assert_eq!(ai.chats(), 2);
    s.close();
}

/// R02 y flujo 3 con cortes reales: se termina el proceso mientras el modelo procesa una
/// factura, otra vez con la factura en revisión y otra justo tras aprobarla. Al reabrir, la
/// tarea interrumpida vuelve a la cola con el intento contado y se procesa una vez; la
/// revisión pendiente se conserva; la aprobada sigue completada y el registro no se repite.
#[test]
fn interrumpir_el_proceso_no_repite_efectos() {
    let dir = tempfile::tempdir().unwrap();
    let data = data_dir_for(&dir);
    let ai = FakeOllama::start(Mode::Hang);
    select_local(&data, &ai.url);

    // Corte 1: el proceso muere con la tarea en `running` (el modelo no ha respondido).
    let mut child = spawn_child("importar", &data, &ai.url);
    let imported = wait_marker("importar", &data, &mut child);
    let started = Instant::now();
    while ai.chats() == 0 {
        assert!(started.elapsed() < WAIT, "el modelo no recibió la petición");
        thread::sleep(Duration::from_millis(25));
    }
    kill(child);

    // Corte 2: al reabrir, la tarea se procesa de nuevo y el proceso muere en revisión.
    ai.set(Mode::Answer);
    let mut child = spawn_child("reabrir", &data, &ai.url);
    let review = wait_marker("reabrir", &data, &mut child);
    kill(child);
    assert_eq!(review["id"], imported["id"]);
    assert_eq!(
        review["attempts"], 2,
        "el intento interrumpido cuenta: {review}"
    );
    assert_eq!(ai.chats(), 2);

    // Corte 3: el proceso muere justo después de aprobar y registrar.
    let mut child = spawn_child("aprobar", &data, &ai.url);
    let approved = wait_marker("aprobar", &data, &mut child);
    kill(child);
    assert_eq!(approved["state"], "completed", "{approved}");

    let s = Session::open(test_deps(&data, &ai.url));
    let tasks = s.tasks();
    assert_eq!(tasks.len(), 1, "{tasks:?}");
    assert_eq!(tasks[0]["state"], "completed");
    assert_eq!(s.registered(), 1, "el registro no se repite al reabrir");
    assert!(s
        .call(
            "approve",
            json!({ "id": id(&tasks[0]), "version": version(&tasks[0]) })
        )
        .is_err());
    let confirmed = s
        .audit()
        .into_iter()
        .filter(|e| e["action"] == "register_invoice" && e["status"] == "confirmed")
        .count();
    assert_eq!(confirmed, 1);
    assert_eq!(ai.chats(), 2, "reabrir no vuelve a llamar al modelo");
    s.close();
}
