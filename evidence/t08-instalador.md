# Evidencia T08 (parcial): instalador NSIS y preparación del equipo

Cubre R08 según D23–D25. El paquete NSIS de Tauri instala la aplicación por usuario con `pdfium.dll` y los datos de idioma `spa`/`eng`, y ejecuta el *bootstrapper* de WebView2 si falta. La vista «Preparación» (`crates/setup`) detecta WebView2, PDFium, los datos de idioma, Tesseract, Ollama y el modelo. Instala desde las fuentes oficiales lo que falta, con progreso, reanudación y comprobación SHA-256. Termina con una respuesta real del modelo local.

## Cómo reproducir

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/setup-ocr.ps1   # prepara vendor/
cargo test -p nestadmin-setup                                               # servidor HTTP local, sin Internet
cargo test -p nestadmin-setup -- --ignored --nocapture                      # detección real y respuesta de Ollama
npm ci --prefix ui
cd crates/desktop
..\..\ui\node_modules\.bin\tauri build --config tauri.installer.json
Start-Process ..\..\target\release\bundle\nsis\NestAdmin_0.1.0_x64-setup.exe -ArgumentList '/S' -Wait
```

## Ejecución registrada

- Fecha: 2026-11-03
- Plataforma: Windows 11 Pro 10.0.26300, Intel Core Ultra 5 125U, 16 GB. Es el equipo de desarrollo, no un equipo limpio.
- Rust 1.99.1 (MSVC), Tauri 2.12.5, NSIS 3.13 (descargado por la CLI de Tauri), Ollama 0.40.14 con `qwen3:8b`, Tesseract 5.4.0.20240606, WebView2 156.0.4314.58.

### Pruebas automáticas (`crates/setup/tests/setup.rs`)

Todas usan un servidor HTTP local y datos sintéticos.

| Prueba | Resultado |
| --- | --- |
| `an_interrupted_download_resumes_and_a_verified_file_is_reused` | El servidor corta a mitad de una descarga de 3 MB. El error es de red y se indica «se reanudará al reintentar». Se conserva el `.part`. El reintento pide `Range: bytes=<lo descargado>-` y completa el archivo. El progreso llega a 3 MB de 3 MB. Una tercera llamada reutiliza el archivo comprobado sin hacer ninguna petición. Superada. |
| `a_file_with_a_different_hash_is_discarded` | Un SHA-256 distinto da un error `Integrity` y no deja ni el archivo ni el `.part`. Superada. |
| `http_errors_and_unreachable_servers_are_network_failures` | HTTP 404 y un servidor que no responde se informan como errores de red. Superada. |
| `the_model_download_reports_progress_and_surfaces_errors` | `POST /api/pull` con progreso por bytes hasta `success`. Una línea `{"error": …}` de Ollama se muestra tal cual. Un flujo que termina sin `success` es un error reintentable. Superada. |
| `the_model_check_needs_a_real_non_empty_answer` | `/api/chat` con `think: false` y temperatura 0. Una respuesta vacía o un modelo inexistente son errores. Superada. |
| `detection_reports_what_is_missing_and_what_is_present` | Detecta lo ausente, Ollama instalado pero parado, datos de idioma incompletos (indica cuál falta) y Ollama 0.5.7 como incompatible. Lo presente no se descarga de nuevo: `install` no emite progreso. PDFium no es instalable desde la aplicación. Superada. |
| `versions_are_compared_numerically` | `0.10.0` > `0.9.6`; `v5.4.0.20240606` → 5.4.0. Superada. |

### En este equipo

| Paso | Resultado |
| --- | --- |
| `detects_this_machine` | WebView2 156.0.4314.58, PDFium, tessdata `spa, eng`, Tesseract 5.4.0.20240606, Ollama 0.40.14 y `qwen3:8b` (digest `500a1f067a9f`), todos presentes. |
| `the_local_model_answers` | `qwen3:8b` respondió «listo» en 54,7 s. El modelo estaba sin cargar en memoria; es una sola medida, no una medida de rendimiento (T12). |
| `tauri build --config tauri.installer.json` | `NestAdmin_0.1.0_x64-setup.exe`, 21 916 179 bytes, SHA-256 `2EF7AF53…E4B5F234`. El instalador no está firmado. |
| Instalación silenciosa (`/S`) | 11 s, sin pedir permisos. Deja en `%LOCALAPPDATA%\NestAdmin` `nestadmin.exe`, `uninstall.exe`, `pdfium\pdfium.dll` con su `LICENSE` y `tessdata\{spa,eng}.traineddata`. Crea la entrada en «Aplicaciones instaladas» (HKCU) y el acceso directo en el menú Inicio. |
| Arranque de la aplicación instalada | Se abre la ventana «NestAdmin» con unos 34 MB de memoria de trabajo. «Preparación» no muestra ningún componente pendiente: la aplicación instalada encuentra PDFium y los datos de idioma junto al ejecutable. Una segunda ejecución termina con código 0 y deja un solo proceso. |
| Desinstalación silenciosa | Termina con código 0. Quita el ejecutable, los recursos, la entrada del registro y el acceso directo. Conserva la base de datos y los archivos de datos de la carpeta (D25). |

Durante el ensayo, el usuario también usó la ventana instalada y consultó el correo del simulador (datos sintéticos, sin cuentas reales). La carpeta de datos de la prueba se eliminó después. La vista «Preparación» se ha comprobado en el navegador con el servidor simulado: «Instalar lo que falta» instala Tesseract y después el modelo, con progreso, y termina con la comprobación del modelo. No se ha recorrido a mano en la ventana instalada.

## Pendiente

- Ensayo en un equipo o máquina virtual limpios, sin WebView2, Ollama, modelo ni Tesseract: descarga real de los instaladores fijados (unos 1,7 GB y 50 MB) y del modelo (unos 5,2 GB), y aceptación y rechazo del diálogo UAC de Tesseract.
- Corte real de red durante esas descargas. La reanudación solo se ha probado con el servidor local.
- Recorrido manual de «Preparación» en la ventana instalada.
- Firma del instalador y revisión de la redistribución de PDFium y de los datos de Tesseract (riesgo abierto).
- La instalación comparte carpeta con los datos (`%LOCALAPPDATA%\NestAdmin`). La desinstalación conserva los datos, pero conviene decidir si separarlos.
