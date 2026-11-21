//! Identidad de las bases, clasificación de las tablas y guardas de papel y de entorno (C1 de
//! T18b, D287, D288, D290). Solo datos sintéticos.

mod profiles_common;

use std::collections::BTreeSet;

use nestadmin_core::profiles::classify::{
    class_of, is_derived, role_guard_names, tables_of, TableClass, CLASSIFICATION,
};
use nestadmin_core::profiles::{
    create_ledger, create_principal, DbIdentity, DbRole, ProfileDirs, ProfileKind,
};
use nestadmin_core::sales::VerifactuEnvironment;
use nestadmin_core::store::{backup_path, SCHEMA_VERSION};
use nestadmin_core::{CoreError, Store};
use profiles_common::{build_legacy, count, raw, Billing, LegacySpec, SIMULATOR};
use rusqlite::Connection;

/// Tablas de una base que no son internas de SQLite ni del índice de texto derivado.
fn real_tables(conn: &Connection) -> BTreeSet<String> {
    conn.prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'")
        .unwrap()
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .map(|r| r.unwrap())
        .filter(|n| !is_derived(n))
        .collect()
}

/// Tablas sin clasificar de una base.
fn unclassified(conn: &Connection) -> Vec<String> {
    real_tables(conn)
        .into_iter()
        .filter(|t| class_of(t).is_none())
        .collect()
}

fn triggers(conn: &Connection) -> BTreeSet<String> {
    conn.prepare("SELECT name FROM sqlite_master WHERE type = 'trigger'")
        .unwrap()
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect()
}

fn error_text(r: rusqlite::Result<usize>) -> String {
    r.expect_err("se esperaba un rechazo").to_string()
}

#[test]
fn the_schema_version_is_18() {
    assert_eq!(SCHEMA_VERSION, 18);
}

#[test]
fn every_table_of_a_migrated_database_is_classified_and_every_classified_table_exists() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("x.db");
    drop(Store::open(&path).unwrap());
    let conn = raw(&path);
    assert_eq!(
        unclassified(&conn),
        Vec::<String>::new(),
        "una tabla sin clasificar: añádala a CLASSIFICATION"
    );
    let existing = real_tables(&conn);
    for (table, _) in CLASSIFICATION {
        assert!(
            existing.contains(*table),
            "{table} está clasificada y no existe"
        );
    }
    // Sin repeticiones.
    let names: BTreeSet<_> = CLASSIFICATION.iter().map(|(n, _)| *n).collect();
    assert_eq!(names.len(), CLASSIFICATION.len());
}

#[test]
fn an_unclassified_table_is_detected() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("x.db");
    drop(Store::open(&path).unwrap());
    let conn = raw(&path);
    conn.execute_batch("CREATE TABLE tabla_nueva_sin_clasificar (id INTEGER PRIMARY KEY);")
        .unwrap();
    assert_eq!(
        unclassified(&conn),
        vec!["tabla_nueva_sin_clasificar".to_owned()]
    );
}

#[test]
fn every_principal_and_ledger_table_has_its_three_role_triggers_and_no_other_has_one() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("x.db");
    drop(Store::open(&path).unwrap());
    let conn = raw(&path);
    let all = triggers(&conn);
    for class in [TableClass::Principal, TableClass::Obligado] {
        for table in tables_of(class) {
            for name in role_guard_names(table) {
                assert!(
                    all.contains(&name),
                    "{table} ({class:?}) no tiene su trigger {name}"
                );
            }
        }
    }
    let guarded: BTreeSet<String> = all
        .into_iter()
        .filter(|t| t.starts_with("role_guard_"))
        .collect();
    let expected: BTreeSet<String> = tables_of(TableClass::Principal)
        .chain(tables_of(TableClass::Obligado))
        .flat_map(role_guard_names)
        .collect();
    assert_eq!(guarded, expected);
    // Las compartidas y el andamiaje no se guardan.
    for table in ["audit_log", "database_identity", "profile_migration"] {
        for name in role_guard_names(table) {
            assert!(!guarded.contains(&name), "{table}");
        }
    }
}

#[test]
fn role_guards_also_block_updates_and_deletes_of_existing_rows() {
    // Una base con filas del otro papel (la copia de la base antigua en pruebas) y con identidad:
    // ni se modifican ni se borran.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("o.db");
    let mut store = Store::open(&path).unwrap();
    store.enqueue("email_triage", "{}").unwrap();
    drop(store);
    let conn = raw(&path);
    conn.execute(
        "INSERT INTO company (id, name, updated_at) VALUES (1, 'Empresa Ficticia SL', 1)",
        [],
    )
    .unwrap();
    // Sin identidad se puede escribir, modificar y borrar como siempre.
    conn.execute("UPDATE company SET name = 'Otra SL'", [])
        .unwrap();
    conn.execute("INSERT INTO database_identity (id, profile, role, taxpayer_id, created_at) VALUES (1, 'pruebas', 'obligado', 1, 1)", [])
        .unwrap();
    for sql in [
        "UPDATE tasks SET attempts = attempts + 1",
        "DELETE FROM tasks",
    ] {
        let message = error_text(conn.execute(sql, []));
        assert!(message.contains("wrong_database"), "{sql}: {message}");
    }
    // Lo propio del obligado sigue siendo escribible.
    conn.execute("UPDATE company SET name = 'Empresa Ficticia 2 SL'", [])
        .unwrap();

    let path = dir.path().join("p.db");
    drop(Store::open(&path).unwrap());
    let conn = raw(&path);
    conn.execute(
        "INSERT INTO company (id, name, updated_at) VALUES (1, 'Empresa Ficticia SL', 1)",
        [],
    )
    .unwrap();
    conn.execute("INSERT INTO database_identity (id, profile, role, taxpayer_id, created_at) VALUES (1, 'normal', 'principal', NULL, 1)", [])
        .unwrap();
    for sql in ["UPDATE company SET name = 'x'", "DELETE FROM company"] {
        let message = error_text(conn.execute(sql, []));
        assert!(message.contains("wrong_database"), "{sql}: {message}");
    }
}

#[test]
fn a_new_ledger_rejects_every_principal_table_and_a_new_principal_every_ledger_table() {
    let dir = tempfile::tempdir().unwrap();
    let ledger = create_ledger(&dir.path().join("o.db"), ProfileKind::Pruebas, 3).unwrap();
    let principal = create_principal(&dir.path().join("p.db"), ProfileKind::Normal).unwrap();
    drop((ledger, principal));
    let ledger = raw(&dir.path().join("o.db"));
    let principal = raw(&dir.path().join("p.db"));

    for table in tables_of(TableClass::Principal) {
        let sql = format!("INSERT INTO \"{table}\" DEFAULT VALUES");
        let message = error_text(ledger.execute(&sql, []));
        assert!(
            message.contains("wrong_database") && message.contains(table),
            "{table} en una base de obligado: {message}"
        );
        // También con `OR IGNORE` y `OR REPLACE`: el rechazo no se puede sortear.
        for verb in ["OR IGNORE", "OR REPLACE"] {
            let sql = format!("INSERT {verb} INTO \"{table}\" DEFAULT VALUES");
            let message = error_text(ledger.execute(&sql, []));
            assert!(
                message.contains("wrong_database"),
                "{verb} {table}: {message}"
            );
        }
    }
    for table in tables_of(TableClass::Obligado) {
        let sql = format!("INSERT INTO \"{table}\" DEFAULT VALUES");
        let message = error_text(principal.execute(&sql, []));
        assert!(
            message.contains("wrong_database") && message.contains(table),
            "{table} en la base principal: {message}"
        );
        let sql = format!("INSERT OR IGNORE INTO \"{table}\" DEFAULT VALUES");
        let message = error_text(principal.execute(&sql, []));
        assert!(
            message.contains("wrong_database"),
            "OR IGNORE {table}: {message}"
        );
    }

    // La auditoría es de todas las bases.
    for conn in [&ledger, &principal] {
        conn.execute(
            "INSERT INTO audit_log (at, action, status) VALUES (1, 'prueba', 'ok')",
            [],
        )
        .unwrap();
    }
}

#[test]
fn a_database_without_identity_keeps_working_as_before() {
    // La base anterior a los perfiles no tiene identidad: ni las guardas de papel ni las de
    // entorno actúan, para que siga funcionando igual hasta el traslado.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nestadmin.db");
    let mut store = Store::open(&path).unwrap();
    assert_eq!(store.identity().unwrap(), None);
    store.enqueue("email_triage", "{}").unwrap();
    drop(store);
    let conn = raw(&path);
    conn.execute(
        "INSERT INTO verifactu_environment_changes (environment, authorization, changed_at) \
         VALUES ('simulador', 'prueba', 1)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO company (id, name, updated_at) VALUES (1, 'Empresa Ficticia SL', 1)",
        [],
    )
    .unwrap();
    conn.execute("INSERT INTO rules (id, version, action_kind, conditions, active, created_at) VALUES ('r', 1, 'send_draft', '{}', 0, 1)", [])
        .unwrap();
}

#[test]
fn open_as_returns_wrong_database_for_any_other_identity() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("o.db");
    drop(create_ledger(&path, ProfileKind::Pruebas, 3).unwrap());
    let expected = DbIdentity::obligado(ProfileKind::Pruebas, 3);
    drop(Store::open_as(&path, &expected).unwrap());

    for other in [
        DbIdentity::obligado(ProfileKind::Normal, 3),
        DbIdentity::obligado(ProfileKind::Pruebas, 4),
        DbIdentity::principal(ProfileKind::Pruebas),
    ] {
        let err = Store::open_as(&path, &other).err().expect("debe fallar");
        assert!(
            matches!(err, CoreError::WrongDatabase { .. }),
            "{other:?}: {err}"
        );
        assert!(err.to_string().starts_with("wrong_database"), "{err}");
        assert!(err.to_string().contains("pruebas/obligado/3"), "{err}");
    }

    // Sin identidad (la base anterior) y ausente.
    let legacy = dir.path().join("nestadmin.db");
    drop(Store::open(&legacy).unwrap());
    let err = Store::open_as(&legacy, &expected).err().unwrap();
    assert!(err.to_string().contains("sin identidad"), "{err}");
    let err = Store::open_as(&dir.path().join("no-existe.db"), &expected)
        .err()
        .unwrap();
    assert!(err.to_string().contains("ausente"), "{err}");
    assert!(
        !dir.path().join("no-existe.db").exists(),
        "open_as no crea archivos"
    );
}

#[test]
fn open_as_does_not_migrate_or_copy_a_database_that_is_not_the_expected_one() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nestadmin.db");
    drop(Store::open(&path).unwrap());
    // Una base «anterior» (esquema 17 simulado): abrirla mal no debe migrarla ni copiarla.
    raw(&path)
        .execute_batch(concat!(
            include_str!("sql/downgrade18.sql"),
            "PRAGMA user_version = 17;"
        ))
        .unwrap();
    let err = Store::open_as(&path, &DbIdentity::principal(ProfileKind::Normal))
        .err()
        .unwrap();
    assert!(matches!(err, CoreError::WrongDatabase { .. }));
    let version: i64 = raw(&path)
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, 17);
    assert!(!backup_path(&path, 17).exists());
}

#[test]
fn identity_is_one_immutable_row_with_a_coherent_role() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("o.db");
    drop(create_ledger(&path, ProfileKind::Pruebas, 3).unwrap());
    let conn = raw(&path);
    assert!(
        error_text(conn.execute("UPDATE database_identity SET profile = 'normal'", []))
            .contains("no se modifica")
    );
    assert!(
        error_text(conn.execute("DELETE FROM database_identity", [])).contains("no se elimina")
    );
    // Una segunda fila no cabe (`id = 1`).
    conn.execute(
        "INSERT INTO database_identity (id, profile, role, taxpayer_id, created_at) \
         VALUES (2, 'normal', 'principal', NULL, 1)",
        [],
    )
    .expect_err("una sola fila");

    let path = dir.path().join("q.db");
    drop(Store::open(&path).unwrap());
    let conn = raw(&path);
    for (role, taxpayer) in [("principal", "7"), ("obligado", "NULL"), ("obligado", "0")] {
        conn.execute(
            &format!(
                "INSERT INTO database_identity (id, profile, role, taxpayer_id, created_at) \
                 VALUES (1, 'normal', '{role}', {taxpayer}, 1)"
            ),
            [],
        )
        .expect_err(&format!("{role}/{taxpayer} no es coherente"));
    }
}

#[test]
fn created_bases_carry_their_identity_and_installation_numbers_never_repeat() {
    let dir = tempfile::tempdir().unwrap();
    let principal = create_principal(&dir.path().join("p.db"), ProfileKind::Normal).unwrap();
    assert_eq!(
        principal.identity().unwrap(),
        Some(DbIdentity::principal(ProfileKind::Normal))
    );
    assert_eq!(principal.installation_number().unwrap(), None);
    let a = create_ledger(&dir.path().join("a.db"), ProfileKind::Normal, 1).unwrap();
    let b = create_ledger(&dir.path().join("b.db"), ProfileKind::Normal, 2).unwrap();
    let a_id = a.identity().unwrap().unwrap();
    assert_eq!(a_id.role, DbRole::Obligado);
    assert_eq!(a_id.taxpayer_id, Some(1));
    let (na, nb) = (
        a.installation_number().unwrap().unwrap(),
        b.installation_number().unwrap().unwrap(),
    );
    assert_ne!(na, nb);
    assert_eq!(na.len(), 16);
    // Una base que ya existe no se pisa.
    drop((principal, a, b));
    assert!(create_ledger(&dir.path().join("a.db"), ProfileKind::Normal, 1).is_err());
    // No quedan archivos a medias.
    let leftovers: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".part") || n.ends_with("-wal") || n.ends_with("-shm"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

#[test]
fn installation_numbers_from_the_same_seed_are_all_different_d291() {
    let mut seen = BTreeSet::new();
    for _ in 0..200 {
        let mut store = Store::open_in_memory().unwrap();
        let number = store.ensure_installation_number("misma-semilla").unwrap();
        assert_eq!(number.len(), 16);
        assert!(number
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'A'..=b'F').contains(&b)));
        assert!(seen.insert(number), "número repetido");
    }
}

#[test]
fn environments_follow_the_profile() {
    let dir = tempfile::tempdir().unwrap();
    let test_path = dir.path().join("pruebas.db");
    let normal_path = dir.path().join("normal.db");
    let test_ledger = create_ledger(&test_path, ProfileKind::Pruebas, 1).unwrap();
    let normal_ledger = create_ledger(&normal_path, ProfileKind::Normal, 1).unwrap();
    // Sin filas, un obligado de pruebas parte del simulador; uno normal nace en producción
    // (D275).
    assert_eq!(
        test_ledger.verifactu_environment().unwrap(),
        VerifactuEnvironment::Simulador
    );
    assert_eq!(
        normal_ledger.verifactu_environment().unwrap(),
        VerifactuEnvironment::Produccion
    );
    drop((test_ledger, normal_ledger));
    assert_eq!(count(&test_path, "verifactu_environment_changes"), 0);
    assert_eq!(count(&normal_path, "verifactu_environment_changes"), 1);

    let insert = |conn: &Connection, env: &str| {
        conn.execute(
            "INSERT INTO verifactu_environment_changes (environment, authorization, changed_at) \
             VALUES (?1, 'prueba', 1)",
            [env],
        )
    };
    let test = raw(&test_path);
    let message = error_text(insert(&test, "produccion"));
    assert!(
        message.contains("no admite el entorno de producción"),
        "{message}"
    );
    insert(&test, "simulador").unwrap();
    insert(&test, "preproduccion").unwrap();

    let normal = raw(&normal_path);
    for env in ["simulador", "preproduccion"] {
        let message = error_text(insert(&normal, env));
        assert!(
            message.contains("solo admite el entorno de producción"),
            "{env}: {message}"
        );
    }
    insert(&normal, "produccion").unwrap();

    // Lotes y entorno de cada registro, por el mismo motivo.
    let batch = |conn: &Connection, env: &str| {
        conn.execute(
            "INSERT INTO verifactu_batches (environment, idempotency_key, header_json, \
             record_count, authorization, started_at) VALUES (?1, ?1 || 'k', '{}', 1, 'x', 1)",
            [env],
        )
    };
    assert!(error_text(batch(&test, "produccion")).contains("no admite"));
    assert!(error_text(batch(&normal, "simulador")).contains("solo admite"));
    let record_env = |conn: &Connection, env: &str| {
        conn.execute(
            "INSERT INTO verifactu_record_environments (record_id, environment) VALUES (999, ?1)",
            [env],
        )
    };
    assert!(error_text(record_env(&test, "produccion")).contains("no admite"));
    assert!(error_text(record_env(&normal, "produccion_heredado")).contains("solo admite"));
    assert!(error_text(record_env(&normal, "simulador")).contains("solo admite"));
    // `produccion_heredado` sí cabe en pruebas: el rechazo, si lo hay, es de la clave foránea,
    // no del perfil.
    let message = error_text(record_env(&test, "produccion_heredado"));
    assert!(!message.contains("entorno de producción"), "{message}");
}

#[test]
fn the_taxpayer_register_is_insert_only_with_registrations_and_withdrawals() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = create_principal(&dir.path().join("p.db"), ProfileKind::Normal).unwrap();
    assert!(store.taxpayers().unwrap().is_empty());
    assert!(!store.has_had_multiple_taxpayers().unwrap());

    store.register_taxpayer(1, "Taller Ficticio SL").unwrap();
    assert!(!store.has_had_multiple_taxpayers().unwrap());
    store.register_taxpayer(2, "Gestoría Ficticia SL").unwrap();
    assert!(store.has_had_multiple_taxpayers().unwrap());
    store.retire_taxpayer(2).unwrap();
    // La baja no borra nada: el indicador de varios obligados sigue en `S` (D274).
    assert!(store.has_had_multiple_taxpayers().unwrap());
    let entries = store.taxpayers().unwrap();
    assert_eq!(entries.len(), 2);
    assert!(entries[0].active && !entries[1].active);
    assert_eq!(entries[0].location, ProfileDirs::ledger_location(1));

    // Ni altas repetidas, ni bajas repetidas o sin alta.
    assert!(store.register_taxpayer(1, "otra vez").is_err());
    assert!(store.retire_taxpayer(2).is_err());
    assert!(matches!(
        store.retire_taxpayer(9),
        Err(CoreError::Profile {
            code: "taxpayer_unknown",
            ..
        })
    ));
    drop(store);
    let conn = raw(&dir.path().join("p.db"));
    assert!(
        error_text(conn.execute("UPDATE taxpayers SET label = 'x'", [])).contains("solo inserción")
    );
    assert!(error_text(conn.execute("DELETE FROM taxpayers", [])).contains("solo inserción"));
    assert!(error_text(conn.execute(
        "INSERT INTO taxpayers (taxpayer_id, event, location, at) VALUES (5, 'baja', 'x', 1)",
        []
    ))
    .contains("no está de alta"));
}

#[test]
fn a_ledger_cannot_register_taxpayers() {
    let dir = tempfile::tempdir().unwrap();
    let mut ledger = create_ledger(&dir.path().join("o.db"), ProfileKind::Normal, 1).unwrap();
    let err = ledger.register_taxpayer(1, "x").err().unwrap();
    assert!(err.to_string().contains("wrong_database"), "{err}");
}

/// Las tablas de envíos de la migración 15, tal como las dejaba, para reconstruir una base E5/D1.
const MIGRATION_15: &str = include_str!("../src/migrations/015_sales_document_sends.sql");

#[test]
fn migration_18_over_a_database_with_sends_keeps_every_count_and_uses_taxpayer_1() {
    let dir = tempfile::tempdir().unwrap();
    let facts = build_legacy(dir.path(), SIMULATOR);
    let path = dir.path().join("nestadmin.db");
    let sends_before = count(&path, "sales_document_sends");
    let triggers_before = count(&path, "sales_send_triggers");
    assert_eq!((sends_before, triggers_before), (1, 1));

    // La deja como la dejaba E5/D1 (esquema 17): sin las tablas nuevas y con los envíos y los
    // disparos de la migración 15, con su clave foránea hacia los documentos.
    let conn = raw(&path);
    conn.execute_batch(
        "CREATE TEMP TABLE keep_sends AS SELECT id, document_id, task_id, version, \
         idempotency_key, account, account_address, recipient, subject, attachment_name, \
         attachment_size, attachment_sha256, authorization, sent_message_id, sent_at \
         FROM sales_document_sends; \
         CREATE TEMP TABLE keep_triggers AS SELECT document_id, task_id, created_at \
         FROM sales_send_triggers; \
         DROP TABLE sales_document_sends; DROP TABLE sales_send_triggers;",
    )
    .unwrap();
    conn.execute_batch(include_str!("sql/downgrade18.sql"))
        .unwrap();
    conn.execute_batch(MIGRATION_15).unwrap();
    conn.execute_batch(
        "INSERT INTO sales_document_sends SELECT * FROM keep_sends; \
         INSERT INTO sales_send_triggers SELECT * FROM keep_triggers; \
         DROP TABLE keep_sends; DROP TABLE keep_triggers; PRAGMA user_version = 17;",
    )
    .unwrap();
    // Sanidad del montaje: la clave foránea antigua existe.
    let fks: i64 = conn
        .query_row(
            "SELECT count(*) FROM pragma_foreign_key_list('sales_document_sends') \
             WHERE \"table\" = 'sales_documents'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(fks, 1);
    drop(conn);

    let store = Store::open(&path).unwrap();
    assert_eq!(store.schema_version().unwrap(), 18);
    assert!(backup_path(&path, 17).exists(), "copia previa a migrar");
    // Los recuentos se conservan, con `taxpayer_id = 1`.
    assert_eq!(count(&path, "sales_document_sends"), sends_before);
    assert_eq!(count(&path, "sales_send_triggers"), triggers_before);
    let ids = profiles_common::query_strings(
        &path,
        "SELECT taxpayer_id, document_id, task_id FROM sales_document_sends",
    );
    assert_eq!(ids.len(), 1);
    assert!(ids[0].starts_with("Integer(1)|"), "{ids:?}");
    // Las consultas de siempre siguen funcionando con ese obligado.
    let invoice = facts.invoice.unwrap();
    let sends = store.sales_document_sends(1, invoice).unwrap();
    assert_eq!(sends.len(), 1);
    assert_eq!(sends[0].task_id, facts.send_task.unwrap());
    // Sin la clave foránea hacia los documentos (D287): el documento vive en la base del obligado.
    let conn = raw(&path);
    for table in ["sales_document_sends", "sales_send_triggers"] {
        let to_documents: i64 = conn
            .query_row(
                &format!(
                    "SELECT count(*) FROM pragma_foreign_key_list('{table}') \
                     WHERE \"table\" = 'sales_documents'"
                ),
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(to_documents, 0, "{table}");
    }
    // Siguen de solo inserción y el disparo único es por `(obligado, documento)`.
    assert!(
        error_text(conn.execute("UPDATE sales_document_sends SET subject = 'x'", []))
            .contains("no se modifica")
    );
    assert!(
        error_text(conn.execute("DELETE FROM sales_document_sends", [])).contains("no se elimina")
    );
    assert!(
        error_text(conn.execute("UPDATE sales_send_triggers SET task_id = 1", []))
            .contains("no se modifica")
    );
    assert!(
        error_text(conn.execute("DELETE FROM sales_send_triggers", [])).contains("no se elimina")
    );
    let task = facts.send_task.unwrap();
    for taxpayer in [1, 2] {
        let result = conn.execute(
            "INSERT INTO sales_send_triggers (taxpayer_id, document_id, task_id, created_at) \
             VALUES (?1, ?2, ?3, 1)",
            rusqlite::params![taxpayer, invoice, task],
        );
        if taxpayer == 1 {
            assert!(
                result.is_err(),
                "ya hay un disparo del obligado 1 para ese documento"
            );
        } else {
            result.unwrap();
        }
    }
}

#[test]
fn migration_18_runs_on_empty_send_tables_and_on_a_database_without_billing() {
    let dir = tempfile::tempdir().unwrap();
    build_legacy(
        dir.path(),
        LegacySpec {
            billing: Billing::None,
            heredado: false,
        },
    );
    let path = dir.path().join("nestadmin.db");
    assert_eq!(count(&path, "sales_document_sends"), 0);
    assert_eq!(count(&path, "sales_send_triggers"), 0);
}
