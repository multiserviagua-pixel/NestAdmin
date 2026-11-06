//! T09 (R09): las credenciales solo viven en el almacén protegido. Con credenciales
//! sintéticas reconocibles (todas contienen `MARKER`) se recorren los flujos principales —
//! guardar la clave de Groq, procesar tareas con Groq simulado, iniciar sesión en Gmail
//! simulado, crear y enviar un borrador— y los errores de red, 401, clave revocada, ausente
//! o inválida. Después se inspeccionan la base de datos, la cola, los registros de correo y
//! demás archivos del directorio de datos, junto con todos los textos que la aplicación
//! mostraría o escribiría en un registro: ninguno contiene la marca.
//!
//! Todo es local: Groq, Gmail y el servicio de tokens se simulan en 127.0.0.1 y el almacén
//! es un doble en memoria (el almacén real de Windows se prueba en `crates/ai`).

use std::collections::BTreeSet;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use nestadmin_ai::credentials::GROQ_API_KEY;
use nestadmin_ai::groq::GroqProvider;
use nestadmin_ai::{
    build_provider, AiError, CredentialError, CredentialStore, MemoryCredentialStore, Provider,
    ProviderChoice, ProviderKind, Secret,
};
use nestadmin_app::{App, AppPaths, INVOICE};
use nestadmin_core::TaskState;
use nestadmin_extract::{ExtractError, Extractor, Ocr};
use nestadmin_mail::{Gmail, Mailbox, OAuthConfig, PendingAuthorization, TokenSource, Tokens};
use nestadmin_simulators::{MailConnector, NewDraft};
use serde_json::{json, Value};

/// Marca común a todas las credenciales de prueba: buscarla detecta también fragmentos.
const MARKER: &str = "PRUEBA_SINTETICA_T09";
const GROQ_KEY: &str = "gsk_PRUEBA_SINTETICA_T09_clave_groq_0001";
const GROQ_REVOKED: &str = "gsk_PRUEBA_SINTETICA_T09_revocada_0002";
const CLIENT_SECRET: &str = "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";
const AUTH_CODE: &str = "4/PRUEBA_SINTETICA_T09_codigo";
const ACCOUNT: &str = "cuenta-t09";

fn access(n: u32) -> String {
    format!("ya29.PRUEBA_SINTETICA_T09_acceso_{n}")
}

fn refresh(n: u32) -> String {
    format!("1//PRUEBA_SINTETICA_T09_renovacion_{n}")
}

// ---------------------------------------------------------------------------------------
// Almacén doble: guarda en memoria y anota qué nombres se escribieron.

#[derive(Default)]
struct RecordingStore {
    inner: MemoryCredentialStore,
    names: Mutex<BTreeSet<String>>,
}

impl CredentialStore for RecordingStore {
    fn get(&self, name: &str) -> Result<Option<Secret>, CredentialError> {
        self.inner.get(name)
    }
    fn set(&self, name: &str, secret: &Secret) -> Result<(), CredentialError> {
        self.names.lock().unwrap().insert(name.to_owned());
        self.inner.set(name, secret)
    }
    fn delete(&self, name: &str) -> Result<(), CredentialError> {
        self.inner.delete(name)
    }
}

// ---------------------------------------------------------------------------------------
// Servidor simulado de Groq, Gmail y el servicio de tokens.

#[derive(Debug, Clone)]
struct Req {
    method: String,
    path: String,

    headers: Vec<(String, String)>,
    body: String,
}

impl Req {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    fn form(&self, name: &str) -> Option<String> {
        self.body.split('&').find_map(|pair| {
            let (key, value) = pair.split_once('=')?;
            (key == name).then(|| percent_decode(value))
        })
    }
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap();
                out.push(u8::from_str_radix(hex, 16).unwrap());
                i += 2;
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8(out).unwrap()
}

#[derive(Default)]
struct World {
    requests: Vec<Req>,
    /// Única clave de Groq que el servidor acepta.
    groq_key: String,
    /// Token de acceso vigente de Gmail.
    access: String,
    issued: u32,
    refresh_revoked: bool,
    /// Gmail rechaza cualquier token (credencial revocada en el proveedor).
    reject_all: bool,
}

type Shared = Arc<Mutex<World>>;

fn serve() -> (String, Shared) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let world = Arc::new(Mutex::new(World {
        groq_key: GROQ_KEY.into(),
        ..World::default()
    }));
    let shared = Arc::clone(&world);
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { return };
            let world = Arc::clone(&shared);
            thread::spawn(move || handle(stream, &world));
        }
    });
    (base, world)
}

fn read(stream: &TcpStream) -> Option<Req> {
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut first = String::new();
    reader.read_line(&mut first).ok()?;
    let mut parts = first.split_whitespace();
    let method = parts.next()?.to_owned();
    let target = parts.next()?.to_owned();
    let mut headers = Vec::new();
    let mut length = 0;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).ok()? == 0 || line == "\r\n" {
            break;
        }
        let (name, value) = line.trim_end().split_once(':')?;
        let name = name.trim().to_ascii_lowercase();
        let value = value.trim().to_owned();
        if name == "content-length" {
            length = value.parse().unwrap_or(0);
        }
        headers.push((name, value));
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body).ok()?;
    let (path, _query) = target.split_once('?').unwrap_or((&target, ""));
    Some(Req {
        method,
        path: path.to_owned(),

        headers,
        body: String::from_utf8_lossy(&body).into_owned(),
    })
}

fn handle(mut stream: TcpStream, world: &Shared) {
    let Some(req) = read(&stream) else { return };
    let (status, body) = {
        let mut w = world.lock().unwrap();
        w.requests.push(req.clone());
        route(&req, &mut w)
    };
    let body = body.to_string();
    let _ = write!(
        stream,
        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

/// Los rechazos citan la credencial recibida, como haría un servicio descuidado o
/// malintencionado: NestAdmin no debe repetirla.
fn route(req: &Req, w: &mut World) -> (u16, Value) {
    let bearer = req.header("authorization").unwrap_or("").to_owned();
    match (req.method.as_str(), req.path.as_str()) {
        ("POST", "/openai/v1/chat/completions") => {
            if bearer != format!("Bearer {}", w.groq_key) {
                return (
                    401,
                    json!({ "error": { "message": format!("Invalid API Key: {bearer}"), "code": bearer } }),
                );
            }
            // Devuelve como salida del modelo el JSON de factura del documento.
            let sent: Value = serde_json::from_str(&req.body).unwrap();
            let content = sent["messages"][1]["content"].clone();
            (
                200,
                json!({
                    "model": "modelo-t09",
                    "choices": [{ "message": { "role": "assistant", "content": content } }],
                    "usage": { "prompt_tokens": 10, "completion_tokens": 5 },
                }),
            )
        }
        ("POST", "/token") => token(req, w),
        (_, path) if path.starts_with("/gmail/v1/users/me/") => {
            if w.reject_all || bearer != format!("Bearer {}", w.access) {
                return (
                    401,
                    json!({ "error": { "code": bearer, "status": bearer, "message": bearer } }),
                );
            }
            gmail(
                req.method.as_str(),
                path.trim_start_matches("/gmail/v1/users/me/"),
            )
        }
        _ => (404, json!({ "error": { "code": "NotFound" } })),
    }
}

fn token(req: &Req, w: &mut World) -> (u16, Value) {
    let grant = req.form("grant_type");
    let valid = match grant.as_deref() {
        Some("authorization_code") => req.form("code").as_deref() == Some(AUTH_CODE),
        Some("refresh_token") => {
            !w.refresh_revoked && req.form("refresh_token") == Some(refresh(w.issued))
        }
        _ => false,
    };
    if !valid || req.form("client_secret").as_deref() != Some(CLIENT_SECRET) {
        let echoed = req
            .form("refresh_token")
            .or_else(|| req.form("code"))
            .unwrap_or_default();
        return (
            400,
            json!({ "error": echoed, "error_description": format!("token {echoed} revocado") }),
        );
    }
    w.issued += 1;
    w.access = access(w.issued);
    (
        200,
        json!({
            "access_token": w.access,
            "refresh_token": refresh(w.issued),
            "expires_in": 3600,
        }),
    )
}

fn gmail(method: &str, path: &str) -> (u16, Value) {
    match (method, path) {
        ("GET", "messages") => (200, json!({ "messages": [{ "id": "g-1" }] })),
        ("GET", "messages/g-1") => (
            200,
            json!({
                "id": "g-1",
                "threadId": "t-1",
                "payload": {
                    "mimeType": "text/plain",
                    "headers": [
                        { "name": "Subject", "value": "Factura pendiente" },
                        { "name": "From", "value": "compras@beta.test" },
                        { "name": "Message-ID", "value": "<t09@beta.test>" },
                    ],
                    "body": { "data": "Tm9zIGZhbHRhIGxhIGZhY3R1cmEu" },
                },
            }),
        ),
        ("POST", "drafts") => (
            200,
            json!({ "id": "r-1", "message": { "id": "dm-1", "threadId": "t-1" } }),
        ),
        ("POST", "drafts/send") => (200, json!({ "id": "dm-1", "labelIds": ["SENT"] })),
        _ => (404, json!({ "error": { "status": "NOT_FOUND" } })),
    }
}

fn closed_port() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    url
}

// ---------------------------------------------------------------------------------------
// Inspección.

/// Busca la marca en UTF-8 y en UTF-16LE (por si algún componente escribe cadenas anchas).
fn contains_marker(bytes: &[u8]) -> bool {
    let wide: Vec<u8> = MARKER.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let has = |needle: &[u8]| bytes.windows(needle.len()).any(|w| w == needle);
    has(MARKER.as_bytes()) || has(&wide)
}

fn files_under(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            found.extend(files_under(&path));
        } else {
            found.push(path);
        }
    }
    found
}

/// Todos los archivos del directorio de datos y todos los textos recogidos deben estar
/// limpios. Devuelve los nombres de archivo inspeccionados.
fn assert_no_secrets(dir: &Path, texts: &[String]) -> Vec<String> {
    let files = files_under(dir);
    for path in &files {
        let bytes = fs::read(path).unwrap();
        assert!(!contains_marker(&bytes), "credencial en {}", path.display());
    }
    for text in texts {
        assert!(
            !text.contains(MARKER),
            "credencial en un texto visible: {text}"
        );
    }
    files
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect()
}

#[test]
fn inspection_detects_the_marker_in_any_file_or_encoding() {
    assert!(contains_marker(GROQ_KEY.as_bytes()));
    let wide: Vec<u8> = GROQ_KEY.encode_utf16().flat_map(u16::to_le_bytes).collect();
    assert!(contains_marker(&wide));
    assert!(!contains_marker(b"sin credenciales"));
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("sub")).unwrap();
    fs::write(dir.path().join("sub").join("fuga.txt"), access(1)).unwrap();
    let caught = std::panic::catch_unwind(|| assert_no_secrets(dir.path(), &[]));
    assert!(caught.is_err(), "la inspección debe encontrar la fuga");
}

// ---------------------------------------------------------------------------------------
// Groq: clave guardada, tareas procesadas y errores de credencial.

struct NoOcr;

impl Ocr for NoOcr {
    fn recognize(&self, _image: &Path, page: usize) -> Result<String, ExtractError> {
        Err(ExtractError::OcrUnavailable(format!("página {page}")))
    }
}

fn groq(endpoint: &str, key: Secret) -> Arc<dyn Provider> {
    Arc::new(GroqProvider::with_endpoint(
        &format!("{endpoint}/openai/v1"),
        "modelo-t09",
        key,
        Duration::from_secs(5),
    ))
}

/// Proveedor construido como lo hace la aplicación: con la clave leída del almacén.
fn groq_from_store(endpoint: &str, store: &dyn CredentialStore) -> Arc<dyn Provider> {
    groq(
        endpoint,
        store.get(GROQ_API_KEY).unwrap().expect("clave guardada"),
    )
}

fn invoice(number: u32) -> String {
    json!({
        "supplier": "Proveedor Sintético SL",
        "invoice_number": format!("T09-{number}"),
        "date": format!("2026-10-{number:02}"),
        "subtotal": "100.00",
        "tax": "21.00",
        "total": "121.00",
    })
    .to_string()
}

fn groq_hits(world: &Shared) -> usize {
    world
        .lock()
        .unwrap()
        .requests
        .iter()
        .filter(|r| r.path.ends_with("/chat/completions"))
        .count()
}

#[test]
fn groq_key_stays_in_the_store_through_tasks_and_credential_errors() {
    let dir = tempfile::tempdir().unwrap();
    let (base, world) = serve();
    let store = Arc::new(RecordingStore::default());
    let mut texts = Vec::new();

    // Guardar la clave, como el comando `save_groq_key` de la ventana.
    store.set(GROQ_API_KEY, &Secret::new(GROQ_KEY)).unwrap();
    let choice = ProviderChoice::Groq {
        model: "modelo-t09".into(),
    };
    let built = build_provider(&choice, store.as_ref(), Duration::from_secs(5)).unwrap();
    texts.push(format!(
        "{:?}",
        groq_from_store(&base, store.as_ref()).kind()
    ));
    texts.push(format!("{:?}", store.get(GROQ_API_KEY).unwrap()));
    texts.push(format!("{:?}", Secret::new(GROQ_KEY)));
    texts.push(Secret::new(GROQ_KEY).to_string());
    assert_eq!(built.kind(), ProviderKind::Groq);
    let provider_debug = format!(
        "{:?}",
        GroqProvider::with_endpoint(&base, "m", Secret::new(GROQ_KEY), Duration::from_secs(1))
    );
    texts.push(provider_debug);

    let extractor = Arc::new(Extractor::new(
        dir.path().join("sin-pdfium"),
        Box::new(NoOcr),
    ));
    let paths = AppPaths::new(dir.path());
    let mut app = App::open(
        &paths,
        groq_from_store(&base, store.as_ref()),
        extractor.clone(),
    )
    .unwrap();

    // 1. Flujo normal con Groq: extracción, revisión y registro; correo con borrador.
    let task = app.import_document("t09-1.txt", &invoice(1)).unwrap();
    let reviewed = app.process_next().unwrap().unwrap();
    assert_eq!(reviewed.state, TaskState::AwaitingReview, "{reviewed:?}");
    let done = app.approve(task.id, 1).unwrap();
    assert_eq!(done.state, TaskState::Completed);
    assert!(app.fetch_mail().unwrap().added > 0);
    while let Some(task) = app.process_next().unwrap() {
        texts.push(serde_json::to_string(&task).unwrap());
    }
    let ok_hits = groq_hits(&world);
    assert!(ok_hits >= 2, "las tareas pasaron por Groq simulado");
    {
        let w = world.lock().unwrap();
        let auth = w.requests[0].header("authorization").unwrap();
        assert_eq!(
            auth,
            format!("Bearer {GROQ_KEY}"),
            "la clave sí viaja al proveedor"
        );
    }

    // 2. Clave revocada: `unauthorized` recuperable, sin el secreto y sin otro proveedor.
    store.set(GROQ_API_KEY, &Secret::new(GROQ_REVOKED)).unwrap();
    app.select_provider(groq_from_store(&base, store.as_ref()));
    let revoked = app.import_document("t09-2.txt", &invoice(2)).unwrap();
    let failed = app.process_next().unwrap().unwrap();
    assert_eq!(failed.id, revoked.id);
    assert_eq!(failed.state, TaskState::Failed);
    let error = failed.error.clone().unwrap();
    assert_eq!(
        (error.code.as_str(), error.recoverable),
        ("unauthorized", true)
    );
    assert!(failed.result.is_none());
    assert_eq!(
        app.provider().provider,
        ProviderKind::Groq,
        "sin cambio de proveedor"
    );
    assert_eq!(
        groq_hits(&world),
        ok_hits + 1,
        "un único intento, sin reintentos ocultos"
    );
    texts.push(serde_json::to_string(&failed).unwrap());

    // 3. Fallo de red: `unavailable`, recuperable, sin el secreto.
    app.select_provider(groq(&closed_port(), Secret::new(GROQ_KEY)));
    app.retry(revoked.id).unwrap();
    let offline = app.process_next().unwrap().unwrap();
    let error = offline.error.clone().unwrap();
    assert_eq!(
        (error.code.as_str(), error.recoverable),
        ("unavailable", true)
    );
    texts.push(serde_json::to_string(&offline).unwrap());

    // 4. Clave con caracteres de control (inválida): se rechaza sin inyectar cabeceras ni
    //    mostrar el secreto.
    let malformed = format!("{GROQ_KEY}\r\nX-Inyectada: {MARKER}");
    app.select_provider(groq(&base, Secret::new(malformed)));
    app.retry(revoked.id).unwrap();
    let invalid = app.process_next().unwrap().unwrap();
    assert_eq!(invalid.state, TaskState::Failed);
    assert!(invalid.error.as_ref().unwrap().recoverable);
    texts.push(serde_json::to_string(&invalid).unwrap());
    assert!(world
        .lock()
        .unwrap()
        .requests
        .iter()
        .all(|r| r.header("x-inyectada").is_none()));

    // 5. Recuperación explícita: el usuario guarda una clave válida y reintenta.
    store.set(GROQ_API_KEY, &Secret::new(GROQ_KEY)).unwrap();
    app.select_provider(groq_from_store(&base, store.as_ref()));
    app.retry(revoked.id).unwrap();
    let recovered = app.process_next().unwrap().unwrap();
    assert_eq!(recovered.state, TaskState::AwaitingReview);

    // 6. Clave ausente: Groq no se construye y el error lo dice, sin pasar a Ollama.
    store.delete(GROQ_API_KEY).unwrap();
    let missing = build_provider(&choice, store.as_ref(), Duration::from_secs(5)).err();
    assert_eq!(missing, Some(AiError::Unauthorized));
    texts.push(missing.unwrap().to_string());

    // Todo lo que la ventana mostraría o escribiría en un registro.
    for task in app.tasks().unwrap() {
        texts.push(serde_json::to_string(&task).unwrap());
        texts.push(format!("{task:?}"));
    }
    for entry in app.audit_log().unwrap() {
        texts.push(serde_json::to_string(&entry).unwrap());
        texts.push(format!("{entry:?}"));
    }
    assert!(app.tasks().unwrap().iter().any(|t| t.kind == INVOICE));
    drop(app);

    // La clave estuvo en el almacén y solo allí.
    assert_eq!(
        *store.names.lock().unwrap(),
        BTreeSet::from([GROQ_API_KEY.to_owned()])
    );
    let files = assert_no_secrets(dir.path(), &texts);
    for expected in [
        "nestadmin.db",
        "simulador-correo.json",
        "simulador-administracion.json",
    ] {
        assert!(
            files.iter().any(|f| f == expected),
            "{expected} inspeccionado: {files:?}"
        );
    }
}

// ---------------------------------------------------------------------------------------
// Correo: inicio de sesión OAuth, tokens renovados y errores de credencial.

fn oauth(base: &str) -> OAuthConfig {
    OAuthConfig {
        auth_url: format!("{base}/authorize"),
        token_url: format!("{base}/token"),
        client_id: "cliente-t09.apps.test".into(),
        client_secret: Some(CLIENT_SECRET.into()),
        scopes: vec!["https://www.googleapis.com/auth/gmail.modify".into()],
        extra_auth_params: Vec::new(),
        redirect_host: "127.0.0.1".into(),
    }
}

/// Hace de navegador: sigue la redirección al puerto local con el código de autorización.
fn browser_redirect(redirect_uri: &str, query: &str) {
    let address = redirect_uri
        .trim_start_matches("http://")
        .trim_end_matches('/');
    let mut stream = TcpStream::connect(address).unwrap();
    write!(stream, "GET /?{query} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n").unwrap();
    let mut response = String::new();
    let _ = stream.read_to_string(&mut response);
}

fn state_of(url: &str) -> String {
    url.split(['?', '&'])
        .find_map(|pair| pair.strip_prefix("state="))
        .unwrap()
        .to_owned()
}

fn mailbox(base: &str, store: &Arc<RecordingStore>, state: &Path) -> Mailbox<Gmail> {
    let source = TokenSource::new(oauth(base), store.clone(), ACCOUNT);
    let gmail = Gmail::new(&format!("{base}/gmail/v1/users/me"), source, 10).unwrap();
    Mailbox::open(gmail, state).unwrap()
}

#[test]
fn mail_tokens_stay_in_the_store_through_sign_in_refresh_and_errors() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("correo").join(format!("{ACCOUNT}.json"));
    let (base, world) = serve();
    let store = Arc::new(RecordingStore::default());
    let config = oauth(&base);
    let mut texts = vec![format!("{config:?}")];

    // 1. Inicio de sesión con PKCE: el código y los tokens no salen del almacén.
    let pending = PendingAuthorization::begin(&config).unwrap();
    texts.push(format!("{pending:?}"));
    let redirect = pending.redirect_uri().to_owned();
    let query = format!(
        "code={}&state={}",
        AUTH_CODE.replace('/', "%2F"),
        state_of(pending.url())
    );
    let browser = thread::spawn(move || browser_redirect(&redirect, &query));
    let tokens = pending.finish(&config, Duration::from_secs(10)).unwrap();
    browser.join().unwrap();
    assert_eq!(tokens.access_token, access(1));
    texts.push(format!("{tokens:?}"));
    let source = TokenSource::new(config.clone(), store.clone(), ACCOUNT);
    source.save(&tokens).unwrap();
    texts.push(format!("{source:?}"));

    // 2. Uso normal: lista, borrador y envío; el registro local de la cuenta se escribe.
    let account = mailbox(&base, &store, &state);
    let page = account.list_messages(None).unwrap();
    assert_eq!(page.messages.len(), 1);
    let draft = NewDraft {
        to: vec!["compras@beta.test".into()],
        subject: None,
        body: "Se la enviamos hoy.".into(),
    };
    let draft_id = account.create_draft("g-1", &draft, "t09-borrador").unwrap();
    let receipt = account.send_draft(&draft_id, "t09-envio").unwrap();
    texts.push(format!("{page:?} {receipt:?}"));
    assert!(state.exists());

    // 3. Token caducado: se renueva con el token de renovación y se guarda en el almacén.
    source
        .save(&Tokens {
            expires_at: 0,
            ..tokens.clone()
        })
        .unwrap();
    mailbox(&base, &store, &state).list_messages(None).unwrap();
    let stored = store.get(&format!("mail/{ACCOUNT}")).unwrap().unwrap();
    assert!(
        stored.expose().contains(&access(2)),
        "token renovado en el almacén"
    );

    // 4. Credencial revocada en el proveedor: 401, una renovación, otro 401 →
    //    `unauthorized`, sin citar el token que el servidor repite.
    world.lock().unwrap().reject_all = true;
    let error = mailbox(&base, &store, &state)
        .list_messages(None)
        .unwrap_err();
    assert_eq!(error.code, "unauthorized");
    texts.push(format!("{error} {error:?}"));

    // 5. Token de renovación revocado: el servicio de tokens lo repite en `error`.
    world.lock().unwrap().refresh_revoked = true;
    let error = mailbox(&base, &store, &state)
        .list_messages(None)
        .unwrap_err();
    assert_eq!(error.code, "unauthorized");
    assert!(error.message.contains("vuelva a iniciar sesión"), "{error}");
    texts.push(format!("{error} {error:?}"));

    // 6. Sin red: `unavailable`, sin el token.
    let offline = closed_port();
    let source_offline = TokenSource::new(oauth(&offline), store.clone(), ACCOUNT);
    let gmail = Gmail::new(&format!("{offline}/gmail/v1/users/me"), source_offline, 10).unwrap();
    let error = Mailbox::in_memory(gmail).list_messages(None).unwrap_err();
    assert_eq!(error.code, "unavailable");
    texts.push(format!("{error} {error:?}"));

    // 7. Credencial ilegible en el almacén: se trata como sesión no iniciada.
    store
        .set(
            &format!("mail/{ACCOUNT}"),
            &Secret::new(format!("{{roto {MARKER}")),
        )
        .unwrap();
    let error = mailbox(&base, &store, &state)
        .list_messages(None)
        .unwrap_err();
    assert_eq!(error.code, "unauthorized");
    texts.push(format!("{error} {error:?}"));

    // 8. Sesión cerrada (credencial ausente): `unauthorized` sin llamar al proveedor.
    let before = world.lock().unwrap().requests.len();
    TokenSource::new(config, store.clone(), ACCOUNT)
        .sign_out()
        .unwrap();
    let error = mailbox(&base, &store, &state)
        .list_messages(None)
        .unwrap_err();
    assert_eq!(error.code, "unauthorized");
    assert!(error.message.contains("no ha iniciado sesión"), "{error}");
    assert_eq!(world.lock().unwrap().requests.len(), before);
    texts.push(format!("{error} {error:?}"));

    assert_eq!(
        *store.names.lock().unwrap(),
        BTreeSet::from([format!("mail/{ACCOUNT}")])
    );
    let files = assert_no_secrets(dir.path(), &texts);
    assert!(
        files.iter().any(|f| f == &format!("{ACCOUNT}.json")),
        "{files:?}"
    );
}
