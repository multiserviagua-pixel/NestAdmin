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
const DROP_SALES: &str = "DROP TABLE received_delivery_notes; DROP TABLE payments;     DROP TABLE document_links; DROP TABLE sales_lines; DROP TABLE sales_documents;     DROP TABLE recurring_skips; DROP TABLE recurring_templates; DROP TABLE series; DROP TABLE products;     DROP TABLE customers; DROP TABLE company; DROP TABLE company_logos;";

/// Quita lo que añadió la migración 4 (panel de reglas), como si la base la hubiera creado
/// la versión anterior.
const DROP_PANEL: &str = "DROP TABLE rule_settings; DROP TABLE rule_suggestions; DROP TABLE rule_signals; DROP TABLE rule_evaluations; DROP TABLE rule_meta; ALTER TABLE actions DROP COLUMN rule_id; ALTER TABLE actions DROP COLUMN rule_version;";

/// Quita lo que añadió la migración 5 (plantillas y ejecuciones de pipeline), como si la base
/// la hubiera creado la versión anterior. Va antes de los `DROP` anteriores.
const DROP_PIPELINES: &str =
    "DROP TABLE pipeline_steps; DROP TABLE pipeline_runs; DROP TABLE task_templates;";

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
        "{DROP_PIPELINES} {DROP_PANEL} {DROP_SALES} DROP TABLE rules; PRAGMA user_version = 1;"
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
    conn.execute_batch(&format!(
        "{DROP_PIPELINES} {DROP_PANEL} {DROP_SALES} PRAGMA user_version = 2;"
    ))
    .unwrap();
    conn.execute(
        "INSERT INTO audit_log (at, action, status) VALUES (1, 'previa', 'ok')",
        [],
    )
    .unwrap();
    drop(conn);

    let store = Store::open(&path).unwrap();
    assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
    assert_eq!(SCHEMA_VERSION, 5);
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
        "recurring_skips",
        "company_logos",
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

/// La migración 4 (D124–D129) añade las tablas del panel y las columnas de `actions` sin
/// tocar las filas anteriores, que quedan sin regla asignada.
#[test]
fn migrating_from_schema_3_adds_the_rules_panel_and_keeps_old_actions() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nestadmin.db");
    drop(Store::open(&path).unwrap());
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch(&format!(
        "{DROP_PIPELINES} {DROP_PANEL} PRAGMA user_version = 3;"
    ))
    .unwrap();
    conn.execute(
        "INSERT INTO tasks (id, kind, input_ref, state, created_at, updated_at) VALUES (1, 'invoice', '{}', 'completed', 1, 1)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO actions (idempotency_key, task_id, kind, version, payload, authorization, status, created_at, updated_at)
         VALUES ('task-1/register_invoice/v1', 1, 'register_invoice', 1, '{}', 'regla alfa v2', 'confirmed', 1, 1)",
        [],
    )
    .unwrap();
    drop(conn);

    let store = Store::open(&path).unwrap();
    assert_eq!(store.schema_version().unwrap(), 5);
    drop(store);
    for table in [
        "rule_meta",
        "rule_evaluations",
        "rule_signals",
        "rule_suggestions",
        "rule_settings",
    ] {
        assert!(has_table(&path, table), "{table} debe existir tras migrar");
        assert!(!has_table(&backup_path(&path, 3), table));
    }
    let conn = rusqlite::Connection::open(&path).unwrap();
    let (rule_id, rule_version, authorization): (Option<String>, Option<i64>, String) = conn
        .query_row(
            "SELECT rule_id, rule_version, authorization FROM actions",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!((rule_id, rule_version), (None, None));
    assert_eq!(authorization, "regla alfa v2");
}

/// La migración 4 desactiva (con una versión nueva) las reglas activas cuyas condiciones
/// llevan claves que hasta ahora no coincidían nunca (`currency`, `sender`, `sender_domain`).
/// El resto, incluidas las de envío sin remitente, queda como estaba.
#[test]
fn migrating_from_schema_3_deactivates_old_rules_whose_conditions_now_apply() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nestadmin.db");
    drop(Store::open(&path).unwrap());
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch(&format!(
        "{DROP_PIPELINES} {DROP_PANEL} PRAGMA user_version = 3;"
    ))
    .unwrap();
    let rule = |id: &str, version: i64, kind: &str, conditions: &str, active: i64| {
        conn.execute(
            "INSERT INTO rules (id, version, action_kind, conditions, active, created_at) VALUES (?1, ?2, ?3, ?4, ?5, 1)",
            rusqlite::params![id, version, kind, conditions, active],
        )
        .unwrap();
    };
    // Última versión activa con `currency`: pasa a inactiva.
    rule(
        "moneda",
        1,
        "register_invoice",
        r#"{"supplier":"Alfa SL"}"#,
        0,
    );
    rule(
        "moneda",
        2,
        "register_invoice",
        r#"{"supplier":"Alfa SL","currency":"EUR"}"#,
        1,
    );
    // Activas de envío con remitente o dominio: pasan a inactivas.
    rule(
        "dominio",
        1,
        "send_draft",
        r#"{"category":"consulta","sender_domain":"alfa.test"}"#,
        1,
    );
    rule(
        "direccion",
        1,
        "send_draft",
        r#"{"category":"consulta","sender":"a@alfa.test"}"#,
        1,
    );
    // Sin esas claves, o ya inactivas: no se tocan (la de envío sin remitente falla cerrada).
    rule(
        "sin-remitente",
        1,
        "send_draft",
        r#"{"category":"consulta"}"#,
        1,
    );
    rule(
        "limite",
        1,
        "register_invoice",
        r#"{"supplier":"X","max_total":"5.00"}"#,
        1,
    );
    rule(
        "apagada",
        1,
        "send_draft",
        r#"{"category":"consulta","sender":"a@alfa.test"}"#,
        0,
    );
    // La clave con valor nulo también cuenta como presente.
    rule(
        "nula",
        1,
        "register_invoice",
        r#"{"supplier":"X","currency":null}"#,
        1,
    );
    drop(conn);

    let store = Store::open(&path).unwrap();
    let latest = |id: &str| {
        let history = store.rule_history(id).unwrap();
        let last = history.last().unwrap().clone();
        (history.len(), last.version, last.active, last.conditions)
    };
    let (n, version, active, conditions) = latest("moneda");
    assert_eq!((n, version, active), (3, 3, false));
    assert_eq!(conditions["currency"], "EUR", "mismas condiciones");
    assert_eq!(latest("dominio").0, 2);
    assert!(!latest("dominio").2);
    assert!(!latest("direccion").2);
    assert!(!latest("nula").2);
    for untouched in ["sin-remitente", "limite"] {
        let (n, _, active, _) = latest(untouched);
        assert_eq!((n, active), (1, true), "{untouched}");
    }
    assert_eq!(latest("apagada").0, 1);
    let log = store.audit_log().unwrap();
    let migrated: Vec<_> = log
        .iter()
        .filter(|e| {
            e.action == "rule" && e.detail.as_deref().is_some_and(|d| d.contains("al migrar"))
        })
        .collect();
    assert_eq!(migrated.len(), 4);
    assert!(migrated.iter().all(|e| e.status == "inactive"));
}

/// La migración 5 (D148) solo añade tablas: tareas, estados y acciones de una base del
/// esquema 4 quedan como estaban, y las tareas siguen listas para ejecutarse.
#[test]
fn migrating_from_schema_4_adds_pipeline_tables_and_keeps_tasks() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nestadmin.db");
    drop(Store::open(&path).unwrap());
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch(&format!("{DROP_PIPELINES} PRAGMA user_version = 4;"))
        .unwrap();
    for (id, state) in [
        (1, "queued"),
        (2, "running"),
        (3, "awaiting_review"),
        (4, "completed"),
        (5, "failed"),
        (6, "cancelled"),
    ] {
        conn.execute(
            "INSERT INTO tasks (id, kind, input_ref, state, attempts, created_at, updated_at) VALUES (?1, 'invoice', '{}', ?2, 1, 1, 1)",
            rusqlite::params![id, state],
        )
        .unwrap();
    }
    drop(conn);

    let store = Store::open(&path).unwrap();
    assert_eq!(store.schema_version().unwrap(), 5);
    let states: Vec<_> = store
        .all_tasks()
        .unwrap()
        .into_iter()
        .rev()
        .map(|t| (t.id, t.state.as_str().to_owned(), t.attempts))
        .collect();
    assert_eq!(
        states,
        [
            (1, "queued".to_owned(), 1),
            (2, "running".to_owned(), 1),
            (3, "awaiting_review".to_owned(), 1),
            (4, "completed".to_owned(), 1),
            (5, "failed".to_owned(), 1),
            (6, "cancelled".to_owned(), 1),
        ]
    );
    let ready: Vec<_> = store.ready_tasks().unwrap().iter().map(|t| t.id).collect();
    assert_eq!(ready, [1], "sin ejecuciones, la cola en espera sigue lista");
    drop(store);
    for table in ["task_templates", "pipeline_runs", "pipeline_steps"] {
        assert!(has_table(&path, table), "{table} debe existir tras migrar");
        assert!(!has_table(&backup_path(&path, 4), table));
    }
    assert_eq!(user_version(&backup_path(&path, 4)), 4);
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
