//! E4b (D229, D230, D231): mensaje nuevo con un PDF adjunto en Gmail y Graph, contra el HTTP
//! falso de `tests/common`. PDF y direcciones sintéticos; ninguna prueba sale a la red.

mod common;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use common::{count, now, serve, tokens, Req, Resp, Shared, World};
use nestadmin_core::Connector;
use nestadmin_mail::{CreatedDraft, DraftStatus, Gmail, Graph, MailProvider, Mailbox};
use nestadmin_simulators::{
    MailConnector, MessagePage, NewDraft, OutgoingAttachment, MAX_OUTGOING_ATTACHMENT_BYTES,
};
use serde_json::{json, Value};

const GMAIL_BASE: &str = "/gmail/v1/users/me";
const MARKER: &str = "MARCA-SINTETICA-7f3a";

/// PDF sintético mínimo con una marca reconocible y, si hace falta, relleno hasta `len`.
fn pdf(len: usize) -> Vec<u8> {
    let mut bytes = format!("%PDF-1.4\n%{MARKER}\n%%EOF\n").into_bytes();
    if bytes.len() < len {
        bytes.resize(len, b' ');
    }
    bytes
}

fn pdf_draft_with(content: Vec<u8>) -> NewDraft {
    NewDraft {
        to: vec!["cliente@ejemplo.test".into()],
        subject: Some("Factura F-2026-0001 — Empresa Ejemplo".into()),
        body: "Hola, Cliente Ejemplo:\n\nLe adjuntamos la factura F-2026-0001.\n\nUn saludo,\nEmpresa Ejemplo"
            .into(),
        attachments: vec![OutgoingAttachment::pdf("F-2026-0001.pdf", content)],
    }
}

fn pdf_draft() -> NewDraft {
    pdf_draft_with(pdf(0))
}

fn not_found() -> Resp {
    Resp::Json(404, json!({ "error": { "status": "NOT_FOUND" } }))
}

// ---------------------------------------------------------------- Gmail

fn gmail_handler(req: &Req, w: &mut World) -> Resp {
    let path = req.url.path().to_owned();
    let api = path.strip_prefix(GMAIL_BASE).map(str::to_owned);
    match (req.method.as_str(), path.as_str(), api.as_deref()) {
        ("POST", "/upload/gmail/v1/users/me/drafts", _) => {
            let n = w.drafts.len() + 1;
            let (draft_id, message_id, thread) =
                (format!("r-{n}"), format!("dm-{n}"), format!("t-{n}"));
            w.drafts.push((
                draft_id.clone(),
                message_id.clone(),
                json!({ "mime": req.body, "threadId": thread, "labelIds": ["DRAFT"] }),
            ));
            Resp::Json(
                200,
                json!({ "id": draft_id, "message": { "id": message_id, "threadId": thread } }),
            )
        }
        ("POST", _, Some("/drafts/send")) => {
            let id = req.json()["id"].as_str().unwrap_or("").to_owned();
            let Some(index) = w.drafts.iter().position(|d| d.0 == id) else {
                return not_found();
            };
            let (_, _, stored) = w.drafts.remove(index);
            let own = stored["mime"]
                .as_str()
                .unwrap_or("")
                .lines()
                .find_map(|l| l.strip_prefix("X-NestAdmin-Id:"))
                .map(|v| v.trim().to_owned())
                .unwrap_or_default();
            let sent_id = format!("s-{}", w.sent.len() + 1);
            w.messages.push(json!({
                "id": sent_id,
                "threadId": stored["threadId"],
                "labelIds": ["SENT"],
                "payload": { "headers": [{ "name": "X-NestAdmin-Id", "value": own }] },
            }));
            w.sent.push(sent_id.clone());
            if w.drop_send_response {
                Resp::Drop
            } else {
                Resp::Json(200, json!({ "id": sent_id, "labelIds": ["SENT"] }))
            }
        }
        ("GET", _, Some(rest)) => {
            let segments: Vec<&str> = rest.trim_start_matches('/').split('/').collect();
            match segments.as_slice() {
                ["drafts", id] => match w.drafts.iter().find(|d| d.0 == *id) {
                    Some(_) => Resp::Json(200, json!({ "id": id })),
                    None => not_found(),
                },
                ["messages", id] => match w.messages.iter().find(|m| m["id"] == *id) {
                    Some(m) => Resp::Json(200, m.clone()),
                    None => not_found(),
                },
                ["threads", id] => {
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
                _ => not_found(),
            }
        }
        _ => not_found(),
    }
}

fn gmail_mailbox() -> (Mailbox<Gmail>, Shared) {
    let (base, world) = serve(gmail_handler);
    let (source, _) = tokens(&base, now() + 3600);
    let gmail = Gmail::new(&format!("{base}{GMAIL_BASE}"), source, 10).unwrap();
    (Mailbox::in_memory(gmail), world)
}

/// MIME guardado por el HTTP falso: cabeceras y cuerpo.
fn stored_mime(world: &Shared, index: usize) -> String {
    world.lock().unwrap().drafts[index].2["mime"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn header_of<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    head.split("\r\n").find_map(|line| {
        let (n, v) = line.split_once(':')?;
        n.eq_ignore_ascii_case(name).then_some(v.trim())
    })
}

fn decode_part(body: &str) -> Vec<u8> {
    STANDARD
        .decode(body.replace("\r\n", "").trim())
        .expect("base64 válido")
}

#[test]
fn gmail_new_message_is_multipart_mixed_sent_through_the_media_upload() {
    let (mailbox, world) = gmail_mailbox();
    let draft = pdf_draft();
    let id = mailbox
        .create_new_draft(&draft, "task-1/create/v1")
        .unwrap();
    assert_eq!(id, "r-1");

    let requests = world.lock().unwrap().requests.clone();
    assert_eq!(
        requests.len(),
        1,
        "una sola petición: sin la API JSON de drafts"
    );
    let req = &requests[0];
    assert_eq!(req.method, "POST");
    assert_eq!(req.url.path(), "/upload/gmail/v1/users/me/drafts");
    assert_eq!(req.query("uploadType").as_deref(), Some("media"));
    assert_eq!(req.header("content-type"), Some("message/rfc822"));

    let mime = stored_mime(&world, 0);
    let (head, rest) = mime.split_once("\r\n\r\n").expect("cabeceras y cuerpo");
    assert_eq!(header_of(head, "To"), Some("cliente@ejemplo.test"));
    // Asunto con caracteres no ASCII: codificado RFC 2047.
    let subject = header_of(head, "Subject").unwrap();
    assert!(subject.starts_with("=?UTF-8?B?"), "{subject}");
    let decoded = STANDARD
        .decode(
            subject
                .trim_start_matches("=?UTF-8?B?")
                .trim_end_matches("?="),
        )
        .unwrap();
    assert_eq!(
        String::from_utf8(decoded).unwrap(),
        "Factura F-2026-0001 — Empresa Ejemplo"
    );
    let own = header_of(head, "X-NestAdmin-Id").unwrap();
    assert!(own.starts_with("<nestadmin.") && own.ends_with("@nestadmin.invalid>"));
    assert_eq!(header_of(head, "Message-ID"), Some(own));
    assert!(header_of(head, "In-Reply-To").is_none(), "fuera de hilo");
    assert!(header_of(head, "References").is_none(), "fuera de hilo");
    assert_eq!(header_of(head, "MIME-Version"), Some("1.0"));

    let content_type = header_of(head, "Content-Type").unwrap();
    let boundary = content_type
        .strip_prefix("multipart/mixed; boundary=\"")
        .and_then(|b| b.strip_suffix('"'))
        .expect("multipart/mixed con frontera");
    assert!(boundary.starts_with("=_NestAdmin_"), "{boundary}");

    let closing = format!("--{boundary}--");
    assert!(rest.trim_end().ends_with(&closing));
    let body = rest.split(&closing).next().unwrap();
    let parts: Vec<&str> = body
        .split(&format!("--{boundary}"))
        .filter(|p| !p.trim().is_empty())
        .collect();
    assert_eq!(parts.len(), 2, "texto y PDF");

    let (text_head, text_body) = parts[0]
        .trim_start_matches("\r\n")
        .split_once("\r\n\r\n")
        .unwrap();
    assert_eq!(
        header_of(text_head, "Content-Type"),
        Some("text/plain; charset=UTF-8")
    );
    assert_eq!(
        header_of(text_head, "Content-Transfer-Encoding"),
        Some("base64")
    );
    assert_eq!(decode_part(text_body), draft.body.as_bytes());

    let (pdf_head, pdf_body) = parts[1]
        .trim_start_matches("\r\n")
        .split_once("\r\n\r\n")
        .unwrap();
    assert_eq!(
        header_of(pdf_head, "Content-Type"),
        Some("application/pdf; name=\"F-2026-0001.pdf\"")
    );
    assert_eq!(
        header_of(pdf_head, "Content-Disposition"),
        Some("attachment; filename=\"F-2026-0001.pdf\"")
    );
    assert_eq!(
        header_of(pdf_head, "Content-Transfer-Encoding"),
        Some("base64")
    );
    assert_eq!(decode_part(pdf_body), draft.attachments[0].content);
    for line in pdf_body.trim().split("\r\n") {
        assert!(line.len() <= 76, "base64 en líneas de 76");
    }
}

#[test]
fn gmail_new_message_without_attachment_is_plain_text() {
    let (mailbox, world) = gmail_mailbox();
    let mut draft = pdf_draft();
    draft.attachments.clear();
    mailbox.create_new_draft(&draft, "k").unwrap();
    let mime = stored_mime(&world, 0);
    let (head, body) = mime.split_once("\r\n\r\n").unwrap();
    assert_eq!(
        header_of(head, "Content-Type"),
        Some("text/plain; charset=UTF-8")
    );
    assert_eq!(decode_part(body), draft.body.as_bytes());
}

#[test]
fn gmail_accepts_exactly_the_size_limit_and_rejects_one_byte_more() {
    let (mailbox, world) = gmail_mailbox();
    let limit = MAX_OUTGOING_ATTACHMENT_BYTES as usize;
    assert_eq!(limit, 3_000_000);
    mailbox
        .create_new_draft(&pdf_draft_with(pdf(limit)), "k-limit")
        .unwrap();
    assert_eq!(count(&world, "POST", "/drafts"), 1);

    let before = world.lock().unwrap().requests.len();
    let e = mailbox
        .create_new_draft(&pdf_draft_with(pdf(limit + 1)), "k-over")
        .unwrap_err();
    assert_eq!(e.code, "invalid_input");
    assert_eq!(
        world.lock().unwrap().requests.len(),
        before,
        "el proveedor no se llama"
    );
}

#[test]
fn graph_accepts_the_size_limit_in_one_request_below_the_graph_ceiling() {
    let (mailbox, world) = graph_mailbox();
    let limit = MAX_OUTGOING_ATTACHMENT_BYTES as usize;
    mailbox
        .create_new_draft(&pdf_draft_with(pdf(limit)), "k-limit")
        .unwrap();
    let requests = world.lock().unwrap().requests.clone();
    assert_eq!(requests.len(), 1);
    assert!(
        requests[0].body.len() < 4_194_304,
        "cuerpo de {} bytes: Graph admite 4 MiB en una sola petición",
        requests[0].body.len()
    );
    assert!(requests[0].body.len() > 4_000_000, "va en base64");
    let e = mailbox
        .create_new_draft(&pdf_draft_with(pdf(limit + 1)), "k-over")
        .unwrap_err();
    assert_eq!(e.code, "invalid_input");
    assert_eq!(world.lock().unwrap().requests.len(), 1);
}

// ---------------------------------------------------------------- Graph

fn graph_handler(req: &Req, w: &mut World) -> Resp {
    let path = req.url.path().trim_start_matches("/v1.0/").to_owned();
    let segments: Vec<&str> = path.split('/').collect();
    match (req.method.as_str(), segments.as_slice()) {
        ("POST", ["me", "messages"]) => {
            let id = format!("borrador={}", w.drafts.len() + 1);
            w.drafts.push((
                id.clone(),
                id.clone(),
                json!({ "id": id, "isDraft": true, "request": req.json() }),
            ));
            Resp::Json(201, json!({ "id": id }))
        }
        ("POST", ["me", "messages", id, "send"]) => {
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
        ("GET", ["me", "messages", id]) => match w.drafts.iter().find(|d| d.0 == *id) {
            Some(draft) => Resp::Json(200, json!({ "id": id, "isDraft": draft.2["isDraft"] })),
            None => Resp::Json(404, json!({ "error": { "code": "ErrorItemNotFound" } })),
        },
        _ => Resp::Json(404, json!({ "error": { "code": "NoRoute" } })),
    }
}

fn graph_mailbox() -> (Mailbox<Graph>, Shared) {
    let (base, world) = serve(graph_handler);
    let (source, _) = tokens(&base, now() + 3600);
    let graph = Graph::new(&format!("{base}/v1.0"), source, 2).unwrap();
    (Mailbox::in_memory(graph), world)
}

#[test]
fn graph_new_message_is_one_post_with_a_file_attachment() {
    let (mailbox, world) = graph_mailbox();
    let draft = pdf_draft();
    let id = mailbox
        .create_new_draft(&draft, "task-1/create/v1")
        .unwrap();
    assert_eq!(id, "borrador=1");

    let requests = world.lock().unwrap().requests.clone();
    assert_eq!(
        requests.len(),
        1,
        "un único POST: atómico, sin createReply ni subida"
    );
    let req = &requests[0];
    assert_eq!(req.method, "POST");
    assert_eq!(req.url.path(), "/v1.0/me/messages");
    let body = req.json();
    assert_eq!(body["subject"], "Factura F-2026-0001 — Empresa Ejemplo");
    assert_eq!(body["body"]["contentType"], "Text");
    assert_eq!(body["body"]["content"], draft.body.as_str());
    assert_eq!(
        body["toRecipients"],
        json!([{ "emailAddress": { "address": "cliente@ejemplo.test" } }])
    );
    let attachments = body["attachments"].as_array().unwrap();
    assert_eq!(attachments.len(), 1);
    assert_eq!(
        attachments[0]["@odata.type"],
        "#microsoft.graph.fileAttachment"
    );
    assert_eq!(attachments[0]["name"], "F-2026-0001.pdf");
    assert_eq!(attachments[0]["contentType"], "application/pdf");
    assert_eq!(
        STANDARD
            .decode(attachments[0]["contentBytes"].as_str().unwrap())
            .unwrap(),
        draft.attachments[0].content
    );
}

// ------------------------------------------------- validación (ambos proveedores)

fn bad(change: impl FnOnce(&mut NewDraft)) -> NewDraft {
    let mut draft = pdf_draft();
    change(&mut draft);
    draft
}

fn attachment(draft: &mut NewDraft) -> &mut OutgoingAttachment {
    &mut draft.attachments[0]
}

/// Contenidos que se rechazan sin llamar al proveedor (D229).
fn rejected_drafts() -> Vec<(&'static str, NewDraft)> {
    vec![
        (
            "nombre con comilla",
            bad(|d| attachment(d).name = "a\".pdf".into()),
        ),
        (
            "nombre con CRLF",
            bad(|d| attachment(d).name = "a\r\nBcc: x@evil.test.pdf".into()),
        ),
        (
            "nombre con barra",
            bad(|d| attachment(d).name = "../a.pdf".into()),
        ),
        (
            "nombre con barra invertida",
            bad(|d| attachment(d).name = "a\\b.pdf".into()),
        ),
        (
            "nombre con espacio",
            bad(|d| attachment(d).name = "a b.pdf".into()),
        ),
        (
            "nombre sin extensión",
            bad(|d| attachment(d).name = "factura".into()),
        ),
        (
            "nombre .PDF",
            bad(|d| attachment(d).name = "factura.PDF".into()),
        ),
        ("nombre vacío", bad(|d| attachment(d).name = ".pdf".into())),
        (
            "nombre de 81 caracteres",
            bad(|d| attachment(d).name = format!("{}.pdf", "a".repeat(81))),
        ),
        (
            "asunto con CRLF",
            bad(|d| d.subject = Some("Hola\r\nBcc: x@evil.test".into())),
        ),
        (
            "asunto con LF",
            bad(|d| d.subject = Some("Hola\nBcc: x@evil.test".into())),
        ),
        ("asunto con NUL", bad(|d| d.subject = Some("Hola\0".into()))),
        ("asunto vacío", bad(|d| d.subject = Some("   ".into()))),
        ("sin asunto", bad(|d| d.subject = None)),
        (
            "asunto de 201 caracteres",
            bad(|d| d.subject = Some("a".repeat(201))),
        ),
        ("cuerpo vacío", bad(|d| d.body = "  ".into())),
        ("sin destinatario", bad(|d| d.to.clear())),
        (
            "dos destinatarios",
            bad(|d| d.to = vec!["a@ejemplo.test".into(), "b@ejemplo.test".into()]),
        ),
        (
            "lista en un destinatario",
            bad(|d| d.to = vec!["a@ejemplo.test, b@ejemplo.test".into()]),
        ),
        (
            "nombre y ángulos",
            bad(|d| d.to = vec!["Ana <a@ejemplo.test>".into()]),
        ),
        (
            "solo ángulos",
            bad(|d| d.to = vec!["<a@ejemplo.test>".into()]),
        ),
        (
            "punto y coma",
            bad(|d| d.to = vec!["a@ejemplo.test;b@evil.test".into()]),
        ),
        (
            "destinatario con CRLF",
            bad(|d| d.to = vec!["a@ejemplo.test\r\nBcc: x@evil.test".into()]),
        ),
        (
            "destinatario sin dominio",
            bad(|d| d.to = vec!["a@ejemplo".into()]),
        ),
        ("dos adjuntos", {
            let mut d = pdf_draft();
            d.attachments
                .push(OutgoingAttachment::pdf("otro.pdf", pdf(0)));
            d
        }),
        (
            "MIME distinto",
            bad(|d| attachment(d).mime = "application/octet-stream".into()),
        ),
        (
            "MIME con parámetros",
            bad(|d| attachment(d).mime = "application/pdf; x=1".into()),
        ),
        (
            "bytes que no son un PDF",
            bad(|d| {
                *attachment(d) = OutgoingAttachment::pdf("a.pdf", b"MZ\x90\x00 no es pdf".to_vec())
            }),
        ),
        (
            "SHA-256 distinto",
            bad(|d| attachment(d).sha256 = "0".repeat(64)),
        ),
        (
            "tamaño declarado distinto",
            bad(|d| attachment(d).size += 1),
        ),
        (
            "tamaño declarado enorme",
            bad(|d| attachment(d).size = MAX_OUTGOING_ATTACHMENT_BYTES + 1),
        ),
    ]
}

fn assert_all_rejected_without_calls(mailbox: &impl MailConnector, world: &Shared) {
    for (case, draft) in rejected_drafts() {
        let e = mailbox.create_new_draft(&draft, "k-malo").expect_err(case);
        assert_eq!(e.code, "invalid_input", "{case}: {e:?}");
    }
    assert!(
        world.lock().unwrap().requests.is_empty(),
        "ningún contenido no válido llega al proveedor"
    );
    assert!(world.lock().unwrap().drafts.is_empty());
}

#[test]
fn gmail_rejects_unsafe_or_invalid_content_without_calling_the_provider() {
    let (mailbox, world) = gmail_mailbox();
    assert_all_rejected_without_calls(&mailbox, &world);
}

#[test]
fn graph_rejects_unsafe_or_invalid_content_without_calling_the_provider() {
    let (mailbox, world) = graph_mailbox();
    assert_all_rejected_without_calls(&mailbox, &world);
}

#[test]
fn a_missing_idempotency_key_is_rejected() {
    let (mailbox, world) = graph_mailbox();
    let e = mailbox.create_new_draft(&pdf_draft(), "  ").unwrap_err();
    assert_eq!(e.code, "invalid_input");
    assert!(world.lock().unwrap().requests.is_empty());
}

// ------------------------------------------------- reutilización y conflicto

fn assert_reuse_and_conflict(mailbox: &impl MailConnector, world: &Shared) {
    let first = mailbox.create_new_draft(&pdf_draft(), "k-1").unwrap();
    let calls = world.lock().unwrap().requests.len();
    // Misma clave y mismos metadatos: el mismo borrador, sin llamar otra vez.
    assert_eq!(
        mailbox.create_new_draft(&pdf_draft(), "k-1").unwrap(),
        first
    );
    assert_eq!(world.lock().unwrap().requests.len(), calls);
    assert_eq!(world.lock().unwrap().drafts.len(), 1);

    // Otro PDF (otro SHA-256) con la misma clave.
    let mut other = pdf(0);
    other.extend_from_slice(b"%otro contenido\n");
    let e = mailbox
        .create_new_draft(&pdf_draft_with(other), "k-1")
        .unwrap_err();
    assert_eq!(e.code, "conflict");
    // Otro asunto, otro cuerpo, otro destinatario, otro nombre.
    for changed in [
        bad(|d| d.subject = Some("Otro asunto".into())),
        bad(|d| d.body = "Otro cuerpo".into()),
        bad(|d| d.to = vec!["otro@ejemplo.test".into()]),
        bad(|d| *attachment(d) = OutgoingAttachment::pdf("otro-nombre.pdf", pdf(0))),
        bad(|d| d.attachments.clear()),
    ] {
        assert_eq!(
            mailbox.create_new_draft(&changed, "k-1").unwrap_err().code,
            "conflict"
        );
    }
    assert_eq!(world.lock().unwrap().requests.len(), calls);
    assert_eq!(world.lock().unwrap().drafts.len(), 1);
}

#[test]
fn gmail_reuses_the_draft_for_the_same_attachment_metadata_and_conflicts_otherwise() {
    let (mailbox, world) = gmail_mailbox();
    assert_reuse_and_conflict(&mailbox, &world);
}

#[test]
fn graph_reuses_the_draft_for_the_same_attachment_metadata_and_conflicts_otherwise() {
    let (mailbox, world) = graph_mailbox();
    assert_reuse_and_conflict(&mailbox, &world);
}

// ------------------------------------------------- estado guardado y respuestas

fn assert_state_has_no_pdf(path: &std::path::Path) {
    let saved = std::fs::read_to_string(path).unwrap();
    let draft = pdf_draft();
    let b64 = STANDARD.encode(&draft.attachments[0].content);
    assert!(
        !saved.contains(&b64),
        "el estado no guarda el PDF en base64"
    );
    assert!(!saved.contains(MARKER), "ni sus bytes");
    let json: Value = serde_json::from_str(&saved).unwrap();
    let attachment = &json["drafts"][0]["content"]["attachments"][0];
    assert_eq!(attachment["name"], "F-2026-0001.pdf");
    assert_eq!(attachment["mime"], "application/pdf");
    assert_eq!(attachment["size"], draft.attachments[0].size);
    assert_eq!(attachment["sha256"], draft.attachments[0].sha256.as_str());
    assert!(attachment.get("content").is_none());
    assert_eq!(json["drafts"][0]["standalone"], true);
    assert_eq!(json["drafts"][0]["message_id"], "");
}

#[test]
fn gmail_local_state_keeps_the_metadata_but_never_the_pdf() {
    let (base, world) = serve(gmail_handler);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cuenta.json");
    let (source, _) = tokens(&base, now() + 3600);
    let gmail = Gmail::new(&format!("{base}{GMAIL_BASE}"), source, 10).unwrap();
    let mailbox = Mailbox::open(gmail, &path).unwrap();
    let id = mailbox.create_new_draft(&pdf_draft(), "k-1").unwrap();
    assert_state_has_no_pdf(&path);

    // Proceso nuevo: el registro basta para reconocer la misma petición sin los bytes.
    let (source, _) = tokens(&base, now() + 3600);
    let gmail = Gmail::new(&format!("{base}{GMAIL_BASE}"), source, 10).unwrap();
    let reopened = Mailbox::open(gmail, &path).unwrap();
    assert_eq!(reopened.create_new_draft(&pdf_draft(), "k-1").unwrap(), id);
    assert_eq!(world.lock().unwrap().drafts.len(), 1);
}

#[test]
fn graph_local_state_keeps_the_metadata_but_never_the_pdf() {
    let (base, _world) = serve(graph_handler);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cuenta.json");
    let (source, _) = tokens(&base, now() + 3600);
    let graph = Graph::new(&format!("{base}/v1.0"), source, 2).unwrap();
    let mailbox = Mailbox::open(graph, &path).unwrap();
    mailbox.create_new_draft(&pdf_draft(), "k-1").unwrap();
    assert_state_has_no_pdf(&path);
}

fn assert_reply_rejects_attachments(mailbox: &impl MailConnector, world: &Shared) {
    let e = mailbox
        .create_draft("m-1", &pdf_draft(), "k-reply")
        .unwrap_err();
    assert_eq!(e.code, "invalid_input");
    assert!(
        world.lock().unwrap().requests.is_empty(),
        "se rechaza antes de llamar al proveedor"
    );
}

#[test]
fn gmail_create_reply_rejects_attachments() {
    let (mailbox, world) = gmail_mailbox();
    assert_reply_rejects_attachments(&mailbox, &world);
}

#[test]
fn graph_create_reply_rejects_attachments() {
    let (mailbox, world) = graph_mailbox();
    assert_reply_rejects_attachments(&mailbox, &world);
}

// ------------------------------------------------- envío y recuperación tras un corte

#[test]
fn gmail_lookup_after_a_cut_finds_the_send_and_does_not_send_twice() {
    let (mailbox, world) = gmail_mailbox();
    let id = mailbox.create_new_draft(&pdf_draft(), "k-draft").unwrap();
    world.lock().unwrap().drop_send_response = true;
    assert_eq!(
        mailbox.send_draft(&id, "k-send").unwrap_err().code,
        "unavailable"
    );
    // Sin hilo de origen: el mensaje enviado se encuentra en el hilo del borrador por su
    // `X-NestAdmin-Id` y la etiqueta SENT.
    let receipt = mailbox.lookup("k-send").unwrap().unwrap();
    assert_eq!(receipt["receipt_id"], "s-1");
    assert_eq!(
        receipt["message_id"], "",
        "mensaje nuevo: sin mensaje de origen"
    );
    assert_eq!(mailbox.send_draft(&id, "k-send").unwrap().receipt_id, "s-1");
    assert_eq!(count(&world, "POST", "/drafts/send"), 1);
    assert_eq!(world.lock().unwrap().sent, ["s-1"]);
}

#[test]
fn graph_lookup_after_a_cut_finds_the_send_and_does_not_send_twice() {
    let (mailbox, world) = graph_mailbox();
    let id = mailbox.create_new_draft(&pdf_draft(), "k-draft").unwrap();
    world.lock().unwrap().drop_send_response = true;
    assert_eq!(
        mailbox.send_draft(&id, "k-send").unwrap_err().code,
        "unavailable"
    );
    let receipt = mailbox.lookup("k-send").unwrap().unwrap();
    assert_eq!(receipt["draft_id"], id.as_str());
    assert_eq!(mailbox.send_draft(&id, "k-send").unwrap().draft_id, id);
    assert_eq!(count(&world, "POST", "/send"), 1);
    assert_eq!(world.lock().unwrap().sent.len(), 1);
}

#[test]
fn lookup_of_a_send_that_never_happened_is_none() {
    let (mailbox, _world) = gmail_mailbox();
    let _ = mailbox.create_new_draft(&pdf_draft(), "k-draft").unwrap();
    assert_eq!(mailbox.lookup("k-send").unwrap(), None);
}

// ------------------------------------------------- proveedor sin soporte (fallo cerrado)

struct WithoutNewMessages;

impl MailProvider for WithoutNewMessages {
    fn list(&self, _: Option<&str>) -> Result<MessagePage, nestadmin_core::ConnectorError> {
        unreachable!()
    }
    fn create_reply(
        &self,
        _: &str,
        _: &NewDraft,
    ) -> Result<CreatedDraft, nestadmin_core::ConnectorError> {
        unreachable!()
    }
    fn send(&self, _: &str) -> Result<String, nestadmin_core::ConnectorError> {
        unreachable!()
    }
    fn status(&self, _: &CreatedDraft) -> Result<DraftStatus, nestadmin_core::ConnectorError> {
        unreachable!()
    }
    fn attachment(&self, _: &str, _: &str) -> Result<Vec<u8>, nestadmin_core::ConnectorError> {
        unreachable!()
    }
}

#[test]
fn a_provider_without_new_message_support_is_unavailable() {
    let mailbox = Mailbox::in_memory(WithoutNewMessages);
    let e = mailbox.create_new_draft(&pdf_draft(), "k").unwrap_err();
    assert_eq!(e.code, "unavailable");
}
