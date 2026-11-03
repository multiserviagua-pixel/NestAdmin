# Evidencia T06 (parcial): simuladores de correo y administración

Cubre la parte de R06 que no depende de la interfaz: el crate `crates/simulators` implementa los contratos de correo (`list_messages`, `create_draft`, `send_draft`) y administración (`find_invoices`, `register_invoice`) de contratos.md, y el trait `Connector` del núcleo para las acciones `send_draft` y `register_invoice`. No contiene ningún cliente de red ni acceso a cuentas: el estado vive en memoria y, opcionalmente, en un archivo JSON local que sobrevive a un reinicio. Todos los datos son sintéticos (dominio reservado `.test`).

## Cómo reproducir

```sh
cargo test -p nestadmin-simulators
```

## Ejecución registrada

- Fecha: 2026-11-03
- Plataforma: Windows 11 Pro 10.0.26300, Intel Core Ultra 5 125U, 16 GB
- Rust 1.99.1 (MSVC)
- Resultado: 15 pruebas superadas, 0 fallidas (11 en `tests/simulators.rs`, 4 en `tests/core_integration.rs`). CI repite `cargo test --locked` en Linux y Windows.

| Prueba | Qué comprueba |
| --- | --- |
| `list_messages_pages_with_stable_ids_and_explicit_absences` | Paginación con cursor e IDs estables; campos ausentes como `None`; cursor desconocido es `invalid_input`. |
| `create_draft_is_idempotent_and_conflicts_on_different_content` | Misma clave y contenido devuelven el mismo borrador; otro contenido o mensaje con la misma clave es `conflict`. |
| `create_draft_rejects_missing_message_and_invalid_input` | Mensaje inexistente es `not_found`; cuerpo, clave o destinatarios no válidos son `invalid_input`; ningún error crea borradores. |
| `send_draft_sends_once_and_returns_original_receipt` | Un solo envío por borrador; repetir la clave devuelve el recibo original; otra clave o borrador es `conflict`; borrador inexistente es `not_found`. |
| `failure_mode_blocks_every_operation_without_effects` | Los modos `unavailable` y `unauthorized` bloquean consultas, acciones y `lookup` sin efectos. |
| `mail_connector_executes_send_draft_and_looks_up_by_key` | `Connector::execute` y `lookup` del correo por clave de idempotencia; carga útil sin `draft_id` es `invalid_input`. |
| `find_invoices_compares_decimals_and_explains_matches` | Comparación decimal ("121.0" = "121.00"), proveedor sin distinguir mayúsculas, motivos de coincidencia y ausencias que no coinciden. |
| `register_invoice_is_idempotent_and_validates_input` | Registro idempotente, `conflict` con otra factura, campos ausentes conservados e importes, moneda y fecha validados. |
| `admin_connector_registers_payload_and_rejects_unknown_fields` | `Connector` de administración; campos desconocidos en la carga útil son `invalid_input`. |
| `state_survives_restart_from_json_file` | Borradores, envíos y registros persisten al reabrir el archivo; tras reiniciar no se reenvía. |
| `unreadable_or_newer_state_file_is_reported_as_unavailable` | Un archivo de estado corrupto o de formato más reciente se rechaza como `unavailable`. |
| `send_interrupted_after_effect_is_reconciled_without_resending` | Con `Store::perform_action`: corte tras enviar y antes de confirmar; al reiniciar se reconcilia con el recibo del simulador y no se reenvía. |
| `send_interrupted_before_effect_executes_exactly_once_after_restart` | Corte antes del envío: al reiniciar se envía exactamente una vez. |
| `register_interrupted_after_effect_is_confirmed_by_reconcile_actions` | Corte tras registrar: `reconcile_actions` confirma la factura sin duplicarla y el historial muestra `pending` y `reconciled`. |
| `unavailable_connector_is_recorded_as_failure_and_retry_registers_once` | Conector caído: la acción queda `failed` sin registro; el reintento registra una sola vez. |

## Decisiones

- El cursor de `list_messages` es el ID del último mensaje entregado, de modo que añadir mensajes no desplaza las páginas ya leídas.
- Enviar con otra clave un borrador ya enviado devuelve `conflict` en lugar de reenviarlo.
- Cada cambio se aplica sobre una copia del estado y solo se adopta tras escribirla (archivo temporal y renombrado); un fallo de escritura se devuelve como `unavailable` sin efecto.
- `find_invoices` devuelve candidatos y motivos; no decide duplicados. Los importes se comparan como decimales en texto, sin coma flotante.
- El modo de fallo es configuración en memoria para pruebas y no se guarda en el archivo.

## Pendiente para completar T06

- Usar los simuladores desde la interfaz de escritorio (consultar correo, revisar borradores y registrar facturas) y verificarlo en Windows.
- Conectores reales de correo y administración, cuando se identifiquen los productos; deben implementar los mismos contratos.
- Conservar la evidencia de origen de cada campo de la factura (contratos.md) junto con la extracción (R05).
- Verificación del aislamiento de cuentas reales a nivel de aplicación (sin dependencias de red en el crate, pero sin prueba de tráfico de red).
