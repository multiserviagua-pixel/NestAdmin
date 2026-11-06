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
        ("GET", ["messages"]) if req.query("q").is_some() => search_sent(req, w),
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
        ("GET", ["messages", id, "attachments", aid]) => {
            let data = w
                .messages
                .iter()
                .find(|m| m["id"] == *id)
                .and_then(|m| m["_attachments"][*aid].as_str().map(str::to_owned));
            match data {
                Some(data) => Resp::Json(200, json!({ "size": 9, "data": data })),
                None => not_found(),
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
            // Como Gmail, el borrador desaparece y queda un mensaje con la etiqueta SENT, la
            // misma cabecera Message-ID y, salvo `keep_sent_id`, otro ID.
            let (_, draft_message_id, stored) = w.drafts.remove(index);
            let sent_id = if w.keep_sent_id {
                draft_message_id
            } else {
                format!("s-{}", w.sent.len() + 1)
            };
            let mut headers = Vec::new();
            if let Some(own) = raw_header(&stored, "Message-ID") {
                headers.push(json!({ "name": "Message-ID", "value": own }));
            }
            w.messages.push(json!({
                "id": sent_id,
                "threadId": stored["threadId"],
                "labelIds": ["SENT"],
                "payload": { "mimeType": "text/plain", "headers": headers },
            }));
            w.sent.push(sent_id.clone());
            if w.drop_send_response {
                Resp::Drop
            } else {
                Resp::Json(200, json!({ "id": sent_id, "labelIds": ["SENT"] }))
            }
        }
        ("GET", ["drafts", id]) => match w.drafts.iter().find(|d| d.0 == *id) {
            Some(_) => Resp::Json(200, json!({ "id": id })),
            None => not_found(),
        },
        _ => not_found(),
    }
}

/// Cabecera del mensaje MIME guardado en un borrador.
fn raw_header(stored: &Value, name: &str) -> Option<String> {
    let raw = URL_SAFE.decode(stored["raw"].as_str()?).ok()?;
    let text = String::from_utf8(raw).ok()?;
    let head = text.split("\r\n\r\n").next()?;
    head.split("\r\n").find_map(|line| {
        let (n, v) = line.split_once(':')?;
        n.eq_ignore_ascii_case(name).then(|| v.trim().to_owned())
    })
}

/// `messages.list` con `q=rfc822msgid:...`: solo mensajes enviados con esa cabecera.
fn search_sent(req: &Req, w: &mut World) -> Resp {
    assert_eq!(req.query("labelIds").as_deref(), Some("SENT"));
    assert_eq!(req.query("includeSpamTrash").as_deref(), Some("true"));
    let q = req.query("q").unwrap();
    let wanted = q
        .strip_prefix("rfc822msgid:")
        .expect("búsqueda por Message-ID");
    let found: Vec<Value> = w
        .messages
        .iter()
        .filter(|m| {
            m["labelIds"]
                .as_array()
                .is_some_and(|l| l.contains(&json!("SENT")))
        })
        .filter(|m| {
            m.pointer("/payload/headers")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .any(|h| h["name"] == "Message-ID" && h["value"] == format!("<{wanted}>"))
        })
        .map(|m| json!({ "id": m["id"], "threadId": m["threadId"] }))
        .collect();
    if found.is_empty() {
        Resp::Json(200, json!({ "resultSizeEstimate": 0 }))
    } else {
        Resp::Json(200, json!({ "messages": found }))
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

fn gmail(base: &str) -> Gmail {
    let (source, _) = tokens(base, now() + 3600);
    Gmail::new(&format!("{base}{BASE}"), source, 10).unwrap()
}

fn mailbox() -> (Mailbox<Gmail>, common::Shared) {
    let (base, world) = serve(handler);
    world.lock().unwrap().messages = sample_messages();
    (Mailbox::in_memory(gmail(&base)), world)
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
    let own = raw_header(stored, "Message-ID").unwrap();
    assert!(own.starts_with("<nestadmin.") && own.ends_with("@nestadmin.invalid>"));
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

    // El borrador ya no existe y el mensaje enviado tiene otro ID, pero conserva el
    // Message-ID propio y la etiqueta SENT: enviado, sin repetir, con el ID real.
    let receipt = mailbox.lookup("k-send").unwrap().unwrap();
    assert_eq!(receipt["receipt_id"], "s-1");
    assert_eq!(receipt["message_id"], "g-1");
    assert_eq!(mailbox.send_draft(&id, "k-send").unwrap().receipt_id, "s-1");
    assert_eq!(world.lock().unwrap().sent, ["s-1"]);
    assert_eq!(count(&world, "POST", "/drafts/send"), 1);
    assert_eq!(
        count(&world, "GET", "/messages/dm-1"),
        1,
        "el ID del mensaje del borrador ya no sirve"
    );
}

#[test]
fn lost_send_is_confirmed_by_the_draft_message_id_if_gmail_keeps_it() {
    let (mailbox, world) = mailbox();
    let id = mailbox.create_draft("g-1", &draft(), "k-draft").unwrap();
    {
        let mut w = world.lock().unwrap();
        w.keep_sent_id = true;
        w.drop_send_response = true;
    }
    let _ = mailbox.send_draft(&id, "k-send");
    assert_eq!(
        mailbox.lookup("k-send").unwrap().unwrap()["receipt_id"],
        "dm-1"
    );
    assert_eq!(count(&world, "POST", "/drafts/send"), 1);
}

#[test]
fn lost_send_is_confirmed_after_reopening_the_connector() {
    let (base, world) = serve(handler);
    world.lock().unwrap().messages = sample_messages();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cuenta-gmail.json");
    let id = {
        let mailbox = Mailbox::open(gmail(&base), &path).unwrap();
        let id = mailbox.create_draft("g-1", &draft(), "k-draft").unwrap();
        world.lock().unwrap().drop_send_response = true;
        assert!(mailbox.send_draft(&id, "k-send").is_err());
        id
    };
    let saved = std::fs::read_to_string(&path).unwrap();
    assert!(
        saved.contains("@nestadmin.invalid>"),
        "el Message-ID propio se guarda"
    );

    // Proceso nuevo: solo el registro local recuerda el Message-ID del borrador.
    let mailbox = Mailbox::open(gmail(&base), &path).unwrap();
    let receipt = mailbox.lookup("k-send").unwrap().unwrap();
    assert_eq!(receipt["receipt_id"], "s-1");
    assert_eq!(mailbox.send_draft(&id, "k-send").unwrap().receipt_id, "s-1");
    drop(mailbox);
    let reopened = Mailbox::open(gmail(&base), &path).unwrap();
    assert_eq!(
        reopened.lookup("k-send").unwrap().unwrap()["receipt_id"],
        "s-1"
    );
    assert_eq!(count(&world, "POST", "/drafts/send"), 1);
    assert_eq!(world.lock().unwrap().sent, ["s-1"]);
}

#[test]
fn another_sent_message_in_the_thread_is_not_taken_as_ours() {
    let (mailbox, world) = mailbox();
    let id = mailbox.create_draft("g-1", &draft(), "k-draft").unwrap();
    world.lock().unwrap().send_status = Some(503);
    let _ = mailbox.send_draft(&id, "k-send");
    {
        // El usuario borra el borrador y responde por su cuenta en el mismo hilo.
        let mut w = world.lock().unwrap();
        w.send_status = None;
        w.drafts.clear();
        w.messages.push(json!({
            "id": "s-otro",
            "threadId": "t-1",
            "labelIds": ["SENT"],
            "payload": { "headers": [{ "name": "Message-ID", "value": "<otro@empresa.test>" }] },
        }));
    }
    assert_eq!(mailbox.lookup("k-send").unwrap_err().code, "not_found");
    assert!(world.lock().unwrap().sent.is_empty());
    assert_eq!(
        count(&world, "POST", "/drafts/send"),
        1,
        "solo el intento fallido"
    );
}

#[test]
fn normal_send_returns_the_sent_message_id() {
    let (mailbox, world) = mailbox();
    let id = mailbox.create_draft("g-1", &draft(), "k-draft").unwrap();
    let receipt = mailbox
        .execute("k-send", &json!({ "draft_id": id }))
        .unwrap();
    assert_eq!(receipt["receipt_id"], "s-1");
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
    let searches = world
        .lock()
        .unwrap()
        .requests
        .iter()
        .filter(|r| {
            r.query("q")
                .is_some_and(|q| q.starts_with("rfc822msgid:nestadmin."))
        })
        .count();
    assert_eq!(
        searches, 2,
        "se buscó el envío por su Message-ID antes de rendirse"
    );
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

#[test]
fn named_parts_are_attachments_except_embedded_images_and_download_decodes_them() {
    let (base, world) = serve(handler);
    let small = URL_SAFE.encode(b"Factura en texto");
    world.lock().unwrap().messages = vec![
        json!({
            "id": "g-1",
            "internalDate": "1793990000000",
            "payload": {
                "mimeType": "multipart/mixed",
                "headers": [{ "name": "Subject", "value": "Facturas" }],
                "parts": [
                    {
                        "partId": "0",
                        "mimeType": "multipart/related",
                        "parts": [
                            { "partId": "0.0", "mimeType": "text/html", "filename": "",
                              "body": { "data": URL_SAFE.encode(b"<p>Hola</p>") } },
                            { "partId": "0.1", "mimeType": "image/png", "filename": "logo.png",
                              "headers": [
                                  { "name": "Content-ID", "value": "<logo@x>" },
                                  { "name": "Content-Disposition", "value": "inline; filename=logo.png" }
                              ],
                              "body": { "attachmentId": "ATT-LOGO", "size": 3 } }
                        ]
                    },
                    { "partId": "1", "mimeType": "application/pdf", "filename": "f-1.pdf",
                      "headers": [{ "name": "Content-Disposition", "value": "attachment; filename=f-1.pdf" }],
                      "body": { "attachmentId": "ATT-1", "size": 9 } },
                    { "partId": "2", "mimeType": "text/plain", "filename": "f-2.txt",
                      "body": { "data": small, "size": 16 } }
                ]
            },
            "_attachments": { "ATT-1": URL_SAFE.encode(b"%PDF-sint") }
        }),
        json!({ "id": "g-2", "payload": { "mimeType": "text/plain", "body": { "data": URL_SAFE.encode(b"x") } } }),
    ];
    let (source, _) = tokens(&base, now() + 3600);
    let gmail = Gmail::new(&format!("{base}/gmail/v1/users/me"), source, 1).unwrap();
    let mailbox = Mailbox::in_memory(gmail);

    let page = mailbox.list_messages(None).unwrap();
    let list = &page.messages[0].attachments;
    let names: Vec<_> = list.iter().map(|a| a.name.as_deref().unwrap()).collect();
    assert_eq!(names, ["f-1.pdf", "f-2.txt"], "sin la imagen incrustada");
    assert_eq!(list[0].id, "ATT-1");
    assert_eq!(list[0].size, Some(9));
    assert_eq!(list[1].id, "part:2", "adjunto pequeño dentro del mensaje");
    assert_eq!(page.messages[0].body.as_deref(), Some("Hola"));

    assert_eq!(
        mailbox.fetch_attachment("g-1", "ATT-1").unwrap(),
        b"%PDF-sint"
    );
    assert_eq!(
        mailbox.fetch_attachment("g-1", "part:2").unwrap(),
        b"Factura en texto"
    );
    assert_eq!(
        mailbox.fetch_attachment("g-1", "part:9").unwrap_err().code,
        "not_found"
    );
    assert_eq!(
        mailbox.fetch_attachment("g-1", "ATT-X").unwrap_err().code,
        "not_found"
    );
}
