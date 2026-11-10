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

## Recorrido en `tauri dev` (aplicación de escritorio)

2026-11-10, `tauri dev --features lectura-bajo-demanda` con `NESTADMIN_DATA_DIR` en una carpeta nueva y vacía, sin copiar nada de la instalación. **Sin IA configurada** (el asistente de primer uso se cerró con «Ahora no»), así que ningún proveedor de IA podía recibir peticiones. La ventana se manejó por su puerto de depuración (WebView2), leyendo el contenido real que pinta y llamando a los comandos reales. Solo datos sintéticos: el buzón simulado (6 correos, uno con adjunto) y una factura inventada de texto.

**Preparación.** Sin IA, las 8 tareas quedaron `queued` y el índice vacío. Para simular tareas anteriores al índice se marcaron como `completed`, directamente en la base de datos de pruebas, 4 correos y las 2 facturas; quedaron 2 correos en cola.

| Paso | Resultado |
|---|---|
| Pasada de fondo, con la aplicación abierta y 2 tareas aún en cola | indexó sola las 6 tareas terminadas: 4 correos con su cuerpo y 2 facturas con el texto extraído en local; `pending_index` 0, `no_text` 0, `failed` 0 |
| Asistente de primer uso en pantalla | no hay botón «Chat» y Ctrl+K no abre el panel |
| Ctrl+K desde Pendientes | abre el panel con el foco en el cuadro y `aria-expanded` verdadero |
| Escribir sin enviar | no añade turnos ni resultados |
| «¿Dónde está la factura de la luz de octubre?» | 1 factura: la sintética, encontrada por «luz» y «octubre» en su texto, sin exigir la palabra «factura» |
| «correo del pedido 2231» | 1 correo, encontrado por su cuerpo |
| «rectificativa» | 1 documento |
| «algo que no existe zzzqqq» | sin resultados; tras la corrección solo busca «zzzqqq» |
| Cada respuesta | dice que es una búsqueda por palabras hecha en el equipo y no una respuesta de la IA |
| Tarjeta de la factura (tarea terminada) | abre la vista Tareas con su fila localizada y con el foco; el panel sigue abierto |
| Tarjeta del correo (tarea terminada) | abre Tareas con su fila localizada; no ofrece «Abrir el documento» |
| «Abrir el documento» | aparece solo en la factura; no se pulsó, porque abre el archivo con un programa del sistema |
| Esc | cierra el panel y devuelve el foco al botón «Chat» |

**Corregido durante el recorrido** (commit siguiente a `9ae735c`): la tarjeta de una tarea ya terminada solo decía que no se podía abrir; ahora lleva a Tareas con la fila localizada y, en facturas, ofrece «Abrir el documento». Se amplió la lista de palabras vacías. Tras la corrección: `cargo fmt`, clippy, `cargo test` de `nestadmin-app` y `nestadmin-desktop`, `npm run build` y `npm test` (276) en verde, y el recorrido de la tabla repetido.

## Lo que espera a la IA también se encuentra (D179 ampliada)

El primer recorrido mostró que, sin IA utilizable, lo importado se quedaba en `queued` y no llegaba al índice. El usuario decidió indexarlo. Tras el cambio: `cargo fmt`, clippy, `cargo test --workspace --locked` (1239 pasan, 0 fallan), `npm run build` y `npm test` (276) en verde. Pruebas nuevas en `crates/core/tests/search.rs` y `crates/app/tests/search_index.rs`: sin IA se indexan la factura y el correo en cola sin cambiar su estado, su resultado, sus transiciones ni el orden; con IA utilizable no se indexan ni cuentan en `pending_index`; una tarea en curso nunca se toma; al terminar, la fila pasa a la completa sin duplicarse, y una fila completa no se degrada.

Segundo recorrido en `tauri dev`, misma carpeta sintética, sin IA configurada:

| Paso | Resultado |
|---|---|
| Al abrir, 2 correos seguían en cola sin fila | la pasada de fondo los indexó; `pending_index` 0 |
| Importar una factura inventada (`factura-agua-sintetica.txt`) | la tarea nace `queued`; la pasada la indexa con su texto en menos de 2 s, sin proveedor ni importe |
| Estado y orden de la cola tras indexar | sin cambios: las 3 tareas siguen `queued`, en el mismo orden |
| «¿Tengo alguna factura del agua?» en el chat | 1 factura, con la etiqueta «En espera de la IA: aún no se ha leído» |
| Tarjeta de esa factura | abre Tareas con su fila localizada («En cola · Esperando al proveedor de IA») |

## Pendiente

- No se vio en la aplicación el paso de una fila provisional a la completa cuando la IA procesa la tarea (no había IA); está cubierto por las pruebas automáticas.
- No se probó en la aplicación con tareas en revisión ni Informativos (sin IA no se llega a esos estados); ese camino de la tarjeta está visto en la vista previa.
- No se pulsó «Abrir el documento» ni se probó «Volver a intentar los que fallaron» en la aplicación.
- La suite completa del espacio de trabajo no se repitió tras las dos últimas correcciones.
- El filtro `party` no distingue mayúsculas solo en ASCII (`lower()` de SQLite); la búsqueda por texto sí ignora tildes y mayúsculas.
- Los documentos que la primera lectura omitió por no parecer factura quedan `no_text`, porque su copia se borra.
