//! Comandos de la ventana para la facturación de venta (fase 11, E3a). Finos: cada uno valida
//! y traduce los datos de la ventana y delega en `nestadmin_app::sales`; no hay lógica de negocio.
//!
//! Formato en la frontera con la ventana:
//! * Fechas `AAAA-MM-DD` e importes, cantidades y porcentajes como cadenas decimales con punto
//!   (`"21"`, `"1234.50"`), para no perder precisión en JavaScript.
//! * Tipos, estados y calificaciones como texto estable: `quote`, `delivery_note`, `invoice`,
//!   `simplified_invoice`, `corrective_invoice`; `draft`, `issued`, `sent`, `accepted`, `rejected`,
//!   `expired`, `converted`, `delivered`, `invoiced`, `voided`; `R1`..`R5`; `substitution` o
//!   `differences`; `transfer`, `cash`, `card`, `other`; `monthly`, `quarterly`, `annual`.
//! * Los argumentos de nivel superior son de una sola palabra (`id`, `to`, `date`...), de modo
//!   que no hay diferencia entre `snake_case` y `camelCase`. Los campos de los objetos van en
//!   `snake_case`.
//!
//! Errores: todos los comandos de venta devuelven `Err(String)` cuyo texto EMPIEZA por el código
//! estable entre corchetes, p. ej. `[verifactu_not_configured] falta configurar...`. La ventana
//! reacciona al código (`verifactu_not_configured`, `verifactu_chain_broken`,
//! `verifactu_clock_behind`, `invalid_input`, `conflict`, `not_found`, `storage`...) y muestra el
//! resto como texto, sin interpretarlo.
//!
//! Envío de documentos por correo (E4b, D232–D241): `sales_prepare_send`, `sales_edit_send`,
//! `sales_approve_send`, `sales_confirm_send_not_sent`, `sales_confirm_send_not_sent_unverified`, `sales_reject_send`, `sales_send_task`,
//! `sales_document_sends` y `sales_send_accounts`. Aprobar, confirmar y descartar tienen comando
//! propio (y no los de las tareas de correo) para que sus errores lleguen con código
//! (`recipient_missing`, `stale_version`, `send_in_progress`...); hacen lo mismo que ellos.

use std::str::FromStr;
use std::sync::MutexGuard;

use nestadmin_app::accounts::AccountView;
use nestadmin_app::remission::{PumpReport, PumpStart, VerifactuStatusView};
use nestadmin_app::{App, AppError, AppResult};
use nestadmin_core::sales::{
    ChainStatus, Company, Customer, CustomerData, DraftInput, DraftLine, IssuerSnapshot,
    LetterheadMargins, ReceivedDeliveryNote, RectificationDraft, RecurringRecord, RecurringRun,
    SalesDocument, SalesFilter, SalesLineRecord, SalesSend, VerifactuEnvironment, VerifactuKind,
    VerifactuRecordRow,
};
use nestadmin_core::Task;
use nestadmin_documents::IsoDate;
use nestadmin_sales::{
    CollectionSummary, DocumentKind, Exemption, Frequency, IssueMode, LineInput, Party, Payment,
    PaymentMethod, Product, ReceivedStatus, RectificationKind, RectificationMethod, Status,
    TaxQualification, TaxRate, Totals,
};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use tauri::ipc::{Invoke, Response};
use tauri::{AppHandle, Runtime, State};

use crate::productor::{self, ProducerState};
use crate::SharedState;

type CmdResult<T> = Result<T, String>;

/// `[codigo] mensaje`: ver la convención en la documentación del módulo.
fn coded(code: &str, message: impl std::fmt::Display) -> String {
    format!("[{code}] {message}")
}

fn sale<T>(result: AppResult<T>) -> CmdResult<T> {
    result.map_err(|e: AppError| coded(e.code(), &e))
}

fn bad(field: &str, why: impl std::fmt::Display) -> String {
    coded("invalid_input", format!("datos no válidos: {field}: {why}"))
}

fn lock<'a>(shared: &'a State<'_, SharedState>) -> CmdResult<MutexGuard<'a, App>> {
    shared.app().map_err(|e| coded("storage", e))
}

// ---- Traducción de la ventana a los tipos de la venta ----

fn date(field: &'static str, text: &str) -> CmdResult<IsoDate> {
    IsoDate::parse(field, text).map_err(|e| bad(field, e))
}

fn opt_date(field: &'static str, text: &Option<String>) -> CmdResult<Option<IsoDate>> {
    match text.as_deref().map(str::trim) {
        None | Some("") => Ok(None),
        Some(t) => date(field, t).map(Some),
    }
}

fn decimal(field: &str, text: &str) -> CmdResult<Decimal> {
    Decimal::from_str(text.trim()).map_err(|_| bad(field, "importe o porcentaje no válido"))
}

fn opt_decimal(field: &str, text: &Option<String>) -> CmdResult<Option<Decimal>> {
    match text.as_deref().map(str::trim) {
        None | Some("") => Ok(None),
        Some(t) => decimal(field, t).map(Some),
    }
}

fn payment_method(text: &str) -> CmdResult<PaymentMethod> {
    PaymentMethod::parse(text).ok_or_else(|| bad("method", "medio de cobro no admitido"))
}

fn frequency(text: &str) -> CmdResult<Frequency> {
    Frequency::parse(text).ok_or_else(|| bad("frequency", "periodicidad no admitida"))
}

fn tax_rate(text: &str) -> CmdResult<TaxRate> {
    TaxRate::from_percent(decimal("tax_percent", text)?).map_err(|e| bad("tax_percent", e))
}

fn show(value: Decimal) -> String {
    value.to_string()
}

fn show_date(value: IsoDate) -> String {
    value.to_string()
}

// ---- Líneas, productos y empresa ----

#[derive(Debug, Deserialize)]
pub struct LineIn {
    pub description: String,
    pub quantity: String,
    pub unit_price: String,
    /// Fracción entre 0 y 1; por defecto `"0"`.
    #[serde(default)]
    pub discount: Option<String>,
    /// Tipo de IVA en porcentaje: `"0"`, `"4"`, `"10"` o `"21"`.
    pub tax_percent: String,
    /// Causa de exención, no sujeción o inversión (`{qualification, description}`); obligatoria
    /// con 0 %.
    #[serde(default)]
    pub exemption: Option<Exemption>,
    /// Producto del que se copió la línea, si lo hay.
    #[serde(default)]
    pub product_id: Option<i64>,
}

impl LineIn {
    fn input(&self) -> CmdResult<LineInput> {
        Ok(LineInput {
            description: self.description.clone(),
            quantity: decimal("quantity", &self.quantity)?,
            unit_price: decimal("unit_price", &self.unit_price)?,
            discount: opt_decimal("discount", &self.discount)?.unwrap_or(Decimal::ZERO),
            tax_rate: tax_rate(&self.tax_percent)?,
            exemption: self.exemption.clone(),
        })
    }

    fn draft(&self) -> CmdResult<DraftLine> {
        Ok(DraftLine {
            input: self.input()?,
            product_id: self.product_id,
        })
    }
}

#[derive(Debug, Serialize)]
pub struct LineOut {
    pub description: String,
    pub quantity: String,
    pub unit_price: String,
    pub discount: String,
    pub tax_percent: String,
    pub exemption: Option<Exemption>,
    /// Importe fijado al emitir; `None` en un borrador.
    pub amount: Option<String>,
    pub product_id: Option<i64>,
}

fn line_out(input: &LineInput, amount: Option<Decimal>, product_id: Option<i64>) -> LineOut {
    LineOut {
        description: input.description.clone(),
        quantity: show(input.quantity),
        unit_price: show(input.unit_price),
        discount: show(input.discount),
        tax_percent: show(input.tax_rate.percent()),
        exemption: input.exemption.clone(),
        amount: amount.map(show),
        product_id,
    }
}

impl From<&SalesLineRecord> for LineOut {
    fn from(l: &SalesLineRecord) -> Self {
        line_out(&l.input, l.amount, l.product_id)
    }
}

#[derive(Debug, Deserialize)]
pub struct ProductIn {
    pub description: String,
    pub unit_price: String,
    pub tax_percent: String,
    #[serde(default)]
    pub exemption: Option<Exemption>,
    #[serde(default)]
    pub retired: bool,
}

impl ProductIn {
    fn product(&self) -> CmdResult<Product> {
        Ok(Product {
            id: 0,
            description: self.description.clone(),
            unit_price: decimal("unit_price", &self.unit_price)?,
            tax_rate: tax_rate(&self.tax_percent)?,
            exemption: self.exemption.clone(),
            retired: self.retired,
        })
    }
}

#[derive(Debug, Serialize)]
pub struct ProductOut {
    pub id: u64,
    pub description: String,
    pub unit_price: String,
    pub tax_percent: String,
    pub exemption: Option<Exemption>,
    pub retired: bool,
}

impl From<Product> for ProductOut {
    fn from(p: Product) -> Self {
        Self {
            id: p.id,
            description: p.description,
            unit_price: show(p.unit_price),
            tax_percent: show(p.tax_rate.percent()),
            exemption: p.exemption,
            retired: p.retired,
        }
    }
}

/// Datos del emisor sin el logotipo, que tiene sus propios comandos.
#[derive(Debug, Default, Deserialize, Serialize)]
pub struct CompanyData {
    pub name: Option<String>,
    pub tax_id: Option<String>,
    pub address: Option<String>,
    pub email: Option<String>,
    pub iban: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct CompanyOut {
    #[serde(flatten)]
    pub data: CompanyData,
    pub has_logo: bool,
    pub logo_mime: Option<String>,
    /// Membrete PDF vigente (D222); el contenido no viaja a la ventana.
    pub has_letterhead: bool,
    /// Márgenes del área útil en milímetros (los de fábrica si nunca se cambiaron).
    pub letterhead_margins: LetterheadMargins,
}

impl From<Company> for CompanyOut {
    fn from(c: Company) -> Self {
        Self {
            has_logo: c.logo.is_some(),
            logo_mime: c.logo_mime,
            has_letterhead: false,
            letterhead_margins: LetterheadMargins::default(),
            data: CompanyData {
                name: c.name,
                tax_id: c.tax_id,
                address: c.address,
                email: c.email,
                iban: c.iban,
            },
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct ReceivedNoteIn {
    pub supplier_name: String,
    #[serde(default)]
    pub supplier_tax_id: Option<String>,
    pub supplier_number: String,
    pub date: String,
}

#[derive(Debug, Serialize)]
pub struct ReceivedNoteOut {
    pub id: i64,
    pub supplier_name: String,
    pub supplier_tax_id: Option<String>,
    pub supplier_number: String,
    pub date: String,
    pub status: ReceivedStatus,
    pub document_ref: Option<String>,
    pub invoice_ref: Option<String>,
}

impl From<ReceivedDeliveryNote> for ReceivedNoteOut {
    fn from(n: ReceivedDeliveryNote) -> Self {
        Self {
            id: n.id,
            supplier_name: n.supplier_name,
            supplier_tax_id: n.supplier_tax_id,
            supplier_number: n.supplier_number,
            date: show_date(n.date),
            status: n.status,
            document_ref: n.document_ref,
            invoice_ref: n.invoice_ref,
        }
    }
}
// ---- Documentos ----

#[derive(Debug, Deserialize)]
pub struct RectificationIn {
    pub kind: RectificationKind,
    pub method: RectificationMethod,
    #[serde(default)]
    pub rectified_base: Option<String>,
    #[serde(default)]
    pub rectified_tax: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct DraftIn {
    pub kind: DocumentKind,
    #[serde(default)]
    pub customer_id: Option<i64>,
    #[serde(default)]
    pub lines: Vec<LineIn>,
    #[serde(default)]
    pub issue_date: Option<String>,
    #[serde(default)]
    pub operation_date: Option<String>,
    #[serde(default)]
    pub due_date: Option<String>,
    /// Retención en porcentaje.
    #[serde(default)]
    pub withholding_percent: Option<String>,
    #[serde(default)]
    pub recipient_requested_tax_id: bool,
    #[serde(default)]
    pub art_4_2_declared: bool,
    /// Solo al editar una rectificativa.
    #[serde(default)]
    pub rectification: Option<RectificationIn>,
}

impl DraftIn {
    fn input(&self) -> CmdResult<DraftInput> {
        let rectification = self
            .rectification
            .as_ref()
            .map(|r| {
                Ok::<_, String>(RectificationDraft {
                    kind: r.kind,
                    method: r.method,
                    rectified_base: opt_decimal("rectified_base", &r.rectified_base)?,
                    rectified_tax: opt_decimal("rectified_tax", &r.rectified_tax)?,
                })
            })
            .transpose()?;
        Ok(DraftInput {
            kind: self.kind,
            customer_id: self.customer_id,
            lines: self
                .lines
                .iter()
                .map(LineIn::draft)
                .collect::<Result<_, _>>()?,
            issue_date: opt_date("issue_date", &self.issue_date)?,
            operation_date: opt_date("operation_date", &self.operation_date)?,
            due_date: opt_date("due_date", &self.due_date)?,
            withholding_percent: opt_decimal("withholding_percent", &self.withholding_percent)?,
            recipient_requested_tax_id: self.recipient_requested_tax_id,
            art_4_2_declared: self.art_4_2_declared,
            rectification,
        })
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct FilterIn {
    #[serde(default)]
    pub kind: Option<DocumentKind>,
    #[serde(default)]
    pub status: Option<Status>,
    #[serde(default)]
    pub include_superseded: bool,
}

#[derive(Debug, Serialize)]
pub struct BreakdownOut {
    pub tax_percent: String,
    pub qualification: Option<TaxQualification>,
    pub descriptions: Vec<String>,
    pub base: String,
    pub tax: String,
}

#[derive(Debug, Serialize)]
pub struct TotalsOut {
    pub currency: String,
    pub line_amounts: Vec<String>,
    pub breakdown: Vec<BreakdownOut>,
    pub base_total: String,
    pub tax_total: String,
    pub total: String,
    pub withholding: String,
    pub amount_due: String,
}

impl From<&Totals> for TotalsOut {
    fn from(t: &Totals) -> Self {
        Self {
            currency: t.currency.clone(),
            line_amounts: t.line_amounts.iter().copied().map(show).collect(),
            breakdown: t
                .breakdown
                .iter()
                .map(|b| BreakdownOut {
                    tax_percent: show(b.tax_rate.percent()),
                    qualification: b.qualification,
                    descriptions: b.descriptions.clone(),
                    base: show(b.base),
                    tax: show(b.tax),
                })
                .collect(),
            base_total: show(t.base_total),
            tax_total: show(t.tax_total),
            total: show(t.total),
            withholding: show(t.withholding),
            amount_due: show(t.amount_due),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct RectificationOut {
    pub kind: RectificationKind,
    pub method: RectificationMethod,
    pub rectified_base: Option<String>,
    pub rectified_tax: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct DocumentOut {
    pub id: i64,
    pub kind: DocumentKind,
    pub status: Status,
    pub series_year: Option<u16>,
    pub sequence: Option<u32>,
    pub revision: u32,
    /// `F-2026-0001`; `None` en un borrador nuevo.
    pub number: Option<String>,
    pub superseded_by: Option<i64>,
    pub customer_id: Option<i64>,
    pub issue_date: Option<String>,
    pub operation_date: Option<String>,
    pub due_date: Option<String>,
    pub withholding_percent: Option<String>,
    pub currency: String,
    /// Copia del emisor tomada al emitir (con `logo_sha256` para `sales_logo_by_hash`).
    pub issuer: Option<IssuerSnapshot>,
    pub recipient: Option<Party>,
    pub totals: Option<TotalsOut>,
    pub lines: Vec<LineOut>,
    pub rectifies_id: Option<i64>,
    pub rectification: Option<RectificationOut>,
    pub recipient_requested_tax_id: bool,
    pub art_4_2_declared: bool,
    pub recurring_template_id: Option<i64>,
    pub recurring_period: Option<u32>,
    pub created_at: i64,
    pub updated_at: i64,
}

impl From<SalesDocument> for DocumentOut {
    fn from(d: SalesDocument) -> Self {
        Self {
            id: d.id,
            kind: d.kind,
            status: d.status,
            series_year: d.series_year,
            sequence: d.sequence,
            revision: d.revision,
            number: d.number,
            superseded_by: d.superseded_by,
            customer_id: d.customer_id,
            issue_date: d.issue_date.map(show_date),
            operation_date: d.operation_date.map(show_date),
            due_date: d.due_date.map(show_date),
            withholding_percent: d.withholding_percent.map(show),
            currency: d.currency,
            issuer: d.issuer,
            recipient: d.recipient,
            totals: d.totals.as_ref().map(TotalsOut::from),
            lines: d.lines.iter().map(LineOut::from).collect(),
            rectifies_id: d.rectifies_id,
            rectification: d.rectification.map(|r| RectificationOut {
                kind: r.kind,
                method: r.method,
                rectified_base: r.rectified_base.map(show),
                rectified_tax: r.rectified_tax.map(show),
            }),
            recipient_requested_tax_id: d.recipient_requested_tax_id,
            art_4_2_declared: d.art_4_2_declared,
            recurring_template_id: d.recurring_template_id,
            recurring_period: d.recurring_period,
            created_at: d.created_at,
            updated_at: d.updated_at,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct LinkOut {
    pub from_id: i64,
    pub to_id: i64,
    pub relation: String,
}

// ---- Cobros ----

#[derive(Debug, Serialize)]
pub struct CollectionOut {
    /// `pending`, `partial`, `paid`, `overdue`, `to_refund`, `substituted`,
    /// `applied_to_original`, `voided` o `voided_with_payments`.
    pub state: &'static str,
    pub payable: String,
    pub paid: String,
    pub outstanding: String,
}

impl From<CollectionSummary> for CollectionOut {
    fn from(c: CollectionSummary) -> Self {
        Self {
            state: c.state.as_str(),
            payable: show(c.payable),
            paid: show(c.paid),
            outstanding: show(c.outstanding),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct PaymentOut {
    pub id: u64,
    pub date: String,
    pub amount: String,
    pub method: &'static str,
}

impl From<Payment> for PaymentOut {
    fn from(p: Payment) -> Self {
        Self {
            id: p.id,
            date: show_date(p.date),
            amount: show(p.amount),
            method: p.method.as_str(),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct PaymentAdded {
    pub payment_id: i64,
    pub collection: CollectionOut,
}

// ---- Recurrentes ----

#[derive(Debug, Deserialize)]
pub struct RecurringIn {
    pub customer_id: i64,
    pub lines: Vec<LineIn>,
    pub frequency: String,
    pub first_day: String,
    #[serde(default)]
    pub end: Option<String>,
}

type RecurringParts = (Vec<LineInput>, Frequency, IsoDate, Option<IsoDate>);

impl RecurringIn {
    fn parts(&self) -> CmdResult<RecurringParts> {
        Ok((
            self.lines
                .iter()
                .map(LineIn::input)
                .collect::<Result<_, _>>()?,
            frequency(&self.frequency)?,
            date("first_day", &self.first_day)?,
            opt_date("end", &self.end)?,
        ))
    }
}

#[derive(Debug, Serialize)]
pub struct RecurringOut {
    pub id: u64,
    pub customer_id: u64,
    pub lines: Vec<LineOut>,
    pub frequency: &'static str,
    pub first_day: String,
    pub end: Option<String>,
    pub active: bool,
}

impl From<RecurringRecord> for RecurringOut {
    fn from(r: RecurringRecord) -> Self {
        let t = r.template;
        Self {
            id: t.id,
            customer_id: t.customer_id,
            lines: t.lines.iter().map(|l| line_out(l, None, None)).collect(),
            frequency: t.frequency.as_str(),
            first_day: show_date(t.first_day),
            end: t.end.map(show_date),
            active: r.active,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct GeneratedOut {
    pub document_id: i64,
    pub template_id: u64,
    pub period: u32,
    /// `recurring:<plantilla>:<periodo>`, la clave de idempotencia.
    pub key: String,
    /// `requires_review` (siempre en esta etapa) o `auto_issue_by_rule`.
    pub mode: &'static str,
}

#[derive(Debug, Serialize)]
pub struct FailedOut {
    pub template_id: i64,
    pub reason: String,
}

#[derive(Debug, Serialize)]
pub struct RunOut {
    pub created: Vec<GeneratedOut>,
    pub failed: Vec<FailedOut>,
}

impl From<RecurringRun> for RunOut {
    fn from(run: RecurringRun) -> Self {
        Self {
            created: run
                .created
                .into_iter()
                .map(|g| GeneratedOut {
                    document_id: g.document_id,
                    template_id: g.key.template_id,
                    period: g.key.period,
                    key: g.key.to_string(),
                    mode: match g.mode {
                        IssueMode::RequiresReview => "requires_review",
                        IssueMode::AutoIssueByRule => "auto_issue_by_rule",
                    },
                })
                .collect(),
            failed: run
                .failed
                .into_iter()
                .map(|(template_id, reason)| FailedOut {
                    template_id,
                    reason,
                })
                .collect(),
        }
    }
}

// ---- Verifactu ----

#[derive(Debug, Serialize)]
pub struct RecordOut {
    pub id: i64,
    /// `alta` o `anulacion`.
    pub kind: &'static str,
    pub document_id: i64,
    pub issuer_tax_id: String,
    pub invoice_number: String,
    /// `DD-MM-AAAA`, como va en el registro.
    pub issue_date: String,
    pub invoice_type: Option<String>,
    pub total: Option<String>,
    pub tax_total: Option<String>,
    pub previous_id: Option<i64>,
    pub previous_hash: Option<String>,
    pub generated_at: String,
    pub hash: String,
    /// Registro completo tal como se guardó.
    pub record: serde_json::Value,
    pub created_at: i64,
}

impl From<VerifactuRecordRow> for RecordOut {
    fn from(r: VerifactuRecordRow) -> Self {
        Self {
            id: r.id,
            kind: match r.kind {
                VerifactuKind::Alta => "alta",
                VerifactuKind::Anulacion => "anulacion",
            },
            document_id: r.document_id,
            issuer_tax_id: r.issuer_tax_id,
            invoice_number: r.invoice_number,
            issue_date: r.issue_date,
            invoice_type: r.invoice_type,
            total: r.total,
            tax_total: r.tax_total,
            previous_id: r.previous_id,
            previous_hash: r.previous_hash,
            generated_at: r.generated_at,
            hash: r.hash,
            record: serde_json::to_value(&r.record).unwrap_or(serde_json::Value::Null),
            created_at: r.created_at,
        }
    }
}

/// Estado de la cadena de registros: una rotura es un estado, no un error.
#[derive(Debug, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ChainOut {
    Empty,
    Ok { records: usize },
    Broken { at_id: i64, reason: String },
}

impl From<ChainStatus> for ChainOut {
    fn from(c: ChainStatus) -> Self {
        match c {
            ChainStatus::Empty => Self::Empty,
            ChainStatus::Ok { records } => Self::Ok { records },
            ChainStatus::Broken { at_id, reason } => Self::Broken { at_id, reason },
        }
    }
}

#[derive(Debug, Serialize)]
pub struct SetupStatus {
    pub producer_configured: bool,
    pub producer_problem: Option<String>,
    pub chain: ChainOut,
    /// Hoy según el reloj de la venta, `AAAA-MM-DD`.
    pub today: String,
}
fn producer_state(shared: &State<'_, SharedState>) -> CmdResult<ProducerState> {
    shared
        .producer
        .lock()
        .map(|p| p.clone())
        .map_err(|_| coded("storage", "estado interno no disponible"))
}

/// Vuelve a leer `verifactu-productor.json` (p. ej. tras copiarlo con la aplicación abierta).
#[tauri::command]
fn reload_verifactu_producer(shared: State<'_, SharedState>) -> CmdResult<ProducerState> {
    let state = productor::load(&mut *lock(&shared)?, &shared.data_dir);
    if let Some(problem) = &state.problem {
        eprintln!("productor de Verifactu no válido: {problem}");
    }
    *shared
        .producer
        .lock()
        .map_err(|_| coded("storage", "estado interno no disponible"))? = state.clone();
    Ok(state)
}

/// Avisos de la ventana de venta: productor y cadena de registros.
#[tauri::command]
fn sales_setup_status(shared: State<'_, SharedState>) -> CmdResult<SetupStatus> {
    let producer = producer_state(&shared)?;
    let app = lock(&shared)?;
    Ok(SetupStatus {
        producer_configured: producer.configured,
        producer_problem: producer.problem,
        chain: sale(app.verifactu_chain_status())?.into(),
        today: show_date(sale(app.sales_today())?),
    })
}

// ---- Empresa ----

#[tauri::command]
fn sales_company(shared: State<'_, SharedState>) -> CmdResult<Option<CompanyOut>> {
    let app = lock(&shared)?;
    let has_letterhead = sale(app.sales_letterhead())?.is_some();
    let margins = sale(app.sales_letterhead_margins())?;
    Ok(sale(app.sales_company())?.map(|c| {
        let mut out = CompanyOut::from(c);
        out.has_letterhead = has_letterhead;
        out.letterhead_margins = margins;
        out
    }))
}

/// Guarda los datos del emisor conservando su logotipo actual.
#[tauri::command]
fn sales_save_company(shared: State<'_, SharedState>, company: CompanyData) -> CmdResult<()> {
    let mut app = lock(&shared)?;
    let current = sale(app.sales_company())?.unwrap_or_default();
    sale(app.set_sales_company(&Company {
        name: company.name,
        tax_id: company.tax_id,
        address: company.address,
        email: company.email,
        iban: company.iban,
        logo: current.logo,
        logo_mime: current.logo_mime,
    }))
}

/// El logotipo llega como cuerpo binario y su tipo (`image/png` o `image/jpeg`) en la cabecera
/// `x-nestadmin-mime`, como `import_file` con el nombre.
#[tauri::command]
fn sales_set_company_logo(
    shared: State<'_, SharedState>,
    request: tauri::ipc::Request<'_>,
) -> CmdResult<()> {
    let tauri::ipc::InvokeBody::Raw(bytes) = request.body() else {
        return Err(bad("logo", "el logotipo debe enviarse como datos binarios"));
    };
    let mime = request
        .headers()
        .get("x-nestadmin-mime")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| bad("logo_mime", "falta el tipo del logotipo"))?
        .to_owned();
    let mut app = lock(&shared)?;
    let mut company = sale(app.sales_company())?.unwrap_or_default();
    company.logo = Some(bytes.clone());
    company.logo_mime = Some(mime);
    sale(app.set_sales_company(&company))
}

#[tauri::command]
fn sales_remove_company_logo(shared: State<'_, SharedState>) -> CmdResult<()> {
    let mut app = lock(&shared)?;
    let mut company = sale(app.sales_company())?.unwrap_or_default();
    company.logo = None;
    company.logo_mime = None;
    sale(app.set_sales_company(&company))
}

/// Bytes del logotipo actual (PNG o JPEG; se distinguen por su cabecera). `[not_found]` si no hay.
#[tauri::command]
fn sales_company_logo(shared: State<'_, SharedState>) -> CmdResult<Response> {
    sale(lock(&shared)?.sales_company())?
        .and_then(|c| c.logo)
        .map(Response::new)
        .ok_or_else(|| coded("not_found", "la empresa no tiene logotipo"))
}

/// Bytes del logotipo con ese SHA-256 (el `issuer.logo_sha256` de un documento emitido).
#[tauri::command]
fn sales_logo_by_hash(shared: State<'_, SharedState>, sha256: String) -> CmdResult<Response> {
    sale(lock(&shared)?.sales_company_logo(&sha256))?
        .map(|(bytes, _mime)| Response::new(bytes))
        .ok_or_else(|| coded("not_found", "logotipo inexistente"))
}

// ---- Membrete y PDF de los documentos (E4a, D222, D223) ----

/// El PDF del membrete llega como cuerpo binario. Se comprueba con PDFium (una o dos páginas A4
/// verticales, sin contraseña, hasta 5 MB): si no vale, `[invalid_input]` con el motivo; sin
/// PDFium, `[pdf_unavailable]`. Conserva los márgenes actuales.
#[tauri::command]
fn sales_set_letterhead(
    shared: State<'_, SharedState>,
    request: tauri::ipc::Request<'_>,
) -> CmdResult<()> {
    let tauri::ipc::InvokeBody::Raw(bytes) = request.body() else {
        return Err(bad(
            "letterhead",
            "el membrete debe enviarse como datos binarios",
        ));
    };
    sale(lock(&shared)?.set_sales_letterhead(bytes))
}

#[tauri::command]
fn sales_remove_letterhead(shared: State<'_, SharedState>) -> CmdResult<()> {
    sale(lock(&shared)?.remove_sales_letterhead())
}

/// Márgenes del área útil en milímetros enteros (0 a 100, con un área mínima de 100 x 150 mm).
/// Solo afectan a los documentos que se emitan después.
#[tauri::command]
fn sales_set_letterhead_margins(
    shared: State<'_, SharedState>,
    margins: LetterheadMargins,
) -> CmdResult<()> {
    sale(lock(&shared)?.set_sales_letterhead_margins(margins))
}

/// Ruta del PDF de un documento emitido (`documentos\ventas\F-2026-0001.pdf`), que se genera si
/// falta. Un borrador da `[conflict]`.
#[tauri::command]
fn sales_document_pdf(shared: State<'_, SharedState>, id: i64) -> CmdResult<String> {
    let path = sale(lock(&shared)?.sales_document_pdf(id))?;
    Ok(path.to_string_lossy().into_owned())
}

/// Genera el PDF si falta y lo abre con el visor del sistema, como «Abrir original» (D49): la
/// ventana solo envía el id del documento, nunca una ruta.
#[tauri::command]
fn sales_open_document_pdf(shared: State<'_, SharedState>, id: i64) -> CmdResult<()> {
    let path = sale(lock(&shared)?.sales_document_pdf(id))?;
    crate::open_file(&path).map_err(|e| coded("io", e))
}

// ---- Envío de documentos por correo (E4b, D232–D241) ----

/// Prepara el envío de un documento emitido: crea la tarea `send_sales_document` en revisión con
/// el destinatario (el correo del cliente), la cuenta, el asunto, el cuerpo y los metadatos del
/// PDF. No envía nada: hay que aprobarla (R07). Un envío abierto da `[send_in_progress]`.
#[tauri::command]
fn sales_prepare_send<R: Runtime>(
    shared: State<'_, SharedState>,
    handle: AppHandle<R>,
    id: i64,
) -> CmdResult<Task> {
    let task = sale(lock(&shared)?.prepare_sales_send(id))?;
    crate::changed(&handle);
    Ok(task)
}

/// Cambia destinatario, cuenta, asunto y cuerpo de un envío en revisión; deja una versión nueva
/// que hay que aprobar otra vez.
#[tauri::command]
fn sales_edit_send<R: Runtime>(
    shared: State<'_, SharedState>,
    handle: AppHandle<R>,
    id: i64,
    to: String,
    subject: String,
    body: String,
    account: String,
) -> CmdResult<Task> {
    let task = sale(lock(&shared)?.edit_sales_send(id, &to, &subject, &body, &account))?;
    crate::changed(&handle);
    Ok(task)
}

/// Aprueba la versión vista y envía. Devuelve la tarea (completada, fallida o de vuelta en
/// revisión); un error de validación llega como `[codigo] mensaje`.
#[tauri::command]
fn sales_approve_send<R: Runtime>(
    shared: State<'_, SharedState>,
    handle: AppHandle<R>,
    id: i64,
    version: i64,
) -> CmdResult<Task> {
    let task = sale(lock(&shared)?.approve(id, version));
    crate::changed(&handle);
    task
}

/// El usuario confirma que el intento anterior no se envió; prepara una versión nueva (D50).
#[tauri::command]
fn sales_confirm_send_not_sent<R: Runtime>(
    shared: State<'_, SharedState>,
    handle: AppHandle<R>,
    id: i64,
) -> CmdResult<Task> {
    let task = sale(lock(&shared)?.confirm_not_sent(id));
    crate::changed(&handle);
    task
}

/// Salida explícita cuando no se puede comprobar si salió (D246): la cuenta del intento ya no
/// existe, es otro buzón o falta la marca del intento. Lo pide la persona; queda en el registro.
#[tauri::command]
fn sales_confirm_send_not_sent_unverified<R: Runtime>(
    shared: State<'_, SharedState>,
    handle: AppHandle<R>,
    id: i64,
) -> CmdResult<Task> {
    let task = sale(lock(&shared)?.confirm_not_sent_unverified(id));
    crate::changed(&handle);
    task
}

/// Descarta un envío sin intento sin cerrar; con uno pendiente da `[send_in_progress]` (D235).
#[tauri::command]
fn sales_reject_send<R: Runtime>(
    shared: State<'_, SharedState>,
    handle: AppHandle<R>,
    id: i64,
) -> CmdResult<Task> {
    let task = sale(lock(&shared)?.reject(id))?;
    crate::changed(&handle);
    Ok(task)
}

/// La tarea de envío abierta de un documento (en revisión, en curso o fallida), si la hay.
#[tauri::command]
fn sales_send_task(shared: State<'_, SharedState>, id: i64) -> CmdResult<Option<Task>> {
    sale(lock(&shared)?.sales_send_task(id))
}

/// Envíos confirmados de un documento, del más antiguo al más reciente.
#[tauri::command]
fn sales_document_sends(shared: State<'_, SharedState>, id: i64) -> CmdResult<Vec<SalesSend>> {
    sale(lock(&shared)?.sales_document_sends(id))
}

/// Cuentas entre las que se puede elegir para enviar un documento.
#[tauri::command]
fn sales_send_accounts(shared: State<'_, SharedState>) -> CmdResult<Vec<AccountView>> {
    Ok(lock(&shared)?.sales_send_accounts())
}

// ---- Clientes y productos ----

#[tauri::command]
fn sales_customers(
    shared: State<'_, SharedState>,
    retired: Option<bool>,
) -> CmdResult<Vec<Customer>> {
    sale(lock(&shared)?.customers(retired.unwrap_or(false)))
}

#[tauri::command]
fn sales_customer(shared: State<'_, SharedState>, id: i64) -> CmdResult<Customer> {
    sale(lock(&shared)?.customer(id))
}

#[tauri::command]
fn sales_create_customer(shared: State<'_, SharedState>, data: CustomerData) -> CmdResult<i64> {
    sale(lock(&shared)?.create_customer(&data))
}

#[tauri::command]
fn sales_update_customer(
    shared: State<'_, SharedState>,
    id: i64,
    data: CustomerData,
) -> CmdResult<()> {
    sale(lock(&shared)?.update_customer(id, &data))
}

#[tauri::command]
fn sales_retire_customer(shared: State<'_, SharedState>, id: i64) -> CmdResult<()> {
    sale(lock(&shared)?.retire_customer(id))
}

#[tauri::command]
fn sales_products(
    shared: State<'_, SharedState>,
    retired: Option<bool>,
) -> CmdResult<Vec<ProductOut>> {
    Ok(sale(lock(&shared)?.products(retired.unwrap_or(false)))?
        .into_iter()
        .map(ProductOut::from)
        .collect())
}

#[tauri::command]
fn sales_product(shared: State<'_, SharedState>, id: i64) -> CmdResult<ProductOut> {
    Ok(sale(lock(&shared)?.product(id))?.into())
}

#[tauri::command]
fn sales_create_product(shared: State<'_, SharedState>, product: ProductIn) -> CmdResult<i64> {
    let product = product.product()?;
    sale(lock(&shared)?.create_product(&product))
}

#[tauri::command]
fn sales_update_product(
    shared: State<'_, SharedState>,
    id: i64,
    product: ProductIn,
) -> CmdResult<()> {
    let product = product.product()?;
    sale(lock(&shared)?.update_product(id, &product))
}

#[tauri::command]
fn sales_retire_product(shared: State<'_, SharedState>, id: i64) -> CmdResult<()> {
    sale(lock(&shared)?.retire_product(id))
}

// ---- Albaranes recibidos ----

#[tauri::command]
fn sales_received_notes(shared: State<'_, SharedState>) -> CmdResult<Vec<ReceivedNoteOut>> {
    Ok(sale(lock(&shared)?.received_delivery_notes())?
        .into_iter()
        .map(Into::into)
        .collect())
}

#[tauri::command]
fn sales_received_note(shared: State<'_, SharedState>, id: i64) -> CmdResult<ReceivedNoteOut> {
    Ok(sale(lock(&shared)?.received_delivery_note(id))?.into())
}

#[tauri::command]
fn sales_create_received_note(
    shared: State<'_, SharedState>,
    note: ReceivedNoteIn,
) -> CmdResult<i64> {
    let day = date("date", &note.date)?;
    sale(lock(&shared)?.create_received_delivery_note(
        &note.supplier_name,
        note.supplier_tax_id.as_deref(),
        &note.supplier_number,
        day,
    ))
}

/// Marca el albarán recibido como revisado, con sus referencias opcionales.
#[tauri::command]
fn sales_review_received_note(
    shared: State<'_, SharedState>,
    id: i64,
    document: Option<String>,
    invoice: Option<String>,
) -> CmdResult<()> {
    sale(lock(&shared)?.review_received_delivery_note(id, document.as_deref(), invoice.as_deref()))
}

// ---- Borradores y consulta ----

#[tauri::command]
fn sales_create_draft(shared: State<'_, SharedState>, draft: DraftIn) -> CmdResult<i64> {
    let input = draft.input()?;
    sale(lock(&shared)?.create_sales_draft(&input))
}

#[tauri::command]
fn sales_update_draft(shared: State<'_, SharedState>, id: i64, draft: DraftIn) -> CmdResult<()> {
    let input = draft.input()?;
    sale(lock(&shared)?.update_sales_draft(id, &input))
}

#[tauri::command]
fn sales_delete_draft(shared: State<'_, SharedState>, id: i64) -> CmdResult<()> {
    sale(lock(&shared)?.delete_sales_draft(id))
}

/// Añade al borrador `id` una línea copiada del producto `product`.
#[tauri::command]
fn sales_add_line_from_product(
    shared: State<'_, SharedState>,
    id: i64,
    product: i64,
    quantity: String,
    discount: Option<String>,
) -> CmdResult<()> {
    let quantity = decimal("quantity", &quantity)?;
    let discount = opt_decimal("discount", &discount)?.unwrap_or(Decimal::ZERO);
    sale(lock(&shared)?.add_sales_line_from_product(id, product, quantity, discount))
}

#[tauri::command]
fn sales_documents(
    shared: State<'_, SharedState>,
    filter: Option<FilterIn>,
) -> CmdResult<Vec<DocumentOut>> {
    let filter = filter.unwrap_or_default();
    let filter = SalesFilter {
        kind: filter.kind,
        status: filter.status,
        include_superseded: filter.include_superseded,
    };
    Ok(sale(lock(&shared)?.list_sales_documents(&filter))?
        .into_iter()
        .map(Into::into)
        .collect())
}

#[tauri::command]
fn sales_document(shared: State<'_, SharedState>, id: i64) -> CmdResult<DocumentOut> {
    Ok(sale(lock(&shared)?.sales_document(id))?.into())
}

/// Documentos vinculados a `id` (origen o destino) con su relación.
#[tauri::command]
fn sales_links(shared: State<'_, SharedState>, id: i64) -> CmdResult<Vec<LinkOut>> {
    Ok(sale(lock(&shared)?.sales_links(id))?
        .into_iter()
        .map(|(from_id, to_id, relation)| LinkOut {
            from_id,
            to_id,
            relation,
        })
        .collect())
}

// ---- Emisión, anulación y estados ----

/// Emitir es la acción explícita del usuario. Una factura se emite con su registro Verifactu y
/// exige productor (`[verifactu_not_configured]`); `date` por defecto es hoy.
#[tauri::command]
fn sales_issue(
    shared: State<'_, SharedState>,
    id: i64,
    date: Option<String>,
) -> CmdResult<DocumentOut> {
    let day = opt_date("date", &date)?;
    Ok(sale(lock(&shared)?.issue_sales_document(id, day))?.into())
}

/// Anula una factura emitida y genera su registro de anulación.
#[tauri::command]
fn sales_void(shared: State<'_, SharedState>, id: i64) -> CmdResult<DocumentOut> {
    Ok(sale(lock(&shared)?.void_sales_invoice(id))?.into())
}

#[tauri::command]
fn sales_transition(shared: State<'_, SharedState>, id: i64, to: Status) -> CmdResult<DocumentOut> {
    Ok(sale(lock(&shared)?.transition_sales_document(id, to))?.into())
}

/// Crea un borrador de revisión de un documento emitido; devuelve su id.
#[tauri::command]
fn sales_revise(shared: State<'_, SharedState>, id: i64) -> CmdResult<i64> {
    sale(lock(&shared)?.revise_sales_document(id))
}

/// Convierte uno o varios documentos en un borrador del tipo `to`; devuelve su id.
#[tauri::command]
fn sales_convert(
    shared: State<'_, SharedState>,
    ids: Vec<i64>,
    to: DocumentKind,
) -> CmdResult<i64> {
    sale(lock(&shared)?.convert_sales_documents(&ids, to))
}

/// Borrador de factura rectificativa de la factura `id`; devuelve su id.
#[tauri::command]
fn sales_create_corrective(
    shared: State<'_, SharedState>,
    id: i64,
    kind: RectificationKind,
    method: RectificationMethod,
) -> CmdResult<i64> {
    sale(lock(&shared)?.create_corrective_draft(id, kind, method))
}

// ---- Cobros ----

#[tauri::command]
fn sales_add_payment(
    shared: State<'_, SharedState>,
    id: i64,
    date: String,
    amount: String,
    method: String,
) -> CmdResult<PaymentAdded> {
    let day = self::date("date", &date)?;
    let amount = decimal("amount", &amount)?;
    let method = payment_method(&method)?;
    let (payment_id, collection) = sale(lock(&shared)?.add_sales_payment(id, day, amount, method))?;
    Ok(PaymentAdded {
        payment_id,
        collection: collection.into(),
    })
}

/// Quita el cobro `id` (identificador del cobro, no del documento).
#[tauri::command]
fn sales_remove_payment(shared: State<'_, SharedState>, id: i64) -> CmdResult<CollectionOut> {
    Ok(sale(lock(&shared)?.remove_sales_payment(id))?.into())
}

#[tauri::command]
fn sales_payments(shared: State<'_, SharedState>, id: i64) -> CmdResult<Vec<PaymentOut>> {
    Ok(sale(lock(&shared)?.sales_payments(id))?
        .into_iter()
        .map(Into::into)
        .collect())
}

#[tauri::command]
fn sales_collection(shared: State<'_, SharedState>, id: i64) -> CmdResult<CollectionOut> {
    Ok(sale(lock(&shared)?.sales_collection(id))?.into())
}

// ---- Recurrentes ----

#[tauri::command]
fn sales_recurring_templates(shared: State<'_, SharedState>) -> CmdResult<Vec<RecurringOut>> {
    Ok(sale(lock(&shared)?.recurring_templates())?
        .into_iter()
        .map(Into::into)
        .collect())
}

#[tauri::command]
fn sales_recurring_template(shared: State<'_, SharedState>, id: i64) -> CmdResult<RecurringOut> {
    Ok(sale(lock(&shared)?.recurring_template(id))?.into())
}

#[tauri::command]
fn sales_create_recurring(shared: State<'_, SharedState>, template: RecurringIn) -> CmdResult<i64> {
    let (lines, frequency, first_day, end) = template.parts()?;
    sale(lock(&shared)?.create_recurring_template(
        template.customer_id,
        &lines,
        frequency,
        first_day,
        end,
    ))
}

#[tauri::command]
fn sales_update_recurring(
    shared: State<'_, SharedState>,
    id: i64,
    template: RecurringIn,
) -> CmdResult<()> {
    let (lines, frequency, first_day, end) = template.parts()?;
    sale(lock(&shared)?.update_recurring_template(
        id,
        template.customer_id,
        &lines,
        frequency,
        first_day,
        end,
    ))
}

#[tauri::command]
fn sales_set_recurring_active(
    shared: State<'_, SharedState>,
    id: i64,
    active: bool,
) -> CmdResult<()> {
    sale(lock(&shared)?.set_recurring_active(id, active))
}

/// Genera los borradores vencidos hasta hoy. Solo borradores: ninguno se emite.
#[tauri::command]
fn sales_generate_recurring(shared: State<'_, SharedState>) -> CmdResult<RunOut> {
    Ok(sale(lock(&shared)?.generate_recurring_drafts())?.into())
}

// ---- Verifactu ----

#[tauri::command]
fn verifactu_records(shared: State<'_, SharedState>) -> CmdResult<Vec<RecordOut>> {
    Ok(sale(lock(&shared)?.verifactu_records())?
        .into_iter()
        .map(Into::into)
        .collect())
}

/// Estado de la remisión (E5, D249-D252, D256): entorno, estado de cada registro, último lote,
/// espera, paro por `Fault` y pendientes. Las marcas de tiempo van en milisegundos Unix.
#[tauri::command]
fn verifactu_status(shared: State<'_, SharedState>) -> CmdResult<VerifactuStatusView> {
    sale(lock(&shared)?.verifactu_status())
}

/// «Remitir ahora»: una pasada manual de la remisión (D250). La llamada de red se hace sin el
/// cerrojo de la aplicación y fuera del hilo de la ventana, igual que el tick (`resume_verifactu`).
#[tauri::command]
async fn verifactu_submit_now(shared: State<'_, SharedState>) -> CmdResult<PumpReport> {
    let worker = std::sync::Arc::clone(shared.inner());
    tauri::async_runtime::spawn_blocking(move || {
        let start = worker
            .app()
            .map_err(|e| coded("storage", e))
            .and_then(|mut app| sale(app.verifactu_pump_begin(true)))?;
        match start {
            PumpStart::Idle(report) => Ok(report),
            PumpStart::Job(job) => {
                let sent = job.run();
                let mut app = worker.app().map_err(|e| coded("storage", e))?;
                sale(app.verifactu_pump_finish(sent))
            }
        }
    })
    .await
    .map_err(|e| coded("storage", e))?
}

#[derive(Debug, Serialize)]
pub struct SubsanationOut {
    pub rejected_record_id: i64,
    /// Registro nuevo de subsanación.
    pub record_id: i64,
    /// `false` si ya existía (doble clic = un solo registro).
    pub created: bool,
}

/// Subsana un registro rechazado: crea un registro nuevo vinculado y pendiente de remitir (D253).
#[tauri::command]
fn verifactu_subsanate(
    shared: State<'_, SharedState>,
    record_id: i64,
) -> CmdResult<SubsanationOut> {
    let done = sale(lock(&shared)?.verifactu_subsanate(record_id))?;
    Ok(SubsanationOut {
        rejected_record_id: done.rejected_record_id,
        record_id: done.record_id,
        created: done.created,
    })
}

/// Entorno de remisión vigente: `simulador`, `preproduccion` o `produccion`.
#[tauri::command]
fn verifactu_environment(shared: State<'_, SharedState>) -> CmdResult<String> {
    Ok(sale(lock(&shared)?.verifactu_environment())?
        .as_str()
        .to_owned())
}

#[derive(Debug, Serialize)]
pub struct EnvironmentChangeOut {
    pub environment: String,
    /// `false` si ya era el vigente.
    pub changed: bool,
}

/// Cambia el entorno de remisión. Hoy solo `simulador` se puede elegir (D247): los demás dan
/// `[environment_unavailable]` (o `[verifactu_environment_busy]` si hay registros por remitir).
#[tauri::command]
fn verifactu_set_environment(
    shared: State<'_, SharedState>,
    environment: String,
) -> CmdResult<EnvironmentChangeOut> {
    let target = VerifactuEnvironment::parse(environment.trim())
        .ok_or_else(|| bad("environment", "entorno no admitido"))?;
    let changed = sale(lock(&shared)?.set_verifactu_environment(target, "usuario_configuracion"))?;
    Ok(EnvironmentChangeOut {
        environment: target.as_str().to_owned(),
        changed,
    })
}

/// `configure` reparte los comandos por este criterio (prefijo o nombre) entre la lista
/// principal y la de venta. Así la lista principal, que contrasta `ui/src/api.ts` uno a uno, no
/// incluye los de venta hasta que la ventana de venta los llame.
pub fn is_sales_command(name: &str) -> bool {
    name.starts_with("sales_")
        || name.starts_with("verifactu_")
        || name == "reload_verifactu_producer"
}

/// Fija el tipo del manejador generado por `generate_handler!` para `R`.
pub fn typed<R: Runtime, F: Fn(Invoke<R>) -> bool + Send + Sync + 'static>(f: F) -> F {
    f
}

pub fn handler<R: Runtime>() -> impl Fn(Invoke<R>) -> bool + Send + Sync + 'static {
    tauri::generate_handler![
        reload_verifactu_producer,
        sales_setup_status,
        sales_company,
        sales_save_company,
        sales_set_company_logo,
        sales_remove_company_logo,
        sales_company_logo,
        sales_logo_by_hash,
        sales_set_letterhead,
        sales_remove_letterhead,
        sales_set_letterhead_margins,
        sales_document_pdf,
        sales_open_document_pdf,
        sales_prepare_send,
        sales_edit_send,
        sales_approve_send,
        sales_confirm_send_not_sent,
        sales_confirm_send_not_sent_unverified,
        sales_reject_send,
        sales_send_task,
        sales_document_sends,
        sales_send_accounts,
        sales_customers,
        sales_customer,
        sales_create_customer,
        sales_update_customer,
        sales_retire_customer,
        sales_products,
        sales_product,
        sales_create_product,
        sales_update_product,
        sales_retire_product,
        sales_received_notes,
        sales_received_note,
        sales_create_received_note,
        sales_review_received_note,
        sales_create_draft,
        sales_update_draft,
        sales_delete_draft,
        sales_add_line_from_product,
        sales_documents,
        sales_document,
        sales_links,
        sales_issue,
        sales_void,
        sales_transition,
        sales_revise,
        sales_convert,
        sales_create_corrective,
        sales_add_payment,
        sales_remove_payment,
        sales_payments,
        sales_collection,
        sales_recurring_templates,
        sales_recurring_template,
        sales_create_recurring,
        sales_update_recurring,
        sales_set_recurring_active,
        sales_generate_recurring,
        verifactu_records,
        verifactu_status,
        verifactu_submit_now,
        verifactu_subsanate,
        verifactu_environment,
        verifactu_set_environment,
    ]
}
