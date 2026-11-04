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

## Pendiente para completar T13

- Ensayo con cuentas de prueba reales de Microsoft 365 y Gmail: requiere registrar la aplicación en Entra ID y en Google Cloud. En particular, falta confirmar con Gmail real que `drafts.send` conserva el ID del mensaje del borrador; si no lo conserva, una respuesta perdida se informa como `not_found` en lugar de confirmarse, sin reenviar.
- Integración en `crates/app` con varias cuentas (IDs y claves prefijados por cuenta), pantalla de inicio de sesión y límite de mensajes leídos por consulta: la capa de aplicación aún usa el simulador.
- Calidad de clasificación y borradores con correo real: sin medir.
