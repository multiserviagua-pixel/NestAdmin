# Evidencia T05b y T13b (parcial): origen de correos y documentos en la revisión

Cubre D48 y D49: la cuenta (nombre y dirección del buzón), el remitente, los destinatarios y la fecha de cada correo; el origen de cada documento (archivo o adjunto de correo, fecha de importación y SHA-256); la apertura de la copia guardada; la página y la línea de cada campo extraído; el renombrado de cuentas y el simulador como cuenta simulada con remitentes sintéticos. Todos los datos son sintéticos (dominio `.test`); Gmail y Graph se simulan en 127.0.0.1.

## Cómo reproducir

```sh
cargo test -p nestadmin-app -p nestadmin-simulators -p nestadmin-extract --locked
cd ui && npm ci && npm run build && npx vite --port 5297
```

La vista previa (`npx vite`) usa `ui/src/mock.ts`: tres cuentas con dirección (simulador, Outlook y Gmail), dos correos de cuentas distintas, una factura importada con su ruta, una factura adjunta a un correo de Gmail, escaneada en dos páginas, y una factura antigua sin origen registrado.

## Ejecución registrada

- Fecha: 2026-11-05
- Plataforma: Windows 11 Pro 10.0.26300, Rust 1.99.1 (MSVC), Node.js 26.13.0
- `cargo test --workspace --locked --no-fail-fast`: 161 pruebas superadas, 0 fallidas y 7 ignoradas (las que necesitan PDFium, Tesseract, Ollama o el almacén real). En una de las ejecuciones falló `local_failure_never_reaches_the_external_provider` de `crates/ai`, que esta rama no toca: conectar con un puerto cerrado tardó más de los 300 ms de la prueba y dio `timeout` en lugar de `unavailable`; en la ejecución siguiente pasó.
- `cargo clippy --workspace --all-targets --locked -- -D warnings` y `cargo fmt --all --check` sin avisos; `npm run build` en `ui/` correcto; `python scripts/check.py` correcto.

| Prueba | Qué comprueba |
| --- | --- |
| `each_mail_keeps_the_address_and_name_of_its_account` (app) | La dirección se obtiene al iniciar sesión (Gmail `profile`; Graph `/me` sin `mail`, con `userPrincipalName`) y se guarda en `cuentas.json` sin tokens. Cada tarea de correo guarda `origin` con cuenta, proveedor, nombre y dirección; quitar la cuenta no lo borra. |
| `an_address_the_provider_does_not_give_stays_absent` (app) | Si el proveedor falla, la cuenta se conecta con `address` ausente (no se escribe `null` inventado en la lista); al renovar la sesión se obtiene y sobrevive a reabrir; si vuelve a fallar se conserva la anterior. |
| `accounts_can_be_renamed_without_changing_their_id` (app) | Renombrar recorta espacios, admite nombres repetidos (la dirección distingue), vacío vuelve a «Microsoft 1» o «Correo simulado», una cuenta inexistente es error; se guarda y sobrevive a reabrir, y los mensajes no se vuelven a encolar porque el ID no cambia. |
| `imported_documents_record_where_they_came_from` (app) | `import_path` guarda nombre, ruta original, fecha, tamaño y SHA-256 (comparado con uno calculado en la prueba). La copia está en `documentos\` y es la que se abre; sin ella, error «ya no está». El resultado lleva el mismo `origin` y `sources` con página 1 y línea de cada campo; la moneda, que no aparece en el texto, no tiene procedencia. Un archivo sin ruta conocida la deja en `null`. |
| `mail_origin_is_kept_in_mail_tasks_and_their_attachments` (app) | `M-0005` («Cambio de horario») llega con remitente sintético, destinatarios y fecha, y origen «Correo simulado · administracion@nestadmin.test». `import_attachment` hereda cuenta, ID del mensaje, remitente y asunto; solo admite tareas de correo; un correo no tiene documento que abrir. |
| `a_draft_without_sender_is_never_sent_by_a_rule` (app) | El caso sin remitente pasa a ser `M-0006` y sigue sin enviarse por regla. |
| `amounts_and_dates_are_found_as_written_with_their_page` (app, unitaria) | `1493.75` se encuentra como `1.493,75` en la página 2 con su línea; `2026-10-05` como `05/10/2026` en la página 1; un importe que no aparece no tiene página. |
| `each_found_text_names_its_page` (extract) | `pages_containing` devuelve las páginas por su `offset`, sin repetir, y nada para textos vacíos o ausentes. |
| `text_scanned_and_mixed_pdfs_use_the_right_method` (extract, `#[ignore]`, requiere PDFium y Tesseract) | Ampliada: en el PDF mixto, la página 2 empieza después de la 1 y el número de factura aparece en ambas. No se ejecutó en esta sesión. |
| `an_older_sample_state_gets_the_synthetic_senders_without_touching_other_messages` (simulators) | Un estado anterior recibe `M-0006` y el remitente de `M-0005`; un mensaje cambiado a mano no se toca y reabrir no cambia nada más. |

## Vista previa (2026-11-05)

Con `?` sin parámetros en `http://localhost:5297`:

- **Pendientes:** el selector «Origen» lista «Todos los orígenes (5)», cada cuenta como «Nombre · dirección (n)» y «Archivos importados (2)». Cada elemento muestra debajo del título «remitente → cuenta · dirección», «Adjunto de facturas@proveedor-alfa.test · Facturación · facturas@empresa.test», «Archivo importado el …» o «Archivo importado (origen no registrado)».
- **Correo «Cambio de horario»:** «Cuenta: Correo simulado · administracion@nestadmin.test (Simulador, datos sintéticos)», «De: logistica@transportes-delta.test», «Para: administracion@nestadmin.test», «Recibido: 6/10/2026». Ya no aparece «(desconectada)».
- **Factura adjunta escaneada:** recuadro de origen (adjunto, cuenta, remitente, asunto, nombre del adjunto, fecha, SHA-256 corto y tamaño) con «Abrir original»; la tabla de campos tiene la columna «Dónde aparece en el original» con «pág. 1 · Fecha: 01/10/2026», «pág. 2 · TOTAL 125,00 €» y, para la moneda, «No localizado en el texto; compruébelo en el original».
- **Correo:** cada cuenta muestra su dirección y un botón «Renombrar»; renombrar Outlook como «Facturación» deja dos cuentas con el mismo nombre distinguidas por la dirección, y el filtro de Pendientes muestra el nombre nuevo.

## Sin verificar

- «Importar documento» con el selector del sistema y «Abrir original» con el visor de Windows no se han probado en la aplicación instalada (en este equipo hay una prueba del instalador en curso). La vista previa solo avisa de lo que haría.
- La dirección con cuentas reales de Gmail y Microsoft 365, y el permiso nuevo `User.Read` en la pantalla de consentimiento de Microsoft.
- Los conectores no descargan adjuntos: el origen «adjunto de correo» no aparece todavía en la aplicación.
- La localización por página con OCR de facturas reales.
