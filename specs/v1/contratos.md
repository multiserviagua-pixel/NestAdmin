# Contratos propuestos v1

Estos contratos son independientes del transporte y no afirman que existan implementaciones. Los identificadores son estables; las implementaciones validan las entradas y devuelven errores estructurados sin secretos. Los ejemplos expresan semántica, no un esquema de serialización definitivo.

## Proveedor de IA

`generate(request) -> result | error`

- Solicitud: `request_id`, `task` (extracción, clasificación o borrador), contenido mínimo necesario, formato esperado y configuración explícita del proveedor.
- Resultado: `request_id`, proveedor y modelo realmente utilizados, salida estructurada o texto, y metadatos disponibles de tiempo/uso. Los campos no encontrados se devuelven como `null`, sin inventar datos.
- Errores: proveedor no disponible, credenciales inválidas, límite de tiempo, respuesta inválida. Ninguno autoriza cambiar a otro proveedor. El adaptador local solo envía contenido a la instancia local configurada; el externo solo se invoca tras su selección explícita.
- Selección inicial: Ollama / Qwen3 8B. El identificador y versión concretos del modelo deberán registrarse en cada evaluación. Groq es el primer adaptador externo.

## Correo

`list_messages(cursor) -> page`, `create_draft(message_id, draft, idempotency_key) -> draft_id`, `send_draft(draft_id, idempotency_key) -> receipt`. Los mensajes y borradores tienen ID estable, asunto, remitente/destinatarios, cuerpo y fecha cuando estén disponibles. El envío es una acción sujeta a revisión o regla. El simulador conserva y expone sus acciones sin conexión a cuentas reales.

## Administración

`find_invoices(criteria) -> matches`, `register_invoice(invoice, idempotency_key) -> receipt`. La factura incluye proveedor, número, fecha, moneda, base, impuestos y total; cada campo puede faltar y debe conservar su evidencia. La comparación de importes es decimal y la detección de duplicados devuelve candidatos y motivos, no una decisión destructiva. El registro está sujeto a revisión o regla. La clave de idempotencia identifica una única intención y reutilizarla devuelve el resultado previo sin volver a ejecutarla.

## Cola, revisión y auditoría

Una tarea tiene ID, tipo, entrada referenciada, estado (`queued`, `running`, `awaiting_review`, `completed`, `failed`), fecha de creación/actualización, intentos, resultado y error estructurado opcionales. Cada transición se persiste. Tras una interrupción se recuperan tareas en curso; las operaciones con efectos se reconcilian mediante su clave de idempotencia antes de reintentarlas. Una aprobación se refiere a una versión concreta de la acción; si su contenido cambia, requiere nueva revisión.

Una regla tiene ID, versión, tipo de acción, condiciones explícitas y estado activo. Sin coincidencia inequívoca y validación satisfactoria, la acción queda pendiente. El historial indica acción, estado, hora y motivo de autorización (aprobación o regla y versión) sin secretos. Los fallos son visibles y no se registran como éxitos.
