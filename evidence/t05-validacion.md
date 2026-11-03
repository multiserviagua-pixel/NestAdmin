# Evidencia T05 (parcial): validación de facturas sin PDF ni OCR

Cubre la parte de R05 que no depende de la extracción con IA, del PDF ni del OCR: modelo de factura con ausencia explícita y moneda, importes con `rust_decimal` (D04), comprobación exacta base + IVA = total, campos ilegibles anotados sin perder el resto de la factura, aviso de posibles duplicados con motivos e idempotente por identificador, y lectura de texto plano UTF-8, en `crates/documents`. Todos los datos de prueba son sintéticos.

## Cómo reproducir

```sh
cargo test -p nestadmin-documents --locked
```

## Ejecución registrada

- Fecha: 2026-11-03
- Plataforma: Windows 11 Pro 10.0.26300, Intel Core Ultra 5 125U, 16 GB
- Rust 1.99.1 (MSVC), `rust_decimal` 1.43.0
- Resultado: 18 pruebas superadas (2 unitarias y 16 de integración), 0 fallidas.

| Prueba | Qué comprueba |
| --- | --- |
| `amounts_accept_only_two_decimals_with_point` | Importes en euros con punto y dos decimales; coma decimal, miles, otra precisión o texto son `invalid_amount`. |
| `dates_must_exist_in_the_calendar` | Fechas ISO con día existente (2024-02-29 válido); cualquier otro formato es `invalid_date`. |
| `century_leap_year_rule` | Regla secular de bisiestos: 1900-02-29 se rechaza y 2000-02-29 se acepta. |
| `coherent_invoice_has_no_issues_and_keeps_evidence` | Una factura coherente no tiene incidencias y conserva la evidencia literal. |
| `decimal_arithmetic_is_exact` | 0.10 + 0.20 = 0.30 exacto; un céntimo de diferencia es `total_mismatch`. |
| `overflowing_sum_is_a_total_mismatch` | Cerca de `Decimal::MAX` la suma que desborda o que `rust_decimal` redondearía a menos decimales (503.35 + 0.06 frente a 503.40) es `total_mismatch`, nunca una igualdad aproximada. |
| `absent_values_are_reported_and_never_inferred` | `missing_tax` y `missing_total` sin deducir el valor; los campos ausentes quedan `None`. |
| `unreadable_amount_or_date_is_a_field_error_not_a_zero` | Un importe o una fecha ilegibles dejan el campo en `None` y quedan en `unreadable` con campo, código, literal y evidencia; se anotan todos, no solo el primero, y el resto de campos se conserva. Un IVA o total ilegible también se señala como ausente. |
| `unreadable_field_still_registers_and_detects_duplicates` | Una copia con el IVA ilegible se registra, se señala como `possible_duplicate` junto al error del campo y las facturas posteriores se comparan con ella. |
| `currency_is_kept_with_evidence_and_never_assumed` | Moneda explícita o tomada del sufijo de un importe legible, con evidencia; sin indicio queda `None`; otra moneda es `unsupported_currency`. |
| `contract_fields_use_two_decimals_and_iso_dates` | Salida normalizada en el formato del contrato del evaluador. |
| `duplicates_are_flagged_with_reasons_and_never_discarded` | Candidatos con motivo (mismo proveedor y número; mismo proveedor, fecha y total); todas las facturas siguen registradas. |
| `same_supplier_date_and_total_without_number_is_flagged` | Dos copias sin número legible con el mismo proveedor, fecha y total se señalan por ese motivo. |
| `total_mismatch_and_possible_duplicate_together` | Una factura incoherente repetida recibe ambas etiquetas. |
| `reprocessing_the_same_id_is_idempotent` | Reprocesar el mismo identificador (trabajo recuperado o reintentado) da el mismo resultado, no se señala como duplicado de sí mismo y no hace crecer el registro; datos corregidos sustituyen la entrada en su lugar. |
| `absent_fields_never_produce_a_duplicate_match` | Un campo ausente no coincide con nada; proveedores distintos no son duplicados. |
| `plain_text_is_read_as_utf8_and_pdf_is_not_supported_yet` | `.txt` UTF-8 (sin BOM); UTF-8 inválido, vacío y archivo inexistente son errores explícitos; PDF devuelve `unsupported_format`. |
| `evaluation_corpus_labels_match_expected_in_order` | Recorre `evaluation/data/inputs/invoices.json` en orden con un registro vacío, usando como campos los de `expected` (solo en la prueba), y obtiene las mismas etiquetas, incluida D09 como duplicado de D01 por ambos motivos. |

## Alcance respecto al contrato

- La moneda (contratos.md, D04) se conserva en `Invoice::currency` con su evidencia. Solo se admite el euro; otra moneda es un error explícito del campo, sin conversión. No forma parte de los seis campos del contrato del evaluador, que no cambia.
- Las etiquetas del evaluador siguen siendo las cuatro previstas; un campo ilegible no añade etiqueta nueva y se informa aparte en `ValidationReport::unreadable`.

## Qué falta para completar T05

- Extracción real de los campos con IA (Ollama/Groq, fase 4): la prueba del corpus toma los campos de las expectativas y por tanto **no mide la extracción**; solo valida etiquetas y duplicados sobre campos correctos.
- PDF con texto mediante PDFium (D09) y facturas escaneadas mediante OCR local (D10, T04).
- Persistencia del registro de duplicados en la base local y su presentación en la interfaz para revisión. El registro en memoria es idempotente por identificador, pero no sobrevive a un reinicio.
- Ejecución en CI: no se registra resultado aquí hasta que exista una ejecución terminada.
