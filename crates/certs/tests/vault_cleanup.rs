//! B3: limpieza del almacén de PFX (solo Windows, como el almacén).
#![cfg(windows)]

mod pfx_support;

use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use nestadmin_certs::{
    acquire, cleanup_vault, import_pfx, AcquireMode, CertificateSource, VaultCleanup,
};
use pfx_support::*;

const ENTROPY: &[u8] = b"entropia-de-instalacion-sintetica";

fn reference(n: u8) -> String {
    format!("{n:02x}").repeat(16)
}

fn names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

#[test]
fn removes_tmp_and_unreferenced_bin_but_never_what_is_kept() {
    let dir = tempfile::tempdir().unwrap();
    let (a, b, c) = (reference(0xa1), reference(0xb2), reference(0xc3));
    for r in [&a, &b, &c] {
        fs::write(dir.path().join(format!("{r}.bin")), b"cifrado").unwrap();
    }
    fs::write(
        dir.path().join(format!("{}.tmp", reference(0xd4))),
        b"a medias",
    )
    .unwrap();
    fs::write(dir.path().join(format!("{a}.tmp")), b"a medias").unwrap();
    // Nada de esto es del almacén: no se toca.
    fs::write(dir.path().join("notas.txt"), b"x").unwrap();
    fs::write(dir.path().join("A1A1.bin"), b"x").unwrap();
    fs::write(dir.path().join(format!("{}.BIN", reference(0xe5))), b"x").unwrap();
    fs::create_dir(dir.path().join(format!("{}.bin", reference(0xf6)))).unwrap();

    let report = cleanup_vault(dir.path(), &[a.as_str(), c.as_str()]).unwrap();
    assert_eq!(
        report,
        VaultCleanup {
            tmp_removed: 2,
            bin_removed: 1
        }
    );
    let left = names(dir.path());
    assert!(left.contains(&format!("{a}.bin")), "{left:?}");
    assert!(left.contains(&format!("{c}.bin")), "{left:?}");
    assert!(!left.contains(&format!("{b}.bin")));
    assert!(!left.iter().any(|n| n.ends_with(".tmp")));
    for untouched in ["notas.txt", "A1A1.bin"] {
        assert!(left.contains(&untouched.to_string()), "{left:?}");
    }
    assert!(left.contains(&format!("{}.bin", reference(0xf6))));
    assert!(left.contains(&format!("{}.BIN", reference(0xe5))));

    // Idempotente.
    assert_eq!(
        cleanup_vault(dir.path(), &[a.as_str(), c.as_str()]).unwrap(),
        VaultCleanup::default()
    );
}

#[test]
fn a_missing_directory_is_not_an_error_and_an_empty_keep_removes_all_bins() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(
        cleanup_vault(&dir.path().join("no-existe"), &[]).unwrap(),
        VaultCleanup::default()
    );
    let r = reference(0x11);
    fs::write(dir.path().join(format!("{r}.bin")), b"x").unwrap();
    assert_eq!(cleanup_vault(dir.path(), &[]).unwrap().bin_removed, 1);
}

#[test]
fn a_kept_pfx_still_works_after_the_cleanup() {
    let dir = tempfile::tempdir().unwrap();
    let material = modern(PASSWORD);
    let (kept, _) = import_pfx(&material.pfx, PASSWORD, dir.path(), ENTROPY).unwrap();
    let other = modern(PASSWORD);
    let (dropped, _) = import_pfx(&other.pfx, PASSWORD, dir.path(), ENTROPY).unwrap();
    fs::write(dir.path().join(format!("{}.tmp", reference(0x22))), b"x").unwrap();

    let report = cleanup_vault(dir.path(), &[kept.as_str()]).unwrap();
    assert_eq!(
        report,
        VaultCleanup {
            tmp_removed: 1,
            bin_removed: 1
        }
    );
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    let silent = |reference: &str| {
        acquire(
            &CertificateSource::Pfx {
                reference: reference.to_string(),
            },
            dir.path(),
            ENTROPY,
            AcquireMode::Silent,
            now,
        )
    };
    assert!(silent(&kept).is_ok());
    assert!(silent(&dropped).is_err());
}
