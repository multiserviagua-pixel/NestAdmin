//! Perfiles de datos, identidad de cada base y creación de bases (D271, D273, D287-D290, R37,
//! R38). Este módulo no conoce la aplicación: resuelve carpetas, escribe el puntero
//! `perfiles.json`, comprueba que una base es la esperada y crea bases principales y de
//! obligado nuevas. El traslado de la base anterior está en [`migrate`].
//!
//! Disposición en disco (el perfil normal es la propia carpeta de datos, porque la entrada de
//! cada tarea guarda la ruta absoluta del documento):
//!
//! ```text
//! <datos>/                    perfil normal
//!   perfiles.json             puntero (su escritura confirma el traslado)
//!   principal.db  correo/  documentos/
//!   obligados/<id>/obligado.db  ventas/  certificados/
//!   nestadmin.trasladada.db   base anterior, intacta
//!   perfiles/pruebas/         principal.db  obligados/<id>/...
//! ```

pub mod classify;
pub mod migrate;

use std::fs::{self, File};
use std::io::Write as _;
use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OpenFlags, OptionalExtension};

use crate::error::{CoreError, Result};
use crate::store::now_ms;
use crate::Store;

pub use classify::{class_of, TableClass, CLASSIFICATION};
pub use migrate::{
    finish_pending_rename, migrate, migrate_step, migration_report, principal_import_tables,
    startup_state, Decision, MigrationReport, MigrationState, StartupState, PRINCIPAL_EXCLUDED,
};

/// Puntero de perfil: existe cuando el traslado terminó o la instalación es nueva.
pub const POINTER_FILE: &str = "perfiles.json";
/// Base de datos anterior a los perfiles.
pub const LEGACY_DB: &str = "nestadmin.db";
/// Nombre de la base anterior una vez trasladada.
pub const LEGACY_RENAMED_DB: &str = "nestadmin.trasladada.db";
pub const PRINCIPAL_DB: &str = "principal.db";
pub const LEDGER_DB: &str = "obligado.db";
/// Código de error de una base que no es la esperada.
pub const CODE_WRONG_DATABASE: &str = "wrong_database";

/// Perfil de datos de una base (D271).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileKind {
    Normal,
    Pruebas,
}

impl ProfileKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Pruebas => "pruebas",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        match text {
            "normal" => Some(Self::Normal),
            "pruebas" => Some(Self::Pruebas),
            _ => None,
        }
    }
}

/// Papel de una base: la principal (tareas, correo, reglas) o la de un obligado (D287).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DbRole {
    Principal,
    Obligado,
}

impl DbRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Principal => "principal",
            Self::Obligado => "obligado",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        match text {
            "principal" => Some(Self::Principal),
            "obligado" => Some(Self::Obligado),
            _ => None,
        }
    }
}

/// Lo que una base dice ser (fila inmutable `database_identity`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DbIdentity {
    pub profile: ProfileKind,
    pub role: DbRole,
    /// Solo en una base de obligado.
    pub taxpayer_id: Option<i64>,
}

impl DbIdentity {
    pub fn principal(profile: ProfileKind) -> Self {
        Self {
            profile,
            role: DbRole::Principal,
            taxpayer_id: None,
        }
    }

    pub fn obligado(profile: ProfileKind, taxpayer_id: i64) -> Self {
        Self {
            profile,
            role: DbRole::Obligado,
            taxpayer_id: Some(taxpayer_id),
        }
    }

    /// Texto sin datos personales: `normal/principal`, `pruebas/obligado/3`.
    pub fn describe(&self) -> String {
        match self.taxpayer_id {
            Some(id) => format!("{}/{}/{id}", self.profile.as_str(), self.role.as_str()),
            None => format!("{}/{}", self.profile.as_str(), self.role.as_str()),
        }
    }
}

/// Carpetas de un perfil.
#[derive(Debug, Clone)]
pub struct ProfileDirs {
    root: PathBuf,
}

impl ProfileDirs {
    /// El perfil normal es la propia carpeta de datos; el de pruebas, `perfiles/pruebas/`.
    pub fn of(data_dir: &Path, kind: ProfileKind) -> Self {
        let root = match kind {
            ProfileKind::Normal => data_dir.to_path_buf(),
            ProfileKind::Pruebas => data_dir.join("perfiles").join("pruebas"),
        };
        Self { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn principal_db(&self) -> PathBuf {
        self.root.join(PRINCIPAL_DB)
    }

    pub fn ledgers_dir(&self) -> PathBuf {
        self.root.join("obligados")
    }

    pub fn ledger_dir(&self, taxpayer_id: i64) -> PathBuf {
        self.ledgers_dir().join(taxpayer_id.to_string())
    }

    pub fn ledger_db(&self, taxpayer_id: i64) -> PathBuf {
        self.ledger_dir(taxpayer_id).join(LEDGER_DB)
    }

    /// PDF de los documentos de venta de un obligado.
    pub fn sales_dir(&self, taxpayer_id: i64) -> PathBuf {
        self.ledger_dir(taxpayer_id).join("ventas")
    }

    /// Almacén de certificados de un obligado.
    pub fn certificates_dir(&self, taxpayer_id: i64) -> PathBuf {
        self.ledger_dir(taxpayer_id).join("certificados")
    }

    /// Ubicación de un obligado relativa a la carpeta del perfil, como la guarda `taxpayers`.
    pub fn ledger_location(taxpayer_id: i64) -> String {
        format!("obligados/{taxpayer_id}")
    }
}

// --- Puntero ---

/// Contenido de `perfiles.json`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Pointer {
    pub version: u32,
}

fn pointer_error(message: impl Into<String>) -> CoreError {
    CoreError::Profile {
        code: "pointer_invalid",
        message: message.into(),
    }
}

/// Lee el puntero. `None` si no existe; un puntero ilegible o de otra versión es un error, no
/// una instalación nueva.
pub fn read_pointer(data_dir: &Path) -> Result<Option<Pointer>> {
    let path = data_dir.join(POINTER_FILE);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let pointer: Pointer =
        serde_json::from_str(&text).map_err(|_| pointer_error("perfiles.json no se puede leer"))?;
    if pointer.version != 1 {
        return Err(pointer_error(format!(
            "perfiles.json es de la versión {}, que esta compilación no conoce",
            pointer.version
        )));
    }
    Ok(Some(pointer))
}

/// Escribe el puntero de forma atómica: `.tmp`, `sync_all` y renombrado. Ese renombrado es la
/// confirmación del traslado (D289).
pub fn write_pointer(data_dir: &Path) -> Result<()> {
    write_pointer_with(data_dir, &mut || Ok(()))
}

/// Como [`write_pointer`], con un gancho entre el `.tmp` ya en disco y el renombrado.
pub(crate) fn write_pointer_with(
    data_dir: &Path,
    between: &mut dyn FnMut() -> Result<()>,
) -> Result<()> {
    fs::create_dir_all(data_dir)?;
    let tmp = data_dir.join(format!("{POINTER_FILE}.tmp"));
    let mut file = File::create(&tmp)?;
    file.write_all(br#"{"version":1}"#)?;
    file.sync_all()?;
    drop(file);
    between()?;
    fs::rename(&tmp, data_dir.join(POINTER_FILE))?;
    Ok(())
}

// --- Identidad ---

fn has_table(conn: &Connection, name: &str) -> Result<bool> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
            [name],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

fn identity_of(conn: &Connection) -> Result<Option<DbIdentity>> {
    if !has_table(conn, "database_identity")? {
        return Ok(None);
    }
    let row: Option<(String, String, Option<i64>)> = conn
        .query_row(
            "SELECT profile, role, taxpayer_id FROM database_identity WHERE id = 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let Some((profile, role, taxpayer_id)) = row else {
        return Ok(None);
    };
    match (ProfileKind::parse(&profile), DbRole::parse(&role)) {
        (Some(profile), Some(role)) => Ok(Some(DbIdentity {
            profile,
            role,
            taxpayer_id,
        })),
        _ => Err(CoreError::Profile {
            code: "identity_invalid",
            message: "la identidad de la base no es válida".into(),
        }),
    }
}

/// Lee la identidad de una base sin abrirla como [`Store`] (sin migrarla ni crear copias):
/// `None` si no tiene identidad (una base anterior a los perfiles).
pub fn read_identity(path: &Path) -> Result<Option<DbIdentity>> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_URI,
    )?;
    identity_of(&conn)
}

fn wrong_database(expected: &DbIdentity, found: impl Into<String>) -> CoreError {
    CoreError::WrongDatabase {
        expected: expected.describe(),
        found: found.into(),
    }
}

/// Una fila de `taxpayers`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaxpayerEntry {
    pub taxpayer_id: i64,
    pub label: String,
    pub location: String,
    /// `false` tras la baja.
    pub active: bool,
}

impl Store {
    /// Abre una base comprobando antes, sin migrarla, que es la esperada: un archivo que no
    /// existe, que no tiene identidad o que dice ser otra cosa falla con `wrong_database`.
    pub fn open_as(path: &Path, expected: &DbIdentity) -> Result<Self> {
        if !path.is_file() {
            return Err(wrong_database(expected, "ausente"));
        }
        match read_identity(path)? {
            Some(found) if found == *expected => {}
            Some(found) => return Err(wrong_database(expected, found.describe())),
            None => return Err(wrong_database(expected, "sin identidad")),
        }
        let store = Self::open(path)?;
        // Otra comprobación tras abrir: entre la lectura y la apertura el archivo pudo cambiar.
        match store.identity()? {
            Some(found) if found == *expected => Ok(store),
            Some(found) => Err(wrong_database(expected, found.describe())),
            None => Err(wrong_database(expected, "sin identidad")),
        }
    }

    /// Identidad de esta base, si la tiene.
    pub fn identity(&self) -> Result<Option<DbIdentity>> {
        identity_of(&self.conn)
    }

    /// Escribe la identidad, una sola vez (la tabla es de una fila inmutable).
    pub(crate) fn set_identity(&mut self, identity: &DbIdentity) -> Result<()> {
        self.conn.execute(
            "INSERT INTO database_identity (id, profile, role, taxpayer_id, created_at) \
             VALUES (1, ?1, ?2, ?3, ?4)",
            params![
                identity.profile.as_str(),
                identity.role.as_str(),
                identity.taxpayer_id,
                now_ms()
            ],
        )?;
        Ok(())
    }

    /// Da de alta a un obligado en el registro de la base principal (D273).
    pub fn register_taxpayer(&mut self, taxpayer_id: i64, label: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO taxpayers (taxpayer_id, event, label, location, at) \
             VALUES (?1, 'alta', ?2, ?3, ?4)",
            params![
                taxpayer_id,
                label,
                ProfileDirs::ledger_location(taxpayer_id),
                now_ms()
            ],
        )?;
        Ok(())
    }

    /// Da de baja a un obligado: el registro conserva el alta y añade la baja.
    pub fn retire_taxpayer(&mut self, taxpayer_id: i64) -> Result<()> {
        self.conn.execute(
            "INSERT INTO taxpayers (taxpayer_id, event, label, location, at) \
             SELECT taxpayer_id, 'baja', label, location, ?2 FROM taxpayers \
             WHERE taxpayer_id = ?1 AND event = 'alta'",
            params![taxpayer_id, now_ms()],
        )?;
        let retired: i64 = self.conn.query_row(
            "SELECT count(*) FROM taxpayers WHERE taxpayer_id = ?1 AND event = 'baja'",
            [taxpayer_id],
            |r| r.get(0),
        )?;
        if retired == 0 {
            return Err(CoreError::Profile {
                code: "taxpayer_unknown",
                message: format!("el obligado {taxpayer_id} no está dado de alta"),
            });
        }
        Ok(())
    }

    /// Obligados del registro, en orden de alta, con su estado.
    pub fn taxpayers(&self) -> Result<Vec<TaxpayerEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT a.taxpayer_id, a.label, a.location, \
                    NOT EXISTS (SELECT 1 FROM taxpayers b \
                                WHERE b.taxpayer_id = a.taxpayer_id AND b.event = 'baja') \
             FROM taxpayers a WHERE a.event = 'alta' ORDER BY a.id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(TaxpayerEntry {
                taxpayer_id: r.get(0)?,
                label: r.get(1)?,
                location: r.get(2)?,
                active: r.get(3)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// `true` si el registro ha tenido alguna vez más de un obligado, dados de baja incluidos:
    /// de ahí sale `IndicadorMultiplesOT` (D274).
    pub fn has_had_multiple_taxpayers(&self) -> Result<bool> {
        let n: i64 = self.conn.query_row(
            "SELECT count(DISTINCT taxpayer_id) FROM taxpayers",
            [],
            |r| r.get(0),
        )?;
        Ok(n > 1)
    }
}

// --- Archivos ---

/// Borra un archivo de base de datos y sus auxiliares; lo que no existe no es un error.
pub(crate) fn remove_db_files(path: &Path) -> Result<()> {
    for suffix in ["", "-wal", "-shm", "-journal"] {
        let mut name = path.as_os_str().to_owned();
        name.push(suffix);
        match fs::remove_file(PathBuf::from(name)) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

pub(crate) fn sync_file(path: &Path) -> Result<()> {
    let file = fs::OpenOptions::new().write(true).open(path)?;
    file.sync_all()?;
    Ok(())
}

/// Fuerza a disco el archivo ya cerrado y lo renombra a su nombre definitivo.
pub(crate) fn publish(part: &Path, target: &Path) -> Result<()> {
    ensure_no_wal(part)?;
    sync_file(part)?;
    fs::rename(part, target)?;
    Ok(())
}

/// Comprueba que no queda un `-wal` junto a la base: si lo hay (un proceso que murió sin cerrarla),
/// intenta volcarlo con un checkpoint completo y, si no lo consigue, falla con `wal_pending`.
/// Renombrar una base con un `-wal` sin volcar dejaría sus últimas filas atadas al nombre antiguo.
pub(crate) fn ensure_no_wal(path: &Path) -> Result<()> {
    let mut name = path.as_os_str().to_owned();
    name.push("-wal");
    let wal = PathBuf::from(name);
    if !wal.exists() {
        return Ok(());
    }
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_URI,
    )?;
    let busy: i64 = conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| r.get(0))?;
    conn.close().map_err(|(_, e)| CoreError::Storage(e))?;
    if busy != 0 || wal.exists() {
        return Err(CoreError::Profile {
            code: "wal_pending",
            message: "queda un registro de escritura sin volcar junto a la base".into(),
        });
    }
    Ok(())
}

/// Ruta `.part` de un archivo.
pub(crate) fn part_path(target: &Path) -> PathBuf {
    let mut name = target.as_os_str().to_owned();
    name.push(".part");
    PathBuf::from(name)
}

// --- Creación de bases ---

/// Crea en `part` una base con el esquema actual y vacía, y la abre. Las migraciones se aplican en
/// memoria y la base se vuelca al archivo de una vez: migrar directamente sobre el archivo cuesta
/// un commit con `fsync` por migración, que en un traslado (varias bases nuevas) y en las pruebas
/// de recuperación se nota. Quien llama borró antes cualquier resto de `part`.
fn create_blank(part: &Path) -> Result<Store> {
    let memory = Store::open_in_memory()?;
    let mut file = Connection::open(part)?;
    {
        let backup = rusqlite::backup::Backup::new(&memory.conn, &mut file)?;
        backup.run_to_completion(256, std::time::Duration::ZERO, None)?;
    }
    file.close().map_err(|(_, e)| CoreError::Storage(e))?;
    drop(memory);
    Store::open(part)
}

/// Abre en `part` (borrando antes cualquier resto) una base principal nueva con su identidad.
/// Con `first_taxpayer`, la deja con ese obligado dado de alta. Devuelve el `Store` abierto;
/// quien llama lo cierra con `finish` y lo publica con [`publish`].
pub(crate) fn build_principal(
    part: &Path,
    profile: ProfileKind,
    first_taxpayer: Option<(i64, &str)>,
) -> Result<Store> {
    remove_db_files(part)?;
    if let Some(parent) = part.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut store = create_blank(part)?;
    store.set_identity(&DbIdentity::principal(profile))?;
    if let Some((id, label)) = first_taxpayer {
        store.register_taxpayer(id, label)?;
    }
    Ok(store)
}

/// Autorización escrita en la fila de entorno con la que nace un obligado del perfil normal.
const NEW_LEDGER_ENVIRONMENT_AUTH: &str = "alta del obligado (D275)";

/// Abre en `part` una base de obligado nueva: su identidad, un número de instalación propio
/// (D274) y, en el perfil normal, su fila de entorno `produccion` (D275). En pruebas no hay fila:
/// el entorno por defecto es `simulador`.
pub(crate) fn build_ledger(part: &Path, profile: ProfileKind, taxpayer_id: i64) -> Result<Store> {
    remove_db_files(part)?;
    if let Some(parent) = part.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut store = create_blank(part)?;
    store.set_identity(&DbIdentity::obligado(profile, taxpayer_id))?;
    if profile == ProfileKind::Normal {
        store.conn.execute(
            "INSERT INTO verifactu_environment_changes (environment, authorization, changed_at) \
             VALUES ('produccion', ?1, ?2)",
            params![NEW_LEDGER_ENVIRONMENT_AUTH, now_ms()],
        )?;
    }
    store.ensure_installation_number(&part.to_string_lossy())?;
    Ok(store)
}

/// Crea una base principal nueva en `path`, que no debe existir, y la devuelve abierta. Se
/// escribe en `<path>.part` y se renombra al terminar: un corte no deja una base a medias.
pub fn create_principal(path: &Path, profile: ProfileKind) -> Result<Store> {
    ensure_absent(path)?;
    let part = part_path(path);
    build_principal(&part, profile, None)?.finish()?;
    publish(&part, path)?;
    Store::open_as(path, &DbIdentity::principal(profile))
}

/// Crea la base de un obligado nuevo en `path`, que no debe existir, y la devuelve abierta.
pub fn create_ledger(path: &Path, profile: ProfileKind, taxpayer_id: i64) -> Result<Store> {
    ensure_absent(path)?;
    let part = part_path(path);
    build_ledger(&part, profile, taxpayer_id)?.finish()?;
    publish(&part, path)?;
    Store::open_as(path, &DbIdentity::obligado(profile, taxpayer_id))
}

fn ensure_absent(path: &Path) -> Result<()> {
    if path.exists() {
        return Err(CoreError::Profile {
            code: "database_exists",
            message: "ya existe una base de datos en esa ruta".into(),
        });
    }
    Ok(())
}

// --- Importación desde la base anterior ---

/// Adjunta una base en solo lectura (`ATTACH … mode=ro`) con el alias `alias`. Debe hacerse
/// fuera de una transacción.
pub(crate) fn attach_read_only(conn: &Connection, path: &Path, alias: &str) -> Result<()> {
    conn.execute(&format!("ATTACH DATABASE ?1 AS {alias}"), [file_uri(path)])?;
    Ok(())
}

/// `file:` URI con `mode=ro` para una ruta del sistema. Las barras invertidas pasan a `/`, los
/// caracteres que no son seguros en una URI (espacios, tildes, `%`, `?`, `#`…) se escriben como
/// `%XX` en UTF-8, una ruta con letra de unidad (`C:/…`) lleva tres barras tras `file:` y una ruta
/// UNC (`\\servidor\recurso\…`) lleva cuatro (autoridad vacía y ruta `//servidor/recurso/…`, que es
/// como SQLite entiende una ruta de red). El prefijo `\\?\` de las rutas extendidas se quita.
pub(crate) fn file_uri(path: &Path) -> String {
    let mut text = path.to_string_lossy().replace('\\', "/");
    if let Some(rest) = text.strip_prefix("//?/UNC/") {
        text = format!("//{rest}");
    } else if let Some(rest) = text.strip_prefix("//?/") {
        text = rest.to_owned();
    }
    let unc = text.starts_with("//");
    let body = if unc {
        text.as_str()
    } else {
        text.trim_start_matches('/')
    };
    let mut encoded = String::with_capacity(body.len() + 16);
    for ch in body.chars() {
        match ch {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' | '/' | ':' => {
                encoded.push(ch)
            }
            other => {
                let mut buf = [0u8; 4];
                for byte in other.encode_utf8(&mut buf).bytes() {
                    encoded.push_str(&format!("%{byte:02X}"));
                }
            }
        }
    }
    if unc {
        format!("file://{encoded}?mode=ro")
    } else {
        format!("file:///{encoded}?mode=ro")
    }
}

/// Columnas de una tabla de la base principal de la conexión, en orden.
pub(crate) fn columns_of(conn: &Connection, table: &str) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(&format!("PRAGMA main.table_info(\"{table}\")"))?;
    let rows = stmt.query_map([], |r| r.get::<_, String>(1))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// `INSERT INTO main.<t> (cols) SELECT cols FROM old.<t> [WHERE …]`, con la columna `active`
/// sustituida por `active_override` si se indica. Devuelve las filas insertadas.
pub(crate) fn import_table(
    conn: &Connection,
    table: &str,
    filter: Option<&str>,
    override_column: Option<(&str, &str)>,
) -> Result<usize> {
    let columns = columns_of(conn, table)?;
    let target = columns
        .iter()
        .map(|c| format!("\"{c}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let source = columns
        .iter()
        .map(|c| match override_column {
            Some((name, value)) if name == c => value.to_owned(),
            _ => format!("\"{c}\""),
        })
        .collect::<Vec<_>>()
        .join(", ");
    let wher = filter.map(|f| format!(" WHERE {f}")).unwrap_or_default();
    let n = conn.execute(
        &format!(
            "INSERT INTO main.\"{table}\" ({target}) SELECT {source} FROM old.\"{table}\"{wher}"
        ),
        [],
    )?;
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::file_uri;
    use std::path::Path;

    #[test]
    fn drive_paths_get_three_slashes_and_forward_slashes() {
        assert_eq!(
            file_uri(Path::new(r"C:\Users\Ana\datos\nestadmin.db")),
            "file:///C:/Users/Ana/datos/nestadmin.db?mode=ro"
        );
    }

    #[test]
    fn unix_paths_get_three_slashes() {
        assert_eq!(
            file_uri(Path::new("/var/datos/nestadmin.db")),
            "file:///var/datos/nestadmin.db?mode=ro"
        );
    }

    #[test]
    fn unc_paths_get_an_empty_authority() {
        assert_eq!(
            file_uri(Path::new(r"\\servidor\recurso\carpeta\nestadmin.db")),
            "file:////servidor/recurso/carpeta/nestadmin.db?mode=ro"
        );
    }

    #[test]
    fn extended_length_prefixes_are_removed() {
        assert_eq!(
            file_uri(Path::new(r"\\?\C:\datos\a.db")),
            "file:///C:/datos/a.db?mode=ro"
        );
        assert_eq!(
            file_uri(Path::new(r"\\?\UNC\servidor\recurso\a.db")),
            "file:////servidor/recurso/a.db?mode=ro"
        );
    }

    #[test]
    fn spaces_accents_percent_and_query_characters_are_escaped_in_utf8() {
        assert_eq!(
            file_uri(Path::new(r"C:\Mis datos\ñandú 100%\a#b?c.db")),
            "file:///C:/Mis%20datos/%C3%B1and%C3%BA%20100%25/a%23b%3Fc.db?mode=ro"
        );
        assert_eq!(
            file_uri(Path::new(r"\\servidor\Recurso con espacio\ñ.db")),
            "file:////servidor/Recurso%20con%20espacio/%C3%B1.db?mode=ro"
        );
    }
}
