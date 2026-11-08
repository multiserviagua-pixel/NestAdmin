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
| D123–D135 | `claude/panel-reglas`: panel de reglas, sugerencias y primera lectura | Reservado |
| D136–D145 | `claude/rediseno-ui`: rediseño visual y separación de Pendientes (D136–D138; el resto, hueco), integrado el 2026-11-08 | Asignado |

Siguiente número libre: **D146**.
