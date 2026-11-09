# T28–T30 (E3) — Triaje en la capa de aplicación

Pruebas automáticas con proveedor y correo simulados y datos sintéticos, en Windows. No hay recorrido en `tauri dev` ni medida con un modelo real; las pantallas de Informativos, Archivar y Redactar son de E4.

## Qué se comprueba

- `crates/core/tests/born_tasks.rs`: `enqueue_closed` y `enqueue_review` crean la tarea y su transición desde `NULL` en una transacción (un corte simulado con un disparador SQL no deja nada), sin pasar por la cola y sin reabrir tareas finales; `enqueue_redraft` es idempotente, archiva el origen en la misma transacción y una tarea cancelada libera el hueco.
- `crates/core/tests/schema.rs`: migración 005 desde el esquema 4 con copia previa, entradas que no son JSON y el índice único parcial de `redraft_of`.
- `crates/core/src/mail_signals.rs` y `crates/core/tests/triage_corpus.rs`: la ruta (`route`) es la de producción y coincide con las 148 etiquetas del corpus, con `hidden_fn` 0.
- `crates/ai/tests/triage_tasks.rs`: documento delimitado como dato, filtraciones de instrucciones, marcadores y categorías, y avisos de enlace, correo, teléfono e IBAN del borrador.
- `crates/app/tests/triage.rs`: informativo cerrado sin IA y sin proveedor configurado, con adjuntos importados y sin duplicar; corte en el cierre; rebote en revisión sin IA ni borrador; dos llamadas con lo que sale en cada una; `needs_reply` falso, ausente y borrador vacío; respaldo local en la clasificación implica redacción local; una prueba por condición de la compuerta, tarea antigua sin `signals` y destinatario no-reply; aprobar sin borrador; Archivar sin señal `rejected`; Redactar dos veces una sola tarea, que acaba en revisión aunque coincida una regla; exclusiones del panel; suite de inyección sobre `evaluation/data/inputs/injection.json` con un proveedor comprometido.
- `evaluation/tests/test_evaluate.py`: `needs_reply` falso sin borrador solo cuenta si el esperado lo permite; los 20 correos de R10 siguen exigiendo borrador.

## Límites

- Graph sin `internetMessageHeaders` deja `signals` sin comprobar: ninguna regla envía a esos mensajes hasta que el conector pida las cabeceras.
- La suite de inyección con proveedor simulado comprueba la defensa del código, no el comportamiento de un modelo real (eso es `evidence/t30-triaje-base.md`).
