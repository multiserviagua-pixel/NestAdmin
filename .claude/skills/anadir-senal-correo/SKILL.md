---
name: anadir-senal-correo
description: Añade o cambia una señal de correo automático en NestAdmin (dirección o cabecera). Úsela cuando haya que detectar un nuevo tipo de remitente o cabecera en el triaje local, para que función, conectores, reglas, corpus y especificación queden alineados.
---

# Añadir una señal de correo

Reglas que no cambian: pruebas manuales sobre `tauri dev`; instalador solo para la versión final o si el usuario lo pide; git solo local, sin push ni PR; fixtures siempre sintéticos.

## Antes de escribir código

1. Lea D157 y D161 en `specs/v1/decisiones.md`.
2. Decida si es fuerte, débil o rebote con este criterio: **una señal fuerte oculta el correo**. Si un cliente real puede llevarla, es débil. En caso de duda, débil.
3. Si es posible, mida con cuántos correos reales coincide (solo recuentos, con la skill `medir-calidad-correo`).

## Pasos, en este orden

1. **Función pura** en `crates/core/src/mail_signals.rs`: un código estable, nunca el valor de la cabecera.
2. **Tabla de casos:** variantes de mayúsculas, espacios, comentarios y parámetros, casos que no deben coincidir y valores ausentes.
3. **Conectores:** Gmail y Graph piden la cabecera si hace falta; `crates/simulators` imita al proveedor real. Graph sin datos da `None`, no una lista vacía.
4. **Prueba con centinela:** la cabecera en bruto no llega al almacén.
5. **Bloqueos de reglas:** `blockers` en `crates/app/src/rules.rs` debe bloquear con la señal nueva, con un motivo legible (D125).
6. **Panel:** las tareas triadas siguen excluidas de simulación y sugerencias.
7. **Corpus:** casos en `evaluation/data/{inputs,expected}/triage.json`, incluido uno de un cliente real que no deba ocultarse.
8. **Textos:** motivo en `ui/src/pendientes-text.ts` y mock en `ui/src/mock.ts`.
9. **Especificación:** decisión o revisión, contratos y matriz.

## Comprobaciones

`cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `npm test` en `ui/`, `python scripts/check.py` y los evaluadores. La métrica que no puede empeorar es `hidden_fn`.
