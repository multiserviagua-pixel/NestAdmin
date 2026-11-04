//! Casos T08b y T08c de specs/v1/pruebas-instalador.md. Ollama y el servidor de descargas
//! se simulan en 127.0.0.1 y el sistema (buscar, verificar, instalar y arrancar Ollama,
//! probar Tesseract, leer la memoria) es un doble que registra las llamadas (D40, D46). Nada se instala en el
//! equipo ni sale a la red; solo datos sintéticos.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use nestadmin_ai::{AiError, GenerateRequest, GenerateResult, Provider, ProviderKind, Task};
use nestadmin_setup::{
    explain_timeouts, parse_version, Cancel, Check, Config, ErrorCode, Event, Memory, MemoryLevel,
    Missing, Pinned, Setup, SetupError, Startup, Status, Step, StepState, System,
};
use sha2::{Digest, Sha256};

// ---- Servidor HTTP mínimo ----

struct Request {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
}

impl Request {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// Respuesta en bruto; vacía = cerrar la conexión sin responder (servicio caído).
type Handler = dyn Fn(&Request) -> Vec<u8> + Send + Sync;

fn serve(handler: Arc<Handler>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let handler = handler.clone();
            thread::spawn(move || answer(stream, &*handler));
        }
    });
    url
}

fn answer(mut stream: TcpStream, handler: &Handler) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut line = String::new();
    if reader.read_line(&mut line).unwrap_or(0) == 0 {
        return;
    }
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("").to_owned();
    let path = parts.next().unwrap_or("").to_owned();
    let mut headers = Vec::new();
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).unwrap_or(0) == 0 {
            return;
        }
        let header = header.trim_end();
        if header.is_empty() {
            break;
        }
        if let Some((k, v)) = header.split_once(':') {
            headers.push((k.trim().to_owned(), v.trim().to_owned()));
        }
    }
    let length = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = vec![0u8; length];
    let _ = reader.read_exact(&mut body);
    let response = handler(&Request {
        method,
        path,
        headers,
    });
    let _ = stream.write_all(&response);
    let _ = stream.flush();
}

fn response(status: &str, headers: &[(&str, String)], body: &[u8]) -> Vec<u8> {
    let mut out = format!("HTTP/1.1 {status}\r\nConnection: close\r\n");
    for (k, v) in headers {
        out.push_str(&format!("{k}: {v}\r\n"));
    }
    out.push_str("\r\n");
    let mut out = out.into_bytes();
    out.extend_from_slice(body);
    out
}

fn json(status: &str, body: &str) -> Vec<u8> {
    response(
        status,
        &[
            ("Content-Type", "application/json".into()),
            ("Content-Length", body.len().to_string()),
        ],
        body.as_bytes(),
    )
}

// ---- Ollama simulado ----

#[derive(Clone, PartialEq)]
enum Pull {
    Success,
    CutOnce,
    AlwaysCut,
    Error(&'static str),
}

struct Ollama {
    running: bool,
    version: String,
    has_model: bool,
    pull: Pull,
    pulls: u32,
    answer: &'static str,
    /// Tiempo que tarda el modelo en responder (cargarlo con poca memoria es lento).
    chat_delay: Duration,
    requests: u32,
}

type SharedOllama = Arc<Mutex<Ollama>>;

fn fake_ollama(state: SharedOllama) -> String {
    serve(Arc::new(move |request: &Request| {
        let mut s = state.lock().unwrap();
        s.requests += 1;
        if !s.running {
            return Vec::new();
        }
        match (request.method.as_str(), request.path.as_str()) {
            ("GET", "/api/version") => json("200 OK", &format!("{{\"version\":\"{}\"}}", s.version)),
            ("GET", "/api/tags") if s.has_model => json(
                "200 OK",
                "{\"models\":[{\"name\":\"qwen3:8b\",\"model\":\"qwen3:8b\",\"digest\":\"500a1f067a9f\"}]}",
            ),
            ("GET", "/api/tags") => json("200 OK", "{\"models\":[]}"),
            ("POST", "/api/pull") => {
                s.pulls += 1;
                let progress = "{\"status\":\"pulling manifest\"}\n{\"status\":\"pulling a1\",\"total\":5200,\"completed\":2600}\n";
                let cut = match s.pull.clone() {
                    Pull::Error(message) => {
                        return json("200 OK", &format!("{progress}{{\"error\":\"{message}\"}}\n"))
                    }
                    Pull::AlwaysCut => true,
                    Pull::CutOnce => s.pulls == 1,
                    Pull::Success => false,
                };
                if cut {
                    return json("200 OK", progress);
                }
                s.has_model = true;
                json(
                    "200 OK",
                    &format!("{progress}{{\"status\":\"pulling a1\",\"total\":5200,\"completed\":5200}}\n{{\"status\":\"success\"}}\n"),
                )
            }
            ("POST", "/api/chat") => {
                let delay = s.chat_delay;
                if !delay.is_zero() {
                    drop(s);
                    thread::sleep(delay);
                    s = state.lock().unwrap();
                }
                let content = serde_json::to_string(&serde_json::json!({ "respuesta": s.answer })).unwrap();
                let body = serde_json::json!({ "model": "qwen3:8b", "message": { "role": "assistant", "content": content } });
                json("200 OK", &body.to_string())
            }
            _ => json("404 Not Found", "{}"),
        }
    }))
}

// ---- Servidor de descargas ----

struct Downloads {
    cut_first: bool,
    ranges: Vec<Option<String>>,
}

fn installer_bytes() -> Vec<u8> {
    (0..2 * 1024 * 1024u32).map(|i| (i % 251) as u8).collect()
}

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn fake_downloads(state: Arc<Mutex<Downloads>>) -> String {
    let data = Arc::new(installer_bytes());
    serve(Arc::new(move |request: &Request| {
        let mut s = state.lock().unwrap();
        let range = request.header("range").map(str::to_owned);
        s.ranges.push(range.clone());
        match range {
            Some(range) => {
                let start: usize = range
                    .trim_start_matches("bytes=")
                    .trim_end_matches('-')
                    .parse()
                    .unwrap();
                let rest = &data[start..];
                response(
                    "206 Partial Content",
                    &[("Content-Length", rest.len().to_string())],
                    rest,
                )
            }
            None if s.cut_first => {
                s.cut_first = false;
                response(
                    "200 OK",
                    &[("Content-Length", data.len().to_string())],
                    &data[..data.len() / 2],
                )
            }
            None => response(
                "200 OK",
                &[("Content-Length", data.len().to_string())],
                &data,
            ),
        }
    }))
}

// ---- Doble del sistema ----

struct SystemState {
    exe: bool,
    signature_ok: bool,
    tesseract_ok: bool,
    /// Versión que deja instalada el instalador de Ollama.
    installs: &'static str,
    start_delay: Duration,
    /// Memoria `(total, libre)` que informa el equipo.
    memory: Option<(u64, u64)>,
    calls: Vec<String>,
}

#[derive(Clone)]
struct Double {
    state: Arc<Mutex<SystemState>>,
    ollama: SharedOllama,
}

impl System for Double {
    fn ollama_executable(&self) -> Option<PathBuf> {
        self.state
            .lock()
            .unwrap()
            .exe
            .then(|| PathBuf::from("ollama.exe"))
    }

    fn verify_signature(&self, file: &Path, publisher: &str) -> Result<(), String> {
        assert!(file.is_file(), "se verifica el archivo descargado");
        assert_eq!(publisher, "Ollama Inc.");
        let mut s = self.state.lock().unwrap();
        s.calls.push("verify_signature".into());
        if s.signature_ok {
            Ok(())
        } else {
            Err("firma «NotSigned»".into())
        }
    }

    fn install_ollama(&self, _installer: &Path) -> Result<(), String> {
        let mut s = self.state.lock().unwrap();
        s.calls.push("install".into());
        s.exe = true;
        // El instalador sustituye la versión y deja el servicio detenido.
        let mut o = self.ollama.lock().unwrap();
        o.version = s.installs.into();
        o.running = false;
        Ok(())
    }

    fn start_ollama(&self, _executable: &Path) -> Result<(), String> {
        let delay = {
            let mut s = self.state.lock().unwrap();
            s.calls.push("start".into());
            s.start_delay
        };
        let ollama = self.ollama.clone();
        thread::spawn(move || {
            thread::sleep(delay);
            ollama.lock().unwrap().running = true;
        });
        Ok(())
    }

    fn tesseract_version(&self, _exe: &Path, _tessdata: &Path) -> Result<String, String> {
        if self.state.lock().unwrap().tesseract_ok {
            Ok("5.4.0.20240606".into())
        } else {
            Err("Tesseract no encuentra los datos de idioma spa y eng".into())
        }
    }

    fn memory(&self) -> Option<(u64, u64)> {
        self.state.lock().unwrap().memory
    }
}

const GIB: u64 = 1 << 30;

/// `n` GiB (admite fracciones, como los 15,6 GiB de un equipo de 16 GB).
fn gib(n: f64) -> u64 {
    (n * GIB as f64) as u64
}

// ---- Escenario ----

struct Scenario {
    setup: Setup,
    ollama: SharedOllama,
    downloads: Arc<Mutex<Downloads>>,
    system: Arc<Mutex<SystemState>>,
    dir: tempfile::TempDir,
}

/// Equipo con el OCR del paquete; Ollama según los parámetros.
fn scenario(running: bool, exe: bool, version: &str, has_model: bool) -> Scenario {
    let dir = tempfile::tempdir().unwrap();
    for file in [
        "pdfium/pdfium.dll",
        "tesseract/tesseract.exe",
        "tessdata/spa.traineddata",
        "tessdata/eng.traineddata",
    ] {
        let path = dir.path().join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"sintetico").unwrap();
    }
    let ollama = Arc::new(Mutex::new(Ollama {
        running,
        version: version.into(),
        has_model,
        pull: Pull::Success,
        pulls: 0,
        answer: "listo",
        chat_delay: Duration::ZERO,
        requests: 0,
    }));
    let downloads = Arc::new(Mutex::new(Downloads {
        cut_first: false,
        ranges: Vec::new(),
    }));
    let system = Arc::new(Mutex::new(SystemState {
        exe,
        signature_ok: true,
        tesseract_ok: true,
        installs: "0.40.14",
        start_delay: Duration::from_millis(50),
        memory: Some((gib(16.0), gib(10.0))),
        calls: Vec::new(),
    }));
    let mut config = Config::windows(
        dir.path().join("tesseract/tesseract.exe"),
        dir.path().join("tessdata"),
        dir.path().join("pdfium"),
        dir.path().join("descargas"),
    );
    config.ollama_url = fake_ollama(ollama.clone());
    config.ollama_installer = Pinned {
        url: format!("{}/OllamaSetup.exe", fake_downloads(downloads.clone())),
        file_name: "OllamaSetup-0.40.14.exe".into(),
        sha256: sha256(&installer_bytes()),
    };
    config.retry_delay = Duration::from_millis(10);
    config.start_timeout = Duration::from_secs(3);
    config.verify_timeout = Duration::from_secs(5);
    let double = Double {
        state: system.clone(),
        ollama: ollama.clone(),
    };
    Scenario {
        setup: Setup::new(config, Box::new(double)),
        ollama,
        downloads,
        system,
        dir,
    }
}

impl Scenario {
    fn prepare(&self) -> (Result<nestadmin_setup::Report, SetupError>, Vec<Event>) {
        let mut events = Vec::new();
        let result = self
            .setup
            .prepare(&mut |e| events.push(e), &Cancel::default());
        (result, events)
    }

    fn calls(&self) -> Vec<String> {
        self.system.lock().unwrap().calls.clone()
    }

    fn installer_left(&self) -> bool {
        std::fs::read_dir(self.dir.path().join("descargas"))
            .map(|entries| entries.count() > 0)
            .unwrap_or(false)
    }
}

/// Estado final de cada paso según los eventos.
fn final_states(events: &[Event]) -> Vec<(Step, StepState)> {
    Step::ALL
        .iter()
        .filter_map(|step| {
            events
                .iter()
                .rev()
                .find(|e| e.step == *step && e.state != StepState::Running)
                .map(|e| (*step, e.state))
        })
        .collect()
}

use Step::{Model, Ocr, Ollama as OllamaStep, Service, Verify};
use StepState::{Done, Failed, Skipped};

#[test]
fn t08b_01_clean_machine_installs_everything_and_ends_ready() {
    let s = scenario(false, false, "", false);
    assert_eq!(s.setup.check(), Check::Missing(Missing::Ollama));
    let (result, events) = s.prepare();
    let report = result.unwrap();
    assert_eq!(
        final_states(&events),
        [
            (Ocr, Done),
            (OllamaStep, Done),
            (Service, Done),
            (Model, Done),
            (Verify, Done)
        ]
    );
    assert!(events
        .iter()
        .any(|e| e.step == OllamaStep && e.done.is_some() && e.total == Some(2 * 1024 * 1024)));
    assert!(events
        .iter()
        .any(|e| e.step == Model && e.done == Some(5200) && e.total == Some(5200)));
    assert_eq!(s.calls(), ["verify_signature", "install", "start"]);
    assert!(!s.installer_left(), "el instalador descargado se borra");
    assert_eq!(
        (
            report.ollama_version.as_str(),
            report.ollama_installed,
            report.answer.as_str()
        ),
        ("0.40.14", true, "listo")
    );
    assert_eq!(report.model_digest.as_deref(), Some("500a1f067a9f"));
    assert_eq!(s.setup.check(), Check::Ready);
}

#[test]
fn t08b_02_everything_present_is_reused_and_only_verified() {
    let s = scenario(true, true, "0.40.14", true);
    assert_eq!(s.setup.check(), Check::Ready);
    let (result, events) = s.prepare();
    assert!(!result.unwrap().ollama_installed);
    assert_eq!(
        final_states(&events),
        [
            (Ocr, Done),
            (OllamaStep, Skipped),
            (Service, Skipped),
            (Model, Skipped),
            (Verify, Done)
        ]
    );
    assert!(
        s.calls().is_empty(),
        "ni instala ni arranca: {:?}",
        s.calls()
    );
    assert_eq!(s.ollama.lock().unwrap().pulls, 0);
    assert!(s.downloads.lock().unwrap().ranges.is_empty());
}

#[test]
fn t08b_03_stopped_ollama_is_only_started() {
    let s = scenario(false, true, "0.40.14", true);
    assert_eq!(s.setup.check(), Check::Stopped);
    let (result, events) = s.prepare();
    result.unwrap();
    assert_eq!(s.calls(), ["start"]);
    assert_eq!(
        final_states(&events)[1..3],
        [(OllamaStep, Skipped), (Service, Done)]
    );
}

#[test]
fn t08b_04_outdated_running_ollama_is_updated() {
    let s = scenario(true, true, "0.5.7", true);
    assert_eq!(s.setup.check(), Check::Missing(Missing::OllamaOutdated));
    let (result, _) = s.prepare();
    let report = result.unwrap();
    assert_eq!(
        (report.ollama_version.as_str(), report.ollama_installed),
        ("0.40.14", true)
    );
    assert_eq!(s.calls(), ["verify_signature", "install", "start"]);
}

#[test]
fn t08b_05_outdated_stopped_ollama_is_found_on_start_then_updated() {
    let s = scenario(false, true, "0.5.7", true);
    assert_eq!(s.setup.check(), Check::Stopped);
    let (result, events) = s.prepare();
    assert_eq!(result.unwrap().ollama_version, "0.40.14");
    assert_eq!(s.calls(), ["start", "verify_signature", "install", "start"]);
    assert_eq!(
        final_states(&events)[1..3],
        [(OllamaStep, Done), (Service, Done)]
    );
}

#[test]
fn t08b_06_cut_downloads_resume_and_retry() {
    let s = scenario(false, false, "", false);
    s.downloads.lock().unwrap().cut_first = true;
    s.ollama.lock().unwrap().pull = Pull::CutOnce;
    let (result, events) = s.prepare();
    result.unwrap();
    let ranges = s.downloads.lock().unwrap().ranges.clone();
    assert_eq!(ranges.len(), 2);
    assert_eq!(
        ranges[1].as_deref(),
        Some(format!("bytes={}-", 1024 * 1024).as_str())
    );
    for step in [OllamaStep, Model] {
        assert!(
            events
                .iter()
                .any(|e| e.step == step && e.message.starts_with("Reintento 2 de 3")),
            "{step:?}"
        );
    }
    assert_eq!(s.ollama.lock().unwrap().pulls, 2);
}

#[test]
fn t08b_07_model_network_failures_stop_after_three_attempts_and_resume_later() {
    let s = scenario(false, false, "", false);
    s.ollama.lock().unwrap().pull = Pull::AlwaysCut;
    let (result, events) = s.prepare();
    assert_eq!(result.unwrap_err().code, ErrorCode::Network);
    let last = events.last().unwrap();
    assert_eq!((last.step, last.state), (Model, Failed));
    assert_eq!(s.ollama.lock().unwrap().pulls, 3);

    s.ollama.lock().unwrap().pull = Pull::Success;
    let (result, events) = s.prepare();
    result.unwrap();
    assert_eq!(
        s.calls(),
        ["verify_signature", "install", "start"],
        "no repite la instalación"
    );
    assert_eq!(
        final_states(&events)[1..4],
        [(OllamaStep, Skipped), (Service, Skipped), (Model, Done)]
    );
}

#[test]
fn t08b_08_installer_without_valid_signature_is_not_run_and_is_deleted() {
    let s = scenario(false, false, "", false);
    s.system.lock().unwrap().signature_ok = false;
    let (result, events) = s.prepare();
    assert_eq!(result.unwrap_err().code, ErrorCode::Signature);
    assert_eq!(s.calls(), ["verify_signature"]);
    assert!(!s.installer_left());
    assert_eq!(final_states(&events).last(), Some(&(OllamaStep, Failed)));
}

#[test]
fn t08b_08b_installer_with_a_different_hash_is_rejected_before_the_signature() {
    let mut s = scenario(false, false, "", false);
    let mut config = s.setup.config().clone();
    config.ollama_installer.sha256 = sha256(b"otro");
    let double = Double {
        state: s.system.clone(),
        ollama: s.ollama.clone(),
    };
    s.setup = Setup::new(config, Box::new(double));
    let (result, _) = s.prepare();
    assert_eq!(result.unwrap_err().code, ErrorCode::Signature);
    assert!(s.calls().is_empty());
    assert!(!s.installer_left());
}

#[test]
fn t08b_09_model_rejected_by_ollama_is_not_retried() {
    let s = scenario(true, true, "0.40.14", false);
    s.ollama.lock().unwrap().pull = Pull::Error("no space left on device");
    let (result, _) = s.prepare();
    let error = result.unwrap_err();
    assert_eq!(error.code, ErrorCode::ModelFailed);
    assert!(error.message.contains("no space left on device"));
    assert_eq!(s.ollama.lock().unwrap().pulls, 1);
}

#[test]
fn t08b_10_empty_answer_fails_verification() {
    let s = scenario(true, true, "0.40.14", true);
    s.ollama.lock().unwrap().answer = "  ";
    let (result, events) = s.prepare();
    assert_eq!(result.unwrap_err().code, ErrorCode::VerifyFailed);
    assert_eq!(final_states(&events).last(), Some(&(Verify, Failed)));
}

#[test]
fn t08b_11_cancelling_during_a_download_installs_nothing() {
    let s = scenario(false, false, "", false);
    let cancel = Cancel::default();
    let mut events = Vec::new();
    let result = s.setup.prepare(
        &mut |e| {
            if e.step == OllamaStep && e.done.is_some() {
                cancel.cancel();
            }
            events.push(e);
        },
        &cancel,
    );
    assert_eq!(result.unwrap_err().code, ErrorCode::Cancelled);
    assert!(s.calls().is_empty());
    assert_eq!(
        events.last().map(|e| (e.step, e.state)),
        Some((OllamaStep, Failed))
    );
}

#[test]
fn t08b_12_missing_or_broken_ocr_fails_without_downloading() {
    let s = scenario(false, false, "", false);
    std::fs::remove_file(s.dir.path().join("tessdata/eng.traineddata")).unwrap();
    assert_eq!(s.setup.check(), Check::Missing(Missing::Ocr));
    let (result, events) = s.prepare();
    assert_eq!(result.unwrap_err().code, ErrorCode::OcrMissing);
    assert_eq!(final_states(&events), [(Ocr, Failed)]);

    let s = scenario(true, true, "0.40.14", true);
    s.system.lock().unwrap().tesseract_ok = false;
    let (result, _) = s.prepare();
    assert_eq!(result.unwrap_err().code, ErrorCode::OcrMissing);
    assert!(s.downloads.lock().unwrap().ranges.is_empty());
    assert_eq!(s.ollama.lock().unwrap().pulls, 0);
}

#[test]
fn t08b_13_states_events_and_errors_serialize_to_the_contract() {
    let to = |v: &dyn erased::Ser| v.json();
    assert_eq!(to(&Check::Ready), r#"{"status":"ready"}"#);
    assert_eq!(to(&Check::Stopped), r#"{"status":"stopped"}"#);
    assert_eq!(
        to(&Check::Missing(Missing::OllamaOutdated)),
        r#"{"status":"missing","component":"ollama_outdated"}"#
    );
    assert_eq!(
        to(&SetupError::new(ErrorCode::ModelFailed, "x")),
        r#"{"code":"model_failed","message":"x"}"#
    );
    let event = Event {
        step: Service,
        state: Skipped,
        done: None,
        total: None,
        message: "m".into(),
    };
    assert_eq!(
        to(&event),
        r#"{"step":"service","state":"skipped","done":null,"total":null,"message":"m"}"#
    );
    let status = Status {
        check: Check::Missing(Missing::Model),
        memory: Memory::assess(None),
    };
    assert_eq!(
        to(&status),
        r#"{"status":"missing","component":"model","memory":{"total_bytes":null,"available_bytes":null,"level":"unknown","warning":null}}"#
    );
    for code in [
        ErrorCode::Network,
        ErrorCode::Signature,
        ErrorCode::InstallFailed,
        ErrorCode::Unavailable,
        ErrorCode::ModelFailed,
        ErrorCode::VerifyFailed,
        ErrorCode::OcrMissing,
        ErrorCode::Cancelled,
    ] {
        assert_eq!(to(&code), format!("\"{}\"", code.as_str()));
    }

    // Los mensajes de error no llevan rutas del equipo.
    let s = scenario(false, false, "", false);
    s.system.lock().unwrap().signature_ok = false;
    let error = s.prepare().0.unwrap_err();
    let text = to(&error);
    assert!(!text.contains(&*s.dir.path().to_string_lossy()), "{text}");
    if let Some(home) = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")) {
        assert!(!text.contains(&*home.to_string_lossy()), "{text}");
    }
}

/// Serialización sin depender del tipo concreto.
mod erased {
    pub trait Ser {
        fn json(&self) -> String;
    }
    impl<T: serde::Serialize> Ser for T {
        fn json(&self) -> String {
            serde_json::to_string(self).unwrap()
        }
    }
}

#[test]
fn t08b_14_memory_is_classified_against_16_and_8_gb() {
    let level = |total: f64| Memory::assess(Some((gib(total), gib(10.0)))).level;
    assert_eq!(level(16.0), MemoryLevel::Recommended);
    // Lo que informa Windows de un equipo de 16 GB con gráficos integrados.
    assert_eq!(level(15.6), MemoryLevel::Recommended);
    assert_eq!(level(32.0), MemoryLevel::Recommended);
    assert_eq!(level(12.0), MemoryLevel::BelowRecommended);
    assert_eq!(level(9.0), MemoryLevel::BelowRecommended);
    assert_eq!(level(8.0), MemoryLevel::Insufficient);
    assert_eq!(level(7.7), MemoryLevel::Insufficient);
    assert_eq!(level(4.0), MemoryLevel::Insufficient);

    let ok = Memory::assess(Some((gib(16.0), gib(10.0))));
    assert_eq!(ok.warning, None);
    assert_eq!(ok.timeout_cause(), None);

    let twelve = Memory::assess(Some((gib(12.0), gib(8.0))));
    let warning = twelve.warning.clone().unwrap();
    assert!(
        warning.contains("12,0 GB") && warning.contains("16 GB"),
        "{warning}"
    );
    assert!(
        warning.contains("Groq") && !warning.contains("probablemente no"),
        "{warning}"
    );

    let eight = Memory::assess(Some((gib(7.7), gib(3.0))));
    let warning = eight.warning.clone().unwrap();
    assert!(warning.contains("7,7 GB"), "{warning}");
    assert!(
        warning.contains("probablemente no responderá a tiempo"),
        "{warning}"
    );
    assert!(warning.contains("3,0 GB libres"), "{warning}");

    // Con 16 GB pero casi todo ocupado solo se sugiere cerrar programas.
    let busy = Memory::assess(Some((gib(16.0), gib(2.0))));
    assert_eq!(busy.level, MemoryLevel::Recommended);
    assert!(busy
        .warning
        .clone()
        .unwrap()
        .contains("cierre otros programas"));
    assert_eq!(busy.timeout_cause(), None);

    let unknown = Memory::assess(None);
    assert_eq!(
        (
            unknown.level,
            unknown.warning.clone(),
            unknown.timeout_cause()
        ),
        (MemoryLevel::Unknown, None, None)
    );
}

#[test]
fn t08b_15_low_memory_warns_without_blocking_or_switching_provider() {
    for (total, level) in [
        (16.0, MemoryLevel::Recommended),
        (12.0, MemoryLevel::BelowRecommended),
        (8.0, MemoryLevel::Insufficient),
    ] {
        let s = scenario(true, true, "0.40.14", true);
        s.system.lock().unwrap().memory = Some((gib(total), gib(6.0)));
        let status = s.setup.status();
        assert_eq!(status.check, Check::Ready);
        assert_eq!(status.memory.level, level);
        let (result, events) = s.prepare();
        let report = result.unwrap_or_else(|e| panic!("{total} GB: {e}"));
        assert_eq!(report.memory.level, level);
        assert_eq!(final_states(&events).last(), Some(&(Verify, Done)));
        assert!(s.calls().is_empty(), "no se instala ni se arranca nada más");
    }
}

#[test]
fn t08b_16_verification_timeout_names_memory_as_the_likely_cause() {
    for (total, mentions) in [(8.0, true), (12.0, true), (16.0, false)] {
        let mut s = scenario(true, true, "0.40.14", true);
        s.system.lock().unwrap().memory = Some((gib(total), gib(4.0)));
        s.ollama.lock().unwrap().chat_delay = Duration::from_millis(1500);
        let mut config = s.setup.config().clone();
        config.verify_timeout = Duration::from_millis(300);
        let double = Double {
            state: s.system.clone(),
            ollama: s.ollama.clone(),
        };
        s.setup = Setup::new(config, Box::new(double));
        let (result, events) = s.prepare();
        let error = result.unwrap_err();
        assert_eq!(error.code, ErrorCode::VerifyFailed, "{total} GB");
        assert!(error.message.starts_with("timeout:"), "{}", error.message);
        assert_eq!(
            error.message.contains("falta de memoria"),
            mentions,
            "{total} GB: {}",
            error.message
        );
        assert_eq!(final_states(&events).last(), Some(&(Verify, Failed)));
    }
}

/// Proveedor que siempre agota el tiempo.
struct Slow(ProviderKind);

impl Provider for Slow {
    fn kind(&self) -> ProviderKind {
        self.0
    }
    fn model(&self) -> &str {
        "sintetico"
    }
    fn generate(&self, _request: &GenerateRequest) -> Result<GenerateResult, AiError> {
        Err(AiError::Timeout)
    }
}

#[test]
fn t08b_17_task_timeouts_on_low_memory_keep_the_code_and_explain_the_cause() {
    let request = GenerateRequest {
        request_id: "sintetico".into(),
        task: Task::Classification,
        instructions: "x".into(),
        content: "x".into(),
        schema: None,
    };
    let eight = Memory::assess(Some((gib(8.0), gib(3.0))));
    let error = explain_timeouts(Arc::new(Slow(ProviderKind::Ollama)), &eight)
        .generate(&request)
        .unwrap_err();
    assert_eq!(error.code(), "timeout");
    let text = error.to_string();
    assert!(text.contains("no respondió a tiempo"), "{text}");
    assert!(
        text.contains("falta de memoria") && text.contains("8,0 GB"),
        "{text}"
    );

    // Con 16 GB, o con Groq elegido, el error no cambia.
    let sixteen = Memory::assess(Some((gib(16.0), gib(10.0))));
    let error = explain_timeouts(Arc::new(Slow(ProviderKind::Ollama)), &sixteen)
        .generate(&request)
        .unwrap_err();
    assert_eq!(error, AiError::Timeout);
    let groq = explain_timeouts(Arc::new(Slow(ProviderKind::Groq)), &eight);
    assert_eq!(groq.kind(), ProviderKind::Groq);
    assert_eq!(groq.generate(&request).unwrap_err(), AiError::Timeout);
}

#[test]
fn t08c_01_stopped_ollama_is_started_once_at_startup() {
    let s = scenario(false, true, "0.40.14", true);
    assert_eq!(s.setup.startup(true), Startup::Started);
    assert_eq!(s.setup.startup(true), Startup::Ready);
    assert_eq!(s.calls(), ["start"]);
}

#[test]
fn t08c_02_missing_model_asks_for_the_wizard_without_downloading() {
    let s = scenario(true, true, "0.40.14", false);
    assert_eq!(
        s.setup.startup(true),
        Startup::NeedsPreparation {
            check: Check::Missing(Missing::Model)
        }
    );
    assert_eq!(s.ollama.lock().unwrap().pulls, 0);
    let s = scenario(false, false, "", false);
    assert_eq!(
        s.setup.startup(true),
        Startup::NeedsPreparation {
            check: Check::Missing(Missing::Ollama)
        }
    );
    assert!(s.calls().is_empty());
}

#[test]
fn t08c_03_groq_selected_checks_nothing_local() {
    let s = scenario(false, true, "0.40.14", true);
    assert_eq!(s.setup.startup(false), Startup::NotLocal);
    assert!(s.calls().is_empty());
    assert_eq!(s.ollama.lock().unwrap().requests, 0);
}

#[test]
fn t08c_04_slow_start_is_awaited_off_the_caller_thread() {
    let s = scenario(false, true, "0.40.14", true);
    s.system.lock().unwrap().start_delay = Duration::from_millis(1500);
    let setup = Arc::new(s.setup);
    let worker = {
        let setup = setup.clone();
        thread::spawn(move || setup.startup(true))
    };
    // Quien llama (la ventana) sigue libre mientras la comprobación espera.
    thread::sleep(Duration::from_millis(200));
    assert!(!worker.is_finished());
    assert_eq!(worker.join().unwrap(), Startup::Started);
}

#[test]
fn versions_are_compared_numerically() {
    assert_eq!(parse_version("0.40.14"), Some((0, 40, 14)));
    assert_eq!(parse_version("v5.4.0.20240606"), Some((5, 4, 0)));
    assert_eq!(parse_version("desconocida"), None);
    assert!(parse_version("0.10.0").unwrap() > parse_version("0.9.6").unwrap());
}

/// Comprobación real en este equipo (no instala nada).
#[test]
#[ignore = "inspecciona el equipo real y pide una respuesta a Ollama"]
fn this_machine_is_ready_and_the_local_model_answers() {
    let vendor = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../vendor");
    let setup = Setup::new(
        Config::windows(
            vendor.join("tesseract/tesseract.exe"),
            vendor.join("tessdata"),
            vendor.join("pdfium/bin"),
            std::env::temp_dir().join("nestadmin-descargas"),
        ),
        Box::new(nestadmin_setup::WindowsSystem),
    );
    println!(
        "status: {}",
        serde_json::to_string(&setup.status()).unwrap()
    );
    let mut events = Vec::new();
    let report = setup.prepare(&mut |e| events.push(e), &Cancel::default());
    for event in &events {
        println!("{}", serde_json::to_string(event).unwrap());
    }
    println!("{}", serde_json::to_string(&report).unwrap());
    assert!(report.is_ok());
}

/// Lectura real de la memoria de este equipo (no cambia nada).
#[test]
#[cfg(windows)]
fn this_machine_memory_is_read() {
    let total = nestadmin_setup::WindowsSystem
        .memory()
        .map(|(total, _)| total);
    assert!(total.is_some_and(|t| t > GIB), "{total:?}");
    let memory = Memory::assess(nestadmin_setup::WindowsSystem.memory());
    assert_ne!(memory.level, MemoryLevel::Unknown);
    println!("memoria: {}", serde_json::to_string(&memory).unwrap());
}
