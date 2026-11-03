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

## Versiones de desarrollo fijadas

| Componente | Versión | Licencia | Dónde se fija |
| --- | --- | --- | --- |
| Rust | 1.99.1 (MSVC) | MIT/Apache-2.0 | `rust-toolchain.toml` |
| Node.js | 26.13.0 | MIT | `.node-version` |
| Python (harness) | 3.12 | PSF | CI |
| Tauri / tauri-build / CLI | 2.12.5 / 2.7.4 / 2.12.4 | MIT/Apache-2.0 | `Cargo.lock`, `package-lock.json` |
| Ollama | 0.40.14 | MIT | instalador |
| Qwen3 8B | `qwen3:8b` (digest a registrar al descargar) | Apache-2.0 | instalador y evaluación |
| Tesseract | 5.4.0 (build UB Mannheim) | Apache-2.0 | instalador |
| PDFium | por fijar al integrar D09 | BSD-3/Apache-2.0 | instalador |

Las dependencias de Rust y npm quedan fijadas por sus archivos de bloqueo, que se versionan. Cambiar una versión de esta tabla requiere una entrada nueva en este registro.

## Riesgos abiertos

- El rendimiento de `qwen3:8b` en el equipo objetivo no está medido; puede ser necesario otro modelo o cuantización (R12).
- La redistribución de PDFium y Tesseract debe revisarse antes de publicar el instalador.
- El umbral de detección de páginas escaneadas y la calidad del OCR en facturas reales están por medir.
