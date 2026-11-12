//! DPAPI de usuario (D278): `CryptProtectData` y `CryptUnprotectData` con entropía por
//! instalación y sin interfaz (`CRYPTPROTECT_UI_FORBIDDEN`). Los búferes en claro se ponen a
//! cero.

use std::ptr;

use windows_sys::Win32::Foundation::{LocalFree, HLOCAL};
use windows_sys::Win32::Security::Cryptography::{
    CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
};
use zeroize::{Zeroize, Zeroizing};

/// Descripción fija que DPAPI guarda en claro junto al blob: nunca datos del titular.
const DESCRIPTION: [u16; 10] = [
    b'N' as u16,
    b'e' as u16,
    b's' as u16,
    b't' as u16,
    b'A' as u16,
    b'd' as u16,
    b'm' as u16,
    b'i' as u16,
    b'n' as u16,
    0,
];

fn blob(data: &[u8]) -> CRYPT_INTEGER_BLOB {
    CRYPT_INTEGER_BLOB {
        cbData: data.len() as u32,
        pbData: data.as_ptr() as *mut u8,
    }
}

/// Cifra para el usuario actual de Windows. `None` si DPAPI falla.
pub(crate) fn protect(plain: &[u8], entropy: &[u8]) -> Option<Vec<u8>> {
    let input = blob(plain);
    let entropy_blob = blob(entropy);
    let mut out = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: ptr::null_mut(),
    };
    // SAFETY: los punteros apuntan a búferes vivos durante la llamada; la salida la reserva
    // DPAPI y se libera con `LocalFree` tras copiarla.
    unsafe {
        let ok = CryptProtectData(
            &input,
            DESCRIPTION.as_ptr(),
            if entropy.is_empty() {
                ptr::null()
            } else {
                &entropy_blob
            },
            ptr::null(),
            ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out,
        );
        if ok == 0 || out.pbData.is_null() {
            return None;
        }
        let copy = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
        LocalFree(out.pbData as HLOCAL);
        Some(copy)
    }
}

/// Descifra un blob de [`protect`] con la misma entropía y el mismo usuario. `None` si no
/// coincide, está manipulado o es de otro usuario.
pub(crate) fn unprotect(sealed: &[u8], entropy: &[u8]) -> Option<Zeroizing<Vec<u8>>> {
    let input = blob(sealed);
    let entropy_blob = blob(entropy);
    let mut out = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: ptr::null_mut(),
    };
    // SAFETY: igual que en `protect`; el búfer de salida se pone a cero antes de liberarlo.
    unsafe {
        let ok = CryptUnprotectData(
            &input,
            ptr::null_mut(),
            if entropy.is_empty() {
                ptr::null()
            } else {
                &entropy_blob
            },
            ptr::null(),
            ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out,
        );
        if ok == 0 || out.pbData.is_null() {
            return None;
        }
        let raw = std::slice::from_raw_parts_mut(out.pbData, out.cbData as usize);
        let copy = Zeroizing::new(raw.to_vec());
        raw.zeroize();
        LocalFree(out.pbData as HLOCAL);
        Some(copy)
    }
}
