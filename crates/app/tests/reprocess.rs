//! D50: reprocesar una tarea cuyo envío ya se intentó. Con el simulador y con Gmail y Graph
//! simulados en 127.0.0.1 (datos sintéticos, dominio `.test`): corte tras crear el borrador,
//! envío con la respuesta perdida, reproceso con otro texto, borrador borrado por el usuario y
//! reinicio de la aplicación entre medias. En ningún caso hay dos envíos.
//!
//! Los cortes del proceso se simulan con un pánico: el almacén de credenciales falla al
//! guardar el token renovado justo antes de la llamada de envío, cuando el borrador ya existe
//! en el proveedor y la acción consta como pendiente. Después se descarta la aplicación y se
//! vuelve a abrir sobre los mismos datos.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::{STANDARD, URL_SAFE, URL_SAFE_NO_PAD};
use base64::Engine;
use nestadmin_ai::{
    AiError, CredentialError, CredentialStore, GenerateRequest, GenerateResult,
    MemoryCredentialStore, Provider, ProviderKind, Secret, Usage,
};
use nestadmin_app::accounts::{MailEnv, MailProfiles, MailProviderKind, ProviderProfile};
use nestadmin_app::{App, AppPaths};
use nestadmin_core::{Task, TaskState};
use nestadmin_extract::{ExtractError, Extractor, Ocr};
use nestadmin_mail::{OAuthConfig, Tokens};
use nestadmin_simulators::FailureMode;
use serde_json::{json, Value};
use url::Url;

// ---- IA que propone otro texto cada vez que se le pide ----

#[derive(Default)]
struct FakeProvider {
    calls: Mutex<HashMap<String, usize>>,
}

impl FakeProvider {
    fn calls_for(&self, task: i64) -> usize {
        self.calls
            .lock()
            .unwrap()
            .get(&task.to_string())
            .copied()
            .unwrap_or(0)
    }
}

impl Provider for FakeProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Ollama
    }
    fn model(&self) -> &str {
        "simulado"
    }
    fn generate(&self, request: &GenerateRequest) -> Result<GenerateResult, AiError> {
        let n = {
            let mut calls = self.calls.lock().unwrap();
            let n = calls.entry(request.request_id.clone()).or_default();
            *n += 1;
            *n
        };
        Ok(GenerateResult {
            request_id: request.request_id.clone(),
            provider: ProviderKind::Ollama,
            model: "simulado".into(),
            output: json!({ "category": "solicitud_factura", "draft": proposal(n) }),
            latency_ms: 1,
            usage: Usage::default(),
        })
    }
}

fn proposal(n: usize) -> String {
    format!("Adjuntamos la factura (propuesta {n}).")
}

struct NoOcr;

impl Ocr for NoOcr {
    fn recognize(&self, _image: &Path, page: usize) -> Result<String, ExtractError> {
        Err(ExtractError::OcrUnavailable(format!("página {page}")))
    }
}

// ---- Almacén de credenciales que simula un corte del proceso ----

/// Mientras `armed` está activo, guardar un token provoca un pánico: el proceso "se corta"
/// justo antes de la siguiente llamada al proveedor.
struct CrashStore {
    inner: MemoryCredentialStore,
    armed: Arc<AtomicBool>,
}

impl CredentialStore for CrashStore {
    fn get(&self, name: &str) -> Result<Option<Secret>, CredentialError> {
        self.inner.get(name)
    }
    fn set(&self, name: &str, secret: &Secret) -> Result<(), CredentialError> {
        if self.armed.swap(false, Ordering::SeqCst) {
            panic!("corte simulado del proceso antes de enviar");
        }
        self.inner.set(name, secret)
    }
    fn delete(&self, name: &str) -> Result<(), CredentialError> {
        self.inner.delete(name)
    }
}

// ---- Gmail y Graph simulados en un mismo servidor ----

struct Req {
    method: String,
    url: Url,
    body: String,
}

impl Req {
    fn query(&self, name: &str) -> Option<String> {
        self.url
            .query_pairs()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.into_owned())
    }
}

enum Resp {
    Json(u16, Value),
    Empty(u16),
    /// El efecto ocurre, pero la respuesta se pierde.
    Drop,
}

#[derive(Default)]
struct GmailDraft {
    message_id: String,
    rfc822: String,
    body: String,
}

#[derive(Default)]
struct World {
    /// Graph: ID del borrador → (¿sigue siendo borrador?, texto).
    graph_drafts: HashMap<String, (bool, String)>,
    gmail_drafts: HashMap<String, GmailDraft>,
    /// Gmail: (ID del enviado, `Message-ID`).
    gmail_sent: Vec<(String, String)>,
    /// Borradores creados en total (los dos proveedores).
    created: usize,
    /// Texto de cada envío realizado (los dos proveedores).
    sends: Vec<String>,
    /// El próximo envío ocurre pero su respuesta se pierde.
    drop_send: bool,
    /// Los envíos fallan con 500 sin efecto.
    fail_send: bool,
    /// Todas las rutas de API (no la de tokens) responden 503.
    unavailable: bool,
    /// Al crear el próximo borrador, el siguiente guardado de token corta el proceso.
    crash_after_create: bool,
    armed: Arc<AtomicBool>,
}

type Shared = Arc<Mutex<World>>;

fn serve(armed: Arc<AtomicBool>) -> (String, Shared) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let world = Arc::new(Mutex::new(World {
        armed,
        ..World::default()
    }));
    let shared = Arc::clone(&world);
    let origin = base.clone();
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { return };
            let world = Arc::clone(&shared);
            let origin = origin.clone();
            thread::spawn(move || handle(stream, &world, &origin));
        }
    });
    (base, world)
}

fn read(stream: &TcpStream, base: &str) -> Option<Req> {
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut first = String::new();
    reader.read_line(&mut first).ok()?;
    let mut parts = first.split_whitespace();
    let method = parts.next()?.to_owned();
    let target = parts.next()?.to_owned();
    let mut length = 0;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).ok()? == 0 || line == "\r\n" {
            break;
        }
        let (name, value) = line.trim_end().split_once(':')?;
        if name.trim().eq_ignore_ascii_case("content-length") {
            length = value.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body).ok()?;
    Some(Req {
        method,
        url: Url::parse(&format!("{base}{target}")).unwrap(),
        body: String::from_utf8_lossy(&body).into_owned(),
    })
}

fn handle(mut stream: TcpStream, world: &Shared, base: &str) {
    let Some(req) = read(&stream, base) else {
        return;
    };
    let resp = {
        let mut w = world.lock().unwrap();
        let path = req.url.path().to_owned();
        if path == "/token" {
            // Token que caduca enseguida: cada llamada lo renueva y lo guarda.
            Resp::Json(
                200,
                json!({ "access_token": "acceso-sintetico", "expires_in": 0 }),
            )
        } else if w.unavailable {
            Resp::Json(503, json!({ "error": { "code": "Forzado" } }))
        } else if let Some(rest) = path.strip_prefix("/v1.0/") {
            graph(&req, rest, &mut w)
        } else if let Some(rest) = path.strip_prefix("/gmail/v1/users/me/") {
            gmail(&req, rest, &mut w)
        } else {
            Resp::Json(404, json!({ "error": { "code": "Desconocido" } }))
        }
    };
    let (status, body) = match resp {
        Resp::Json(status, body) => (status, body.to_string()),
        Resp::Empty(status) => (status, String::new()),
        Resp::Drop => return,
    };
    let _ = write!(
        stream,
        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

fn after_create(w: &mut World) {
    w.created += 1;
    if std::mem::take(&mut w.crash_after_create) {
        w.armed.store(true, Ordering::SeqCst);
    }
}

fn not_found() -> Resp {
    Resp::Json(
        404,
        json!({ "error": { "code": "ErrorItemNotFound", "status": "NOT_FOUND" } }),
    )
}

/// Graph mínimo: un mensaje en la bandeja, `createReply`, `send` e `isDraft`.
fn graph(req: &Req, path: &str, w: &mut World) -> Resp {
    let segments: Vec<&str> = path.split('/').collect();
    match (req.method.as_str(), segments.as_slice()) {
        ("GET", ["me", "mailFolders", "inbox", "messages"]) => Resp::Json(
            200,
            json!({ "value": [{
                "id": "g1", "subject": "Solicitud de factura",
                "from": { "emailAddress": { "address": "facturas@proveedor-g.test" } },
                "body": { "contentType": "text", "content": "¿Pueden enviarnos la factura?" },
            }] }),
        ),
        ("POST", ["me", "messages", "g1", "createReply"]) => {
            let body: Value = serde_json::from_str(&req.body).unwrap();
            // Los IDs no se reutilizan aunque el usuario borre borradores.
            let id = format!("borrador-{}", w.created + 1);
            let text = body["comment"].as_str().unwrap().to_owned();
            w.graph_drafts.insert(id.clone(), (true, text));
            after_create(w);
            Resp::Json(201, json!({ "id": id }))
        }
        ("POST", ["me", "messages", id, "send"]) => {
            if w.fail_send {
                return Resp::Json(500, json!({ "error": { "code": "Forzado" } }));
            }
            match w.graph_drafts.get_mut(*id) {
                Some((is_draft @ true, text)) => {
                    *is_draft = false;
                    let text = text.clone();
                    w.sends.push(text);
                    if std::mem::take(&mut w.drop_send) {
                        Resp::Drop
                    } else {
                        Resp::Empty(202)
                    }
                }
                _ => not_found(),
            }
        }
        ("GET", ["me", "messages", id]) => match w.graph_drafts.get(*id) {
            Some((is_draft, _)) => Resp::Json(200, json!({ "id": id, "isDraft": is_draft })),
            None => not_found(),
        },
        _ => Resp::Json(404, json!({ "error": { "code": "Desconocido" } })),
    }
}

/// Lee el `Message-ID` y el cuerpo (base64) de un mensaje MIME de borrador.
fn parse_mime(raw: &str) -> (String, String) {
    let mime = String::from_utf8(URL_SAFE.decode(raw).unwrap()).unwrap();
    let (head, body) = mime.split_once("\r\n\r\n").unwrap();
    let rfc822 = head
        .lines()
        .find_map(|l| l.strip_prefix("Message-ID: "))
        .unwrap()
        .to_owned();
    let compact: String = body.split_whitespace().collect();
    (
        rfc822,
        String::from_utf8(STANDARD.decode(compact).unwrap()).unwrap(),
    )
}

/// Gmail mínimo: un mensaje en la bandeja, borradores, envío con ID nuevo y búsqueda por
/// `rfc822msgid:` entre los enviados.
fn gmail(req: &Req, path: &str, w: &mut World) -> Resp {
    let segments: Vec<&str> = path.split('/').collect();
    match (req.method.as_str(), segments.as_slice()) {
        ("GET", ["messages"]) => match req.query("q") {
            Some(q) => {
                let wanted = format!("<{}>", q.trim_start_matches("rfc822msgid:"));
                let found: Vec<Value> = w
                    .gmail_sent
                    .iter()
                    .filter(|(_, rfc)| *rfc == wanted)
                    .map(|(id, _)| json!({ "id": id }))
                    .collect();
                Resp::Json(200, json!({ "messages": found }))
            }
            None => Resp::Json(200, json!({ "messages": [{ "id": "m1" }] })),
        },
        ("GET", ["messages", "m1"]) => Resp::Json(
            200,
            json!({
                "id": "m1", "threadId": "hilo-m1", "labelIds": ["INBOX"],
                "payload": {
                    "mimeType": "text/plain",
                    "headers": [
                        { "name": "Subject", "value": "Solicitud de factura" },
                        { "name": "From", "value": "Proveedor <facturas@proveedor-m.test>" },
                        { "name": "Message-ID", "value": "<original-m1@proveedor-m.test>" },
                    ],
                    "body": { "data": URL_SAFE_NO_PAD.encode("¿Pueden enviarnos la factura?") },
                },
            }),
        ),
        ("GET", ["messages", id]) => {
            if let Some((id, rfc)) = w.gmail_sent.iter().find(|(s, _)| s == id) {
                return Resp::Json(
                    200,
                    json!({ "id": id, "labelIds": ["SENT"],
                            "payload": { "headers": [{ "name": "Message-ID", "value": rfc }] } }),
                );
            }
            if w.gmail_drafts.values().any(|d| d.message_id == *id) {
                return Resp::Json(200, json!({ "id": id, "labelIds": ["DRAFT"] }));
            }
            not_found()
        }
        ("POST", ["drafts"]) => {
            let body: Value = serde_json::from_str(&req.body).unwrap();
            let (rfc822, text) = parse_mime(body["message"]["raw"].as_str().unwrap());
            let n = w.created + 1;
            let (id, message_id) = (format!("r-{n}"), format!("dm-{n}"));
            w.gmail_drafts.insert(
                id.clone(),
                GmailDraft {
                    message_id: message_id.clone(),
                    rfc822,
                    body: text,
                },
            );
            after_create(w);
            Resp::Json(200, json!({ "id": id, "message": { "id": message_id } }))
        }
        ("POST", ["drafts", "send"]) => {
            if w.fail_send {
                return Resp::Json(500, json!({ "error": { "status": "INTERNAL" } }));
            }
            let body: Value = serde_json::from_str(&req.body).unwrap();
            match w.gmail_drafts.remove(body["id"].as_str().unwrap()) {
                Some(draft) => {
                    // Gmail da al mensaje enviado un ID distinto del del borrador.
                    let sent_id = format!("s-{}", w.gmail_sent.len() + 1);
                    w.gmail_sent.push((sent_id.clone(), draft.rfc822));
                    w.sends.push(draft.body);
                    if std::mem::take(&mut w.drop_send) {
                        Resp::Drop
                    } else {
                        Resp::Json(200, json!({ "id": sent_id }))
                    }
                }
                None => not_found(),
            }
        }
        ("GET", ["drafts", id]) => match w.gmail_drafts.contains_key(*id) {
            true => Resp::Json(200, json!({ "id": id })),
            false => not_found(),
        },
        _ => not_found(),
    }
}

// ---- Montaje ----

#[derive(Clone, Copy, Debug)]
enum Kind {
    Graph,
    Gmail,
}

const REAL: [Kind; 2] = [Kind::Graph, Kind::Gmail];

impl Kind {
    fn message_id(self) -> &'static str {
        match self {
            Kind::Graph => "microsoft-1:g1",
            Kind::Gmail => "google-1:m1",
        }
    }
    fn account(self) -> &'static str {
        match self {
            Kind::Graph => "microsoft-1",
            Kind::Gmail => "google-1",
        }
    }
}

fn oauth(base: &str) -> OAuthConfig {
    OAuthConfig {
        auth_url: format!("{base}/authorize"),
        token_url: format!("{base}/token"),
        client_id: "cliente-sintetico".into(),
        client_secret: None,
        scopes: vec!["correo".into()],
        extra_auth_params: Vec::new(),
        redirect_host: "127.0.0.1".into(),
    }
}

/// Tokens ya caducados: cada llamada los renueva en `/token` y los guarda en el almacén.
fn expiring_tokens() -> Tokens {
    Tokens {
        access_token: "acceso-sintetico".into(),
        refresh_token: Some("renovacion-sintetica".into()),
        expires_at: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs(),
    }
}

struct Fixture {
    dir: tempfile::TempDir,
    store: Arc<CrashStore>,
    base: String,
    world: Shared,
    ai: Arc<FakeProvider>,
}

impl Fixture {
    fn new() -> Self {
        let armed = Arc::new(AtomicBool::new(false));
        let (base, world) = serve(Arc::clone(&armed));
        Self {
            dir: tempfile::tempdir().unwrap(),
            store: Arc::new(CrashStore {
                inner: MemoryCredentialStore::default(),
                armed,
            }),
            base,
            world,
            ai: Arc::new(FakeProvider::default()),
        }
    }

    fn world(&self) -> std::sync::MutexGuard<'_, World> {
        self.world.lock().unwrap()
    }

    fn open(&self) -> App {
        let profile = |api: &str| ProviderProfile {
            oauth: oauth(&self.base),
            api_base: format!("{}{api}", self.base),
        };
        let extractor = Extractor::new(self.dir.path().join("sin-pdfium"), Box::new(NoOcr));
        App::open_with_mail(
            &AppPaths::new(self.dir.path()),
            self.ai.clone(),
            Arc::new(extractor),
            MailEnv {
                credentials: self.store.clone(),
                profiles: MailProfiles {
                    google: Some(profile("/gmail/v1/users/me")),
                    microsoft: Some(profile("/v1.0")),
                },
            },
        )
        .unwrap()
    }

    /// Aplicación con el simulador, una cuenta de Microsoft y otra de Google, y todas las
    /// tareas de correo en revisión.
    fn start(&self) -> App {
        let mut app = self.open();
        app.add_mail_account(MailProviderKind::Microsoft, "Oficina", &expiring_tokens())
            .unwrap();
        app.add_mail_account(MailProviderKind::Google, "Gmail", &expiring_tokens())
            .unwrap();
        app.fetch_mail().unwrap();
        process_all(&mut app);
        app
    }
}

fn process_all(app: &mut App) -> Vec<Task> {
    let mut done = Vec::new();
    while let Some(task) = app.process_next().unwrap() {
        done.push(task);
    }
    done
}

fn task_for(app: &App, message_id: &str) -> Task {
    app.tasks()
        .unwrap()
        .into_iter()
        .find(|t| serde_json::from_str::<Value>(&t.input_ref).unwrap()["message_id"] == message_id)
        .unwrap()
}

fn statuses(app: &App, task: i64) -> Vec<String> {
    app.audit_log()
        .unwrap()
        .into_iter()
        .filter(|e| e.task_id == Some(task))
        .map(|e| e.status)
        .collect()
}

fn body(task: &Task) -> String {
    task.result.as_ref().unwrap()["draft"]["body"]
        .as_str()
        .unwrap()
        .to_owned()
}

// ---- Pruebas ----

#[test]
fn a_lost_send_response_and_a_restart_complete_the_task_without_resending() {
    for kind in REAL {
        let fixture = Fixture::new();
        let mut app = fixture.start();
        let task = task_for(&app, kind.message_id());
        assert_eq!(body(&task), proposal(1));

        // El envío ocurre, pero la respuesta se pierde: la tarea falla visible.
        fixture.world().drop_send = true;
        let failed = app.approve(task.id, 1).unwrap();
        assert_eq!(failed.state, TaskState::Failed, "{kind:?}");
        assert_eq!(fixture.world().sends, [proposal(1)]);

        // Reinicio y reintento: no se pide otro texto a la IA; el conector confirma el envío
        // y la tarea se completa sin volver a enviar ni a pedir aprobación.
        drop(app);
        let mut app = fixture.open();
        app.retry(task.id).unwrap();
        process_all(&mut app);
        let done = app
            .tasks()
            .unwrap()
            .into_iter()
            .find(|t| t.id == task.id)
            .unwrap();
        assert_eq!(done.state, TaskState::Completed, "{kind:?}");
        assert_eq!(body(&done), proposal(1));
        let result = done.result.unwrap();
        assert_eq!(result["receipt"]["account"], kind.account());
        assert!(result.get("recovery").is_none());
        assert_eq!(
            fixture.ai.calls_for(task.id),
            1,
            "{kind:?}: sin otra llamada a la IA"
        );
        let world = fixture.world();
        assert_eq!(world.sends, [proposal(1)], "{kind:?}: un solo envío");
        assert_eq!(world.created, 1);
        drop(world);
        assert_eq!(
            statuses(&app, task.id),
            ["pending", "failed", "reconciled"],
            "{kind:?}"
        );
    }
}

#[test]
fn a_cut_after_creating_the_draft_resumes_the_same_draft_after_restart() {
    for kind in REAL {
        let fixture = Fixture::new();
        let mut app = fixture.start();
        let task = task_for(&app, kind.message_id());

        // El proceso se corta después de crear el borrador y antes de enviarlo.
        fixture.world().crash_after_create = true;
        let cut = catch_unwind(AssertUnwindSafe(|| app.approve(task.id, 1)));
        assert!(cut.is_err(), "{kind:?}: corte simulado");
        drop(app);
        assert_eq!(fixture.world().created, 1);
        assert!(fixture.world().sends.is_empty());

        // Al reabrir, la acción pendiente se reconcilia (el borrador sigue sin enviar) y la
        // tarea sigue en revisión con el mismo contenido y versión.
        let mut app = fixture.open();
        let pending = app
            .tasks()
            .unwrap()
            .into_iter()
            .find(|t| t.id == task.id)
            .unwrap();
        assert_eq!(pending.state, TaskState::AwaitingReview, "{kind:?}");
        assert_eq!(body(&pending), proposal(1));
        let result = pending.result.unwrap();
        assert_eq!(result["version"], 1);
        assert_eq!(result["attempt"]["version"], 1);

        // Aprobar de nuevo reutiliza el mismo borrador y envía una sola vez.
        let done = app.approve(task.id, 1).unwrap();
        assert_eq!(done.state, TaskState::Completed, "{kind:?}");
        let world = fixture.world();
        assert_eq!(world.created, 1, "{kind:?}: el mismo borrador");
        assert_eq!(world.sends, [proposal(1)], "{kind:?}: un solo envío");
        drop(world);
        assert_eq!(fixture.ai.calls_for(task.id), 1);
    }
}

#[test]
fn a_cut_while_a_rule_sends_returns_to_review_without_asking_the_ai_again() {
    let fixture = Fixture::new();
    let mut app = fixture.open();
    app.add_mail_account(MailProviderKind::Microsoft, "Oficina", &expiring_tokens())
        .unwrap();
    app.remove_mail_account("simulador").unwrap();
    app.create_rule(
        "facturas",
        "send_draft",
        &json!({ "category": "solicitud_factura" }),
    )
    .unwrap();
    app.set_rule_active("facturas", true).unwrap();
    app.fetch_mail().unwrap();

    fixture.world().crash_after_create = true;
    let cut = catch_unwind(AssertUnwindSafe(|| app.process_next()));
    assert!(cut.is_err(), "corte simulado durante el envío por regla");
    drop(app);

    // Al reabrir, la tarea en curso vuelve a la cola; al reprocesarla no se llama a la IA y,
    // como el envío no consta, vuelve a revisión: no se repite sin aprobación.
    let mut app = fixture.open();
    let task = task_for(&app, "microsoft-1:g1");
    assert_eq!(task.state, TaskState::Queued);
    process_all(&mut app);
    let review = app
        .tasks()
        .unwrap()
        .into_iter()
        .find(|t| t.id == task.id)
        .unwrap();
    assert_eq!(review.state, TaskState::AwaitingReview);
    assert_eq!(body(&review), proposal(1));
    let result = review.result.unwrap();
    assert_eq!(result["recovery"]["version"], 1);
    assert_eq!(result["recovery"]["needs_confirmation"], false);
    assert_eq!(fixture.ai.calls_for(task.id), 1);
    assert!(fixture.world().sends.is_empty());

    let done = app.approve(task.id, 1).unwrap();
    assert_eq!(done.state, TaskState::Completed);
    let world = fixture.world();
    assert_eq!((world.created, world.sends.len()), (1, 1));
}

#[test]
fn a_new_text_after_a_lost_send_never_sends_a_second_version() {
    for kind in REAL {
        let fixture = Fixture::new();
        let mut app = fixture.start();
        let task = task_for(&app, kind.message_id());

        fixture.world().drop_send = true;
        app.approve(task.id, 1).unwrap();
        // El proveedor no responde al reprocesar: vuelve a revisión con el mismo texto.
        fixture.world().unavailable = true;
        app.retry(task.id).unwrap();
        process_all(&mut app);
        let review = app
            .tasks()
            .unwrap()
            .into_iter()
            .find(|t| t.id == task.id)
            .unwrap();
        assert_eq!(review.state, TaskState::AwaitingReview, "{kind:?}");
        assert_eq!(body(&review), proposal(1));
        assert_eq!(review.result.unwrap()["recovery"]["error"], "unavailable");
        fixture.world().unavailable = false;

        // El usuario cambia el texto y aprueba: antes se comprueba la versión anterior, que sí
        // salió, y la tarea se completa con ella sin enviar la nueva.
        let edited = app.edit_draft(task.id, "Texto corregido a mano.").unwrap();
        assert_eq!(edited.result.unwrap()["version"], 2);
        let done = app.approve(task.id, 2).unwrap();
        assert_eq!(done.state, TaskState::Completed, "{kind:?}");
        assert_eq!(body(&done), proposal(1), "{kind:?}: muestra lo que salió");
        assert_eq!(done.result.unwrap()["version"], 1);
        let world = fixture.world();
        assert_eq!(world.sends, [proposal(1)], "{kind:?}: un solo envío");
        assert_eq!(world.created, 1);
    }
}

#[test]
fn a_new_text_after_a_failed_send_replaces_the_unsent_draft() {
    for kind in REAL {
        let fixture = Fixture::new();
        let mut app = fixture.start();
        let task = task_for(&app, kind.message_id());

        // El envío falla sin efecto: el borrador queda en el buzón sin enviar.
        fixture.world().fail_send = true;
        assert_eq!(app.approve(task.id, 1).unwrap().state, TaskState::Failed);
        fixture.world().fail_send = false;
        app.retry(task.id).unwrap();
        process_all(&mut app);
        assert_eq!(fixture.ai.calls_for(task.id), 1, "{kind:?}");

        app.edit_draft(task.id, "Texto corregido a mano.").unwrap();
        let done = app.approve(task.id, 2).unwrap();
        assert_eq!(done.state, TaskState::Completed, "{kind:?}");
        assert_eq!(body(&done), "Texto corregido a mano.");
        let world = fixture.world();
        assert_eq!(world.sends, ["Texto corregido a mano."], "{kind:?}");
        assert_eq!(
            world.created, 2,
            "{kind:?}: otro borrador para la versión 2"
        );
        drop(world);
        assert_eq!(
            statuses(&app, task.id),
            ["pending", "failed", "superseded", "pending", "confirmed"],
            "{kind:?}"
        );
    }
}

#[test]
fn a_draft_deleted_by_the_user_needs_explicit_confirmation_before_a_new_one() {
    for kind in REAL {
        let fixture = Fixture::new();
        let mut app = fixture.start();
        let task = task_for(&app, kind.message_id());

        fixture.world().fail_send = true;
        app.approve(task.id, 1).unwrap();
        // El usuario borra el borrador en su buzón; luego se reinicia la aplicación.
        {
            let mut world = fixture.world();
            world.fail_send = false;
            world.graph_drafts.clear();
            world.gmail_drafts.clear();
        }
        drop(app);
        let mut app = fixture.open();
        app.retry(task.id).unwrap();
        process_all(&mut app);
        let review = app
            .tasks()
            .unwrap()
            .into_iter()
            .find(|t| t.id == task.id)
            .unwrap();
        assert_eq!(review.state, TaskState::AwaitingReview, "{kind:?}");
        let result = review.result.unwrap();
        assert_eq!(result["recovery"]["needs_confirmation"], true, "{kind:?}");
        assert_eq!(result["recovery"]["error"], "not_found");

        // Aprobar otra vez la misma versión no reenvía: falla visible con `not_found`.
        let again = app.approve(task.id, 1).unwrap();
        assert_eq!(again.state, TaskState::Failed, "{kind:?}");
        assert_eq!(again.error.unwrap().code, "not_found");
        app.retry(task.id).unwrap();
        process_all(&mut app);

        // Cambiar el texto tampoco basta: exige confirmar que no se envió.
        app.edit_draft(task.id, "Texto corregido a mano.").unwrap();
        let blocked = app.approve(task.id, 2).unwrap();
        assert_eq!(blocked.state, TaskState::AwaitingReview, "{kind:?}");
        assert_eq!(
            blocked.result.unwrap()["recovery"]["needs_confirmation"],
            true
        );
        assert!(fixture.world().sends.is_empty());

        // Con la confirmación, el mismo texto pasa a una versión nueva que se aprueba aparte.
        let confirmed = app.confirm_not_sent(task.id).unwrap();
        assert_eq!(confirmed.state, TaskState::AwaitingReview);
        let result = confirmed.result.unwrap();
        assert_eq!(result["version"], 3);
        assert!(result.get("recovery").is_none());
        assert!(fixture.world().sends.is_empty(), "confirmar no envía nada");
        let done = app.approve(task.id, 3).unwrap();
        assert_eq!(done.state, TaskState::Completed, "{kind:?}");
        let world = fixture.world();
        assert_eq!(world.sends, ["Texto corregido a mano."], "{kind:?}");
        assert_eq!(world.created, 2);
        drop(world);
        assert_eq!(fixture.ai.calls_for(task.id), 1);
        assert!(statuses(&app, task.id).contains(&"superseded".to_owned()));
    }
}

#[test]
fn confirming_not_sent_completes_the_task_if_it_was_sent_after_all() {
    let fixture = Fixture::new();
    let mut app = fixture.start();
    let task = task_for(&app, Kind::Graph.message_id());
    fixture.world().drop_send = true;
    app.approve(task.id, 1).unwrap();
    fixture.world().unavailable = true;
    app.retry(task.id).unwrap();
    process_all(&mut app);
    fixture.world().unavailable = false;

    // El usuario cree que no salió, pero el proveedor lo ve enviado: se completa sin enviar.
    let done = app.confirm_not_sent(task.id).unwrap();
    assert_eq!(done.state, TaskState::Completed);
    assert_eq!(fixture.world().sends.len(), 1);
    // Sin ningún intento anterior no hay nada que confirmar.
    let other = task_for(&app, Kind::Gmail.message_id());
    assert!(app.confirm_not_sent(other.id).is_err());
}

#[test]
fn the_simulator_retry_reuses_the_approved_text() {
    let fixture = Fixture::new();
    let mut app = fixture.open();
    app.fetch_mail().unwrap();
    process_all(&mut app);
    let task = task_for(&app, "simulador:M-0002");
    assert_eq!(body(&task), proposal(1));

    app.simulators()
        .0
        .set_failure(Some(FailureMode::Unavailable));
    assert_eq!(app.approve(task.id, 1).unwrap().state, TaskState::Failed);
    app.simulators().0.set_failure(None);
    drop(app);

    let mut app = fixture.open();
    app.retry(task.id).unwrap();
    process_all(&mut app);
    let review = app
        .tasks()
        .unwrap()
        .into_iter()
        .find(|t| t.id == task.id)
        .unwrap();
    assert_eq!(review.state, TaskState::AwaitingReview);
    assert_eq!(
        body(&review),
        proposal(1),
        "mismo texto, sin otra propuesta"
    );
    assert_eq!(fixture.ai.calls_for(task.id), 1);
    assert_eq!(app.approve(task.id, 1).unwrap().state, TaskState::Completed);
    let sent = app.simulators().0.sent().unwrap();
    assert_eq!(sent.len(), 1);
    let drafts = app.simulators().0.drafts().unwrap();
    assert_eq!(drafts.len(), 1);
    assert_eq!(drafts[0].content.body, proposal(1));
}
