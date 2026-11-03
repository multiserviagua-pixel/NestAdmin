# Evidencia T05 (parcial): validación de facturas sin PDF ni OCR

Cubre la parte de R05 que no depende de la extracción con IA, del PDF ni del OCR: modelo de factura con ausencia explícita, importes con `rust_decimal` (D04), comprobación base + IVA = total, aviso de posibles duplicados con motivos y lectura de texto plano UTF-8, en `crates/documents`. Todos los datos de prueba son sintéticos.

## Cómo reproducir

```sh
cargo test -p nestadmin-documents
```

## Ejecución registrada

- Fecha: 2026-11-03
- Plataforma: Windows 11 Pro 10.0.26300, Intel Core Ultra 5 125U, 16 GB
- Rust 1.99.1 (MSVC), `rust_decimal` 1.43.0
- Resultado: 11 pruebas superadas (2 unitarias y 9 de integración), 0 fallidas. CI repite la ejecución en Linux y Windows.

| Prueba | Qué comprueba |
| --- | --- |
| `amounts_accept_only_two_decimals_with_point` | Importes en euros con punto y dos decimales; coma decimal, miles, otra precisión o texto son `invalid_amount`. |
| `dates_must_exist_in_the_calendar` | Fechas ISO con día existente (bisiestos incluidos); cualquier otro formato es `invalid_date`. |
| `coherent_invoice_has_no_issues_and_keeps_evidence` | Una factura coherente no tiene incidencias y conserva la evidencia literal. |
| `decimal_arithmetic_is_exact` | 0.10 + 0.20 = 0.30 exacto; un céntimo de diferencia es `total_mismatch`. |
| `absent_values_are_reported_and_never_inferred` | `missing_tax` y `missing_total` sin deducir el valor; los campos ausentes quedan `None`. |
| `unreadable_amount_or_date_is_an_error_not_a_zero` | Un importe o una fecha ilegibles son un error con el campo y el literal, nunca cero. |
| `contract_fields_use_two_decimals_and_iso_dates` | Salida normalizada en el formato del contrato del evaluador. |
| `duplicates_are_flagged_with_reasons_and_never_discarded` | Candidatos con motivo (mismo proveedor y número; mismo proveedor, fecha y total); todas las facturas siguen registradas. |
| `absent_fields_never_produce_a_duplicate_match` | Un campo ausente no coincide con nada; proveedores distintos no son duplicados. |
| `plain_text_is_read_as_utf8_and_pdf_is_not_supported_yet` | `.txt` UTF-8 (sin BOM); UTF-8 inválido, vacío y archivo inexistente son errores explícitos; PDF devuelve `unsupported_format`. |
| `evaluation_corpus_labels_match_expected_in_order` | Recorre `evaluation/data/inputs/invoices.json` en orden con un registro vacío, usando como campos los de `expected` (solo en la prueba), y obtiene las mismas etiquetas, incluida D09 como duplicado de D01 por ambos motivos. |

## Qué falta para completar T05

- Extracción real de los campos con IA (Ollama/Groq, fase 4): la prueba del corpus toma los campos de las expectativas y por tanto **no mide la extracción**; solo valida etiquetas y duplicados sobre campos correctos.
- PDF con texto mediante PDFium (D09) y facturas escaneadas mediante OCR local (D10, T04).
- Persistencia del registro de duplicados en la base local y su presentación en la interfaz para revisión.
