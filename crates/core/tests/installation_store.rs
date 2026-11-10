//! Número de instalación (fase 11, migración 12). Prueba independiente de las demás. Solo datos
//! sintéticos.

use std::path::Path;

use nestadmin_core::Store;

fn raw(path: &Path) -> rusqlite::Connection {
    rusqlite::Connection::open(path).unwrap()
}

#[test]
fn a_fresh_store_has_no_installation_number() {
    let store = Store::open_in_memory().unwrap();
    assert_eq!(store.installation_number().unwrap(), None);
}

#[test]
fn the_number_is_16_uppercase_hex_and_created_once() {
    let mut store = Store::open_in_memory().unwrap();
    let first = store.ensure_installation_number("semilla-a").unwrap();
    assert_eq!(first.len(), 16);
    assert!(first
        .bytes()
        .all(|b| b.is_ascii_digit() || (b'A'..=b'F').contains(&b)));
    // Otra semilla no lo cambia: ya existe.
    assert_eq!(
        store.ensure_installation_number("semilla-b").unwrap(),
        first
    );
    assert_eq!(store.installation_number().unwrap(), Some(first));
}

#[test]
fn the_number_survives_reopening() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("n.db");
    let first = Store::open(&path)
        .unwrap()
        .ensure_installation_number("x")
        .unwrap();
    let mut store = Store::open(&path).unwrap();
    assert_eq!(store.installation_number().unwrap(), Some(first.clone()));
    assert_eq!(store.ensure_installation_number("y").unwrap(), first);
}

#[test]
fn two_connections_competing_get_the_same_number() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("n.db");
    drop(Store::open(&path).unwrap());
    let handles: Vec<_> = (0..4)
        .map(|i| {
            let path = path.clone();
            std::thread::spawn(move || {
                let mut store = Store::open(&path).unwrap();
                store
                    .ensure_installation_number(&format!("hilo-{i}"))
                    .unwrap()
            })
        })
        .collect();
    let numbers: Vec<String> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    assert!(numbers.iter().all(|n| *n == numbers[0]), "{numbers:?}");
    let count: i64 = raw(&path)
        .query_row("SELECT count(*) FROM installation", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn update_delete_and_a_second_row_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("n.db");
    let number = Store::open(&path)
        .unwrap()
        .ensure_installation_number("x")
        .unwrap();
    let conn = raw(&path);
    assert!(conn
        .execute("UPDATE installation SET number = 'AAAAAAAAAAAAAAAA'", [])
        .is_err());
    assert!(conn.execute("DELETE FROM installation", []).is_err());
    assert!(conn
        .execute(
            "INSERT INTO installation (id, number, created_at) VALUES (2, 'BBBBBBBBBBBBBBBB', 0)",
            []
        )
        .is_err());
    let kept: String = conn
        .query_row("SELECT number FROM installation", [], |r| r.get(0))
        .unwrap();
    assert_eq!(kept, number);
}
