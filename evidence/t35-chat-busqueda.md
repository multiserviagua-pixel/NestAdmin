# T35 — Índice local y búsqueda del chat (E1)

2026-11-10, Windows 11, commit `071cb67`. Solo datos sintéticos. Cubre la etapa E1 del chat de la aplicación (D179–D181, R32): el índice local, la búsqueda y el panel del chat respondiendo con la búsqueda por palabras. El asistente con IA (E2) no existe todavía.

## Qué se ejecutó

| Comprobación | Resultado |
|---|---|
| `python scripts/check.py` | correcto |
| `npm run build --prefix ui` (`tsc --noEmit` y `vite build`) | correcto |
| `npm test --prefix ui` | 275 pasan, 0 fallan |
| `cargo fmt --all --check` | correcto |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | sin avisos |
| `cargo test --workspace --locked` (antes del ajuste de las palabras de tipo) | 1228 pasan, 0 fallan, 15 ignoradas |
| `cargo test -p nestadmin-app --locked` y `-p nestadmin-desktop --locked` (después del ajuste) | 620 pasan, 0 fallan |

La suite completa del espacio de trabajo no se repitió tras el último ajuste (`071cb67`), que solo toca `crates/app/src/search.rs`, sus pruebas y la interfaz; se repitieron las de esos dos paquetes y las de la interfaz.

## Pruebas automáticas por caso

| Caso | Dónde | Qué comprueba |
|---|---|---|
| T35a | `crates/core/tests/search.rs`, `crates/app/tests/search_index.rs` (`t35a_*`) | La migración 010 aplica sobre una base nueva y sobre una v9. Una factura sintética queda indexada con su texto y sus campos al terminar; un correo normal, con su cuerpo; un correo guardado sin cuerpo queda `no_text` y se encuentra por asunto; una nueva redacción no se indexa aparte. `complete_indexed` guarda tarea e índice en la misma transacción: si el índice falla, la tarea no cambia. El texto centinela no aparece en `audit_log`. |
| T35b | `crates/app/tests/search_index.rs` (`t35b_*`) | Indexar lo anterior en dos fases (`index_pending`, `store_index`) es repetible: no duplica filas ni cambia `state`, `result` ni las transiciones de ninguna tarea. Original ausente da `no_text`; un error de extracción da `failed` con su código; `reindex_failed` las devuelve a pendientes. Cero llamadas al proveedor de IA y cero consultas al conector de correo (espía). |
| T35c | `crates/core/tests/search.rs`, `crates/app/tests/search_index.rs` (`t35c_*`, `d181_*`) | Encuentra con y sin tildes, con mayúsculas y por prefijo; la sintaxis de FTS5 en la consulta se trata como texto; cada filtro acota; fecha mal formada y estado desconocido son `invalid_filter`; la página lleva `total`, `pending_index`, `no_text` y `failed`; tope de 50 y fragmento de 200 caracteres. `search_question` quita palabras vacías, exige todas las palabras y, sin resultados, admite cualquiera; «factura» o «correo» acotan el tipo y no se exigen como palabra. |
| T35d | `ui/tests/assistant-text.test.mjs` | Textos de la respuesta en cada rama, singulares y plurales, formato de importe y fecha, y ausencia de HTML. |

## Recorrido en la vista previa del navegador (no es `tauri dev`)

Con `npm run dev` de `ui/` y los datos sintéticos de `ui/src/mock.ts`, commit `c1d313f`:

| Paso | Resultado |
|---|---|
| Ctrl+K desde Pendientes | abre el panel con el foco en el cuadro |
| Escribir una pregunta | no busca ni envía nada |
| Intro con «¿Dónde está la factura de la luz?» | responde que no encontró nada con «luz» y avisa de que es una búsqueda por palabras en el equipo, no de la IA |
| Intro con «factura de suministros» | 2 tarjetas (una factura y un correo), con la línea de documentos en preparación, sin texto y fallidos |
| Activar la tarjeta de la factura | abre su tarea en Pendientes y el panel sigue abierto |
| Esc | cierra el panel y devuelve el foco al botón «Chat» (`aria-expanded` falso) |
| Tema oscuro | el panel se lee bien |
| Consola | sin errores |

## Pendiente

- **Recorrido en `tauri dev`** (T35d) con una carpeta de datos solo sintética: no se ha hecho. Falta ver en la aplicación real el panel, la apertura de una tarea desde una tarjeta y que ninguna petición sale al preguntar.
- **Pasada de fondo que indexa lo anterior** (`index_pass` en `crates/desktop/src/main.rs`): sus dos fases están probadas en `crates/app`, pero el hilo que las encadena solo está compilado, no ejecutado.
- El filtro `party` no distingue mayúsculas solo en ASCII (`lower()` de SQLite); la búsqueda por texto sí ignora tildes y mayúsculas.
- Los documentos que la primera lectura omitió por no parecer factura quedan `no_text`, porque su copia se borra.
