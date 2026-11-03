//! Verificación T05 sin PDF ni OCR: ausencias explícitas, aritmética decimal, importes
//! incoherentes y posibles duplicados con motivos. Todos los datos son sintéticos.

use std::path::{Path, PathBuf};

use nestadmin_documents::{
    process_invoice, validate, DocumentError, DocumentFormat, DocumentText, DuplicateReason,
    DuplicateRegistry, Evidenced, FieldError, FieldErrorKind, Invoice, IsoDate, Issue,
    LocalDocumentText, RawInvoice,
};
use rust_decimal::Decimal;
use serde_json::Value;

fn raw(fields: [Option<&str>; 6]) -> RawInvoice {
    let f = |v: Option<&str>| v.map(|s| Evidenced::new(s.to_owned()));
    RawInvoice {
        supplier: f(fields[0]),
        invoice_number: f(fields[1]),
        date: f(fields[2]),
        currency: None,
        subtotal: f(fields[3]),
        tax: f(fields[4]),
        total: f(fields[5]),
    }
}

fn invoice(fields: [Option<&str>; 6]) -> Invoice {
    let inv = Invoice::from_raw(&raw(fields));
    assert_eq!(inv.unreadable, vec![], "factura sintética válida");
    inv
}

#[test]
fn coherent_invoice_has_no_issues_and_keeps_evidence() {
    let mut source = raw([
        Some("Ejemplo Uno SL"),
        Some("A-1"),
        Some("2026-03-01"),
        Some("100.00"),
        Some("21.00"),
        Some("121.00"),
    ]);
    source.total = Some(Evidenced::with_evidence(
        "121.00".to_owned(),
        "Total: 121.00 EUR",
    ));
    let inv = Invoice::from_raw(&source);
    assert_eq!(validate(&inv), vec![]);
    let total = inv.total.as_ref().unwrap();
    assert_eq!(total.value, Decimal::new(12100, 2));
    assert_eq!(total.evidence.as_deref(), Some("Total: 121.00 EUR"));
}

#[test]
fn decimal_arithmetic_is_exact() {
    // 0.10 + 0.20 = 0.30 exacto; en coma flotante binaria fallaría la igualdad.
    let ok = invoice([
        Some("S"),
        Some("1"),
        None,
        Some("0.10"),
        Some("0.20"),
        Some("0.30"),
    ]);
    assert_eq!(validate(&ok), vec![]);
    // Un céntimo de diferencia es incoherente: no se tolera ni se redondea.
    let off = invoice([
        Some("S"),
        Some("2"),
        None,
        Some("33.33"),
        Some("7.00"),
        Some("40.34"),
    ]);
    assert_eq!(validate(&off), vec![Issue::TotalMismatch]);
}

#[test]
fn absent_values_are_reported_and_never_inferred() {
    let no_tax = invoice([
        Some("S"),
        Some("1"),
        None,
        Some("100.00"),
        None,
        Some("121.00"),
    ]);
    assert_eq!(validate(&no_tax), vec![Issue::MissingTax]);
    assert_eq!(no_tax.tax, None, "el IVA no se deduce de base y total");

    let no_total = invoice([
        Some("S"),
        Some("2"),
        None,
        Some("75.00"),
        Some("15.75"),
        None,
    ]);
    assert_eq!(validate(&no_total), vec![Issue::MissingTotal]);
    assert_eq!(no_total.total, None, "el total no se calcula");

    let nothing = invoice([None, None, None, None, None, None]);
    assert_eq!(
        validate(&nothing),
        vec![Issue::MissingTax, Issue::MissingTotal]
    );
    let fields = nothing.contract_fields();
    assert!(fields.iter().all(|(_, v)| v.is_none()), "{fields:?}");

    // Texto vacío equivale a ausencia, como en el evaluador.
    let blank = invoice([Some("  "), None, Some(""), None, Some(" "), None]);
    assert_eq!(blank.supplier, None);
    assert_eq!(blank.date, None);
    assert_eq!(blank.tax, None);
}

#[test]
fn unreadable_amount_or_date_is_a_field_error_not_a_zero() {
    let mut source = raw([
        Some("Ejemplo SL"),
        Some("A-1"),
        None,
        Some("100.00"),
        Some("21,00"),
        Some("121.00"),
    ]);
    source.tax = Some(Evidenced::with_evidence("21,00".to_owned(), "IVA: 21,00"));
    let inv = Invoice::from_raw(&source);
    // El campo ilegible no consta y queda anotado con su literal; el resto se conserva.
    assert_eq!(inv.tax, None);
    assert!(inv.is_unreadable("tax"));
    assert_eq!(
        inv.unreadable,
        vec![FieldError {
            field: "tax".to_owned(),
            kind: FieldErrorKind::InvalidAmount,
            literal: "21,00".to_owned(),
            evidence: Some("IVA: 21,00".to_owned()),
        }]
    );
    assert_eq!(inv.unreadable[0].code(), "invalid_amount");
    assert_eq!(inv.subtotal.as_ref().unwrap().value, Decimal::new(10000, 2));
    assert_eq!(inv.total.as_ref().unwrap().value, Decimal::new(12100, 2));
    assert_eq!(inv.supplier.as_ref().unwrap().value, "Ejemplo SL");
    // Sin IVA utilizable se pide revisión; no se compara base + IVA con el total.
    assert_eq!(validate(&inv), vec![Issue::MissingTax]);

    // Se anotan todos los campos ilegibles, no solo el primero.
    let inv = Invoice::from_raw(&raw([
        None,
        None,
        Some("31/01/2026"),
        Some("1.000,00"),
        None,
        Some("abc"),
    ]));
    let got: Vec<_> = inv
        .unreadable
        .iter()
        .map(|e| (e.field.as_str(), e.code()))
        .collect();
    assert_eq!(
        got,
        [
            ("date", "invalid_date"),
            ("subtotal", "invalid_amount"),
            ("total", "invalid_amount")
        ]
    );
    assert_eq!((inv.date, inv.subtotal, inv.total), (None, None, None));

    // Los mismos errores siguen disponibles como función de interpretación directa.
    let err = IsoDate::parse("date", "31/01/2026").unwrap_err();
    assert!(
        matches!(&err, DocumentError::InvalidDate { field: "date", literal } if literal == "31/01/2026")
    );
}

#[test]
fn unreadable_field_still_registers_and_detects_duplicates() {
    let mut registry = DuplicateRegistry::new();
    let first = invoice([
        Some("Ejemplo SL"),
        Some("X-1"),
        Some("2026-05-01"),
        Some("10.00"),
        Some("2.10"),
        Some("12.10"),
    ]);
    process_invoice(&mut registry, "a", &first);

    // Misma factura con el IVA mal escrito: se registra y se señala como duplicado junto
    // al error del campo.
    let garbled = Invoice::from_raw(&raw([
        Some("Ejemplo SL"),
        Some("X-1"),
        Some("2026-05-01"),
        Some("10.00"),
        Some("2,10"),
        Some("12.10"),
    ]));
    let report = process_invoice(&mut registry, "b", &garbled);
    assert_eq!(
        report.issues,
        vec![Issue::MissingTax, Issue::PossibleDuplicate]
    );
    assert_eq!(report.duplicates[0].existing_id, "a");
    assert_eq!(
        report.duplicates[0].reasons,
        vec![
            DuplicateReason::SameSupplierAndNumber,
            DuplicateReason::SameSupplierDateAndTotal
        ]
    );
    assert_eq!(report.unreadable.len(), 1);
    assert_eq!(report.unreadable[0].field, "tax");
    assert_eq!(report.unreadable[0].literal, "2,10");
    assert_eq!(registry.len(), 2);

    // Las facturas posteriores también se comparan con la de campo ilegible.
    let later = Invoice::from_raw(&raw([
        Some("Ejemplo SL"),
        Some("X-1"),
        None,
        None,
        None,
        None,
    ]));
    let report = process_invoice(&mut registry, "c", &later);
    let ids: Vec<_> = report
        .duplicates
        .iter()
        .map(|c| c.existing_id.as_str())
        .collect();
    assert_eq!(ids, ["a", "b"]);
}

#[test]
fn currency_is_kept_with_evidence_and_never_assumed() {
    // Moneda explícita del extractor.
    let mut source = raw([None, None, None, Some("10.00"), Some("2.10"), Some("12.10")]);
    source.currency = Some(Evidenced::with_evidence("€".to_owned(), "Importes en €"));
    let inv = Invoice::from_raw(&source);
    assert_eq!(
        inv.currency,
        Some(Evidenced::with_evidence("EUR".to_owned(), "Importes en €"))
    );

    // Sin moneda explícita se toma del sufijo de un importe legible, con su literal.
    let inv = invoice([
        None,
        None,
        None,
        Some("10.00"),
        Some("2.10"),
        Some("12.10 EUR"),
    ]);
    assert_eq!(
        inv.currency,
        Some(Evidenced::with_evidence("EUR".to_owned(), "12.10 EUR"))
    );

    // Sin indicio en el documento la moneda no consta; no se supone el euro.
    let inv = invoice([None, None, None, Some("10.00"), Some("2.10"), Some("12.10")]);
    assert_eq!(inv.currency, None);

    // Un importe ilegible no sirve como indicio de moneda.
    let inv = Invoice::from_raw(&raw([None, None, None, None, None, Some("12,10 EUR")]));
    assert_eq!(inv.currency, None);

    // Otra moneda es un error del campo, nunca una conversión ni un euro supuesto.
    let mut source = raw([None, None, None, None, None, Some("12.10 EUR")]);
    source.currency = Some(Evidenced::new("USD".to_owned()));
    let inv = Invoice::from_raw(&source);
    assert_eq!(inv.currency, None);
    assert_eq!(inv.unreadable[0].field, "currency");
    assert_eq!(inv.unreadable[0].code(), "unsupported_currency");
    assert_eq!(inv.total.as_ref().unwrap().value, Decimal::new(1210, 2));
}

#[test]
fn reprocessing_the_same_id_is_idempotent() {
    let mut registry = DuplicateRegistry::new();
    let first = invoice([
        Some("Ejemplo SL"),
        Some("X-1"),
        Some("2026-05-01"),
        Some("10.00"),
        Some("2.10"),
        Some("12.10"),
    ]);
    let other = invoice([
        Some("Otro SL"),
        Some("Y-1"),
        Some("2026-05-02"),
        Some("5.00"),
        Some("1.05"),
        Some("6.05"),
    ]);
    let initial = process_invoice(&mut registry, "a", &first);
    process_invoice(&mut registry, "b", &other);

    // Un trabajo recuperado vuelve a procesar "a": no es duplicado de sí mismo, el
    // resultado coincide con el primero y el registro no crece.
    let again = process_invoice(&mut registry, "a", &first);
    assert_eq!(again, initial);
    assert!(again.duplicates.is_empty());
    assert_eq!(registry.len(), 2);

    // Una copia real con otro id sí se señala, una sola vez y frente a "a".
    let copy = process_invoice(&mut registry, "c", &first);
    assert_eq!(copy.issues, vec![Issue::PossibleDuplicate]);
    assert_eq!(copy.duplicates.len(), 1);
    assert_eq!(copy.duplicates[0].existing_id, "a");

    // Reintentar "c" da el mismo resultado sin añadir entradas.
    assert_eq!(process_invoice(&mut registry, "c", &first), copy);
    assert_eq!(registry.len(), 3);

    // Reprocesar "a" con datos corregidos sustituye su entrada en el mismo lugar: "c" ya
    // no coincide con la nueva "a" por número, solo por proveedor, fecha y total.
    let corrected = invoice([
        Some("Ejemplo SL"),
        Some("X-9"),
        Some("2026-05-01"),
        Some("10.00"),
        Some("2.10"),
        Some("12.10"),
    ]);
    process_invoice(&mut registry, "a", &corrected);
    assert_eq!(registry.len(), 3);
    let check = registry.candidates(&first);
    assert_eq!(check[0].existing_id, "a");
    assert_eq!(
        check[0].reasons,
        vec![DuplicateReason::SameSupplierDateAndTotal]
    );
}

#[test]
fn overflowing_sum_is_a_total_mismatch() {
    // Decimal::MAX con dos decimales: la suma desborda y no puede confirmar el total.
    let max = "792281625142643375935439503.35";
    let inv = invoice([None, None, None, Some(max), Some("1.00"), Some(max)]);
    assert_eq!(validate(&inv), vec![Issue::TotalMismatch]);

    // Si la suma no cabe con dos decimales, `rust_decimal` la redondearía a menos
    // decimales: 503.35 + 0.06 = 503.41 no puede darse por igual a 503.40.
    let inv = invoice([
        None,
        None,
        None,
        Some(max),
        Some("0.06"),
        Some("792281625142643375935439503.40"),
    ]);
    assert_eq!(validate(&inv), vec![Issue::TotalMismatch]);
}

#[test]
fn same_supplier_date_and_total_without_number_is_flagged() {
    // Duplicado escaneado de nuevo sin número legible en ninguna de las dos copias.
    let mut registry = DuplicateRegistry::new();
    let scan = invoice([
        Some("Ejemplo SL"),
        None,
        Some("2026-05-01"),
        Some("10.00"),
        Some("2.10"),
        Some("12.10"),
    ]);
    process_invoice(&mut registry, "a", &scan);
    let report = process_invoice(&mut registry, "b", &scan);
    assert_eq!(report.issues, vec![Issue::PossibleDuplicate]);
    assert_eq!(
        report.duplicates[0].reasons,
        vec![DuplicateReason::SameSupplierDateAndTotal]
    );
}

#[test]
fn century_leap_year_rule() {
    assert_eq!(
        IsoDate::parse("date", "1900-02-29").unwrap_err().code(),
        "invalid_date"
    );
    assert_eq!(
        IsoDate::parse("date", "2000-02-29").unwrap().to_string(),
        "2000-02-29"
    );
}

#[test]
fn total_mismatch_and_possible_duplicate_together() {
    let mut registry = DuplicateRegistry::new();
    let wrong = invoice([
        Some("Ejemplo SL"),
        Some("X-1"),
        Some("2026-05-01"),
        Some("10.00"),
        Some("2.10"),
        Some("12.00"),
    ]);
    let report = process_invoice(&mut registry, "a", &wrong);
    assert_eq!(report.issues, vec![Issue::TotalMismatch]);
    let report = process_invoice(&mut registry, "b", &wrong);
    assert_eq!(
        report.issues,
        vec![Issue::TotalMismatch, Issue::PossibleDuplicate]
    );
    assert_eq!(report.duplicates[0].existing_id, "a");
}

#[test]
fn contract_fields_use_two_decimals_and_iso_dates() {
    let inv = invoice([
        Some(" Ejemplo SL "),
        Some("B-7"),
        Some("2026-02-28"),
        Some("10.00 EUR"),
        Some("2.10€"),
        Some("12.10"),
    ]);
    let fields: Vec<_> = inv
        .contract_fields()
        .into_iter()
        .map(|(_, v)| v.unwrap())
        .collect();
    assert_eq!(
        fields,
        ["Ejemplo SL", "B-7", "2026-02-28", "10.00", "2.10", "12.10"]
    );
}

#[test]
fn duplicates_are_flagged_with_reasons_and_never_discarded() {
    let mut registry = DuplicateRegistry::new();
    let first = invoice([
        Some("Ejemplo SL"),
        Some("X-1"),
        Some("2026-05-01"),
        Some("10.00"),
        Some("2.10"),
        Some("12.10"),
    ]);
    assert!(process_invoice(&mut registry, "a", &first)
        .duplicates
        .is_empty());

    // Mismo proveedor (otra grafía) y número, distinto importe.
    let same_number = invoice([
        Some("EJEMPLO  sl"),
        Some("x-1"),
        Some("2026-06-01"),
        Some("20.00"),
        Some("4.20"),
        Some("24.20"),
    ]);
    let report = process_invoice(&mut registry, "b", &same_number);
    assert_eq!(report.issues, vec![Issue::PossibleDuplicate]);
    assert_eq!(report.duplicates.len(), 1);
    assert_eq!(report.duplicates[0].existing_id, "a");
    assert_eq!(
        report.duplicates[0].reasons,
        vec![DuplicateReason::SameSupplierAndNumber]
    );

    // Mismo proveedor, fecha y total con otro número.
    let same_amount = invoice([
        Some("Ejemplo SL"),
        Some("X-2"),
        Some("2026-05-01"),
        Some("10.00"),
        Some("2.10"),
        Some("12.10"),
    ]);
    let report = process_invoice(&mut registry, "c", &same_amount);
    assert_eq!(report.duplicates[0].existing_id, "a");
    assert_eq!(
        report.duplicates[0].reasons,
        vec![DuplicateReason::SameSupplierDateAndTotal]
    );

    // Copia exacta de la primera: ambos motivos frente a "a", el número frente a "b" e
    // importe y fecha frente a "c".
    let report = process_invoice(&mut registry, "d", &first);
    let ids: Vec<_> = report
        .duplicates
        .iter()
        .map(|c| c.existing_id.as_str())
        .collect();
    assert_eq!(ids, ["a", "b", "c"]);
    assert_eq!(
        report.duplicates[0].reasons,
        vec![
            DuplicateReason::SameSupplierAndNumber,
            DuplicateReason::SameSupplierDateAndTotal
        ]
    );

    // Todas siguen registradas.
    assert_eq!(registry.len(), 4);
}

#[test]
fn absent_fields_never_produce_a_duplicate_match() {
    let mut registry = DuplicateRegistry::new();
    let partial = invoice([
        None,
        Some("X-1"),
        Some("2026-05-01"),
        None,
        None,
        Some("12.10"),
    ]);
    process_invoice(&mut registry, "a", &partial);
    // Sin proveedor no hay coincidencia, aunque número, fecha y total sean iguales.
    assert!(registry.candidates(&partial).is_empty());

    let other = invoice([Some("Otro SL"), None, Some("2026-05-01"), None, None, None]);
    process_invoice(&mut registry, "b", &other);
    let again = invoice([Some("Otro SL"), None, Some("2026-05-01"), None, None, None]);
    assert!(
        registry.candidates(&again).is_empty(),
        "sin número ni total no hay motivo"
    );

    // Proveedores distintos con el mismo número no son duplicados.
    let different = invoice([Some("Tercero SL"), Some("X-1"), None, None, None, None]);
    assert!(registry.candidates(&different).is_empty());
}

#[test]
fn plain_text_is_read_as_utf8_and_pdf_is_not_supported_yet() {
    let dir = tempfile::tempdir().unwrap();
    let txt = dir.path().join("factura.TXT");
    std::fs::write(&txt, "\u{feff}Factura Z-1. Proveedor: Ñandú SL.").unwrap();
    let extracted = LocalDocumentText.extract_text(&txt).unwrap();
    assert_eq!(extracted.format, DocumentFormat::PlainText);
    assert_eq!(extracted.text, "Factura Z-1. Proveedor: Ñandú SL.");

    let latin1 = dir.path().join("latin1.txt");
    std::fs::write(&latin1, [b'F', 0xE1, b'c']).unwrap();
    let err = LocalDocumentText.extract_text(&latin1).unwrap_err();
    assert_eq!(err.code(), "invalid_utf8");

    let empty = dir.path().join("vacio.txt");
    std::fs::write(&empty, " \n\t").unwrap();
    assert_eq!(
        LocalDocumentText.extract_text(&empty).unwrap_err().code(),
        "empty_text"
    );

    let missing = dir.path().join("no-existe.txt");
    assert_eq!(
        LocalDocumentText.extract_text(&missing).unwrap_err().code(),
        "io"
    );

    // El PDF no se abre siquiera: el error es de formato, no de lectura.
    let pdf = dir.path().join("no-existe.pdf");
    let err = LocalDocumentText.extract_text(&pdf).unwrap_err();
    assert!(matches!(&err, DocumentError::UnsupportedFormat { format } if format == "pdf"));
    assert_eq!(err.to_string(), "formato pdf no soportado todavía");

    let other = Path::new("imagen.png");
    assert_eq!(
        LocalDocumentText.extract_text(other).unwrap_err().code(),
        "unsupported_format"
    );
}

fn evaluation_file(kind: &str) -> Value {
    let path: PathBuf = [
        env!("CARGO_MANIFEST_DIR"),
        "..",
        "..",
        "evaluation",
        "data",
        kind,
        "invoices.json",
    ]
    .iter()
    .collect();
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// Recorre el corpus en el orden de `inputs` con un registro vacío. Los campos proceden de
/// `expected` porque aún no hay extracción con IA: la prueba verifica validación y
/// duplicados, no la extracción. `expected` nunca se entrega a un modelo.
#[test]
fn evaluation_corpus_labels_match_expected_in_order() {
    let inputs = evaluation_file("inputs");
    let expected = evaluation_file("expected");
    let expected_by_id = |id: &str| {
        expected
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["id"] == id)
            .unwrap_or_else(|| panic!("{id} sin resultado esperado"))["expected"]
            .clone()
    };

    let mut registry = DuplicateRegistry::new();
    let mut checked = Vec::new();
    for case in inputs.as_array().unwrap() {
        let id = case["id"].as_str().unwrap();
        let gold = expected_by_id(id);
        let field = |name: &str| {
            gold["fields"][name]
                .as_str()
                .map(|s| Evidenced::new(s.to_owned()))
        };
        let source = RawInvoice {
            supplier: field("supplier"),
            invoice_number: field("invoice_number"),
            date: field("date"),
            currency: None,
            subtotal: field("subtotal"),
            tax: field("tax"),
            total: field("total"),
        };
        let inv = Invoice::from_raw(&source);
        assert_eq!(inv.unreadable, vec![], "{id}");

        // La normalización conserva exactamente los campos del contrato.
        for (name, value) in inv.contract_fields() {
            assert_eq!(
                value.as_deref(),
                gold["fields"][name].as_str(),
                "{id}.{name}"
            );
        }

        let report = process_invoice(&mut registry, id, &inv);
        let mut got: Vec<_> = report.issues.iter().map(|i| i.as_str()).collect();
        got.sort_unstable();
        let mut want: Vec<_> = gold["issues"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        want.sort_unstable();
        assert_eq!(got, want, "{id}");

        if id == "D09" {
            assert_eq!(report.duplicates.len(), 1);
            assert_eq!(report.duplicates[0].existing_id, "D01");
            assert_eq!(
                report.duplicates[0].reasons,
                vec![
                    DuplicateReason::SameSupplierAndNumber,
                    DuplicateReason::SameSupplierDateAndTotal
                ]
            );
        } else {
            assert!(report.duplicates.is_empty(), "{id}");
        }
        checked.push(id.to_owned());
    }
    assert_eq!(checked.len(), 10);
    assert_eq!(registry.len(), 10, "ninguna factura se descarta");
}
