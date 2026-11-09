# Evidencia T26 (parcial): vistas de Tareas

Cubre R25 y D147. Todos los datos son sintéticos (`ui/src/mock.ts`, dominio `.test`).

## Cómo reproducir

```sh
cd ui && npm ci && npm run build && npm test
npx vite --port 5181
```

La vista previa admite `?muchos` (más de 50 tarjetas en «Con error» y «Cerradas», y tareas en todos los estados) y `?modelo-404` (espera global del proveedor, para ver «Esperando al proveedor de IA»).

## Resultados

| Comprobación | Método | Resultado |
|---|---|---|
| Tipos y compilación | `npm run build` | Pasa |
| Columna de cada estado, estado desconocido en «Con error», orden por columna, límite de 50 y «Ver más», filtro por estado, motivo corto, preferencia inválida y cortes de direcciones largas | `npm test` (`tests/tasks-text.test.mjs`) | Pasa |
| Contraste AA de los tokens y mínimo de 12 px | `npm test` (`tests/tokens.test.mjs`) | Pasa |
| Tablero a 860 px: con tres columnas con tarjetas y dos vacías caben sin desplazamiento; con las cinco llenas, el desplazamiento horizontal es del tablero y la página no se desborda | Vista previa, por script | Pasa |
| La preferencia de vista se guarda y se recupera tras recargar | Vista previa, por script | Pasa |
| «Ver más» añade tarjetas y el foco no se pierde | Vista previa con `?muchos`, por script | Pasa |
| A 1180 px, con «Cerradas» plegada por defecto, las otras cuatro columnas caben sin desplazamiento horizontal (924 de 924 px); el botón desplegar/plegar conserva el foco y `aria-expanded` | Vista previa con `?muchos`, por script y captura | Pasa |
| Motivos legibles (`humanizeTaskError`), tarea reencolada sin su error anterior y espera en color secundario | `npm test` y vista previa con `?muchos&modelo-404` | Pasa |
| Lista a 860 px: la columna de acciones queda visible dentro de la tabla (queda un desplazamiento horizontal pequeño) | Vista previa, por script y captura | Revisado a ojo |
| Aspecto del tablero en claro y oscuro | Capturas a 860 y 1180 px | Revisado a ojo |

## Sin comprobar

- **Windows real:** la ventana con WebView2 no se ha recorrido a mano ni está automatizada. Por eso T26 sigue parcial.
- **Teclado y lectores de pantalla:** los roles ARIA (secciones con título, listas, pestañas) se han revisado por código; no se ha recorrido el tablero solo con teclado ni probado con Narrador.
- **Auditoría automática:** la herramienta Axe no estaba disponible en esta sesión.
- **Plantillas de pipeline:** el motor, los comandos IPC y `api.ts`, `types.ts` y `mock.ts` existen y tienen pruebas de Rust (D148–D151, `crates/core/tests/pipeline.rs`, `crates/app/tests/pipelines.rs`, `crates/app/tests/reprocess.rs`, `crates/desktop/src/recorrido.rs`), pero **la interfaz de plantillas (fase 4) no existe**: la vista previa solo expone una ejecución de muestra con `?pipelines` a través de `mock.ts`, sin pantalla que la muestre. No hay recorrido real en Windows.
