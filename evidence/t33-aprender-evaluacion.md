# T33h (E8) — Evaluación sintética de «aprender a responder», capa local

Solo recuentos. El corpus es sintético (`evaluation/data/{inputs,expected}/learn.json`, dominios `.test` y `.example`); el informe completo está en `evaluation/results/aprender-local.json` (ignorado por Git). Capa local, sin IA ni red: `cargo run -p nestadmin-app --bin nestadmin-learn-local -- --out evaluation/predictions/aprender-local.jsonl` y `python evaluation/evaluate_learn.py evaluation/predictions/aprender-local.jsonl`. Windows, sin modelo, sin `tauri dev`, sin instalador.

## Recuentos

| Concepto | Valor |
|---|---|
| Casos | 47 (30 deben pasar, 17 deben descartarse) |
| Pasan / descartados | 27 / 20 |
| Descartes esperados acertados (con su código) | 17 de 17 |
| Falsos descartes (no bloquean) | 3 de 30, todos `unmasked` |
| Casos con fuga | 1 de 27 que pasan (tipo `dni`) |
| Puerta fugas = 0 | **no cumple** |
| Puerta descartes esperados | cumple |
| Capa de IA (categoría, `needs_reply`, `reply_kind`) | no medido (sin modelo) |

Recuentos de la recogida (`LearnCounts`): 47 examinados, 4 automáticos, 1 sin señales comprobadas, 1 sin remitente, 1 sin hilo, 40 candidatos, 1 sin respuesta propia, 5 con cita dudosa, 2 dirigidas a otro destinatario, 1 con firma residual, 1 vacío, 3 sin enmascarar del todo y 27 pares.

## Cobertura del corpus

Citas en es, en, ca, fr, it, de y pt (7); Outlook aplanado (4); citas dudosas (4); reenvío (1) y reenvío interno (2); automáticos (4), señal débil (1), sin señales, sin hilo, sin remitente y sin respuesta (1 cada uno); datos sensibles (5: IBAN, DNI/NIE/CIF, teléfonos, URL y correos, tarjeta y clave); nombre del remitente en el saludo (2); firmas sueltas (4); inyección en la cita (2) y en el cuerpo (1); respuestas normales (6, con firma compartida).

## Hallazgos

- **Fuga (1 caso):** un CIF escrito con guion entre la letra y las ocho cifras (`B-12345678`) llega sin enmascarar. Con otros formatos (`B12345678`, `B-1234567-4`, DNI y NIE con separadores) se enmascara. Sin corregir; la decisión es de la sesión principal.
- **Falsos descartes por `unmasked` (3 de 30):** el comprobador final trata como un IBAN compacto un texto con una palabra que acaba en dos letras seguida de cifras («a las 10:00 … nosotros», «F-3310 was received?», «a 1.250 euros más IVA»). El motivo es la heurística del IBAN compacto (D169), por diseño conservadora; el coste es aprender menos, no seguridad. Depende del contexto: el alias `R10` o superior en un saludo puede provocarlo también.

## Límites

- 47 casos sintéticos no acreditan la calidad con correo real; detectan errores sistemáticos de la capa local.
- No mide la IA (categoría y `reply_kind`): sin modelo disponible, «no medido». El corpus ya lleva las etiquetas esperadas y el evaluador acepta predicciones con `--ai`.
- Las puertas son internas y provisionales (D164); no se han ajustado para que pasen.
- La búsqueda de cadenas prohibidas comprueba lo que saldría por caso (cifras sin separadores; nombres por palabra entera; el resto, texto sin distinguir mayúsculas). No detecta variantes que no estén listadas en el corpus.
