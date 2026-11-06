# Evidencia T10 (parcial): corpus sintético con Groq

Misma colección sintética v1 que [t10-ollama-qwen3.md](t10-ollama-qwen3.md) (10 facturas y 20 correos), ahora con el proveedor externo elegido explícitamente (`--provider groq --model …`). Las predicciones y el informe completo están fuera de Git (`evaluation/predictions/`, `evaluation/results/`). Son mediciones observadas sobre un corpus pequeño y artificial, no una precisión garantizada.

## Cómo reproducir

En Windows, con la clave guardada en el Administrador de credenciales (servicio `NestAdmin`, nombre `groq_api_key`):

```sh
cargo run --release -p nestadmin-ai --bin nestadmin-predict -- --provider groq --model qwen/qwen3.8-27b --timeout-secs 120 --out evaluation/predictions/groq-qwen3.8-27b.jsonl
python evaluation/evaluate.py evaluation/predictions/groq-qwen3.8-27b.jsonl --compare evaluation/predictions/ollama-qwen3-8b.jsonl --output comparacion.json
```

Esta ejecución se hizo en un contenedor Linux cuya salida a Internet pasa por un proxy obligatorio que inyecta la clave. El adaptador no usa proxy (D12), así que se lanzó con `NESTADMIN_GROQ_API_KEY` (marcador; solo la herramienta de evaluación y solo fuera de Windows) y `--groq-endpoint http://127.0.0.1:8787`, un reenvío local hacia `https://api.groq.com/openai/v1`. La opción solo admite destinos de bucle local y solo con `--provider groq`; la aplicación no la tiene.

## Condiciones

- Fecha: 2026-11-06
- Equipo: contenedor Linux en la nube, no el equipo objetivo. Las latencias incluyen el reenvío y el proxy, y **no son comparables** con las de Ollama en el portátil.
- Modelo `qwen/qwen3.8-27b` (el que informa la API), temperatura 0, modo JSON (`response_format: json_object`) con el esquema en las instrucciones, concurrencia 1, plazo de 120 s por caso
- Plan gratuito de Groq: 1000 tokens de salida por minuto. La mayoría de las esperas son reintentos por 429.
- Memoria: no observable para un proveedor remoto y no se presenta como medida.

## Resultados

| Métrica | Groq `qwen/qwen3.8-27b` | Ollama `qwen3:8b` (T10, 2026-11-03) |
| --- | --- | --- |
| Casos recibidos / errores | 30 / 0 | 30 / 0 |
| Campos de factura correctos | 60 de 60 | 60 de 60 |
| Campos ausentes inventados | 0 de 2 | 0 de 2 |
| Incidencias (VP / FP / FN) | 4 / 0 / 0 | 4 / 0 / 0 |
| Clasificación de correos | 20 de 20 | 20 de 20 |
| Heurística de borradores | 19 de 20 | 13 de 20 |
| Latencia media por caso | 4,9 s, con esperas por límite de peticiones | 16,5 s |
| Duración total | 2 min 26 s | 8 min 14 s |

## Hallazgo y corrección: límite de peticiones

La primera ejecución dio 16 errores `unavailable`. Groq respondía HTTP 429 con `Retry-After` por superar los tokens por minuto, y el adaptador lo trataba como proveedor caído. Con el plan gratuito, a un usuario le habría pasado lo mismo. Se corrigió en el producto:

- Nuevo error del contrato `rate_limited` (`AiError::RateLimited`), recuperable.
- `GroqProvider` reintenta con el **mismo** proveedor esperando lo que pide `Retry-After` (2 s si no lo da, 30 s como máximo y hasta 10 veces). Si la espera no cabe en el plazo, devuelve `rate_limited` sin esperar. Nunca cambia de proveedor.
- Pruebas `rate_limit_waits_what_the_provider_asks_and_retries_the_same_provider` y `rate_limit_beyond_the_deadline_is_reported_without_waiting` en `crates/ai/tests/providers.rs`, con servidor simulado.

Con la corrección se registraron 32 respuestas 429 recuperadas, sin ningún error.

## Revisión manual de los borradores

- **E13–E19:** dejan marcadores `[Nombre]` y `[Su Nombre]`. El usuario debe completarlos antes de enviar, y una regla automática no debería enviarlos así.
- **E17:** «Quedo a la espera de su disponibilidad para agendar la conversación … el próximo lunes». No falla la heurística por compromiso, sino por la expresión literal. Es más prudente que el de Ollama, que fijaba la reunión.
- **E20:** anuncia que «estamos preparando un resumen general de nuestras prestaciones», un compromiso que no consta en el mensaje.
- **E09, E10:** mencionan emitir la factura rectificativa «en su caso». Es condicional, sin afirmar que ya se haya hecho.
- Ninguno afirma que se haya enviado una factura, confirmado un pago o completado un registro.

## Limitaciones y pendiente

- Falta repetir esta ejecución en el equipo objetivo (Core Ultra 5, 16 GB) con la clave en el Administrador de credenciales, para que la latencia sea comparable (T12).
- Falta la memoria del proceso local en ambas ejecuciones (T12).
- El corpus es pequeño y limpio, y no permite extrapolar a documentos reales.
- Los modelos son distintos (27B remoto frente a 8B local). La comparación describe las dos opciones disponibles, no el mismo modelo.
