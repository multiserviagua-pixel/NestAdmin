//! Correcciones tras la revisión de E1-E3a (D213-D219): destinatarios por tipo de factura,
//! fecha de operación, anulación con el IdFactura del alta, cambio de NIF con cadena, coherencia
//! de columnas, reloj adelantado y migración desde la versión 11. Prueba independiente de
//! `verifactu_store.rs` (D197). Solo datos sintéticos.

use std::path::Path;

use nestadmin_core::sales::{
    ChainStatus, Company, CustomerData, DraftInput, DraftLine, SalesStoreError, VerifactuContext,
    CODE_CLOCK_BEHIND,
};
use nestadmin_core::store::SCHEMA_VERSION;
use nestadmin_core::Store;
use nestadmin_documents::IsoDate;
use nestadmin_sales::{
    DocumentKind, LineInput, RectificationKind, RectificationMethod, Status, TaxRate,
};
use nestadmin_verifactu::{
    Fecha, FechaHoraHuso, Persona, RegistroFacturacion, SistemaInformatico, TipoFactura,
};
use rust_decimal::Decimal;

fn d(text: &str) -> Decimal {
    text.parse().unwrap()
}

fn date(text: &str) -> IsoDate {
    IsoDate::parse("test", text).unwrap()
}

fn system() -> SistemaInformatico {
    SistemaInformatico {
        productor: Persona::con_nif("Productor Demo SL", "B00000034"),
        nombre_sistema: "NestAdmin".into(),
        id_sistema: "NA".into(),
        version: "0.1.0".into(),
        numero_instalacion: "INST-0001".into(),
        solo_verifactu: true,
        multi_ot: false,
        indicador_multiples_ot: false,
    }
}

fn base_unix() -> i64 {
    FechaHoraHuso::new(Fecha::new(2026, 3, 1).unwrap(), 10, 0, 0, 60)
        .unwrap()
        .unix()
}

fn ctx(offset: i64) -> VerifactuContext {
    VerifactuContext {
        system: system(),
        now: FechaHoraHuso::desde_unix(base_unix() + offset, 60).unwrap(),
    }
}

fn line(price: &str) -> LineInput {
    LineInput {
        description: "Servicio de montaje".into(),
        quantity: d("1"),
        unit_price: d(price),
        discount: Decimal::ZERO,
        tax_rate: TaxRate::TwentyOne,
        exemption: None,
    }
}

fn company(name: &str, nif: &str) -> Company {
    Company {
        name: Some(name.into()),
        tax_id: Some(nif.into()),
        address: Some("Calle Ficticia 1, 28000 Madrid".into()),
        email: Some("demo@example.test".into()),
        iban: None,
        logo: None,
        logo_mime: None,
    }
}

/// Empresa y dos clientes: uno con NIF (devuelto primero) y otro sin NIF.
fn setup(store: &mut Store) -> (i64, i64) {
    store
        .set_sales_company(&company("Taller Demo SL", "B12345674"))
        .unwrap();
    let with_nif = store
        .create_customer(&CustomerData {
            name: "Cliente Uno SA".into(),
            tax_id: Some("12345678Z".into()),
            address: Some("Avenida Inventada 2, 08000 Barcelona".into()),
            email: None,
        })
        .unwrap();
    let without_nif = store
        .create_customer(&CustomerData {
            name: "Cliente Dos".into(),
            tax_id: None,
            address: None,
            email: None,
        })
        .unwrap();
    (with_nif, without_nif)
}

fn draft(kind: DocumentKind, customer: Option<i64>, price: &str) -> DraftInput {
    let mut input = DraftInput::new(kind);
    input.customer_id = customer;
    input.lines = vec![DraftLine::from(line(price))];
    input
}

fn issue(store: &mut Store, id: i64, on: &str, ctx: &VerifactuContext) -> String {
    store
        .issue_sales_invoice_registered(id, date(on), date(on), ctx)
        .unwrap()
        .number
        .unwrap()
}

fn alta(store: &Store, index: usize) -> nestadmin_verifactu::RegistroAlta {
    match &store.verifactu_records().unwrap()[index].record {
        RegistroFacturacion::Alta(a) => a.clone(),
        other => panic!("se esperaba un alta, hay {other:?}"),
    }
}

fn tamper(path: &Path, sql: &str) {
    let conn = rusqlite::Connection::open(path).unwrap();
    conn.execute_batch("DROP TRIGGER verifactu_records_no_update;")
        .unwrap();
    conn.execute_batch(sql).unwrap();
}

#[test]
fn simplified_invoice_has_no_recipients_even_when_the_customer_has_a_nif() {
    let mut store = Store::open_in_memory().unwrap();
    let (with_nif, _) = setup(&mut store);
    let id = store
        .create_sales_draft(&draft(
            DocumentKind::SimplifiedInvoice,
            Some(with_nif),
            "20.00",
        ))
        .unwrap();
    issue(&mut store, id, "2026-03-01", &ctx(0));
    let a = alta(&store, 0);
    assert_eq!(a.tipo_factura, TipoFactura::F2);
    assert!(a.destinatarios.is_empty());
}

#[test]
fn simplified_invoice_with_a_customer_without_nif_is_issued() {
    let mut store = Store::open_in_memory().unwrap();
    let (_, without_nif) = setup(&mut store);
    let id = store
        .create_sales_draft(&draft(
            DocumentKind::SimplifiedInvoice,
            Some(without_nif),
            "20.00",
        ))
        .unwrap();
    issue(&mut store, id, "2026-03-01", &ctx(0));
    assert!(alta(&store, 0).destinatarios.is_empty());
}

#[test]
fn r5_corrective_of_a_simplified_invoice_has_no_recipients() {
    let mut store = Store::open_in_memory().unwrap();
    let (with_nif, _) = setup(&mut store);
    let original = store
        .create_sales_draft(&draft(
            DocumentKind::SimplifiedInvoice,
            Some(with_nif),
            "20.00",
        ))
        .unwrap();
    issue(&mut store, original, "2026-03-01", &ctx(0));
    let corrective = store
        .create_corrective_draft(
            original,
            RectificationKind::R5,
            RectificationMethod::Substitution,
        )
        .unwrap();
    store
        .update_sales_draft(
            corrective,
            &draft(DocumentKind::CorrectiveInvoice, Some(with_nif), "10.00"),
        )
        .unwrap();
    issue(&mut store, corrective, "2026-03-02", &ctx(60));
    let a = alta(&store, 1);
    assert_eq!(a.tipo_factura, TipoFactura::R5);
    assert!(a.destinatarios.is_empty());
}

#[test]
fn full_invoice_still_carries_name_and_nif_of_the_recipient() {
    let mut store = Store::open_in_memory().unwrap();
    let (with_nif, _) = setup(&mut store);
    let id = store
        .create_sales_draft(&draft(DocumentKind::Invoice, Some(with_nif), "100.00"))
        .unwrap();
    issue(&mut store, id, "2026-03-01", &ctx(0));
    assert_eq!(alta(&store, 0).destinatarios.len(), 1);
}

#[test]
fn operation_date_is_recorded_only_when_it_differs_from_the_issue_date() {
    let mut store = Store::open_in_memory().unwrap();
    let (with_nif, _) = setup(&mut store);
    let mut different = draft(DocumentKind::Invoice, Some(with_nif), "100.00");
    different.operation_date = Some(date("2026-02-15"));
    let a_id = store.create_sales_draft(&different).unwrap();
    let mut same = draft(DocumentKind::Invoice, Some(with_nif), "50.00");
    same.operation_date = Some(date("2026-03-01"));
    let b_id = store.create_sales_draft(&same).unwrap();
    let plain = store
        .create_sales_draft(&draft(DocumentKind::Invoice, Some(with_nif), "10.00"))
        .unwrap();
    issue(&mut store, a_id, "2026-03-01", &ctx(0));
    issue(&mut store, b_id, "2026-03-01", &ctx(10));
    issue(&mut store, plain, "2026-03-01", &ctx(20));
    assert_eq!(
        alta(&store, 0).fecha_operacion.map(|f| f.to_string()),
        Some("15-02-2026".to_owned())
    );
    assert_eq!(alta(&store, 1).fecha_operacion, None);
    assert_eq!(alta(&store, 2).fecha_operacion, None);
}

#[test]
fn voiding_an_invoice_without_a_stored_alta_is_rejected() {
    let mut store = Store::open_in_memory().unwrap();
    let (with_nif, _) = setup(&mut store);
    let id = store
        .create_sales_draft(&draft(DocumentKind::Invoice, Some(with_nif), "100.00"))
        .unwrap();
    // Emitida sin registro (camino con gancho vacío): no tiene alta guardada.
    store
        .issue_sales_document_with(id, date("2026-03-01"), date("2026-03-01"), |_, _| Ok(()))
        .unwrap();
    let err = store
        .void_sales_invoice_registered(id, &ctx(60))
        .unwrap_err();
    assert_eq!(err.code(), "conflict");
    assert!(err.to_string().contains("alta"), "{err}");
    assert_eq!(store.sales_document(id).unwrap().status, Status::Issued);
    assert!(store.verifactu_records().unwrap().is_empty());
}

#[test]
fn voiding_twice_is_rejected_and_writes_no_second_cancellation() {
    let mut store = Store::open_in_memory().unwrap();
    let (with_nif, _) = setup(&mut store);
    let id = store
        .create_sales_draft(&draft(DocumentKind::Invoice, Some(with_nif), "100.00"))
        .unwrap();
    issue(&mut store, id, "2026-03-01", &ctx(0));
    store.void_sales_invoice_registered(id, &ctx(60)).unwrap();
    assert!(store.void_sales_invoice_registered(id, &ctx(120)).is_err());
    assert_eq!(store.verifactu_records().unwrap().len(), 2);
    assert_eq!(
        store.verify_verifactu_chain().unwrap(),
        ChainStatus::Ok { records: 2 }
    );
}

#[test]
fn cancellation_uses_the_id_of_the_stored_alta() {
    let mut store = Store::open_in_memory().unwrap();
    let (with_nif, _) = setup(&mut store);
    let id = store
        .create_sales_draft(&draft(DocumentKind::Invoice, Some(with_nif), "100.00"))
        .unwrap();
    issue(&mut store, id, "2026-03-01", &ctx(0));
    store.void_sales_invoice_registered(id, &ctx(60)).unwrap();
    let rows = store.verifactu_records().unwrap();
    assert_eq!(
        rows[1].record.id_factura(),
        rows[0].record.id_factura(),
        "la anulación apunta al IdFactura del alta"
    );
}

#[test]
fn plain_void_of_an_invoice_is_rejected() {
    let mut store = Store::open_in_memory().unwrap();
    let (with_nif, _) = setup(&mut store);
    let id = store
        .create_sales_draft(&draft(DocumentKind::Invoice, Some(with_nif), "100.00"))
        .unwrap();
    issue(&mut store, id, "2026-03-01", &ctx(0));
    let err = store.void_sales_invoice(id).unwrap_err();
    assert_eq!(err.code(), "conflict");
    assert_eq!(store.sales_document(id).unwrap().status, Status::Issued);
}

#[test]
fn company_tax_id_cannot_change_once_records_exist_but_other_fields_can() {
    let mut store = Store::open_in_memory().unwrap();
    let (with_nif, _) = setup(&mut store);
    // Sin registros todavía, el NIF se puede cambiar.
    store
        .set_sales_company(&company("Taller Demo SL", "B00000034"))
        .unwrap();
    store
        .set_sales_company(&company("Taller Demo SL", "B12345674"))
        .unwrap();
    let id = store
        .create_sales_draft(&draft(DocumentKind::Invoice, Some(with_nif), "100.00"))
        .unwrap();
    issue(&mut store, id, "2026-03-01", &ctx(0));

    let err = store
        .set_sales_company(&company("Taller Demo SL", "B00000034"))
        .unwrap_err();
    assert!(matches!(err, SalesStoreError::Conflict(_)), "{err}");
    assert_eq!(
        store.sales_company().unwrap().unwrap().tax_id.as_deref(),
        Some("B12345674")
    );
    // El resto de datos sí cambia, con el mismo NIF.
    store
        .set_sales_company(&company("Taller Demo Nuevo SL", "B12345674"))
        .unwrap();
    assert_eq!(
        store.sales_company().unwrap().unwrap().name.as_deref(),
        Some("Taller Demo Nuevo SL")
    );
}

fn two_records(path: &Path) {
    let mut store = Store::open(path).unwrap();
    let (with_nif, _) = setup(&mut store);
    for (i, price) in ["100.00", "50.00"].into_iter().enumerate() {
        let id = store
            .create_sales_draft(&draft(DocumentKind::Invoice, Some(with_nif), price))
            .unwrap();
        issue(&mut store, id, "2026-03-01", &ctx(30 * i as i64));
    }
}

#[test]
fn a_tampered_previous_hash_column_breaks_the_chain() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nestadmin.db");
    two_records(&path);
    tamper(
        &path,
        &format!(
            "UPDATE verifactu_records SET previous_hash = '{}' WHERE id = 2;",
            "A".repeat(64)
        ),
    );
    let store = Store::open(&path).unwrap();
    match store.verify_verifactu_chain().unwrap() {
        ChainStatus::Broken { at_id, reason } => {
            assert_eq!(at_id, 2);
            assert!(reason.contains("anterior"), "{reason}");
        }
        other => panic!("se esperaba una cadena rota, hay {other:?}"),
    }
}

#[test]
fn a_tampered_column_that_disagrees_with_record_json_breaks_the_chain() {
    for (column, sql) in [
        (
            "total",
            "UPDATE verifactu_records SET total = '1.00' WHERE id = 1;",
        ),
        (
            "invoice_number",
            "UPDATE verifactu_records SET invoice_number = 'F-2026-0099' WHERE id = 1;",
        ),
        (
            "issuer_tax_id",
            "UPDATE verifactu_records SET issuer_tax_id = 'B00000034' WHERE id = 1;",
        ),
        (
            "tax_total",
            "UPDATE verifactu_records SET tax_total = '0.00' WHERE id = 1;",
        ),
        (
            "invoice_type",
            "UPDATE verifactu_records SET invoice_type = 'F2' WHERE id = 1;",
        ),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nestadmin.db");
        two_records(&path);
        tamper(&path, sql);
        let mut store = Store::open(&path).unwrap();
        match store.verify_verifactu_chain().unwrap() {
            ChainStatus::Broken { at_id, reason } => {
                assert_eq!(at_id, 1, "{column}");
                assert!(reason.contains(column), "{column}: {reason}");
            }
            other => panic!("{column}: se esperaba una cadena rota, hay {other:?}"),
        }
        // Y bloquea la siguiente emisión.
        let customer = store
            .customers(false)
            .unwrap()
            .into_iter()
            .find(|c| c.data.tax_id.is_some())
            .unwrap()
            .id;
        let third = store
            .create_sales_draft(&draft(DocumentKind::Invoice, Some(customer), "10.00"))
            .unwrap();
        let err = store
            .issue_sales_invoice_registered(third, date("2026-03-02"), date("2026-03-02"), &ctx(90))
            .unwrap_err();
        assert_eq!(err.code(), "verifactu_chain_broken", "{column}");
    }
}

#[test]
fn a_clock_ahead_blocks_until_real_time_passes_and_says_the_last_time() {
    let mut store = Store::open_in_memory().unwrap();
    let (with_nif, _) = setup(&mut store);
    let first = store
        .create_sales_draft(&draft(DocumentKind::Invoice, Some(with_nif), "100.00"))
        .unwrap();
    let second = store
        .create_sales_draft(&draft(DocumentKind::Invoice, Some(with_nif), "50.00"))
        .unwrap();
    // El primer registro se generó con el reloj una hora adelantado (11:00:00).
    issue(&mut store, first, "2026-03-01", &ctx(3600));
    // Con la hora real (10:05) se bloquea y el mensaje dice la hora del último registro.
    let err = store
        .issue_sales_invoice_registered(second, date("2026-03-01"), date("2026-03-01"), &ctx(300))
        .unwrap_err();
    assert_eq!(err.code(), CODE_CLOCK_BEHIND);
    assert!(err.to_string().contains("2026-03-01T11:00:00"), "{err}");
    assert_eq!(store.sales_document(second).unwrap().number, None);
    // Cuando la hora real alcanza la del último registro, se emite.
    assert_eq!(
        issue(&mut store, second, "2026-03-01", &ctx(3600)),
        "F-2026-0002"
    );
}

/// Base creada por la versión 11 (la de main, con el asistente) que ya tenía facturas de venta
/// emitidas: al abrirla con la versión 13 conserva los datos y gana las tablas nuevas.
#[test]
fn a_version_11_database_with_issued_invoices_migrates_to_the_current_version() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nestadmin.db");
    let (invoice_id, number);
    {
        let mut store = Store::open(&path).unwrap();
        let (with_nif, _) = setup(&mut store);
        invoice_id = store
            .create_sales_draft(&draft(DocumentKind::Invoice, Some(with_nif), "100.00"))
            .unwrap();
        number = store
            .issue_sales_document_with(
                invoice_id,
                date("2026-03-01"),
                date("2026-03-01"),
                |_, _| Ok(()),
            )
            .unwrap()
            .number
            .unwrap();
    }
    // Como la dejaría la 11: sin registros Verifactu ni número de instalación.
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch(
        "DROP TABLE sales_send_triggers; DROP TABLE sales_document_sends; DROP TABLE company_letterheads; ALTER TABLE company DROP COLUMN letterhead_sha256; ALTER TABLE company DROP COLUMN letterhead_top_mm; ALTER TABLE company DROP COLUMN letterhead_right_mm; ALTER TABLE company DROP COLUMN letterhead_bottom_mm; ALTER TABLE company DROP COLUMN letterhead_left_mm; DROP TABLE installation; DROP TABLE verifactu_records; PRAGMA user_version = 11;",
    )
    .unwrap();
    drop(conn);

    let mut store = Store::open(&path).unwrap();
    assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
    assert_eq!(SCHEMA_VERSION, 15);
    let doc = store.sales_document(invoice_id).unwrap();
    assert_eq!(doc.status, Status::Issued);
    assert_eq!(doc.number.as_deref(), Some(number.as_str()));
    assert_eq!(
        store.sales_company().unwrap().unwrap().name.as_deref(),
        Some("Taller Demo SL")
    );
    assert_eq!(store.customers(false).unwrap().len(), 2);
    assert_eq!(store.verify_verifactu_chain().unwrap(), ChainStatus::Empty);
    assert_eq!(store.installation_number().unwrap(), None);
    // Y se puede seguir facturando con registro: la serie continúa tras la factura anterior.
    let next = store
        .create_sales_draft(&draft(
            DocumentKind::Invoice,
            Some(doc.customer_id.unwrap()),
            "10.00",
        ))
        .unwrap();
    assert_eq!(
        issue(&mut store, next, "2026-03-02", &ctx(60)),
        "F-2026-0002"
    );
    assert_eq!(
        store.verify_verifactu_chain().unwrap(),
        ChainStatus::Ok { records: 1 }
    );
}
