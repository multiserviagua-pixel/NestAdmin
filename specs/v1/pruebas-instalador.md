# Casos de prueba del instalador v1

Especificación de las pruebas de R08 (decisiones D36–D41, contrato «Preparación de componentes locales»). Un caso descrito aquí no es una prueba superada: la [matriz](trazabilidad.md) recoge el estado y la evidencia.

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

## T08 — Instalación real en Windows

Requiere Windows 11 limpio (máquina virtual con instantánea) y, para tiempos, el equipo objetivo. Registrar versión del instalador, fecha, red, duración y capturas del asistente.

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
