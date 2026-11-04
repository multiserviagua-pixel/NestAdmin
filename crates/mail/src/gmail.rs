//! Gmail y Google Workspace mediante Gmail API v1 (D30). La respuesta se compone como
//! mensaje MIME en el hilo original (`In-Reply-To`, `References` y `threadId`).

use base64::engine::general_purpose::{URL_SAFE, URL_SAFE_NO_PAD};
use base64::Engine;
use nestadmin_simulators::{codes, Message, MessagePage, NewDraft};
use serde_json::{json, Value};

use crate::http::{Api, Method};
use crate::mailbox::{CreatedDraft, DraftStatus, MailProvider};
use crate::oauth::TokenSource;
use crate::text::{
    decode_text, encode_header, format_utc_millis, header_addresses, header_value, html_to_text,
    mime_base64,
};
use crate::{error, MailResult};

pub const DEFAULT_BASE: &str = "https://gmail.googleapis.com/gmail/v1/users/me";

pub struct Gmail {
    api: Api,
    page_size: usize,
}

impl Gmail {
    pub fn new(base_url: &str, tokens: TokenSource, page_size: usize) -> MailResult<Self> {
        Ok(Self {
            api: Api::new(base_url, tokens, None)?,
            page_size: page_size.clamp(1, 100),
        })
    }

    pub fn tokens(&self) -> &TokenSource {
        self.api.tokens()
    }

    fn get_message(&self, id: &str, query: &[(&str, &str)]) -> MailResult<Value> {
        let url = self.api.url(&["messages", id], query);
        self.api.call(Method::Get, &url, None)
    }
}

fn header<'a>(message: &'a Value, name: &str) -> Option<&'a str> {
    message
        .pointer("/payload/headers")
        .and_then(Value::as_array)?
        .iter()
        .find(|h| {
            h.get("name")
                .and_then(Value::as_str)
                .is_some_and(|n| n.eq_ignore_ascii_case(name))
        })
        .and_then(|h| h.get("value").and_then(Value::as_str))
}

fn part_data(part: &Value) -> Option<String> {
    let data = part.pointer("/body/data").and_then(Value::as_str)?;
    // Gmail usa base64 URL; según el mensaje, con o sin relleno.
    let bytes = URL_SAFE
        .decode(data)
        .or_else(|_| URL_SAFE_NO_PAD.decode(data.trim_end_matches('=')))
        .ok()?;
    Some(decode_text(&bytes))
}

/// Primera parte del tipo pedido, recorriendo las partes anidadas en orden.
fn find_part(part: &Value, mime: &str) -> Option<String> {
    let kind = part.get("mimeType").and_then(Value::as_str).unwrap_or("");
    let attachment = part
        .get("filename")
        .and_then(Value::as_str)
        .is_some_and(|f| !f.is_empty());
    if kind.eq_ignore_ascii_case(mime) && !attachment {
        if let Some(text) = part_data(part) {
            return Some(text);
        }
    }
    part.get("parts")
        .and_then(Value::as_array)?
        .iter()
        .find_map(|p| find_part(p, mime))
}

fn body(message: &Value) -> Option<String> {
    let payload = message.get("payload")?;
    let text = find_part(payload, "text/plain")
        .map(|t| t.trim().to_owned())
        .or_else(|| find_part(payload, "text/html").map(|h| html_to_text(&h)))?;
    (!text.is_empty()).then_some(text)
}

fn message(value: &Value) -> Option<Message> {
    Some(Message {
        id: value.get("id").and_then(Value::as_str)?.to_owned(),
        subject: header(value, "Subject")
            .filter(|s| !s.trim().is_empty())
            .map(str::to_owned),
        from: header(value, "From").and_then(|h| header_addresses(h).into_iter().next()),
        to: header(value, "To").map(header_addresses),
        body: body(value),
        date: value
            .get("internalDate")
            .and_then(Value::as_str)
            .and_then(|ms| ms.parse::<u64>().ok())
            .map(format_utc_millis),
    })
}

/// Mensaje RFC 5322 de respuesta, en UTF-8 con el cuerpo en base64.
fn reply_mime(draft: &NewDraft, original: &Value) -> String {
    let subject = draft.subject.clone().unwrap_or_else(|| {
        let original = header(original, "Subject").unwrap_or("").trim();
        if original.to_ascii_lowercase().starts_with("re:") {
            original.to_owned()
        } else {
            format!("Re: {original}")
        }
    });
    let mut lines = vec![
        format!("To: {}", header_value(&draft.to.join(", "))),
        format!("Subject: {}", encode_header(&subject)),
    ];
    if let Some(id) = header(original, "Message-ID").map(header_value) {
        let references = header(original, "References")
            .map(|r| format!("{} {id}", header_value(r)))
            .unwrap_or_else(|| id.clone());
        lines.push(format!("In-Reply-To: {id}"));
        lines.push(format!("References: {references}"));
    }
    lines.extend([
        "MIME-Version: 1.0".to_owned(),
        "Content-Type: text/plain; charset=UTF-8".to_owned(),
        "Content-Transfer-Encoding: base64".to_owned(),
        String::new(),
        mime_base64(draft.body.as_bytes()),
    ]);
    lines.join("\r\n")
}

fn labels(message: &Value) -> Vec<&str> {
    message
        .get("labelIds")
        .and_then(Value::as_array)
        .map(|l| l.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default()
}

impl MailProvider for Gmail {
    fn list(&self, cursor: Option<&str>) -> MailResult<MessagePage> {
        let size = self.page_size.to_string();
        let mut query = vec![("labelIds", "INBOX"), ("maxResults", size.as_str())];
        if let Some(token) = cursor {
            query.push(("pageToken", token));
        }
        let url = self.api.url(&["messages"], &query);
        let page = self.api.call(Method::Get, &url, None)?;
        let mut messages = Vec::new();
        for entry in page
            .get("messages")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some(id) = entry.get("id").and_then(Value::as_str) else {
                continue;
            };
            let full = self.get_message(id, &[("format", "full")])?;
            messages.extend(message(&full));
        }
        Ok(MessagePage {
            messages,
            next_cursor: page
                .get("nextPageToken")
                .and_then(Value::as_str)
                .filter(|t| !t.is_empty())
                .map(str::to_owned),
        })
    }

    fn create_reply(&self, message_id: &str, draft: &NewDraft) -> MailResult<CreatedDraft> {
        let original = self.get_message(
            message_id,
            &[
                ("format", "metadata"),
                ("metadataHeaders", "Subject"),
                ("metadataHeaders", "Message-ID"),
                ("metadataHeaders", "References"),
            ],
        )?;
        let mut message = json!({ "raw": URL_SAFE.encode(reply_mime(draft, &original)) });
        if let Some(thread) = original.get("threadId").and_then(Value::as_str) {
            message["threadId"] = json!(thread);
        }
        let url = self.api.url(&["drafts"], &[]);
        let created = self
            .api
            .call(Method::Post, &url, Some(&json!({ "message": message })))?;
        let draft_id = created
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| error(codes::UNAVAILABLE, "Gmail no devolvió el ID del borrador"))?;
        Ok(CreatedDraft {
            draft_id: draft_id.to_owned(),
            message_id: created
                .pointer("/message/id")
                .and_then(Value::as_str)
                .map(str::to_owned),
        })
    }

    fn send(&self, draft_id: &str) -> MailResult<String> {
        let url = self.api.url(&["drafts", "send"], &[]);
        let sent = self
            .api
            .call(Method::Post, &url, Some(&json!({ "id": draft_id })))?;
        sent.get("id")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| {
                error(
                    codes::UNAVAILABLE,
                    "Gmail no devolvió el ID del mensaje enviado",
                )
            })
    }

    /// Si el borrador existe, no se ha enviado. Si ya no existe, se mira la etiqueta `SENT`
    /// del mensaje que contenía; sin esa prueba no se da por enviado ni se reenvía.
    fn status(&self, draft_id: &str, message_id: Option<&str>) -> MailResult<DraftStatus> {
        let url = self
            .api
            .url(&["drafts", draft_id], &[("format", "minimal")]);
        match self.api.call(Method::Get, &url, None) {
            Ok(_) => return Ok(DraftStatus::Draft),
            Err(e) if e.code != codes::NOT_FOUND => return Err(e),
            Err(_) => {}
        }
        let gone = || {
            error(
                codes::NOT_FOUND,
                format!("el borrador {draft_id} ya no existe; no se reenvía"),
            )
        };
        let message_id = message_id.ok_or_else(gone)?;
        let message = self
            .get_message(message_id, &[("format", "minimal")])
            .map_err(|e| {
                if e.code == codes::NOT_FOUND {
                    gone()
                } else {
                    e
                }
            })?;
        if labels(&message).contains(&"SENT") {
            Ok(DraftStatus::Sent(message_id.to_owned()))
        } else {
            Err(gone())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reply_is_threaded_utf8_mime() {
        let original = json!({ "payload": { "headers": [
            { "name": "Subject", "value": "Factura de octubre" },
            { "name": "Message-ID", "value": "<abc@cliente.test>" },
            { "name": "References", "value": "<prev@cliente.test>" },
        ]}});
        let draft = NewDraft {
            to: vec!["ana@cliente.test".into()],
            subject: None,
            body: "Gracias, la revisamos mañana.".into(),
        };
        let mime = reply_mime(&draft, &original);
        assert!(mime.contains("To: ana@cliente.test\r\n"));
        assert!(mime.contains("Subject: Re: Factura de octubre\r\n"));
        assert!(mime.contains("In-Reply-To: <abc@cliente.test>\r\n"));
        assert!(mime.contains("References: <prev@cliente.test> <abc@cliente.test>\r\n"));
        let encoded = mime.split("\r\n\r\n").nth(1).unwrap().replace("\r\n", "");
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .unwrap();
        assert_eq!(String::from_utf8(decoded).unwrap(), draft.body);
    }

    #[test]
    fn plain_text_is_preferred_and_attachments_ignored() {
        let b64 = |t: &str| URL_SAFE.encode(t);
        let value = json!({
            "id": "m1",
            "internalDate": "1793664000000",
            "payload": {
                "mimeType": "multipart/mixed",
                "headers": [
                    { "name": "From", "value": "Ana <ana@cliente.test>" },
                    { "name": "To", "value": "compras@empresa.test" },
                ],
                "parts": [
                    { "mimeType": "text/plain", "filename": "notas.txt", "body": { "data": b64("adjunto") } },
                    { "mimeType": "multipart/alternative", "parts": [
                        { "mimeType": "text/html", "body": { "data": b64("<p>Hola</p>") } },
                        { "mimeType": "text/plain", "body": { "data": b64("Hola en texto") } },
                    ]},
                ],
            },
        });
        let m = message(&value).unwrap();
        assert_eq!(m.body.as_deref(), Some("Hola en texto"));
        assert_eq!(m.from.as_deref(), Some("ana@cliente.test"));
        assert_eq!(m.to, Some(vec!["compras@empresa.test".to_owned()]));
        assert_eq!(m.subject, None, "un asunto ausente no se inventa");
        assert_eq!(m.date.as_deref(), Some("2026-11-03T00:00:00Z"));
    }
}
