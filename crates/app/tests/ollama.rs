//! Flujo completo contra el Ollama local real, con datos sintéticos. No se ejecuta por
//! defecto porque necesita Ollama y `qwen3:8b`:
//! `cargo test -p nestadmin-app --test ollama -- --ignored --nocapture`

use std::sync::Arc;
use std::time::Duration;

use nestadmin_ai::ollama::{OllamaProvider, DEFAULT_MODEL, DEFAULT_URL};
use nestadmin_app::{App, AppPaths};
use nestadmin_core::TaskState;

#[test]
#[ignore = "requiere Ollama local con qwen3:8b"]
fn document_and_mail_flow_with_local_ollama() {
    let dir = tempfile::tempdir().unwrap();
    let provider =
        OllamaProvider::new(DEFAULT_URL, DEFAULT_MODEL, Duration::from_secs(300)).unwrap();
    let mut app = App::open(&AppPaths::new(dir.path()), Arc::new(provider)).unwrap();

    app.import_document(
        "sintetica.txt",
        "Factura F-901. Proveedor: Alba Solar SL. Fecha: 2026-01-10. Base: 100.00 EUR. IVA: 21.00 EUR. Total: 121.00 EUR.",
    )
    .unwrap();
    let invoice = app.process_next().unwrap().unwrap();
    let result = invoice.result.clone().unwrap_or_default();
    println!("factura: {:?} {}", invoice.state, result["fields"]);
    assert_eq!(
        invoice.state,
        TaskState::AwaitingReview,
        "{:?}",
        invoice.error
    );
    assert_eq!(result["fields"]["total"], "121.00");
    assert_eq!(result["provider"], "ollama");

    let before = app.simulators().1.record_count().unwrap();
    assert_eq!(
        app.approve(invoice.id, 1).unwrap().state,
        TaskState::Completed
    );
    assert_eq!(app.simulators().1.record_count().unwrap(), before + 1);

    app.fetch_mail().unwrap();
    let email = app.process_next().unwrap().unwrap();
    let result = email.result.clone().unwrap_or_default();
    println!(
        "correo: {:?} {} / {}",
        email.state, result["category"], result["draft"]["body"]
    );
    assert_eq!(email.state, TaskState::AwaitingReview, "{:?}", email.error);
    assert!(app.simulators().0.sent().unwrap().is_empty());
}
