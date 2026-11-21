//! Motor del traslado de la base anterior a los perfiles (D271, D272, D289; R38). Sin la
//! aplicación: trabaja sobre una carpeta de datos y se puede llamar paso a paso.
//!
//! Se retoma desde cualquier punto con el marcador `profile_migration`, una fila de la base
//! anterior cuyo paso solo avanza y que guarda los recuentos, la cadena y la decisión. Cada paso
//! escribe `*.part`, renombra al terminar y, solo después, avanza el marcador; al retomar, un
//! paso sin marcar borra su salida y la rehace. Hasta que existe el puntero (paso 6) no se usa
//! nada nuevo, y la base anterior solo cambia por el marcador y el renombrado final (paso 7).
//!
//! 1. Verificar: integridad, claves foráneas, recuentos y cadena.
//! 2. Copiar a pruebas (solo con datos de facturación): `rusqlite::backup`, mismas
//!    comprobaciones sobre la copia, identidad, base principal de pruebas y `documentos/ventas`.
//! 3. Decisión «¿la empresa y los clientes son reales?» (D271): sin ella el motor se detiene.
//! 4. Base principal normal nueva con las tablas del papel principal, sin las tareas de envío de
//!    venta ni lo que dependa de ellas ni la auditoría de ventas.
//! 5. Obligado 1 del perfil normal, solo si son reales: empresa, logotipos, membretes, clientes,
//!    productos y plantillas recurrentes (inactivas), con sus identificadores.
//! 6. Puntero `perfiles.json`.
//! 7. Renombrar la base anterior a `nestadmin.trasladada.db`.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{params, Connection, OpenFlags, OptionalExtension, TransactionBehavior};
use sha2::{Digest, Sha256};

use super::classify::is_derived;
use super::{
    attach_read_only, build_ledger, build_principal, ensure_no_wal, import_table, part_path,
    publish, read_pointer, remove_db_files, write_pointer_with, DbIdentity, ProfileDirs,
    ProfileKind, LEGACY_DB, LEGACY_RENAMED_DB,
};
use crate::error::{CoreError, Result};
use crate::sales::ChainStatus;
use crate::store::now_ms;
use crate::Store;

/// Respuesta a «¿la empresa y los clientes de esta instalación son reales?» (paso 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Real,
    Ficticia,
}

impl Decision {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Real => "real",
            Self::Ficticia => "ficticia",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        match text {
            "real" => Some(Self::Real),
            "ficticia" => Some(Self::Ficticia),
            _ => None,
        }
    }
}

/// Dónde está el traslado tras ejecutar un paso.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MigrationState {
    /// No queda nada por hacer: hay puntero y la base anterior ya se renombró (o no existía).
    Done,
    /// Hay pasos pendientes; `completed` es el último completado (0 a 6).
    InProgress { completed: u8 },
    /// El motor se detuvo en el paso 3: falta la decisión.
    AwaitingDecision,
}

/// Qué hacer al arrancar según los archivos de la carpeta de datos.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartupState {
    /// Con puntero: se abre el perfil normal, aunque la base antigua siga sin renombrar (el
    /// renombrado se completa después con [`finish_pending_rename`]).
    Ready,
    /// Sin puntero y con `nestadmin.db`: hay que hacer o retomar el traslado.
    NeedsMigration,
    /// Ni puntero ni base anterior: instalación nueva.
    NewInstallation,
}

/// Lo que el traslado dejó fuera o cambió, para que la aplicación lo muestre: la ausencia de
/// datos es explícita.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationReport {
    /// La decisión «¿la empresa y los clientes son reales?» ya guardada, si se llegó a tomar.
    pub decision: Option<Decision>,
    /// Tareas de envío de documentos de venta que no pasaron a la base principal, por estado
    /// (las pendientes de revisión o aprobadas no se envían: el documento vive en el perfil de
    /// pruebas).
    pub discarded_send_tasks: BTreeMap<String, i64>,
    /// Filas de auditoría de ventas (`sales.*` y `verifactu.*`) que no pasaron a la principal.
    pub discarded_sales_audit_rows: i64,
    /// Plantillas recurrentes importadas desactivadas al obligado del perfil normal.
    pub deactivated_recurring_templates: i64,
}

fn failure(code: &'static str, message: impl Into<String>) -> CoreError {
    CoreError::Profile {
        code,
        message: message.into(),
    }
}

/// Estado de arranque según los archivos. Con puntero es siempre `Ready`: una base antigua sin
/// renombrar no impide arrancar.
pub fn startup_state(data_dir: &Path) -> Result<StartupState> {
    if read_pointer(data_dir)?.is_some() {
        return Ok(StartupState::Ready);
    }
    Ok(if data_dir.join(LEGACY_DB).is_file() {
        StartupState::NeedsMigration
    } else {
        StartupState::NewInstallation
    })
}

/// Cerrojo entre procesos del traslado: un archivo en la carpeta de datos abierto en exclusiva
/// mientras dura (en Windows, sin compartir; en otros sistemas, creado con `create_new`).
struct MigrationLock {
    file: Option<File>,
    path: PathBuf,
}

const LOCK_FILE: &str = "migrar.cerrojo";

impl MigrationLock {
    /// `None` si otro proceso (u otro intento de este) lo tiene.
    fn try_acquire(dir: &Path) -> Result<Option<Self>> {
        fs::create_dir_all(dir)?;
        let path = dir.join(LOCK_FILE);
        let mut options = OpenOptions::new();
        options.write(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.create(true).share_mode(0);
        }
        #[cfg(not(windows))]
        options.create_new(true);
        match options.open(&path) {
            Ok(file) => Ok(Some(Self {
                file: Some(file),
                path,
            })),
            Err(e) if is_lock_conflict(&e) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
}

fn is_lock_conflict(e: &std::io::Error) -> bool {
    // 32 y 33: ERROR_SHARING_VIOLATION y ERROR_LOCK_VIOLATION en Windows.
    matches!(
        e.kind(),
        std::io::ErrorKind::AlreadyExists | std::io::ErrorKind::PermissionDenied
    ) || matches!(e.raw_os_error(), Some(32) | Some(33))
}

impl Drop for MigrationLock {
    fn drop(&mut self) {
        drop(self.file.take());
        // Si otro proceso lo abrió entre medias, el borrado falla y no pasa nada.
        let _ = fs::remove_file(&self.path);
    }
}

fn lock_or_fail(dir: &Path) -> Result<MigrationLock> {
    MigrationLock::try_acquire(dir)?.ok_or_else(|| {
        failure(
            "migration_locked",
            "otro proceso está trasladando los datos; espere a que termine",
        )
    })
}

/// Ejecuta el siguiente paso pendiente. Sin base anterior ni puntero crea una instalación nueva;
/// con puntero intenta además cerrar un traslado a medias (paso 7). `decision` solo se usa en el
/// paso 3 y, una vez guardada en el marcador, no puede cambiar. Con el cerrojo tomado por otro
/// proceso devuelve `migration_locked`.
pub fn migrate_step(data_dir: &Path, decision: Option<Decision>) -> Result<MigrationState> {
    step_inner(data_dir, decision, &mut |_| false)
}

/// Ejecuta pasos hasta terminar o hasta que falte la decisión, con el cerrojo tomado todo el
/// tiempo.
pub fn migrate(data_dir: &Path, decision: Option<Decision>) -> Result<MigrationState> {
    migrate_inner(data_dir, decision, &mut |_| false)
}

fn step_inner(
    data_dir: &Path,
    decision: Option<Decision>,
    hook: &mut dyn FnMut(&str) -> bool,
) -> Result<MigrationState> {
    let _lock = lock_or_fail(data_dir)?;
    run_step(&mut Ctx::new(data_dir, hook), decision)
}

fn migrate_inner(
    data_dir: &Path,
    decision: Option<Decision>,
    hook: &mut dyn FnMut(&str) -> bool,
) -> Result<MigrationState> {
    let _lock = lock_or_fail(data_dir)?;
    let mut ctx = Ctx::new(data_dir, hook);
    loop {
        match run_step(&mut ctx, decision)? {
            MigrationState::InProgress { .. } => {}
            other => return Ok(other),
        }
    }
}

/// Como [`migrate_step`], con un gancho que se llama en cada punto de corte con su etiqueta; si
/// devuelve `true` el paso se interrumpe con el error `interrupted`, como un corte del proceso.
/// Solo para pruebas de recuperación (característica `test-hooks`).
#[cfg(feature = "test-hooks")]
pub fn migrate_step_with_hook(
    data_dir: &Path,
    decision: Option<Decision>,
    hook: &mut dyn FnMut(&str) -> bool,
) -> Result<MigrationState> {
    step_inner(data_dir, decision, hook)
}

/// Como [`migrate`], con el gancho de [`migrate_step_with_hook`].
#[cfg(feature = "test-hooks")]
pub fn migrate_with_hook(
    data_dir: &Path,
    decision: Option<Decision>,
    hook: &mut dyn FnMut(&str) -> bool,
) -> Result<MigrationState> {
    migrate_inner(data_dir, decision, hook)
}

/// Intenta una sola vez, sin esperar, el renombrado del paso 7 si el puntero ya existe y la base
/// antigua sigue sin renombrar con el marcador en el paso 5 o más. Devuelve `true` si no queda
/// nada pendiente. Un archivo bloqueado, un cerrojo tomado o cualquier fallo devuelven `false`:
/// la aplicación arranca igual y lo vuelve a intentar en el siguiente arranque.
pub fn finish_pending_rename(data_dir: &Path) -> Result<bool> {
    if read_pointer(data_dir)?.is_none() {
        return Ok(false);
    }
    let legacy = data_dir.join(LEGACY_DB);
    if !legacy.is_file() {
        return Ok(true);
    }
    let Some(_lock) = MigrationLock::try_acquire(data_dir)? else {
        return Ok(false);
    };
    Ok(finish_after_pointer(data_dir, &legacy))
}

/// Cierra un traslado con puntero y sin renombrar; `true` si lo consiguió. No falla.
fn finish_after_pointer(data_dir: &Path, legacy: &Path) -> bool {
    let Ok(Some(marker)) = read_legacy_marker(legacy) else {
        return false;
    };
    if marker.step < 5 {
        return false;
    }
    if marker.step < 6 && set_step(legacy, 6, None).is_err() {
        return false;
    }
    try_rename_legacy(legacy, &data_dir.join(LEGACY_RENAMED_DB)).is_ok()
}

/// Lo que el traslado dejó fuera, leído de la base antigua (renombrada o no). `None` si no hay
/// base antigua o no llegó a empezar el traslado.
pub fn migration_report(data_dir: &Path) -> Result<Option<MigrationReport>> {
    let path = [LEGACY_DB, LEGACY_RENAMED_DB]
        .iter()
        .map(|n| data_dir.join(n))
        .find(|p| p.is_file());
    let Some(path) = path else {
        return Ok(None);
    };
    let conn = Connection::open_with_flags(
        &path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_URI,
    )?;
    let Some(marker) = read_marker(&conn)? else {
        return Ok(None);
    };
    let mut discarded_send_tasks = BTreeMap::new();
    {
        let mut stmt = conn.prepare(
            "SELECT state, count(*) FROM tasks WHERE kind = 'send_sales_document' \
             GROUP BY state ORDER BY state",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
        for row in rows {
            let (state, n) = row?;
            discarded_send_tasks.insert(state, n);
        }
    }
    let discarded_sales_audit_rows: i64 = conn.query_row(
        "SELECT count(*) FROM audit_log WHERE action LIKE 'sales.%' OR action LIKE 'verifactu.%'",
        [],
        |r| r.get(0),
    )?;
    let deactivated_recurring_templates =
        if marker.has_billing() && marker.decision == Some(Decision::Real) {
            conn.query_row(
                "SELECT count(*) FROM recurring_templates WHERE active = 1",
                [],
                |r| r.get(0),
            )?
        } else {
            0
        };
    Ok(Some(MigrationReport {
        decision: marker.decision,
        discarded_send_tasks,
        discarded_sales_audit_rows,
        deactivated_recurring_templates,
    }))
}

struct Ctx<'a> {
    dir: &'a Path,
    hook: &'a mut dyn FnMut(&str) -> bool,
}

impl<'a> Ctx<'a> {
    fn new(dir: &'a Path, hook: &'a mut dyn FnMut(&str) -> bool) -> Self {
        Self { dir, hook }
    }

    fn cut(&mut self, label: &str) -> Result<()> {
        if (self.hook)(label) {
            Err(failure("interrupted", label))
        } else {
            Ok(())
        }
    }
}

// --- Marcador ---

struct Marker {
    step: u8,
    decision: Option<Decision>,
    counts: BTreeMap<String, i64>,
    chain: ChainInfo,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ChainInfo {
    state: &'static str,
    head: Option<String>,
    digest: String,
}

/// Tablas cuyos datos significan que la instalación llegó a facturar o a preparar facturación.
const BILLING_TABLES: [&str; 7] = [
    "company",
    "customers",
    "products",
    "series",
    "recurring_templates",
    "sales_documents",
    "verifactu_records",
];

impl Marker {
    fn has_billing(&self) -> bool {
        BILLING_TABLES
            .iter()
            .any(|t| self.counts.get(*t).copied().unwrap_or(0) > 0)
    }
}

/// Columnas del marcador: paso, decisión, recuentos, estado, última huella y resumen de la cadena.
type MarkerRow = (i64, Option<String>, String, String, Option<String>, String);

fn read_marker(conn: &Connection) -> Result<Option<Marker>> {
    let exists = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'profile_migration'",
            [],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    if !exists {
        return Ok(None);
    }
    let row: Option<MarkerRow> = conn
        .query_row(
            "SELECT step, decision, counts_json, chain_state, chain_head, chain_digest \
             FROM profile_migration WHERE id = 1",
            [],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                ))
            },
        )
        .optional()?;
    let Some((step, decision, counts, state, head, digest)) = row else {
        return Ok(None);
    };
    let state = match state.as_str() {
        "empty" => "empty",
        "ok" => "ok",
        _ => "broken",
    };
    Ok(Some(Marker {
        step: step as u8,
        decision: decision.as_deref().and_then(Decision::parse),
        counts: serde_json::from_str(&counts)?,
        chain: ChainInfo {
            state,
            head,
            digest,
        },
    }))
}

/// Marcador de una base anterior sin migrarla ni escribir. La conexión es de lectura y escritura
/// solo para que, al cerrarse, no deje restos del WAL (una de solo lectura los deja).
fn read_legacy_marker(path: &Path) -> Result<Option<Marker>> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_URI,
    )?;
    read_marker(&conn)
}

fn set_step(legacy: &Path, step: u8, decision: Option<Decision>) -> Result<()> {
    let conn = Connection::open(legacy)?;
    conn.busy_timeout(Duration::from_secs(5))?;
    conn.execute(
        "UPDATE profile_migration SET step = ?1, updated_at = ?2, \
         decision = COALESCE(decision, ?3) WHERE id = 1 AND step < ?1",
        params![step, now_ms(), decision.map(Decision::as_str)],
    )?;
    conn.close().map_err(|(_, e)| CoreError::Storage(e))
}

// --- Comprobaciones ---

/// Recuento de filas de cada tabla (sin las derivadas ni el andamiaje del traslado, que cambian
/// a propósito entre la base anterior y su copia).
fn count_tables(conn: &Connection) -> Result<BTreeMap<String, i64>> {
    let names: Vec<String> = {
        let mut stmt = conn.prepare(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' \
             ORDER BY name",
        )?;
        let rows = stmt.query_map([], |r| r.get(0))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    let mut counts = BTreeMap::new();
    for name in names {
        if is_derived(&name) || name == "profile_migration" || name == "database_identity" {
            continue;
        }
        let n: i64 = conn.query_row(&format!("SELECT count(*) FROM \"{name}\""), [], |r| {
            r.get(0)
        })?;
        counts.insert(name, n);
    }
    Ok(counts)
}

fn chain_info(store: &Store) -> Result<ChainInfo> {
    let status = store
        .verify_verifactu_chain()
        .map_err(|e| failure("chain_unreadable", e.to_string()))?;
    let state = match status {
        ChainStatus::Empty => "empty",
        ChainStatus::Ok { .. } => "ok",
        ChainStatus::Broken { .. } => "broken",
    };
    let mut stmt = store
        .conn
        .prepare("SELECT hash FROM verifactu_records ORDER BY id")?;
    let hashes: Vec<String> = stmt
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    let mut hasher = Sha256::new();
    for hash in &hashes {
        hasher.update(hash.as_bytes());
        hasher.update(b"\n");
    }
    let digest = hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    Ok(ChainInfo {
        state,
        head: hashes.last().cloned(),
        digest,
    })
}

fn check_integrity(conn: &Connection) -> Result<()> {
    let verdict: String = conn.query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
    if verdict != "ok" {
        return Err(failure(
            "migration_integrity",
            "la base de datos no pasa la comprobación de integridad; el traslado no continúa",
        ));
    }
    check_foreign_keys(conn)
}

fn check_foreign_keys(conn: &Connection) -> Result<()> {
    let mut stmt = conn.prepare("PRAGMA foreign_key_check")?;
    if stmt.exists([])? {
        return Err(failure(
            "migration_integrity",
            "hay filas con una referencia a otra que no existe",
        ));
    }
    Ok(())
}

// --- Pasos ---

fn run_step(ctx: &mut Ctx<'_>, decision: Option<Decision>) -> Result<MigrationState> {
    let dir = ctx.dir;
    let pointer = read_pointer(dir)?.is_some();
    let legacy = dir.join(LEGACY_DB);
    if !legacy.is_file() {
        if !pointer {
            new_installation(ctx)?;
        }
        return Ok(MigrationState::Done);
    }
    if pointer {
        // Puntero escrito y base anterior sin renombrar: solo falta el paso 7, con sus reintentos.
        // El arranque no pasa por aquí (con puntero es `Ready` y usa `finish_pending_rename`).
        return match read_legacy_marker(&legacy)? {
            Some(marker) if marker.step >= 5 => {
                if marker.step < 6 {
                    set_step(&legacy, 6, None)?;
                }
                step7(ctx, &legacy)?;
                Ok(MigrationState::Done)
            }
            _ => Ok(MigrationState::Done),
        };
    }

    // Abrir la base anterior la lleva al esquema actual (con su copia previa, D61): hace falta
    // para que exista el marcador.
    let marker = {
        let store = Store::open(&legacy)?;
        let marker = read_marker(&store.conn)?;
        store.finish()?;
        marker
    };
    let completed = marker.as_ref().map(|m| m.step).unwrap_or(0);
    let next = completed + 1;
    match (next, marker) {
        (1, _) => step1(ctx, &legacy)?,
        (2, Some(m)) => step2(ctx, &legacy, &m)?,
        (3, Some(m)) => {
            if !step3(&legacy, &m, decision)? {
                return Ok(MigrationState::AwaitingDecision);
            }
            ctx.cut("step3:done")?;
        }
        (4, Some(m)) => step4(ctx, &legacy, &m)?,
        (5, Some(m)) => step5(ctx, &legacy, &m)?,
        (6, Some(m)) => step6(ctx, &legacy, &m)?,
        (7.., _) => step7(ctx, &legacy)?,
        _ => {
            return Err(failure(
                "migration_state",
                "el marcador del traslado no es coherente",
            ))
        }
    }
    Ok(if next >= 7 {
        MigrationState::Done
    } else {
        MigrationState::InProgress { completed: next }
    })
}

/// Un registro de producción aceptado por la AEAT no se puede archivar en pruebas: el traslado se
/// detiene antes de escribir nada. (Hoy no existe: la remisión a producción llega con T18b.)
fn ensure_no_production_history(conn: &Connection) -> Result<()> {
    let accepted: i64 = conn.query_row(
        "SELECT count(*) FROM verifactu_record_environments e \
         JOIN verifactu_record_results r ON r.record_id = e.record_id \
         JOIN verifactu_batches b ON b.id = r.batch_id \
         WHERE e.environment = 'produccion' AND b.environment = 'produccion' \
           AND r.status IN ('accepted', 'accepted_with_errors')",
        [],
        |r| r.get(0),
    )?;
    if accepted > 0 {
        return Err(failure(
            "production_history_present",
            "la base tiene registros aceptados en producción: no se archivan en pruebas",
        ));
    }
    Ok(())
}

/// La copia de pruebas no puede quedar con el entorno `produccion` vigente: si lo está, se añade
/// una fila `simulador` (el trigger de entorno solo vigila las inserciones y la copia llevaría la
/// última fila tal cual).
fn ensure_test_environment(store: &Store) -> Result<()> {
    let current: Option<String> = store
        .conn
        .query_row(
            "SELECT environment FROM verifactu_environment_changes ORDER BY id DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(env) = current {
        if env != "simulador" && env != "preproduccion" {
            store.conn.execute(
                "INSERT INTO verifactu_environment_changes (environment, authorization, \
                 changed_at) VALUES ('simulador', 'traslado (D272)', ?1)",
                [now_ms()],
            )?;
        }
    }
    Ok(())
}

fn step1(ctx: &mut Ctx<'_>, legacy: &Path) -> Result<()> {
    let store = Store::open(legacy)?;
    check_integrity(&store.conn)?;
    ensure_no_production_history(&store.conn)?;
    let counts = count_tables(&store.conn)?;
    // Una cadena rota no detiene el traslado: se archiva en pruebas tal como está.
    let chain = chain_info(&store)?;
    store.conn.execute(
        "INSERT OR IGNORE INTO profile_migration \
         (id, step, decision, counts_json, chain_state, chain_head, chain_digest, started_at, \
          updated_at) VALUES (1, 1, NULL, ?1, ?2, ?3, ?4, ?5, ?5)",
        params![
            serde_json::to_string(&counts)?,
            chain.state,
            chain.head,
            chain.digest,
            now_ms()
        ],
    )?;
    store.finish()?;
    ctx.cut("step1:done")
}

fn step2(ctx: &mut Ctx<'_>, legacy: &Path, marker: &Marker) -> Result<()> {
    if marker.has_billing() {
        let dir = ctx.dir;
        let test = ProfileDirs::of(dir, ProfileKind::Pruebas);

        // Copia de la base entera como obligado 1 del perfil de pruebas.
        let target = test.ledger_db(1);
        let part = part_path(&target);
        fs::create_dir_all(test.ledger_dir(1))?;
        remove_db_files(&part)?;
        remove_db_files(&target)?;
        backup_to(legacy, &part)?;
        let mut copy = Store::open(&part)?;
        if let Err(e) = check_integrity(&copy.conn) {
            drop(copy);
            remove_db_files(&part)?;
            return Err(e);
        }
        let copied_counts = count_tables(&copy.conn)?;
        let copied_chain = chain_info(&copy)?;
        if copied_counts != marker.counts || copied_chain != marker.chain {
            drop(copy);
            remove_db_files(&part)?;
            return Err(failure(
                "migration_copy_mismatch",
                "la copia no coincide con la base original (recuentos o cadena)",
            ));
        }
        copy.set_identity(&DbIdentity::obligado(ProfileKind::Pruebas, 1))?;
        ensure_test_environment(&copy)?;
        copy.finish()?;
        ctx.cut("step2:copy-written")?;
        publish(&part, &target)?;
        ctx.cut("step2:copy-renamed")?;

        // Base principal del perfil de pruebas, con el alta del obligado 1.
        let principal = test.principal_db();
        let principal_part = part_path(&principal);
        build_principal(
            &principal_part,
            ProfileKind::Pruebas,
            Some((1, "copia de la instalación anterior")),
        )?
        .finish()?;
        remove_db_files(&principal)?;
        ctx.cut("step2:principal-written")?;
        publish(&principal_part, &principal)?;
        ctx.cut("step2:principal-renamed")?;

        // Los PDF de venta viajan con la copia (se pueden regenerar, D226).
        move_sales_documents(dir, &test.sales_dir(1))?;
        ctx.cut("step2:ventas-moved")?;
    }
    set_step(legacy, 2, None)?;
    ctx.cut("step2:done")
}

/// Copia coherente de una base con la API de copia de SQLite (no del archivo, por el WAL).
fn backup_to(source: &Path, target: &Path) -> Result<()> {
    let src = Connection::open_with_flags(
        source,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_URI,
    )?;
    let mut dst = Connection::open(target)?;
    {
        let backup = rusqlite::backup::Backup::new(&src, &mut dst)?;
        backup.run_to_completion(64, Duration::from_millis(25), None)?;
    }
    dst.close().map_err(|(_, e)| CoreError::Storage(e))?;
    sync_part(target)
}

fn sync_part(path: &Path) -> Result<()> {
    super::sync_file(path)
}

/// Mueve `documentos/ventas` a `target`. Es repetible: lo que ya está en destino no se pisa.
fn move_sales_documents(dir: &Path, target: &Path) -> Result<()> {
    let source = dir.join("documentos").join("ventas");
    if !source.is_dir() {
        return Ok(());
    }
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    if !target.exists() {
        fs::rename(&source, target)?;
        return Ok(());
    }
    for entry in fs::read_dir(&source)? {
        let entry = entry?;
        let to = target.join(entry.file_name());
        if !to.exists() {
            fs::rename(entry.path(), to)?;
        }
    }
    if fs::read_dir(&source)?.next().is_none() {
        fs::remove_dir(&source)?;
    }
    Ok(())
}

/// Paso 3. `true` si el paso quedó completo; `false` si falta la decisión.
fn step3(legacy: &Path, marker: &Marker, decision: Option<Decision>) -> Result<bool> {
    if !marker.has_billing() {
        set_step(legacy, 3, None)?;
        return Ok(true);
    }
    let effective = match (marker.decision, decision) {
        (Some(stored), Some(given)) if stored != given => {
            return Err(failure(
                "decision_conflict",
                "el traslado ya tiene una decisión distinta guardada",
            ))
        }
        (stored, given) => stored.or(given),
    };
    match effective {
        None => Ok(false),
        Some(effective) => {
            set_step(legacy, 3, Some(effective))?;
            Ok(true)
        }
    }
}

/// Tablas del papel principal que se importan a la base principal normal, con su filtro. Las
/// tareas de envío de venta y todo lo que dependa de ellas, y la auditoría de ventas, quedan
/// fuera: viven en el obligado y en el perfil de pruebas.
/// Tablas del papel principal que NO se importan a la principal normal: los envíos de venta y sus
/// disparos viven en el perfil de pruebas con el documento al que se refieren (D287), y el
/// registro de obligados se crea de nuevo (D289). Una prueba exige que las tablas `Principal` de
/// [`super::classify`] sean exactamente la unión de las importadas y estas.
pub const PRINCIPAL_EXCLUDED: &[&str] =
    &["sales_document_sends", "sales_send_triggers", "taxpayers"];

/// Tablas que se importan a la base principal normal.
pub fn principal_import_tables() -> Vec<&'static str> {
    PRINCIPAL_IMPORT.iter().map(|(t, _)| *t).collect()
}

const TASK_FILTER: &str = "task_id IN (SELECT id FROM main.tasks)";
const PRINCIPAL_IMPORT: &[(&str, Option<&str>)] = &[
    ("tasks", Some("kind <> 'send_sales_document'")),
    ("task_transitions", Some(TASK_FILTER)),
    ("actions", Some(TASK_FILTER)),
    ("rules", None),
    ("rule_meta", None),
    ("rule_evaluations", Some(TASK_FILTER)),
    (
        "rule_signals",
        Some("task_id IS NULL OR task_id IN (SELECT id FROM main.tasks)"),
    ),
    ("rule_suggestions", None),
    ("rule_settings", None),
    ("sender_preferences", None),
    ("business_profile", None),
    ("reply_style", None),
    ("task_templates", None),
    (
        "pipeline_runs",
        Some("subject_task_id IN (SELECT id FROM main.tasks)"),
    ),
    (
        "pipeline_steps",
        Some(
            "run_id IN (SELECT id FROM main.pipeline_runs) \
             AND (task_id IS NULL OR task_id IN (SELECT id FROM main.tasks))",
        ),
    ),
    ("search_index", Some(TASK_FILTER)),
    ("assistant_conversations", None),
    ("assistant_turns", None),
    ("assistant_calls", None),
    (
        "audit_log",
        Some(
            "action NOT LIKE 'sales.%' AND action NOT LIKE 'verifactu.%' \
             AND (task_id IS NULL OR task_id IN (SELECT id FROM main.tasks))",
        ),
    ),
];

fn step4(ctx: &mut Ctx<'_>, legacy: &Path, marker: &Marker) -> Result<()> {
    let normal = ProfileDirs::of(ctx.dir, ProfileKind::Normal);
    let target = normal.principal_db();
    let part = part_path(&target);
    remove_db_files(&target)?;
    // El alta del obligado 1 del perfil normal solo existe si la decisión es «reales».
    let real = marker.has_billing() && marker.decision == Some(Decision::Real);
    let first = real.then_some((1, "obligado de la instalación anterior"));
    let mut store = build_principal(&part, ProfileKind::Normal, first)?;

    attach_read_only(&store.conn, legacy, "old")?;
    {
        let tx = store
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute_batch("PRAGMA defer_foreign_keys = ON")?;
        for (table, filter) in PRINCIPAL_IMPORT {
            let inserted = import_table(&tx, table, *filter, None)? as i64;
            let original = marker.counts.get(*table).copied().unwrap_or(0);
            // Lo insertado más lo que el filtro dejó fuera (filas de tareas excluidas o de
            // auditoría de ventas) debe cuadrar con el recuento original.
            let left_out: i64 = match filter {
                None => 0,
                Some(f) => tx.query_row(
                    &format!("SELECT count(*) FROM old.\"{table}\" WHERE NOT COALESCE(({f}), 0)"),
                    [],
                    |r| r.get(0),
                )?,
            };
            if inserted + left_out != original {
                return Err(failure(
                    "migration_import_mismatch",
                    format!("la importación de {table} no cuadra con el recuento original"),
                ));
            }
        }
        tx.commit()?;
    }
    store.conn.execute_batch("DETACH DATABASE old")?;
    check_foreign_keys(&store.conn)?;
    store.finish()?;
    ctx.cut("step4:part-written")?;
    publish(&part, &target)?;
    ctx.cut("step4:renamed")?;
    set_step(legacy, 4, None)?;
    ctx.cut("step4:done")
}

fn step5(ctx: &mut Ctx<'_>, legacy: &Path, marker: &Marker) -> Result<()> {
    if marker.has_billing() && marker.decision == Some(Decision::Real) {
        let normal = ProfileDirs::of(ctx.dir, ProfileKind::Normal);
        let target = normal.ledger_db(1);
        let part = part_path(&target);
        remove_db_files(&target)?;
        let mut store = build_ledger(&part, ProfileKind::Normal, 1)?;
        attach_read_only(&store.conn, legacy, "old")?;
        {
            let tx = store
                .conn
                .transaction_with_behavior(TransactionBehavior::Immediate)?;
            tx.execute_batch("PRAGMA defer_foreign_keys = ON")?;
            // Sin series ni documentos: la numeración y la cadena del obligado empiezan de cero.
            for table in [
                "company_logos",
                "company_letterheads",
                "company",
                "customers",
                "products",
            ] {
                let inserted = import_table(&tx, table, None, None)? as i64;
                if inserted != marker.counts.get(table).copied().unwrap_or(0) {
                    return Err(failure(
                        "migration_import_mismatch",
                        format!("la importación de {table} no coincide con el recuento original"),
                    ));
                }
            }
            // Las plantillas recurrentes llegan inactivas: no deben generar nada sin que el
            // obligado las revise.
            let inserted = import_table(&tx, "recurring_templates", None, Some(("active", "0")))?;
            if inserted as i64
                != marker
                    .counts
                    .get("recurring_templates")
                    .copied()
                    .unwrap_or(0)
            {
                return Err(failure(
                    "migration_import_mismatch",
                    "la importación de recurring_templates no coincide con el recuento original",
                ));
            }
            tx.commit()?;
        }
        store.conn.execute_batch("DETACH DATABASE old")?;
        check_foreign_keys(&store.conn)?;
        store.finish()?;
        ctx.cut("step5:part-written")?;
        publish(&part, &target)?;
        ctx.cut("step5:renamed")?;
    }
    set_step(legacy, 5, None)?;
    ctx.cut("step5:done")
}

fn step6(ctx: &mut Ctx<'_>, legacy: &Path, marker: &Marker) -> Result<()> {
    let dir = ctx.dir;
    let normal = ProfileDirs::of(dir, ProfileKind::Normal);
    Store::open_as(
        &normal.principal_db(),
        &DbIdentity::principal(ProfileKind::Normal),
    )?
    .finish()?;
    if marker.has_billing() && marker.decision == Some(Decision::Real) {
        Store::open_as(
            &normal.ledger_db(1),
            &DbIdentity::obligado(ProfileKind::Normal, 1),
        )?
        .finish()?;
    }
    write_pointer_with(dir, &mut || ctx.cut("step6:pointer-tmp"))?;
    ctx.cut("step6:pointer-renamed")?;
    set_step(legacy, 6, None)?;
    ctx.cut("step6:done")
}

fn step7(ctx: &mut Ctx<'_>, legacy: &Path) -> Result<()> {
    let target: PathBuf = ctx.dir.join(LEGACY_RENAMED_DB);
    let mut last = None;
    for attempt in 0..10 {
        // Un antivirus o el indexador pueden tener el archivo un instante: se reintenta.
        match try_rename_legacy(legacy, &target) {
            Ok(()) => {
                last = None;
                break;
            }
            Err(e) => {
                last = Some(e);
                if attempt < 9 {
                    std::thread::sleep(Duration::from_millis(200));
                }
            }
        }
    }
    if let Some(e) = last {
        return Err(e);
    }
    // La base archivada no se vuelve a escribir: el marcador se queda en el paso 6.
    ctx.cut("step7:renamed")
}

/// Renombra la base antigua una vez, sin dejar un `-wal` con filas sin volcar.
fn try_rename_legacy(legacy: &Path, target: &Path) -> Result<()> {
    ensure_no_wal(legacy)?;
    fs::rename(legacy, target)?;
    Ok(())
}

/// Instalación nueva: base principal vacía del perfil normal y puntero.
fn new_installation(ctx: &mut Ctx<'_>) -> Result<()> {
    let dir = ctx.dir;
    let normal = ProfileDirs::of(dir, ProfileKind::Normal);
    let target = normal.principal_db();
    if target.exists() {
        // Un arranque anterior llegó a publicar la base y se cortó antes del puntero.
        Store::open_as(&target, &DbIdentity::principal(ProfileKind::Normal))?.finish()?;
    } else {
        let part = part_path(&target);
        build_principal(&part, ProfileKind::Normal, None)?.finish()?;
        ctx.cut("new:part-written")?;
        publish(&part, &target)?;
        ctx.cut("new:renamed")?;
    }
    write_pointer_with(dir, &mut || ctx.cut("new:pointer-tmp"))?;
    ctx.cut("new:done")
}
