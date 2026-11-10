//! Registros Verifactu encadenados con la emisión y la anulación (fase 11, migración 11).
//! Prueba independiente de `sales_store.rs`. Solo datos sintéticos: NIF de prueba y nombres
//! inventados.

use std::path::Path;

use nestadmin_core::sales::{
    ChainStatus, Company, CustomerData, DraftInput, DraftLine, VerifactuContext, VerifactuKind,
    CODE_CHAIN_BROKEN, CODE_CLOCK_BEHIND, CODE_RECORD_INVALID,
};
use nestadmin_core::Store;
use nestadmin_documents::IsoDate;
use nestadmin_sales::{
    DocumentKind, Exemption, LineInput, RectificationKind, RectificationMethod, Status,
    TaxQualification, TaxRate,
};
use nestadmin_verifactu::{
    Calificacion, Encadenamiento, Fecha, FechaHoraHuso, Persona, RegistroFacturacion,
    SistemaInformatico, TipoFactura, TipoRectificativa,
};
use rust_decimal::Decimal;

fn d(text: &str) -> Decimal {
    text.parse().unwrap()
}

fn date(text: &str) -> IsoDate {
    IsoDate::parse("test", text).unwrap()
}

/// Sistema informático sintético (productor y NIF de prueba).
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

/// Segundos Unix de 2026-03-01 10:00:00 +01:00; el reloj de las pruebas se desplaza de aquí.
fn base_unix() -> i64 {
    FechaHoraHuso::new(Fecha::new(2026, 3, 1).unwrap(), 10, 0, 0, 60)
        .unwrap()
        .unix()
}

/// Contexto con el reloj `offset` segundos después de la base.
fn ctx(offset: i64) -> VerifactuContext {
    VerifactuContext {
        system: system(),
        now: FechaHoraHuso::desde_unix(base_unix() + offset, 60).unwrap(),
    }
}

fn line(description: &str, quantity: &str, price: &str) -> LineInput {
    LineInput {
        description: description.into(),
        quantity: d(quantity),
        unit_price: d(price),
        discount: Decimal::ZERO,
        tax_rate: TaxRate::TwentyOne,
        exemption: None,
    }
}

fn zero_line(description: &str, price: &str, qualification: TaxQualification) -> LineInput {
    LineInput {
        tax_rate: TaxRate::Zero,
        exemption: Some(Exemption {
            qualification,
            description: Some("Causa de prueba".into()),
        }),
        ..line(description, "1", price)
    }
}

fn setup(store: &mut Store) -> i64 {
    store
        .set_sales_company(&Company {
            name: Some("Taller Demo SL".into()),
            tax_id: Some("B12345674".into()),
            address: Some("Calle Ficticia 1, 28000 Madrid".into()),
            email: Some("demo@example.test".into()),
            iban: Some("ES0000000000000000000000".into()),
            logo: None,
            logo_mime: None,
        })
        .unwrap();
    store
        .create_customer(&CustomerData {
            name: "Cliente Uno SA".into(),
            tax_id: Some("12345678Z".into()),
            address: Some("Avenida Inventada 2, 08000 Barcelona".into()),
            email: Some("cliente@example.test".into()),
        })
        .unwrap()
}

fn draft_with(kind: DocumentKind, customer: Option<i64>, lines: Vec<LineInput>) -> DraftInput {
    let mut input = DraftInput::new(kind);
    input.customer_id = customer;
    input.lines = lines.into_iter().map(DraftLine::from).collect();
    input
}

fn invoice(store: &mut Store, customer: i64, price: &str) -> i64 {
    let input = draft_with(
        DocumentKind::Invoice,
        Some(customer),
        vec![line("Servicio de montaje", "1", price)],
    );
    store.create_sales_draft(&input).unwrap()
}

/// Emite con registro Verifactu y devuelve el número.
fn issue(store: &mut Store, id: i64, on: &str, ctx: &VerifactuContext) -> String {
    store
        .issue_sales_invoice_registered(id, date(on), date(on), ctx)
        .unwrap()
        .number
        .unwrap()
}

fn alta(row: &nestadmin_core::sales::VerifactuRecordRow) -> &nestadmin_verifactu::RegistroAlta {
    match &row.record {
        RegistroFacturacion::Alta(a) => a,
        other => panic!("se esperaba un alta, hay {other:?}"),
    }
}

fn raw(path: &Path) -> rusqlite::Connection {
    rusqlite::Connection::open(path).unwrap()
}

fn count(path: &Path, table: &str) -> i64 {
    raw(path)
        .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}

#[test]
fn a_fresh_store_has_an_empty_chain() {
    let store = Store::open_in_memory().unwrap();
    assert!(store.verifactu_records().unwrap().is_empty());
    assert_eq!(store.verify_verifactu_chain().unwrap(), ChainStatus::Empty);
}

#[test]
fn first_invoice_starts_the_chain_and_second_links_to_it() {
    let mut store = Store::open_in_memory().unwrap();
    let customer = setup(&mut store);
    let first = invoice(&mut store, customer, "100.00");
    let second = invoice(&mut store, customer, "50.00");
    assert_eq!(
        issue(&mut store, first, "2026-03-01", &ctx(0)),
        "F-2026-0001"
    );
    assert_eq!(
        issue(&mut store, second, "2026-03-01", &ctx(30)),
        "F-2026-0002"
    );

    let rows = store.verifactu_records().unwrap();
    assert_eq!(rows.len(), 2);
    let (a, b) = (&rows[0], &rows[1]);
    assert_eq!(a.kind, VerifactuKind::Alta);
    assert_eq!(a.document_id, first);
    assert_eq!(a.invoice_number, "F-2026-0001");
    assert_eq!(a.issuer_tax_id, "B12345674");
    assert_eq!(a.issue_date, "01-03-2026");
    assert_eq!(a.invoice_type.as_deref(), Some("F1"));
    assert_eq!(a.total.as_deref(), Some("121.00"));
    assert_eq!(a.tax_total.as_deref(), Some("21.00"));
    assert_eq!(a.generated_at, "2026-03-01T10:00:00+01:00");
    assert_eq!(a.previous_id, None);
    assert_eq!(a.previous_hash, None);
    assert_eq!(a.hash.len(), 64);
    assert!(a
        .hash
        .bytes()
        .all(|c| c.is_ascii_digit() || (b'A'..=b'F').contains(&c)));
    assert_eq!(alta(a).encadenamiento, Encadenamiento::PrimerRegistro);
    assert_eq!(alta(a).nombre_razon_emisor, "Taller Demo SL");
    assert_eq!(alta(a).sistema_informatico, system());

    assert_eq!(b.document_id, second);
    assert_eq!(b.previous_id, Some(a.id));
    assert_eq!(b.previous_hash.as_deref(), Some(a.hash.as_str()));
    match &alta(b).encadenamiento {
        Encadenamiento::RegistroAnterior { id_factura, huella } => {
            assert_eq!(id_factura.num_serie, "F-2026-0001");
            assert_eq!(id_factura.nif_emisor, "B12345674");
            assert_eq!(huella, &a.hash);
        }
        other => panic!("se esperaba RegistroAnterior, hay {other:?}"),
    }
    assert_eq!(
        store.verify_verifactu_chain().unwrap(),
        ChainStatus::Ok { records: 2 }
    );
}

#[test]
fn stored_hash_matches_the_hash_recomputed_by_the_verifactu_crate() {
    let mut store = Store::open_in_memory().unwrap();
    let customer = setup(&mut store);
    let id = invoice(&mut store, customer, "100.00");
    issue(&mut store, id, "2026-03-01", &ctx(0));
    let row = &store.verifactu_records().unwrap()[0];
    assert_eq!(row.record.calcular_huella(), row.hash);
    assert_eq!(row.record.huella(), row.hash);
    // Y el JSON guardado reconstruye el mismo registro.
    let json: String = {
        // La fila se ha leído con el JSON; reserializar da el mismo registro.
        serde_json::to_string(&row.record).unwrap()
    };
    let again: RegistroFacturacion = serde_json::from_str(&json).unwrap();
    assert_eq!(again, row.record);
}

#[test]
fn quotes_and_delivery_notes_write_no_record() {
    let mut store = Store::open_in_memory().unwrap();
    let customer = setup(&mut store);
    for kind in [DocumentKind::Quote, DocumentKind::DeliveryNote] {
        let input = draft_with(
            kind,
            Some(customer),
            vec![line("Servicio de montaje", "1", "10.00")],
        );
        let id = store.create_sales_draft(&input).unwrap();
        let issued = store
            .issue_sales_invoice_registered(id, date("2026-03-01"), date("2026-03-01"), &ctx(0))
            .unwrap();
        assert_eq!(issued.status, Status::Issued);
    }
    assert!(store.verifactu_records().unwrap().is_empty());
    assert_eq!(store.verify_verifactu_chain().unwrap(), ChainStatus::Empty);
}

#[test]
fn simplified_invoice_is_f2_without_recipient() {
    let mut store = Store::open_in_memory().unwrap();
    setup(&mut store);
    let input = draft_with(
        DocumentKind::SimplifiedInvoice,
        None,
        vec![line("Venta de mostrador", "1", "20.00")],
    );
    let id = store.create_sales_draft(&input).unwrap();
    issue(&mut store, id, "2026-03-01", &ctx(0));
    let rows = store.verifactu_records().unwrap();
    assert_eq!(rows[0].invoice_type.as_deref(), Some("F2"));
    assert_eq!(alta(&rows[0]).tipo_factura, TipoFactura::F2);
    assert!(alta(&rows[0]).destinatarios.is_empty());
}

#[test]
fn full_invoice_carries_the_recipient_and_the_breakdown() {
    let mut store = Store::open_in_memory().unwrap();
    let customer = setup(&mut store);
    let id = invoice(&mut store, customer, "100.00");
    issue(&mut store, id, "2026-03-01", &ctx(0));
    let rows = store.verifactu_records().unwrap();
    let a = alta(&rows[0]);
    assert_eq!(a.destinatarios.len(), 1);
    assert_eq!(a.destinatarios[0].nombre_razon, "Cliente Uno SA");
    assert_eq!(a.desglose.len(), 1);
    assert_eq!(a.desglose[0].calificacion, Calificacion::S1);
    assert_eq!(a.desglose[0].base_imponible.to_string(), "100.00");
    assert_eq!(
        a.desglose[0].cuota_repercutida.map(|c| c.to_string()),
        Some("21.00".to_owned())
    );
    assert_eq!(
        a.desglose[0].tipo_impositivo.map(|t| t.to_string()),
        Some("21.00".to_owned())
    );
    assert_eq!(a.desglose[0].clave_regimen.as_deref(), Some("01"));
    assert_eq!(a.cuota_total.to_string(), "21.00");
    assert_eq!(a.importe_total.to_string(), "121.00");
    assert!(a.tipo_rectificativa.is_none());
    assert!(a.facturas_rectificadas.is_empty());
}

#[test]
fn exempt_and_not_subject_lines_map_to_their_aeat_codes_without_rate_or_tax() {
    let mut store = Store::open_in_memory().unwrap();
    let customer = setup(&mut store);
    let input = draft_with(
        DocumentKind::Invoice,
        Some(customer),
        vec![
            line("Servicio gravado", "1", "100.00"),
            zero_line("Formación exenta", "30.00", TaxQualification::E1),
            zero_line("Operación no sujeta", "40.00", TaxQualification::N2),
        ],
    );
    let id = store.create_sales_draft(&input).unwrap();
    issue(&mut store, id, "2026-03-01", &ctx(0));
    let rows = store.verifactu_records().unwrap();
    let a = alta(&rows[0]);
    assert_eq!(a.desglose.len(), 3);
    let by_code = |c: Calificacion| a.desglose.iter().find(|x| x.calificacion == c).unwrap();
    assert!(by_code(Calificacion::S1).tipo_impositivo.is_some());
    let exempt = by_code(Calificacion::Exenta(1));
    assert_eq!(exempt.base_imponible.to_string(), "30.00");
    assert!(exempt.tipo_impositivo.is_none() && exempt.cuota_repercutida.is_none());
    let not_subject = by_code(Calificacion::N2);
    assert_eq!(not_subject.base_imponible.to_string(), "40.00");
    assert!(not_subject.cuota_repercutida.is_none());
    // La descripción de la operación reúne las del desglose.
    assert!(a.descripcion_operacion.contains("Causa de prueba"));
}

#[test]
fn substitution_corrective_records_rectified_amounts_and_the_rectified_invoice() {
    let mut store = Store::open_in_memory().unwrap();
    let customer = setup(&mut store);
    let original = invoice(&mut store, customer, "100.00");
    issue(&mut store, original, "2026-03-01", &ctx(0));
    let corrective = store
        .create_corrective_draft(
            original,
            RectificationKind::R1,
            RectificationMethod::Substitution,
        )
        .unwrap();
    let input = draft_with(
        DocumentKind::CorrectiveInvoice,
        Some(customer),
        vec![line("Servicio de montaje", "1", "80.00")],
    );
    store.update_sales_draft(corrective, &input).unwrap();
    assert_eq!(
        issue(&mut store, corrective, "2026-03-05", &ctx(60)),
        "FR-2026-0001"
    );

    let rows = store.verifactu_records().unwrap();
    let a = alta(&rows[1]);
    assert_eq!(a.tipo_factura, TipoFactura::R1);
    assert_eq!(a.tipo_rectificativa, Some(TipoRectificativa::Sustitucion));
    let amounts = a.importe_rectificacion.as_ref().unwrap();
    assert_eq!(amounts.base_rectificada.to_string(), "100.00");
    assert_eq!(amounts.cuota_rectificada.to_string(), "21.00");
    assert_eq!(a.facturas_rectificadas.len(), 1);
    assert_eq!(a.facturas_rectificadas[0].num_serie, "F-2026-0001");
    assert_eq!(a.facturas_rectificadas[0].nif_emisor, "B12345674");
    assert_eq!(
        a.facturas_rectificadas[0].fecha_expedicion.to_string(),
        "01-03-2026"
    );
    assert_eq!(a.importe_total.to_string(), "96.80");
    assert_eq!(
        store.verify_verifactu_chain().unwrap(),
        ChainStatus::Ok { records: 2 }
    );
}

#[test]
fn differences_corrective_has_no_rectified_amounts_but_names_the_rectified_invoice() {
    let mut store = Store::open_in_memory().unwrap();
    let customer = setup(&mut store);
    let original = invoice(&mut store, customer, "100.00");
    issue(&mut store, original, "2026-03-01", &ctx(0));
    let corrective = store
        .create_corrective_draft(
            original,
            RectificationKind::R1,
            RectificationMethod::Differences,
        )
        .unwrap();
    let input = draft_with(
        DocumentKind::CorrectiveInvoice,
        Some(customer),
        vec![line("Descuento acordado", "-1", "10.00")],
    );
    store.update_sales_draft(corrective, &input).unwrap();
    issue(&mut store, corrective, "2026-03-05", &ctx(60));

    let rows = store.verifactu_records().unwrap();
    let a = alta(&rows[1]);
    assert_eq!(a.tipo_factura, TipoFactura::R1);
    assert_eq!(a.tipo_rectificativa, Some(TipoRectificativa::Diferencias));
    assert!(a.importe_rectificacion.is_none());
    assert_eq!(a.facturas_rectificadas.len(), 1);
    assert_eq!(a.facturas_rectificadas[0].num_serie, "F-2026-0001");
    // La diferencia es negativa y se guarda con signo.
    assert_eq!(a.importe_total.to_string(), "-12.10");
    assert_eq!(rows[1].total.as_deref(), Some("-12.10"));
}

#[test]
fn voiding_writes_a_chained_cancellation_record() {
    let mut store = Store::open_in_memory().unwrap();
    let customer = setup(&mut store);
    let id = invoice(&mut store, customer, "100.00");
    issue(&mut store, id, "2026-03-01", &ctx(0));
    let voided = store.void_sales_invoice_registered(id, &ctx(120)).unwrap();
    assert_eq!(voided.status, Status::Voided);

    let rows = store.verifactu_records().unwrap();
    assert_eq!(rows.len(), 2);
    let cancel = &rows[1];
    assert_eq!(cancel.kind, VerifactuKind::Anulacion);
    assert_eq!(cancel.document_id, id);
    assert_eq!(cancel.invoice_number, "F-2026-0001");
    assert_eq!(cancel.issue_date, "01-03-2026");
    assert_eq!(cancel.invoice_type, None);
    assert_eq!(cancel.total, None);
    assert_eq!(cancel.tax_total, None);
    assert_eq!(cancel.previous_id, Some(rows[0].id));
    assert_eq!(cancel.previous_hash.as_deref(), Some(rows[0].hash.as_str()));
    match &cancel.record {
        RegistroFacturacion::Anulacion(a) => {
            assert_eq!(a.id_factura.num_serie, "F-2026-0001");
            assert_eq!(a.id_factura.nif_emisor, "B12345674");
            assert_eq!(a.encadenamiento, rows[0].record.siguiente_encadenamiento());
        }
        other => panic!("se esperaba una anulación, hay {other:?}"),
    }
    assert_eq!(
        store.verify_verifactu_chain().unwrap(),
        ChainStatus::Ok { records: 2 }
    );
}

#[test]
fn a_clock_behind_the_last_record_fails_without_consuming_a_number() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nestadmin.db");
    let mut store = Store::open(&path).unwrap();
    let customer = setup(&mut store);
    let first = invoice(&mut store, customer, "100.00");
    let second = invoice(&mut store, customer, "50.00");
    issue(&mut store, first, "2026-03-01", &ctx(1000));

    // 61 segundos antes del último registro: se rechaza.
    let err = store
        .issue_sales_invoice_registered(second, date("2026-03-01"), date("2026-03-01"), &ctx(939))
        .unwrap_err();
    assert_eq!(err.code(), CODE_CLOCK_BEHIND);
    assert_eq!(store.sales_document(second).unwrap().status, Status::Draft);
    assert_eq!(store.sales_document(second).unwrap().number, None);
    assert_eq!(count(&path, "verifactu_records"), 1);

    // Un reloj ligeramente atrasado (hasta 60 s) se admite y toma el número esperado.
    assert_eq!(
        issue(&mut store, second, "2026-03-01", &ctx(940)),
        "F-2026-0002"
    );
    assert_eq!(
        store.verify_verifactu_chain().unwrap(),
        ChainStatus::Ok { records: 2 }
    );
}

/// Quita los triggers de la tabla, solo en la prueba, para simular una base manipulada.
fn tamper(path: &Path, sql: &str) {
    let conn = raw(path);
    conn.execute_batch("DROP TRIGGER verifactu_records_no_update;")
        .unwrap();
    conn.execute_batch(sql).unwrap();
}

#[test]
fn a_tampered_chain_is_reported_broken_and_blocks_the_next_issue() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nestadmin.db");
    let (first, second);
    {
        let mut store = Store::open(&path).unwrap();
        let customer = setup(&mut store);
        first = invoice(&mut store, customer, "100.00");
        second = invoice(&mut store, customer, "50.00");
        issue(&mut store, first, "2026-03-01", &ctx(0));
        issue(&mut store, second, "2026-03-01", &ctx(30));
    }
    // Se altera el importe del último registro sin tocar su huella.
    tamper(
        &path,
        "UPDATE verifactu_records SET record_json = replace(record_json, '\"importe_total\":6050', '\"importe_total\":1') WHERE id = 2;",
    );
    let json: String = raw(&path)
        .query_row(
            "SELECT record_json FROM verifactu_records WHERE id = 2",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(
        json.contains("\"importe_total\":1"),
        "la manipulación no se aplicó: {json}"
    );

    let mut store = Store::open(&path).unwrap();
    match store.verify_verifactu_chain().unwrap() {
        ChainStatus::Broken { at_id, .. } => assert_eq!(at_id, 2),
        other => panic!("se esperaba una cadena rota, hay {other:?}"),
    }
    let customer = store.customers(false).unwrap()[0].id;
    let third = invoice(&mut store, customer, "10.00");
    let err = store
        .issue_sales_invoice_registered(third, date("2026-03-02"), date("2026-03-02"), &ctx(60))
        .unwrap_err();
    assert_eq!(err.code(), CODE_CHAIN_BROKEN);
    assert_eq!(store.sales_document(third).unwrap().status, Status::Draft);
    assert_eq!(count(&path, "verifactu_records"), 2);
    // La serie no avanzó: la última emitida sigue siendo la segunda.
    let last: i64 = raw(&path)
        .query_row("SELECT last FROM series WHERE kind = 'invoice'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(last, 2);
}

#[test]
fn tampering_an_earlier_record_also_blocks_the_next_issue() {
    // La comprobación previa recorre la cadena entera (D83), no solo el último registro.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nestadmin.db");
    {
        let mut store = Store::open(&path).unwrap();
        let customer = setup(&mut store);
        let first = invoice(&mut store, customer, "100.00");
        let second = invoice(&mut store, customer, "50.00");
        issue(&mut store, first, "2026-03-01", &ctx(0));
        issue(&mut store, second, "2026-03-01", &ctx(30));
    }
    tamper(
        &path,
        "UPDATE verifactu_records SET record_json = replace(record_json, '\"importe_total\":12100', '\"importe_total\":1') WHERE id = 1;",
    );
    let json: String = raw(&path)
        .query_row(
            "SELECT record_json FROM verifactu_records WHERE id = 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(
        json.contains("\"importe_total\":1,") || json.contains("\"importe_total\":1}"),
        "la manipulación no se aplicó: {json}"
    );

    let mut store = Store::open(&path).unwrap();
    let customer = store.customers(false).unwrap()[0].id;
    let third = invoice(&mut store, customer, "10.00");
    let err = store
        .issue_sales_invoice_registered(third, date("2026-03-02"), date("2026-03-02"), &ctx(60))
        .unwrap_err();
    assert_eq!(err.code(), CODE_CHAIN_BROKEN);
    assert_eq!(store.sales_document(third).unwrap().status, Status::Draft);
    assert_eq!(count(&path, "verifactu_records"), 2);
}

#[test]
fn a_tampered_hash_column_is_reported_broken_too() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nestadmin.db");
    {
        let mut store = Store::open(&path).unwrap();
        let customer = setup(&mut store);
        let first = invoice(&mut store, customer, "100.00");
        issue(&mut store, first, "2026-03-01", &ctx(0));
    }
    tamper(
        &path,
        &format!(
            "UPDATE verifactu_records SET hash = '{}' WHERE id = 1;",
            "A".repeat(64)
        ),
    );
    let mut store = Store::open(&path).unwrap();
    assert!(matches!(
        store.verify_verifactu_chain().unwrap(),
        ChainStatus::Broken { at_id: 1, .. }
    ));
    let customer = store.customers(false).unwrap()[0].id;
    let next = invoice(&mut store, customer, "10.00");
    let err = store
        .issue_sales_invoice_registered(next, date("2026-03-02"), date("2026-03-02"), &ctx(60))
        .unwrap_err();
    assert_eq!(err.code(), CODE_CHAIN_BROKEN);
    assert_eq!(store.sales_document(next).unwrap().number, None);
}

#[test]
fn records_cannot_be_updated_or_deleted() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nestadmin.db");
    let mut store = Store::open(&path).unwrap();
    let customer = setup(&mut store);
    let id = invoice(&mut store, customer, "100.00");
    issue(&mut store, id, "2026-03-01", &ctx(0));

    let conn = raw(&path);
    let update = conn
        .execute(
            "UPDATE verifactu_records SET invoice_number = 'X' WHERE id = 1",
            [],
        )
        .unwrap_err();
    assert!(update.to_string().contains("no se modifica"), "{update}");
    let delete = conn
        .execute("DELETE FROM verifactu_records WHERE id = 1", [])
        .unwrap_err();
    assert!(delete.to_string().contains("no se elimina"), "{delete}");
    assert_eq!(count(&path, "verifactu_records"), 1);
}

#[test]
fn the_chain_cannot_fork_or_start_twice() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nestadmin.db");
    let mut store = Store::open(&path).unwrap();
    let customer = setup(&mut store);
    let first = invoice(&mut store, customer, "100.00");
    let second = invoice(&mut store, customer, "50.00");
    issue(&mut store, first, "2026-03-01", &ctx(0));
    issue(&mut store, second, "2026-03-01", &ctx(30));

    let conn = raw(&path);
    let insert = |previous: &str, hash: &str| {
        conn.execute(
            &format!(
                "INSERT INTO verifactu_records (kind, document_id, issuer_tax_id, invoice_number, \
                 issue_date, invoice_type, total, tax_total, previous_id, previous_hash, \
                 generated_at, hash, record_json, created_at) \
                 VALUES ('alta', {first}, 'B12345674', 'F-2026-0001', '01-03-2026', 'F1', '1.00', \
                 '0.00', {previous}, {previous_hash}, '2026-03-01T10:00:00+01:00', '{hash}', '{{}}', 0)",
                previous_hash = if previous == "NULL" { "NULL" } else { "'X'" },
            ),
            [],
        )
    };
    // Un segundo sucesor del primer registro.
    let fork = insert("1", &"B".repeat(64)).unwrap_err();
    assert!(fork.to_string().contains("UNIQUE"), "{fork}");
    // Un segundo primer registro.
    let second_first = insert("NULL", &"C".repeat(64)).unwrap_err();
    assert!(
        second_first.to_string().contains("UNIQUE"),
        "{second_first}"
    );
    assert_eq!(count(&path, "verifactu_records"), 2);
}

#[test]
fn reopening_the_database_keeps_the_chain_and_continues_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nestadmin.db");
    let first_hash;
    {
        let mut store = Store::open(&path).unwrap();
        let customer = setup(&mut store);
        let id = invoice(&mut store, customer, "100.00");
        issue(&mut store, id, "2026-03-01", &ctx(0));
        first_hash = store.verifactu_records().unwrap()[0].hash.clone();
    }
    let mut store = Store::open(&path).unwrap();
    assert_eq!(
        store.verify_verifactu_chain().unwrap(),
        ChainStatus::Ok { records: 1 }
    );
    let customer = store.customers(false).unwrap()[0].id;
    let id = invoice(&mut store, customer, "20.00");
    issue(&mut store, id, "2026-03-02", &ctx(86_400));
    let rows = store.verifactu_records().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[1].previous_hash.as_deref(), Some(first_hash.as_str()));
    assert_eq!(
        store.verify_verifactu_chain().unwrap(),
        ChainStatus::Ok { records: 2 }
    );
}

#[test]
fn a_record_that_cannot_be_built_rolls_the_issue_back() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nestadmin.db");
    let mut store = Store::open(&path).unwrap();
    let customer = setup(&mut store);
    let id = invoice(&mut store, customer, "100.00");

    // Un identificador de sistema de más de dos caracteres no valida al sellar el registro.
    let mut bad = ctx(0);
    bad.system.id_sistema = "ABC".into();
    let err = store
        .issue_sales_invoice_registered(id, date("2026-03-01"), date("2026-03-01"), &bad)
        .unwrap_err();
    assert_eq!(err.code(), CODE_RECORD_INVALID);
    assert!(err.to_string().contains("IdSistemaInformatico"), "{err}");
    assert_eq!(store.sales_document(id).unwrap().status, Status::Draft);
    assert_eq!(store.sales_document(id).unwrap().number, None);
    assert_eq!(count(&path, "verifactu_records"), 0);

    // La serie no consumió el número: la siguiente emisión correcta toma el primero.
    assert_eq!(issue(&mut store, id, "2026-03-01", &ctx(0)), "F-2026-0001");
    assert_eq!(count(&path, "verifactu_records"), 1);
}

#[test]
fn plain_issue_without_registration_still_works_and_leaves_no_record() {
    // La emisión sin Verifactu sigue disponible para quien no lo conecte (p. ej. otras pruebas).
    let mut store = Store::open_in_memory().unwrap();
    let customer = setup(&mut store);
    let id = invoice(&mut store, customer, "100.00");
    store
        .issue_sales_document(id, date("2026-03-01"), date("2026-03-01"))
        .unwrap();
    assert!(store.verifactu_records().unwrap().is_empty());
}
