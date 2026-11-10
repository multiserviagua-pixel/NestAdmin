//! Índice local y búsqueda de la etapa E1 del chat (D179, D180, D181, R32; T35a-c). Solo datos
//! sintéticos: un proveedor espía que cuenta sus llamadas y guarda lo que recibe, el simulador de
//! correo (con sus contadores de consultas) y SQLite abierto aparte para mirar las tablas.

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use nestadmin_ai::{
    AiError, GenerateRequest, GenerateResult, Provider, ProviderKind, Task as AiTask, Usage,
};
use nestadmin_app::search::{extract_for_index, question_terms, IndexOutcome, Matched};
use nestadmin_app::{App, AppError, AppPaths};
use nestadmin_core::{CoreError, SearchFilters, SearchKind, SearchStatus, TaskState};
use nestadmin_extract::{ExtractError, Extractor, Ocr};
use nestadmin_simulators::Message;
use serde_json::{json, Value};

const SENTINEL: &str = "SENTINELA-INDICE-42";

/// Responde como el proveedor de `flows.rs` y guarda cada petición (tarea y contenido).
struct Spy {
    calls: AtomicUsize,
    requests: Mutex<Vec<(bool, String)>>,
}

impl Spy {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            calls: AtomicUsize::new(0),
            requests: Mutex::new(Vec::new()),
        })
    }
    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl Provider for Spy {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Ollama
    }
    fn model(&self) -> &str {
        "simulado"
    }
    fn generate(&self, request: &GenerateRequest) -> Result<GenerateResult, AiError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let extraction = matches!(request.task, AiTask::Extraction);
        self.requests
            .lock()
            .unwrap()
            .push((extraction, request.content.clone()));
        let output = match request.task {
            AiTask::Extraction => serde_json::from_str(nestadmin_ai::tasks::document_text(
                &request.content,
            ))
            .unwrap_or_else(|_| {
                json!({
                    "supplier": "Proveedor Alfa Suministros SL", "invoice_number": "FS-2026-014",
                    "date": "2026-10-01", "subtotal": "120.00", "tax": "25.20", "total": "145.20",
                })
            }),
            _ => json!({ "category": "solicitud_factura", "draft": "Adjuntamos la factura." }),
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

fn open(dir: &tempfile::TempDir, spy: &Arc<Spy>) -> App {
    let extractor = Extractor::new(dir.path().join("sin-pdfium"), Box::new(NoOcr));
    App::open(
        &AppPaths::new(dir.path()),
        Arc::clone(spy) as Arc<dyn Provider>,
        Arc::new(extractor),
    )
    .unwrap()
}

fn sql(dir: &tempfile::TempDir) -> rusqlite::Connection {
    rusqlite::Connection::open(dir.path().join("nestadmin.db")).unwrap()
}

/// Texto de una factura sintética: el JSON que el proveedor simulado devuelve tal cual. `note`
/// es texto libre del documento que no forma parte de los campos.
fn invoice(supplier: &str, number: &str, total: &str, note: &str) -> String {
    let day: u32 = number
        .trim_start_matches(|c: char| !c.is_ascii_digit())
        .parse()
        .unwrap();
    json!({
        "supplier": supplier, "invoice_number": number,
        "date": format!("2026-10-{day:02}"),
        "subtotal": "100.00", "tax": "21.00", "total": total,
        "zz_nota": note,
    })
    .to_string()
}

fn message(id: &str, subject: &str, from: &str, body: &str, date: &str) -> Message {
    Message {
        id: id.into(),
        subject: Some(subject.into()),
        from: Some(from.into()),
        to: Some(vec!["administracion@nestadmin.test".into()]),
        body: Some(body.into()),
        date: Some(date.into()),
        attachments: Vec::new(),
        signals: None,
        thread_key: None,
    }
    .with_checked_signals(&[])
}

fn drain(app: &mut App) {
    while app.process_next().unwrap().is_some() {}
}

/// Todas las filas de una tabla como texto, para comparar antes y después.
fn dump(conn: &rusqlite::Connection, table: &str) -> Vec<String> {
    let columns: Vec<String> = conn
        .prepare(&format!("SELECT name FROM pragma_table_info('{table}')"))
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let row = columns
        .iter()
        .map(|c| format!("ifnull(CAST(\"{c}\" AS TEXT), '~')"))
        .collect::<Vec<_>>()
        .join(" || '|' || ");
    conn.prepare(&format!("SELECT {row} FROM \"{table}\" ORDER BY 1"))
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

fn count(conn: &rusqlite::Connection, query: &str) -> i64 {
    conn.query_row(query, [], |r| r.get(0)).unwrap()
}

/// Inserta una tarea ya cerrada tal como la dejaría una versión anterior (sin fila de índice).
fn insert_task(conn: &rusqlite::Connection, kind: &str, input: &Value, state: &str) -> i64 {
    conn.execute(
        "INSERT INTO tasks (kind, input_ref, state, result, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, 1, 1)",
        rusqlite::params![
            kind,
            input.to_string(),
            state,
            json!({ "type": kind }).to_string()
        ],
    )
    .unwrap();
    conn.last_insert_rowid()
}

fn hits(app: &App, query: &str) -> Vec<nestadmin_core::SearchHit> {
    app.search(query, SearchFilters::default(), 0)
        .unwrap()
        .results
}

#[test]
fn t35a_finished_documents_and_mail_are_indexed_without_leaking_the_text() {
    let dir = tempfile::tempdir().unwrap();
    let spy = Spy::new();
    let mut app = open(&dir, &spy);

    // Una factura: queda indexada con su texto y sus campos al terminar, sin ninguna pasada.
    // El texto va tras más de 160 caracteres: el resultado de la tarea ya guarda la línea de cada
    // campo localizado (hasta 160), y eso no es lo que se comprueba aquí.
    let note = format!(
        "Consumo eléctrico de octubre {}{SENTINEL}",
        "relleno ".repeat(30)
    );
    app.import_document(
        "luz-octubre.txt",
        &invoice("Distribución Eléctrica SL", "F-1", "121.50", &note),
    )
    .unwrap();
    let task = app.process_next().unwrap().unwrap();
    assert_eq!(task.state, TaskState::AwaitingReview);
    let page = app
        .search("eléctrico", SearchFilters::default(), 0)
        .unwrap();
    assert_eq!(page.pending_index, 0);
    let hit = &page.results[0];
    assert_eq!(hit.task_id, task.id);
    assert_eq!(hit.kind, SearchKind::Invoice);
    assert_eq!(hit.index_status, SearchStatus::Indexed);
    assert_eq!(hit.title, "luz-octubre.txt");
    assert_eq!(hit.party, "Distribución Eléctrica SL");
    assert_eq!(hit.number, "F-1");
    assert_eq!(hit.doc_date.as_deref(), Some("2026-10-01"));
    assert_eq!(hit.total_minor, Some(12_150));
    assert_eq!(hit.task_state, TaskState::AwaitingReview);
    assert!(hit.snippet.contains("eléctrico") || hit.snippet.contains("Consumo"));

    // Correos: uno normal con su cuerpo y un informativo (remitente `no-reply`).
    app.simulators()
        .0
        .add_message(message(
            "M-0100",
            "Aviso de mantenimiento",
            "no-reply@servicio-omega.test",
            "Ventana de mantenimiento programado el sábado.",
            "2026-10-08T09:00:00Z",
        ))
        .unwrap();
    app.fetch_mail().unwrap();
    drain(&mut app);
    let order = hits(&app, "pedido 2231");
    assert_eq!(order.len(), 1);
    assert_eq!(order[0].kind, SearchKind::Email);
    assert_eq!(order[0].index_status, SearchStatus::Indexed);
    assert_eq!(order[0].party, "cliente.beta@ejemplo.test");
    assert_eq!(order[0].doc_date.as_deref(), Some("2026-10-02"));
    let notice = hits(&app, "mantenimiento programado");
    assert_eq!(notice.len(), 1, "el informativo también se indexa");
    assert_eq!(notice[0].task_state, TaskState::Completed);
    assert_eq!(notice[0].index_status, SearchStatus::Indexed);
    assert_eq!(notice[0].doc_date.as_deref(), Some("2026-10-08"));
    assert_eq!(
        app.search("", SearchFilters::default(), 0)
            .unwrap()
            .pending_index,
        0
    );

    // Un correo guardado sin cuerpo (como en la primera lectura): `no_text`, pero se encuentra
    // por asunto y remitente.
    let conn = sql(&dir);
    insert_task(
        &conn,
        "email",
        &json!({ "message_id": "simulador:M-0200", "message": {
            "id": "simulador:M-0200", "subject": "Renovación del seguro",
            "from": "seguros@aseguradora.test", "date": "2026-09-20T10:00:00Z",
        } }),
        "completed",
    );
    assert!(app.index_pending(5).unwrap().is_empty());
    let found = hits(&app, "renovación seguro");
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].index_status, SearchStatus::NoText);
    assert_eq!(found[0].party, "seguros@aseguradora.test");
    assert_eq!(found[0].doc_date.as_deref(), Some("2026-09-20"));
    assert_eq!(hits(&app, "aseguradora").len(), 1);

    // El texto extraído solo viaja en la petición de extracción de la IA (la habitual): ni en
    // las demás peticiones, ni en el historial, ni en ninguna tabla salvo el índice.
    for (extraction, content) in spy.requests.lock().unwrap().iter() {
        if content.contains(SENTINEL) {
            assert!(extraction, "el texto extraído salió en otra petición");
        }
    }
    for entry in app.audit_log().unwrap() {
        assert!(!format!("{entry:?}").contains(SENTINEL));
    }
    let tables: Vec<String> = conn
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'search_%'")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    for table in tables {
        for row in dump(&conn, &table) {
            assert!(!row.contains(SENTINEL), "{table}: {row}");
        }
    }
    assert_eq!(
        count(
            &conn,
            &format!("SELECT count(*) FROM search_index WHERE body LIKE '%{SENTINEL}%'")
        ),
        1
    );
}

#[test]
fn t35a_a_redraft_is_not_indexed_apart() {
    let dir = tempfile::tempdir().unwrap();
    let spy = Spy::new();
    let mut app = open(&dir, &spy);
    let conn = sql(&dir);
    insert_task(
        &conn,
        "email",
        &json!({ "redraft_of": 1, "message": {
            "id": "a:1", "subject": "Re: algo", "from": "a@x.test", "body": "texto",
        } }),
        "awaiting_review",
    );
    assert!(app.index_pending(5).unwrap().is_empty());
    assert!(!app.index_backlog().unwrap());
    let page = app.search("", SearchFilters::default(), 0).unwrap();
    assert_eq!((page.total, page.pending_index), (0, 0));
}

#[test]
fn t35b_indexing_earlier_tasks_is_resumable_and_has_no_side_effects() {
    let dir = tempfile::tempdir().unwrap();
    let spy = Spy::new();
    let mut app = open(&dir, &spy);
    for (name, number, total) in [("a.txt", "F-1", "121.00"), ("b.txt", "F-2", "60.50")] {
        app.import_document(
            name,
            &invoice("Alfa SL", number, total, "papel reciclado para oficina"),
        )
        .unwrap();
    }
    app.fetch_mail().unwrap();
    drain(&mut app);

    // Se parte de tareas «anteriores a E1»: sin ninguna fila de índice.
    let conn = sql(&dir);
    let total = count(&conn, "SELECT count(*) FROM search_index");
    assert!(total >= 4, "{total}");
    conn.execute("DELETE FROM search_index", []).unwrap();
    assert_eq!(count(&conn, "SELECT count(*) FROM search_fts"), 0);
    let tasks_before = dump(&conn, "tasks");
    let transitions_before = dump(&conn, "task_transitions");
    let audit_before = dump(&conn, "audit_log");
    let calls = spy.calls();
    let mail = app.simulators().0;
    let lookups = (
        mail.sent_lookups(),
        mail.thread_lookups(),
        mail.thread_of_lookups(),
        mail.sent_body_lookups(),
    );
    let sent = mail.sent().unwrap().len();
    let drafts = mail.drafts().unwrap().len();
    assert_eq!(
        app.search("", SearchFilters::default(), 0)
            .unwrap()
            .pending_index as i64,
        total
    );

    // Por pasos de dos tareas; cortar y repetir no duplica nada.
    let extractor = app.index_extractor();
    let mut steps = 0;
    while app.index_backlog().unwrap() {
        for pending in app.index_pending(2).unwrap() {
            let outcome = extract_for_index(&extractor, &pending.path);
            assert!(matches!(outcome, IndexOutcome::Text(_)));
            app.store_index(pending.task_id, outcome.clone()).unwrap();
            // Repetir el mismo paso (un corte justo antes de seguir) no cambia nada.
            app.store_index(pending.task_id, outcome).unwrap();
        }
        steps += 1;
        assert!(steps < 20, "la pasada no avanza");
    }
    assert_eq!(count(&conn, "SELECT count(*) FROM search_index"), total);
    assert_eq!(
        count(&conn, "SELECT count(DISTINCT task_id) FROM search_index"),
        total
    );
    assert_eq!(hits(&app, "reciclado").len(), 2);
    assert_eq!(hits(&app, "pedido 2231").len(), 1);

    // Ni el estado, ni el resultado, ni las transiciones, ni el historial, ni la IA, ni el
    // conector de correo: nada se toca.
    assert_eq!(dump(&conn, "tasks"), tasks_before);
    assert_eq!(dump(&conn, "task_transitions"), transitions_before);
    assert_eq!(dump(&conn, "audit_log"), audit_before);
    assert_eq!(spy.calls(), calls);
    let mail = app.simulators().0;
    assert_eq!(
        (
            mail.sent_lookups(),
            mail.thread_lookups(),
            mail.thread_of_lookups(),
            mail.sent_body_lookups(),
        ),
        lookups
    );
    assert_eq!(mail.sent().unwrap().len(), sent);
    assert_eq!(mail.drafts().unwrap().len(), drafts);
}

#[test]
fn t35b_missing_original_is_no_text_and_extraction_errors_are_failed_with_a_code() {
    let dir = tempfile::tempdir().unwrap();
    let spy = Spy::new();
    let mut app = open(&dir, &spy);
    let conn = sql(&dir);

    // Original ausente: la copia guardada se borra.
    let task = app
        .import_document("falta.txt", &invoice("Alfa SL", "F-1", "121.00", "nota"))
        .unwrap();
    app.process_next().unwrap();
    let input: Value = serde_json::from_str(&app.tasks().unwrap()[0].input_ref).unwrap();
    std::fs::remove_file(input["path"].as_str().unwrap()).unwrap();
    // Un PDF sin PDFium falla al extraer con un código recuperable.
    let pdf = app.import_file("Factura.PDF", b"%PDF-1.4").unwrap();
    app.process_next().unwrap();
    assert_eq!(
        app.tasks()
            .unwrap()
            .iter()
            .find(|t| t.id == pdf.id)
            .unwrap()
            .state,
        TaskState::Failed
    );

    conn.execute("DELETE FROM search_index", []).unwrap();
    let calls = spy.calls();
    let extractor = app.index_extractor();
    let pending = app.index_pending(10).unwrap();
    // La factura sin original ya quedó `no_text`; solo el PDF espera su extracción.
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].task_id, pdf.id);
    let missing: (String, String) = conn
        .query_row(
            "SELECT status, title FROM search_index WHERE task_id = ?1",
            [task.id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(missing, ("no_text".into(), "falta.txt".into()));

    let outcome = extract_for_index(&extractor, &pending[0].path);
    assert_eq!(outcome, IndexOutcome::Failed("pdf_unavailable".into()));
    app.store_index(pdf.id, outcome).unwrap();
    let (status, detail): (String, String) = conn
        .query_row(
            "SELECT status, detail FROM search_index WHERE task_id = ?1",
            [pdf.id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        (status.as_str(), detail.as_str()),
        ("failed", "pdf_unavailable")
    );
    let page = app.search("", SearchFilters::default(), 0).unwrap();
    assert_eq!((page.failed, page.no_text, page.pending_index), (1, 1, 0));

    // Una fila con texto no se pisa con un fallo posterior.
    app.import_document("buena.txt", &invoice("Beta SL", "F-2", "10.00", "nota"))
        .unwrap();
    drain(&mut app);
    let good = app.tasks().unwrap().iter().map(|t| t.id).max().unwrap();
    app.store_index(good, IndexOutcome::Failed("ocr_failed".into()))
        .unwrap();
    app.store_index(good, IndexOutcome::NoText).unwrap();
    assert_eq!(
        count(
            &conn,
            &format!(
                "SELECT count(*) FROM search_index WHERE task_id = {good} AND status = 'indexed'"
            )
        ),
        1
    );

    // «Reintentar»: las filas fallidas vuelven a pendientes.
    assert_eq!(app.reindex_failed().unwrap(), 1);
    assert_eq!(
        count(
            &conn,
            "SELECT count(*) FROM search_index WHERE status = 'failed'"
        ),
        0
    );
    assert!(app.index_backlog().unwrap());
    assert_eq!(app.index_pending(10).unwrap().len(), 1);
    assert_eq!(app.reindex_failed().unwrap(), 0);
    assert_eq!(spy.calls(), calls + 1, "solo la factura buena usó la IA");
}

#[test]
fn t35c_search_matches_without_accents_by_prefix_and_with_filters() {
    let dir = tempfile::tempdir().unwrap();
    let spy = Spy::new();
    let mut app = open(&dir, &spy);
    for (name, supplier, number, total, note) in [
        (
            "luz.txt",
            "Distribución Eléctrica SL",
            "F-1",
            "121.00",
            "Factura de la luz de octubre",
        ),
        (
            "agua.txt",
            "Aguas del Sur SA",
            "F-5",
            "60.50",
            "Consumo de agua del trimestre",
        ),
        (
            "papel.txt",
            "Papelería Central SL",
            "F-9",
            "300.00",
            "Papel y tóner",
        ),
    ] {
        app.import_document(name, &invoice(supplier, number, total, note))
            .unwrap();
    }
    drain(&mut app);

    // Con y sin tildes, con mayúsculas y por prefijo.
    for query in ["eléctrica", "ELECTRICA", "electr", "distribucion electrica"] {
        let found = hits(&app, query);
        assert_eq!(found.len(), 1, "{query}");
        assert_eq!(found[0].title, "luz.txt");
    }
    // La sintaxis de FTS5 es texto: no falla ni se interpreta.
    for query in [
        "\"electr* OR NEAR(",
        "luz OR agua",
        "title:luz",
        "-luz",
        "*",
    ] {
        assert!(
            app.search(query, SearchFilters::default(), 0).is_ok(),
            "{query}"
        );
    }
    assert!(hits(&app, "luz OR agua").is_empty());

    // Sin términos, lista por fecha descendente con los filtros.
    let all = app.search("", SearchFilters::default(), 0).unwrap();
    assert_eq!(all.total, 3);
    let dates: Vec<_> = all
        .results
        .iter()
        .map(|h| h.doc_date.clone().unwrap())
        .collect();
    assert_eq!(dates, ["2026-10-09", "2026-10-05", "2026-10-01"]);
    assert_eq!(all.results[1].title, "agua.txt");
    let filtered = |f: SearchFilters| {
        app.search("", f, 0)
            .unwrap()
            .results
            .iter()
            .map(|h| h.title.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        filtered(SearchFilters {
            from: Some("2026-10-02".into()),
            to: Some("2026-10-06".into()),
            ..Default::default()
        }),
        ["agua.txt"]
    );
    assert_eq!(
        filtered(SearchFilters {
            party: Some("CENTRAL".into()),
            ..Default::default()
        }),
        ["papel.txt"]
    );
    assert_eq!(
        filtered(SearchFilters {
            min_total_minor: Some(6_000),
            max_total_minor: Some(12_100),
            ..Default::default()
        }),
        ["agua.txt", "luz.txt"]
    );
    assert_eq!(
        filtered(SearchFilters {
            kind: Some(SearchKind::Email),
            ..Default::default()
        }),
        Vec::<String>::new()
    );
    assert_eq!(
        filtered(SearchFilters {
            status: Some("awaiting_review".into()),
            kind: Some(SearchKind::Invoice),
            ..Default::default()
        })
        .len(),
        3
    );
    assert!(filtered(SearchFilters {
        status: Some("completed".into()),
        ..Default::default()
    })
    .is_empty());
    // Los filtros se combinan con los términos.
    let page = app
        .search(
            "factura",
            SearchFilters {
                to: Some("2026-10-04".into()),
                ..Default::default()
            },
            0,
        )
        .unwrap();
    assert_eq!(page.total, 1);
    assert_eq!(page.results[0].title, "luz.txt");
    // Pasar del final da una página vacía, con el total.
    let past = app.search("", SearchFilters::default(), 50).unwrap();
    assert!(past.results.is_empty());
    assert_eq!(past.total, 3);
}

#[test]
fn t35c_unknown_filters_are_invalid_filter_errors() {
    let dir = tempfile::tempdir().unwrap();
    let spy = Spy::new();
    let app = open(&dir, &spy);
    for filters in [
        SearchFilters {
            from: Some("2026-13-45".into()),
            ..Default::default()
        },
        SearchFilters {
            to: Some("ayer".into()),
            ..Default::default()
        },
        SearchFilters {
            status: Some("inventado".into()),
            ..Default::default()
        },
    ] {
        let error = app.search("luz", filters, 0).unwrap_err();
        assert!(
            matches!(error, AppError::Core(CoreError::InvalidFilter(_))),
            "{error:?}"
        );
        assert!(error.to_string().contains("filtro de búsqueda no válido"));
    }
}

#[test]
fn t35c_page_reports_what_is_missing_explicitly() {
    let dir = tempfile::tempdir().unwrap();
    let spy = Spy::new();
    let mut app = open(&dir, &spy);
    let conn = sql(&dir);
    insert_task(
        &conn,
        "email",
        &json!({ "message": { "id": "a:1", "subject": "Hola", "from": "a@x.test" } }),
        "completed",
    );
    insert_task(
        &conn,
        "invoice",
        &json!({ "document": "x.pdf", "path": "no-existe.pdf" }),
        "completed",
    );
    let page = app.search("", SearchFilters::default(), 0).unwrap();
    assert_eq!((page.total, page.pending_index, page.no_text), (0, 2, 0));
    assert!(app.index_pending(10).unwrap().is_empty());
    let page = app.search("", SearchFilters::default(), 0).unwrap();
    assert_eq!((page.total, page.pending_index, page.no_text), (2, 0, 2));
    assert!(page
        .results
        .iter()
        .all(|h| h.index_status == SearchStatus::NoText));
}

#[test]
fn t35c_question_terms_keep_the_useful_words() {
    assert_eq!(
        question_terms("¿Dónde está la factura de la luz de octubre?"),
        ["luz", "octubre"]
    );
    assert_eq!(
        question_terms("Muéstrame las facturas de Aguas del Sur, por favor."),
        ["aguas", "sur"]
    );
    assert_eq!(question_terms("Busca el pedido 2231"), ["pedido", "2231"]);
    assert!(question_terms("¿Qué tengo?").is_empty());
    assert!(question_terms("  ¿¡ ?! ").is_empty());
}

#[test]
fn t35c_search_question_requires_all_words_then_admits_any() {
    let dir = tempfile::tempdir().unwrap();
    let spy = Spy::new();
    let mut app = open(&dir, &spy);
    app.import_document(
        "luz.txt",
        &invoice(
            "Distribución Eléctrica SL",
            "F-1",
            "121.00",
            "Factura de la luz de octubre",
        ),
    )
    .unwrap();
    app.import_document(
        "agua.txt",
        &invoice(
            "Aguas del Sur SA",
            "F-5",
            "60.50",
            "Consumo de agua de marzo",
        ),
    )
    .unwrap();
    drain(&mut app);
    let calls = spy.calls();

    let all = app
        .search_question("¿Dónde está la factura de la luz de octubre?")
        .unwrap();
    assert_eq!(all.matched, Matched::All);
    assert_eq!(all.terms, ["luz", "octubre"]);
    assert_eq!(all.kind, Some(SearchKind::Invoice));
    assert_eq!(all.page.results.len(), 1);
    assert_eq!(all.page.results[0].title, "luz.txt");

    // Con «marzo» no hay ningún documento que tenga todas las palabras: se admite cualquiera.
    let any = app.search_question("factura de la luz de marzo").unwrap();
    assert_eq!(any.matched, Matched::Any);
    assert_eq!(any.terms, ["luz", "marzo"]);
    assert_eq!(any.kind, Some(SearchKind::Invoice));
    assert_eq!(any.page.total, 2);

    let none = app.search_question("¿Dónde está el contrato zzz?").unwrap();
    assert_eq!(none.matched, Matched::None);
    assert_eq!(none.page.total, 0);
    let single = app.search_question("zzzz").unwrap();
    assert_eq!(single.matched, Matched::None);

    // Sin palabras útiles no se busca nada, pero los recuentos siguen ahí.
    for text in ["", "¿Qué tengo?", "   "] {
        let empty = app.search_question(text).unwrap();
        assert_eq!(empty.matched, Matched::None, "{text:?}");
        assert!(empty.terms.is_empty() && empty.page.results.is_empty());
        assert_eq!(empty.page.total, 0);
        assert_eq!(empty.page.pending_index, 0);
    }
    assert_eq!(
        serde_json::to_value(&all).unwrap()["matched"],
        "all",
        "serde en snake_case"
    );
    assert_eq!(spy.calls(), calls, "sin ninguna petición a la IA");
}

/// Una factura y dos correos sintéticos; ninguno de los textos de la factura contiene la palabra
/// «factura».
fn kind_fixture() -> (tempfile::TempDir, Arc<Spy>, App) {
    let dir = tempfile::tempdir().unwrap();
    let spy = Spy::new();
    let mut app = open(&dir, &spy);
    app.import_document(
        "luz.txt",
        &invoice(
            "Distribución Eléctrica SL",
            "F-1",
            "121.00",
            "Consumo de la luz de octubre",
        ),
    )
    .unwrap();
    drain(&mut app);
    let conn = sql(&dir);
    for (id, subject, body) in [
        ("a:1", "Aviso de la luz", "La luz sube en octubre."),
        ("a:2", "Reunión", "Nos vemos el jueves."),
    ] {
        insert_task(
            &conn,
            "email",
            &json!({ "message": {
                "id": id, "subject": subject, "from": "ana@cliente.test",
                "date": "2026-10-03", "body": body,
            } }),
            "completed",
        );
    }
    assert!(app.index_pending(10).unwrap().is_empty());
    (dir, spy, app)
}

#[test]
fn d181_a_single_kind_word_filters_and_is_not_a_term() {
    let (_dir, spy, app) = kind_fixture();
    let calls = spy.calls();
    // La factura no contiene «factura», pero se encuentra con «la factura de la luz».
    let a = app
        .search_question("¿Dónde está la factura de la luz de octubre?")
        .unwrap();
    assert_eq!(a.terms, ["luz", "octubre"]);
    assert_eq!(
        (a.matched, a.kind),
        (Matched::All, Some(SearchKind::Invoice))
    );
    assert_eq!(a.page.total, 1);
    assert_eq!(a.page.results[0].title, "luz.txt");
    // El correo con las mismas palabras solo sale cuando se pide un correo.
    let m = app.search_question("el correo de la luz").unwrap();
    assert_eq!((m.matched, m.kind), (Matched::All, Some(SearchKind::Email)));
    assert_eq!(m.page.total, 1);
    assert_eq!(m.page.results[0].title, "Aviso de la luz");
    assert_eq!(
        serde_json::to_value(&m).unwrap()["kind"],
        "email",
        "serde en snake_case"
    );
    assert_eq!(spy.calls(), calls);
}

#[test]
fn d181_both_kinds_or_none_do_not_filter() {
    let (_dir, _spy, app) = kind_fixture();
    let both = app.search_question("factura o correo de la luz").unwrap();
    assert_eq!(both.terms, ["luz"]);
    assert_eq!((both.matched, both.kind), (Matched::All, None));
    assert_eq!(both.page.total, 2);
    let none = app.search_question("la luz").unwrap();
    assert_eq!((none.matched, none.kind), (Matched::All, None));
    assert_eq!(none.page.total, 2);
}

#[test]
fn d181_only_a_kind_lists_it_by_date() {
    let (_dir, _spy, app) = kind_fixture();
    let a = app.search_question("mis facturas").unwrap();
    assert!(a.terms.is_empty());
    assert_eq!(
        (a.matched, a.kind),
        (Matched::All, Some(SearchKind::Invoice))
    );
    assert_eq!(a.page.total, 1);
    let m = app.search_question("¿Qué correos tengo?").unwrap();
    assert_eq!((m.matched, m.kind), (Matched::All, Some(SearchKind::Email)));
    assert_eq!(m.page.total, 2);
    // Un tipo genérico no filtra ni cuenta como palabra.
    let none = app.search_question("mis documentos").unwrap();
    assert_eq!((none.matched, none.kind), (Matched::None, None));
}

#[test]
fn d181_falls_back_to_no_kind_filter_when_the_kind_finds_nothing() {
    let (_dir, _spy, app) = kind_fixture();
    // «jueves» solo está en un correo: con el filtro de facturas no hay nada, sin él sí.
    let a = app.search_question("la factura del jueves").unwrap();
    assert_eq!(a.terms, ["jueves"]);
    assert_eq!((a.matched, a.kind), (Matched::All, None));
    assert_eq!(a.page.results[0].title, "Reunión");
    // Con varias palabras, primero «cualquiera» con el filtro y después sin él.
    let b = app.search_question("factura de luz y jueves").unwrap();
    assert_eq!(
        (b.matched, b.kind),
        (Matched::Any, Some(SearchKind::Invoice))
    );
    assert_eq!(b.page.total, 1);
    assert_eq!(b.page.results[0].title, "luz.txt");
    // Nada en ninguna parte.
    let n = app.search_question("la factura zzzz").unwrap();
    assert_eq!((n.matched, n.page.total), (Matched::None, 0));
}
