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
const DROP_PANEL: &str = "DROP TABLE rule_settings; DROP TABLE rule_suggestions; DROP TABLE rule_signals; DROP TABLE rule_evaluations; DROP TABLE rule_meta; ALTER TABLE actions DROP COLUMN rule_id; ALTER TABLE actions DROP COLUMN rule_version; DROP INDEX IF EXISTS tasks_redraft_of; DROP TABLE IF EXISTS sender_preferences; DROP TABLE IF EXISTS business_profile;";

/// Quita lo que añadió la migración 8 (plantillas y ejecuciones de pipeline), como si la base
/// la hubiera creado la versión anterior. Va antes de los `DROP` anteriores.
const DROP_PIPELINES: &str =
    "DROP TABLE pipeline_steps; DROP TABLE pipeline_runs; DROP TABLE task_templates;";

/// Quita lo que añadió la migración 12 (propuestas del asistente y origen de los eventos).
const DROP_PROPOSALS: &str =
    "DROP TABLE assistant_proposals; ALTER TABLE audit_log DROP COLUMN origin;";

/// Quita lo que añadió la migración 11 (conversaciones del asistente) y la 12, que depende de ella.
/// Va antes de los `DROP` anteriores.
const DROP_ASSISTANT: &str = "DROP TABLE assistant_proposals; ALTER TABLE audit_log DROP COLUMN origin; DROP TABLE assistant_calls; DROP TABLE assistant_turns; DROP TABLE assistant_conversations;";

/// Quita lo que añadió la migración 10 (índice de búsqueda; los triggers caen con su tabla).
/// Va antes de los `DROP` anteriores.
const DROP_SEARCH: &str = "DROP TABLE search_fts; DROP TABLE search_index;";

/// Quita lo que añadió la migración 9 (`reply_style`), como si la base la hubiera creado la
/// versión anterior. Va antes de los `DROP` anteriores.
const DROP_LEARNED: &str = "DROP TABLE reply_style;";

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
        "{DROP_ASSISTANT} {DROP_SEARCH} {DROP_LEARNED} {DROP_PIPELINES} {DROP_PANEL} {DROP_SALES} DROP TABLE rules; PRAGMA user_version = 1;"
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
        "{DROP_ASSISTANT} {DROP_SEARCH} {DROP_LEARNED} {DROP_PIPELINES} {DROP_PANEL} {DROP_SALES} PRAGMA user_version = 2;"
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
    assert_eq!(SCHEMA_VERSION, 13);
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
        "{DROP_ASSISTANT} {DROP_SEARCH} {DROP_LEARNED} {DROP_PIPELINES} {DROP_PANEL} PRAGMA user_version = 3;"
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
    assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
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
        "{DROP_ASSISTANT} {DROP_SEARCH} {DROP_LEARNED} {DROP_PIPELINES} {DROP_PANEL} PRAGMA user_version = 3;"
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

/// La migración 8 (D148) solo añade tablas: tareas, estados y acciones de una base del
/// esquema 7 quedan como estaban, y las tareas siguen listas para ejecutarse.
#[test]
fn migrating_from_schema_7_adds_pipeline_tables_and_keeps_tasks() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nestadmin.db");
    drop(Store::open(&path).unwrap());
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch(&format!(
        "{DROP_ASSISTANT} {DROP_SEARCH} {DROP_LEARNED} {DROP_PIPELINES} PRAGMA user_version = 7;"
    ))
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
    assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
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
        assert!(!has_table(&backup_path(&path, 7), table));
    }
    assert_eq!(user_version(&backup_path(&path, 7)), 7);
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

/// La migración 5 añade el índice único de «Redactar respuesta» (D159), con copia previa del
/// esquema 4, sin tocar las tareas que ya existen y sin romper entradas que no son JSON.
#[test]
fn migrating_from_schema_4_adds_the_redraft_index_and_keeps_a_copy() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nestadmin.db");
    drop(Store::open(&path).unwrap());
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch(&format!(
        "{DROP_ASSISTANT} {DROP_SEARCH} {DROP_LEARNED} {DROP_PIPELINES} DROP INDEX tasks_redraft_of; DROP TABLE sender_preferences; DROP TABLE business_profile; PRAGMA user_version = 4;"
    ))
    .unwrap();
    for (id, input) in [(1, "{}"), (2, "no es json"), (3, r#"{"redraft_of": 1}"#)] {
        conn.execute(
            "INSERT INTO tasks (id, kind, input_ref, state, created_at, updated_at) VALUES (?1, 'email', ?2, 'queued', 1, 1)",
            rusqlite::params![id, input],
        )
        .unwrap();
    }
    drop(conn);

    let mut store = Store::open(&path).unwrap();
    assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
    let backup = backup_path(&path, 4);
    assert_eq!(backup.file_name().unwrap(), "nestadmin.esquema-4.db");
    assert_eq!(user_version(&backup), 4);
    // Lo anterior sigue ahí.
    assert_eq!(store.all_tasks().unwrap().len(), 3);
    // Una entrada que no es JSON se acepta; un segundo `redraft_of` vivo, no.
    store
        .enqueue("email", "otra entrada que no es json")
        .unwrap();
    assert!(store.enqueue("email", r#"{"redraft_of": 1}"#).is_err());
    // Cancelada la primera, deja el sitio libre.
    store.cancel(3).unwrap();
    store.enqueue("email", r#"{"redraft_of": 1}"#).unwrap();
}

/// La migración 6 añade las preferencias por remitente (D161), con copia previa del esquema 5,
/// sin tocar nada de lo anterior, y la tabla rechaza claves sin normalizar y rutas desconocidas.
#[test]
fn migrating_from_schema_5_adds_sender_preferences_and_keeps_a_copy() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nestadmin.db");
    drop(Store::open(&path).unwrap());
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch(&format!(
        "{DROP_ASSISTANT} {DROP_SEARCH} {DROP_LEARNED} {DROP_PIPELINES} DROP TABLE sender_preferences; DROP TABLE business_profile; PRAGMA user_version = 5;"
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
    assert_eq!(store.audit_log().unwrap().len(), 1);
    assert!(store.sender_preferences().unwrap().is_empty());
    drop(store);

    let backup = backup_path(&path, 5);
    assert_eq!(backup.file_name().unwrap(), "nestadmin.esquema-5.db");
    assert_eq!(user_version(&backup), 5);
    assert!(!has_table(&backup, "sender_preferences"));
    assert!(has_table(&path, "sender_preferences"));

    let conn = rusqlite::Connection::open(&path).unwrap();
    let insert = |scope: &str, key: &str, route: &str| {
        conn.execute(
            "INSERT INTO sender_preferences (scope, key, route, updated_at) VALUES (?1, ?2, ?3, 1)",
            rusqlite::params![scope, key, route],
        )
    };
    assert!(insert("address", "a@b.test", "review").is_ok());
    assert!(
        insert("address", "a@b.test", "informative").is_err(),
        "clave única"
    );
    for (scope, key, route) in [
        ("otro", "a@b.test", "review"),
        ("address", "A@b.test", "review"),
        ("address", " a@b.test", "review"),
        ("address", "", "review"),
        ("domain", "b.test", "siempre"),
    ] {
        assert!(
            insert(scope, key, route).is_err(),
            "{scope} {key:?} {route}"
        );
    }
}

/// La migración 7 añade el perfil del negocio (D163), con copia previa del esquema 6, sin tocar
/// nada de lo anterior; la tabla es de fila única y rechaza más de 2.000 caracteres en total.
#[test]
fn migrating_from_schema_6_adds_the_business_profile_and_keeps_a_copy() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nestadmin.db");
    drop(Store::open(&path).unwrap());
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch(&format!(
        "{DROP_ASSISTANT} {DROP_SEARCH} {DROP_LEARNED} {DROP_PIPELINES} DROP TABLE business_profile; PRAGMA user_version = 6;"
    ))
    .unwrap();
    conn.execute(
        "INSERT INTO audit_log (at, action, status) VALUES (1, 'previa', 'ok')",
        [],
    )
    .unwrap();
    drop(conn);

    let mut store = Store::open(&path).unwrap();
    assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
    assert_eq!(store.audit_log().unwrap().len(), 1);
    assert!(store.business_profile().unwrap().is_none());

    let backup = backup_path(&path, 6);
    assert_eq!(backup.file_name().unwrap(), "nestadmin.esquema-6.db");
    assert_eq!(user_version(&backup), 6);
    assert!(!has_table(&backup, "business_profile"));
    assert!(has_table(&path, "business_profile"));

    // Guardar, sustituir y borrar: una sola fila y un evento por cambio, sin el texto.
    let mut profile = nestadmin_core::BusinessProfile {
        hours: "Lunes a viernes de 9 a 14".into(),
        ..Default::default()
    };
    store.save_business_profile(&profile).unwrap();
    profile.tone = "Cercano".into();
    store.save_business_profile(&profile).unwrap();
    assert_eq!(store.business_profile().unwrap().unwrap().0, profile);
    assert!(store.clear_business_profile().unwrap());
    assert!(!store.clear_business_profile().unwrap());
    let log = store.audit_log().unwrap();
    let events: Vec<_> = log
        .iter()
        .filter(|e| e.action == "business_profile")
        .collect();
    assert_eq!(events.len(), 3);
    assert!(events
        .iter()
        .all(|e| !e.detail.as_deref().unwrap_or("").contains("Lunes")));

    // La tabla misma impone la fila única y el tope.
    drop(store);
    let conn = rusqlite::Connection::open(&path).unwrap();
    let insert = |id: i64, hours: String| {
        conn.execute(
            "INSERT INTO business_profile (id, hours, updated_at) VALUES (?1, ?2, 1)",
            rusqlite::params![id, hours],
        )
    };
    assert!(insert(2, "x".into()).is_err(), "solo la fila 1");
    assert!(
        insert(1, "x".repeat(2001)).is_err(),
        "más de 2.000 caracteres"
    );
    assert!(insert(1, "x".repeat(2000)).is_ok());
}

/// La migración 9 (D171, D172) reconstruye `rule_signals` para que `task_id` admita NULL solo en
/// las señales `learned`, sin perder las filas anteriores, y añade `reply_style`.
#[test]
fn migrating_from_schema_8_keeps_the_signals_and_allows_learned_ones_without_a_task() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nestadmin.db");
    drop(Store::open(&path).unwrap());
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch(&format!(
        "{DROP_ASSISTANT} {DROP_SEARCH} DROP TABLE reply_style; DROP TABLE rule_signals;
         CREATE TABLE rule_signals (
             id INTEGER PRIMARY KEY, kind TEXT NOT NULL, action_kind TEXT NOT NULL,
             subject_key TEXT NOT NULL, edit_state TEXT NOT NULL,
             task_id INTEGER NOT NULL REFERENCES tasks (id), content_version INTEGER NOT NULL,
             source_key TEXT, at INTEGER NOT NULL, state TEXT NOT NULL DEFAULT 'valid',
             UNIQUE (kind, task_id, content_version));
         INSERT INTO tasks (id, kind, input_ref, state, created_at, updated_at) VALUES (1, 'email', '{{}}', 'completed', 1, 1);
         INSERT INTO rule_signals (id, kind, action_kind, subject_key, edit_state, task_id, content_version, source_key, at, state)
         VALUES (7, 'approved', 'send_draft', 'solicitud_factura|ana@alfa.test', 'unedited', 1, 1, NULL, 5, 'valid');
         PRAGMA user_version = 8;"
    ))
    .unwrap();
    drop(conn);

    let store = Store::open(&path).unwrap();
    assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
    drop(store);
    assert_eq!(user_version(&backup_path(&path, 8)), 8);
    assert!(!has_table(&backup_path(&path, 8), "reply_style"));
    assert!(has_table(&path, "reply_style"));

    let conn = rusqlite::Connection::open(&path).unwrap();
    let kept: (i64, String, i64) = conn
        .query_row("SELECT id, state, task_id FROM rule_signals", [], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .unwrap();
    assert_eq!(
        kept,
        (7, "valid".to_owned(), 1),
        "la fila anterior se conserva"
    );
    let learned = |key: &str| {
        conn.execute(
            "INSERT INTO rule_signals (kind, action_kind, subject_key, edit_state, task_id, content_version, source_key, at)
             VALUES ('learned', 'send_draft', 'solicitud_factura|ana@alfa.test', 'unknown', NULL, 0, ?1, 1)",
            [key],
        )
    };
    assert!(learned("learned|a|m1").is_ok());
    assert!(learned("learned|a|m2").is_ok());
    assert!(
        learned("learned|a|m1").is_err(),
        "única por correo recibido"
    );
    // Solo `learned` puede no tener tarea.
    assert!(conn
        .execute(
            "INSERT INTO rule_signals (kind, action_kind, subject_key, edit_state, task_id, content_version, at)
             VALUES ('approved', 'send_draft', 'x|y@z.test', 'unedited', NULL, 1, 1)",
            [],
        )
        .is_err());
}

/// La migración 12 (D185) añade `assistant_proposals` y la columna `origin` del historial: sobre
/// una base nueva y sobre una de la versión 11, que conserva sus filas con el origen nulo.
#[test]
fn a_new_database_has_proposals_and_a_null_origin_for_ordinary_events() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nestadmin.db");
    let mut store = Store::open(&path).unwrap();
    assert_eq!(SCHEMA_VERSION, 13);
    store
        .record_note("evento", "ok", &serde_json::json!({}))
        .unwrap();
    let log = store.audit_log().unwrap();
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].origin, None, "un evento corriente no lleva origen");
    drop(store);
    assert!(has_table(&path, "assistant_proposals"));
}

#[test]
fn migrating_from_schema_11_adds_proposals_and_keeps_previous_events_with_no_origin() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nestadmin.db");
    drop(Store::open(&path).unwrap());
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch(&format!("{DROP_PROPOSALS} PRAGMA user_version = 11;"))
        .unwrap();
    conn.execute(
        "INSERT INTO audit_log (at, action, status) VALUES (1, 'previa', 'ok')",
        [],
    )
    .unwrap();
    drop(conn);
    assert!(!has_table(&path, "assistant_proposals"));

    let store = Store::open(&path).unwrap();
    assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
    let log = store.audit_log().unwrap();
    assert_eq!(log.len(), 1, "los datos previos se conservan");
    assert_eq!(log[0].origin, None);
    drop(store);
    let backup = backup_path(&path, 11);
    assert_eq!(user_version(&backup), 11);
    assert!(!has_table(&backup, "assistant_proposals"));
    assert!(has_table(&path, "assistant_proposals"));
}

/// Mientras hay un origen fijado, todo evento lo lleva en el mismo `INSERT`; al quitarlo, vuelve a
/// ser nulo. El origen es de la conexión: no queda en el archivo.
#[test]
fn the_audit_origin_stamps_events_only_while_it_is_set_and_never_persists() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nestadmin.db");
    let mut store = Store::open(&path).unwrap();
    let note = serde_json::json!({});
    store.record_note("antes", "ok", &note).unwrap();
    store.set_audit_origin(Some("chat")).unwrap();
    store.record_note("durante", "ok", &note).unwrap();
    store.record_event(1, "durante_tarea", "ok", &note).ok();
    store.set_audit_origin(None).unwrap();
    store.record_note("despues", "ok", &note).unwrap();
    let origin = |action: &str| {
        store
            .audit_log()
            .unwrap()
            .into_iter()
            .find(|e| e.action == action)
            .map(|e| e.origin)
    };
    assert_eq!(origin("antes"), Some(None));
    assert_eq!(origin("durante"), Some(Some("chat".to_owned())));
    assert_eq!(origin("despues"), Some(None));
    // Un corte con el origen puesto no lo deja en la base: otra conexión escribe sin origen.
    store.set_audit_origin(Some("chat")).unwrap();
    drop(store);
    let mut again = Store::open(&path).unwrap();
    again.record_note("tras_el_corte", "ok", &note).unwrap();
    let last = again
        .audit_log()
        .unwrap()
        .into_iter()
        .find(|e| e.action == "tras_el_corte")
        .unwrap();
    assert_eq!(last.origin, None);
}

/// Quita lo que añadió la migración 13 (la tabla de propuestas con el CHECK ampliado) y la vuelve a
/// dejar como la creó la 12: con diez tipos de propuesta.
const TABLE_OF_SCHEMA_12: &str = "DROP TABLE assistant_proposals;
CREATE TABLE assistant_proposals (
    id INTEGER PRIMARY KEY,
    conversation_id INTEGER REFERENCES assistant_conversations (id) ON DELETE CASCADE,
    turn_id INTEGER,
    kind TEXT NOT NULL CHECK (kind IN (
        'approve', 'reject', 'retry', 'archive', 'confirm_not_sent', 'edit_draft',
        'fetch_mail', 'mail_limit', 'rename_account', 'sender_preference'
    )),
    task_id INTEGER, seen_version INTEGER, seen_fp TEXT,
    args_json TEXT NOT NULL, card_json TEXT NOT NULL, idem_key TEXT NOT NULL UNIQUE,
    status TEXT NOT NULL DEFAULT 'pending' CHECK (status IN (
        'pending', 'executing', 'confirmed', 'rejected', 'expired', 'stale', 'failed'
    )),
    receipt_json TEXT, created_at INTEGER NOT NULL, expires_at INTEGER NOT NULL, resolved_at INTEGER
);
CREATE INDEX assistant_proposals_conversation ON assistant_proposals (conversation_id, status);
CREATE INDEX assistant_proposals_task ON assistant_proposals (task_id, status);";

/// La migración 13 (D191) reconstruye `assistant_proposals` para admitir los cinco tipos nuevos: una
/// propuesta anterior (aquí una confirmada, con su recibo, y una pendiente) sobrevive intacta, la
/// tabla admite los tipos nuevos y sigue rechazando uno desconocido, y conserva índices y unicidad.
#[test]
fn migrating_from_schema_12_keeps_the_proposals_and_admits_the_new_kinds() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nestadmin.db");
    drop(Store::open(&path).unwrap());
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch(&format!("{TABLE_OF_SCHEMA_12} PRAGMA user_version = 12;"))
        .unwrap();
    conn.execute_batch(
        "INSERT INTO assistant_conversations (id, created_at, updated_at) VALUES (1, 1, 1);
         INSERT INTO assistant_proposals
            (id, conversation_id, turn_id, kind, task_id, seen_version, seen_fp, args_json,
             card_json, idem_key, status, receipt_json, created_at, expires_at, resolved_at)
         VALUES
            (4, 1, 2, 'mail_limit', NULL, NULL, 'fp-a', '{\"limit\":50}', '{\"effect\":\"none\"}',
             'prop/mail_limit/0/1-0', 'confirmed', '{\"outcome\":\"done\"}', 10, 99, 20),
            (9, 1, 2, 'fetch_mail', NULL, NULL, 'fp-b', '{}', '{\"effect\":\"none\"}',
             'prop/fetch_mail/0/2-1', 'pending', NULL, 11, 99, NULL);",
    )
    .unwrap();
    drop(conn);

    let store = Store::open(&path).unwrap();
    assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
    drop(store);
    let backup = backup_path(&path, 12);
    assert_eq!(user_version(&backup), 12);

    let conn = rusqlite::Connection::open(&path).unwrap();
    type Row = (i64, String, String, Option<String>, Option<String>);
    let rows: Vec<Row> = conn
        .prepare(
            "SELECT id, kind, status, receipt_json, seen_fp FROM assistant_proposals ORDER BY id",
        )
        .unwrap()
        .query_map([], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        rows,
        vec![
            (
                4,
                "mail_limit".to_owned(),
                "confirmed".to_owned(),
                Some("{\"outcome\":\"done\"}".to_owned()),
                Some("fp-a".to_owned())
            ),
            (
                9,
                "fetch_mail".to_owned(),
                "pending".to_owned(),
                None,
                Some("fp-b".to_owned())
            ),
        ],
        "las propuestas anteriores se conservan tal cual"
    );
    let insert = |kind: &str, key: &str| {
        conn.execute(
            "INSERT INTO assistant_proposals
                (conversation_id, kind, args_json, card_json, idem_key, created_at, expires_at)
             VALUES (1, ?1, '{}', '{}', ?2, 1, 2)",
            [kind, key],
        )
    };
    for (n, kind) in nestadmin_core::PROPOSAL_KINDS.iter().enumerate() {
        assert!(insert(kind, &format!("k{n}")).is_ok(), "{kind}");
    }
    assert!(
        insert("activate_rule", "k-x").is_err(),
        "un tipo desconocido se rechaza"
    );
    assert!(
        insert("import", "k0").is_err(),
        "la clave de idempotencia sigue siendo única"
    );
    let indexes: i64 = conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type = 'index' AND tbl_name = 'assistant_proposals'
             AND name IN ('assistant_proposals_conversation', 'assistant_proposals_task')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(indexes, 2);
    // `AUTOINCREMENT`: el identificador de una propuesta no se reutiliza ni tras borrarlas todas.
    let before = conn.last_insert_rowid();
    assert!(
        before > 9,
        "sigue a los identificadores anteriores: {before}"
    );
    conn.execute("DELETE FROM assistant_proposals", []).unwrap();
    assert!(insert("import", "otra-vez").is_ok());
    assert!(conn.last_insert_rowid() > before, "no se reutiliza");
    // La tabla vieja no queda.
    assert!(!has_table(&path, "assistant_proposals_new"));
}
