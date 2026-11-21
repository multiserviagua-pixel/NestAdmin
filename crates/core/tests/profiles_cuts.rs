//! Cortes inyectados en el traslado (C1 de T18b, D272, D289): tras cada paso y cada renombrado
//! el proceso «muere»; al retomar, el resultado es idéntico al de un traslado sin cortes
//! (recuentos, huellas de la cadena, mismos identificadores de tareas y ningún envío ni tarea
//! duplicados) y la base antigua queda intacta salvo el marcador y el renombrado final. Solo
//! datos sintéticos.
//!
//! Para que la suite sea rápida, la base de partida de cada caso se construye una sola vez, la
//! referencia se calcula una vez y cada punto de corte se recorre en su propia carpeta temporal,
//! en paralelo (por tandas).

mod profiles_common;

use std::path::{Path, PathBuf};

use nestadmin_core::profiles::migrate::{migrate_with_hook, MigrationState};
use nestadmin_core::profiles::{read_pointer, startup_state, Decision, StartupState};
use nestadmin_core::CoreError;
use profiles_common::{
    build_legacy, copy_dir_all, dump_all, files_under, outcome, Billing, LegacySpec, SIMULATOR,
};

const HEREDADO: LegacySpec = LegacySpec {
    billing: Billing::Full,
    heredado: true,
};
const MASTERS: LegacySpec = LegacySpec {
    billing: Billing::MastersOnly,
    heredado: false,
};
const EMPTY: LegacySpec = LegacySpec {
    billing: Billing::None,
    heredado: false,
};

/// Cuántos puntos de corte se recorren a la vez dentro de un caso.
const BATCH: usize = 8;

fn is_interrupted(result: &Result<MigrationState, CoreError>) -> bool {
    matches!(
        result,
        Err(CoreError::Profile {
            code: "interrupted",
            ..
        })
    )
}

/// La base antigua, esté ya renombrada o no.
fn legacy_path(dir: &Path) -> PathBuf {
    let plain = dir.join("nestadmin.db");
    if plain.exists() {
        plain
    } else {
        dir.join("nestadmin.trasladada.db")
    }
}

fn assert_clean(dir: &Path) {
    let bad: Vec<_> = files_under(dir)
        .into_iter()
        .filter(|f| {
            f.ends_with(".part")
                || f.ends_with(".tmp")
                || f.ends_with(".cerrojo")
                || f.ends_with("-wal")
                || f.ends_with("-shm")
        })
        .collect();
    assert!(bad.is_empty(), "restos: {bad:?}");
}

/// Una base de partida ya construida: se copia, no se rehace.
struct Template {
    dir: tempfile::TempDir,
}

impl Template {
    fn new(spec: LegacySpec) -> Self {
        let dir = tempfile::tempdir().unwrap();
        build_legacy(dir.path(), spec);
        Self { dir }
    }

    /// Una copia nueva en su propia carpeta temporal.
    fn copy(&self) -> tempfile::TempDir {
        let target = tempfile::tempdir().unwrap();
        copy_dir_all(self.dir.path(), target.path());
        target
    }

    fn before(&self) -> std::collections::BTreeMap<String, Vec<String>> {
        dump_all(&self.dir.path().join("nestadmin.db"))
    }
}

/// Traslado de referencia sin cortes: su huella y las etiquetas de todos los puntos de corte.
fn reference(template: &Template, decision: Option<Decision>) -> (String, Vec<String>) {
    let dir = template.copy();
    let mut labels = Vec::new();
    let end = migrate_with_hook(dir.path(), decision, &mut |label| {
        labels.push(label.to_owned());
        false
    })
    .unwrap();
    assert_eq!(end, MigrationState::Done);
    assert_clean(dir.path());
    (outcome(dir.path()), labels)
}

/// Corta en el punto `index`, comprueba el estado del corte y retoma sin cortes.
fn cut_at(
    template: &Template,
    decision: Option<Decision>,
    index: usize,
    label: &str,
    expected: &str,
    before: &std::collections::BTreeMap<String, Vec<String>>,
) {
    let dir = template.copy();
    let data = dir.path();

    let mut calls = 0usize;
    let cut = migrate_with_hook(data, decision, &mut |_| {
        let hit = calls == index;
        calls += 1;
        hit
    });
    assert!(is_interrupted(&cut), "corte {index} ({label}): {cut:?}");

    // En el corte, la base antigua está intacta y el puntero solo existe si ya se renombró.
    assert_eq!(
        &dump_all(&legacy_path(data)),
        before,
        "corte {index} ({label})"
    );
    let pointer_written = label.starts_with("step6:pointer-renamed")
        || label.starts_with("step6:done")
        || label.starts_with("step7")
        || label == "new:done";
    assert_eq!(
        read_pointer(data).unwrap().is_some(),
        pointer_written,
        "corte {index} ({label})"
    );
    if !pointer_written {
        assert_ne!(startup_state(data).unwrap(), StartupState::Ready, "{label}");
    }

    // Retomar da el mismo resultado.
    let end = migrate_with_hook(data, decision, &mut |_| false).unwrap();
    assert_eq!(end, MigrationState::Done, "corte {index} ({label})");
    assert_clean(data);
    assert_eq!(outcome(data), expected, "corte {index} ({label})");
    assert_eq!(
        &dump_all(&legacy_path(data)),
        before,
        "corte {index} ({label})"
    );
    assert_eq!(startup_state(data).unwrap(), StartupState::Ready);
}

/// Corta en cada punto, una vez, y retoma sin cortes.
fn cut_everywhere(spec: LegacySpec, decision: Option<Decision>) {
    let template = Template::new(spec);
    let before = template.before();
    let (expected, labels) = reference(&template, decision);
    assert!(labels.len() >= 4, "{labels:?}");
    let indexed: Vec<(usize, &String)> = labels.iter().enumerate().collect();
    for batch in indexed.chunks(BATCH) {
        std::thread::scope(|scope| {
            let handles: Vec<_> = batch
                .iter()
                .map(|(index, label)| {
                    let (template, expected, before) = (&template, &expected, &before);
                    scope.spawn(move || cut_at(template, decision, *index, label, expected, before))
                })
                .collect();
            for handle in handles {
                handle.join().unwrap();
            }
        });
    }
}

#[test]
fn cut_after_every_step_and_rename_with_the_simulator_and_real_data() {
    cut_everywhere(SIMULATOR, Some(Decision::Real));
}

#[test]
fn cut_everywhere_with_inherited_production_records() {
    cut_everywhere(HEREDADO, Some(Decision::Real));
}

#[test]
fn cut_everywhere_with_a_fictitious_company() {
    cut_everywhere(SIMULATOR, Some(Decision::Ficticia));
}

#[test]
fn cut_everywhere_with_only_masters() {
    cut_everywhere(MASTERS, Some(Decision::Real));
}

#[test]
fn cut_everywhere_with_an_empty_installation() {
    cut_everywhere(EMPTY, None);
}

#[test]
fn cut_everywhere_in_a_new_installation() {
    let (expected, labels) = {
        let dir = tempfile::tempdir().unwrap();
        let mut labels = Vec::new();
        let end = migrate_with_hook(dir.path(), None, &mut |l| {
            labels.push(l.to_owned());
            false
        })
        .unwrap();
        assert_eq!(end, MigrationState::Done);
        (outcome(dir.path()), labels)
    };
    assert_eq!(
        labels,
        [
            "new:part-written",
            "new:renamed",
            "new:pointer-tmp",
            "new:done"
        ]
    );
    for index in 0..labels.len() {
        let dir = tempfile::tempdir().unwrap();
        let mut calls = 0usize;
        let cut = migrate_with_hook(dir.path(), None, &mut |_| {
            let hit = calls == index;
            calls += 1;
            hit
        });
        assert!(is_interrupted(&cut), "{index}");
        // Antes del puntero sigue siendo una instalación nueva, aunque haya una base publicada:
        // al retomar se reutiliza, no se duplica.
        let ready = index == 3;
        assert_eq!(
            startup_state(dir.path()).unwrap() == StartupState::Ready,
            ready,
            "{index}"
        );
        assert_eq!(
            migrate_with_hook(dir.path(), None, &mut |_| false).unwrap(),
            MigrationState::Done
        );
        assert_clean(dir.path());
        assert_eq!(outcome(dir.path()), expected, "{index}");
    }
}

#[test]
fn a_storm_of_cuts_still_converges_to_the_same_result() {
    // Cada intento muere un punto más tarde que el anterior: ninguna combinación de reintentos
    // deja un resultado distinto ni duplica nada.
    let template = Template::new(SIMULATOR);
    let before = template.before();
    let (expected, _) = reference(&template, Some(Decision::Real));
    let dir = template.copy();
    let data = dir.path();
    let mut attempt = 0usize;
    loop {
        let mut calls = 0usize;
        let limit = attempt;
        let result = migrate_with_hook(data, Some(Decision::Real), &mut |_| {
            let hit = calls == limit;
            calls += 1;
            hit
        });
        match result {
            Ok(MigrationState::Done) => break,
            Err(CoreError::Profile {
                code: "interrupted",
                ..
            }) => {}
            other => panic!("intento {attempt}: {other:?}"),
        }
        assert_eq!(dump_all(&legacy_path(data)), before, "intento {attempt}");
        attempt += 1;
        assert!(attempt < 40, "no converge");
    }
    assert_clean(data);
    assert_eq!(outcome(data), expected);
    assert_eq!(dump_all(&legacy_path(data)), before);
}

#[test]
fn a_cut_while_waiting_for_the_decision_resumes_to_the_same_result() {
    let template = Template::new(SIMULATOR);
    let (expected, _) = reference(&template, Some(Decision::Real));
    let dir = template.copy();
    let data = dir.path();
    // Se detiene en el paso 3, y un corte en el paso 2 antes de eso no cambia nada.
    let mut calls = 0usize;
    let cut = migrate_with_hook(data, None, &mut |_| {
        let hit = calls == 3;
        calls += 1;
        hit
    });
    assert!(is_interrupted(&cut));
    assert_eq!(
        migrate_with_hook(data, None, &mut |_| false).unwrap(),
        MigrationState::AwaitingDecision
    );
    assert_eq!(
        migrate_with_hook(data, Some(Decision::Real), &mut |_| false).unwrap(),
        MigrationState::Done
    );
    assert_eq!(outcome(data), expected);
}
