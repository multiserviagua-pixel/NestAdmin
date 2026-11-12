//! Envío de la remisión Verifactu con certificado (T18b-D1, D278–D283, migración 17):
//! configuración por base, intentos sin envío que no cuentan (D50, D252, D280), clasificación de
//! los `Fault` (D281), espera creciente, incidencias y `Incidencia = S` (D282), paro por
//! autorización y tablas de solo inserción. Independiente de `verifactu_remission_store.rs`
//! (D197). Todo con datos sintéticos y con el simulador local en 127.0.0.1; nada contacta con
//! la AEAT.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::time::Duration;

use nestadmin_core::sales::{
    retry_wait_seconds, BatchOutcome, Company, CustomerData, DraftInput, DraftLine, FaultClass,
    SenderCapacity, VerifactuBatch, VerifactuContext, VerifactuEnvironment, VerifactuStatus,
    WaitMode, FAST_RETRIES, MAX_AUTOMATIC_FAULTS, MAX_RETRY_WAIT_SECONDS,
};
use nestadmin_core::Store;
use nestadmin_documents::IsoDate;
use nestadmin_sales::{DocumentKind, LineInput, TaxRate};
use nestadmin_verifactu::simulador::{ConfigSimulador, SimuladorAeat};
use nestadmin_verifactu::{
    Cabecera, FalloSoap, Fecha, FechaHoraHuso, Persona, PersonaEs, ResultadoEnvio,
    SistemaInformatico,
};
use rust_decimal::Decimal;

const SIM: VerifactuEnvironment = VerifactuEnvironment::Simulador;

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

/// Milisegundos Unix del reloj `offset` segundos después de la base (el de los registros).
fn ms(offset: i64) -> i64 {
    (base_unix() + offset) * 1000
}

fn ctx(offset: i64) -> VerifactuContext {
    VerifactuContext {
        system: system(),
        now: FechaHoraHuso::desde_unix(base_unix() + offset, 60).unwrap(),
        environment: SIM,
    }
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

/// Crea y emite una factura; devuelve (documento, id del registro de alta).
fn issue_at(store: &mut Store, customer: i64, ctx: &VerifactuContext) -> (i64, i64) {
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
    let id = store.create_sales_draft(&input).unwrap();
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
    let (head, body) = resp.split_once("\r\n\r\n")?;
    let status = head.split_whitespace().nth(1)?.parse().ok()?;
    Some((status, body.to_owned()))
}

fn simulator() -> SimuladorAeat {
    SimuladorAeat::iniciar(ConfigSimulador::default()).unwrap()
}

fn fault(code: u32) -> ResultadoEnvio {
    ResultadoEnvio::Fallo(FalloSoap {
        faultcode: "env:Client".into(),
        faultstring: format!("Codigo[{code}].Fallo simulado"),
        codigo: Some(code),
    })
}

fn open_batch(store: &mut Store, now: i64) -> VerifactuBatch {
    store
        .open_verifactu_batch(SIM, &header(), now, true)
        .unwrap()
        .expect("había pendientes")
}

/// Envía el lote al simulador (el intento ya está anotado) y lo cierra con la respuesta.
fn respond(store: &mut Store, sim: &SimuladorAeat, batch: &VerifactuBatch, now: i64) {
    let (status, body) = post(&sim.url(), &batch.envio.a_soap().unwrap()).expect("respuesta");
    assert!(status == 200 || status == 500, "HTTP {status}");
    let result = ResultadoEnvio::desde_xml(&body).unwrap();
    store.close_verifactu_batch(batch.id, &result, now).unwrap();
}

// --- Tablas de solo inserción ---

#[test]
fn the_new_tables_are_insert_only() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nestadmin.db");
    let mut store = Store::open(&path).unwrap();
    let customer = setup(&mut store);
    issue_at(&mut store, customer, &ctx(0));
    store
        .set_verifactu_remission_settings(
            Some(r#"{"kind":"pfx","reference":"00112233445566778899aabbccddeeff"}"#),
            SenderCapacity::Collaborator,
            None,
            "usuario_configuracion",
            ms(1),
        )
        .unwrap();
    store
        .remember_verifactu_tls12(&"ab".repeat(32), ms(2))
        .unwrap();
    let batch = open_batch(&mut store, ms(3));
    store
        .note_verifactu_unsent_attempt(batch.id, "connect_failed", ms(4))
        .unwrap();
    store.begin_verifactu_attempt(batch.id, ms(5)).unwrap();
    store
        .close_verifactu_batch(batch.id, &fault(4112), ms(6))
        .unwrap();

    for table in [
        "verifactu_remission_settings",
        "verifactu_tls12_preferences",
        "verifactu_unsent_attempts",
        "verifactu_incidents",
        "verifactu_authorization_halts",
    ] {
        assert!(count(&path, table) >= 1, "{table} sin filas");
        let conn = raw(&path);
        let update = match table {
            "verifactu_tls12_preferences" => {
                "UPDATE verifactu_tls12_preferences SET learned_at = 0"
            }
            "verifactu_remission_settings" => {
                "UPDATE verifactu_remission_settings SET changed_at = 0"
            }
            "verifactu_unsent_attempts" => "UPDATE verifactu_unsent_attempts SET attempted_at = 0",
            "verifactu_incidents" => "UPDATE verifactu_incidents SET at = 0",
            _ => "UPDATE verifactu_authorization_halts SET at = 0",
        };
        assert!(conn.execute(update, []).is_err(), "UPDATE de {table}");
        assert!(
            conn.execute(&format!("DELETE FROM {table}"), []).is_err(),
            "DELETE de {table}"
        );
    }
}

// --- Configuración (D278, D283) ---

#[test]
fn the_settings_are_an_insert_only_history_and_the_representative_is_never_deduced() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nestadmin.db");
    let mut store = Store::open(&path).unwrap();

    let unset = store.verifactu_remission_settings().unwrap();
    assert_eq!(unset.id, 0);
    assert_eq!(unset.sender_capacity, SenderCapacity::Taxpayer);
    assert!(unset.certificate_source.is_none() && unset.representative.is_none());

    let source = r#"{"kind":"store","thumbprint_sha256":"ab12"}"#;
    let representative = PersonaEs::new("Gestoría Demo SL", "B00000034");
    assert!(store
        .set_verifactu_remission_settings(
            Some(source),
            SenderCapacity::Collaborator,
            Some(&representative),
            "usuario_configuracion",
            ms(1),
        )
        .unwrap());
    let current = store.verifactu_remission_settings().unwrap();
    assert_eq!(current.certificate_source.as_deref(), Some(source));
    assert_eq!(current.sender_capacity, SenderCapacity::Collaborator);
    assert_eq!(current.representative.as_ref(), Some(&representative));

    // La misma configuración no escribe nada.
    assert!(!store
        .set_verifactu_remission_settings(
            Some(source),
            SenderCapacity::Collaborator,
            Some(&representative),
            "usuario_configuracion",
            ms(2),
        )
        .unwrap());
    assert_eq!(count(&path, "verifactu_remission_settings"), 1);

    // Quitar el representante es un cambio expreso: una fila nueva, la anterior se conserva.
    assert!(store
        .set_verifactu_remission_settings(
            Some(source),
            SenderCapacity::Taxpayer,
            None,
            "usuario_configuracion",
            ms(3),
        )
        .unwrap());
    assert_eq!(count(&path, "verifactu_remission_settings"), 2);
    assert!(store
        .verifactu_remission_settings()
        .unwrap()
        .representative
        .is_none());

    // Validación: NIF del representante, JSON del certificado y autorización.
    let bad_nif = PersonaEs::new("Gestoría Demo SL", "123");
    for (cert, rep, auth) in [
        (Some(source), Some(&bad_nif), "usuario_configuracion"),
        (Some("no es json"), None, "usuario_configuracion"),
        (Some("[1]"), None, "usuario_configuracion"),
        (Some(source), None, "  "),
    ] {
        let error = store
            .set_verifactu_remission_settings(cert, SenderCapacity::Taxpayer, rep, auth, ms(4))
            .unwrap_err();
        assert_eq!(error.code(), "invalid_input", "{cert:?} {rep:?} {auth:?}");
    }
    assert_eq!(count(&path, "verifactu_remission_settings"), 2);

    // El historial deja la calidad y si hay certificado y representante, nunca sus valores.
    let detail: String = raw(&path)
        .query_row(
            "SELECT group_concat(detail) FROM audit_log WHERE action = 'verifactu.remission_settings'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(detail.contains("collaborator"), "{detail}");
    for secret in ["B00000034", "Gestoría", "ab12"] {
        assert!(!detail.contains(secret), "{secret} en el historial");
    }
}

#[test]
fn the_tls12_preference_is_remembered_by_fingerprint_without_touching_the_settings() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nestadmin.db");
    let mut store = Store::open(&path).unwrap();
    let fingerprint = "AB".repeat(32);
    assert!(!store.verifactu_tls12_preferred(&fingerprint).unwrap());
    assert!(store.remember_verifactu_tls12(&fingerprint, ms(1)).unwrap());
    assert!(!store.remember_verifactu_tls12(&fingerprint, ms(2)).unwrap());
    assert!(store.verifactu_tls12_preferred(&fingerprint).unwrap());
    assert!(store
        .verifactu_tls12_preferred(&fingerprint.to_lowercase())
        .unwrap());
    assert!(!store.verifactu_tls12_preferred(&"cd".repeat(32)).unwrap());
    assert_eq!(count(&path, "verifactu_tls12_preferences"), 1);
    assert_eq!(count(&path, "verifactu_remission_settings"), 0);
    assert_eq!(
        store
            .remember_verifactu_tls12("corta", ms(3))
            .unwrap_err()
            .code(),
        "invalid_input"
    );
}

// --- Intentos sin envío (D280) ---

#[test]
fn an_unsent_attempt_does_not_count_and_a_foreign_3000_is_not_taken_as_ours() {
    let mut store = Store::open_in_memory().unwrap();
    let customer = setup(&mut store);
    let (_, alta) = issue_at(&mut store, customer, &ctx(0));
    let sim = simulator();
    // Otro remitente ya envió este mismo registro a la AEAT.
    let record = store
        .verifactu_records()
        .unwrap()
        .into_iter()
        .find(|r| r.id == alta)
        .unwrap()
        .record;
    sim.sembrar(&[record]);

    let batch = open_batch(&mut store, ms(10));
    // Primero falla el certificado, antes de anotar el intento: no hay intento.
    store
        .note_verifactu_unsent_attempt(batch.id, "certificate_needs_user", ms(11))
        .unwrap();
    let summary = store.verifactu_batch_summary(batch.id).unwrap().unwrap();
    assert_eq!((summary.attempts, summary.unsent_attempts), (0, 1));
    let last = store.verifactu_last_unsent_attempt().unwrap().unwrap();
    assert_eq!(last.reason_code, "certificate_needs_user");
    assert!(!last.voided_attempt);

    // Después el intento se anota (D252), pero el saludo TLS falla antes de escribir el cuerpo.
    let first = store
        .begin_verifactu_attempt_ticket(batch.id, ms(20))
        .unwrap();
    assert_eq!(first.counted, 1);
    store
        .void_verifactu_attempt(first.attempt_id, "tls_handshake_failed", ms(21))
        .unwrap();
    let summary = store.verifactu_batch_summary(batch.id).unwrap().unwrap();
    assert_eq!((summary.attempts, summary.unsent_attempts), (0, 2));
    assert!(
        store
            .verifactu_last_unsent_attempt()
            .unwrap()
            .unwrap()
            .voided_attempt
    );
    // Anular dos veces el mismo intento no repite nada.
    assert_eq!(
        store
            .void_verifactu_attempt(first.attempt_id, "tls_handshake_failed", ms(22))
            .unwrap_err()
            .code(),
        "conflict"
    );

    // El primer intento real: la AEAT ya tenía el registro (de otro remitente) y responde 3000.
    let second = store
        .begin_verifactu_attempt_ticket(batch.id, ms(30))
        .unwrap();
    assert_eq!(second.counted, 1, "los anulados no cuentan");
    respond(&mut store, &sim, &batch, ms(31));
    assert_eq!(
        state(&store, alta),
        VerifactuStatus::UnexpectedDuplicate,
        "un duplicado ajeno no se da por aceptado (D50)"
    );
}

#[test]
fn a_real_attempt_without_response_does_count_so_the_own_duplicate_is_accepted() {
    let mut store = Store::open_in_memory().unwrap();
    let customer = setup(&mut store);
    let (_, alta) = issue_at(&mut store, customer, &ctx(0));
    let sim = simulator();
    let batch = open_batch(&mut store, ms(10));
    // El cuerpo llega, la AEAT lo guarda y la respuesta se pierde.
    store.begin_verifactu_attempt(batch.id, ms(20)).unwrap();
    sim.estado().perder_siguiente_respuesta = true;
    assert!(post(&sim.url(), &batch.envio.a_soap().unwrap()).is_none());
    // Un intento sin envío entre medias no cambia la cuenta.
    store
        .note_verifactu_unsent_attempt(batch.id, "connect_failed", ms(40))
        .unwrap();
    let second = store
        .begin_verifactu_attempt_ticket(batch.id, ms(50))
        .unwrap();
    assert_eq!(second.counted, 2);
    respond(&mut store, &sim, &batch, ms(51));
    let view = store.verifactu_record_state(alta).unwrap();
    assert_eq!(view.status, VerifactuStatus::Accepted);
    assert!(view.duplicate);
}

#[test]
fn an_attempt_cannot_be_voided_after_the_batch_closed_and_reasons_are_stable_codes() {
    let mut store = Store::open_in_memory().unwrap();
    let customer = setup(&mut store);
    issue_at(&mut store, customer, &ctx(0));
    let sim = simulator();
    let batch = open_batch(&mut store, ms(10));
    let ticket = store
        .begin_verifactu_attempt_ticket(batch.id, ms(20))
        .unwrap();
    for bad in ["", "Texto con espacios", "NIF B12345674", &"a".repeat(65)] {
        assert_eq!(
            store
                .void_verifactu_attempt(ticket.attempt_id, bad, ms(21))
                .unwrap_err()
                .code(),
            "invalid_input",
            "{bad:?}"
        );
        assert_eq!(
            store
                .note_verifactu_unsent_attempt(batch.id, bad, ms(21))
                .unwrap_err()
                .code(),
            "invalid_input"
        );
    }
    respond(&mut store, &sim, &batch, ms(30));
    assert_eq!(
        store
            .void_verifactu_attempt(ticket.attempt_id, "connect_failed", ms(31))
            .unwrap_err()
            .code(),
        "conflict"
    );
    assert_eq!(
        store
            .note_verifactu_unsent_attempt(batch.id, "connect_failed", ms(31))
            .unwrap_err()
            .code(),
        "conflict"
    );
}

// --- Clasificación de los Fault (D281) ---

#[test]
fn an_authorization_fault_halts_the_remission_until_the_user_declares_or_changes_the_settings() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nestadmin.db");
    let mut store = Store::open(&path).unwrap();
    let customer = setup(&mut store);
    let (_, alta) = issue_at(&mut store, customer, &ctx(0));
    assert!(store.verifactu_authorization_halt().unwrap().is_none());

    let batch = open_batch(&mut store, ms(10));
    store.begin_verifactu_attempt(batch.id, ms(11)).unwrap();
    let closure = store
        .close_verifactu_batch(batch.id, &fault(4112), ms(12))
        .unwrap();
    assert_eq!(closure.outcome, BatchOutcome::Fault);
    let halt = store.verifactu_authorization_halt().unwrap().unwrap();
    assert_eq!((halt.error_code, halt.batch_id), (4112, batch.id));
    assert_eq!(state(&store, alta), VerifactuStatus::Pending);
    // No es un `Fault` de esquema: no cuenta para el paro de tres, y no abre una incidencia.
    assert_eq!(store.verifactu_consecutive_faults(SIM).unwrap(), 0);
    assert!(!store.verifactu_incident_open().unwrap());

    // «Remitir ahora» declarando que ya lo arregló.
    assert!(store
        .resume_verifactu_authorization("usuario_declara_autorizacion", ms(20))
        .unwrap());
    assert!(store.verifactu_authorization_halt().unwrap().is_none());
    assert!(!store
        .resume_verifactu_authorization("usuario_declara_autorizacion", ms(21))
        .unwrap());
    assert_eq!(
        store
            .resume_verifactu_authorization(" ", ms(22))
            .unwrap_err()
            .code(),
        "invalid_input"
    );

    // Otro `Fault` de autorización (el representante no es válido): vuelve a parar, y cambiar
    // la configuración de remisión la reanuda.
    let batch = open_batch(&mut store, ms(30));
    store.begin_verifactu_attempt(batch.id, ms(31)).unwrap();
    store
        .close_verifactu_batch(batch.id, &fault(4124), ms(32))
        .unwrap();
    assert_eq!(
        store
            .verifactu_authorization_halt()
            .unwrap()
            .unwrap()
            .error_code,
        4124
    );
    assert!(store
        .set_verifactu_remission_settings(
            None,
            SenderCapacity::Attorney,
            None,
            "usuario_configuracion",
            ms(40),
        )
        .unwrap());
    assert!(store.verifactu_authorization_halt().unwrap().is_none());
    // Una configuración que no cambia nada no reanuda nada.
    let batch = open_batch(&mut store, ms(50));
    store.begin_verifactu_attempt(batch.id, ms(51)).unwrap();
    store
        .close_verifactu_batch(batch.id, &fault(4105), ms(52))
        .unwrap();
    assert!(!store
        .set_verifactu_remission_settings(
            None,
            SenderCapacity::Attorney,
            None,
            "usuario_configuracion",
            ms(60),
        )
        .unwrap());
    assert!(store.verifactu_authorization_halt().unwrap().is_some());
    // El historial deja las tres cosas: paro, declaración y cambio de configuración.
    let conn = raw(&path);
    let mut stmt = conn
        .prepare("SELECT action, detail FROM audit_log WHERE action LIKE 'verifactu.authorization_%' ORDER BY id")
        .unwrap();
    let rows: Vec<(String, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    let actions: Vec<&str> = rows.iter().map(|(a, _)| a.as_str()).collect();
    assert_eq!(
        actions,
        [
            "verifactu.authorization_halt",
            "verifactu.authorization_resume",
            "verifactu.authorization_halt",
            "verifactu.authorization_resume",
            "verifactu.authorization_halt",
        ]
    );
    assert!(rows[1].1.contains("declaration") && rows[3].1.contains("settings_change"));
}

#[test]
fn a_technical_fault_never_halts_and_opens_an_incident() {
    let mut store = Store::open_in_memory().unwrap();
    let customer = setup(&mut store);
    let (_, alta) = issue_at(&mut store, customer, &ctx(0));
    for (n, code) in [4108u32, 4110, 4134, 4141, 4108, 4110, 4134, 4141]
        .into_iter()
        .enumerate()
    {
        let now = ms(10 + n as i64 * 5000);
        // Nunca se abre «paro»: ni siquiera de forma automática tras más de tres seguidos.
        let batch = store
            .open_verifactu_batch(SIM, &header(), now, false)
            .unwrap()
            .expect("sigue habiendo pendientes");
        store.begin_verifactu_attempt(batch.id, now).unwrap();
        store
            .close_verifactu_batch(batch.id, &fault(code), now)
            .unwrap();
        assert_eq!(state(&store, alta), VerifactuStatus::Pending);
        assert_eq!(store.verifactu_consecutive_faults(SIM).unwrap(), 0);
        assert!(store.verifactu_authorization_halt().unwrap().is_none());
        assert!(store.verifactu_incident_open().unwrap());
    }
    assert_eq!(store.verifactu_incidents().unwrap().len(), 1);
}

#[test]
fn schema_faults_still_stop_after_three_and_technical_ones_neither_count_nor_reset() {
    let mut store = Store::open_in_memory().unwrap();
    let customer = setup(&mut store);
    issue_at(&mut store, customer, &ctx(0));
    let now = std::cell::Cell::new(ms(10));
    let close_with = |store: &mut Store, code: u32| {
        now.set(now.get() + 5000);
        let now = now.get();
        let batch = store
            .open_verifactu_batch(SIM, &header(), now, true)
            .unwrap()
            .unwrap();
        store.begin_verifactu_attempt(batch.id, now).unwrap();
        store
            .close_verifactu_batch(batch.id, &fault(code), now)
            .unwrap();
        store.verifactu_consecutive_faults(SIM).unwrap()
    };
    assert_eq!(close_with(&mut store, 4102), 1);
    assert_eq!(
        close_with(&mut store, 4108),
        1,
        "el técnico no cuenta ni corta"
    );
    assert_eq!(close_with(&mut store, 4104), 2);
    assert_eq!(close_with(&mut store, 4116), MAX_AUTOMATIC_FAULTS);
    assert_eq!(
        store
            .open_verifactu_batch(SIM, &header(), now.get() + 1000, false)
            .unwrap_err()
            .code(),
        "verifactu_batch_rejected"
    );
    // Uno de autorización reinicia la racha de esquema (su paro es otro).
    assert_eq!(close_with(&mut store, 4112), 0);
    assert_eq!(
        FaultClass::of(Some(4112)),
        FaultClass::Authorization,
        "la tabla de clases es la de D281"
    );
}

// --- Espera creciente (D281, art. 16.4) ---

#[test]
fn the_wait_after_failures_grows_up_to_one_hour_and_each_asker_sees_its_own() {
    let mut store = Store::open_in_memory().unwrap();
    let customer = setup(&mut store);
    issue_at(&mut store, customer, &ctx(0));
    let batch = open_batch(&mut store, ms(10));
    assert_eq!(store.verifactu_next_send_at(SIM).unwrap(), None);

    // Fallos reales sin respuesta: 60 s los primeros y después el doble, hasta una hora.
    let mut now = ms(100);
    let mut last_attempt = now;
    for n in 1..=14u32 {
        last_attempt = now;
        store.begin_verifactu_attempt(batch.id, now).unwrap();
        let expected = now + i64::from(retry_wait_seconds(n)) * 1000;
        assert_eq!(
            store.verifactu_next_send_at(SIM).unwrap(),
            Some(expected),
            "{n}"
        );
        assert_eq!(
            store
                .verifactu_failure_streak(SIM, WaitMode::Automatic)
                .unwrap(),
            n
        );
        now = expected;
    }
    assert_eq!(retry_wait_seconds(FAST_RETRIES), 60);
    assert_eq!(retry_wait_seconds(FAST_RETRIES + 1), 120);
    let last = store.verifactu_next_send_at(SIM).unwrap().unwrap();
    assert_eq!(
        last,
        last_attempt + i64::from(MAX_RETRY_WAIT_SECONDS) * 1000
    );

    // «Remitir ahora» sigue respetando los fallos que sí llegaron a la AEAT; el arranque no.
    assert_eq!(
        store
            .verifactu_next_send_at_mode(SIM, WaitMode::Manual)
            .unwrap(),
        Some(last)
    );
    assert_eq!(
        store
            .verifactu_next_send_at_mode(SIM, WaitMode::Startup)
            .unwrap(),
        None
    );
}

#[test]
fn unsent_attempts_slow_the_automatic_remission_but_never_manual_or_startup() {
    let mut store = Store::open_in_memory().unwrap();
    let customer = setup(&mut store);
    issue_at(&mut store, customer, &ctx(0));
    let batch = open_batch(&mut store, ms(10));
    store
        .note_verifactu_unsent_attempt(batch.id, "certificate_needs_user", ms(100))
        .unwrap();
    assert_eq!(
        store.verifactu_next_send_at(SIM).unwrap(),
        Some(ms(100) + 60_000)
    );
    // El usuario acaba de arreglar su certificado: nada llegó a la AEAT, no hay nada que esperar.
    assert_eq!(
        store
            .verifactu_next_send_at_mode(SIM, WaitMode::Manual)
            .unwrap(),
        None
    );
    assert_eq!(
        store
            .verifactu_next_send_at_mode(SIM, WaitMode::Startup)
            .unwrap(),
        None
    );
    for n in 0..8 {
        store
            .note_verifactu_unsent_attempt(batch.id, "certificate_needs_user", ms(200 + n))
            .unwrap();
    }
    // Nueve fallos sin envío seguidos: ya crece, y con tope de una hora.
    let at = store.verifactu_next_send_at(SIM).unwrap().unwrap();
    assert_eq!(at, ms(207) + i64::from(retry_wait_seconds(9)) * 1000);

    // Una respuesta de la AEAT reinicia la cuenta; su `TiempoEsperaEnvio` sigue mandando.
    let sim = simulator();
    store.begin_verifactu_attempt(batch.id, ms(300)).unwrap();
    respond(&mut store, &sim, &batch, ms(301));
    assert_eq!(
        store
            .verifactu_failure_streak(SIM, WaitMode::Automatic)
            .unwrap(),
        0
    );
    let wait = store.verifactu_next_send_at(SIM).unwrap().unwrap();
    assert!(wait > ms(301), "la espera de la AEAT se respeta");
    assert_eq!(
        store
            .verifactu_next_send_at_mode(SIM, WaitMode::Startup)
            .unwrap(),
        Some(wait),
        "el arranque respeta la espera fijada por la AEAT"
    );
}

// --- Incidencias (D282) ---

#[test]
fn incidencia_is_decided_when_opening_a_batch_and_an_open_batch_keeps_its_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nestadmin.db");
    let mut store = Store::open(&path).unwrap();
    let customer = setup(&mut store);
    let sim = simulator();

    // Lote 1: antes de cualquier incidencia, sin `Incidencia`.
    issue_at(&mut store, customer, &ctx(0));
    let first = open_batch(&mut store, ms(10));
    let first_bytes = first.envio.a_soap().unwrap();
    assert!(!first_bytes.contains("Incidencia"));
    store.begin_verifactu_attempt(first.id, ms(11)).unwrap();

    // Sin respuesta: se abre la incidencia (D282).
    assert!(store
        .open_verifactu_incident(Some(first.id), "read_timeout", ms(20))
        .unwrap());
    assert!(!store
        .open_verifactu_incident(Some(first.id), "read_timeout", ms(21))
        .unwrap());
    assert!(store.verifactu_incident_open().unwrap());

    // Una factura emitida durante la incidencia, y un reintento del lote abierto: mismos bytes,
    // aunque ya haya una incidencia (y aunque se reabra la base).
    let (_, during) = issue_at(&mut store, customer, &ctx(30));
    assert_eq!(
        store
            .unfinished_verifactu_batch()
            .unwrap()
            .unwrap()
            .envio
            .a_soap()
            .unwrap(),
        first_bytes
    );
    drop(store);
    let mut store = Store::open(&path).unwrap();
    let reloaded = store.unfinished_verifactu_batch().unwrap().unwrap();
    assert_eq!(reloaded.envio.a_soap().unwrap(), first_bytes);

    // El envío con respuesta cierra la incidencia.
    store.begin_verifactu_attempt(first.id, ms(100)).unwrap();
    respond(&mut store, &sim, &reloaded, ms(101));
    assert!(!store.verifactu_incident_open().unwrap());
    let incidents = store.verifactu_incidents().unwrap();
    assert_eq!(incidents.len(), 1);
    assert_eq!(
        (incidents[0].opened_at, incidents[0].closed_at),
        (ms(20), Some(ms(101)))
    );
    assert_eq!(incidents[0].reason_code, "read_timeout");

    // Lote 2: lleva un registro generado durante la incidencia, aunque esta ya se cerró.
    let second = open_batch(&mut store, ms(200));
    assert_eq!(second.record_ids, vec![during]);
    let incidencia = second
        .envio
        .cabecera
        .remision_voluntaria
        .as_ref()
        .and_then(|r| r.incidencia);
    assert_eq!(incidencia, Some(true));
    assert!(second.envio.a_soap().unwrap().contains("Incidencia>S<"));
    store.begin_verifactu_attempt(second.id, ms(201)).unwrap();
    respond(&mut store, &sim, &second, ms(202));

    // Lote 3: un registro posterior a la incidencia no la lleva.
    issue_at(&mut store, customer, &ctx(300));
    let third = open_batch(&mut store, ms(400));
    assert!(third.envio.cabecera.remision_voluntaria.is_none());
    assert!(!third.envio.a_soap().unwrap().contains("Incidencia"));
}

#[test]
fn a_record_generated_before_the_incident_does_not_mark_its_batch() {
    let mut store = Store::open_in_memory().unwrap();
    let customer = setup(&mut store);
    issue_at(&mut store, customer, &ctx(0));
    let first = open_batch(&mut store, ms(10));
    store.begin_verifactu_attempt(first.id, ms(11)).unwrap();
    // Un `Fault` técnico cierra el lote y abre la incidencia; el registro es anterior a ella.
    store
        .close_verifactu_batch(first.id, &fault(4108), ms(20))
        .unwrap();
    assert!(store.verifactu_incident_open().unwrap());
    let retry = open_batch(&mut store, ms(100));
    assert!(
        retry.envio.cabecera.remision_voluntaria.is_none(),
        "se generó antes de la incidencia"
    );
    // Uno generado durante la incidencia (abierta aún) sí marca el lote que lo lleve.
    store.begin_verifactu_attempt(retry.id, ms(101)).unwrap();
    store
        .close_verifactu_batch(retry.id, &fault(4108), ms(102))
        .unwrap();
    issue_at(&mut store, customer, &ctx(150));
    let marked = open_batch(&mut store, ms(200));
    assert_eq!(marked.record_ids.len(), 2);
    assert_eq!(
        marked
            .envio
            .cabecera
            .remision_voluntaria
            .as_ref()
            .and_then(|r| r.incidencia),
        Some(true)
    );
}

#[test]
fn only_network_and_tls_unsent_attempts_open_an_incident() {
    let mut store = Store::open_in_memory().unwrap();
    let customer = setup(&mut store);
    issue_at(&mut store, customer, &ctx(0));
    let batch = open_batch(&mut store, ms(10));
    // Configuración o compilación: no son una incidencia técnica (D282).
    for (n, reason) in [
        "network_disabled",
        "wrong_environment",
        "certificate_missing",
        "certificate_expired",
        "certificate_needs_user",
        "certificate_rejected",
        "settings_changed",
    ]
    .into_iter()
    .enumerate()
    {
        store
            .note_verifactu_unsent_attempt(batch.id, reason, ms(20 + n as i64))
            .unwrap();
        assert!(!store.verifactu_incident_open().unwrap(), "{reason}");
    }
    let ticket = store
        .begin_verifactu_attempt_ticket(batch.id, ms(40))
        .unwrap();
    store
        .void_verifactu_attempt(ticket.attempt_id, "certificate_key_unsupported", ms(41))
        .unwrap();
    assert!(!store.verifactu_incident_open().unwrap());
    // Red y TLS sí.
    store
        .note_verifactu_unsent_attempt(batch.id, "tls_handshake_failed", ms(50))
        .unwrap();
    assert!(store.verifactu_incident_open().unwrap());
    assert_eq!(
        store.verifactu_incidents().unwrap()[0].reason_code,
        "tls_handshake_failed"
    );
}

#[test]
fn the_unsent_table_refuses_a_closed_batch_and_an_attempt_of_another_batch() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nestadmin.db");
    let mut store = Store::open(&path).unwrap();
    let customer = setup(&mut store);
    issue_at(&mut store, customer, &ctx(0));
    let sim = simulator();
    let first = open_batch(&mut store, ms(10));
    let ticket = store
        .begin_verifactu_attempt_ticket(first.id, ms(11))
        .unwrap();
    respond(&mut store, &sim, &first, ms(12));
    issue_at(&mut store, customer, &ctx(100));
    let second = open_batch(&mut store, ms(110));
    let conn = raw(&path);
    // Lote cerrado.
    let closed = conn.execute(
        "INSERT INTO verifactu_unsent_attempts (batch_id, attempt_id, reason_code, attempted_at) \
         VALUES (?1, NULL, 'connect_failed', 1)",
        [first.id],
    );
    assert!(closed.unwrap_err().to_string().contains("cerrado"));
    // Intento de otro lote.
    let crossed = conn.execute(
        "INSERT INTO verifactu_unsent_attempts (batch_id, attempt_id, reason_code, attempted_at) \
         VALUES (?1, ?2, 'connect_failed', 1)",
        [second.id, ticket.attempt_id],
    );
    assert!(crossed.unwrap_err().to_string().contains("otro lote"));
    assert_eq!(count(&path, "verifactu_unsent_attempts"), 0);
}

#[test]
fn a_duplicate_after_a_no_response_attempt_and_a_technical_fault_is_our_own() {
    let mut store = Store::open_in_memory().unwrap();
    let customer = setup(&mut store);
    let (_, alta) = issue_at(&mut store, customer, &ctx(0));
    let sim = simulator();
    // Lote A: el cuerpo llega, la AEAT lo guarda y se pierde la respuesta.
    let a = open_batch(&mut store, ms(10));
    store.begin_verifactu_attempt(a.id, ms(11)).unwrap();
    sim.estado().perder_siguiente_respuesta = true;
    assert!(post(&sim.url(), &a.envio.a_soap().unwrap()).is_none());
    // Reintento: un Fault técnico cierra A y sus registros vuelven a pendientes.
    store.begin_verifactu_attempt(a.id, ms(70)).unwrap();
    store
        .close_verifactu_batch(a.id, &fault(4108), ms(71))
        .unwrap();
    // Lote B: la AEAT responde 3000 y, como A pudo haberlo registrado, es propio.
    let b = open_batch(&mut store, ms(200));
    assert_eq!(b.record_ids, vec![alta]);
    store.begin_verifactu_attempt(b.id, ms(201)).unwrap();
    respond(&mut store, &sim, &b, ms(202));
    let view = store.verifactu_record_state(alta).unwrap();
    assert_eq!(view.status, VerifactuStatus::Accepted);
    assert!(view.duplicate);
}

#[test]
fn a_duplicate_after_a_single_attempt_fault_or_only_voided_attempts_is_not_ours() {
    let mut store = Store::open_in_memory().unwrap();
    let customer = setup(&mut store);
    let (_, alta) = issue_at(&mut store, customer, &ctx(0));
    let sim = simulator();
    let record = store.verifactu_records().unwrap()[0].record.clone();
    sim.sembrar(&[record]); // otro remitente lo envió
                            // A: un intento anulado (nunca escrito) y un Fault con un solo intento que cuenta.
    let a = open_batch(&mut store, ms(10));
    let voided = store.begin_verifactu_attempt_ticket(a.id, ms(11)).unwrap();
    store
        .void_verifactu_attempt(voided.attempt_id, "connect_failed", ms(12))
        .unwrap();
    store.begin_verifactu_attempt(a.id, ms(70)).unwrap();
    store
        .close_verifactu_batch(a.id, &fault(4108), ms(71))
        .unwrap();
    let b = open_batch(&mut store, ms(200));
    store.begin_verifactu_attempt(b.id, ms(201)).unwrap();
    respond(&mut store, &sim, &b, ms(202));
    assert_eq!(state(&store, alta), VerifactuStatus::UnexpectedDuplicate);
}

#[test]
fn a_v16_database_with_an_open_batch_keeps_its_bytes_and_attempts_and_startup_resends_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nestadmin.db");
    let first_bytes;
    {
        let mut store = Store::open(&path).unwrap();
        let customer = setup(&mut store);
        issue_at(&mut store, customer, &ctx(0));
        let batch = open_batch(&mut store, ms(10));
        first_bytes = batch.envio.a_soap().unwrap();
        // Dos intentos sin respuesta, como los dejaba E5.
        store.begin_verifactu_attempt(batch.id, ms(11)).unwrap();
        store.begin_verifactu_attempt(batch.id, ms(80)).unwrap();
    }
    // Como la dejaría la versión 16: sin las tablas de la migración 17.
    let conn = raw(&path);
    conn.execute_batch(concat!(
        include_str!("sql/downgrade18.sql"),
        "DROP TABLE verifactu_authorization_halts; DROP TABLE verifactu_incidents; \
         DROP TABLE verifactu_unsent_attempts; DROP TABLE verifactu_tls12_preferences; \
         DROP TABLE verifactu_remission_settings; PRAGMA user_version = 16;"
    ))
    .unwrap();
    drop(conn);

    let mut store = Store::open(&path).unwrap();
    assert_eq!(store.schema_version().unwrap(), 18);
    let batch = store.unfinished_verifactu_batch().unwrap().unwrap();
    assert_eq!(batch.envio.a_soap().unwrap(), first_bytes, "mismos bytes");
    assert_eq!(batch.attempts, 2, "los intentos siguen contando");
    assert!(!first_bytes.contains("Incidencia"));
    // El arranque no espera los fallos anteriores y reenvía el lote; el 3000 sería propio.
    assert_eq!(
        store
            .verifactu_next_send_at_mode(SIM, WaitMode::Startup)
            .unwrap(),
        None
    );
    assert!(store.verifactu_next_send_at(SIM).unwrap().is_some());
    let sim = simulator();
    let record = store.verifactu_records().unwrap()[0].clone();
    sim.sembrar(std::slice::from_ref(&record.record));
    store.begin_verifactu_attempt(batch.id, ms(300)).unwrap();
    respond(&mut store, &sim, &batch, ms(301));
    let view = store.verifactu_record_state(record.id).unwrap();
    assert_eq!(
        (view.status, view.duplicate),
        (VerifactuStatus::Accepted, true)
    );
}

#[test]
fn a_settings_change_during_a_send_resumes_an_authorization_halt_it_caused() {
    let mut store = Store::open_in_memory().unwrap();
    let customer = setup(&mut store);
    issue_at(&mut store, customer, &ctx(0));
    let batch = open_batch(&mut store, ms(10));
    store.begin_verifactu_attempt(batch.id, ms(11)).unwrap();
    store
        .close_verifactu_batch(batch.id, &fault(4112), ms(12))
        .unwrap();
    assert!(store.verifactu_authorization_halt().unwrap().is_some());
    assert!(store
        .resume_verifactu_authorization_after_settings_change(
            "configuracion_cambiada_durante_envio",
            ms(13)
        )
        .unwrap());
    assert!(store.verifactu_authorization_halt().unwrap().is_none());
}
