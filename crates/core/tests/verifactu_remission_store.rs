//! Almacenamiento de la remisión Verifactu (E5, D247–D259, migración 16): entorno, lotes,
//! estado derivado, duplicados, `Fault`, subsanación y simulador sembrado. Independiente de
//! `verifactu_store.rs` (D197). Todo con datos sintéticos y con el simulador local en
//! 127.0.0.1; nada contacta con la AEAT.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::time::Duration;

use nestadmin_core::sales::{
    BatchClosure, BatchOutcome, Company, CustomerData, DraftInput, DraftLine, RecordEnvironment,
    VerifactuContext, VerifactuEnvironment, VerifactuKind, VerifactuStatus, CODE_BATCH_OPEN,
    CODE_BATCH_REJECTED, CODE_ENVIRONMENT_BUSY, CODE_ENVIRONMENT_CHANGED, MAX_AUTOMATIC_FAULTS,
    OBLIGATION_AUTHORIZATION,
};
use nestadmin_core::Store;
use nestadmin_documents::IsoDate;
use nestadmin_sales::{DocumentKind, LineInput, TaxRate};
use nestadmin_verifactu::simulador::{ConfigSimulador, ErrorForzado, SimuladorAeat};
use nestadmin_verifactu::{
    Cabecera, Entorno, Fecha, FechaHoraHuso, Persona, PersonaEs, RechazoPrevio,
    RegistroFacturacion, ResultadoEnvio, SistemaInformatico,
};
use rust_decimal::Decimal;

const T0: i64 = 1_800_000_000_000;

fn date(text: &str) -> IsoDate {
    IsoDate::parse("test", text).unwrap()
}

fn system() -> SistemaInformatico {
    SistemaInformatico {
        productor: Persona::con_nif("Productor Demo SL", "B00000034"),
        nombre_sistema: "NestAdmin".into(),
        id_sistema: "NA".into(),
        version: "0.1.0".into(),
        numero_instalacion: "INST-0001".into(),
        solo_verifactu: true,
        multi_ot: false,
        indicador_multiples_ot: false,
    }
}

fn base_unix() -> i64 {
    FechaHoraHuso::new(Fecha::new(2026, 3, 1).unwrap(), 10, 0, 0, 60)
        .unwrap()
        .unix()
}

fn ctx_in(env: VerifactuEnvironment, offset: i64) -> VerifactuContext {
    VerifactuContext {
        system: system(),
        now: FechaHoraHuso::desde_unix(base_unix() + offset, 60).unwrap(),
        environment: env,
    }
}

/// Contexto en el entorno simulador con el reloj `offset` segundos después de la base.
fn ctx(offset: i64) -> VerifactuContext {
    ctx_in(VerifactuEnvironment::Simulador, offset)
}

fn header() -> Cabecera {
    Cabecera::new(PersonaEs::new("Taller Demo SL", "B12345674"))
}

fn setup(store: &mut Store) -> i64 {
    store
        .set_sales_company(&Company {
            name: Some("Taller Demo SL".into()),
            tax_id: Some("B12345674".into()),
            address: Some("Calle Ficticia 1, 28000 Madrid".into()),
            email: Some("demo@example.test".into()),
            iban: None,
            logo: None,
            logo_mime: None,
        })
        .unwrap();
    store
        .create_customer(&CustomerData {
            name: "Cliente Uno SA".into(),
            tax_id: Some("12345678Z".into()),
            address: Some("Avenida Inventada 2, 08000 Barcelona".into()),
            email: Some("cliente@example.test".into()),
        })
        .unwrap()
}

fn new_invoice(store: &mut Store, customer: i64) -> i64 {
    let mut input = DraftInput::new(DocumentKind::Invoice);
    input.customer_id = Some(customer);
    input.lines = vec![DraftLine::from(LineInput {
        description: "Servicio de montaje".into(),
        quantity: Decimal::ONE,
        unit_price: "100.00".parse().unwrap(),
        discount: Decimal::ZERO,
        tax_rate: TaxRate::TwentyOne,
        exemption: None,
    })];
    store.create_sales_draft(&input).unwrap()
}

/// Crea y emite una factura; devuelve (documento, id del registro de alta).
fn issue_at(store: &mut Store, customer: i64, ctx: &VerifactuContext) -> (i64, i64) {
    let id = new_invoice(store, customer);
    store
        .issue_sales_invoice_registered(id, date("2026-03-01"), date("2026-03-01"), ctx)
        .unwrap();
    let record = store
        .verifactu_records()
        .unwrap()
        .into_iter()
        .rev()
        .find(|r| r.document_id == id)
        .unwrap();
    (id, record.id)
}

fn last_record_id(store: &Store) -> i64 {
    store.verifactu_records().unwrap().last().unwrap().id
}

fn serie_of(store: &Store, record_id: i64) -> String {
    store
        .verifactu_records()
        .unwrap()
        .into_iter()
        .find(|r| r.id == record_id)
        .unwrap()
        .invoice_number
}

fn state(store: &Store, record_id: i64) -> VerifactuStatus {
    store.verifactu_record_state(record_id).unwrap().status
}

fn raw(path: &Path) -> rusqlite::Connection {
    rusqlite::Connection::open(path).unwrap()
}

fn count(path: &Path, table: &str) -> i64 {
    raw(path)
        .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}

// --- Transporte mínimo contra el simulador (sin red real) ---

fn post(url: &str, body: &str) -> Option<(u16, String)> {
    let rest = url.strip_prefix("http://")?;
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    let mut s = TcpStream::connect(host).ok()?;
    s.set_read_timeout(Some(Duration::from_secs(10))).ok()?;
    write!(
        s,
        "POST /{path} HTTP/1.1\r\nHost: {host}\r\nContent-Type: text/xml; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .ok()?;
    let mut resp = String::new();
    s.read_to_string(&mut resp).ok()?;
    if resp.is_empty() {
        return None;
    }
    let (head, body) = resp.split_once("\r\n\r\n")?;
    let status = head.split_whitespace().nth(1)?.parse().ok()?;
    Some((status, body.to_owned()))
}

enum Sent {
    NoPending,
    /// Sin respuesta entendida: el lote sigue abierto.
    NoResponse,
    Closed(BatchClosure),
}

impl Sent {
    fn closed(self) -> BatchClosure {
        match self {
            Sent::Closed(c) => c,
            Sent::NoPending => panic!("no había nada que remitir"),
            Sent::NoResponse => panic!("sin respuesta"),
        }
    }
}

/// Un ciclo de la bomba: reenvía el lote abierto o abre uno, anota el intento antes de la
/// llamada y solo cierra con una respuesta entendida o un `Fault`.
fn pump(store: &mut Store, sim: &SimuladorAeat, now: i64) -> Sent {
    let batch = match store.unfinished_verifactu_batch().unwrap() {
        Some(b) => b,
        None => match store
            .open_verifactu_batch(VerifactuEnvironment::Simulador, &header(), now, true)
            .unwrap()
        {
            Some(b) => b,
            None => return Sent::NoPending,
        },
    };
    store.begin_verifactu_attempt(batch.id, now).unwrap();
    match post(&sim.url(), &batch.envio.a_soap().unwrap()) {
        Some((200 | 500, body)) => match ResultadoEnvio::desde_xml(&body) {
            Ok(result) => {
                Sent::Closed(store.close_verifactu_batch(batch.id, &result, now).unwrap())
            }
            Err(_) => Sent::NoResponse,
        },
        _ => Sent::NoResponse,
    }
}

fn simulator() -> SimuladorAeat {
    SimuladorAeat::iniciar(ConfigSimulador::default()).unwrap()
}

fn force(sim: &SimuladorAeat, serie: &str, code: u32, accepted: bool) {
    sim.estado().config.errores.insert(
        serie.to_owned(),
        ErrorForzado {
            codigo: code,
            descripcion: "error forzado de prueba".into(),
            aceptado: accepted,
        },
    );
}

// --- Entorno ---

#[test]
fn the_environment_defaults_to_simulator_and_each_record_stores_its_own() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nestadmin.db");
    let mut store = Store::open(&path).unwrap();
    let customer = setup(&mut store);
    assert_eq!(
        store.verifactu_environment().unwrap(),
        VerifactuEnvironment::Simulador
    );
    let (id, alta) = issue_at(&mut store, customer, &ctx(0));
    store.void_sales_invoice_registered(id, &ctx(60)).unwrap();
    let anulacion = last_record_id(&store);
    for record in [alta, anulacion] {
        let env = store.verifactu_record_environment(record).unwrap();
        assert_eq!(env, RecordEnvironment::Simulador);
        assert_eq!(env.qr_entorno(), Entorno::Pruebas);
    }
    assert_eq!(count(&path, "verifactu_record_environments"), 2);

    // Con registros pendientes no se cambia de entorno: el de los ya creados no se toca.
    assert_eq!(
        store
            .set_verifactu_environment(VerifactuEnvironment::Preproduccion, "ajuste explícito")
            .unwrap_err()
            .code(),
        CODE_ENVIRONMENT_BUSY
    );
}

#[test]
fn a_failed_emission_leaves_no_record_and_no_environment_row() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nestadmin.db");
    let mut store = Store::open(&path).unwrap();
    let customer = setup(&mut store);
    issue_at(&mut store, customer, &ctx(1000));
    let second = new_invoice(&mut store, customer);
    // Reloj atrasado: la emisión se deshace entera.
    store
        .issue_sales_invoice_registered(second, date("2026-03-01"), date("2026-03-01"), &ctx(0))
        .unwrap_err();
    assert_eq!(count(&path, "verifactu_records"), 1);
    assert_eq!(count(&path, "verifactu_record_environments"), 1);
}

#[test]
fn a_context_with_a_stale_environment_generates_nothing() {
    let mut store = Store::open_in_memory().unwrap();
    let customer = setup(&mut store);
    let id = new_invoice(&mut store, customer);
    let err = store
        .issue_sales_invoice_registered(
            id,
            date("2026-03-01"),
            date("2026-03-01"),
            &ctx_in(VerifactuEnvironment::Preproduccion, 0),
        )
        .unwrap_err();
    assert_eq!(err.code(), CODE_ENVIRONMENT_CHANGED);
    assert!(store.verifactu_records().unwrap().is_empty());
    assert_eq!(
        store.sales_document(id).unwrap().status,
        nestadmin_sales::Status::Draft
    );
}

#[test]
fn the_environment_change_history_is_append_only_and_changes_are_traceable() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nestadmin.db");
    let mut store = Store::open(&path).unwrap();
    assert!(!store
        .set_verifactu_environment(VerifactuEnvironment::Simulador, "ajuste")
        .unwrap());
    assert_eq!(count(&path, "verifactu_environment_changes"), 0);
    assert!(store
        .set_verifactu_environment(VerifactuEnvironment::Preproduccion, "ajuste explícito")
        .unwrap());
    assert!(store
        .set_verifactu_environment(VerifactuEnvironment::Simulador, "vuelta al simulador")
        .unwrap());
    assert_eq!(
        store.verifactu_environment().unwrap(),
        VerifactuEnvironment::Simulador
    );
    assert_eq!(count(&path, "verifactu_environment_changes"), 2);
    // Sin autorización no cambia.
    let err = store
        .set_verifactu_environment(VerifactuEnvironment::Produccion, "  ")
        .unwrap_err();
    assert_eq!(err.code(), "invalid_input");
    // El cambio consta en el historial con su autorización.
    let log = store.audit_log().unwrap();
    assert!(log.iter().any(|e| e.action == "verifactu.environment"
        && e.authorization.as_deref() == Some("ajuste explícito")));
    let conn = raw(&path);
    assert!(conn
        .execute(
            "UPDATE verifactu_environment_changes SET environment = 'produccion'",
            []
        )
        .is_err());
    assert!(conn
        .execute("DELETE FROM verifactu_environment_changes", [])
        .is_err());
}

#[test]
fn the_environment_cannot_change_with_pending_sending_or_unsubsanated_rejected_records() {
    let mut store = Store::open_in_memory().unwrap();
    let customer = setup(&mut store);
    let sim = simulator();
    let (_, first) = issue_at(&mut store, customer, &ctx(0));

    // Pendiente.
    let busy = |s: &mut Store| {
        s.set_verifactu_environment(VerifactuEnvironment::Preproduccion, "ajuste")
            .unwrap_err()
    };
    assert_eq!(busy(&mut store).code(), CODE_ENVIRONMENT_BUSY);

    // En envío: lote abierto sin resultado.
    let batch = store
        .open_verifactu_batch(VerifactuEnvironment::Simulador, &header(), T0, false)
        .unwrap()
        .unwrap();
    assert_eq!(state(&store, first), VerifactuStatus::Sending);
    assert_eq!(busy(&mut store).code(), CODE_ENVIRONMENT_BUSY);

    // Rechazado sin subsanar.
    force(&sim, &serie_of(&store, first), 1100, false);
    store.begin_verifactu_attempt(batch.id, T0).unwrap();
    let (_, body) = post(&sim.url(), &batch.envio.a_soap().unwrap()).unwrap();
    store
        .close_verifactu_batch(batch.id, &ResultadoEnvio::desde_xml(&body).unwrap(), T0)
        .unwrap();
    assert_eq!(state(&store, first), VerifactuStatus::Rejected);
    assert_eq!(busy(&mut store).code(), CODE_ENVIRONMENT_BUSY);

    // Subsanado y aceptado: ya se puede.
    sim.estado().config.errores.clear();
    store.create_verifactu_subsanation(first, &ctx(60)).unwrap();
    assert_eq!(
        busy(&mut store).code(),
        CODE_ENVIRONMENT_BUSY,
        "la subsanación está pendiente"
    );
    pump(&mut store, &sim, T0 + 1).closed();
    assert!(store
        .set_verifactu_environment(VerifactuEnvironment::Preproduccion, "ajuste")
        .unwrap());
}

#[test]
fn records_before_migration_16_get_the_inherited_production_environment() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nestadmin.db");
    let first;
    {
        let mut store = Store::open(&path).unwrap();
        let customer = setup(&mut store);
        first = issue_at(&mut store, customer, &ctx(0)).1;
    }
    // Como la dejaría la versión 15: sin las tablas de remisión.
    let conn = raw(&path);
    conn.execute_batch(
        concat!(include_str!("sql/downgrade18.sql"), "DROP TABLE verifactu_authorization_halts; DROP TABLE verifactu_incidents; DROP TABLE verifactu_unsent_attempts; DROP TABLE verifactu_tls12_preferences; DROP TABLE verifactu_remission_settings; DROP TABLE verifactu_subsanations; DROP TABLE verifactu_record_results; \
         DROP TABLE verifactu_batch_results; DROP TABLE verifactu_batch_attempts; \
         DROP TABLE verifactu_batch_records; DROP TABLE verifactu_batches; \
         DROP TABLE verifactu_record_environments; DROP TABLE verifactu_environment_changes; \
         PRAGMA user_version = 15;"),
    )
    .unwrap();
    drop(conn);

    let mut store = Store::open(&path).unwrap();
    let env = store.verifactu_record_environment(first).unwrap();
    assert_eq!(env, RecordEnvironment::ProduccionHeredado);
    assert_eq!(
        env.qr_entorno(),
        Entorno::Produccion,
        "mismos bytes del PDF (D226)"
    );
    // Los registros nuevos llevan el entorno vigente.
    let customer = store.customers(false).unwrap()[0].id;
    let (_, second) = issue_at(&mut store, customer, &ctx(60));
    assert_eq!(
        store.verifactu_record_environment(second).unwrap(),
        RecordEnvironment::Simulador
    );
    // Los heredados son pendientes de producción, no del simulador.
    assert_eq!(
        store
            .verifactu_pending_records(VerifactuEnvironment::Simulador)
            .unwrap()
            .iter()
            .map(|r| r.id)
            .collect::<Vec<_>>(),
        vec![second]
    );
    assert_eq!(
        store
            .verifactu_pending_records(VerifactuEnvironment::Produccion)
            .unwrap()
            .iter()
            .map(|r| r.id)
            .collect::<Vec<_>>(),
        vec![first]
    );
}

#[test]
fn a_record_without_an_environment_row_is_an_error_not_a_silent_production() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nestadmin.db");
    let mut store = Store::open(&path).unwrap();
    let customer = setup(&mut store);
    let (_, record) = issue_at(&mut store, customer, &ctx(0));
    let conn = raw(&path);
    conn.execute_batch(
        "DROP TRIGGER verifactu_record_environments_no_delete; \
         DELETE FROM verifactu_record_environments;",
    )
    .unwrap();
    drop(conn);
    assert_eq!(
        store
            .verifactu_record_environment(record)
            .unwrap_err()
            .code(),
        "storage"
    );
    assert_eq!(
        store
            .verifactu_pending_records(VerifactuEnvironment::Simulador)
            .unwrap_err()
            .code(),
        "storage"
    );
    assert!(store
        .open_verifactu_batch(VerifactuEnvironment::Simulador, &header(), T0, true)
        .is_err());
}

#[test]
fn the_remission_tables_are_append_only() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nestadmin.db");
    let mut store = Store::open(&path).unwrap();
    let customer = setup(&mut store);
    let sim = simulator();
    let (_, first) = issue_at(&mut store, customer, &ctx(0));
    force(&sim, &serie_of(&store, first), 1100, false);
    pump(&mut store, &sim, T0).closed();
    store.create_verifactu_subsanation(first, &ctx(60)).unwrap();
    let conn = raw(&path);
    for table in [
        "verifactu_record_environments",
        "verifactu_batches",
        "verifactu_batch_records",
        "verifactu_batch_attempts",
        "verifactu_batch_results",
        "verifactu_record_results",
        "verifactu_subsanations",
    ] {
        assert!(count(&path, table) > 0, "{table} sin filas");
        let column: String = conn
            .query_row(
                &format!("SELECT name FROM pragma_table_info('{table}') LIMIT 1"),
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            conn.execute(&format!("UPDATE {table} SET {column} = {column}"), [])
                .is_err(),
            "UPDATE en {table}"
        );
        assert!(
            conn.execute(&format!("DELETE FROM {table}"), []).is_err(),
            "DELETE en {table}"
        );
    }
}

// --- Pendientes y composición del lote ---

#[test]
fn pending_records_come_in_chain_order_and_a_batch_takes_them_in_that_order() {
    let mut store = Store::open_in_memory().unwrap();
    let customer = setup(&mut store);
    let ids: Vec<i64> = (0..3)
        .map(|i| issue_at(&mut store, customer, &ctx(i * 30)).1)
        .collect();
    let pending: Vec<i64> = store
        .verifactu_pending_records(VerifactuEnvironment::Simulador)
        .unwrap()
        .iter()
        .map(|r| r.id)
        .collect();
    assert_eq!(pending, ids);
    let batch = store
        .open_verifactu_batch(VerifactuEnvironment::Simulador, &header(), T0, false)
        .unwrap()
        .unwrap();
    assert_eq!(batch.record_ids, ids);
    assert_eq!(batch.envio.registros.len(), 3);
    assert_eq!(batch.attempts, 0);
    assert_eq!(batch.environment, VerifactuEnvironment::Simulador);
    // Ya no son pendientes: están en envío.
    assert!(store
        .verifactu_pending_records(VerifactuEnvironment::Simulador)
        .unwrap()
        .is_empty());
    assert!(ids
        .iter()
        .all(|id| state(&store, *id) == VerifactuStatus::Sending));
}

#[test]
fn there_is_never_a_second_open_batch_even_from_another_connection() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nestadmin.db");
    let mut a = Store::open(&path).unwrap();
    let customer = setup(&mut a);
    issue_at(&mut a, customer, &ctx(0));
    let mut b = Store::open(&path).unwrap();
    let batch = a
        .open_verifactu_batch(VerifactuEnvironment::Simulador, &header(), T0, false)
        .unwrap()
        .unwrap();
    // Otra emisión deja otro pendiente; la segunda conexión no abre un lote paralelo.
    issue_at(&mut a, customer, &ctx(30));
    let err = b
        .open_verifactu_batch(VerifactuEnvironment::Simulador, &header(), T0, true)
        .unwrap_err();
    assert_eq!(err.code(), CODE_BATCH_OPEN);
    assert_eq!(count(&path, "verifactu_batches"), 1);
    assert_eq!(
        b.unfinished_verifactu_batch().unwrap().unwrap().id,
        batch.id
    );
}

#[test]
fn a_batch_has_at_most_1000_records_and_the_rest_go_in_the_next_one() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nestadmin.db");
    let mut store = Store::open(&path).unwrap();
    let customer = setup(&mut store);
    let (doc, first) = issue_at(&mut store, customer, &ctx(0));
    // 1000 altas más, insertadas directamente: emitirlas una a una verificaría la cadena
    // entera cada vez. Cadena coherente y huellas distintas; el lote no recalcula huellas.
    let template = store.verifactu_records().unwrap().pop().unwrap();
    let conn = raw(&path);
    let mut previous = template.clone();
    for i in 0..1000u64 {
        let RegistroFacturacion::Alta(mut alta) = previous.record.clone() else {
            unreachable!()
        };
        alta.id_factura.num_serie = format!("BULK-{i:04}");
        alta.encadenamiento = previous.record.siguiente_encadenamiento();
        alta.huella = format!("{:064X}", i + 1);
        let record = RegistroFacturacion::Alta(alta);
        let id = record.id_factura().clone();
        conn.execute(
            "INSERT INTO verifactu_records (kind, document_id, issuer_tax_id, invoice_number, \
             issue_date, invoice_type, total, tax_total, previous_id, previous_hash, \
             generated_at, hash, record_json, created_at) \
             VALUES ('alta', ?1, ?2, ?3, ?4, 'F1', '121.00', '21.00', ?5, ?6, ?7, ?8, ?9, 0)",
            rusqlite::params![
                doc,
                id.nif_emisor,
                id.num_serie,
                id.fecha_expedicion.to_string(),
                previous.id,
                previous.hash,
                template.generated_at,
                record.huella(),
                serde_json::to_string(&record).unwrap(),
            ],
        )
        .unwrap();
        let new_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO verifactu_record_environments (record_id, environment) \
             VALUES (?1, 'simulador')",
            [new_id],
        )
        .unwrap();
        previous = nestadmin_core::sales::VerifactuRecordRow {
            id: new_id,
            hash: record.huella().to_owned(),
            record,
            ..previous
        };
    }
    assert_eq!(
        store
            .verifactu_pending_records(VerifactuEnvironment::Simulador)
            .unwrap()
            .len(),
        1001
    );
    let batch = store
        .open_verifactu_batch(VerifactuEnvironment::Simulador, &header(), T0, false)
        .unwrap()
        .unwrap();
    assert_eq!(batch.record_ids.len(), 1000);
    assert_eq!(batch.record_ids[0], first);
    // Cierra el lote con una respuesta que acepta lo que lleva (sin simulador: 1000 líneas).
    store.begin_verifactu_attempt(batch.id, T0).unwrap();
    let lines: String = batch
        .envio
        .registros
        .iter()
        .map(|r| {
            let id = r.id_factura();
            format!(
                "<tikR:RespuestaLinea><tikR:IDFactura><tik:IDEmisorFactura>{}</tik:IDEmisorFactura><tik:NumSerieFactura>{}</tik:NumSerieFactura><tik:FechaExpedicionFactura>{}</tik:FechaExpedicionFactura></tikR:IDFactura><tikR:Operacion><tik:TipoOperacion>Alta</tik:TipoOperacion></tikR:Operacion><tikR:EstadoRegistro>Correcto</tikR:EstadoRegistro></tikR:RespuestaLinea>",
                id.nif_emisor, id.num_serie, id.fecha_expedicion
            )
        })
        .collect();
    let xml = format!(
        "<env:Envelope xmlns:env=\"http://schemas.xmlsoap.org/soap/envelope/\"><env:Body><tikR:RespuestaRegFactuSistemaFacturacion xmlns:tikR=\"urn:r\" xmlns:tik=\"urn:t\"><tikR:CSV>CSVX</tikR:CSV><tikR:TiempoEsperaEnvio>60</tikR:TiempoEsperaEnvio><tikR:EstadoEnvio>Correcto</tikR:EstadoEnvio>{lines}</tikR:RespuestaRegFactuSistemaFacturacion></env:Body></env:Envelope>"
    );
    let closure = store
        .close_verifactu_batch(batch.id, &ResultadoEnvio::desde_xml(&xml).unwrap(), T0)
        .unwrap();
    assert_eq!(closure.accepted, 1000);
    assert!(closure.missing_record_ids.is_empty());
    let second = store
        .open_verifactu_batch(
            VerifactuEnvironment::Simulador,
            &header(),
            T0 + 60_000,
            false,
        )
        .unwrap()
        .unwrap();
    assert_eq!(second.record_ids.len(), 1);
}

#[test]
fn the_alta_and_the_anulacion_of_the_same_invoice_go_in_different_batches() {
    let mut store = Store::open_in_memory().unwrap();
    let customer = setup(&mut store);
    let sim = simulator();
    let (doc, alta) = issue_at(&mut store, customer, &ctx(0));
    store.void_sales_invoice_registered(doc, &ctx(60)).unwrap();
    let anulacion = last_record_id(&store);
    // Una tercera factura detrás de la anulación no se cuela en el primer lote.
    let (_, other) = issue_at(&mut store, customer, &ctx(120));

    let pending: Vec<i64> = store
        .verifactu_pending_records(VerifactuEnvironment::Simulador)
        .unwrap()
        .iter()
        .map(|r| r.id)
        .collect();
    assert_eq!(pending, vec![alta, anulacion, other]);

    let first = store
        .open_verifactu_batch(VerifactuEnvironment::Simulador, &header(), T0, false)
        .unwrap()
        .unwrap();
    assert_eq!(
        first.record_ids,
        vec![alta],
        "el lote se corta antes de la anulación"
    );
    store.begin_verifactu_attempt(first.id, T0).unwrap();
    let (_, body) = post(&sim.url(), &first.envio.a_soap().unwrap()).unwrap();
    store
        .close_verifactu_batch(first.id, &ResultadoEnvio::desde_xml(&body).unwrap(), T0)
        .unwrap();

    let second = store
        .open_verifactu_batch(
            VerifactuEnvironment::Simulador,
            &header(),
            T0 + 60_000,
            false,
        )
        .unwrap()
        .unwrap();
    assert_eq!(second.record_ids, vec![anulacion, other]);
    let kinds: Vec<VerifactuKind> = store
        .verifactu_records()
        .unwrap()
        .iter()
        .filter(|r| second.record_ids.contains(&r.id))
        .map(|r| r.kind)
        .collect();
    assert_eq!(kinds, vec![VerifactuKind::Anulacion, VerifactuKind::Alta]);
}

// --- Cierre, estado derivado y reenvío ---

#[test]
fn a_response_closes_the_batch_and_each_record_gets_its_state() {
    let mut store = Store::open_in_memory().unwrap();
    let customer = setup(&mut store);
    let sim = simulator();
    let a = issue_at(&mut store, customer, &ctx(0)).1;
    let b = issue_at(&mut store, customer, &ctx(30)).1;
    let c = issue_at(&mut store, customer, &ctx(60)).1;
    force(&sim, &serie_of(&store, b), 2001, true);
    force(&sim, &serie_of(&store, c), 1100, false);

    let closure = pump(&mut store, &sim, T0).closed();
    assert_eq!(closure.outcome, BatchOutcome::Responded);
    assert_eq!(closure.accepted, 1);
    assert_eq!(closure.accepted_with_errors, 1);
    assert_eq!(closure.rejected, 1);
    assert!(closure.csv.as_deref().unwrap().starts_with("CSVSIM"));
    assert_eq!(closure.wait_seconds, 60);

    assert_eq!(state(&store, a), VerifactuStatus::Accepted);
    let b_state = store.verifactu_record_state(b).unwrap();
    assert_eq!(b_state.status, VerifactuStatus::AcceptedWithErrors);
    assert_eq!(b_state.aeat_code, Some(2001));
    let c_state = store.verifactu_record_state(c).unwrap();
    assert_eq!(c_state.status, VerifactuStatus::Rejected);
    assert_eq!(c_state.aeat_code, Some(1100));
    assert_eq!(
        c_state.aeat_description.as_deref(),
        Some("error forzado de prueba")
    );
    // Nada pendiente; el rechazado no se reenvía solo.
    assert!(matches!(
        pump(&mut store, &sim, T0 + 60_000),
        Sent::NoPending
    ));
    let summary = store
        .verifactu_last_batch(VerifactuEnvironment::Simulador)
        .unwrap()
        .unwrap();
    let result = summary.result.unwrap();
    assert_eq!(summary.record_count, 3);
    assert_eq!(result.outcome, BatchOutcome::Responded);
    assert_eq!(result.send_state.as_deref(), Some("ParcialmenteCorrecto"));
    assert_eq!(
        store
            .verifactu_accepted_records(VerifactuEnvironment::Simulador)
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn an_open_batch_is_resent_identical_even_after_reopening_the_database() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nestadmin.db");
    let (batch_id, bytes);
    {
        let mut store = Store::open(&path).unwrap();
        let customer = setup(&mut store);
        issue_at(&mut store, customer, &ctx(0));
        issue_at(&mut store, customer, &ctx(30));
        let batch = store
            .open_verifactu_batch(VerifactuEnvironment::Simulador, &header(), T0, false)
            .unwrap()
            .unwrap();
        assert_eq!(store.begin_verifactu_attempt(batch.id, T0).unwrap(), 1);
        batch_id = batch.id;
        bytes = batch.envio.a_soap().unwrap();
        // Entre la apertura y el reenvío se emite otra factura: no entra en el lote abierto.
        issue_at(&mut store, customer, &ctx(60));
    }
    let mut store = Store::open(&path).unwrap();
    let again = store.unfinished_verifactu_batch().unwrap().unwrap();
    assert_eq!(again.id, batch_id);
    assert_eq!(again.attempts, 1);
    assert_eq!(again.envio.a_soap().unwrap(), bytes, "mismos bytes y orden");
    assert_eq!(again.record_ids.len(), 2);
    assert_eq!(
        count(&path, "verifactu_batches"),
        1,
        "sin lote nuevo ni registros nuevos"
    );
    assert_eq!(count(&path, "verifactu_records"), 3);
    assert_eq!(store.begin_verifactu_attempt(batch_id, T0 + 1).unwrap(), 2);
}

#[test]
fn a_lost_response_resent_as_a_duplicate_counts_as_accepted_without_a_second_record() {
    let mut store = Store::open_in_memory().unwrap();
    let customer = setup(&mut store);
    let sim = simulator();
    let a = issue_at(&mut store, customer, &ctx(0)).1;
    let b = issue_at(&mut store, customer, &ctx(30)).1;
    sim.estado().perder_siguiente_respuesta = true;
    assert!(matches!(pump(&mut store, &sim, T0), Sent::NoResponse));
    assert_eq!(state(&store, a), VerifactuStatus::Sending);
    assert_eq!(sim.estado().registros.len(), 2, "el envío sí llegó");

    let closure = pump(&mut store, &sim, T0 + 60_000).closed();
    assert_eq!(closure.accepted, 2);
    assert_eq!(closure.rejected, 0);
    for id in [a, b] {
        let s = store.verifactu_record_state(id).unwrap();
        assert_eq!(s.status, VerifactuStatus::Accepted);
        assert!(s.duplicate, "aceptado por duplicado de un intento anterior");
    }
    assert_eq!(store.verifactu_records().unwrap().len(), 2);
    assert_eq!(
        store
            .verifactu_last_batch(VerifactuEnvironment::Simulador)
            .unwrap()
            .unwrap()
            .attempts,
        2
    );
    // Cerrarlo otra vez no repite nada.
    let batch_id = closure.batch_id;
    let err = store
        .close_verifactu_batch(
            batch_id,
            &ResultadoEnvio::Fallo(nestadmin_verifactu::FalloSoap {
                faultcode: "x".into(),
                faultstring: "x".into(),
                codigo: None,
            }),
            T0,
        )
        .unwrap_err();
    assert_eq!(err.code(), "conflict");
    assert_eq!(
        store
            .begin_verifactu_attempt(batch_id, T0)
            .unwrap_err()
            .code(),
        "conflict"
    );
}

#[test]
fn a_duplicate_the_database_never_sent_is_an_unexpected_duplicate_and_not_accepted() {
    let mut store = Store::open_in_memory().unwrap();
    let customer = setup(&mut store);
    let sim = simulator();
    let a = issue_at(&mut store, customer, &ctx(0)).1;
    // La AEAT (simulada) ya tiene ese registro por otra vía, no por un lote de esta base.
    sim.sembrar(&[store.verifactu_records().unwrap()[0].record.clone()]);

    let closure = pump(&mut store, &sim, T0).closed();
    assert_eq!(closure.accepted, 0);
    assert_eq!(closure.unexpected_duplicates, 1);
    let s = store.verifactu_record_state(a).unwrap();
    assert_eq!(s.status, VerifactuStatus::UnexpectedDuplicate);
    assert_eq!(s.aeat_code, Some(3000));
    assert!(s.aeat_description.unwrap().contains("Correcta"));
    assert!(store
        .verifactu_accepted_records(VerifactuEnvironment::Simulador)
        .unwrap()
        .is_empty());
    // No es pendiente: no se reenvía en bucle.
    assert!(matches!(
        pump(&mut store, &sim, T0 + 60_000),
        Sent::NoPending
    ));
}

#[test]
fn a_duplicate_alta_the_aeat_holds_as_annulled_is_not_accepted_even_when_resent() {
    let mut store = Store::open_in_memory().unwrap();
    let customer = setup(&mut store);
    let sim = simulator();
    let (doc, a) = issue_at(&mut store, customer, &ctx(0));
    store.void_sales_invoice_registered(doc, &ctx(60)).unwrap();
    let records = store.verifactu_records().unwrap();
    // La AEAT tiene la factura dada de baja; el lote con el alta se pierde y se reenvía.
    sim.sembrar(&[records[0].record.clone(), records[1].record.clone()]);
    sim.estado().perder_siguiente_respuesta = true;
    assert!(matches!(pump(&mut store, &sim, T0), Sent::NoResponse));
    let closure = pump(&mut store, &sim, T0 + 60_000).closed();
    assert_eq!(closure.accepted, 0);
    assert_eq!(closure.unexpected_duplicates, 1);
    let s = store.verifactu_record_state(a).unwrap();
    assert_eq!(s.status, VerifactuStatus::UnexpectedDuplicate);
    assert!(s.aeat_description.unwrap().contains("Anulada"));
}

#[test]
fn a_resent_anulacion_answered_with_3001_counts_as_accepted_only_if_it_was_in_an_open_batch() {
    let mut store = Store::open_in_memory().unwrap();
    let customer = setup(&mut store);
    let sim = simulator();
    let (doc, alta) = issue_at(&mut store, customer, &ctx(0));
    pump(&mut store, &sim, T0).closed();
    assert_eq!(state(&store, alta), VerifactuStatus::Accepted);
    store.void_sales_invoice_registered(doc, &ctx(60)).unwrap();
    let anulacion = last_record_id(&store);

    // Se pierde la respuesta; el reenvío del mismo lote recibe 3001: aceptada.
    sim.estado().perder_siguiente_respuesta = true;
    assert!(matches!(
        pump(&mut store, &sim, T0 + 60_000),
        Sent::NoResponse
    ));
    let closure = pump(&mut store, &sim, T0 + 120_000).closed();
    assert_eq!(closure.accepted, 1);
    let s = store.verifactu_record_state(anulacion).unwrap();
    assert_eq!(s.status, VerifactuStatus::Accepted);
    assert!(s.duplicate);
    assert_eq!(s.aeat_code, Some(3001));
}

#[test]
fn a_3001_on_the_first_attempt_of_an_anulacion_is_a_rejection() {
    let mut store = Store::open_in_memory().unwrap();
    let customer = setup(&mut store);
    let sim = simulator();
    let (doc, _) = issue_at(&mut store, customer, &ctx(0));
    pump(&mut store, &sim, T0).closed();
    store.void_sales_invoice_registered(doc, &ctx(60)).unwrap();
    let anulacion = last_record_id(&store);
    // La baja ya consta en la AEAT por otra vía (no por un lote de esta base).
    sim.sembrar(&[store.verifactu_records().unwrap().pop().unwrap().record]);
    let closure = pump(&mut store, &sim, T0 + 60_000).closed();
    assert_eq!(closure.accepted, 0);
    assert_eq!(closure.rejected, 1);
    let s = store.verifactu_record_state(anulacion).unwrap();
    assert_eq!(s.status, VerifactuStatus::Rejected);
    assert_eq!(s.aeat_code, Some(3001));
    assert!(!s.duplicate);
}

#[test]
fn a_partial_response_leaves_the_missing_record_pending_and_explicit() {
    let mut store = Store::open_in_memory().unwrap();
    let customer = setup(&mut store);
    let a = issue_at(&mut store, customer, &ctx(0)).1;
    let b = issue_at(&mut store, customer, &ctx(30)).1;
    let batch = store
        .open_verifactu_batch(VerifactuEnvironment::Simulador, &header(), T0, false)
        .unwrap()
        .unwrap();
    store.begin_verifactu_attempt(batch.id, T0).unwrap();
    let only_first = {
        let id = batch.envio.registros[0].id_factura();
        format!(
            "<env:Envelope xmlns:env=\"http://schemas.xmlsoap.org/soap/envelope/\"><env:Body><tikR:RespuestaRegFactuSistemaFacturacion xmlns:tikR=\"urn:r\" xmlns:tik=\"urn:t\"><tikR:TiempoEsperaEnvio>90</tikR:TiempoEsperaEnvio><tikR:EstadoEnvio>ParcialmenteCorrecto</tikR:EstadoEnvio><tikR:RespuestaLinea><tikR:IDFactura><tik:IDEmisorFactura>{}</tik:IDEmisorFactura><tik:NumSerieFactura>{}</tik:NumSerieFactura><tik:FechaExpedicionFactura>{}</tik:FechaExpedicionFactura></tikR:IDFactura><tikR:Operacion><tik:TipoOperacion>Alta</tik:TipoOperacion></tikR:Operacion><tikR:EstadoRegistro>Correcto</tikR:EstadoRegistro></tikR:RespuestaLinea></tikR:RespuestaRegFactuSistemaFacturacion></env:Body></env:Envelope>",
            id.nif_emisor, id.num_serie, id.fecha_expedicion
        )
    };
    let closure = store
        .close_verifactu_batch(
            batch.id,
            &ResultadoEnvio::desde_xml(&only_first).unwrap(),
            T0,
        )
        .unwrap();
    assert_eq!(closure.accepted, 1);
    assert_eq!(closure.missing_record_ids, vec![b]);
    assert_eq!(closure.wait_seconds, 90);
    assert_eq!(state(&store, a), VerifactuStatus::Accepted);
    assert_eq!(state(&store, b), VerifactuStatus::Pending);
    let next = store
        .open_verifactu_batch(
            VerifactuEnvironment::Simulador,
            &header(),
            T0 + 90_000,
            false,
        )
        .unwrap()
        .unwrap();
    assert_eq!(next.record_ids, vec![b]);
}

// --- Fault, control de flujo ---

#[test]
fn a_schema_fault_closes_the_batch_returns_records_to_pending_and_stops_after_three() {
    let mut store = Store::open_in_memory().unwrap();
    let customer = setup(&mut store);
    // Un censo que no incluye al emisor: la AEAT simulada responde `Fault` 4104.
    let sim = SimuladorAeat::iniciar(ConfigSimulador {
        censo: Some(vec!["B99999999".into()]),
        ..ConfigSimulador::default()
    })
    .unwrap();
    let a = issue_at(&mut store, customer, &ctx(0)).1;

    for n in 1..=MAX_AUTOMATIC_FAULTS {
        let closure = pump(&mut store, &sim, T0 + i64::from(n) * 60_000).closed();
        assert_eq!(closure.outcome, BatchOutcome::Fault);
        assert_eq!(closure.fault_code, Some(4104));
        assert_eq!(
            state(&store, a),
            VerifactuStatus::Pending,
            "vuelve a pendientes"
        );
        assert_eq!(
            store
                .verifactu_consecutive_faults(VerifactuEnvironment::Simulador)
                .unwrap(),
            n
        );
    }
    // Tras tres `Fault` seguidos la remisión automática se detiene...
    let err = store
        .open_verifactu_batch(
            VerifactuEnvironment::Simulador,
            &header(),
            T0 + 600_000,
            false,
        )
        .unwrap_err();
    assert_eq!(err.code(), CODE_BATCH_REJECTED);
    // ...hasta que el usuario la pide.
    assert!(store
        .open_verifactu_batch(
            VerifactuEnvironment::Simulador,
            &header(),
            T0 + 600_000,
            true
        )
        .unwrap()
        .is_some());
    let last = store
        .verifactu_last_batch(VerifactuEnvironment::Simulador)
        .unwrap()
        .unwrap();
    assert!(last.result.is_none());
}

#[test]
fn a_responded_batch_resets_the_fault_counter() {
    let mut store = Store::open_in_memory().unwrap();
    let customer = setup(&mut store);
    let sim = simulator();
    let bad = SimuladorAeat::iniciar(ConfigSimulador {
        censo: Some(vec!["B99999999".into()]),
        ..ConfigSimulador::default()
    })
    .unwrap();
    issue_at(&mut store, customer, &ctx(0));
    pump(&mut store, &bad, T0).closed();
    pump(&mut store, &bad, T0 + 60_000).closed();
    assert_eq!(
        store
            .verifactu_consecutive_faults(VerifactuEnvironment::Simulador)
            .unwrap(),
        2
    );
    pump(&mut store, &sim, T0 + 120_000).closed();
    assert_eq!(
        store
            .verifactu_consecutive_faults(VerifactuEnvironment::Simulador)
            .unwrap(),
        0
    );
}

#[test]
fn the_next_send_follows_the_aeat_wait_and_the_minimum_after_a_failure() {
    let mut store = Store::open_in_memory().unwrap();
    let customer = setup(&mut store);
    let sim = SimuladorAeat::iniciar(ConfigSimulador {
        tiempo_espera_envio: 120,
        ..ConfigSimulador::default()
    })
    .unwrap();
    assert_eq!(
        store
            .verifactu_next_send_at(VerifactuEnvironment::Simulador)
            .unwrap(),
        None
    );
    issue_at(&mut store, customer, &ctx(0));
    // Sin respuesta: espera mínima de 60 s desde el último intento.
    sim.estado().fallo_http_siguiente = Some(503);
    assert!(matches!(pump(&mut store, &sim, T0), Sent::NoResponse));
    assert_eq!(
        store
            .verifactu_next_send_at(VerifactuEnvironment::Simulador)
            .unwrap(),
        Some(T0 + 60_000)
    );
    // Con respuesta: fin del lote más la espera que indica la AEAT.
    pump(&mut store, &sim, T0 + 60_000).closed();
    assert_eq!(
        store
            .verifactu_next_send_at(VerifactuEnvironment::Simulador)
            .unwrap(),
        Some(T0 + 60_000 + 120_000)
    );
    // Un lote nuevo sin respuesta: manda el mayor de los dos.
    issue_at(&mut store, customer, &ctx(30));
    sim.estado().fallo_http_siguiente = Some(503);
    assert!(matches!(
        pump(&mut store, &sim, T0 + 150_000),
        Sent::NoResponse
    ));
    assert_eq!(
        store
            .verifactu_next_send_at(VerifactuEnvironment::Simulador)
            .unwrap(),
        Some(T0 + 180_000).max(Some(T0 + 150_000 + 60_000))
    );
}

#[test]
fn issuing_while_a_batch_is_open_or_the_network_is_down_is_independent_of_the_remission() {
    let mut store = Store::open_in_memory().unwrap();
    let customer = setup(&mut store);
    let sim = simulator();
    let (_, first) = issue_at(&mut store, customer, &ctx(0));
    sim.estado().fallo_http_siguiente = Some(503);
    assert!(matches!(pump(&mut store, &sim, T0), Sent::NoResponse));
    // Sin red y con un lote abierto la emisión sigue funcionando: el registro nuevo queda
    // pendiente y el lote abierto no cambia.
    let (_, second) = issue_at(&mut store, customer, &ctx(30));
    assert_eq!(state(&store, first), VerifactuStatus::Sending);
    assert_eq!(state(&store, second), VerifactuStatus::Pending);
    assert_eq!(
        store
            .unfinished_verifactu_batch()
            .unwrap()
            .unwrap()
            .record_ids,
        vec![first]
    );
}

#[test]
fn batch_opening_and_closing_leave_the_obligation_authorization_in_the_history() {
    let mut store = Store::open_in_memory().unwrap();
    let customer = setup(&mut store);
    let sim = simulator();
    issue_at(&mut store, customer, &ctx(0));
    pump(&mut store, &sim, T0).closed();
    let expected = format!("{OBLIGATION_AUTHORIZATION} simulador");
    let log = store.audit_log().unwrap();
    for action in ["verifactu.batch_open", "verifactu.batch_close"] {
        let entry = log.iter().find(|e| e.action == action).unwrap();
        assert_eq!(entry.authorization.as_deref(), Some(expected.as_str()));
    }
}

// --- Subsanación ---

#[test]
fn a_rejected_alta_is_subsanated_once_and_the_new_record_is_chained_pending_and_accepted() {
    let mut store = Store::open_in_memory().unwrap();
    let customer = setup(&mut store);
    let sim = simulator();
    let (doc, original) = issue_at(&mut store, customer, &ctx(0));
    force(&sim, &serie_of(&store, original), 1100, false);
    pump(&mut store, &sim, T0).closed();
    assert_eq!(state(&store, original), VerifactuStatus::Rejected);

    let sub = store
        .create_verifactu_subsanation(original, &ctx(120))
        .unwrap();
    assert!(sub.created);
    // Doble llamada (doble clic): el mismo registro, ninguno nuevo.
    let again = store
        .create_verifactu_subsanation(original, &ctx(180))
        .unwrap();
    assert!(!again.created);
    assert_eq!(again.record_id, sub.record_id);
    assert_eq!(store.verifactu_records().unwrap().len(), 2);

    let rows = store.verifactu_records().unwrap();
    let (old, new) = (&rows[0], &rows[1]);
    assert_eq!(new.id, sub.record_id);
    assert_eq!(new.document_id, doc);
    assert_eq!(new.previous_id, Some(old.id));
    assert_eq!(new.invoice_number, old.invoice_number);
    let RegistroFacturacion::Alta(a) = &new.record else {
        panic!("se esperaba un alta")
    };
    assert!(a.subsanacion);
    assert_eq!(a.rechazo_previo, Some(RechazoPrevio::X));
    assert_eq!(
        store.verify_verifactu_chain().unwrap(),
        nestadmin_core::sales::ChainStatus::Ok { records: 2 }
    );
    // El rechazado queda como estaba (subsanado, con su código) y el nuevo, pendiente.
    let old_state = store.verifactu_record_state(original).unwrap();
    assert_eq!(old_state.status, VerifactuStatus::Subsanated);
    assert_eq!(old_state.subsanation_record_id, Some(sub.record_id));
    assert_eq!(old_state.aeat_code, Some(1100));
    assert_eq!(state(&store, sub.record_id), VerifactuStatus::Pending);
    assert_eq!(
        store.verifactu_record_environment(sub.record_id).unwrap(),
        RecordEnvironment::Simulador
    );

    // El simulador acepta la subsanación con RechazoPrevio=X sin registro previo.
    sim.estado().config.errores.clear();
    let closure = pump(&mut store, &sim, T0 + 60_000).closed();
    assert_eq!(closure.accepted, 1);
    assert_eq!(state(&store, sub.record_id), VerifactuStatus::Accepted);
    // El QR y `registered_id` siguen usando la primera alta del documento.
    store.void_sales_invoice_registered(doc, &ctx(240)).unwrap();
    let rows = store.verifactu_records().unwrap();
    let RegistroFacturacion::Anulacion(cancel) = &rows[2].record else {
        panic!("se esperaba una anulación")
    };
    assert_eq!(&cancel.id_factura, rows[0].record.id_factura());
    assert!(!cancel.sin_registro_previo, "el alta subsanada consta");
    assert!(!cancel.rechazo_previo);
}

#[test]
fn only_a_rejected_record_is_subsanated() {
    let mut store = Store::open_in_memory().unwrap();
    let customer = setup(&mut store);
    let sim = simulator();
    let (_, accepted_with_errors) = issue_at(&mut store, customer, &ctx(0));
    let (_, pending) = issue_at(&mut store, customer, &ctx(30));
    let (_, accepted) = issue_at(&mut store, customer, &ctx(60));
    force(&sim, &serie_of(&store, accepted_with_errors), 2001, true);
    let batch = store
        .open_verifactu_batch(VerifactuEnvironment::Simulador, &header(), T0, false)
        .unwrap()
        .unwrap();
    // Pendiente y en envío.
    assert_eq!(
        store
            .create_verifactu_subsanation(pending, &ctx(90))
            .unwrap_err()
            .code(),
        "conflict"
    );
    store.begin_verifactu_attempt(batch.id, T0).unwrap();
    let (_, body) = post(&sim.url(), &batch.envio.a_soap().unwrap()).unwrap();
    store
        .close_verifactu_batch(batch.id, &ResultadoEnvio::desde_xml(&body).unwrap(), T0)
        .unwrap();
    for id in [accepted_with_errors, accepted] {
        assert_eq!(
            store
                .create_verifactu_subsanation(id, &ctx(90))
                .unwrap_err()
                .code(),
            "conflict"
        );
    }
    assert_eq!(
        store
            .create_verifactu_subsanation(9999, &ctx(90))
            .unwrap_err()
            .code(),
        "not_found"
    );
    assert_eq!(store.verifactu_records().unwrap().len(), 3);
}

#[test]
fn an_alta_is_not_subsanated_when_the_invoice_already_has_an_anulacion() {
    let mut store = Store::open_in_memory().unwrap();
    let customer = setup(&mut store);
    let sim = simulator();
    let (doc, alta) = issue_at(&mut store, customer, &ctx(0));
    force(&sim, &serie_of(&store, alta), 1100, false);
    pump(&mut store, &sim, T0).closed();
    store.void_sales_invoice_registered(doc, &ctx(60)).unwrap();
    let err = store
        .create_verifactu_subsanation(alta, &ctx(120))
        .unwrap_err();
    assert_eq!(err.code(), "conflict");
    assert_eq!(store.verifactu_records().unwrap().len(), 2);
}

#[test]
fn an_anulacion_over_a_rejected_alta_carries_sin_registro_previo_and_is_accepted() {
    let mut store = Store::open_in_memory().unwrap();
    let customer = setup(&mut store);
    let sim = simulator();
    let (doc, alta) = issue_at(&mut store, customer, &ctx(0));
    force(&sim, &serie_of(&store, alta), 1100, false);
    pump(&mut store, &sim, T0).closed();
    sim.estado().config.errores.clear();
    store.void_sales_invoice_registered(doc, &ctx(60)).unwrap();
    let anulacion = last_record_id(&store);
    let RegistroFacturacion::Anulacion(a) =
        store.verifactu_records().unwrap().pop().unwrap().record
    else {
        panic!("se esperaba una anulación")
    };
    assert!(
        a.sin_registro_previo,
        "el alta rechazada no consta en la AEAT"
    );
    assert!(!a.rechazo_previo);
    let closure = pump(&mut store, &sim, T0 + 60_000).closed();
    assert_eq!(closure.accepted, 1);
    assert_eq!(state(&store, anulacion), VerifactuStatus::Accepted);
}

#[test]
fn a_rejected_anulacion_is_subsanated_with_rechazo_previo() {
    let mut store = Store::open_in_memory().unwrap();
    let customer = setup(&mut store);
    let sim = simulator();
    let (doc, _) = issue_at(&mut store, customer, &ctx(0));
    pump(&mut store, &sim, T0).closed();
    store.void_sales_invoice_registered(doc, &ctx(60)).unwrap();
    let anulacion = last_record_id(&store);
    force(&sim, &serie_of(&store, anulacion), 1100, false);
    pump(&mut store, &sim, T0 + 60_000).closed();
    assert_eq!(state(&store, anulacion), VerifactuStatus::Rejected);

    let sub = store
        .create_verifactu_subsanation(anulacion, &ctx(120))
        .unwrap();
    assert!(sub.created);
    assert!(
        !store
            .create_verifactu_subsanation(anulacion, &ctx(150))
            .unwrap()
            .created
    );
    let RegistroFacturacion::Anulacion(a) =
        store.verifactu_records().unwrap().pop().unwrap().record
    else {
        panic!("se esperaba una anulación")
    };
    assert!(a.rechazo_previo);
    assert!(!a.sin_registro_previo, "el alta aceptada consta");
    sim.estado().config.errores.clear();
    // En el simulador la baja rechazada no llegó a registrarse: se acepta la nueva.
    let closure = pump(&mut store, &sim, T0 + 120_000).closed();
    assert_eq!(closure.accepted, 1);
    assert_eq!(state(&store, sub.record_id), VerifactuStatus::Accepted);
}

// --- Simulador sembrado ---

#[test]
fn a_simulator_seeded_with_the_accepted_records_does_not_reject_an_anulacion_after_reopening() {
    let mut store = Store::open_in_memory().unwrap();
    let customer = setup(&mut store);
    let (doc, alta) = issue_at(&mut store, customer, &ctx(0));
    // Primera sesión: el alta se acepta.
    pump(&mut store, &simulator(), T0).closed();
    assert_eq!(state(&store, alta), VerifactuStatus::Accepted);
    store.void_sales_invoice_registered(doc, &ctx(60)).unwrap();
    let anulacion = last_record_id(&store);

    // «Reabrir»: un simulador nuevo sin memoria rechaza la baja con 3002.
    let forgetful = simulator();
    let closure = pump(&mut store, &forgetful, T0 + 60_000).closed();
    assert_eq!(closure.rejected, 1);
    assert_eq!(
        store.verifactu_record_state(anulacion).unwrap().aeat_code,
        Some(3002)
    );

    // Con el simulador sembrado con lo aceptado guardado, la baja se acepta.
    let mut store = Store::open_in_memory().unwrap();
    let customer = setup(&mut store);
    let (doc, _) = issue_at(&mut store, customer, &ctx(0));
    pump(&mut store, &simulator(), T0).closed();
    store.void_sales_invoice_registered(doc, &ctx(60)).unwrap();
    let anulacion = last_record_id(&store);
    let seeded = simulator();
    seeded.sembrar(
        &store
            .verifactu_accepted_records(VerifactuEnvironment::Simulador)
            .unwrap(),
    );
    let closure = pump(&mut store, &seeded, T0 + 60_000).closed();
    assert_eq!(closure.accepted, 1);
    assert_eq!(state(&store, anulacion), VerifactuStatus::Accepted);
}

// --- Revisión de E5: respuestas parciales, emisores y siembra con errores ---

const FAULT_XML: &str = "<?xml version=\"1.0\"?><env:Envelope xmlns:env=\"http://schemas.xmlsoap.org/soap/envelope/\"><env:Body><env:Fault><faultcode>env:Client</faultcode><faultstring>Codigo[4104].Error en la cabecera</faultstring></env:Fault></env:Body></env:Envelope>";

/// Respuesta parcial con una sola línea (la del registro dado, aceptada).
fn partial_answer(record: &RegistroFacturacion, operation: &str) -> ResultadoEnvio {
    let id = record.id_factura();
    let xml = format!(
        "<env:Envelope xmlns:env=\"http://schemas.xmlsoap.org/soap/envelope/\"><env:Body><tikR:RespuestaRegFactuSistemaFacturacion xmlns:tikR=\"urn:r\" xmlns:tik=\"urn:t\"><tikR:TiempoEsperaEnvio>60</tikR:TiempoEsperaEnvio><tikR:EstadoEnvio>ParcialmenteCorrecto</tikR:EstadoEnvio><tikR:RespuestaLinea><tikR:IDFactura><tik:IDEmisorFactura>{}</tik:IDEmisorFactura><tik:NumSerieFactura>{}</tik:NumSerieFactura><tik:FechaExpedicionFactura>{}</tik:FechaExpedicionFactura></tikR:IDFactura><tikR:Operacion><tik:TipoOperacion>{operation}</tik:TipoOperacion></tikR:Operacion><tikR:EstadoRegistro>Correcto</tikR:EstadoRegistro></tikR:RespuestaLinea></tikR:RespuestaRegFactuSistemaFacturacion></env:Body></env:Envelope>",
        id.nif_emisor, id.num_serie, id.fecha_expedicion
    );
    ResultadoEnvio::desde_xml(&xml).unwrap()
}

fn record_of(store: &Store, record_id: i64) -> RegistroFacturacion {
    store
        .verifactu_records()
        .unwrap()
        .into_iter()
        .find(|r| r.id == record_id)
        .unwrap()
        .record
}

/// Abre un lote, anota el intento y lo cierra con la respuesta dada.
fn close_with(store: &mut Store, result: &ResultadoEnvio, now: i64) -> BatchClosure {
    let batch = store
        .open_verifactu_batch(VerifactuEnvironment::Simulador, &header(), now, true)
        .unwrap()
        .unwrap();
    store.begin_verifactu_attempt(batch.id, now).unwrap();
    store.close_verifactu_batch(batch.id, result, now).unwrap()
}

#[test]
fn a_record_missing_from_a_partial_response_is_our_own_duplicate_in_the_next_batch() {
    let mut store = Store::open_in_memory().unwrap();
    let customer = setup(&mut store);
    let sim = simulator();
    let a = issue_at(&mut store, customer, &ctx(0)).1;
    let b = issue_at(&mut store, customer, &ctx(30)).1;
    let answer = partial_answer(&record_of(&store, a), "Alta");
    let closure = close_with(&mut store, &answer, T0);
    assert_eq!(
        (closure.accepted, closure.missing_record_ids.clone()),
        (1, vec![b])
    );
    // La AEAT sí tenía b aunque la línea no llegara: el 3000 del lote siguiente es suyo.
    sim.sembrar(&[record_of(&store, b)]);
    let closure = pump(&mut store, &sim, T0 + 60_000).closed();
    assert_eq!((closure.accepted, closure.unexpected_duplicates), (1, 0));
    let s = store.verifactu_record_state(b).unwrap();
    assert_eq!(s.status, VerifactuStatus::Accepted);
    assert!(s.duplicate);
    assert_eq!(s.aeat_code, Some(3000));
}

#[test]
fn a_void_missing_from_a_partial_response_answered_3001_in_the_next_batch_is_accepted() {
    let mut store = Store::open_in_memory().unwrap();
    let customer = setup(&mut store);
    let sim = simulator();
    let (doc, _) = issue_at(&mut store, customer, &ctx(0));
    pump(&mut store, &sim, T0).closed();
    store.void_sales_invoice_registered(doc, &ctx(60)).unwrap();
    let void = last_record_id(&store);
    let other = issue_at(&mut store, customer, &ctx(90)).1;
    let answer = partial_answer(&record_of(&store, other), "Alta");
    let closure = close_with(&mut store, &answer, T0 + 60_000);
    assert_eq!(closure.missing_record_ids, vec![void]);
    sim.sembrar(&[record_of(&store, void)]);
    let closure = pump(&mut store, &sim, T0 + 120_000).closed();
    assert_eq!((closure.accepted, closure.rejected), (1, 0));
    let s = store.verifactu_record_state(void).unwrap();
    assert_eq!(s.status, VerifactuStatus::Accepted);
    assert!(s.duplicate);
    assert_eq!(s.aeat_code, Some(3001));
}

#[test]
fn a_record_that_was_only_in_a_fault_batch_is_not_our_duplicate() {
    let mut store = Store::open_in_memory().unwrap();
    let customer = setup(&mut store);
    let sim = simulator();
    let a = issue_at(&mut store, customer, &ctx(0)).1;
    let fault = ResultadoEnvio::desde_xml(FAULT_XML).unwrap();
    close_with(&mut store, &fault, T0);
    // El Fault rechaza el mensaje entero: la AEAT no guardó nada, así que un 3000 es ajeno.
    sim.sembrar(&[record_of(&store, a)]);
    let closure = pump(&mut store, &sim, T0 + 60_000).closed();
    assert_eq!((closure.accepted, closure.unexpected_duplicates), (0, 1));
    assert_eq!(state(&store, a), VerifactuStatus::UnexpectedDuplicate);
}

#[test]
fn a_batch_is_cut_when_the_issuer_changes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nestadmin.db");
    let mut store = Store::open(&path).unwrap();
    let customer = setup(&mut store);
    let a = issue_at(&mut store, customer, &ctx(0)).1;
    let b = issue_at(&mut store, customer, &ctx(30)).1;
    let c = issue_at(&mut store, customer, &ctx(60)).1;
    // El segundo registro pasa a ser de otro emisor (el trigger solo se quita en la prueba).
    let conn = raw(&path);
    conn.execute_batch("DROP TRIGGER verifactu_records_no_update;")
        .unwrap();
    conn.execute(
        "UPDATE verifactu_records SET issuer_tax_id = 'B00000034' WHERE id = ?1",
        [b],
    )
    .unwrap();
    drop(conn);
    let first = store
        .open_verifactu_batch(VerifactuEnvironment::Simulador, &header(), T0, false)
        .unwrap()
        .unwrap();
    assert_eq!(
        first.record_ids,
        vec![a],
        "el lote se corta ante el otro emisor"
    );
    store.begin_verifactu_attempt(first.id, T0).unwrap();
    let sim = simulator();
    let result =
        ResultadoEnvio::desde_xml(&post(&sim.url(), &first.envio.a_soap().unwrap()).unwrap().1)
            .unwrap();
    store.close_verifactu_batch(first.id, &result, T0).unwrap();
    let second = store
        .open_verifactu_batch(
            VerifactuEnvironment::Simulador,
            &header(),
            T0 + 60_000,
            false,
        )
        .unwrap()
        .unwrap();
    assert_eq!(
        second.record_ids,
        vec![b],
        "el siguiente lote es del otro emisor"
    );
    store
        .begin_verifactu_attempt(second.id, T0 + 60_000)
        .unwrap();
    let _ = c;
}

#[test]
fn the_accepted_seed_keeps_accepted_with_errors_and_the_simulator_stores_it() {
    let mut store = Store::open_in_memory().unwrap();
    let customer = setup(&mut store);
    let sim = simulator();
    let a = issue_at(&mut store, customer, &ctx(0)).1;
    let b = issue_at(&mut store, customer, &ctx(30)).1;
    force(&sim, &serie_of(&store, a), 2000, true);
    let closure = pump(&mut store, &sim, T0).closed();
    assert_eq!((closure.accepted, closure.accepted_with_errors), (1, 1));
    let seed = store
        .verifactu_accepted_seed(VerifactuEnvironment::Simulador)
        .unwrap();
    let flags: Vec<bool> = seed.iter().map(|(_, with_errors)| *with_errors).collect();
    assert_eq!(flags, vec![true, false], "{a} con errores, {b} correcto");
    let fresh = simulator();
    fresh.sembrar_con_estado(&seed);
    let state = fresh.estado();
    let stored = |record: &RegistroFacturacion| {
        let id = record.id_factura();
        state
            .registros
            .get(&(
                id.nif_emisor.clone(),
                id.num_serie.clone(),
                id.fecha_expedicion.to_string(),
            ))
            .unwrap()
            .estado
            .clone()
    };
    assert_eq!(stored(&seed[0].0), "AceptadaConErrores");
    assert_eq!(stored(&seed[1].0), "Correcta");
}

#[test]
fn a_batch_summary_is_available_by_id_for_open_and_closed_batches() {
    let mut store = Store::open_in_memory().unwrap();
    let customer = setup(&mut store);
    issue_at(&mut store, customer, &ctx(0));
    assert!(store.verifactu_batch_summary(1).unwrap().is_none());
    let fault = ResultadoEnvio::desde_xml(FAULT_XML).unwrap();
    let batch = store
        .open_verifactu_batch(VerifactuEnvironment::Simulador, &header(), T0, true)
        .unwrap()
        .unwrap();
    let open = store.verifactu_batch_summary(batch.id).unwrap().unwrap();
    assert!(open.result.is_none());
    store.begin_verifactu_attempt(batch.id, T0).unwrap();
    store.close_verifactu_batch(batch.id, &fault, T0).unwrap();
    let closed = store.verifactu_batch_summary(batch.id).unwrap().unwrap();
    assert_eq!(closed.result.unwrap().outcome, BatchOutcome::Fault);
    assert_eq!(closed.environment, VerifactuEnvironment::Simulador);
}
