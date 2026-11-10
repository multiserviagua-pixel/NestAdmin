# T36 — Asistente de consulta del chat (E2)

2026-11-10, Windows 11, commit `243cf3a`. Solo datos sintéticos. Cubre la etapa E2 del chat de la aplicación (D182–D184 en su nivel libre, D187, D188, D190; R33): el asistente con IA que responde con citas. No ejecuta acciones (eso es E3).

## Qué se ejecutó

| Comprobación | Resultado |
|---|---|
| `cargo fmt --all` | sin cambios pendientes |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | sin avisos |
| `cargo test --workspace --locked` | 1297 pasan, 0 fallan |
| `npm run build --prefix ui` (`tsc --noEmit` y `vite build`) | correcto |
| `npm test --prefix ui` | 289 pasan, 0 fallan |
| `python scripts/check.py` | correcto |

## Pruebas automáticas por caso

Todas con un proveedor simulado con guion y un proveedor externo espía que registra cada petición (`crates/app/tests/assistant.rs`, `crates/ai/src/assistant.rs`, `crates/core/tests/assistant.rs`).

| Caso | Qué comprueba |
|---|---|
| T36a | Herramienta válida y respuesta con citas; una cita no devuelta por una herramienta se descarta; herramienta o argumento fuera de la lista cuenta como salida no válida; tope de 6 pasos y de 2 salidas no válidas; cancelación; la cifra viene de `aggregate`; `open_view` solo anota la vista. Un documento sintético con instrucciones dentro llega delimitado como dato y no consigue ni una herramienta ni una cita. |
| T36b | Con proveedor externo y sin la opción, el espía externo recibe cero peticiones: el turno usa la IA local o devuelve `consent_required` con la búsqueda por palabras. La opción se retira al cambiar proveedor, modelo o servidor, queda auditada y ninguna herramienta puede concederla. Retirada o con el destino cambiado entre el inicio y la ejecución del turno, no sale nada. Sin IA local en marcha, o con el respaldo local desactivado, el asistente no arranca nada. |
| T36c | Lo enviado respeta los topes de turnos, fragmentos y caracteres. Centinelas sintéticos de IBAN, teléfono, identificador fiscal, correo y credencial no aparecen en ninguna petición externa, tampoco cuando cruzan el corte de un fragmento (se comprueban las dos mitades por separado) ni cuando vienen de un turno anterior respondido por la IA local. Ninguna herramienta devuelve rutas de archivo, nombres de cuenta ni direcciones de buzón. Lo guardado para «Ver lo enviado» coincide con lo que recibió el espía. Respaldo solo de externo a local, un único intento al externo por turno ante `unavailable`, `timeout` y `rate_limited`, sin abrir el periodo de respaldo de la cola. `unauthorized`, `no_credit` e `invalid_response` no respaldan ni dejan efectos. |
| T36d | Los turnos se guardan y se recuperan; borrar una conversación o todas elimina turnos y llamadas; `audit_log` no contiene el texto de la pregunta ni de la respuesta. Un turno con error o cancelado conserva lo que llegó a enviar y queda fuera del historial de los siguientes. |

La prueba del dato partido falla si se restaura el troceo anterior a la corrección, de modo que comprueba lo que dice.

## Revisión

`arquitecto` revisó el fondo (commit `9c8b35b`) con la lista de `revisar-envio-reglas`. Encontró un fallo bloqueante (un dato sensible partido entre dos fragmentos salía sin enmascarar) y cuatro importantes (envíos sin rastro en errores, el asistente arrancaba la IA local, el respaldo se decidía en cada paso y alteraba la cola, y huecos en las pruebas). Todos corregidos en `243cf3a`; las decisiones quedaron en D179, D182, D183, D188 y D190. Las correcciones no han pasado una segunda revisión.

## Recorrido en la vista previa del navegador (no es `tauri dev`)

Con `npm run dev` de `ui/` y los datos sintéticos de `ui/src/mock.ts`:

| Paso | Resultado |
|---|---|
| Preguntar sin la autorización | explica el motivo, ofrece «Abrir Configuración» y muestra la búsqueda por palabras con su aviso |
| «Abrir Configuración» | lleva al bloque «Asistente»; el chat sigue abierto |
| «Ver qué saldría y autorizar…» | destino (proveedor, modelo y servidor), texto literal de un envío de ejemplo, «Puede salir», «Nunca sale» y los límites, antes del botón «Aceptar y autorizar» |
| Autorizar y repetir la pregunta | respuesta con la tarjeta del documento citado, «Respondió Ollama cloud · gemma4:31b» y «Ver lo enviado» |
| Una URL en la respuesta | se ve como texto, no como enlace |

## Pendiente

- **Recorrido en `tauri dev` con una IA real** (T36e): no se ha hecho. Falta ver un turno completo contra un proveedor o contra la IA local, «Ver lo enviado» con el texto real y la opción de Configuración en la aplicación.
- Las correcciones de la revisión no han pasado una segunda revisión de `arquitecto`.
- El panel del chat, al no ser modal, tapa parte del contenido en ventanas estrechas (visto a 800 px).
- Las filas ya indexadas antes de esta etapa conservan la moneda vacía hasta que se vuelvan a indexar.
- No se midió la calidad de las respuestas con un modelo real; el conjunto de evaluación es la etapa E5.
