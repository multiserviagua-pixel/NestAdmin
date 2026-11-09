# T28 — Triaje en la versión de desarrollo y revisión de seguridad

2026-11-09, Windows 11, `scripts/dev-pruebas.ps1` (`tauri dev`, datos en la carpeta de pruebas, fuera del repositorio), IA en Ollama cloud con `gemma4:31b`, sin reglas activas. Esquema migrado de 4 a 7 al arrancar, con las copias previas.

## Recorrido

Se añadieron al buzón simulado cuatro correos sintéticos y se pulsó «Consultar correo» por UI Automation. La consulta también leyó un correo nuevo de la cuenta de Gmail de pruebas.

| Correo | Resultado |
|---|---|
| Remitente `no-reply@…` | `completed`, ruta `informativo`, código `noreply_address`, sin IA ni borrador |
| Remitente `MAILER-DAEMON@…` | en revisión, ruta `rebote`, sin IA ni borrador |
| Pregunta de cliente por el horario | en revisión con borrador; sin perfil, el borrador dice que se confirmará el horario y no lo inventa |
| «Recibido, muchas gracias» | en revisión, `needs_reply` falso, sin borrador |
| Correo nuevo de Gmail (real) | en revisión, `needs_reply` falso, sin borrador |

No se aprobó ni se envió nada.

## Revisión de seguridad (E0–E6)

Revisión independiente de `63d65b2..eb8cc84` con un remitente que controla el correo entero. Sin hallazgos de gravedad alta. Corregidos en este commit:

- **Media:** el asunto `Re: …` lo escribe el remitente y salía sin examinar; ahora `draft_flags` examina asunto y cuerpo, y una regla no envía un asunto con IBAN, enlace o teléfono (prueba en `crates/app/tests/triage.rs`). El asunto ya no repite `Re: Re:`.
- **Baja:** el `$select` de Graph llevaba un salto de línea literal; corregido.

Comprobado sin problemas: compuerta de reglas cerrada en todos los casos, perfil solo con consentimiento y solo su huella guardada, sin respaldo hacia otro externo, consultas SQL con parámetros, la interfaz inserta texto sin HTML, cabeceras sin CRLF, y `X-NestAdmin-Id` no sirve para falsificar D156 ni D162.

## Sin cubrir

- Pantallas de Informativos, Archivar, Redactar, preferencias y perfil recorridas en la vista previa con datos simulados, no en la ventana de `tauri dev`.
- Cabeceras de Graph con una cuenta real (la de Microsoft está desconectada) y `last_message_is_ours` en Graph, que devuelve desconocido.
- Calidad con correo de negocio real (E7).

## 2026-11-09 — Cabeceras de Outlook con la cuenta real

Se volvió a conectar la cuenta personal de Outlook en `tauri dev` (sin pedir inicio de sesión: Microsoft usó la sesión abierta del navegador), se envió un correo sintético desde la cuenta de Gmail de pruebas y se pulsó «Consultar correo».

| Comprobación | Resultado |
|---|---|
| Señales del mensaje de Outlook | `signals` = `[]`: comprobadas y sin señales. El listado de Graph devuelve `internetMessageHeaders` con `$select`, así que estos correos ya no quedan como «sin comprobar». |
| Identificador del hilo | `thread_key` presente (`conversationId`) |
| Triaje | en revisión, `needs_reply` falso, sin borrador (el correo decía que no requería respuesta) |
| Desconectar al terminar | sin credenciales de la cuenta |

**Sin cubrir:** un correo real de Outlook con señales (`Auto-Submitted`, `List-*`) para ver los códigos; `last_message_is_ours` en Graph sigue devolviendo desconocido.
