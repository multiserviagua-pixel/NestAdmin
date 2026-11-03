# Evidencia T07 (parcial): revisión predeterminada, reglas e historial

Cubre la lógica de R07 y de los flujos 1–3 en `crates/app`, que une la cola durable, la IA, la validación de facturas, los simuladores y las reglas. La ventana (T01) solo llama a esta capa.

## Comportamiento implementado

- Toda tarea con acción (`register_invoice`, `send_draft`) termina en `awaiting_review` salvo que **exactamente una** regla activa coincida y no haya bloqueos: incidencias o campos ilegibles en una factura, o un borrador sin destinatario.
- Condiciones admitidas: para registrar, proveedor (sin distinguir mayúsculas) y total máximo opcional, comparado como decimal; para enviar, la categoría. Una regla con condiciones no válidas se rechaza al crearla.
- Las reglas se crean inactivas; cada cambio crea una versión nueva y una fila en el historial. La automatización guarda la regla y su versión como autorización.
- La aprobación indica la versión vista. Editar un borrador crea una versión nueva; aprobar la anterior se rechaza (`StaleVersion`) y no se envía nada.
- La acción se ejecuta mediante `Store::perform_action` con clave `task-{id}/{acción}/v{versión}`. El borrador previo al envío usa su propia clave por versión, así que repetir tras un corte no crea otro.
- La llamada al modelo se hace sin el cerrojo de la aplicación (`begin_next` → `Job::run` → `finish`).

## Cómo reproducir

```sh
cargo test -p nestadmin-app
cargo test -p nestadmin-app --test ollama -- --ignored --nocapture   # requiere Ollama con qwen3:8b
```

## Ejecución registrada

- Fecha: 2026-11-03
- Plataforma: Windows 11 Pro 10.0.26300, Intel Core Ultra 5 125U, 16 GB
- Rust 1.99.1 (MSVC); Ollama 0.40.14 con `qwen3:8b`
- Resultado: 10 pruebas superadas con proveedor simulado; la prueba con Ollama local, superada en 76 s.

| Prueba | Qué comprueba |
| --- | --- |
| `invoice_waits_for_review_and_registers_once_after_approval` | Sin regla, la factura espera revisión y no registra; aprobada, registra una vez; aprobarla otra vez es error y no repite el registro. |
| `issues_and_duplicates_are_reported_by_code` | `total_mismatch` y `possible_duplicate` calculados por código, con el candidato a duplicado. |
| `an_active_matching_rule_automates_only_clean_invoices` | Regla inactiva → revisión; activa → registro automático con autorización `rule` y versión; incidencias, otro proveedor o dos reglas coincidentes → revisión. |
| `invalid_rules_are_rejected` | Sin proveedor, importe sin dos decimales, categoría desconocida o acción desconocida. |
| `mail_is_fetched_once_and_edited_drafts_need_new_approval` | Consultar dos veces no duplica tareas; editar crea la versión 2; aprobar la 1 se rechaza sin enviar; aprobar la 2 envía una vez con la clave `v2` y el texto editado. |
| `a_draft_without_sender_is_never_sent_by_a_rule` | Con una regla de envío activa, el mensaje sin remitente queda en revisión. |
| `model_failure_is_visible_and_retry_requeues` | Fallo del proveedor → tarea fallida con código `unavailable` y sin resultado; reintento explícito. |
| `empty_documents_and_unknown_tasks_are_rejected` | Documento vacío y aprobación de una tarea inexistente. |
| `restart_recovers_work_without_duplicate_effects` | Tarea en curso al cerrar → vuelve a la cola al reabrir y se procesa; ningún registro repetido; el registro de duplicados se reconstruye tras reiniciar. |
| `audit_log_records_actions_and_rules` | El historial contiene la regla y el registro confirmado. |
| `document_and_mail_flow_with_local_ollama` (ignorada por defecto) | Con Ollama real: factura sintética extraída (total 121.00) y en revisión; aprobada, registrada; correo clasificado con borrador en revisión y sin envíos. |

## Pendiente

- Recorrer el mismo flujo desde la ventana (importar, revisar, editar, aprobar, reglas e historial) y registrarlo.
- La moneda no forma parte de la extracción: se registra como ausente.
- Importar PDF y documentos escaneados depende de T04.
