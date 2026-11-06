# Instrucciones del proyecto

- Lea `PLAN.md` y las especificaciones de `specs/v1/` antes de cambiar el comportamiento. Mantenga requisitos, contratos, matriz y pruebas alineados; registre los cambios de alcance.
- Priorice el procesamiento local. Nunca añada respaldo automático hacia un servicio de IA externo. Las acciones de envío y registro requieren revisión salvo una regla expresa activa.
- No incluya documentos reales, credenciales, datos personales ni resultados privados en fixtures, registros o el repositorio. Use exclusivamente datos sintéticos para las pruebas y mantenga los resultados de evaluación separados del contenido de usuario.
- Evite efectos duplicados al recuperar trabajos y cubra los fallos de modelos, OCR, red y credenciales. La ausencia de datos debe ser explícita.
- Ejecute `python scripts/check.py` al modificar la documentación. Ejecute además las pruebas y comprobaciones pertinentes del código que se implemente. El verificador documental no acredita que exista o funcione la aplicación.
- No marque criterios de aceptación como cumplidos sin evidencia reproducible. Las pruebas de instalador, escritorio y rendimiento requieren Windows y, cuando corresponda, el hardware objetivo.
- No construya el instalador (`scripts/build-clean.ps1`, `tauri build`, NSIS) salvo que una prueba concreta lo necesite: es lento y costoso. Valide los cambios con pruebas, clippy, formato, `check.py` y comprobaciones baratas; el instalador se compila una sola vez al final de cada etapa o para una prueba de instalación acordada. Esta regla vale para todos los agentes y subagentes.

## Enrutado de modelos (ahorro de créditos)

La sesión principal usa Opus para coordinar y decidir; el trabajo mecánico se delega a los subagentes de `.claude/agents/`, cada uno con su modelo fijado. Los subagentes sin modelo propio usan Sonnet (`CLAUDE_CODE_SUBAGENT_MODEL` en `.claude/settings.json`).

| Tarea | Agente | Modelo |
|---|---|---|
| Localizar código, requisitos, contratos o pruebas | `explorador` | Haiku |
| Ejecutar `check.py`, fmt, clippy y pruebas | `verificador` | Haiku |
| Implementar un cambio acotado con brief claro | `implementador` | Sonnet |
| Diseño, cambios de alcance, revisión de diffs sensibles | `arquitecto` o sesión principal | Opus |

- Delegue las lecturas amplias y las ejecuciones de pruebas en lugar de hacerlas en la sesión principal.
- No asigne Opus a un subagente salvo para diseño o revisión.
- Workflows: como máximo unos 5 agentes salvo petición expresa. `/code-review` en nivel `low` o `medium` por defecto.
- Todo brief a un subagente repite la regla de no compilar el instalador.
