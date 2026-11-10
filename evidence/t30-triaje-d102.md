# T30 — Triaje e inyección con el texto completo del correo (D102)

2026-11-10, Windows 11, Ollama cloud con `gemma4:31b`, concurrencia 1, sin perfil. Código en `f07bf89`: el texto del correo lo arma `email_text` con las líneas `De`, `Para`, `Fecha` y `Asunto`, y el corpus trae `to` y `date` sintéticos. Solo corpus sintético (D164); predicciones e informes fuera de Git (`evaluation/predictions/`, `evaluation/results/`, comprobado con `git check-ignore`). Se compara con la medida de base ([t30-triaje-base.md](t30-triaje-base.md), solo `De` y `Asunto`), con sus predicciones pasadas por el evaluador actual. Los recuentos los calcula el evaluador; la comparación por caso, un script sobre los dos informes.

```sh
cargo run --release -p nestadmin-ai --bin nestadmin-predict -- --provider ollama-cloud --model gemma4:31b --set triaje --out evaluation/predictions/triaje-gemma4-d102.jsonl
cargo run --release -p nestadmin-ai --bin nestadmin-predict -- --provider ollama-cloud --model gemma4:31b --set inyeccion --out evaluation/predictions/inyeccion-gemma4-d102.jsonl
python evaluation/evaluate_triage.py evaluation/predictions/triaje-gemma4-d102.jsonl --output triaje-gemma4-d102.json
python evaluation/evaluate_injection.py evaluation/predictions/inyeccion-gemma4-d102.jsonl --output inyeccion-gemma4-d102.json
```

## Triaje (148 casos)

| Medida | Base | Con D102 |
|---|---|---|
| Capa local: ruta correcta | 148/148; `hidden_fn` 0 | 148/148; `hidden_fn` 0 |
| Respuestas recibidas, errores, degradados | 148, 0, 0 | 148, 0, 0 |
| `needs_reply` verdadero | 80/80 (cobertura 1,0) | 79/80 (cobertura 0,9875, precisión 1,0) |
| `needs_reply` falso | 68/68 | 68/68 |
| Categoría (80 que requieren respuesta) | 76/80 | 76/80 (un caso empeora y otro mejora) |
| Rúbrica literal de borrador | 68/80 | 66/80 (cuatro empeoran y dos mejoran) |
| Borradores con una dirección de correo | 0 de 80 | 0 de 79 |
| Latencia media declarada | 889 ms | 800 ms |

La base de la tabla anterior decía 75/80 en categoría; pasada por el evaluador actual da 76/80, que es la cifra comparable.

Puertas provisionales de D164: `hidden_fn` 0 y cobertura de `needs_reply` verdadero ≥ 0,95. Se cumplen.

## Inyección (41 casos)

| Medida | Base | Con D102 |
|---|---|---|
| Filtración de instrucciones, categorías o marcadores | 0 | 0 |
| Contactos fuera del perfil | 0 | 0 |
| Cadenas prohibidas | 0 | 0 |
| `needs_reply` distinto del esperado | 5 | 4 |

Puerta de cero envíos no autorizados: se cumple.

## Lectura

- No hay regresión sistemática. Los cambios van en los dos sentidos y afectan a pocos casos, y cada medición es una sola ejecución.
- El riesgo que señaló la revisión no aparece en este corpus: con las direcciones visibles en `De` y `Para`, ningún borrador las copia, así que `draft_flags` no marcaría más borradores por ese motivo.

## Lo que no permite afirmar

- No separa el efecto del formato nuevo de la variación entre ejecuciones: hay una sola ejecución por lado.
- Es un corpus sintético con el mismo autor para casos y etiquetas. No acredita la calidad con correo real (E7 pendiente).
- La comprobación de direcciones en los borradores es un patrón literal sobre el texto, no la función `draft_flags` de la aplicación.
- Mide el modelo, no la aplicación. Las comprobaciones de inyección son literales.
