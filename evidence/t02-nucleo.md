# Evidencia T02 (parcial): recuperación en el núcleo

Cubre la parte de R02 que no depende de la interfaz: persistencia de tareas, recuperación tras interrupción y ausencia de efectos externos duplicados en `crates/core`. Las interrupciones se simulan cerrando la base SQLite o con un pánico en el punto crítico, y reabriendo el mismo archivo.

## Cómo reproducir

```sh
cargo test -p nestadmin-core --test recovery
```

## Ejecución registrada

- Fecha: 2026-11-03
- Plataforma: Windows 11 Pro 10.0.26300, Intel Core Ultra 5 125U, 16 GB
- Rust 1.99.1 (MSVC), SQLite incluido en `rusqlite` 0.41
- Resultado: 10 pruebas superadas, 0 fallidas. CI repite la ejecución en Linux y Windows.

| Prueba | Qué comprueba |
| --- | --- |
| `tasks_survive_restart_in_every_state_and_running_is_requeued` | Las tareas persisten en cada estado; `running` vuelve a `queued` con el intento contado y queda en el historial. |
| `invalid_transitions_are_rejected_and_final_states_stay_final` | Solo se permiten las transiciones del contrato; `completed` y `cancelled` son finales. |
| `failed_task_keeps_error_visible_until_explicit_retry_succeeds` | Un fallo no se reintenta solo; el error sigue visible hasta que la tarea se completa. |
| `confirmed_action_is_not_executed_again` | Repetir una acción confirmada devuelve el recibo sin llamar al conector. |
| `interruption_after_external_effect_is_reconciled_without_duplicate` | Corte tras el efecto externo y antes de confirmar: al reanudar se reconcilia y no se repite. |
| `interruption_before_external_effect_executes_exactly_once_on_retry` | Conector caído: queda `failed`; el reintento ejecuta una sola vez y el historial registra la regla. |
| `reconcile_actions_confirms_known_effects_and_keeps_unknown_pending` | La reconciliación confirma lo que ocurrió y deja pendiente lo que no consta. |
| `changed_content_needs_new_version_and_never_reuses_key` | Cambiar el contenido aprobado exige una versión nueva; no se reutiliza la clave. |
| `audit_log_does_not_store_action_content` | El historial no guarda el contenido de la acción. |
| `newer_schema_is_refused` | No se abre una base con un esquema más reciente que el soportado. |
| `recorded_attempts_survive_restart_and_superseded_actions_are_never_reconciled` | El resultado guardado antes de ejecutar (`record_attempt`, D50) sobrevive al corte y a la recuperación; una acción `superseded` no se reconcilia ni se ejecuta, y sustituirla dos veces deja una sola entrada en el historial. |
| `reconcile_action_confirms_a_failed_attempt_that_did_happen` | Una acción `failed` cuyo efecto sí ocurrió se confirma con el recibo del conector sin volver a ejecutarla. |

Ejecución con D50, 2026-11-05, mismo equipo: 12 pruebas superadas, 0 fallidas.

## Reproceso en la capa de aplicación (D50)

`cargo test -p nestadmin-app --locked --test reprocess` cubre el reproceso de una tarea cuya acción ya se intentó, con el simulador y con Gmail y Graph simulados, incluidos cortes del proceso (pánico) y reinicios de la aplicación entre medias: no se pide otro texto a la IA, lo ya realizado se completa sin repetirlo y otra versión solo se ejecuta tras comprobar que la anterior no salió. Detalle de escenarios en [la evidencia T13](t13-conectores-correo.md#reproceso-de-un-envío-ya-intentado-d50).

## Pendiente para completar T02

- Reinicio real de la aplicación Tauri y recuperación al arrancar (fase 6).
- Mostrar estado, incidencias y pendientes en la interfaz.

La reconciliación con los simuladores de correo y administración en lugar del conector de prueba se verifica en [la evidencia T06](t06-simuladores.md) (`crates/simulators/tests/core_integration.rs`); las pruebas de este documento siguen usando el conector de prueba.
