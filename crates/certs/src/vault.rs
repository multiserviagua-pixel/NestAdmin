//! Custodia de PFX (D278): un archivo `<referencia>.bin` cifrado con DPAPI de usuario, con
//! entropía por instalación y sin interfaz. Nunca en el Administrador de credenciales (D121).
//!
//! Formato del archivo (versión 2): `NACERT` + byte de versión + blob de DPAPI. El texto en
//! claro del blob es la referencia (32 bytes ASCII, para que no se puedan intercambiar los
//! archivos de dos referencias) + `longitud del PFX (u32 LE)` + PFX + contraseña (UTF-8). Se escribe a un
//! temporal y se renombra, y los búferes en claro se ponen a cero.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use windows_sys::Win32::Security::Cryptography::{
    BCryptGenRandom, BCRYPT_USE_SYSTEM_PREFERRED_RNG,
};
use zeroize::Zeroizing;

use crate::dpapi;
use crate::store::{pick_pfx_leaf, Store, MAX_PFX_BYTES};
use crate::{inspect, CertificateInfo};

const MAGIC: &[u8; 6] = b"NACERT";
const FORMAT_VERSION: u8 = 2;
const REFERENCE_LEN: usize = 32;
const HEADER_LEN: usize = MAGIC.len() + 1;
/// Windows `ERROR_INVALID_PASSWORD`.
const ERROR_INVALID_PASSWORD: u32 = 86;

/// Errores al importar o retirar un PFX. El texto es fijo: nunca lleva texto del sistema, ruta,
/// NIF ni nombre.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PfxError {
    #[error("la contraseña del archivo de certificado no es correcta")]
    WrongPassword,
    #[error("el archivo no es un certificado PFX válido")]
    Invalid,
    #[error("el archivo no contiene ningún certificado con clave privada")]
    NoPrivateKey,
    #[error("no se pudo guardar o borrar el certificado de forma segura")]
    Storage,
}

impl PfxError {
    /// Código estable para la interfaz y los registros.
    pub fn code(&self) -> &'static str {
        match self {
            PfxError::WrongPassword => "pfx_wrong_password",
            PfxError::Invalid => "pfx_invalid",
            PfxError::NoPrivateKey => "pfx_no_private_key",
            PfxError::Storage => "pfx_storage_failed",
        }
    }
}

/// Lo que guarda el archivo, en claro y en memoria: se pone a cero al soltarlo.
pub(crate) struct PfxSecret {
    pub pfx: Zeroizing<Vec<u8>>,
    pub password: Zeroizing<String>,
}

/// Una referencia válida son exactamente 32 caracteres hexadecimales en minúscula: así nunca
/// puede apuntar fuera de `store_dir`.
pub(crate) fn valid_reference(reference: &str) -> bool {
    reference.len() == 32
        && reference
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn path_of(store_dir: &Path, reference: &str) -> PathBuf {
    store_dir.join(format!("{reference}.bin"))
}

fn new_reference() -> Option<String> {
    let mut bytes = [0u8; 16];
    // SAFETY: búfer válido de 16 bytes; sin algoritmo, usa el generador del sistema.
    let status = unsafe {
        BCryptGenRandom(
            std::ptr::null_mut(),
            bytes.as_mut_ptr(),
            bytes.len() as u32,
            BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        )
    };
    (status == 0).then(|| bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// Importa un PFX: lo analiza con Windows sin persistir la clave, exige un certificado con
/// clave privada, lo inspecciona y guarda el PFX con su contraseña en un archivo cifrado con
/// DPAPI. Devuelve la referencia opaca y los datos del certificado. Admite PFX antiguos (RC2,
/// 3DES, SHA-1), que analiza Windows. `entropy` es la de la instalación.
///
/// `store_dir` debe ser la carpeta de datos del usuario de la aplicación (por ejemplo bajo
/// `%LOCALAPPDATA%`): DPAPI de usuario protege el contenido, pero no los permisos de la carpeta.
/// Se rechaza un PFX de más de 1 MiB (`Invalid`) y una `entropy` vacía (`Storage`).
///
/// Quien llama es dueño de `bytes` y de `password` y debe ponerlos a cero cuando acabe
/// (`zeroize`); aquí se ponen a cero las copias internas, salvo las que hace Windows.
pub fn import_pfx(
    bytes: &[u8],
    password: &str,
    store_dir: &Path,
    entropy: &[u8],
) -> Result<(String, CertificateInfo), PfxError> {
    if entropy.is_empty() {
        return Err(PfxError::Storage);
    }
    if bytes.len() > MAX_PFX_BYTES {
        return Err(PfxError::Invalid);
    }
    let pfx_len = u32::try_from(bytes.len()).map_err(|_| PfxError::Invalid)?;
    let store = Store::import_pfx(bytes, password).map_err(|code| {
        if code == ERROR_INVALID_PASSWORD {
            PfxError::WrongPassword
        } else {
            PfxError::Invalid
        }
    })?;
    let leaf = pick_pfx_leaf(&store).ok_or(PfxError::NoPrivateKey)?;
    let info = inspect(leaf.as_der()).map_err(|_| PfxError::Invalid)?;
    drop(leaf);
    drop(store);

    let reference = new_reference().ok_or(PfxError::Storage)?;
    let mut plain: Zeroizing<Vec<u8>> = Zeroizing::new(Vec::with_capacity(
        REFERENCE_LEN + 4 + bytes.len() + password.len(),
    ));
    plain.extend_from_slice(reference.as_bytes());
    plain.extend_from_slice(&pfx_len.to_le_bytes());
    plain.extend_from_slice(bytes);
    plain.extend_from_slice(password.as_bytes());
    let sealed = dpapi::protect(&plain, entropy).ok_or(PfxError::Storage)?;
    drop(plain);

    let mut file = Vec::with_capacity(HEADER_LEN + sealed.len());
    file.extend_from_slice(MAGIC);
    file.push(FORMAT_VERSION);
    file.extend_from_slice(&sealed);
    write_atomic(store_dir, &reference, &file).map_err(|_| PfxError::Storage)?;
    Ok((reference, info))
}

fn write_atomic(store_dir: &Path, reference: &str, data: &[u8]) -> std::io::Result<()> {
    fs::create_dir_all(store_dir)?;
    let target = path_of(store_dir, reference);
    let tmp = store_dir.join(format!("{reference}.tmp"));
    let result = (|| {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
        fs::rename(&tmp, &target)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// Retira un PFX custodiado. Idempotente: si no existe (o la referencia no es válida) no es un
/// error.
pub fn remove_pfx(reference: &str, store_dir: &Path) -> Result<(), PfxError> {
    if !valid_reference(reference) {
        return Ok(());
    }
    match fs::remove_file(path_of(store_dir, reference)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(PfxError::Storage),
    }
}

/// Lee y descifra un PFX custodiado. `None` si no existe, la cabecera no es la esperada, el
/// archivo está manipulado o DPAPI no lo descifra (otro usuario o entropía distinta).
pub(crate) fn load(reference: &str, store_dir: &Path, entropy: &[u8]) -> Option<PfxSecret> {
    if !valid_reference(reference) || entropy.is_empty() {
        return None;
    }
    let file = fs::read(path_of(store_dir, reference)).ok()?;
    if file.len() <= HEADER_LEN
        || &file[..MAGIC.len()] != MAGIC
        || file[MAGIC.len()] != FORMAT_VERSION
    {
        return None;
    }
    let plain = dpapi::unprotect(&file[HEADER_LEN..], entropy)?;
    // La referencia cifrada debe ser la pedida: un `.bin` copiado sobre otro no vale.
    if plain.get(..REFERENCE_LEN)? != reference.as_bytes() {
        return None;
    }
    let body = plain.get(REFERENCE_LEN..)?;
    let len = u32::from_le_bytes(body.get(..4)?.try_into().ok()?) as usize;
    let pfx = body.get(4..4usize.checked_add(len)?)?;
    let password = std::str::from_utf8(body.get(4 + len..)?).ok()?;
    Some(PfxSecret {
        pfx: Zeroizing::new(pfx.to_vec()),
        password: Zeroizing::new(password.to_owned()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn references_are_validated() {
        assert!(valid_reference(&"a1".repeat(16)));
        assert!(!valid_reference(""));
        assert!(!valid_reference(&"A1".repeat(16)));
        assert!(!valid_reference("../../etc/passwd"));
        assert!(!valid_reference(&"a".repeat(33)));
    }

    #[test]
    fn new_references_are_random_and_valid() {
        let a = new_reference().unwrap();
        let b = new_reference().unwrap();
        assert!(valid_reference(&a));
        assert_ne!(a, b);
    }
}
