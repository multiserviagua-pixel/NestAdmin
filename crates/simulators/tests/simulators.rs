//! Verificación T06 de los simuladores por separado: consulta, borradores, envío, registro,
//! idempotencia, conflictos, errores del contrato, persistencia tras reinicio, bloqueo del
//! archivo de estado y fallos de escritura.

use std::path::Path;

use nestadmin_core::Connector;
use nestadmin_simulators::{
    codes, AdminConnector, AdminSimulator, ConnectorError, FailureMode, Invoice, InvoiceCriteria,
    MailConnector, MailSimulator, Message, NewDraft,
};
use serde_json::json;

fn draft(body: &str) -> NewDraft {
    NewDraft {
        to: vec!["cliente.beta@ejemplo.test".into()],
        subject: Some("Re: Consulta sobre el pedido 2231".into()),
        body: body.into(),
    }
}

fn code<T>(result: Result<T, ConnectorError>) -> String {
    match result {
        Ok(_) => panic!("se esperaba un error"),
        Err(e) => e.code,
    }
}

fn invoice(number: &str, total: Option<&str>) -> Invoice {
    Invoice {
        supplier: Some("Proveedor Delta S.L.".into()),
        number: Some(number.into()),
        date: Some("2026-10-20".into()),
        currency: Some("EUR".into()),
        base: Some("50.00".into()),
        taxes: Some("10.50".into()),
        total: total.map(str::to_owned),
    }
}

#[test]
fn list_messages_pages_with_stable_ids_and_explicit_absences() {
    let mail = MailSimulator::in_memory().with_page_size(2);
    let mut ids = Vec::new();
    let mut cursor = None;
    loop {
        let page = mail.list_messages(cursor.as_deref()).unwrap();
        assert!(page.messages.len() <= 2);
        ids.extend(page.messages.iter().map(|m| m.id.clone()));
        match page.next_cursor {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }
    assert_eq!(
        ids,
        ["M-0001", "M-0002", "M-0003", "M-0004", "M-0005", "M-0006"]
    );
    // Releer la misma página da los mismos IDs.
    let again = mail.list_messages(Some("M-0002")).unwrap();
    assert_eq!(again.messages[0].id, "M-0003");
    assert_eq!(again.messages[0].subject, None, "asunto ausente como None");
    assert_eq!(again.messages[0].date, None);
    assert_eq!(
        code(mail.list_messages(Some("M-9999"))),
        codes::INVALID_INPUT
    );

    // Destinatarios desconocidos: None, no una lista vacía.
    let unknown = MailSimulator::with_messages(vec![Message {
        id: "M-0100".into(),
        subject: None,
        from: None,
        to: None,
        body: None,
        date: None,
        attachments: Vec::new(),
    }]);
    let page = unknown.list_messages(None).unwrap();
    assert_eq!(page.messages[0].to, None);
    assert_eq!(page.messages[0].body, None);
    assert_eq!(page.next_cursor, None);
}

#[test]
fn create_draft_is_idempotent_and_conflicts_on_different_content() {
    let mail = MailSimulator::in_memory();
    let id = mail
        .create_draft("M-0002", &draft("Entrega el día 10."), "k1")
        .unwrap();
    let again = mail
        .create_draft("M-0002", &draft("Entrega el día 10."), "k1")
        .unwrap();
    assert_eq!(id, again);
    let drafts = mail.drafts().unwrap();
    assert_eq!(drafts.len(), 1);
    // Fecha de creación en UTC con formato AAAA-MM-DDTHH:MM:SSZ.
    let date = drafts[0].date.as_deref().expect("fecha de creación");
    assert_eq!(date.len(), 20);
    assert!(date.ends_with('Z') && date.as_bytes()[10] == b'T');
    assert_eq!(
        code(mail.create_draft("M-0002", &draft("Entrega el día 12."), "k1")),
        codes::CONFLICT
    );
    assert_eq!(
        code(mail.create_draft("M-0001", &draft("Entrega el día 10."), "k1")),
        codes::CONFLICT
    );
    let other = mail
        .create_draft("M-0002", &draft("Otra respuesta."), "k2")
        .unwrap();
    assert_ne!(id, other);
}

#[test]
fn create_draft_rejects_missing_message_and_invalid_input() {
    let mail = MailSimulator::in_memory();
    assert_eq!(
        code(mail.create_draft("M-9999", &draft("Hola."), "k")),
        codes::NOT_FOUND
    );
    assert_eq!(
        code(mail.create_draft("M-0001", &draft("   "), "k")),
        codes::INVALID_INPUT
    );
    assert_eq!(
        code(mail.create_draft("M-0001", &draft("Hola."), " ")),
        codes::INVALID_INPUT
    );
    let mut no_recipient = draft("Hola.");
    no_recipient.to.clear();
    assert_eq!(
        code(mail.create_draft("M-0001", &no_recipient, "k")),
        codes::INVALID_INPUT
    );
    let mut bad_recipient = draft("Hola.");
    bad_recipient.to = vec!["sin-arroba".into()];
    assert_eq!(
        code(mail.create_draft("M-0001", &bad_recipient, "k")),
        codes::INVALID_INPUT
    );
    assert!(
        mail.drafts().unwrap().is_empty(),
        "ningún error crea borradores"
    );
}

#[test]
fn send_draft_sends_once_and_returns_original_receipt() {
    let mail = MailSimulator::in_memory();
    let id = mail
        .create_draft("M-0002", &draft("Entrega el día 10."), "d1")
        .unwrap();
    let receipt = mail.send_draft(&id, "s1").unwrap();
    assert_eq!(receipt.draft_id, id);
    assert_eq!(receipt.message_id, "M-0002");
    assert_eq!(mail.send_draft(&id, "s1").unwrap(), receipt);
    assert_eq!(mail.sent().unwrap(), vec![receipt.clone()]);
    // Otra clave para un borrador ya enviado no lo reenvía.
    assert_eq!(code(mail.send_draft(&id, "s2")), codes::CONFLICT);
    // La misma clave para otro borrador es otra intención: conflicto.
    let other = mail
        .create_draft("M-0001", &draft("Recibida."), "d2")
        .unwrap();
    assert_eq!(code(mail.send_draft(&other, "s1")), codes::CONFLICT);
    assert_eq!(code(mail.send_draft("D-9999", "s3")), codes::NOT_FOUND);
    assert_eq!(mail.sent().unwrap().len(), 1);
    assert_eq!(
        mail.drafts().unwrap()[0].sent_receipt.as_deref(),
        Some(receipt.receipt_id.as_str())
    );
}

#[test]
fn failure_mode_blocks_every_operation_without_effects() {
    let mail = MailSimulator::in_memory();
    let id = mail
        .create_draft("M-0002", &draft("Entrega el día 10."), "d1")
        .unwrap();
    for mode in [FailureMode::Unavailable, FailureMode::Unauthorized] {
        mail.set_failure(Some(mode));
        assert_eq!(code(mail.list_messages(None)), mode.code());
        assert_eq!(code(mail.send_draft(&id, "s1")), mode.code());
        assert_eq!(code(mail.lookup("s1")), mode.code());
        assert_eq!(
            code(mail.execute("s1", &json!({ "draft_id": id }))),
            mode.code()
        );
    }
    mail.set_failure(None);
    assert!(
        mail.sent().unwrap().is_empty(),
        "un error nunca cuenta como envío"
    );

    let admin = AdminSimulator::in_memory();
    let payload = serde_json::to_value(invoice("D-1", Some("60.50"))).unwrap();
    for mode in [FailureMode::Unavailable, FailureMode::Unauthorized] {
        admin.set_failure(Some(mode));
        assert_eq!(
            code(admin.find_invoices(&InvoiceCriteria::default())),
            mode.code()
        );
        assert_eq!(
            code(admin.register_invoice(&invoice("D-1", Some("60.50")), "r1")),
            mode.code()
        );
        assert_eq!(code(admin.lookup("r1")), mode.code());
        assert_eq!(code(admin.execute("r1", &payload)), mode.code());
    }
    admin.set_failure(None);
    assert_eq!(admin.record_count().unwrap(), 2, "un error nunca registra");
    assert_eq!(admin.lookup("r1").unwrap(), None);
}

#[test]
fn contracts_are_usable_through_trait_objects() {
    // Quien usa los contratos solo conoce los traits; un conector real los sustituye.
    let mail_sim = MailSimulator::in_memory();
    let mail: &dyn MailConnector = &mail_sim;
    let page = mail.list_messages(None).unwrap();
    let message_id = page.messages[1].id.clone();
    let draft_id = mail
        .create_draft(&message_id, &draft("Entrega el día 10."), "d1")
        .unwrap();
    let receipt = mail.send_draft(&draft_id, "s1").unwrap();
    // El mismo objeto sirve al núcleo como `Connector` para reconciliar.
    let as_connector: &dyn Connector = mail;
    assert_eq!(
        as_connector.lookup("s1").unwrap(),
        Some(serde_json::to_value(&receipt).unwrap())
    );

    let admin_sim = AdminSimulator::in_memory();
    let admin: &dyn AdminConnector = &admin_sim;
    let registered = admin
        .register_invoice(&invoice("D-1", Some("60.50")), "r1")
        .unwrap();
    let found = admin
        .find_invoices(&InvoiceCriteria {
            number: Some("D-1".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].record_id, registered.record_id);
    let as_connector: &dyn Connector = admin;
    assert!(as_connector.lookup("r1").unwrap().is_some());
}

#[test]
fn mail_connector_executes_send_draft_and_looks_up_by_key() {
    let mail = MailSimulator::in_memory();
    let id = mail
        .create_draft("M-0002", &draft("Entrega el día 10."), "d1")
        .unwrap();
    assert_eq!(mail.lookup("task-1/send_draft/v1").unwrap(), None);
    let receipt = mail
        .execute("task-1/send_draft/v1", &json!({ "draft_id": id }))
        .unwrap();
    assert_eq!(
        mail.lookup("task-1/send_draft/v1").unwrap(),
        Some(receipt.clone())
    );
    assert_eq!(
        mail.execute("task-1/send_draft/v1", &json!({ "draft_id": id }))
            .unwrap(),
        receipt
    );
    assert_eq!(code(mail.execute("k", &json!({}))), codes::INVALID_INPUT);
    assert_eq!(mail.sent().unwrap().len(), 1);
}

#[test]
fn find_invoices_compares_decimals_and_explains_matches() {
    let admin = AdminSimulator::in_memory();
    let by_total = admin
        .find_invoices(&InvoiceCriteria {
            supplier: Some("proveedor alfa s.l.".into()),
            total: Some("121.0".into()),
            ..Default::default()
        })
        .unwrap();
    // F-0002 no coincide en nada (otro proveedor, sin total) y no es candidato.
    assert_eq!(by_total.len(), 1);
    assert_eq!(by_total[0].record_id, "F-0001");
    assert_eq!(by_total[0].reasons, ["mismo proveedor", "mismo total"]);
    assert!(by_total[0].differences.is_empty());
    // Una factura sin total no coincide en el total: se indica la ausencia.
    let gamma_total = admin
        .find_invoices(&InvoiceCriteria {
            number: Some("G-0042".into()),
            total: Some("250.00".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(gamma_total.len(), 1);
    assert_eq!(gamma_total[0].record_id, "F-0002");
    assert_eq!(gamma_total[0].reasons, ["mismo número"]);
    assert_eq!(gamma_total[0].differences, ["total ausente en el registro"]);
    assert_eq!(
        admin
            .find_invoices(&InvoiceCriteria::default())
            .unwrap()
            .len(),
        2
    );
    let bad = InvoiceCriteria {
        total: Some("1,21".into()),
        ..Default::default()
    };
    assert_eq!(code(admin.find_invoices(&bad)), codes::INVALID_INPUT);
}

#[test]
fn find_invoices_reports_partial_duplicates_with_differences() {
    let admin = AdminSimulator::in_memory();
    admin
        .register_invoice(&invoice("D-2026-007", Some("60.50")), "r1")
        .unwrap();
    // Mismo proveedor y número con otro total y otra fecha: posible duplicado a revisar.
    let found = admin
        .find_invoices(&InvoiceCriteria {
            supplier: Some("Proveedor Delta S.L.".into()),
            number: Some("D-2026-007".into()),
            date: Some("2026-10-21".into()),
            total: Some("70.00".into()),
        })
        .unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].record_id, "F-0003");
    assert_eq!(found[0].reasons, ["mismo proveedor", "mismo número"]);
    assert_eq!(found[0].differences, ["fecha distinta", "total distinto"]);

    // Los candidatos con más coincidencias van primero.
    admin
        .register_invoice(&invoice("D-2026-008", Some("70.00")), "r2")
        .unwrap();
    let ranked = admin
        .find_invoices(&InvoiceCriteria {
            supplier: Some("Proveedor Delta S.L.".into()),
            number: Some("D-2026-008".into()),
            ..Default::default()
        })
        .unwrap();
    let ids: Vec<&str> = ranked.iter().map(|m| m.record_id.as_str()).collect();
    assert_eq!(ids, ["F-0004", "F-0003"]);
    assert_eq!(ranked[1].differences, ["número distinto"]);
}

#[test]
fn register_invoice_is_idempotent_and_validates_input() {
    let admin = AdminSimulator::in_memory();
    let receipt = admin
        .register_invoice(&invoice("D-1", Some("60.50")), "r1")
        .unwrap();
    assert_eq!(receipt.record_id, "F-0003");
    assert_eq!(
        admin
            .register_invoice(&invoice("D-1", Some("60.50")), "r1")
            .unwrap(),
        receipt
    );
    assert_eq!(
        code(admin.register_invoice(&invoice("D-1", Some("60.51")), "r1")),
        codes::CONFLICT
    );
    // Campos ausentes se conservan como None.
    let partial = admin.register_invoice(&invoice("D-2", None), "r2").unwrap();
    assert_eq!(partial.invoice.total, None);
    assert_eq!(admin.record_count().unwrap(), 4);

    for bad in [
        Invoice::default(),
        Invoice {
            total: Some("12.".into()),
            ..invoice("X", None)
        },
        Invoice {
            base: Some("1e3".into()),
            ..invoice("X", None)
        },
        Invoice {
            currency: Some("eur".into()),
            ..invoice("X", None)
        },
        Invoice {
            date: Some("20/10/2026".into()),
            ..invoice("X", None)
        },
        Invoice {
            date: Some("2026-02-30".into()),
            ..invoice("X", None)
        },
        Invoice {
            date: Some("2026-04-31".into()),
            ..invoice("X", None)
        },
        Invoice {
            date: Some("2026-02-29".into()),
            ..invoice("X", None)
        },
        Invoice {
            date: Some("2100-02-29".into()),
            ..invoice("X", None)
        },
    ] {
        assert_eq!(
            code(admin.register_invoice(&bad, "r3")),
            codes::INVALID_INPUT
        );
    }
    assert_eq!(
        code(admin.register_invoice(&invoice("D-3", None), "")),
        codes::INVALID_INPUT
    );
    assert_eq!(
        admin.record_count().unwrap(),
        4,
        "ningún error registra facturas"
    );
    // Un 29 de febrero de año bisiesto sí es válido (2000 lo es por ser múltiplo de 400).
    for (n, date) in ["2028-02-29", "2000-02-29", "2026-12-31"]
        .iter()
        .enumerate()
    {
        let leap = Invoice {
            date: Some((*date).into()),
            ..invoice(&format!("L-{n}"), None)
        };
        admin.register_invoice(&leap, &format!("leap-{n}")).unwrap();
    }
    assert_eq!(admin.record_count().unwrap(), 7);
}

#[test]
fn admin_connector_registers_payload_and_rejects_unknown_fields() {
    let admin = AdminSimulator::in_memory();
    let payload = serde_json::to_value(invoice("D-1", Some("60.50"))).unwrap();
    assert_eq!(admin.lookup("task-2/register_invoice/v1").unwrap(), None);
    let receipt = admin
        .execute("task-2/register_invoice/v1", &payload)
        .unwrap();
    assert_eq!(
        admin.lookup("task-2/register_invoice/v1").unwrap(),
        Some(receipt.clone())
    );
    assert_eq!(
        admin
            .execute("task-2/register_invoice/v1", &payload)
            .unwrap(),
        receipt
    );
    assert_eq!(
        code(admin.execute("k", &json!({ "importe": "1.00" }))),
        codes::INVALID_INPUT
    );
    assert_eq!(admin.record_count().unwrap(), 3);
}

#[test]
fn state_survives_restart_from_json_file() {
    let dir = tempfile::tempdir().unwrap();
    let mail_path = dir.path().join("mail.json");
    let admin_path = dir.path().join("admin.json");
    let (draft_id, receipt) = {
        let mail = MailSimulator::open(&mail_path).unwrap();
        let id = mail
            .create_draft("M-0002", &draft("Entrega el día 10."), "d1")
            .unwrap();
        let receipt = mail.send_draft(&id, "s1").unwrap();
        let admin = AdminSimulator::open(&admin_path).unwrap();
        admin
            .register_invoice(&invoice("D-1", Some("60.50")), "r1")
            .unwrap();
        (id, receipt)
    };

    let mail = MailSimulator::open(&mail_path).unwrap();
    assert_eq!(mail.sent().unwrap(), vec![receipt.clone()]);
    assert_eq!(mail.send_draft(&draft_id, "s1").unwrap(), receipt);
    assert_eq!(
        mail.sent().unwrap().len(),
        1,
        "tras reiniciar no se reenvía"
    );
    assert_eq!(mail.list_messages(None).unwrap().messages.len(), 6);

    let admin = AdminSimulator::open(&admin_path).unwrap();
    assert_eq!(admin.record_count().unwrap(), 3);
    assert!(admin.lookup("r1").unwrap().is_some());
}

#[test]
fn second_open_on_same_file_is_refused_and_no_effect_is_lost() {
    let dir = tempfile::tempdir().unwrap();
    let mail_path = dir.path().join("mail.json");
    let admin_path = dir.path().join("admin.json");

    let mail = MailSimulator::open(&mail_path).unwrap();
    let id = mail
        .create_draft("M-0002", &draft("Entrega el día 10."), "d1")
        .unwrap();
    // Un segundo simulador sobre el mismo archivo tendría su propia copia y borraría
    // los efectos del primero al guardar: se rechaza.
    assert_eq!(code(MailSimulator::open(&mail_path)), codes::UNAVAILABLE);
    let receipt = mail.send_draft(&id, "s1").unwrap();
    drop(mail);
    let reopened = MailSimulator::open(&mail_path).unwrap();
    assert_eq!(reopened.sent().unwrap(), vec![receipt]);
    assert_eq!(
        reopened
            .lookup("s1")
            .unwrap()
            .map(|r| r["draft_id"].clone()),
        Some(json!(id))
    );

    let admin = AdminSimulator::open(&admin_path).unwrap();
    assert_eq!(code(AdminSimulator::open(&admin_path)), codes::UNAVAILABLE);
    admin
        .register_invoice(&invoice("D-1", Some("60.50")), "r1")
        .unwrap();
    drop(admin);
    let reopened = AdminSimulator::open(&admin_path).unwrap();
    assert_eq!(reopened.record_count().unwrap(), 3);
    assert!(reopened.lookup("r1").unwrap().is_some());
}

/// Sustituye el archivo de estado por un directorio no vacío: el renombrado del guardado
/// falla en Windows y en Linux. Devuelve el contenido original para restaurarlo.
fn break_state_file(path: &Path) -> Vec<u8> {
    let original = std::fs::read(path).unwrap();
    std::fs::remove_file(path).unwrap();
    std::fs::create_dir(path).unwrap();
    std::fs::write(path.join("ocupado"), "x").unwrap();
    original
}

fn restore_state_file(path: &Path, original: &[u8]) {
    std::fs::remove_dir_all(path).unwrap();
    std::fs::write(path, original).unwrap();
}

#[test]
fn failed_save_returns_unavailable_without_effect() {
    let dir = tempfile::tempdir().unwrap();
    let mail_path = dir.path().join("mail.json");
    let admin_path = dir.path().join("admin.json");

    let mail = MailSimulator::open(&mail_path).unwrap();
    let id = mail
        .create_draft("M-0002", &draft("Entrega el día 10."), "d1")
        .unwrap();
    let original = break_state_file(&mail_path);
    assert_eq!(code(mail.send_draft(&id, "s1")), codes::UNAVAILABLE);
    assert_eq!(
        code(mail.execute("s1", &json!({ "draft_id": id }))),
        codes::UNAVAILABLE
    );
    assert!(mail.sent().unwrap().is_empty(), "sin envío en memoria");
    assert_eq!(mail.lookup("s1").unwrap(), None);
    assert_eq!(mail.drafts().unwrap()[0].sent_receipt, None);
    drop(mail);
    restore_state_file(&mail_path, &original);
    let mail = MailSimulator::open(&mail_path).unwrap();
    assert!(mail.sent().unwrap().is_empty(), "sin envío tras reabrir");
    assert_eq!(mail.lookup("s1").unwrap(), None);
    // Con el archivo de nuevo disponible, el envío ocurre una sola vez.
    mail.send_draft(&id, "s1").unwrap();
    assert_eq!(mail.sent().unwrap().len(), 1);

    let admin = AdminSimulator::open(&admin_path).unwrap();
    let original = break_state_file(&admin_path);
    assert_eq!(
        code(admin.register_invoice(&invoice("D-1", Some("60.50")), "r1")),
        codes::UNAVAILABLE
    );
    assert_eq!(admin.record_count().unwrap(), 2);
    assert_eq!(admin.lookup("r1").unwrap(), None);
    drop(admin);
    restore_state_file(&admin_path, &original);
    let admin = AdminSimulator::open(&admin_path).unwrap();
    assert_eq!(
        admin.record_count().unwrap(),
        2,
        "sin registro tras reabrir"
    );
    assert_eq!(admin.lookup("r1").unwrap(), None);
}

#[test]
fn unreadable_or_newer_state_file_is_reported_as_unavailable() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mail.json");
    std::fs::write(&path, "{ no es json").unwrap();
    assert_eq!(code(MailSimulator::open(&path)), codes::UNAVAILABLE);
    std::fs::write(&path, r#"{"version": 99, "state": {"records": []}}"#).unwrap();
    assert_eq!(code(AdminSimulator::open(&path)), codes::UNAVAILABLE);
}

#[test]
fn an_older_sample_state_gets_the_synthetic_senders_without_touching_other_messages() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mail.json");
    // Estado guardado por una versión anterior: M-0005 sin remitente y sin M-0006, más un
    // mensaje cambiado a mano que no debe tocarse.
    let mut old: Vec<Message> = nestadmin_simulators::mail::sample_messages()
        .into_iter()
        .filter(|m| m.id != "M-0006")
        .collect();
    old[4].from = None;
    old[2].from = None;
    old[2].body = Some("Texto propio".into());
    std::fs::write(
        &path,
        json!({ "version": 1, "state": { "messages": old, "drafts": [], "sent": [] } }).to_string(),
    )
    .unwrap();

    let mail = MailSimulator::open(&path).unwrap();
    let messages = mail.list_messages(None).unwrap().messages;
    assert_eq!(messages.len(), 6);
    assert_eq!(
        messages[4].from.as_deref(),
        Some("logistica@transportes-delta.test")
    );
    assert_eq!(messages[2].from, None, "un mensaje cambiado no se toca");
    assert_eq!(messages[5].from, None, "M-0006 es el caso sin remitente");
    drop(mail);
    // Abrirlo otra vez no cambia nada más.
    let again = MailSimulator::open(&path).unwrap();
    assert_eq!(again.list_messages(None).unwrap().messages, messages);
}

#[test]
fn sample_invoice_attachment_is_listed_and_downloadable_without_effects() {
    let sim = MailSimulator::in_memory();
    let page = sim.list_messages(None).unwrap();
    let m1 = page.messages.iter().find(|m| m.id == "M-0001").unwrap();
    assert_eq!(m1.attachments.len(), 1);
    let attachment = &m1.attachments[0];
    assert_eq!(attachment.name.as_deref(), Some("factura-FS-2026-014.txt"));
    let content = sim.fetch_attachment("M-0001", &attachment.id).unwrap();
    assert_eq!(Some(content.len() as u64), attachment.size);
    assert!(String::from_utf8(content)
        .unwrap()
        .contains("Total: 145.20 EUR"));
    assert!(page
        .messages
        .iter()
        .filter(|m| m.id != "M-0001")
        .all(|m| m.attachments.is_empty()));

    assert_eq!(
        sim.fetch_attachment("M-0001", "A-9").unwrap_err().code,
        "not_found"
    );
    sim.set_attachment_failure(Some(FailureMode::Unavailable));
    assert_eq!(
        sim.fetch_attachment("M-0001", &attachment.id)
            .unwrap_err()
            .code,
        "unavailable"
    );
    // La lista no se ve afectada por el fallo de descarga.
    assert!(sim.list_messages(None).is_ok());
}
