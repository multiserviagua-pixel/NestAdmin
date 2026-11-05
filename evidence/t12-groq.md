# Evidencia T12 (parcial): Groq frente a Ollama en el corpus sintético

Primera ejecución del corpus sintético v1 (10 facturas y 20 correos) con Groq, comparada con la de Ollama y Qwen3 8B de [T10](t10-ollama-qwen3.md). Las predicciones y los informes completos se guardan fuera de Git (`evaluation/predictions/`, `evaluation/results/`); aquí solo constan las métricas agregadas y la revisión manual. Son mediciones observadas sobre un corpus pequeño y artificial, no una precisión garantizada. Ningún dato de usuario se envió a Groq: solo las entradas de `evaluation/data/inputs/`.

## Cómo reproducir

La clave se guarda antes en el administrador de credenciales de Windows (servicio `NestAdmin`, nombre `groq_api_key`), por ejemplo desde Configuración en la ventana. Nunca va en la línea de órdenes ni en el repositorio.

```sh
cargo build --release -p nestadmin-ai --bin nestadmin-predict
./target/release/nestadmin-predict --provider groq --model openai/gpt-oss-20b --pause-secs 20 --out evaluation/predictions/groq-openai-gpt-oss-20b.jsonl
./target/release/nestadmin-predict --provider groq --model openai/gpt-oss-120b --pause-secs 20 --out evaluation/predictions/groq-openai-gpt-oss-120b.jsonl
./target/release/nestadmin-predict --provider groq --model qwen/qwen3.8-27b --pause-secs 20 --out evaluation/predictions/groq-qwen-qwen3.8-27b.jsonl
python evaluation/evaluate.py evaluation/predictions/ollama-qwen3-8b.jsonl --compare evaluation/predictions/groq-qwen-qwen3.8-27b.jsonl --output comparacion.json
```

## Condiciones

- Fecha: 2026-11-05
- Cliente: Windows 11 Pro 10.0.26300, Intel Core Ultra 5 125U, 16 GB RAM, conexión doméstica
- Groq, API compatible con OpenAI, cuenta de nivel gratuito: 1 000 peticiones y **8 000 tokens por minuto** por modelo (cabeceras `x-ratelimit-*`)
- Modelos disponibles en la cuenta ese día: `openai/gpt-oss-20b`, `openai/gpt-oss-120b`, `qwen/qwen3.8-27b`. `llama-3.3-70b-versatile` y `qwen/qwen3-32b` devolvieron `model_not_found`
- Temperatura 0, `response_format: json_object` con el esquema en las instrucciones (el modo JSON de Groq no valida el esquema), concurrencia 1 por modelo, los tres modelos en paralelo, límite de 300 s por caso
- `--pause-secs 20` entre casos para no superar la cuota; la pausa no cuenta en la latencia por caso
- Las etiquetas de incidencia las calcula `crates/documents` a partir de los campos extraídos (D14)
- Referencia local: la ejecución de T10 del 2026-11-03 (Ollama 0.40.14, `qwen3:8b`, mismo equipo, sin pausa)

## Resultados

| Métrica | Ollama `qwen3:8b` | Groq `gpt-oss-20b` | Groq `gpt-oss-120b` | Groq `qwen3.8-27b` |
| --- | --- | --- | --- | --- |
| Casos recibidos / errores | 30 / 0 | 30 / 0 | 30 / 0 | 30 / 0 |
| Campos de factura correctos | 60 de 60 | 60 de 60 | 60 de 60 | 60 de 60 |
| Campos ausentes inventados | 0 de 2 | 0 de 2 | 0 de 2 | 0 de 2 |
| Incidencias (VP / FP / FN) | 4 / 0 / 0 | 4 / 0 / 0 | 4 / 0 / 0 | 4 / 0 / 0 |
| Clasificación de correos | 20 de 20 | 20 de 20 | 20 de 20 | 20 de 20 |
| Heurística de borradores | 13 de 20 | 13 de 20 | 18 de 20 | 19 de 20 |
| Latencia media por caso | 16,5 s | 1,0 s | 1,2 s | 0,7 s |
| Memoria | no medida | no observable | no observable | no observable |

La latencia de Groq la mide el cliente por petición e incluye la red; la de Ollama incluye la carga del modelo en el primer caso. La memoria del servidor remoto no es observable, y la del cliente no se midió.

## Cuota del nivel gratuito

Una primera ejecución sin pausa falló en 9 (`gpt-oss-120b`), 10 (`gpt-oss-20b`) y 16 (`qwen3.8-27b`) de los 20 correos: Groq respondió HTTP 429 al superar los 8 000 tokens por minuto, tras las facturas. Las facturas no fallaron porque se procesan primero. NestAdmin informa ese 429 como `unavailable`, sin distinguirlo de una caída del servicio, y no reintenta: en uso real con esta cuota, una ráfaga de correos dejaría tareas fallidas con un motivo poco claro. Esa ejecución se descartó; los resultados de arriba son de la ejecución con pausa.

## Revisión manual de los borradores

Ninguno afirma que se haya enviado una factura, confirmado un pago o completado un registro. Problemas que la heurística no detecta:

- **Marcadores sin rellenar:** `qwen3.8-27b` pone «[Nombre]» y «[Su Nombre]» en E13–E19, y `gpt-oss-120b` pone «[Su nombre]» en varios. Si se aprueban sin editar, se enviarían tal cual.
- **Compromisos en nombre del usuario:** los tres aceptan la reunión del lunes en E17, como ya hacía Ollama. `qwen3.8-27b` promete enviar el documento (E01, E02) y un resumen de servicios (E20), y «incorporar al registro» la documentación (E13).
- **Contenido inexistente:** `gpt-oss-120b` en E20 dice «Adjuntamos una breve descripción…» sin adjunto.
- **Registro:** `gpt-oss-20b` tutea en E17 («¿A qué hora te viene bien?») y usa variantes no peninsulares («monto», «a la brevedad»). E01 empieza con «Claro, le enviaré…».

Una mejor puntuación en la heurística no implica borradores más seguros: `qwen3.8-27b`, el que más pasa, es también el que más marcadores deja.

## Cambio en la herramienta

`nestadmin-predict` admite `--pause-secs N` (por defecto 0) para esperar entre casos sin contarlo en la latencia, y lo registra como `pause_secs` en el `.meta.json`. No cambia el producto.

## Limitaciones y pendiente

- Corpus pequeño y artificial; no permite extrapolar a documentos reales.
- Groq se usó desde la herramienta de evaluación, no desde la ventana de NestAdmin.
- Falta medir la memoria en el equipo objetivo, incluidos los procesos de Ollama.
- Los modelos de Groq cambian sin aviso: los de esta ejecución pueden no existir más adelante.
- Pendiente de decisión: cómo tratar el 429 en el producto (mensaje propio con el límite de uso, espera según `retry-after` o ambas).
