# Casos de prueba del instalador v1

Especificación de las pruebas de R08 (decisiones D36–D40, D46, D50 y D51, contrato «Preparación de componentes locales»). Un caso descrito aquí no es una prueba superada: la [matriz](trazabilidad.md) recoge el estado y la evidencia.

Reglas comunes:

- Solo datos sintéticos. Ninguna prueba automática instala nada en el equipo ni sale a la red: Ollama y el servidor de descargas se simulan en 127.0.0.1 y las operaciones del sistema (buscar, verificar firma, instalar y arrancar Ollama) se sustituyen por un doble que registra las llamadas.
- Cada caso comprueba el resultado, el código de error del contrato cuando falla, los eventos emitidos (paso y estado final) y las llamadas al sistema, para demostrar que no se repite trabajo.
- Ningún caso, ni siquiera un fallo, puede activar un proveedor externo.

## T08b — Preparación automática (sin Windows real)

| Caso | Situación inicial | Resultado esperado |
| --- | --- | --- |
| T08b-01 | Equipo limpio: sin Ollama, sin modelo. | `check` → `missing: ollama`. `prepare` descarga, verifica la firma, instala, arranca, descarga el modelo y verifica. Los pasos terminan en `done`; hay progreso en bytes del instalador y del modelo; el instalador descargado se borra; después `check` → `ready`. |
| T08b-02 | Ollama compatible en marcha y modelo presente. | `check` → `ready`. `prepare` no instala, no arranca y no descarga: `ollama`, `service` y `model` en `skipped`, `verify` en `done`. |
| T08b-03 | Ollama compatible instalado y detenido. | `check` → `stopped`. `prepare` solo lo arranca; no reinstala. |
| T08b-04 | Ollama en marcha con versión anterior a la mínima. | `check` → `missing: ollama_outdated`. `prepare` actualiza y el informe da la versión fijada. |
| T08b-05 | Ollama anterior a la mínima, instalado y detenido. | La versión se descubre al arrancarlo; entonces se actualiza y se vuelve a arrancar. |
| T08b-06 | La descarga del instalador y la del modelo se cortan una vez. | El instalador continúa con una petición `Range` desde el byte guardado; el modelo se vuelve a pedir. Hay un evento «Reintento 2 de 3» en cada paso y la preparación termina bien. |
| T08b-07 | La descarga del modelo falla por red en todos los intentos. | Tras 3 intentos, error `network` con último evento `failed` en `model`. Un reintento posterior continúa sin repetir pasos ya hechos. |
| T08b-08 | El instalador descargado no tiene firma válida de Ollama Inc. | Error `signature`; el instalador no se ejecuta y el archivo se borra. |
| T08b-09 | Ollama rechaza el modelo por falta de espacio. | Error `model_failed` con el motivo; no se reintenta automáticamente. |
| T08b-10 | El modelo responde vacío. | Error `verify_failed`; el instalador no da la preparación por buena. |
| T08b-11 | El usuario cancela durante una descarga. | Error `cancelled` antes de instalar nada; último evento `failed`. |
| T08b-12 | Faltan los componentes de OCR del paquete o no responden. | `check` → `missing: ocr`; `prepare` falla con `ocr_missing` sin descargar nada. |
| T08b-13 | Errores y estados serializados. | Códigos y formas exactos del contrato, sin secretos ni rutas de usuario. |
| T08b-14 | El doble informa 16, 15,6, 12, 9, 8 y 7,7 GiB, o no informa memoria. | Niveles `recommended` (16 y 15,6), `below_recommended` (12 y 9), `insufficient` (8 y 7,7) y `unknown`. Sin aviso con 16 GB; aviso que recomienda 16 GB con 12; aviso de que probablemente no responderá a tiempo con 8. Con menos de 6 GiB libres se sugiere cerrar programas. Sin memoria leída no se avisa. |
| T08b-15 | Todo listo, con 16, 12 y 8 GB. | `status` da `ready` con el nivel que corresponde y `prepare` termina bien en los tres casos: el aviso no bloquea, no instala ni arranca nada más y no cambia de proveedor. |
| T08b-16 | El modelo tarda más que el plazo de verificación, con 8, 12 y 16 GB. | Error `verify_failed` con mensaje `timeout:`; con 8 y 12 GB el mensaje cita la falta de memoria como causa probable y con 16 GB no. |
| T08b-17 | Una tarea con Ollama agota el tiempo en un equipo de 8 GB. | El error conserva el código `timeout` y su mensaje cita la memoria del equipo. Con 16 GB, o con Groq elegido, el error no cambia. |

## T08c — Comprobación al abrir la aplicación (D39)

| Caso | Situación inicial | Resultado esperado |
| --- | --- | --- |
| T08c-01 | Proveedor Ollama, servicio detenido, todo instalado. | Se arranca sin intervención y la ventana principal se abre; abrir de nuevo no lo vuelve a arrancar. |
| T08c-02 | Proveedor Ollama, falta el modelo. | No descarga en segundo plano: la ventana muestra el asistente. |
| T08c-03 | Proveedor Groq elegido. | No se comprueba ni se arranca nada local. |
| T08c-04 | La comprobación tarda (servicio lento al arrancar). | La ventana no se bloquea mientras espera. |

## T08d — Construcción del paquete (D50, D51)

Se comprueban en la máquina de empaquetado con `scripts/build-clean.ps1`, sin instalar nada. El cliente OAuth de las pruebas es sintético (valores inventados); nunca el real.

| Caso | Situación inicial | Resultado esperado |
| --- | --- | --- |
| T08d-01 | Sin `-ClienteOAuth` ni `NESTADMIN_OAUTH_INSTALADOR`. | El instalador se construye; el paquete no contiene `correo-oauth.json` y el paso lo dice. |
| T08d-02 | `-ClienteOAuth` con un archivo sintético fuera del repositorio. | El paquete contiene `correo-oauth.json` junto al ejecutable, idéntico al indicado (misma huella); su contenido no aparece en la salida; el árbol del clon queda limpio y la configuración temporal se borra. |
| T08d-03 | `-ClienteOAuth` inexistente, dentro del repositorio o de la carpeta de construcción, que no es JSON o sin ningún cliente. | Falla antes de clonar con un mensaje que lo explica. Con marcadores `PONER-AQUI` solo avisa. |
| T08d-04 | `scripts/correo-oauth.ps1` con un JSON sintético de Google de tipo `installed` y un GUID. | Escribe `google` y `microsoft` en UTF-8 sin BOM. Rechaza un JSON de tipo `web`, un ID que no es GUID, una salida dentro del repositorio y sobrescribir sin `-Sobrescribir`. |
| T08d-05 | Ganchos NSIS. | `makensis` compila el instalador con los ganchos; el script generado contiene la redirección de carpeta, la retirada de la copia anterior y el aviso de desinstalación. |

## T08 — Instalación real en Windows

Requiere Windows 11 limpio (máquina virtual con instantánea) y, para tiempos, el equipo objetivo. Registrar versión del instalador, fecha, red, duración y capturas del asistente. `scripts/t08-registro.ps1`, ejecutado dentro de la máquina virtual, instala o desinstala y anota en un archivo de texto el estado antes y después: versión de Windows, WebView2, Ollama y modelo, huella del instalador, código de salida, duración y archivos instalados, sin nombres de equipo ni de usuario. Una ejecución en Windows 10 22H2 se registra aparte como compatibilidad adicional; no sustituye a la de Windows 11.

| Caso | Situación inicial | Resultado esperado |
| --- | --- | --- |
| T08-01 | Windows limpio con Internet. | Una sola ejecución del instalador termina con la IA verificada; al abrir el icono, la aplicación procesa un documento sintético sin más pasos. Sin aviso de elevación salvo si falta WebView2. |
| T08-02 | Ollama y modelo ya instalados. | No se descargan de nuevo; la instalación termina en poco tiempo. |
| T08-03 | Red cortada a mitad de la descarga del modelo. | El asistente informa, permite reintentar y continúa desde lo descargado. |
| T08-04 | Sin Internet desde el inicio. | Mensaje claro y opción de reintentar; al cancelar, el instalador termina y la aplicación muestra el asistente al abrirse. |
| T08-05 | Cancelar el asistente y abrir la aplicación después. | El asistente reaparece y completa lo que faltaba. |
| T08-06 | Reiniciar el equipo tras instalar y abrir el icono. | Ollama se arranca solo y la aplicación funciona (T08c-01 en real). |
| T08-07 | Instalar encima de una versión anterior de NestAdmin. | Conserva datos y configuración; la comprobación final pasa sin descargas. |
| T08-08 | Instalación silenciosa (`/S`). | Mismo resultado que T08-01 sin preguntas. |
| T08-09 | Desinstalar. | Se quita NestAdmin; Ollama y el modelo se conservan porque pueden usarlos otras aplicaciones, y así se informa. |
| T08-10 | Instalación nueva (D51). | El programa queda en `%LOCALAPPDATA%\Programs\NestAdmin` (también con `/S`); al abrir, los datos se crean en `%LOCALAPPDATA%\NestAdmin`, que no contiene nada del programa. El acceso del menú Inicio apunta a la carpeta nueva. |
| T08-11 | Instalar la versión nueva encima de una anterior instalada en `%LOCALAPPDATA%\NestAdmin`, con tareas, una cuenta de correo y configuración propias; con interfaz (eligiendo desinstalar antes y sin hacerlo) y con `/S`; una vez con la versión anterior abierta. | Los datos siguen y la aplicación los muestra. En `%LOCALAPPDATA%\NestAdmin` ya no quedan `nestadmin.exe`, `uninstall.exe`, `pdfium\`, `tessdata\` ni `tesseract\`. Hay una sola entrada en «Aplicaciones instaladas», con la carpeta nueva. Los accesos del menú Inicio, del escritorio y de la barra de tareas abren la versión nueva. La copia abierta se cierra antes de retirarla (con interfaz, preguntando). |
| T08-12 | Desinstalar tras T08-10 o T08-11, con interfaz (marcando y sin marcar la casilla de borrar datos de la aplicación) y con `/S`. | Desaparece `%LOCALAPPDATA%\Programs\NestAdmin` y su entrada; `%LOCALAPPDATA%\NestAdmin` queda intacta en los tres casos. Con interfaz, el aviso da la ruta de los datos; en silencio, el registro de detalles. |
| T08-13 | Instalador construido con y sin `-ClienteOAuth` (con los clientes reales del editor, solo en la máquina de prueba). | Con él, `correo-oauth.json` está junto a `nestadmin.exe` y «Correo» ofrece añadir cuentas de Gmail y Microsoft 365; sin él, ambos figuran como no configurados y la aplicación funciona igual. |
| T08-14 | En el asistente con interfaz, elegir otra carpeta (por ejemplo `D:\Apps\NestAdmin`) y después reinstalar. | Se instala en la carpeta elegida y la reinstalación la mantiene; no se mueve a `Programs`. |
