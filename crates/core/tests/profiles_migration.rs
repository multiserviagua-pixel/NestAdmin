//! Motor del traslado de la base anterior a los perfiles (C1 de T18b, D271, D272, D287-D289):
//! los casos de instalación, el marcador, el puntero y las garantías sobre la base antigua. Los
//! cortes inyectados están en `profiles_cuts.rs`. Solo datos sintéticos.

mod profiles_common;

use std::path::Path;

use std::collections::BTreeSet;

use nestadmin_core::profiles::classify::{tables_of, TableClass};
use nestadmin_core::profiles::migrate::{migrate_step_with_hook, migrate_with_hook};
use nestadmin_core::profiles::{
    finish_pending_rename, migrate, migrate_step, migration_report, principal_import_tables,
    read_pointer, startup_state, write_pointer, DbIdentity, Decision, MigrationState, ProfileDirs,
    ProfileKind, StartupState, POINTER_FILE, PRINCIPAL_EXCLUDED,
};
use nestadmin_core::sales::{ChainStatus, VerifactuEnvironment};
use nestadmin_core::{CoreError, Store};
use profiles_common::{
    build_legacy, count, dump_all, files_under, query_strings, raw, Billing, LegacySpec, SIMULATOR,
};

const HEREDADO: LegacySpec = LegacySpec {
    billing: Billing::Full,
    heredado: true,
};
const MASTERS: LegacySpec = LegacySpec {
    billing: Billing::MastersOnly,
    heredado: false,
};
const EMPTY: LegacySpec = LegacySpec {
    billing: Billing::None,
    heredado: false,
};

fn normal(dir: &Path) -> ProfileDirs {
    ProfileDirs::of(dir, ProfileKind::Normal)
}

fn pruebas(dir: &Path) -> ProfileDirs {
    ProfileDirs::of(dir, ProfileKind::Pruebas)
}

fn assert_no_leftovers(dir: &Path) {
    let bad: Vec<_> = files_under(dir)
        .into_iter()
        .filter(|f| {
            f.ends_with(".part")
                || f.ends_with(".tmp")
                || f.ends_with("-wal")
                || f.ends_with("-shm")
        })
        .collect();
    assert!(bad.is_empty(), "restos: {bad:?}");
}

fn marker(path: &Path) -> (i64, Option<String>, String) {
    raw(path)
        .query_row(
            "SELECT step, decision, chain_state FROM profile_migration WHERE id = 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap()
}

fn chain(path: &Path, identity: DbIdentity) -> ChainStatus {
    let store = Store::open_as(path, &identity).unwrap();
    store.verify_verifactu_chain().unwrap()
}

#[test]
fn a_simulator_installation_moves_whole_to_the_test_profile_and_keeps_the_masters_when_real() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path();
    let facts = build_legacy(data, SIMULATOR);
    let legacy = data.join("nestadmin.db");
    let before = dump_all(&legacy);
    let legacy_number = Store::open(&legacy).unwrap().installation_number().unwrap();
    assert_eq!(startup_state(data).unwrap(), StartupState::NeedsMigration);

    assert_eq!(
        migrate(data, Some(Decision::Real)).unwrap(),
        MigrationState::Done
    );
    assert_eq!(startup_state(data).unwrap(), StartupState::Ready);
    assert_no_leftovers(data);
    assert!(read_pointer(data).unwrap().is_some());

    // La base anterior se renombra y queda intacta salvo el marcador.
    assert!(!legacy.exists());
    let renamed = data.join("nestadmin.trasladada.db");
    assert_eq!(dump_all(&renamed), before);
    assert_eq!(marker(&renamed), (6, Some("real".into()), "ok".into()));

    // Perfil de pruebas: copia entera del obligado 1, con su cadena y sus envíos.
    let test = pruebas(data);
    let test_ledger = test.ledger_db(1);
    assert_eq!(dump_all(&test_ledger), before);
    assert_eq!(
        chain(&test_ledger, DbIdentity::obligado(ProfileKind::Pruebas, 1)),
        ChainStatus::Ok { records: 2 }
    );
    assert_eq!(count(&test_ledger, "sales_document_sends"), 1);
    assert_eq!(
        Store::open(&test_ledger)
            .unwrap()
            .installation_number()
            .unwrap(),
        legacy_number,
        "la copia conserva su número de instalación"
    );
    // Los PDF de venta viajan con ella.
    assert!(test.sales_dir(1).join("F-2026-0001.pdf").is_file());
    assert!(!data.join("documentos").join("ventas").exists());
    // Su principal de pruebas: sin tareas, sin reglas, con el alta del obligado 1.
    let test_principal = test.principal_db();
    let store = Store::open_as(
        &test_principal,
        &DbIdentity::principal(ProfileKind::Pruebas),
    )
    .unwrap();
    assert_eq!(store.taxpayers().unwrap().len(), 1);
    drop(store);
    for table in [
        "tasks",
        "rules",
        "actions",
        "search_index",
        "sales_document_sends",
    ] {
        assert_eq!(count(&test_principal, table), 0, "{table}");
    }

    // Principal normal: todo el papel principal, sin las tareas de envío de venta ni lo que
    // dependa de ellas, y con los mismos identificadores.
    let principal = normal(data).principal_db();
    let store = Store::open_as(&principal, &DbIdentity::principal(ProfileKind::Normal)).unwrap();
    assert_eq!(store.taxpayers().unwrap().len(), 1);
    drop(store);
    let send_task = facts.send_task.unwrap();
    let kept: Vec<String> = query_strings(&principal, "SELECT id FROM tasks ORDER BY id");
    let original: Vec<String> = query_strings(
        &renamed,
        "SELECT id FROM tasks WHERE kind <> 'send_sales_document' ORDER BY id",
    );
    assert_eq!(kept, original);
    assert_eq!(kept.len(), facts.mail_tasks.len());
    assert!(!kept.contains(&format!("Integer({send_task})")));
    assert_eq!(count(&principal, "sales_document_sends"), 0);
    assert_eq!(count(&principal, "sales_send_triggers"), 0);
    assert_eq!(count(&principal, "rules"), 1);
    assert_eq!(count(&principal, "search_index"), 1);
    assert_eq!(
        raw(&principal)
            .query_row(
                "SELECT count(*) FROM search_fts WHERE search_fts MATCH 'correo'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        1,
        "el índice de texto se rellena con los triggers de search_index"
    );
    assert_eq!(
        count(&principal, "actions") + count(&principal, "task_transitions"),
        count(&renamed, "actions") + non_send_transitions(&renamed)
    );
    for action in query_strings(&principal, "SELECT action FROM audit_log") {
        assert!(
            !action.contains("sales.") && !action.contains("verifactu."),
            "auditoría de ventas en la base principal: {action}"
        );
    }
    // Nada del papel de obligado en la principal.
    for table in [
        "company",
        "customers",
        "sales_documents",
        "verifactu_records",
        "installation",
    ] {
        assert_eq!(count(&principal, table), 0, "{table}");
    }

    // Obligado 1 del perfil normal: solo los datos maestros, con sus identificadores.
    let ledger = normal(data).ledger_db(1);
    let store = Store::open_as(&ledger, &DbIdentity::obligado(ProfileKind::Normal, 1)).unwrap();
    assert_eq!(
        store.verifactu_environment().unwrap(),
        VerifactuEnvironment::Produccion
    );
    let number = store.installation_number().unwrap().unwrap();
    assert_eq!(number.len(), 16);
    assert_ne!(
        Some(number),
        legacy_number,
        "número de instalación nuevo (D274)"
    );
    assert_eq!(
        store.sales_company().unwrap().unwrap().name.as_deref(),
        Some("Taller Ficticio SL")
    );
    assert_eq!(store.customers(true).unwrap().len(), 1);
    assert_eq!(store.customers(true).unwrap()[0].id, 1);
    assert_eq!(store.products(true).unwrap().len(), 1);
    drop(store);
    assert_eq!(count(&ledger, "company_logos"), 1);
    assert_eq!(
        query_strings(&ledger, "SELECT id, active FROM recurring_templates"),
        vec!["Integer(1)|Integer(0)".to_owned()],
        "las plantillas recurrentes llegan inactivas"
    );
    for table in [
        "series",
        "sales_documents",
        "sales_lines",
        "verifactu_records",
        "payments",
    ] {
        assert_eq!(count(&ledger, table), 0, "{table}");
    }
    assert_eq!(count(&ledger, "verifactu_environment_changes"), 1);
    assert_eq!(
        chain(&ledger, DbIdentity::obligado(ProfileKind::Normal, 1)),
        ChainStatus::Empty
    );
}

fn non_send_transitions(path: &Path) -> i64 {
    raw(path)
        .query_row(
            "SELECT count(*) FROM task_transitions WHERE task_id IN \
             (SELECT id FROM tasks WHERE kind <> 'send_sales_document')",
            [],
            |r| r.get(0),
        )
        .unwrap()
}

#[test]
fn records_marked_inherited_production_travel_to_the_test_profile_only() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path();
    build_legacy(data, HEREDADO);
    migrate(data, Some(Decision::Real)).unwrap();

    let envs = query_strings(
        &pruebas(data).ledger_db(1),
        "SELECT DISTINCT environment FROM verifactu_record_environments",
    );
    assert_eq!(envs, vec!["Text(\"produccion_heredado\")".to_owned()]);
    // La base normal no tiene ni un registro ni su entorno: la cadena de producción empieza de
    // cero (D272, D275).
    let ledger = normal(data).ledger_db(1);
    assert_eq!(count(&ledger, "verifactu_records"), 0);
    assert_eq!(count(&ledger, "verifactu_record_environments"), 0);
}

#[test]
fn an_installation_with_only_masters_still_copies_to_tests_and_imports_the_masters() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path();
    build_legacy(data, MASTERS);
    assert_eq!(
        migrate(data, Some(Decision::Real)).unwrap(),
        MigrationState::Done
    );

    let test_ledger = pruebas(data).ledger_db(1);
    assert_eq!(count(&test_ledger, "company"), 1);
    assert_eq!(count(&test_ledger, "customers"), 1);
    assert_eq!(count(&test_ledger, "verifactu_records"), 0);
    assert_eq!(
        chain(&test_ledger, DbIdentity::obligado(ProfileKind::Pruebas, 1)),
        ChainStatus::Empty
    );
    let ledger = normal(data).ledger_db(1);
    assert_eq!(count(&ledger, "company"), 1);
    assert_eq!(count(&ledger, "customers"), 1);
    assert_eq!(count(&ledger, "products"), 1);
    assert_eq!(count(&ledger, "recurring_templates"), 1);
    assert_no_leftovers(data);
}

#[test]
fn a_fictitious_company_stays_only_in_the_test_profile() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path();
    build_legacy(data, SIMULATOR);
    assert_eq!(
        migrate(data, Some(Decision::Ficticia)).unwrap(),
        MigrationState::Done
    );
    // El perfil normal no contiene nunca obligados ficticios (D271).
    assert!(
        !normal(data).ledgers_dir().exists(),
        "ni carpeta de obligados"
    );
    let store = Store::open_as(
        &normal(data).principal_db(),
        &DbIdentity::principal(ProfileKind::Normal),
    )
    .unwrap();
    assert!(store.taxpayers().unwrap().is_empty());
    assert!(!store.has_had_multiple_taxpayers().unwrap());
    drop(store);
    // Todo lo demás sigue como con «reales».
    assert_eq!(count(&normal(data).principal_db(), "rules"), 1);
    let test_ledger = pruebas(data).ledger_db(1);
    assert_eq!(count(&test_ledger, "verifactu_records"), 2);
    assert_eq!(
        marker(&data.join("nestadmin.trasladada.db")),
        (6, Some("ficticia".into()), "ok".into())
    );
    assert_no_leftovers(data);
}

#[test]
fn an_installation_without_billing_data_skips_the_copy_and_the_question() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path();
    let facts = build_legacy(data, EMPTY);
    // Sin pregunta: no hace falta decisión.
    assert_eq!(migrate(data, None).unwrap(), MigrationState::Done);
    assert!(!data.join("perfiles").exists(), "sin perfil de pruebas");
    assert!(!normal(data).ledgers_dir().exists());
    let principal = normal(data).principal_db();
    assert_eq!(count(&principal, "tasks"), facts.mail_tasks.len() as i64);
    assert_eq!(count(&principal, "rules"), 1);
    assert!(read_pointer(data).unwrap().is_some());
    assert!(data.join("nestadmin.trasladada.db").is_file());
    assert!(!data.join("nestadmin.db").exists());
    assert_eq!(
        marker(&data.join("nestadmin.trasladada.db")),
        (6, None, "empty".into())
    );
    assert_no_leftovers(data);
}

#[test]
fn a_new_installation_gets_an_empty_principal_database_and_the_pointer() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("datos");
    assert_eq!(startup_state(&data).unwrap(), StartupState::NewInstallation);
    assert_eq!(migrate(&data, None).unwrap(), MigrationState::Done);
    assert_eq!(startup_state(&data).unwrap(), StartupState::Ready);
    assert_eq!(
        files_under(&data),
        vec!["perfiles.json".to_owned(), "principal.db".to_owned()]
    );
    let store = Store::open_as(
        &normal(&data).principal_db(),
        &DbIdentity::principal(ProfileKind::Normal),
    )
    .unwrap();
    assert!(store.taxpayers().unwrap().is_empty());
    drop(store);
    // Repetirlo no cambia nada.
    let before = profiles_common::outcome(&data);
    assert_eq!(migrate(&data, None).unwrap(), MigrationState::Done);
    assert_eq!(migrate_step(&data, None).unwrap(), MigrationState::Done);
    assert_eq!(profiles_common::outcome(&data), before);
}

#[test]
fn the_engine_stops_at_step_3_until_it_gets_the_decision() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path();
    build_legacy(data, SIMULATOR);
    let legacy = data.join("nestadmin.db");

    // Pasos 1 y 2 sin decisión; el 3 se detiene y no avanza por mucho que se repita.
    assert_eq!(
        migrate_step(data, None).unwrap(),
        MigrationState::InProgress { completed: 1 }
    );
    assert_eq!(
        migrate_step(data, None).unwrap(),
        MigrationState::InProgress { completed: 2 }
    );
    for _ in 0..3 {
        assert_eq!(
            migrate_step(data, None).unwrap(),
            MigrationState::AwaitingDecision
        );
    }
    assert_eq!(marker(&legacy).0, 2);
    assert_eq!(
        migrate(data, None).unwrap(),
        MigrationState::AwaitingDecision
    );
    // Hasta que hay decisión no existe nada nuevo del perfil normal ni el puntero.
    assert!(read_pointer(data).unwrap().is_none());
    assert!(!normal(data).principal_db().exists());
    assert_eq!(startup_state(data).unwrap(), StartupState::NeedsMigration);
    // La copia de pruebas sí está (paso 2).
    assert!(pruebas(data).ledger_db(1).is_file());

    assert_eq!(
        migrate_step(data, Some(Decision::Ficticia)).unwrap(),
        MigrationState::InProgress { completed: 3 }
    );
    assert_eq!(marker(&legacy), (3, Some("ficticia".into()), "ok".into()));
    // Una decisión posterior distinta no cambia lo guardado.
    assert_eq!(
        migrate(data, Some(Decision::Real)).unwrap(),
        MigrationState::Done
    );
    assert_eq!(
        marker(&data.join("nestadmin.trasladada.db")).1,
        Some("ficticia".into())
    );
    assert!(!normal(data).ledgers_dir().exists());
}

#[test]
fn the_marker_only_moves_forward_and_keeps_its_counts_and_decision() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path();
    build_legacy(data, SIMULATOR);
    for _ in 0..3 {
        migrate_step(data, Some(Decision::Real)).unwrap();
    }
    let conn = raw(&data.join("nestadmin.db"));
    for sql in [
        "UPDATE profile_migration SET step = 1",
        "UPDATE profile_migration SET decision = 'ficticia'",
        "UPDATE profile_migration SET decision = NULL",
        "UPDATE profile_migration SET counts_json = '{}'",
        "UPDATE profile_migration SET chain_digest = 'otra'",
        "UPDATE profile_migration SET chain_head = NULL",
        "DELETE FROM profile_migration",
    ] {
        let message = conn.execute(sql, []).expect_err(sql).to_string();
        assert!(
            message.contains("solo avanza") || message.contains("no se elimina"),
            "{sql}: {message}"
        );
    }
    // Avanzar sí.
    conn.execute("UPDATE profile_migration SET step = 4", [])
        .unwrap();
}

#[test]
fn a_failed_integrity_check_aborts_before_anything_is_written() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path();
    build_legacy(data, SIMULATOR);
    let legacy = data.join("nestadmin.db");
    // Una fila que apunta a una tarea inexistente.
    raw(&legacy)
        .execute_batch(
            "PRAGMA foreign_keys = OFF; \
             INSERT INTO task_transitions (task_id, from_state, to_state, reason, at) \
             VALUES (99999, NULL, 'queued', 'huérfana', 1);",
        )
        .unwrap();
    let err = migrate(data, Some(Decision::Real)).err().unwrap();
    assert!(
        matches!(
            err,
            CoreError::Profile {
                code: "migration_integrity",
                ..
            }
        ),
        "{err}"
    );
    assert!(read_pointer(data).unwrap().is_none());
    assert!(!data.join("perfiles").exists());
    assert!(!normal(data).principal_db().exists());
    assert_eq!(
        count(&legacy, "profile_migration"),
        0,
        "sin marcador: no se empezó"
    );
}

#[test]
fn a_broken_chain_does_not_abort_it_is_archived_in_tests_as_it_is() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path();
    build_legacy(data, SIMULATOR);
    let legacy = data.join("nestadmin.db");
    raw(&legacy)
        .execute_batch(
            "DROP TRIGGER verifactu_records_no_update; \
             UPDATE verifactu_records SET total = '999.99' WHERE id = 1; \
             CREATE TRIGGER verifactu_records_no_update BEFORE UPDATE ON verifactu_records \
             BEGIN SELECT RAISE(ABORT, 'un registro Verifactu no se modifica'); END;",
        )
        .unwrap();
    assert_eq!(
        migrate(data, Some(Decision::Real)).unwrap(),
        MigrationState::Done
    );
    let renamed = data.join("nestadmin.trasladada.db");
    assert_eq!(marker(&renamed).2, "broken");
    let status = chain(
        &pruebas(data).ledger_db(1),
        DbIdentity::obligado(ProfileKind::Pruebas, 1),
    );
    assert!(matches!(status, ChainStatus::Broken { .. }), "{status:?}");
    // La cadena rota no pasa a la base normal.
    assert_eq!(count(&normal(data).ledger_db(1), "verifactu_records"), 0);
}

#[test]
fn leftovers_of_an_earlier_attempt_are_discarded_not_trusted() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path();
    build_legacy(data, SIMULATOR);
    std::fs::create_dir_all(pruebas(data).ledger_dir(1)).unwrap();
    std::fs::write(
        pruebas(data).ledger_dir(1).join("obligado.db.part"),
        b"basura",
    )
    .unwrap();
    std::fs::write(data.join("principal.db.part"), b"basura").unwrap();
    std::fs::write(data.join("perfiles.json.tmp"), b"{").unwrap();
    // Un `.tmp` del puntero no es un puntero.
    assert!(read_pointer(data).unwrap().is_none());
    migrate(data, Some(Decision::Real)).unwrap();
    assert_no_leftovers(data);
    assert_eq!(count(&pruebas(data).ledger_db(1), "verifactu_records"), 2);
}

#[test]
fn the_pointer_is_written_atomically_and_read_back() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path();
    assert_eq!(read_pointer(data).unwrap(), None);
    write_pointer(data).unwrap();
    assert_eq!(read_pointer(data).unwrap().unwrap().version, 1);
    assert_eq!(files_under(data), vec![POINTER_FILE.to_owned()], "sin .tmp");
    // Un puntero de otra versión o ilegible es un error, no una instalación nueva.
    std::fs::write(data.join(POINTER_FILE), br#"{"version":2}"#).unwrap();
    assert!(matches!(
        read_pointer(data),
        Err(CoreError::Profile {
            code: "pointer_invalid",
            ..
        })
    ));
    std::fs::write(data.join(POINTER_FILE), b"no es json").unwrap();
    assert!(read_pointer(data).is_err());
}

#[test]
fn profile_folders_follow_the_design() {
    let root = Path::new("datos");
    let normal = ProfileDirs::of(root, ProfileKind::Normal);
    assert_eq!(normal.root(), root);
    assert_eq!(normal.principal_db(), root.join("principal.db"));
    assert_eq!(
        normal.ledger_db(7),
        root.join("obligados").join("7").join("obligado.db")
    );
    assert_eq!(
        normal.sales_dir(7),
        root.join("obligados").join("7").join("ventas")
    );
    assert_eq!(
        normal.certificates_dir(7),
        root.join("obligados").join("7").join("certificados")
    );
    let test = ProfileDirs::of(root, ProfileKind::Pruebas);
    assert_eq!(test.root(), root.join("perfiles").join("pruebas"));
    assert_eq!(
        test.ledger_db(1),
        root.join("perfiles")
            .join("pruebas")
            .join("obligados")
            .join("1")
            .join("obligado.db")
    );
}

#[test]
fn the_normal_profile_never_gets_a_test_environment_after_the_move() {
    // Tras el traslado, el obligado normal solo admite producción y el de pruebas no la admite.
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path();
    build_legacy(data, SIMULATOR);
    migrate(data, Some(Decision::Real)).unwrap();
    let insert = |path: &Path, env: &str| {
        raw(path).execute(
            "INSERT INTO verifactu_environment_changes (environment, authorization, changed_at) \
             VALUES (?1, 'prueba', 1)",
            [env],
        )
    };
    assert!(insert(&normal(data).ledger_db(1), "simulador").is_err());
    assert!(insert(&pruebas(data).ledger_db(1), "produccion").is_err());
    insert(&pruebas(data).ledger_db(1), "preproduccion").unwrap();
}

#[test]
fn the_test_copy_never_keeps_production_as_its_current_environment() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path();
    build_legacy(data, SIMULATOR);
    raw(&data.join("nestadmin.db"))
        .execute(
            "INSERT INTO verifactu_environment_changes (environment, authorization, changed_at) \
             VALUES ('produccion', 'prueba', 1)",
            [],
        )
        .unwrap();
    migrate(data, Some(Decision::Real)).unwrap();
    let test_ledger = pruebas(data).ledger_db(1);
    let store =
        Store::open_as(&test_ledger, &DbIdentity::obligado(ProfileKind::Pruebas, 1)).unwrap();
    assert_eq!(
        store.verifactu_environment().unwrap(),
        VerifactuEnvironment::Simulador
    );
    drop(store);
    // La historia se conserva y la fila nueva consta como del traslado.
    assert_eq!(
        query_strings(
            &test_ledger,
            "SELECT environment, authorization FROM verifactu_environment_changes ORDER BY id"
        ),
        vec![
            "Text(\"produccion\")|Text(\"prueba\")".to_owned(),
            "Text(\"simulador\")|Text(\"traslado (D272)\")".to_owned()
        ]
    );
}

#[test]
fn accepted_production_records_abort_the_move_before_anything_is_written() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path();
    build_legacy(data, SIMULATOR);
    let legacy = data.join("nestadmin.db");
    raw(&legacy)
        .execute_batch(
            "DROP TRIGGER verifactu_record_environments_no_update; \
             UPDATE verifactu_record_environments SET environment = 'produccion' WHERE record_id = 1; \
             CREATE TRIGGER verifactu_record_environments_no_update \
             BEFORE UPDATE ON verifactu_record_environments \
             BEGIN SELECT RAISE(ABORT, 'el entorno de un registro Verifactu no se modifica'); END; \
             INSERT INTO verifactu_batches (environment, idempotency_key, header_json, record_count, \
             authorization, started_at) VALUES ('produccion', 'lote-1', '{}', 1, 'x', 1); \
             INSERT INTO verifactu_record_results (batch_id, record_id, status) \
             VALUES (1, 1, 'accepted');",
        )
        .unwrap();
    let err = migrate(data, Some(Decision::Real)).err().unwrap();
    assert!(
        matches!(
            err,
            CoreError::Profile {
                code: "production_history_present",
                ..
            }
        ),
        "{err}"
    );
    assert_eq!(
        count(&legacy, "profile_migration"),
        0,
        "sin marcador: no se empezó"
    );
    assert!(read_pointer(data).unwrap().is_none());
    assert!(!data.join("perfiles").exists());
    assert!(!normal(data).principal_db().exists());
    assert_no_leftovers(data);
}

#[test]
fn principal_tables_are_exactly_the_imported_ones_plus_the_explicitly_excluded() {
    let imported = principal_import_tables();
    let mut all: BTreeSet<&str> = imported.iter().copied().collect();
    assert_eq!(all.len(), imported.len(), "una tabla importada dos veces");
    for table in PRINCIPAL_EXCLUDED {
        assert!(
            all.insert(table),
            "{table} está importada y excluida a la vez"
        );
    }
    // `audit_log` es de todas las bases: se importa filtrada y no figura entre las principales.
    assert!(all.remove("audit_log"));
    let principal: BTreeSet<&str> = tables_of(TableClass::Principal).collect();
    assert_eq!(all, principal);
}

#[test]
fn the_report_says_what_the_move_left_out() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path();
    assert_eq!(migration_report(data).unwrap(), None);
    build_legacy(data, SIMULATOR);
    assert_eq!(migration_report(data).unwrap(), None, "sin marcador aún");
    migrate(data, Some(Decision::Real)).unwrap();
    let report = migration_report(data).unwrap().unwrap();
    assert_eq!(report.decision, Some(Decision::Real));
    assert_eq!(
        report.discarded_send_tasks.get("completed").copied(),
        Some(1),
        "{report:?}"
    );
    assert!(report.discarded_sales_audit_rows > 0);
    assert_eq!(report.deactivated_recurring_templates, 1);

    let dir = tempfile::tempdir().unwrap();
    build_legacy(dir.path(), SIMULATOR);
    migrate(dir.path(), Some(Decision::Ficticia)).unwrap();
    let report = migration_report(dir.path()).unwrap().unwrap();
    assert_eq!(report.decision, Some(Decision::Ficticia));
    assert_eq!(report.deactivated_recurring_templates, 0);

    let dir = tempfile::tempdir().unwrap();
    build_legacy(dir.path(), EMPTY);
    migrate(dir.path(), None).unwrap();
    let report = migration_report(dir.path()).unwrap().unwrap();
    assert_eq!(report.decision, None);
    assert!(report.discarded_send_tasks.is_empty());
    assert_eq!(report.discarded_sales_audit_rows, 0);
}

#[test]
fn a_second_attempt_while_the_first_runs_is_refused_with_migration_locked() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path();
    build_legacy(data, SIMULATOR);
    let mut refused = Vec::new();
    let end = migrate_with_hook(data, Some(Decision::Real), &mut |label| {
        if label == "step2:copy-written" {
            refused.push(migrate_step(data, Some(Decision::Real)));
            refused.push(migrate(data, Some(Decision::Real)));
            refused.push(migrate_step_with_hook(data, None, &mut |_| false));
        }
        false
    })
    .unwrap();
    assert_eq!(end, MigrationState::Done);
    assert_eq!(refused.len(), 3);
    for result in refused {
        assert!(
            matches!(
                result,
                Err(CoreError::Profile {
                    code: "migration_locked",
                    ..
                })
            ),
            "{result:?}"
        );
    }
    // El cerrojo se suelta al terminar y el resultado no se resintió.
    assert_no_leftovers(data);
    assert_eq!(migrate_step(data, None).unwrap(), MigrationState::Done);
    assert_eq!(count(&pruebas(data).ledger_db(1), "verifactu_records"), 2);
}

#[cfg(windows)]
#[test]
fn a_locked_old_database_does_not_block_startup_and_the_rename_finishes_later() {
    use std::os::windows::fs::OpenOptionsExt;

    let dir = tempfile::tempdir().unwrap();
    let data = dir.path();
    build_legacy(data, SIMULATOR);
    let legacy = data.join("nestadmin.db");
    let before = dump_all(&legacy);
    // Un handle que no comparte el borrado ni el renombrado (como un antivirus o el indexador).
    let held = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(3)
        .open(&legacy)
        .unwrap();
    let err = migrate(data, Some(Decision::Real))
        .err()
        .expect("el paso 7 no puede renombrar");
    assert!(matches!(err, CoreError::Io(_)), "{err}");

    // El puntero ya está: se arranca con el perfil normal y la base antigua sigue en su sitio.
    assert_eq!(startup_state(data).unwrap(), StartupState::Ready);
    assert!(legacy.is_file());
    assert!(
        !finish_pending_rename(data).unwrap(),
        "bloqueada: no espera ni falla"
    );
    assert!(legacy.is_file());

    drop(held);
    assert!(finish_pending_rename(data).unwrap());
    assert!(!legacy.exists());
    let renamed = data.join("nestadmin.trasladada.db");
    assert_eq!(dump_all(&renamed), before);
    assert_eq!(
        marker(&renamed).0,
        6,
        "la base archivada no se vuelve a escribir"
    );
    assert!(
        finish_pending_rename(data).unwrap(),
        "ya no queda nada pendiente"
    );
    assert_no_leftovers(data);
}

#[test]
fn startup_is_ready_with_a_pointer_even_if_the_old_database_is_still_there() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path();
    build_legacy(data, SIMULATOR);
    // Se corta justo después de escribir el puntero: la base antigua queda sin renombrar.
    let cut = migrate_with_hook(data, Some(Decision::Real), &mut |label| {
        label == "step6:pointer-renamed"
    });
    assert!(cut.is_err());
    assert_eq!(startup_state(data).unwrap(), StartupState::Ready);
    assert!(data.join("nestadmin.db").is_file());
    assert!(finish_pending_rename(data).unwrap());
    assert!(!data.join("nestadmin.db").exists());
    assert_eq!(marker(&data.join("nestadmin.trasladada.db")).0, 6);
    // Sin puntero no hay nada que cerrar.
    let other = tempfile::tempdir().unwrap();
    build_legacy(other.path(), SIMULATOR);
    assert!(!finish_pending_rename(other.path()).unwrap());
    assert!(other.path().join("nestadmin.db").is_file());
}

#[test]
fn rows_still_in_the_wal_of_the_old_database_reach_the_copy_and_the_principal() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a");
    build_legacy(&a, SIMULATOR);
    let b = dir.path().join("b");
    std::fs::create_dir_all(&b).unwrap();
    // Escribe tras el último checkpoint y copia la base con su `-wal` sin cerrar la conexión:
    // es lo que deja un proceso que murió.
    let mut store = Store::open(&a.join("nestadmin.db")).unwrap();
    for n in 0..3 {
        store
            .enqueue("email_triage", &format!(r#"{{"mail":"WAL-{n}"}}"#))
            .unwrap();
    }
    std::fs::copy(a.join("nestadmin.db"), b.join("nestadmin.db")).unwrap();
    std::fs::copy(a.join("nestadmin.db-wal"), b.join("nestadmin.db-wal")).unwrap();
    drop(store);
    assert!(
        b.join("nestadmin.db-wal").is_file(),
        "el montaje necesita un -wal real"
    );
    // Sanidad: el archivo principal por sí solo no tiene esas filas.
    let only_main = b.join("solo-principal.db");
    std::fs::copy(b.join("nestadmin.db"), &only_main).unwrap();
    assert_eq!(
        count_wal_rows(&only_main),
        0,
        "las filas están solo en el -wal"
    );
    std::fs::remove_file(&only_main).unwrap();

    migrate(&b, Some(Decision::Real)).unwrap();
    assert_no_leftovers(&b);
    // 2 de correo + 3 del WAL (la tarea de envío no pasa a la principal).
    assert_eq!(count(&normal(&b).principal_db(), "tasks"), 5);
    // La copia de pruebas tiene además la tarea de envío.
    assert_eq!(count(&pruebas(&b).ledger_db(1), "tasks"), 6);
    assert_eq!(count_wal_rows(&pruebas(&b).ledger_db(1)), 3);
}

fn count_wal_rows(path: &Path) -> i64 {
    raw(path)
        .query_row(
            "SELECT count(*) FROM tasks WHERE input_ref LIKE '%WAL-%'",
            [],
            |r| r.get(0),
        )
        .unwrap()
}

#[test]
fn folders_with_spaces_accents_and_percent_work_with_the_attach() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("Carpeta ñandú 100% prueba");
    build_legacy(&data, SIMULATOR);
    migrate(&data, Some(Decision::Real)).unwrap();
    assert_no_leftovers(&data);
    assert_eq!(count(&normal(&data).ledger_db(1), "customers"), 1);
    assert_eq!(count(&normal(&data).principal_db(), "tasks"), 2);
    assert_eq!(count(&pruebas(&data).ledger_db(1), "verifactu_records"), 2);
}
