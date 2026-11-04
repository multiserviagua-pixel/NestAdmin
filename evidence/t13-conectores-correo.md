# Evidencia T13 (parcial): conectores de correo Graph y Gmail

Cubre los conectores de `crates/mail` y su integración con varias cuentas en `crates/app`, `crates/desktop` y `ui/`. Graph, Gmail y el servicio de tokens se simulan en 127.0.0.1 con datos sintéticos (dominio `.test`); ninguna prueba automática sale a la red ni usa credenciales reales. La prueba manual con Gmail real se describe aparte.

## Cómo reproducir

```sh
cargo test -p nestadmin-mail --locked
```

## Ejecución registrada

- Fecha: 2026-11-03
- Plataforma: Windows 11 Pro 10.0.26300
- Rust 1.99.1 (MSVC), `ureq` 3.5.1
- Resultado: 31 pruebas superadas, 0 fallidas (10 unitarias, 13 de Graph, 8 de Gmail y OAuth). `cargo clippy -p nestadmin-mail --all-targets -- -D warnings` sin avisos.

Reconciliación de Gmail con `Message-ID` propio (D41), 2026-11-04, mismo equipo y versiones: 35 pruebas superadas, 0 fallidas (11 unitarias, 13 de Graph sin cambios, 11 de Gmail y OAuth); clippy sin avisos. El Gmail simulado da ahora al mensaje enviado un ID distinto del borrador, como el real.

| Área | Qué comprueban las pruebas |
| --- | --- |
| Normalización | Graph y Gmail dan el mismo `Message`: HTML convertido a texto sin scripts ni estilos, parte `text/plain` preferida, adjuntos ignorados, Latin-1 legible y campos ausentes como `None` (asunto vacío, sin destinatarios, sin cuerpo). |
| Paginación | El cursor de Graph se sigue solo si es del mismo origen y ruta; uno de otro servidor se rechaza sin enviar ninguna petición. Gmail usa `pageToken`. |
| Borradores | La misma clave con el mismo contenido devuelve el mismo borrador sin otra llamada; con otro contenido, `conflict`. Destinatarios con saltos de línea, cuerpo vacío o clave vacía no llegan al proveedor. La respuesta va en el hilo (`createReply`; `In-Reply-To`, `References` y `threadId`) en UTF-8. |
| Envío único | Si el envío ocurre y la respuesta se pierde, `lookup` lo confirma y no hay un segundo envío: Graph por `isDraft`; Gmail, aunque el mensaje enviado tenga otro ID, buscando con `rfc822msgid:` el mensaje con etiqueta `SENT` y el `Message-ID` propio del borrador, y el recibo lleva el ID real (si Gmail conservara el ID, basta la etiqueta `SENT`). Otro mensaje enviado por el usuario en el mismo hilo no se toma por el nuestro. Un envío que falla sin efecto se repite. Un borrador borrado por el usuario es `not_found` y no se reenvía. Clave reutilizada con otro borrador u otra clave para un borrador enviado: `conflict`. |
| Reinicio | El registro local por cuenta sobrevive a reabrir el conector: misma clave, mismo borrador; envío pendiente reconciliado. En Gmail, el `Message-ID` propio se guarda en el registro y un conector nuevo confirma con él el envío cortado, sin segundo envío. |
| Tokens | Token caducado: se renueva antes de llamar. 401: una renovación y un reintento. Renovación rechazada: `unauthorized` sin insistir. Ni errores ni `Debug` muestran tokens o el secreto de cliente. |
| Inicio de sesión | URL con `client_id`, `redirect_uri` en 127.0.0.1, `state` y reto S256; el verificador canjeado corresponde al reto (incluido el ejemplo de RFC 7636). `state` distinto, consentimiento denegado o tiempo agotado: `unauthorized` sin canjear ningún código. |
| Errores | 403 → `unauthorized`, 404 → `not_found`, 400 → `invalid_input`, 409 → `conflict`, 429 y 500 → `unavailable`. |

## Prueba con una cuenta de Gmail real

`crates/mail/examples/probar_gmail.rs` recorre el conector con una cuenta de prueba real desde la terminal, sin la aplicación. Conviene usar una cuenta creada para la prueba, no un buzón de trabajo. Los tokens van al administrador de credenciales de Windows (`mail/prueba-gmail`). El cliente OAuth y el registro local de borradores van a `%LOCALAPPDATA%\NestAdmin-pruebas`, fuera del repositorio. Sin `--mostrar` no imprime nada del contenido del correo, y ningún resultado con datos reales se guarda en el repositorio.

Registro previo en Google Cloud Console (una vez):

1. Crear un proyecto y habilitar **Gmail API** (APIs y servicios → Biblioteca).
2. **Pantalla de consentimiento OAuth:** tipo «Externo», estado «Prueba», y añadir la cuenta de prueba en «Usuarios de prueba». El permiso `gmail.modify` es restringido, pero en estado de prueba funciona sin verificación de Google para esos usuarios. Los tokens de renovación caducan a los 7 días.
3. **Credenciales → Crear ID de cliente OAuth**, tipo **«Aplicación de escritorio»**, y descargar el JSON. No hace falta registrar la redirección: Google admite cualquier puerto de `127.0.0.1` para ese tipo. El archivo no debe entrar en el repositorio.

Recorrido:

```powershell
cargo run -p nestadmin-mail --example probar_gmail -- entrar C:\ruta\client_secret_XXXX.json   # abre el navegador; PKCE y state
cargo run -p nestadmin-mail --example probar_gmail -- leer                                     # recuentos, campos presentes y segunda página
cargo run -p nestadmin-mail --example probar_gmail -- borrador <id_mensaje> <su_propia_dirección>  # respuesta en el hilo; repetir la clave da el mismo ID
cargo run -p nestadmin-mail --example probar_gmail -- enviar <id_borrador> --confirmar         # envío real; repetir la clave no reenvía
cargo run -p nestadmin-mail --example probar_gmail -- salir                                    # borra los tokens
```

Para el envío, conviene responder a un mensaje que la cuenta se haya enviado a sí misma, con su propia dirección como destinatario. Así la prueba no escribe a nadie más.

### Ejecución registrada (Gmail)

- Fecha: 2026-11-04. Windows 11 Pro, equipo de desarrollo.
- Cuenta: una cuenta de Gmail de prueba, añadida como usuario de prueba de un proyecto de Google Cloud en estado «Prueba», con cliente «Aplicación de escritorio».
- No se anotan IDs de mensajes ni contenido de la cuenta.

| Paso | Resultado |
| --- | --- |
| `entrar` | Inicio de sesión en el navegador del usuario, tras el aviso «Google no verificó esta app» propio del estado de prueba. Redirección a `127.0.0.1` con PKCE y `state` aceptada. Los tokens se guardaron en el administrador de credenciales. |
| `leer` | 10 mensajes en la primera página y 7 en la segunda, cursor incluido, en 6 s. Todos llegaron con asunto, remitente, destinatarios, fecha y cuerpo de texto (entre unos 1 000 y 5 800 caracteres). |
| `buscar` | Localizó por el asunto el mensaje que la cuenta se envió a sí misma. |
| `borrador` | Creó la respuesta en el hilo de ese mensaje, dirigida solo a la propia cuenta. Repetir la clave devolvió el mismo ID de borrador, sin crear otro. |
| `enviar --confirmar` | Envió el borrador una vez. Repetir la clave devolvió el recibo guardado, sin un segundo envío. |

**Hallazgo:** Gmail **no conserva** el ID del mensaje del borrador al enviarlo. El mensaje enviado tiene un ID distinto del que tenía el borrador. Por tanto, si el proceso se corta después de enviar y antes de anotar el recibo, la reconciliación (D33) no puede confirmar el envío por la etiqueta `SENT`: lo informa como `not_found` y no reenvía. Es el comportamiento seguro previsto, pero deja la respuesta sin confirmar. No se ha provocado ese corte con la cuenta real.

**Corrección (D41), solo probada con el Gmail simulado:** cada borrador lleva un `Message-ID` propio guardado en el registro local, y la reconciliación busca con él el mensaje enviado. Esta ejecución real fue anterior al cambio.

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

- Ensayo con una cuenta de prueba real de Microsoft 365: requiere registrar la aplicación en Entra ID.
- Gmail: provocar con la cuenta real un corte entre el envío y el recibo y comprobar que la reconciliación por `Message-ID` propio (D41) lo confirma; en particular, que Gmail mantiene esa cabecera al enviar. La parte simulada está hecha.
- Recorrido con una cuenta real desde la ventana: añadir la cuenta, consultar, aprobar un envío y desconectar. Requiere `correo-oauth.json` con los clientes del editor (D43).
- Si tras una respuesta perdida la tarea se reprocesa y la IA propone otro texto, crear el borrador con la misma clave da `conflict` y la tarea queda fallida sin enviar: nunca hay doble envío, pero hace falta una salida para el usuario (comportamiento heredado del núcleo, también con el simulador).
- Calidad de clasificación y borradores con correo real: sin medir.
