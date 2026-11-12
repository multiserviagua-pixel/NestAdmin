//! Índice local de búsqueda (D179, D180). Datos sintéticos.

use nestadmin_core::store::SCHEMA_VERSION;
use nestadmin_core::{
    CoreError, SearchDoc, SearchFilters, SearchKind, SearchQuery, SearchStatus, Store, TaskState,
    SEARCH_PAGE_SIZE, SNIPPET_CHARS,
};
use serde_json::json;

fn doc(task_id: i64, title: &str, party: &str, body: &str) -> SearchDoc {
    SearchDoc {
        task_id,
        kind: SearchKind::Invoice,
        status: SearchStatus::Indexed,
        title: title.into(),
        party: party.into(),
        number: format!("F-{task_id}"),
        doc_date: Some("2026-03-15".into()),
        total_minor: Some(12_100),
        currency: "EUR".into(),
        body: body.into(),
        detail: String::new(),
    }
}

fn q(terms: &[&str]) -> SearchQuery {
    SearchQuery {
        terms: terms.iter().map(|t| t.to_string()).collect(),
        ..SearchQuery::default()
    }
}

/// Crea una tarea `invoice` terminada y la indexa.
fn add(store: &mut Store, title: &str, party: &str, body: &str) -> i64 {
    let id = store.enqueue("invoice", "x").unwrap().id;
    store.claim(id).unwrap();
    store
        .complete_indexed(id, &json!({}), &doc(id, title, party, body))
        .unwrap();
    id
}

fn ids(store: &Store, query: &SearchQuery) -> Vec<i64> {
    store
        .search(query)
        .unwrap()
        .results
        .iter()
        .map(|h| h.task_id)
        .collect()
}

#[test]
fn migration_applies_on_new_and_on_v9_database() {
    assert_eq!(SCHEMA_VERSION, 17);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("n.db");
    let mut store = Store::open(&path).unwrap();
    assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
    add(&mut store, "Factura", "Acme", "texto");
    drop(store);

    // Una base v9: sin el índice y sin su FTS.
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch("DROP TABLE verifactu_authorization_halts; DROP TABLE verifactu_incidents; DROP TABLE verifactu_unsent_attempts; DROP TABLE verifactu_tls12_preferences; DROP TABLE verifactu_remission_settings; DROP TABLE verifactu_subsanations; DROP TABLE verifactu_record_results; DROP TABLE verifactu_batch_results; DROP TABLE verifactu_batch_attempts; DROP TABLE verifactu_batch_records; DROP TABLE verifactu_batches; DROP TABLE verifactu_record_environments; DROP TABLE verifactu_environment_changes; DROP TABLE sales_send_triggers; DROP TABLE sales_document_sends; DROP TABLE company_letterheads; ALTER TABLE company DROP COLUMN letterhead_sha256; ALTER TABLE company DROP COLUMN letterhead_top_mm; ALTER TABLE company DROP COLUMN letterhead_right_mm; ALTER TABLE company DROP COLUMN letterhead_bottom_mm; ALTER TABLE company DROP COLUMN letterhead_left_mm; DROP TABLE installation; DROP TABLE verifactu_records; DROP TABLE assistant_calls; DROP TABLE assistant_turns; DROP TABLE assistant_conversations; DROP TABLE search_fts; DROP TABLE search_index; PRAGMA user_version = 9;")
        .unwrap();
    drop(conn);
    let mut store = Store::open(&path).unwrap();
    assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
    let id = store.enqueue("invoice", "y").unwrap().id;
    store
        .index_document(&doc(id, "Tras migrar", "Acme", ""))
        .unwrap();
    assert_eq!(ids(&store, &q(&["migrar"])), vec![id]);
}

#[test]
fn matches_without_accents_case_and_prefix() {
    let mut s = Store::open_in_memory().unwrap();
    let a = add(
        &mut s,
        "Suministro",
        "Eléctrica Norte",
        "Factura de electricidad de marzo",
    );
    let b = add(&mut s, "Servicio", "Gestoría", "Facturación mensual");
    assert_eq!(ids(&s, &q(&["electricidad"])), vec![a]);
    assert_eq!(ids(&s, &q(&["eléctricidad"])), vec![a]);
    assert_eq!(ids(&s, &q(&["facturacion"])), vec![b]);
    assert_eq!(ids(&s, &q(&["FACTURACIÓN"])), vec![b]);
    assert_eq!(ids(&s, &q(&["electri"])), vec![a]);
    // El prefijo solo vale para el último término.
    assert!(ids(&s, &q(&["electri", "marzo"])).is_empty());
    assert_eq!(ids(&s, &q(&["marzo", "electri"])), vec![a]);
}

#[test]
fn fts_syntax_in_terms_is_plain_text() {
    let mut s = Store::open_in_memory().unwrap();
    add(
        &mut s,
        "Factura",
        "Acme",
        "pago OR cobro; a:b -c NEAR(x y) \"cita\" *",
    );
    for t in [
        "OR", "NEAR(", "*", "\"", ":", "-", "a:b", "\"cita\"", "x*", "NOT", "AND", "(", ")", "^",
    ] {
        for any in [false, true] {
            let query = SearchQuery {
                terms: vec![t.to_string()],
                any,
                ..SearchQuery::default()
            };
            s.search(&query).unwrap_or_else(|e| panic!("{t:?}: {e}"));
        }
    }
    s.search(&q(&["pago", "OR", "NEAR(", "\"", ":"])).unwrap();
    // Como texto: «OR» se encuentra solo si está en el documento.
    assert_eq!(s.search(&q(&["cobro", "OR"])).unwrap().total, 1);
    assert_eq!(
        s.search(&q(&["cobro", "OR", "inexistente"])).unwrap().total,
        0
    );
}

#[test]
fn and_versus_or() {
    let mut s = Store::open_in_memory().unwrap();
    let a = add(&mut s, "A", "P", "alfa beta");
    let b = add(&mut s, "B", "P", "alfa gamma");
    assert_eq!(ids(&s, &q(&["alfa", "beta"])), vec![a]);
    let any = SearchQuery {
        any: true,
        ..q(&["beta", "gamma"])
    };
    let mut got = ids(&s, &any);
    got.sort();
    assert_eq!(got, vec![a, b]);
}

#[test]
fn each_filter_narrows_and_bad_ones_fail() {
    let mut s = Store::open_in_memory().unwrap();
    let a = add(&mut s, "Uno", "Acme SL", "texto");
    let b = add(&mut s, "Dos", "Beta SA", "texto");
    let e = s.enqueue("email", "m").unwrap().id;
    s.claim(e).unwrap();
    s.complete_indexed(
        e,
        &json!({}),
        &SearchDoc {
            kind: SearchKind::Email,
            doc_date: Some("2026-05-01".into()),
            total_minor: None,
            ..doc(e, "Asunto", "ana@example.test", "texto")
        },
    )
    .unwrap();
    s.index_document(&SearchDoc {
        doc_date: Some("2026-01-10".into()),
        total_minor: Some(500),
        ..doc(b, "Dos", "Beta SA", "texto")
    })
    .unwrap();
    let c = s.enqueue("invoice", "c").unwrap().id;
    s.cancel(c).unwrap();
    s.index_document(&SearchDoc {
        doc_date: Some("2026-02-01".into()),
        total_minor: None,
        ..doc(c, "Tres", "Gamma", "texto")
    })
    .unwrap();

    let with = |f: SearchFilters| {
        let mut v = ids(
            &s,
            &SearchQuery {
                filters: f,
                ..q(&["texto"])
            },
        );
        v.sort();
        v
    };
    assert_eq!(with(SearchFilters::default()), vec![a, b, e, c]);
    assert_eq!(
        with(SearchFilters {
            kind: Some(SearchKind::Email),
            ..Default::default()
        }),
        vec![e]
    );
    assert_eq!(
        with(SearchFilters {
            from: Some("2026-03-01".into()),
            ..Default::default()
        }),
        vec![a, e]
    );
    assert_eq!(
        with(SearchFilters {
            to: Some("2026-03-15".into()),
            ..Default::default()
        }),
        vec![a, b, c]
    );
    assert_eq!(
        with(SearchFilters {
            party: Some("BETA".into()),
            ..Default::default()
        }),
        vec![b]
    );
    assert_eq!(
        with(SearchFilters {
            min_total_minor: Some(1_000),
            ..Default::default()
        }),
        vec![a]
    );
    assert_eq!(
        with(SearchFilters {
            max_total_minor: Some(1_000),
            ..Default::default()
        }),
        vec![b]
    );
    assert_eq!(
        with(SearchFilters {
            status: Some("cancelled".into()),
            ..Default::default()
        }),
        vec![c]
    );

    for bad in [
        SearchFilters {
            from: Some("2026-13-01".into()),
            ..Default::default()
        },
        SearchFilters {
            to: Some("2026-02-30".into()),
            ..Default::default()
        },
        SearchFilters {
            from: Some("15/03/2026".into()),
            ..Default::default()
        },
        SearchFilters {
            status: Some("pendiente".into()),
            ..Default::default()
        },
    ] {
        let err = s
            .search(&SearchQuery {
                filters: bad,
                ..SearchQuery::default()
            })
            .unwrap_err();
        assert!(matches!(err, CoreError::InvalidFilter(_)), "{err}");
    }
    // Bisiesto válido.
    s.search(&SearchQuery {
        filters: SearchFilters {
            from: Some("2028-02-29".into()),
            ..Default::default()
        },
        ..SearchQuery::default()
    })
    .unwrap();
}

#[test]
fn no_terms_lists_by_date_descending_nulls_last() {
    let mut s = Store::open_in_memory().unwrap();
    let a = add(&mut s, "A", "P", "");
    let b = add(&mut s, "B", "P", "");
    let c = add(&mut s, "C", "P", "");
    let d = add(&mut s, "D", "P", "");
    for (id, date) in [
        (a, Some("2026-01-01")),
        (b, None),
        (c, Some("2026-06-01")),
        (d, Some("2026-06-01")),
    ] {
        s.index_document(&SearchDoc {
            doc_date: date.map(String::from),
            ..doc(id, "T", "P", "")
        })
        .unwrap();
    }
    assert_eq!(ids(&s, &SearchQuery::default()), vec![d, c, a, b]);
}

#[test]
fn reindexing_updates_without_duplicating_and_delete_keeps_fts_consistent() {
    let mut s = Store::open_in_memory().unwrap();
    let id = add(&mut s, "Antes", "P", "palabraantigua");
    assert_eq!(s.search(&q(&["palabraantigua"])).unwrap().total, 1);
    s.index_document(&doc(id, "Después", "P", "palabranueva"))
        .unwrap();
    assert_eq!(s.search(&q(&["palabraantigua"])).unwrap().total, 0);
    assert_eq!(ids(&s, &q(&["palabranueva"])), vec![id]);
    assert_eq!(s.search(&SearchQuery::default()).unwrap().total, 1);

    // Borrar la fila deja el FTS sin rastro (y su integridad se puede comprobar).
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    drop(conn);
    s.index_document(&doc(id, "Otra", "P", "palabrafinal"))
        .unwrap();
    assert_eq!(ids(&s, &q(&["palabrafinal"])), vec![id]);
}

#[test]
fn delete_from_index_removes_fts_entry() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("n.db");
    let mut s = Store::open(&path).unwrap();
    let id = add(&mut s, "Doc", "P", "palabrabuscada");
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute("DELETE FROM search_index WHERE task_id = ?1", [id])
        .unwrap();
    conn.execute(
        "INSERT INTO search_fts (search_fts) VALUES ('integrity-check')",
        [],
    )
    .unwrap();
    drop(conn);
    assert_eq!(s.search(&q(&["palabrabuscada"])).unwrap().total, 0);
}

#[test]
fn complete_indexed_is_atomic() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("n.db");
    let mut s = Store::open(&path).unwrap();
    let id = s.enqueue("invoice", "x").unwrap().id;
    s.claim(id).unwrap();
    // Rompe el índice desde fuera: el trigger fallará al insertar.
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch("DROP TABLE search_fts;").unwrap();
    drop(conn);
    let err = s.complete_indexed(id, &json!({"a": 1}), &doc(id, "T", "P", "b"));
    assert!(err.is_err());
    let task = s.task(id).unwrap();
    assert_eq!(task.state, TaskState::Running);
    assert!(task.result.is_none());
    assert_eq!(
        s.transitions(id).unwrap().len(),
        2,
        "creada y en curso, nada más"
    );
}

#[test]
fn indexed_variants_leave_task_and_index_together() {
    let mut s = Store::open_in_memory().unwrap();
    let id = s.enqueue("invoice", "x").unwrap().id;
    s.claim(id).unwrap();
    let t = s
        .await_review_indexed(id, &json!({"k": 1}), &doc(999, "Rev", "P", "revisable"))
        .unwrap();
    assert_eq!(t.state, TaskState::AwaitingReview);
    assert_eq!(
        ids(&s, &q(&["revisable"])),
        vec![id],
        "task_id sale de la tarea"
    );

    let closed = s
        .enqueue_closed_indexed(
            "email",
            "m",
            &json!({}),
            "cerrada",
            &doc(0, "Cerrado", "a@example.test", "cuerpo"),
        )
        .unwrap();
    assert_eq!(closed.state, TaskState::Completed);
    let review = s
        .enqueue_review_indexed(
            "email",
            "m2",
            &json!({}),
            "rebote",
            &doc(0, "Rebote", "b@example.test", ""),
        )
        .unwrap();
    assert_eq!(review.state, TaskState::AwaitingReview);
    assert_eq!(ids(&s, &q(&["cuerpo"])), vec![closed.id]);
    assert_eq!(ids(&s, &q(&["rebote"])), vec![review.id]);
}

#[test]
fn unindexed_and_status_counts() {
    let mut s = Store::open_in_memory().unwrap();
    let queued = s.enqueue("invoice", "q").unwrap().id;
    let running = s.enqueue("invoice", "r").unwrap().id;
    let done = s.enqueue("invoice", "d").unwrap().id;
    let mail = s.enqueue("email", "m").unwrap().id;
    let other = s.enqueue("document", "o").unwrap().id;
    for id in [running, done, mail, other] {
        s.claim(id).unwrap();
    }
    s.complete(done, &json!({})).unwrap();
    s.complete(mail, &json!({})).unwrap();
    s.complete(other, &json!({})).unwrap();

    let pending: Vec<i64> = s
        .unindexed_tasks(10, false)
        .unwrap()
        .iter()
        .map(|t| t.id)
        .collect();
    assert_eq!(
        pending,
        vec![done, mail],
        "ni en cola, ni en curso, ni otros tipos"
    );
    assert_eq!(s.unindexed_tasks(1, false).unwrap().len(), 1);
    assert_eq!(s.search(&SearchQuery::default()).unwrap().pending_index, 2);

    // Con `include_queued` entra también la tarea en cola, nunca la que está en curso.
    let with_queued: Vec<i64> = s
        .unindexed_tasks(10, true)
        .unwrap()
        .iter()
        .map(|t| t.id)
        .collect();
    assert_eq!(with_queued, vec![queued, done, mail]);
    assert!(!with_queued.contains(&running));
    let counted = s
        .search(&SearchQuery {
            include_queued: true,
            ..SearchQuery::default()
        })
        .unwrap();
    assert_eq!(counted.pending_index, 3);

    s.index_document(&SearchDoc {
        status: SearchStatus::NoText,
        ..doc(done, "Sin texto", "P", "")
    })
    .unwrap();
    s.index_document(&SearchDoc {
        status: SearchStatus::Failed,
        detail: "ocr".into(),
        kind: SearchKind::Email,
        ..doc(mail, "Falló", "P", "")
    })
    .unwrap();
    let page = s.search(&SearchQuery::default()).unwrap();
    assert_eq!((page.pending_index, page.no_text, page.failed), (0, 1, 1));
    assert!(s.unindexed_tasks(10, false).unwrap().is_empty());
    assert_eq!(s.unindexed_tasks(10, true).unwrap().len(), 1);
}

#[test]
fn pagination_cap_and_multibyte_snippet() {
    let mut s = Store::open_in_memory().unwrap();
    let long = "ñandú ".repeat(200);
    for _ in 0..(SEARCH_PAGE_SIZE + 5) {
        add(&mut s, "Doc", "P", &long);
    }
    let first = s.search(&q(&["ñandú"])).unwrap();
    assert_eq!(first.results.len(), SEARCH_PAGE_SIZE);
    assert_eq!(first.total, SEARCH_PAGE_SIZE + 5);
    let second = s
        .search(&SearchQuery {
            offset: SEARCH_PAGE_SIZE,
            ..q(&["ñandú"])
        })
        .unwrap();
    assert_eq!(second.results.len(), 5);
    for hit in first.results.iter().chain(&second.results) {
        assert!(!hit.snippet.is_empty());
        assert!(hit.snippet.chars().count() <= SNIPPET_CHARS);
    }
    // Sin términos, el fragmento sale del comienzo del cuerpo y también respeta el tope.
    let list = s.search(&SearchQuery::default()).unwrap();
    assert!(list.results[0].snippet.chars().count() <= SNIPPET_CHARS);
    assert!(list.results[0].snippet.starts_with("ñandú"));
}
