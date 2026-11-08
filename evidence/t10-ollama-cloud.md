# Evidencia T12 (parcial): corpus sintético con Ollama cloud

Misma colección sintética v1 que [t10-ollama-qwen3.md](t10-ollama-qwen3.md) (10 facturas y 20 correos), con el proveedor externo Ollama cloud (`gemma4:31b`), elegido explícitamente. Sustituye a Groq como proveedor externo que se compara, porque Groq ha quedado fuera del diseño (decisión del usuario del 2026-11-07, registrada en el PR del cambio de alcance). Las predicciones y el informe completo están fuera de Git. Son mediciones observadas sobre un corpus pequeño y artificial.

## Cómo reproducir

El adaptador de Ollama cloud no está en `main`: está en la rama `claude/fase-11b-modelo-venta` (commit `f321bd9`). Se midió desde una copia de trabajo de esa rama, con una copia de `scripts/t12-medir.ps1` de esta rama que solo añade `ollama-cloud` a los proveedores admitidos. La clave está en el Administrador de credenciales (servicio `NestAdmin`, nombre `ollama_cloud_api_key`) y no consta en ningún archivo.

```powershell
git worktree add ..\nestadmin-fase11b claude/fase-11b-modelo-venta
# copiar scripts\t12-medir.ps1 como ..\nestadmin-fase11b\t12-medir-cloud.ps1, con ValidateSet ampliado a 'ollama-cloud' y $Root = $PSScriptRoot
powershell -NoProfile -ExecutionPolicy Bypass -File ..\nestadmin-fase11b\t12-medir-cloud.ps1 -Proveedor ollama-cloud -Modelo gemma4:31b
```

## Condiciones

- Fecha: 2026-11-07. Mismo equipo objetivo que la medición local: Windows 11 Pro 26H2 (26300.9818), Intel Core Ultra 5 125U, 15,52 GB visibles; 5,12 GB libres al empezar.
- Modelo `gemma4:31b` en `https://ollama.com/api/chat`, temperatura 0, `think: false`, `format` con el esquema JSON, concurrencia 1, plazo de 300 s por caso.
- Memoria: la del servidor remoto no es observable. En el equipo, el cliente usó 0,01 GB y los procesos de Ollama locales, ajenos a la prueba, 0,04 GB de conjunto de trabajo (0,13 GB privados), sin modelo local cargado.

## Resultados

| Métrica | Ollama cloud `gemma4:31b` | Ollama local `qwen3:8b` (mismo día y equipo) |
| --- | --- | --- |
| Casos recibidos / errores | 30 / **20** | 30 / 0 |
| Campos de factura correctos | 60 de 60 | 60 de 60 |
| Campos ausentes inventados | 0 de 2 | 0 de 2 |
| Incidencias (VP / FP / FN) | 4 / 0 / 0 | 4 / 0 / 0 |
| Clasificación de correos | **0 de 20** (los 20 con error) | 20 de 20 |
| Heurística de borradores | 0 de 20 | 13 de 20 |
| Latencia media por caso | 0,8 s | 16,9 s |
| Duración total | 25 s | 509 s |
| Memoria en el equipo | 0,01 GB (cliente) | 5,22 GB (5,87 GB privada) |

## Hallazgo: la API de ollama.com no respeta `format`

Los 20 correos terminan en `invalid_response` en unos 0,8 s. Se reprodujo a mano contra `https://ollama.com/api/chat` con un correo sintético (`E01`):

- Con `format` igual al esquema (con o sin `enum`) o con `format: "json"`, `gemma4:31b` responde en Markdown libre («Aquí tienes la clasificación…», con encabezados y dos borradores alternativos), no en JSON.
- Solo devuelve JSON si las instrucciones lo piden expresamente, y aun así lo envuelve en un bloque ```` ```json ```` e inventa la categoría («Solicitud de Factura» en lugar de `solicitud_factura`).
- Las facturas sí funcionan porque sus instrucciones ya piden la salida en JSON.

Consecuencia: con este proveedor no se puede dar por hecho que la salida respeta el esquema. Hay que pedir el JSON y las categorías exactas en las instrucciones, validar siempre la respuesta y tratar el fallo como error explícito. La validación actual ya lo hace: ningún correo se clasificó mal en silencio. Se ha comunicado a la sesión del cambio de alcance (IA externa por defecto). Esta medición no se ha repetido con instrucciones corregidas: el código del adaptador está en otra rama.

## Pendiente

- Corregir las instrucciones del proveedor en la nube y repetir T12 para los correos.
- La latencia en la nube depende de la red y de la carga del servicio. Es una sola ejecución.

## 2026-11-08: repetición con las instrucciones corregidas (D105)

Mismo equipo objetivo y mismo corpus sintético (`synthetic-v1`), `scripts/t12-medir.ps1 -Proveedor ollama-cloud -Modelo gemma4:31b`, código de `59c936e`. Una sola ejecución; solo salieron datos sintéticos.

| Métrica | Resultado |
| --- | --- |
| Casos recibidos / errores | 30 / 0 |
| Campos de factura correctos | 60 de 60 |
| Campos ausentes inventados | 0 de 2 |
| Incidencias (VP / FP / FN) | 4 / 0 / 0 |
| Clasificación de correos | 20 de 20 |
| Heurística de borradores | 17 de 20 |
| Latencia media por caso | 0,75 s |
| Duración total | 23 s |
| Memoria en el equipo | 0,01 GB (cliente) |

Las instrucciones explícitas resuelven el fallo de los 20 correos. El corpus es sintético y pequeño: no mide la calidad con correo real, donde ya se observó que el modelo local propone responder a remitentes automáticos (T13). La latencia depende de la red y de la carga del servicio. Las predicciones y el informe quedan fuera de Git.

## 2026-11-08: panel real con Ollama cloud (versión de desarrollo)

Carpeta de datos nueva y temporal, solo con el correo simulado (borrada al terminar), con la clave de Ollama cloud ya guardada en el Administrador de credenciales. Código de `f5d395d`.

- Sin `configuracion.json` se abre el asistente de primer uso.
- Bloque de Ollama cloud: la lista responde con la clave guardada y `gemma4:31b` queda elegido. No se pudo leer la lista completa con la automatización de la ventana.
- Tras confirmar el destino, la prueba sintética responde («Proveedor listo»). `configuracion.json` guarda la elección y la confirmación, sin la clave.
- Consulta del correo simulado: 7 tareas en revisión, las 7 con `ollama_cloud` y `gemma4:31b`, sin errores y sin respaldo.

No se ha probado con proveedor real: clave inválida, respaldo local, compatible con OpenAI ni Claude. El rechazo de clave solo se comprobó con pruebas y con la vista simulada.

Instalador de cierre construido con `scripts/build-clean.ps1` desde `f5d395d` (todas sus comprobaciones correctas; SHA-256 `7405C75A7AF935EDD124004557B5A7FE365DC6DC2FE86CFBE093CE70842F9715`). No se ha instalado: T08e pide un Windows limpio sin clave guardada, y este equipo tiene Ollama y claves. No incluye el rediseño visual integrado después en `main` (`5c17a02`).
