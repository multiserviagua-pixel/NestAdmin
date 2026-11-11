//! Documentos de venta (D72–D82): borradores, emisión con número correlativo, transiciones,
//! revisiones, anulación, rectificativas y conversión. Cada operación pública es una
//! transacción: ante cualquier fallo no se consume número ni cambia el estado (D73).

use nestadmin_documents::IsoDate;
use nestadmin_sales::{
    calculate, can_convert, can_correct, can_revise, can_void, ensure_editable, next_revision,
    prepare_issue, record_input, status_after_conversion, transition, DocumentKind, DocumentNumber,
    InvoiceContent, InvoiceRecordInput, LineInput, LineMode, Party, Rectification,
    RectificationKind, RectificationMethod, SeriesCounter, SeriesKey, Status, Totals,
};
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::master::{
    company_letterhead_ref, company_logo_sha256, load_company, load_customer, load_product,
    Company, Customer, LetterheadMargins,
};
use super::{
    audit, clean, clean_tax_id, dec, kind_from, method_from, method_text, money, parse_date, plain,
    qualification_from, rect_kind_from, rect_kind_text, status_from, tax_rate_from, SalesResult,
    SalesStoreError,
};
use crate::store::{now_ms, Store};

/// Línea de un borrador; `product_id` solo registra de qué producto se copió (D79).
#[derive(Debug, Clone, PartialEq)]
pub struct DraftLine {
    pub input: LineInput,
    pub product_id: Option<i64>,
}

impl From<LineInput> for DraftLine {
    fn from(input: LineInput) -> Self {
        Self {
            input,
            product_id: None,
        }
    }
}

/// Referencia de rectificación guardada en la rectificativa (D81).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RectificationDraft {
    pub kind: RectificationKind,
    pub method: RectificationMethod,
    pub rectified_base: Option<Decimal>,
    pub rectified_tax: Option<Decimal>,
}

/// Contenido editable de un borrador. La rectificativa se crea con
/// [`Store::create_corrective_draft`].
#[derive(Debug, Clone, PartialEq)]
pub struct DraftInput {
    pub kind: DocumentKind,
    pub customer_id: Option<i64>,
    pub lines: Vec<DraftLine>,
    pub issue_date: Option<IsoDate>,
    pub operation_date: Option<IsoDate>,
    pub due_date: Option<IsoDate>,
    pub withholding_percent: Option<Decimal>,
    pub recipient_requested_tax_id: bool,
    pub art_4_2_declared: bool,
    /// Solo en una rectificativa: cambia el tipo o el método al editar (None: se conservan).
    pub rectification: Option<RectificationDraft>,
}

impl DraftInput {
    pub fn new(kind: DocumentKind) -> Self {
        Self {
            kind,
            customer_id: None,
            lines: Vec::new(),
            issue_date: None,
            operation_date: None,
            due_date: None,
            withholding_percent: None,
            recipient_requested_tax_id: false,
            art_4_2_declared: false,
            rectification: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SalesLineRecord {
    pub input: LineInput,
    /// Importe de la línea fijado al emitir (D75); `None` en borrador.
    pub amount: Option<Decimal>,
    pub product_id: Option<i64>,
}

/// Copia de los datos del emisor tomada al emitir (D90).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssuerSnapshot {
    pub party: Party,
    pub email: Option<String>,
    pub iban: Option<String>,
    /// SHA-256 del logotipo vigente al emitir; se lee con `Store::company_logo_by_hash` y
    /// no cambia aunque la empresa cambie de logotipo después (D76, D90).
    #[serde(default)]
    pub logo_sha256: Option<String>,
    /// SHA-256 del membrete PDF vigente al emitir (D222), leído con
    /// `Store::letterhead_by_hash`. Los documentos emitidos antes del membrete, o sin él, no lo
    /// llevan y se dibujan con el diseño normal y el logotipo.
    #[serde(default)]
    pub letterhead_sha256: Option<String>,
    /// Márgenes del área útil vigentes al emitir; solo con membrete.
    #[serde(default)]
    pub letterhead_margins: Option<LetterheadMargins>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SalesDocument {
    pub id: i64,
    pub kind: DocumentKind,
    pub status: Status,
    pub series_year: Option<u16>,
    pub sequence: Option<u32>,
    pub revision: u32,
    /// `F-2026-0001`; `None` en un borrador que no es revisión de un número existente.
    pub number: Option<String>,
    pub superseded_by: Option<i64>,
    pub customer_id: Option<i64>,
    pub issue_date: Option<IsoDate>,
    pub operation_date: Option<IsoDate>,
    pub due_date: Option<IsoDate>,
    pub withholding_percent: Option<Decimal>,
    pub currency: String,
    pub issuer: Option<IssuerSnapshot>,
    pub recipient: Option<Party>,
    /// Fijados al emitir; en un borrador se calculan al vuelo (o `None` si aún no son válidos).
    pub totals: Option<Totals>,
    pub lines: Vec<SalesLineRecord>,
    pub rectifies_id: Option<i64>,
    pub rectification: Option<RectificationDraft>,
    pub recipient_requested_tax_id: bool,
    pub art_4_2_declared: bool,
    pub recurring_template_id: Option<i64>,
    pub recurring_period: Option<u32>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Default)]
pub struct SalesFilter {
    pub kind: Option<DocumentKind>,
    pub status: Option<Status>,
    /// Incluye las revisiones sustituidas por otra posterior (siguen consultables, D74).
    pub include_superseded: bool,
}

/// Documento nuevo en borrador, común a todas las vías de creación.
pub(crate) struct NewDraft {
    pub kind: DocumentKind,
    pub customer_id: Option<i64>,
    pub issue_date: Option<IsoDate>,
    pub operation_date: Option<IsoDate>,
    pub due_date: Option<IsoDate>,
    pub withholding: Option<Decimal>,
    pub recipient_requested_tax_id: bool,
    pub art_4_2_declared: bool,
    /// Año y secuencia de un número ya consumido (borrador de una revisión).
    pub number: Option<(u16, u32)>,
    pub revision: u32,
    pub rectifies_id: Option<i64>,
    pub rectification: Option<RectificationDraft>,
    pub recurring: Option<(i64, u32)>,
    pub lines: Vec<DraftLine>,
}

impl NewDraft {
    pub(crate) fn new(kind: DocumentKind) -> Self {
        Self {
            kind,
            customer_id: None,
            issue_date: None,
            operation_date: None,
            due_date: None,
            withholding: None,
            recipient_requested_tax_id: false,
            art_4_2_declared: false,
            number: None,
            revision: 1,
            rectifies_id: None,
            rectification: None,
            recurring: None,
            lines: Vec::new(),
        }
    }
}

fn date_text(date: Option<IsoDate>) -> Option<String> {
    date.map(|d| d.to_string())
}

pub(crate) fn insert_draft(conn: &Connection, draft: &NewDraft) -> SalesResult<i64> {
    let now = now_ms();
    conn.execute(
        "INSERT INTO sales_documents (kind, status, series_year, sequence, revision, customer_id, \
         issue_date, operation_date, due_date, withholding_percent, rectifies_id, \
         rectification_kind, rectification_method, rectified_base, rectified_tax, \
         recipient_requested_tax_id, art_4_2_declared, recurring_template_id, recurring_period, \
         created_at, updated_at) \
         VALUES (?1, 'draft', ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, \
         ?17, ?18, ?19, ?19)",
        params![
            draft.kind.as_str(),
            draft.number.map(|(year, _)| i64::from(year)),
            draft.number.map(|(_, sequence)| i64::from(sequence)),
            draft.revision,
            draft.customer_id,
            date_text(draft.issue_date),
            date_text(draft.operation_date),
            date_text(draft.due_date),
            draft.withholding.map(plain),
            draft.rectifies_id,
            draft.rectification.map(|r| rect_kind_text(r.kind)),
            draft.rectification.map(|r| method_text(r.method)),
            draft
                .rectification
                .and_then(|r| r.rectified_base)
                .map(money),
            draft.rectification.and_then(|r| r.rectified_tax).map(money),
            draft.recipient_requested_tax_id,
            draft.art_4_2_declared,
            draft.recurring.map(|(template, _)| template),
            draft.recurring.map(|(_, period)| i64::from(period)),
            now,
        ],
    )?;
    let id = conn.last_insert_rowid();
    write_lines(conn, id, &draft.lines)?;
    Ok(id)
}

/// Sustituye las líneas del borrador (los triggers impiden tocar las de un documento emitido).
fn write_lines(conn: &Connection, document_id: i64, lines: &[DraftLine]) -> SalesResult<()> {
    conn.execute(
        "DELETE FROM sales_lines WHERE document_id = ?1",
        [document_id],
    )?;
    for (index, line) in lines.iter().enumerate() {
        insert_line(conn, document_id, index as i64 + 1, line)?;
    }
    Ok(())
}

fn insert_line(
    conn: &Connection,
    document_id: i64,
    position: i64,
    line: &DraftLine,
) -> SalesResult<()> {
    let input = &line.input;
    conn.execute(
        "INSERT INTO sales_lines (document_id, position, description, quantity, unit_price, \
         discount, tax_rate, qualification, exemption_description, amount, product_id) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, NULL, ?10)",
        params![
            document_id,
            position,
            input.description,
            plain(input.quantity),
            plain(input.unit_price),
            plain(input.discount),
            plain(input.tax_rate.percent()),
            input.exemption.as_ref().map(|e| e.qualification.as_str()),
            input
                .exemption
                .as_ref()
                .and_then(|e| e.description_text().map(str::to_owned)),
            line.product_id,
        ],
    )?;
    Ok(())
}

fn load_lines(conn: &Connection, document_id: i64) -> SalesResult<Vec<SalesLineRecord>> {
    let mut stmt = conn.prepare(
        "SELECT description, quantity, unit_price, discount, tax_rate, qualification, \
         exemption_description, amount, product_id FROM sales_lines WHERE document_id = ?1 \
         ORDER BY position",
    )?;
    let raws = stmt
        .query_map([document_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                [r.get::<_, String>(1)?, r.get(2)?, r.get(3)?, r.get(4)?],
                (
                    r.get::<_, Option<String>>(5)?,
                    r.get::<_, Option<String>>(6)?,
                ),
                r.get::<_, Option<String>>(7)?,
                r.get::<_, Option<i64>>(8)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    raws.into_iter()
        .map(
            |(
                description,
                [quantity, price, discount, rate],
                (qualification, ex_desc),
                amount,
                product_id,
            )| {
                let exemption = match qualification {
                    Some(q) => Some(nestadmin_sales::Exemption {
                        qualification: qualification_from(&q)?,
                        description: ex_desc,
                    }),
                    None => None,
                };
                Ok(SalesLineRecord {
                    input: LineInput {
                        description,
                        quantity: dec("quantity", &quantity)?,
                        unit_price: dec("unit_price", &price)?,
                        discount: dec("discount", &discount)?,
                        tax_rate: tax_rate_from(&rate)?,
                        exemption,
                    },
                    amount: amount.map(|a| dec("amount", &a)).transpose()?,
                    product_id,
                })
            },
        )
        .collect()
}

struct Raw {
    id: i64,
    kind: String,
    status: String,
    series_year: Option<i64>,
    sequence: Option<i64>,
    revision: i64,
    superseded_by: Option<i64>,
    customer_id: Option<i64>,
    issue_date: Option<String>,
    operation_date: Option<String>,
    due_date: Option<String>,
    withholding: Option<String>,
    issuer: Option<String>,
    recipient: Option<String>,
    totals: Option<String>,
    rectifies_id: Option<i64>,
    rect_kind: Option<String>,
    rect_method: Option<String>,
    rect_base: Option<String>,
    rect_tax: Option<String>,
    requested: bool,
    art_4_2: bool,
    recurring_template: Option<i64>,
    recurring_period: Option<i64>,
    created_at: i64,
    updated_at: i64,
}

const RAW_COLS: &str = "id, kind, status, series_year, sequence, revision, superseded_by, \
    customer_id, issue_date, operation_date, due_date, withholding_percent, issuer_snapshot, \
    recipient_snapshot, totals, rectifies_id, rectification_kind, rectification_method, \
    rectified_base, rectified_tax, recipient_requested_tax_id, art_4_2_declared, \
    recurring_template_id, recurring_period, created_at, updated_at";

fn raw_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Raw> {
    Ok(Raw {
        id: r.get(0)?,
        kind: r.get(1)?,
        status: r.get(2)?,
        series_year: r.get(3)?,
        sequence: r.get(4)?,
        revision: r.get(5)?,
        superseded_by: r.get(6)?,
        customer_id: r.get(7)?,
        issue_date: r.get(8)?,
        operation_date: r.get(9)?,
        due_date: r.get(10)?,
        withholding: r.get(11)?,
        issuer: r.get(12)?,
        recipient: r.get(13)?,
        totals: r.get(14)?,
        rectifies_id: r.get(15)?,
        rect_kind: r.get(16)?,
        rect_method: r.get(17)?,
        rect_base: r.get(18)?,
        rect_tax: r.get(19)?,
        requested: r.get::<_, i64>(20)? != 0,
        art_4_2: r.get::<_, i64>(21)? != 0,
        recurring_template: r.get(22)?,
        recurring_period: r.get(23)?,
        created_at: r.get(24)?,
        updated_at: r.get(25)?,
    })
}

fn opt_date(field: &'static str, text: Option<String>) -> SalesResult<Option<IsoDate>> {
    text.map(|t| parse_date(field, &t)).transpose()
}

fn draft_mode(kind: DocumentKind, rect: Option<&RectificationDraft>) -> LineMode {
    match (kind, rect.map(|r| r.method)) {
        (DocumentKind::CorrectiveInvoice, Some(RectificationMethod::Differences)) => {
            LineMode::Difference
        }
        _ => LineMode::Standard,
    }
}

pub(crate) fn load_document(conn: &Connection, id: i64) -> SalesResult<SalesDocument> {
    let raw = conn
        .query_row(
            &format!("SELECT {RAW_COLS} FROM sales_documents WHERE id = ?1"),
            [id],
            raw_row,
        )
        .optional()?
        .ok_or(SalesStoreError::not_found("documento", id))?;
    let kind = kind_from(&raw.kind)?;
    let status = status_from(&raw.status)?;
    let lines = load_lines(conn, id)?;
    let rectification = match (&raw.rect_kind, &raw.rect_method) {
        (Some(k), Some(m)) => Some(RectificationDraft {
            kind: rect_kind_from(k)?,
            method: method_from(m)?,
            rectified_base: raw
                .rect_base
                .as_deref()
                .map(|v| dec("rectified_base", v))
                .transpose()?,
            rectified_tax: raw
                .rect_tax
                .as_deref()
                .map(|v| dec("rectified_tax", v))
                .transpose()?,
        }),
        _ => None,
    };
    let withholding_percent = raw
        .withholding
        .as_deref()
        .map(|v| dec("withholding_percent", v))
        .transpose()?;
    let totals = match (status, &raw.totals) {
        (Status::Draft, _) => {
            let inputs: Vec<LineInput> = lines.iter().map(|l| l.input.clone()).collect();
            calculate(
                &inputs,
                withholding_percent,
                draft_mode(kind, rectification.as_ref()),
            )
            .ok()
        }
        (_, Some(json)) => Some(serde_json::from_str(json)?),
        (_, None) => None,
    };
    let series_year = raw.series_year.map(|y| y as u16);
    let sequence = raw.sequence.map(|s| s as u32);
    let number = match (series_year, sequence) {
        (Some(year), Some(sequence)) => Some(
            DocumentNumber {
                series: SeriesKey::new(kind, year)?,
                sequence,
            }
            .to_string(),
        ),
        _ => None,
    };
    Ok(SalesDocument {
        id: raw.id,
        kind,
        status,
        series_year,
        sequence,
        revision: raw.revision as u32,
        number,
        superseded_by: raw.superseded_by,
        customer_id: raw.customer_id,
        issue_date: opt_date("issue_date", raw.issue_date)?,
        operation_date: opt_date("operation_date", raw.operation_date)?,
        due_date: opt_date("due_date", raw.due_date)?,
        withholding_percent,
        currency: "EUR".to_owned(),
        issuer: raw
            .issuer
            .as_deref()
            .map(serde_json::from_str)
            .transpose()?,
        recipient: raw
            .recipient
            .as_deref()
            .map(serde_json::from_str)
            .transpose()?,
        totals,
        lines,
        rectifies_id: raw.rectifies_id,
        rectification,
        recipient_requested_tax_id: raw.requested,
        art_4_2_declared: raw.art_4_2,
        recurring_template_id: raw.recurring_template,
        recurring_period: raw.recurring_period.map(|p| p as u32),
        created_at: raw.created_at,
        updated_at: raw.updated_at,
    })
}

/// `keep_customer` es el cliente que el borrador ya tenía: seguir con él no se rechaza aunque
/// se haya retirado después (una rectificativa debe poder hacerse igualmente).
fn check_draft_input(
    conn: &Connection,
    input: &DraftInput,
    keep_customer: Option<i64>,
) -> SalesResult<()> {
    if let Some(customer_id) = input.customer_id {
        let customer = load_customer(conn, customer_id)?;
        if customer.retired && keep_customer != Some(customer_id) {
            return Err(SalesStoreError::invalid(
                "customer_id",
                "el cliente está retirado",
            ));
        }
    }
    if let Some(w) = input.withholding_percent {
        if w < Decimal::ZERO || w > Decimal::from(100) {
            return Err(SalesStoreError::invalid(
                "withholding_percent",
                "la retención debe estar entre 0 y 100",
            ));
        }
    }
    for line in &input.lines {
        if let Some(product_id) = line.product_id {
            load_product(conn, product_id)?;
        }
    }
    Ok(())
}

fn party_of_company(company: &Company) -> Party {
    Party {
        name: clean(&company.name),
        tax_id: clean_tax_id(&company.tax_id),
        address: clean(&company.address),
    }
}

fn party_of_customer(customer: &Customer) -> Party {
    Party {
        name: clean(&Some(customer.data.name.clone())),
        tax_id: clean_tax_id(&customer.data.tax_id),
        address: clean(&customer.data.address),
    }
}

fn parse_series(doc: &SalesDocument) -> SalesResult<Option<DocumentNumber>> {
    match (doc.series_year, doc.sequence) {
        (Some(year), Some(sequence)) => Ok(Some(DocumentNumber {
            series: SeriesKey::new(doc.kind, year)?,
            sequence,
        })),
        _ => Ok(None),
    }
}

fn number_detail(doc: &SalesDocument) -> serde_json::Value {
    json!({ "id": doc.id, "number": doc.number, "revision": doc.revision })
}

pub(crate) fn touch_status(
    conn: &Connection,
    id: i64,
    from: Status,
    to: Status,
) -> SalesResult<()> {
    let changed = conn.execute(
        "UPDATE sales_documents SET status = ?3, updated_at = ?4 WHERE id = ?1 AND status = ?2",
        params![id, from.as_str(), to.as_str(), now_ms()],
    )?;
    if changed != 1 {
        return Err(SalesStoreError::Conflict(format!(
            "el documento {id} cambió de estado durante la operación"
        )));
    }
    Ok(())
}

/// R5 corresponde a rectificar una simplificada y solo a ella (D81).
fn check_rectification_kind(original: &SalesDocument, kind: RectificationKind) -> SalesResult<()> {
    let simplified = original.kind == DocumentKind::SimplifiedInvoice;
    if simplified != (kind == RectificationKind::R5) {
        return Err(SalesStoreError::invalid(
            "rectification.kind",
            "R5 corresponde a rectificar una simplificada y solo a ella",
        ));
    }
    Ok(())
}

/// Base y cuota de la factura rectificada (D113): por sustitución, siempre las guardadas de la
/// original; por diferencias no se admiten.
fn rectified_amounts(
    original: &SalesDocument,
    method: RectificationMethod,
) -> SalesResult<(Option<Decimal>, Option<Decimal>)> {
    match method {
        RectificationMethod::Substitution => {
            let totals = original.totals.as_ref().ok_or_else(|| {
                SalesStoreError::Corrupt(format!("la factura {} no tiene totales", original.id))
            })?;
            Ok((Some(totals.base_total), Some(totals.tax_total)))
        }
        RectificationMethod::Differences => Ok((None, None)),
    }
}

/// Una factura no admite dos sustituciones en vigor ni mezclar sustitución con diferencias
/// (D81, D88); varias por diferencias sí. `exclude` es la propia rectificativa si ya existe.
fn ensure_corrective_compatible(
    conn: &Connection,
    original_id: i64,
    method: RectificationMethod,
    exclude: Option<i64>,
) -> SalesResult<()> {
    let mut stmt = conn.prepare(
        "SELECT rectification_method FROM sales_documents WHERE rectifies_id = ?1 \
         AND status IN ('issued', 'sent') AND id <> ?2",
    )?;
    let methods = stmt
        .query_map(params![original_id, exclude.unwrap_or(-1)], |r| {
            r.get::<_, Option<String>>(0)
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for text in methods {
        let existing = method_from(&text.unwrap_or_default())?;
        match (method, existing) {
            (RectificationMethod::Differences, RectificationMethod::Differences) => {}
            (RectificationMethod::Substitution, RectificationMethod::Substitution) => {
                return Err(SalesStoreError::Conflict(format!(
                    "la factura {original_id} ya tiene una rectificativa por sustitución en vigor"
                )))
            }
            _ => {
                return Err(SalesStoreError::Conflict(format!(
                    "la factura {original_id} ya tiene rectificativas en vigor de otro método: \
                     no se mezclan sustitución y diferencias"
                )))
            }
        }
    }
    Ok(())
}

/// Reglas de la rectificativa que se evalúan al crearla y de nuevo al emitirla.
fn rectification_for_issue(
    conn: &Connection,
    doc: &SalesDocument,
) -> SalesResult<Option<Rectification>> {
    if doc.kind != DocumentKind::CorrectiveInvoice {
        return Ok(None);
    }
    let (Some(original_id), Some(rect)) = (doc.rectifies_id, doc.rectification) else {
        return Err(SalesStoreError::invalid(
            "rectification",
            "la rectificativa no referencia ninguna factura",
        ));
    };
    let original = load_document(conn, original_id)?;
    can_correct(original.kind, original.status)?;
    check_rectification_kind(&original, rect.kind)?;
    ensure_corrective_compatible(conn, original_id, rect.method, Some(doc.id))?;
    let number = parse_series(&original)?.ok_or_else(|| {
        SalesStoreError::Conflict(format!("la factura {original_id} no tiene número"))
    })?;
    // Lo que traiga el borrador en base y cuota se ignora (D113).
    let (rectified_base, rectified_tax) = rectified_amounts(&original, rect.method)?;
    Ok(Some(Rectification {
        rectified_number: Some(number),
        rectified_date: original.issue_date,
        kind: Some(rect.kind),
        method: Some(rect.method),
        rectified_base,
        rectified_tax,
    }))
}

/// Id de la revisión anterior (`revision - 1`) con el mismo número.
fn previous_revision_id(conn: &Connection, doc: &SalesDocument) -> SalesResult<Option<i64>> {
    let (Some(year), Some(sequence)) = (doc.series_year, doc.sequence) else {
        return Ok(None);
    };
    Ok(conn
        .query_row(
            "SELECT id FROM sales_documents WHERE kind = ?1 AND series_year = ?2 \
             AND sequence = ?3 AND revision = ?4",
            params![
                doc.kind.as_str(),
                year,
                sequence,
                doc.revision.saturating_sub(1)
            ],
            |r| r.get(0),
        )
        .optional()?)
}

/// `true` si hay un borrador de revisión posterior pendiente de emitir (D74).
fn has_pending_revision(conn: &Connection, doc: &SalesDocument) -> SalesResult<bool> {
    let (Some(year), Some(sequence)) = (doc.series_year, doc.sequence) else {
        return Ok(false);
    };
    let pending: i64 = conn.query_row(
        "SELECT count(*) FROM sales_documents WHERE kind = ?1 AND series_year = ?2 \
         AND sequence = ?3 AND revision > ?4 AND status = 'draft'",
        params![doc.kind.as_str(), year, sequence, doc.revision],
        |r| r.get(0),
    )?;
    Ok(pending > 0)
}

fn issue_in_tx<F>(
    tx: &Transaction<'_>,
    id: i64,
    issue_date: IsoDate,
    today: IsoDate,
    hook: F,
) -> SalesResult<SalesDocument>
where
    F: FnOnce(&Transaction<'_>, Option<&InvoiceRecordInput>) -> SalesResult<()>,
{
    let doc = load_document(tx, id)?;
    transition(doc.kind, doc.status, Status::Issued)?;

    // Fechas (D73): no se expide con fecha futura ni con vencimiento anterior a la expedición.
    if issue_date > today {
        return Err(SalesStoreError::invalid(
            "issue_date",
            "la fecha de expedición no puede ser posterior a hoy",
        ));
    }
    if doc.due_date.is_some_and(|due| due < issue_date) {
        return Err(SalesStoreError::invalid(
            "due_date",
            "el vencimiento no puede ser anterior a la fecha de expedición",
        ));
    }
    // Una revisión solo se emite si la anterior sigue vigente (D74).
    let previous_id = if doc.revision > 1 {
        let previous_id = previous_revision_id(tx, &doc)?.ok_or_else(|| {
            SalesStoreError::Conflict(format!("no existe la revisión anterior del documento {id}"))
        })?;
        let previous = load_document(tx, previous_id)?;
        if can_revise(previous.kind, previous.status).is_err() || previous.superseded_by.is_some() {
            return Err(SalesStoreError::Conflict(format!(
                "la revisión anterior del documento {id} ya no admite revisión (estado {})",
                previous.status.as_str()
            )));
        }
        if previous.issue_date.is_some_and(|prev| issue_date < prev) {
            return Err(SalesStoreError::invalid(
                "issue_date",
                "una revisión no puede tener fecha anterior a la de la revisión previa",
            ));
        }
        Some(previous_id)
    } else {
        None
    };

    let company = load_company(tx)?;
    let customer = doc.customer_id.map(|c| load_customer(tx, c)).transpose()?;
    let logo_sha256 = company_logo_sha256(tx)?;
    // El membrete y los márgenes vigentes se copian como el logotipo: un cambio posterior no
    // altera este documento (D222).
    let letterhead = company_letterhead_ref(tx)?;
    let issuer = company.as_ref().map(|c| IssuerSnapshot {
        party: party_of_company(c),
        email: clean(&c.email),
        iban: clean(&c.iban),
        logo_sha256: logo_sha256.clone(),
        letterhead_sha256: letterhead.as_ref().map(|(hash, _)| hash.clone()),
        letterhead_margins: letterhead.as_ref().map(|(_, margins)| *margins),
    });
    let recipient = customer.as_ref().map(party_of_customer);
    let lines: Vec<LineInput> = doc.lines.iter().map(|l| l.input.clone()).collect();

    let rectification = rectification_for_issue(tx, &doc)?;
    let rectified_base = rectification.as_ref().and_then(|r| r.rectified_base);
    let rectified_tax = rectification.as_ref().and_then(|r| r.rectified_tax);
    let content = InvoiceContent {
        kind: doc.kind,
        issue_date: Some(issue_date),
        operation_date: doc.operation_date,
        issuer: issuer.as_ref().map(|i| i.party.clone()).unwrap_or_default(),
        recipient: recipient.clone(),
        lines: lines.clone(),
        withholding_percent: doc.withholding_percent,
        rectification,
        recipient_requested_tax_id: doc.recipient_requested_tax_id,
        art_4_2_declared: doc.art_4_2_declared,
    };
    let totals = if doc.kind.is_invoice() {
        prepare_issue(&content).map_err(SalesStoreError::from_issues)?
    } else {
        calculate(&lines, doc.withholding_percent, LineMode::Standard)?
    };

    // Número: una revisión conserva el suyo; el resto toma el siguiente de la serie (D73, D74).
    let number = match parse_series(&doc)? {
        Some(number) => {
            SeriesCounter::with_last(number.series, number.sequence - 1, None)
                .commit(&number, issue_date)?;
            number
        }
        None => {
            let key = SeriesKey::new(doc.kind, issue_date.year)?;
            let stored: Option<(i64, Option<String>)> = tx
                .query_row(
                    "SELECT last, last_date FROM series WHERE kind = ?1 AND year = ?2",
                    params![doc.kind.as_str(), i64::from(key.year)],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            let (last, last_date) = match stored {
                Some((last, date)) => (last as u32, opt_date("series.last_date", date)?),
                None => (0, None),
            };
            let mut counter = SeriesCounter::with_last(key, last, last_date);
            let number = counter.next_number()?;
            counter.commit(&number, issue_date)?;
            tx.execute(
                "INSERT INTO series (kind, year, last, last_date) VALUES (?1, ?2, ?3, ?4) \
                 ON CONFLICT(kind, year) DO UPDATE SET last = excluded.last, \
                 last_date = excluded.last_date",
                params![
                    doc.kind.as_str(),
                    i64::from(key.year),
                    counter.last(),
                    date_text(counter.last_date()),
                ],
            )?;
            number
        }
    };

    let record = if doc.kind.is_invoice() {
        Some(record_input(&content, number, &totals).map_err(SalesStoreError::from_issues)?)
    } else {
        None
    };

    for (position, amount) in totals.line_amounts.iter().enumerate() {
        tx.execute(
            "UPDATE sales_lines SET amount = ?3 WHERE document_id = ?1 AND position = ?2",
            params![id, position as i64 + 1, money(*amount)],
        )?;
    }
    let changed = tx.execute(
        "UPDATE sales_documents SET status = 'issued', series_year = ?2, sequence = ?3, \
         issue_date = ?4, issuer_snapshot = ?5, recipient_snapshot = ?6, totals = ?7, \
         base_total = ?8, tax_total = ?9, total = ?10, amount_due = ?11, updated_at = ?12, \
         rectified_base = ?13, rectified_tax = ?14 \
         WHERE id = ?1 AND status = 'draft'",
        params![
            id,
            i64::from(number.series.year),
            number.sequence,
            issue_date.to_string(),
            issuer.as_ref().map(serde_json::to_string).transpose()?,
            recipient.as_ref().map(serde_json::to_string).transpose()?,
            serde_json::to_string(&totals)?,
            money(totals.base_total),
            money(totals.tax_total),
            money(totals.total),
            money(totals.amount_due),
            now_ms(),
            rectified_base.map(money),
            rectified_tax.map(money),
        ],
    )?;
    if changed != 1 {
        return Err(SalesStoreError::Conflict(format!(
            "el documento {id} cambió de estado durante la emisión"
        )));
    }
    if let Some(previous_id) = previous_id {
        let replaced = tx.execute(
            "UPDATE sales_documents SET superseded_by = ?1, updated_at = ?2 \
             WHERE id = ?3 AND superseded_by IS NULL",
            params![id, now_ms(), previous_id],
        )?;
        if replaced != 1 {
            return Err(SalesStoreError::Conflict(format!(
                "la revisión anterior del documento {id} ya está sustituida"
            )));
        }
    }

    // El gancho escribe en la misma transacción (registro Verifactu, D83): si falla, se
    // deshace también el número.
    hook(tx, record.as_ref())?;

    let issued = load_document(tx, id)?;
    audit(tx, "sales.issue", &number_detail(&issued))?;
    Ok(issued)
}

impl Store {
    // --- Borradores ---

    pub fn create_sales_draft(&mut self, input: &DraftInput) -> SalesResult<i64> {
        if input.kind == DocumentKind::CorrectiveInvoice {
            return Err(SalesStoreError::invalid(
                "kind",
                "una rectificativa se crea con create_corrective_draft",
            ));
        }
        let tx = self.conn.transaction()?;
        check_draft_input(&tx, input, None)?;
        let mut draft = NewDraft::new(input.kind);
        apply_input(&mut draft, input);
        let id = insert_draft(&tx, &draft)?;
        tx.commit()?;
        Ok(id)
    }

    /// Edita un borrador; un documento ya emitido no se edita (D74, D81).
    pub fn update_sales_draft(&mut self, id: i64, input: &DraftInput) -> SalesResult<()> {
        let tx = self.conn.transaction()?;
        let current = load_document(&tx, id)?;
        ensure_editable(current.status)?;
        if input.kind != current.kind {
            return Err(SalesStoreError::invalid(
                "kind",
                "el tipo de un documento no se cambia",
            ));
        }
        check_draft_input(&tx, input, current.customer_id)?;
        // En una rectificativa solo se cambian el tipo y el método; base y cuota rectificadas
        // se recalculan de la original, nunca se toman de la entrada (D113).
        let rectification = match current.kind {
            DocumentKind::CorrectiveInvoice => {
                let wanted = input.rectification.or(current.rectification);
                let (Some(original_id), Some(wanted)) = (current.rectifies_id, wanted) else {
                    return Err(SalesStoreError::invalid(
                        "rectification",
                        "la rectificativa no referencia ninguna factura",
                    ));
                };
                let original = load_document(&tx, original_id)?;
                check_rectification_kind(&original, wanted.kind)?;
                let (rectified_base, rectified_tax) = rectified_amounts(&original, wanted.method)?;
                Some(RectificationDraft {
                    kind: wanted.kind,
                    method: wanted.method,
                    rectified_base,
                    rectified_tax,
                })
            }
            _ => None,
        };
        tx.execute(
            "UPDATE sales_documents SET customer_id = ?2, issue_date = ?3, operation_date = ?4, \
             due_date = ?5, withholding_percent = ?6, recipient_requested_tax_id = ?7, \
             art_4_2_declared = ?8, rectification_kind = ?9, rectification_method = ?10, \
             rectified_base = ?11, rectified_tax = ?12, updated_at = ?13 WHERE id = ?1",
            params![
                id,
                input.customer_id,
                date_text(input.issue_date),
                date_text(input.operation_date),
                date_text(input.due_date),
                input.withholding_percent.map(plain),
                input.recipient_requested_tax_id,
                input.art_4_2_declared,
                rectification.map(|r| rect_kind_text(r.kind)),
                rectification.map(|r| method_text(r.method)),
                rectification.and_then(|r| r.rectified_base).map(money),
                rectification.and_then(|r| r.rectified_tax).map(money),
                now_ms(),
            ],
        )?;
        write_lines(&tx, id, &input.lines)?;
        tx.commit()?;
        Ok(())
    }

    /// Añade al borrador una línea copiada de un producto del catálogo (D79).
    pub fn add_line_from_product(
        &mut self,
        id: i64,
        product_id: i64,
        quantity: Decimal,
        discount: Decimal,
    ) -> SalesResult<()> {
        let tx = self.conn.transaction()?;
        let doc = load_document(&tx, id)?;
        ensure_editable(doc.status)?;
        let product = load_product(&tx, product_id)?;
        let line = product.to_line(quantity, discount)?;
        insert_line(
            &tx,
            id,
            doc.lines.len() as i64 + 1,
            &DraftLine {
                input: line,
                product_id: Some(product_id),
            },
        )?;
        tx.execute(
            "UPDATE sales_documents SET updated_at = ?2 WHERE id = ?1",
            params![id, now_ms()],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn delete_sales_draft(&mut self, id: i64) -> SalesResult<()> {
        let tx = self.conn.transaction()?;
        let doc = load_document(&tx, id)?;
        ensure_editable(doc.status)?;
        // Borrar el borrador de una plantilla recurrente deja constancia del periodo omitido:
        // sin ella el siguiente arranque lo volvería a generar (D87).
        if let (Some(template), Some(period)) = (doc.recurring_template_id, doc.recurring_period) {
            tx.execute(
                "INSERT OR IGNORE INTO recurring_skips (template_id, period, created_at) \
                 VALUES (?1, ?2, ?3)",
                params![template, period, now_ms()],
            )?;
            audit(
                &tx,
                "sales.recurring_skip",
                &json!({ "id": id, "template": template, "period": period }),
            )?;
        }
        let converted: i64 = tx.query_row(
            "SELECT count(*) FROM document_links WHERE to_id = ?1 AND relation = 'converted'",
            [id],
            |r| r.get(0),
        )?;
        if converted > 0 {
            return Err(SalesStoreError::Conflict(
                "el borrador procede de una conversión y no se elimina".into(),
            ));
        }
        tx.execute(
            "DELETE FROM document_links WHERE to_id = ?1 OR from_id = ?1",
            [id],
        )?;
        tx.execute("DELETE FROM sales_documents WHERE id = ?1", [id])?;
        tx.commit()?;
        Ok(())
    }

    pub fn sales_document(&self, id: i64) -> SalesResult<SalesDocument> {
        load_document(&self.conn, id)
    }

    pub fn list_sales_documents(&self, filter: &SalesFilter) -> SalesResult<Vec<SalesDocument>> {
        let mut stmt = self.conn.prepare(
            "SELECT id FROM sales_documents WHERE (?1 IS NULL OR kind = ?1) \
             AND (?2 IS NULL OR status = ?2) AND (?3 OR superseded_by IS NULL) ORDER BY id",
        )?;
        let ids = stmt
            .query_map(
                params![
                    filter.kind.map(DocumentKind::as_str),
                    filter.status.map(Status::as_str),
                    filter.include_superseded
                ],
                |r| r.get::<_, i64>(0),
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        ids.into_iter()
            .map(|id| load_document(&self.conn, id))
            .collect()
    }

    // --- Emisión (D73) ---

    /// `today` es la fecha actual del llamador: no se expide con fecha posterior (D73).
    pub fn issue_sales_document(
        &mut self,
        id: i64,
        issue_date: IsoDate,
        today: IsoDate,
    ) -> SalesResult<SalesDocument> {
        // Una factura solo se emite por el camino con registro Verifactu (D193).
        if load_document(&self.conn, id)?.kind.is_invoice() {
            return Err(SalesStoreError::Conflict(
                "una factura se emite con issue_sales_invoice_registered (registro Verifactu)"
                    .into(),
            ));
        }
        self.issue_sales_document_with(id, issue_date, today, |_, _| Ok(()))
    }

    /// Emite el documento en una sola transacción: valida, toma el número de la serie, fija el
    /// contenido y las copias de emisor y destinatario y, en facturas, entrega a `hook` los
    /// datos del registro Verifactu (D83) dentro de la misma transacción. Si `hook` o cualquier
    /// paso falla, no se consume número ni cambia el estado (T15). Rechaza una fecha de
    /// expedición posterior a `today` y un vencimiento anterior a la expedición.
    ///
    /// El mensaje de un [`SalesStoreError::External`] devuelto por `hook` llega tal cual al
    /// llamador y a los registros: no debe contener secretos (credenciales, tokens, claves).
    pub fn issue_sales_document_with<F>(
        &mut self,
        id: i64,
        issue_date: IsoDate,
        today: IsoDate,
        hook: F,
    ) -> SalesResult<SalesDocument>
    where
        F: FnOnce(&Transaction<'_>, Option<&InvoiceRecordInput>) -> SalesResult<()>,
    {
        // Inmediata: el número se toma con el bloqueo de escritura ya adquirido.
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let issued = issue_in_tx(&tx, id, issue_date, today, hook)?;
        tx.commit()?;
        Ok(issued)
    }

    // --- Transiciones ---

    /// Cambia el estado según D74. La emisión, la anulación y la conversión tienen operaciones
    /// propias porque asignan número, registran en Verifactu o crean otro documento.
    pub fn transition_sales_document(&mut self, id: i64, to: Status) -> SalesResult<SalesDocument> {
        let tx = self.conn.transaction()?;
        let doc = load_document(&tx, id)?;
        transition(doc.kind, doc.status, to)?;
        if doc.superseded_by.is_some() {
            return Err(SalesStoreError::Conflict(
                "la revisión está sustituida por otra posterior".into(),
            ));
        }
        if has_pending_revision(&tx, &doc)? {
            return Err(SalesStoreError::Conflict(
                "hay un borrador de revisión pendiente: emítalo o elimínelo antes".into(),
            ));
        }
        let dedicated = match to {
            Status::Issued => Some("issue_sales_document"),
            Status::Voided => Some("void_sales_invoice"),
            Status::Converted | Status::Invoiced => Some("convert_sales_documents"),
            _ => None,
        };
        if let Some(operation) = dedicated {
            return Err(SalesStoreError::Conflict(format!(
                "el estado {} se alcanza con {operation}",
                to.as_str()
            )));
        }
        touch_status(&tx, id, doc.status, to)?;
        audit(
            &tx,
            "sales.transition",
            &json!({
                "id": id, "number": doc.number, "revision": doc.revision,
                "from": doc.status.as_str(), "to": to.as_str()
            }),
        )?;
        let updated = load_document(&tx, id)?;
        tx.commit()?;
        Ok(updated)
    }

    // --- Revisión (D74) ---

    /// Crea el borrador de la revisión `n+1` de un presupuesto o albarán emitido. Al emitirlo
    /// conserva el número sin consumir la serie y deja sustituida la revisión anterior.
    pub fn revise_sales_document(&mut self, id: i64) -> SalesResult<i64> {
        let tx = self.conn.transaction()?;
        let doc = load_document(&tx, id)?;
        can_revise(doc.kind, doc.status)?;
        let revision = next_revision(doc.kind, doc.status, doc.revision)?;
        let (Some(year), Some(sequence)) = (doc.series_year, doc.sequence) else {
            return Err(SalesStoreError::Conflict(format!(
                "el documento {id} no tiene número que revisar"
            )));
        };
        if doc.superseded_by.is_some() {
            return Err(SalesStoreError::Conflict(
                "la revisión ya está sustituida por otra posterior".into(),
            ));
        }
        let later: i64 = tx.query_row(
            "SELECT count(*) FROM sales_documents WHERE kind = ?1 AND series_year = ?2 \
             AND sequence = ?3 AND revision >= ?4",
            params![doc.kind.as_str(), year, sequence, revision],
            |r| r.get(0),
        )?;
        if later > 0 {
            return Err(SalesStoreError::Conflict(
                "ya hay un borrador de revisión pendiente de emitir".into(),
            ));
        }
        let mut draft = NewDraft::new(doc.kind);
        draft.customer_id = doc.customer_id;
        draft.operation_date = doc.operation_date;
        draft.due_date = doc.due_date;
        draft.withholding = doc.withholding_percent;
        draft.number = Some((year, sequence));
        draft.revision = revision;
        draft.lines = copy_lines(&doc);
        let new_id = insert_draft(&tx, &draft)?;
        link(&tx, id, new_id, "revision")?;
        audit(
            &tx,
            "sales.revise",
            &json!({ "id": id, "new_id": new_id, "number": doc.number, "revision": revision }),
        )?;
        tx.commit()?;
        Ok(new_id)
    }

    // --- Anulación (D82) ---

    pub fn void_sales_invoice(&mut self, id: i64) -> SalesResult<SalesDocument> {
        // Sin registro de anulación una factura no se anula (D193): solo por el camino registrado.
        let doc = load_document(&self.conn, id)?;
        can_void(doc.kind, doc.status)?;
        Err(SalesStoreError::Conflict(format!(
            "la factura {id} se anula con void_sales_invoice_registered (registro Verifactu)"
        )))
    }

    /// Anula una factura `issued` (nunca una `sent`). `hook` recibe el documento dentro de la
    /// transacción para escribir el registro de anulación Verifactu (D83). Con una rectificativa
    /// en vigor (`issued` o `sent`) la factura no se anula (D82). El mensaje de un
    /// [`SalesStoreError::External`] del gancho no debe contener secretos.
    pub fn void_sales_invoice_with<F>(&mut self, id: i64, hook: F) -> SalesResult<SalesDocument>
    where
        F: FnOnce(&Transaction<'_>, &SalesDocument) -> SalesResult<()>,
    {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let doc = load_document(&tx, id)?;
        can_void(doc.kind, doc.status)?;
        // Un envío por correo sin cerrar bloquea la anulación; uno solo en revisión se cancela
        // (D235). Va dentro de la transacción: si la anulación falla, nada de esto se aplica.
        super::sends::guard_void(&tx, id)?;
        let in_force: i64 = tx.query_row(
            "SELECT count(*) FROM sales_documents WHERE rectifies_id = ?1              AND status IN ('issued', 'sent')",
            [id],
            |r| r.get(0),
        )?;
        if in_force > 0 {
            return Err(SalesStoreError::Conflict(format!(
                "la factura {id} tiene una rectificativa en vigor y no se anula (D82)"
            )));
        }
        touch_status(&tx, id, doc.status, Status::Voided)?;
        let voided = load_document(&tx, id)?;
        hook(&tx, &voided)?;
        audit(&tx, "sales.void", &number_detail(&voided))?;
        tx.commit()?;
        Ok(voided)
    }

    // --- Rectificativas (D81) ---

    /// Borrador de rectificativa (serie `FR`) de una factura emitida o enviada. Por sustitución
    /// parte de las líneas de la original; por diferencias empieza sin líneas.
    pub fn create_corrective_draft(
        &mut self,
        original_id: i64,
        kind: RectificationKind,
        method: RectificationMethod,
    ) -> SalesResult<i64> {
        let tx = self.conn.transaction()?;
        let original = load_document(&tx, original_id)?;
        can_correct(original.kind, original.status)?;
        if original.number.is_none() {
            return Err(SalesStoreError::Conflict(format!(
                "la factura {original_id} no tiene número"
            )));
        }
        check_rectification_kind(&original, kind)?;
        ensure_corrective_compatible(&tx, original_id, method, None)?;
        let mut draft = NewDraft::new(DocumentKind::CorrectiveInvoice);
        draft.customer_id = original.customer_id;
        draft.rectifies_id = Some(original_id);
        draft.art_4_2_declared = original.art_4_2_declared;
        draft.recipient_requested_tax_id = original.recipient_requested_tax_id;
        let (rectified_base, rectified_tax) = rectified_amounts(&original, method)?;
        if method == RectificationMethod::Substitution {
            draft.lines = copy_lines(&original);
            draft.operation_date = original.operation_date;
            draft.withholding = original.withholding_percent;
        }
        draft.rectification = Some(RectificationDraft {
            kind,
            method,
            rectified_base,
            rectified_tax,
        });
        let new_id = insert_draft(&tx, &draft)?;
        link(&tx, new_id, original_id, "rectifies")?;
        audit(
            &tx,
            "sales.corrective_draft",
            &json!({ "id": new_id, "rectifies": original_id, "number": original.number }),
        )?;
        tx.commit()?;
        Ok(new_id)
    }

    // --- Conversión (D74) ---

    /// Convierte presupuestos aceptados o albaranes entregados en un borrador de `to_kind`,
    /// vincula origen y destino y deja los orígenes en `converted` o `invoiced`.
    pub fn convert_sales_documents(
        &mut self,
        ids: &[i64],
        to_kind: DocumentKind,
    ) -> SalesResult<i64> {
        let tx = self.conn.transaction()?;
        let mut sources = Vec::with_capacity(ids.len());
        for id in ids {
            if sources.iter().any(|s: &SalesDocument| s.id == *id) {
                return Err(SalesStoreError::invalid(
                    "ids",
                    format!("el documento {id} está repetido"),
                ));
            }
            sources.push(load_document(&tx, *id)?);
        }
        if let Some(superseded) = sources.iter().find(|s| s.superseded_by.is_some()) {
            return Err(SalesStoreError::Conflict(format!(
                "el documento {} está sustituido por una revisión posterior",
                superseded.id
            )));
        }
        for s in &sources {
            if has_pending_revision(&tx, s)? {
                return Err(SalesStoreError::Conflict(format!(
                    "el documento {} tiene un borrador de revisión pendiente",
                    s.id
                )));
            }
        }
        let mut checks = Vec::with_capacity(sources.len());
        for s in &sources {
            let tax_id = match s.customer_id {
                Some(c) => clean_tax_id(&load_customer(&tx, c)?.data.tax_id),
                None => None,
            };
            checks.push(nestadmin_sales::ConversionSource {
                kind: s.kind,
                status: s.status,
                customer_tax_id: tax_id,
            });
        }
        can_convert(&checks, to_kind)?;
        let first = &sources[0];
        if sources
            .iter()
            .any(|s| s.withholding_percent != first.withholding_percent)
        {
            return Err(SalesStoreError::invalid(
                "withholding_percent",
                "los documentos tienen retenciones distintas",
            ));
        }
        let mut draft = NewDraft::new(to_kind);
        draft.customer_id = first.customer_id;
        draft.withholding = first.withholding_percent;
        draft.lines = sources.iter().flat_map(copy_lines).collect();
        let new_id = insert_draft(&tx, &draft)?;
        for source in &sources {
            link(&tx, source.id, new_id, "converted")?;
            if let Some(after) = status_after_conversion(source.kind) {
                transition_after_conversion(&tx, source, after)?;
            }
        }
        audit(
            &tx,
            "sales.convert",
            &json!({
                "from": sources.iter().map(|s| s.id).collect::<Vec<_>>(),
                "to": new_id, "kind": to_kind.as_str()
            }),
        )?;
        tx.commit()?;
        Ok(new_id)
    }

    /// Documentos vinculados a `id` (origen o destino) con su relación.
    pub fn sales_links(&self, id: i64) -> SalesResult<Vec<(i64, i64, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT from_id, to_id, relation FROM document_links \
             WHERE from_id = ?1 OR to_id = ?1 ORDER BY id",
        )?;
        let rows = stmt.query_map([id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
}

fn transition_after_conversion(
    conn: &Connection,
    source: &SalesDocument,
    after: Status,
) -> SalesResult<()> {
    transition(source.kind, source.status, after)?;
    touch_status(conn, source.id, source.status, after)
}

fn copy_lines(doc: &SalesDocument) -> Vec<DraftLine> {
    doc.lines
        .iter()
        .map(|l| DraftLine {
            input: l.input.clone(),
            product_id: l.product_id,
        })
        .collect()
}

fn link(conn: &Connection, from: i64, to: i64, relation: &str) -> SalesResult<()> {
    conn.execute(
        "INSERT INTO document_links (from_id, to_id, relation, created_at) VALUES (?1, ?2, ?3, ?4)",
        params![from, to, relation, now_ms()],
    )?;
    Ok(())
}

fn apply_input(draft: &mut NewDraft, input: &DraftInput) {
    draft.customer_id = input.customer_id;
    draft.issue_date = input.issue_date;
    draft.operation_date = input.operation_date;
    draft.due_date = input.due_date;
    draft.withholding = input.withholding_percent;
    draft.recipient_requested_tax_id = input.recipient_requested_tax_id;
    draft.art_4_2_declared = input.art_4_2_declared;
    draft.lines = input.lines.clone();
}
