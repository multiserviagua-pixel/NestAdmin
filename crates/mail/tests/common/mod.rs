//! Servidor HTTP simulado en 127.0.0.1 para Graph, Gmail y el servicio de tokens. Todos los
//! datos son sintéticos (dominio `.test`) y ninguna prueba sale a la red.

#![allow(dead_code)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use nestadmin_ai::MemoryCredentialStore;
use nestadmin_mail::{OAuthConfig, TokenSource, Tokens};
use serde_json::Value;
use url::Url;

#[derive(Debug, Clone)]
pub struct Req {
    pub method: String,
    pub url: Url,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Req {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    pub fn query(&self, name: &str) -> Option<String> {
        self.url
            .query_pairs()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.into_owned())
    }

    pub fn form(&self, name: &str) -> Option<String> {
        url::form_urlencoded::parse(self.body.as_bytes())
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.into_owned())
    }

    pub fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or(Value::Null)
    }
}

pub enum Resp {
    Json(u16, Value),
    Empty(u16),
    /// Cierra la conexión sin responder: simula una respuesta perdida.
    Drop,
}

/// Estado común del proveedor simulado.
#[derive(Default)]
pub struct World {
    pub base: String,
    pub requests: Vec<Req>,
    pub access_token: String,
    pub refresh_ok: bool,
    pub issued: u32,
    pub code_verifier: Option<String>,
    pub messages: Vec<Value>,
    /// (ID del borrador, ID de su mensaje, mensaje)
    pub drafts: Vec<(String, String, Value)>,
    pub sent: Vec<String>,
    /// El envío ocurre pero la respuesta se pierde.
    pub drop_send_response: bool,
    /// El envío falla antes de producir efecto.
    pub send_status: Option<u16>,
    /// Gmail: el mensaje enviado conserva el ID del mensaje del borrador. Por defecto no,
    /// como se observó con una cuenta real (T13).
    pub keep_sent_id: bool,
    /// Estado forzado para cualquier ruta que contenga el texto.
    pub force: Option<(String, u16)>,
}

pub type Shared = Arc<Mutex<World>>;
pub type Handler = fn(&Req, &mut World) -> Resp;

pub fn serve(handler: Handler) -> (String, Shared) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let world = Arc::new(Mutex::new(World {
        base: base.clone(),
        access_token: "access-0".into(),
        refresh_ok: true,
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
    Some(Req {
        method,
        url: Url::parse(&format!("{base}{target}")).ok()?,
        headers,
        body: String::from_utf8_lossy(&body).into_owned(),
    })
}

fn handle(mut stream: TcpStream, world: &Shared, handler: Handler) {
    let base = world.lock().unwrap().base.clone();
    let Some(req) = read(&stream, &base) else {
        return;
    };
    let resp = {
        let mut w = world.lock().unwrap();
        w.requests.push(req.clone());
        let forced = w
            .force
            .as_ref()
            .filter(|(path, _)| req.url.path().contains(path.as_str()))
            .map(|(_, status)| *status);
        match forced {
            Some(status) => Resp::Json(
                status,
                serde_json::json!({ "error": { "code": "Forzado" } }),
            ),
            None if req.url.path() == "/token" => token(&req, &mut w),
            None if req.header("authorization") != Some(&format!("Bearer {}", w.access_token)) => {
                Resp::Json(
                    401,
                    serde_json::json!({ "error": { "code": "InvalidAuthenticationToken" } }),
                )
            }
            None => handler(&req, &mut w),
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

fn token(req: &Req, w: &mut World) -> Resp {
    let issue = |w: &mut World| {
        w.issued += 1;
        w.access_token = format!("access-{}", w.issued);
        Resp::Json(
            200,
            serde_json::json!({
                "access_token": w.access_token,
                "refresh_token": format!("refresh-{}", w.issued),
                "expires_in": 3600,
            }),
        )
    };
    match req.form("grant_type").as_deref() {
        Some("refresh_token") if w.refresh_ok => issue(w),
        Some("authorization_code") if req.form("code").as_deref() == Some("codigo-sintetico") => {
            w.code_verifier = req.form("code_verifier");
            issue(w)
        }
        _ => Resp::Json(400, serde_json::json!({ "error": "invalid_grant" })),
    }
}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

pub fn oauth(base: &str) -> OAuthConfig {
    OAuthConfig {
        auth_url: format!("{base}/authorize"),
        token_url: format!("{base}/token"),
        client_id: "cliente-sintetico".into(),
        client_secret: None,
        scopes: vec!["Mail.ReadWrite".into()],
        extra_auth_params: Vec::new(),
        redirect_host: "127.0.0.1".into(),
    }
}

/// Fuente de tokens con una sesión ya iniciada; `expires_at` decide si hay que renovar.
pub fn tokens(base: &str, expires_at: u64) -> (TokenSource, Arc<MemoryCredentialStore>) {
    let store = Arc::new(MemoryCredentialStore::default());
    let source = TokenSource::new(oauth(base), store.clone(), "cuenta-prueba");
    source
        .save(&Tokens {
            access_token: "access-0".into(),
            refresh_token: Some("refresh-0".into()),
            expires_at,
        })
        .unwrap();
    (source, store)
}

pub fn count(world: &Shared, method: &str, path_end: &str) -> usize {
    world
        .lock()
        .unwrap()
        .requests
        .iter()
        .filter(|r| r.method == method && r.url.path().ends_with(path_end))
        .count()
}

pub fn oauth_localhost(base: &str) -> OAuthConfig {
    OAuthConfig {
        redirect_host: "localhost".into(),
        ..oauth(base)
    }
}
