# T33 — Aprender a responder en la versión de desarrollo

2026-11-10, Windows 11, `scripts/dev-pruebas.ps1` (`tauri dev`, datos en la carpeta de pruebas, fuera del repositorio), IA en Ollama cloud con `gemma4:31b`. Solo datos sintéticos y recuentos.

## Buzón simulado

14 correos: 3 hilos sintéticos de un cliente con su respuesta propia en Enviados y 1 reenvío interno. Se terminó la primera lectura del simulador y después apareció la tarjeta «Aprender a responder».

## Vista previa

Alias R1 en lugar del nombre y la dirección, citas recortadas y texto delimitado. No salió nada antes de consentir.

## Primer intento: peticiones de presupuesto

El modelo clasificó los 3 pares como `otros`, que es lo correcto con el catálogo de 5 categorías de facturación. No salió ninguna regla. **Limitación anotada:** las respuestas a temas generales caen en `otros` y no generan reglas (ver Pendientes abiertos en `specs/v1/tareas.md`).

## Segundo intento: consultas de pago

| Paso | Resultado |
|---|---|
| Descartes | 1 par descartado por repetir el asunto dentro de una frase; corregido en `a7d57f9` |
| Tras la corrección | 3 pares y 1 descartado por reenvío interno (`not_reply_to_sender`) |
| Envío | a la IA externa, solo tras la vista previa y el consentimiento |
| Perfil | propuesta «formal, breve», con saludo y despedida; aplicada |
| Reglas | 1 regla propuesta con su evidencia; «Aceptar y activar las marcadas (1)» |
| Estado de la regla | Activa · Aprendida · En rodaje: 0 de 3 |
| «Desactivar las aprendidas» | pidió confirmación y dejó la regla inactiva |

No se envió ningún correo.

## Sin cubrir en la aplicación

- El rodaje con un correo nuevo (cubierto por T33g en pruebas).
- La IA local.
- Las cuentas reales y los Enviados reales de Graph y Gmail.
