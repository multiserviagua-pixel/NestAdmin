# Evidencia T08 (parcial): instalador de una sola ejecución

Cubre R08 según D36–D40 y D46; D36–D40 revisan D23–D25. El paquete NSIS de Tauri se instala por usuario e incluye la aplicación, `pdfium.dll`, Tesseract portátil y los datos de idioma `spa` y `eng`. Al terminar de copiar, el gancho `NSIS_HOOK_POSTINSTALL` ejecuta `nestadmin.exe --preparar` y espera. Ese modo muestra solo el asistente y prepara Ollama y el modelo si faltan. Termina con código 0 únicamente cuando el modelo local responde. Los casos están en [pruebas del instalador](../specs/v1/pruebas-instalador.md).

## Cómo reproducir

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/setup-ocr.ps1   # prepara vendor/ (incluye vendor/tesseract)
cargo test -p nestadmin-setup                                               # T08b y T08c con dobles, sin Internet
cargo test -p nestadmin-setup -- --ignored --nocapture                      # este equipo y respuesta real de Ollama
npm ci --prefix ui
cd crates/desktop
..\..\ui\node_modules\.bin\tauri build --config tauri.installer.json
Start-Process ..\..\target\release\bundle\nsis\NestAdmin_0.1.0_x64-setup.exe -ArgumentList '/S' -Wait -PassThru
```

## Ejecución registrada

- Fecha: 2026-11-04.
- Plataforma: Windows 11 Pro 10.0.26300, Intel Core Ultra 5 125U, 16 GB. Es el equipo de desarrollo, no un equipo limpio: Ollama 0.40.14 y `qwen3:8b` ya estaban instalados.
- Versiones: Rust 1.99.1 (MSVC), Tauri 2.12.5, NSIS de la CLI de Tauri, Tesseract 5.4.0.20240606 portátil.

### T08b y T08c: pruebas automáticas (`crates/setup/tests/setup.rs`)

En estas pruebas, Ollama y el servidor de descargas se simulan en 127.0.0.1 y un doble de `System` registra las llamadas (D40). No se instala nada ni se sale a la red. Hay 23 pruebas con dobles superadas, 1 que lee la memoria real de este equipo (solo en Windows) y 1 ignorada, que es la del equipo real descrita más abajo.

| Prueba | Caso |
| --- | --- |
| `t08b_01_clean_machine_installs_everything_and_ends_ready` | T08b-01 |
| `t08b_02_everything_present_is_reused_and_only_verified` | T08b-02 |
| `t08b_03_stopped_ollama_is_only_started` | T08b-03 |
| `t08b_04_outdated_running_ollama_is_updated` | T08b-04 |
| `t08b_05_outdated_stopped_ollama_is_found_on_start_then_updated` | T08b-05 |
| `t08b_06_cut_downloads_resume_and_retry` | T08b-06 |
| `t08b_07_model_network_failures_stop_after_three_attempts_and_resume_later` | T08b-07 |
| `t08b_08_installer_without_valid_signature_is_not_run_and_is_deleted` | T08b-08 |
| `t08b_08b_installer_with_a_different_hash_is_rejected_before_the_signature` | T08b-08 (huella) |
| `t08b_09_model_rejected_by_ollama_is_not_retried` | T08b-09 |
| `t08b_10_empty_answer_fails_verification` | T08b-10 |
| `t08b_11_cancelling_during_a_download_installs_nothing` | T08b-11 |
| `t08b_12_missing_or_broken_ocr_fails_without_downloading` | T08b-12 |
| `t08b_13_states_events_and_errors_serialize_to_the_contract` | T08b-13 (incluye `status` con `memory`) |
| `t08b_14_memory_is_classified_against_16_and_8_gb` | T08b-14 |
| `t08b_15_low_memory_warns_without_blocking_or_switching_provider` | T08b-15 |
| `t08b_16_verification_timeout_names_memory_as_the_likely_cause` | T08b-16 |
| `t08b_17_task_timeouts_on_low_memory_keep_the_code_and_explain_the_cause` | T08b-17 |
| `t08c_01_stopped_ollama_is_started_once_at_startup` | T08c-01 (lógica) |
| `t08c_02_missing_model_asks_for_the_wizard_without_downloading` | T08c-02 (lógica) |
| `t08c_03_groq_selected_checks_nothing_local` | T08c-03 |
| `t08c_04_slow_start_is_awaited_off_the_caller_thread` | T08c-04 (lógica) |
| `versions_are_compared_numerically` | Versión mínima 0.9.0 |

`cargo clippy --workspace --all-targets -- -D warnings` y `cargo test --workspace` terminan sin errores.

### En este equipo

| Paso | Resultado |
| --- | --- |
| `this_machine_memory_is_read` (2026-11-04, D46) | `GlobalMemoryStatusEx` da 16 664 707 072 bytes (15,5 GiB) en este equipo de 16 GB: el margen de 1 GiB lo clasifica como `recommended`. Con 3,7 GiB libres en ese momento, el aviso solo sugiere cerrar programas. |
| `this_machine_is_ready_and_the_local_model_answers` | Usa el Tesseract de `vendor/tesseract`. `ocr` en `done`; `ollama`, `service` y `model` en `skipped`. `qwen3:8b` (digest `500a1f06…`) respondió «listo» en 42,3 s con el modelo sin cargar. Es una sola medida, no de rendimiento (T12). |
| `tauri build --config tauri.installer.json` | `NestAdmin_0.1.0_x64-setup.exe`, 47 929 663 bytes, SHA-256 `7F55C011…D76AF0`. Unos 24 MB más que el paquete sin Tesseract. Sin firmar. |
| T08-08 / T08-02: instalación silenciosa (`/S`) | Código 0 en 116 s, de los que la mayor parte es la verificación del modelo. No descargó Ollama ni el modelo y no pidió permisos. Deja `nestadmin.exe`, `uninstall.exe`, `pdfium\`, `tessdata\` y `tesseract\`, con 29 archivos: el ejecutable, 26 DLL, `LICENSE` y `AUTHORS`. El Tesseract instalado lista `eng` y `spa` con `--tessdata-dir`. |
| `nestadmin.exe --preparar` repetido | Código 0 en 8,5 s, de los que 3 s son la pantalla de éxito, con el modelo ya en memoria. No crea base de datos. La ventana se titula «Preparación de NestAdmin». |
| Cerrar el asistente a los 2,5 s | Código 1 (D36): el instalador lo indicaría y la aplicación ofrecería el asistente al abrirse. |
| T08c-01 en real | Con Ollama y su aplicación de bandeja detenidos y datos en una carpeta temporal (`NESTADMIN_DATA_DIR`), abrir NestAdmin arranca `ollama.exe serve`, cuyo proceso padre es NestAdmin. La API responde a los 18,8 s y la ventana sigue respondiendo mientras tanto. Al cerrar NestAdmin, Ollama sigue en marcha. Después se volvió a abrir la aplicación de bandeja de Ollama y se borró la carpeta temporal. |
| T08-09: desinstalación silenciosa | Código 0. Elimina la carpeta de instalación, que no contenía datos porque `--preparar` no los crea. Ollama y `qwen3:8b` se conservan. El aviso de que se conservan solo aparece en la desinstalación con interfaz; en modo silencioso se omite. |

El asistente se ha comprobado en el navegador con el servidor simulado, con datos sintéticos, en dos modos:

- **Modo normal.** La comprobación al abrir detecta que falta el modelo y abre «Preparación». La preparación muestra el progreso por pasos y bytes, el aviso «Reintento 2 de 3» y el resultado final.
- **Modo instalador (`?preparar`).** Muestra solo el asistente y empieza solo. «Cancelar» deja el paso en «Falló» y ofrece «Reintentar» y «Cerrar sin terminar».
- **Memoria (D46), 2026-11-04.** Con `?preparar&ram=8`, el aviso fuerte aparece en rojo desde el principio y la preparación sigue; la verificación simulada agota el tiempo y el error dice «probablemente por falta de memoria: el equipo tiene 8,0 GB y la IA local necesita 16 GB», con «Reintentar» disponible. Con `?ram=12` en la ventana normal, el aviso es ámbar y ofrece «Ir a Configuración»; con 16 GB no hay aviso. Groq no se activa en ningún caso.

No se ha recorrido a mano en la ventana instalada.

## Pendiente

- Ensayo de D46 en un equipo real de 8 GB: ver el aviso del asistente y el mensaje de tiempo agotado de una tarea con Ollama.
- T08-01, T08-03 a T08-07 en un Windows limpio (máquina virtual con instantánea): descarga real del instalador de Ollama (comprobación real de firma Authenticode de `OllamaSetup.exe`) y del modelo (unos 5,2 GB), cortes reales de red, cancelación y reapertura, reinicio del equipo e instalación encima de una versión anterior.
- Instalación con interfaz (no silenciosa): ver el asistente lanzado por el instalador y sus mensajes.
- Firma del instalador de NestAdmin y revisión de la redistribución de PDFium, Tesseract y sus DLL (riesgo abierto).
- La instalación comparte carpeta con los datos (`%LOCALAPPDATA%\NestAdmin`); la desinstalación conserva los datos, pero conviene decidir si separarlos.
