//! Índice local de búsqueda (D179, D180, R32): sin IA ni red. Una fila por factura/documento o
//! correo en `search_index` y una tabla FTS5 de contenido externo mantenida por triggers.
//! Todo el texto del usuario viaja como parámetro; en FTS5 solo entra entrecomillado.

use rusqlite::{named_params, params, Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{CoreError, Result};
use crate::queue::{insert_born, task_from_row, transition, Task, TaskState, TASK_COLUMNS};
use crate::store::{now_ms, Store};

/// Resultados máximos por página (D180).
pub const SEARCH_PAGE_SIZE: usize = 50;
/// Longitud máxima del fragmento, en caracteres (D180).
pub const SNIPPET_CHARS: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchKind {
    Invoice,
    Email,
}

impl SearchKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Invoice => "invoice",
            Self::Email => "email",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s {
            "invoice" => Some(Self::Invoice),
            "email" => Some(Self::Email),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchStatus {
    Indexed,
    NoText,
    Failed,
}

impl SearchStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Indexed => "indexed",
            Self::NoText => "no_text",
            Self::Failed => "failed",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s {
            "indexed" => Some(Self::Indexed),
            "no_text" => Some(Self::NoText),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }
}

/// Lo que se guarda de un documento o correo para buscarlo.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchDoc {
    pub task_id: i64,
    pub kind: SearchKind,
    pub status: SearchStatus,
    /// Nombre del documento o asunto del correo.
    pub title: String,
    /// Proveedor o remitente.
    pub party: String,
    pub number: String,
    /// Fecha ISO `AAAA-MM-DD`.
    pub doc_date: Option<String>,
    /// Importe total en céntimos.
    pub total_minor: Option<i64>,
    pub currency: String,
    /// Texto extraído o cuerpo; vacío si no hay.
    pub body: String,
    /// Motivo corto de `failed`, sin contenido del documento.
    pub detail: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchFilters {
    pub kind: Option<SearchKind>,
    /// Fecha mínima `AAAA-MM-DD`, inclusive.
    pub from: Option<String>,
    /// Fecha máxima `AAAA-MM-DD`, inclusive.
    pub to: Option<String>,
    /// Subcadena del proveedor o remitente, sin distinguir mayúsculas (solo ASCII).
    pub party: Option<String>,
    pub min_total_minor: Option<i64>,
    pub max_total_minor: Option<i64>,
    /// Estado de la tarea.
    pub status: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchQuery {
    /// Términos ya troceados por quien llama; aquí solo se escapan.
    pub terms: Vec<String>,
    /// `false` exige todos los términos; `true`, cualquiera.
    pub any: bool,
    pub filters: SearchFilters,
    pub offset: usize,
    /// Cuenta como pendientes de indexar también las tareas en cola (D179, «Lo que espera a la
    /// IA»). `false` por defecto: las que están en cola o en curso no cuentan.
    #[serde(default)]
    pub include_queued: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchHit {
    pub task_id: i64,
    pub kind: SearchKind,
    pub title: String,
    pub party: String,
    pub number: String,
    pub doc_date: Option<String>,
    pub total_minor: Option<i64>,
    pub currency: String,
    pub task_state: TaskState,
    pub index_status: SearchStatus,
    /// Texto plano de como mucho `SNIPPET_CHARS` caracteres.
    pub snippet: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchPage {
    pub results: Vec<SearchHit>,
    /// Coincidencias totales con los filtros (no solo las de la página).
    pub total: usize,
    /// Tareas de documento o correo todavía sin fila en el índice (ausencia explícita).
    pub pending_index: usize,
    pub no_text: usize,
    pub failed: usize,
}

/// Fecha `AAAA-MM-DD` real (año de 4 dígitos, mes y día válidos, bisiestos incluidos).
fn valid_iso_date(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return false;
    }
    let digits = |r: std::ops::Range<usize>| b[r].iter().all(u8::is_ascii_digit);
    if !(digits(0..4) && digits(5..7) && digits(8..10)) {
        return false;
    }
    let (Ok(y), Ok(m), Ok(d)) = (
        s[0..4].parse::<u32>(),
        s[5..7].parse::<u32>(),
        s[8..10].parse::<u32>(),
    ) else {
        return false;
    };
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let days = match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return false,
    };
    (1..=days).contains(&d)
}

fn validate(filters: &SearchFilters) -> Result<()> {
    for (name, value) in [("from", &filters.from), ("to", &filters.to)] {
        if let Some(v) = value {
            if !valid_iso_date(v) {
                return Err(CoreError::InvalidFilter(format!(
                    "{name} debe ser una fecha AAAA-MM-DD válida"
                )));
            }
        }
    }
    if let Some(s) = &filters.status {
        if s.parse::<TaskState>().is_err() {
            return Err(CoreError::InvalidFilter(
                "status no es un estado de tarea".into(),
            ));
        }
    }
    Ok(())
}

/// Expresión MATCH de FTS5: cada término entrecomillado (comillas internas duplicadas), el
/// último con prefijo. Nunca se interpreta nada del usuario como sintaxis de FTS5.
fn match_expression(terms: &[String], any: bool) -> Option<String> {
    let mut quoted: Vec<String> = terms
        .iter()
        .map(|t| t.replace('\0', "").trim().to_string())
        .filter(|t| !t.is_empty())
        .map(|t| format!("\"{}\"", t.replace('"', "\"\"")))
        .collect();
    quoted.last_mut()?.push('*');
    Some(quoted.join(if any { " OR " } else { " AND " }))
}

fn clip_chars(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// Filtros comunes; todos los parámetros se enlazan siempre (`NULL` = sin filtro).
const FILTER_SQL: &str = "(:kind IS NULL OR i.kind = :kind)
    AND (:from IS NULL OR i.doc_date >= :from)
    AND (:to IS NULL OR i.doc_date <= :to)
    AND (:party IS NULL OR instr(lower(i.party), lower(:party)) > 0)
    AND (:min IS NULL OR i.total_minor >= :min)
    AND (:max IS NULL OR i.total_minor <= :max)
    AND (:status IS NULL OR t.state = :status)";

const HIT_COLUMNS: &str = "i.task_id, i.kind, i.title, i.party, i.number, i.doc_date,
    i.total_minor, i.currency, t.state, i.status";

fn parse_failure(index: usize, what: &str) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        index,
        rusqlite::types::Type::Text,
        what.to_string().into(),
    )
}

/// Fila de resultado: las columnas de `HIT_COLUMNS` y, en la 11.ª, el fragmento.
fn hit_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SearchHit> {
    let kind: String = row.get(1)?;
    let state: String = row.get(8)?;
    let status: String = row.get(9)?;
    let snippet: Option<String> = row.get(10)?;
    Ok(SearchHit {
        task_id: row.get(0)?,
        kind: SearchKind::parse(&kind).ok_or_else(|| parse_failure(1, "tipo de índice"))?,
        title: row.get(2)?,
        party: row.get(3)?,
        number: row.get(4)?,
        doc_date: row.get(5)?,
        total_minor: row.get(6)?,
        currency: row.get(7)?,
        task_state: state
            .parse()
            .map_err(|_| parse_failure(8, "estado de tarea"))?,
        index_status: SearchStatus::parse(&status)
            .ok_or_else(|| parse_failure(9, "estado de índice"))?,
        snippet: clip_chars(&snippet.unwrap_or_default(), SNIPPET_CHARS),
    })
}

/// Alta o actualización por `task_id`, dentro de la transacción del llamador. El `ON CONFLICT`
/// dispara el trigger de actualización, que deja el FTS coherente.
fn index_in(tx: &Transaction<'_>, doc: &SearchDoc) -> Result<()> {
    tx.execute(
        "INSERT INTO search_index
            (task_id, kind, status, title, party, number, doc_date, total_minor, currency, body, detail, indexed_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
         ON CONFLICT (task_id) DO UPDATE SET
            kind = excluded.kind, status = excluded.status, title = excluded.title,
            party = excluded.party, number = excluded.number, doc_date = excluded.doc_date,
            total_minor = excluded.total_minor, currency = excluded.currency,
            body = excluded.body, detail = excluded.detail, indexed_at = excluded.indexed_at",
        params![
            doc.task_id,
            doc.kind.as_str(),
            doc.status.as_str(),
            doc.title,
            doc.party,
            doc.number,
            doc.doc_date,
            doc.total_minor,
            doc.currency,
            doc.body,
            doc.detail,
            now_ms(),
        ],
    )?;
    Ok(())
}

/// Las tareas de nueva redacción (`redraft_of`) no se indexan aparte (D179): no cuentan como
/// pendientes.
/// Con `include_queued` entran también las tareas en cola; las que están en curso (`running`)
/// nunca.
fn unindexed_sql(include_queued: bool) -> String {
    let excluded = if include_queued {
        "'running'"
    } else {
        "'queued', 'running'"
    };
    format!("{UNINDEXED_SQL} AND state NOT IN ({excluded})")
}

const UNINDEXED_SQL: &str = "kind IN ('invoice', 'email')
    AND NOT (kind = 'email' AND CASE WHEN json_valid(tasks.input_ref)
        THEN json_extract(tasks.input_ref, '$.redraft_of') END IS NOT NULL)
    AND NOT EXISTS (SELECT 1 FROM search_index s WHERE s.task_id = tasks.id)";

fn count(conn: &Connection, sql: &str, p: impl rusqlite::Params) -> Result<usize> {
    let n: i64 = conn.query_row(sql, p, |r| r.get(0))?;
    Ok(usize::try_from(n).unwrap_or(0))
}

impl Store {
    /// Indexa o reindexa un documento (una fila por `task_id`; repetir no duplica).
    pub fn index_document(&mut self, doc: &SearchDoc) -> Result<()> {
        let tx = self.conn.transaction()?;
        index_in(&tx, doc)?;
        tx.commit()?;
        Ok(())
    }

    /// Como `complete`, e indexa el documento en la misma transacción: si el índice falla, la
    /// tarea no cambia. El `task_id` de `doc` se sustituye por `id`.
    pub fn complete_indexed(&mut self, id: i64, result: &Value, doc: &SearchDoc) -> Result<Task> {
        self.transition_indexed(id, TaskState::Completed, "completada", result, doc)
    }

    /// Como `await_review`, e indexa en la misma transacción. El `task_id` de `doc` se sustituye
    /// por `id`.
    pub fn await_review_indexed(
        &mut self,
        id: i64,
        result: &Value,
        doc: &SearchDoc,
    ) -> Result<Task> {
        self.transition_indexed(
            id,
            TaskState::AwaitingReview,
            "pendiente de revisión",
            result,
            doc,
        )
    }

    fn transition_indexed(
        &mut self,
        id: i64,
        to: TaskState,
        reason: &str,
        result: &Value,
        doc: &SearchDoc,
    ) -> Result<Task> {
        let tx = self.conn.transaction()?;
        let task = transition(&tx, id, to, reason, Some(result), None)?;
        index_in(
            &tx,
            &SearchDoc {
                task_id: id,
                ..doc.clone()
            },
        )?;
        tx.commit()?;
        Ok(task)
    }

    /// Como `enqueue_closed`, e indexa en la misma transacción. El `task_id` de `doc` se
    /// sustituye por el de la tarea nueva.
    pub fn enqueue_closed_indexed(
        &mut self,
        kind: &str,
        input_ref: &str,
        result: &Value,
        reason: &str,
        doc: &SearchDoc,
    ) -> Result<Task> {
        self.born_indexed(kind, input_ref, TaskState::Completed, result, reason, doc)
    }

    /// Como `enqueue_review`, e indexa en la misma transacción.
    pub fn enqueue_review_indexed(
        &mut self,
        kind: &str,
        input_ref: &str,
        result: &Value,
        reason: &str,
        doc: &SearchDoc,
    ) -> Result<Task> {
        self.born_indexed(
            kind,
            input_ref,
            TaskState::AwaitingReview,
            result,
            reason,
            doc,
        )
    }

    fn born_indexed(
        &mut self,
        kind: &str,
        input_ref: &str,
        state: TaskState,
        result: &Value,
        reason: &str,
        doc: &SearchDoc,
    ) -> Result<Task> {
        let tx = self.conn.transaction()?;
        let task = insert_born(&tx, kind, input_ref, state, result, reason)?;
        index_in(
            &tx,
            &SearchDoc {
                task_id: task.id,
                ..doc.clone()
            },
        )?;
        tx.commit()?;
        Ok(task)
    }

    /// Borra las filas `failed` para que la pasada de indexado las reintente (D179). Devuelve
    /// cuántas. Los triggers mantienen el FTS.
    pub fn delete_failed_index(&mut self) -> Result<usize> {
        let tx = self.conn.transaction()?;
        let n = tx.execute("DELETE FROM search_index WHERE status = 'failed'", [])?;
        tx.commit()?;
        Ok(n)
    }

    /// Tareas de factura o correo sin fila en el índice que ya no están en cola ni en curso, por
    /// id ascendente. Con `include_queued` entran también las que esperan en cola; las que están
    /// en curso nunca.
    pub fn unindexed_tasks(&self, limit: usize, include_queued: bool) -> Result<Vec<Task>> {
        let filter = unindexed_sql(include_queued);
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {TASK_COLUMNS} FROM tasks WHERE {filter} ORDER BY id LIMIT ?1"
        ))?;
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let rows = stmt.query_map([limit], task_from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Búsqueda local (D180). Sin términos, lista por fecha descendente con los filtros.
    pub fn search(&self, query: &SearchQuery) -> Result<SearchPage> {
        validate(&query.filters)?;
        let f = &query.filters;
        let kind = f.kind.map(SearchKind::as_str);
        let limit = SEARCH_PAGE_SIZE as i64;
        let offset = i64::try_from(query.offset).unwrap_or(i64::MAX);

        let (total, results) = match match_expression(&query.terms, query.any) {
            Some(m) => {
                let total = count(
                    &self.conn,
                    &format!(
                        "SELECT count(*) FROM search_fts
                         JOIN search_index i ON i.task_id = search_fts.rowid
                         JOIN tasks t ON t.id = i.task_id
                         WHERE search_fts MATCH :m AND {FILTER_SQL}"
                    ),
                    named_params! {
                        ":m": m, ":kind": kind, ":from": f.from, ":to": f.to,
                        ":party": f.party, ":min": f.min_total_minor,
                        ":max": f.max_total_minor, ":status": f.status,
                    },
                )?;
                let mut stmt = self.conn.prepare(&format!(
                    "SELECT {HIT_COLUMNS}, snippet(search_fts, 3, '', '', '…', 32)
                     FROM search_fts
                     JOIN search_index i ON i.task_id = search_fts.rowid
                     JOIN tasks t ON t.id = i.task_id
                     WHERE search_fts MATCH :m AND {FILTER_SQL}
                     ORDER BY bm25(search_fts), i.task_id DESC
                     LIMIT :limit OFFSET :offset"
                ))?;
                let rows = stmt.query_map(
                    named_params! {
                        ":m": m, ":kind": kind, ":from": f.from, ":to": f.to,
                        ":party": f.party, ":min": f.min_total_minor,
                        ":max": f.max_total_minor, ":status": f.status,
                        ":limit": limit, ":offset": offset,
                    },
                    hit_from_row,
                )?;
                (total, rows.collect::<rusqlite::Result<Vec<_>>>()?)
            }
            None => {
                let total = count(
                    &self.conn,
                    &format!(
                        "SELECT count(*) FROM search_index i
                         JOIN tasks t ON t.id = i.task_id WHERE {FILTER_SQL}"
                    ),
                    named_params! {
                        ":kind": kind, ":from": f.from, ":to": f.to, ":party": f.party,
                        ":min": f.min_total_minor, ":max": f.max_total_minor,
                        ":status": f.status,
                    },
                )?;
                let mut stmt = self.conn.prepare(&format!(
                    "SELECT {HIT_COLUMNS}, substr(i.body, 1, {SNIPPET_CHARS})
                     FROM search_index i JOIN tasks t ON t.id = i.task_id
                     WHERE {FILTER_SQL}
                     ORDER BY i.doc_date IS NULL, i.doc_date DESC, i.task_id DESC
                     LIMIT :limit OFFSET :offset"
                ))?;
                let rows = stmt.query_map(
                    named_params! {
                        ":kind": kind, ":from": f.from, ":to": f.to, ":party": f.party,
                        ":min": f.min_total_minor, ":max": f.max_total_minor,
                        ":status": f.status, ":limit": limit, ":offset": offset,
                    },
                    hit_from_row,
                )?;
                (total, rows.collect::<rusqlite::Result<Vec<_>>>()?)
            }
        };

        let pending_index = count(
            &self.conn,
            &format!(
                "SELECT count(*) FROM tasks WHERE {}",
                unindexed_sql(query.include_queued)
            ),
            [],
        )?;
        let by_status = |status: &str| {
            count(
                &self.conn,
                "SELECT count(*) FROM search_index WHERE status = ?1",
                [status],
            )
        };
        Ok(SearchPage {
            results,
            total,
            pending_index,
            no_text: by_status("no_text")?,
            failed: by_status("failed")?,
        })
    }

    /// Fila del índice de una tarea, si existe.
    pub fn indexed_document(&self, task_id: i64) -> Result<Option<SearchDoc>> {
        let doc = self
            .conn
            .query_row(
                "SELECT task_id, kind, status, title, party, number, doc_date, total_minor,
                        currency, body, detail
                 FROM search_index WHERE task_id = ?1",
                [task_id],
                |r| {
                    let kind: String = r.get(1)?;
                    let status: String = r.get(2)?;
                    Ok(SearchDoc {
                        task_id: r.get(0)?,
                        kind: SearchKind::parse(&kind)
                            .ok_or_else(|| parse_failure(1, "tipo de índice"))?,
                        status: SearchStatus::parse(&status)
                            .ok_or_else(|| parse_failure(2, "estado de índice"))?,
                        title: r.get(3)?,
                        party: r.get(4)?,
                        number: r.get(5)?,
                        doc_date: r.get(6)?,
                        total_minor: r.get(7)?,
                        currency: r.get(8)?,
                        body: r.get(9)?,
                        detail: r.get(10)?,
                    })
                },
            )
            .optional()?;
        Ok(doc)
    }
}
