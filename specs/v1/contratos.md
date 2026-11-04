# Contratos propuestos v1

Estos contratos son independientes del transporte y no afirman que existan implementaciones. Los identificadores son estables; las implementaciones validan las entradas y devuelven errores estructurados sin secretos. Los ejemplos expresan semántica, no un esquema de serialización definitivo.

## Proveedor de IA

`generate(request) -> result | error`

- Solicitud: `request_id`, `task` (extracción, clasificación o borrador), contenido mínimo necesario, formato esperado y configuración explícita del proveedor.
- Resultado: `request_id`, proveedor y modelo realmente utilizados, salida estructurada o texto, y metadatos disponibles de tiempo/uso. Los campos no encontrados se devuelven como `null`, sin inventar datos.
- Errores: proveedor no disponible (`unavailable`), credenciales inválidas (`unauthorized`), límite de tiempo (`timeout`, que puede añadir una causa probable al mensaje, como la memoria insuficiente de D41), respuesta que no cumple el formato esperado (`invalid_response`). Ninguno autoriza cambiar a otro proveedor. El adaptador local solo envía contenido a la instancia local configurada; el externo solo se invoca tras su selección explícita.
- Selección inicial: Ollama / Qwen3 8B. El identificador y versión concretos del modelo deberán registrarse en cada evaluación. Groq es el primer adaptador externo.

## Preparación de componentes locales

`check() -> status`, `prepare(events) -> report | error` (D36–D39, D41).

- `check` es rápida y no descarga nada: devuelve `ready` (servicio, modelo y OCR disponibles), `stopped` (Ollama instalado pero sin responder) o `missing` con el componente que falta (`ollama`, `ollama_outdated`, `model` u `ocr`). Junto al estado se informa la memoria del equipo (`memory`): total y libre en bytes cuando se pueden leer, nivel (`recommended`, `below_recommended`, `insufficient` o `unknown`) y un aviso legible opcional. La memoria nunca cambia el estado ni bloquea la preparación.
- `prepare` recorre los pasos `ocr`, `ollama`, `service`, `model` y `verify` en ese orden; reutiliza lo que ya está y solo actúa sobre lo que falta. Emite eventos con el paso, su estado (`running`, `done`, `skipped`, `failed`), bytes descargados y totales cuando se conocen y un mensaje legible. Repetirla tras un fallo o una interrupción no duplica instalaciones ni descargas completas.
- El informe indica la versión de Ollama, si se instaló o se reutilizó, el modelo y su digest cuando Ollama lo informa, la respuesta de verificación y la memoria leída.
- Errores: `network` (descarga o conexión fallida, reintentable), `signature` (instalador sin firma válida del editor esperado; no se ejecuta), `install_failed` (el instalador de Ollama terminó con error), `unavailable` (el servicio no responde tras arrancarlo), `model_failed` (Ollama rechazó la descarga del modelo, por ejemplo por espacio), `verify_failed` (el modelo no dio una respuesta válida o no respondió a tiempo; con poca memoria, el mensaje lo señala como causa probable), `ocr_missing` (faltan o no responden los componentes de OCR del paquete; reinstalar lo corrige) y `cancelled`. Ningún error activa un proveedor externo.

## Correo

`list_messages(cursor) -> page`, `create_draft(message_id, draft, idempotency_key) -> draft_id`, `send_draft(draft_id, idempotency_key) -> receipt`. Los mensajes y borradores tienen ID estable, asunto, remitente/destinatarios, cuerpo y fecha cuando estén disponibles. El envío es una acción sujeta a revisión o regla. El simulador conserva y expone sus acciones sin conexión a cuentas reales.

- Errores: `not_found` (mensaje o borrador inexistente), `conflict` (clave de idempotencia reutilizada con contenido distinto), `unavailable` (conector no accesible), `unauthorized` (credenciales inválidas o ausentes) y `invalid_input`. Un error nunca se registra como envío realizado.
- `send_draft` con una clave ya usada para el mismo borrador devuelve el recibo original sin volver a enviar.
- Conectores reales (R13, D30–D35): los IDs de mensaje y borrador son los del proveedor y el cursor es opaco. Cada cuenta es un conector independiente; quien agrupe varias cuentas debe anteponer el identificador de cuenta a los IDs y a las claves para que no choquen. El recibo de envío lleva como `receipt_id` el ID del mensaje enviado en el proveedor. La reconciliación consulta el estado del borrador en el proveedor (D33).

## Administración

`find_invoices(criteria) -> matches`, `register_invoice(invoice, idempotency_key) -> receipt`. La factura incluye proveedor, número, fecha, moneda, base, impuestos y total; cada campo puede faltar y debe conservar su evidencia. La comparación de importes es decimal y la detección de duplicados devuelve candidatos y motivos, no una decisión destructiva. El registro está sujeto a revisión o regla. La clave de idempotencia identifica una única intención y reutilizarla devuelve el resultado previo sin volver a ejecutarla. Los errores siguen la misma clasificación que el correo (`not_found`, `conflict`, `unavailable`, `unauthorized`, `invalid_input`).

## Cola, revisión y auditoría

Una tarea tiene ID, tipo, entrada referenciada, estado (`queued`, `running`, `awaiting_review`, `completed`, `failed`, `cancelled`), fecha de creación/actualización, intentos, resultado y error estructurado opcionales. Cada transición se persiste. `cancelled` es una decisión explícita del usuario sobre una tarea no completada y es final, igual que `completed`; `failed` admite reintento explícito, que vuelve a `queued`. Tras una interrupción, las tareas en `running` vuelven a `queued` con un intento más; las operaciones con efectos se reconcilian mediante su clave de idempotencia antes de reintentarlas.

La clave de idempotencia se deriva de forma determinista de la tarea y de la versión de la acción aprobada. Se persiste en la base local antes de invocar al conector, junto con el estado (`pending`, `confirmed`, `failed`) y el recibo; el conector la recibe y la conserva también. Al recuperar una acción `pending`, se consulta al conector con la misma clave antes de repetirla. Una aprobación se refiere a una versión concreta de la acción; si su contenido cambia, requiere nueva revisión.

Una regla tiene ID, versión, tipo de acción, condiciones explícitas y estado activo. Sin coincidencia inequívoca y validación satisfactoria, la acción queda pendiente. El historial indica acción, estado, hora y motivo de autorización (aprobación o regla y versión) sin secretos. Los fallos son visibles y no se registran como éxitos.
