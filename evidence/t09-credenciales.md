# Evidencia T09 (parcial): credenciales protegidas y ausencia de secretos

Cubre R09 a nivel de biblioteca y de capa de aplicación: la clave de Groq y los tokens de correo solo están en el almacén de credenciales; ni la base de datos, ni la cola, ni el registro local de cada cuenta de correo, ni la configuración, ni los errores, historial o `Debug` los contienen. También cubre los errores de credencial ausente, inválida y revocada. Todas las credenciales son sintéticas y contienen la marca `PRUEBA_SINTETICA_T09`, que se busca en UTF-8 y UTF-16LE (incluidos fragmentos, por ejemplo tras quitar puntos). Groq, Gmail y el servicio de tokens se simulan en 127.0.0.1; ninguna prueba sale a la red.

## Cómo reproducir

```sh
cargo test -p nestadmin-app --test credentials --locked
cargo test -p nestadmin-mail --locked
cargo test -p nestadmin-desktop --locked settings
# Solo en Windows; escribe y borra una credencial de prueba en el almacén del usuario:
cargo test -p nestadmin-ai --test windows_store --locked -- --ignored
```

## Ejecución registrada

- Fecha: 2026-11-04
- Plataforma: Windows 11 Pro 10.0.26300 (equipo de desarrollo)
- Rust 1.99.1 (MSVC), `keyring-core` 1.0.1, `windows-native-keyring-store` 1.1.0, `ureq` 3.5.1
- Resultado: las 3 pruebas de `crates/app/tests/credentials.rs`, las 11 unitarias de `crates/mail` (incluida la nueva), las 2 de `settings.rs` y la del almacén real de Windows (con `--ignored`), superadas. `cargo test --workspace --locked`: 132 superadas, 0 fallidas, 7 ignoradas (las que necesitan Ollama, PDFium o escriben en el sistema). `cargo clippy --workspace --all-targets -- -D warnings` sin avisos; `python scripts/check.py` correcto.

## Fuga encontrada y corregida

Con el token de renovación revocado, el servicio de tokens simulado repite el token en el campo `error` de su respuesta. `crates/mail` copiaba ese campo literalmente en el mensaje de error («rechazó la solicitud (1//PRUEBA_…)»), que la aplicación mostraría y podría escribir en un registro. Del mismo modo, el código de error de Graph o Gmail se filtraba carácter a carácter y podía dejar el token sin sus puntos. Ahora `provider_code` solo admite un identificador corto (`invalid_grant`, `InvalidAuthenticationToken`…) que no contenga ninguna credencial de la petición y, si no, escribe «sin detalle». Se comprobó que la prueba de correo falla sin el arreglo y pasa con él.

## Qué comprueban las pruebas

| Prueba | Qué comprueba |
| --- | --- |
| `groq_key_stays_in_the_store_through_tasks_and_credential_errors` (`crates/app`) | Se guarda la clave en el almacén doble y el proveedor se construye desde él. Una factura pasa por Groq simulado, revisión y registro; los correos del simulador se clasifican. La clave viaja solo como `Bearer` al proveedor. **Revocada** (el servidor responde 401 citando la clave): tarea `failed` con `unauthorized` recuperable, sin resultado, un único intento y sin cambio de proveedor. **Sin red**: `unavailable` recuperable. **Inválida** (con `\r\n` para inyectar una cabecera): la tarea falla de forma recuperable y no llega ninguna cabecera inyectada. **Recuperación**: con la clave válida y reintento explícito, la tarea vuelve a revisión. **Ausente**: Groq no se construye (`unauthorized`). Después se inspeccionan todos los archivos del directorio de datos (`nestadmin.db` y sus anexos, `simulador-correo.json`, `simulador-administracion.json`, documentos) y todos los textos: tareas, errores, historial y `Debug`. El almacén solo recibió el nombre `groq_api_key`. |
| `mail_tokens_stay_in_the_store_through_sign_in_refresh_and_errors` (`crates/app`) | Inicio de sesión OAuth con PKCE y secreto de cliente sintéticos; lista, borrador y envío con el registro local de la cuenta en disco. **Caducado**: se renueva y el token nuevo se guarda en el almacén. **Revocado en el proveedor** (401 que repite el token): una renovación y `unauthorized`. **Token de renovación revocado** (el servicio lo repite en `error`): `unauthorized` con «vuelva a iniciar sesión», sin el token. **Sin red**: `unavailable`. **Credencial ilegible** en el almacén: `unauthorized`. **Sesión cerrada**: `unauthorized` sin ninguna petición al proveedor. Se inspeccionan el registro `cuenta-t09.json` y todos los textos; el almacén solo recibió `mail/cuenta-t09`. |
| `inspection_detects_the_marker_in_any_file_or_encoding` (`crates/app`) | La inspección encuentra la marca en UTF-8, en UTF-16LE y en un archivo de un subdirectorio, para que un resultado limpio no sea vacío. |
| `provider_codes_never_carry_secrets_or_free_text` (`crates/mail`) | El saneador deja pasar `invalid_grant` y rechaza el token, textos que lo contienen o que son parte de él, texto libre y cadenas vacías. |
| `saved_settings_never_contain_the_key` (`crates/desktop`) | `configuracion.json` con Groq elegido y clave guardada no contiene la clave y se vuelve a leer igual. |
| `missing_key_at_startup_fails_explicitly_and_keeps_groq` (`crates/desktop`) | Al arrancar con Groq elegido y sin clave, el proveedor sigue siendo Groq y cada solicitud devuelve `unauthorized`; no hay respaldo a Ollama. |
| `real_windows_store_round_trip_with_a_test_target` (`crates/ai`, marcada) | El Administrador de credenciales de Windows guarda, devuelve (también desde otra instancia) y borra una credencial sintética con el nombre `t09-prueba-<pid>`; borrar dos veces no es un error y la credencial se borra aunque la prueba falle. |

Las pruebas ya existentes siguen cubriendo el resto: `Debug` de `Secret`, `GroqProvider`, `Tokens`, `OAuthConfig` y `TokenSource` sin secretos, y errores de Groq sin la clave (`crates/ai`, T03; `crates/mail`, T13).

## Pendiente para completar T09

- Inspeccionar `%LOCALAPPDATA%\NestAdmin` y los registros tras una sesión real con la aplicación instalada: guardar la clave desde la ventana, procesar y provocar errores. Los comandos de la ventana (`save_groq_key`, `select_provider`) no tienen pruebas propias; las de `settings.rs` y la capa de aplicación cubren su lógica.
- El instalador real (T08) y la desinstalación: no se ha comprobado qué queda en el almacén de credenciales al desinstalar.
- Groq y Gmail reales: los mensajes de error de los servicios reales no se han observado.
- Los conectores de correo aún no están integrados en `crates/app`; la prueba de correo usa `Mailbox` directamente. Cuando se integren, la prueba debe pasar por la capa de aplicación.
- No se verifica cómo cifra Windows la credencial (DPAPI); se confía en el Administrador de credenciales. La memoria del proceso y los volcados por fallo no se inspeccionan.
