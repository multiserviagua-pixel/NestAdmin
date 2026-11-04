//! OAuth de escritorio (D31): código de autorización con PKCE y redirección a un puerto libre
//! de 127.0.0.1. Los tokens se guardan en el almacén de credenciales y nunca aparecen en
//! `Debug`, errores ni registros.

use std::fmt;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use nestadmin_ai::{CredentialStore, Secret};
use nestadmin_simulators::codes;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use ureq::Agent;
use url::Url;

use crate::{checked_url, error, MailResult};

/// Margen antes de la caducidad en el que el token ya se renueva.
const EXPIRY_MARGIN_SECS: u64 = 60;

#[derive(Clone, PartialEq, Eq)]
pub struct OAuthConfig {
    pub auth_url: String,
    pub token_url: String,
    pub client_id: String,
    /// Google lo exige incluso a las aplicaciones instaladas; no se trata como secreto.
    pub client_secret: Option<String>,
    pub scopes: Vec<String>,
    pub extra_auth_params: Vec<(String, String)>,
}

impl fmt::Debug for OAuthConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OAuthConfig")
            .field("auth_url", &self.auth_url)
            .field("token_url", &self.token_url)
            .field("client_id", &self.client_id)
            .field("scopes", &self.scopes)
            .finish_non_exhaustive()
    }
}

impl OAuthConfig {
    /// Microsoft 365 y cuentas personales de Outlook (inquilino `common`).
    pub fn microsoft(client_id: &str) -> Self {
        Self {
            auth_url: "https://login.microsoftonline.com/common/oauth2/v2.0/authorize".into(),
            token_url: "https://login.microsoftonline.com/common/oauth2/v2.0/token".into(),
            client_id: client_id.to_owned(),
            client_secret: None,
            scopes: ["offline_access", "Mail.ReadWrite", "Mail.Send"]
                .map(String::from)
                .to_vec(),
            extra_auth_params: Vec::new(),
        }
    }

    /// Gmail y Google Workspace. `access_type=offline` y `prompt=consent` garantizan un
    /// token de renovación.
    pub fn google(client_id: &str, client_secret: &str) -> Self {
        Self {
            auth_url: "https://accounts.google.com/o/oauth2/v2/auth".into(),
            token_url: "https://oauth2.googleapis.com/token".into(),
            client_id: client_id.to_owned(),
            client_secret: Some(client_secret.to_owned()),
            scopes: vec!["https://www.googleapis.com/auth/gmail.modify".into()],
            extra_auth_params: vec![
                ("access_type".into(), "offline".into()),
                ("prompt".into(), "consent".into()),
            ],
        }
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
    /// Segundos desde 1970 en que caduca el token de acceso.
    pub expires_at: u64,
}

impl fmt::Debug for Tokens {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Tokens")
            .field("access_token", &"***")
            .field("refresh_token", &self.refresh_token.as_ref().map(|_| "***"))
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn random_token(bytes: usize) -> MailResult<String> {
    let mut buffer = vec![0u8; bytes];
    getrandom::fill(&mut buffer)
        .map_err(|_| error(codes::UNAVAILABLE, "no hay fuente de aleatoriedad"))?;
    Ok(URL_SAFE_NO_PAD.encode(buffer))
}

/// Reto PKCE S256 (RFC 7636).
pub fn pkce_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// Sin proxy: las peticiones de token llevan credenciales (D12, D32).
pub(crate) fn agent() -> Agent {
    Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(60)))
        .timeout_connect(Some(Duration::from_secs(10)))
        .http_status_as_error(false)
        .proxy(None)
        .build()
        .into()
}

/// Inicio de sesión en curso: la URL que hay que abrir en el navegador y el puerto local
/// que espera la redirección.
pub struct PendingAuthorization {
    url: String,
    redirect_uri: String,
    state: String,
    verifier: String,
    listener: TcpListener,
}

impl fmt::Debug for PendingAuthorization {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PendingAuthorization")
            .field("redirect_uri", &self.redirect_uri)
            .finish_non_exhaustive()
    }
}

impl PendingAuthorization {
    pub fn begin(config: &OAuthConfig) -> MailResult<Self> {
        let mut auth = checked_url(&config.auth_url)?;
        checked_url(&config.token_url)?;
        let listener = TcpListener::bind("127.0.0.1:0")
            .map_err(|e| error(codes::UNAVAILABLE, format!("sin puerto local libre: {e}")))?;
        let port = listener
            .local_addr()
            .map_err(|e| error(codes::UNAVAILABLE, e.to_string()))?
            .port();
        let redirect_uri = format!("http://127.0.0.1:{port}/");
        let state = random_token(24)?;
        let verifier = random_token(48)?;
        {
            let mut query = auth.query_pairs_mut();
            query
                .append_pair("client_id", &config.client_id)
                .append_pair("response_type", "code")
                .append_pair("redirect_uri", &redirect_uri)
                .append_pair("scope", &config.scopes.join(" "))
                .append_pair("state", &state)
                .append_pair("code_challenge", &pkce_challenge(&verifier))
                .append_pair("code_challenge_method", "S256");
            for (key, value) in &config.extra_auth_params {
                query.append_pair(key, value);
            }
        }
        Ok(Self {
            url: auth.into(),
            redirect_uri,
            state,
            verifier,
            listener,
        })
    }

    /// URL que la aplicación abre en el navegador del usuario.
    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn redirect_uri(&self) -> &str {
        &self.redirect_uri
    }

    /// Espera la redirección del navegador y canjea el código por tokens.
    pub fn finish(self, config: &OAuthConfig, timeout: Duration) -> MailResult<Tokens> {
        let code = self.wait_for_code(timeout)?;
        exchange_code(config, &code, &self.redirect_uri, &self.verifier)
    }

    fn wait_for_code(&self, timeout: Duration) -> MailResult<String> {
        self.listener
            .set_nonblocking(true)
            .map_err(|e| error(codes::UNAVAILABLE, e.to_string()))?;
        let deadline = Instant::now() + timeout;
        loop {
            match self.listener.accept() {
                Ok((stream, _)) => {
                    if let Some(result) = self.handle_redirect(stream) {
                        return result;
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        return Err(error(
                            codes::UNAUTHORIZED,
                            "no se completó el inicio de sesión a tiempo",
                        ));
                    }
                    thread::sleep(Duration::from_millis(100));
                }
                Err(e) => return Err(error(codes::UNAVAILABLE, e.to_string())),
            }
        }
    }

    /// `None` para peticiones ajenas (por ejemplo, el icono de la pestaña): se sigue esperando.
    fn handle_redirect(&self, mut stream: TcpStream) -> Option<MailResult<String>> {
        let _ = stream.set_nonblocking(false);
        let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
        let mut line = String::new();
        BufReader::new(stream.try_clone().ok()?)
            .read_line(&mut line)
            .ok()?;
        let target = line.split_whitespace().nth(1)?;
        let url = Url::parse(&format!("http://127.0.0.1{target}")).ok()?;
        let param = |name: &str| {
            url.query_pairs()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.into_owned())
        };
        let (code, state, denied) = (param("code"), param("state"), param("error"));
        if code.is_none() && denied.is_none() {
            reply(&mut stream, "404 Not Found", "");
            return None;
        }
        if state.as_deref() != Some(self.state.as_str()) {
            reply(
                &mut stream,
                "400 Bad Request",
                "Respuesta de inicio de sesión no válida.",
            );
            return Some(Err(error(
                codes::UNAUTHORIZED,
                "la respuesta de inicio de sesión no corresponde a esta solicitud",
            )));
        }
        if let Some(denied) = denied {
            reply(
                &mut stream,
                "200 OK",
                "Inicio de sesión cancelado. Puede cerrar esta ventana.",
            );
            return Some(Err(error(
                codes::UNAUTHORIZED,
                format!("inicio de sesión rechazado: {denied}"),
            )));
        }
        reply(
            &mut stream,
            "200 OK",
            "Inicio de sesión completado. Puede cerrar esta ventana y volver a NestAdmin.",
        );
        code.map(Ok)
    }
}

fn reply(stream: &mut TcpStream, status: &str, text: &str) {
    let body =
        format!("<!doctype html><meta charset=\"utf-8\"><title>NestAdmin</title><p>{text}</p>");
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

fn exchange_code(
    config: &OAuthConfig,
    code: &str,
    redirect_uri: &str,
    verifier: &str,
) -> MailResult<Tokens> {
    let mut form = vec![
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", redirect_uri),
        ("client_id", config.client_id.as_str()),
        ("code_verifier", verifier),
    ];
    if let Some(secret) = &config.client_secret {
        form.push(("client_secret", secret));
    }
    token_request(config, &form, None)
}

fn refresh(config: &OAuthConfig, refresh_token: &str) -> MailResult<Tokens> {
    let mut form = vec![
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("client_id", config.client_id.as_str()),
    ];
    if let Some(secret) = &config.client_secret {
        form.push(("client_secret", secret));
    }
    token_request(config, &form, Some(refresh_token))
}

/// Si la respuesta de renovación no trae un token de renovación nuevo, se conserva el anterior.
fn token_request(
    config: &OAuthConfig,
    form: &[(&str, &str)],
    previous_refresh: Option<&str>,
) -> MailResult<Tokens> {
    let url = checked_url(&config.token_url)?;
    let mut response = agent()
        .post(url.as_str())
        .send_form(form.iter().copied())
        .map_err(|_| {
            error(
                codes::UNAVAILABLE,
                "no se pudo contactar con el servicio de inicio de sesión",
            )
        })?;
    let status = response.status().as_u16();
    let body: Value = response.body_mut().read_json().unwrap_or(Value::Null);
    match status {
        200..=299 => {}
        400 | 401 => {
            let reason = body
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("rechazado");
            return Err(error(
                codes::UNAUTHORIZED,
                format!("el servicio de inicio de sesión rechazó la solicitud ({reason}); vuelva a iniciar sesión"),
            ));
        }
        _ => {
            return Err(error(
                codes::UNAVAILABLE,
                format!("el servicio de inicio de sesión devolvió el estado HTTP {status}"),
            ))
        }
    }
    let access_token = body
        .get("access_token")
        .and_then(Value::as_str)
        .ok_or_else(|| error(codes::UNAVAILABLE, "respuesta de token sin access_token"))?;
    let expires_in = body
        .get("expires_in")
        .and_then(Value::as_u64)
        .unwrap_or(3600);
    Ok(Tokens {
        access_token: access_token.to_owned(),
        refresh_token: body
            .get("refresh_token")
            .and_then(Value::as_str)
            .or(previous_refresh)
            .map(str::to_owned),
        expires_at: now_secs() + expires_in,
    })
}

/// Tokens de una cuenta: los lee del almacén, los renueva cuando hace falta y guarda el
/// resultado. Un fallo de renovación es `unauthorized`: hay que volver a iniciar sesión.
pub struct TokenSource {
    config: OAuthConfig,
    store: Arc<dyn CredentialStore>,
    name: String,
    cache: Mutex<Option<Tokens>>,
}

impl fmt::Debug for TokenSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TokenSource")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

impl TokenSource {
    /// `account` identifica la cuenta; el nombre en el almacén es `mail/<account>`.
    pub fn new(config: OAuthConfig, store: Arc<dyn CredentialStore>, account: &str) -> Self {
        Self {
            config,
            store,
            name: format!("mail/{account}"),
            cache: Mutex::new(None),
        }
    }

    pub fn config(&self) -> &OAuthConfig {
        &self.config
    }

    pub fn save(&self, tokens: &Tokens) -> MailResult<()> {
        let text =
            serde_json::to_string(tokens).map_err(|e| error(codes::UNAVAILABLE, e.to_string()))?;
        self.store
            .set(&self.name, &Secret::new(text))
            .map_err(|e| error(codes::UNAVAILABLE, e.to_string()))?;
        *self.cache.lock().map_err(|_| poisoned())? = Some(tokens.clone());
        Ok(())
    }

    pub fn signed_in(&self) -> MailResult<bool> {
        Ok(self.load()?.is_some())
    }

    pub fn sign_out(&self) -> MailResult<()> {
        *self.cache.lock().map_err(|_| poisoned())? = None;
        self.store
            .delete(&self.name)
            .map_err(|e| error(codes::UNAVAILABLE, e.to_string()))
    }

    fn load(&self) -> MailResult<Option<Tokens>> {
        if let Some(tokens) = self.cache.lock().map_err(|_| poisoned())?.clone() {
            return Ok(Some(tokens));
        }
        let stored = self
            .store
            .get(&self.name)
            .map_err(|e| error(codes::UNAVAILABLE, e.to_string()))?;
        Ok(stored.and_then(|secret| serde_json::from_str(secret.expose()).ok()))
    }

    /// Token de acceso vigente. Con `force_refresh` se renueva aunque no haya caducado, lo
    /// que se usa tras un 401 del proveedor.
    pub(crate) fn access_token(&self, force_refresh: bool) -> MailResult<Secret> {
        let tokens = self
            .load()?
            .ok_or_else(|| error(codes::UNAUTHORIZED, "la cuenta no ha iniciado sesión"))?;
        if !force_refresh && tokens.expires_at > now_secs() + EXPIRY_MARGIN_SECS {
            return Ok(Secret::new(tokens.access_token));
        }
        let refresh_token = tokens.refresh_token.ok_or_else(|| {
            error(
                codes::UNAUTHORIZED,
                "la sesión caducó; vuelva a iniciar sesión",
            )
        })?;
        let renewed = refresh(&self.config, &refresh_token)?;
        self.save(&renewed)?;
        Ok(Secret::new(renewed.access_token))
    }
}

fn poisoned() -> nestadmin_core::ConnectorError {
    error(codes::UNAVAILABLE, "estado interno no disponible")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_challenge_matches_rfc_7636_example() {
        assert_eq!(
            pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn secrets_are_not_printed() {
        let tokens = Tokens {
            access_token: "token-de-acceso-sintetico".into(),
            refresh_token: Some("token-de-renovacion-sintetico".into()),
            expires_at: 1,
        };
        let shown = format!("{tokens:?}");
        assert!(!shown.contains("sintetico"), "{shown}");
        let config = OAuthConfig::google("cliente.apps.test", "secreto-sintetico");
        assert!(!format!("{config:?}").contains("secreto-sintetico"));
    }
}
