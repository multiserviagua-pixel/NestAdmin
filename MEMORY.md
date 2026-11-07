# Memoria del proyecto

Decisiones de trabajo que deben mantenerse entre sesiones y agentes. Las reglas completas están en [AGENTS.md](AGENTS.md); el alcance, en [PLAN.md](PLAN.md) y [specs/v1/](specs/v1/).

## Enrutado de modelos (2026-11-06)

- La sesión principal usa Opus y coordina; el trabajo mecánico se delega a los agentes de [.claude/agents/](.claude/agents/).
- `explorador` y `verificador` usan Haiku; `implementador`, Sonnet; `arquitecto`, Opus.
- `CLAUDE_CODE_SUBAGENT_MODEL=sonnet` ([.claude/settings.json](.claude/settings.json)) es el modelo por defecto de los subagentes sin modelo propio. No cambia los agentes integrados Explore y Plan.
- Workflows: como máximo unos 5 agentes salvo petición expresa; `/code-review` en nivel `low` o `medium`.

## Flujo automático

- explorador → implementador → verificador (hasta 3 ciclos de corrección) → arquitecto solo para cambios sensibles, sin pedir confirmación entre pasos.
- La automatización es del flujo de desarrollo. La aplicación mantiene el procesamiento local, sin respaldo automático a una IA externa.

## Instalador (2026-11-06)

- Se construye cuando el flujo de trabajo lo necesita: cierre de etapa, prueba de instalación o una prueba que dependa de él. En esos casos está autorizado sin confirmación.
- Los cambios corrientes se validan con pruebas, clippy, formato y `python scripts/check.py`.

## Repositorio

- Solo se versiona lo necesario para que el proyecto funcione y se verifique: código, specs, pruebas, scripts, evidencias, el sitio y la configuración compartida de agentes.
- Fuera de git: `.claude/` salvo `agents/` y `settings.json`, resultados de evaluación, credenciales (`correo-oauth.json`, `.env`), `target/`, `vendor/` y `node_modules/`.
