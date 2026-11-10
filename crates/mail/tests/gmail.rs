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
        ("GET", ["threads", id]) if req.query("format").as_deref() == Some("full") => {
            let messages: Vec<Value> = w
                .messages
                .iter()
                .filter(|m| m["threadId"] == *id)
                .cloned()
                .collect();
            if messages.is_empty() {
                not_found()
            } else {
                Resp::Json(200, json!({ "id": id, "messages": messages }))
            }
        }
        ("GET", ["threads", id]) => {
            // Como Gmail: sin `format=metadata` y `metadataHeaders=X-NestAdmin-Id` no hay
            // cabeceras propias que comparar, y solo se devuelven las cabeceras pedidas.
            // D178 pide la cabecera `To` (destinatarios): lo mismo, sin cuerpo.
            let wanted = req.query("metadataHeaders").unwrap_or_default();
            if req.query("format").as_deref() != Some("metadata")
                || !["X-NestAdmin-Id", "To"].contains(&wanted.as_str())
            {
                return Resp::Json(400, json!({ "error": { "status": "INVALID_ARGUMENT" } }));
            }
            let only_requested = |m: &Value, labels: Value| {
                let headers: Vec<Value> = m
                    .pointer("/payload/headers")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter(|h| h["name"] == wanted.as_str())
                    .cloned()
                    .collect();
                json!({
                    "id": m["id"], "threadId": m["threadId"], "labelIds": labels,
                    "internalDate": m["internalDate"],
                    "payload": { "headers": headers },
                })
            };
            let mut messages: Vec<Value> = w
                .messages
                .iter()
                .filter(|m| m["threadId"] == *id)
                .map(|m| only_requested(m, m["labelIds"].clone()))
                .collect();
            // Mientras existe el borrador, el hilo incluye su mensaje (DRAFT) con la marca.
            for (_, message_id, stored) in &w.drafts {
                if stored["threadId"] == *id {
                    let headers: Vec<Value> = raw_header(stored, "X-NestAdmin-Id")
                        .map(|own| json!({ "name": "X-NestAdmin-Id", "value": own }))
                        .into_iter()
                        .collect();
                    messages.push(json!({
                        "id": message_id, "threadId": id, "labelIds": ["DRAFT"],
                        "payload": { "headers": headers },
                    }));
                }
            }
            if w.thread_without_messages && !messages.is_empty() {
                return Resp::Json(200, json!({ "id": id }));
            }
            if messages.is_empty() {
                not_found()
            } else {
                Resp::Json(200, json!({ "id": id, "messages": messages }))
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
            // Como Gmail, el borrador desaparece y queda un mensaje con la etiqueta SENT, otro
            // `Message-ID` (salvo `keep_message_id`), la cabecera X-NestAdmin-Id intacta y,
            // salvo `keep_sent_id`, otro ID.
            let (_, draft_message_id, stored) = w.drafts.remove(index);
            let sent_id = if w.keep_sent_id {
                draft_message_id
            } else {
                format!("s-{}", w.sent.len() + 1)
            };
            let mut headers = Vec::new();
            let message_id = match raw_header(&stored, "Message-ID") {
                Some(own) if w.keep_message_id => own,
                _ => format!("<CAF{}@mail.gmail.com>", w.sent.len() + 1),
            };
            headers.push(json!({ "name": "Message-ID", "value": message_id }));
            if let Some(own) = raw_header(&stored, "X-NestAdmin-Id") {
                headers.push(json!({ "name": "X-NestAdmin-Id", "value": own }));
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
                    { "name": "Auto-Submitted", "value": "auto-generated" },
                    { "name": "list-unsubscribe", "value": "<https://CENTINELA-9c41.test/baja>" },
                    { "name": "X-Interna", "value": "CENTINELA-9c41" },
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
fn inbox_messages_carry_signals_and_thread_key_without_raw_headers() {
    let (mailbox, _world) = mailbox();
    let first = mailbox.list_messages(None).unwrap();
    let m = &first.messages[0];
    assert_eq!(
        m.signals,
        Some(vec![
            "auto_submitted".to_owned(),
            "list_unsubscribe".to_owned()
        ])
    );
    assert_eq!(m.thread_key.as_deref(), Some("t-1"));
    let serialized = serde_json::to_string(&first.messages).unwrap();
    assert!(
        !serialized.contains("CENTINELA"),
        "la cabecera en bruto no se guarda"
    );
    // Sin cabeceras: comprobado y sin señales, no «sin comprobar».
    let second = mailbox.list_messages(first.next_cursor.as_deref()).unwrap();
    assert_eq!(second.messages[0].signals, Some(Vec::new()));
    assert_eq!(second.messages[0].thread_key, None);
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
    assert_eq!(
        raw_header(stored, "X-NestAdmin-Id").as_deref(),
        Some(own.as_str())
    );
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

    // El borrador ya no existe y el mensaje enviado tiene otro ID y otro Message-ID, pero
    // sigue en el hilo con X-NestAdmin-Id y la etiqueta SENT: enviado, sin repetir.
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
    assert_eq!(count(&world, "GET", "/threads/t-1"), 1);
    assert_eq!(
        rfc_searches(&world),
        0,
        "el hilo basta: no hace falta buscar por Message-ID"
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
        "el identificador propio se guarda"
    );
    let saved_json: Value = serde_json::from_str(&saved).unwrap();
    assert_eq!(
        saved_json["drafts"][0]["thread_id"], "t-1",
        "el hilo se guarda"
    );

    // Proceso nuevo: solo el registro local recuerda el identificador y el hilo del borrador.
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

/// Mensajes `SENT` del mismo hilo cuya marca falta o es otra no confirman el envío.
#[test]
fn sent_messages_in_the_thread_without_our_mark_do_not_confirm() {
    for other in [None, Some("<nestadmin.otro@nestadmin.invalid>")] {
        let (mailbox, world) = mailbox();
        let id = mailbox.create_draft("g-1", &draft(), "k-draft").unwrap();
        world.lock().unwrap().send_status = Some(503);
        let _ = mailbox.send_draft(&id, "k-send");
        {
            let mut w = world.lock().unwrap();
            w.send_status = None;
            let own = raw_header(&w.drafts[0].2, "X-NestAdmin-Id").unwrap();
            w.drafts.clear();
            let headers = other.map_or_else(Vec::new, |v| {
                vec![json!({ "name": "X-NestAdmin-Id", "value": v })]
            });
            w.messages.push(json!({
                "id": "s-otro",
                "threadId": "t-1",
                "labelIds": ["SENT"],
                "payload": { "headers": headers },
            }));
            // Con la marca nuestra pero sin SENT (p. ej. una copia recibida) tampoco vale.
            w.messages.push(json!({
                "id": "i-copia",
                "threadId": "t-1",
                "labelIds": ["INBOX"],
                "payload": { "headers": [{
                    "name": "X-NestAdmin-Id",
                    "value": own
                }] },
            }));
        }
        assert_eq!(mailbox.lookup("k-send").unwrap_err().code, "not_found");
        assert!(world.lock().unwrap().sent.is_empty());
        assert_eq!(count(&world, "POST", "/drafts/send"), 1);
    }
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

fn rfc_searches(world: &common::Shared) -> usize {
    world
        .lock()
        .unwrap()
        .requests
        .iter()
        .filter(|r| {
            r.query("q")
                .is_some_and(|q| q.starts_with("rfc822msgid:nestadmin."))
        })
        .count()
}

/// Cuenta una cuenta con borrador creado y envío cortado, con el registro en disco.
fn cut_send_on_disk(
    base: &str,
    path: &std::path::Path,
    world: &common::Shared,
) -> (String, String) {
    let mailbox = Mailbox::open(gmail(base), path).unwrap();
    let id = mailbox.create_draft("g-1", &draft(), "k-draft").unwrap();
    world.lock().unwrap().drop_send_response = true;
    assert!(mailbox.send_draft(&id, "k-send").is_err());
    (id, std::fs::read_to_string(path).unwrap())
}

#[test]
fn old_record_without_thread_id_loads_and_falls_back_to_the_message_id_search() {
    let (base, world) = serve(handler);
    {
        let mut w = world.lock().unwrap();
        w.messages = sample_messages();
        w.keep_message_id = true;
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cuenta-gmail.json");
    let (id, saved) = cut_send_on_disk(&base, &path, &world);

    // Registro anterior a esta versión: sin `thread_id`.
    let mut json: Value = serde_json::from_str(&saved).unwrap();
    assert_eq!(json["drafts"][0]["thread_id"], "t-1");
    json["drafts"][0]
        .as_object_mut()
        .unwrap()
        .remove("thread_id");
    std::fs::write(&path, json.to_string()).unwrap();

    let mailbox = Mailbox::open(gmail(&base), &path).unwrap();
    assert_eq!(
        mailbox.lookup("k-send").unwrap().unwrap()["receipt_id"],
        "s-1"
    );
    assert_eq!(mailbox.send_draft(&id, "k-send").unwrap().receipt_id, "s-1");
    assert_eq!(count(&world, "GET", "/threads/t-1"), 0);
    assert_eq!(rfc_searches(&world), 1);
    assert_eq!(count(&world, "POST", "/drafts/send"), 1);
}

#[test]
fn old_record_without_thread_id_is_not_confirmed_when_gmail_replaced_the_message_id() {
    let (base, world) = serve(handler);
    world.lock().unwrap().messages = sample_messages();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cuenta-gmail.json");
    let (id, saved) = cut_send_on_disk(&base, &path, &world);
    let mut json: Value = serde_json::from_str(&saved).unwrap();
    json["drafts"][0]
        .as_object_mut()
        .unwrap()
        .remove("thread_id");
    std::fs::write(&path, json.to_string()).unwrap();

    let mailbox = Mailbox::open(gmail(&base), &path).unwrap();
    assert_eq!(mailbox.lookup("k-send").unwrap_err().code, "not_found");
    assert_eq!(
        mailbox.send_draft(&id, "k-send").unwrap_err().code,
        "not_found"
    );
    assert_eq!(count(&world, "POST", "/drafts/send"), 1, "no se reenvía");
}

#[test]
fn thread_not_found_means_not_found_without_resending() {
    let (mailbox, world) = mailbox();
    let id = mailbox.create_draft("g-1", &draft(), "k-draft").unwrap();
    {
        let mut w = world.lock().unwrap();
        w.drop_send_response = true;
        w.force = None;
    }
    let _ = mailbox.send_draft(&id, "k-send");
    world.lock().unwrap().force = Some(("/threads/".into(), 404));
    assert_eq!(mailbox.lookup("k-send").unwrap_err().code, "not_found");
    assert_eq!(
        mailbox.send_draft(&id, "k-send").unwrap_err().code,
        "not_found"
    );
    assert_eq!(count(&world, "POST", "/drafts/send"), 1);
}

/// Un 200 de `threads.get` sin lista `messages` no prueba nada: `unavailable`, no `not_found`,
/// sin reenviar y sin dar el envío por hecho. Solo el 404 del hilo es no encontrado.
#[test]
fn thread_200_without_messages_is_unavailable_never_assumed_sent() {
    let (mailbox, world) = mailbox();
    let id = mailbox.create_draft("g-1", &draft(), "k-draft").unwrap();
    world.lock().unwrap().drop_send_response = true;
    let _ = mailbox.send_draft(&id, "k-send");
    world.lock().unwrap().thread_without_messages = true;
    assert_eq!(mailbox.lookup("k-send").unwrap_err().code, "unavailable");
    assert_eq!(
        mailbox.send_draft(&id, "k-send").unwrap_err().code,
        "unavailable"
    );
    assert!(count(&world, "GET", "/threads/t-1") >= 2);
    assert_eq!(count(&world, "POST", "/drafts/send"), 1, "no se reenvía");

    // Al responder bien el servicio, la misma consulta confirma el envío real.
    world.lock().unwrap().thread_without_messages = false;
    assert_eq!(mailbox.send_draft(&id, "k-send").unwrap().receipt_id, "s-1");
    assert_eq!(count(&world, "POST", "/drafts/send"), 1);
}

/// Mientras el borrador existe, su mensaje (DRAFT) lleva la marca en el hilo y no confirma
/// nada: con el envío fallado sin efecto no consta enviado.
#[test]
fn a_draft_message_carrying_our_mark_does_not_confirm_the_send() {
    let (mailbox, world) = mailbox();
    let id = mailbox.create_draft("g-1", &draft(), "k-draft").unwrap();
    world.lock().unwrap().send_status = Some(503);
    let _ = mailbox.send_draft(&id, "k-send");
    assert!(world.lock().unwrap().sent.is_empty());
    assert_eq!(mailbox.lookup("k-send").unwrap(), None);
    assert!(world.lock().unwrap().sent.is_empty());
}

#[test]
fn thread_server_error_is_unavailable_and_never_assumed_sent() {
    let (mailbox, world) = mailbox();
    let id = mailbox.create_draft("g-1", &draft(), "k-draft").unwrap();
    world.lock().unwrap().drop_send_response = true;
    let _ = mailbox.send_draft(&id, "k-send");
    world.lock().unwrap().force = Some(("/threads/".into(), 500));
    assert_eq!(mailbox.lookup("k-send").unwrap_err().code, "unavailable");
    assert_eq!(
        mailbox.send_draft(&id, "k-send").unwrap_err().code,
        "unavailable"
    );
    assert_eq!(count(&world, "POST", "/drafts/send"), 1);

    // Al volver el servicio, la misma consulta confirma el envío real.
    world.lock().unwrap().force = None;
    assert_eq!(mailbox.send_draft(&id, "k-send").unwrap().receipt_id, "s-1");
    assert_eq!(count(&world, "POST", "/drafts/send"), 1);
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

/// Microsoft (D70): `http://localhost:<puerto>/`, con el navegador llegando por IPv4 o por IPv6.
#[test]
fn localhost_redirect_is_accepted_over_ipv4_and_ipv6() {
    let (base, _) = serve(handler);
    let config = common::oauth_localhost(&base);
    let ipv6 = std::net::TcpListener::bind("[::1]:0").is_ok();
    // El servidor simulado numera los tokens que emite: cada inicio de sesión recibe el siguiente.
    let mut issued = 0;
    for host in ["127.0.0.1", "[::1]"] {
        if host == "[::1]" && !ipv6 {
            eprintln!("sin IPv6 de bucle local: se omite ese caso");
            continue;
        }
        let pending = PendingAuthorization::begin(&config).unwrap();
        let url = pending.url().to_owned();
        let redirect = pending.redirect_uri().to_owned();
        assert!(redirect.starts_with("http://localhost:") && redirect.ends_with('/'));
        assert_eq!(param(&url, "redirect_uri"), redirect);
        let port = Url::parse(&redirect).unwrap().port().unwrap();
        let state = param(&url, "state");
        let target = format!("http://{host}:{port}/");
        let browser = thread::spawn(move || {
            browser_redirect(&target, &format!("code=codigo-sintetico&state={state}"))
        });
        let tokens = pending.finish(&config, Duration::from_secs(10)).unwrap();
        assert!(
            browser
                .join()
                .unwrap()
                .contains("Inicio de sesión completado"),
            "{host}"
        );
        issued += 1;
        assert_eq!(tokens.access_token, format!("access-{issued}"), "{host}");
    }
}

#[test]
fn microsoft_uses_localhost_and_google_the_ip_literal() {
    assert_eq!(
        nestadmin_mail::OAuthConfig::microsoft("x").redirect_host,
        "localhost"
    );
    assert_eq!(
        nestadmin_mail::OAuthConfig::google("x", "y").redirect_host,
        "127.0.0.1"
    );
}

// ---- Estado del hilo (D162) ----

fn thread_message(id: &str, labels: Value, date: Option<&str>) -> Value {
    json!({
        "id": id, "threadId": "t-9", "labelIds": labels, "internalDate": date,
        // El cuerpo existe en el servicio, pero con `format=metadata` no debe salir ni leerse.
        "payload": { "mimeType": "text/plain", "headers": [],
                     "body": { "data": b64("SENTINELA-HILO texto privado del hilo") } },
    })
}

#[test]
fn the_last_message_of_a_thread_decides_whether_it_is_ours_from_metadata_only() {
    let (mailbox, world) = mailbox();
    let set = |messages: Vec<Value>| world.lock().unwrap().messages = messages;

    // Último con SENT: propio, aunque la lista venga desordenada (manda `internalDate`).
    set(vec![
        thread_message("out-1", json!(["SENT"]), Some("2000")),
        thread_message("in-1", json!(["INBOX"]), Some("1000")),
    ]);
    assert_eq!(mailbox.last_message_is_ours("t-9").unwrap(), Some(true));
    // Si el último es de otra persona, no.
    set(vec![
        thread_message("in-1", json!(["INBOX"]), Some("1000")),
        thread_message("out-1", json!(["SENT"]), Some("2000")),
        thread_message("in-2", json!(["INBOX", "UNREAD"]), Some("3000")),
    ]);
    assert_eq!(mailbox.last_message_is_ours("t-9").unwrap(), Some(false));
    // Un borrador o un mensaje de la papelera posterior no cuentan.
    set(vec![
        thread_message("in-1", json!(["INBOX"]), Some("1000")),
        thread_message("out-1", json!(["SENT"]), Some("2000")),
        thread_message("dr-1", json!(["DRAFT"]), Some("9000")),
        thread_message("tr-1", json!(["INBOX", "TRASH"]), Some("9500")),
    ]);
    assert_eq!(mailbox.last_message_is_ours("t-9").unwrap(), Some(true));
    // Sin fechas, manda el orden de la lista.
    set(vec![
        thread_message("in-1", json!(["INBOX"]), None),
        thread_message("out-1", json!(["SENT"]), None),
    ]);
    assert_eq!(mailbox.last_message_is_ours("t-9").unwrap(), Some(true));
    // Solo borradores: no se sabe.
    set(vec![thread_message("dr-1", json!(["DRAFT"]), Some("1"))]);
    assert_eq!(mailbox.last_message_is_ours("t-9").unwrap(), None);

    // Solo se pide `threads.get` con metadatos y la cabecera propia: ni `format=full`, ni
    // mensajes sueltos, ni búsquedas, ni escrituras.
    let w = world.lock().unwrap();
    assert!(!w.requests.is_empty());
    for r in &w.requests {
        assert_eq!(r.method, "GET");
        assert!(r.url.path().ends_with("/threads/t-9"), "{}", r.url);
        let query = |k: &str| {
            r.url
                .query_pairs()
                .find(|(n, _)| n == k)
                .map(|(_, v)| v.into_owned())
        };
        assert_eq!(query("format").as_deref(), Some("metadata"));
        assert_eq!(query("metadataHeaders").as_deref(), Some("X-NestAdmin-Id"));
        assert_ne!(query("format").as_deref(), Some("full"));
    }
}

#[test]
fn an_unknown_missing_or_failing_thread_is_unknown_or_an_error_never_a_guess() {
    let (mailbox, world) = mailbox();
    // Hilo inexistente (404) o 200 sin lista de mensajes: no se sabe.
    assert_eq!(mailbox.last_message_is_ours("t-9").unwrap(), None);
    world.lock().unwrap().messages = vec![thread_message("in-1", json!(["INBOX"]), Some("1"))];
    world.lock().unwrap().thread_without_messages = true;
    assert_eq!(mailbox.last_message_is_ours("t-9").unwrap(), None);
    world.lock().unwrap().thread_without_messages = false;
    // Un fallo del servicio es un error (quien llama lo trata como desconocido), no un valor.
    world.lock().unwrap().force = Some(("/threads/".into(), 500));
    assert_eq!(
        mailbox.last_message_is_ours("t-9").unwrap_err().code,
        "unavailable"
    );
    world.lock().unwrap().force = Some(("/threads/".into(), 401));
    assert!(mailbox.last_message_is_ours("t-9").is_err());
    // Una clave vacía no llama al servicio.
    let before = world.lock().unwrap().requests.len();
    assert_eq!(mailbox.last_message_is_ours("  ").unwrap(), None);
    assert_eq!(world.lock().unwrap().requests.len(), before);
}

// ---- Respuesta propia en Enviados (D168, T33a) ----

fn full_message(id: &str, labels: Value, date: &str, body: &str) -> Value {
    json!({
        "id": id, "threadId": "t-9", "labelIds": labels, "internalDate": date,
        "payload": { "mimeType": "text/plain",
                     "headers": [
                         { "name": "To", "value": "Ana Pérez <ana@cliente.test>, jefe@empresa.test" },
                         { "name": "From", "value": "Ana Pérez <ana@cliente.test>" },
                     ],
                     "body": { "data": b64(body) } },
    })
}

#[test]
fn gmail_finds_the_first_sent_message_after_the_received_one() {
    let (mailbox, world) = mailbox();
    assert!(mailbox.supports_sent());
    world.lock().unwrap().messages = vec![
        full_message("out-0", json!(["SENT"]), "500000", "respuesta anterior"),
        full_message("in-1", json!(["INBOX"]), "1000000", "pregunta"),
        // No SENT: otro remitente, borrador y papelera posteriores no cuentan.
        full_message("in-2", json!(["INBOX"]), "1500000", "otro del cliente"),
        full_message("dr-1", json!(["DRAFT", "SENT"]), "1600000", "borrador"),
        full_message("tr-1", json!(["SENT", "TRASH"]), "1700000", "papelera"),
        // Dos propios posteriores, desordenados: gana el más temprano.
        full_message("out-2", json!(["SENT"]), "3000000", "segunda"),
        full_message(
            "out-1",
            json!(["SENT"]),
            "2000000",
            "Hola,\nLo vemos hoy.\n\nEl lun escribió:\n> pregunta",
        ),
    ];
    let reply = mailbox.own_reply_in_thread("t-9", "in-1").unwrap().unwrap();
    assert_eq!(reply.id, "out-1");
    assert_eq!(reply.date.as_deref(), Some("1970-01-01T00:33:20Z"));
    // M2: los destinatarios de la cabecera `To` (direcciones, sin nombre visible).
    assert_eq!(
        reply.to,
        vec![
            "ana@cliente.test".to_owned(),
            "jefe@empresa.test".to_owned()
        ]
    );
    // Tal como llega: la cita no se recorta aquí.
    assert_eq!(
        reply.body.as_deref(),
        Some("Hola,\nLo vemos hoy.\n\nEl lun escribió:\n> pregunta")
    );

    // Solo `threads.get` con `format=full`: ninguna escritura.
    let w = world.lock().unwrap();
    assert_eq!(w.requests.len(), 1);
    assert_eq!(w.requests[0].method, "GET");
    assert!(w.requests[0].url.path().ends_with("/threads/t-9"));
    assert_eq!(w.requests[0].query("format").as_deref(), Some("full"));
}

#[test]
fn gmail_without_a_later_sent_message_has_no_own_reply() {
    let (mailbox, world) = mailbox();
    // Solo anteriores al recibido y mensajes no SENT.
    world.lock().unwrap().messages = vec![
        full_message("out-0", json!(["SENT"]), "500000", "anterior"),
        full_message("in-1", json!(["INBOX"]), "1000000", "pregunta"),
        full_message("in-2", json!(["INBOX"]), "2000000", "otro"),
    ];
    assert_eq!(mailbox.own_reply_in_thread("t-9", "in-1").unwrap(), None);
    // El recibido no está en el hilo: no se puede ordenar.
    assert_eq!(mailbox.own_reply_in_thread("t-9", "no-esta").unwrap(), None);
    // Hilo inexistente (404): sin respuesta, no error.
    assert_eq!(mailbox.own_reply_in_thread("t-0", "in-1").unwrap(), None);
    // Un fallo del servicio es un error, no `None`.
    world.lock().unwrap().force = Some(("/threads/".into(), 500));
    assert_eq!(
        mailbox.own_reply_in_thread("t-9", "in-1").unwrap_err().code,
        "unavailable"
    );
    // Claves vacías: sin llamada.
    world.lock().unwrap().force = None;
    let before = world.lock().unwrap().requests.len();
    assert_eq!(mailbox.own_reply_in_thread("  ", "in-1").unwrap(), None);
    assert_eq!(mailbox.own_reply_in_thread("t-9", "").unwrap(), None);
    assert_eq!(world.lock().unwrap().requests.len(), before);
}

/// Sin cabecera `To` no se sabe a quién iba el mensaje propio: lista vacía, y aprender descarta
/// el par.
#[test]
fn gmail_sent_reply_without_a_to_header_has_an_empty_recipient_list() {
    let (mailbox, world) = mailbox();
    let mut out = full_message("out-1", json!(["SENT"]), "2000000", "ok");
    out["payload"]["headers"] = json!([]);
    world.lock().unwrap().messages = vec![
        full_message("in-1", json!(["INBOX"]), "1000000", "pregunta"),
        out,
    ];
    let reply = mailbox.own_reply_in_thread("t-9", "in-1").unwrap().unwrap();
    assert!(reply.to.is_empty());
}

/// M1: el nombre visible del remitente (cabecera `From`) se pide solo con metadatos; el cuerpo no
/// se lee. Un mensaje inexistente no es un error.
#[test]
fn gmail_received_extra_gives_only_the_sender_name() {
    let (mailbox, world) = mailbox();
    world.lock().unwrap().messages = vec![full_message(
        "in-1",
        json!(["INBOX"]),
        "1000000",
        "pregunta",
    )];
    let extra = mailbox.received_extra("in-1").unwrap();
    assert_eq!(extra.sender_name.as_deref(), Some("Ana Pérez"));
    assert_eq!(extra.unique_body, None);
    assert_eq!(
        mailbox.received_extra("no-esta").unwrap(),
        Default::default()
    );
    let w = world.lock().unwrap();
    for r in &w.requests {
        assert_eq!(r.method, "GET");
        assert_eq!(r.query("format").as_deref(), Some("metadata"));
        assert_eq!(r.query("metadataHeaders").as_deref(), Some("From"));
    }
}

/// D178: todas las respuestas propias posteriores, por fecha, solo con metadatos. La petición es
/// `format=metadata` con la cabecera `To` y `fields` sin `payload/body`: no pide ningún cuerpo.
#[test]
fn gmail_own_replies_meta_lists_every_later_sent_message_without_asking_for_a_body() {
    let (mailbox, world) = mailbox();
    world.lock().unwrap().messages = vec![
        full_message("out-0", json!(["SENT"]), "500000", "anterior"),
        full_message("in-1", json!(["INBOX"]), "1000000", "pregunta"),
        full_message("in-2", json!(["INBOX"]), "1500000", "otro"),
        full_message("dr-1", json!(["DRAFT", "SENT"]), "1600000", "borrador"),
        full_message("tr-1", json!(["SENT", "TRASH"]), "1700000", "papelera"),
        full_message("out-2", json!(["SENT"]), "3000000", "segunda"),
        full_message("out-1", json!(["SENT"]), "2000000", "primera"),
    ];
    let metas = mailbox.own_replies_meta("t-9", "in-1").unwrap();
    assert_eq!(
        metas.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
        ["out-1", "out-2"]
    );
    assert_eq!(metas[0].date.as_deref(), Some("1970-01-01T00:33:20Z"));
    assert_eq!(
        metas[0].to,
        vec![
            "ana@cliente.test".to_owned(),
            "jefe@empresa.test".to_owned()
        ]
    );
    // Una sola petición de lectura, sin cuerpo.
    let w = world.lock().unwrap();
    assert_eq!(w.requests.len(), 1);
    assert_eq!(w.requests[0].method, "GET");
    assert!(w.requests[0].url.path().ends_with("/threads/t-9"));
    assert_eq!(w.requests[0].query("format").as_deref(), Some("metadata"));
    assert_eq!(
        w.requests[0].query("metadataHeaders").as_deref(),
        Some("To")
    );
    let fields = w.requests[0].query("fields").unwrap();
    assert!(
        !fields.contains("body") && !fields.contains("snippet"),
        "{fields}"
    );
    drop(w);

    // Sin respuestas posteriores, recibido desconocido, hilo inexistente y claves vacías.
    assert!(mailbox
        .own_replies_meta("t-9", "no-esta")
        .unwrap()
        .is_empty());
    assert!(mailbox.own_replies_meta("t-0", "in-1").unwrap().is_empty());
    let before = world.lock().unwrap().requests.len();
    assert!(mailbox.own_replies_meta("  ", "in-1").unwrap().is_empty());
    assert!(mailbox.own_replies_meta("t-9", "").unwrap().is_empty());
    assert_eq!(world.lock().unwrap().requests.len(), before);
    // Un fallo del servicio es un error, no una lista vacía.
    world.lock().unwrap().force = Some(("/threads/".into(), 500));
    assert_eq!(
        mailbox.own_replies_meta("t-9", "in-1").unwrap_err().code,
        "unavailable"
    );
}

/// D178: el hilo de un mensaje recibido se pide con `messages.get` en `format=minimal` y `fields`
/// restringido a `id,threadId`: ni cabeceras, ni cuerpo, ni `snippet`.
#[test]
fn gmail_thread_of_asks_only_for_the_thread_id_without_a_body() {
    let (mailbox, world) = mailbox();
    world.lock().unwrap().messages = vec![full_message(
        "in-1",
        json!(["INBOX"]),
        "1000000",
        "pregunta",
    )];
    assert_eq!(mailbox.thread_of("in-1").unwrap().as_deref(), Some("t-9"));
    {
        let w = world.lock().unwrap();
        assert_eq!(w.requests.len(), 1);
        assert_eq!(w.requests[0].method, "GET");
        assert!(w.requests[0].url.path().ends_with("/messages/in-1"));
        assert_eq!(w.requests[0].query("format").as_deref(), Some("minimal"));
        let fields = w.requests[0].query("fields").unwrap();
        assert_eq!(fields, "id,threadId");
        assert!(!fields.contains("body") && !fields.contains("snippet"));
    }
    // Un mensaje que ya no existe es `not_found`; un identificador vacío no llama al servicio.
    assert_eq!(mailbox.thread_of("no-esta").unwrap_err().code, "not_found");
    let before = world.lock().unwrap().requests.len();
    assert_eq!(mailbox.thread_of(" ").unwrap(), None);
    assert_eq!(world.lock().unwrap().requests.len(), before);
    // Un fallo del servicio es un error.
    world.lock().unwrap().force = Some(("/messages/".into(), 500));
    assert_eq!(mailbox.thread_of("in-1").unwrap_err().code, "unavailable");
}
