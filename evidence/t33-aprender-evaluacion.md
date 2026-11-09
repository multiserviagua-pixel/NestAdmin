# T33h (E8) — Evaluación sintética de «aprender a responder», capa local

Solo recuentos. El corpus es sintético (`evaluation/data/{inputs,expected}/learn.json`, dominios `.test` y `.example`); el informe completo está en `evaluation/results/aprender-local.json` (ignorado por Git). Capa local, sin IA ni red: `cargo run -p nestadmin-app --bin nestadmin-learn-local -- --out evaluation/predictions/aprender-local.jsonl` y `python evaluation/evaluate_learn.py evaluation/predictions/aprender-local.jsonl`. Windows, sin modelo, sin `tauri dev`, sin instalador.

## Recuentos

| Concepto | Valor |
|---|---|
| Casos | 48 (31 deben pasar, 17 deben descartarse) |
| Pasan / descartados | 31 / 17 |
| Descartes esperados acertados (con su código) | 17 de 17 |
| Falsos descartes (no bloquean) | 0 de 31 |
| Casos con fuga | 0 de 31 que pasan |
| Puerta fugas = 0 | cumple |
| Puerta descartes esperados | cumple |
| Capa de IA (categoría, `needs_reply`, `reply_kind`) | no medido (sin modelo) |

Recuentos de la recogida (`LearnCounts`): 48 examinados, 4 automáticos, 1 sin señales comprobadas, 1 sin remitente, 1 sin hilo, 41 candidatos, 1 sin respuesta propia, 5 con cita dudosa, 2 dirigidas a otro destinatario, 1 con firma residual, 1 vacío, 0 sin enmascarar del todo y 31 pares.

## Cobertura del corpus

Citas en es, en, ca, fr, it, de y pt (7); Outlook aplanado (4); citas dudosas (4); reenvío (1) y reenvío interno (2); automáticos (4), señal débil (1), sin señales, sin hilo, sin remitente y sin respuesta (1 cada uno); datos sensibles (5: IBAN, DNI/NIE/CIF, teléfonos, URL y correos, tarjeta y clave); nombre del remitente en el saludo (2); firmas sueltas (4); inyección en la cita (2) y en el cuerpo (1); respuestas normales (7, con firma compartida; una repite el asunto dentro de una frase).

## Hallazgos

Primera pasada (47 casos): 27 pasan, 20 descartados, 17 de 17 descartes esperados, 3 falsos descartes `unmasked` y 1 fuga (`dni`), así que la puerta de fugas no se cumplía. Corregido en una segunda pasada (48 casos, el nuevo es L048):

- **Fuga (1 caso, corregida):** un CIF con guion, punto o espacio entre la letra y las ocho cifras (`B-12345678`) se enmascara ya como `[ID]` y `is_masked` lo detecta.
- **Falsos descartes por `unmasked` (3 de 30, corregidos):** el comprobador final tomaba por IBAN compacto una palabra que acaba en dos letras seguida de cifras. Ahora el IBAN compacto exige un país de la lista de IBAN, las dos cifras de control detrás, la longitud exacta del país y el módulo 97; `mask` exige también el país de la lista. Los alias `R\d+` no lo disparan. La función es compartida con el perfil (`has_compact_iban`) y sus pruebas siguen en verde.
- **Falso descarte por repetir el asunto (L048, corregido):** una respuesta normal que repetía el asunto dentro de una frase se descartaba como `quote_dropped`. El asunto solo cuenta como cita si va en una línea propia o tras una etiqueta de asunto («Asunto:», «Subject:», «Objet:»…); la protección por líneas largas del cuerpo recibido no cambia.

## Límites

- 48 casos sintéticos no acreditan la calidad con correo real; detectan errores sistemáticos de la capa local.
- No mide la IA (categoría y `reply_kind`): sin modelo disponible, «no medido». El corpus ya lleva las etiquetas esperadas y el evaluador acepta predicciones con `--ai`.
- Las puertas son internas y provisionales (D164); no se han ajustado para que pasen.
- La búsqueda de cadenas prohibidas comprueba lo que saldría por caso (cifras sin separadores; nombres por palabra entera; el resto, texto sin distinguir mayúsculas). No detecta variantes que no estén listadas en el corpus.
