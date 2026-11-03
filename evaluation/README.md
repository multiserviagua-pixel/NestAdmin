# Evaluación sintética inicial

Este harness **no implementa ni ejecuta el producto, los modelos, OCR ni servicios**. Compara salidas ya obtenidas frente a 10 facturas y 20 correos artificiales. No hay resultados medidos de proveedores en el repositorio. Para comparar Ollama y Groq hay que producir dos archivos de predicciones con el mismo corpus y la misma configuración; la selección y el envío a Groq requieren una decisión explícita. El evaluador no tiene código de red.

## Corpus y preparación

- `data/inputs/`: únicamente los textos e ID que se deben entregar al sistema evaluado.
- `data/expected/`: etiquetas y rúbricas de referencia; **no enviar al modelo ni incluir en su prompt**.
- `results/`: informes locales. El repositorio debe ignorar su contenido. Los informes no contienen borradores, documentos ni mensajes, pero los JSONL de predicción pueden contenerlos: guárdalos fuera del repositorio y sin credenciales.

Las facturas D01–D10 están ordenadas; D09 repite D01. Para detectar duplicados es necesario procesarlas en orden con un registro inicialmente vacío y conservar ese estado durante la serie. Reiniciar el registro antes de evaluar otro proveedor. Las facturas incluyen IVA ausente, total ausente y total incoherente. Son **textos**, no archivos PDF ni imágenes: los flujos reales de PDF, escaneado y OCR necesitan sus propias pruebas de integración. Las 20 entradas de correo cubren cinco categorías, cuatro mensajes por categoría. Los casos no son representativos de todas las facturas, idiomas y correos reales.

## Contrato de predicciones

Crear un JSONL UTF-8 con un objeto por caso y una línea por objeto. Los ID deben corresponder a los archivos de entrada. Puede faltar un ID: se contabilizará como caso ausente y sus respuestas como incorrectas. Un ID desconocido o repetido hace fallar el evaluador. Cada línea debe tener `result` o un `error` no vacío, nunca ambos. `latency_ms` y `peak_memory_bytes` son opcionales, numéricos, finitos y no negativos. Se pueden incluir también en errores. Ejemplos de forma, no de resultados de un modelo:

```jsonl
{"id":"D01","result":{"fields":{"supplier":"valor","invoice_number":"valor","date":"2026-01-01","subtotal":"0.00","tax":null,"total":"0.00"},"issues":[]},"latency_ms":123,"peak_memory_bytes":456}
{"id":"E01","result":{"category":"solicitud_factura","draft":"Texto del borrador"}}
{"id":"D02","error":"timeout","latency_ms":30000}
```

Las facturas usan exactamente los campos `supplier`, `invoice_number`, `date`, `subtotal`, `tax` y `total`: cadenas o `null` explícito cuando el documento no proporciona el dato. Importes en euros, con punto decimal y dos decimales; fechas ISO `AAAA-MM-DD`. `issues` es una lista de etiquetas: `missing_tax`, `missing_total`, `total_mismatch`, `possible_duplicate`. Para correos, `category` es una de `solicitud_factura`, `consulta_pago`, `incidencia_factura`, `documentacion`, `otros`; `draft` es el borrador propuesto. No se evalúa el envío ni se considera que haya ocurrido.

Los tiempos y memorias son valores comunicados por el proceso que obtiene las predicciones; el evaluador **no los mide ni verifica**. Documentar por separado el equipo, sistema, versión del modelo, configuración, concurrencia, calentamiento y método de medición. En local, indicar si la memoria incluye Ollama y sus procesos; para la API externa, la memoria del servidor remoto no es observable y no se debe presentar como si lo fuera. Sin medidas, omitir los campos: el informe devolverá `null`, nunca cero inventado.

## Ejecución

Requiere Python 3.9 o superior y solo biblioteca estándar:

```sh
python -m unittest discover -s evaluation/tests -v
python evaluation/evaluate.py /ruta/local.jsonl --compare /ruta/externo.jsonl --output comparacion.json
```

`--compare` es opcional. La salida se escribe en `evaluation/results/`, con secciones `first` y, si se solicita, `second`, puntuadas contra el mismo corpus. `--output` admite solo un nombre `.json`, no una ruta. Guardar junto a cada ejecución, fuera de Git, los metadatos experimentales y las predicciones; no usar contenido de usuarios para esta colección sintética.

## Interpretación de métricas

- `cases`: casos totales, predicciones recibidas, ausentes y errores. Los ausentes y errores cuentan como respuestas incorrectas, no desaparecen del denominador.
- `extraction.accuracy`: número de campos correctos dividido entre 60 (seis por cada factura). Comparación exacta de cadenas tras recortar espacios y normalizar mayúsculas; para un valor desconocido se exige `null` explícito. No convierte formatos numéricos o fechas. Se consideran solo los seis campos contratados.
- `hallucinated_absent_fields`: veces que un campo cuyo valor esperado es `null` contiene una cadena no vacía; `expected_absent_fields` es el número de campos sin dato. No detecta todas las invenciones posibles: un valor incorrecto para un campo presente cuenta como error de extracción, y los campos extra se ignoran.
- `issues`: verdaderos positivos, falsos positivos y falsos negativos al comparar conjuntos de etiquetas. Precisión = VP/(VP+FP); cobertura = VP/(VP+FN). Un denominador vacío se representa como `null`. Estos resultados prueban las etiquetas devueltas, no cómo se implementaron los cálculos ni el detector de duplicados.
- `classification.accuracy`: categorías exactas correctas divididas entre 20 correos.
- `draft_heuristic.rate`: borradores que contienen al menos una expresión requerida y ninguna expresión prohibida, dividido entre 20. Es una comprobación literal sin distinción de mayúsculas, no una evaluación fiable de calidad, veracidad, cortesía o seguridad. Revisar manualmente cada borrador antes de sacar conclusiones.
- `performance`: media aritmética de tiempos y máximo de memoria declarados, junto con el número de muestras disponibles. La cobertura puede ser parcial; no comparar proveedores como equivalentes si los métodos, casos o condiciones difieren.

Las pruebas unitarias usan respuestas construidas a partir de la referencia para comprobar el evaluador; **no son una medición del modelo**. Las métricas no fijan un umbral prometido. Además, este harness no sustituye las pruebas de revisión, automatización, recuperación, aislamiento de red, instalador y mediciones en el equipo objetivo.
