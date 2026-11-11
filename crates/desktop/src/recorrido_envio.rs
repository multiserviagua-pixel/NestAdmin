//! Envío de documentos de venta por correo desde la ventana (E4b, D232–D241; D197). Prueba
//! independiente: reutiliza la sesión simulada de Tauri de `recorrido_venta.rs`, con el PDFium
//! real (`vendor/pdfium/bin` hacia arriba desde el crate o `NESTADMIN_PDFIUM_DIR`) y el simulador
//! de correo. Recorre por IPC, con los nombres y argumentos de `ui/src/api.ts`, preparar, editar,
//! aprobar, el historial, el segundo envío abierto y los errores de cliente sin correo y sin
//! cuenta. Datos sintéticos y carpeta temporal; no abre el visor de PDF del sistema.

use std::path::PathBuf;

use serde_json::{json, Value};
use tauri::Manager;

use super::recorrido_venta::{draft, master_data, Session, PRODUCER};
use super::SharedState;

fn pdfium_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("NESTADMIN_PDFIUM_DIR") {
        return PathBuf::from(dir);
    }
    let mut dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    loop {
        let candidate = dir.join("vendor/pdfium/bin");
        if candidate.join("pdfium.dll").exists() {
            return candidate;
        }
        if !dir.pop() {
            panic!(
                "PDFium no disponible: ejecute scripts/setup-ocr.ps1 o defina NESTADMIN_PDFIUM_DIR"
            );
        }
    }
}

/// Correos que ha enviado el simulador (el efecto real, no lo que dice la tarea).
fn sent(s: &Session) -> usize {
    s.app
        .state::<SharedState>()
        .app()
        .unwrap()
        .simulators()
        .0
        .sent()
        .unwrap()
        .len()
}

fn customer_with(s: &Session, name: &str, email: Option<&str>) -> i64 {
    s.id(
        "sales_create_customer",
        json!({ "data": {
            "name": name,
            "tax_id": "A58818501",
            "address": "Avenida Inventada 2, 08000 Barcelona",
            "email": email,
        }}),
    )
}

/// Borrador de factura para `customer` con una línea, emitido.
fn issued_invoice(s: &Session, customer: i64, product: i64) -> i64 {
    let id = draft(s, "invoice", customer, product);
    s.ok("sales_issue", json!({ "id": id }));
    id
}

#[test]
fn el_envio_de_un_documento_por_correo_desde_la_ventana_de_principio_a_fin() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("datos");
    let s = Session::open_with_pdfium(&data, &pdfium_dir());
    let (_, product) = master_data(&s);
    s.write_producer(PRODUCER);
    s.ok("reload_verifactu_producer", json!({}));
    let customer = customer_with(&s, "Cliente Con Correo SA", Some("compras@cliente.test"));
    let invoice = issued_invoice(&s, customer, product);

    // Las cuentas elegibles: solo el simulador; un documento sin envío no tiene tarea ni historial.
    let accounts = s.ok("sales_send_accounts", json!({}));
    assert_eq!(accounts.as_array().unwrap().len(), 1, "{accounts}");
    assert_eq!(accounts[0]["id"], "simulador");
    assert_eq!(
        s.ok("sales_send_task", json!({ "id": invoice })),
        Value::Null
    );
    assert_eq!(
        s.ok("sales_document_sends", json!({ "id": invoice })),
        json!([])
    );

    // Preparar: nace en revisión, con el destinatario del cliente, la cuenta y los metadatos del
    // PDF; no se envía nada.
    let task = s.ok("sales_prepare_send", json!({ "id": invoice }));
    let id = task["id"].as_i64().unwrap();
    assert_eq!(task["kind"], "send_sales_document");
    assert_eq!(task["state"], "awaiting_review", "{task}");
    let r = &task["result"];
    assert_eq!(r["version"], 1);
    assert_eq!(r["document"]["id"], invoice);
    assert_eq!(r["document"]["number"], "F-2026-0001");
    assert_eq!(r["draft"]["to"], json!(["compras@cliente.test"]));
    assert_eq!(r["account"]["id"], "simulador");
    assert_eq!(r["warnings"]["recipient_differs"], false);
    let attachment = &r["draft"]["attachments"][0];
    assert_eq!(attachment["mime"], "application/pdf");
    assert!(attachment["name"].as_str().unwrap().ends_with(".pdf"));
    assert!(attachment["size"].as_i64().unwrap() > 0);
    assert!(r["draft"]["subject"]
        .as_str()
        .unwrap()
        .starts_with("Factura F-2026-0001"));
    assert_eq!(sent(&s), 0, "preparar no envía");

    // La tarea abierta del documento es esa; otro envío del mismo documento se rechaza.
    assert_eq!(s.ok("sales_send_task", json!({ "id": invoice }))["id"], id);
    s.fails(
        "sales_prepare_send",
        json!({ "id": invoice }),
        "send_in_progress",
    );

    // Editar: datos no válidos con su código y sin tocar la versión; uno válido da la versión 2
    // con un destinatario distinto del cliente y el aviso correspondiente.
    let edit = |to: &str, subject: &str, body: &str, account: &str| {
        s.call(
            "sales_edit_send",
            json!({ "id": id, "to": to, "subject": subject, "body": body, "account": account }),
        )
    };
    let body = "Hola:\n\nLe adjuntamos la factura F-2026-0001.\n\nUn saludo.";
    for (to, subject, text, account, code) in [
        (
            "varios@a.test, otro@b.test",
            "Factura",
            body,
            "simulador",
            "recipient_invalid",
        ),
        ("", "Factura", body, "simulador", "recipient_missing"),
        (
            "compras@cliente.test",
            "Linea\r\nBcc: x@y.test",
            body,
            "simulador",
            "subject_invalid",
        ),
        (
            "compras@cliente.test",
            "Factura",
            "  ",
            "simulador",
            "body_empty",
        ),
        (
            "compras@cliente.test",
            "Factura",
            body,
            "",
            "account_required",
        ),
        (
            "compras@cliente.test",
            "Factura",
            body,
            "no-existe",
            "account_unavailable",
        ),
    ] {
        let error = edit(to, subject, text, account).expect_err(code);
        assert!(error.starts_with(&format!("[{code}] ")), "{code}: {error}");
    }
    let edited = edit(
        "otro@cliente.test",
        "Factura F-2026-0001 editada",
        body,
        "simulador",
    )
    .unwrap();
    assert_eq!(edited["result"]["version"], 2);
    assert_eq!(
        edited["result"]["draft"]["to"],
        json!(["otro@cliente.test"])
    );
    assert_eq!(edited["result"]["warnings"]["recipient_differs"], true);
    assert_eq!(edited["state"], "awaiting_review");

    // Aprobar la versión vista antes de editar no vale; sigue sin enviarse nada.
    s.fails(
        "sales_approve_send",
        json!({ "id": id, "version": 1 }),
        "stale_version",
    );
    assert_eq!(sent(&s), 0);

    // Aprobar la versión 2: se envía una vez, la tarea se completa, el documento pasa a `sent` y
    // el historial tiene una fila con el destinatario editado y la autorización.
    let done = s.ok("sales_approve_send", json!({ "id": id, "version": 2 }));
    assert_eq!(done["state"], "completed", "{done}");
    assert_eq!(sent(&s), 1);
    assert_eq!(
        s.ok("sales_document", json!({ "id": invoice }))["status"],
        "sent"
    );
    let history = s.ok("sales_document_sends", json!({ "id": invoice }));
    let rows = history.as_array().unwrap();
    assert_eq!(rows.len(), 1, "{history}");
    assert_eq!(rows[0]["task_id"], id);
    assert_eq!(rows[0]["recipient"], "otro@cliente.test");
    assert_eq!(rows[0]["account"], "simulador");
    assert_eq!(rows[0]["subject"], "Factura F-2026-0001 editada");
    assert!(rows[0]["sent_at"].as_i64().unwrap() > 0);
    assert!(!rows[0]["authorization"].as_str().unwrap().is_empty());
    assert_eq!(
        s.ok("sales_send_task", json!({ "id": invoice })),
        Value::Null
    );

    // Aprobar otra vez no repite el envío ni el historial.
    assert!(s
        .call("sales_approve_send", json!({ "id": id, "version": 2 }))
        .is_err());
    assert_eq!(sent(&s), 1);
    assert_eq!(
        s.ok("sales_document_sends", json!({ "id": invoice }))
            .as_array()
            .unwrap()
            .len(),
        1
    );

    // Un documento enviado se puede reenviar: nueva tarea que se descarta sin enviar nada y sin
    // cambiar el historial.
    let again = s.ok("sales_prepare_send", json!({ "id": invoice }));
    assert_ne!(again["id"], id);
    let rejected = s.ok("sales_reject_send", json!({ "id": again["id"] }));
    assert_eq!(rejected["state"], "cancelled");
    assert_eq!(
        s.ok("sales_send_task", json!({ "id": invoice })),
        Value::Null
    );
    assert_eq!(sent(&s), 1);

    // Un borrador no se envía.
    let pending = draft(&s, "invoice", customer, product);
    s.fails(
        "sales_prepare_send",
        json!({ "id": pending }),
        "status_not_sendable",
    );

    // Cliente sin correo: la tarea nace con el destinatario vacío y aprobarla da
    // `recipient_missing` sin enviar nada.
    let nameless = customer_with(&s, "Cliente Sin Correo SL", None);
    let no_mail = issued_invoice(&s, nameless, product);
    let task = s.ok("sales_prepare_send", json!({ "id": no_mail }));
    assert_eq!(task["result"]["draft"]["to"], json!([]));
    assert_eq!(task["result"]["checks"]["recipient"], "missing");
    s.fails(
        "sales_approve_send",
        json!({ "id": task["id"], "version": 1 }),
        "recipient_missing",
    );
    assert_eq!(sent(&s), 1);
    s.ok("sales_reject_send", json!({ "id": task["id"] }));

    // Sin ninguna cuenta conectada no se prepara el envío ni se crea tarea.
    s.ok("remove_mail_account", json!({ "id": "simulador" }));
    assert_eq!(s.ok("sales_send_accounts", json!({})), json!([]));
    let third = issued_invoice(&s, customer, product);
    s.fails(
        "sales_prepare_send",
        json!({ "id": third }),
        "no_mail_account",
    );
    assert_eq!(s.ok("sales_send_task", json!({ "id": third })), Value::Null);
    s.close();
}
