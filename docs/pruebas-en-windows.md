# Pruebas en el equipo Windows

Lo que queda del plan y solo se puede acreditar en Windows: la versión de prueba, las pruebas desde la ventana, la instalación y las mediciones. Usar solo datos sintéticos (`crates/extract/tests/fixtures/`, el correo simulado y el corpus de `evaluation/`). Anotar fecha, versión (commit), resultado y códigos de error en el archivo de evidencia indicado. No anotar direcciones, asuntos ni documentos reales.

## Forma de trabajo (desde el 2026-11-07)

Las pruebas diarias se hacen con la versión de desarrollo de `main`, sin instalar nada (en el equipo de desarrollo también con el acceso «NestAdmin main (pruebas).bat» del escritorio):

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\dev-pruebas.ps1
```

Usa una carpeta de datos aparte (`%USERPROFILE%\NestAdmin-pruebas`) para no tocar ni migrar la de la versión instalada, y copia allí `correo-oauth.json` de la instalación. Las credenciales del Administrador de credenciales son las mismas. Antes hay que cerrar la versión instalada desde el área de notificación. Sirve para las secciones 2, 3, 4 y 6 y para el correo real; no sirve para la 1 (instalador), para la 5 en lo que depende del paquete ni para medir (la 7 usa `t12-medir.ps1`, que compila optimizado). El instalador se construye una sola vez al cerrar cada etapa (sección 0).

## 0. Construir la versión de prueba

Desde la carpeta del repositorio, en la rama `main` (el desarrollo es solo local desde el 2026-11-07, sin `git pull`):

```powershell
git switch main
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\setup-dev.ps1 -Comprobar
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\setup-ocr.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\build-clean.ps1 -Prueba -ClienteOAuth C:\NestAdmin-config\correo-oauth.json
```

`-Prueba` construye el **instalador de prueba** (feature `lectura-bajo-demanda`, versión con sufijo `-prueba`): la IA solo afina las sugerencias de reglas (y, más adelante, lee la bandeja) cuando usted lo pide, tras ver a qué proveedor se envía y qué datos, para que no gaste sola. **Sin `-Prueba` el instalador es el de la versión final**, donde el afinado automático existe pero está desactivado hasta que la persona lo consiente en el panel de reglas; úselo solo para probar esa versión. Para la versión de desarrollo, `scripts\dev-pruebas.ps1` ya activa la feature.

`-ClienteOAuth` es opcional: sin él, Gmail y Microsoft figuran como no configurados (guía de Microsoft: [guia-microsoft-365.pdf](guia-microsoft-365.pdf)). La construcción tarda: hágala una sola vez para todas las pruebas de esta lista.

## 1. Instalación (T08, T08d) — `evidence/t08-instalador.md`

Instalar con el script de registro, que ya no se queda esperando al terminar. Desde D103 el instalador no prepara la IA local, así que el script no exige la respuesta del modelo; la comprobación de la IA local es opcional (`-ComprobarIALocal`) y su ausencia no acredita nada sobre la IA local ni sobre el proveedor externo:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\t08-registro.ps1 -Instalador <ruta del setup.exe generado>
```

Con interfaz hay que completar el asistente (el script espera hasta 120 min). Si nadie va a estar delante, añada `-Silencioso` (instalación `/S`, T08-08).

- [ ] El archivo `t08-instalar-*.txt` del escritorio contiene «Antes», «Instalación» con código de salida, «Después» y la línea de IA local («no comprobada», o «Comprobación de la IA local» con `-ComprobarIALocal` si se preparó antes). El instalador no debe haber descargado Ollama ni el modelo (T08e-01).
- [ ] T08-10: el programa queda en `%LOCALAPPDATA%\Programs\NestAdmin` y los datos en `%LOCALAPPDATA%\NestAdmin`.
- [ ] T08-11: instalando encima de la versión anterior, los datos siguen y no quedan `nestadmin.exe` ni DLL en la carpeta de datos (el script avisa si quedan).
- [ ] T08-13: con `-ClienteOAuth`, «Correo» ofrece Gmail y Microsoft.
- [ ] T08-12: `t08-registro.ps1 -Accion Desinstalar` quita el programa y conserva los datos.
- [ ] En una máquina virtual limpia (T08-01 a T08-09 y T08e): sin Ollama, sin Internet salvo WebView2, `/S`. La preparación local, con su corte de red durante la descarga del modelo, se prueba aparte desde «Instalar IA local» (T08b, T08e-04).

## 2. Ventana y área de notificación (R01) — `evidence/t01-escritorio.md`

- [ ] Importar un documento y cerrar la ventana mientras se procesa: la ventana se oculta y la tarea termina (se ve al restaurar).
- [ ] Restaurar desde el icono del área de notificación.
- [ ] «Salir de NestAdmin por completo» desde el icono: en el Administrador de tareas no queda `nestadmin.exe`.
- [ ] Abrir el icono dos veces: solo hay una instancia.

## 3. Interrupción y recuperación (R02) — `evidence/t02-nucleo.md`

- [ ] Importar varios documentos y, con uno en proceso, cerrar NestAdmin por completo. Al abrirlo, el documento se procesa una sola vez y ninguna tarea queda «en proceso» para siempre.
- [ ] Aprobar una respuesta del correo simulado y cortar el proceso (Administrador de tareas › Finalizar tarea) justo después. Al reabrir y reintentar, el correo simulado tiene un solo envío.

## 4. Revisión, reglas e historial (R07) — `evidence/t07-revision-reglas.md`

- [ ] Importar `factura-girada-exif.jpg` y `dos-facturas.jpg` (de `crates\extract\tests\fixtures`). La foto girada se lee bien; la de dos facturas da dos revisiones con «Factura 1 de 2» y «2 de 2».
- [ ] Editar un campo, aprobar y comprobar el registro en el historial.
- [ ] Crear una regla para un proveedor, activarla: una factura limpia de ese proveedor se registra sola y lo dice el historial; una con incidencias o de otro proveedor pasa a revisión; una factura separada de una imagen con varias pasa siempre a revisión.
- [ ] Desactivar la regla: vuelve a pedir revisión.
- [ ] Consultar el correo simulado: M-0001 trae su factura adjunta, que aparece como documento con origen en ese correo.
- [ ] **Su foto de varias facturas** (`C:\Users\HumanaIA\Desktop\20261105_091431.jpg`): importarla y comprobar que hay una revisión por factura con los datos correctos. Anotar el resultado sin copiar los datos: es un documento real y no se sube al repositorio.

## 5. Asistente al abrir (T08c) — `evidence/t08-instalador.md`

- [ ] Con Ollama detenido (`taskkill /IM ollama.exe /F` y el icono de Ollama cerrado), abrir NestAdmin: arranca Ollama sin preguntar (T08c-01).
- [ ] Con el modelo quitado (`ollama rm qwen3:8b`), abrir NestAdmin: muestra el asistente y no descarga nada en segundo plano (T08c-02).
- [ ] Con Ollama cloud (u otro externo) elegido, abrir NestAdmin: no arranca nada local y solo se comprueba el OCR del paquete (T08c-03, D103).

## 6. Credenciales con la aplicación instalada (T09) — `evidence/t09-credenciales.md`

- [ ] Guardar una clave de Ollama cloud en Configuración y conectar una cuenta de correo de prueba. Buscar el valor de la clave en `%LOCALAPPDATA%\NestAdmin` (`findstr /S /M "<primeros caracteres>" *`): no debe aparecer en ningún archivo. Solo debe estar en el Administrador de credenciales (`NestAdmin`, `ollama_cloud_api_key` y `mail/<cuenta>`). Si una instalación anterior dejó una clave `groq_api_key`, la aplicación no la lee ni la borra: se puede quitar a mano desde el Administrador de credenciales. Los secretos de más de 1000 caracteres (por ejemplo, los tokens de Microsoft) se reparten en entradas `<nombre>#1..N` del Administrador de credenciales (D121); es lo esperado y tampoco deben aparecer en ningún archivo.

## 7. Medición en el equipo objetivo (T12) — `evidence/t10-ollama-qwen3.md` y `evidence/t10-ollama-cloud.md`

Con el equipo libre (otra IA o aplicaciones pesadas falsean memoria y tiempo) y el modelo local descargado de la memoria. Groq se retiró (D100). Ollama cloud se mide desde `main` (los proveedores externos ya están integrados), como explica `evidence/t10-ollama-cloud.md`, con la clave en el Administrador de credenciales (servicio `NestAdmin`, nombre `ollama_cloud_api_key`).

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\t12-medir.ps1 -Proveedor ollama
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\t12-medir.ps1 -Proveedor ollama-cloud -Modelo gemma4:31b
```

- [ ] Copiar a la evidencia las cifras de `evaluation\results\t12-*.txt` (memoria, duración y métricas), sin las predicciones.

## 8. Tesseract sin DLL GPL (D69) — `specs/v1/redistribucion-tesseract.md`

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\build-tesseract.ps1
$env:NESTADMIN_TESSERACT = "$PWD\vendor\tesseract-min\tesseract.exe"
cargo test -p nestadmin-extract -- --include-ignored
```

Si un antivirus (Bitdefender en el equipo de desarrollo) bloquea el ejecutable recién compilado con «Acceso denegado», hace falta una exclusión para `vendor\tesseract-min\`, que debe añadir quien administre el antivirus.

- [ ] El script termina sin «depende de DLL que no son del sistema» y las pruebas pasan. Después se cambia el paquete según los pasos del plan.
