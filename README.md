# NestAdmin

Asistente administrativo local para Windows descrito en [PLAN.md](PLAN.md). Ya existen la cola durable, los proveedores de IA (Ollama local por defecto y Groq por elección explícita), la validación de facturas, los simuladores de correo y administración, las reglas con revisión predeterminada, la lectura de texto, PDF y PDF escaneado con OCR local, una aplicación de escritorio Tauri y su instalador NSIS de una sola ejecución, que incluye el OCR y termina cuando la IA local responde. El correo real de Gmail y Microsoft 365 se conecta desde la sección «Correo» de la ventana, con varias cuentas a la vez, cuando la instalación incluye los clientes OAuth del editor (`correo-oauth.json`, ver [D43](specs/v1/decisiones.md)). **Faltan el ensayo con cuentas reales desde la ventana, el sistema administrativo real y las mediciones en el equipo objetivo**; el estado de cada requisito está en la [matriz de trazabilidad](specs/v1/trazabilidad.md).

## Contenido

- [Especificaciones v1](specs/v1/README.md): requisitos, flujos, decisiones, contratos, tareas y trazabilidad de pruebas.
- [Instrucciones del proyecto](AGENTS.md): pautas para cambios y comprobaciones.
- [Harness de evaluación](evaluation/README.md): 10 facturas y 20 correos sintéticos, expectativas separadas, evaluación sin red y pruebas del evaluador.
- [Evidencias](evidence/): ejecuciones registradas de cada caso de prueba.
- Código: `crates/core` (almacenamiento, cola y acciones idempotentes), `crates/ai` (proveedores), `crates/documents` (validación y duplicados), `crates/extract` (texto, PDF y OCR), `crates/setup` (preparación del equipo), `crates/mail` (conectores de Microsoft 365 y Gmail), `crates/simulators`, `crates/app` (flujos, revisión y reglas), `crates/desktop` (ventana Tauri) y `ui/` (interfaz).
- [Integración continua](.github/workflows/verify.yml): especificaciones, evaluador, interfaz y crates en Linux y Windows.

## Herramientas de desarrollo

En Windows, `scripts/setup-dev.ps1` deja las herramientas en las versiones fijadas ([decisiones](specs/v1/decisiones.md), «Versiones de desarrollo fijadas»): Python 3.12, Node.js de `.node-version`, rustup con la versión de Rust de `rust-toolchain.toml`, Visual Studio Build Tools 2022 con MSVC x64 y el SDK de Windows 11, y WebView2. Instala con winget solo lo que falta o no coincide y reutiliza lo demás, así que se puede repetir. Solo pide las confirmaciones de Windows (UAC) de los instaladores que lo necesitan. NSIS no hace falta: tauri-cli lo descarga al empaquetar.

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/setup-dev.ps1
```

Para ver el estado sin instalar ni cambiar nada (sale con código 1 si falta algo):

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/setup-dev.ps1 -Comprobar
```

No instala Ollama, modelos ni OCR.

Para la IA local, `scripts/setup-ai.ps1` instala Ollama y descarga `qwen3:8b` (unos 5,2 GB), reutilizando lo ya presente:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/setup-ai.ps1
```

Para PDF y OCR, `scripts/setup-ocr.ps1` descarga PDFium y los datos de idioma español e inglés en `vendor/` (comprobados por SHA-256, excluidos de Git), instala Tesseract 5.4.0 con winget y copia su versión portátil a `vendor/tesseract` para el paquete:

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

Para ver solo la interfaz en un navegador, con datos sintéticos simulados: `npm run dev --prefix ui` y abrir `http://localhost:5173`. `?preparar` imita el modo del instalador y `?ram=12` u `?ram=8`, un equipo con poca memoria.

## Requisitos del equipo

- Windows de 64 bits; solo se ha probado en Windows 11.
- **Memoria: 16 GB recomendados** para la IA local (Ollama con `qwen3:8b`, unos 5,2 GB), el equipo objetivo de [PLAN.md](PLAN.md) con Intel Core Ultra 5. Con menos de 16 GB funciona más despacio; **con 8 GB o menos, la IA local probablemente no responderá a tiempo**. La instalación no se bloquea: el asistente lo advierte y, si una petición agota el tiempo, el mensaje indica la memoria como causa probable. Groq puede elegirse a mano en Configuración; NestAdmin nunca lo activa por su cuenta (D46).
- Espacio en disco para Ollama y el modelo (este, unos 5,2 GB), además de la aplicación.
- Internet durante la instalación, si faltan Ollama o el modelo.

Los umbrales son provisionales hasta medir en el equipo objetivo (R12).

## Generar el instalador

Con `vendor/` preparado por `scripts/setup-ocr.ps1`, se generan primero los avisos de licencia que viajan en el instalador (`vendor/THIRD_PARTY_NOTICES.txt`, D60):

```powershell
python scripts/third_party.py --textos vendor/THIRD_PARTY_NOTICES.txt
npm ci --prefix ui
cd crates/desktop
..\..\ui\node_modules\.bin\tauri build --config tauri.installer.json
```

El instalador queda en `target/release/bundle/nsis/`. Se instala por usuario e incluye PDFium, Tesseract portátil, los datos de idioma y los avisos de licencia de terceros. Antes de terminar, ejecuta `nestadmin.exe --preparar`, que instala o arranca Ollama y descarga el modelo `qwen3:8b` si faltan, comprueba la huella SHA-256 y la firma de Ollama Inc. del instalador y solo da la instalación por buena cuando el modelo responde (D36–D38). Al abrir NestAdmin, un Ollama detenido se arranca solo; si falta algo, la ventana muestra el mismo asistente (D39). El asistente avisa si la memoria es menor que la recomendada (D46).

## Licencias de terceros

[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) lista los componentes que se distribuyen o se descargan (versión, verificación y licencia), los crates de Rust enlazados en `nestadmin.exe` y los paquetes npm de la interfaz. Lo genera `python scripts/third_party.py` desde `Cargo.lock` y `ui/package-lock.json`: tras cambiar una dependencia, vuelva a ejecutarlo; CI falla con `--check` si no está al día. Es la licencia que declara cada paquete, no una revisión legal; los puntos pendientes antes de publicar están en los riesgos de [decisiones](specs/v1/decisiones.md).

## Actualizaciones

La [política de actualizaciones](specs/v1/actualizaciones.md) resume qué se actualiza y cómo: NestAdmin con versiones firmadas que el usuario acepta al abrir y nunca a mitad de una tarea (propuesta pendiente de la clave de firma del editor), Ollama solo desde el asistente cuando queda por debajo de la versión mínima, el modelo fijado por digest, el OCR junto con NestAdmin y WebView2 por Windows. Antes de migrar la base, NestAdmin guarda una copia `nestadmin.esquema-N.db` para poder volver a la versión anterior.

## Verificación local

Desde la raíz del repositorio:

```sh
python scripts/check.py
python scripts/third_party.py --check
python -m unittest discover -s scripts/tests -v
python -m unittest discover -s evaluation/tests -v
npm run build --prefix ui
cargo test --workspace
```

Para comprobar que todo esto funciona desde cero, `scripts/build-clean.ps1` clona el repositorio en una carpeta nueva y ejecuta allí todos los pasos, incluido el instalador. Termina comprobando que lo generado queda ignorado por Git y que las pruebas no tocan los datos del usuario (T11):

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/build-clean.ps1
```

`cargo test --workspace` usa un proveedor de IA simulado. La prueba del flujo completo con el Ollama local se ejecuta aparte: `cargo test -p nestadmin-app --test ollama -- --ignored`. Estas comprobaciones no acreditan por sí solas los criterios de aceptación: cada requisito necesita la evidencia indicada en la matriz. El protocolo para puntuar predicciones está en [evaluation/README.md](evaluation/README.md); las predicciones y resultados locales se guardan fuera del contenido del usuario y están excluidos de Git.

Quedan pendientes la firma del instalador, su ensayo en un equipo limpio, las pruebas de privacidad y hardware objetivo, y la comparación con Groq.
