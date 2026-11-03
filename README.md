# NestAdmin

Asistente administrativo local para Windows descrito en [PLAN.md](PLAN.md). Ya existen la cola durable, los proveedores de IA (Ollama local por defecto y Groq por elección explícita), la validación de facturas, los simuladores de correo y administración, las reglas con revisión predeterminada, la lectura de texto, PDF y PDF escaneado con OCR local y una primera aplicación de escritorio Tauri. **Faltan el instalador, los conectores reales y las mediciones en el equipo objetivo**; el estado de cada requisito está en la [matriz de trazabilidad](specs/v1/trazabilidad.md).

## Contenido

- [Especificaciones v1](specs/v1/README.md): requisitos, flujos, decisiones, contratos, tareas y trazabilidad de pruebas.
- [Instrucciones del proyecto](AGENTS.md): pautas para cambios y comprobaciones.
- [Harness de evaluación](evaluation/README.md): 10 facturas y 20 correos sintéticos, expectativas separadas, evaluación sin red y pruebas del evaluador.
- [Evidencias](evidence/): ejecuciones registradas de cada caso de prueba.
- Código: `crates/core` (almacenamiento, cola y acciones idempotentes), `crates/ai` (proveedores), `crates/documents` (validación y duplicados), `crates/extract` (texto, PDF y OCR), `crates/simulators`, `crates/app` (flujos, revisión y reglas), `crates/desktop` (ventana Tauri) y `ui/` (interfaz).
- [Integración continua](.github/workflows/verify.yml): especificaciones, evaluador, interfaz y crates en Linux y Windows.

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

Para PDF y OCR, `scripts/setup-ocr.ps1` descarga PDFium y los datos de idioma español e inglés en `vendor/` (comprobados por SHA-256, excluidos de Git) e instala Tesseract 5 con winget:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/setup-ocr.ps1
```

## Abrir la aplicación en desarrollo

```powershell
npm ci --prefix ui
cd crates/desktop
..\..\ui\node_modules\.bin\tauri dev
```

La ventana usa los datos de `%LOCALAPPDATA%\NestAdmin`; para probar sin tocarlos, defina antes `$env:NESTADMIN_DATA_DIR` con otra carpeta. Cerrar la ventana la oculta: para salir, use «Salir de NestAdmin por completo» en el icono del área de notificación. Se importan archivos `.txt` y `.pdf`; las páginas escaneadas pasan por OCR local, y la revisión indica en qué páginas se usó.

Para ver solo la interfaz en un navegador, con datos sintéticos simulados: `npm run dev --prefix ui` y abrir `http://localhost:5173`.

## Verificación local

Desde la raíz del repositorio:

```sh
python scripts/check.py
python -m unittest discover -s scripts/tests -v
python -m unittest discover -s evaluation/tests -v
npm run build --prefix ui
cargo test --workspace
```

`cargo test --workspace` usa un proveedor de IA simulado. La prueba del flujo completo con el Ollama local se ejecuta aparte: `cargo test -p nestadmin-app --test ollama -- --ignored`. Estas comprobaciones no acreditan por sí solas los criterios de aceptación: cada requisito necesita la evidencia indicada en la matriz. El protocolo para puntuar predicciones está en [evaluation/README.md](evaluation/README.md); las predicciones y resultados locales se guardan fuera del contenido del usuario y están excluidos de Git.

Quedan pendientes el empaquetado Windows, las pruebas de OCR, instalador, privacidad y hardware objetivo, y la comparación con Groq.
