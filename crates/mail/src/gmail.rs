//! Gmail y Google Workspace mediante Gmail API v1 (D30). La respuesta se compone como
//! mensaje MIME en el hilo original (`In-Reply-To`, `References` y `threadId`).
//!
//! Gmail da al mensaje enviado un ID distinto del que tenía el borrador. Por eso cada
//! borrador lleva un `Message-ID` generado aquí y anotado en el registro local: tras un corte
//! durante el envío, se busca un mensaje con la etiqueta `SENT` y esa cabecera (D41).

use base64::engine::general_purpose::{URL_SAFE, URL_SAFE_NO_PAD};
use base64::Engine;
use nestadmin_simulators::{codes, Attachment, Message, MessagePage, NewDraft};
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

    /// Dirección del buzón (`users.getProfile`); `None` si la respuesta no la trae (D48).
    pub fn address(&self) -> MailResult<Option<String>> {
        let url = self.api.url(&["profile"], &[]);
        let profile = self.api.call(Method::Get, &url, None)?;
        Ok(profile
            .get("emailAddress")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|a| a.contains('@'))
            .map(str::to_owned))
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

/// Gmail usa base64 URL; según el mensaje, con o sin relleno.
fn decode_b64url(data: &str) -> Option<Vec<u8>> {
    URL_SAFE
        .decode(data)
        .or_else(|_| URL_SAFE_NO_PAD.decode(data.trim_end_matches('=')))
        .ok()
}

fn part_data(part: &Value) -> Option<String> {
    let data = part.pointer("/body/data").and_then(Value::as_str)?;
    Some(decode_text(&decode_b64url(data)?))
}

fn part_header<'a>(part: &'a Value, name: &str) -> Option<&'a str> {
    part.get("headers")
        .and_then(Value::as_array)?
        .iter()
        .find(|h| {
            h.get("name")
                .and_then(Value::as_str)
                .is_some_and(|n| n.eq_ignore_ascii_case(name))
        })
        .and_then(|h| h.get("value").and_then(Value::as_str))
}

/// Prefijo del identificador de un adjunto que Gmail entrega dentro del propio mensaje (sin
/// `attachmentId`); le sigue el `partId`.
const INLINE_PART: &str = "part:";

/// Adjuntos con nombre, en orden, sin las imágenes incrustadas en el cuerpo (con
/// `Content-ID` y sin disposición `attachment`).
fn collect_attachments(part: &Value, out: &mut Vec<Attachment>) {
    let name = part
        .get("filename")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|f| !f.is_empty());
    if let Some(name) = name {
        let disposition = part_header(part, "Content-Disposition").unwrap_or("");
        let embedded = part_header(part, "Content-ID").is_some()
            && !disposition
                .trim_start()
                .to_ascii_lowercase()
                .starts_with("attachment");
        let id = match part.pointer("/body/attachmentId").and_then(Value::as_str) {
            Some(id) => Some(id.to_owned()),
            None => part
                .get("partId")
                .and_then(Value::as_str)
                .filter(|_| part.pointer("/body/data").is_some())
                .map(|p| format!("{INLINE_PART}{p}")),
        };
        if let (false, Some(id)) = (embedded, id) {
            out.push(Attachment {
                id,
                name: Some(name.to_owned()),
                mime: part
                    .get("mimeType")
                    .and_then(Value::as_str)
                    .filter(|m| !m.is_empty())
                    .map(str::to_owned),
                size: part.pointer("/body/size").and_then(Value::as_u64),
            });
        }
    }
    for child in part
        .get("parts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        collect_attachments(child, out);
    }
}

fn find_part_by_id<'a>(part: &'a Value, id: &str) -> Option<&'a Value> {
    if part.get("partId").and_then(Value::as_str) == Some(id) {
        return Some(part);
    }
    part.get("parts")
        .and_then(Value::as_array)?
        .iter()
        .find_map(|p| find_part_by_id(p, id))
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
        attachments: {
            let mut list = Vec::new();
            if let Some(payload) = value.get("payload") {
                collect_attachments(payload, &mut list);
            }
            list
        },
        date: value
            .get("internalDate")
            .and_then(Value::as_str)
            .and_then(|ms| ms.parse::<u64>().ok())
            .map(format_utc_millis),
    })
}

/// Dominio del `Message-ID` propio: reservado (RFC 2606), así no se atribuye a nadie.
const MESSAGE_ID_DOMAIN: &str = "nestadmin.invalid";

/// `Message-ID` nuevo con 128 bits aleatorios en hexadecimal, que el buscador de Gmail
/// trata como una sola palabra.
fn new_message_id() -> MailResult<String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes)
        .map_err(|_| error(codes::UNAVAILABLE, "no hay fuente de aleatoriedad"))?;
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    Ok(format!("<nestadmin.{hex}@{MESSAGE_ID_DOMAIN}>"))
}

/// Mensaje RFC 5322 de respuesta, en UTF-8 con el cuerpo en base64.
fn reply_mime(draft: &NewDraft, original: &Value, own_id: &str) -> String {
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
        format!("Message-ID: {own_id}"),
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

fn is_sent(message: &Value) -> bool {
    labels(message).contains(&"SENT")
}

/// Compara dos `Message-ID` sin tener en cuenta espacios ni ángulos.
fn same_message_id(a: &str, b: &str) -> bool {
    let bare = |s: &str| {
        s.trim()
            .trim_start_matches('<')
            .trim_end_matches('>')
            .to_owned()
    };
    bare(a) == bare(b)
}

impl Gmail {
    /// Mensaje con la etiqueta `SENT` y la cabecera `Message-ID` indicada, si existe. Incluye
    /// papelera y spam: un mensaje enviado y luego borrado sigue estando enviado.
    fn find_sent(&self, rfc822_id: &str) -> MailResult<Option<String>> {
        let bare = rfc822_id
            .trim()
            .trim_start_matches('<')
            .trim_end_matches('>');
        let query = format!("rfc822msgid:{bare}");
        let url = self.api.url(
            &["messages"],
            &[
                ("q", query.as_str()),
                ("labelIds", "SENT"),
                ("includeSpamTrash", "true"),
                ("maxResults", "10"),
            ],
        );
        let page = self.api.call(Method::Get, &url, None)?;
        for entry in page
            .get("messages")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some(id) = entry.get("id").and_then(Value::as_str) else {
                continue;
            };
            // Se comprueban la cabecera y la etiqueta en el propio mensaje, sin fiarse solo
            // de la búsqueda.
            let message = self.get_message(
                id,
                &[("format", "metadata"), ("metadataHeaders", "Message-ID")],
            )?;
            if is_sent(&message)
                && header(&message, "Message-ID").is_some_and(|h| same_message_id(h, rfc822_id))
            {
                return Ok(Some(id.to_owned()));
            }
        }
        Ok(None)
    }
}

impl MailProvider for Gmail {
    fn attachment(&self, message_id: &str, attachment_id: &str) -> MailResult<Vec<u8>> {
        let data = if let Some(part_id) = attachment_id.strip_prefix(INLINE_PART) {
            let full = self.get_message(message_id, &[("format", "full")])?;
            full.get("payload")
                .and_then(|p| find_part_by_id(p, part_id))
                .and_then(|p| p.pointer("/body/data"))
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| error(codes::NOT_FOUND, "el mensaje ya no tiene ese adjunto"))?
        } else {
            let url = self
                .api
                .url(&["messages", message_id, "attachments", attachment_id], &[]);
            let body =
                self.api
                    .call_limited(Method::Get, &url, None, crate::ATTACHMENT_RESPONSE_LIMIT)?;
            body.get("data")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| error(codes::UNAVAILABLE, "Gmail no devolvió el adjunto"))?
        };
        decode_b64url(&data).ok_or_else(|| {
            error(
                codes::UNAVAILABLE,
                "el adjunto de Gmail no es base64 válido",
            )
        })
    }

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
        let own_id = new_message_id()?;
        let mut message = json!({ "raw": URL_SAFE.encode(reply_mime(draft, &original, &own_id)) });
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
            rfc822_id: Some(own_id),
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

    /// Si el borrador existe, no se ha enviado. Si ya no existe, se busca el mensaje enviado:
    /// primero por el ID del mensaje del borrador (por si Gmail lo conserva) y después por el
    /// `Message-ID` propio con la etiqueta `SENT`. Sin esa prueba no se da por enviado ni se
    /// reenvía.
    fn status(&self, draft: &CreatedDraft) -> MailResult<DraftStatus> {
        let draft_id = draft.draft_id.as_str();
        let url = self
            .api
            .url(&["drafts", draft_id], &[("format", "minimal")]);
        match self.api.call(Method::Get, &url, None) {
            Ok(_) => return Ok(DraftStatus::Draft),
            Err(e) if e.code != codes::NOT_FOUND => return Err(e),
            Err(_) => {}
        }
        if let Some(message_id) = &draft.message_id {
            match self.get_message(message_id, &[("format", "minimal")]) {
                Ok(message) if is_sent(&message) => {
                    return Ok(DraftStatus::Sent(message_id.clone()))
                }
                Ok(_) => {}
                Err(e) if e.code == codes::NOT_FOUND => {}
                Err(e) => return Err(e),
            }
        }
        if let Some(rfc822_id) = &draft.rfc822_id {
            if let Some(sent_id) = self.find_sent(rfc822_id)? {
                return Ok(DraftStatus::Sent(sent_id));
            }
        }
        Err(error(
            codes::NOT_FOUND,
            format!("el borrador {draft_id} ya no existe y no consta enviado; no se reenvía"),
        ))
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
        let mime = reply_mime(&draft, &original, "<propio@nestadmin.invalid>");
        assert!(mime.contains("To: ana@cliente.test\r\n"));
        assert!(mime.contains("Subject: Re: Factura de octubre\r\n"));
        assert!(mime.contains("In-Reply-To: <abc@cliente.test>\r\n"));
        assert!(mime.contains("References: <prev@cliente.test> <abc@cliente.test>\r\n"));
        assert!(mime.contains("Message-ID: <propio@nestadmin.invalid>\r\n"));
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

    #[test]
    fn own_message_ids_are_unique_and_searchable() {
        let (a, b) = (new_message_id().unwrap(), new_message_id().unwrap());
        assert_ne!(a, b);
        assert!(a.starts_with("<nestadmin.") && a.ends_with("@nestadmin.invalid>"));
        let left = &a[11..a.find('@').unwrap()];
        assert_eq!(left.len(), 32);
        assert!(left.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(same_message_id(
            &a,
            &format!(" {} ", a.trim_matches(['<', '>']))
        ));
        assert!(!same_message_id(&a, &b));
    }
}
