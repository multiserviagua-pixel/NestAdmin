# Decisiones técnicas v1

Registro fechado de decisiones que concretan [requisitos.md](requisitos.md). Una decisión nueva o revisada añade una entrada con fecha y motivo; no reescribe las anteriores. Las versiones son las elegidas para desarrollar, no evidencia de que la aplicación funcione con ellas.

## 2026-11-03 — Arquitectura y componentes

- **D01 Proceso único.** El servicio local (cola, almacenamiento, adaptadores) se ejecuta dentro del proceso Tauri, en Rust. Al cerrar la ventana el proceso sigue activo con icono en el área de notificación (R01). *Motivo:* un solo binario que empaquetar y sin un segundo proceso que supervisar.
- **D02 Sin servidor HTTP propio.** La interfaz se comunica con el núcleo solo mediante comandos IPC de Tauri, con capacidades limitadas a la ventana principal. No se abre ningún puerto. Una segunda instancia se redirige a la primera (`tauri-plugin-single-instance`). *Motivo:* impedir el acceso desde otras páginas o procesos (requisitos, decisiones técnicas).
- **D03 Almacenamiento.** SQLite en `%LOCALAPPDATA%\NestAdmin\nestadmin.db` mediante `rusqlite` con SQLite incluido, modo WAL. Tablas separadas para tareas, transiciones, resultados, acciones e idempotencia, reglas y auditoría. Las métricas de evaluación nunca se escriben en esta base (R10, R11). Las migraciones son numeradas y se aplican al arrancar.
- **D04 Importes.** `rust_decimal` en el núcleo; se almacenan como texto decimal normalizado con dos decimales y moneda, junto a la evidencia literal del documento (R05).
- **D05 Interfaz.** TypeScript con Vite y sin framework de componentes. *Motivo:* pocas pantallas y menos dependencias que auditar; se revisará si la interfaz crece.
- **D06 IA local.** Ollama en `http://127.0.0.1:11434`, modelo `qwen3:8b`, temperatura 0 y salida restringida con esquema JSON. El adaptador rechaza cualquier URL de Ollama que no sea de bucle local (R03).
- **D07 IA externa.** Groq mediante su API compatible con OpenAI. Solo se construye el cliente tras selección explícita y con credencial presente. El modelo es configurable; el identificador usado se registra en cada evaluación y se elige al medir (T12), no ahora.
- **D08 Credenciales.** Administrador de credenciales de Windows mediante el crate `keyring`. Ni la base de datos ni los registros contienen la clave; los registros redactan cabeceras de autorización (R09).
- **D09 PDF.** PDFium mediante `pdfium-render` para extraer texto y para rasterizar páginas sin texto. La DLL de PDFium se distribuye con la aplicación.
- **D10 OCR.** Tesseract 5 con datos `spa` y `eng`, invocado como proceso local. Se considera escaneada la página cuyo texto extraído no alcanza un mínimo de caracteres; el umbral se fijará con pruebas (R04). *Alternativa descartada por ahora:* el OCR integrado de Windows, que depende de paquetes de idioma instalados por el usuario.
- **D11 Instalador.** Paquete NSIS de Tauri, con un paso de arranque de la aplicación que detecta y descarga Ollama, el modelo y Tesseract (R08). WebView2 viene con Windows 11; el instalador usa el *bootstrapper* de Microsoft si falta.

## 2026-11-03 — Integración de IA (fase 4)

- **D12 Cliente HTTP.** `ureq` 3 síncrono con TLS `rustls` y **sin proxy**: las variables `HTTP_PROXY` del sistema no pueden desviar el contenido de un documento hacia otro equipo. Plazo de conexión propio, más corto que el total: un fallo al conectar se informa como `unavailable` y no como `timeout`. *Motivo:* el núcleo es síncrono y no necesita un runtime asíncrono.
- **D13 Credenciales.** Se usa `keyring-core` con `windows-native-keyring-store` en lugar del crate `keyring`, que su propia documentación desaconseja para aplicaciones. Concreta D08 sin cambiarla.
- **D14 Incidencias calculadas.** El modelo solo extrae campos; las etiquetas `missing_tax`, `missing_total`, `total_mismatch` y `possible_duplicate` las calcula el código de `crates/documents`. *Motivo:* comprobaciones programadas y reproducibles (R05); el modelo no decide sobre importes.

## 2026-11-03 — Aplicación de escritorio (fase 6)

- **D15 Capas.** `crates/app` contiene los flujos (cola, IA, validación, simuladores, reglas) sin depender de Tauri y se prueba sin ventana; `crates/desktop` solo expone comandos, el hilo de la cola y el área de notificación. La llamada al modelo se hace fuera del cerrojo para no bloquear la ventana.
- **D16 Interfaz.** Vite y TypeScript sin framework en `ui/`; el texto se inserta siempre como texto, nunca como HTML. En desarrollo, fuera de Tauri, un servidor simulado con datos sintéticos permite ver la interfaz en un navegador; la compilación de producción lo elimina.
- **D17 Reglas.** Condiciones cerradas por acción (proveedor y total máximo para registrar; categoría para enviar). Se automatiza solo con una única regla activa coincidente y sin incidencias, campos ilegibles ni borrador sin destinatario. *Motivo:* condiciones comprobables por código sobre datos validados (R07).
- **D18 Datos y configuración.** Datos en `%LOCALAPPDATA%\NestAdmin` (`NESTADMIN_DATA_DIR` lo sustituye para pruebas). La elección de proveedor se guarda en `configuracion.json`; la clave de Groq solo en el almacén de credenciales (D13). Una elección guardada que no se puede preparar no se sustituye por otra al arrancar (flujo 4).
- **D19 CI.** La aplicación de escritorio se compila y analiza solo en Windows; en Linux se excluye `nestadmin-desktop`, que necesita WebKitGTK y no es un objetivo de la v1.

## 2026-11-03 — PDF y OCR (resto de la fase 5)

- **D20 Extracción.** `crates/extract` concreta D09 y D10: PDFium `chromium/8125` (bblanchon/pdfium-binaries) con `pdfium-render` 0.9.5; umbral inicial de 20 caracteres visibles por página; rasterizado a 300 ppp en escala de grises; Tesseract con `spa+eng` y `--psm 3`, sin ventana de consola. Un fallo en cualquier página hace fallar el documento entero.
- **D21 Datos de idioma propios.** El instalador silencioso de UB Mannheim solo trae inglés, así que NestAdmin usa su propia carpeta `tessdata` (`spa` y `eng` de tesseract-ocr/tessdata 4.1.0, comprobados por SHA-256) mediante `--tessdata-dir`. En desarrollo, `scripts/setup-ocr.ps1` prepara PDFium y los datos en `vendor/`; la aplicación instalada los buscará junto al ejecutable (T08). `NESTADMIN_PDFIUM_DIR`, `NESTADMIN_TESSDATA` y `NESTADMIN_TESSERACT` los sustituyen.
- **D22 Importación binaria.** La ventana envía el archivo como cuerpo binario de la llamada IPC, con el nombre en una cabecera, en lugar de un JSON con números. Se admiten `.txt` y `.pdf` de hasta 50 MB.

## 2026-11-03 — Instalador y preparación (fase 7)

- **D23 Reparto del instalador.** Concreta D11. El paquete NSIS de Tauri se instala por usuario, sin permisos de administrador, e incluye la aplicación, `pdfium.dll` (con su licencia) y los datos de idioma `spa` y `eng` en `pdfium\` y `tessdata\` junto al ejecutable; si falta WebView2, ejecuta en silencio el *bootstrapper* de Microsoft. Ollama, el modelo y Tesseract no van en el paquete (unos 7 GB en total): los instala la propia aplicación desde la vista «Preparación» (`crates/setup`). Los recursos se añaden solo al empaquetar (`tauri.installer.json`), para que CI y desarrollo no dependan de `vendor/`.
- **D24 Descargas fijadas.** Los instaladores de Ollama 0.40.14 y Tesseract 5.4.0 se descargan de sus publicaciones oficiales en GitHub y solo se ejecutan si su SHA-256 coincide con el fijado en el código. Una descarga cortada se conserva como `.part` y el reintento pide solo lo que falta (`Range`); un hash distinto descarta el archivo. El modelo se descarga con `POST /api/pull` de Ollama, que reanuda capas y comprueba sus huellas. Todo lo ya presente y compatible se reutiliza: Ollama 0.9.0 o posterior (por `think` y la salida con esquema), Tesseract 5.x y el modelo configurado.
- **D25 Intervención.** Ollama se instala por usuario en silencio (`/VERYSILENT`). Tesseract necesita permisos de administrador: Windows muestra su diálogo UAC en ese paso y, si el usuario lo rechaza, el paso queda fallido con «permiso denegado» y se puede reintentar. La preparación termina con una respuesta real del modelo local (`/api/chat`, temperatura 0, sin razonamiento). Desinstalar NestAdmin no quita Ollama, el modelo, Tesseract ni los datos de `%LOCALAPPDATA%\NestAdmin`, carpeta que el instalador por usuario de Tauri también usa para el programa: el desinstalador solo borra los archivos que instaló.

## Versiones de desarrollo fijadas

| Componente | Versión | Licencia | Dónde se fija |
| --- | --- | --- | --- |
| Rust | 1.99.1 (MSVC) | MIT/Apache-2.0 | `rust-toolchain.toml` |
| Node.js | 26.13.0 | MIT | `.node-version` |
| Python (harness) | 3.12 | PSF | CI |
| Tauri / tauri-build / CLI | 2.12.5 / 2.7.4 / 2.12.4 | MIT/Apache-2.0 | `Cargo.lock`, `ui/package-lock.json` |
| tauri-plugin-single-instance / `@tauri-apps/api` | 2.5.5 / 2.12.4 | MIT/Apache-2.0 | `Cargo.lock`, `ui/package-lock.json` |
| Vite / TypeScript | 8.3.5 / 7.0.2 | MIT / Apache-2.0 | `ui/package-lock.json` |
| Ollama | 0.40.14 (`OllamaSetup.exe`, SHA-256 fijado) | MIT | `crates/setup` |
| Qwen3 8B | `qwen3:8b` (digest a registrar al descargar) | Apache-2.0 | instalador y evaluación |
| Tesseract | 5.4.0.20240606 (build UB Mannheim, SHA-256 fijado) | Apache-2.0 | `crates/setup`, `scripts/setup-ocr.ps1` |
| Datos de idioma Tesseract | tessdata 4.1.0 (`spa`, `eng`) | Apache-2.0 | `scripts/setup-ocr.ps1` (SHA-256) |
| PDFium / pdfium-render | `chromium/8125` / 0.9.5 | BSD-3/Apache-2.0 / MIT-Apache-2.0 | `scripts/setup-ocr.ps1` (SHA-256), `Cargo.lock` |

Las dependencias de Rust y npm quedan fijadas por sus archivos de bloqueo, que se versionan. Cambiar una versión de esta tabla requiere una entrada nueva en este registro.

## Riesgos abiertos

- El rendimiento de `qwen3:8b` en el equipo objetivo no está medido; puede ser necesario otro modelo o cuantización (R12).
- La redistribución de PDFium y Tesseract debe revisarse antes de publicar el instalador, que además no está firmado (SmartScreen lo advertirá).
- El umbral de detección de páginas escaneadas y la calidad del OCR en facturas reales están por medir.
