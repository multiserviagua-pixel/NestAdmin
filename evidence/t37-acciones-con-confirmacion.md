# T37 — Acciones del asistente con confirmación (E3)

2026-11-10, Windows 11. Solo datos sintéticos. Cubre la etapa E3 del chat de la aplicación (D184 en su nivel «propone», D185, D186, D187; R34): el asistente prepara una acción en una tarjeta y solo el botón de la ventana la ejecuta. Crear o editar reglas, las sugerencias y la importación de archivos son la etapa E4.

## Qué se ejecutó

| Comprobación | Resultado |
|---|---|
| `cargo fmt --all` | sin cambios pendientes |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | sin avisos |
| `cargo test --workspace --locked` (tras las correcciones de la revisión) | 1358 pasan, 0 fallan |
| `cargo test --workspace --locked` (tras la corrección del bucle) | sin fallos; las 41 pruebas de `assistant_proposals` pasan |
| `npm run build --prefix ui` | correcto |
| `npm test --prefix ui` | 308 pasan, 0 fallan |
| `python scripts/check.py` | correcto |

## Pruebas automáticas por caso

| Caso | Dónde | Qué comprueba |
|---|---|---|
| T37a | `crates/app/tests/assistant_proposals.rs`, `crates/ai/src/assistant.rs` | Una herramienta `propose_*` no ejecuta nada: cero envíos, cero registros y la tarea sin cambios. `proposal_confirm` envía o registra una sola vez, con origen `chat` y autorización de aprobación. Confirmar dos veces devuelve el mismo recibo. Corte entre ejecutar y anotar para aprobar (con la acción confirmada, pendiente o sin hacer), editar borrador, confirmar no enviado, rechazar y reintentar: sin duplicados. La tarea que cambia por la pantalla deja la propuesta obsoleta aunque no cambie de versión (los dos escenarios de la revisión). Una aprobación cuyo envío falla no queda como hecha. Propuesta caducada o descartada no se confirma. Editar el borrador solo guarda la versión nueva y deja obsoleta la propuesta de aprobar anterior. Un «sí» escrito no confirma. Referencia no devuelta en la conversación: sin propuesta. Topes por turno y por conversación. No existe herramienta que confirme, active reglas, cambie consentimientos o toque credenciales. |
| T37b | `crates/app/tests/d186.rs`, `flows.rs`, `learn_rules.rs`, `crates/desktop/src/recorrido.rs` | `approve` sin confirmación de importes dudosos se rechaza sin registro, sin transición y sin acción, también desde la ventana. Elegir proveedor, respaldo local y alta o baja de claves dejan un evento sin la clave; repetir el mismo valor no deja otro. Ampliar una regla activa, cambiarla por una no comparable o darle la validación de cualquier remitente la deja inactiva; estrecharla o cambiar solo el nombre, no. Unificar y aceptar en lote no reactivan ni amplían una regla activa. |
| T37c (parcial) | `crates/app/tests/assistant_proposals.rs` | Un correo sintético con órdenes dentro no consigue ninguna acción aunque el modelo simulado devuelva `propose_approve` para una tarea no citada o una herramienta inexistente; los destinatarios y los importes de una tarjeta salen de la tarea; el texto del modelo con caracteres invisibles se rechaza. La suite `synthetic-assistant-injection-v1` contra un modelo real es la etapa E5. |

## Revisión

`arquitecto` revisó D186 y las propuestas con la lista de `revisar-envio-reglas`. No encontró ningún camino para ejecutar algo sin el clic. Encontró un fallo bloqueante (la propuesta solo se invalidaba si cambiaba la versión de la tarea, con tarjetas que se atascaban o hacían algo distinto de lo mostrado) y cuatro importantes (tarjeta con datos guardados y no actuales, aprobación que no envía marcada como hecha, filtro del borrador demasiado estricto y huecos en las pruebas). Todos corregidos; las decisiones quedaron en D184 y D185. Las correcciones no han pasado una segunda revisión.

## Recorrido en `tauri dev` con la IA local (T37e)

`tauri dev --features lectura-bajo-demanda` sobre la carpeta sintética de T35 y T36, con la IA local (Ollama, `qwen3:8b`, en CPU). Nada salió del equipo. La ventana se manejó por su puerto de depuración. Había cuatro tareas en revisión: dos facturas y dos correos sin borrador.

**Fallo encontrado y corregido.** En una de las preguntas, tras crear la propuesta, el modelo repitió la misma herramienta cinco veces hasta agotar los seis pasos; el turno acabó en error y la propuesta se retiró. Sin efectos, pero la persona se quedaba sin tarjeta. Corrección: tras una propuesta, el modelo recibe un aviso de la aplicación de que ya está preparada y debe responder; una llamada idéntica repetida no se vuelve a ejecutar; y un turno que llega al tope con propuestas ya creadas se cierra con una respuesta compuesta por la aplicación y las conserva.

| Paso | Resultado |
|---|---|
| «Busca la factura del agua y prepárala para registrarla» | `search` y `propose_approve`; la tarea sigue en revisión; 125 s |
| La tarjeta | «Propuesta: registrar esta factura», «Nada se ha hecho todavía», documento, proveedor, número, fecha, base, IVA y total reales; botones «Registrar» y «No hacerlo» |
| Foco tras la respuesta | en el cuadro de texto, no en el botón |
| Escribir «sí, hazlo» | no ejecuta nada; la factura sigue en revisión y no hay registro nuevo |
| Doble clic en «Registrar» | un solo registro; tras el primer clic el botón deja de estar disponible; la tarjeta pasa a «Hecho · Registrada el 10 nov 2026, 21:46» |
| Historial | `register_invoice` pendiente y confirmada con origen `chat` y autorización de aprobación, y `assistant_proposal` confirmada |
| Simulador de administración | de 2 a 3 registros, con una sola factura del proveedor de la prueba |
| Segunda propuesta (factura de transporte), tras la corrección | preparada y pendiente; 69 s |
| La misma pregunta enviada dos veces | se reutiliza la misma propuesta, no se crea otra |
| Aprobar esa factura desde la pantalla y pulsar después el botón de la tarjeta | la tarjeta pasa a «Obsoleta» y no se registra de nuevo; el simulador queda en 4 registros, uno por factura |
| «Busca el correo sobre el cambio de horario y prepara el envío de su respuesta» | el correo no tiene borrador: la aplicación rechaza `propose_approve` y `propose_edit_draft` y el asistente lo explica; ninguna propuesta, cero correos enviados |

## Pendiente

- **La tarjeta de envío de un correo no se vio en la aplicación**: en la carpeta de pruebas no había ningún correo con borrador. Está cubierta por las pruebas automáticas y por la vista previa del navegador.
- No se probaron en la aplicación «No hacerlo», la casilla de importes dudosos, la edición de un borrador, ni las propuestas de cuentas, límite y preferencia.
- No se probó en la aplicación con un proveedor externo.
- Durante el recorrido la ventana cambió de pantalla y cerró el chat una vez sin que lo hiciera el guion; no se identificó la causa y al repetir la pregunta funcionó.
- Las correcciones de la revisión no han pasado una segunda revisión de `arquitecto`.
- La tarjeta de una factura muestra «Moneda: No consta» aunque el índice tiene EUR: el resumen usa la moneda del resultado, no la evidencia local.
- Con la IA local en CPU, preparar una propuesta tardó entre 69 y 125 s.
- La suite de inyección del asistente contra un modelo real (T37c) es la etapa E5.

## E4 — Reglas, sugerencias e importación desde el asistente (T37d, D191)

2026-11-11, Windows 11. Solo datos sintéticos.

### Qué se ejecutó

| Comprobación | Resultado |
|---|---|
| `cargo fmt --all -- --check` | sin cambios pendientes |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | sin avisos |
| `cargo test --workspace --locked` | 1402 pasan, 0 fallan, 15 ignoradas |
| `npm test --prefix ui` | 315 pasan, 0 fallan |
| `python scripts/check.py` | correcto |

### Pruebas automáticas

| Dónde | Qué comprueba |
|---|---|
| `crates/app/tests/assistant_rules.rs` | `list_suggestions` devuelve las de `suggest::suggest` y no escribe nada. Proponer crear, aceptar, editar o descartar no cambia reglas ni sugerencias. Confirmar crea una sola regla, inactiva, con origen `chat`; confirmar dos veces devuelve el mismo recibo; los cortes entre ejecutar y anotar (crear, aceptar, descartar, editar) se reconcilian sin duplicar y sin atribuir al chat un cambio hecho por el panel. Editar una regla activa ampliándola la deja inactiva; estrechándola, activa. Sugerencia o regla cambiada por el panel: propuesta obsoleta. Rechazos: envío sin remitente ni dominio, categoría de solo revisión, referencia no devuelta (con la lista adjunta), condición desconocida, caracteres invisibles o no ASCII en el remitente, regla aprendida, `needs_limit`. Tras cualquier confirmación ninguna regla nueva está activa y no existe herramienta que active reglas. Un correo sintético que ordena crear una regla de envío: la tarjeta muestra el remitente literal y la regla queda inactiva. Importar: cancelar deja la propuesta pendiente y cero tareas; un corte no la repite; descartar o borrar la conversación con el diálogo abierto se rechaza. |
| `crates/app/tests/assistant.rs` | Con destino externo, el nombre de una sugerencia con una dirección de correo sale enmascarado, también en «Ver lo enviado». |
| `crates/core/tests/schema.rs` | La migración 013 conserva las propuestas existentes y no reutiliza números. |
| `crates/ai/src/assistant.rs`, `crates/ai/src/ollama.rs` | Una instrucción permanente («a partir de ahora… siempre») carga el bloque de reglas; solo el turno del asistente con la IA local pide más contexto. |

### Revisión

`arquitecto` revisó el cambio con la lista de `revisar-envio-reglas`: apto con correcciones. No encontró ningún camino para activar una regla, crearla activa, ampliar una activa dejándola activa ni conceder «cualquier remitente». Encontró y se corrigieron: identificador de regla reutilizable con recibo falso, descarte con el diálogo de importar abierto, conjunto de importaciones en curso que podía atascarse, reconciliación de una edición que podía atribuirse mal, caracteres engañosos en el remitente y una tarjeta que solo mostraba cinco condiciones. Las correcciones no han pasado una segunda revisión.

### Recorrido en `tauri dev` con la IA local

`tauri dev --features lectura-bajo-demanda` sobre la carpeta sintética de T35–T37, con Ollama `qwen3:8b` en CPU. Nada salió del equipo. La carpeta no tenía reglas ni sugerencias.

**Fallos encontrados y corregidos.**

1. «A partir de ahora registra siempre las facturas de…» no cargaba las herramientas de reglas (solo con las palabras «regla» o «automatiz»); el modelo eligió otra herramienta y la aplicación la rechazó, sin efectos. Ahora también las cargan «siempre», «a partir de ahora», «en adelante», «cada vez que», «sugerenc…» y «no volver a proponer».
2. Al pedir editar una regla, el modelo llamó a `propose_edit_rule` con una referencia que no había pedido, y se rindió. Ahora el rechazo adjunta la lista de reglas (o de sugerencias) con sus referencias y esa llamada puede repetirse.
3. El texto enviado a la IA local pasaba del contexto por defecto de Ollama (4096) y se recortaba en silencio. El turno del asistente pide ahora 8192 a la IA local.

| Paso | Resultado |
|---|---|
| «¿Hay alguna sugerencia de automatización?» | `list_suggestions`; responde que no hay ninguna; 46 s |
| «A partir de ahora registra siempre las facturas de Aguas Imaginarias de Levante S.A. de hasta 100 euros» | `propose_create_rule`; tarjeta «Propuesta: crear una regla (quedará inactiva)» con acción, proveedor, importe máximo y «Habría cubierto 1 caso anterior»; ninguna regla creada; 197 s |
| Doble clic en «Crear regla» | una sola regla (`chat-…`), inactiva, versión 1; tras el primer clic el botón deja de estar disponible; Historial: `rule` inactiva y `assistant_proposal` confirmada, ambas con origen `chat` |
| «Abrir en Reglas» | abre la pantalla Reglas |
| «Cambia la regla de Aguas Imaginarias para que el importe máximo sea 80 euros» (tras la corrección) | `propose_edit_rule`; tarjeta con «Condiciones antes» y «Condiciones después» y «La regla está inactiva y seguirá así»; 144 s. El modelo acortó el nombre del proveedor y la tarjeta lo muestra, con «No habría cubierto ningún caso anterior» |
| «No hacerlo» en esa tarjeta | «Descartada · No se hizo: usted la descartó»; la regla sigue igual (versión 1, mismas condiciones); un solo evento, `assistant_proposal` rechazada |
| «Quiero importar una factura que tengo en el equipo» | `propose_import`; tarjeta «Propuesta: importar documentos» con «Elegir archivos…» y «No hacerlo»; ninguna tarea nueva; 113 s |

### Pendiente

- **El diálogo del sistema para importar no se abrió en la aplicación**: no se puede manejar por el puerto de depuración. Faltan por ver: cancelar (la tarjeta sigue pendiente), importar un archivo y el recibo con sus recuentos, y que el diálogo sea modal.
- **Aceptar y «no volver a proponer» una sugerencia no se vieron en la aplicación**: la carpeta de pruebas no tiene sugerencias. Solo pruebas automáticas.
- No se probó una regla de envío de correo ni la edición de una regla activa en la aplicación.
- No se probó con un proveedor externo.
- Las correcciones de la revisión y las tres del recorrido no han pasado una segunda revisión de `arquitecto`.
- Calidad del modelo local: al editar acortó el nombre del proveedor, y el texto de su respuesta contradecía a la tarjeta («no se puede editar») aunque la propuesta estaba preparada.
- Con la IA local en CPU, la primera pregunta tras abrir la aplicación agota los 180 s (carga del modelo); las siguientes tardan entre 46 y 197 s. Pedir 8192 de contexto a la IA local usa más memoria; no se ha medido.
- No hay prueba de que `suggest` oculte las categorías de solo revisión vista desde el asistente; la cubren las pruebas de `suggest`.
