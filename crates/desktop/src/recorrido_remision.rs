//! Remisión de los registros Verifactu desde la ventana (E5, encargo C; D197, D247-D261).
//! Recorrido por IPC, con los nombres y argumentos de `ui/src/api.ts`, contra el simulador de la
//! AEAT en proceso (127.0.0.1; nada sale de la máquina). Reutiliza la sesión simulada de Tauri de
//! `recorrido_venta.rs`: emitir, ejecutar la pasada, ver el registro aceptado, forzar un rechazo
//! con el modo de depuración, subsanar, remitir otra vez, aceptado; la espera entre lotes con las
//! marcas de tiempo en milisegundos; y el cambio de entorno (solo el simulador se puede elegir).
//! Datos sintéticos y carpeta temporal.
//!
//! El hilo de la cola de la sesión también remite en su tick, así que el recorrido no supone quién
//! hace cada pasada: espera al estado que importa con la pasada manual y adelanta el reloj de la
//! venta cuando la remisión dice que hay que esperar.

use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use nestadmin_app::remission::SimMode;
use serde_json::{json, Value};
use tauri::Manager;

use super::recorrido_venta::{draft, master_data, Session, PRODUCER};
use super::SharedState;

/// Milisegundos Unix de 2020-01-01: cualquier marca en segundos queda muy por debajo.
const MS_2020: i64 = 1_577_836_800_000;

fn status(s: &Session) -> Value {
    s.ok("verifactu_status", json!({}))
}

/// Alta (o anulación) de un documento dentro del estado.
fn record_of(status: &Value, record_id: i64) -> Value {
    status["records"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["record_id"] == record_id)
        .unwrap_or_else(|| panic!("el registro {record_id} no consta: {status}"))
        .clone()
}

/// Id del alta Verifactu de un documento emitido.
fn alta_of(s: &Session, document: i64) -> i64 {
    let records = s.ok("verifactu_records", json!({}));
    records
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["document_id"] == document && r["kind"] == "alta")
        .unwrap_or_else(|| panic!("el documento {document} no tiene alta: {records}"))["id"]
        .as_i64()
        .unwrap()
}

/// Hace pasadas manuales (adelantando el reloj cuando toca esperar) hasta que el registro llegue
/// al estado pedido; devuelve el estado de entonces.
fn settle(s: &Session, record_id: i64, wanted: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let st = status(s);
        if record_of(&st, record_id)["status"] == wanted {
            return st;
        }
        assert!(
            Instant::now() < deadline,
            "el registro {record_id} no llegó a {wanted}: {st}"
        );
        let report = s.ok("verifactu_submit_now", json!({}));
        match report["state"].as_str().unwrap() {
            "waiting" => s.advance_clock(report["seconds"].as_i64().unwrap() + 1),
            _ => std::thread::sleep(Duration::from_millis(50)),
        }
    }
}

fn issue(s: &Session, customer: i64, product: i64) -> i64 {
    let id = draft(s, "invoice", customer, product);
    s.ok("sales_issue", json!({ "id": id }));
    id
}

#[test]
fn la_remision_verifactu_desde_la_ventana_de_principio_a_fin_con_el_simulador() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("datos");
    let s = Session::open(&data);
    let (customer, product) = master_data(&s);
    s.write_producer(PRODUCER);
    s.ok("reload_verifactu_producer", json!({}));

    // Sin registros: entorno simulador con su aviso, nada pendiente y la pasada dice «idle».
    let st = status(&s);
    assert_eq!(st["environment"], "simulador");
    assert_eq!(st["selectable_environments"], json!(["simulador"]));
    assert!(st["environment_notice"].as_str().unwrap().contains("AEAT"));
    assert_eq!(st["records"], json!([]));
    assert_eq!(st["last_batch"], Value::Null);
    assert_eq!(st["pending"], 0);
    assert_eq!(st["halted"], false);
    assert_eq!(s.ok("verifactu_submit_now", json!({}))["state"], "idle");
    assert_eq!(s.ok("verifactu_environment", json!({})), "simulador");

    // Emitir deja el registro pendiente (o ya en curso si el tick fue más rápido) y la pasada
    // lo deja aceptado, con el lote, el CSV y la espera en milisegundos.
    let first = issue(&s, customer, product);
    let first_record = alta_of(&s, first);
    let st = settle(&s, first_record, "accepted");
    let record = record_of(&st, first_record);
    assert_eq!(record["kind"], "alta");
    assert_eq!(record["invoice_number"], "F-2026-0001");
    assert_eq!(record["issue_date"], "01-03-2026");
    assert_eq!(record["environment"], "simulador");
    assert_eq!(record["duplicate"], false);
    assert_eq!(st["pending"], 0);
    assert_eq!(st["rejected"], 0);
    let batch = &st["last_batch"];
    assert_eq!(batch["outcome"], "responded", "{st}");
    assert_eq!(batch["record_count"], 1);
    assert!(batch["csv"].as_str().is_some_and(|c| !c.is_empty()), "{st}");
    assert_eq!(batch["wait_seconds"], 60);
    // Las marcas de tiempo son milisegundos (no se multiplican por 1000 en la ventana).
    let started = batch["started_at"].as_i64().unwrap();
    let finished = batch["finished_at"].as_i64().unwrap();
    assert!(started > MS_2020 && finished >= started, "{batch}");
    assert!(finished - started < 3_600_000, "{batch}");
    let next = st["next_send_at"].as_i64().unwrap();
    assert!(next >= finished + 59_000, "{next} vs {finished}");
    assert!(next <= finished + 61_000);
    // Nada pendiente: la pasada manual no envía nada aunque no haya pasado la espera.
    assert_eq!(s.ok("verifactu_submit_now", json!({}))["state"], "idle");

    // Rechazo forzado con el modo de depuración: el siguiente envío se rechaza.
    s.advance_clock(61);
    s.app
        .state::<SharedState>()
        .app()
        .unwrap()
        .set_verifactu_sim_mode(Some(SimMode::RejectNext));
    let second = issue(&s, customer, product);
    let second_record = alta_of(&s, second);
    let st = settle(&s, second_record, "rejected");
    let record = record_of(&st, second_record);
    assert_eq!(record["aeat_code"], 1100, "{record}");
    assert_eq!(record["aeat_description"], "Rechazo simulado");
    assert_eq!(st["rejected"], 1);
    assert_eq!(st["pending"], 0);
    assert_eq!(record_of(&st, first_record)["status"], "accepted");

    // Con un rechazado sin subsanar no se cambia de entorno (y nada cambia).
    s.fails(
        "verifactu_set_environment",
        json!({ "environment": "preproduccion" }),
        "verifactu_environment_busy",
    );
    assert_eq!(s.ok("verifactu_environment", json!({})), "simulador");

    // Subsanar crea un registro nuevo vinculado y pendiente; otra vez, el mismo (doble clic).
    // Subsanar algo que no está rechazado se rechaza con código.
    let done = s.ok("verifactu_subsanate", json!({ "recordId": second_record }));
    assert_eq!(done["created"], true, "{done}");
    assert_eq!(done["rejected_record_id"], second_record);
    let fix = done["record_id"].as_i64().unwrap();
    assert_ne!(fix, second_record);
    let again = s.ok("verifactu_subsanate", json!({ "recordId": second_record }));
    assert_eq!(again["created"], false);
    assert_eq!(again["record_id"], fix);
    assert!(s
        .call("verifactu_subsanate", json!({ "recordId": first_record }))
        .is_err());
    let st = status(&s);
    let rejected = record_of(&st, second_record);
    assert_eq!(rejected["status"], "subsanated", "{rejected}");
    assert_eq!(rejected["subsanation_record_id"], fix);
    assert_eq!(st["rejected"], 0);

    // Remitir otra vez: la subsanación queda aceptada.
    let st = settle(&s, fix, "accepted");
    assert_eq!(record_of(&st, fix)["kind"], "alta");
    assert_eq!(record_of(&st, second_record)["status"], "subsanated");
    assert_eq!(st["pending"], 0);

    // La espera se respeta y se dice cuánto falta, en segundos y con el instante en ms.
    let third = issue(&s, customer, product);
    let third_record = alta_of(&s, third);
    let report = s.ok("verifactu_submit_now", json!({}));
    assert_eq!(report["state"], "waiting", "{report}");
    let seconds = report["seconds"].as_i64().unwrap();
    assert!((1..=60).contains(&seconds), "{report}");
    assert!(report["until"].as_i64().unwrap() > MS_2020, "{report}");
    let st = status(&s);
    assert_eq!(record_of(&st, third_record)["status"], "pending");
    assert_eq!(st["pending"], 1);
    let remaining = st["wait_remaining_seconds"].as_i64().unwrap();
    assert!((1..=60).contains(&remaining), "{st}");
    s.advance_clock(seconds + 1);
    settle(&s, third_record, "accepted");

    // Entornos: solo el simulador se puede elegir; preproducción y producción dicen por qué no.
    for environment in ["preproduccion", "produccion"] {
        let error = s.fails(
            "verifactu_set_environment",
            json!({ "environment": environment }),
            "environment_unavailable",
        );
        assert!(error.contains(environment), "{error}");
    }
    s.fails(
        "verifactu_set_environment",
        json!({ "environment": "marte" }),
        "invalid_input",
    );
    let same = s.ok(
        "verifactu_set_environment",
        json!({ "environment": "simulador" }),
    );
    assert_eq!(
        same,
        json!({ "environment": "simulador", "changed": false })
    );
    assert_eq!(s.ok("verifactu_environment", json!({})), "simulador");
    s.close();
}

/// Servidor que acepta conexiones y no contesta nunca (como una red que se cuelga).
struct SilentServer {
    url: String,
    hits: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl SilentServer {
    fn start() -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}/servicio", listener.local_addr().unwrap());
        let hits = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let (h, s) = (hits.clone(), stop.clone());
        let handle = std::thread::spawn(move || {
            let mut held = Vec::new();
            while !s.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        h.fetch_add(1, Ordering::SeqCst);
                        held.push(stream);
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(20)),
                }
            }
        });
        Self {
            url,
            hits,
            stop,
            handle: Some(handle),
        }
    }
}

impl Drop for SilentServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

#[test]
fn remitir_ahora_no_retiene_el_cerrojo_durante_la_red_y_otro_comando_responde() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("datos");
    let s = Session::open(&data);
    let (customer, product) = master_data(&s);
    s.write_producer(PRODUCER);
    s.ok("reload_verifactu_producer", json!({}));

    // Todo envío va a un servidor que acepta la conexión y no contesta.
    let server = SilentServer::start();
    s.app
        .state::<SharedState>()
        .app()
        .unwrap()
        .set_verifactu_endpoint(Some(server.url.clone()));
    let doc = issue(&s, customer, product);
    let record = alta_of(&s, doc);

    // «Remitir ahora» desde otro hilo: se queda esperando la respuesta que no llega.
    let call = s.caller();
    let manual = std::thread::spawn(move || call("verifactu_submit_now", json!({})));
    let deadline = Instant::now() + Duration::from_secs(15);
    while server.hits.load(Ordering::SeqCst) == 0 {
        assert!(Instant::now() < deadline, "ningún envío llegó al servidor");
        std::thread::sleep(Duration::from_millis(20));
    }

    // Con la llamada de red en curso (hasta 10 s de espera), otros comandos responden al momento:
    // la aplicación no está bloqueada.
    for command in [
        "verifactu_status",
        "sales_setup_status",
        "verifactu_environment",
    ] {
        let started = Instant::now();
        let value = s.ok(command, json!({}));
        let took = started.elapsed();
        assert!(
            took < Duration::from_secs(3),
            "{command} tardó {took:?} con la remisión en la red: {value}"
        );
    }
    let st = status(&s);
    assert_eq!(record_of(&st, record)["status"], "sending", "{st}");

    // La pasada manual acaba sin respuesta (o, si la del tick iba primero, dice que hay otra en
    // curso); el lote queda abierto y se ve el motivo.
    let report = manual.join().unwrap().expect("la pasada manual contesta");
    let state = report["state"].as_str().unwrap().to_owned();
    assert!(
        state == "no_response" || state == "busy",
        "pasada manual: {report}"
    );
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let st = status(&s);
        if st["last_error"]["code"] == "unavailable" {
            assert_eq!(record_of(&st, record)["status"], "sending");
            assert_eq!(st["last_batch"]["outcome"], Value::Null, "{st}");
            break;
        }
        assert!(Instant::now() < deadline, "no se vio el último error: {st}");
        std::thread::sleep(Duration::from_millis(100));
    }
    s.close();
}
