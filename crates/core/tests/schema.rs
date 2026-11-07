//! Versión de esquema y copia previa a migrar (D03, D61). Datos sintéticos.

use nestadmin_core::store::{backup_path, SCHEMA_VERSION};
use nestadmin_core::{CoreError, Store};

fn user_version(path: &std::path::Path) -> i64 {
    rusqlite::Connection::open(path)
        .unwrap()
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap()
}

fn has_rules_table(path: &std::path::Path) -> bool {
    rusqlite::Connection::open(path)
        .unwrap()
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'rules'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
        == 1
}

/// Quita lo que añadió la migración 3 (los triggers caen con sus tablas), como si la base la
/// hubiera creado la versión anterior.
const DROP_SALES: &str = "DROP TABLE received_delivery_notes; DROP TABLE payments;     DROP TABLE document_links; DROP TABLE sales_lines; DROP TABLE sales_documents;     DROP TABLE recurring_templates; DROP TABLE series; DROP TABLE products;     DROP TABLE customers; DROP TABLE company;";

fn has_table(path: &std::path::Path, name: &str) -> bool {
    rusqlite::Connection::open(path)
        .unwrap()
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
            [name],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
        == 1
}

#[test]
fn new_database_has_current_schema_and_no_backup() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nestadmin.db");
    let store = Store::open(&path).unwrap();
    assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
    drop(store);
    for version in 0..=SCHEMA_VERSION {
        assert!(!backup_path(&path, version).exists());
    }
}

#[test]
fn migrating_keeps_a_copy_of_the_previous_schema() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nestadmin.db");
    drop(Store::open(&path).unwrap());
    // Simula una base creada por la versión anterior (esquema 1, sin la tabla de reglas).
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch(&format!(
        "{DROP_SALES} DROP TABLE rules; PRAGMA user_version = 1;"
    ))
    .unwrap();
    drop(conn);

    let store = Store::open(&path).unwrap();
    assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
    drop(store);

    let backup = backup_path(&path, 1);
    assert_eq!(backup.file_name().unwrap(), "nestadmin.esquema-1.db");
    assert_eq!(user_version(&backup), 1);
    assert!(!has_rules_table(&backup));
    assert!(has_rules_table(&path));
    assert!(!backup.with_extension("db.part").exists());

    // Volver atrás: la copia se abre con una compilación que conoce ese esquema.
    // Esta la migra de nuevo y deja otra copia del mismo esquema, sin fallar.
    std::fs::copy(&backup, &path).unwrap();
    drop(Store::open(&path).unwrap());
    assert_eq!(user_version(&path), SCHEMA_VERSION);
    assert_eq!(user_version(&backup), 1);
}

#[test]
fn migrating_from_schema_2_adds_sales_tables_and_keeps_a_copy() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nestadmin.db");
    drop(Store::open(&path).unwrap());
    // Base de la versión anterior (esquema 2): con reglas y sin tablas de venta.
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch(&format!("{DROP_SALES} PRAGMA user_version = 2;"))
        .unwrap();
    conn.execute(
        "INSERT INTO audit_log (at, action, status) VALUES (1, 'previa', 'ok')",
        [],
    )
    .unwrap();
    drop(conn);

    let store = Store::open(&path).unwrap();
    assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
    assert_eq!(SCHEMA_VERSION, 3);
    assert_eq!(
        store.audit_log().unwrap().len(),
        1,
        "los datos previos se conservan"
    );
    drop(store);

    let backup = backup_path(&path, 2);
    assert_eq!(backup.file_name().unwrap(), "nestadmin.esquema-2.db");
    assert_eq!(user_version(&backup), 2);
    assert!(has_rules_table(&backup));
    for table in [
        "company",
        "customers",
        "products",
        "series",
        "sales_documents",
        "sales_lines",
        "document_links",
        "payments",
        "recurring_templates",
        "received_delivery_notes",
    ] {
        assert!(
            !has_table(&backup, table),
            "{table} no debe estar en la copia"
        );
        assert!(has_table(&path, table), "{table} debe existir tras migrar");
    }
    assert!(!backup_path(&path, 1).exists());
}

#[test]
fn newer_schema_error_points_to_the_backup() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nestadmin.db");
    drop(Store::open(&path).unwrap());
    rusqlite::Connection::open(&path)
        .unwrap()
        .pragma_update(None, "user_version", SCHEMA_VERSION + 1)
        .unwrap();
    let Err(error) = Store::open(&path) else {
        panic!("una base más reciente debe rechazarse");
    };
    assert!(matches!(error, CoreError::UnsupportedSchema { .. }));
    assert!(error
        .to_string()
        .contains(&format!("nestadmin.esquema-{SCHEMA_VERSION}.db")));
    // Rechazarla no la modifica.
    assert_eq!(user_version(&path), SCHEMA_VERSION + 1);
}
