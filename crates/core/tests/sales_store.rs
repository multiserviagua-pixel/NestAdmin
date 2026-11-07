//! Persistencia de la facturación de venta (D72–D90, migración 3). Solo datos sintéticos:
//! NIF de prueba con dígito de control válido y nombres inventados.

use std::path::Path;

use nestadmin_core::sales::{
    Company, CustomerData, DraftInput, DraftLine, RectificationDraft, SalesFilter, SalesStoreError,
};
use nestadmin_core::Store;
use nestadmin_documents::IsoDate;
use nestadmin_sales::{
    CollectionState, DocumentKind, Exemption, Frequency, IssueMode, LineInput, PaymentMethod,
    Product, RectificationKind, RectificationMethod, Status, TaxQualification, TaxRate,
};
use rust_decimal::Decimal;

fn d(text: &str) -> Decimal {
    text.parse().unwrap()
}

fn date(text: &str) -> IsoDate {
    IsoDate::parse("test", text).unwrap()
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

fn company() -> Company {
    Company {
        name: Some("Taller Demo SL".into()),
        tax_id: Some("B12345674".into()),
        address: Some("Calle Ficticia 1, 28000 Madrid".into()),
        email: Some("demo@example.test".into()),
        iban: Some("ES0000000000000000000000".into()),
        logo: None,
        logo_mime: None,
    }
}

fn customer_data(name: &str, tax_id: &str) -> CustomerData {
    CustomerData {
        name: name.into(),
        tax_id: Some(tax_id.into()),
        address: Some("Avenida Inventada 2, 08000 Barcelona".into()),
        email: Some("cliente@example.test".into()),
    }
}

/// Almacén con empresa y un cliente completo.
fn setup(store: &mut Store) -> i64 {
    store.set_sales_company(&company()).unwrap();
    store
        .create_customer(&customer_data("Cliente Uno SA", "12345678Z"))
        .unwrap()
}

fn draft(kind: DocumentKind, customer: i64, price: &str) -> DraftInput {
    let mut input = DraftInput::new(kind);
    input.customer_id = Some(customer);
    input.lines = vec![DraftLine::from(line("Servicio de montaje", "1", price))];
    input
}

fn invoice(store: &mut Store, customer: i64, price: &str) -> i64 {
    store
        .create_sales_draft(&draft(DocumentKind::Invoice, customer, price))
        .unwrap()
}

fn issue(store: &mut Store, id: i64, on: &str) -> String {
    store
        .issue_sales_document(id, date(on), date(on))
        .unwrap()
        .number
        .unwrap()
}

fn series_rows(path: &Path) -> i64 {
    rusqlite::Connection::open(path)
        .unwrap()
        .query_row("SELECT count(*) FROM series", [], |r| r.get(0))
        .unwrap()
}

fn actions(store: &Store) -> Vec<String> {
    store
        .audit_log()
        .unwrap()
        .into_iter()
        .map(|e| e.action)
        .collect()
}

#[test]
fn issuing_is_correlative_and_survives_reopening() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nestadmin.db");
    let (first, second);
    {
        let mut store = Store::open(&path).unwrap();
        let customer = setup(&mut store);
        first = invoice(&mut store, customer, "100.00");
        second = invoice(&mut store, customer, "50.00");
        assert_eq!(store.sales_document(first).unwrap().number, None);
        assert_eq!(issue(&mut store, first, "2026-03-01"), "F-2026-0001");
        assert_eq!(issue(&mut store, second, "2026-03-02"), "F-2026-0002");
    }
    let mut store = Store::open(&path).unwrap();
    let a = store.sales_document(first).unwrap();
    assert_eq!(a.number.as_deref(), Some("F-2026-0001"));
    assert_eq!(a.status, Status::Issued);
    let totals = a.totals.unwrap();
    assert_eq!(totals.total.to_string(), "121.00");
    assert_eq!(a.lines[0].amount.unwrap().to_string(), "100.00");
    assert_eq!(a.issuer.unwrap().party.tax_id.as_deref(), Some("B12345674"));
    // La numeración continúa donde quedó.
    let customer = store.customers(false).unwrap()[0].id;
    let third = invoice(&mut store, customer, "10.00");
    assert_eq!(issue(&mut store, third, "2026-03-03"), "F-2026-0003");
    // Otro tipo y otro año son series distintas (D73).
    let quote = store
        .create_sales_draft(&draft(DocumentKind::Quote, customer, "10.00"))
        .unwrap();
    assert_eq!(issue(&mut store, quote, "2026-03-03"), "P-2026-0001");
    let next_year = invoice(&mut store, customer, "10.00");
    assert_eq!(issue(&mut store, next_year, "2027-01-02"), "F-2027-0001");
}

#[test]
fn a_cut_during_issuing_leaves_no_gap_and_no_state_change() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nestadmin.db");
    let mut store = Store::open(&path).unwrap();
    let customer = setup(&mut store);
    let id = invoice(&mut store, customer, "100.00");

    let error = store
        .issue_sales_document_with(id, date("2026-03-01"), date("2026-03-01"), |tx, record| {
            // El gancho ve el registro de la factura y escribe en la misma transacción.
            let record = record.expect("las facturas entregan datos de registro");
            assert_eq!(record.number.to_string(), "F-2026-0001");
            tx.execute(
                "INSERT INTO audit_log (at, action, status) VALUES (0, 'hook', 'ok')",
                [],
            )?;
            Err(SalesStoreError::External {
                code: "unavailable".into(),
                message: "corte simulado".into(),
            })
        })
        .unwrap_err();
    assert_eq!(error.code(), "unavailable");

    let after = store.sales_document(id).unwrap();
    assert_eq!((after.status, after.number), (Status::Draft, None));
    assert!(after.lines[0].amount.is_none());
    assert_eq!(series_rows(&path), 0, "no se consumió ningún número");
    assert!(
        !actions(&store)
            .iter()
            .any(|a| a == "hook" || a == "sales.issue"),
        "el gancho y el historial se deshacen con la transacción"
    );

    // La siguiente emisión toma el mismo número.
    assert_eq!(issue(&mut store, id, "2026-03-01"), "F-2026-0001");
    assert_eq!(actions(&store), vec!["sales.issue"]);
}

#[test]
fn a_date_before_the_last_one_of_the_series_is_rejected_without_consuming_a_number() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nestadmin.db");
    let mut store = Store::open(&path).unwrap();
    let customer = setup(&mut store);
    let a = invoice(&mut store, customer, "10.00");
    let b = invoice(&mut store, customer, "10.00");
    issue(&mut store, a, "2026-03-10");

    let error = store
        .issue_sales_document(b, date("2026-03-09"), date("2026-03-09"))
        .unwrap_err();
    assert!(matches!(error, SalesStoreError::Series(_)), "{error}");
    assert_eq!(error.code(), "conflict");
    assert_eq!(store.sales_document(b).unwrap().status, Status::Draft);
    // El mismo día sí vale, y el número es el siguiente: no hubo hueco.
    assert_eq!(issue(&mut store, b, "2026-03-10"), "F-2026-0002");
    assert_eq!(series_rows(&path), 1);
}

#[test]
fn a_revision_cannot_be_issued_in_another_year() {
    let mut store = Store::open_in_memory().unwrap();
    let customer = setup(&mut store);
    let quote = store
        .create_sales_draft(&draft(DocumentKind::Quote, customer, "10.00"))
        .unwrap();
    issue(&mut store, quote, "2026-12-30");
    let revision = store.revise_sales_document(quote).unwrap();
    let error = store
        .issue_sales_document(revision, date("2027-01-02"), date("2027-01-02"))
        .unwrap_err();
    assert!(matches!(error, SalesStoreError::Series(_)), "{error}");
    assert_eq!(
        store.sales_document(revision).unwrap().status,
        Status::Draft
    );
}

#[test]
fn an_invoice_missing_mandatory_data_is_rejected_without_a_number() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nestadmin.db");
    let mut store = Store::open(&path).unwrap();
    // Sin empresa configurada.
    let customer = store
        .create_customer(&customer_data("Cliente Uno SA", "12345678Z"))
        .unwrap();
    let id = invoice(&mut store, customer, "10.00");
    let error = store
        .issue_sales_document(id, date("2026-03-01"), date("2026-03-01"))
        .unwrap_err();
    assert_eq!(error.code(), "invalid_input");
    assert!(error.to_string().contains("issuer"), "{error}");

    // Cliente sin domicilio: factura completa lo exige (D80).
    store.set_sales_company(&company()).unwrap();
    let mut no_address = customer_data("Cliente Dos SL", "87654321X");
    no_address.address = None;
    let other = store.create_customer(&no_address).unwrap();
    let id2 = invoice(&mut store, other, "10.00");
    let error = store
        .issue_sales_document(id2, date("2026-03-01"), date("2026-03-01"))
        .unwrap_err();
    assert_eq!(error.code(), "invalid_input");
    assert!(error.to_string().contains("recipient.address"), "{error}");
    // Sin líneas.
    let empty = store
        .create_sales_draft(&DraftInput::new(DocumentKind::Invoice))
        .unwrap();
    assert_eq!(
        store
            .issue_sales_document(empty, date("2026-03-01"), date("2026-03-01"))
            .unwrap_err()
            .code(),
        "invalid_input"
    );
    assert_eq!(series_rows(&path), 0);
    assert!(store.audit_log().unwrap().is_empty());
}

#[test]
fn triggers_reject_direct_changes_to_issued_documents() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nestadmin.db");
    let mut store = Store::open(&path).unwrap();
    let customer = setup(&mut store);
    let id = invoice(&mut store, customer, "100.00");
    issue(&mut store, id, "2026-03-01");
    let draft_id = invoice(&mut store, customer, "5.00");

    let raw = rusqlite::Connection::open(&path).unwrap();
    for sql in [
        "UPDATE sales_documents SET totals = '{}' WHERE id = ?1",
        "UPDATE sales_documents SET total = '1.00' WHERE id = ?1",
        "UPDATE sales_documents SET amount_due = '1.00' WHERE id = ?1",
        "UPDATE sales_documents SET sequence = 9 WHERE id = ?1",
        "UPDATE sales_documents SET issue_date = '2026-01-01' WHERE id = ?1",
        "UPDATE sales_documents SET due_date = '2026-01-01' WHERE id = ?1",
        "UPDATE sales_documents SET customer_id = NULL WHERE id = ?1",
        "UPDATE sales_documents SET recipient_snapshot = '{}' WHERE id = ?1",
        "UPDATE sales_documents SET status = 'draft' WHERE id = ?1",
        "DELETE FROM sales_documents WHERE id = ?1",
        "UPDATE sales_lines SET amount = '1.00' WHERE document_id = ?1",
        "UPDATE sales_lines SET unit_price = '1' WHERE document_id = ?1",
        "DELETE FROM sales_lines WHERE document_id = ?1",
        "INSERT INTO sales_lines (document_id, position, description, quantity, unit_price, \
         discount, tax_rate) VALUES (?1, 9, 'x', '1', '1', '0', '21')",
    ] {
        assert!(raw.execute(sql, [id]).is_err(), "debía fallar: {sql}");
    }
    // El estado sí puede avanzar, y un borrador no se numera a mano.
    raw.execute(
        "UPDATE sales_documents SET status = 'sent' WHERE id = ?1",
        [id],
    )
    .unwrap();
    assert!(raw
        .execute(
            "UPDATE sales_documents SET status = 'issued' WHERE id = ?1",
            [draft_id]
        )
        .is_err());
    // Un número consumido no retrocede ni se borra.
    assert!(raw.execute("UPDATE series SET last = 0", []).is_err());
    assert!(raw.execute("DELETE FROM series", []).is_err());
    drop(raw);

    let doc = store.sales_document(id).unwrap();
    assert_eq!(doc.totals.unwrap().total.to_string(), "121.00");
    assert_eq!(doc.lines[0].amount.unwrap().to_string(), "100.00");
}

#[test]
fn issued_documents_keep_their_own_copy_of_company_customer_and_product() {
    let mut store = Store::open_in_memory().unwrap();
    let customer = setup(&mut store);
    let product = store
        .create_product(&Product {
            id: 0,
            description: "Revisión anual".into(),
            unit_price: d("80"),
            tax_rate: TaxRate::TwentyOne,
            exemption: None,
            retired: false,
        })
        .unwrap();
    let id = store
        .create_sales_draft(&draft(DocumentKind::Invoice, customer, "20.00"))
        .unwrap();
    store
        .add_line_from_product(id, product, d("2"), d("0.5"))
        .unwrap();
    let before = {
        store
            .issue_sales_document(id, date("2026-03-01"), date("2026-03-01"))
            .unwrap();
        store.sales_document(id).unwrap()
    };
    assert_eq!(before.lines[1].input.description, "Revisión anual");
    assert_eq!(before.lines[1].amount.unwrap().to_string(), "80.00");

    let mut changed = company();
    changed.name = Some("Otra Razón Social SL".into());
    changed.iban = Some("ES9999999999999999999999".into());
    store.set_sales_company(&changed).unwrap();
    store
        .update_customer(customer, &customer_data("Nombre Nuevo SA", "87654321X"))
        .unwrap();
    store
        .update_product(
            product,
            &Product {
                id: 0,
                description: "Otra cosa".into(),
                unit_price: d("999"),
                tax_rate: TaxRate::Ten,
                exemption: None,
                retired: false,
            },
        )
        .unwrap();
    store.retire_product(product).unwrap();

    assert_eq!(store.sales_document(id).unwrap(), before);
    let issuer = before.issuer.unwrap();
    assert_eq!(issuer.party.name.as_deref(), Some("Taller Demo SL"));
    assert_eq!(issuer.iban.as_deref(), Some("ES0000000000000000000000"));
    assert_eq!(
        before.recipient.unwrap().name.as_deref(),
        Some("Cliente Uno SA")
    );
    // Un producto retirado no se añade a documentos nuevos.
    let again = invoice(&mut store, customer, "1.00");
    assert!(store
        .add_line_from_product(again, product, d("1"), Decimal::ZERO)
        .is_err());
}

#[test]
fn drafts_are_editable_and_issued_documents_are_not() {
    let mut store = Store::open_in_memory().unwrap();
    let customer = setup(&mut store);
    let id = invoice(&mut store, customer, "10.00");
    let mut edited = draft(DocumentKind::Invoice, customer, "30.00");
    edited.withholding_percent = Some(d("15"));
    store.update_sales_draft(id, &edited).unwrap();
    let totals = store.sales_document(id).unwrap().totals.unwrap();
    assert_eq!(
        (totals.total.to_string(), totals.amount_due.to_string()),
        ("36.30".to_owned(), "31.80".to_owned())
    );
    store
        .issue_sales_document(id, date("2026-03-01"), date("2026-03-01"))
        .unwrap();
    let error = store.update_sales_draft(id, &edited).unwrap_err();
    assert_eq!(error.code(), "invalid_transition");
    assert!(store.delete_sales_draft(id).is_err());

    let other = invoice(&mut store, customer, "1.00");
    store.delete_sales_draft(other).unwrap();
    assert_eq!(store.sales_document(other).unwrap_err().code(), "not_found");
    // Cambiar de tipo o usar un cliente retirado se rechaza.
    let again = invoice(&mut store, customer, "1.00");
    assert!(store
        .update_sales_draft(again, &draft(DocumentKind::Quote, customer, "1.00"))
        .is_err());
    store.retire_customer(customer).unwrap();
    assert_eq!(
        store
            .create_sales_draft(&draft(DocumentKind::Invoice, customer, "1.00"))
            .unwrap_err()
            .code(),
        "invalid_input"
    );
}

#[test]
fn voiding_only_works_from_issued_and_never_reuses_the_number() {
    let mut store = Store::open_in_memory().unwrap();
    let customer = setup(&mut store);
    let (a, b, c) = (
        invoice(&mut store, customer, "10.00"),
        invoice(&mut store, customer, "10.00"),
        invoice(&mut store, customer, "10.00"),
    );
    issue(&mut store, a, "2026-03-01");
    issue(&mut store, b, "2026-03-02");

    let mut seen = Vec::new();
    let voided = store
        .void_sales_invoice_with(b, |_, doc| {
            seen.push(doc.number.clone());
            Ok(())
        })
        .unwrap();
    assert_eq!(voided.status, Status::Voided);
    assert_eq!(seen, vec![Some("F-2026-0002".to_owned())]);

    // Un fallo del gancho no anula.
    let failing = store.void_sales_invoice_with(a, |_, _| {
        Err(SalesStoreError::External {
            code: "unavailable".into(),
            message: "sin registro".into(),
        })
    });
    assert!(failing.is_err());
    assert_eq!(store.sales_document(a).unwrap().status, Status::Issued);

    // Enviada: error explícito que remite a la rectificativa (D82).
    store.transition_sales_document(a, Status::Sent).unwrap();
    let error = store.void_sales_invoice(a).unwrap_err();
    assert_eq!(error.code(), "invalid_transition");
    assert!(error.to_string().contains("rectificativa"), "{error}");
    // Un borrador tampoco se anula; el generico no anula ni emite.
    assert!(store.void_sales_invoice(c).is_err());
    assert!(store.transition_sales_document(c, Status::Issued).is_err());
    // Voided no se reutiliza.
    assert_eq!(issue(&mut store, c, "2026-03-03"), "F-2026-0003");
    assert!(store.transition_sales_document(b, Status::Issued).is_err());
    assert!(actions(&store).contains(&"sales.void".to_owned()));
}

#[test]
fn quote_and_delivery_note_transitions_follow_d74() {
    let mut store = Store::open_in_memory().unwrap();
    let customer = setup(&mut store);
    let quote = store
        .create_sales_draft(&draft(DocumentKind::Quote, customer, "10.00"))
        .unwrap();
    assert_eq!(
        store
            .transition_sales_document(quote, Status::Sent)
            .unwrap_err()
            .code(),
        "invalid_transition"
    );
    issue(&mut store, quote, "2026-03-01");
    store
        .transition_sales_document(quote, Status::Sent)
        .unwrap();
    store
        .transition_sales_document(quote, Status::Accepted)
        .unwrap();
    assert!(store
        .transition_sales_document(quote, Status::Rejected)
        .is_err());
    // La conversión tiene su propia operación.
    assert!(store
        .transition_sales_document(quote, Status::Converted)
        .is_err());
    let transitions = actions(&store)
        .iter()
        .filter(|a| *a == "sales.transition")
        .count();
    assert_eq!(transitions, 2);
}

#[test]
fn payments_partial_full_excess_and_removal_leave_a_trail() {
    let mut store = Store::open_in_memory().unwrap();
    let customer = setup(&mut store);
    let id = invoice(&mut store, customer, "100.00");
    let today = date("2026-03-20");
    // Un borrador no admite cobros.
    assert!(store
        .add_sales_payment(id, today, d("10"), PaymentMethod::Cash, today)
        .is_err());
    let mut input = draft(DocumentKind::Invoice, customer, "100.00");
    input.due_date = Some(date("2026-03-31"));
    store.update_sales_draft(id, &input).unwrap();
    store
        .issue_sales_document(id, date("2026-03-01"), date("2026-03-01"))
        .unwrap();
    assert_eq!(
        store.sales_collection(id, today).unwrap().state,
        CollectionState::Pending
    );

    let (first, summary) = store
        .add_sales_payment(id, today, d("50.00"), PaymentMethod::Transfer, today)
        .unwrap();
    assert_eq!(summary.state, CollectionState::Partial);
    assert_eq!(summary.outstanding.to_string(), "71.00");
    let (_, summary) = store
        .add_sales_payment(id, today, d("71.00"), PaymentMethod::Card, today)
        .unwrap();
    assert_eq!(summary.state, CollectionState::Paid);

    let error = store
        .add_sales_payment(id, today, d("0.01"), PaymentMethod::Cash, today)
        .unwrap_err();
    assert_eq!(error.code(), "invalid_input");
    assert_eq!(
        store.sales_payments(id).unwrap().len(),
        2,
        "el exceso no se guarda"
    );
    assert!(store
        .add_sales_payment(id, today, d("1.005"), PaymentMethod::Cash, today)
        .is_err());

    let summary = store.remove_sales_payment(first, today).unwrap();
    assert_eq!(summary.state, CollectionState::Partial);
    assert_eq!(
        store.remove_sales_payment(first, today).unwrap_err().code(),
        "not_found"
    );
    // Vencida si pasa la fecha sin cobrar del todo.
    let late = store.sales_collection(id, date("2026-04-15")).unwrap();
    assert_eq!(late.state, CollectionState::Overdue);

    let log = actions(&store);
    assert_eq!(
        log.iter().filter(|a| *a == "sales.payment_added").count(),
        2
    );
    assert_eq!(
        log.iter().filter(|a| *a == "sales.payment_removed").count(),
        1
    );
}

#[test]
fn a_differences_corrective_adjusts_the_outstanding_amount() {
    let mut store = Store::open_in_memory().unwrap();
    let customer = setup(&mut store);
    let original = invoice(&mut store, customer, "100.00");
    issue(&mut store, original, "2026-03-01");
    let today = date("2026-03-20");
    store
        .add_sales_payment(original, today, d("50.00"), PaymentMethod::Transfer, today)
        .unwrap();

    // R5 solo corresponde a simplificadas.
    assert_eq!(
        store
            .create_corrective_draft(
                original,
                RectificationKind::R5,
                RectificationMethod::Differences
            )
            .unwrap_err()
            .code(),
        "invalid_input"
    );
    let corrective = store
        .create_corrective_draft(
            original,
            RectificationKind::R1,
            RectificationMethod::Differences,
        )
        .unwrap();
    let mut input = DraftInput::new(DocumentKind::CorrectiveInvoice);
    input.customer_id = Some(customer);
    input.lines = vec![DraftLine::from(line("Descuento acordado", "-1", "10.00"))];
    store.update_sales_draft(corrective, &input).unwrap();
    // El borrador todavía no cuenta.
    assert_eq!(
        store
            .sales_collection(original, today)
            .unwrap()
            .outstanding
            .to_string(),
        "71.00"
    );
    let number = issue(&mut store, corrective, "2026-03-15");
    assert_eq!(number, "FR-2026-0001");
    let summary = store.sales_collection(original, today).unwrap();
    assert_eq!(summary.payable.to_string(), "108.90");
    assert_eq!(summary.outstanding.to_string(), "58.90");
    // La rectificativa por diferencias no se cobra por separado.
    assert!(store
        .add_sales_payment(corrective, today, d("1"), PaymentMethod::Cash, today)
        .is_err());
    assert!(store.sales_links(corrective).unwrap().contains(&(
        corrective,
        original,
        "rectifies".to_owned()
    )));
    // Y la original ya no se puede anular ni rectificar de nuevo como si nada: sigue emitida.
    assert_eq!(
        store.sales_document(original).unwrap().status,
        Status::Issued
    );
}

#[test]
fn a_substitution_corrective_takes_over_the_collection() {
    let mut store = Store::open_in_memory().unwrap();
    let customer = setup(&mut store);
    let original = invoice(&mut store, customer, "100.00");
    issue(&mut store, original, "2026-03-01");
    let corrective = store
        .create_corrective_draft(
            original,
            RectificationKind::R1,
            RectificationMethod::Substitution,
        )
        .unwrap();
    let draft_doc = store.sales_document(corrective).unwrap();
    assert_eq!(
        draft_doc.lines.len(),
        1,
        "parte de las líneas de la original"
    );
    assert_eq!(
        draft_doc
            .rectification
            .unwrap()
            .rectified_base
            .unwrap()
            .to_string(),
        "100.00"
    );
    let mut input = draft(DocumentKind::CorrectiveInvoice, customer, "80.00");
    input.rectification = None;
    store.update_sales_draft(corrective, &input).unwrap();
    issue(&mut store, corrective, "2026-03-05");
    let today = date("2026-03-20");
    assert_eq!(
        store.sales_collection(original, today).unwrap().state,
        CollectionState::Substituted
    );
    assert!(store
        .add_sales_payment(original, today, d("10"), PaymentMethod::Cash, today)
        .is_err());
    assert_eq!(
        store
            .sales_collection(corrective, today)
            .unwrap()
            .payable
            .to_string(),
        "96.80"
    );
}

#[test]
fn recurring_drafts_are_idempotent_and_overdue_ones_need_review() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nestadmin.db");
    let mut store = Store::open(&path).unwrap();
    let customer = setup(&mut store);
    let template = store
        .create_recurring_template(
            customer,
            &[line("Cuota mensual de mantenimiento", "1", "40.00")],
            Frequency::Monthly,
            date("2026-01-15"),
            None,
        )
        .unwrap();
    assert!(store
        .create_recurring_template(customer, &[], Frequency::Monthly, date("2026-01-15"), None)
        .is_err());

    let today = date("2026-03-20");
    let run = store.generate_recurring_drafts(today, |_| true).unwrap();
    assert!(run.failed.is_empty());
    let modes: Vec<_> = run.created.iter().map(|g| (g.key.period, g.mode)).collect();
    assert_eq!(
        modes,
        vec![
            (0, IssueMode::RequiresReview),
            (1, IssueMode::RequiresReview),
            (2, IssueMode::AutoIssueByRule),
        ]
    );
    let doc = store.sales_document(run.created[2].document_id).unwrap();
    assert_eq!(doc.status, Status::Draft);
    assert_eq!(doc.operation_date, Some(date("2026-03-15")));
    assert_eq!(
        doc.issue_date,
        Some(today),
        "la expedición es la de la generación"
    );
    assert_eq!(doc.recurring_template_id, Some(template));

    // Repetir (o reabrir tras un corte) no duplica.
    assert!(store
        .generate_recurring_drafts(today, |_| true)
        .unwrap()
        .created
        .is_empty());
    drop(store);
    let mut store = Store::open(&path).unwrap();
    assert!(store
        .generate_recurring_drafts(today, |_| true)
        .unwrap()
        .created
        .is_empty());
    let next = store
        .generate_recurring_drafts(date("2026-04-16"), |_| false)
        .unwrap();
    assert_eq!(next.created.len(), 1);
    assert_eq!(next.created[0].mode, IssueMode::RequiresReview);
    assert_eq!(
        store
            .list_sales_documents(&SalesFilter::default())
            .unwrap()
            .len(),
        4
    );
    // La restricción única lo garantiza también por debajo.
    let raw = rusqlite::Connection::open(&path).unwrap();
    assert!(raw
        .execute(
            "INSERT INTO sales_documents (kind, status, recurring_template_id, recurring_period, \
             created_at, updated_at) VALUES ('invoice', 'draft', ?1, 0, 0, 0)",
            [template],
        )
        .is_err());
    // Eliminar un borrador recurrente deja constancia del periodo omitido (D87): no se regenera,
    // ni en esta sesión ni tras reabrir.
    store
        .delete_sales_draft(run.created[0].document_id)
        .unwrap();
    assert!(actions(&store).contains(&"sales.recurring_skip".to_owned()));
    assert!(store
        .generate_recurring_drafts(date("2026-04-16"), |_| false)
        .unwrap()
        .created
        .is_empty());
    drop(store);
    let mut store = Store::open(&path).unwrap();
    assert!(store
        .generate_recurring_drafts(date("2026-04-16"), |_| false)
        .unwrap()
        .created
        .is_empty());
    // Una plantilla inactiva no genera.
    store.set_recurring_active(template, false).unwrap();
    assert!(store
        .generate_recurring_drafts(date("2026-09-01"), |_| false)
        .unwrap()
        .created
        .is_empty());
    assert!(actions(&store).contains(&"sales.recurring_draft".to_owned()));
}

#[test]
fn revisions_keep_the_number_and_supersede_the_previous_one() {
    let mut store = Store::open_in_memory().unwrap();
    let customer = setup(&mut store);
    let quote = store
        .create_sales_draft(&draft(DocumentKind::Quote, customer, "100.00"))
        .unwrap();
    // Un borrador no se revisa; una factura nunca.
    assert!(store.revise_sales_document(quote).is_err());
    assert_eq!(issue(&mut store, quote, "2026-03-01"), "P-2026-0001");

    let revision = store.revise_sales_document(quote).unwrap();
    assert!(
        store.revise_sales_document(quote).is_err(),
        "solo un borrador de revisión pendiente"
    );
    let rev_draft = store.sales_document(revision).unwrap();
    assert_eq!(
        (
            rev_draft.status,
            rev_draft.revision,
            rev_draft.number.as_deref()
        ),
        (Status::Draft, 2, Some("P-2026-0001"))
    );
    store
        .update_sales_draft(revision, &draft(DocumentKind::Quote, customer, "120.00"))
        .unwrap();
    let issued = store
        .issue_sales_document(revision, date("2026-03-05"), date("2026-03-05"))
        .unwrap();
    assert_eq!(issued.number.as_deref(), Some("P-2026-0001"));
    assert_eq!(issued.totals.unwrap().base_total.to_string(), "120.00");

    // La serie no avanzó y la anterior queda consultable y sustituida.
    let next = store
        .create_sales_draft(&draft(DocumentKind::Quote, customer, "1.00"))
        .unwrap();
    assert_eq!(issue(&mut store, next, "2026-03-06"), "P-2026-0002");
    let old = store.sales_document(quote).unwrap();
    assert_eq!(old.superseded_by, Some(revision));
    assert_eq!(old.totals.unwrap().base_total.to_string(), "100.00");
    assert!(store
        .transition_sales_document(quote, Status::Sent)
        .is_err());
    let current = store.list_sales_documents(&SalesFilter::default()).unwrap();
    assert_eq!(current.len(), 2);
    let all = store
        .list_sales_documents(&SalesFilter {
            include_superseded: true,
            ..SalesFilter::default()
        })
        .unwrap();
    assert_eq!(all.len(), 3);
    // La revisión sustituida no admite otra revisión.
    assert!(store.revise_sales_document(quote).is_err());
    assert!(actions(&store).contains(&"sales.revise".to_owned()));
    // Una factura nunca tiene revisiones (D81).
    let inv = invoice(&mut store, customer, "1.00");
    issue(&mut store, inv, "2026-03-07");
    assert!(store.revise_sales_document(inv).is_err());
}

#[test]
fn quotes_and_delivery_notes_convert_into_invoice_drafts() {
    let mut store = Store::open_in_memory().unwrap();
    let customer = setup(&mut store);
    let other = store
        .create_customer(&customer_data("Cliente Dos SL", "87654321X"))
        .unwrap();
    let quote = store
        .create_sales_draft(&draft(DocumentKind::Quote, customer, "100.00"))
        .unwrap();
    issue(&mut store, quote, "2026-03-01");
    assert!(
        store
            .convert_sales_documents(&[quote], DocumentKind::Invoice)
            .is_err(),
        "debe estar aceptado"
    );
    store
        .transition_sales_document(quote, Status::Accepted)
        .unwrap();
    let from_quote = store
        .convert_sales_documents(&[quote], DocumentKind::Invoice)
        .unwrap();
    assert_eq!(
        store.sales_document(quote).unwrap().status,
        Status::Converted
    );
    let converted = store.sales_document(from_quote).unwrap();
    assert_eq!(
        (converted.kind, converted.status),
        (DocumentKind::Invoice, Status::Draft)
    );
    assert_eq!(converted.lines[0].input.description, "Servicio de montaje");
    assert!(store.sales_links(quote).unwrap().contains(&(
        quote,
        from_quote,
        "converted".to_owned()
    )));
    assert!(store.delete_sales_draft(from_quote).is_err());
    assert!(store
        .convert_sales_documents(&[quote], DocumentKind::Invoice)
        .is_err());

    // Albaranes del mismo cliente en una sola factura; de otro cliente, no.
    let mut notes = Vec::new();
    for (who, on) in [
        (customer, "2026-03-02"),
        (customer, "2026-03-03"),
        (other, "2026-03-03"),
    ] {
        let n = store
            .create_sales_draft(&draft(DocumentKind::DeliveryNote, who, "10.00"))
            .unwrap();
        issue(&mut store, n, on);
        store
            .transition_sales_document(n, Status::Delivered)
            .unwrap();
        notes.push(n);
    }
    assert!(store
        .convert_sales_documents(&[notes[0], notes[2]], DocumentKind::Invoice)
        .is_err());
    let invoice_id = store
        .convert_sales_documents(&[notes[0], notes[1]], DocumentKind::Invoice)
        .unwrap();
    assert_eq!(store.sales_document(invoice_id).unwrap().lines.len(), 2);
    assert_eq!(
        store.sales_document(notes[0]).unwrap().status,
        Status::Invoiced
    );
    assert_eq!(
        store.sales_document(notes[2]).unwrap().status,
        Status::Delivered
    );
    assert_eq!(
        actions(&store)
            .iter()
            .filter(|a| *a == "sales.convert")
            .count(),
        2
    );
}

#[test]
fn zero_rate_lines_keep_their_qualification() {
    let mut store = Store::open_in_memory().unwrap();
    let customer = setup(&mut store);
    let mut input = DraftInput::new(DocumentKind::Invoice);
    input.customer_id = Some(customer);
    input.lines = vec![DraftLine::from(LineInput {
        description: "Formación exenta".into(),
        quantity: d("2"),
        unit_price: d("30.5"),
        discount: d("0.1"),
        tax_rate: TaxRate::Zero,
        exemption: Some(Exemption {
            qualification: TaxQualification::E1,
            description: Some("Art. 20 LIVA".into()),
        }),
    })];
    let id = store.create_sales_draft(&input).unwrap();
    let issued = store
        .issue_sales_document(id, date("2026-03-01"), date("2026-03-01"))
        .unwrap();
    let totals = issued.totals.unwrap();
    assert_eq!(totals.total.to_string(), "54.90");
    assert_eq!(
        issued.lines[0]
            .input
            .exemption
            .as_ref()
            .unwrap()
            .qualification,
        TaxQualification::E1
    );
    assert_eq!(issued.lines[0].input, input.lines[0].input);
}

#[test]
fn received_delivery_notes_go_from_received_to_reviewed_once() {
    let mut store = Store::open_in_memory().unwrap();
    let id = store
        .create_received_delivery_note(
            "Proveedor Demo SL",
            Some("B12345674"),
            "ALB-77/2026",
            date("2026-03-04"),
        )
        .unwrap();
    let note = store.received_delivery_note(id).unwrap();
    assert_eq!(note.supplier_number, "ALB-77/2026");
    store
        .review_received_delivery_note(id, Some("doc-1"), None)
        .unwrap();
    assert_eq!(
        store
            .received_delivery_note(id)
            .unwrap()
            .document_ref
            .as_deref(),
        Some("doc-1")
    );
    assert!(store.review_received_delivery_note(id, None, None).is_err());
    assert_eq!(store.received_delivery_notes().unwrap().len(), 1);
    assert!(store
        .create_received_delivery_note("", None, "1", date("2026-03-04"))
        .is_err());
}

#[test]
fn company_logo_must_be_png_or_jpeg() {
    let mut store = Store::open_in_memory().unwrap();
    assert!(store.sales_company().unwrap().is_none());
    let mut c = company();
    c.logo = Some(vec![1, 2, 3]);
    c.logo_mime = Some("image/gif".into());
    assert_eq!(
        store.set_sales_company(&c).unwrap_err().code(),
        "invalid_input"
    );
    c.logo_mime = Some("image/png".into());
    store.set_sales_company(&c).unwrap();
    assert_eq!(store.sales_company().unwrap().unwrap(), c);
}

// --- Revisión de D73, D74, D81, D82, D88, D90, D99 ---

fn raw(path: &Path) -> rusqlite::Connection {
    rusqlite::Connection::open(path).unwrap()
}

/// Rectificativa por diferencias con una línea de descuento, lista para emitir.
fn differences_corrective(store: &mut Store, customer: i64, original: i64) -> i64 {
    let id = store
        .create_corrective_draft(
            original,
            RectificationKind::R1,
            RectificationMethod::Differences,
        )
        .unwrap();
    let mut input = DraftInput::new(DocumentKind::CorrectiveInvoice);
    input.customer_id = Some(customer);
    input.lines = vec![DraftLine::from(line("Descuento acordado", "-1", "10.00"))];
    store.update_sales_draft(id, &input).unwrap();
    id
}

#[test]
fn a_revision_is_not_issued_when_the_previous_one_was_accepted_or_converted() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nestadmin.db");
    let mut store = Store::open(&path).unwrap();
    let customer = setup(&mut store);
    let quote = store
        .create_sales_draft(&draft(DocumentKind::Quote, customer, "100.00"))
        .unwrap();
    issue(&mut store, quote, "2026-03-01");
    store
        .transition_sales_document(quote, Status::Sent)
        .unwrap();
    let revision = store.revise_sales_document(quote).unwrap();

    // Con un borrador de revisión pendiente, la anterior no avanza ni se convierte.
    assert_eq!(
        store
            .transition_sales_document(quote, Status::Accepted)
            .unwrap_err()
            .code(),
        "conflict"
    );
    assert_eq!(
        store
            .convert_sales_documents(&[quote], DocumentKind::Invoice)
            .unwrap_err()
            .code(),
        "conflict"
    );

    // Si aun así el estado cambiara por debajo (carrera o base alterada), emitir la revisión
    // se rechaza sin consumir nada.
    raw(&path)
        .execute(
            "UPDATE sales_documents SET status = 'converted' WHERE id = ?1",
            [quote],
        )
        .unwrap();
    let error = store
        .issue_sales_document(revision, date("2026-03-05"), date("2026-03-05"))
        .unwrap_err();
    assert_eq!(error.code(), "conflict", "{error}");
    let after = store.sales_document(revision).unwrap();
    assert_eq!(after.status, Status::Draft);
    assert_eq!(store.sales_document(quote).unwrap().superseded_by, None);
}

#[test]
fn issuing_rejects_future_dates_bad_due_dates_and_revisions_dated_backwards() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nestadmin.db");
    let mut store = Store::open(&path).unwrap();
    let customer = setup(&mut store);
    let id = invoice(&mut store, customer, "10.00");
    let error = store
        .issue_sales_document(id, date("2026-03-10"), date("2026-03-09"))
        .unwrap_err();
    assert_eq!(error.code(), "invalid_input", "{error}");
    assert_eq!(series_rows(&path), 0, "no se consumió número");
    assert_eq!(store.sales_document(id).unwrap().status, Status::Draft);

    let mut input = draft(DocumentKind::Invoice, customer, "10.00");
    input.due_date = Some(date("2026-03-01"));
    store.update_sales_draft(id, &input).unwrap();
    let error = store
        .issue_sales_document(id, date("2026-03-05"), date("2026-03-05"))
        .unwrap_err();
    assert_eq!(error.code(), "invalid_input", "{error}");
    assert!(error.to_string().contains("due_date"), "{error}");
    assert_eq!(series_rows(&path), 0);
    input.due_date = Some(date("2026-03-05"));
    store.update_sales_draft(id, &input).unwrap();
    store
        .issue_sales_document(id, date("2026-03-05"), date("2026-03-05"))
        .unwrap();

    let quote = store
        .create_sales_draft(&draft(DocumentKind::Quote, customer, "10.00"))
        .unwrap();
    issue(&mut store, quote, "2026-03-10");
    let revision = store.revise_sales_document(quote).unwrap();
    let error = store
        .issue_sales_document(revision, date("2026-03-09"), date("2026-03-12"))
        .unwrap_err();
    assert_eq!(error.code(), "invalid_input", "{error}");
    store
        .issue_sales_document(revision, date("2026-03-10"), date("2026-03-12"))
        .unwrap();
}

#[test]
fn payments_of_the_original_count_for_the_substitute() {
    let mut store = Store::open_in_memory().unwrap();
    let customer = setup(&mut store);
    let original = invoice(&mut store, customer, "100.00");
    issue(&mut store, original, "2026-03-01");
    let today = date("2026-03-20");
    let (original_payment, _) = store
        .add_sales_payment(original, today, d("121.00"), PaymentMethod::Transfer, today)
        .unwrap();
    let corrective = store
        .create_corrective_draft(
            original,
            RectificationKind::R1,
            RectificationMethod::Substitution,
        )
        .unwrap();
    issue(&mut store, corrective, "2026-03-05");

    let summary = store.sales_collection(corrective, today).unwrap();
    assert_eq!(summary.state, CollectionState::Paid);
    assert_eq!(summary.paid.to_string(), "121.00");
    // Un cobro adicional en la sustituta es un exceso.
    let error = store
        .add_sales_payment(corrective, today, d("1.00"), PaymentMethod::Cash, today)
        .unwrap_err();
    assert_eq!(error.code(), "invalid_input");
    assert!(error.to_string().contains("supera"), "{error}");

    // Retirar el cobro de la original sigue permitido y vuelve a dejar pendiente la sustituta.
    store.remove_sales_payment(original_payment, today).unwrap();
    let summary = store.sales_collection(corrective, today).unwrap();
    assert_eq!(summary.state, CollectionState::Pending);
    assert_eq!(summary.outstanding.to_string(), "121.00");
}

#[test]
fn incompatible_correctives_are_rejected_when_created_and_when_issued() {
    let mut store = Store::open_in_memory().unwrap();
    let customer = setup(&mut store);

    // Sustitución en vigor: ni otra sustitución ni diferencias.
    let a = invoice(&mut store, customer, "100.00");
    issue(&mut store, a, "2026-03-01");
    let first = store
        .create_corrective_draft(a, RectificationKind::R1, RectificationMethod::Substitution)
        .unwrap();
    let second = store
        .create_corrective_draft(a, RectificationKind::R1, RectificationMethod::Substitution)
        .unwrap();
    issue(&mut store, first, "2026-03-02");
    for method in [
        RectificationMethod::Substitution,
        RectificationMethod::Differences,
    ] {
        let error = store
            .create_corrective_draft(a, RectificationKind::R1, method)
            .unwrap_err();
        assert_eq!(error.code(), "conflict", "{error}");
    }
    // El borrador creado antes tampoco se puede emitir ahora.
    let error = store
        .issue_sales_document(second, date("2026-03-03"), date("2026-03-03"))
        .unwrap_err();
    assert_eq!(error.code(), "conflict", "{error}");
    assert_eq!(store.sales_document(second).unwrap().status, Status::Draft);

    // Varias por diferencias sí; después, la sustitución no.
    let b = invoice(&mut store, customer, "100.00");
    issue(&mut store, b, "2026-03-04");
    let d1 = differences_corrective(&mut store, customer, b);
    let d2 = differences_corrective(&mut store, customer, b);
    let sub = store
        .create_corrective_draft(b, RectificationKind::R1, RectificationMethod::Substitution)
        .unwrap();
    issue(&mut store, d1, "2026-03-05");
    issue(&mut store, d2, "2026-03-05");
    assert_eq!(
        store
            .create_corrective_draft(b, RectificationKind::R1, RectificationMethod::Substitution)
            .unwrap_err()
            .code(),
        "conflict"
    );
    let error = store
        .issue_sales_document(sub, date("2026-03-06"), date("2026-03-06"))
        .unwrap_err();
    assert_eq!(error.code(), "conflict", "{error}");
    let more = differences_corrective(&mut store, customer, b);
    issue(&mut store, more, "2026-03-06");
}

#[test]
fn an_invoice_with_a_corrective_in_force_cannot_be_voided() {
    let mut store = Store::open_in_memory().unwrap();
    let customer = setup(&mut store);
    let original = invoice(&mut store, customer, "100.00");
    issue(&mut store, original, "2026-03-01");
    let corrective = differences_corrective(&mut store, customer, original);
    issue(&mut store, corrective, "2026-03-02");
    let error = store.void_sales_invoice(original).unwrap_err();
    assert_eq!(error.code(), "conflict", "{error}");
    assert_eq!(
        store.sales_document(original).unwrap().status,
        Status::Issued
    );
    // Anulada la rectificativa, la original se puede anular.
    store.void_sales_invoice(corrective).unwrap();
    assert_eq!(
        store.void_sales_invoice(original).unwrap().status,
        Status::Voided
    );
}

#[test]
fn rectified_base_and_tax_always_come_from_the_original() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nestadmin.db");
    let mut store = Store::open(&path).unwrap();
    let customer = setup(&mut store);
    let original = invoice(&mut store, customer, "100.00");
    issue(&mut store, original, "2026-03-01");
    let corrective = store
        .create_corrective_draft(
            original,
            RectificationKind::R1,
            RectificationMethod::Substitution,
        )
        .unwrap();

    // La edición solo cambia tipo y método: los importes que traiga la entrada se ignoran.
    let mut input = draft(DocumentKind::CorrectiveInvoice, customer, "80.00");
    input.rectification = Some(RectificationDraft {
        kind: RectificationKind::R2,
        method: RectificationMethod::Substitution,
        rectified_base: Some(d("5.00")),
        rectified_tax: Some(d("1.00")),
    });
    store.update_sales_draft(corrective, &input).unwrap();
    let stored = store
        .sales_document(corrective)
        .unwrap()
        .rectification
        .unwrap();
    assert_eq!(stored.kind, RectificationKind::R2);
    assert_eq!(stored.rectified_base.unwrap().to_string(), "100.00");
    assert_eq!(stored.rectified_tax.unwrap().to_string(), "21.00");

    // Y si la base se altera por debajo, al emitir vuelve a tomarse de la original.
    raw(&path)
        .execute(
            "UPDATE sales_documents SET rectified_base = '1.00', rectified_tax = '0.10' \
             WHERE id = ?1",
            [corrective],
        )
        .unwrap();
    let issued = store
        .issue_sales_document(corrective, date("2026-03-05"), date("2026-03-05"))
        .unwrap();
    let rect = issued.rectification.unwrap();
    assert_eq!(rect.rectified_base.unwrap().to_string(), "100.00");
    assert_eq!(rect.rectified_tax.unwrap().to_string(), "21.00");
}

#[test]
fn issued_documents_cite_their_own_logo_by_hash() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nestadmin.db");
    let mut store = Store::open(&path).unwrap();
    let customer = setup(&mut store);
    let mut with_logo = company();
    with_logo.logo = Some(b"abc".to_vec());
    with_logo.logo_mime = Some("image/png".into());
    store.set_sales_company(&with_logo).unwrap();

    let first = invoice(&mut store, customer, "10.00");
    store
        .issue_sales_document(first, date("2026-03-01"), date("2026-03-01"))
        .unwrap();
    let hash = store
        .sales_document(first)
        .unwrap()
        .issuer
        .unwrap()
        .logo_sha256
        .unwrap();
    assert_eq!(
        hash,
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );

    // Cambiar el logotipo después no altera lo emitido y el anterior sigue disponible.
    with_logo.logo = Some(b"otro logotipo".to_vec());
    store.set_sales_company(&with_logo).unwrap();
    let second = invoice(&mut store, customer, "10.00");
    store
        .issue_sales_document(second, date("2026-03-02"), date("2026-03-02"))
        .unwrap();
    let other = store
        .sales_document(second)
        .unwrap()
        .issuer
        .unwrap()
        .logo_sha256
        .unwrap();
    assert_ne!(other, hash);
    assert_eq!(
        store
            .sales_document(first)
            .unwrap()
            .issuer
            .unwrap()
            .logo_sha256
            .as_deref(),
        Some(hash.as_str())
    );
    assert_eq!(
        store.company_logo_by_hash(&hash).unwrap(),
        Some((b"abc".to_vec(), "image/png".to_owned()))
    );
    assert_eq!(store.company_logo_by_hash("0000").unwrap(), None);
    // Quitar el logotipo tampoco afecta a lo emitido; la tabla es de solo inserción.
    with_logo.logo = None;
    with_logo.logo_mime = None;
    store.set_sales_company(&with_logo).unwrap();
    assert!(store.company_logo_by_hash(&hash).unwrap().is_some());
    let conn = raw(&path);
    assert!(conn
        .execute("UPDATE company_logos SET mime = 'image/jpeg'", [])
        .is_err());
    assert!(conn.execute("DELETE FROM company_logos", []).is_err());
}

#[test]
fn recurring_templates_that_cannot_run_fail_alone() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nestadmin.db");
    let mut store = Store::open(&path).unwrap();
    let customer = setup(&mut store);
    let retired = store
        .create_customer(&customer_data("Cliente Retirado SL", "87654321X"))
        .unwrap();
    let lines = [line("Cuota mensual", "1", "40.00")];
    let first_day = date("2026-03-01");
    let ok = store
        .create_recurring_template(customer, &lines, Frequency::Monthly, first_day, None)
        .unwrap();
    let with_retired = store
        .create_recurring_template(retired, &lines, Frequency::Monthly, first_day, None)
        .unwrap();
    let corrupt = store
        .create_recurring_template(customer, &lines, Frequency::Monthly, first_day, None)
        .unwrap();
    store.retire_customer(retired).unwrap();
    raw(&path)
        .execute(
            "UPDATE recurring_templates SET lines = 'no es json' WHERE id = ?1",
            [corrupt],
        )
        .unwrap();

    let run = store
        .generate_recurring_drafts(date("2026-03-20"), |_| true)
        .unwrap();
    assert_eq!(run.created.len(), 1);
    assert_eq!(run.created[0].key.template_id, ok as u64);
    let failed: Vec<i64> = run.failed.iter().map(|(id, _)| *id).collect();
    assert_eq!(failed, vec![with_retired, corrupt]);

    // Una plantilla inexistente no se actualiza en silencio.
    assert_eq!(
        store
            .update_recurring_template(999, customer, &lines, Frequency::Monthly, first_day, None)
            .unwrap_err()
            .code(),
        "not_found"
    );
}

#[test]
fn a_late_recurring_draft_is_not_auto_issued_after_a_cut() {
    let mut store = Store::open_in_memory().unwrap();
    let customer = setup(&mut store);
    store
        .create_recurring_template(
            customer,
            &[line("Cuota mensual", "1", "40.00")],
            Frequency::Monthly,
            date("2026-01-15"),
            None,
        )
        .unwrap();
    let run = store
        .generate_recurring_drafts(date("2026-03-20"), |_| true)
        .unwrap();
    let (old, current) = (run.created[0].document_id, run.created[2].document_id);
    let on_time = date("2026-03-20");
    assert_eq!(
        store.recurring_issue_mode(current, on_time, true).unwrap(),
        IssueMode::AutoIssueByRule
    );
    assert_eq!(
        store.recurring_issue_mode(current, on_time, false).unwrap(),
        IssueMode::RequiresReview
    );
    assert_eq!(
        store.recurring_issue_mode(old, on_time, true).unwrap(),
        IssueMode::RequiresReview
    );
    // Tras un corte largo el periodo actual ya no es aquel.
    assert_eq!(
        store
            .recurring_issue_mode(current, date("2026-05-20"), true)
            .unwrap(),
        IssueMode::RequiresReview
    );
    // Un documento que no es recurrente es un error explícito.
    let plain = invoice(&mut store, customer, "1.00");
    assert_eq!(
        store
            .recurring_issue_mode(plain, on_time, true)
            .unwrap_err()
            .code(),
        "conflict"
    );
}

#[test]
fn a_corrective_can_be_made_for_a_customer_retired_afterwards() {
    let mut store = Store::open_in_memory().unwrap();
    let customer = setup(&mut store);
    let original = invoice(&mut store, customer, "100.00");
    issue(&mut store, original, "2026-03-01");
    store.retire_customer(customer).unwrap();

    let corrective = store
        .create_corrective_draft(
            original,
            RectificationKind::R1,
            RectificationMethod::Substitution,
        )
        .unwrap();
    // Seguir con el mismo cliente no se rechaza; cambiar a otro retirado, sí.
    let mut input = draft(DocumentKind::CorrectiveInvoice, customer, "80.00");
    input.rectification = None;
    store.update_sales_draft(corrective, &input).unwrap();
    let other = store
        .create_customer(&customer_data("Otro Cliente SL", "87654321X"))
        .unwrap();
    store.retire_customer(other).unwrap();
    input.customer_id = Some(other);
    assert_eq!(
        store
            .update_sales_draft(corrective, &input)
            .unwrap_err()
            .code(),
        "invalid_input"
    );
    assert_eq!(issue(&mut store, corrective, "2026-03-05"), "FR-2026-0001");
}

#[test]
fn triggers_guard_status_series_links_and_payments() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nestadmin.db");
    let mut store = Store::open(&path).unwrap();
    let customer = setup(&mut store);
    let paid = invoice(&mut store, customer, "10.00");
    issue(&mut store, paid, "2026-03-01");
    let today = date("2026-03-02");
    store
        .add_sales_payment(paid, today, d("5.00"), PaymentMethod::Cash, today)
        .unwrap();
    let voided = invoice(&mut store, customer, "10.00");
    issue(&mut store, voided, "2026-03-02");
    store.void_sales_invoice(voided).unwrap();
    let pending = invoice(&mut store, customer, "10.00");
    let quote = store
        .create_sales_draft(&draft(DocumentKind::Quote, customer, "10.00"))
        .unwrap();
    issue(&mut store, quote, "2026-03-03");
    let revision = store.revise_sales_document(quote).unwrap();
    store
        .issue_sales_document(revision, date("2026-03-04"), date("2026-03-04"))
        .unwrap();

    let conn = raw(&path);
    for (sql, why) in [
        (
            "INSERT INTO sales_documents (kind, status, created_at, updated_at) \
             VALUES ('invoice', 'issued', 0, 0)",
            "nace en borrador",
        ),
        (
            "UPDATE series SET last_date = '2000-01-01' WHERE kind = 'invoice'",
            "la fecha de la serie no retrocede",
        ),
        (
            "UPDATE series SET last_date = NULL WHERE kind = 'invoice'",
            "la fecha de la serie no se borra",
        ),
        (
            "UPDATE sales_documents SET superseded_by = NULL WHERE superseded_by IS NOT NULL",
            "superseded_by no se desfija",
        ),
        (
            "UPDATE sales_documents SET superseded_by = 1 WHERE superseded_by IS NOT NULL",
            "superseded_by no se reescribe",
        ),
        (
            "UPDATE sales_documents SET status = 'issued' WHERE status = 'voided'",
            "de anulada no se sale",
        ),
        (
            "UPDATE document_links SET relation = 'rectifies'",
            "los vínculos no se modifican",
        ),
        (
            "UPDATE payments SET amount = '1.00'",
            "los cobros no se modifican",
        ),
    ] {
        assert!(conn.execute(sql, []).is_err(), "debía fallar: {why}");
    }
    for document in [voided, pending] {
        assert!(
            conn.execute(
                "INSERT INTO payments (document_id, date, amount, method, created_at) \
                 VALUES (?1, '2026-03-05', '1.00', 'cash', 0)",
                [document],
            )
            .is_err(),
            "no admite cobros: {document}"
        );
    }
    // Alta y baja de cobros sí.
    conn.execute(
        "INSERT INTO payments (document_id, date, amount, method, created_at) \
         VALUES (?1, '2026-03-05', '1.00', 'cash', 0)",
        [paid],
    )
    .unwrap();
    conn.execute("DELETE FROM payments WHERE amount = '1.00'", [])
        .unwrap();
}
