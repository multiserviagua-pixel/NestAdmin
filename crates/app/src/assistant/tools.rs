//! Herramientas de nivel «libre» del asistente (D184): solo leen o navegan. Llaman a métodos de
//! `App`, nunca a comandos de la ventana, y devuelven un valor JSON ya preparado para el modelo.
//!
//! Con un destino externo, todo texto que viene de un documento, un correo o una regla pasa por
//! `learn::mask` y después por `learn::is_masked`; lo que no la supera no sale y se cuenta
//! (`withheld`). Las referencias son opacas: `doc:<id de tarea>` para documentos y correos y
//! `acct:N` para cuentas. Nunca se devuelven rutas, huellas, nombres de cuenta, direcciones de
//! buzón, credenciales ni el detalle libre del historial.

use std::cell::Cell;

use nestadmin_ai::assistant::{amount_minor, doc_id, OPEN_VIEWS};
use nestadmin_core::{
    AggregateQuery, SearchFilters, SearchHit, SearchKind, SearchQuery, SearchStatus,
};
use serde_json::{json, Map, Value};

use super::{MAX_FRAGMENTS, MAX_FRAGMENT_CHARS};
use crate::learn::{is_masked, mask};
use crate::search::question_parts;
use crate::{App, AppResult};

/// Resultados de `search` y filas de `list_tasks` como mucho.
pub const MAX_RESULTS: usize = 8;
/// Fragmentos del texto que devuelve `get_document` como mucho.
pub const MAX_DOC_FRAGMENTS: usize = 3;
/// Caracteres de un campo corto (título, proveedor, número, nombre de regla…).
const FIELD_CHARS: usize = 120;
/// Eventos de una tarea como mucho.
const MAX_EVENTS: usize = 20;
/// Reglas como mucho.
const MAX_RULES: usize = 20;
/// Lo que ocupa el lugar de un texto que no supera el enmascarado.
pub const WITHHELD_MARK: &str = "[retenido]";

/// Lo que el asistente pidió ejecutar.
#[derive(Debug, Clone)]
pub struct ToolCall {
    pub name: String,
    /// Argumentos ya validados contra el esquema de la herramienta.
    pub args: Map<String, Value>,
    /// El destino de este turno es un proveedor externo: el texto sale enmascarado.
    pub external: bool,
}

/// Lo que devuelve una herramienta.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolOutput {
    /// Resultado para el modelo.
    pub value: Value,
    /// Referencias `doc:N` que contiene: las únicas citables.
    pub refs: Vec<String>,
    /// Vista a la que debe navegar la ventana (`open_view`).
    pub navigate: Option<String>,
    /// Textos que no superaron el enmascarado y se sustituyeron por una marca.
    pub withheld: usize,
}

/// Texto de un resultado de herramienta, con el enmascarado si el destino es externo.
pub(crate) struct Text {
    external: bool,
    withheld: Cell<usize>,
}

impl Text {
    pub(crate) fn new(external: bool) -> Self {
        Self {
            external,
            withheld: Cell::new(0),
        }
    }

    /// Texto de como mucho `max` caracteres, sin saltos de línea ni caracteres de control en los
    /// campos cortos, y enmascarado si sale del equipo.
    fn clipped(&self, raw: &str, max: usize, keep_lines: bool) -> String {
        let clean: String = raw
            .chars()
            .map(|c| {
                if c == '\n' && keep_lines {
                    c
                } else if c.is_control() {
                    ' '
                } else {
                    c
                }
            })
            .take(max)
            .collect();
        let clean = clean.trim().to_owned();
        if !self.external || clean.is_empty() {
            return clean;
        }
        let masked = mask(&clean);
        if is_masked(&masked) {
            masked
        } else {
            self.withheld.set(self.withheld.get() + 1);
            WITHHELD_MARK.to_owned()
        }
    }

    pub(crate) fn field(&self, raw: &str) -> String {
        self.clipped(raw, FIELD_CHARS, false)
    }

    pub(crate) fn fragment(&self, raw: &str) -> String {
        self.clipped(raw, MAX_FRAGMENT_CHARS, true)
    }

    pub(crate) fn withheld(&self) -> usize {
        self.withheld.get()
    }
}

fn output(value: Value, refs: Vec<String>, text: &Text) -> ToolOutput {
    ToolOutput {
        value,
        refs,
        navigate: None,
        withheld: text.withheld(),
    }
}

fn error(code: &str) -> ToolOutput {
    ToolOutput {
        value: json!({ "error": code }),
        refs: Vec::new(),
        navigate: None,
        withheld: 0,
    }
}

/// Importe en céntimos como `1234.56`.
pub(crate) fn format_minor(minor: i64) -> String {
    let sign = if minor < 0 { "-" } else { "" };
    let abs = minor.unsigned_abs();
    format!("{sign}{}.{:02}", abs / 100, abs % 100)
}

/// Fecha y hora UTC `AAAA-MM-DDTHH:MMZ` de un instante en milisegundos.
pub(crate) fn iso_minute(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let days = secs.div_euclid(86_400);
    let rest = secs.rem_euclid(86_400);
    // Días desde 1970 a fecha civil (algoritmo de Howard Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}Z",
        rest / 3600,
        rest % 3600 / 60
    )
}

fn str_arg<'a>(args: &'a Map<String, Value>, name: &str) -> Option<&'a str> {
    args.get(name).and_then(Value::as_str)
}

fn kind_arg(args: &Map<String, Value>) -> Option<SearchKind> {
    match str_arg(args, "kind") {
        Some("invoice") => Some(SearchKind::Invoice),
        Some("email") => Some(SearchKind::Email),
        _ => None,
    }
}

fn kind_text(kind: SearchKind) -> &'static str {
    kind.as_str()
}

fn status_text(status: SearchStatus) -> &'static str {
    status.as_str()
}

/// Una tarjeta de resultado. `snippet` solo si hay fragmentos disponibles.
pub(crate) fn hit_value(hit: &SearchHit, text: &Text, with_snippet: bool) -> Value {
    let mut value = json!({
        "ref": format!("doc:{}", hit.task_id),
        "kind": kind_text(hit.kind),
        "title": text.field(&hit.title),
        "party": text.field(&hit.party),
        "number": text.field(&hit.number),
        "date": hit.doc_date,
        "total": hit.total_minor.map(format_minor),
        "currency": hit.currency,
        "state": hit.task_state.as_str(),
        "index_status": status_text(hit.index_status),
    });
    if with_snippet && !hit.snippet.trim().is_empty() {
        value["snippet"] = json!(text.fragment(&hit.snippet));
    }
    value
}

/// Resultado de `search` a partir de lo encontrado. Lo usan la herramienta y la vista previa.
pub(crate) fn search_value(
    hits: &[SearchHit],
    total: usize,
    counts: (usize, usize, usize),
    matched: &str,
    text: &Text,
) -> (Value, Vec<String>) {
    let shown: Vec<&SearchHit> = hits.iter().take(MAX_RESULTS).collect();
    let results: Vec<Value> = shown
        .iter()
        .enumerate()
        .map(|(i, hit)| hit_value(hit, text, i < MAX_FRAGMENTS))
        .collect();
    let refs = shown.iter().map(|h| format!("doc:{}", h.task_id)).collect();
    (
        json!({
            "matched": matched,
            "total": total,
            "shown": results.len(),
            "pending_index": counts.0,
            "no_text": counts.1,
            "failed": counts.2,
            "results": results,
        }),
        refs,
    )
}

/// Cuántos fragmentos de texto lleva un resultado (los `snippet` de `search` y los `fragments`
/// de `get_document`).
pub(crate) fn fragment_count(value: &Value) -> usize {
    let snippets = value["results"].as_array().map_or(0, |r| {
        r.iter().filter(|hit| hit.get("snippet").is_some()).count()
    });
    snippets + value["fragments"].as_array().map_or(0, Vec::len)
}

/// Quita los fragmentos de un resultado (los de pasos antiguos, cuando se llena el cupo de la
/// llamada, D183).
pub(crate) fn strip_fragments(value: &mut Value) {
    if let Some(results) = value["results"].as_array_mut() {
        for hit in results {
            if let Some(object) = hit.as_object_mut() {
                object.remove("snippet");
            }
        }
    }
    if let Some(object) = value.as_object_mut() {
        object.remove("fragments");
    }
}

/// Frase de una regla, sin direcciones ni dominios (la ventana tiene su propia frase completa).
fn rule_phrase(rule: &nestadmin_core::Rule, any_sender: bool, text: &Text) -> String {
    let c = &rule.conditions;
    let field = |key: &str| c[key].as_str().map(str::trim).filter(|s| !s.is_empty());
    match rule.action_kind.as_str() {
        "register_invoice" => {
            let who = field("supplier")
                .map(|s| format!("de {}", text.field(s)))
                .unwrap_or_else(|| "(la regla no indica proveedor)".to_owned());
            let max = field("max_total")
                .map(|m| format!(" de hasta {} euros", text.field(m)))
                .unwrap_or_default();
            format!("Registrar sin revisión las facturas {who}{max}")
        }
        "send_draft" => {
            let to = if field("sender").is_some() {
                "a una dirección concreta"
            } else if field("sender_domain").is_some() {
                "a cualquier dirección de un dominio concreto"
            } else if any_sender {
                "a cualquier remitente"
            } else {
                "sin remitente indicado (no actúa)"
            };
            let when = field("category")
                .map(|k| format!("cuando se clasifique como {}", text.field(k)))
                .unwrap_or_else(|| "(la regla no indica categoría)".to_owned());
            format!("Enviar sin revisión la respuesta redactada por la IA {to} {when}")
        }
        _ => "Tipo de regla desconocido".to_owned(),
    }
}

fn action_text(kind: &str) -> &str {
    match kind {
        "register_invoice" => "registrar_factura",
        "send_draft" => "enviar_respuesta",
        other => other,
    }
}

impl App {
    /// Ejecuta una herramienta de nivel libre. Solo lee (o anota una navegación). Un fallo no es un
    /// error del turno: vuelve al modelo como `{"error": <código>}`.
    pub fn assistant_tool(&self, call: &ToolCall) -> ToolOutput {
        let text = Text::new(call.external);
        let result = match call.name.as_str() {
            "search" => self.tool_search(&call.args, &text),
            "get_document" => self.tool_get_document(&call.args, &text),
            "aggregate" => self.tool_aggregate(&call.args),
            "list_tasks" => self.tool_list_tasks(&call.args, &text),
            "list_rules" => self.tool_list_rules(&text),
            "history" => self.tool_history(&call.args),
            "mail_accounts" => Ok(self.tool_mail_accounts()),
            "ai_status" => self.tool_ai_status(),
            "open_view" => Ok(tool_open_view(&call.args)),
            _ => Ok(error("unknown_tool")),
        };
        match result {
            Ok(out) => out,
            Err(e) => error(match e {
                crate::AppError::Core(nestadmin_core::CoreError::InvalidFilter(_)) => {
                    "invalid_filter"
                }
                _ => "unavailable",
            }),
        }
    }

    fn tool_search(&self, args: &Map<String, Value>, text: &Text) -> AppResult<ToolOutput> {
        let query = str_arg(args, "query").unwrap_or_default();
        let (terms, named) = question_parts(query);
        let explicit = kind_arg(args);
        // Como en la búsqueda por palabras (D181): un único tipo nombrado («facturas») acota, y si
        // así no hay nada se busca sin él. Un tipo dado como argumento nunca se relaja.
        let inferred = match (explicit, named.as_slice()) {
            (None, [one]) => Some(*one),
            _ => None,
        };
        let party = str_arg(args, "party").map(str::to_owned);
        let minor = |name: &str| str_arg(args, name).and_then(amount_minor);
        let filters = |kind: Option<SearchKind>| SearchFilters {
            kind,
            from: str_arg(args, "from").map(str::to_owned),
            to: str_arg(args, "to").map(str::to_owned),
            party: party.clone(),
            min_total_minor: minor("min_total"),
            max_total_minor: minor("max_total"),
            status: None,
        };
        let include_queued = self.index_queued();
        let run = |any: bool, kind: Option<SearchKind>| {
            self.store.search(&SearchQuery {
                terms: terms.clone(),
                any,
                filters: filters(kind),
                offset: 0,
                include_queued,
            })
        };
        let attempts: Vec<Option<SearchKind>> = match (explicit, inferred) {
            (Some(k), _) => vec![Some(k)],
            (None, Some(k)) => vec![Some(k), None],
            (None, None) => vec![None],
        };
        let mut last = None;
        for kind in attempts {
            let page = run(false, kind)?;
            if page.total > 0 || terms.is_empty() {
                return Ok(self.search_output(&page, "all", text));
            }
            if terms.len() > 1 {
                let any = run(true, kind)?;
                if any.total > 0 {
                    return Ok(self.search_output(&any, "any", text));
                }
            }
            last = Some(page);
        }
        let page = last.expect("al menos un intento");
        Ok(self.search_output(&page, "none", text))
    }

    fn search_output(
        &self,
        page: &nestadmin_core::SearchPage,
        matched: &str,
        text: &Text,
    ) -> ToolOutput {
        let (value, refs) = search_value(
            &page.results,
            page.total,
            (page.pending_index, page.no_text, page.failed),
            matched,
            text,
        );
        output(value, refs, text)
    }

    fn tool_get_document(&self, args: &Map<String, Value>, text: &Text) -> AppResult<ToolOutput> {
        let Some(id) = str_arg(args, "ref").and_then(doc_id) else {
            return Ok(error("bad_ref"));
        };
        let Some(task) = self.store.task_by_id(id)? else {
            return Ok(error("not_found"));
        };
        let reference = format!("doc:{id}");
        let mut value = json!({
            "ref": reference,
            "kind": task.kind,
            "state": task.state.as_str(),
        });
        let Some(doc) = self.store.indexed_document(id)? else {
            value["index_status"] = json!("not_indexed");
            value["fragments"] = json!([]);
            return Ok(output(value, vec![reference], text));
        };
        value["title"] = json!(text.field(&doc.title));
        value["party"] = json!(text.field(&doc.party));
        value["number"] = json!(text.field(&doc.number));
        value["date"] = json!(doc.doc_date);
        value["total"] = json!(doc.total_minor.map(format_minor));
        value["currency"] = json!(doc.currency);
        value["index_status"] = json!(status_text(doc.status));
        // Los primeros fragmentos del texto, de como mucho `MAX_FRAGMENT_CHARS` cada uno.
        let chars: Vec<char> = doc.body.chars().collect();
        let fragments: Vec<String> = chars
            .chunks(MAX_FRAGMENT_CHARS)
            .take(MAX_DOC_FRAGMENTS)
            .map(|chunk| text.fragment(&chunk.iter().collect::<String>()))
            .filter(|f| !f.is_empty())
            .collect();
        value["fragments"] = json!(fragments);
        Ok(output(value, vec![reference], text))
    }

    fn tool_aggregate(&self, args: &Map<String, Value>) -> AppResult<ToolOutput> {
        let op = str_arg(args, "op").unwrap_or("count");
        let aggregate = self.store.aggregate(&AggregateQuery {
            kind: kind_arg(args),
            from: str_arg(args, "from").map(str::to_owned),
            to: str_arg(args, "to").map(str::to_owned),
            party: str_arg(args, "party").map(str::to_owned),
            include_queued: self.index_queued(),
        })?;
        let mut value = json!({
            "op": op,
            "documents": aggregate.documents,
            "with_total": aggregate.with_total,
            "without_total": aggregate.without_total,
            "excluded_cancelled": aggregate.excluded_cancelled,
            "pending_index": aggregate.pending_index,
        });
        if op == "sum_total" {
            // Una suma por moneda: nunca se mezclan monedas.
            value["by_currency"] = Value::Array(
                aggregate
                    .by_currency
                    .iter()
                    .map(|c| {
                        json!({
                            "currency": if c.currency.is_empty() { "sin_moneda" } else { c.currency.as_str() },
                            "documents": c.documents,
                            "total_minor": c.total_minor,
                            "total": format_minor(c.total_minor),
                        })
                    })
                    .collect(),
            );
        }
        Ok(ToolOutput {
            value,
            refs: Vec::new(),
            navigate: None,
            withheld: 0,
        })
    }

    fn tool_list_tasks(&self, args: &Map<String, Value>, text: &Text) -> AppResult<ToolOutput> {
        let state = str_arg(args, "state");
        let mut counts = Map::new();
        for c in self.store.task_counts()? {
            counts.insert(c.state, json!(c.count));
        }
        let rows = self.store.recent_task_rows(state, MAX_RESULTS)?;
        let refs: Vec<String> = rows.iter().map(|r| format!("doc:{}", r.task_id)).collect();
        let recent: Vec<Value> = rows
            .iter()
            .map(|r| {
                json!({
                    "ref": format!("doc:{}", r.task_id),
                    "kind": r.kind,
                    "state": r.state,
                    "title": text.field(&r.title),
                    "party": text.field(&r.party),
                    "date": r.doc_date,
                    "updated": iso_minute(r.updated_at),
                })
            })
            .collect();
        Ok(output(
            json!({ "counts": counts, "recent": recent }),
            refs,
            text,
        ))
    }

    fn tool_list_rules(&self, text: &Text) -> AppResult<ToolOutput> {
        let views = self.rule_views()?;
        let rules: Vec<Value> = views
            .iter()
            .filter(|v| v.archived_at.is_none())
            .take(MAX_RULES)
            .map(|v| {
                json!({
                    "name": text.field(&v.name),
                    "action": action_text(&v.rule.action_kind),
                    "active": v.rule.active,
                    "phrase": rule_phrase(&v.rule, v.any_sender_ack_at.is_some(), text),
                })
            })
            .collect();
        Ok(output(json!({ "rules": rules }), Vec::new(), text))
    }

    fn tool_history(&self, args: &Map<String, Value>) -> AppResult<ToolOutput> {
        let Some(id) = str_arg(args, "ref").and_then(doc_id) else {
            return Ok(error("bad_ref"));
        };
        if self.store.task_by_id(id)?.is_none() {
            return Ok(error("not_found"));
        }
        let events: Vec<Value> = self
            .store
            .task_events(id, MAX_EVENTS)?
            .iter()
            .map(|e| json!({ "code": e.code, "status": e.status, "at": iso_minute(e.at) }))
            .collect();
        Ok(ToolOutput {
            value: json!({ "ref": format!("doc:{id}"), "events": events }),
            refs: Vec::new(),
            navigate: None,
            withheld: 0,
        })
    }

    fn tool_mail_accounts(&self) -> ToolOutput {
        let view = self.mail_accounts();
        let accounts: Vec<Value> = view
            .accounts
            .iter()
            .enumerate()
            .map(|(i, a)| {
                json!({
                    "ref": format!("acct:{}", i + 1),
                    "provider": serde_json::to_value(a.provider).unwrap_or(Value::Null),
                    "status": serde_json::to_value(a.status).unwrap_or(Value::Null),
                })
            })
            .collect();
        ToolOutput {
            value: json!({ "accounts": accounts }),
            refs: Vec::new(),
            navigate: None,
            withheld: 0,
        }
    }

    fn tool_ai_status(&self) -> AppResult<ToolOutput> {
        let info = self.provider();
        let consent = self.assistant_consent()?;
        Ok(ToolOutput {
            value: json!({
                "provider": info.provider,
                "model": info.model,
                "local": info.provider == nestadmin_ai::ProviderKind::Ollama,
                "waiting": self.ai_wait().map(|w| w.code()),
                "assistant_external_option": consent.state,
            }),
            refs: Vec::new(),
            navigate: None,
            withheld: 0,
        })
    }
}

fn tool_open_view(args: &Map<String, Value>) -> ToolOutput {
    match str_arg(args, "view").filter(|v| OPEN_VIEWS.contains(v)) {
        Some(view) => ToolOutput {
            value: json!({ "opened": view }),
            refs: Vec::new(),
            navigate: Some(view.to_owned()),
            withheld: 0,
        },
        None => error("bad_view"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amounts_and_dates_are_formatted_exactly() {
        assert_eq!(format_minor(12_100), "121.00");
        assert_eq!(format_minor(5), "0.05");
        assert_eq!(format_minor(-250), "-2.50");
        assert_eq!(iso_minute(0), "1970-01-01T00:00Z");
        assert_eq!(iso_minute(1_792_000_000_000), "2026-10-14T17:46Z");
        assert_eq!(iso_minute(951_782_400_000), "2000-02-29T00:00Z");
    }

    #[test]
    fn a_withheld_text_is_counted_and_replaced() {
        let local = Text::new(false);
        assert_eq!(local.field(" Ana ana@x.test "), "Ana ana@x.test");
        assert_eq!(local.withheld(), 0);
        let external = Text::new(true);
        assert_eq!(external.field("Escriba a ana@x.test"), "Escriba a [CORREO]");
        assert_eq!(external.withheld(), 0);
    }

    #[test]
    fn fragments_are_counted_and_stripped() {
        let mut value = json!({
            "results": [{ "ref": "doc:1", "snippet": "a" }, { "ref": "doc:2" }],
            "fragments": ["x", "y"],
        });
        assert_eq!(fragment_count(&value), 3);
        strip_fragments(&mut value);
        assert_eq!(fragment_count(&value), 0);
        assert_eq!(value["results"][0]["ref"], "doc:1");
    }
}
