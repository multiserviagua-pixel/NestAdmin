//! Membrete PDF de fondo en el núcleo (E4a, D222, D223; D197): migración 13 a 14, tablas de solo
//! inserción, márgenes, copia en el emisor al emitir y que un cambio posterior no altera los
//! documentos ya emitidos. El PDF real y PDFium se prueban en `crates/app/tests/sales_pdf.rs`.
//! Solo datos sintéticos.

use nestadmin_core::sales::{Company, CustomerData, DraftInput, DraftLine, LetterheadMargins};
use nestadmin_core::store::{backup_path, SCHEMA_VERSION};
use nestadmin_core::Store;
use nestadmin_documents::IsoDate;
use nestadmin_sales::{DocumentKind, LineInput, TaxRate};
use rust_decimal::Decimal;

fn date(text: &str) -> IsoDate {
    IsoDate::parse("test", text).unwrap()
}

/// Un PDF mínimo sintético: el núcleo solo comprueba la cabecera y el tamaño.
fn pdf(tag: &str) -> Vec<u8> {
    format!("%PDF-1.4\n% membrete sintetico {tag}\n").into_bytes()
}

fn setup(store: &mut Store) -> i64 {
    store
        .set_sales_company(&Company {
            name: Some("Taller Demo SL".into()),
            tax_id: Some("B12345674".into()),
            address: Some("Calle Ficticia 1, 28000 Madrid".into()),
            email: None,
            iban: None,
            logo: None,
            logo_mime: None,
        })
        .unwrap();
    store
        .create_customer(&CustomerData {
            name: "Cliente Uno SA".into(),
            tax_id: Some("A58818501".into()),
            address: None,
            email: None,
        })
        .unwrap()
}

fn issue_quote(store: &mut Store, customer: i64) -> i64 {
    let mut input = DraftInput::new(DocumentKind::Quote);
    input.customer_id = Some(customer);
    input.lines = vec![DraftLine::from(LineInput {
        description: "Trabajo".into(),
        quantity: "1".parse::<Decimal>().unwrap(),
        unit_price: "10".parse::<Decimal>().unwrap(),
        discount: Decimal::ZERO,
        tax_rate: TaxRate::TwentyOne,
        exemption: None,
    })];
    let id = store.create_sales_draft(&input).unwrap();
    store
        .issue_sales_document(id, date("2026-03-01"), date("2026-03-01"))
        .unwrap();
    id
}

fn margins(top: u16, right: u16, bottom: u16, left: u16) -> LetterheadMargins {
    LetterheadMargins {
        top_mm: top,
        right_mm: right,
        bottom_mm: bottom,
        left_mm: left,
    }
}

#[test]
fn a_version_13_database_migrates_and_keeps_its_issued_documents() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nestadmin.db");
    let id;
    {
        let mut store = Store::open(&path).unwrap();
        let customer = setup(&mut store);
        id = issue_quote(&mut store, customer);
    }
    // Como la dejaría la versión 13: sin membretes ni columnas nuevas en la empresa.
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch(
        "DROP TABLE verifactu_authorization_halts; DROP TABLE verifactu_incidents; DROP TABLE verifactu_unsent_attempts; DROP TABLE verifactu_tls12_preferences; DROP TABLE verifactu_remission_settings; DROP TABLE verifactu_subsanations; DROP TABLE verifactu_record_results; DROP TABLE verifactu_batch_results; DROP TABLE verifactu_batch_attempts; DROP TABLE verifactu_batch_records; DROP TABLE verifactu_batches; DROP TABLE verifactu_record_environments; DROP TABLE verifactu_environment_changes; DROP TABLE sales_send_triggers; DROP TABLE sales_document_sends; \
         DROP TABLE company_letterheads; \
         ALTER TABLE company DROP COLUMN letterhead_sha256; \
         ALTER TABLE company DROP COLUMN letterhead_top_mm; \
         ALTER TABLE company DROP COLUMN letterhead_right_mm; \
         ALTER TABLE company DROP COLUMN letterhead_bottom_mm; \
         ALTER TABLE company DROP COLUMN letterhead_left_mm; \
         PRAGMA user_version = 13;",
    )
    .unwrap();
    drop(conn);

    let mut store = Store::open(&path).unwrap();
    assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
    assert_eq!(SCHEMA_VERSION, 17);
    assert!(backup_path(&path, 13).exists(), "copia previa a migrar");

    // Los datos se conservan; la empresa queda sin membrete y con los márgenes de fábrica.
    let doc = store.sales_document(id).unwrap();
    assert_eq!(doc.number.as_deref(), Some("P-2026-0001"));
    let issuer = doc.issuer.unwrap();
    assert_eq!(issuer.letterhead_sha256, None);
    assert_eq!(issuer.letterhead_margins, None);
    assert_eq!(
        store.sales_company().unwrap().unwrap().name.as_deref(),
        Some("Taller Demo SL")
    );
    assert!(store.sales_letterhead().unwrap().is_none());
    assert_eq!(
        store.sales_letterhead_margins().unwrap(),
        LetterheadMargins::default()
    );

    // Y el membrete ya funciona sobre la base migrada.
    let hash = store
        .set_sales_letterhead(&pdf("migrada"), LetterheadMargins::default())
        .unwrap();
    assert_eq!(store.sales_letterhead().unwrap().unwrap().sha256, hash);
}

#[test]
fn letterheads_are_insert_only_and_stored_once_per_content() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nestadmin.db");
    let mut store = Store::open(&path).unwrap();
    setup(&mut store);
    let first = store
        .set_sales_letterhead(&pdf("a"), LetterheadMargins::default())
        .unwrap();
    // El mismo contenido da el mismo hash y no se duplica.
    assert_eq!(
        store
            .set_sales_letterhead(&pdf("a"), margins(40, 20, 25, 20))
            .unwrap(),
        first
    );
    let second = store
        .set_sales_letterhead(&pdf("b"), LetterheadMargins::default())
        .unwrap();
    assert_ne!(first, second);
    drop(store);

    let conn = rusqlite::Connection::open(&path).unwrap();
    let count: i64 = conn
        .query_row("SELECT count(*) FROM company_letterheads", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 2);
    for sql in [
        "UPDATE company_letterheads SET data = x'00'",
        "DELETE FROM company_letterheads",
    ] {
        let err = conn.execute(sql, []).unwrap_err().to_string();
        assert!(err.contains("solo inserción"), "{sql}: {err}");
    }
}

#[test]
fn the_core_checks_what_it_can_and_the_margins() {
    let mut store = Store::open_in_memory().unwrap();
    setup(&mut store);
    let rejected = |store: &mut Store, bytes: &[u8], reason: &str| {
        let err = store
            .set_sales_letterhead(bytes, LetterheadMargins::default())
            .unwrap_err();
        assert_eq!(err.code(), "invalid_input");
        assert!(err.to_string().contains(reason), "{err}");
    };
    rejected(&mut store, b"", "vacío");
    rejected(&mut store, b"no es un pdf", "debe ser un PDF");
    let mut big = pdf("grande");
    big.resize(5 * 1024 * 1024 + 1, b' ');
    rejected(&mut store, &big, "5 MB");
    assert!(store.sales_letterhead().unwrap().is_none());

    for bad in [
        margins(101, 20, 25, 20),
        margins(35, 20, 25, 101),
        margins(35, 100, 25, 100),
        margins(100, 20, 100, 20),
    ] {
        let err = store.set_sales_letterhead_margins(bad).unwrap_err();
        assert_eq!(err.code(), "invalid_input", "{bad:?}");
    }
    // Los márgenes extremos que dejan justo el área mínima valen.
    store
        .set_sales_letterhead_margins(margins(50, 55, 97, 55))
        .unwrap();
    assert_eq!(
        store.sales_letterhead_margins().unwrap(),
        margins(50, 55, 97, 55)
    );
}

#[test]
fn issuing_copies_the_letterhead_and_later_changes_do_not_alter_issued_documents() {
    let mut store = Store::open_in_memory().unwrap();
    let customer = setup(&mut store);

    let before = issue_quote(&mut store, customer);
    store
        .set_sales_letterhead_margins(margins(40, 15, 30, 18))
        .unwrap();
    let hash_a = store
        .set_sales_letterhead(&pdf("a"), margins(40, 15, 30, 18))
        .unwrap();
    let with_a = issue_quote(&mut store, customer);
    let snapshot_a = store.sales_document(with_a).unwrap().issuer.unwrap();
    assert_eq!(
        snapshot_a.letterhead_sha256.as_deref(),
        Some(hash_a.as_str())
    );
    assert_eq!(snapshot_a.letterhead_margins, Some(margins(40, 15, 30, 18)));

    // Otro membrete y otros márgenes, y después ninguno: los emitidos no cambian.
    let hash_b = store
        .set_sales_letterhead(&pdf("b"), margins(20, 20, 20, 20))
        .unwrap();
    let with_b = issue_quote(&mut store, customer);
    store.remove_sales_letterhead().unwrap();
    assert!(store.sales_letterhead().unwrap().is_none());
    let without = issue_quote(&mut store, customer);

    let issuer = |id| store.sales_document(id).unwrap().issuer.unwrap();
    assert_eq!(issuer(before).letterhead_sha256, None);
    assert_eq!(issuer(with_a), snapshot_a);
    assert_eq!(
        issuer(with_b).letterhead_sha256.as_deref(),
        Some(hash_b.as_str())
    );
    assert_eq!(
        issuer(with_b).letterhead_margins,
        Some(margins(20, 20, 20, 20))
    );
    assert_eq!(issuer(without).letterhead_sha256, None);
    assert_eq!(issuer(without).letterhead_margins, None);

    // Los contenidos antiguos siguen disponibles por su hash, y quitar el membrete conserva los
    // márgenes para el siguiente.
    assert_eq!(store.letterhead_by_hash(&hash_a).unwrap(), Some(pdf("a")));
    assert_eq!(store.letterhead_by_hash(&hash_b).unwrap(), Some(pdf("b")));
    assert_eq!(store.letterhead_by_hash(&"0".repeat(64)).unwrap(), None);
    assert_eq!(
        store.sales_letterhead_margins().unwrap(),
        margins(20, 20, 20, 20)
    );
    // Los datos del emisor no se tocan al cambiar el membrete.
    assert_eq!(
        store.sales_company().unwrap().unwrap().name.as_deref(),
        Some("Taller Demo SL")
    );
}

#[test]
fn a_company_row_created_by_the_letterhead_alone_keeps_working() {
    let mut store = Store::open_in_memory().unwrap();
    store
        .set_sales_letterhead(&pdf("sola"), LetterheadMargins::default())
        .unwrap();
    assert!(store.sales_letterhead().unwrap().is_some());
    // Los datos del emisor se guardan después sin perder el membrete.
    store
        .set_sales_company(&Company {
            name: Some("Taller Demo SL".into()),
            tax_id: Some("B12345674".into()),
            ..Company::default()
        })
        .unwrap();
    assert!(store.sales_letterhead().unwrap().is_some());
    assert_eq!(
        store.sales_company().unwrap().unwrap().tax_id.as_deref(),
        Some("B12345674")
    );
}
