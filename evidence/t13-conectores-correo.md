# Evidencia T13 (parcial): conectores de correo Graph y Gmail

Cubre la parte de R13 que no necesita cuentas reales, en `crates/mail`. Graph, Gmail y el servicio de tokens se simulan en 127.0.0.1 con datos sintéticos (dominio `.test`); ninguna prueba sale a la red ni usa credenciales reales.

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

## Pendiente para completar T13

- Ensayo con una cuenta de prueba real de Microsoft 365: requiere registrar la aplicación en Entra ID.
- Gmail: mejorar la reconciliación tras un corte durante el envío, ya que el ID del borrador no sirve. Por ejemplo, buscar en el hilo un mensaje enviado con la cabecera `Message-ID` que generó el borrador. Hay que probarlo provocando el corte.
- Integración en `crates/app` con varias cuentas (IDs y claves prefijados por cuenta), pantalla de inicio de sesión y límite de mensajes leídos por consulta: la capa de aplicación aún usa el simulador.
- Calidad de clasificación y borradores con correo real: sin medir.
