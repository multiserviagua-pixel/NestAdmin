# Evidencia T13 (parcial): conectores de correo Graph y Gmail

Cubre la parte de R13 que no necesita cuentas reales: los conectores de `crates/mail` y su integración con varias cuentas en `crates/app`, `crates/desktop` y `ui/`. Graph, Gmail y el servicio de tokens se simulan en 127.0.0.1 con datos sintéticos (dominio `.test`); ninguna prueba sale a la red ni usa credenciales reales.

## Cómo reproducir

```sh
cargo test -p nestadmin-mail --locked
```

## Ejecución registrada

- Fecha: 2026-11-03
- Plataforma: Windows 11 Pro 10.0.26300
- Rust 1.99.1 (MSVC), `ureq` 3.5.1
- Resultado: 31 pruebas superadas, 0 fallidas (10 unitarias, 13 de Graph, 8 de Gmail y OAuth). `cargo clippy -p nestadmin-mail --all-targets -- -D warnings` sin avisos.

| Área | Qué comprueban las pruebas |
| --- | --- |
| Normalización | Graph y Gmail dan el mismo `Message`: HTML convertido a texto sin scripts ni estilos, parte `text/plain` preferida, adjuntos ignorados, Latin-1 legible y campos ausentes como `None` (asunto vacío, sin destinatarios, sin cuerpo). |
| Paginación | El cursor de Graph se sigue solo si es del mismo origen y ruta; uno de otro servidor se rechaza sin enviar ninguna petición. Gmail usa `pageToken`. |
| Borradores | La misma clave con el mismo contenido devuelve el mismo borrador sin otra llamada; con otro contenido, `conflict`. Destinatarios con saltos de línea, cuerpo vacío o clave vacía no llegan al proveedor. La respuesta va en el hilo (`createReply`; `In-Reply-To`, `References` y `threadId`) en UTF-8. |
| Envío único | Si el envío ocurre y la respuesta se pierde, `lookup` lo confirma consultando el borrador (Graph `isDraft`, Gmail etiqueta `SENT`) y no hay un segundo envío. Un envío que falla sin efecto se repite. Un borrador borrado por el usuario es `not_found` y no se reenvía. Clave reutilizada con otro borrador u otra clave para un borrador enviado: `conflict`. |
| Reinicio | El registro local por cuenta sobrevive a reabrir el conector: misma clave, mismo borrador; envío pendiente reconciliado. |
| Tokens | Token caducado: se renueva antes de llamar. 401: una renovación y un reintento. Renovación rechazada: `unauthorized` sin insistir. Ni errores ni `Debug` muestran tokens o el secreto de cliente. |
| Inicio de sesión | URL con `client_id`, `redirect_uri` en 127.0.0.1, `state` y reto S256; el verificador canjeado corresponde al reto (incluido el ejemplo de RFC 7636). `state` distinto, consentimiento denegado o tiempo agotado: `unauthorized` sin canjear ningún código. |
| Errores | 403 → `unauthorized`, 404 → `not_found`, 400 → `invalid_input`, 409 → `conflict`, 429 y 500 → `unavailable`. |

## Integración en la aplicación (D42–D45)

```sh
cargo test -p nestadmin-app --locked --test mail_accounts
cargo test -p nestadmin-desktop --locked
npm run build --prefix ui
```

- Fecha: 2026-11-04
- Plataforma: Windows 11 Pro 10.0.26300, Rust 1.99.1 (MSVC)
- Resultado: 7 pruebas de `tests/mail_accounts.rs` y 2 unitarias de `accounts.rs` superadas; las 11 de `tests/flows.rs` siguen pasando con el simulador como cuenta. En `crates/desktop`, 3 pruebas de lectura de `correo-oauth.json`. Las cifras de todo el espacio de trabajo están en la descripción del PR.

Gmail y Graph se simulan con un servidor HTTP propio de la prueba en 127.0.0.1 (mensajes de `proveedor-*.test`, tokens `acceso-sintetico`), y los tokens se guardan en un `MemoryCredentialStore`.

| Área | Qué comprueban las pruebas |
| --- | --- |
| Varias cuentas | Simulador, Microsoft y Google se leen en la misma consulta; IDs `microsoft-1:g1`, `google-1:m2`, `simulador:M-…`; una segunda consulta no encola nada. |
| Tokens | Quedan en el almacén como `mail/microsoft-1` y `mail/google-1`; `correo/cuentas.json` no los contiene. Desconectar los borra; si se borran por fuera, la cuenta figura «sin sesión». |
| Revisión (R07) | Sin regla activa, todas las respuestas quedan en revisión y Graph no recibe ningún borrador. Procesar no vuelve a pedir el mensaje al proveedor. |
| Envío único | Aprobada una respuesta de Microsoft, el borrador va al mensaje original y el recibo lleva `account` e IDs con prefijo. Si el envío ocurre y la respuesta se pierde, la tarea falla visible; tras reiniciar, reintentar y aprobar, se confirma como `reconciled` sin un segundo envío. El registro local guarda las claves `microsoft-1/task-N/…`. |
| Fallos aislados | Gmail con 500 → `unavailable`; Microsoft con sesión caducada sin renovación → `unauthorized` sin el token en el mensaje; el simulador se lee igualmente y la pantalla muestra el último error de cada cuenta. Volver a iniciar sesión lo corrige. |
| Límite | 0 y 501 se rechazan; con límite 3, ninguna cuenta lee más de 3 y Graph no pide una segunda página. |
| Configuración | Sin cliente OAuth, las cuentas reales figuran «proveedor no configurado» y no se puede iniciar sesión; la lista sobrevive al reinicio. El archivo de ejemplo no configura nada; uno roto se informa. |

Pantalla «Correo», comprobada en la vista previa de la interfaz (`npm run dev --prefix ui`, datos simulados de `ui/src/mock.ts`): tabla de cuentas con proveedor, estado, última consulta (leídos y nuevos, o el error en rojo con su motivo) y botones «Volver a iniciar sesión» y «Desconectar» (con confirmación); tarjeta «Añadir cuenta» con nombre opcional y botones de Google y Microsoft, que muestran «Esperando al navegador…» con la dirección y «Cancelar»; tarjeta del límite por cuenta. «Consultar correo» resume los mensajes nuevos y las cuentas que fallaron. `?sin-cuentas` muestra la pantalla vacía y `?sin-cliente`, una instalación sin clientes OAuth. En la revisión de una respuesta aparece la cuenta de origen. El flujo OAuth real desde la ventana Tauri no se ha ejecutado.

## Pendiente para completar T13

- Ensayo con cuentas de prueba reales de Microsoft 365 y Gmail: requiere registrar la aplicación en Entra ID y en Google Cloud. En particular, falta confirmar con Gmail real que `drafts.send` conserva el ID del mensaje del borrador; si no lo conserva, una respuesta perdida se informa como `not_found` en lugar de confirmarse, sin reenviar.
- Recorrido con una cuenta real desde la ventana: añadir la cuenta, consultar, aprobar un envío y desconectar. Requiere `correo-oauth.json` con los clientes del editor (D43).
- Si tras una respuesta perdida la tarea se reprocesa y la IA propone otro texto, crear el borrador con la misma clave da `conflict` y la tarea queda fallida sin enviar: nunca hay doble envío, pero hace falta una salida para el usuario (comportamiento heredado del núcleo, también con el simulador).
- Calidad de clasificación y borradores con correo real: sin medir.
