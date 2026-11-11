//! Prueba manual del conector de Gmail con una cuenta de prueba real (T13, segunda parte).
//! No forma parte de la aplicación ni de `cargo test`: la ejecuta una persona en su terminal.
//!
//! ```text
//! cargo run -p nestadmin-mail --example probar_gmail -- entrar <client_secret.json>
//! cargo run -p nestadmin-mail --example probar_gmail -- leer [--mostrar]
//! cargo run -p nestadmin-mail --example probar_gmail -- buscar <texto del asunto>
//! cargo run -p nestadmin-mail --example probar_gmail -- borrador <id_mensaje> <para>
//! cargo run -p nestadmin-mail --example probar_gmail -- enviar <id_borrador> --confirmar
//! cargo run -p nestadmin-mail --example probar_gmail -- cortar-envio <id_borrador> --confirmar
//! cargo run -p nestadmin-mail --example probar_gmail -- salir
//! ```
//!
//! `entrar` acepta el JSON que descarga Google Cloud o el `correo-oauth.json` de la aplicación.
//! `cortar-envio` envía de verdad y termina el proceso antes de anotar el recibo; después,
//! `enviar` con el mismo borrador debe confirmarlo por el `Message-ID` propio (D41) sin reenviar.
//!
//! Los tokens se guardan en el administrador de credenciales de Windows (`mail/prueba-gmail`)
//! y el registro local de borradores y el cliente OAuth en `%LOCALAPPDATA%\NestAdmin-pruebas`,
//! fuera del repositorio y de los datos de NestAdmin. Sin `--mostrar` no se imprime nada del
//! contenido del correo: solo recuentos y qué campos llegaron.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use nestadmin_ai::CredentialStore;
use nestadmin_core::ConnectorError;
use nestadmin_mail::gmail::DEFAULT_BASE;
use nestadmin_mail::{
    CreatedDraft, DraftStatus, Gmail, MailProvider, Mailbox, OAuthConfig, PendingAuthorization,
    TokenSource,
};
use nestadmin_simulators::{MailConnector, Message, MessagePage, NewDraft};
use serde_json::{json, Value};

const ACCOUNT: &str = "prueba-gmail";
const CLIENT_FILE: &str = "cliente-google.json";
const STATE_FILE: &str = "gmail-prueba.json";
/// Código de salida de `cortar-envio`: el envío se hizo y el recibo no se anotó.
const CUT_EXIT: i32 = 3;

type Result<T> = std::result::Result<T, String>;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["entrar", client] => sign_in(Path::new(client)),
        ["leer"] => read(false),
        ["leer", "--mostrar"] => read(true),
        ["buscar", subject] => find(subject),
        ["borrador", message_id, to] => draft(message_id, to),
        ["enviar", draft_id, "--confirmar"] => send(draft_id),
        ["enviar", _] => Err("enviar manda un correo real: repita la orden con --confirmar".into()),
        ["cortar-envio", draft_id, "--confirmar"] => send_and_cut(draft_id),
        ["cortar-envio", _] => {
            Err("cortar-envio manda un correo real: repita la orden con --confirmar".into())
        }
        ["salir"] => sign_out(),
        _ => Err("uso: entrar <client_secret.json> | leer [--mostrar] | buscar <asunto> | borrador <id_mensaje> <para> | enviar <id_borrador> --confirmar | cortar-envio <id_borrador> --confirmar | salir".into()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("Error: {message}");
            ExitCode::FAILURE
        }
    }
}

fn work_dir() -> Result<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let dir = base.join("NestAdmin-pruebas");
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("no se pudo crear {}: {e}", dir.display()))?;
    Ok(dir)
}

#[cfg(windows)]
fn store() -> Result<Arc<dyn CredentialStore>> {
    nestadmin_ai::credentials::WindowsCredentialStore::new()
        .map(|s| Arc::new(s) as Arc<dyn CredentialStore>)
        .map_err(|e| format!("administrador de credenciales no disponible: {e}"))
}

#[cfg(not(windows))]
fn store() -> Result<Arc<dyn CredentialStore>> {
    Err("esta prueba guarda los tokens en el administrador de credenciales de Windows".into())
}

/// El JSON que descarga Google Cloud para un cliente de tipo «Aplicación de escritorio», o el
/// `correo-oauth.json` de la aplicación.
fn client_from_download(path: &Path) -> Result<(String, String)> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("no se pudo leer {}: {e}", path.display()))?;
    let value: Value = serde_json::from_str(&text).map_err(|e| format!("JSON no válido: {e}"))?;
    let installed = value
        .get("installed")
        .or_else(|| value.get("google"))
        .ok_or(
            "el archivo no es de un cliente de escritorio de Google (falta «installed» o «google»)",
        )?;
    let field = |name: &str| {
        installed[name]
            .as_str()
            .map(str::to_owned)
            .ok_or(format!("falta «{name}» en el archivo del cliente"))
    };
    Ok((field("client_id")?, field("client_secret")?))
}

fn oauth_config() -> Result<OAuthConfig> {
    let path = work_dir()?.join(CLIENT_FILE);
    let text = std::fs::read_to_string(&path).map_err(|_| {
        "no hay sesión de prueba: ejecute primero «entrar <client_secret.json>»".to_owned()
    })?;
    let value: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let id = value["client_id"].as_str().ok_or("cliente sin client_id")?;
    let secret = value["client_secret"]
        .as_str()
        .ok_or("cliente sin client_secret")?;
    Ok(OAuthConfig::google(id, secret))
}

fn mailbox() -> Result<Mailbox<Gmail>> {
    Mailbox::open(gmail()?, &work_dir()?.join(STATE_FILE)).map_err(|e| e.message)
}

fn gmail() -> Result<Gmail> {
    let tokens = TokenSource::new(oauth_config()?, store()?, ACCOUNT);
    if !tokens.signed_in().map_err(|e| e.message)? {
        return Err(
            "no hay sesión de prueba: ejecute primero «entrar <client_secret.json>»".into(),
        );
    }
    Gmail::new(DEFAULT_BASE, tokens, 10).map_err(|e| e.message)
}

/// Gmail real que termina el proceso justo después de enviar, antes de anotar el recibo.
struct CutAfterSend(Gmail);

impl MailProvider for CutAfterSend {
    fn list(&self, cursor: Option<&str>) -> std::result::Result<MessagePage, ConnectorError> {
        self.0.list(cursor)
    }

    fn create_reply(
        &self,
        message_id: &str,
        draft: &NewDraft,
    ) -> std::result::Result<CreatedDraft, ConnectorError> {
        self.0.create_reply(message_id, draft)
    }

    fn send(&self, draft_id: &str) -> std::result::Result<String, ConnectorError> {
        self.0.send(draft_id)?;
        println!("Enviado. Se corta el proceso sin anotar el recibo.");
        std::process::exit(CUT_EXIT);
    }

    fn status(&self, draft: &CreatedDraft) -> std::result::Result<DraftStatus, ConnectorError> {
        self.0.status(draft)
    }

    fn attachment(
        &self,
        message_id: &str,
        attachment_id: &str,
    ) -> std::result::Result<Vec<u8>, ConnectorError> {
        self.0.attachment(message_id, attachment_id)
    }

    fn supports_sent(&self) -> bool {
        self.0.supports_sent()
    }

    fn own_reply_in_thread(
        &self,
        thread_key: &str,
        after_id: &str,
    ) -> std::result::Result<Option<nestadmin_simulators::OwnReply>, ConnectorError> {
        self.0.own_reply_in_thread(thread_key, after_id)
    }
}

/// Usa la misma clave que `send`, de modo que `enviar` retome este envío.
fn send_and_cut(draft_id: &str) -> Result<()> {
    let mailbox = Mailbox::open(CutAfterSend(gmail()?), &work_dir()?.join(STATE_FILE))
        .map_err(|e| e.message)?;
    mailbox
        .send_draft(draft_id, &format!("prueba-envio-{draft_id}"))
        .map_err(|e| format!("{}: {}", e.code, e.message))?;
    Err("el envío terminó sin pasar por el corte".into())
}

fn open_browser(url: &str) {
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("rundll32")
            .args(["url.dll,FileProtocolHandler", url])
            .spawn();
    }
    #[cfg(not(windows))]
    let _ = url;
}

fn sign_in(client: &Path) -> Result<()> {
    let (id, secret) = client_from_download(client)?;
    let saved = work_dir()?.join(CLIENT_FILE);
    let body = json!({ "client_id": id, "client_secret": secret });
    std::fs::write(
        &saved,
        serde_json::to_vec_pretty(&body).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let config = OAuthConfig::google(&id, &secret);
    let pending = PendingAuthorization::begin(&config).map_err(|e| e.message)?;
    println!(
        "Se abre el navegador para iniciar sesión en Google. Si no se abre, copie esta dirección:"
    );
    println!("{}", pending.url());
    println!("Esperando la autorización (5 minutos como máximo)…");
    open_browser(pending.url());
    let tokens = pending
        .finish(&config, Duration::from_secs(300))
        .map_err(|e| format!("{}: {}", e.code, e.message))?;
    TokenSource::new(config, store()?, ACCOUNT)
        .save(&tokens)
        .map_err(|e| e.message)?;
    println!(
        "Sesión iniciada. Tokens guardados en el administrador de credenciales (mail/{ACCOUNT})."
    );
    Ok(())
}

fn present(message: &Message) -> String {
    let mut fields = Vec::new();
    for (name, ok) in [
        ("asunto", message.subject.is_some()),
        ("de", message.from.is_some()),
        ("para", message.to.is_some()),
        ("fecha", message.date.is_some()),
        ("cuerpo", message.body.is_some()),
    ] {
        fields.push(if ok {
            name.to_owned()
        } else {
            format!("sin {name}")
        });
    }
    fields.join(", ")
}

fn short(text: &Option<String>, max: usize) -> String {
    match text {
        None => "(no consta)".into(),
        Some(t) if t.chars().count() > max => {
            format!("{}…", t.chars().take(max).collect::<String>())
        }
        Some(t) => t.clone(),
    }
}

fn read(show: bool) -> Result<()> {
    let mailbox = mailbox()?;
    let page = mailbox
        .list_messages(None)
        .map_err(|e| format!("{}: {}", e.code, e.message))?;
    println!(
        "Bandeja de entrada: {} mensajes en la primera página; {} más páginas.",
        page.messages.len(),
        if page.next_cursor.is_some() {
            "hay"
        } else {
            "no hay"
        }
    );
    if let Some(cursor) = &page.next_cursor {
        let next = mailbox
            .list_messages(Some(cursor))
            .map_err(|e| format!("{}: {}", e.code, e.message))?;
        println!("Segunda página: {} mensajes.", next.messages.len());
    }
    for message in &page.messages {
        let body = message.body.as_ref().map_or(0, |b| b.chars().count());
        println!(
            "- {} · {} · cuerpo de {body} caracteres",
            message.id,
            present(message)
        );
        if show {
            println!(
                "    {} · {} · {}",
                short(&message.date, 20),
                short(&message.from, 40),
                short(&message.subject, 60)
            );
        }
    }
    Ok(())
}

/// Solo imprime el ID y la fecha de los mensajes cuyo asunto contiene el texto, para
/// localizar el mensaje de prueba sin mostrar el resto de la bandeja.
fn find(subject: &str) -> Result<()> {
    let mailbox = mailbox()?;
    let needle = subject.to_lowercase();
    let mut cursor = None;
    let mut found = 0;
    for _ in 0..5 {
        let page = mailbox
            .list_messages(cursor.as_deref())
            .map_err(|e| format!("{}: {}", e.code, e.message))?;
        for message in &page.messages {
            let matches = message
                .subject
                .as_ref()
                .is_some_and(|s| s.to_lowercase().contains(&needle));
            if matches {
                found += 1;
                println!("- {} · {}", message.id, short(&message.date, 30));
            }
        }
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    println!("{found} mensajes con «{subject}» en el asunto.");
    Ok(())
}

fn draft(message_id: &str, to: &str) -> Result<()> {
    let mailbox = mailbox()?;
    let content = NewDraft {
        to: vec![to.to_owned()],
        subject: Some("Prueba de NestAdmin".into()),
        body: "Borrador de prueba creado por el conector de Gmail de NestAdmin. Puede borrarlo."
            .into(),
        attachments: Vec::new(),
    };
    let key = format!("prueba-borrador-{message_id}");
    let first = mailbox
        .create_draft(message_id, &content, &key)
        .map_err(|e| format!("{}: {}", e.code, e.message))?;
    let again = mailbox
        .create_draft(message_id, &content, &key)
        .map_err(|e| format!("{}: {}", e.code, e.message))?;
    println!("Borrador creado en el hilo del mensaje {message_id}: {first}");
    println!(
        "Repetir la misma clave devuelve {again}: {}",
        if first == again {
            "el mismo borrador (idempotente)"
        } else {
            "OTRO borrador (fallo)"
        }
    );
    println!("Compruébelo en Gmail → Borradores. Para enviarlo: enviar {first} --confirmar");
    Ok(())
}

fn send(draft_id: &str) -> Result<()> {
    let mailbox = mailbox()?;
    let key = format!("prueba-envio-{draft_id}");
    let first = mailbox
        .send_draft(draft_id, &key)
        .map_err(|e| format!("{}: {}", e.code, e.message))?;
    let again = mailbox
        .send_draft(draft_id, &key)
        .map_err(|e| format!("{}: {}", e.code, e.message))?;
    // `receipt_id` es el mensaje enviado; `message_id`, el mensaje al que se responde.
    println!(
        "Enviado: mensaje {} (respuesta a {})",
        first.receipt_id, first.message_id
    );
    println!(
        "Repetir la misma clave devuelve {}: {}",
        again.receipt_id,
        if first == again {
            "el mismo recibo, sin segundo envío"
        } else {
            "OTRO recibo (fallo)"
        }
    );
    Ok(())
}

fn sign_out() -> Result<()> {
    let tokens = TokenSource::new(oauth_config()?, store()?, ACCOUNT);
    tokens.sign_out().map_err(|e| e.message)?;
    println!("Tokens borrados del administrador de credenciales. Puede revocar el acceso en myaccount.google.com/permissions.");
    Ok(())
}
