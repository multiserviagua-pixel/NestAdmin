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
- La misma ejecución con Groq está en [T12](t12-groq.md). Falta la medición de memoria, incluida la de los procesos de Ollama.
- La latencia la mide el propio cliente por petición; no incluye el procesamiento del documento.
