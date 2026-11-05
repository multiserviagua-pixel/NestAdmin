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

## Reinicio de la aplicación de escritorio (2026-11-05, D53)

`cargo test -p nestadmin-desktop --locked recorrido::` termina procesos reales de la aplicación (comandos y cola de `crates/desktop` con el runtime simulado de Tauri) con la tarea en curso, en revisión y justo tras aprobar, y comprueba al reabrir la recuperación sin efectos duplicados; también la salida completa con tareas en cola, y que los comandos `tasks` y `audit_log` devuelven estado, incidencias, errores y pendientes. Detalle y resultados en [T07](t07-revision-reglas.md#recorrido-desde-la-ventana-d53).

## Pendiente para completar T02

- Ver estado, incidencias y pendientes pintados en la ventana WebView2 solo se ha comprobado a mano.
- El corte entre el efecto externo y su confirmación no se provoca con el proceso completo (no hay un punto de corte determinista); lo cubren las pruebas del núcleo de esta página y de [T06](t06-simuladores.md).

La reconciliación con los simuladores de correo y administración en lugar del conector de prueba se verifica en [la evidencia T06](t06-simuladores.md) (`crates/simulators/tests/core_integration.rs`); las pruebas de este documento siguen usando el conector de prueba.
