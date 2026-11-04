//! Microsoft 365 / Outlook mediante Microsoft Graph v1.0 (D30). Se piden IDs inmutables,
//! que no cambian al enviar el borrador, y el cuerpo en texto.

use nestadmin_simulators::{codes, Message, MessagePage, NewDraft};
use serde_json::{json, Value};

use crate::http::{Api, Method};
use crate::mailbox::{CreatedDraft, DraftStatus, MailProvider};
use crate::oauth::TokenSource;
use crate::text::html_to_text;
use crate::{error, MailResult};

pub const DEFAULT_BASE: &str = "https://graph.microsoft.com/v1.0";
const PREFER: &str = "IdType=\"ImmutableId\", outlook.body-content-type=\"text\"";
const SELECT: &str = "id,subject,from,toRecipients,receivedDateTime,body";

pub struct Graph {
    api: Api,
    page_size: usize,
}

impl Graph {
    pub fn new(base_url: &str, tokens: TokenSource, page_size: usize) -> MailResult<Self> {
        Ok(Self {
            api: Api::new(base_url, tokens, Some(PREFER))?,
            page_size: page_size.clamp(1, 100),
        })
    }

    pub fn tokens(&self) -> &TokenSource {
        self.api.tokens()
    }
}

fn address(value: &Value) -> Option<String> {
    value
        .pointer("/emailAddress/address")
        .and_then(Value::as_str)
        .filter(|a| !a.is_empty())
        .map(str::to_owned)
}

fn non_empty(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .map(str::to_owned)
}

fn message(value: &Value) -> Option<Message> {
    let body = value.get("body").and_then(|body| {
        let content = body.get("content").and_then(Value::as_str)?;
        let html = body
            .get("contentType")
            .and_then(Value::as_str)
            .is_some_and(|t| t.eq_ignore_ascii_case("html"));
        let text = if html {
            html_to_text(content)
        } else {
            content.trim().to_owned()
        };
        (!text.is_empty()).then_some(text)
    });
    Some(Message {
        id: value.get("id").and_then(Value::as_str)?.to_owned(),
        subject: non_empty(value.get("subject")),
        from: value.get("from").and_then(address),
        to: value
            .get("toRecipients")
            .and_then(Value::as_array)
            .map(|list| list.iter().filter_map(address).collect()),
        body,
        date: non_empty(value.get("receivedDateTime")),
    })
}

impl MailProvider for Graph {
    fn list(&self, cursor: Option<&str>) -> MailResult<MessagePage> {
        let url = match cursor {
            Some(link) => self.api.same_origin(link)?,
            None => self.api.url(
                &["me", "mailFolders", "inbox", "messages"],
                &[
                    ("$top", &self.page_size.to_string()),
                    ("$orderby", "receivedDateTime desc"),
                    ("$select", SELECT),
                ],
            ),
        };
        let body = self.api.call(Method::Get, &url, None)?;
        let messages = body
            .get("value")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                error(
                    codes::UNAVAILABLE,
                    "respuesta de Graph sin lista de mensajes",
                )
            })?
            .iter()
            .filter_map(message)
            .collect();
        Ok(MessagePage {
            messages,
            next_cursor: non_empty(body.get("@odata.nextLink")),
        })
    }

    /// `createReply` deja el borrador en el hilo con el original citado; el texto propuesto va
    /// como comentario y los destinatarios aprobados sustituyen a los propuestos por Outlook.
    fn create_reply(&self, message_id: &str, draft: &NewDraft) -> MailResult<CreatedDraft> {
        let mut message = json!({
            "toRecipients": draft
                .to
                .iter()
                .map(|a| json!({ "emailAddress": { "address": a } }))
                .collect::<Vec<_>>(),
        });
        if let Some(subject) = &draft.subject {
            message["subject"] = json!(subject);
        }
        let url = self
            .api
            .url(&["me", "messages", message_id, "createReply"], &[]);
        let body = self.api.call(
            Method::Post,
            &url,
            Some(&json!({ "message": message, "comment": draft.body })),
        )?;
        let draft_id = body
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| error(codes::UNAVAILABLE, "Graph no devolvió el ID del borrador"))?;
        Ok(CreatedDraft {
            draft_id: draft_id.to_owned(),
            message_id: None,
            rfc822_id: None,
        })
    }

    /// Con IDs inmutables, el mensaje enviado conserva el ID del borrador.
    fn send(&self, draft_id: &str) -> MailResult<String> {
        let url = self.api.url(&["me", "messages", draft_id, "send"], &[]);
        self.api.call(Method::Post, &url, None)?;
        Ok(draft_id.to_owned())
    }

    fn status(&self, draft: &CreatedDraft) -> MailResult<DraftStatus> {
        let draft_id = draft.draft_id.as_str();
        let url = self
            .api
            .url(&["me", "messages", draft_id], &[("$select", "id,isDraft")]);
        let body = self.api.call(Method::Get, &url, None).map_err(|e| {
            if e.code == codes::NOT_FOUND {
                error(
                    codes::NOT_FOUND,
                    format!("el borrador {draft_id} ya no existe; no se reenvía"),
                )
            } else {
                e
            }
        })?;
        match body.get("isDraft").and_then(Value::as_bool) {
            Some(false) => Ok(DraftStatus::Sent(draft_id.to_owned())),
            Some(true) => Ok(DraftStatus::Draft),
            None => Err(error(
                codes::UNAVAILABLE,
                "Graph no indicó si el mensaje es borrador",
            )),
        }
    }
}
