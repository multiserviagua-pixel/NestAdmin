//! Utilidades de las pruebas de perfiles y del traslado (C1 de T18b). Todo es sintético: NIF de
//! prueba válidos, empresas y clientes inventados, sin documentos ni credenciales reales.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use nestadmin_core::profiles::classify::is_derived;
use nestadmin_core::sales::{
    Company, CustomerData, DraftInput, DraftLine, NewSalesSend, VerifactuContext,
    VerifactuEnvironment, SEND_SALES_DOCUMENT,
};
use nestadmin_core::{SearchDoc, SearchKind, SearchStatus, Store};
use nestadmin_documents::IsoDate;
use nestadmin_sales::{DocumentKind, LineInput, TaxRate};
use nestadmin_verifactu::{Fecha, FechaHoraHuso, Persona, SistemaInformatico};
use rusqlite::Connection;
use rust_decimal::Decimal;
use serde_json::json;

pub fn date(text: &str) -> IsoDate {
    IsoDate::parse("test", text).unwrap()
}

pub fn ctx() -> VerifactuContext {
    let base = FechaHoraHuso::new(Fecha::new(2026, 3, 1).unwrap(), 10, 0, 0, 60)
        .unwrap()
        .unix();
    VerifactuContext {
        system: SistemaInformatico {
            productor: Persona::con_nif("Productor Demo SL", "B00000034"),
            nombre_sistema: "NestAdmin".into(),
            id_sistema: "NA".into(),
            version: "0.1.0".into(),
            numero_instalacion: "INST-0001".into(),
            solo_verifactu: true,
            multi_ot: false,
            indicador_multiples_ot: false,
        },
        now: FechaHoraHuso::desde_unix(base, 60).unwrap(),
        environment: VerifactuEnvironment::Simulador,
    }
}

pub fn raw(path: &Path) -> Connection {
    let conn = Connection::open(path).unwrap();
    conn.busy_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    conn
}

/// Cuántos datos de facturación lleva la base anterior de la prueba.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Billing {
    /// Empresa, cliente, producto, plantilla, facturas con registros y un envío cerrado.
    Full,
    /// Empresa, cliente, producto y plantilla, sin documentos ni registros.
    MastersOnly,
    /// Solo tareas y reglas: nada de facturación.
    None,
}

#[derive(Clone, Copy, Debug)]
pub struct LegacySpec {
    pub billing: Billing,
    /// Marca los registros como `produccion_heredado` (D258), como los dejó la migración 16.
    pub heredado: bool,
}

pub const SIMULATOR: LegacySpec = LegacySpec {
    billing: Billing::Full,
    heredado: false,
};

#[derive(Debug, Clone)]
pub struct Facts {
    pub invoice: Option<i64>,
    pub send_task: Option<i64>,
    pub mail_tasks: Vec<i64>,
}

fn sha(tag: char) -> String {
    tag.to_string().repeat(64)
}

/// Crea `<dir>/nestadmin.db` como la dejaría la aplicación anterior a los perfiles (con la
/// migración 18 ya aplicada pero sin identidad), con los datos que pide `spec`.
pub fn build_legacy(dir: &Path, spec: LegacySpec) -> Facts {
    fs::create_dir_all(dir).unwrap();
    let path = dir.join("nestadmin.db");
    let mut store = Store::open(&path).unwrap();
    let mut facts = Facts {
        invoice: None,
        send_task: None,
        mail_tasks: vec![],
    };

    // Tareas de correo, una con su entrada de búsqueda, y una regla: papel de la base principal.
    let t1 = store.enqueue("email_triage", r#"{"mail":"M-1"}"#).unwrap();
    let t2 = store
        .enqueue_closed_indexed(
            "email_triage",
            r#"{"mail":"M-2"}"#,
            &json!({"ok": true}),
            "sin acción",
            &SearchDoc {
                task_id: 0,
                kind: SearchKind::Email,
                status: SearchStatus::Indexed,
                title: "Pedido de prueba".into(),
                party: "proveedor@example.test".into(),
                number: String::new(),
                doc_date: None,
                total_minor: None,
                currency: String::new(),
                body: "texto sintético de un correo".into(),
                detail: String::new(),
            },
        )
        .unwrap();
    facts.mail_tasks = vec![t1.id, t2.id];
    store
        .create_rule(
            "regla-prueba",
            "send_draft",
            &json!({"sender": "cliente@example.test"}),
        )
        .unwrap();
    store
        .record_note("business_note", "ok", &json!({"n": 1}))
        .unwrap();

    if spec.billing != Billing::None {
        store
            .set_sales_company(&Company {
                name: Some("Taller Ficticio SL".into()),
                tax_id: Some("B12345674".into()),
                address: Some("Calle Inventada 1, 28000 Madrid".into()),
                email: Some("demo@example.test".into()),
                iban: None,
                logo: Some(b"\x89PNG-sintetico".to_vec()),
                logo_mime: Some("image/png".into()),
            })
            .unwrap();
        let customer = store
            .create_customer(&CustomerData {
                name: "Cliente Uno SA".into(),
                tax_id: Some("12345678Z".into()),
                address: Some("Avenida Ficticia 2, 08000 Barcelona".into()),
                email: Some("cliente@example.test".into()),
            })
            .unwrap();
        if spec.billing == Billing::Full {
            let mut input = DraftInput::new(DocumentKind::Invoice);
            input.customer_id = Some(customer);
            input.lines = vec![DraftLine::from(LineInput {
                description: "Trabajo".into(),
                quantity: "1".parse::<Decimal>().unwrap(),
                unit_price: "10".parse::<Decimal>().unwrap(),
                discount: Decimal::ZERO,
                tax_rate: TaxRate::TwentyOne,
                exemption: None,
            })];
            let invoice = store.create_sales_draft(&input).unwrap();
            store
                .issue_sales_invoice_registered(
                    invoice,
                    date("2026-03-01"),
                    date("2026-03-01"),
                    &ctx(),
                )
                .unwrap();
            // Segunda factura: la cadena tiene dos eslabones.
            let second = store.create_sales_draft(&input).unwrap();
            store
                .issue_sales_invoice_registered(
                    second,
                    date("2026-03-02"),
                    date("2026-03-02"),
                    &ctx(),
                )
                .unwrap();
            // Envío por correo del documento: tarea de envío, disparo y envío cerrado.
            let task = store
                .enqueue_sales_send(
                    invoice,
                    &json!({"type": SEND_SALES_DOCUMENT, "version": 1}),
                    "preparado",
                    true,
                )
                .unwrap()
                .unwrap();
            store
                .complete_sales_send(
                    task.id,
                    &json!({"type": SEND_SALES_DOCUMENT, "version": 1, "sent": true}),
                    &NewSalesSend {
                        document_id: invoice,
                        task_id: task.id,
                        version: 1,
                        idempotency_key: format!("task-{}/{SEND_SALES_DOCUMENT}/v1", task.id),
                        account: "simulador".into(),
                        account_address: Some("administracion@nestadmin.test".into()),
                        recipient: "cliente@example.test".into(),
                        subject: "Factura de prueba".into(),
                        attachment_name: "F-2026-0001.pdf".into(),
                        attachment_size: 1234,
                        attachment_sha256: sha('a'),
                        authorization: "aprobación".into(),
                        sent_message_id: Some("S-0001".into()),
                    },
                )
                .unwrap();
            facts.invoice = Some(invoice);
            facts.send_task = Some(task.id);
        }
    }
    drop(store);

    if spec.billing != Billing::None {
        let conn = raw(&path);
        conn.execute(
            "INSERT INTO products (description, unit_price, tax_rate, created_at, updated_at) \
             VALUES ('Hora de trabajo', '10.00', '21', 1, 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO recurring_templates (customer_id, lines, frequency, first_day, active, \
             created_at, updated_at) VALUES (1, '[]', 'monthly', '2026-04-01', 1, 1, 1)",
            [],
        )
        .unwrap();
        if spec.heredado {
            conn.execute_batch(
                "DROP TRIGGER verifactu_record_environments_no_update; \
                 UPDATE verifactu_record_environments SET environment = 'produccion_heredado'; \
                 CREATE TRIGGER verifactu_record_environments_no_update \
                 BEFORE UPDATE ON verifactu_record_environments \
                 BEGIN SELECT RAISE(ABORT, 'el entorno de un registro Verifactu no se modifica'); END;",
            )
            .unwrap();
        }
        if spec.billing == Billing::Full {
            let ventas = dir.join("documentos").join("ventas");
            fs::create_dir_all(&ventas).unwrap();
            fs::write(ventas.join("F-2026-0001.pdf"), b"%PDF-sintetico").unwrap();
        }
    }
    facts
}

/// Todas las filas de todas las tablas de una base (sin el índice derivado, el marcador ni la
/// identidad), como texto estable, para comparar bases entre sí.
pub fn dump_all(path: &Path) -> BTreeMap<String, Vec<String>> {
    let conn = Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE | rusqlite::OpenFlags::SQLITE_OPEN_URI,
    )
    .unwrap();
    let names: Vec<String> = conn
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    let mut out = BTreeMap::new();
    for name in names {
        if is_derived(&name) || name == "profile_migration" || name == "database_identity" {
            continue;
        }
        let mut stmt = conn
            .prepare(&format!("SELECT * FROM \"{name}\" ORDER BY 1, 2"))
            .unwrap();
        let cols = stmt.column_count();
        let rows: Vec<String> = stmt
            .query_map([], |r| {
                let mut parts = Vec::with_capacity(cols);
                for i in 0..cols {
                    let v: rusqlite::types::Value = r.get(i)?;
                    parts.push(format!("{v:?}"));
                }
                Ok(parts.join("|"))
            })
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        out.insert(name, rows);
    }
    out
}

pub fn count(path: &Path, table: &str) -> i64 {
    raw(path)
        .query_row(&format!("SELECT count(*) FROM \"{table}\""), [], |r| {
            r.get(0)
        })
        .unwrap()
}

pub fn query_strings(path: &Path, sql: &str) -> Vec<String> {
    let conn = raw(path);
    let mut stmt = conn.prepare(sql).unwrap();
    let cols = stmt.column_count();
    let rows = stmt
        .query_map([], |r| {
            let mut parts = Vec::new();
            for i in 0..cols {
                let v: rusqlite::types::Value = r.get(i)?;
                parts.push(format!("{v:?}"));
            }
            Ok(parts.join("|"))
        })
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    rows
}

/// Archivos de la carpeta de datos, recursivos, con ruta relativa y `/`; sin los `.esquema-N`
/// de la copia previa a migrar (dependen del esquema de partida, no del traslado).
pub fn files_under(dir: &Path) -> Vec<String> {
    fn walk(base: &Path, dir: &Path, out: &mut Vec<String>) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(base, &path, out);
            } else {
                let rel = path
                    .strip_prefix(base)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push(rel);
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out.retain(|f| !f.contains(".esquema-"));
    out.sort();
    out
}

/// Huella del resultado de un traslado en una carpeta de datos: archivos presentes, recuentos de
/// cada base y las columnas estables de lo que no debe cambiar al retomar (identificadores de
/// tareas, claves de envío, huellas de la cadena, altas de obligados). Excluye marcas de tiempo y
/// el número de instalación, que se generan de nuevo en cada rehacer.
pub fn outcome(dir: &Path) -> String {
    let mut text = String::new();
    for f in files_under(dir) {
        text.push_str(&format!("file {f}\n"));
    }
    let dbs: Vec<PathBuf> = files_under(dir)
        .into_iter()
        .filter(|f| f.ends_with("principal.db") || f.ends_with("obligado.db"))
        .map(|f| dir.join(f))
        .collect();
    for db in dbs {
        let rel = db
            .strip_prefix(dir)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        text.push_str(&format!("== {rel}\n"));
        for (table, rows) in dump_all(&db) {
            text.push_str(&format!("{table}: {}\n", rows.len()));
        }
        for sql in [
            "SELECT id, kind, input_ref, state FROM tasks ORDER BY id",
            "SELECT idempotency_key, task_id, kind, status FROM actions ORDER BY idempotency_key",
            "SELECT taxpayer_id, document_id, task_id, idempotency_key FROM sales_document_sends ORDER BY id",
            "SELECT taxpayer_id, document_id, task_id FROM sales_send_triggers ORDER BY document_id",
            "SELECT id, hash FROM verifactu_records ORDER BY id",
            "SELECT record_id, environment FROM verifactu_record_environments ORDER BY record_id",
            "SELECT taxpayer_id, event, location FROM taxpayers ORDER BY id",
            "SELECT id, name FROM customers ORDER BY id",
            "SELECT id, customer_id, active FROM recurring_templates ORDER BY id",
            "SELECT id, version, active FROM rules ORDER BY id, version",
            "SELECT profile, role, taxpayer_id FROM database_identity",
            "SELECT environment FROM verifactu_environment_changes ORDER BY id",
        ] {
            for row in query_strings(&db, sql) {
                text.push_str(&format!("  {row}\n"));
            }
        }
    }
    text
}
