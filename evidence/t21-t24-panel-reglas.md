# Evidencia T07b, T07c, T21–T24c (parcial): panel de reglas, sugerencias y primera lectura

Cubre R07, R21, R22, R23 y R24, y las decisiones D123–D134. Todos los datos son sintéticos (`ui/src/mock.ts`, proveedor simulado, correo simulado, dominio `.test`). Nada de esto prueba la aplicación instalada, un proveedor externo real ni un buzón real.

## Cómo reproducir

```sh
python scripts/check.py
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --no-fail-fast
cargo clippy -p nestadmin-app -p nestadmin-desktop --all-targets --features lectura-bajo-demanda -- -D warnings
cargo test -p nestadmin-app -p nestadmin-desktop --features lectura-bajo-demanda --no-fail-fast
cd ui && npm ci && npm run build && npm test
python -m unittest discover -s evaluation/tests
python -m unittest discover -s scripts/tests
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/build-clean.ps1 -Prueba
```

La vista previa (`npx vite` en `ui/`) admite `?refine=auto`, `auto-consent`, `blocked`, `failed`, `invalid` y `broken`, y `?read=reading`, `interrupted`, `listing_failed`, `stopped`, `held`, `waiting` y `pending`.

## Resultados (2026-11-09, commit 0f3fe24, Windows 11, equipo de desarrollo)

| Comprobación | Resultado |
|---|---|
| `check.py`, formato y clippy, con y sin la feature | Pasan, sin avisos |
| `cargo test --workspace` | 765 pasan, 0 fallan, 15 ignoradas (requieren Ollama, PDFium, Tesseract, XSD o el almacén de Windows; no ejecutadas) |
| Pruebas de app y escritorio con `lectura-bajo-demanda` | 289 pasan, 0 fallan, 3 ignoradas |
| `npm run build` y `npm test` | Pasa; 146 pasan, 0 fallan |
| `unittest` de `evaluation/tests` y `scripts/tests` | 14 y 24, pasan |
| `build-clean.ps1 -Prueba`, desde un clon limpio | Código 0; `NestAdmin_0.1.0-prueba_x64-setup.exe`, 49 213 355 bytes, sin cliente OAuth. Tauri y NSIS aceptan el sufijo |

## Vista previa con datos sintéticos, por script

| Comprobación | Resultado |
|---|---|
| Hallazgos con su qué, por qué y consecuencia, en cada tipo | Revisado |
| «Unificar» con doble clic rápido crea una sola regla | Pasa (15 → 14 reglas, 3 archivadas) |
| Pestaña Sugeridas: frase, porqué en cifras, rango, aviso de límite alto, ocultas | Revisado |
| Aviso previo de la primera lectura y estados `reading`, `held`, `stopped`, `listing_failed` e `interrupted` | Revisado |
| Sin desbordamiento horizontal a 1024 px en esas vistas | Pasa |

## Revisión de diseño

Cada etapa (E3, E4a, E4b, E5) pasó por revisión de `arquitecto`, por lectura de código, con correcciones hasta quedar sin bloqueantes. La última revisión de E5 dejó dos bloqueantes, corregidos y cubiertos por pruebas, sin una revisión posterior de esa corrección.

## Sin verificar

- Recorrido manual en la aplicación de escritorio instalada (WebView2): ratón, teclado y Esc en las ayudas; todas las pantallas con reglas y tareas reales. Ningún criterio de T21 a T24c queda cumplido sin él.
- El instalador no se ha instalado ni ejecutado.
- Proveedor externo real y correo real de Google o Microsoft, incluida una lectura de 100 mensajes y sus tiempos.
- `evaluation/evaluate_refine.py` no se ha ejecutado contra ningún modelo: la calidad del afinado está sin medir.
- La retención del cerrojo durante cada paso de la lectura, en uso real.
- Los umbrales de sugerencia (3 y 5 aprobaciones, 5 vistas) y el volumen de 100 mensajes no tienen respaldo empírico.
- La heurística `looks_like_invoice` solo se ha probado con textos sintéticos.

## Bloquea distribuir con datos reales (D102, D133, D134)

Política de privacidad actualizada, encargo de tratamiento y revisión del responsable de protección de datos del resumen de afinado y de los documentos enviados en la primera lectura. No se resuelve con código.
