# NestAdmin

Asistente administrativo local para Windows descrito en [PLAN.md](PLAN.md). Ya existen la cola durable, los proveedores de IA (Ollama cloud por defecto, y la IA local, compatible con OpenAI y Claude por elección explícita), la validación de facturas, los simuladores de correo y administración, las reglas con revisión predeterminada, la lectura de texto, PDF y PDF escaneado con OCR local, una aplicación de escritorio Tauri y su instalador NSIS de una sola ejecución, que incluye el OCR y termina sin preparar la IA local (D103). Por defecto la IA es externa (Ollama cloud, con la clave y el destino que confirma el usuario); la IA local es opcional y sirve de respaldo (D100–D102). El correo real de Gmail y Microsoft 365 se conecta desde la sección «Correo» de la ventana, con varias cuentas a la vez, cuando la instalación incluye los clientes OAuth del editor (`correo-oauth.json`, ver [D43](specs/v1/decisiones.md)). **Faltan el ensayo con cuentas reales desde la ventana, el sistema administrativo real y las mediciones en el equipo objetivo**; el estado de cada requisito está en la [matriz de trazabilidad](specs/v1/trazabilidad.md).

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
..\..\ui\node_modules\.bin\tauri dev --features lectura-bajo-demanda
```

La feature `lectura-bajo-demanda` es el canal de prueba: la IA solo afina las sugerencias de reglas cuando usted lo pide (con un aviso previo del proveedor y los datos); sin ella (versión final) el afinado automático está desactivado hasta que se consienta en el panel. `scripts\dev-pruebas.ps1` y `scripts\build-clean.ps1 -Prueba` la activan.

La ventana usa los datos de `%LOCALAPPDATA%\NestAdmin`; para probar sin tocarlos, defina antes `$env:NESTADMIN_DATA_DIR` con otra carpeta. Cerrar la ventana la oculta: para salir, use «Salir de NestAdmin por completo» en el icono del área de notificación. Se importan archivos `.txt`, `.pdf` y fotos `.jpg` o `.png`; las páginas escaneadas y las fotos pasan por OCR local, y la revisión indica en qué páginas se usó y si su calidad es baja.

Para ver solo la interfaz en un navegador, con datos sintéticos simulados: `npm run dev --prefix ui` y abrir `http://localhost:5173`. `?preparar` imita el modo del instalador y `?ram=12` u `?ram=8`, un equipo con poca memoria.

## Requisitos del equipo

- Windows de 64 bits; solo se ha probado en Windows 11.
- **Memoria: 16 GB recomendados solo si se instala la IA local opcional** (Ollama con `qwen3:8b`, unos 5,2 GB), el equipo objetivo de [PLAN.md](PLAN.md) con Intel Core Ultra 5. Con menos de 16 GB funciona más despacio; **con 8 GB o menos, la IA local probablemente no responderá a tiempo**. El aviso de memoria solo aparece al preparar o usar la IA local; si una petición local agota el tiempo, el mensaje indica la memoria como causa probable (D46). Con la IA externa por defecto la memoria local no es el límite.
- Espacio en disco para la aplicación y, solo con la IA local opcional, para Ollama y el modelo (unos 5,2 GB).
- Internet para el proveedor de IA externo y el correo. La instalación solo lo necesita para WebView2 si falta; no descarga Ollama ni el modelo (D103).

Los umbrales son provisionales hasta medir en el equipo objetivo (R12).

## IA externa por defecto y qué datos salen del equipo

En una instalación nueva la selección inicial es **Ollama cloud** con el modelo `gemma4:31b` (D100). El asistente de primer uso guía cuatro pasos: guardar la clave del proveedor (va al Administrador de credenciales de Windows, nunca a archivos), elegir el modelo de la lista que devuelve el proveedor, confirmar el destino al que saldrá el texto y superar una prueba con un texto sintético, sin datos del usuario (D100, D122). Hasta entonces las tareas esperan en la cola y no sale nada. Las alternativas, por elección explícita, son un servicio compatible con OpenAI, Claude y la IA local.

Qué sale del equipo con un proveedor externo (D102): solo lo que la tarea necesita, es decir, las instrucciones y el esquema de salida, el texto del documento ya extraído en el equipo (la lectura de PDF y el OCR son siempre locales) y, de un correo, su asunto, remitente, destinatarios, fecha y cuerpo en texto. **No salen** el archivo original, las imágenes ni la huella, la base de datos, los tokens ni las credenciales de correo, otras claves, las direcciones de los buzones (salvo que aparezcan en el propio correo), los nombres de cuenta, las rutas ni el historial. Cada clave viaja solo a su proveedor. Con Ollama cloud el destino es Ollama Inc., principalmente en EE. UU. (D108). La alternativa para que el texto no salga del equipo es la IA local (Ollama con `qwen3:8b`), que se instala desde el asistente o desde Configuración («Instalar IA local»). Si el proveedor externo no responde y el respaldo local está activado y preparado, esa tarea se repite con la IA local y el resultado lo indica; nunca se pasa a otro proveedor externo ni de la IA local a uno externo (D101).

**Pendiente:** `gemma4:31b` en Ollama cloud no está validado hasta medirlo con el corpus completo en el equipo objetivo (D104, D105; en la medición del 2026-11-07 fallaron los 20 correos), y la revisión con el responsable de protección de datos (encargo de tratamiento y transferencias, D93, D111) está pendiente antes de usar la aplicación con datos reales.

## Generar el instalador

Con `vendor/` preparado por `scripts/setup-ocr.ps1`, se generan primero los avisos de licencia que viajan en el instalador (`vendor/THIRD_PARTY_NOTICES.txt`, D60):

```powershell
python scripts/third_party.py --textos vendor/THIRD_PARTY_NOTICES.txt
npm ci --prefix ui
cd crates/desktop
..\..\ui\node_modules\.bin\tauri build --config tauri.installer.json
```

Así se construye sin el cliente OAuth del editor: el correo real figurará como «no configurado». Para incluirlo, use `scripts/build-clean.ps1 -ClienteOAuth` (siguiente apartado) o añada a mano un segundo `--config` con el recurso, escrito fuera del repositorio:

```powershell
..\..\ui\node_modules\.bin\tauri build --config tauri.installer.json --config C:\fuera\oauth-recurso.json
# oauth-recurso.json: { "bundle": { "resources": { "C:/fuera/correo-oauth.json": "correo-oauth.json" } } }
```

El instalador queda en `target/release/bundle/nsis/`. Se instala por usuario en `%LOCALAPPDATA%\Programs\NestAdmin` e incluye PDFium, Tesseract portátil, los datos de idioma y los avisos de licencia de terceros; los datos van aparte, en `%LOCALAPPDATA%\NestAdmin`. Instalar encima de una versión anterior, que ponía el programa en la carpeta de datos, retira de ahí solo los archivos del programa y reapunta los accesos directos; desinstalar quita el programa y conserva los datos (D52). Incluye PDFium, Tesseract portátil y los datos de idioma. Desde D103 el instalador ya no prepara la IA local: copia el programa, el OCR y los datos de idioma, instala WebView2 si falta y termina, sin Internet salvo para WebView2. Al abrir NestAdmin con un proveedor externo solo se comprueba el OCR del paquete y no se arranca nada local; si falta, la ventana muestra un aviso. Con la IA local elegida, un Ollama detenido se arranca solo y, si falta algo, la ventana muestra el asistente (D39). Ollama y el modelo `qwen3:8b` solo se instalan desde el asistente de primer uso o desde Configuración, con la comprobación de la huella SHA-256 y de la firma de Ollama Inc. (D37, D38), y el aviso de memoria de D46; `nestadmin.exe --preparar` sigue disponible desde la terminal.

### Cliente OAuth del editor

Gmail y Microsoft 365 necesitan los clientes OAuth de quien distribuye NestAdmin, en `correo-oauth.json` (D43, D51). Ese archivo **nunca entra en Git ni en la CI**: se guarda en la máquina de empaquetado, fuera del repositorio (`.gitignore` lo excluye por si acaso). El formato está en `crates/desktop/correo-oauth.ejemplo.json`.

1. **Google.** En Google Cloud Console, «APIs y servicios» → «Credenciales» → «Crear credenciales» → «ID de cliente de OAuth», tipo **App de escritorio**. Descargue el JSON (`client_secret_….json`): sus valores están en el bloque `installed` (`client_id` y `client_secret`). Un cliente de tipo «Aplicación web» (bloque `web`) no sirve. Este tipo admite cualquier puerto de `127.0.0.1` sin registrar la redirección.
2. **Microsoft.** En Entra ID, «Registros de aplicaciones» → su aplicación → «Información general»: copie el **Id. de aplicación (cliente)**, un GUID. La aplicación necesita la plataforma «Aplicaciones móviles y de escritorio» con una redirección de bucle local: NestAdmin usa `http://127.0.0.1:<puerto>/` con un puerto variable. Este registro no se ha ensayado aún con Microsoft 365 real (T13).
3. Genere el archivo fuera del repositorio (basta con uno de los dos proveedores):

   ```powershell
   powershell -NoProfile -ExecutionPolicy Bypass -File scripts/correo-oauth.ps1 -Google C:\fuera\client_secret_XXXX.json -MicrosoftClientId <GUID> -Salida C:\fuera\correo-oauth.json
   ```

4. Construya el instalador con él. También puede indicarse con la variable `NESTADMIN_OAUTH_INSTALADOR`:

   ```powershell
   powershell -NoProfile -ExecutionPolicy Bypass -File scripts/build-clean.ps1 -ClienteOAuth C:\fuera\correo-oauth.json
   ```

El instalador lo copia junto a `nestadmin.exe`. La construcción comprueba que el archivo está fuera del repositorio, que es JSON con al menos un cliente y que el paquete lleva exactamente ese archivo, sin mostrar su contenido. Sin `-ClienteOAuth` el instalador se construye igual. Para desarrollo, `NESTADMIN_MAIL_OAUTH` apunta la aplicación a otro archivo.

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
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/build-clean.ps1                                  # sin cliente OAuth
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/build-clean.ps1 -ClienteOAuth C:\fuera\correo-oauth.json
```

`cargo test --workspace` usa un proveedor de IA simulado. La prueba del flujo completo con el Ollama local se ejecuta aparte: `cargo test -p nestadmin-app --test ollama -- --ignored`. Estas comprobaciones no acreditan por sí solas los criterios de aceptación: cada requisito necesita la evidencia indicada en la matriz. El protocolo para puntuar predicciones está en [evaluation/README.md](evaluation/README.md); las predicciones y resultados locales se guardan fuera del contenido del usuario y están excluidos de Git.

Quedan pendientes la firma del instalador, su ensayo en un equipo limpio, las pruebas de privacidad y hardware objetivo, y la medición de Ollama cloud (`gemma4:31b`) en el equipo objetivo.
