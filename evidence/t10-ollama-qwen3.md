# Evidencia T10 (parcial): corpus sintético con Ollama y Qwen3 8B

Primera ejecución del corpus sintético v1 (10 facturas y 20 correos) con el proveedor local. Las predicciones y el informe completo se guardan fuera de Git (`evaluation/predictions/`, `evaluation/results/`); aquí solo constan las métricas agregadas y la revisión manual. Son mediciones observadas sobre un corpus pequeño y artificial, no una precisión garantizada.

## Cómo reproducir

```sh
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/setup-ai.ps1
cargo run --release -p nestadmin-ai --bin nestadmin-predict -- --provider ollama --out evaluation/predictions/ollama-qwen3-8b.jsonl
python evaluation/evaluate.py evaluation/predictions/ollama-qwen3-8b.jsonl --output ollama-qwen3-8b.json
```

## Condiciones

- Fecha: 2026-11-03
- Equipo: Windows 11 Pro 10.0.26300, Intel Core Ultra 5 125U, 16 GB RAM (equivalente al objetivo de R12), sin GPU dedicada
- Ollama 0.40.14, modelo `qwen3:8b`, digest `500a1f067a9f782620b40bee6f7b0c89e17ae61f686b92c24933e4ca4b2b8b41` (4,87 GB)
- Temperatura 0, salida con esquema JSON, `think: false`, concurrencia 1, límite de 300 s por caso
- Sin calentamiento previo: el primer caso incluye la carga del modelo
- Las etiquetas de incidencia las calcula `crates/documents` a partir de los campos extraídos (D14)

## Resultados

| Métrica | Valor |
| --- | --- |
| Casos recibidos / errores | 30 / 0 |
| Campos de factura correctos | 60 de 60 |
| Campos ausentes inventados | 0 de 2 |
| Incidencias (VP / FP / FN) | 4 / 0 / 0 |
| Clasificación de correos | 20 de 20 |
| Heurística de borradores | 13 de 20 |
| Latencia media por caso | 16,5 s (30 muestras) |
| Duración total | 8 min 14 s |
| Memoria | no medida |

## Revisión manual de los borradores

Los 7 borradores que no pasan la heurística (E09–E12, E17, E18, E20) responden con sentido, pero no contienen la expresión literal que busca la rúbrica (`revis`, `consulta`, `mensaje`). Ninguno de los 20 afirma que se haya enviado una factura, confirmado un pago o completado un registro.

Problemas observados que la heurística no detecta:

- **E17:** el borrador se compromete a una reunión el lunes en nombre del usuario.
- **E20:** remite a una «página web» que no consta en el mensaje.
- **E09, E11, E12:** empiezan con «Ha detectado…», una construcción poco natural.

Las instrucciones no se han ajustado a las palabras de la rúbrica, para no sesgar la medición.

## Limitaciones y pendiente

- Corpus pequeño y artificial: textos limpios, sin PDF, escaneos ni variedad real de formatos. No permite extrapolar la calidad a documentos reales.
- Falta la misma ejecución con Groq para comparar (requiere la clave del usuario) y la medición de memoria, incluida la de los procesos de Ollama; ambas forman parte de T12.
- La latencia la mide el propio cliente por petición; no incluye el procesamiento del documento.

## 2026-11-07 — T12 en el equipo objetivo: tiempo y memoria

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\t12-medir.ps1 -Proveedor ollama
```

- Commit `e5adfe1` (rama `pruebas-windows-2026-11-06`). Windows 11 Pro 26H2 (26300.9818), Intel Core Ultra 5 125U, 15,52 GB visibles, sin GPU dedicada. Ollama 0.40.14, `qwen3:8b` (`500a1f067a9f`).
- Modelo descargado de la memoria antes de empezar (`keep_alive: 0`): la duración incluye la carga.
- Condiciones del equipo: 3,12 GB libres al empezar; seguían abiertos WSL, Edge, Bitdefender y varias sesiones de Claude Code. Otra IA había estado trabajando en el equipo justo antes y se esperó a que terminara.

| Métrica | Valor |
| --- | --- |
| Casos recibidos / errores | 30 / 0 |
| Campos de factura correctos | 60 de 60 |
| Campos ausentes inventados | 0 de 2 |
| Incidencias (VP / FP / FN) | 4 / 0 / 0 |
| Clasificación de correos | 20 de 20 |
| Heurística de borradores | 13 de 20 |
| Latencia media por caso | 16,9 s (30 muestras) |
| Duración total | 509 s (8 min 29 s) |
| Pico de Ollama, conjunto de trabajo (`ollama*` y `llama-server*`) | 5,22 GB |
| Pico de Ollama, memoria privada | 5,87 GB |
| Pico del cliente `nestadmin-predict` | 0,01 GB |

Método: conjunto de trabajo y memoria privada muestreados cada 500 ms; no incluye memoria compartida de la GPU integrada. Una sola ejecución.

Hallazgos del script, corregidos en la rama antes de esta medición:

- Desde Ollama 0.40 el modelo se carga en `llama-server.exe`, hijo de `ollama.exe`. El filtro `ollama*` lo dejaba fuera: una primera pasada dio un pico de 0,04 GB con el modelo cargado (`llama-server` tenía 3,18 GB de conjunto de trabajo y 5,74 GB privados). Esa pasada no se usa como medida de memoria; sus métricas de calidad coinciden con las de arriba (16,1 s de media, 485 s).
- Los picos se guardaban como `Int32` y `[Math]::Max` fallaba al pasar de 2 GB.
- El archivo estaba en UTF-8 sin BOM y Windows PowerShell 5.1 no lo analizaba.

Lectura: con 16 GB, el modelo local ocupa unos 5,2–5,9 GB y deja poco margen con otras aplicaciones abiertas (3,1 GB libres antes de cargarlo). Cada caso tarda unos 17 s. Comparación con el proveedor en la nube en [t10-ollama-cloud.md](t10-ollama-cloud.md).
