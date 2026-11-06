# Instrucciones del proyecto

- Lea `PLAN.md` y las especificaciones de `specs/v1/` antes de cambiar el comportamiento. Mantenga requisitos, contratos, matriz y pruebas alineados; registre los cambios de alcance.
- Priorice el procesamiento local. Nunca añada respaldo automático hacia un servicio de IA externo. Las acciones de envío y registro requieren revisión salvo una regla expresa activa.
- No incluya documentos reales, credenciales, datos personales ni resultados privados en fixtures, registros o el repositorio. Use exclusivamente datos sintéticos para las pruebas y mantenga los resultados de evaluación separados del contenido de usuario.
- Evite efectos duplicados al recuperar trabajos y cubra los fallos de modelos, OCR, red y credenciales. La ausencia de datos debe ser explícita.
- Ejecute `python scripts/check.py` al modificar la documentación. Ejecute además las pruebas y comprobaciones pertinentes del código que se implemente. El verificador documental no acredita que exista o funcione la aplicación.
- No marque criterios de aceptación como cumplidos sin evidencia reproducible. Las pruebas de instalador, escritorio y rendimiento requieren Windows y, cuando corresponda, el hardware objetivo.
- Construya el instalador (`scripts/build-clean.ps1`, `tauri build`, NSIS) solo cuando el flujo de trabajo lo necesite: al cerrar una etapa, para una prueba de instalación o cuando una prueba concreta dependa de él. Está autorizado en esos casos sin pedir confirmación; no lo compile para validar cambios corrientes, que se comprueban con pruebas, clippy, formato y `check.py`. Esta regla vale para todos los agentes y subagentes.

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
- Todo brief a un subagente repite la regla del instalador: solo cuando el flujo lo necesite.

### Flujo automático

La sesión principal encadena los agentes sin pedir confirmación entre pasos:

1. `explorador` localiza el código, las specs y las pruebas afectadas.
2. `implementador` aplica el cambio con un brief concreto.
3. `verificador` ejecuta las comprobaciones. Si fallan, vuelve a `implementador` con la salida literal (como máximo 3 ciclos); después se informa al usuario.
4. `arquitecto` revisa solo si el cambio toca alcance, credenciales, OAuth, envío o registro automáticos.
5. Si el flujo cierra una etapa o requiere una prueba de instalación, se construye el instalador.

La automatización del flujo de desarrollo no cambia las reglas de la aplicación: sin respaldo automático a una IA externa y con revisión de envíos y registros salvo regla expresa activa.
