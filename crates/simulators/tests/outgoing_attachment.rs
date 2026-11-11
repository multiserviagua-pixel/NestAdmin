//! E4b (D229, D230): mensaje nuevo con un PDF adjunto en el simulador de correo. PDF y
//! direcciones sintéticos.

use base64::Engine as _;
use nestadmin_core::Connector;
use nestadmin_simulators::{
    MailConnector, MailSimulator, NewDraft, OutgoingAttachment, MAX_OUTGOING_ATTACHMENT_BYTES,
};
use serde_json::json;

const MARKER: &str = "MARCA-SINTETICA-7f3a";

fn pdf(len: usize) -> Vec<u8> {
    let mut bytes = format!("%PDF-1.4\n%{MARKER}\n%%EOF\n").into_bytes();
    if bytes.len() < len {
        bytes.resize(len, b' ');
    }
    bytes
}

fn draft_with(content: Vec<u8>) -> NewDraft {
    NewDraft {
        to: vec!["cliente@ejemplo.test".into()],
        subject: Some("Factura F-2026-0001 — Empresa Ejemplo".into()),
        body: "Le adjuntamos la factura F-2026-0001.".into(),
        attachments: vec![OutgoingAttachment::pdf("F-2026-0001.pdf", content)],
    }
}

fn draft() -> NewDraft {
    draft_with(pdf(0))
}

fn bad(change: impl FnOnce(&mut NewDraft)) -> NewDraft {
    let mut d = draft();
    change(&mut d);
    d
}

fn attachment(d: &mut NewDraft) -> &mut OutgoingAttachment {
    &mut d.attachments[0]
}

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
            "nombre con espacio",
            bad(|d| attachment(d).name = "a b.pdf".into()),
        ),
        (
            "nombre sin extensión",
            bad(|d| attachment(d).name = "factura".into()),
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
        ("asunto con NUL", bad(|d| d.subject = Some("Hola\0".into()))),
        ("asunto vacío", bad(|d| d.subject = Some(" ".into()))),
        ("sin asunto", bad(|d| d.subject = None)),
        (
            "asunto de 201 caracteres",
            bad(|d| d.subject = Some("a".repeat(201))),
        ),
        ("cuerpo vacío", bad(|d| d.body = " ".into())),
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
            "destinatario con CRLF",
            bad(|d| d.to = vec!["a@ejemplo.test\r\nBcc: x@evil.test".into()]),
        ),
        ("dos adjuntos", {
            let mut d = draft();
            d.attachments
                .push(OutgoingAttachment::pdf("otro.pdf", pdf(0)));
            d
        }),
        (
            "MIME distinto",
            bad(|d| attachment(d).mime = "application/octet-stream".into()),
        ),
        (
            "bytes que no son un PDF",
            bad(|d| *attachment(d) = OutgoingAttachment::pdf("a.pdf", b"MZ no es pdf".to_vec())),
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

#[test]
fn a_valid_pdf_creates_a_standalone_draft_with_its_metadata() {
    let sim = MailSimulator::in_memory();
    let d = draft();
    let id = sim.create_new_draft(&d, "task-1/create/v1").unwrap();
    let drafts = sim.drafts().unwrap();
    assert_eq!(drafts.len(), 1);
    assert_eq!(drafts[0].id, id);
    assert!(drafts[0].standalone);
    assert_eq!(drafts[0].message_id, "");
    assert_eq!(drafts[0].content, d, "mismos metadatos");
    // Guarda name, size, sha256, no los bytes.
    assert!(drafts[0].content.attachments[0].content.is_empty());
}

#[test]
fn rejects_invalid_content_and_creates_nothing() {
    let sim = MailSimulator::in_memory();
    for (case, d) in rejected_drafts() {
        let e = sim.create_new_draft(&d, "k-malo").expect_err(case);
        assert_eq!(e.code, "invalid_input", "{case}: {e:?}");
    }
    assert!(sim.drafts().unwrap().is_empty());
    let e = sim.create_new_draft(&draft(), "  ").unwrap_err();
    assert_eq!(e.code, "invalid_input");
}

#[test]
fn accepts_exactly_the_limit_and_rejects_one_byte_more() {
    let sim = MailSimulator::in_memory();
    let limit = MAX_OUTGOING_ATTACHMENT_BYTES as usize;
    assert_eq!(limit, 3_000_000);
    sim.create_new_draft(&draft_with(pdf(limit)), "k-limit")
        .unwrap();
    let e = sim
        .create_new_draft(&draft_with(pdf(limit + 1)), "k-over")
        .unwrap_err();
    assert_eq!(e.code, "invalid_input");
    assert_eq!(sim.drafts().unwrap().len(), 1);
}

#[test]
fn same_key_and_metadata_reuse_the_draft_and_other_content_conflicts() {
    let sim = MailSimulator::in_memory();
    let first = sim.create_new_draft(&draft(), "k-1").unwrap();
    assert_eq!(sim.create_new_draft(&draft(), "k-1").unwrap(), first);
    assert_eq!(sim.drafts().unwrap().len(), 1);

    let mut other = pdf(0);
    other.extend_from_slice(b"%otro contenido\n");
    assert_eq!(
        sim.create_new_draft(&draft_with(other), "k-1")
            .unwrap_err()
            .code,
        "conflict"
    );
    for changed in [
        bad(|d| d.subject = Some("Otro asunto".into())),
        bad(|d| d.body = "Otro cuerpo".into()),
        bad(|d| d.to = vec!["otro@ejemplo.test".into()]),
        bad(|d| d.attachments.clear()),
    ] {
        assert_eq!(
            sim.create_new_draft(&changed, "k-1").unwrap_err().code,
            "conflict"
        );
    }
    assert_eq!(sim.drafts().unwrap().len(), 1);
}

#[test]
fn a_reply_rejects_attachments_and_creates_nothing() {
    let sim = MailSimulator::in_memory();
    let first = sim.list_messages(None).unwrap().messages[0].id.clone();
    let e = sim.create_draft(&first, &draft(), "k-reply").unwrap_err();
    assert_eq!(e.code, "invalid_input");
    assert!(sim.drafts().unwrap().is_empty());
}

#[test]
fn a_key_is_not_shared_between_a_reply_and_a_new_message() {
    let sim = MailSimulator::in_memory();
    let first = sim.list_messages(None).unwrap().messages[0].id.clone();
    let reply = NewDraft {
        attachments: Vec::new(),
        ..draft()
    };
    sim.create_draft(&first, &reply, "k-1").unwrap();
    let e = sim.create_new_draft(&reply, "k-1").unwrap_err();
    assert_eq!(e.code, "conflict");
    // Y al revés: una respuesta no reutiliza el borrador de un mensaje nuevo.
    sim.create_new_draft(&draft(), "k-2").unwrap();
    let e = sim.create_draft(&first, &reply, "k-2").unwrap_err();
    assert_eq!(e.code, "conflict");
}

#[test]
fn the_persisted_state_has_the_metadata_but_never_the_pdf() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("correo.json");
    let d = draft();
    let id = {
        let sim = MailSimulator::open(&path).unwrap();
        sim.create_new_draft(&d, "k-1").unwrap()
    };
    let saved = std::fs::read_to_string(&path).unwrap();
    let b64 = base64::engine::general_purpose::STANDARD.encode(&d.attachments[0].content);
    assert!(!saved.contains(&b64));
    assert!(!saved.contains(MARKER));
    assert!(saved.contains(&d.attachments[0].sha256));
    assert!(saved.contains("F-2026-0001.pdf"));

    // Tras reabrir, la misma petición reconoce el borrador sin los bytes guardados.
    let sim = MailSimulator::open(&path).unwrap();
    assert_eq!(sim.create_new_draft(&d, "k-1").unwrap(), id);
    assert_eq!(sim.drafts().unwrap().len(), 1);
}

#[test]
fn a_standalone_draft_is_sent_once_and_lookup_finds_it() {
    let sim = MailSimulator::in_memory();
    let id = sim.create_new_draft(&draft(), "k-draft").unwrap();
    assert_eq!(sim.lookup("k-send").unwrap(), None);
    let receipt = sim.send_draft(&id, "k-send").unwrap();
    assert_eq!(receipt.message_id, "", "sin mensaje de origen");
    assert_eq!(sim.send_draft(&id, "k-send").unwrap(), receipt);
    assert_eq!(sim.sent().unwrap().len(), 1);
    // Otra clave para el mismo borrador no envía de nuevo.
    assert_eq!(sim.send_draft(&id, "k-otra").unwrap_err().code, "conflict");
    assert_eq!(
        sim.lookup("k-send").unwrap(),
        Some(json!({
            "receipt_id": receipt.receipt_id,
            "draft_id": id,
            "message_id": "",
            "idempotency_key": "k-send",
        }))
    );
    assert_eq!(sim.sent().unwrap().len(), 1);
}
