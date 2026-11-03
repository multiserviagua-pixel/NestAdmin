use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::Connection;

use crate::error::{CoreError, Result};

/// Migraciones numeradas; la posición en la lista es la versión de esquema (D03).
const MIGRATIONS: &[&str] = &[
    r#"
CREATE TABLE tasks (
    id          INTEGER PRIMARY KEY,
    kind        TEXT NOT NULL,
    input_ref   TEXT NOT NULL,
    state       TEXT NOT NULL,
    attempts    INTEGER NOT NULL DEFAULT 0,
    result      TEXT,
    error       TEXT,
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL
);
CREATE INDEX tasks_state ON tasks (state, id);

CREATE TABLE task_transitions (
    id          INTEGER PRIMARY KEY,
    task_id     INTEGER NOT NULL REFERENCES tasks (id),
    from_state  TEXT,
    to_state    TEXT NOT NULL,
    reason      TEXT NOT NULL,
    at          INTEGER NOT NULL
);

CREATE TABLE actions (
    idempotency_key TEXT PRIMARY KEY,
    task_id         INTEGER NOT NULL REFERENCES tasks (id),
    kind            TEXT NOT NULL,
    version         INTEGER NOT NULL,
    payload         TEXT NOT NULL,
    authorization   TEXT NOT NULL,
    status          TEXT NOT NULL,
    receipt         TEXT,
    error           TEXT,
    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL
);

CREATE TABLE audit_log (
    id              INTEGER PRIMARY KEY,
    at              INTEGER NOT NULL,
    task_id         INTEGER,
    action          TEXT NOT NULL,
    status          TEXT NOT NULL,
    authorization   TEXT,
    detail          TEXT
);
"#,
    r#"
CREATE TABLE rules (
    id          TEXT NOT NULL,
    version     INTEGER NOT NULL,
    action_kind TEXT NOT NULL,
    conditions  TEXT NOT NULL,
    active      INTEGER NOT NULL,
    created_at  INTEGER NOT NULL,
    PRIMARY KEY (id, version)
);
"#,
];

/// Base de datos local. Cada operación pública se confirma en una transacción,
/// de modo que una interrupción deja el estado anterior o el siguiente, nunca uno intermedio.
pub struct Store {
    pub(crate) conn: Connection,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        Self::init(Connection::open(path)?)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        // WAL no aplica a bases en memoria; SQLite devuelve entonces "memory".
        let _mode: String = conn.query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))?;
        conn.execute_batch("PRAGMA foreign_keys = ON; PRAGMA synchronous = FULL;")?;
        let mut store = Self { conn };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&mut self) -> Result<()> {
        let current: i64 = self
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))?;
        let supported = MIGRATIONS.len() as i64;
        if current > supported {
            return Err(CoreError::UnsupportedSchema {
                found: current,
                supported,
            });
        }
        for (index, sql) in MIGRATIONS.iter().enumerate().skip(current as usize) {
            let tx = self.conn.transaction()?;
            tx.execute_batch(sql)?;
            tx.pragma_update(None, "user_version", index as i64 + 1)?;
            tx.commit()?;
        }
        Ok(())
    }

    pub fn schema_version(&self) -> Result<i64> {
        Ok(self
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))?)
    }
}

pub(crate) fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
