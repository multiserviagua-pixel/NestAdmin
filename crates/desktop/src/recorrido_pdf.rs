//! PDF de los documentos de venta y membrete desde la ventana (E4a, D222, D223; D197). Prueba
//! independiente de `recorrido_venta.rs`: reutiliza su sesión simulada de Tauri, pero con el
//! PDFium real (`vendor/pdfium/bin` hacia arriba desde el crate o `NESTADMIN_PDFIUM_DIR`; se
//! instala con `scripts/setup-ocr.ps1`). Datos sintéticos y carpeta temporal. No llama a
//! `sales_open_document_pdf`, que abriría el visor del sistema: solo se comprueba que el comando
//! está registrado (`recorrido.rs` contrasta `ui/src/api.ts` con las listas de comandos).

use std::path::PathBuf;

use nestadmin_extract::pdfium_render::prelude::*;
use nestadmin_extract::Extractor;
use serde_json::{json, Value};
use tauri::ipc::InvokeBody;

use super::recorrido_venta::{draft, master_data, Session, PRODUCER};

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

/// PDF sintético de membrete con `pages` páginas A4 verticales.
fn letterhead(pages: usize) -> Vec<u8> {
    let extractor = Extractor::new(pdfium_dir(), Box::new(NoOcr));
    let pdfium = extractor.pdfium().unwrap();
    let mut document = pdfium.create_new_pdf().unwrap();
    let font = document.fonts_mut().helvetica();
    for i in 0..pages {
        let mut page = document
            .pages_mut()
            .create_page_at_end(PdfPagePaperSize::a4())
            .unwrap();
        page.objects_mut()
            .create_text_object(
                PdfPoints::new(30.0),
                PdfPoints::new(820.0),
                format!("MEMBRETE SINTETICO {}", i + 1),
                font,
                PdfPoints::new(10.0),
            )
            .unwrap();
    }
    document.save_to_bytes().unwrap()
}

struct NoOcr;

impl nestadmin_extract::Ocr for NoOcr {
    fn recognize(
        &self,
        _image: &std::path::Path,
        page: usize,
    ) -> Result<String, nestadmin_extract::ExtractError> {
        Err(nestadmin_extract::ExtractError::OcrUnavailable(format!(
            "página {page}"
        )))
    }
}

fn set_letterhead(s: &Session, bytes: Vec<u8>) -> Result<Value, Value> {
    s.request(
        "sales_set_letterhead",
        InvokeBody::Raw(bytes),
        Default::default(),
    )
    .map(|body| body.deserialize::<Value>().unwrap())
}

fn error_text(error: Value) -> String {
    error
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| error.to_string())
}

fn pdf_text(path: &str) -> String {
    let extractor = Extractor::new(pdfium_dir(), Box::new(NoOcr));
    let pdfium = extractor.pdfium().unwrap();
    let bytes = std::fs::read(path).unwrap();
    let document = pdfium.load_pdf_from_byte_slice(&bytes, None).unwrap();
    document
        .pages()
        .iter()
        .map(|p| p.text().unwrap().all())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn el_membrete_y_el_pdf_de_un_documento_emitido_desde_la_ventana() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("datos");
    let s = Session::open_with_pdfium(&data, &pdfium_dir());
    let (customer, product) = master_data(&s);
    s.write_producer(PRODUCER);
    s.ok("reload_verifactu_producer", json!({}));

    // Sin membrete: márgenes de fábrica y nada que quitar.
    let company = s.ok("sales_company", json!({}));
    assert_eq!(company["has_letterhead"], false);
    assert_eq!(
        company["letterhead_margins"],
        json!({ "top_mm": 35, "right_mm": 20, "bottom_mm": 25, "left_mm": 20 })
    );

    // Un PDF no válido se rechaza con su motivo y no se guarda.
    for (bytes, reason) in [
        (b"esto no es un PDF".to_vec(), "no es un PDF válido"),
        (letterhead(3), "una o dos páginas"),
    ] {
        let error = error_text(set_letterhead(&s, bytes).unwrap_err());
        assert!(error.starts_with("[invalid_input] "), "{error}");
        assert!(error.contains(reason), "{error}");
    }
    assert_eq!(s.ok("sales_company", json!({}))["has_letterhead"], false);
    // El membrete debe llegar como datos binarios.
    s.fails(
        "sales_set_letterhead",
        json!({ "letterhead": "texto" }),
        "invalid_input",
    );

    // Uno válido se guarda; el contenido no viaja a la ventana.
    set_letterhead(&s, letterhead(2)).unwrap();
    let company = s.ok("sales_company", json!({}));
    assert_eq!(company["has_letterhead"], true);
    assert!(company.get("letterhead").is_none());

    // Márgenes: se validan y se guardan sin tocar los datos de la empresa.
    s.fails(
        "sales_set_letterhead_margins",
        json!({ "margins": { "top_mm": 101, "right_mm": 20, "bottom_mm": 25, "left_mm": 20 } }),
        "invalid_input",
    );
    s.ok(
        "sales_set_letterhead_margins",
        json!({ "margins": { "top_mm": 40, "right_mm": 15, "bottom_mm": 30, "left_mm": 18 } }),
    );
    let company = s.ok("sales_company", json!({}));
    assert_eq!(
        company["letterhead_margins"],
        json!({ "top_mm": 40, "right_mm": 15, "bottom_mm": 30, "left_mm": 18 })
    );
    assert_eq!(company["name"], "Taller Demo SL");

    // Un borrador no tiene PDF.
    let quote = draft(&s, "quote", customer, product);
    s.fails("sales_document_pdf", json!({ "id": quote }), "conflict");

    // Presupuesto y factura emitidos con el membrete: su PDF lleva el fondo y los datos.
    s.ok("sales_issue", json!({ "id": quote }));
    let quote_pdf = s
        .ok("sales_document_pdf", json!({ "id": quote }))
        .as_str()
        .unwrap()
        .to_owned();
    assert!(quote_pdf.ends_with("P-2026-0001.pdf"), "{quote_pdf}");
    assert!(PathBuf::from(&quote_pdf).starts_with(&data));
    let text = pdf_text(&quote_pdf);
    assert!(text.contains("MEMBRETE SINTETICO 1"), "{text}");
    assert!(text.contains("PRESUPUESTO Nº P-2026-0001"), "{text}");
    assert!(text.contains("96,80 €"), "{text}");

    let invoice = draft(&s, "invoice", customer, product);
    s.ok("sales_issue", json!({ "id": invoice }));
    let invoice_pdf = s
        .ok("sales_document_pdf", json!({ "id": invoice }))
        .as_str()
        .unwrap()
        .to_owned();
    let text = pdf_text(&invoice_pdf);
    assert!(text.contains("VERI*FACTU"), "{text}");
    assert!(text.contains("FACTURA Nº F-2026-0001"), "{text}");
    // Pedirlo de nuevo devuelve la misma ruta sin regenerar.
    assert_eq!(
        s.ok("sales_document_pdf", json!({ "id": invoice })),
        json!(invoice_pdf)
    );

    // Quitar el membrete no altera los documentos ya emitidos: se regenera con su membrete.
    s.ok("sales_remove_letterhead", json!({}));
    assert_eq!(s.ok("sales_company", json!({}))["has_letterhead"], false);
    std::fs::remove_file(&invoice_pdf).unwrap();
    s.ok("sales_document_pdf", json!({ "id": invoice }));
    assert!(pdf_text(&invoice_pdf).contains("FACTURA Nº F-2026-0001"));
    // Una factura nueva ya sale sin membrete.
    let plain = draft(&s, "invoice", customer, product);
    s.ok("sales_issue", json!({ "id": plain }));
    let plain_pdf = s
        .ok("sales_document_pdf", json!({ "id": plain }))
        .as_str()
        .unwrap()
        .to_owned();
    assert!(!pdf_text(&plain_pdf).contains("MEMBRETE"));
    s.close();
}
