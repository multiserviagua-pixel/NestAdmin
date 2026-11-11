# Numeración de decisiones

Cada decisión (`D<n>`) tiene un número definitivo desde que se escribe, para que integrar una rama no obligue a renumerar. Hasta el 2026-11-07 se renumeró tres veces por colisiones entre ramas (fase 9e, fase 11a y la de venta con los proveedores externos).

## Reglas

1. Antes de escribir su primera decisión, una rama reserva un bloque con un commit en `main` que añade una fila `Reservado` a la tabla. El bloque empieza en el siguiente número libre y deja margen (por ejemplo, diez números). Los huecos están permitidos.
2. La rama solo usa números de su bloque. Si se le queda corto, reserva otro bloque en `main`.
3. Al integrar la rama, su fila pasa a `Asignado`. Los números que no llegó a usar quedan como hueco.
4. Una referencia a un número de un bloque `Reservado` todavía sin integrar es válida; cualquier otra referencia a una decisión inexistente es un error. Los números antiguos de una renumeración se citan entre acentos graves (`` `D92` ``), que el verificador no lee como referencias.
5. Si hay que renumerar una rama antigua, se hace con `scripts/renumerar.py` en la propia rama antes de fusionarle `main`, para que los números viejos solo signifiquen lo suyo.

`scripts/check.py` comprueba que no hay decisiones repetidas, que cada una cae en un bloque de esta tabla, que los bloques no se solapan y que no se citan decisiones inexistentes en la especificación, `PLAN.md`, `AGENTS.md` y `README.md`.

## Bloques

| Bloque | Rama | Estado |
| --- | --- | --- |
| D01–D105 | `main` (historia hasta el 2026-11-07, con huecos D26–D29, D54–D59 y D94–D99) | Asignado |
| D106–D118 | `claude/fase-11-venta-integracion`: proveedores externos D106–D112 (antes `D92`–`D98`) y persistencia de venta D113–D118 (antes `D99`–`D104`), integrados el 2026-11-07 | Asignado |
| D119–D120 | `fase-9b-importes-es`: importes en formato español D119 e importes dudosos D120 (antes `D48`–`D49`), integrados el 2026-11-07 | Asignado |
| D121–D121 | Credenciales largas repartidas en trozos del Administrador de credenciales (tokens de Microsoft), 2026-11-07 | Asignado |
| D122–D122 | Lista de modelos de cada proveedor y prueba sintética, 2026-11-08 | Asignado |
| D123–D135 | `claude/panel-reglas`: panel de reglas, sugerencias y primera lectura (D123–D134; D135, hueco), integrado el 2026-11-09 | Asignado |
| D136–D145 | `claude/rediseno-ui`: rediseño visual y separación de Pendientes (D136–D138; el resto, hueco), integrado el 2026-11-08 | Asignado |
| D146–D155 | `claude/tareas-pipelines`: errores del proveedor de IA, vistas de Tareas y plantillas de pipeline (D146–D151; D152–D155, hueco), integrado el 2026-11-09 | Asignado |
| D156–D156 | Reconciliación de Gmail por marca propia en el hilo, 2026-11-09 | Asignado |
| D157–D166 | Triaje del correo, inyección, preferencias y perfil (D157–D164; D165–D166, hueco), 2026-11-09 | Asignado |
| D167–D176 | Aprender a responder en la primera carga (D167–D175; D176, reserva), 2026-11-09 | Asignado |
| D177–D177 | Catálogo de categorías ampliado, 2026-11-10 | Asignado |
| D178–D178 | Aprendizaje continuo de las respuestas hechas fuera, 2026-11-10 | Asignado |
| D179–D192 | Chat de la aplicación: asistente y búsqueda (D179–D190; D191–D192, reserva), 2026-11-10 | Asignado |
| D193–D212 | `claude/fase-11-conexion`: conexión de la venta a la aplicación (registros Verifactu, capa de aplicación, ventana, PDF, envío y remisión), 2026-11-10 | Reservado |
| D213–D219 | Correcciones tras la revisión de E1–E3a de la conexión de la venta (el bloque D193–D212 se agotó), 2026-11-10 | Asignado |
| D220–D221 | Correcciones tras el recorrido de la venta en `tauri dev`, 2026-11-10 | Asignado |
| D222–D222 | Membrete PDF de fondo en los documentos de venta (cambio de alcance), 2026-11-10 | Asignado |
| D223–D227 | PDF de los documentos de venta y membrete (etapa E4a), 2026-11-10 | Asignado |
| D228–D228 | Membrete con la zona del texto demasiado oscura, rechazado (concreta D222), 2026-11-10 | Asignado |
| D229–D246 | Envío de documentos de venta por correo con revisión (etapa E4b): contrato de adjuntos de salida y mensaje nuevo D229–D231, recuperación en una cuenta D240; tarea, estados, revisión nueva, anulación, plantilla, cuenta, regla y migración 15 D232–D239; pruebas y límites D241; correcciones de la revisión de seguridad (ids de cuenta que no se reutilizan, mismo buzón, cierre en lugar de descarte, una tarea por documento) D242–D245 y segunda ronda (lista simple y contadores aparte, salida explícita de un envío no comprobable, reparación al abrir, líneas de asunto de 76) D246, 2026-11-11 | Asignado |
| D247–D262 | `claude/fase-11-conexion`: remisión de los registros Verifactu a la AEAT con el simulador local (etapa E5): alcance D247, entorno y QR D248, estado derivado D249, excepción de R07 y bomba D250, control de flujo D251, lote sin pérdida ni duplicado D252, subsanación D253, simulador y transporte D254–D255, cambio de entorno D256, composición del lote D257, entorno heredado D258, `Fault` D259 y pruebas D260 (D261–D262, reserva), 2026-11-11 | Reservado |

Siguiente número libre: **D263**.
