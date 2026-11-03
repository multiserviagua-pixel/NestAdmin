# Evidencia T04 (parcial): texto, PDF con texto y PDF escaneado con OCR local

Cubre R04 con documentos sintéticos generados en las propias pruebas (no hay PDF de ejemplo en el repositorio). La extracción está en `crates/extract`: texto plano UTF-8, PDF mediante PDFium (D09) y, por cada página con menos de 20 caracteres visibles, rasterizado a 300 ppp y OCR con Tesseract 5 como proceso local con `spa+eng` (D10). La capa de aplicación la ejecuta en `Job::run`, fuera del cerrojo, antes de llamar al modelo.

Si una página falla (o el OCR no reconoce nada en una página escaneada), el documento entero falla con el motivo y un código: nunca se pasa al modelo una extracción incompleta.

| Código | Cuándo | Reintentable |
| --- | --- | --- |
| `pdf_unavailable` | No se puede cargar `pdfium.dll`. | Sí |
| `ocr_unavailable` | Falta Tesseract o los datos de idioma. | Sí |
| `ocr_failed` | Tesseract falla en una página o no reconoce texto en ella; indica la página. | Sí |
| `document_unreadable` | PDF dañado o protegido, texto que no es UTF-8, archivo inexistente. | No |
| `empty_document` | El documento no contiene texto. | No |
| `unsupported_format` | Extensión distinta de `.txt` o `.pdf`. | No |

## Cómo reproducir

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/setup-ocr.ps1
cargo test -p nestadmin-extract                                   # incluye las pruebas sin PDFium
cargo test -p nestadmin-extract --test pdf -- --ignored            # PDFium y Tesseract reales
cargo test -p nestadmin-app --test ollama -- --ignored --nocapture --test-threads 1
```

`scripts/setup-ocr.ps1` descarga PDFium `chromium/8125` de bblanchon/pdfium-binaries y los datos `spa` y `eng` de tesseract-ocr/tessdata 4.1.0 en `vendor/` (excluido de Git), comprobando SHA-256, e instala Tesseract 5.4.0 (UB Mannheim) con winget. El SHA-256 de PDFium coincide con el `digest` que GitHub publica para el archivo; los de los datos de idioma, con los objetos del repositorio oficial.

## Ejecución registrada

- Fecha: 2026-11-03
- Plataforma: Windows 11 Pro 10.0.26300, Intel Core Ultra 5 125U, 16 GB
- Rust 1.99.1 (MSVC), pdfium-render 0.9.5, PDFium 8125, Tesseract 5.4.0.20240606, Ollama 0.40.14 con `qwen3:8b`

| Prueba | Resultado |
| --- | --- |
| `plain.rs` (4 pruebas, siempre en CI) | Texto plano en una página; vacío, no UTF-8, extensión no admitida e inexistente con su código; PDFium ausente → `pdf_unavailable`; Tesseract o datos de idioma ausentes → `ocr_unavailable`. Superadas. |
| `text_scanned_and_mixed_pdfs_use_the_right_method` | PDF con texto → `pdf_text`; página escaneada → `ocr`, y el OCR recupera número, proveedor, fecha y total; PDF mixto → `[pdf_text, ocr]`. Superada. |
| `ocr_failure_never_returns_a_partial_extraction` | En un PDF mixto, un fallo del OCR en la página 2 o un OCR sin texto devuelve `ocr_failed` con la página y ninguna extracción. Superada. |
| `damaged_and_blank_pdfs_are_explicit_errors` | PDF dañado → `document_unreadable`; página en blanco → `empty_document`. Superada. |
| `only_text_and_pdf_files_are_imported_and_pdf_failures_are_visible` (`crates/app`) | Solo se importan `.txt` y `.pdf` no vacíos; sin PDFium la tarea queda fallida con `pdf_unavailable` reintentable y sin resultado; el resultado guarda las páginas y su método. Superada. |
| `scanned_pdf_goes_through_local_ocr_and_ollama` (`crates/app`) | Factura escaneada sintética → OCR local → Ollama → pendiente de revisión. |

Observado con Ollama en la última prueba: **5 ejecuciones, 4 con todos los campos correctos** (base 200.00, IVA 42.00, total 242.00, sin incidencias). En una, el modelo devolvió base, IVA y total como ausentes a partir del mismo texto de OCR. No inventó valores: la factura quedó en revisión con los campos marcados «No consta». No es una tasa de acierto representativa: es un único documento sintético.

## Pendiente

- Calibrar el umbral de 20 caracteres y medir la calidad del OCR con más documentos y escaneos con ruido, inclinación o baja resolución (riesgo abierto en decisiones.md).
- PDF protegido con contraseña: el código lo identifica, pero no hay un caso de prueba sintético.
- Empaquetar `pdfium.dll` y los datos de idioma con la aplicación y detectar Tesseract en el instalador (T08).
- Las pruebas con PDFium y Tesseract no se ejecutan en CI.
