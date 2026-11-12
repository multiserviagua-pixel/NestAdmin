//! B2: lista del almacén de Windows (solo Windows). No se instala nada y no se imprime ningún
//! dato de los certificados reales del equipo donde se ejecuta: solo se comprueban propiedades.
#![cfg(windows)]

use nestadmin_certs::{list_store_certificates, StoreLocation};

#[test]
fn listing_is_stable_deduplicated_and_never_fails() {
    let first = list_store_certificates();
    let second = list_store_certificates();
    let fingerprints = |list: &[nestadmin_certs::StoreCertificate]| {
        list.iter()
            .map(|c| c.info.fingerprint.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(fingerprints(&first), fingerprints(&second));

    let mut sorted = fingerprints(&first);
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), first.len(), "sin duplicados por huella");

    for c in &first {
        assert_eq!(c.info.fingerprint.len(), 64);
        assert!(matches!(
            c.location,
            StoreLocation::CurrentUser | StoreLocation::LocalMachine
        ));
        // Un proveedor de tarjeta nunca es exportable.
        if c.provider == nestadmin_certs::KeyProvider::SmartCard {
            assert_eq!(c.exportable, Some(false));
        }
    }
    // El `Debug` no lleva nombres ni NIF.
    for c in &first {
        let text = format!("{c:?}");
        if let Some(name) = &c.info.subject_name {
            assert!(!text.contains(name.as_str()));
        }
    }
}
