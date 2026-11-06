//! T13 (Graph): normalización, paginación sin salir del origen, borrador idempotente, envío
//! único con reconciliación, renovación de token y traducción de errores.

mod common;

use common::{count, now, serve, tokens, Req, Resp, World};
use nestadmin_core::Connector;
use nestadmin_mail::{Graph, Mailbox};
use nestadmin_simulators::{MailConnector, NewDraft};
use serde_json::{json, Value};

fn handler(req: &Req, w: &mut World) -> Resp {
    let path = req.url.path().trim_start_matches("/v1.0/");
    let segments: Vec<&str> = path.split('/').collect();
    match (req.method.as_str(), segments.as_slice()) {
        ("GET", ["me", "mailFolders", "inbox", "messages"]) => {
            if req.query("$skiptoken").as_deref() == Some("2") {
                return Resp::Json(200, json!({ "value": [w.messages[2]] }));
            }
            let next = format!("{}/v1.0/me/mailFolders/inbox/messages?$skiptoken=2", w.base);
            Resp::Json(
                200,
                json!({ "value": [w.messages[0], w.messages[1]], "@odata.nextLink": next }),
            )
        }
        ("POST", ["me", "messages", original, "createReply"]) => {
            if !w.messages.iter().any(|m| m["id"] == *original) {
                return Resp::Json(404, json!({ "error": { "code": "ErrorItemNotFound" } }));
            }
            let id = format!("borrador={}", w.drafts.len() + 1);
            let draft = json!({ "id": id, "isDraft": true, "request": req.json() });
            w.drafts.push((id.clone(), id.clone(), draft));
            Resp::Json(201, json!({ "id": id }))
        }
        ("POST", ["me", "messages", id, "send"]) => {
            if let Some(status) = w.send_status {
                return Resp::Json(status, json!({ "error": { "code": "ServiceUnavailable" } }));
            }
            let Some(draft) = w.drafts.iter_mut().find(|d| d.0 == *id) else {
                return Resp::Json(404, json!({ "error": { "code": "ErrorItemNotFound" } }));
            };
            draft.2["isDraft"] = json!(false);
            w.sent.push((*id).to_owned());
            if w.drop_send_response {
                Resp::Drop
            } else {
                Resp::Empty(202)
            }
        }
        ("GET", ["me", "messages", id, "attachments"]) => {
            let Some(m) = w.messages.iter().find(|m| m["id"] == *id) else {
                return Resp::Json(404, json!({ "error": { "code": "ErrorItemNotFound" } }));
            };
            // Como Graph con `$select`: sin `contentBytes`.
            let list: Vec<Value> = m["_attachments"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(|mut a| {
                    a.as_object_mut().unwrap().remove("contentBytes");
                    a
                })
                .collect();
            Resp::Json(200, json!({ "value": list }))
        }
        ("GET", ["me", "messages", id, "attachments", aid]) => {
            let found = w
                .messages
                .iter()
                .find(|m| m["id"] == *id)
                .and_then(|m| m["_attachments"].as_array())
                .and_then(|list| list.iter().find(|a| a["id"] == *aid).cloned());
            match found {
                Some(a) => Resp::Json(200, a),
                None => Resp::Json(404, json!({ "error": { "code": "ErrorItemNotFound" } })),
            }
        }
        ("GET", ["me", "messages", id]) => match w.drafts.iter().find(|d| d.0 == *id) {
            Some(draft) => Resp::Json(200, json!({ "id": id, "isDraft": draft.2["isDraft"] })),
            None => Resp::Json(404, json!({ "error": { "code": "ErrorItemNotFound" } })),
        },
        _ => Resp::Json(404, json!({ "error": { "code": "NoRoute" } })),
    }
}

fn sample_messages() -> Vec<Value> {
    vec![
        json!({
            "id": "m-1",
            "subject": "Factura FS-2026-014",
            "from": { "emailAddress": { "name": "Ana", "address": "ana@cliente.test" } },
            "toRecipients": [{ "emailAddress": { "address": "compras@empresa.test" } }],
            "receivedDateTime": "2026-11-02T09:30:00Z",
            "body": { "contentType": "text", "content": "Adjunto la factura de octubre.\r\n" },
        }),
        json!({
            "id": "m-2",
            "subject": "",
            "from": { "emailAddress": { "address": "pagos@proveedor.test" } },
            "body": { "contentType": "html", "content": "<p>¿Cuándo&nbsp;pagan?</p>" },
        }),
        json!({ "id": "m-3", "subject": "Sin cuerpo" }),
    ]
}

fn mailbox() -> (Mailbox<Graph>, common::Shared) {
    let (base, world) = serve(handler);
    world.lock().unwrap().messages = sample_messages();
    let (source, _) = tokens(&base, now() + 3600);
    let graph = Graph::new(&format!("{base}/v1.0"), source, 2).unwrap();
    (Mailbox::in_memory(graph), world)
}

fn draft(body: &str) -> NewDraft {
    NewDraft {
        to: vec!["ana@cliente.test".into()],
        subject: None,
        body: body.into(),
    }
}

#[test]
fn inbox_is_listed_normalized_and_paged_within_the_service() {
    let (mailbox, world) = mailbox();
    let first = mailbox.list_messages(None).unwrap();
    assert_eq!(first.messages.len(), 2);
    let m1 = &first.messages[0];
    assert_eq!(m1.subject.as_deref(), Some("Factura FS-2026-014"));
    assert_eq!(m1.from.as_deref(), Some("ana@cliente.test"));
    assert_eq!(m1.to, Some(vec!["compras@empresa.test".to_owned()]));
    assert_eq!(m1.body.as_deref(), Some("Adjunto la factura de octubre."));
    assert_eq!(m1.date.as_deref(), Some("2026-11-02T09:30:00Z"));
    let m2 = &first.messages[1];
    assert_eq!(m2.subject, None, "un asunto vacío es dato ausente");
    assert_eq!(
        m2.to, None,
        "sin destinatarios conocidos no hay lista vacía"
    );
    assert_eq!(m2.body.as_deref(), Some("¿Cuándo pagan?"));

    let second = mailbox.list_messages(first.next_cursor.as_deref()).unwrap();
    assert_eq!(second.messages[0].id, "m-3");
    assert_eq!(second.messages[0].body, None);
    assert_eq!(second.next_cursor, None);

    let w = world.lock().unwrap();
    let req = &w.requests[0];
    assert_eq!(req.header("authorization"), Some("Bearer access-0"));
    assert!(req
        .header("prefer")
        .unwrap()
        .contains("IdType=\"ImmutableId\""));
    assert_eq!(req.query("$top").as_deref(), Some("2"));
}

#[test]
fn cursor_pointing_elsewhere_is_rejected_without_sending_the_token() {
    let (mailbox, world) = mailbox();
    for cursor in [
        "https://atacante.test/v1.0/me/messages",
        "http://127.0.0.1:1/v1.0/me/messages",
    ] {
        let error = mailbox.list_messages(Some(cursor)).unwrap_err();
        assert_eq!(error.code, "invalid_input", "{cursor}");
    }
    assert!(world.lock().unwrap().requests.is_empty());
}

#[test]
fn reply_draft_is_threaded_and_idempotent_per_key() {
    let (mailbox, world) = mailbox();
    let id = mailbox
        .create_draft("m-1", &draft("Gracias, Ana."), "task-1/create_draft/v1")
        .unwrap();
    let again = mailbox
        .create_draft("m-1", &draft("Gracias, Ana."), "task-1/create_draft/v1")
        .unwrap();
    assert_eq!(id, again);
    assert_eq!(count(&world, "POST", "/createReply"), 1);

    let conflict = mailbox
        .create_draft("m-1", &draft("Otro texto"), "task-1/create_draft/v1")
        .unwrap_err();
    assert_eq!(conflict.code, "conflict");

    let w = world.lock().unwrap();
    let request = w.drafts[0].2["request"].clone();
    assert_eq!(request["comment"], "Gracias, Ana.");
    assert_eq!(
        request["message"]["toRecipients"][0]["emailAddress"]["address"],
        "ana@cliente.test"
    );
}

#[test]
fn invalid_drafts_never_reach_the_provider() {
    let (mailbox, world) = mailbox();
    let mut bad = draft("Hola");
    bad.to = vec!["ana@cliente.test\r\nBcc: otro@x.test".into()];
    assert_eq!(
        mailbox.create_draft("m-1", &bad, "k").unwrap_err().code,
        "invalid_input"
    );
    assert_eq!(
        mailbox
            .create_draft("m-1", &draft("  "), "k")
            .unwrap_err()
            .code,
        "invalid_input"
    );
    assert_eq!(
        mailbox
            .create_draft("m-1", &draft("Hola"), " ")
            .unwrap_err()
            .code,
        "invalid_input"
    );
    assert!(world.lock().unwrap().requests.is_empty());
}

#[test]
fn draft_is_sent_once_even_if_the_response_is_lost() {
    let (mailbox, world) = mailbox();
    let id = mailbox
        .create_draft("m-1", &draft("Gracias."), "task-1/create_draft/v1")
        .unwrap();
    world.lock().unwrap().drop_send_response = true;
    let error = mailbox.send_draft(&id, "task-1/send_draft/v1").unwrap_err();
    assert_eq!(error.code, "unavailable");
    assert_eq!(world.lock().unwrap().sent.len(), 1, "el envío ocurrió");

    // El núcleo pregunta antes de repetir: el borrador ya no es borrador, se confirma.
    let receipt = mailbox.lookup("task-1/send_draft/v1").unwrap().unwrap();
    assert_eq!(receipt["receipt_id"], json!(id));
    assert_eq!(receipt["message_id"], "m-1");
    let again = mailbox.send_draft(&id, "task-1/send_draft/v1").unwrap();
    assert_eq!(again.receipt_id, id);
    assert_eq!(
        world.lock().unwrap().sent.len(),
        1,
        "nunca se envía dos veces"
    );
    assert_eq!(count(&world, "POST", "/send"), 1);
}

#[test]
fn failed_send_without_effect_is_sent_on_retry() {
    let (mailbox, world) = mailbox();
    let id = mailbox
        .create_draft("m-1", &draft("Gracias."), "k-draft")
        .unwrap();
    world.lock().unwrap().send_status = Some(503);
    assert_eq!(
        mailbox.send_draft(&id, "k-send").unwrap_err().code,
        "unavailable"
    );
    world.lock().unwrap().send_status = None;

    assert_eq!(
        mailbox.lookup("k-send").unwrap(),
        None,
        "sigue siendo borrador"
    );
    let receipt = mailbox
        .execute("k-send", &json!({ "draft_id": id }))
        .unwrap();
    assert_eq!(receipt["draft_id"], json!(id));
    assert_eq!(world.lock().unwrap().sent, [id]);
}

#[test]
fn same_key_with_another_draft_or_another_key_for_a_sent_draft_conflicts() {
    let (mailbox, world) = mailbox();
    let a = mailbox.create_draft("m-1", &draft("A"), "k-a").unwrap();
    let b = mailbox.create_draft("m-1", &draft("B"), "k-b").unwrap();
    mailbox.send_draft(&a, "send-a").unwrap();
    assert_eq!(
        mailbox.send_draft(&b, "send-a").unwrap_err().code,
        "conflict"
    );
    assert_eq!(
        mailbox.send_draft(&a, "send-otra").unwrap_err().code,
        "conflict"
    );
    assert_eq!(world.lock().unwrap().sent, [a]);
}

#[test]
fn deleted_draft_is_reported_and_never_resent() {
    let (mailbox, world) = mailbox();
    let id = mailbox
        .create_draft("m-1", &draft("Gracias."), "k-draft")
        .unwrap();
    world.lock().unwrap().send_status = Some(503);
    let _ = mailbox.send_draft(&id, "k-send");
    {
        let mut w = world.lock().unwrap();
        w.send_status = None;
        w.drafts.clear();
    }
    assert_eq!(mailbox.lookup("k-send").unwrap_err().code, "not_found");
    assert!(world.lock().unwrap().sent.is_empty());
}

#[test]
fn unknown_draft_is_not_found_without_calling_the_provider() {
    let (mailbox, world) = mailbox();
    assert_eq!(
        mailbox.send_draft("ajeno", "k").unwrap_err().code,
        "not_found"
    );
    assert!(world.lock().unwrap().requests.is_empty());
}

#[test]
fn expired_or_rejected_token_is_renewed_once() {
    let (base, world) = serve(handler);
    world.lock().unwrap().messages = sample_messages();
    let (source, _) = tokens(&base, now() - 10);
    let mailbox = Mailbox::in_memory(Graph::new(&format!("{base}/v1.0"), source, 2).unwrap());
    mailbox.list_messages(None).unwrap();
    assert_eq!(
        count(&world, "POST", "/token"),
        1,
        "caducado: se renueva antes de llamar"
    );

    // El proveedor revoca el token vigente: un 401 provoca una renovación y un reintento.
    world.lock().unwrap().access_token = "revocado".into();
    mailbox.list_messages(None).unwrap();
    assert_eq!(count(&world, "POST", "/token"), 2);

    // Si tampoco se puede renovar, se pide iniciar sesión y no se insiste.
    {
        let mut w = world.lock().unwrap();
        w.access_token = "revocado".into();
        w.refresh_ok = false;
    }
    assert_eq!(
        mailbox.list_messages(None).unwrap_err().code,
        "unauthorized"
    );
    assert_eq!(count(&world, "POST", "/token"), 3);
}

#[test]
fn rejected_refresh_asks_to_sign_in_again_without_leaking_tokens() {
    let (base, world) = serve(handler);
    world.lock().unwrap().refresh_ok = false;
    let (source, _) = tokens(&base, now() - 10);
    let mailbox = Mailbox::in_memory(Graph::new(&format!("{base}/v1.0"), source, 2).unwrap());
    let error = mailbox.list_messages(None).unwrap_err();
    assert_eq!(error.code, "unauthorized");
    let shown = format!("{error} {error:?}");
    assert!(
        !shown.contains("refresh-0") && !shown.contains("access-0"),
        "{shown}"
    );
}

#[test]
fn provider_errors_map_to_contract_codes() {
    for (status, code) in [
        (403, "unauthorized"),
        (404, "not_found"),
        (400, "invalid_input"),
        (409, "conflict"),
        (429, "unavailable"),
        (500, "unavailable"),
    ] {
        let (mailbox, world) = mailbox();
        world.lock().unwrap().force = Some(("/inbox".into(), status));
        assert_eq!(
            mailbox.list_messages(None).unwrap_err().code,
            code,
            "HTTP {status}"
        );
    }
    let (mailbox, _) = mailbox();
    assert_eq!(
        mailbox
            .create_draft("no-existe", &draft("Hola"), "k")
            .unwrap_err()
            .code,
        "not_found"
    );
}

#[test]
fn local_record_survives_a_restart() {
    let (base, world) = serve(handler);
    world.lock().unwrap().messages = sample_messages();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("correo").join("cuenta-prueba.json");
    let open = || {
        let (source, _) = tokens(&base, now() + 3600);
        Mailbox::open(
            Graph::new(&format!("{base}/v1.0"), source, 2).unwrap(),
            &path,
        )
        .unwrap()
    };
    let id = {
        let mailbox = open();
        let id = mailbox
            .create_draft("m-1", &draft("Gracias."), "k-draft")
            .unwrap();
        world.lock().unwrap().drop_send_response = true;
        let _ = mailbox.send_draft(&id, "k-send");
        id
    };
    // Tras reiniciar, la misma clave devuelve el mismo borrador y el envío se reconcilia.
    let mailbox = open();
    assert_eq!(
        mailbox
            .create_draft("m-1", &draft("Gracias."), "k-draft")
            .unwrap(),
        id
    );
    assert!(mailbox.lookup("k-send").unwrap().is_some());
    assert_eq!(count(&world, "POST", "/createReply"), 1);
    assert_eq!(world.lock().unwrap().sent.len(), 1);
}

fn with_attachments(world: &common::Shared) {
    let mut w = world.lock().unwrap();
    w.messages[0]["hasAttachments"] = json!(true);
    w.messages[0]["_attachments"] = json!([
        {
            "@odata.type": "#microsoft.graph.fileAttachment",
            "id": "adj=1",
            "name": "factura-octubre.pdf",
            "contentType": "application/pdf",
            "size": 9,
            "isInline": false,
            "contentBytes": base64::Engine::encode(&base64::engine::general_purpose::STANDARD, b"%PDF-sint"),
        },
        {
            "@odata.type": "#microsoft.graph.fileAttachment",
            "id": "adj=2",
            "name": "logo.png",
            "contentType": "image/png",
            "size": 3,
            "isInline": true,
            "contentBytes": "AAAA",
        },
        {
            "@odata.type": "#microsoft.graph.itemAttachment",
            "id": "adj=3",
            "name": "Mensaje reenviado",
            "isInline": false,
        },
    ]);
}

#[test]
fn file_attachments_are_listed_without_inline_images_and_downloaded() {
    let (mailbox, world) = mailbox();
    with_attachments(&world);
    let page = mailbox.list_messages(None).unwrap();
    let list = &page.messages[0].attachments;
    assert_eq!(
        list.len(),
        1,
        "sin incrustados ni elementos de Outlook: {list:?}"
    );
    assert_eq!(list[0].id, "adj=1");
    assert_eq!(list[0].name.as_deref(), Some("factura-octubre.pdf"));
    assert_eq!(list[0].mime.as_deref(), Some("application/pdf"));
    assert_eq!(list[0].size, Some(9));
    assert!(page.messages[1].attachments.is_empty());
    {
        let w = world.lock().unwrap();
        let listing = w
            .requests
            .iter()
            .filter(|r| r.url.path().ends_with("/attachments"))
            .collect::<Vec<_>>();
        assert_eq!(listing.len(), 1, "solo se piden los adjuntos de m-1");
        assert!(listing[0].query("$select").unwrap().contains("isInline"));
    }

    assert_eq!(
        mailbox.fetch_attachment("m-1", "adj=1").unwrap(),
        b"%PDF-sint"
    );
    let other = mailbox.fetch_attachment("m-1", "adj=3").unwrap_err();
    assert_eq!(
        other.code, "invalid_input",
        "un elemento de Outlook no se descarga"
    );
    assert_eq!(
        mailbox
            .fetch_attachment("m-1", "no-existe")
            .unwrap_err()
            .code,
        "not_found"
    );
    assert_eq!(
        mailbox.fetch_attachment("m-1", " ").unwrap_err().code,
        "invalid_input"
    );
}
