# Evidencia T06 (parcial): simuladores de correo y administración

Cubre la parte de R06 que no depende de la interfaz: el crate `crates/simulators` define los contratos de correo (`list_messages`, `create_draft`, `send_draft`) y administración (`find_invoices`, `register_invoice`) de contratos.md como los traits reemplazables `MailConnector` y `AdminConnector`, que extienden el trait `Connector` del núcleo para las acciones `send_draft` y `register_invoice`, y los implementa con `MailSimulator` y `AdminSimulator`. Un conector real sustituirá a los simuladores implementando esos mismos traits. No contiene ningún cliente de red ni acceso a cuentas: el estado vive en memoria y, opcionalmente, en un archivo JSON local que sobrevive a un reinicio. Todos los datos son sintéticos (dominio reservado `.test`).

## Cómo reproducir

```sh
cargo test -p nestadmin-simulators
```

## Ejecución registrada

- Fecha: 2026-11-03
- Plataforma: Windows 11 Pro 10.0.26300, Intel Core Ultra 5 125U, 16 GB
- Rust 1.99.1 (MSVC)
- Resultado: 21 pruebas superadas, 0 fallidas (1 unitaria en `src/mail.rs`, 15 en `tests/simulators.rs`, 5 en `tests/core_integration.rs`). CI repite `cargo test --locked` en Linux y Windows.

| Prueba | Qué comprueba |
| --- | --- |
| `format_utc_handles_epoch_and_leap_day` | Conversión de la fecha de creación de los borradores a UTC, incluido un 29 de febrero. |
| `list_messages_pages_with_stable_ids_and_explicit_absences` | Paginación con cursor e IDs estables; asunto, fecha, cuerpo y destinatarios ausentes como `None`; cursor desconocido es `invalid_input`. |
| `create_draft_is_idempotent_and_conflicts_on_different_content` | Misma clave y contenido devuelven el mismo borrador, con fecha de creación; otro contenido o mensaje con la misma clave es `conflict`. |
| `create_draft_rejects_missing_message_and_invalid_input` | Mensaje inexistente es `not_found`; cuerpo, clave o destinatarios no válidos son `invalid_input`; ningún error crea borradores. |
| `send_draft_sends_once_and_returns_original_receipt` | Un solo envío por borrador; repetir la clave devuelve el recibo original; otra clave o borrador es `conflict`; borrador inexistente es `not_found`. |
| `failure_mode_blocks_every_operation_without_effects` | En correo y en administración, los modos `unavailable` y `unauthorized` bloquean consultas, acciones, `execute` y `lookup` sin efectos. |
| `contracts_are_usable_through_trait_objects` | Consulta, borrador, envío, registro y búsqueda a través de `&dyn MailConnector` y `&dyn AdminConnector`, y el mismo objeto usado como `&dyn Connector`. |
| `mail_connector_executes_send_draft_and_looks_up_by_key` | `Connector::execute` y `lookup` del correo por clave de idempotencia; carga útil sin `draft_id` es `invalid_input`. |
| `find_invoices_compares_decimals_and_explains_matches` | Comparación decimal ("121.0" = "121.00"), proveedor sin distinguir mayúsculas, motivos de coincidencia y total ausente en el registro indicado como diferencia. |
| `find_invoices_reports_partial_duplicates_with_differences` | Mismo proveedor y número con otra fecha y otro total: candidato con motivos `mismo proveedor` y `mismo número` y diferencias `fecha distinta` y `total distinto`; los candidatos con más coincidencias van primero. |
| `register_invoice_is_idempotent_and_validates_input` | Registro idempotente, `conflict` con otra factura, campos ausentes conservados e importes, moneda y fecha validados (incluidas fechas inexistentes como 2026-02-30 y años bisiestos). |
| `admin_connector_registers_payload_and_rejects_unknown_fields` | `Connector` de administración; campos desconocidos en la carga útil son `invalid_input`. |
| `state_survives_restart_from_json_file` | Borradores, envíos y registros persisten al reabrir el archivo; tras reiniciar no se reenvía. |
| `second_open_on_same_file_is_refused_and_no_effect_is_lost` | Un segundo simulador sobre el mismo archivo se rechaza con `unavailable`; tras cerrar el primero, sus efectos siguen visibles en `sent`, `record_count` y `lookup`. |
| `failed_save_returns_unavailable_without_effect` | Si el archivo de estado no se puede escribir (sustituido por un directorio), `send_draft`, `execute` y `register_invoice` devuelven `unavailable` sin efecto en memoria ni en `lookup`, y el efecto sigue ausente al restaurar el archivo y reabrir. |
| `unreadable_or_newer_state_file_is_reported_as_unavailable` | Un archivo de estado corrupto o de formato más reciente se rechaza como `unavailable`. |
| `send_interrupted_after_effect_is_reconciled_without_resending` | Con `Store::perform_action`: corte tras enviar y antes de confirmar; al reiniciar se reconcilia con el recibo del simulador y no se reenvía. |
| `send_interrupted_before_effect_executes_exactly_once_after_restart` | Corte antes del envío: al reiniciar se envía exactamente una vez. |
| `register_interrupted_after_effect_is_confirmed_by_reconcile_actions` | Corte tras registrar: `reconcile_actions` confirma la factura sin duplicarla y el historial muestra `pending` y `reconciled`. |
| `reconcile_with_failing_lookup_keeps_action_pending_until_connector_answers` | Corte tras registrar y simulador en `unavailable` o `unauthorized`: `reconcile_actions` devuelve `false` y la acción sigue `pending`; al recuperarse el simulador se confirma sin duplicar. |
| `unavailable_connector_is_recorded_as_failure_and_retry_registers_once` | Conector caído: la acción queda `failed` sin registro; el reintento registra una sola vez. |

## Decisiones

- Los contratos son traits (`MailConnector`, `AdminConnector`) que extienden `Connector`; sus llamadores no dependen de los simuladores.
- El cursor de `list_messages` es el ID del último mensaje entregado, de modo que añadir mensajes no desplaza las páginas ya leídas.
- Enviar con otra clave un borrador ya enviado devuelve `conflict` en lugar de reenviarlo.
- Cada cambio se aplica sobre una copia del estado y solo se adopta tras escribirla (archivo temporal llevado a disco con `sync_all` y renombrado); un fallo de escritura se devuelve como `unavailable` sin efecto.
- Un archivo de estado solo puede estar abierto por un simulador a la vez: se toma un bloqueo exclusivo del sistema operativo sobre `<archivo>.lock`, que se libera al cerrar el simulador o al terminar el proceso.
- `find_invoices` devuelve como candidatos los registros que coinciden en al menos un criterio, con los motivos de coincidencia y las diferencias (valor distinto o ausente); no decide duplicados. Sin criterios devuelve todos. Los importes se comparan como decimales en texto, sin coma flotante.
- Los destinatarios desconocidos de un mensaje son `None`. Un borrador exige destinatarios y cuerpo porque sin ellos no se puede enviar, y guarda su fecha de creación en UTC (`None` si el reloj no la da).
- El modo de fallo es configuración en memoria para pruebas y no se guarda en el archivo.

## Pendiente para completar T06

- Usar los simuladores desde la interfaz de escritorio (consultar correo, revisar borradores y registrar facturas) y verificarlo en Windows.
- Conectores reales de correo y administración, cuando se identifiquen los productos; deben implementar `MailConnector` y `AdminConnector`.
- Conservar la evidencia de origen de cada campo de la factura (contratos.md) junto con la extracción (R05).
- Verificación del aislamiento de cuentas reales a nivel de aplicación (sin dependencias de red en el crate, pero sin prueba de tráfico de red).
