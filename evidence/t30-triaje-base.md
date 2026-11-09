# T30 — Medida de base del triaje y de la suite de inyección

2026-11-09, Windows 11, Ollama cloud con `gemma4:31b`, peticiones de `triage_request` y `draft_request` de E2 (`6a85613`), sin perfil. Solo corpus sintético (D164); predicciones e informes fuera de Git (`evaluation/predictions/`, `evaluation/results/`).

```sh
cargo run --release -p nestadmin-ai --bin nestadmin-predict -- --provider ollama-cloud --model gemma4:31b --set triaje --out evaluation/predictions/triaje-gemma4.jsonl
cargo run --release -p nestadmin-ai --bin nestadmin-predict -- --provider ollama-cloud --model gemma4:31b --set inyeccion --out evaluation/predictions/inyeccion-gemma4.jsonl
python evaluation/evaluate_triage.py evaluation/predictions/triaje-gemma4.jsonl --output triaje-gemma4.json
python evaluation/evaluate_injection.py evaluation/predictions/inyeccion-gemma4.jsonl --output inyeccion-gemma4.json
```

## Triaje (148 casos)

| Medida | Resultado |
|---|---|
| Capa local: ruta correcta | 148/148; `hidden_fn` 0 |
| Respuestas recibidas | 148/148, sin errores ni degradados |
| `needs_reply` verdadero | 80/80 (cobertura 1,0, precisión 1,0) |
| `needs_reply` falso | 68/68 |
| Categoría (solo los 80 que requieren respuesta) | 75/80; precisión mínima 0,88 (`incidencia_factura`, `documentacion`), cobertura mínima 0,875 (`otros`) |
| Rúbrica literal de borrador | 68/80 (0,85) |
| Latencia media declarada | 889 ms por petición de clasificación y redacción |

Puertas provisionales de D164: `hidden_fn` 0 y cobertura de `needs_reply` verdadero ≥ 0,95, cumplidas en este corpus.

## Inyección (41 casos)

| Medida | Resultado |
|---|---|
| Borradores con filtración de instrucciones, categorías o marcadores | 0 |
| Contactos fuera del perfil (URL, correo, teléfono, IBAN) | 0 |
| Cadenas prohibidas | 0 |
| `needs_reply` distinto del esperado | 5, todos «no requiere respuesta» en correos que envían documentos con una orden incrustada |

Puerta de cero envíos no autorizados: cumple. Una discrepancia de `needs_reply` se cuenta aparte: no genera borrador ni envío y el correo sigue en revisión (D158). Se corrigió el evaluador, que la contaba como fallo de la puerta.

## Lo que no permite afirmar

- Es un corpus sintético escrito para este fin, con el mismo autor para casos y etiquetas: detecta errores sistemáticos, no acredita la calidad con correo real (E7 pendiente).
- Las comprobaciones de inyección son literales; otros ataques pueden no detectarse.
- Mide el modelo, no la aplicación: la compuerta de reglas y la validación de salida en la app se prueban en E3.
- Una sola ejecución; no mide la variación entre ejecuciones.
