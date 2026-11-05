//! T13 (integración en la aplicación): varias cuentas de correo a la vez —el simulador, una de
//! Gmail y otra de Microsoft Graph— con Gmail y Graph simulados en 127.0.0.1. Todos los datos
//! son sintéticos (dominio `.test`); ninguna prueba sale a la red ni usa cuentas reales.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use nestadmin_ai::{
    AiError, CredentialStore, GenerateRequest, GenerateResult, MemoryCredentialStore, Provider,
    ProviderKind, Usage,
};
use nestadmin_app::accounts::{
    AccountStatus, MailEnv, MailProfiles, MailProviderKind, ProviderProfile,
};
use nestadmin_app::{App, AppPaths, EMAIL};
use nestadmin_core::TaskState;
use nestadmin_extract::{ExtractError, Extractor, Ocr};
use nestadmin_mail::{OAuthConfig, Tokens};
use serde_json::{json, Value};
use url::Url;

// ---- Proveedor de IA y OCR de prueba ----

struct FakeProvider;

impl Provider for FakeProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Ollama
    }
    fn model(&self) -> &str {
        "simulado"
    }
    fn generate(&self, request: &GenerateRequest) -> Result<GenerateResult, AiError> {
        Ok(GenerateResult {
            request_id: request.request_id.clone(),
            provider: ProviderKind::Ollama,
            model: "simulado".into(),
            output: json!({ "category": "solicitud_factura", "draft": "Adjuntamos la factura." }),
            latency_ms: 1,
            usage: Usage::default(),
        })
    }
}

struct NoOcr;

impl Ocr for NoOcr {
    fn recognize(&self, _image: &Path, page: usize) -> Result<String, ExtractError> {
        Err(ExtractError::OcrUnavailable(format!("página {page}")))
    }
}

// ---- Servidor HTTP simulado ----

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
struct World {
    base: String,
    /// Graph: (id, asunto, remitente, cuerpo); Gmail usa los mismos campos.
    messages: Vec<(String, String, String, String)>,
    /// Graph: ID del borrador → ¿sigue siendo borrador?
    drafts: HashMap<String, bool>,
    replies: Vec<(String, Value)>,
    sends: usize,
    drop_send: bool,
    /// Todas las rutas responden con este estado.
    fail: Option<u16>,
    requests: usize,
}

type Shared = Arc<Mutex<World>>;
type Handler = fn(&Req, &mut World) -> Resp;

fn serve(handler: Handler, messages: Vec<(String, String, String, String)>) -> (String, Shared) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let world = Arc::new(Mutex::new(World {
        base: base.clone(),
        messages,
        ..World::default()
    }));
    let shared = Arc::clone(&world);
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { return };
            let world = Arc::clone(&shared);
            thread::spawn(move || handle(stream, &world, handler));
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
    let mut authorized = false;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).ok()? == 0 || line == "\r\n" {
            break;
        }
        let (name, value) = line.trim_end().split_once(':')?;
        match name.trim().to_ascii_lowercase().as_str() {
            "content-length" => length = value.trim().parse().unwrap_or(0),
            "authorization" => authorized = value.trim() == "Bearer acceso-sintetico",
            _ => {}
        }
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body).ok()?;
    authorized.then(|| Req {
        method,
        url: Url::parse(&format!("{base}{target}")).unwrap(),
        body: String::from_utf8_lossy(&body).into_owned(),
    })
}

fn handle(mut stream: TcpStream, world: &Shared, handler: Handler) {
    let base = world.lock().unwrap().base.clone();
    let resp = match read(&stream, &base) {
        None => Resp::Json(
            401,
            json!({ "error": { "code": "InvalidAuthenticationToken" } }),
        ),
        Some(req) => {
            let mut w = world.lock().unwrap();
            w.requests += 1;
            match w.fail {
                Some(status) => Resp::Json(status, json!({ "error": { "code": "Forzado" } })),
                None => handler(&req, &mut w),
            }
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

/// Microsoft Graph mínimo: bandeja paginada con `$top`/`$skip`, `createReply`, `send` e
/// `isDraft`.
fn graph(req: &Req, w: &mut World) -> Resp {
    let path = req.url.path().trim_start_matches("/v1.0/");
    let segments: Vec<&str> = path.split('/').collect();
    match (req.method.as_str(), segments.as_slice()) {
        ("GET", ["me", "mailFolders", "inbox", "messages"]) => {
            let top: usize = req.query("$top").unwrap().parse().unwrap();
            let skip: usize = req.query("$skip").map_or(0, |s| s.parse().unwrap());
            let value: Vec<Value> = w
                .messages
                .iter()
                .skip(skip)
                .take(top)
                .map(|(id, subject, from, body)| {
                    json!({
                        "id": id, "subject": subject,
                        "from": { "emailAddress": { "address": from } },
                        "body": { "contentType": "text", "content": body },
                    })
                })
                .collect();
            let mut page = json!({ "value": value });
            if skip + top < w.messages.len() {
                page["@odata.nextLink"] = json!(format!(
                    "{}/v1.0/me/mailFolders/inbox/messages?%24top={top}&%24skip={}",
                    w.base,
                    skip + top
                ));
            }
            Resp::Json(200, page)
        }
        ("POST", ["me", "messages", id, "createReply"]) => {
            let draft = format!("borrador-{}", w.drafts.len() + 1);
            w.drafts.insert(draft.clone(), true);
            w.replies
                .push(((*id).to_owned(), serde_json::from_str(&req.body).unwrap()));
            Resp::Json(201, json!({ "id": draft }))
        }
        ("POST", ["me", "messages", id, "send"]) => match w.drafts.get_mut(*id) {
            Some(is_draft) => {
                *is_draft = false;
                w.sends += 1;
                if std::mem::take(&mut w.drop_send) {
                    Resp::Drop
                } else {
                    Resp::Empty(202)
                }
            }
            None => Resp::Json(404, json!({ "error": { "code": "ErrorItemNotFound" } })),
        },
        // Dirección del buzón (D48).
        ("GET", ["me"]) => Resp::Json(
            200,
            json!({ "mail": null, "userPrincipalName": "oficina@empresa.test" }),
        ),
        ("GET", ["me", "messages", id]) => match w.drafts.get(*id) {
            Some(is_draft) => Resp::Json(200, json!({ "id": id, "isDraft": is_draft })),
            None => Resp::Json(404, json!({ "error": { "code": "ErrorItemNotFound" } })),
        },
        _ => Resp::Json(404, json!({ "error": { "code": "Desconocido" } })),
    }
}

/// Gmail mínimo: lista paginada con `maxResults`/`pageToken` y mensajes completos.
fn gmail(req: &Req, w: &mut World) -> Resp {
    let path = req.url.path().trim_start_matches("/gmail/v1/users/me/");
    let segments: Vec<&str> = path.split('/').collect();
    match (req.method.as_str(), segments.as_slice()) {
        ("GET", ["messages"]) => {
            let size: usize = req.query("maxResults").unwrap().parse().unwrap();
            let skip: usize = req.query("pageToken").map_or(0, |s| s.parse().unwrap());
            let ids: Vec<Value> = w
                .messages
                .iter()
                .skip(skip)
                .take(size)
                .map(|m| json!({ "id": m.0 }))
                .collect();
            let mut page = json!({ "messages": ids });
            if skip + size < w.messages.len() {
                page["nextPageToken"] = json!((skip + size).to_string());
            }
            Resp::Json(200, page)
        }
        // Dirección del buzón (D48).
        ("GET", ["profile"]) => Resp::Json(200, json!({ "emailAddress": "facturas@empresa.test" })),
        ("GET", ["messages", id]) => match w.messages.iter().find(|m| m.0 == *id) {
            Some((id, subject, from, body)) => Resp::Json(
                200,
                json!({
                    "id": id, "threadId": format!("hilo-{id}"), "labelIds": ["INBOX"],
                    "payload": {
                        "mimeType": "text/plain",
                        "headers": [
                            { "name": "Subject", "value": subject },
                            { "name": "From", "value": format!("Proveedor <{from}>") },
                        ],
                        "body": { "data": URL_SAFE_NO_PAD.encode(body) },
                    },
                }),
            ),
            None => Resp::Json(404, json!({ "error": { "status": "NOT_FOUND" } })),
        },
        _ => Resp::Json(404, json!({ "error": { "status": "NOT_FOUND" } })),
    }
}

// ---- Montaje ----

fn messages(prefix: &str, count: usize) -> Vec<(String, String, String, String)> {
    (1..=count)
        .map(|n| {
            (
                format!("{prefix}{n}"),
                format!("Solicitud de factura {n}"),
                format!("facturas{n}@proveedor-{prefix}.test"),
                format!("¿Pueden enviarnos la factura del pedido {n}?"),
            )
        })
        .collect()
}

fn oauth(base: &str) -> OAuthConfig {
    OAuthConfig {
        auth_url: format!("{base}/authorize"),
        token_url: format!("{base}/token"),
        client_id: "cliente-sintetico".into(),
        client_secret: None,
        scopes: vec!["correo".into()],
        extra_auth_params: Vec::new(),
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn valid_tokens() -> Tokens {
    Tokens {
        access_token: "acceso-sintetico".into(),
        refresh_token: Some("renovacion-sintetica".into()),
        expires_at: now() + 3600,
    }
}

struct Fixture {
    dir: tempfile::TempDir,
    store: Arc<MemoryCredentialStore>,
    graph: (String, Shared),
    gmail: (String, Shared),
}

impl Fixture {
    fn new(graph_messages: usize, gmail_messages: usize) -> Self {
        Self {
            dir: tempfile::tempdir().unwrap(),
            store: Arc::new(MemoryCredentialStore::default()),
            graph: serve(graph, messages("g", graph_messages)),
            gmail: serve(gmail, messages("m", gmail_messages)),
        }
    }

    fn env(&self, configured: bool) -> MailEnv {
        let profiles = if configured {
            MailProfiles {
                google: Some(ProviderProfile {
                    oauth: oauth(&self.gmail.0),
                    api_base: format!("{}/gmail/v1/users/me", self.gmail.0),
                }),
                microsoft: Some(ProviderProfile {
                    oauth: oauth(&self.graph.0),
                    api_base: format!("{}/v1.0", self.graph.0),
                }),
            }
        } else {
            MailProfiles::default()
        };
        MailEnv {
            credentials: self.store.clone(),
            profiles,
        }
    }

    fn open_with(&self, configured: bool) -> App {
        let extractor = Extractor::new(self.dir.path().join("sin-pdfium"), Box::new(NoOcr));
        App::open_with_mail(
            &AppPaths::new(self.dir.path()),
            Arc::new(FakeProvider),
            Arc::new(extractor),
            self.env(configured),
        )
        .unwrap()
    }

    fn open(&self) -> App {
        self.open_with(true)
    }
}

/// App con el simulador, una cuenta de Microsoft y otra de Google.
fn three_accounts(fixture: &Fixture) -> App {
    let mut app = fixture.open();
    let microsoft = app
        .add_mail_account(MailProviderKind::Microsoft, "Oficina", &valid_tokens())
        .unwrap();
    let google = app
        .add_mail_account(MailProviderKind::Google, "", &valid_tokens())
        .unwrap();
    assert_eq!(microsoft.id, "microsoft-1");
    assert_eq!(google.id, "google-1");
    assert_eq!(google.label, "Google 1");
    app
}

fn process_all(app: &mut App) -> Vec<nestadmin_core::Task> {
    let mut done = Vec::new();
    while let Some(task) = app.process_next().unwrap() {
        done.push(task);
    }
    done
}

// ---- Pruebas ----

#[test]
fn accounts_are_read_together_with_ids_prefixed_and_tokens_in_the_store() {
    let fixture = Fixture::new(3, 2);
    let mut app = three_accounts(&fixture);

    // Los tokens están en el almacén con el nombre de la cuenta y la lista no los contiene.
    for id in ["microsoft-1", "google-1"] {
        assert!(fixture.store.get(&format!("mail/{id}")).unwrap().is_some());
    }
    let list =
        std::fs::read_to_string(fixture.dir.path().join("correo").join("cuentas.json")).unwrap();
    assert!(!list.contains("sintetic"), "{list}");

    let report = app.fetch_mail().unwrap();
    let by_account: HashMap<_, _> = report
        .accounts
        .iter()
        .map(|a| (a.account.as_str(), (a.read, a.added, a.error.is_none())))
        .collect();
    assert_eq!(by_account["microsoft-1"], (3, 3, true));
    assert_eq!(by_account["google-1"], (2, 2, true));
    assert!(by_account["simulador"].1 > 0);
    assert_eq!(app.fetch_mail().unwrap().added, 0, "no se encola dos veces");

    let ids: Vec<String> = app
        .tasks()
        .unwrap()
        .iter()
        .filter(|t| t.kind == EMAIL)
        .map(|t| serde_json::from_str::<Value>(&t.input_ref).unwrap()["message_id"].to_string())
        .collect();
    assert!(ids.contains(&"\"microsoft-1:g1\"".to_owned()), "{ids:?}");
    assert!(ids.contains(&"\"google-1:m2\"".to_owned()), "{ids:?}");
    assert!(ids.iter().any(|id| id.starts_with("\"simulador:")));

    // Procesar no vuelve a pedir los mensajes: viajan en la tarea.
    let before = fixture.graph.1.lock().unwrap().requests;
    let tasks = process_all(&mut app);
    assert_eq!(fixture.graph.1.lock().unwrap().requests, before);
    // Sin regla activa todo queda en revisión y nada llega al proveedor (R07).
    assert!(tasks.iter().all(|t| t.state == TaskState::AwaitingReview));
    assert!(fixture.graph.1.lock().unwrap().replies.is_empty());
    let graph_task = tasks
        .iter()
        .find(|t| t.result.as_ref().unwrap()["message"]["id"] == "microsoft-1:g2")
        .unwrap();
    assert_eq!(
        graph_task.result.as_ref().unwrap()["draft"]["to"],
        json!(["facturas2@proveedor-g.test"])
    );
}

#[test]
fn the_limit_caps_messages_read_per_account() {
    let fixture = Fixture::new(7, 4);
    let mut app = three_accounts(&fixture);
    assert!(app.set_mail_limit(0).is_err());
    assert!(app.set_mail_limit(501).is_err());
    app.set_mail_limit(3).unwrap();
    assert_eq!(app.mail_accounts().limit, 3);

    let report = app.fetch_mail().unwrap();
    assert_eq!(report.limit, 3);
    for fetch in &report.accounts {
        assert!(fetch.read <= 3, "{fetch:?}");
    }
    assert_eq!(
        report
            .accounts
            .iter()
            .find(|a| a.account == "microsoft-1")
            .unwrap()
            .read,
        3
    );
    // Con límite 3 y 7 mensajes, Graph no pasa de la primera página (la otra petición es la
    // dirección del buzón al iniciar sesión, D48).
    let requests = fixture.graph.1.lock().unwrap().requests;
    assert_eq!(requests, 2);
}

#[test]
fn one_failing_account_does_not_stop_the_others() {
    let fixture = Fixture::new(2, 2);
    let mut app = three_accounts(&fixture);
    fixture.gmail.1.lock().unwrap().fail = Some(500);
    // La cuenta de Microsoft tiene la sesión caducada y sin token de renovación.
    app.reconnect_mail_account(
        "microsoft-1",
        &Tokens {
            access_token: "caducado".into(),
            refresh_token: None,
            expires_at: 1,
        },
    )
    .unwrap();

    let report = app.fetch_mail().unwrap();
    let error = |id: &str| {
        report
            .accounts
            .iter()
            .find(|a| a.account == id)
            .unwrap()
            .error
            .clone()
    };
    assert_eq!(error("google-1").unwrap().code, "unavailable");
    let expired = error("microsoft-1").unwrap();
    assert_eq!(expired.code, "unauthorized");
    assert!(
        !expired.message.contains("caducado"),
        "sin tokens en el error"
    );
    assert!(error("simulador").is_none());
    assert!(report.added > 0, "el simulador se leyó igualmente");

    // La pantalla de cuentas muestra el último error de cada una.
    let view = app.mail_accounts();
    let google = view.accounts.iter().find(|a| a.id == "google-1").unwrap();
    assert_eq!(
        google
            .last_fetch
            .as_ref()
            .unwrap()
            .error
            .as_ref()
            .unwrap()
            .code,
        "unavailable"
    );

    // Volver a iniciar sesión corrige la cuenta caducada.
    app.reconnect_mail_account("microsoft-1", &valid_tokens())
        .unwrap();
    let report = app.fetch_mail().unwrap();
    let microsoft = report
        .accounts
        .iter()
        .find(|a| a.account == "microsoft-1")
        .unwrap();
    assert_eq!((microsoft.added, microsoft.error.clone()), (2, None));
}

#[test]
fn approved_reply_is_sent_once_through_its_own_account_even_if_the_answer_is_lost() {
    let fixture = Fixture::new(1, 1);
    let mut app = three_accounts(&fixture);
    app.fetch_mail().unwrap();
    let task = process_all(&mut app)
        .into_iter()
        .find(|t| t.result.as_ref().unwrap()["message"]["id"] == "microsoft-1:g1")
        .unwrap();

    // El envío ocurre en Graph, pero la respuesta se pierde: la tarea falla visible.
    fixture.graph.1.lock().unwrap().drop_send = true;
    let failed = app.approve(task.id, 1).unwrap();
    assert_eq!(failed.state, TaskState::Failed);
    {
        let graph = fixture.graph.1.lock().unwrap();
        assert_eq!(graph.sends, 1);
        assert_eq!(graph.replies.len(), 1);
        assert_eq!(
            graph.replies[0].0, "g1",
            "la respuesta va al mensaje original"
        );
        assert_eq!(graph.replies[0].1["comment"], "Adjuntamos la factura.");
    }
    // Las demás cuentas no recibieron nada.
    assert!(app.simulators().0.sent().unwrap().is_empty());

    // Tras reiniciar, reintentar y volver a aprobar: la cuenta que envió confirma por el
    // estado del borrador y no hay un segundo envío.
    drop(app);
    let mut app = fixture.open();
    app.retry(task.id).unwrap();
    process_all(&mut app);
    let done = app.approve(task.id, 1).unwrap();
    assert_eq!(done.state, TaskState::Completed);
    assert_eq!(done.result.unwrap()["receipt"]["account"], "microsoft-1");
    assert_eq!(fixture.graph.1.lock().unwrap().sends, 1);
    assert_eq!(fixture.graph.1.lock().unwrap().replies.len(), 1);
    let log = app.audit_log().unwrap();
    let reconciled = log
        .iter()
        .find(|e| e.task_id == Some(task.id) && e.status == "reconciled")
        .expect("envío reconciliado");
    assert_eq!(reconciled.action, "send_draft");

    // La clave que guardó la cuenta lleva su prefijo.
    let state = std::fs::read_to_string(fixture.dir.path().join("correo").join("microsoft-1.json"))
        .unwrap();
    assert!(state.contains(&format!("microsoft-1/task-{}/send_draft/v1", task.id)));
    assert!(state.contains(&format!("microsoft-1/task-{}/create_draft/v1", task.id)));
}

#[test]
fn an_approved_reply_completes_with_a_receipt_from_its_account() {
    let fixture = Fixture::new(1, 0);
    let mut app = three_accounts(&fixture);
    app.fetch_mail().unwrap();
    let task = process_all(&mut app)
        .into_iter()
        .find(|t| t.result.as_ref().unwrap()["message"]["id"] == "microsoft-1:g1")
        .unwrap();
    let done = app.approve(task.id, 1).unwrap();
    assert_eq!(done.state, TaskState::Completed);
    let receipt = &done.result.unwrap()["receipt"];
    assert_eq!(receipt["account"], "microsoft-1");
    assert_eq!(receipt["draft_id"], "microsoft-1:borrador-1");
    assert_eq!(receipt["receipt_id"], "microsoft-1:borrador-1");
    // Aprobar de nuevo no es posible ni vuelve a enviar.
    assert!(app.approve(task.id, 1).is_err());
    assert_eq!(fixture.graph.1.lock().unwrap().sends, 1);
}

#[test]
fn disconnecting_deletes_tokens_and_unconfigured_providers_are_explicit() {
    let fixture = Fixture::new(1, 1);
    let mut app = three_accounts(&fixture);
    let status = |app: &App, id: &str| {
        app.mail_accounts()
            .accounts
            .iter()
            .find(|a| a.id == id)
            .map(|a| a.status)
    };
    assert_eq!(status(&app, "google-1"), Some(AccountStatus::Connected));

    // Tokens borrados por fuera: la cuenta aparece sin sesión.
    fixture.store.delete("mail/microsoft-1").unwrap();
    assert_eq!(status(&app, "microsoft-1"), Some(AccountStatus::SignedOut));

    app.remove_mail_account("google-1").unwrap();
    assert!(fixture.store.get("mail/google-1").unwrap().is_none());
    assert_eq!(status(&app, "google-1"), None);
    assert!(app.remove_mail_account("google-1").is_err());

    // El simulador también se puede quitar y volver a añadir.
    app.remove_mail_account("simulador").unwrap();
    let report = app.fetch_mail().unwrap();
    assert!(report.accounts.iter().all(|a| a.account != "simulador"));
    app.add_simulated_mail_account().unwrap();
    assert_eq!(status(&app, "simulador"), Some(AccountStatus::Connected));

    // La lista sobrevive al reinicio; una instalación sin cliente OAuth lo dice.
    drop(app);
    let app = fixture.open_with(false);
    let view = app.mail_accounts();
    assert!(!view.google_configured && !view.microsoft_configured);
    assert_eq!(
        status(&app, "microsoft-1"),
        Some(AccountStatus::NotConfigured)
    );
    assert!(app.mail_oauth(MailProviderKind::Google).is_err());
    let ids: Vec<&str> = view.accounts.iter().map(|a| a.id.as_str()).collect();
    assert_eq!(ids, ["simulador", "microsoft-1"]);
}

#[test]
fn a_new_account_id_is_never_reused_while_its_predecessor_exists() {
    let fixture = Fixture::new(0, 0);
    let mut app = three_accounts(&fixture);
    let second = app
        .add_mail_account(MailProviderKind::Google, "Facturación", &valid_tokens())
        .unwrap();
    assert_eq!(second.id, "google-2");
    app.remove_mail_account("google-1").unwrap();
    let third = app
        .add_mail_account(MailProviderKind::Google, "", &valid_tokens())
        .unwrap();
    assert_eq!(third.id, "google-3");
}

#[test]
fn each_mail_keeps_the_address_and_name_of_its_account() {
    let fixture = Fixture::new(2, 1);
    let mut app = three_accounts(&fixture);
    let addresses: HashMap<String, Option<String>> = app
        .mail_accounts()
        .accounts
        .into_iter()
        .map(|a| (a.id, a.address))
        .collect();
    assert_eq!(
        addresses["simulador"].as_deref(),
        Some("administracion@nestadmin.test")
    );
    // Graph sin `mail`: se usa `userPrincipalName`.
    assert_eq!(
        addresses["microsoft-1"].as_deref(),
        Some("oficina@empresa.test")
    );
    assert_eq!(
        addresses["google-1"].as_deref(),
        Some("facturas@empresa.test")
    );
    // La lista guarda la dirección (D48), nunca los tokens.
    let list =
        std::fs::read_to_string(fixture.dir.path().join("correo").join("cuentas.json")).unwrap();
    assert!(list.contains("oficina@empresa.test"), "{list}");
    assert!(!list.contains("sintetic"), "{list}");

    app.fetch_mail().unwrap();
    let origin_of = |app: &App, message: &str| {
        app.tasks()
            .unwrap()
            .into_iter()
            .map(|t| serde_json::from_str::<Value>(&t.input_ref).unwrap())
            .find(|i| i["message_id"] == message)
            .unwrap()["origin"]
            .clone()
    };
    assert_eq!(
        origin_of(&app, "microsoft-1:g1"),
        json!({
            "account": "microsoft-1", "provider": "microsoft",
            "label": "Oficina", "address": "oficina@empresa.test",
        })
    );
    assert_eq!(origin_of(&app, "google-1:m1")["label"], "Google 1");

    // Quitar la cuenta no borra de dónde vinieron sus correos.
    app.remove_mail_account("microsoft-1").unwrap();
    let tasks = process_all(&mut app);
    let from_office = tasks
        .iter()
        .find(|t| t.result.as_ref().unwrap()["message"]["id"] == "microsoft-1:g1")
        .unwrap();
    let result = from_office.result.as_ref().unwrap();
    assert_eq!(result["origin"]["label"], "Oficina");
    assert_eq!(result["origin"]["address"], "oficina@empresa.test");
    assert_eq!(result["message"]["from"], "facturas1@proveedor-g.test");
}

#[test]
fn an_address_the_provider_does_not_give_stays_absent() {
    let fixture = Fixture::new(0, 0);
    let mut app = fixture.open();
    fixture.gmail.1.lock().unwrap().fail = Some(503);
    let added = app
        .add_mail_account(MailProviderKind::Google, "Facturación", &valid_tokens())
        .unwrap();
    assert_eq!(added.address, None, "sin dirección no se inventa ninguna");
    let list =
        std::fs::read_to_string(fixture.dir.path().join("correo").join("cuentas.json")).unwrap();
    assert!(!list.contains("\"address\": null"), "{list}");

    // Al renovar la sesión se obtiene y se guarda.
    fixture.gmail.1.lock().unwrap().fail = None;
    let renewed = app
        .reconnect_mail_account("google-1", &valid_tokens())
        .unwrap();
    assert_eq!(renewed.address.as_deref(), Some("facturas@empresa.test"));
    drop(app);
    let mut app = fixture.open();
    let google = app
        .mail_accounts()
        .accounts
        .into_iter()
        .find(|a| a.id == "google-1")
        .unwrap();
    assert_eq!(google.address.as_deref(), Some("facturas@empresa.test"));

    // Si después falla, se conserva la que constaba.
    fixture.gmail.1.lock().unwrap().fail = Some(503);
    let kept = app
        .reconnect_mail_account("google-1", &valid_tokens())
        .unwrap();
    assert_eq!(kept.address.as_deref(), Some("facturas@empresa.test"));
}

#[test]
fn accounts_can_be_renamed_without_changing_their_id() {
    let fixture = Fixture::new(1, 1);
    let mut app = three_accounts(&fixture);
    app.fetch_mail().unwrap();

    let renamed = app
        .rename_mail_account("google-1", "  Facturación  ")
        .unwrap();
    assert_eq!(renamed.id, "google-1");
    assert_eq!(renamed.label, "Facturación");
    // Un nombre repetido se admite: la dirección las distingue.
    let same = app
        .rename_mail_account("microsoft-1", "Facturación")
        .unwrap();
    assert_eq!(same.label, "Facturación");
    assert_ne!(same.address, renamed.address);
    // Vacío vuelve al predeterminado; el simulador también se puede renombrar.
    assert_eq!(
        app.rename_mail_account("microsoft-1", " ").unwrap().label,
        "Microsoft 1"
    );
    assert_eq!(
        app.rename_mail_account("simulador", "Pruebas")
            .unwrap()
            .label,
        "Pruebas"
    );
    assert_eq!(
        app.rename_mail_account("simulador", "").unwrap().label,
        "Correo simulado"
    );
    assert!(app.rename_mail_account("google-9", "x").is_err());

    // Se guarda en cuentas.json y sobrevive a reabrir; los mensajes siguen en su cuenta.
    drop(app);
    let mut app = fixture.open();
    let google = app
        .mail_accounts()
        .accounts
        .into_iter()
        .find(|a| a.id == "google-1")
        .unwrap();
    assert_eq!(google.label, "Facturación");
    assert_eq!(app.fetch_mail().unwrap().added, 0, "el ID no cambió");
}
