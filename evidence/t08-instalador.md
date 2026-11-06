# Evidencia T08 (parcial): instalador de una sola ejecución

Cubre R08 según D36–D40, D46, D51 y D52; D36–D40 revisan D23–D25. El paquete NSIS de Tauri se instala por usuario e incluye la aplicación, `pdfium.dll`, Tesseract portátil y los datos de idioma `spa` y `eng`. Al terminar de copiar, el gancho `NSIS_HOOK_POSTINSTALL` ejecuta `nestadmin.exe --preparar` y espera. Ese modo muestra solo el asistente y prepara Ollama y el modelo si faltan. Termina con código 0 únicamente cuando el modelo local responde. Los casos están en [pruebas del instalador](../specs/v1/pruebas-instalador.md).

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

### T08d: cliente OAuth en el paquete y carpeta del programa (D51, D52), 2026-11-05

Por indicación del usuario, el instalador no se ha construido en esta fase: se construirá una sola vez al final. Una construcción limpia empezada con un cliente sintético se detuvo durante `cargo test`, antes del paso del instalador, y no aporta resultado de T08d. No se ha instalado ni ejecutado nada en este equipo. Lo comprobado, en el equipo de desarrollo (Windows 11 Pro 10.0.26300):

| Caso | Resultado |
| --- | --- |
| T08d-03 | `build-clean.ps1 -ClienteOAuth` falla antes de clonar, con su mensaje, si el archivo no existe, está dentro del repositorio, no es JSON o no tiene ningún cliente, y si se combina con `-SkipInstaller`. Con los marcadores del archivo de ejemplo solo avisa y sigue. |
| T08d-04 | `scripts/correo-oauth.ps1` con un JSON sintético de Google de tipo `installed` y un GUID inventado escribe `google` y `microsoft`; rechaza un JSON de tipo `web`, un ID que no es GUID, una salida dentro del repositorio y sobrescribir sin `-Sobrescribir`. |
| T08d-05 (parcial) | `makensis` de la caché de Tauri compila sin errores ni avisos un script de prueba que incluye `installer-hooks.nsh` con las macros de la plantilla (`IsShortcutTarget`, `SetShortcutTarget`, `CheckIfAppIsRunning`) simuladas, una página de carpeta de MUI2 y las tres macros de gancho. Falta compilarlo con la plantilla real de Tauri. |
| T08d-01, T08d-02 | Sin ejecutar: requieren construir el instalador. |

`tauri.installer.json` y `tauri.conf.json` siguen siendo JSON válido; el recurso se añade con un segundo `--config`, que la CLI 2.12.4 fusiona en orden (`tauri build --help`).

Cómo comprobarlo en la construcción final, sin instalar:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/correo-oauth.ps1 -MicrosoftClientId 11111111-2222-3333-4444-555555555555 -Salida C:\fuera\oauth-sintetico.json
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/build-clean.ps1 -ClienteOAuth C:\fuera\oauth-sintetico.json   # T08d-02
# En la carpeta del clon: target\release\nsis\x64\installer.nsi debe contener
#   File /a "/oname=correo-oauth.json" …, NestAdminChooseDir y el aviso de datos conservados (T08d-05).
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/build-clean.ps1   # T08d-01: el paso dice «Sin cliente OAuth»
```

## Pendiente

- T08-10 a T08-14 (D51, D52) en un Windows de prueba: carpeta `%LOCALAPPDATA%\Programs\NestAdmin`, instalación encima de la versión anterior con datos, desinstalación que conserva los datos y cliente OAuth empaquetado. `scripts/t08-registro.ps1` ya anota por separado la carpeta del programa (leída del registro) y la de datos, avisa de restos del programa en la carpeta de datos y comprueba si los accesos directos están rotos.

- Ensayo de D46 en un equipo real de 8 GB: ver el aviso del asistente y el mensaje de tiempo agotado de una tarea con Ollama.
- T08-01, T08-03 a T08-07 en un Windows limpio (máquina virtual con instantánea): descarga real del instalador de Ollama (comprobación real de firma Authenticode de `OllamaSetup.exe`) y del modelo (unos 5,2 GB), cortes reales de red, cancelación y reapertura, reinicio del equipo e instalación encima de una versión anterior.
- Instalación con interfaz (no silenciosa): ver el asistente lanzado por el instalador y sus mensajes.
- Firma del instalador de NestAdmin y revisión de la redistribución de PDFium, Tesseract y sus DLL (riesgo abierto).
- Las ejecuciones registradas arriba son de la versión anterior a D52, que instalaba el programa en la carpeta de datos (`%LOCALAPPDATA%\NestAdmin`). D52 los separa; falta ensayarlo (T08-10 a T08-12).

## 2026-11-06 — `t08-registro.ps1` no anotaba el resultado

Síntoma comunicado por el usuario: la instalación terminaba, pero el archivo de registro no tenía el estado «Después». Causa: `Start-Process -Wait` de Windows PowerShell 5.1 espera a todos los descendientes del proceso, no solo a él. El gancho `NSIS_HOOK_POSTINSTALL` ejecuta `nestadmin.exe --preparar`, que arranca `ollama serve` sin ventana (`crates/setup/src/system.rs`), y la página final puede abrir NestAdmin. Ninguno de los dos termina, así que el script se quedaba esperando indefinidamente.

Corrección: el script espera solo al instalador (`WaitForExit` con plazo `-PlazoMinutos`), anota los procesos que siguen en marcha, escribe el estado final en un bloque `finally` (también tras un error o Ctrl+C) y termina con una respuesta real de `qwen3:8b` (`/api/chat`, temperatura 0, sin razonamiento), con su duración. Comprobado fuera de Windows: el script se analiza sin errores con PowerShell 7.4.6, y la espera devuelve al momento el código de salida de un proceso que deja un hijo vivo. **Falta ejecutarlo en Windows**, con el instalador real.
