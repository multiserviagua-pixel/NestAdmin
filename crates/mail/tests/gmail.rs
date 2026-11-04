//! T13 (Gmail): lista con mensajes completos, respuesta en el hilo, envío único con
//! reconciliación por etiqueta `SENT` e inicio de sesión OAuth con PKCE y `state`.

mod common;

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use base64::engine::general_purpose::{STANDARD, URL_SAFE};
use base64::Engine;
use common::{count, now, oauth, serve, tokens, Req, Resp, World};
use nestadmin_ai::MemoryCredentialStore;
use nestadmin_core::Connector;
use nestadmin_mail::oauth::pkce_challenge;
use nestadmin_mail::{Gmail, Mailbox, PendingAuthorization, TokenSource};
use nestadmin_simulators::{MailConnector, NewDraft};
use serde_json::{json, Value};
use url::Url;

const BASE: &str = "/gmail/v1/users/me/";

fn not_found() -> Resp {
    Resp::Json(404, json!({ "error": { "status": "NOT_FOUND" } }))
}

fn handler(req: &Req, w: &mut World) -> Resp {
    let path = req.url.path().trim_start_matches(BASE);
    let segments: Vec<&str> = path.split('/').collect();
    match (req.method.as_str(), segments.as_slice()) {
        ("GET", ["messages"]) => {
            assert_eq!(req.query("labelIds").as_deref(), Some("INBOX"));
            if req.query("pageToken").as_deref() == Some("p2") {
                Resp::Json(200, json!({ "messages": [{ "id": "g-2" }] }))
            } else {
                Resp::Json(
                    200,
                    json!({ "messages": [{ "id": "g-1" }], "nextPageToken": "p2" }),
                )
            }
        }
        ("GET", ["messages", id]) => {
            if let Some(m) = w.messages.iter().find(|m| m["id"] == *id) {
                return Resp::Json(200, m.clone());
            }
            match w.drafts.iter().find(|d| d.1 == *id) {
                Some(d) => Resp::Json(200, json!({ "id": id, "labelIds": d.2["labelIds"] })),
                None if w.sent.iter().any(|s| s == id) => {
                    Resp::Json(200, json!({ "id": id, "labelIds": ["SENT"] }))
                }
                None => not_found(),
            }
        }
        ("POST", ["drafts"]) => {
            let n = w.drafts.len() + 1;
            let (draft_id, message_id) = (format!("r-{n}"), format!("dm-{n}"));
            let mut stored = req.json()["message"].clone();
            stored["labelIds"] = json!(["DRAFT"]);
            w.drafts
                .push((draft_id.clone(), message_id.clone(), stored.clone()));
            Resp::Json(
                200,
                json!({ "id": draft_id, "message": { "id": message_id, "threadId": stored["threadId"] } }),
            )
        }
        ("POST", ["drafts", "send"]) => {
            if let Some(status) = w.send_status {
                return Resp::Json(status, json!({ "error": { "status": "UNAVAILABLE" } }));
            }
            let id = req.json()["id"].as_str().unwrap_or("").to_owned();
            let Some(index) = w.drafts.iter().position(|d| d.0 == id) else {
                return not_found();
            };
            // Como Gmail, el borrador desaparece y su mensaje queda con la etiqueta SENT.
            let (_, message_id, _) = w.drafts.remove(index);
            w.sent.push(message_id.clone());
            if w.drop_send_response {
                Resp::Drop
            } else {
                Resp::Json(200, json!({ "id": message_id, "labelIds": ["SENT"] }))
            }
        }
        ("GET", ["drafts", id]) => match w.drafts.iter().find(|d| d.0 == *id) {
            Some(_) => Resp::Json(200, json!({ "id": id })),
            None => not_found(),
        },
        _ => not_found(),
    }
}

fn b64(text: &str) -> String {
    URL_SAFE.encode(text)
}

fn sample_messages() -> Vec<Value> {
    vec![
        json!({
            "id": "g-1",
            "threadId": "t-1",
            "internalDate": "1793664000000",
            "payload": {
                "mimeType": "text/plain",
                "headers": [
                    { "name": "Subject", "value": "Pedido 2026-77" },
                    { "name": "From", "value": "\"Compras, Beta\" <compras@beta.test>" },
                    { "name": "To", "value": "Admin <admin@empresa.test>, otra@empresa.test" },
                    { "name": "Message-ID", "value": "<pedido-77@beta.test>" },
                ],
                "body": { "data": b64("Necesitamos la factura del pedido.") },
            },
        }),
        json!({
            "id": "g-2",
            "payload": { "mimeType": "text/html", "headers": [], "body": { "data": b64("<div>Solo HTML</div>") } },
        }),
    ]
}

fn mailbox() -> (Mailbox<Gmail>, common::Shared) {
    let (base, world) = serve(handler);
    world.lock().unwrap().messages = sample_messages();
    let (source, _) = tokens(&base, now() + 3600);
    let gmail = Gmail::new(&format!("{base}{BASE}"), source, 10).unwrap();
    (Mailbox::in_memory(gmail), world)
}

fn draft() -> NewDraft {
    NewDraft {
        to: vec!["compras@beta.test".into()],
        subject: None,
        body: "Se la enviamos hoy. Un saludo.".into(),
    }
}

#[test]
fn inbox_pages_return_full_normalized_messages() {
    let (mailbox, world) = mailbox();
    let first = mailbox.list_messages(None).unwrap();
    let m = &first.messages[0];
    assert_eq!(m.subject.as_deref(), Some("Pedido 2026-77"));
    assert_eq!(m.from.as_deref(), Some("compras@beta.test"));
    assert_eq!(
        m.to,
        Some(vec![
            "admin@empresa.test".to_owned(),
            "otra@empresa.test".to_owned()
        ])
    );
    assert_eq!(
        m.body.as_deref(),
        Some("Necesitamos la factura del pedido.")
    );
    assert_eq!(m.date.as_deref(), Some("2026-11-03T00:00:00Z"));
    assert_eq!(first.next_cursor.as_deref(), Some("p2"));

    let second = mailbox.list_messages(first.next_cursor.as_deref()).unwrap();
    let m2 = &second.messages[0];
    assert_eq!(m2.body.as_deref(), Some("Solo HTML"));
    assert_eq!(
        (
            m2.subject.as_ref(),
            m2.from.as_ref(),
            m2.to.as_ref(),
            m2.date.as_ref()
        ),
        (None, None, None, None)
    );
    assert_eq!(second.next_cursor, None);
    let w = world.lock().unwrap();
    assert!(w
        .requests
        .iter()
        .any(|r| r.query("format").as_deref() == Some("full")));
}

#[test]
fn reply_is_created_in_the_original_thread() {
    let (mailbox, world) = mailbox();
    let id = mailbox.create_draft("g-1", &draft(), "k-draft").unwrap();
    assert_eq!(id, "r-1");
    assert_eq!(
        mailbox.create_draft("g-1", &draft(), "k-draft").unwrap(),
        "r-1"
    );
    assert_eq!(count(&world, "POST", "/drafts"), 1);

    let w = world.lock().unwrap();
    let stored = &w.drafts[0].2;
    assert_eq!(stored["threadId"], "t-1");
    let raw = String::from_utf8(URL_SAFE.decode(stored["raw"].as_str().unwrap()).unwrap()).unwrap();
    assert!(raw.contains("To: compras@beta.test\r\n"));
    assert!(raw.contains("Subject: Re: Pedido 2026-77\r\n"));
    assert!(raw.contains("In-Reply-To: <pedido-77@beta.test>\r\n"));
    let body = raw.split("\r\n\r\n").nth(1).unwrap().replace("\r\n", "");
    assert_eq!(
        String::from_utf8(STANDARD.decode(body).unwrap()).unwrap(),
        draft().body
    );
}

#[test]
fn draft_is_sent_once_even_if_the_response_is_lost() {
    let (mailbox, world) = mailbox();
    let id = mailbox.create_draft("g-1", &draft(), "k-draft").unwrap();
    world.lock().unwrap().drop_send_response = true;
    assert_eq!(
        mailbox.send_draft(&id, "k-send").unwrap_err().code,
        "unavailable"
    );

    // El borrador ya no existe y su mensaje tiene la etiqueta SENT: enviado, sin repetir.
    let receipt = mailbox.lookup("k-send").unwrap().unwrap();
    assert_eq!(receipt["receipt_id"], "dm-1");
    assert_eq!(receipt["message_id"], "g-1");
    assert_eq!(
        mailbox.send_draft(&id, "k-send").unwrap().receipt_id,
        "dm-1"
    );
    assert_eq!(world.lock().unwrap().sent, ["dm-1"]);
    assert_eq!(count(&world, "POST", "/drafts/send"), 1);
}

#[test]
fn normal_send_returns_the_sent_message_id() {
    let (mailbox, world) = mailbox();
    let id = mailbox.create_draft("g-1", &draft(), "k-draft").unwrap();
    let receipt = mailbox
        .execute("k-send", &json!({ "draft_id": id }))
        .unwrap();
    assert_eq!(receipt["receipt_id"], "dm-1");
    assert_eq!(mailbox.lookup("k-send").unwrap(), Some(receipt));
    assert_eq!(world.lock().unwrap().sent.len(), 1);
}

#[test]
fn draft_deleted_by_the_user_is_not_resent() {
    let (mailbox, world) = mailbox();
    let id = mailbox.create_draft("g-1", &draft(), "k-draft").unwrap();
    world.lock().unwrap().send_status = Some(503);
    let _ = mailbox.send_draft(&id, "k-send");
    {
        let mut w = world.lock().unwrap();
        w.send_status = None;
        w.drafts.clear();
    }
    assert_eq!(mailbox.lookup("k-send").unwrap_err().code, "not_found");
    assert_eq!(
        mailbox.send_draft(&id, "k-send").unwrap_err().code,
        "not_found"
    );
    assert!(world.lock().unwrap().sent.is_empty());
}

/// Hace de navegador: sigue la redirección al puerto local con la query indicada.
fn browser_redirect(redirect_uri: &str, query: &str) -> String {
    let url = Url::parse(redirect_uri).unwrap();
    let mut stream = TcpStream::connect((url.host_str().unwrap(), url.port().unwrap())).unwrap();
    write!(stream, "GET /?{query} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n").unwrap();
    let mut response = String::new();
    let _ = stream.read_to_string(&mut response);
    response
}

fn param(url: &str, name: &str) -> String {
    Url::parse(url)
        .unwrap()
        .query_pairs()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.into_owned())
        .unwrap()
}

#[test]
fn sign_in_uses_pkce_and_state_and_stores_tokens() {
    let (base, world) = serve(handler);
    let config = oauth(&base);
    let pending = PendingAuthorization::begin(&config).unwrap();
    let url = pending.url().to_owned();
    assert!(url.starts_with(&format!("{base}/authorize?")));
    assert_eq!(param(&url, "client_id"), "cliente-sintetico");
    assert_eq!(param(&url, "response_type"), "code");
    assert_eq!(param(&url, "code_challenge_method"), "S256");
    assert_eq!(param(&url, "redirect_uri"), pending.redirect_uri());
    assert!(pending.redirect_uri().starts_with("http://127.0.0.1:"));
    let (state, challenge) = (param(&url, "state"), param(&url, "code_challenge"));

    let redirect = pending.redirect_uri().to_owned();
    let browser = thread::spawn(move || {
        // El icono de la pestaña no interrumpe la espera.
        let icon = browser_redirect(&redirect, "favicon");
        assert!(icon.starts_with("HTTP/1.1 404"));
        browser_redirect(&redirect, &format!("code=codigo-sintetico&state={state}"))
    });
    let tokens = pending.finish(&config, Duration::from_secs(10)).unwrap();
    assert!(browser
        .join()
        .unwrap()
        .contains("Inicio de sesión completado"));
    assert_eq!(tokens.access_token, "access-1");
    assert_eq!(tokens.refresh_token.as_deref(), Some("refresh-1"));
    let verifier = world.lock().unwrap().code_verifier.clone().unwrap();
    assert_eq!(
        pkce_challenge(&verifier),
        challenge,
        "el verificador corresponde al reto"
    );

    let store = Arc::new(MemoryCredentialStore::default());
    let source = TokenSource::new(config, store, "cuenta-gmail");
    assert!(!source.signed_in().unwrap());
    source.save(&tokens).unwrap();
    assert!(source.signed_in().unwrap());
    source.sign_out().unwrap();
    assert!(!source.signed_in().unwrap());
}

#[test]
fn sign_in_with_wrong_state_or_denied_consent_fails() {
    let (base, world) = serve(handler);
    let config = oauth(&base);
    for query in ["code=codigo-sintetico&state=otro", "error=access_denied"] {
        let pending = PendingAuthorization::begin(&config).unwrap();
        let redirect = pending.redirect_uri().to_owned();
        let state = param(pending.url(), "state");
        let query = if query.starts_with("error") {
            format!("{query}&state={state}")
        } else {
            query.to_owned()
        };
        let sent = query.clone();
        let browser = thread::spawn(move || browser_redirect(&redirect, &sent));
        let error = pending
            .finish(&config, Duration::from_secs(10))
            .unwrap_err();
        browser.join().unwrap();
        assert_eq!(error.code, "unauthorized", "{query}");
    }
    assert_eq!(
        count(&world, "POST", "/token"),
        0,
        "no se canjea ningún código"
    );
}

#[test]
fn sign_in_times_out_when_the_browser_never_returns() {
    let (base, _) = serve(handler);
    let config = oauth(&base);
    let pending = PendingAuthorization::begin(&config).unwrap();
    let error = pending
        .finish(&config, Duration::from_millis(300))
        .unwrap_err();
    assert_eq!(error.code, "unauthorized");
}
