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
        ("GET", ["me", "mailFolders", "sentitems", "messages"]) => {
            // Como Graph: el filtro es una expresión OData; la comilla simple se duplica.
            let filter = req.query("$filter").unwrap_or_default();
            let Some(quoted) = filter.strip_prefix("conversationId eq '") else {
                return Resp::Json(400, json!({ "error": { "code": "BadFilter" } }));
            };
            let Some(quoted) = quoted.strip_suffix('\'') else {
                return Resp::Json(400, json!({ "error": { "code": "BadFilter" } }));
            };
            if quoted.replace("''", "").contains('\'') {
                return Resp::Json(400, json!({ "error": { "code": "BadFilter" } }));
            }
            let conversation = quoted.replace("''", "'");
            let list: Vec<Value> = w
                .messages
                .iter()
                .filter(|m| m["_sent"] == true && m["conversationId"] == conversation.as_str())
                .cloned()
                .collect();
            Resp::Json(200, json!({ "value": list }))
        }
        ("GET", ["me", "messages", id]) if w.messages.iter().any(|m| m["id"] == *id) => {
            let m = w.messages.iter().find(|m| m["id"] == *id).unwrap();
            // Como Graph con `$select`: solo lo pedido.
            let wanted = req.query("$select").unwrap_or_default();
            let mut out = json!({ "id": id, "receivedDateTime": m["receivedDateTime"] });
            for field in ["from", "uniqueBody"] {
                if wanted.split(',').any(|w| w == field) {
                    out[field] = m[field].clone();
                }
            }
            Resp::Json(200, out)
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
            "conversationId": "c-1",
            "internetMessageHeaders": [
                { "name": "Precedence", "value": "Bulk" },
                { "name": "X-Interna", "value": "CENTINELA-5d20" },
            ],
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
fn inbox_messages_carry_signals_only_when_headers_arrive() {
    let (mailbox, world) = mailbox();
    let first = mailbox.list_messages(None).unwrap();
    let m1 = &first.messages[0];
    assert_eq!(m1.signals, Some(vec!["precedence_bulk".to_owned()]));
    assert_eq!(m1.thread_key.as_deref(), Some("c-1"));
    assert!(!serde_json::to_string(&first.messages)
        .unwrap()
        .contains("CENTINELA"));
    // Sin `internetMessageHeaders`: no comprobado (`None`), y sin hilo.
    let m2 = &first.messages[1];
    assert_eq!(m2.signals, None);
    assert_eq!(m2.thread_key, None);
    let list = world
        .lock()
        .unwrap()
        .requests
        .iter()
        .find(|r| r.url.path().ends_with("/inbox/messages"))
        .and_then(|r| r.query("$select"))
        .unwrap();
    assert!(list.contains("internetMessageHeaders") && list.contains("conversationId"));
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

/// Estado del hilo (D162): Graph no lo implementa (ver la documentación de `graph.rs`), así que
/// siempre es desconocido y no hace ninguna llamada: el correo sigue el flujo normal.
#[test]
fn graph_reports_the_thread_state_as_unknown_without_calling_the_service() {
    let (mailbox, world) = mailbox();
    assert_eq!(
        mailbox.last_message_is_ours("conversacion-1").unwrap(),
        None
    );
    assert!(world.lock().unwrap().requests.is_empty());
}

fn sent_item(id: &str, conversation: &str, sent: &str, body: &str) -> Value {
    json!({
        "id": id, "_sent": true, "conversationId": conversation, "sentDateTime": sent,
        "body": { "contentType": "text", "content": body },
        "toRecipients": [{ "emailAddress": { "address": "ana@cliente.test" } }],
    })
}

/// T33a (Graph): la primera respuesta de `sentitems` posterior al recibido; ignora otras
/// conversaciones y respuestas anteriores; sin respuesta, `None`.
#[test]
fn graph_finds_the_first_own_reply_after_the_received_message() {
    let (mailbox, world) = mailbox();
    assert!(mailbox.supports_sent());
    world.lock().unwrap().messages.extend([
        // Anterior al recibido (m-1: 2026-11-02T09:30:00Z).
        sent_item("s-0", "c-1", "2026-11-01T08:00:00Z", "respuesta vieja"),
        // Dos posteriores, desordenadas: gana la más temprana.
        sent_item("s-2", "c-1", "2026-11-04T10:00:00Z", "segunda"),
        sent_item(
            "s-1",
            "c-1",
            "2026-11-03T10:00:00Z",
            "Hola,\r\nLo vemos hoy.\r\n",
        ),
        // Otra conversación.
        sent_item("s-x", "c-9", "2026-11-03T09:00:00Z", "ajena"),
    ]);
    let reply = mailbox.own_reply_in_thread("c-1", "m-1").unwrap().unwrap();
    assert_eq!(reply.id, "s-1");
    assert_eq!(reply.date.as_deref(), Some("2026-11-03T10:00:00Z"));
    assert_eq!(reply.body.as_deref(), Some("Hola,\r\nLo vemos hoy."));
    // M2: a quién iba la respuesta (aprender descarta un reenvío interno).
    assert_eq!(reply.to, vec!["ana@cliente.test".to_owned()]);
    // Conversación sin respuestas, o mensaje desconocido: `None`, no error.
    assert_eq!(mailbox.own_reply_in_thread("c-7", "m-1").unwrap(), None);
    assert_eq!(
        mailbox.own_reply_in_thread("c-1", "no-existe").unwrap(),
        None
    );
    // Claves vacías: sin llamada.
    let before = world.lock().unwrap().requests.len();
    assert_eq!(mailbox.own_reply_in_thread(" ", "m-1").unwrap(), None);
    assert_eq!(world.lock().unwrap().requests.len(), before);
}

/// El filtro duplica la comilla simple del `conversationId`; sin ello se podría inyectar OData.
#[test]
fn graph_sent_filter_escapes_a_quote_in_the_conversation_id() {
    let (mailbox, world) = mailbox();
    let hostile = "c-1' or 'a'='a";
    world.lock().unwrap().messages.extend([
        sent_item("s-h", hostile, "2026-11-03T10:00:00Z", "solo de la hostil"),
        sent_item("s-1", "c-1", "2026-11-03T11:00:00Z", "de c-1"),
    ]);
    let reply = mailbox
        .own_reply_in_thread(hostile, "m-1")
        .unwrap()
        .unwrap();
    assert_eq!(reply.id, "s-h");
    let w = world.lock().unwrap();
    let filter = w
        .requests
        .iter()
        .find(|r| r.url.path().ends_with("/sentitems/messages"))
        .and_then(|r| r.query("$filter"))
        .unwrap();
    assert_eq!(filter, "conversationId eq 'c-1'' or ''a''=''a'");
}

/// Sin fecha de recepción no se puede ordenar: no hay respuesta, y no se consulta Enviados.
#[test]
fn graph_without_a_received_date_gives_no_reply() {
    let (mailbox, world) = mailbox();
    world
        .lock()
        .unwrap()
        .messages
        .push(sent_item("s-1", "c-1", "2026-11-03T10:00:00Z", "x"));
    // m-2 no tiene `receivedDateTime`.
    assert_eq!(mailbox.own_reply_in_thread("c-1", "m-2").unwrap(), None);
    assert_eq!(count(&world, "GET", "/sentitems/messages"), 0);
}

/// A1: con `uniqueBody` (sin la cita del hilo) la respuesta propia no lleva el historial, que al
/// convertir el HTML pierde las `>` de `<blockquote>`; sin él, el cuerpo de siempre. Y los
/// destinatarios salen de `toRecipients`.
#[test]
fn graph_sent_reply_prefers_the_unique_body_and_carries_the_recipients() {
    let (mailbox, world) = mailbox();
    let mut unique = sent_item(
        "s-1",
        "c-1",
        "2026-11-03T10:00:00Z",
        "Hola, vale.\nEl lun, Ana escribió:\nhistorial sin marcas",
    );
    unique["uniqueBody"] = json!({ "contentType": "html", "content": "<p>Hola, vale.</p>" });
    unique["toRecipients"] = json!([
        { "emailAddress": { "address": "ana@cliente.test" } },
        { "emailAddress": { "address": "jefe@empresa.test" } },
    ]);
    world.lock().unwrap().messages.push(unique);
    let reply = mailbox.own_reply_in_thread("c-1", "m-1").unwrap().unwrap();
    assert_eq!(reply.body.as_deref(), Some("Hola, vale."));
    assert_eq!(
        reply.to,
        vec![
            "ana@cliente.test".to_owned(),
            "jefe@empresa.test".to_owned()
        ]
    );
    let w = world.lock().unwrap();
    let select = w
        .requests
        .iter()
        .find(|r| r.url.path().ends_with("/sentitems/messages"))
        .and_then(|r| r.query("$select"))
        .unwrap();
    assert!(select.contains("uniqueBody") && select.contains("toRecipients"));
}

/// Una respuesta sin `toRecipients` queda con la lista vacía: aprender no sabe a quién iba.
#[test]
fn graph_sent_reply_without_recipients_has_an_empty_list() {
    let (mailbox, world) = mailbox();
    let mut item = sent_item("s-1", "c-1", "2026-11-03T10:00:00Z", "ok");
    item.as_object_mut().unwrap().remove("toRecipients");
    world.lock().unwrap().messages.push(item);
    let reply = mailbox.own_reply_in_thread("c-1", "m-1").unwrap().unwrap();
    assert!(reply.to.is_empty());
}

/// A1/M1: el recibido da su cuerpo sin cita y el nombre visible del remitente, solo de lectura.
#[test]
fn graph_received_extra_gives_the_unique_body_and_the_sender_name() {
    let (mailbox, world) = mailbox();
    world.lock().unwrap().messages[0]["uniqueBody"] =
        json!({ "contentType": "text", "content": "Adjunto la factura.\r\n" });
    let extra = mailbox.received_extra("m-1").unwrap();
    assert_eq!(extra.unique_body.as_deref(), Some("Adjunto la factura."));
    assert_eq!(extra.sender_name.as_deref(), Some("Ana"));
    // Sin nombre ni cuerpo sin cita: nada.
    let none = mailbox.received_extra("m-2").unwrap();
    assert_eq!((none.unique_body, none.sender_name), (None, None));
    // Un mensaje que ya no existe no es un error; un identificador vacío no llama al servicio.
    assert_eq!(
        mailbox.received_extra("no-existe").unwrap(),
        Default::default()
    );
    let before = world.lock().unwrap().requests.len();
    assert_eq!(mailbox.received_extra(" ").unwrap(), Default::default());
    assert_eq!(world.lock().unwrap().requests.len(), before);
    let w = world.lock().unwrap();
    assert!(w.requests.iter().all(|r| r.method == "GET"));
}

/// D178: todas las respuestas propias posteriores al recibido, por fecha, solo con metadatos. El
/// `$select` de Enviados no pide `body` ni `uniqueBody`.
#[test]
fn graph_own_replies_meta_lists_every_later_reply_without_asking_for_a_body() {
    let (mailbox, world) = mailbox();
    world.lock().unwrap().messages.extend([
        sent_item("s-0", "c-1", "2026-11-01T08:00:00Z", "vieja"),
        sent_item("s-2", "c-1", "2026-11-04T10:00:00Z", "segunda"),
        sent_item("s-1", "c-1", "2026-11-03T10:00:00Z", "primera"),
        sent_item("s-x", "c-9", "2026-11-03T09:00:00Z", "ajena"),
    ]);
    let metas = mailbox.own_replies_meta("c-1", "m-1").unwrap();
    assert_eq!(
        metas.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
        ["s-1", "s-2"]
    );
    assert_eq!(metas[0].date.as_deref(), Some("2026-11-03T10:00:00Z"));
    assert_eq!(metas[0].to, vec!["ana@cliente.test".to_owned()]);
    {
        let w = world.lock().unwrap();
        let select = w
            .requests
            .iter()
            .find(|r| r.url.path().ends_with("/sentitems/messages"))
            .and_then(|r| r.query("$select"))
            .unwrap();
        assert!(!select.to_lowercase().contains("body"), "{select}");
        assert!(select.contains("toRecipients") && select.contains("sentDateTime"));
    }
    assert!(mailbox.own_replies_meta("c-7", "m-1").unwrap().is_empty());
    assert!(mailbox
        .own_replies_meta("c-1", "no-existe")
        .unwrap()
        .is_empty());
    let before = world.lock().unwrap().requests.len();
    assert!(mailbox.own_replies_meta(" ", "m-1").unwrap().is_empty());
    assert_eq!(world.lock().unwrap().requests.len(), before);
}
