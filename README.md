# NestAdmin

Preparación del asistente administrativo local para Windows descrito en [PLAN.md](PLAN.md). El repositorio contiene el plan, especificaciones versionadas y herramientas de desarrollo y evaluación; **la aplicación, los conectores reales, el instalador y los modelos aún no están implementados**.

## Contenido

- [Especificaciones v1](specs/v1/README.md): requisitos, flujos, decisiones, contratos, tareas y trazabilidad de pruebas. Los criterios funcionales están pendientes de evidencia.
- [Instrucciones del proyecto](AGENTS.md): pautas para cambios y comprobaciones.
- [Harness de evaluación](evaluation/README.md): 10 facturas y 20 correos sintéticos, expectativas separadas, evaluación sin red y pruebas del evaluador.
- [Integración continua](.github/workflows/verify.yml): comprobaciones de especificaciones y del evaluador en Linux y Windows.

## Herramientas de desarrollo

En Windows, `scripts/setup-dev.ps1` instala mediante winget lo que falte de Python 3.12, Node.js LTS, rustup y MSVC Build Tools, y reutiliza lo ya instalado:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/setup-dev.ps1
```

Puede requerir permisos de administrador. No instala Ollama, modelos ni OCR.

Para la IA local, `scripts/setup-ai.ps1` instala Ollama y descarga `qwen3:8b` (unos 5,2 GB), reutilizando lo ya presente:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/setup-ai.ps1
```

## Verificación local

Con Python 3.9 o posterior, sin instalar dependencias adicionales, desde la raíz del repositorio:

```sh
python scripts/check.py
python -m unittest discover -s scripts/tests -v
python -m unittest discover -s evaluation/tests -v
cargo test
```

`cargo test` prueba el núcleo de `crates/core` (almacenamiento, cola durable y acciones idempotentes) y los simuladores locales de correo y administración de `crates/simulators` (sin red ni cuentas reales, con estado opcional en JSON); todavía no existe la aplicación de escritorio. Estas comprobaciones verifican la estructura y trazabilidad documental y el funcionamiento del evaluador; no acreditan los criterios de aceptación del producto. El protocolo para puntuar y comparar predicciones existentes se encuentra en [evaluation/README.md](evaluation/README.md). No se han medido todavía Ollama ni Groq; el evaluador no invoca modelos ni mide por sí mismo tiempo o memoria.

La construcción y el empaquetado de Windows quedan pendientes de implementar junto con la aplicación y de fijar versiones de sus herramientas y dependencias. También quedan pendientes las pruebas de interfaz, OCR, instalador, privacidad y hardware objetivo. Los resultados locales de evaluación se guardan fuera del contenido del usuario y están excluidos de Git.
