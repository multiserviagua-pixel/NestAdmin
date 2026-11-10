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

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use nestadmin_core::sales::{
    ChainStatus, Company, Customer, CustomerData, DraftInput, Letterhead, LetterheadMargins,
    ReceivedDeliveryNote, RecurringRecord, RecurringRun, SalesDocument, SalesFilter,
    SalesStoreError, VerifactuContext, VerifactuRecordRow,
};
use nestadmin_documents::IsoDate;
use nestadmin_sales::{
    validate_tax_id, CollectionSummary, DocumentKind, Frequency, LineInput, Payment, PaymentMethod,
    Product, RectificationKind, RectificationMethod, Status,
};
use nestadmin_verifactu::tipos::validar_nif;
use nestadmin_verifactu::{
    CodigoQr, Entorno, FechaHoraHuso, Persona, RegistroFacturacion, SistemaInformatico,
};
use rust_decimal::Decimal;

use crate::{sales_pdf, App, AppError, AppResult};

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
        self.store.set_sales_letterhead(bytes, margins)?;
        Ok(())
    }

    /// Quita el membrete vigente. Los documentos ya emitidos conservan el suyo.
    pub fn remove_sales_letterhead(&mut self) -> AppResult<()> {
        Ok(self.store.remove_sales_letterhead()?)
    }

    /// Cambia los márgenes del área útil; solo afecta a los documentos que se emitan después.
    pub fn set_sales_letterhead_margins(&mut self, margins: LetterheadMargins) -> AppResult<()> {
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
        if doc.revision > 1 {
            name.push_str(&format!("-r{}", doc.revision));
        }
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
        if self.store.sales_document(id)?.kind.is_invoice() {
            let ctx = self.verifactu_context()?;
            Ok(self
                .store
                .issue_sales_invoice_registered(id, issue_date, today, &ctx)?)
        } else {
            Ok(self.store.issue_sales_document(id, issue_date, today)?)
        }
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
