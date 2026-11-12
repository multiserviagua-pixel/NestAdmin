//! Recorrido de la facturación de venta desde la ventana (fase 11, E3a; D197). Prueba
//! independiente de `recorrido.rs`: arranca la aplicación de escritorio real —los mismos comandos
//! y la misma carga del productor que `main`— con el runtime simulado de Tauri y llama a los
//! comandos de venta por IPC con los nombres y argumentos que usará la ventana. La venta no usa
//! IA ni red: el proveedor queda sin elegir y no se llama. Datos sintéticos y carpeta temporal;
//! reloj fijo de prueba (2026-03-01 10:00 +01:00).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};

use nestadmin_ai::MemoryCredentialStore;
use nestadmin_app::sales::SalesClock;
use nestadmin_extract::{ExtractError, Ocr};
use nestadmin_setup::System;
use nestadmin_verifactu::{Fecha, FechaHoraHuso};
use serde_json::{json, Value};
use tauri::ipc::{CallbackFn, InvokeBody, InvokeResponseBody};
use tauri::test::{get_ipc_response, mock_builder, MockRuntime, INVOKE_KEY};
use tauri::webview::InvokeRequest;
use tauri::{WebviewUrl, WebviewWindow, WebviewWindowBuilder};

use super::*;

struct NoSystem;

impl System for NoSystem {
    fn ollama_executable(&self) -> Option<PathBuf> {
        None
    }
    fn verify_signature(&self, _file: &Path, _publisher: &str) -> Result<(), String> {
        Err("no disponible en pruebas".into())
    }
    fn install_ollama(&self, _installer: &Path) -> Result<(), String> {
        Err("no disponible en pruebas".into())
    }
    fn start_ollama(&self, _executable: &Path) -> Result<(), String> {
        Err("no disponible en pruebas".into())
    }
    fn tesseract_version(&self, _executable: &Path, _tessdata: &Path) -> Result<String, String> {
        Err("no disponible en pruebas".into())
    }
    fn memory(&self) -> Option<(u64, u64)> {
        None
    }
}

struct NoOcr;

impl Ocr for NoOcr {
    fn recognize(&self, _image: &Path, page: usize) -> Result<String, ExtractError> {
        Err(ExtractError::OcrUnavailable(format!("página {page}")))
    }
}

/// Reloj de pruebas: segundos Unix desplazables, con huso +01:00.
struct FixedClock(AtomicI64);

impl FixedClock {
    /// 2026-03-01 10:00:00 +01:00.
    fn new() -> Arc<Self> {
        let base = FechaHoraHuso::new(Fecha::new(2026, 3, 1).unwrap(), 10, 0, 0, 60)
            .unwrap()
            .unix();
        Arc::new(Self(AtomicI64::new(base)))
    }

    fn advance(&self, seconds: i64) {
        self.0.fetch_add(seconds, Ordering::SeqCst);
    }
}

impl SalesClock for FixedClock {
    fn now(&self) -> FechaHoraHuso {
        FechaHoraHuso::desde_unix(self.0.load(Ordering::SeqCst), 60).unwrap()
    }
}

fn deps(data_dir: &Path, pdfium: &Path) -> Deps {
    let config = nestadmin_setup::Config::windows(
        data_dir.join("sin-tesseract.exe"),
        data_dir.join("sin-tessdata"),
        pdfium.to_owned(),
        data_dir.join("descargas"),
    );
    Deps {
        data_dir: data_dir.to_owned(),
        credentials: Arc::new(MemoryCredentialStore::default()),
        setup: Setup::new(config, Box::new(NoSystem)),
        extractor: Extractor::new(pdfium.to_owned(), Box::new(NoOcr)),
        tray: false,
    }
}

pub(super) struct Session {
    pub(super) app: tauri::App<MockRuntime>,
    window: WebviewWindow<MockRuntime>,
    worker: Option<thread::JoinHandle<()>>,
    clock: Arc<FixedClock>,
    pub(super) data_dir: PathBuf,
}

impl Session {
    /// Abre la aplicación sobre `data_dir` con el reloj fijo, como al ejecutar `nestadmin.exe`.
    pub(super) fn open(data_dir: &Path) -> Self {
        Self::open_with_pdfium(data_dir, &data_dir.join("sin-pdfium"))
    }

    /// Como `open`, con la carpeta de PDFium indicada (los PDF de venta necesitan la real).
    pub(super) fn open_with_pdfium(data_dir: &Path, pdfium: &Path) -> Self {
        let app = configure(mock_builder(), false)
            .build(tauri::generate_context!())
            .expect("la aplicación se construye");
        let worker = start(app.handle(), deps(data_dir, pdfium)).expect("la aplicación arranca");
        let window = WebviewWindowBuilder::new(&app, "main", WebviewUrl::default())
            .build()
            .expect("ventana principal");
        let clock = FixedClock::new();
        app.state::<SharedState>()
            .app()
            .unwrap()
            .set_sales_clock(clock.clone());
        Self {
            app,
            window,
            worker: Some(worker),
            clock,
            data_dir: data_dir.to_owned(),
        }
    }

    pub(super) fn request(
        &self,
        cmd: &str,
        body: InvokeBody,
        headers: tauri::http::HeaderMap,
    ) -> Result<InvokeResponseBody, Value> {
        let url = if cfg!(windows) {
            "http://tauri.localhost"
        } else {
            "tauri://localhost"
        };
        get_ipc_response(
            &self.window,
            InvokeRequest {
                cmd: cmd.into(),
                callback: CallbackFn(0),
                error: CallbackFn(1),
                url: url.parse().unwrap(),
                body,
                headers,
                invoke_key: INVOKE_KEY.to_owned(),
            },
        )
    }

    #[cfg_attr(not(debug_assertions), allow(dead_code))]
    /// Un llamador de comandos que se puede llevar a otro hilo (la ventana es `Send`): sirve para
    /// probar que un comando lento no bloquea a los demás.
    pub(super) fn caller(&self) -> impl Fn(&str, Value) -> Result<Value, String> + Send + 'static {
        let window = self.window.clone();
        move |cmd, args| {
            let url = if cfg!(windows) {
                "http://tauri.localhost"
            } else {
                "tauri://localhost"
            };
            get_ipc_response(
                &window,
                InvokeRequest {
                    cmd: cmd.into(),
                    callback: CallbackFn(0),
                    error: CallbackFn(1),
                    url: url.parse().unwrap(),
                    body: InvokeBody::Json(args),
                    headers: Default::default(),
                    invoke_key: INVOKE_KEY.to_owned(),
                },
            )
            .map(|body| body.deserialize::<Value>().unwrap())
            .map_err(|e| {
                e.as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| e.to_string())
            })
        }
    }

    /// Un comando con argumentos JSON, como `invoke(cmd, args)`.
    pub(super) fn call(&self, cmd: &str, args: Value) -> Result<Value, String> {
        self.request(cmd, InvokeBody::Json(args), Default::default())
            .map(|body| body.deserialize::<Value>().unwrap())
            .map_err(|e| {
                e.as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| e.to_string())
            })
    }

    pub(super) fn ok(&self, cmd: &str, args: Value) -> Value {
        self.call(cmd, args)
            .unwrap_or_else(|e| panic!("{cmd} falló: {e}"))
    }

    /// El comando falla y su texto empieza por `[code]`.
    pub(super) fn fails(&self, cmd: &str, args: Value, code: &str) -> String {
        let error = self
            .call(cmd, args)
            .expect_err(&format!("{cmd} debía fallar"));
        assert!(
            error.starts_with(&format!("[{code}] ")),
            "{cmd}: se esperaba [{code}], llegó: {error}"
        );
        error
    }

    pub(super) fn id(&self, cmd: &str, args: Value) -> i64 {
        self.ok(cmd, args).as_i64().unwrap()
    }

    fn status(&self) -> Value {
        self.ok("sales_setup_status", json!({}))
    }

    #[cfg_attr(not(debug_assertions), allow(dead_code))]
    /// Adelanta el reloj de la venta (el de la remisión, D251) estos segundos.
    pub(super) fn advance_clock(&self, seconds: i64) {
        self.clock.advance(seconds);
    }

    pub(super) fn write_producer(&self, text: &str) {
        std::fs::write(self.data_dir.join(productor::FILE), text).unwrap();
    }

    pub(super) fn close(mut self) {
        stop_worker(self.app.handle());
        if let Some(worker) = self.worker.take() {
            worker.join().unwrap();
        }
        let _ = self.window.destroy();
    }
}

pub(super) const PRODUCER: &str = r#"{ "name": "Productor Demo SL", "tax_id": "B00000034" }"#;

/// Cliente y producto (80 EUR al 21 %) con la empresa; devuelve (cliente, producto).
pub(super) fn master_data(s: &Session) -> (i64, i64) {
    s.ok(
        "sales_save_company",
        json!({ "company": {
            "name": "Taller Demo SL",
            "tax_id": "B12345674",
            "address": "Calle Ficticia 1, 28000 Madrid",
            "email": "demo@example.test",
        }}),
    );
    let customer = s.id(
        "sales_create_customer",
        json!({ "data": {
            "name": "Cliente Uno SA",
            "tax_id": "89890001K",
            "address": "Avenida Inventada 2, 08000 Barcelona",
        }}),
    );
    let product = s.id(
        "sales_create_product",
        json!({ "product": {
            "description": "Revisión anual",
            "unit_price": "80",
            "tax_percent": "21",
        }}),
    );
    (customer, product)
}

/// Borrador de `kind` con una unidad del producto.
pub(super) fn draft(s: &Session, kind: &str, customer: i64, product: i64) -> i64 {
    let id = s.id(
        "sales_create_draft",
        json!({ "draft": { "kind": kind, "customer_id": customer } }),
    );
    s.ok(
        "sales_add_line_from_product",
        json!({ "id": id, "product": product, "quantity": "1" }),
    );
    id
}

#[test]
fn la_venta_completa_desde_la_ventana_con_productor_cargado_del_archivo() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("datos");
    let s = Session::open(&data);

    // Sin archivo del productor: no configurado y sin problema; sin facturas, cadena vacía.
    let status = s.status();
    assert_eq!(status["producer_configured"], false);
    assert_eq!(status["producer_problem"], Value::Null);
    assert_eq!(status["chain"], json!({ "state": "empty" }));
    assert_eq!(status["today"], "2026-03-01");

    // Empresa (con logotipo sintético), cliente y producto.
    let (customer, product) = master_data(&s);
    let company = s.ok("sales_company", json!({}));
    assert_eq!(company["name"], "Taller Demo SL");
    assert_eq!(company["has_logo"], false);
    let mut headers = tauri::http::HeaderMap::new();
    headers.insert("x-nestadmin-mime", "image/png".parse().unwrap());
    s.request(
        "sales_set_company_logo",
        InvokeBody::Raw(b"logotipo sintetico".to_vec()),
        headers,
    )
    .unwrap_or_else(|e| panic!("sales_set_company_logo falló: {e}"));
    let company = s.ok("sales_company", json!({}));
    assert_eq!(company["has_logo"], true);
    assert_eq!(company["logo_mime"], "image/png");
    // Guardar los datos conserva el logotipo.
    s.ok(
        "sales_save_company",
        json!({ "company": {
            "name": "Taller Demo SL",
            "tax_id": "B12345674",
            "address": "Calle Ficticia 1, 28000 Madrid",
        }}),
    );
    assert_eq!(s.ok("sales_company", json!({}))["has_logo"], true);
    let logo = s
        .request(
            "sales_company_logo",
            InvokeBody::Json(json!({})),
            Default::default(),
        )
        .unwrap();
    assert!(matches!(logo, InvokeResponseBody::Raw(b) if b == b"logotipo sintetico"));
    assert_eq!(
        s.ok("sales_customers", json!({})).as_array().unwrap().len(),
        1
    );
    assert_eq!(s.ok("sales_products", json!({}))[0]["unit_price"], "80.00");

    // Presupuesto: se emite sin productor, queda sin registro Verifactu.
    let quote = draft(&s, "quote", customer, product);
    let doc = s.ok("sales_document", json!({ "id": quote }));
    assert_eq!(doc["status"], "draft");
    assert_eq!(doc["lines"].as_array().unwrap().len(), 1);
    let issued = s.ok("sales_issue", json!({ "id": quote }));
    assert_eq!(issued["status"], "issued");
    assert_eq!(issued["number"], "P-2026-0001");
    assert_eq!(issued["issue_date"], "2026-03-01");
    assert_eq!(issued["totals"]["total"], "96.80");
    assert_eq!(s.ok("verifactu_records", json!({})), json!([]));

    // Aceptar, convertir en factura y enlazar.
    let accepted = s.ok("sales_transition", json!({ "id": quote, "to": "accepted" }));
    assert_eq!(accepted["status"], "accepted");
    let invoice = s.id("sales_convert", json!({ "ids": [quote], "to": "invoice" }));
    assert_eq!(
        s.ok("sales_document", json!({ "id": quote }))["status"],
        "converted"
    );
    assert_eq!(
        s.ok("sales_links", json!({ "id": invoice }))
            .as_array()
            .unwrap()
            .len(),
        1
    );

    // Emitir la factura sin productor: error con código estable y nada cambia.
    let error = s.fails(
        "sales_issue",
        json!({ "id": invoice }),
        "verifactu_not_configured",
    );
    assert!(error.contains("productor"), "{error}");
    let doc = s.ok("sales_document", json!({ "id": invoice }));
    assert_eq!(
        (doc["status"].as_str(), doc["number"].clone()),
        (Some("draft"), Value::Null)
    );
    s.fails(
        "sales_void",
        json!({ "id": invoice }),
        "verifactu_not_configured",
    );

    // Cargar el productor del archivo local (copiado con la aplicación abierta).
    s.write_producer(PRODUCER);
    let state = s.ok("reload_verifactu_producer", json!({}));
    assert_eq!(state, json!({ "configured": true, "problem": null }));
    assert_eq!(s.status()["producer_configured"], true);

    // Emitir la factura: alta Verifactu y cadena correcta.
    s.clock.advance(60);
    let issued = s.ok("sales_issue", json!({ "id": invoice }));
    assert_eq!(issued["status"], "issued");
    assert_eq!(issued["number"], "F-2026-0001");
    let records = s.ok("verifactu_records", json!({}));
    let records = records.as_array().unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["kind"], "alta");
    assert_eq!(records[0]["document_id"], invoice);
    assert_eq!(records[0]["invoice_number"], "F-2026-0001");
    assert!(records[0]["record"].is_object());
    assert_eq!(s.status()["chain"], json!({ "state": "ok", "records": 1 }));

    // Cobro parcial y total.
    let partial = s.ok(
        "sales_add_payment",
        json!({ "id": invoice, "date": "2026-03-01", "amount": "50", "method": "transfer" }),
    );
    assert_eq!(partial["collection"]["state"], "partial");
    assert_eq!(partial["collection"]["paid"], "50.00");
    let total = s.ok(
        "sales_add_payment",
        json!({ "id": invoice, "date": "2026-03-01", "amount": "46.80", "method": "cash" }),
    );
    assert_eq!(total["collection"]["state"], "paid");
    assert_eq!(
        s.ok("sales_payments", json!({ "id": invoice }))
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        s.ok("sales_collection", json!({ "id": invoice }))["state"],
        "paid"
    );
    // Quitar el último cobro vuelve a parcial.
    let back = s.ok("sales_remove_payment", json!({ "id": total["payment_id"] }));
    assert_eq!(back["state"], "partial");

    // Anular otra factura: su registro de anulación encadena con los anteriores.
    let other = draft(&s, "invoice", customer, product);
    s.clock.advance(60);
    s.ok("sales_issue", json!({ "id": other }));
    s.clock.advance(60);
    let voided = s.ok("sales_void", json!({ "id": other }));
    assert_eq!(voided["status"], "voided");
    let records = s.ok("verifactu_records", json!({}));
    let kinds: Vec<&str> = records
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["alta", "alta", "anulacion"]);
    assert_eq!(s.status()["chain"], json!({ "state": "ok", "records": 3 }));

    // La consulta filtra por tipo y estado.
    let invoices = s.ok(
        "sales_documents",
        json!({ "filter": { "kind": "invoice" } }),
    );
    assert_eq!(invoices.as_array().unwrap().len(), 2);
    let voided_only = s.ok(
        "sales_documents",
        json!({ "filter": { "status": "voided" } }),
    );
    assert_eq!(voided_only.as_array().unwrap().len(), 1);
    assert_eq!(
        s.ok("sales_documents", json!({})).as_array().unwrap().len(),
        3
    );

    // Recurrentes: solo borradores, y una segunda pasada no duplica.
    let template = s.id(
        "sales_create_recurring",
        json!({ "template": {
            "customer_id": customer,
            "lines": [{ "description": "Cuota mensual", "quantity": "1", "unit_price": "10", "tax_percent": "21" }],
            "frequency": "monthly",
            "first_day": "2026-01-01",
        }}),
    );
    let run = s.ok("sales_generate_recurring", json!({}));
    let created = run["created"].as_array().unwrap();
    assert_eq!(created.len(), 3, "enero, febrero y marzo: {run}");
    assert!(created.iter().all(|c| c["mode"] == "requires_review"));
    for c in created {
        let doc = s.ok("sales_document", json!({ "id": c["document_id"] }));
        assert_eq!(doc["status"], "draft");
        assert_eq!(doc["recurring_template_id"], template);
    }
    let again = s.ok("sales_generate_recurring", json!({}));
    assert_eq!(again["created"], json!([]));
    assert_eq!(
        s.ok("verifactu_records", json!({}))
            .as_array()
            .unwrap()
            .len(),
        3
    );
    s.ok(
        "sales_set_recurring_active",
        json!({ "id": template, "active": false }),
    );
    assert_eq!(
        s.ok("sales_recurring_template", json!({ "id": template }))["active"],
        false
    );

    // Errores de entrada y de datos con su código estable.
    s.fails("sales_document", json!({ "id": 99_999 }), "not_found");
    s.fails(
        "sales_add_payment",
        json!({ "id": invoice, "date": "2026-13-45", "amount": "1", "method": "cash" }),
        "invalid_input",
    );
    s.fails(
        "sales_create_product",
        json!({ "product": { "description": "X", "unit_price": "1", "tax_percent": "7" } }),
        "invalid_input",
    );
    // Los comandos que no son de venta siguen en la lista principal.
    assert_eq!(s.ok("tasks", json!({})), json!([]));
    s.close();
}

#[test]
fn el_productor_se_lee_al_arrancar_y_un_archivo_invalido_deja_el_motivo() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("datos");
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(
        data.join(productor::FILE),
        r#"{ "name": "Secreto Demo SL", "tax_id": "#,
    )
    .unwrap();

    // Archivo roto al arrancar: la aplicación arranca, no hay productor y consta el motivo.
    let s = Session::open(&data);
    let status = s.status();
    assert_eq!(status["producer_configured"], false);
    let problem = status["producer_problem"].as_str().unwrap();
    assert!(
        problem.contains(productor::FILE) && !problem.contains("Secreto"),
        "{problem}"
    );
    let (customer, product) = master_data(&s);
    let invoice = draft(&s, "invoice", customer, product);
    s.fails(
        "sales_issue",
        json!({ "id": invoice }),
        "verifactu_not_configured",
    );

    // NIF inválido: tampoco se configura y el motivo no vuelca el contenido.
    s.write_producer(r#"{ "name": "Secreto Demo SL", "tax_id": "123" }"#);
    let state = s.ok("reload_verifactu_producer", json!({}));
    assert_eq!(state["configured"], false);
    assert!(!state["problem"].as_str().unwrap().contains("Secreto"));
    s.fails(
        "sales_issue",
        json!({ "id": invoice }),
        "verifactu_not_configured",
    );

    // Archivo válido: se emite. Quitarlo y recargar vuelve a «no configurado».
    s.write_producer(PRODUCER);
    assert_eq!(
        s.ok("reload_verifactu_producer", json!({}))["configured"],
        true
    );
    s.ok("sales_issue", json!({ "id": invoice }));
    std::fs::remove_file(data.join(productor::FILE)).unwrap();
    let state = s.ok("reload_verifactu_producer", json!({}));
    assert_eq!(state, json!({ "configured": false, "problem": null }));
    let second = draft(&s, "invoice", customer, product);
    s.clock.advance(60);
    s.fails(
        "sales_issue",
        json!({ "id": second }),
        "verifactu_not_configured",
    );
    s.close();
}

/// Un archivo válido presente al arrancar se carga sin llamar a nada (una carpeta de datos por
/// sesión: los simuladores no se pueden reabrir en el mismo proceso).
#[test]
fn un_archivo_valido_presente_al_arrancar_configura_el_productor() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("datos");
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(data.join(productor::FILE), PRODUCER).unwrap();
    let s = Session::open(&data);
    let status = s.status();
    assert_eq!(status["producer_configured"], true);
    assert_eq!(status["producer_problem"], Value::Null);
    s.close();
}

/// Todo `#[tauri::command]` de `ventas.rs` está en su lista y viceversa, y la lista principal de
/// `main.rs` no incluye ninguno (los reparte `is_sales_command`).
#[test]
fn los_comandos_de_venta_estan_todos_registrados_en_su_lista() {
    let source = include_str!("ventas.rs");
    let defined: std::collections::BTreeSet<&str> = source
        .split("#[tauri::command]")
        .skip(1)
        .filter_map(|rest| {
            let rest = &rest[rest.find("fn ")? + 3..];
            Some(&rest[..rest.find(['(', '<'])?])
        })
        .collect();
    let list = &source[source.find("generate_handler![").unwrap() + "generate_handler![".len()..];
    let registered: std::collections::BTreeSet<&str> = list[..list.find(']').unwrap()]
        .split(',')
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .collect();
    assert_eq!(defined, registered);
    assert!(defined.len() >= 40);
    assert!(
        defined.iter().all(|n| ventas::is_sales_command(n)),
        "{defined:?}"
    );
    let main = include_str!("main.rs");
    let main_list = &main[main.find("generate_handler![").unwrap()..];
    let main_list = &main_list[..main_list.find(']').unwrap()];
    assert!(
        defined
            .iter()
            .all(|n| !main_list.contains(&format!("{n},"))),
        "la lista principal no debe incluir comandos de venta"
    );
}
