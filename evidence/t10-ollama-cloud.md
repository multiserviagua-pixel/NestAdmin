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
