# Evidencia T04 (parcial): texto, PDF, PDF escaneado y fotos con OCR local

Cubre R04 con documentos sintéticos generados en las propias pruebas (no hay documentos de ejemplo en el repositorio) y una medición local con una foto real, de la que solo se registran métricas agregadas. La extracción está en `crates/extract`: texto plano UTF-8; PDF mediante PDFium (D09) y, por cada página con menos de 60 caracteres visibles (D60), rasterizado a 300 ppp y OCR con Tesseract 5 como proceso local con `spa+eng` (D10); y fotos o escaneos `.jpg`, `.jpeg` y `.png` de una página, enderezados según su EXIF y llevados a una resolución útil antes del OCR (D62). Cada página con OCR lleva la confianza media de Tesseract y, por debajo de 85, la marca `low_confidence`, que la revisión muestra como «OCR de baja calidad» (D61). La capa de aplicación ejecuta la extracción en `Job::run`, fuera del cerrojo, antes de llamar al modelo.

Si una página falla (o el OCR no reconoce nada, o tarda más de 90 s), el documento entero falla con el motivo y un código: nunca se pasa al modelo una extracción incompleta.

| Código | Cuándo | Reintentable |
| --- | --- | --- |
| `pdf_unavailable` | No se puede cargar `pdfium.dll`. | Sí |
| `ocr_unavailable` | Falta Tesseract o los datos de idioma. | Sí |
| `ocr_failed` | Tesseract falla en una página, no reconoce texto o supera 90 s; indica la página. | Sí |
| `document_unreadable` | PDF dañado o protegido, imagen dañada, texto que no es UTF-8, archivo inexistente. | No |
| `empty_document` | El documento no contiene texto. | No |
| `unsupported_format` | Extensión distinta de `.txt`, `.pdf`, `.jpg`, `.jpeg` o `.png`. | No |

## Cómo reproducir

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/setup-ocr.ps1
cargo test -p nestadmin-extract                                   # sin PDFium ni Tesseract (también en CI)
cargo test -p nestadmin-extract --test pdf -- --ignored --test-threads 1   # PDFium y Tesseract empaquetado
cargo run --release -p nestadmin-extract --example calibrar_ocr -- --facturas 6
cargo test -p nestadmin-app --test ollama -- --ignored --nocapture --test-threads 1
```

`scripts/setup-ocr.ps1` descarga PDFium `chromium/8125` de bblanchon/pdfium-binaries y los datos `spa` y `eng` de tesseract-ocr/tessdata 4.1.0 en `vendor/` (excluido de Git), comprobando SHA-256, instala Tesseract 5.4.0 (UB Mannheim) con winget y copia su versión portátil a `vendor/tesseract`, la que lleva el instalador (D38). Las pruebas y la calibración usan esa copia, no la de Program Files.

## Tesseract empaquetado (D38)

- `Tesseract::locate` decide qué `tesseract.exe` se usa. En la aplicación instalada (compilación sin `debug_assertions`) devuelve siempre `tesseract\tesseract.exe` junto al ejecutable, aunque falte: entonces el OCR falla con `ocr_unavailable` y no se busca un Tesseract del sistema ni del `PATH`. `crates/desktop` la usa; solo `NESTADMIN_TESSERACT` y `NESTADMIN_TESSERACT_DIR` la sustituyen de forma explícita.
- `installed_app_only_uses_the_packaged_tesseract` (siempre, en CI) comprueba esas reglas sin ejecutar nada.
- `packaged_tesseract_reads_a_phone_photo_from_an_install_folder` (con `--ignored`) copia `vendor/tesseract` a una carpeta temporal con la forma de la instalación, comprueba que `locate` la elige, reduce el `PATH` a `System32` y reconoce con ella una foto de móvil sintética con EXIF: confianza 90,3 y los cuatro campos. Es decir, la copia del paquete funciona sola, con sus DLL, sin instalar nada en el equipo. El instalador real se acredita en T08.

## Calibración con escaneos sintéticos

`crates/extract/examples/calibrar_ocr.rs` genera facturas inventadas (proveedor, número, fecha, conceptos e importes en formato español, en Helvetica, Times o Courier de 10 a 14 puntos), las rasteriza con PDFium y las degrada; cada imagen pasa por el extractor de la aplicación con el Tesseract empaquetado. Un campo cuenta como encontrado si su valor exacto aparece en el texto (sin tener en cuenta los espacios); los seis campos son número, proveedor, fecha, base, IVA y total. Generador determinista: las mismas cifras en cada ejecución salvo los tiempos.

### Umbral de página escaneada (D60)

| Página | Caracteres visibles | Umbral 20 | Umbral 60 |
| --- | --- | --- | --- |
| Factura con capa de texto (6 facturas) | 306–380 | texto | texto |
| Escaneada con sello «Página 1 de 1» | 10 | OCR | OCR |
| Escaneada con sello de fecha de escaneo | 26 | **texto (sin OCR)** | OCR |
| Escaneada con nombre de la aplicación de escáner | 33 | **texto (sin OCR)** | OCR |
| Escaneada con sello de copia auténtica | 41 | **texto (sin OCR)** | OCR |

`a_scanned_page_with_a_short_text_stamp_still_goes_through_ocr` reproduce el caso de 33 caracteres con PDFium real: con 20 la página queda como texto; con el valor por defecto pasa por OCR y se recuperan número, proveedor y total.

### OCR por condición (D61, D62)

| Condición | Confianza media | Mínima | Campos encontrados | Facturas completas | Tiempo medio |
| --- | --- | --- | --- | --- | --- |
| limpia 300 ppp | 90,8 | 90,3 | 36/36 (100 %) | 6/6 | 1,2 s |
| 200 ppp | 91,4 | 90,1 | 36/36 (100 %) | 6/6 | 1,0 s |
| 150 ppp | 90,7 | 89,6 | 36/36 (100 %) | 6/6 | 1,0 s |
| 100 ppp | 91,1 | 90,0 | 36/36 (100 %) | 6/6 | 1,3 s |
| 75 ppp | 90,7 | 88,4 | 36/36 (100 %) | 6/6 | 1,1 s |
| ruido σ=20 | 89,4 | 83,2 | 33/36 (92 %) | 4/6 | 2,0 s |
| ruido σ=45 | 30,9 | 0,0 | 0/36 (0 %) | 0/6 | 60,6 s |
| giro 1,5° | 91,2 | 90,4 | 36/36 (100 %) | 6/6 | 0,9 s |
| giro 4° | 89,2 | 86,2 | 32/36 (89 %) | 4/6 | 0,9 s |
| desenfoque + JPEG q35 | 91,4 | 90,3 | 36/36 (100 %) | 6/6 | 1,1 s |
| 150 ppp + ruido + giro 2° | 52,2 | 0,0 | 18/36 (50 %) | 3/6 | 1,6 s |
| móvil 12 Mpx, EXIF 6 | 90,3 | 86,6 | 36/36 (100 %) | 6/6 | 1,7 s |
| móvil 2 Mpx, EXIF 6, q60 | 69,8 | 0,0 | 12/36 (33 %) | 0/6 | 1,4 s |
| móvil 12 Mpx sin EXIF (girada) | 34,9 | 28,2 | 0/36 (0 %) | 0/6 | 1,9 s |

Tiempos de la última ejecución (en ejecuciones con el equipo cargado por otras compilaciones fueron de 1,4 a 11 s por página). Con ruido σ=45 Tesseract no lee nada: sin límite tardó 178 s por página en una ejecución anterior; ahora cada página se corta a los 90 s con `ocr_failed` (D61), y alguna falla antes por no reconocer texto. La ampliación de D62 mantiene el 100 % de campos hasta 75 ppp. La condición «sin EXIF» genera la foto igual que «móvil 12 Mpx, EXIF 6», pero sin la etiqueta de orientación. Una confianza mínima de 0,0 corresponde a una página que falló (`ocr_failed`).

### Umbral de confianza (D61)

Una página es «incompleta» si le falta al menos uno de los seis campos (25 de 84).

| Umbral | Marcadas | Incompletas marcadas | Incompletas sin marcar | Completas marcadas |
| --- | --- | --- | --- | --- |
| 50 | 16/84 | 16/25 | 9 | 0 |
| 60 | 16/84 | 16/25 | 9 | 0 |
| 70 | 16/84 | 16/25 | 9 | 0 |
| 75 | 16/84 | 16/25 | 9 | 0 |
| 80 | 16/84 | 16/25 | 9 | 0 |
| 85 | 21/84 | 21/25 | 4 | 0 |
| 90 | 33/84 | 25/25 | 0 | 8 |

Se elige 85: es el valor más alto sin falsas alarmas (con 90 se marcan 8 páginas completas). Las cuatro páginas incompletas que no marca:

- ruido σ=20: confianza 89,1, 5 de 6 campos;
- giro 4°: confianza 86,2 y 86,5, 4 de 6 campos;
- móvil 2 Mpx con JPEG de calidad 60: confianza 89,3 y **ningún campo**: Tesseract leyó con seguridad solo una parte de la página.

La última muestra que una confianza alta no garantiza una lectura completa: el aviso complementa la revisión obligatoria, no la sustituye.

## Foto real (solo en local)

El 2026-11-05 se probó una foto de factura hecha con móvil que el usuario dejó en su equipo. **No se ha copiado al repositorio ni se registra ningún valor del documento**: solo métricas agregadas, con `calibrar_ocr -- --imagen <ruta>`, que nunca imprime el texto.

| Métrica | Resultado |
| --- | --- |
| Archivo | JPEG de 3,6 MB, 4000×2252 px, orientación EXIF «girar 90°» |
| Imagen entregada al OCR | 1975×3508 px (enderezada y reducida, D62) |
| Tiempo de extracción | 7,1–16,8 s en tres ejecuciones (variable por la carga del equipo) |
| Caracteres reconocidos | 2109 |
| Confianza media | 71,5 → marcada como OCR de baja calidad |
| Campos con forma de factura | palabra «factura»: sí; «total»: sí; fecha: sí; importe con decimales: sí; NIF/CIF: sí |
| Sin aplicar el EXIF (imagen tal como se guardó) | confianza 30,2; solo aparece un importe |
| Girada 180° o 90° más | 32,3 y 76,4; la de 76,4 pierde la palabra «factura» |

La foto mostró el problema que D62 corrige: el móvil guarda la imagen apaisada y la orientación en EXIF; sin aplicarla, el OCR no lee nada útil. El caso sintético equivalente es la condición «móvil 12 Mpx, EXIF 6» (y su contraste «sin EXIF»), y en CI `exif_orientation_of_a_phone_photo_is_applied_before_ocr` y `the_ocr_receives_the_upright_normalized_image`. La confianza de 71,5 en una foto real frente a 90 en las sintéticas indica que los escaneos sintéticos son más fáciles que una foto real: con un solo documento no se puede generalizar.

## Pruebas registradas

- Fecha: 2026-11-05
- Plataforma: Windows 11 Pro 10.0.26300, Intel Core Ultra 5 125U, 16 GB
- Rust 1.99.1 (MSVC), pdfium-render 0.9.5, PDFium 8125, Tesseract 5.4.0.20240606 empaquetado (`vendor/tesseract`), datos tessdata 4.1.0

| Prueba | Resultado |
| --- | --- |
| `plain.rs` (4 pruebas, siempre en CI) | Texto plano en una página; vacío, no UTF-8, extensión no admitida e inexistente con su código; PDFium ausente → `pdf_unavailable`; Tesseract o datos de idioma ausentes → `ocr_unavailable`. Superadas. |
| `photo.rs` (6 pruebas, siempre en CI) | EXIF 6 endereza la foto antes del OCR; reducción de fotos grandes y ampliación de escaneos pequeños; confianza y `low_confidence` en el resultado y en el JSON; imagen dañada o inexistente → `document_unreadable`; OCR vacío → `ocr_failed`; reglas de `Tesseract::locate`. Superadas. |
| Pruebas unitarias de `ocr.rs` (2, siempre en CI) | Confianza media ponderada del TSV; un proceso que no termina a tiempo se mata y devuelve «agotado». Superadas. |
| `text_scanned_and_mixed_pdfs_use_the_right_method` | PDF con texto → `pdf_text`; página escaneada → `ocr` con confianza y sin marca de baja calidad, y el OCR recupera número, proveedor, fecha y total; PDF mixto → `[pdf_text, ocr]`. Superada. |
| `a_scanned_page_with_a_short_text_stamp_still_goes_through_ocr` | Ver D60. Superada. |
| `packaged_tesseract_reads_a_phone_photo_from_an_install_folder` | Ver D38. Superada. |
| `ocr_failure_never_returns_a_partial_extraction` | En un PDF mixto, un fallo del OCR en la página 2 o un OCR sin texto devuelve `ocr_failed` con la página y ninguna extracción. Superada. |
| `damaged_and_blank_pdfs_are_explicit_errors` | PDF dañado → `document_unreadable`; página en blanco → `empty_document`. Superada. |
| `only_text_pdf_and_image_files_are_imported_and_failures_are_visible` (`crates/app`) | Se importan `.txt`, `.pdf` e imágenes no vacíos; sin PDFium la tarea queda fallida con `pdf_unavailable` reintentable; una foto ilegible, con `document_unreadable`; ninguna deja resultado; el resultado guarda las páginas y su método. Superada. |

### Ejecución anterior con Ollama (2026-11-03)

`scanned_pdf_goes_through_local_ocr_and_ollama` (`crates/app`): factura escaneada sintética → OCR local → Ollama 0.40.14 con `qwen3:8b` → pendiente de revisión. 5 ejecuciones, 4 con todos los campos correctos (base 200.00, IVA 42.00, total 242.00, sin incidencias); en una, el modelo devolvió base, IVA y total como ausentes, sin inventar valores: la factura quedó en revisión con los campos marcados «No consta». No es una tasa de acierto representativa: es un único documento sintético. No se ha repetido en esta fase (Ollama estaba ocupado por la prueba del instalador en curso); la prueba ya usa el Tesseract empaquetado.

## Pendiente

- Medir con más documentos reales (solo se ha probado una foto); los umbrales de D60–D62 pueden cambiar.
- PDF protegido con contraseña: el código lo identifica, pero no hay un caso de prueba sintético.
- Las pruebas con PDFium y Tesseract no se ejecutan en CI.
- Comprobar la aplicación instalada con una foto desde la ventana (T08); aquí se acredita la copia empaquetada en una carpeta con la forma de la instalación, no el instalador.
