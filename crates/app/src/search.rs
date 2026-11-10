//! Índice local y búsqueda (D179, D180, D181, R32). Sin IA ni red: el índice se construye con lo
//! que la tarea ya guarda y con el texto que se extrae en local (PDF u OCR). Una pasada de
//! indexado nunca cambia el estado de una tarea ni toca el conector de correo.
//!
//! La extracción puede tardar, así que la pasada se hace en dos fases para no retener el cerrojo
//! de `App`: `index_pending` toma lo pendiente (y resuelve en el acto lo que no necesita
//! extraer), quien llama extrae con `extract_for_index` sin el cerrojo, y `store_index` guarda.
//! «Pendiente» es «tarea sin fila», y `task_id` es clave primaria: repetir tras un corte no
//! duplica nada.

use std::path::{Path, PathBuf};

use nestadmin_core::{
    SearchDoc, SearchFilters, SearchKind, SearchPage, SearchQuery, SearchStatus, Task,
};
use nestadmin_extract::{ExtractError, Extraction, Extractor};
use serde::Serialize;
use serde_json::Value;

use crate::{App, AppResult, EMAIL, INVOICE};

/// Tareas que toma cada llamada de `index_pending` en la pasada de fondo.
pub const INDEX_BATCH: usize = 5;

/// Palabras vacías e interrogativas del español que `question_terms` descarta (D181). Lista
/// cerrada; se compara en minúsculas y sin tildes.
const STOP_WORDS: &[&str] = &[
    // Artículos, preposiciones y conjunciones.
    "el",
    "la",
    "lo",
    "los",
    "las",
    "un",
    "una",
    "unos",
    "unas",
    "al",
    "del",
    "de",
    "a",
    "en",
    "con",
    "por",
    "para",
    "sin",
    "sobre",
    "entre",
    "desde",
    "hasta",
    "y",
    "o",
    "u",
    "e",
    "que",
    // Pronombres y posesivos.
    "me",
    "te",
    "se",
    "le",
    "les",
    "nos",
    "mi",
    "mis",
    "tu",
    "tus",
    "su",
    "sus",
    "yo",
    "ese",
    "esa",
    "eso",
    "esto",
    "este",
    "estos",
    "estas",
    // Interrogativos.
    "donde",
    "cual",
    "cuales",
    "como",
    "cuando",
    "cuanto",
    "cuantos",
    "cuanta",
    "cuantas",
    "quien",
    "quienes",
    "porque",
    // Verbos de petición y de estado.
    "esta",
    "estan",
    "estoy",
    "es",
    "son",
    "hay",
    "tengo",
    "tiene",
    "tienes",
    "tenemos",
    "ha",
    "he",
    "han",
    "busca",
    "buscar",
    "busco",
    "buscame",
    "encuentra",
    "encontrar",
    "encuentro",
    "encuentrame",
    "muestra",
    "muestrame",
    "mostrar",
    "dame",
    "dar",
    "dime",
    "decir",
    "quiero",
    "necesito",
    "puedes",
    "puedo",
    "ver",
    "localiza",
    "localizar",
];

/// Palabras de tipo de `question_parts` (D181): listas cerradas, sin tildes y en minúsculas.
const INVOICE_WORDS: &[&str] = &["factura", "facturas", "recibo", "recibos"];
const EMAIL_WORDS: &[&str] = &[
    "correo", "correos", "email", "emails", "mail", "mails", "mensaje", "mensajes",
];
/// Genéricas que solo se quitan, sin filtrar.
const GENERIC_WORDS: &[&str] = &[
    "documento",
    "documentos",
    "archivo",
    "archivos",
    "fichero",
    "ficheros",
];

/// Lo que devuelve la extracción local de una factura para el índice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IndexOutcome {
    /// Texto extraído.
    Text(String),
    /// El documento no tiene texto (`ExtractError::Empty` o texto vacío).
    NoText,
    /// Falló la extracción; el código corto de `ExtractError::code`, nunca contenido.
    Failed(String),
}

/// Una factura con original cuyo texto hay que extraer fuera del cerrojo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingIndex {
    pub task_id: i64,
    pub path: PathBuf,
}

/// Cómo se casó una pregunta con los documentos (D181).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Matched {
    /// Con todas las palabras.
    All,
    /// Con alguna de ellas, porque con todas no había resultados.
    Any,
    /// Sin resultados o sin palabras útiles.
    None,
}

/// Respuesta por palabras del chat cuando no hay IA utilizable (D181).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct QuestionAnswer {
    pub page: SearchPage,
    /// Las palabras con las que se buscó.
    pub terms: Vec<String>,
    pub matched: Matched,
    /// Tipo por el que se filtró en la búsqueda que dio este resultado (`null` si no se filtró).
    pub kind: Option<SearchKind>,
}

/// Extrae el texto de un original, en local y sin ninguna llamada externa. Se llama fuera del
/// cerrojo de `App`.
pub fn extract_for_index(extractor: &Extractor, path: &Path) -> IndexOutcome {
    outcome_of(extractor.extract(path))
}

fn outcome_of(extracted: Result<Extraction, ExtractError>) -> IndexOutcome {
    match extracted {
        Ok(e) if e.text.trim().is_empty() => IndexOutcome::NoText,
        Ok(e) => IndexOutcome::Text(e.text),
        Err(ExtractError::Empty) => IndexOutcome::NoText,
        Err(e) => IndexOutcome::Failed(e.code().to_owned()),
    }
}

fn text_of(value: &Value) -> String {
    value
        .as_str()
        .map(|s| s.trim().to_owned())
        .unwrap_or_default()
}

/// `AAAA-MM-DD` si el texto empieza por una fecha así (las de Gmail, Microsoft y el simulador).
fn iso_date(text: &str) -> Option<String> {
    let head = text.trim().get(..10)?;
    let b = head.as_bytes();
    let digits = |r: std::ops::Range<usize>| b[r].iter().all(u8::is_ascii_digit);
    if b[4] != b'-' || b[7] != b'-' || !(digits(0..4) && digits(5..7) && digits(8..10)) {
        return None;
    }
    let month: u32 = head[5..7].parse().ok()?;
    let day: u32 = head[8..10].parse().ok()?;
    ((1..=12).contains(&month) && (1..=31).contains(&day)).then(|| head.to_owned())
}

/// Importe en euros (`1210.00`) a céntimos, sin coma flotante. `None` si no se puede leer.
fn minor_units(total: &str) -> Option<i64> {
    let amount = nestadmin_documents::parse_amount("total", total).ok()?;
    let extra = 2u32.checked_sub(amount.scale())?;
    let minor = amount.mantissa().checked_mul(10i128.pow(extra))?;
    i64::try_from(minor).ok()
}

/// Fila de una factura a partir de su entrada y su resultado, sin texto (`no_text`). Los campos
/// ausentes quedan vacíos o `None`.
pub(crate) fn invoice_doc(input: &Value, result: &Value) -> SearchDoc {
    let field = |name: &str, payload: &str| {
        let value = text_of(&result["fields"][name]);
        if value.is_empty() {
            text_of(&result["action"]["payload"][payload])
        } else {
            value
        }
    };
    let title = [&result["document"], &input["document"]]
        .into_iter()
        .map(text_of)
        .find(|t| !t.is_empty())
        .or_else(|| {
            let path = input["path"].as_str()?;
            Path::new(path)
                .file_name()
                .and_then(|n| n.to_str())
                .map(str::to_owned)
        })
        .unwrap_or_default();
    let total = field("total", "total");
    SearchDoc {
        task_id: 0,
        kind: SearchKind::Invoice,
        status: SearchStatus::NoText,
        title,
        party: field("supplier", "supplier"),
        number: field("invoice_number", "number"),
        doc_date: iso_date(&field("date", "date")),
        total_minor: minor_units(&total),
        currency: field("currency", "currency"),
        body: String::new(),
        detail: String::new(),
    }
}

/// Fila de un correo. El cuerpo solo se toma si la tarea guarda el mensaje completo en su
/// entrada (`input.message`); sin él queda `no_text`, con asunto, remitente y fecha. Nunca se
/// pide nada al conector. `None` para una nueva redacción (`redraft_of`), que no se indexa aparte.
pub(crate) fn email_doc(input: &Value, result: &Value) -> Option<SearchDoc> {
    if input.get("redraft_of").is_some_and(|v| !v.is_null()) {
        return None;
    }
    let stored = input["message"].is_object();
    // Asunto, remitente y fecha: de la entrada y, si esta no guarda el mensaje (tareas de antes
    // de las cuentas), del resultado, que ya lo guarda.
    let message = if stored {
        &input["message"]
    } else {
        &result["message"]
    };
    let body = if stored {
        text_of(&input["message"]["body"])
    } else {
        String::new()
    };
    Some(SearchDoc {
        task_id: 0,
        kind: SearchKind::Email,
        status: if body.is_empty() {
            SearchStatus::NoText
        } else {
            SearchStatus::Indexed
        },
        title: text_of(&message["subject"]),
        party: text_of(&message["from"]),
        number: String::new(),
        doc_date: message["date"].as_str().and_then(iso_date),
        total_minor: None,
        currency: String::new(),
        body,
        detail: String::new(),
    })
}

/// La misma fila con el texto extraído: `indexed` con texto, `no_text` sin él.
pub(crate) fn with_text(mut doc: SearchDoc, text: &str) -> SearchDoc {
    if text.trim().is_empty() {
        doc.status = SearchStatus::NoText;
        doc.body.clear();
    } else {
        doc.status = SearchStatus::Indexed;
        doc.body = text.to_owned();
    }
    doc
}

/// Fila de una tarea tal como está guardada (sin texto en las facturas). `None` si no se indexa:
/// otro tipo de tarea o una nueva redacción.
pub(crate) fn doc_for_task(task: &Task) -> Option<SearchDoc> {
    let input: Value = serde_json::from_str(&task.input_ref).unwrap_or(Value::Null);
    let result = task.result.clone().unwrap_or(Value::Null);
    let mut doc = match task.kind.as_str() {
        INVOICE => Some(invoice_doc(&input, &result)),
        EMAIL => email_doc(&input, &result),
        _ => None,
    }?;
    doc.task_id = task.id;
    Some(doc)
}

/// Fila que se guarda junto con el primer resultado de una tarea. Una factura solo se indexa si
/// ya se extrajo su texto; si no, la recoge la pasada de fondo.
pub(crate) fn doc_on_finish(
    task: &Task,
    result: &Value,
    extraction: Option<&Extraction>,
) -> Option<SearchDoc> {
    let input: Value = serde_json::from_str(&task.input_ref).unwrap_or(Value::Null);
    match task.kind.as_str() {
        INVOICE => {
            let extraction = extraction?;
            Some(with_text(invoice_doc(&input, result), &extraction.text))
        }
        EMAIL => email_doc(&input, result),
        _ => None,
    }
}

/// Palabras útiles de una pregunta en lenguaje corriente (D181): minúsculas, sin signos, sin
/// palabras vacías ni interrogativas, con los números y sin repetidas.
pub fn question_terms(text: &str) -> Vec<String> {
    question_parts(text).0
}

/// Como `question_terms`, pero separa las palabras de tipo (D181): «factura», «correo» y sus
/// sinónimos no son términos; dicen qué tipo se busca. Las genéricas («documento», «archivo»)
/// solo se quitan. Devuelve los términos y los tipos nombrados, sin repetir.
pub fn question_parts(text: &str) -> (Vec<String>, Vec<SearchKind>) {
    const MARKS: &[char] = &['¿', '?', '¡', '!', ';', ':', '(', ')', '"', '«', '»'];
    let mut terms: Vec<String> = Vec::new();
    let mut kinds: Vec<SearchKind> = Vec::new();
    for raw in text.to_lowercase().split_whitespace() {
        let word: String = raw.chars().filter(|c| !MARKS.contains(c)).collect();
        // El punto y la coma solo sobran en los extremos: dentro de un número («1.234,56») no.
        let word = word.trim_matches(|c| c == '.' || c == ',').to_owned();
        if word.is_empty() {
            continue;
        }
        let plain = strip_accents(&word);
        let named = if INVOICE_WORDS.contains(&plain.as_str()) {
            Some(SearchKind::Invoice)
        } else if EMAIL_WORDS.contains(&plain.as_str()) {
            Some(SearchKind::Email)
        } else {
            None
        };
        if let Some(kind) = named {
            if !kinds.contains(&kind) {
                kinds.push(kind);
            }
            continue;
        }
        if STOP_WORDS.contains(&plain.as_str())
            || GENERIC_WORDS.contains(&plain.as_str())
            || terms.contains(&word)
        {
            continue;
        }
        terms.push(word);
    }
    (terms, kinds)
}

fn strip_accents(word: &str) -> String {
    word.chars()
        .map(|c| match c {
            'á' | 'à' | 'ä' | 'â' => 'a',
            'é' | 'è' | 'ë' | 'ê' => 'e',
            'í' | 'ì' | 'ï' | 'î' => 'i',
            'ó' | 'ò' | 'ö' | 'ô' => 'o',
            'ú' | 'ù' | 'ü' | 'û' => 'u',
            'ñ' => 'n',
            other => other,
        })
        .collect()
}

impl App {
    /// Extractor local que usan las tareas, para que quien indexa lo use fuera del cerrojo.
    pub fn index_extractor(&self) -> std::sync::Arc<Extractor> {
        std::sync::Arc::clone(&self.extractor)
    }

    /// Si quedan tareas por indexar (sin fila, ya fuera de la cola).
    pub fn index_backlog(&self) -> AppResult<bool> {
        Ok(!self.store.unindexed_tasks(1)?.is_empty())
    }

    /// Fase 1 de la pasada de indexado: toma hasta `limit` tareas sin fila. Las que no necesitan
    /// extraer (correos, y facturas sin original) se indexan en el acto; de las facturas con
    /// original devuelve dónde está para extraer fuera del cerrojo. No cambia el estado de
    /// ninguna tarea ni usa la IA, la red o el conector de correo.
    pub fn index_pending(&mut self, limit: usize) -> AppResult<Vec<PendingIndex>> {
        let mut pending = Vec::new();
        for task in self.store.unindexed_tasks(limit)? {
            let Some(doc) = doc_for_task(&task) else {
                continue;
            };
            if doc.kind == SearchKind::Email {
                self.store.index_document(&doc)?;
                continue;
            }
            let input: Value = serde_json::from_str(&task.input_ref).unwrap_or(Value::Null);
            match self.locate_document(task.id, &input) {
                Ok(path) => pending.push(PendingIndex {
                    task_id: task.id,
                    path,
                }),
                // Sin original (ya no está o está fuera de la carpeta): `no_text`, y así se dice.
                Err(_) => self.store.index_document(&doc)?,
            }
        }
        Ok(pending)
    }

    /// Fase 2: guarda lo extraído. `indexed` con el texto, `no_text` sin él, `failed` con el
    /// código del error. Si la tarea ya tiene fila no se la pisa con peor información: solo un
    /// texto nuevo sustituye a una fila que no tiene texto.
    pub fn store_index(&mut self, task_id: i64, outcome: IndexOutcome) -> AppResult<()> {
        let task = self.store.task(task_id)?;
        let Some(mut doc) = doc_for_task(&task) else {
            return Ok(());
        };
        if let Some(existing) = self.store.indexed_document(task_id)? {
            let better = matches!(outcome, IndexOutcome::Text(_))
                && existing.status != SearchStatus::Indexed;
            if !better {
                return Ok(());
            }
        }
        match outcome {
            IndexOutcome::Text(text) => doc = with_text(doc, &text),
            IndexOutcome::NoText => doc.status = SearchStatus::NoText,
            IndexOutcome::Failed(code) => {
                doc.status = SearchStatus::Failed;
                doc.detail = code;
            }
        }
        self.store.index_document(&doc)?;
        Ok(())
    }

    /// Borra las filas `failed` para que la pasada las reintente. Devuelve cuántas.
    pub fn reindex_failed(&mut self) -> AppResult<usize> {
        Ok(self.store.delete_failed_index()?)
    }

    /// Búsqueda local (D180): trocea el texto por espacios y exige todos los términos. Sin
    /// texto, lista por fecha con los filtros. Un filtro no válido es `invalid_filter`.
    pub fn search(
        &self,
        terms_text: &str,
        filters: SearchFilters,
        offset: usize,
    ) -> AppResult<SearchPage> {
        let terms = terms_text.split_whitespace().map(str::to_owned).collect();
        Ok(self.store.search(&SearchQuery {
            terms,
            any: false,
            filters,
            offset,
        })?)
    }

    /// Respuesta por palabras (D181): quita signos y palabras vacías, exige todas las palabras
    /// y, si así no hay resultados y hay más de una, admite cualquiera. Sin palabras útiles
    /// no busca nada y devuelve solo los recuentos.
    pub fn search_question(&self, text: &str) -> AppResult<QuestionAnswer> {
        let (terms, kinds) = question_parts(text);
        // Un único tipo nombrado filtra; los dos o ninguno, no.
        let kind = match kinds.as_slice() {
            [one] => Some(*one),
            _ => None,
        };
        let run = |any: bool, kind: Option<SearchKind>| {
            self.store.search(&SearchQuery {
                terms: terms.clone(),
                any,
                filters: SearchFilters {
                    kind,
                    ..SearchFilters::default()
                },
                offset: 0,
            })
        };
        if terms.is_empty() {
            if kinds.is_empty() {
                let mut page = self.store.search(&SearchQuery::default())?;
                page.results.clear();
                page.total = 0;
                return Ok(QuestionAnswer {
                    page,
                    terms,
                    matched: Matched::None,
                    kind: None,
                });
            }
            // Solo tipo («mis facturas»): lista ese tipo por fecha.
            let page = run(false, kind)?;
            return Ok(QuestionAnswer {
                page,
                terms,
                matched: Matched::All,
                kind,
            });
        }
        // Con el filtro de tipo (si lo hay) y, si así no sale nada, sin él.
        let mut last = None;
        let attempts: &[Option<SearchKind>] = if kind.is_some() {
            &[kind, None]
        } else {
            &[None]
        };
        for &attempt in attempts {
            let page = run(false, attempt)?;
            if page.total > 0 {
                return Ok(QuestionAnswer {
                    page,
                    terms,
                    matched: Matched::All,
                    kind: attempt,
                });
            }
            if terms.len() > 1 {
                let page = run(true, attempt)?;
                if page.total > 0 {
                    return Ok(QuestionAnswer {
                        page,
                        terms,
                        matched: Matched::Any,
                        kind: attempt,
                    });
                }
            }
            last = Some(page);
        }
        Ok(QuestionAnswer {
            page: last.expect("al menos un intento"),
            terms,
            matched: Matched::None,
            kind,
        })
    }

    /// Completa la tarea e indexa su fila en la misma transacción. Si la fila no se pudo calcular
    /// (`doc` es `None`) o el índice falla, la tarea se completa igual y la recoge la pasada.
    pub(crate) fn complete_indexing(
        &mut self,
        id: i64,
        result: &Value,
        doc: Option<&SearchDoc>,
    ) -> AppResult<Task> {
        if let Some(doc) = doc {
            if let Ok(task) = self.store.complete_indexed(id, result, doc) {
                return Ok(task);
            }
        }
        Ok(self.store.complete(id, result)?)
    }

    /// Como `complete_indexing`, para dejar la tarea pendiente de revisión.
    pub(crate) fn review_indexing(
        &mut self,
        id: i64,
        result: &Value,
        doc: Option<&SearchDoc>,
    ) -> AppResult<Task> {
        if let Some(doc) = doc {
            if let Ok(task) = self.store.await_review_indexed(id, result, doc) {
                return Ok(task);
            }
        }
        Ok(self.store.await_review(id, result)?)
    }

    /// Crea una tarea ya cerrada (`closed`) o pendiente de revisión e indexa su fila en la misma
    /// transacción; sin fila, o si el índice falla, la crea como antes.
    pub(crate) fn enqueue_born_indexing(
        &mut self,
        input: &str,
        result: &Value,
        reason: &str,
        review: bool,
        doc: Option<&SearchDoc>,
    ) -> AppResult<Task> {
        if let Some(doc) = doc {
            let done = if review {
                self.store
                    .enqueue_review_indexed(EMAIL, input, result, reason, doc)
            } else {
                self.store
                    .enqueue_closed_indexed(EMAIL, input, result, reason, doc)
            };
            if let Ok(task) = done {
                return Ok(task);
            }
        }
        Ok(if review {
            self.store.enqueue_review(EMAIL, input, result, reason)?
        } else {
            self.store.enqueue_closed(EMAIL, input, result, reason)?
        })
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn invoice_row_takes_fields_and_converts_the_total_to_cents() {
        let input = json!({ "document": "luz-oct.pdf", "path": "C:/x/1.pdf" });
        let result = json!({
            "document": "luz-oct.pdf",
            "fields": {
                "supplier": "Suministros Luz SL", "invoice_number": "L-2026-10",
                "date": "2026-10-05", "total": "1210.50", "currency": "EUR",
            },
        });
        let doc = invoice_doc(&input, &result);
        assert_eq!(doc.title, "luz-oct.pdf");
        assert_eq!(doc.party, "Suministros Luz SL");
        assert_eq!(doc.number, "L-2026-10");
        assert_eq!(doc.doc_date.as_deref(), Some("2026-10-05"));
        assert_eq!(doc.total_minor, Some(121_050));
        assert_eq!(doc.currency, "EUR");
        assert_eq!(doc.status, SearchStatus::NoText);
    }

    #[test]
    fn invoice_row_leaves_missing_fields_empty() {
        let input = json!({ "document": "x.pdf" });
        let doc = invoice_doc(&input, &Value::Null);
        assert_eq!(doc.title, "x.pdf");
        assert!(doc.party.is_empty() && doc.number.is_empty() && doc.currency.is_empty());
        assert_eq!(doc.doc_date, None);
        assert_eq!(doc.total_minor, None);
    }

    #[test]
    fn invoice_row_falls_back_to_the_action_payload() {
        let result = json!({
            "action": { "payload": { "supplier": "Beta", "total": "10.05", "date": "bad" } },
        });
        let doc = invoice_doc(&Value::Null, &result);
        assert_eq!(doc.party, "Beta");
        assert_eq!(doc.total_minor, Some(1005));
        assert_eq!(doc.doc_date, None);
    }

    #[test]
    fn minor_units_rejects_what_it_cannot_read() {
        assert_eq!(minor_units("0.99"), Some(99));
        assert_eq!(minor_units("-12.30"), Some(-1230));
        assert_eq!(minor_units("abc"), None);
        assert_eq!(minor_units(""), None);
    }

    #[test]
    fn email_with_the_stored_message_is_indexed_with_its_body() {
        let input = json!({ "message": {
            "id": "m1", "subject": "Factura de octubre", "from": "ana@cliente.test",
            "date": "2026-10-03T08:00:00Z", "body": "Adjunto la factura.",
        } });
        let doc = email_doc(&input, &Value::Null).unwrap();
        assert_eq!(doc.status, SearchStatus::Indexed);
        assert_eq!(doc.title, "Factura de octubre");
        assert_eq!(doc.party, "ana@cliente.test");
        assert_eq!(doc.doc_date.as_deref(), Some("2026-10-03"));
        assert_eq!(doc.body, "Adjunto la factura.");
    }

    #[test]
    fn email_without_a_stored_body_is_no_text_but_findable() {
        let input = json!({ "message": {
            "id": "m1", "subject": "Hola", "from": "ana@cliente.test", "date": "2026-10-03",
        } });
        let doc = email_doc(&input, &Value::Null).unwrap();
        assert_eq!(doc.status, SearchStatus::NoText);
        assert!(doc.body.is_empty());
        assert_eq!(doc.title, "Hola");
        assert_eq!(doc.party, "ana@cliente.test");
    }

    #[test]
    fn email_without_input_message_takes_headers_from_the_result_but_no_body() {
        let result = json!({ "message": {
            "subject": "Asunto", "from": "b@x.test", "body": "texto",
        } });
        let doc = email_doc(&json!({ "message_id": "a:1" }), &result).unwrap();
        assert_eq!(doc.title, "Asunto");
        assert!(doc.body.is_empty());
        assert_eq!(doc.status, SearchStatus::NoText);
    }

    #[test]
    fn a_redraft_is_not_indexed() {
        assert!(email_doc(&json!({ "redraft_of": 3, "message": {} }), &Value::Null).is_none());
    }

    #[test]
    fn dates_are_normalized_only_when_they_start_with_a_real_date() {
        assert_eq!(
            iso_date("2026-10-03T08:00:00Z").as_deref(),
            Some("2026-10-03")
        );
        assert_eq!(iso_date("3 Oct 2026"), None);
        assert_eq!(iso_date("2026-13-01"), None);
        assert_eq!(iso_date(""), None);
    }

    #[test]
    fn outcome_maps_extraction_results() {
        let ok = |text: &str| {
            Ok(Extraction {
                text: text.into(),
                pages: Vec::new(),
            })
        };
        assert_eq!(outcome_of(ok("hola")), IndexOutcome::Text("hola".into()));
        assert_eq!(outcome_of(ok("  \n")), IndexOutcome::NoText);
        assert_eq!(outcome_of(Err(ExtractError::Empty)), IndexOutcome::NoText);
        assert_eq!(
            outcome_of(Err(ExtractError::OcrUnavailable("detalle privado".into()))),
            IndexOutcome::Failed("ocr_unavailable".into())
        );
    }

    #[test]
    fn question_terms_drop_marks_and_empty_words() {
        assert_eq!(
            question_terms("¿Dónde está la factura de la luz de octubre?"),
            ["luz", "octubre"]
        );
        assert_eq!(
            question_terms("Muéstrame las facturas de Proveedor Alfa, 2026."),
            ["proveedor", "alfa", "2026"]
        );
        assert_eq!(
            question_terms("dame el importe 1.234,56 y 1.234,56"),
            ["importe", "1.234,56"]
        );
    }

    #[test]
    fn question_parts_split_kind_words_from_terms() {
        let (terms, kinds) = question_parts("¿Dónde está la factura de la luz?");
        assert_eq!(
            (terms, kinds),
            (vec!["luz".to_owned()], vec![SearchKind::Invoice])
        );
        let (terms, kinds) = question_parts("Correo y recibo del archivo de Ana");
        assert_eq!(terms, ["ana"]);
        assert_eq!(kinds, [SearchKind::Email, SearchKind::Invoice]);
        let (terms, kinds) = question_parts("mis documentos");
        assert!(terms.is_empty() && kinds.is_empty());
    }

    #[test]
    fn question_with_only_empty_words_has_no_terms() {
        assert!(question_terms("¿Qué tengo?").is_empty());
        assert!(question_terms("   ").is_empty());
        assert!(question_terms("").is_empty());
    }
}
