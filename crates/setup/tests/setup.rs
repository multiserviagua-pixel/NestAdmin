//! Preparación con un servidor HTTP local de prueba: descargas reanudables, integridad,
//! descarga del modelo, verificación y detección de lo que falta. Sin acceso a Internet.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;

use nestadmin_setup::{
    agent, download, parse_version, Component, Config, Pinned, Progress, Setup, SetupError, State,
};
use sha2::{Digest, Sha256};

struct Request {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: String,
}

impl Request {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

type Handler = dyn Fn(&Request) -> Vec<u8> + Send + Sync;

/// Servidor de un solo hilo: atiende cada conexión con `handler`, que devuelve la
/// respuesta en bruto (puede ser incompleta para simular un corte), y la cierra.
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
        reader.read_line(&mut header).unwrap();
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
    reader.read_exact(&mut body).unwrap();
    let request = Request {
        method,
        path,
        headers,
        body: String::from_utf8_lossy(&body).into_owned(),
    };
    let _ = stream.write_all(&handler(&request));
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

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Contenido sintético de 3 MB.
fn payload() -> Vec<u8> {
    (0..3 * 1024 * 1024u32).map(|i| (i % 251) as u8).collect()
}

fn pinned(url: &str, bytes: &[u8]) -> Pinned {
    Pinned {
        url: format!("{url}/instalador.exe"),
        file_name: "instalador.exe".into(),
        sha256: sha256(bytes),
    }
}

fn config(url: &str, dir: &std::path::Path) -> Config {
    let mut config = Config::windows(
        dir.join("pdfium"),
        dir.join("tessdata"),
        dir.join("tesseract.exe"),
        dir.join("descargas"),
    );
    config.ollama_url = url.to_owned();
    config.ollama_exe = dir.join("ollama.exe");
    config
}

#[test]
fn an_interrupted_download_resumes_and_a_verified_file_is_reused() {
    let data = Arc::new(payload());
    let first_cut = Arc::new(Mutex::new(true));
    let ranges = Arc::new(Mutex::new(Vec::<Option<String>>::new()));
    let url = {
        let (data, first_cut, ranges) = (data.clone(), first_cut.clone(), ranges.clone());
        serve(Arc::new(move |request: &Request| {
            let range = request.header("range").map(str::to_owned);
            ranges.lock().unwrap().push(range.clone());
            match range {
                None => {
                    let mut cut = first_cut.lock().unwrap();
                    let length = ("Content-Length", data.len().to_string());
                    if *cut {
                        // Primer intento: se corta a mitad de la descarga.
                        *cut = false;
                        response("200 OK", &[length], &data[..data.len() / 2])
                    } else {
                        response("200 OK", &[length], &data)
                    }
                }
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
            }
        }))
    };
    let dir = tempfile::tempdir().unwrap();
    let pinned = pinned(&url, &data);
    let agent = agent();
    let mut events: Vec<Progress> = Vec::new();

    let error = download(&agent, &pinned, dir.path(), "ollama", &mut |p| {
        events.push(p)
    })
    .unwrap_err();
    assert!(
        matches!(error, SetupError::Network(ref m) if m.contains("reanudará")),
        "{error:?}"
    );
    let partial = dir.path().join("instalador.exe.part");
    let kept = std::fs::metadata(&partial).unwrap().len();
    assert!(kept > 0 && kept < data.len() as u64);

    let path = download(&agent, &pinned, dir.path(), "ollama", &mut |p| {
        events.push(p)
    })
    .unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), *data);
    assert!(!partial.exists());
    assert_eq!(
        ranges.lock().unwrap()[1].as_deref(),
        Some(format!("bytes={kept}-").as_str()),
        "el reintento pide solo lo que falta"
    );
    let last = events.iter().rev().find(|p| p.done.is_some()).unwrap();
    assert_eq!(
        (last.done, last.total),
        (Some(data.len() as u64), Some(data.len() as u64))
    );

    // Un archivo completo y comprobado no se vuelve a descargar.
    let requests = ranges.lock().unwrap().len();
    download(&agent, &pinned, dir.path(), "ollama", &mut |_| {}).unwrap();
    assert_eq!(ranges.lock().unwrap().len(), requests);
}

#[test]
fn a_file_with_a_different_hash_is_discarded() {
    let data = Arc::new(payload());
    let url = {
        let data = data.clone();
        serve(Arc::new(move |_: &Request| {
            response(
                "200 OK",
                &[("Content-Length", data.len().to_string())],
                &data,
            )
        }))
    };
    let dir = tempfile::tempdir().unwrap();
    let mut pinned = pinned(&url, &data);
    pinned.sha256 = sha256(b"otro contenido");
    let error = download(&agent(), &pinned, dir.path(), "tesseract", &mut |_| {}).unwrap_err();
    assert!(matches!(error, SetupError::Integrity(_)), "{error:?}");
    assert!(!dir.path().join("instalador.exe").exists());
    assert!(!dir.path().join("instalador.exe.part").exists());
}

#[test]
fn http_errors_and_unreachable_servers_are_network_failures() {
    let url = serve(Arc::new(|_: &Request| json("404 Not Found", "{}")));
    let dir = tempfile::tempdir().unwrap();
    let error = download(
        &agent(),
        &pinned(&url, b"x"),
        dir.path(),
        "ollama",
        &mut |_| {},
    )
    .unwrap_err();
    assert_eq!(error, SetupError::Network("estado HTTP 404".into()));

    let closed = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap();
    let error = download(
        &agent(),
        &pinned(&format!("http://{closed}"), b"x"),
        dir.path(),
        "ollama",
        &mut |_| {},
    )
    .unwrap_err();
    assert!(matches!(error, SetupError::Network(_)), "{error:?}");
}

#[test]
fn the_model_download_reports_progress_and_surfaces_errors() {
    let requests = Arc::new(Mutex::new(Vec::<String>::new()));
    let url = {
        let requests = requests.clone();
        serve(Arc::new(move |request: &Request| {
            if request.path == "/api/tags" {
                return json("200 OK", "{\"models\":[]}");
            }
            assert_eq!(
                (request.method.as_str(), request.path.as_str()),
                ("POST", "/api/pull")
            );
            requests.lock().unwrap().push(request.body.clone());
            let body = concat!(
                "{\"status\":\"pulling manifest\"}\n",
                "{\"status\":\"pulling a1b2\",\"digest\":\"sha256:a1b2\",\"total\":1000,\"completed\":400}\n",
                "{\"status\":\"pulling a1b2\",\"digest\":\"sha256:a1b2\",\"total\":1000,\"completed\":1000}\n",
                "{\"status\":\"verifying sha256 digest\"}\n",
                "{\"status\":\"success\"}\n",
            );
            json("200 OK", body)
        }))
    };
    let dir = tempfile::tempdir().unwrap();
    let setup = Setup::new(config(&url, dir.path()));
    let mut events = Vec::new();
    setup
        .install(Component::Model, &mut |p| events.push(p))
        .unwrap();
    let body: serde_json::Value = serde_json::from_str(&requests.lock().unwrap()[0]).unwrap();
    assert_eq!(body["model"], "qwen3:8b");
    assert!(events
        .iter()
        .any(|p| p.done == Some(400) && p.total == Some(1000)));
    assert_eq!(events.last().unwrap().step, "success");

    let failing = serve(Arc::new(|_: &Request| {
        json(
            "200 OK",
            "{\"status\":\"pulling manifest\"}\n{\"error\":\"no space left on device\"}\n",
        )
    }));
    let setup = Setup::new(config(&failing, dir.path()));
    let error = setup.install(Component::Model, &mut |_| {}).unwrap_err();
    assert_eq!(
        error,
        SetupError::Network("Ollama: no space left on device".into())
    );

    let cut = serve(Arc::new(|_: &Request| {
        json("200 OK", "{\"status\":\"pulling manifest\"}\n")
    }));
    let setup = Setup::new(config(&cut, dir.path()));
    let error = setup.install(Component::Model, &mut |_| {}).unwrap_err();
    assert!(
        matches!(error, SetupError::Network(ref m) if m.contains("reanudará")),
        "{error:?}"
    );
}

#[test]
fn the_model_check_needs_a_real_non_empty_answer() {
    let url = serve(Arc::new(|request: &Request| {
        let body: serde_json::Value = serde_json::from_str(&request.body).unwrap();
        assert_eq!(request.path, "/api/chat");
        assert_eq!(body["think"], false);
        assert_eq!(body["options"]["temperature"], 0);
        json(
            "200 OK",
            "{\"message\":{\"role\":\"assistant\",\"content\":\" listo \"}}",
        )
    }));
    let dir = tempfile::tempdir().unwrap();
    let verification = Setup::new(config(&url, dir.path())).verify_model().unwrap();
    assert_eq!(
        (verification.model.as_str(), verification.answer.as_str()),
        ("qwen3:8b", "listo")
    );

    let empty = serve(Arc::new(|_: &Request| {
        json(
            "200 OK",
            "{\"message\":{\"role\":\"assistant\",\"content\":\"\"}}",
        )
    }));
    let error = Setup::new(config(&empty, dir.path()))
        .verify_model()
        .unwrap_err();
    assert!(matches!(error, SetupError::Model(_)), "{error:?}");

    let missing = serve(Arc::new(|_: &Request| {
        json(
            "404 Not Found",
            "{\"error\":\"model 'qwen3:8b' not found\"}",
        )
    }));
    let error = Setup::new(config(&missing, dir.path()))
        .verify_model()
        .unwrap_err();
    assert_eq!(
        error,
        SetupError::Model("model 'qwen3:8b' not found".into())
    );
}

#[test]
fn detection_reports_what_is_missing_and_what_is_present() {
    let dir = tempfile::tempdir().unwrap();
    let closed = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap();
    let setup = Setup::new(config(&format!("http://{closed}"), dir.path()));
    for component in [
        Component::Pdfium,
        Component::Tessdata,
        Component::Tesseract,
        Component::Ollama,
        Component::Model,
    ] {
        assert_eq!(
            setup.detect_one(component).state,
            State::Missing,
            "{component:?}"
        );
    }
    assert!(!setup.detect_one(Component::Pdfium).installable);
    assert!(setup.detect_one(Component::Ollama).installable);
    let error = setup.install(Component::Pdfium, &mut |_| {}).unwrap_err();
    assert!(matches!(error, SetupError::NotInstallable(_)));

    // Ollama instalado pero parado.
    std::fs::write(dir.path().join("ollama.exe"), b"").unwrap();
    assert_eq!(
        setup.detect_one(Component::Ollama).state,
        State::Stopped { version: None }
    );

    std::fs::create_dir_all(dir.path().join("pdfium")).unwrap();
    std::fs::write(dir.path().join("pdfium/pdfium.dll"), b"").unwrap();
    std::fs::create_dir_all(dir.path().join("tessdata")).unwrap();
    std::fs::write(dir.path().join("tessdata/spa.traineddata"), b"").unwrap();
    assert!(setup.detect_one(Component::Pdfium).ready());
    let tessdata = setup.detect_one(Component::Tessdata);
    assert_eq!(tessdata.state, State::Missing);
    assert!(tessdata.detail.contains("eng"));
    std::fs::write(dir.path().join("tessdata/eng.traineddata"), b"").unwrap();
    assert!(setup.detect_one(Component::Tessdata).ready());

    let url = serve(Arc::new(|request: &Request| {
        match request.path.as_str() {
        "/api/version" => json("200 OK", "{\"version\":\"0.40.14\"}"),
        "/api/tags" => json(
            "200 OK",
            "{\"models\":[{\"name\":\"qwen3:8b\",\"model\":\"qwen3:8b\",\"digest\":\"500a1f067a9f782620b40bee6f7b0c89e17ae61f686b92c24933e4ca4b2b8b41\"}]}",
        ),
        _ => json("404 Not Found", "{}"),
    }
    }));
    let setup = Setup::new(config(&url, dir.path()));
    assert_eq!(
        setup.detect_one(Component::Ollama).state,
        State::Present {
            version: Some("0.40.14".into())
        }
    );
    assert_eq!(
        setup.detect_one(Component::Model).state,
        State::Present {
            version: Some("500a1f067a9f".into())
        }
    );
    // Lo presente no se reinstala ni se vuelve a descargar.
    setup
        .install(Component::Ollama, &mut |_| panic!("no debe descargar"))
        .unwrap();
    setup
        .install(Component::Model, &mut |_| panic!("no debe descargar"))
        .unwrap();

    let old = serve(Arc::new(|_: &Request| {
        json("200 OK", "{\"version\":\"0.5.7\"}")
    }));
    let setup = Setup::new(config(&old, dir.path()));
    assert!(matches!(
        setup.detect_one(Component::Ollama).state,
        State::Incompatible { ref found, .. } if found == "0.5.7"
    ));
}

#[test]
fn versions_are_compared_numerically() {
    assert_eq!(parse_version("0.40.14"), Some((0, 40, 14)));
    assert_eq!(parse_version("v5.4.0.20240606"), Some((5, 4, 0)));
    assert_eq!(parse_version("5.3"), Some((5, 3, 0)));
    assert_eq!(parse_version("desconocida"), None);
    assert!(parse_version("0.10.0").unwrap() > parse_version("0.9.6").unwrap());
}

fn this_machine() -> Setup {
    let vendor = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../vendor");
    Setup::new(Config::windows(
        vendor.join("pdfium/bin"),
        vendor.join("tessdata"),
        PathBuf::from(r"C:\Program Files\Tesseract-OCR\tesseract.exe"),
        std::env::temp_dir().join("nestadmin-descargas"),
    ))
}

/// Detección real en este equipo (no modifica nada).
#[test]
#[ignore = "inspecciona el equipo real"]
fn detects_this_machine() {
    for status in this_machine().detect() {
        println!("{}", serde_json::to_string(&status).unwrap());
    }
}

/// Respuesta real del Ollama local con `qwen3:8b`.
#[test]
#[ignore = "necesita Ollama con qwen3:8b"]
fn the_local_model_answers() {
    let verification = this_machine().verify_model().unwrap();
    println!("{}", serde_json::to_string(&verification).unwrap());
    assert!(!verification.answer.is_empty());
}
