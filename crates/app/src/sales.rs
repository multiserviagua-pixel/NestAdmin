//! Facturación de venta en la capa de aplicación (fase 11, E2). Delgada: cada operación delega en
//! `nestadmin_core::sales` (que ya escribe su historial) y añade solo lo que core no tiene: la
//! fecha de hoy según un reloj inyectable, la identidad del sistema informático Verifactu
//! (productor, versión, número de instalación) y la elección entre emitir con o sin registro.
//!
//! * Una FACTURA se emite y se anula con su registro Verifactu (misma transacción). Sin productor
//!   configurado (`set_verifactu_producer`) falla con `verifactu_not_configured` antes de tocar
//!   nada: la factura sigue en borrador y la serie no consume número.
//! * Un presupuesto o un albarán se emite sin registro y no exige productor.
//! * Nada se remite a la AEAT: solo se dejan los registros listos y verificables.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use nestadmin_core::actions::idempotency_key;
use nestadmin_core::sales::{
    is_sendable, ChainStatus, Company, Customer, CustomerData, DraftInput, Letterhead,
    LetterheadMargins, NewSalesSend, ReceivedDeliveryNote, RecurringRecord, RecurringRun,
    SalesDocument, SalesFilter, SalesSend, SalesStoreError, VerifactuContext, VerifactuRecordRow,
    SEND_SALES_DOCUMENT,
};
use nestadmin_core::{
    ActionRequest, Authorization, Connector, ConnectorError, CoreError, Rule, Task,
};
use nestadmin_documents::IsoDate;
use nestadmin_sales::{
    validate_tax_id, CollectionSummary, DocumentKind, Frequency, LineInput, Payment, PaymentMethod,
    Product, RectificationKind, RectificationMethod, Status,
};
use nestadmin_simulators::{NewDraft, OutgoingAttachment, MAX_OUTGOING_ATTACHMENT_BYTES};
use nestadmin_verifactu::tipos::validar_nif;
use nestadmin_verifactu::{
    CodigoQr, Entorno, FechaHoraHuso, Persona, RegistroFacturacion, SistemaInformatico,
};
use rust_decimal::Decimal;
use serde_json::{json, Value};

use crate::accounts::{split_id, AccountStatus, AccountView, Accounts, MailProviderKind};
use crate::rules::{evaluate_with, is_email_address, RuleDecision};
use crate::{sales_pdf, task_error, App, AppError, AppResult, Previous};

/// Código del error al emitir o anular una factura sin productor de software configurado.
pub const CODE_NOT_CONFIGURED: &str = "verifactu_not_configured";

/// Nombre, identificador y versión del sistema informático (SIF) que consta en cada registro.
pub const SIF_NAME: &str = "NestAdmin";
pub const SIF_ID: &str = "NA";
pub const SIF_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Reloj de la facturación de venta. En producción es [`LocalClock`]; las pruebas lo sustituyen
/// con `App::set_sales_clock`.
pub trait SalesClock: Send + Sync {
    /// Instante actual con el huso horario local.
    fn now(&self) -> FechaHoraHuso;

    /// Como `now()`, pero con error explícito si la hora no se puede representar. La aplicación
    /// usa siempre esta; por defecto delega en `now()` (los relojes de prueba no fallan).
    fn try_now(&self) -> Result<FechaHoraHuso, String> {
        Ok(self.now())
    }

    /// Fecha de hoy según `try_now()`.
    fn try_today(&self) -> Result<IsoDate, String> {
        let date = self.try_now()?.fecha();
        Ok(IsoDate {
            year: date.anio(),
            month: date.mes(),
            day: date.dia(),
        })
    }
}

/// Hora local de Windows con su huso (`AAAA-MM-DDThh:mm:ss±hh:mm`), vía `chrono::Local`.
#[derive(Debug, Clone, Copy, Default)]
pub struct LocalClock;

impl SalesClock for LocalClock {
    /// Solo para el trait: la aplicación llama a `try_now`. Falla si la hora no es representable.
    fn now(&self) -> FechaHoraHuso {
        self.try_now()
            .expect("la hora local no es representable; use try_now")
    }

    fn try_now(&self) -> Result<FechaHoraHuso, String> {
        let local = chrono::Local::now();
        let offset_minutes = (local.offset().local_minus_utc() / 60) as i16;
        FechaHoraHuso::desde_unix(local.timestamp(), offset_minutes)
            .map_err(|e| format!("la hora local no se puede representar: {e}"))
    }
}

/// Código del error cuando el reloj no da una hora representable (no se inventa una).
pub const CODE_CLOCK_UNAVAILABLE: &str = "clock_unavailable";

fn clock_error(why: String) -> AppError {
    AppError::Sales(SalesStoreError::External {
        code: CODE_CLOCK_UNAVAILABLE.to_owned(),
        message: why,
    })
}

/// Códigos de los errores del PDF: PDFium no está disponible o la generación falló.
pub const CODE_PDF_UNAVAILABLE: &str = "pdf_unavailable";
pub const CODE_PDF_FAILED: &str = "pdf_failed";

fn pdf_unavailable(e: nestadmin_extract::ExtractError) -> AppError {
    AppError::Sales(SalesStoreError::External {
        code: CODE_PDF_UNAVAILABLE.to_owned(),
        message: e.to_string(),
    })
}

fn not_configured() -> AppError {
    AppError::Sales(SalesStoreError::External {
        code: CODE_NOT_CONFIGURED.to_owned(),
        message: "falta configurar el productor del software (nombre y NIF) para emitir o \
                  anular facturas con registro Verifactu"
            .to_owned(),
    })
}

impl App {
    /// Sustituye el reloj de la venta (pruebas).
    pub fn set_sales_clock(&mut self, clock: Arc<dyn SalesClock>) {
        self.sales_clock = clock;
    }

    /// Fecha de hoy según el reloj de la venta.
    pub fn sales_today(&self) -> AppResult<IsoDate> {
        self.sales_clock.try_today().map_err(clock_error)
    }

    /// Fija el productor del software (nombre y NIF del editor) que consta en los registros
    /// Verifactu. Lo llama quien arranca la aplicación; NestAdmin no lo inventa.
    pub fn set_verifactu_producer(&mut self, name: &str, tax_id: &str) -> AppResult<()> {
        let (name, tax_id) = (name.trim(), tax_id.trim());
        if name.is_empty() || name.chars().count() > 120 {
            return Err(SalesStoreError::invalid(
                "producer.name",
                "obligatorio, hasta 120 caracteres",
            )
            .into());
        }
        // Mayúsculas y sin espacios ni guiones, y validado con el mismo control de NIF que usa la
        // venta (D214): el productor no entra en la cadena con un NIF que la AEAT rechazaría.
        let tax_id: String = tax_id
            .chars()
            .filter(|c| !c.is_whitespace() && *c != '-')
            .collect::<String>()
            .to_uppercase();
        validate_tax_id(&tax_id)
            .map_err(|e| SalesStoreError::invalid("producer.tax_id", e.to_string()))?;
        validar_nif(&tax_id, "producer.tax_id")
            .map_err(|e| SalesStoreError::invalid("producer.tax_id", e.to_string()))?;
        self.verifactu_producer = Some(Persona::con_nif(name, &tax_id));
        Ok(())
    }

    /// Quita el productor (el archivo local desapareció o dejó de ser válido): emitir una
    /// factura vuelve a fallar con `verifactu_not_configured`.
    pub fn clear_verifactu_producer(&mut self) {
        self.verifactu_producer = None;
    }

    /// Si hay un productor configurado.
    pub fn has_verifactu_producer(&self) -> bool {
        self.verifactu_producer.is_some()
    }

    /// Contexto Verifactu para registrar ahora: productor configurado, número de instalación
    /// (se genera la primera vez y no cambia) y hora del reloj.
    fn verifactu_context(&mut self) -> AppResult<VerifactuContext> {
        let producer = self.verifactu_producer.clone().ok_or_else(not_configured)?;
        let seed = self.documents.to_string_lossy().into_owned();
        let installation = self.store.ensure_installation_number(&seed)?;
        Ok(VerifactuContext {
            system: SistemaInformatico {
                productor: producer,
                nombre_sistema: SIF_NAME.to_owned(),
                id_sistema: SIF_ID.to_owned(),
                version: SIF_VERSION.to_owned(),
                numero_instalacion: installation,
                solo_verifactu: true,
                multi_ot: false,
                indicador_multiples_ot: false,
            },
            now: self.sales_clock.try_now().map_err(clock_error)?,
        })
    }

    // --- Empresa ---

    pub fn sales_company(&self) -> AppResult<Option<Company>> {
        Ok(self.store.sales_company()?)
    }

    /// Guarda los datos del emisor, con su logotipo si lo lleva.
    pub fn set_sales_company(&mut self, company: &Company) -> AppResult<()> {
        Ok(self.store.set_sales_company(company)?)
    }

    /// Logotipo (contenido y tipo MIME) con ese SHA-256.
    pub fn sales_company_logo(&self, sha256: &str) -> AppResult<Option<(Vec<u8>, String)>> {
        Ok(self.store.company_logo_by_hash(sha256)?)
    }

    // --- Membrete (D222, D223) ---

    /// Membrete vigente de la empresa (PDF y márgenes), si lo hay.
    pub fn sales_letterhead(&self) -> AppResult<Option<Letterhead>> {
        Ok(self.store.sales_letterhead()?)
    }

    /// Márgenes guardados del área útil, con o sin membrete.
    pub fn sales_letterhead_margins(&self) -> AppResult<LetterheadMargins> {
        Ok(self.store.sales_letterhead_margins()?)
    }

    /// Membrete (contenido) con ese SHA-256, el `issuer.letterhead_sha256` de un documento.
    pub fn sales_letterhead_by_hash(&self, sha256: &str) -> AppResult<Option<Vec<u8>>> {
        Ok(self.store.letterhead_by_hash(sha256)?)
    }

    /// Guarda un PDF como membrete tras comprobarlo con PDFium: se abre, sin contraseña, de una o
    /// dos páginas A4 verticales y de hasta 5 MB. Conserva los márgenes actuales. Un PDF no
    /// válido da `invalid_input` con el motivo; sin PDFium, `pdf_unavailable`.
    pub fn set_sales_letterhead(&mut self, bytes: &[u8]) -> AppResult<()> {
        let pdfium = self.extractor.pdfium().map_err(pdf_unavailable)?;
        sales_pdf::check_letterhead(pdfium, bytes)
            .map_err(|why| SalesStoreError::invalid("letterhead", why))?;
        let margins = self.store.sales_letterhead_margins()?;
        sales_pdf::check_letterhead_text_area(pdfium, bytes, &margins)
            .map_err(|why| SalesStoreError::invalid("letterhead", why))?;
        self.store.set_sales_letterhead(bytes, margins)?;
        Ok(())
    }

    /// Quita el membrete vigente. Los documentos ya emitidos conservan el suyo.
    pub fn remove_sales_letterhead(&mut self) -> AppResult<()> {
        Ok(self.store.remove_sales_letterhead()?)
    }

    /// Cambia los márgenes del área útil; solo afecta a los documentos que se emitan después.
    /// Con membrete guardado se comprueba que la nueva área útil sigue siendo clara (D228,
    /// `invalid_input` en `letterhead.margins`; sin PDFium, `pdf_unavailable`); sin membrete no
    /// hace falta PDFium. Si se rechaza no se guarda nada.
    pub fn set_sales_letterhead_margins(&mut self, margins: LetterheadMargins) -> AppResult<()> {
        margins.validate()?;
        if let Some(letterhead) = self.store.sales_letterhead()? {
            let pdfium = self.extractor.pdfium().map_err(pdf_unavailable)?;
            sales_pdf::check_letterhead_text_area(pdfium, &letterhead.data, &margins)
                .map_err(|why| SalesStoreError::invalid("letterhead.margins", why))?;
        }
        Ok(self.store.set_sales_letterhead_margins(margins)?)
    }

    // --- PDF del documento emitido (E4a, D76, D223) ---

    /// Ruta del PDF de un documento emitido, que se genera si falta. Se guarda en
    /// `documentos/ventas/<número>.pdf` (`<número>-r<n>.pdf` desde la revisión 2) y se escribe
    /// entero o no se escribe: un archivo existente es siempre un PDF completo. Un borrador no
    /// tiene PDF (`conflict`); una factura sin su registro de alta tampoco, porque el QR sale de
    /// ese registro.
    pub fn sales_document_pdf(&mut self, id: i64) -> AppResult<PathBuf> {
        let doc = self.store.sales_document(id)?;
        let number = match (&doc.number, doc.status) {
            (Some(number), status) if status != Status::Draft => number.clone(),
            _ => {
                return Err(SalesStoreError::Conflict(
                    "solo se genera el PDF de un documento emitido".into(),
                )
                .into())
            }
        };
        let name = pdf_stem(&number, doc.revision);
        let folder = self.documents.join("ventas");
        let path = folder.join(format!("{name}.pdf"));
        if path.metadata().is_ok_and(|m| m.is_file() && m.len() > 0) {
            return Ok(path);
        }
        let bytes = self.render_sales_pdf(&doc)?;
        fs::create_dir_all(&folder)?;
        let part = folder.join(format!("{name}.pdf.part"));
        fs::write(&part, &bytes)?;
        fs::rename(&part, &path)?;
        Ok(path)
    }

    /// Dibuja el PDF de un documento emitido sin guardarlo: lee el logotipo y el membrete que cita
    /// su copia del emisor (no los vigentes) y, en una factura, el QR de su registro de alta.
    pub fn render_sales_pdf(&self, doc: &SalesDocument) -> AppResult<Vec<u8>> {
        let pdfium = self.extractor.pdfium().map_err(pdf_unavailable)?;
        let issuer = doc.issuer.as_ref();
        let logo = match issuer.and_then(|i| i.logo_sha256.as_deref()) {
            Some(hash) => self.store.company_logo_by_hash(hash)?.map(|(b, _)| b),
            None => None,
        };
        let letterhead = match issuer.and_then(|i| i.letterhead_sha256.as_deref()) {
            Some(hash) => Some(self.store.letterhead_by_hash(hash)?.ok_or_else(|| {
                SalesStoreError::Corrupt(format!("el membrete {hash} citado por el documento"))
            })?),
            None => None,
        };
        let rectified_number = match doc.rectifies_id {
            Some(id) => self.store.sales_document(id)?.number,
            None => None,
        };
        let qr = if doc.kind.is_invoice() {
            let alta = self
                .store
                .verifactu_records()?
                .into_iter()
                .find_map(|row| match row.record {
                    RegistroFacturacion::Alta(a) if row.document_id == doc.id => Some(a),
                    _ => None,
                })
                .ok_or_else(|| {
                    SalesStoreError::Conflict(
                        "la factura no tiene registro de alta Verifactu: no se puede generar su \
                         QR"
                        .into(),
                    )
                })?;
            Some(
                CodigoQr::de_registro(Entorno::Produccion, &alta).map_err(|e| {
                    SalesStoreError::External {
                        code: CODE_PDF_FAILED.to_owned(),
                        message: format!("no se pudo construir el QR: {e}"),
                    }
                })?,
            )
        } else {
            None
        };
        sales_pdf::render(
            pdfium,
            &sales_pdf::PdfSource {
                document: doc,
                rectified_number,
                logo,
                letterhead,
                qr,
            },
        )
        .map_err(|e| {
            AppError::Sales(SalesStoreError::External {
                code: CODE_PDF_FAILED.to_owned(),
                message: e.to_string(),
            })
        })
    }

    // --- Clientes y productos ---

    pub fn create_customer(&mut self, data: &CustomerData) -> AppResult<i64> {
        Ok(self.store.create_customer(data)?)
    }
    pub fn update_customer(&mut self, id: i64, data: &CustomerData) -> AppResult<()> {
        Ok(self.store.update_customer(id, data)?)
    }
    pub fn retire_customer(&mut self, id: i64) -> AppResult<()> {
        Ok(self.store.retire_customer(id)?)
    }
    pub fn customer(&self, id: i64) -> AppResult<Customer> {
        Ok(self.store.customer(id)?)
    }
    pub fn customers(&self, include_retired: bool) -> AppResult<Vec<Customer>> {
        Ok(self.store.customers(include_retired)?)
    }

    pub fn create_product(&mut self, product: &Product) -> AppResult<i64> {
        Ok(self.store.create_product(product)?)
    }
    pub fn update_product(&mut self, id: i64, product: &Product) -> AppResult<()> {
        Ok(self.store.update_product(id, product)?)
    }
    pub fn retire_product(&mut self, id: i64) -> AppResult<()> {
        Ok(self.store.retire_product(id)?)
    }
    pub fn product(&self, id: i64) -> AppResult<Product> {
        Ok(self.store.product(id)?)
    }
    pub fn products(&self, include_retired: bool) -> AppResult<Vec<Product>> {
        Ok(self.store.products(include_retired)?)
    }

    // --- Albaranes recibidos ---

    pub fn create_received_delivery_note(
        &mut self,
        supplier_name: &str,
        supplier_tax_id: Option<&str>,
        supplier_number: &str,
        date: IsoDate,
    ) -> AppResult<i64> {
        Ok(self.store.create_received_delivery_note(
            supplier_name,
            supplier_tax_id,
            supplier_number,
            date,
        )?)
    }
    pub fn received_delivery_note(&self, id: i64) -> AppResult<ReceivedDeliveryNote> {
        Ok(self.store.received_delivery_note(id)?)
    }
    pub fn received_delivery_notes(&self) -> AppResult<Vec<ReceivedDeliveryNote>> {
        Ok(self.store.received_delivery_notes()?)
    }
    pub fn review_received_delivery_note(
        &mut self,
        id: i64,
        document_ref: Option<&str>,
        invoice_ref: Option<&str>,
    ) -> AppResult<()> {
        Ok(self
            .store
            .review_received_delivery_note(id, document_ref, invoice_ref)?)
    }

    // --- Borradores y consulta ---

    pub fn create_sales_draft(&mut self, input: &DraftInput) -> AppResult<i64> {
        Ok(self.store.create_sales_draft(input)?)
    }
    pub fn update_sales_draft(&mut self, id: i64, input: &DraftInput) -> AppResult<()> {
        Ok(self.store.update_sales_draft(id, input)?)
    }
    pub fn delete_sales_draft(&mut self, id: i64) -> AppResult<()> {
        Ok(self.store.delete_sales_draft(id)?)
    }
    pub fn add_sales_line_from_product(
        &mut self,
        id: i64,
        product_id: i64,
        quantity: Decimal,
        discount: Decimal,
    ) -> AppResult<()> {
        Ok(self
            .store
            .add_line_from_product(id, product_id, quantity, discount)?)
    }
    pub fn sales_document(&self, id: i64) -> AppResult<SalesDocument> {
        Ok(self.store.sales_document(id)?)
    }
    pub fn list_sales_documents(&self, filter: &SalesFilter) -> AppResult<Vec<SalesDocument>> {
        Ok(self.store.list_sales_documents(filter)?)
    }
    pub fn sales_links(&self, id: i64) -> AppResult<Vec<(i64, i64, String)>> {
        Ok(self.store.sales_links(id)?)
    }

    // --- Emisión, anulación y estados ---

    /// Emite el documento; `issue_date` por defecto es hoy según el reloj, y una fecha posterior
    /// a hoy se rechaza. Una factura se emite con su registro de alta Verifactu (exige
    /// productor configurado); un presupuesto o un albarán, sin registro.
    pub fn issue_sales_document(
        &mut self,
        id: i64,
        issue_date: Option<IsoDate>,
    ) -> AppResult<SalesDocument> {
        let today = self.sales_today()?;
        let issue_date = issue_date.unwrap_or(today);
        let issued = if self.store.sales_document(id)?.kind.is_invoice() {
            let ctx = self.verifactu_context()?;
            self.store
                .issue_sales_invoice_registered(id, issue_date, today, &ctx)?
        } else {
            self.store.issue_sales_document(id, issue_date, today)?
        };
        // La regla por cliente (D238) solo actúa al emitir y nunca puede impedir ni deshacer la
        // emisión: lo que falle queda anotado en el historial.
        self.send_on_issue(&issued);
        Ok(issued)
    }

    /// Anula una factura emitida y genera su registro de anulación Verifactu.
    pub fn void_sales_invoice(&mut self, id: i64) -> AppResult<SalesDocument> {
        let ctx = self.verifactu_context()?;
        Ok(self.store.void_sales_invoice_registered(id, &ctx)?)
    }

    pub fn transition_sales_document(&mut self, id: i64, to: Status) -> AppResult<SalesDocument> {
        Ok(self.store.transition_sales_document(id, to)?)
    }
    pub fn revise_sales_document(&mut self, id: i64) -> AppResult<i64> {
        Ok(self.store.revise_sales_document(id)?)
    }
    pub fn convert_sales_documents(
        &mut self,
        ids: &[i64],
        to_kind: DocumentKind,
    ) -> AppResult<i64> {
        Ok(self.store.convert_sales_documents(ids, to_kind)?)
    }
    pub fn create_corrective_draft(
        &mut self,
        original_id: i64,
        kind: RectificationKind,
        method: RectificationMethod,
    ) -> AppResult<i64> {
        Ok(self
            .store
            .create_corrective_draft(original_id, kind, method)?)
    }

    // --- Cobros ---

    pub fn add_sales_payment(
        &mut self,
        document_id: i64,
        date: IsoDate,
        amount: Decimal,
        method: PaymentMethod,
    ) -> AppResult<(i64, CollectionSummary)> {
        let today = self.sales_today()?;
        Ok(self
            .store
            .add_sales_payment(document_id, date, amount, method, today)?)
    }
    pub fn remove_sales_payment(&mut self, payment_id: i64) -> AppResult<CollectionSummary> {
        let today = self.sales_today()?;
        Ok(self.store.remove_sales_payment(payment_id, today)?)
    }
    pub fn sales_payments(&self, document_id: i64) -> AppResult<Vec<Payment>> {
        Ok(self.store.sales_payments(document_id)?)
    }
    /// Estado de cobro derivado a la fecha de hoy.
    pub fn sales_collection(&self, document_id: i64) -> AppResult<CollectionSummary> {
        Ok(self
            .store
            .sales_collection(document_id, self.sales_today()?)?)
    }

    // --- Recurrentes ---

    pub fn create_recurring_template(
        &mut self,
        customer_id: i64,
        lines: &[LineInput],
        frequency: Frequency,
        first_day: IsoDate,
        end: Option<IsoDate>,
    ) -> AppResult<i64> {
        Ok(self
            .store
            .create_recurring_template(customer_id, lines, frequency, first_day, end)?)
    }
    pub fn update_recurring_template(
        &mut self,
        id: i64,
        customer_id: i64,
        lines: &[LineInput],
        frequency: Frequency,
        first_day: IsoDate,
        end: Option<IsoDate>,
    ) -> AppResult<()> {
        Ok(self.store.update_recurring_template(
            id,
            customer_id,
            lines,
            frequency,
            first_day,
            end,
        )?)
    }
    pub fn set_recurring_active(&mut self, id: i64, active: bool) -> AppResult<()> {
        Ok(self.store.set_recurring_active(id, active)?)
    }
    pub fn recurring_template(&self, id: i64) -> AppResult<RecurringRecord> {
        Ok(self.store.recurring_template(id)?)
    }
    pub fn recurring_templates(&self) -> AppResult<Vec<RecurringRecord>> {
        Ok(self.store.recurring_templates()?)
    }
    /// Genera los borradores vencidos hasta hoy. Solo borradores: sin reglas expresas de
    /// emisión automática en esta etapa, ninguno se emite.
    pub fn generate_recurring_drafts(&mut self) -> AppResult<RecurringRun> {
        let today = self.sales_today()?;
        Ok(self.store.generate_recurring_drafts(today, |_| false)?)
    }

    // --- Verifactu ---

    /// Estado de la cadena de registros guardada (una rotura es un estado, no un error).
    pub fn verifactu_chain_status(&self) -> AppResult<ChainStatus> {
        Ok(self.store.verify_verifactu_chain()?)
    }
    pub fn verifactu_records(&self) -> AppResult<Vec<VerifactuRecordRow>> {
        Ok(self.store.verifactu_records()?)
    }
}

// ---------------------------------------------------------------------------------------------
// Envío de documentos de venta por correo (E4b, D232–D241)
// ---------------------------------------------------------------------------------------------
//
// El envío es una tarea `send_sales_document` que nace en revisión (`Store::enqueue_sales_send`) y
// pasa por el mismo camino de aprobación, recuperación (D50) y regla que el resto de acciones, con
// tres diferencias: el PDF se regenera al ejecutar y, si cambió, vuelve a revisión (D232); la
// recuperación consulta solo la cuenta que lo envió, y si no responde es `unavailable`, nunca «no
// enviado» (D240); y al confirmarse el recibo el historial, el estado del documento y la tarea se
// escriben en una sola transacción (D233, D239). No usa IA y no deja señales de aprendizaje.

/// Códigos estables de los errores del envío de un documento (D229, D232–D238). Los comparte
/// `AppError::code()` con el resto de errores de venta, y también aparecen en `task.error.code`.
pub mod send_codes {
    pub const NO_MAIL_ACCOUNT: &str = "no_mail_account";
    pub const ACCOUNT_REQUIRED: &str = "account_required";
    pub const ACCOUNT_UNAVAILABLE: &str = "account_unavailable";
    pub const RECIPIENT_MISSING: &str = "recipient_missing";
    pub const RECIPIENT_INVALID: &str = "recipient_invalid";
    pub const SUBJECT_INVALID: &str = "subject_invalid";
    pub const BODY_EMPTY: &str = "body_empty";
    pub const ATTACHMENT_TOO_LARGE: &str = "attachment_too_large";
    pub const STATUS_NOT_SENDABLE: &str = "status_not_sendable";
    pub const DOCUMENT_REVISED: &str = "document_revised";
    pub const DOCUMENT_VOIDED: &str = "document_voided";
    pub const PDF_CHANGED: &str = "pdf_changed";
    pub const SEND_IN_PROGRESS: &str = "send_in_progress";
}

/// Longitud máxima del asunto y de una dirección (D229).
const MAX_SUBJECT_CHARS: usize = 200;
const MAX_ADDRESS_CHARS: usize = 254;

/// Motivo de revisión cuando el PDF regenerado no coincide con el que se vio (D232).
pub const PDF_CHANGED_REASON: &str =
    "el PDF del documento ya no es el que revisó; compruébelo de nuevo antes de enviarlo";
/// Motivo de revisión cuando el documento tiene una revisión nueva (D234).
pub const DOCUMENT_REVISED_REASON: &str =
    "el documento tiene una revisión nueva; revise el envío con la revisión vigente";

fn send_error(code: &str, message: impl Into<String>) -> AppError {
    AppError::Sales(SalesStoreError::External {
        code: code.to_owned(),
        message: message.into(),
    })
}

/// Nombre del tipo de documento en el asunto y en el cuerpo (D236).
fn kind_title(kind: DocumentKind) -> &'static str {
    match kind {
        DocumentKind::Quote => "Presupuesto",
        DocumentKind::DeliveryNote => "Albarán",
        DocumentKind::Invoice => "Factura",
        DocumentKind::SimplifiedInvoice => "Factura simplificada",
        DocumentKind::CorrectiveInvoice => "Factura rectificativa",
    }
}

fn kind_with_article(kind: DocumentKind) -> &'static str {
    match kind {
        DocumentKind::Quote => "el presupuesto",
        DocumentKind::DeliveryNote => "el albarán",
        DocumentKind::Invoice => "la factura",
        DocumentKind::SimplifiedInvoice => "la factura simplificada",
        DocumentKind::CorrectiveInvoice => "la factura rectificativa",
    }
}

/// Una sola línea sin caracteres de control ni espacios repetidos: los nombres que vienen de los
/// datos maestros no pueden romper el asunto ni el saludo (D229).
fn plain_line(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Asunto del correo (D236): `<Tipo> <número>[ (revisión N)] — <empresa>`. Sin empresa, sin el
/// final; nunca «None».
pub fn sales_mail_subject(
    kind: DocumentKind,
    number: &str,
    revision: u32,
    company: Option<&str>,
) -> String {
    let mut subject = format!("{} {}", kind_title(kind), plain_line(number));
    if revision > 1 {
        subject.push_str(&format!(" (revisión {revision})"));
    }
    if let Some(company) = company.map(plain_line).filter(|c| !c.is_empty()) {
        subject.push_str(" — ");
        subject.push_str(&company);
    }
    subject.chars().take(MAX_SUBJECT_CHARS).collect()
}

/// Cuerpo del correo (D236), plantilla fija sin IA: «Hola[, <cliente>]:», la frase del adjunto con
/// el número y la fecha de emisión si constan, y la despedida con la empresa si consta.
pub fn sales_mail_body(
    kind: DocumentKind,
    number: &str,
    issue_date: Option<IsoDate>,
    customer: Option<&str>,
    company: Option<&str>,
) -> String {
    let mut body = match customer.map(plain_line).filter(|c| !c.is_empty()) {
        Some(name) => format!("Hola, {name}:"),
        None => "Hola:".to_owned(),
    };
    body.push_str(&format!(
        "\n\nLe adjuntamos {} {}",
        kind_with_article(kind),
        plain_line(number)
    ));
    if let Some(date) = issue_date {
        body.push_str(&format!(
            " del {:02}/{:02}/{:04}",
            date.day, date.month, date.year
        ));
    }
    body.push_str(".\n\nUn saludo,");
    if let Some(company) = company.map(plain_line).filter(|c| !c.is_empty()) {
        body.push('\n');
        body.push_str(&company);
    }
    body
}

/// Dirección simple de destinatario (D229): la misma validación que los conectores, más estricta.
fn address_ok(address: &str) -> bool {
    is_email_address(address) && address.chars().count() <= MAX_ADDRESS_CHARS
}

fn check_subject(subject: &str) -> AppResult<()> {
    if subject.trim().is_empty() {
        return Err(send_error(
            send_codes::SUBJECT_INVALID,
            "el asunto está vacío",
        ));
    }
    if subject.chars().count() > MAX_SUBJECT_CHARS {
        return Err(send_error(
            send_codes::SUBJECT_INVALID,
            format!("el asunto supera los {MAX_SUBJECT_CHARS} caracteres"),
        ));
    }
    if subject.chars().any(char::is_control) {
        return Err(send_error(
            send_codes::SUBJECT_INVALID,
            "el asunto no puede llevar saltos de línea ni otros caracteres de control",
        ));
    }
    Ok(())
}

fn check_recipient(to: &str) -> AppResult<()> {
    let to = to.trim();
    if to.is_empty() {
        return Err(send_error(
            send_codes::RECIPIENT_MISSING,
            "falta el destinatario: el cliente no tiene correo; indique una dirección",
        ));
    }
    if !address_ok(to) {
        return Err(send_error(
            send_codes::RECIPIENT_INVALID,
            "el destinatario debe ser una única dirección de correo, como usuario@dominio.es",
        ));
    }
    Ok(())
}

/// Resultado de `check_recipient` como el texto que guarda `checks.recipient`.
fn recipient_state(to: &str) -> &'static str {
    match check_recipient(to) {
        Ok(()) => "ok",
        Err(_) if to.trim().is_empty() => "missing",
        Err(_) => "invalid",
    }
}

/// Estados de un documento que impiden enviarlo, con su código (D233).
fn check_sendable(doc: &SalesDocument) -> AppResult<()> {
    if doc.status == Status::Voided {
        return Err(send_error(
            send_codes::DOCUMENT_VOIDED,
            "el documento está anulado y no se envía",
        ));
    }
    if doc.superseded_by.is_some() {
        return Err(send_error(
            send_codes::DOCUMENT_REVISED,
            "el documento tiene una revisión posterior: envíe la revisión vigente",
        ));
    }
    if doc.number.is_none() || !is_sendable(doc.kind, doc.status) {
        return Err(send_error(
            send_codes::STATUS_NOT_SENDABLE,
            format!(
                "un documento en estado «{}» no se puede enviar por correo",
                doc.status.as_str()
            ),
        ));
    }
    Ok(())
}

/// Cuentas entre las que se puede elegir para enviar (D237): las conectadas; si hay alguna real,
/// «Correo simulado» no se propone.
fn send_pool(accounts: &[AccountView]) -> Vec<&AccountView> {
    let connected: Vec<&AccountView> = accounts
        .iter()
        .filter(|a| a.status == AccountStatus::Connected)
        .collect();
    let real: Vec<&AccountView> = connected
        .iter()
        .copied()
        .filter(|a| a.provider != MailProviderKind::Simulator)
        .collect();
    if real.is_empty() {
        connected
    } else {
        real
    }
}

/// Cuenta de envío por defecto (D237): la indicada si está entre las posibles; si no, la cuya
/// dirección es la de la empresa (sin mayúsculas); si no, la única posible; si no, ninguna (el
/// envío se prepara y aprobar da `account_required`). Sin ninguna cuenta capaz de enviar, error.
fn choose_account<'a>(
    accounts: &'a [AccountView],
    company_email: Option<&str>,
    hint: Option<&str>,
) -> AppResult<Option<&'a AccountView>> {
    let pool = send_pool(accounts);
    if pool.is_empty() {
        return Err(send_error(
            send_codes::NO_MAIL_ACCOUNT,
            "no hay ninguna cuenta de correo conectada desde la que enviar: conecte una cuenta",
        ));
    }
    if let Some(found) = hint.and_then(|h| pool.iter().find(|a| a.id == h)) {
        return Ok(Some(found));
    }
    let company = company_email.map(str::trim).filter(|e| !e.is_empty());
    if let Some(found) = company.and_then(|company| {
        pool.iter().find(|a| {
            a.address
                .as_deref()
                .is_some_and(|address| address.trim().eq_ignore_ascii_case(company))
        })
    }) {
        return Ok(Some(found));
    }
    Ok(match pool.as_slice() {
        [only] => Some(*only),
        _ => None,
    })
}

fn account_json(account: &AccountView) -> Value {
    json!({
        "id": account.id,
        "provider": account.provider,
        "label": account.label,
        "address": account.address,
    })
}

fn attachment_meta(attachment: &OutgoingAttachment) -> Value {
    json!({
        "name": attachment.name,
        "mime": attachment.mime,
        "size": attachment.size,
        "sha256": attachment.sha256,
    })
}

/// Nombre del archivo PDF sin extensión: el número con solo `[A-Za-z0-9_-]` y `-r<n>` desde la
/// revisión 2. Lo comparten el archivo local (`sales_document_pdf`) y el adjunto del correo.
fn pdf_stem(number: &str, revision: u32) -> String {
    let mut name: String = number
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if revision > 1 {
        name.push_str(&format!("-r{revision}"));
    }
    name
}

/// Estado del envío guardado en la tarea: la cuenta que hizo cada intento (D240). Se escribe
/// antes de llamar a ningún conector, así que siempre consta la cuenta de una acción que existe.
fn attempt_account(result: &Value, version: i64) -> Option<String> {
    result["attempt_accounts"][version.to_string()]
        .as_str()
        .map(str::to_owned)
        .or_else(|| {
            (result["attempt"]["version"].as_i64() == Some(version))
                .then(|| {
                    result["attempt"]["account"]["id"]
                        .as_str()
                        .map(str::to_owned)
                })
                .flatten()
        })
}

/// Cómo era la cuenta cuando se hizo un intento (D242): la dirección del buzón y las veces que se
/// había vuelto a iniciar sesión. Se escribe una sola vez por versión, antes de llamar a ningún
/// conector.
#[derive(Debug, Clone, PartialEq, Eq)]
struct AttemptMark {
    address: Option<String>,
    epoch: u32,
}

fn attempt_mark(result: &Value, version: i64) -> Option<AttemptMark> {
    let mark = result["attempt_marks"][version.to_string()]
        .as_object()
        .filter(|m| m.contains_key("address"))?;
    Some(AttemptMark {
        address: mark["address"].as_str().map(str::to_owned),
        epoch: mark
            .get("epoch")
            .and_then(Value::as_u64)
            .and_then(|e| u32::try_from(e).ok())
            .unwrap_or(0),
    })
}

/// Dos direcciones de buzón son la misma si ambas faltan o si coinciden sin distinguir
/// mayúsculas (D242).
fn same_mailbox(a: Option<&str>, b: Option<&str>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => a.trim().eq_ignore_ascii_case(b.trim()),
        _ => false,
    }
}

fn unavailable(message: impl Into<String>) -> ConnectorError {
    ConnectorError {
        code: "unavailable".to_owned(),
        message: message.into(),
    }
}

/// Consulta a la cuenta que hizo un intento (D240, D242). Sin la marca del intento, o si la
/// cuenta ya no es el mismo buzón, `unavailable`. Si la cuenta se reconectó después del intento y
/// su registro local no conoce la clave, no se puede afirmar «no enviado»: devuelve `not_found`,
/// que solo cierra la persona con «Confirmo que no se envió».
fn lookup_marked(
    accounts: &Accounts,
    account: &str,
    mark: Option<&AttemptMark>,
    key: &str,
) -> Result<Option<Value>, ConnectorError> {
    let Some(mark) = mark else {
        return Err(unavailable(
            "no consta la dirección de la cuenta que hizo el envío; no se puede saber si salió",
        ));
    };
    if let Some((_, current)) = accounts.identity(account) {
        if !same_mailbox(mark.address.as_deref(), current.as_deref()) {
            return Err(unavailable(format!(
                "la cuenta {account} ya no es el buzón desde el que se hizo el envío; no se \
                 puede saber si salió"
            )));
        }
    }
    let found = accounts.lookup_in(account, key)?;
    if found.is_none() && accounts.epoch_of(account) != Some(mark.epoch) {
        return Err(ConnectorError {
            code: nestadmin_simulators::codes::NOT_FOUND.to_owned(),
            message: format!(
                "la cuenta {account} se reconectó después del intento y no consta el envío; \
                 confirme que no se envió"
            ),
        });
    }
    Ok(found)
}

/// Un conector que consulta y ejecuta solo en la cuenta indicada (D240). `perform_action` pregunta
/// con él si una acción ya iniciada se hizo: nunca recorre las demás cuentas ni se salta una caída.
struct SingleAccount<'a> {
    accounts: &'a Accounts,
    account: &'a str,
    mark: Option<AttemptMark>,
}

impl Connector for SingleAccount<'_> {
    fn lookup(&self, key: &str) -> Result<Option<Value>, ConnectorError> {
        lookup_marked(self.accounts, self.account, self.mark.as_ref(), key)
    }
    fn execute(&self, key: &str, payload: &Value) -> Result<Value, ConnectorError> {
        self.accounts.execute(key, payload)
    }
}

/// Cada clave pendiente con la cuenta que la inició, para reconciliar al abrir (D240).
struct KeyedAccounts<'a> {
    accounts: &'a Accounts,
    by_key: HashMap<String, (String, Option<AttemptMark>)>,
}

impl Connector for KeyedAccounts<'_> {
    fn lookup(&self, key: &str) -> Result<Option<Value>, ConnectorError> {
        match self.by_key.get(key) {
            Some((account, mark)) => lookup_marked(self.accounts, account, mark.as_ref(), key),
            None => Err(ConnectorError {
                code: "unavailable".to_owned(),
                message: "no consta la cuenta que inició este envío".to_owned(),
            }),
        }
    }
    fn execute(&self, _key: &str, _payload: &Value) -> Result<Value, ConnectorError> {
        Err(ConnectorError {
            code: "invalid_input".to_owned(),
            message: "la reconciliación no ejecuta envíos".to_owned(),
        })
    }
}

impl App {
    // --- Consultas ---

    /// Solo para pruebas: añade una cuenta con un conector propio (por ejemplo, uno que pierde la
    /// respuesta del envío o no responde a las consultas) para probar la recuperación (D50, D240).
    #[doc(hidden)]
    pub fn insert_mail_connector_for_tests(
        &mut self,
        id: &str,
        connector: Box<dyn nestadmin_simulators::MailConnector + Send>,
    ) {
        self.mail.insert_test_connector(id, connector);
    }

    /// Envíos confirmados de un documento, del más antiguo al más reciente (D239).
    pub fn sales_document_sends(&self, document_id: i64) -> AppResult<Vec<SalesSend>> {
        Ok(self.store.sales_document_sends(document_id)?)
    }

    /// La tarea de envío abierta de un documento (en revisión, en curso o fallida), si la hay.
    pub fn sales_send_task(&self, document_id: i64) -> AppResult<Option<Task>> {
        Ok(self.store.open_sales_send_task(document_id)?)
    }

    /// Cuentas entre las que se puede elegir para enviar un documento (D237): las conectadas, sin
    /// «Correo simulado» si hay alguna cuenta real.
    pub fn sales_send_accounts(&self) -> Vec<AccountView> {
        let view = self.mail.view();
        send_pool(&view.accounts).into_iter().cloned().collect()
    }

    /// Una regla de envío de documentos fija, al crearse o cambiar, la dirección actual de su
    /// cuenta (`account_address`, D242), que el cliente de la interfaz no tiene que enviar. Si la
    /// cuenta no consta o no tiene dirección, la condición se quita y la regla no automatiza.
    pub(crate) fn bound_rule_conditions(&self, action_kind: &str, conditions: &Value) -> Value {
        let mut conditions = conditions.clone();
        if action_kind != SEND_SALES_DOCUMENT {
            return conditions;
        }
        let address = conditions["account"]
            .as_str()
            .and_then(|id| self.mail.identity(id.trim()))
            .and_then(|(_, address)| address)
            .filter(|a| !a.trim().is_empty());
        if let Some(fields) = conditions.as_object_mut() {
            match address {
                Some(address) => {
                    fields.insert("account_address".into(), json!(address.trim()));
                }
                None => {
                    fields.remove("account_address");
                }
            }
        }
        conditions
    }

    // --- Preparar y editar ---

    /// Prepara el envío por correo de un documento emitido: crea la tarea `send_sales_document`
    /// pendiente de revisión, con el destinatario (el correo del cliente), la cuenta, el asunto y
    /// el cuerpo de la plantilla y los metadatos del PDF que se adjuntaría (D236, D237). No envía
    /// nada: el envío exige aprobarla (R07). Errores: `status_not_sendable`, `document_voided`,
    /// `document_revised`, `no_mail_account`, `attachment_too_large`, `send_in_progress` y los del
    /// PDF; sin cuenta capaz de enviar no se crea tarea.
    pub fn prepare_sales_send(&mut self, document_id: i64) -> AppResult<Task> {
        if self.store.open_sales_send_task(document_id)?.is_some() {
            return Err(send_error(
                send_codes::SEND_IN_PROGRESS,
                "este documento ya tiene un envío por correo abierto: termínelo o descártelo",
            ));
        }
        let result = self.build_send(document_id, None, false)?;
        let task = self.store.enqueue_sales_send(
            document_id,
            &result,
            "envío por correo preparado; pendiente de revisión",
            false,
        )?;
        task.ok_or_else(|| {
            send_error(
                send_codes::SEND_IN_PROGRESS,
                "este documento ya tiene un envío por correo abierto",
            )
        })
    }

    /// Cambia el destinatario, el asunto, el cuerpo y la cuenta de un envío en revisión. Valida lo
    /// mismo que la aprobación y deja una versión nueva que exige aprobarse otra vez (D232).
    /// Una regla nunca usa un destinatario editado (D238).
    pub fn edit_sales_send(
        &mut self,
        task_id: i64,
        to: &str,
        subject: &str,
        body: &str,
        account: &str,
    ) -> AppResult<Task> {
        let (task, mut result) = self.reviewable(task_id)?;
        if task.kind != SEND_SALES_DOCUMENT {
            return Err(AppError::Invalid(
                "solo se editan envíos de documentos de venta".into(),
            ));
        }
        let (to, subject) = (to.trim(), subject.trim());
        check_recipient(to)?;
        check_subject(subject)?;
        if body.trim().is_empty() {
            return Err(send_error(send_codes::BODY_EMPTY, "el cuerpo está vacío"));
        }
        let view = self.mail.view();
        let account = account.trim();
        let Some(chosen) = send_pool(&view.accounts)
            .into_iter()
            .find(|a| a.id == account)
        else {
            return Err(send_error(
                if account.is_empty() {
                    send_codes::ACCOUNT_REQUIRED
                } else {
                    send_codes::ACCOUNT_UNAVAILABLE
                },
                "elija una cuenta de correo conectada",
            ));
        };
        result["draft"]["to"] = json!([to]);
        result["draft"]["subject"] = json!(subject);
        result["draft"]["body"] = json!(body);
        result["account"] = account_json(chosen);
        result["checks"]["account"] = json!("ok");
        result["checks"]["recipient"] = json!(recipient_state(to));
        result["warnings"] = json!({ "recipient_differs": recipient_differs(&result) });
        result["edited"] = json!(true);
        result["version"] = json!(result["version"].as_i64().unwrap_or(1) + 1);
        if let Some(fields) = result.as_object_mut() {
            fields.remove("evaluation");
        }
        Ok(self.store.revise(task_id, &result)?)
    }

    /// Contenido completo del envío: documento, destinatario, plantilla, cuenta y PDF. Con
    /// `lenient` (disparo de la regla al emitir) un PDF que no se puede generar o es demasiado
    /// grande no impide crear la tarea: queda anotado en `checks.pdf` y bloquea la regla.
    fn build_send(
        &mut self,
        document_id: i64,
        hint: Option<&str>,
        lenient: bool,
    ) -> AppResult<Value> {
        let doc = self.store.sales_document(document_id)?;
        self.compose_send(&doc, hint, None, lenient)
    }

    fn compose_send(
        &self,
        doc: &SalesDocument,
        hint: Option<&str>,
        to_override: Option<&str>,
        lenient: bool,
    ) -> AppResult<Value> {
        check_sendable(doc)?;
        let number = doc.number.clone().unwrap_or_default();
        let accounts = self.mail.view().accounts;
        let company_email = self.store.sales_company()?.and_then(|c| c.email);
        let account = choose_account(&accounts, company_email.as_deref(), hint)?;
        let customer_email = match doc.customer_id {
            Some(id) => self
                .store
                .customer(id)
                .ok()
                .and_then(|c| c.data.email)
                .map(|e| e.trim().to_owned())
                .filter(|e| !e.is_empty()),
            None => None,
        };
        let (attachment, pdf_check) = match self.send_attachment(doc) {
            Ok(attachment) => (Some(attachment), "ok"),
            Err(error) if lenient => (
                None,
                if error.code() == send_codes::ATTACHMENT_TOO_LARGE {
                    "too_large"
                } else {
                    "failed"
                },
            ),
            Err(error) => return Err(error),
        };
        let company = doc.issuer.as_ref().and_then(|i| i.party.name.as_deref());
        let customer_name = doc.recipient.as_ref().and_then(|r| r.name.as_deref());
        let to = to_override
            .map(|t| t.trim().to_owned())
            .or_else(|| customer_email.clone())
            .unwrap_or_default();
        let draft_to: Vec<String> = if to.is_empty() {
            Vec::new()
        } else {
            vec![to.clone()]
        };
        let mut result = json!({
            "type": SEND_SALES_DOCUMENT,
            "version": 1,
            "document": {
                "id": doc.id,
                "kind": doc.kind.as_str(),
                "number": number,
                "revision": doc.revision,
                "status": doc.status.as_str(),
            },
            "customer_id": doc.customer_id,
            "customer_email_at_prepare": customer_email,
            "draft": {
                "to": draft_to,
                "subject": sales_mail_subject(doc.kind, &number, doc.revision, company),
                "body": sales_mail_body(doc.kind, &number, doc.issue_date, customer_name, company),
                "attachments": attachment.iter().map(attachment_meta).collect::<Vec<_>>(),
            },
            "account": account.map(account_json),
            "checks": {
                "sendable": true,
                "pdf": pdf_check,
                "account": if account.is_some() { "ok" } else { "missing" },
                "recipient": recipient_state(&to),
            },
            "action": { "kind": SEND_SALES_DOCUMENT },
        });
        result["warnings"] = json!({ "recipient_differs": recipient_differs(&result) });
        Ok(result)
    }

    /// PDF del documento tal como se adjuntaría: regenerado (no el archivo guardado) y de como
    /// mucho 3 MB (D229). Una factura sin su registro de alta no tiene PDF: `status_not_sendable`.
    fn send_attachment(&self, doc: &SalesDocument) -> AppResult<OutgoingAttachment> {
        let bytes = self.render_sales_pdf(doc).map_err(|error| match error {
            AppError::Sales(SalesStoreError::Conflict(why)) => {
                send_error(send_codes::STATUS_NOT_SENDABLE, why)
            }
            other => other,
        })?;
        if bytes.len() as u64 > MAX_OUTGOING_ATTACHMENT_BYTES {
            return Err(send_error(
                send_codes::ATTACHMENT_TOO_LARGE,
                format!(
                    "el PDF pesa {:.1} MB y el máximo para enviarlo por correo es {:.0} MB: \
                     use un membrete más ligero",
                    bytes.len() as f64 / 1_000_000.0,
                    MAX_OUTGOING_ATTACHMENT_BYTES as f64 / 1_000_000.0
                ),
            ));
        }
        let stem: String = pdf_stem(doc.number.as_deref().unwrap_or(""), doc.revision)
            .chars()
            .take(76)
            .collect();
        Ok(OutgoingAttachment::pdf(&format!("{stem}.pdf"), bytes))
    }

    // --- Aprobar y ejecutar ---

    /// Valida el contenido que se va a enviar: destinatario, asunto, cuerpo y cuenta. Un fallo
    /// no anota ningún intento ni acción.
    fn validate_send(&self, result: &Value) -> AppResult<()> {
        let recipients = result["draft"]["to"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        match recipients.as_slice() {
            [] => check_recipient("")?,
            [one] => match one.as_str() {
                Some(address) => check_recipient(address)?,
                None => return Err(invalid_recipient()),
            },
            _ => return Err(invalid_recipient()),
        }
        check_subject(result["draft"]["subject"].as_str().unwrap_or(""))?;
        if result["draft"]["body"]
            .as_str()
            .is_none_or(|b| b.trim().is_empty())
        {
            return Err(send_error(send_codes::BODY_EMPTY, "el cuerpo está vacío"));
        }
        let Some(account) = result["account"]["id"].as_str().filter(|a| !a.is_empty()) else {
            return Err(send_error(
                send_codes::ACCOUNT_REQUIRED,
                "elija la cuenta de correo desde la que se envía",
            ));
        };
        // La misma lista que al editar (D237) y el mismo buzón que al preparar (D242).
        let view = self.mail.view();
        let Some(current) = send_pool(&view.accounts)
            .into_iter()
            .find(|a| a.id == account)
        else {
            return Err(send_error(
                send_codes::ACCOUNT_UNAVAILABLE,
                "la cuenta de correo elegida no está conectada o no se puede elegir",
            ));
        };
        if !same_mailbox(
            result["account"]["address"].as_str(),
            current.address.as_deref(),
        ) {
            return Err(send_error(
                send_codes::ACCOUNT_UNAVAILABLE,
                "la cuenta de correo elegida ya no es el mismo buzón que al preparar el envío: \
                 elíjala otra vez",
            ));
        }
        Ok(())
    }

    /// Aprobación de un envío de documento: valida y ejecuta una sola vez. No anota señales de
    /// aprendizaje (D129). Una versión distinta de la que se vio da `stale_version` antes.
    pub(crate) fn approve_sales_send(&mut self, task: Task, result: Value) -> AppResult<Task> {
        self.validate_send(&result)?;
        self.execute_sales_send(task, result, Authorization::Approval)
    }

    fn fail_send(
        &mut self,
        task: &Task,
        code: &str,
        message: impl Into<String>,
        recoverable: bool,
    ) -> AppResult<Task> {
        Ok(self
            .store
            .fail(task.id, &task_error(code, message, recoverable))?)
    }

    /// Ejecuta el envío una sola vez (D50, D232–D235). Orden: intentos anteriores (si alguno
    /// salió, se completa con él), estado del documento, revisión nueva (vuelve a revisión),
    /// PDF regenerado (si cambió, vuelve a revisión), intento anotado, borrador y envío.
    pub(crate) fn execute_sales_send(
        &mut self,
        task: Task,
        mut result: Value,
        authorization: Authorization,
    ) -> AppResult<Task> {
        let version = result["version"].as_i64().unwrap_or(1);
        // Quien autoriza lo que se ejecuta ahora es quien consta en el resultado (D129).
        if matches!(authorization, Authorization::Approval)
            && result["authorization"]["type"] == "rule"
        {
            result["rule_attempt"] = result["authorization"].clone();
        }
        result["authorization"] = serde_json::to_value(&authorization).unwrap_or_default();
        match self.check_previous(&task, Some(version), Some(&authorization))? {
            Previous::Clear => {}
            Previous::Done {
                version,
                receipt,
                authorization: done_by,
            } => {
                if done_by.starts_with("regla ") && result["rule_attempt"]["type"] == "rule" {
                    result["authorization"] = result["rule_attempt"].clone();
                }
                return self.complete_with(task.id, result, version, receipt, None);
            }
            Previous::Unknown { version, error }
                if error.code == nestadmin_simulators::codes::NOT_FOUND =>
            {
                result["review_reason"] = json!(crate::MISSING_REASON);
                result["recovery"] = json!({
                    "version": version,
                    "error": error.code,
                    "needs_confirmation": true,
                });
                return self.back_to_review(&task, &result, None);
            }
            Previous::Unknown { error, .. } => {
                return self.fail_send(
                    &task,
                    &error.code,
                    format!("no se pudo comprobar el intento anterior: {error}"),
                    true,
                );
            }
        }
        let Some(document_id) = result["document"]["id"].as_i64() else {
            return self.fail_send(
                &task,
                "invalid_input",
                "el envío no indica el documento",
                false,
            );
        };
        let doc = self.store.sales_document(document_id)?;
        if doc.status == Status::Voided {
            return self.fail_send(
                &task,
                send_codes::DOCUMENT_VOIDED,
                "el documento está anulado y no se envía",
                false,
            );
        }
        if doc.superseded_by.is_some() {
            return self.send_revised(task, result, doc);
        }
        if !is_sendable(doc.kind, doc.status) {
            return self.fail_send(
                &task,
                send_codes::STATUS_NOT_SENDABLE,
                format!(
                    "un documento en estado «{}» no se puede enviar por correo",
                    doc.status.as_str()
                ),
                false,
            );
        }
        let attachment = match self.send_attachment(&doc) {
            Ok(attachment) => attachment,
            Err(error) => {
                let recoverable = error.code() != send_codes::ATTACHMENT_TOO_LARGE;
                return self.fail_send(&task, error.code(), error.to_string(), recoverable);
            }
        };
        let seen = result["draft"]["attachments"][0]["sha256"].as_str();
        if seen != Some(attachment.sha256.as_str()) {
            // El PDF ya no es el que la persona vio (por ejemplo, se cambió el membrete o PDFium):
            // nada se envía y vuelve a revisión con los metadatos nuevos (D232).
            result["pdf_changed"] = json!({ "from": seen, "to": attachment.sha256 });
            result["draft"]["attachments"] = json!([attachment_meta(&attachment)]);
            result["checks"]["pdf"] = json!("ok");
            result["version"] = json!(version + 1);
            result["review_reason"] = json!(PDF_CHANGED_REASON);
            if let Some(fields) = result.as_object_mut() {
                fields.remove("recovery");
            }
            return self.back_to_review(&task, &result, None);
        }
        // Lo que sale es del buzón que se vio al preparar, también con una regla (D242).
        let account = result["account"]["id"].as_str().unwrap_or("").to_owned();
        let Some((_, current_address)) = self.mail.identity(&account) else {
            return self.fail_send(
                &task,
                send_codes::ACCOUNT_UNAVAILABLE,
                "la cuenta de correo elegida no está conectada",
                true,
            );
        };
        if !same_mailbox(
            result["account"]["address"].as_str(),
            current_address.as_deref(),
        ) {
            return self.fail_send(
                &task,
                send_codes::ACCOUNT_UNAVAILABLE,
                "la cuenta de correo ya no es el mismo buzón que al preparar el envío",
                true,
            );
        }
        // Se guarda lo que se va a ejecutar, y con qué cuenta, antes de llamar a ningún conector
        // (D50, D240).
        result["attempt"] = json!({
            "version": version,
            "draft": result["draft"],
            "document": result["document"],
            "account": result["account"],
        });
        result["attempt_accounts"][version.to_string()] = json!(account);
        // La marca no se reescribe al repetir la misma versión: si la cuenta se reconectó entre
        // medias, tiene que seguir constando cómo era al primer intento (D242).
        if attempt_mark(&result, version).is_none() {
            result["attempt_marks"][version.to_string()] = json!({
                "address": result["account"]["address"],
                "epoch": self.mail.epoch_of(&account).unwrap_or(0),
            });
        }
        if let Some(fields) = result.as_object_mut() {
            fields.remove("recovery");
        }
        self.store.record_attempt(task.id, &result)?;
        match self.send_sales(&task, &result, version, attachment, &account, authorization) {
            Ok(done) => {
                // Si lo ejecutó una regla y aquí solo se reconcilia, consta la regla (D129).
                if matches!(done, nestadmin_core::ActionOutcome::Reconciled(_))
                    && result["rule_attempt"]["type"] == "rule"
                {
                    let by_rule = self.store.task_actions(task.id)?.iter().any(|a| {
                        a.kind == SEND_SALES_DOCUMENT
                            && a.version == version
                            && a.authorization.starts_with("regla ")
                    });
                    if by_rule {
                        result["authorization"] = result["rule_attempt"].clone();
                    }
                }
                result["receipt"] = done.receipt().clone();
                self.complete_sales_send(task.id, result, version)
            }
            Err(error) => {
                let code = match &error {
                    CoreError::Connector(e) => e.code.clone(),
                    _ => "action_failed".to_owned(),
                };
                self.fail_send(&task, &code, error.to_string(), true)
            }
        }
    }

    /// Crea el borrador del mensaje nuevo con el PDF y lo envía con la acción `send_sales_document`
    /// (clave `task-<id>/send_sales_document/v<n>`). Repetirlo tras un corte devuelve el mismo
    /// borrador y la misma carga; la recuperación pregunta solo a la cuenta de la tarea (D240).
    fn send_sales(
        &mut self,
        task: &Task,
        result: &Value,
        version: i64,
        attachment: OutgoingAttachment,
        account: &str,
        authorization: Authorization,
    ) -> Result<nestadmin_core::ActionOutcome, CoreError> {
        let mut draft: NewDraft = serde_json::from_value(result["draft"].clone())?;
        draft.attachments = vec![attachment];
        let draft_id = self.mail.create_new_draft(
            account,
            &draft,
            &format!("task-{}/create_sales_draft/v{version}", task.id),
        )?;
        let request = ActionRequest {
            task_id: task.id,
            kind: SEND_SALES_DOCUMENT.into(),
            version,
            payload: json!({ "draft_id": draft_id }),
            authorization,
        };
        let connector = SingleAccount {
            accounts: &self.mail,
            account,
            mark: attempt_mark(result, version),
        };
        self.store.perform_action(&request, &connector)
    }

    /// El documento tiene una revisión posterior (D234). Ya se comprobó que ningún intento salió:
    /// la tarea vuelve a revisión, con versión nueva, sobre la revisión vigente y su PDF.
    fn send_revised(&mut self, task: Task, result: Value, doc: SalesDocument) -> AppResult<Task> {
        let mut latest = doc;
        for _ in 0..100 {
            match latest.superseded_by {
                Some(next) => latest = self.store.sales_document(next)?,
                None => break,
            }
        }
        let version = result["version"].as_i64().unwrap_or(1);
        let to = result["draft"]["to"][0].as_str().map(str::to_owned);
        let account = result["account"]["id"].as_str().map(str::to_owned);
        let mut fresh = match self.compose_send(&latest, account.as_deref(), to.as_deref(), false) {
            Ok(fresh) => fresh,
            Err(error) => {
                let recoverable = error.code() != send_codes::ATTACHMENT_TOO_LARGE;
                return self.fail_send(&task, error.code(), error.to_string(), recoverable);
            }
        };
        fresh["version"] = json!(version + 1);
        fresh["review_reason"] = json!(DOCUMENT_REVISED_REASON);
        fresh["revised"] = json!({ "from_document": result["document"]["id"] });
        for key in ["attempt", "attempt_accounts", "attempt_marks", "edited"] {
            if !result[key].is_null() {
                fresh[key] = result[key].clone();
            }
        }
        // La tarea pasa a ser la de la revisión vigente. Si esa revisión ya tiene otra abierta o un
        // envío registrado, esta se cancela: nunca dos tareas abiertas para el mismo documento
        // (D244).
        if !self.store.repoint_sales_send(task.id, latest.id)? {
            return Ok(self.store.cancel_with_reason(
                task.id,
                "document_revised: la revisión vigente ya tiene su propio envío",
            )?);
        }
        self.back_to_review(&task, &fresh, None)
    }

    /// Cierra la tarea con el recibo ya confirmado: historial de envíos, estado del documento y
    /// tarea en una sola transacción, idempotente (D233, D239). Si faltan datos para el historial,
    /// la tarea se completa igualmente y lo dice en `status_error`: el correo ya salió.
    pub(crate) fn complete_sales_send(
        &mut self,
        task_id: i64,
        mut result: Value,
        version: i64,
    ) -> AppResult<Task> {
        let key = idempotency_key(task_id, SEND_SALES_DOCUMENT, version);
        let authorization = self
            .store
            .task_actions(task_id)?
            .into_iter()
            .rfind(|a| a.idempotency_key == key && a.status == "confirmed")
            .map(|a| a.authorization)
            .unwrap_or_else(|| match result["authorization"]["type"].as_str() {
                Some("rule") => format!(
                    "regla {} v{}",
                    result["authorization"]["rule_id"].as_str().unwrap_or("?"),
                    result["authorization"]["rule_version"]
                        .as_i64()
                        .unwrap_or(0)
                ),
                _ => "aprobación".to_owned(),
            });
        let attachment = result["draft"]["attachments"][0].clone();
        let send = (|| {
            Some(NewSalesSend {
                document_id: result["document"]["id"].as_i64()?,
                task_id,
                version,
                idempotency_key: key.clone(),
                account: result["account"]["id"].as_str()?.to_owned(),
                account_address: result["account"]["address"].as_str().map(str::to_owned),
                recipient: result["draft"]["to"][0].as_str()?.to_owned(),
                subject: result["draft"]["subject"].as_str()?.to_owned(),
                attachment_name: attachment["name"].as_str()?.to_owned(),
                attachment_size: attachment["size"].as_i64().filter(|s| *s > 0)?,
                attachment_sha256: attachment["sha256"]
                    .as_str()
                    .filter(|s| s.len() == 64)?
                    .to_owned(),
                authorization: authorization.clone(),
                sent_message_id: ["message_id", "receipt_id"].iter().find_map(|field| {
                    result["receipt"][*field]
                        .as_str()
                        .map(|id| split_id(id).1)
                        .filter(|id| !id.is_empty())
                        .map(str::to_owned)
                }),
            })
        })();
        match send {
            Some(send) => Ok(self
                .store
                .complete_sales_send(task_id, &result, &send)?
                .task),
            None => {
                result["status_error"] =
                    json!("faltan datos del envío para el historial; el correo salió");
                Ok(self.store.complete(task_id, &result)?)
            }
        }
    }

    /// Consulta a la cuenta que hizo el intento si ya envió con esa clave (D240). Sin la cuenta
    /// guardada, `unavailable`: no se afirma «no enviado» sin preguntar a quien pudo enviar.
    pub(crate) fn sales_lookup(
        &self,
        task: &Task,
        action: &nestadmin_core::ActionRecord,
    ) -> Result<Option<Value>, ConnectorError> {
        let result = task.result.clone().unwrap_or(Value::Null);
        match attempt_account(&result, action.version) {
            Some(account) => lookup_marked(
                &self.mail,
                &account,
                attempt_mark(&result, action.version).as_ref(),
                &action.idempotency_key,
            ),
            None => Err(unavailable(
                "no consta la cuenta que inició el envío; no se puede saber si salió",
            )),
        }
    }

    /// Al abrir: los envíos de documentos que quedaron pendientes se reconcilian cada uno en su
    /// cuenta. Una cuenta que no responde los deja pendientes (nunca «no enviados»). Los cierra
    /// después `complete_confirmed_reviews`, que escribe el historial y el estado del documento.
    pub(crate) fn reconcile_sales_sends(&mut self) -> AppResult<()> {
        let mut by_key = HashMap::new();
        for task in self.store.all_tasks()? {
            if task.kind != SEND_SALES_DOCUMENT || task.state.is_final() {
                continue;
            }
            let result = task.result.clone().unwrap_or(Value::Null);
            for action in self.store.task_actions(task.id)? {
                if action.kind == SEND_SALES_DOCUMENT && action.status == "pending" {
                    if let Some(account) = attempt_account(&result, action.version) {
                        by_key.insert(
                            action.idempotency_key.clone(),
                            (account, attempt_mark(&result, action.version)),
                        );
                    }
                }
            }
        }
        if by_key.is_empty() {
            return Ok(());
        }
        let connector = KeyedAccounts {
            accounts: &self.mail,
            by_key,
        };
        self.store
            .reconcile_actions(SEND_SALES_DOCUMENT, &connector)?;
        Ok(())
    }

    /// Un envío con un intento sin cerrar no se descarta: pudo salir (D235).
    pub(crate) fn ensure_send_discardable(&self, task: &Task) -> AppResult<()> {
        let unsettled = self
            .store
            .task_actions(task.id)?
            .into_iter()
            .any(|a| a.kind == SEND_SALES_DOCUMENT && a.is_unsettled());
        if unsettled {
            return Err(send_error(
                send_codes::SEND_IN_PROGRESS,
                "el envío tiene un intento sin cerrar: confirme que no se envió antes de \
                 descartarlo",
            ));
        }
        Ok(())
    }

    // --- Regla por cliente al emitir (D238) ---

    /// Tras emitir un documento: si hay una regla activa de envío para su cliente, prepara el
    /// envío y, si la regla aplica y nada lo bloquea, lo ejecuta con la regla como autorización.
    /// Un fallo aquí nunca afecta a la emisión: se anota en el historial sin contenido.
    fn send_on_issue(&mut self, doc: &SalesDocument) {
        if let Err(error) = self.try_send_on_issue(doc) {
            let note = json!({ "document": doc.id, "code": error.code() });
            let _ = self
                .store
                .record_note("sales.send_on_issue", "error", &note);
        }
    }

    fn try_send_on_issue(&mut self, doc: &SalesDocument) -> AppResult<()> {
        let Some(customer_id) = doc.customer_id else {
            return Ok(());
        };
        let (rules, _) = self.rules_for_evaluation()?;
        let candidates: Vec<&Rule> = rules
            .iter()
            .filter(|r| {
                r.active
                    && r.action_kind == SEND_SALES_DOCUMENT
                    && r.conditions["customer_id"].as_i64() == Some(customer_id)
            })
            .collect();
        if candidates.is_empty() {
            return Ok(());
        }
        // Con una sola regla candidata, su cuenta es la preferida; con varias, ninguna: la
        // coincidencia múltiple va a revisión.
        let hint = match candidates.as_slice() {
            [only] => only.conditions["account"].as_str().map(str::to_owned),
            _ => None,
        };
        let mut result = self.build_send(doc.id, hint.as_deref(), true)?;
        let evaluation = evaluate_with(&rules, &HashSet::new(), SEND_SALES_DOCUMENT, &result);
        result["evaluation"] = json!(evaluation.explanation);
        if let RuleDecision::Review(reason) = &evaluation.decision {
            result["review_reason"] = json!(reason);
        }
        let Some(task) = self.store.enqueue_sales_send(
            doc.id,
            &result,
            "envío preparado al emitir; pendiente de revisión",
            true,
        )?
        else {
            return Ok(());
        };
        if let RuleDecision::Automate {
            rule_id,
            rule_version,
        } = evaluation.decision
        {
            self.execute_sales_send(
                task,
                result,
                Authorization::Rule {
                    rule_id,
                    rule_version,
                },
            )?;
        }
        Ok(())
    }
}

fn invalid_recipient() -> AppError {
    send_error(
        send_codes::RECIPIENT_INVALID,
        "el destinatario debe ser una única dirección de correo, como usuario@dominio.es",
    )
}

/// El destinatario del borrador no es el correo del cliente (D236): aviso, no bloqueo.
fn recipient_differs(result: &Value) -> bool {
    let to = result["draft"]["to"][0].as_str();
    match (to, result["customer_email_at_prepare"].as_str()) {
        (Some(to), Some(email)) => !to.trim().eq_ignore_ascii_case(email.trim()),
        // Sin correo del cliente no hay con qué comparar: el aviso propio es «el cliente no tiene
        // correo» (`customer_email_at_prepare` nulo), no esta diferencia.
        _ => false,
    }
}

#[cfg(test)]
mod send_template_tests {
    use super::*;

    #[test]
    fn every_kind_has_its_text_and_missing_data_is_omitted_never_none() {
        let date = Some(IsoDate {
            year: 2026,
            month: 3,
            day: 1,
        });
        let table = [
            (DocumentKind::Quote, "Presupuesto", "el presupuesto"),
            (DocumentKind::DeliveryNote, "Albarán", "el albarán"),
            (DocumentKind::Invoice, "Factura", "la factura"),
            (
                DocumentKind::SimplifiedInvoice,
                "Factura simplificada",
                "la factura simplificada",
            ),
            (
                DocumentKind::CorrectiveInvoice,
                "Factura rectificativa",
                "la factura rectificativa",
            ),
        ];
        for (kind, title, phrase) in table {
            assert_eq!(
                sales_mail_subject(kind, "X-1", 1, Some("Empresa SL")),
                format!("{title} X-1 — Empresa SL")
            );
            assert_eq!(
                sales_mail_subject(kind, "X-1", 3, None),
                format!("{title} X-1 (revisión 3)")
            );
            let full = sales_mail_body(kind, "X-1", date, Some("Cliente SA"), Some("Empresa SL"));
            assert_eq!(
                full,
                format!(
                    "Hola, Cliente SA:\n\nLe adjuntamos {phrase} X-1 del 01/03/2026.\n\nUn saludo,\nEmpresa SL"
                )
            );
            let bare = sales_mail_body(kind, "X-1", None, None, None);
            assert_eq!(
                bare,
                format!("Hola:\n\nLe adjuntamos {phrase} X-1.\n\nUn saludo,")
            );
            assert!(!bare.contains("None") && !full.contains("None"));
        }
    }

    #[test]
    fn names_from_the_master_data_cannot_break_the_subject() {
        let subject = sales_mail_subject(
            DocumentKind::Invoice,
            "F-1",
            1,
            Some("Empresa\r\nBcc: x@ejemplo.test  SL"),
        );
        assert!(!subject.chars().any(char::is_control));
        assert_eq!(subject, "Factura F-1 — Empresa Bcc: x@ejemplo.test SL");
        let long = sales_mail_subject(DocumentKind::Quote, "P-1", 1, Some(&"E".repeat(500)));
        assert_eq!(long.chars().count(), 200);
        assert_eq!(pdf_stem("F 2026/0001", 2), "F_2026_0001-r2");
    }
}
