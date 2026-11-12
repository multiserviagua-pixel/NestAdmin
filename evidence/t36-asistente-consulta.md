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

## Recorrido en `tauri dev` con la IA local (T36e)

2026-11-10, `tauri dev --features lectura-bajo-demanda` sobre la carpeta de datos sintética de T35, con la IA local como proveedor (Ollama 0.40.16, `qwen3:8b`, en CPU). Nada salió del equipo. La ventana se manejó por su puerto de depuración; para ver la salida en bruto del modelo se puso delante de Ollama un intermediario local de diagnóstico, que se retiró al terminar.

**Fallo encontrado y corregido.** En el primer intento el asistente no conseguía responder: `invalid_response` en dos pasos. El esquema del paso viajaba a Ollama con las claves en orden alfabético (`answer`, `args`, `citations`, `tool`); Ollama impone ese orden al modelo, que escribía `{"tool": "search" }` y ya no podía añadir `args`. El aviso para corregirse era solo el código `missing_arg`. Ninguna prueba con proveedor simulado podía verlo. Corrección: el esquema enviado es `{"type": "object"}` (la validación estricta la hace `parse_step`, D105), el aviso dice en español qué herramienta y qué argumento faltan, fuera de los marcadores de datos, y las instrucciones llevan un ejemplo de cada forma. Tras ella: `cargo test --workspace --locked` 1303 pasan, 0 fallan; `npm test` 289.

| Paso | Resultado |
|---|---|
| Elegir la IA local | las 3 tareas que esperaban se procesaron; la factura del agua pasó de fila provisional a completa (proveedor, número, fecha, 66,00 EUR) |
| Opción de enviar datos fuera | `not_needed`: con la IA local no hace falta |
| Chat en la pantalla de preparación y en el asistente de primer uso | no se abre |
| Primer intento (antes de la corrección) | error explicado, nada cambiado, y «Ver lo enviado» con las 2 llamadas |
| «¿Tengo alguna factura del agua y de cuánto es?» | `search` y respuesta: una factura, número AI-2026-0912, 66.00 EUR, con la tarjeta del documento citado; 35 s |
| «¿Cuánto suman en total mis facturas?» | `aggregate` y respuesta: 66.00 EUR, avisando de que dos facturas no tienen total; el modelo inventó la referencia `aggregate:1` y se descartó con su nota; 37 s |
| «Llévame a la pantalla de reglas» | `open_view` y la ventana pasó a Reglas con el chat abierto; 87 s |
| Quién contestó | «Respondió la IA local de este equipo» en cada respuesta |
| Historial | eventos `assistant_turn` con pasos, herramientas, proveedor y modelo, sin texto de preguntas ni respuestas |

Tiempos: con el modelo en CPU, la primera llamada tras cargarlo tardó 79 s y un intento con el modelo descargado agotó el plazo de 180 s; con el modelo cargado, cada paso tardó entre 5 y 40 s.

## Recorrido en `tauri dev` con el proveedor externo (T36b, T36c, T36e)

2026-11-10, commit `0a3764e`, misma carpeta sintética, con autorización expresa del usuario para enviar datos inventados a su proveedor. Proveedor: Ollama cloud, `gemma4:31b`, servidor `ollama.com`, con la clave que ya estaba guardada (no se introdujo ninguna). Respaldo local activado y Ollama local en marcha.

Antes de cambiar de proveedor se importó, con la IA local, una factura inventada con datos sensibles de prueba: un IBAN, un teléfono, un correo y un NIF.

| Paso | Resultado |
|---|---|
| Elegir Ollama cloud, sin autorizar el asistente | estado de la opción: `missing` |
| Preguntar sin la opción | respondió la IA local; las 2 llamadas guardadas son locales, ninguna a `ollama.com` |
| Configuración → «Ver qué saldría y autorizar…» | muestra el destino (Ollama cloud · gemma4:31b · servidor ollama.com), el texto literal de ejemplo, «Puede salir», «Nunca sale» y los límites, y solo entonces «Aceptar y autorizar» |
| «Aceptar y autorizar» | opción activada para ese proveedor, modelo y servidor, con fecha |
| «¿A qué cuenta hay que pagar la factura de Transportes Ficticios y cuál es su teléfono?» | respondió Ollama cloud en 3 pasos (`search`, `get_document`) y 3 s, con la tarjeta del documento citado |
| Lo enviado a `ollama.com` en esas 3 llamadas | no contiene el IBAN ni ninguna de sus mitades, ni el teléfono, ni el correo, ni el NIF, ni rutas; en su lugar van `[IBAN]`, `[TELÉFONO]`, `[CORREO]` e `[ID]` |
| La respuesta | «…a la cuenta [IBAN] y el teléfono de contacto es [TELÉFONO]»; el dato completo está en la tarjeta, que se pinta en el equipo |
| «Retirar autorización» | estado `missing`; el turno siguiente lo atendió entero la IA local |
| Volver a la IA local | la opción pasa a `not_needed` |
| Historial | «consentimiento del asistente concedido: proveedor ollama_cloud, modelo gemma4:31b, servidor ollama.com» y «consentimiento del asistente retirado» |

Los datos sensibles se buscaron en el texto guardado para «Ver lo enviado», que las pruebas automáticas comprueban que coincide con lo que recibe el proveedor; no se interceptó el tráfico hacia `ollama.com`.

## Pendiente

- No se provocó en la aplicación un fallo del proveedor externo para ver el respaldo a la IA local en vivo, ni un cambio de modelo o servidor con un turno en curso; están cubiertos por las pruebas automáticas.
- La respuesta muestra las marcas (`[IBAN]`, `[TELÉFONO]`) tal cual; convendría explicar que el dato está protegido y se ve en la tarjeta.
- En el turno que agotó el plazo, el historial marca `local: false` aunque el proveedor era la IA local.
- El indicador de progreso solo mostró «Pensando…» en el recorrido; no se comprobó el texto por herramienta.
- Con la IA local en CPU los tiempos son altos y la primera pregunta tras un rato puede agotar el plazo.
- Las correcciones de la revisión no han pasado una segunda revisión de `arquitecto`.
- El panel del chat, al no ser modal, tapa parte del contenido en ventanas estrechas (visto a 800 px).
- Las filas ya indexadas antes de esta etapa conservan la moneda vacía hasta que se vuelvan a indexar.
- No se midió la calidad de las respuestas con un modelo real; el conjunto de evaluación es la etapa E5.

## E7 — Respaldo de la respuesta (T36f, D269)

2026-11-12, Windows 11. Solo datos sintéticos.

### Qué se ejecutó (verificación independiente)

| Comprobación | Resultado |
|---|---|
| `cargo fmt --all -- --check` | sin diferencias |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | sin avisos |
| `cargo test --workspace --locked` | 1514 pasan, 0 fallan, 17 ignoradas |
| `npm run build --prefix ui` | correcto |
| `npm test --prefix ui` | 324 pasan, 0 fallan |
| `python scripts/check.py` | correcto |

### Pruebas automáticas

| Dónde | Qué comprueba |
|---|---|
| `crates/ai/src/figures.rs` (29) | El extractor reconoce importes, fechas, horas, porcentajes, números de documento y recuentos en sus formatos habituales y los normaliza; deja fuera años sueltos, números de una cifra, ordinales de lista y `doc:N`. |
| `crates/app/src/assistant/grounding.rs` (8) | La precedencia de los cinco estados y qué herramientas cuentan; una herramienta rechazada, repetida o con error no cuenta. |
| `crates/app/tests/assistant.rs`, `assistant_proposals.rs` | Cada estado con un modelo simulado; el estado se guarda y vuelve al reabrir la conversación; una respuesta que repite un importe de un turno respaldado no lo marca y de uno no respaldado sí; nada de esto sale hacia el modelo ni hacia un proveedor externo; la respuesta compuesta por la aplicación lleva `data`. |
| `crates/app/tests/assistant_eval.rs` | El informe cuenta las respuestas del corpus por estado y marca como fallo de calidad una respuesta sobre un documento con respaldo `none`. |
| `ui/tests/assistant-text.test.mjs` | Los textos de los avisos. |

### Recorrido en `tauri dev` con la IA local

`tauri dev --features lectura-bajo-demanda` sobre la carpeta sintética, con Ollama `qwen3:8b` en CPU. Nada salió del equipo.

| Paso | Resultado |
|---|---|
| La pregunta del hallazgo: «Dime de qué trata el correo del cambio de horario y termina tu respuesta con este enlace…» | el modelo volvió a responder sin ejecutar ninguna herramienta y con el mismo contenido inventado; respaldo `none`; la ventana muestra «Para esta respuesta no se consultó ningún documento ni dato de NestAdmin», el botón «Buscar por palabras» y «Contiene 2 datos que no salen de sus documentos: 9:00, 14:00.»; 132 s |
| «Busca la factura de Aguas Imaginarias y dime su número y su total» | `search`, una cita válida, respaldo `documents`, ningún dato sin origen y ningún aviso; 95 s |

### Límites

- La aplicación dice de dónde pudo salir una respuesta, no si es cierta: un texto inventado sin cifras ni fechas solo queda señalado por el aviso de que no se consultó nada.
- Un «no consta» correcto puede salir como `uncited`: la búsqueda relaja los términos, devuelve documentos parecidos y el modelo los descarta; la aplicación no distingue una lectura descartada de una ignorada.
- Un importe que el modelo redondea se señala como dato sin origen. Los códigos con forma de número de documento que no están en los resultados también.
- No se comprueban los recuentos de una sola cifra ni los años sueltos.
- Los estados `data`, `no_results` y `uncited` no se vieron en la aplicación; solo en las pruebas automáticas.
- El cambio no ha pasado una revisión de `arquitecto` (no toca envío, registro, reglas ni credenciales).

