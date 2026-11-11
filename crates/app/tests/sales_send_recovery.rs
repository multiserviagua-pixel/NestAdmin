//! Envío de documentos de venta con cuentas de Gmail reales (simuladas en 127.0.0.1): que el
//! identificador de una cuenta no se reutilice, que lo que se prepara salga solo del mismo buzón
//! (D242), que «Correo simulado» no se pueda elegir si hay una cuenta real (D237) y que una cuenta
//! reconectada con el registro local vacío no permita concluir «no enviado» (D243). Solo datos
//! sintéticos (dominio `.test`); ninguna prueba sale a la red.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use nestadmin_ai::{
    AiError, GenerateRequest, GenerateResult, MemoryCredentialStore, Provider, ProviderKind,
};
use nestadmin_app::accounts::{MailEnv, MailProfiles, MailProviderKind, ProviderProfile};
use nestadmin_app::rules::SEND_SALES_DOCUMENT;
use nestadmin_app::{App, AppPaths};
use nestadmin_core::sales::{Company, CustomerData, DraftInput, DraftLine};
use nestadmin_core::TaskState;
use nestadmin_extract::{ExtractError, Extractor, Ocr};
use nestadmin_mail::{OAuthConfig, Tokens};
use nestadmin_sales::{DocumentKind, LineInput, Status, TaxRate};
use rust_decimal::Decimal;
use serde_json::{json, Value};
use url::Url;

struct NoAi;

impl Provider for NoAi {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Ollama
    }
    fn model(&self) -> &str {
        "simulado"
    }
    fn generate(&self, _request: &GenerateRequest) -> Result<GenerateResult, AiError> {
        panic!("el envío de documentos no usa IA");
    }
}

struct NoOcr;

impl Ocr for NoOcr {
    fn recognize(&self, _image: &Path, page: usize) -> Result<String, ExtractError> {
        Err(ExtractError::OcrUnavailable(format!("página {page}")))
    }
}

fn pdfium_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("NESTADMIN_PDFIUM_DIR") {
        return PathBuf::from(dir);
    }
    let mut dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    loop {
        let candidate = dir.join("vendor/pdfium/bin");
        if candidate.join("pdfium.dll").exists() {
            return candidate;
        }
        if !dir.pop() {
            panic!(
                "PDFium no disponible: ejecute scripts/setup-ocr.ps1 o defina NESTADMIN_PDFIUM_DIR"
            );
        }
    }
}

// ---- Gmail mínimo: perfil, subida del borrador, envío y consulta del borrador ----

struct Req {
    method: String,
    url: Url,
}

enum Resp {
    Json(u16, Value),
    /// El efecto ocurre, pero la respuesta se pierde.
    Drop,
}

struct World {
    base: String,
    /// Dirección que devuelve el perfil del buzón; `None` si el proveedor no la da.
    address: Option<String>,
    uploads: usize,
    sends: usize,
    drop_send: bool,
}

type Shared = Arc<Mutex<World>>;

fn serve() -> (String, Shared) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let world = Arc::new(Mutex::new(World {
        base: base.clone(),
        address: Some("oficina@empresa.test".into()),
        uploads: 0,
        sends: 0,
        drop_send: false,
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
    })
}

fn route(req: &Req, w: &mut World) -> Resp {
    let path = req.url.path().to_owned();
    match (req.method.as_str(), path.as_str()) {
        ("GET", "/gmail/v1/users/me/profile") => {
            Resp::Json(200, json!({ "emailAddress": w.address }))
        }
        ("POST", "/upload/gmail/v1/users/me/drafts") => {
            w.uploads += 1;
            let n = w.uploads;
            Resp::Json(
                200,
                json!({
                    "id": format!("d-{n}"),
                    "message": { "id": format!("m-{n}"), "threadId": format!("t-{n}") },
                }),
            )
        }
        ("POST", "/gmail/v1/users/me/drafts/send") => {
            w.sends += 1;
            if w.drop_send {
                Resp::Drop
            } else {
                Resp::Json(200, json!({ "id": format!("enviado-{}", w.sends) }))
            }
        }
        // El borrador sigue existiendo: no consta enviado.
        ("GET", p) if p.starts_with("/gmail/v1/users/me/drafts/") => Resp::Json(200, json!({})),
        _ => Resp::Json(404, json!({ "error": { "status": "NOT_FOUND" } })),
    }
}

fn handle(mut stream: TcpStream, world: &Shared) {
    let base = world.lock().unwrap().base.clone();
    let resp = match read(&stream, &base) {
        None => Resp::Json(401, json!({ "error": { "status": "UNAUTHENTICATED" } })),
        Some(req) => route(&req, &mut world.lock().unwrap()),
    };
    let (status, body) = match resp {
        Resp::Json(status, body) => (status, body.to_string()),
        Resp::Drop => return,
    };
    let _ = write!(
        stream,
        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

// ---- Montaje ----

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn tokens() -> Tokens {
    Tokens {
        access_token: "acceso-sintetico".into(),
        refresh_token: Some("renovacion-sintetica".into()),
        expires_at: now() + 3600,
    }
}

struct Fixture {
    dir: tempfile::TempDir,
    store: Arc<MemoryCredentialStore>,
    gmail: (String, Shared),
}

impl Fixture {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().unwrap(),
            store: Arc::new(MemoryCredentialStore::default()),
            gmail: serve(),
        }
    }

    fn set_address(&self, address: &str) {
        self.gmail.1.lock().unwrap().address = Some(address.into());
    }

    fn clear_address(&self) {
        self.gmail.1.lock().unwrap().address = None;
    }

    fn correo(&self) -> PathBuf {
        self.dir.path().join("correo")
    }

    fn counters(&self) -> (usize, usize) {
        let w = self.gmail.1.lock().unwrap();
        (w.uploads, w.sends)
    }

    fn env(&self) -> MailEnv {
        let base = &self.gmail.0;
        MailEnv {
            credentials: self.store.clone(),
            profiles: MailProfiles {
                google: Some(ProviderProfile {
                    oauth: OAuthConfig {
                        auth_url: format!("{base}/authorize"),
                        token_url: format!("{base}/token"),
                        client_id: "cliente-sintetico".into(),
                        client_secret: None,
                        scopes: vec!["correo".into()],
                        extra_auth_params: Vec::new(),
                        redirect_host: "127.0.0.1".into(),
                    },
                    api_base: format!("{base}/gmail/v1/users/me"),
                }),
                microsoft: None,
            },
        }
    }

    fn open(&self) -> App {
        let extractor = Extractor::new(pdfium_dir(), Box::new(NoOcr));
        App::open_with_mail(
            &AppPaths::new(self.dir.path()),
            Arc::new(NoAi),
            Arc::new(extractor),
            self.env(),
        )
        .unwrap()
    }
}

const CLIENT_EMAIL: &str = "cliente@example.test";
const MAILBOX_A: &str = "oficina@empresa.test";
const MAILBOX_B: &str = "otra@empresa.test";

fn d(text: &str) -> Decimal {
    text.parse().unwrap()
}

/// Empresa y un cliente con correo; devuelve el cliente.
fn setup(app: &mut App) -> i64 {
    app.set_sales_company(&Company {
        name: Some("Taller Demo SL".into()),
        tax_id: Some("B12345674".into()),
        address: Some("Calle Ficticia 1, 28000 Madrid".into()),
        email: Some("demo@example.test".into()),
        iban: None,
        logo: None,
        logo_mime: None,
    })
    .unwrap();
    app.create_customer(&CustomerData {
        name: "Cliente Uno SA".into(),
        tax_id: Some("A58818501".into()),
        address: Some("Avenida Inventada 2, 08000 Barcelona".into()),
        email: Some(CLIENT_EMAIL.into()),
    })
    .unwrap()
}

fn issue_quote(app: &mut App, customer: i64) -> i64 {
    let mut input = DraftInput::new(DocumentKind::Quote);
    input.customer_id = Some(customer);
    input.lines = vec![DraftLine::from(LineInput {
        description: "Revisión anual".into(),
        quantity: d("2"),
        unit_price: d("40"),
        discount: Decimal::ZERO,
        tax_rate: TaxRate::TwentyOne,
        exemption: None,
    })];
    let id = app.create_sales_draft(&input).unwrap();
    app.issue_sales_document(id, None).unwrap();
    id
}

fn code(error: &nestadmin_app::AppError) -> String {
    error.code().to_owned()
}

fn add_google(app: &mut App) -> String {
    app.add_mail_account(MailProviderKind::Google, "", &tokens())
        .unwrap()
        .id
}

fn process_all(app: &mut App) {
    while app.process_next().unwrap().is_some() {}
}

fn task_state(app: &App, task: i64) -> TaskState {
    app.tasks()
        .unwrap()
        .into_iter()
        .find(|t| t.id == task)
        .unwrap()
        .state
}

fn result_of(app: &App, task: i64) -> Value {
    app.tasks()
        .unwrap()
        .into_iter()
        .find(|t| t.id == task)
        .unwrap()
        .result
        .unwrap()
}

// ---------------------------------------------------------------------------------------------

#[test]
fn an_account_id_is_never_reused_and_the_counter_survives_reopening_and_legacy_lists() {
    let fixture = Fixture::new();
    let mut app = fixture.open();
    assert_eq!(add_google(&mut app), "google-1");
    assert_eq!(add_google(&mut app), "google-2");
    // Quitar la última no libera su número: antes salía otra vez «google-2».
    app.remove_mail_account("google-2").unwrap();
    assert_eq!(add_google(&mut app), "google-3");
    // Quitarlas todas tampoco reinicia la cuenta.
    app.remove_mail_account("google-1").unwrap();
    app.remove_mail_account("google-3").unwrap();
    assert_eq!(add_google(&mut app), "google-4");
    app.remove_mail_account("google-4").unwrap();

    // El contador consta en su archivo y sobrevive a reabrir la aplicación; `cuentas.json` sigue
    // siendo una lista simple (D246).
    drop(app);
    let list: Value = serde_json::from_str(
        &std::fs::read_to_string(fixture.correo().join("cuentas.json")).unwrap(),
    )
    .unwrap();
    assert!(list.is_array(), "{list}");
    let counters: Value = serde_json::from_str(
        &std::fs::read_to_string(fixture.correo().join("cuentas-contadores.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(counters["google"], 4);
    let mut app = fixture.open();
    assert_eq!(add_google(&mut app), "google-5");
    drop(app);

    // Una lista anterior, sin contador, lo inicia con el máximo actual.
    std::fs::write(
        fixture.dir.path().join("correo").join("cuentas.json"),
        json!([
            { "id": "simulador", "provider": "simulator", "label": "Correo simulado" },
            { "id": "google-7", "provider": "google", "label": "Google 7", "address": MAILBOX_A },
        ])
        .to_string(),
    )
    .unwrap();
    let mut app = fixture.open();
    assert_eq!(add_google(&mut app), "google-8");
    app.remove_mail_account("google-8").unwrap();
    assert_eq!(add_google(&mut app), "google-9");
}

#[test]
fn a_send_prepared_for_a_removed_account_never_leaves_from_the_account_that_replaces_it() {
    let fixture = Fixture::new();
    let mut app = fixture.open();
    let customer = setup(&mut app);
    fixture.set_address(MAILBOX_A);
    assert_eq!(add_google(&mut app), "google-1");
    let id = issue_quote(&mut app, customer);
    let task = app.prepare_sales_send(id).unwrap();
    let prepared = task.result.clone().unwrap();
    assert_eq!(prepared["account"]["id"], "google-1");
    assert_eq!(prepared["account"]["address"], MAILBOX_A);

    // Se quita la cuenta y se añade otra, de otro buzón: no hereda el identificador.
    app.remove_mail_account("google-1").unwrap();
    fixture.set_address(MAILBOX_B);
    let replacement = add_google(&mut app);
    assert_eq!(replacement, "google-2");

    let error = app.approve(task.id, 1).unwrap_err();
    assert_eq!(code(&error), "account_unavailable");
    assert_eq!(fixture.counters(), (0, 0), "no se subió ni envió nada");
    assert_eq!(task_state(&app, task.id), TaskState::AwaitingReview);
    assert!(app.sales_document_sends(id).unwrap().is_empty());
    // Tampoco se puede editar la tarea hacia la cuenta que ya no existe.
    let error = app
        .edit_sales_send(task.id, CLIENT_EMAIL, "Asunto", "Texto", "google-1")
        .unwrap_err();
    assert_eq!(code(&error), "account_unavailable");
}

#[test]
fn reconnecting_an_account_to_another_mailbox_blocks_the_prepared_send_and_the_rule() {
    let fixture = Fixture::new();
    let mut app = fixture.open();
    let customer = setup(&mut app);
    fixture.set_address(MAILBOX_A);
    assert_eq!(add_google(&mut app), "google-1");
    let id = issue_quote(&mut app, customer);
    let task = app.prepare_sales_send(id).unwrap();
    app.create_rule(
        "envio-cliente",
        SEND_SALES_DOCUMENT,
        &json!({ "customer_id": customer, "recipient": CLIENT_EMAIL, "account": "google-1" }),
    )
    .unwrap();
    app.set_rule_active("envio-cliente", true).unwrap();
    // La regla fija la dirección del buzón al guardarse.
    let rule = app.rules().unwrap().pop().unwrap();
    assert_eq!(rule.conditions["account_address"], MAILBOX_A);

    // El mismo identificador pasa a otro buzón (se vuelve a iniciar sesión con otra cuenta).
    fixture.set_address(MAILBOX_B);
    let view = app.reconnect_mail_account("google-1", &tokens()).unwrap();
    assert_eq!(view.address.as_deref(), Some(MAILBOX_B));

    // Lo que se preparó para el buzón anterior no sale desde el nuevo.
    let error = app.approve(task.id, 1).unwrap_err();
    assert_eq!(code(&error), "account_unavailable");
    assert_eq!(fixture.counters(), (0, 0));
    app.reject(task.id).unwrap();

    // La regla tampoco: la tarea nueva se queda en revisión, sin enviar.
    let other = issue_quote(&mut app, customer);
    assert_eq!(fixture.counters(), (0, 0), "la regla no envió");
    assert_eq!(
        app.sales_document(other).unwrap().status,
        Status::Issued,
        "sigue emitido"
    );
    let pending = app.sales_send_task(other).unwrap().expect("en revisión");
    assert_eq!(pending.state, TaskState::AwaitingReview);
    assert!(app.sales_document_sends(other).unwrap().is_empty());
}

#[test]
fn the_simulated_mailbox_cannot_be_chosen_when_editing_or_approving_if_a_real_account_exists() {
    let fixture = Fixture::new();
    let mut app = fixture.open();
    let customer = setup(&mut app);
    assert_eq!(add_google(&mut app), "google-1");
    let id = issue_quote(&mut app, customer);
    let task = app.prepare_sales_send(id).unwrap();
    assert_eq!(task.result.clone().unwrap()["account"]["id"], "google-1");

    // Al editar: la misma lista que al proponer.
    let error = app
        .edit_sales_send(task.id, CLIENT_EMAIL, "Asunto", "Texto", "simulador")
        .unwrap_err();
    assert_eq!(code(&error), "account_unavailable");
    let ids: Vec<String> = app
        .sales_send_accounts()
        .into_iter()
        .map(|a| a.id)
        .collect();
    assert_eq!(ids, ["google-1"]);

    // Al aprobar: un contenido que ya apunta al simulador (por ejemplo, tocado en la base) se
    // rechaza con el mismo criterio y no envía nada.
    let conn = rusqlite::Connection::open(fixture.dir.path().join("nestadmin.db")).unwrap();
    conn.execute(
        "UPDATE tasks SET result = json_set(result, '$.account.id', 'simulador', \
         '$.account.address', 'administracion@nestadmin.test') WHERE id = ?1",
        [task.id],
    )
    .unwrap();
    drop(conn);
    let error = app.approve(task.id, 1).unwrap_err();
    assert_eq!(code(&error), "account_unavailable");
    assert!(app.simulators().0.sent().unwrap().is_empty());
    assert_eq!(fixture.counters(), (0, 0));
}

#[test]
fn after_a_cut_a_reconnected_account_with_an_empty_local_record_never_concludes_not_sent() {
    let fixture = Fixture::new();
    let mut app = fixture.open();
    let customer = setup(&mut app);
    fixture.set_address(MAILBOX_A);
    assert_eq!(add_google(&mut app), "google-1");
    let id = issue_quote(&mut app, customer);
    let task = app.prepare_sales_send(id).unwrap();

    // El envío se hace y la respuesta se pierde: la tarea falla con el intento anotado.
    fixture.gmail.1.lock().unwrap().drop_send = true;
    let failed = app.approve(task.id, 1).unwrap();
    assert_eq!(failed.state, TaskState::Failed);
    assert_eq!(fixture.counters(), (1, 1));
    fixture.gmail.1.lock().unwrap().drop_send = false;

    // La cuenta se reconecta (mismo buzón) con su registro local de borradores vacío.
    let record = fixture.dir.path().join("correo").join("google-1.json");
    assert!(
        record.exists(),
        "el registro local guarda la clave del envío"
    );
    std::fs::remove_file(&record).unwrap();
    app.reconnect_mail_account("google-1", &tokens()).unwrap();

    // Reprocesar no concluye «no enviado» ni envía: pide la confirmación de la persona.
    app.retry(task.id).unwrap();
    process_all(&mut app);
    assert_eq!(task_state(&app, task.id), TaskState::AwaitingReview);
    let result = result_of(&app, task.id);
    assert_eq!(result["recovery"]["error"], "not_found");
    assert_eq!(result["recovery"]["needs_confirmation"], true);
    assert_eq!(fixture.counters(), (1, 1), "nada se envió otra vez");

    // Editar la tarea (versión nueva) y aprobar tampoco reenvía: sigue sin poder afirmarse.
    app.edit_sales_send(task.id, CLIENT_EMAIL, "Asunto nuevo", "Texto", "google-1")
        .unwrap();
    let back = app.approve(task.id, 2).unwrap();
    assert_ne!(back.state, TaskState::Completed);
    assert_eq!(fixture.counters(), (1, 1), "ni con la versión nueva");
    assert!(app.sales_document_sends(id).unwrap().is_empty());
    assert_eq!(
        app.sales_document(id).unwrap().status,
        Status::Issued,
        "sin recibo no hay `sent`"
    );

    // Solo «Confirmo que no se envió», de la persona, abre la versión nueva; luego se aprueba.
    process_all(&mut app);
    let confirmed = app.confirm_not_sent(task.id).unwrap();
    assert_eq!(confirmed.state, TaskState::AwaitingReview);
    assert_eq!(fixture.counters(), (1, 1), "confirmar no envía");
    let version = confirmed.result.unwrap()["version"].as_i64().unwrap();
    let done = app.approve(task.id, version).unwrap();
    assert_eq!(done.state, TaskState::Completed);
    assert_eq!(fixture.counters(), (2, 2));
    assert_eq!(app.sales_document_sends(id).unwrap().len(), 1);
}

// ---- Segunda ronda de correcciones (D246) ----

/// Lo que lee `main` de `cuentas.json`: una lista de este registro (sin `deny_unknown_fields`).
#[derive(Debug, serde::Deserialize)]
#[allow(dead_code)]
struct MainAccountRecord {
    id: String,
    provider: String,
    label: String,
    #[serde(default)]
    address: Option<String>,
}

fn read_as_main(fixture: &Fixture) -> Vec<MainAccountRecord> {
    let text = std::fs::read_to_string(fixture.correo().join("cuentas.json")).unwrap();
    serde_json::from_str(&text).expect("`main` lee cuentas.json como una lista de cuentas")
}

#[test]
fn cuentas_json_stays_a_plain_list_that_main_reads_and_the_branch_format_is_migrated() {
    let fixture = Fixture::new();
    fixture.set_address(MAILBOX_A);
    let mut app = fixture.open();
    assert_eq!(add_google(&mut app), "google-1");
    app.reconnect_mail_account("google-1", &tokens()).unwrap();
    app.remove_mail_account("google-1").unwrap();
    assert_eq!(add_google(&mut app), "google-2");
    drop(app);
    let list = read_as_main(&fixture);
    assert_eq!(
        list.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(),
        ["simulador", "google-2"]
    );
    assert_eq!(list[1].address.as_deref(), Some(MAILBOX_A));

    // El formato que escribió esta rama, `{counters, accounts}`, se lee y se migra a los dos
    // archivos: la lista simple y los contadores.
    std::fs::remove_file(fixture.correo().join("cuentas-contadores.json")).unwrap();
    std::fs::write(
        fixture.correo().join("cuentas.json"),
        json!({
            "counters": { "google": 6 },
            "accounts": [
                { "id": "simulador", "provider": "simulator", "label": "Correo simulado" },
                { "id": "google-2", "provider": "google", "label": "Google 2",
                  "address": MAILBOX_A, "epoch": 3 },
            ],
        })
        .to_string(),
    )
    .unwrap();
    let mut app = fixture.open();
    let list = read_as_main(&fixture);
    assert_eq!(list.len(), 2, "ya es una lista");
    let counters: Value = serde_json::from_str(
        &std::fs::read_to_string(fixture.correo().join("cuentas-contadores.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(counters["google"], 6);
    assert_eq!(add_google(&mut app), "google-7");
    // Una lista simple sin archivo de contadores se abre con el valor por defecto.
    drop(app);
    std::fs::remove_file(fixture.correo().join("cuentas-contadores.json")).unwrap();
    let mut app = fixture.open();
    assert_eq!(add_google(&mut app), "google-8");
}

#[test]
fn counters_started_from_a_plain_list_also_count_the_ids_in_tasks() {
    let fixture = Fixture::new();
    let mut app = fixture.open();
    let customer = setup(&mut app);
    assert_eq!(add_google(&mut app), "google-1");
    assert_eq!(add_google(&mut app), "google-2");
    let id = issue_quote(&mut app, customer);
    let task = app.prepare_sales_send(id).unwrap();
    app.edit_sales_send(task.id, CLIENT_EMAIL, "Asunto", "Texto", "google-2")
        .unwrap();
    app.remove_mail_account("google-2").unwrap();
    drop(app);

    // `main` reescribe la lista sin contadores y sin la cuenta quitada.
    std::fs::remove_file(fixture.correo().join("cuentas-contadores.json")).unwrap();
    std::fs::write(
        fixture.correo().join("cuentas.json"),
        json!([
            { "id": "simulador", "provider": "simulator", "label": "Correo simulado" },
            { "id": "google-1", "provider": "google", "label": "Google 1", "address": MAILBOX_A },
        ])
        .to_string(),
    )
    .unwrap();
    let mut app = fixture.open();
    // La tarea guarda `google-2`: su identificador no vuelve a darse.
    assert_eq!(add_google(&mut app), "google-3");
}

#[test]
fn a_send_without_any_mailbox_address_fails_closed_and_the_case_and_spaces_of_an_address_do_not_matter(
) {
    let fixture = Fixture::new();
    let mut app = fixture.open();
    let customer = setup(&mut app);

    // Sin dirección en la cuenta ni en lo preparado (ambas faltan): no se afirma «mismo buzón».
    fixture.clear_address();
    assert_eq!(add_google(&mut app), "google-1");
    let id = issue_quote(&mut app, customer);
    let task = app.prepare_sales_send(id).unwrap();
    assert!(task.result.clone().unwrap()["account"]["address"].is_null());
    let error = app.approve(task.id, 1).unwrap_err();
    assert_eq!(code(&error), "account_unavailable");
    assert_eq!(fixture.counters(), (0, 0));
    app.reject(task.id).unwrap();

    // Con dirección, mayúsculas y espacios de más no cambian el buzón.
    fixture.set_address("  Oficina@Empresa.TEST ");
    app.reconnect_mail_account("google-1", &tokens()).unwrap();
    let task = app.prepare_sales_send(id).unwrap();
    fixture.set_address(MAILBOX_A);
    app.reconnect_mail_account("google-1", &tokens()).unwrap();
    let done = app.approve(task.id, 1).unwrap();
    assert_eq!(done.state, TaskState::Completed);
    assert_eq!(fixture.counters(), (1, 1));
}
