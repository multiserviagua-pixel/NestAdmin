# Instrucciones del proyecto

- Lea `PLAN.md` y las especificaciones de `specs/v1/` antes de cambiar el comportamiento. Mantenga requisitos, contratos, matriz y pruebas alineados; registre los cambios de alcance.
- La IA usa por defecto un proveedor externo (cloud/API) elegido y confirmado por el usuario, con su clave en el almacén protegido; la IA local (Ollama) es opcional y sirve de respaldo (D100–D103). El único respaldo automático permitido es del proveedor externo elegido hacia la IA local, en las condiciones de D101; nunca añada respaldo automático hacia otro servicio de IA externo ni de la IA local hacia uno externo, y no envíe a un proveedor externo más contenido que el que la tarea necesita. OCR y extracción de texto siguen siendo locales. Las acciones de envío y registro requieren revisión salvo una regla expresa activa.
- No incluya documentos reales, credenciales, datos personales ni resultados privados en fixtures, registros o el repositorio. Use exclusivamente datos sintéticos para las pruebas y mantenga los resultados de evaluación separados del contenido de usuario.
- Evite efectos duplicados al recuperar trabajos y cubra los fallos de modelos, OCR, red y credenciales. La ausencia de datos debe ser explícita.
- Ejecute `python scripts/check.py` al modificar la documentación. Ejecute además las pruebas y comprobaciones pertinentes del código que se implemente. El verificador documental no acredita que exista o funcione la aplicación.
- No marque criterios de aceptación como cumplidos sin evidencia reproducible. Las pruebas de instalador, escritorio y rendimiento requieren Windows y, cuando corresponda, el hardware objetivo.
- Las pruebas manuales se hacen sobre `tauri dev`, no sobre un instalador. El instalador (`scripts/build-clean.ps1`, `tauri build`, NSIS) se deja para la versión final: no lo construya ni lo proponga antes, salvo que el usuario lo pida expresamente. Cerrar una etapa o incluir commits nuevos no son motivo, y las pruebas de instalación quedan para esa versión. Los cambios corrientes se comprueban con pruebas, clippy, formato y `check.py`. Esta regla vale para todos los agentes y subagentes.
- Desarrollo solo en local (desde el 2026-11-07, hasta nuevo aviso del usuario): se permiten commits, ramas y merges en el repositorio local, pero no `git push`, ni abrir, actualizar o fusionar PR, ni relanzar o esperar el CI de GitHub (`gh pr`, `gh run`). La verificación es local: pruebas, clippy, formato y `check.py`. Esta regla vale para todos los agentes y subagentes.

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
- Todo brief a un subagente repite la regla del instalador: pruebas manuales sobre `tauri dev`; instalador solo para la versión final o si el usuario lo pide.

Skills del proyecto en `.claude/skills/`: `medir-calidad-correo`, `anadir-senal-correo` y `revisar-envio-reglas`.

### Flujo automático

La sesión principal encadena los agentes sin pedir confirmación entre pasos:

1. `explorador` localiza el código, las specs y las pruebas afectadas.
2. `implementador` aplica el cambio con un brief concreto.
3. `verificador` ejecuta las comprobaciones. Si fallan, vuelve a `implementador` con la salida literal (como máximo 3 ciclos); después se informa al usuario.
4. `arquitecto` revisa solo si el cambio toca alcance, credenciales, OAuth, envío o registro automáticos.
5. Para la prueba manual se usa `tauri dev`. El instalador se construye para la versión final o si el usuario lo pide.

La automatización del flujo de desarrollo no cambia las reglas de la aplicación: el único respaldo automático de IA es del proveedor externo elegido a la IA local (D101), nunca hacia otro proveedor externo ni de local a externo, y los envíos y registros requieren revisión salvo regla expresa activa.
