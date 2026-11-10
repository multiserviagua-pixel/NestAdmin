//! D186: comprobaciones que pasan al fondo antes de exponer acciones al asistente. (a) `approve`
//! rechaza una factura con importe dudoso sin la confirmación de importes; (c) editar una regla
//! activa solo la deja activa si no amplía lo que haría sin revisión. Datos sintéticos.

use std::path::Path;
use std::sync::Arc;

use nestadmin_ai::{
    AiError, GenerateRequest, GenerateResult, Provider, ProviderKind, Task as AiTask, Usage,
};
use nestadmin_app::{App, AppError, AppPaths, EMAIL, INVOICE};
use nestadmin_core::TaskState;
use nestadmin_extract::{ExtractError, Extractor, Ocr};
use serde_json::{json, Value};

/// Devuelve el JSON incluido en el documento (extracción) o una respuesta fija (correo).
struct FakeProvider;

impl Provider for FakeProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Ollama
    }
    fn model(&self) -> &str {
        "simulado"
    }
    fn generate(&self, request: &GenerateRequest) -> Result<GenerateResult, AiError> {
        let output = match request.task {
            AiTask::Extraction => {
                serde_json::from_str(nestadmin_ai::tasks::document_text(&request.content))
                    .unwrap_or_else(|_| json!({}))
            }
            _ => {
                json!({ "category": "solicitud_factura", "draft": "Adjuntamos la factura solicitada." })
            }
        };
        Ok(GenerateResult {
            request_id: request.request_id.clone(),
            provider: ProviderKind::Ollama,
            model: "simulado".into(),
            output,
            latency_ms: 1,
            usage: Usage::default(),
        })
    }
}

struct NoOcr;

impl Ocr for NoOcr {
    fn recognize(&self, _image: &Path, page: usize) -> Result<String, ExtractError> {
        Err(ExtractError::OcrUnavailable(format!("página {page}")))
    }
}

fn open(dir: &tempfile::TempDir) -> App {
    let extractor = Extractor::new(dir.path().join("sin-pdfium"), Box::new(NoOcr));
    App::open(
        &AppPaths::new(dir.path()),
        Arc::new(FakeProvider),
        Arc::new(extractor),
    )
    .unwrap()
}

/// Factura legible con el euro visible: una regla activa que coincida la registra.
fn clean_invoice(supplier: &str, number: &str, total: &str) -> String {
    let day: u32 = number
        .trim_start_matches(|c: char| !c.is_ascii_digit())
        .parse()
        .unwrap();
    json!({
        "supplier": supplier, "invoice_number": number, "date": format!("2026-10-{day:02}"),
        "subtotal": "100.00", "tax": "21.00", "total": total, "moneda": "EUR",
    })
    .to_string()
}

/// Factura con «1.000» en la base: importe dudoso (D120), siempre a revisión.
fn doubtful_invoice(number: &str) -> String {
    let day: u32 = number
        .trim_start_matches(|c: char| !c.is_ascii_digit())
        .parse()
        .unwrap();
    json!({
        "supplier": "Alfa SL", "invoice_number": number, "date": format!("2026-10-{day:02}"),
        "subtotal": "1.000", "tax": "210,00", "total": "1.210,00", "moneda": "EUR",
    })
    .to_string()
}

fn count(dir: &tempfile::TempDir, query: &str) -> i64 {
    rusqlite::Connection::open(dir.path().join("nestadmin.db"))
        .unwrap()
        .query_row(query, [], |r| r.get(0))
        .unwrap()
}

// ---- (a) Importes dudosos ----

#[test]
fn a_doubtful_invoice_is_not_approved_without_the_amount_confirmation() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = open(&dir);
    let before = app.simulators().1.record_count().unwrap();
    app.import_document("dudosa.txt", &doubtful_invoice("F-2"))
        .unwrap();
    let task = app.process_next().unwrap().unwrap();
    assert_eq!(task.state, TaskState::AwaitingReview);
    assert_eq!(task.kind, INVOICE);
    assert!(task.result.as_ref().unwrap()["amount_review"].is_string());

    let actions = count(&dir, "SELECT COUNT(*) FROM actions");
    let audits = count(&dir, "SELECT COUNT(*) FROM audit_log");
    let signals = count(&dir, "SELECT COUNT(*) FROM rule_signals");
    for result in [
        app.approve(task.id, 1),
        app.approve_reviewed(task.id, 1, false),
    ] {
        let error = result.unwrap_err();
        assert!(matches!(error, AppError::AmountAckRequired), "{error}");
        assert!(error.to_string().contains("importes"), "{error}");
    }
    // Sin efectos: ni registro, ni transición, ni acción, ni evento, ni señal.
    assert_eq!(app.simulators().1.record_count().unwrap(), before);
    let again = app.tasks().unwrap().into_iter().find(|t| t.id == task.id);
    assert_eq!(again.unwrap().state, TaskState::AwaitingReview);
    assert_eq!(count(&dir, "SELECT COUNT(*) FROM actions"), actions);
    assert_eq!(count(&dir, "SELECT COUNT(*) FROM audit_log"), audits);
    assert_eq!(count(&dir, "SELECT COUNT(*) FROM rule_signals"), signals);

    // Una versión caducada sigue fallando por la versión, antes que por los importes.
    assert!(matches!(
        app.approve_reviewed(task.id, 7, false),
        Err(AppError::StaleVersion { .. })
    ));

    // Con la confirmación se registra una sola vez; repetirlo ya no tiene nada que aprobar.
    let done = app.approve_reviewed(task.id, 1, true).unwrap();
    assert_eq!(done.state, TaskState::Completed);
    assert_eq!(app.simulators().1.record_count().unwrap(), before + 1);
    assert!(app.approve_reviewed(task.id, 1, true).is_err());
    assert_eq!(app.simulators().1.record_count().unwrap(), before + 1);
}

#[test]
fn an_invoice_without_doubtful_amounts_and_a_mail_are_approved_as_before() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = open(&dir);
    let before = app.simulators().1.record_count().unwrap();
    app.import_document("limpia.txt", &clean_invoice("Alfa SL", "F-1", "121.00"))
        .unwrap();
    let invoice = app.process_next().unwrap().unwrap();
    assert_eq!(invoice.state, TaskState::AwaitingReview);
    assert!(invoice.result.as_ref().unwrap()["amount_review"].is_null());
    assert_eq!(
        app.approve(invoice.id, 1).unwrap().state,
        TaskState::Completed
    );
    assert_eq!(app.simulators().1.record_count().unwrap(), before + 1);

    // Un correo no tiene importes que confirmar.
    app.fetch_mail().unwrap();
    let mail = loop {
        let task = app.process_next().unwrap().unwrap();
        if task.kind == EMAIL && task.state == TaskState::AwaitingReview {
            break task;
        }
    };
    let version = mail.result.as_ref().unwrap()["version"]
        .as_i64()
        .unwrap_or(1);
    assert_eq!(
        app.approve(mail.id, version).unwrap().state,
        TaskState::Completed
    );
}

// ---- (c) Editar una regla activa ----

fn rule_view(app: &App, id: &str) -> nestadmin_app::panel::RuleView {
    app.rule_views()
        .unwrap()
        .into_iter()
        .find(|v| v.rule.id == id)
        .unwrap()
}

fn audit_rule_rows(app: &App) -> Vec<(String, String)> {
    app.audit_log()
        .unwrap()
        .into_iter()
        .filter(|e| e.action == "rule")
        .map(|e| (e.status, e.detail.unwrap_or_default()))
        .collect()
}

fn active_invoice_rule(app: &mut App, id: &str, conditions: &Value) {
    app.create_rule(id, "register_invoice", conditions).unwrap();
    app.set_rule_active(id, true).unwrap();
}

fn active_send_rule(app: &mut App, id: &str, conditions: &Value, any_sender: bool) {
    app.create_rule_full(nestadmin_app::panel::NewRule {
        id: Some(id),
        name: None,
        action_kind: "send_draft",
        conditions,
        allow_any_sender: any_sender,
    })
    .unwrap();
    app.set_rule_active(id, true).unwrap();
}

/// Edita por el camino atómico del panel; devuelve la vista con `deactivated_by_edit`.
fn edit(app: &mut App, id: &str, conditions: &Value, any: bool) -> nestadmin_app::panel::RuleView {
    app.update_rule_with_ack(id, None, conditions, any).unwrap()
}

#[test]
fn narrowing_or_keeping_an_active_rule_leaves_it_active() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = open(&dir);
    active_invoice_rule(
        &mut app,
        "alfa",
        &json!({ "supplier": "Alfa SL", "max_total": "500.00" }),
    );
    // Igual y más estrecha: sigue activa.
    for max in ["500.00", "300.00", "100.00"] {
        let view = edit(
            &mut app,
            "alfa",
            &json!({ "supplier": "Alfa SL", "max_total": max }),
            false,
        );
        assert!(view.rule.active && !view.deactivated_by_edit, "{max}");
    }
    // Solo cambia el nombre: no desactiva.
    let named = app
        .update_rule_with_ack(
            "alfa",
            Some("Alfa hasta 100"),
            &json!({ "supplier": "Alfa SL", "max_total": "100.00" }),
            false,
        )
        .unwrap();
    assert!(named.rule.active && !named.deactivated_by_edit);
    assert!(rule_view(&app, "alfa").rule.active);

    // Envío: añadir remitente a una regla por dominio la estrecha; de la dirección al dominio, no.
    active_send_rule(
        &mut app,
        "envio",
        &json!({ "category": "solicitud_factura", "sender_domain": "alfa.test" }),
        false,
    );
    let narrower = edit(
        &mut app,
        "envio",
        &json!({ "category": "solicitud_factura", "sender": "ana@alfa.test" }),
        false,
    );
    assert!(narrower.rule.active && !narrower.deactivated_by_edit);
    // Con la validación de cualquier remitente ya dada, mantenerla es igual, no más amplio.
    active_send_rule(
        &mut app,
        "todos",
        &json!({ "category": "consulta_pago" }),
        true,
    );
    let same = edit(
        &mut app,
        "todos",
        &json!({ "category": "consulta_pago" }),
        true,
    );
    assert!(same.rule.active && !same.deactivated_by_edit);
    assert!(same.any_sender_ack_at.is_some());
    // Pasar de cualquier remitente a un dominio estrecha.
    let narrow = edit(
        &mut app,
        "todos",
        &json!({ "category": "consulta_pago", "sender_domain": "alfa.test" }),
        false,
    );
    assert!(narrow.rule.active && !narrow.deactivated_by_edit);
}

#[test]
fn widening_or_doubtful_edits_leave_the_active_rule_inactive_in_the_same_operation() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = open(&dir);
    let base = json!({ "supplier": "Alfa SL", "max_total": "200.00" });
    let widenings = [
        // Subir el límite.
        json!({ "supplier": "Alfa SL", "max_total": "500.00" }),
        // Quitar el límite.
        json!({ "supplier": "Alfa SL" }),
        // Nombre solo parecido (`Similar`).
        json!({ "supplier": "ALFA, S.L.", "max_total": "200.00" }),
        // Otro proveedor (`Disjoint`).
        json!({ "supplier": "Beta SL", "max_total": "200.00" }),
    ];
    for (n, conditions) in widenings.iter().enumerate() {
        let id = format!("alfa-{n}");
        active_invoice_rule(&mut app, &id, &base);
        let versions = rule_view(&app, &id).rule.version;
        let view = edit(&mut app, &id, conditions, false);
        assert!(
            !view.rule.active && view.deactivated_by_edit,
            "{conditions}"
        );
        // Una sola versión nueva, ya inactiva, con su evento.
        assert_eq!(view.rule.version, versions + 1);
        assert_eq!(view.rule.conditions, *conditions);
        let rows = audit_rule_rows(&app);
        assert!(
            rows.iter().any(|(status, detail)| status == "inactive"
                && detail.starts_with(&format!("{id} v{}", view.rule.version))
                && detail.contains("desactivada al editarla")),
            "{rows:?}"
        );
        // Una vista posterior ya no repite el aviso: solo vale para la respuesta de la edición.
        assert!(!rule_view(&app, &id).deactivated_by_edit);
    }

    // Condiciones no comparables: una regla de envío cuya edición la deja sin categoría.
    active_send_rule(
        &mut app,
        "envio",
        &json!({ "category": "solicitud_factura", "sender": "ana@alfa.test" }),
        false,
    );
    let other_category = edit(
        &mut app,
        "envio",
        &json!({ "category": "consulta_pago", "sender": "ana@alfa.test" }),
        false,
    );
    assert!(other_category.deactivated_by_edit && !other_category.rule.active);

    // De la dirección a todo el dominio amplía.
    active_send_rule(
        &mut app,
        "dominio",
        &json!({ "category": "solicitud_factura", "sender": "ana@alfa.test" }),
        false,
    );
    let domain = edit(
        &mut app,
        "dominio",
        &json!({ "category": "solicitud_factura", "sender_domain": "alfa.test" }),
        false,
    );
    assert!(domain.deactivated_by_edit && !domain.rule.active);

    // Dar la validación de cualquier remitente a una regla activa que no la tenía.
    active_send_rule(
        &mut app,
        "cualquiera",
        &json!({ "category": "solicitud_factura", "sender_domain": "alfa.test" }),
        false,
    );
    let any = edit(
        &mut app,
        "cualquiera",
        &json!({ "category": "solicitud_factura" }),
        true,
    );
    assert!(any.deactivated_by_edit && !any.rule.active);
    assert!(
        any.any_sender_ack_at.is_some(),
        "la validación sí se guarda"
    );

    // `update_rule` usa la misma lógica.
    active_invoice_rule(&mut app, "simple", &base);
    let widened = app
        .update_rule("simple", &json!({ "supplier": "Alfa SL" }))
        .unwrap();
    assert!(widened.deactivated_by_edit && !widened.rule.active);
    active_invoice_rule(&mut app, "simple-2", &base);
    let narrowed = app
        .update_rule(
            "simple-2",
            &json!({ "supplier": "Alfa SL", "max_total": "100.00" }),
        )
        .unwrap();
    assert!(!narrowed.deactivated_by_edit && narrowed.rule.active);
}

#[test]
fn an_inactive_rule_stays_as_it_is_and_activating_again_is_a_separate_act() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = open(&dir);
    app.create_rule(
        "alfa",
        "register_invoice",
        &json!({ "supplier": "Alfa SL", "max_total": "200.00" }),
    )
    .unwrap();
    let view = edit(&mut app, "alfa", &json!({ "supplier": "Alfa SL" }), false);
    assert!(!view.rule.active && !view.deactivated_by_edit);
    // Sin evento de desactivación: la regla ya estaba inactiva.
    assert!(!audit_rule_rows(&app)
        .iter()
        .any(|(_, d)| d.contains("desactivada al editarla")));

    // Una regla desactivada por la edición se vuelve a activar con el acto de siempre.
    active_invoice_rule(
        &mut app,
        "beta",
        &json!({ "supplier": "Beta SL", "max_total": "200.00" }),
    );
    let widened = edit(&mut app, "beta", &json!({ "supplier": "Beta SL" }), false);
    assert!(widened.deactivated_by_edit);
    assert!(app.set_rule_active("beta", true).unwrap().active);
}

#[test]
fn after_the_deactivation_a_task_that_the_rule_would_have_run_goes_to_review() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = open(&dir);
    let before = app.simulators().1.record_count().unwrap();
    active_invoice_rule(
        &mut app,
        "alfa",
        &json!({ "supplier": "Alfa SL", "max_total": "200.00" }),
    );
    // Con la regla activa, una factura de 121 € se registra sola.
    app.import_document("1.txt", &clean_invoice("Alfa SL", "F-1", "121.00"))
        .unwrap();
    assert_eq!(
        app.process_next().unwrap().unwrap().state,
        TaskState::Completed
    );
    assert_eq!(app.simulators().1.record_count().unwrap(), before + 1);

    // Se amplía el límite: queda inactiva y la siguiente factura, que habría pasado, va a revisión.
    let view = edit(
        &mut app,
        "alfa",
        &json!({ "supplier": "Alfa SL", "max_total": "900.00" }),
        false,
    );
    assert!(view.deactivated_by_edit);
    app.import_document("2.txt", &clean_invoice("Alfa SL", "F-2", "121.00"))
        .unwrap();
    let task = app.process_next().unwrap().unwrap();
    assert_eq!(task.state, TaskState::AwaitingReview);
    assert_eq!(app.simulators().1.record_count().unwrap(), before + 1);
}
