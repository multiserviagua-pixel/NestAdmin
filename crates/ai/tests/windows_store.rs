//! T09 en Windows: el Administrador de credenciales real guarda, devuelve y borra una
//! credencial sintética con un nombre de prueba. Escribe en el almacén del usuario, por lo
//! que está marcada y se ejecuta a propósito:
//!
//! ```text
//! cargo test -p nestadmin-ai --test windows_store -- --ignored
//! ```

#![cfg(windows)]

use nestadmin_ai::credentials::WindowsCredentialStore;
use nestadmin_ai::{CredentialStore, Secret};

/// Borra la credencial aunque la prueba falle a mitad.
struct Cleanup<'a>(&'a WindowsCredentialStore, &'a str);

impl Drop for Cleanup<'_> {
    fn drop(&mut self) {
        let _ = self.0.delete(self.1);
    }
}

#[test]
#[ignore = "escribe en el Administrador de credenciales de Windows"]
fn real_windows_store_round_trip_with_a_test_target() {
    let store = WindowsCredentialStore::new().unwrap();
    let name = format!("t09-prueba-{}", std::process::id());
    let _cleanup = Cleanup(&store, &name);
    let secret = Secret::new("gsk_PRUEBA_SINTETICA_T09_almacen_real");

    assert_eq!(store.get(&name).unwrap(), None, "nombre de prueba libre");
    store.set(&name, &secret).unwrap();
    assert_eq!(store.get(&name).unwrap(), Some(secret.clone()));

    // Otra instancia (como tras reiniciar la aplicación) lee la misma credencial.
    let reopened = WindowsCredentialStore::new().unwrap();
    assert_eq!(reopened.get(&name).unwrap(), Some(secret));

    store.delete(&name).unwrap();
    assert_eq!(store.get(&name).unwrap(), None);
    // Borrar lo que no existe no es un error.
    store.delete(&name).unwrap();
}
