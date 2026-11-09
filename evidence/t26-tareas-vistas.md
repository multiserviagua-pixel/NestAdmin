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
| Lista (cuatro columnas: Elemento, Estado, Motivo o detalle y Acciones): a 860, 1000 y 1180 px sin desplazamiento horizontal, 0 celdas recortadas y 0 solapes entre las celdas de elemento, motivo y acciones en las 23 filas (anchos de motivo: 246, 222 y 361 px) | Vista previa con `?pipelines`, medidas del DOM | Pasa |
| Aspecto del tablero en claro y oscuro | Capturas a 860 y 1180 px | Revisado a ojo |

## Interfaz de plantillas (fase 4, D151)

Con `?pipelines` la vista previa trae una plantilla activa y otra inactiva y siete ejecuciones sintéticas: bloqueada por paso fallido, esperando, bloqueada por paso cancelado, por adjunto pendiente y por `attachment_rejected` (el adjunto no se puede importar), una con un paso omitido cuyo adjunto llegó después (`linked_after_waive`) y otra con una factura fallida omitida (que no «llegó después»). Sin el parámetro no hay plantillas (estado vacío). Se sirvió con `npx vite` en un puerto libre y se detuvo al terminar. Las capturas y el recorrido con teclado se hicieron en Edge sin ventana sobre el simulado, no en la aplicación de escritorio.

| Comprobación | Método | Resultado |
|---|---|---|
| Cada texto del indicador, su prioridad sobre la espera de la IA, tarea sin ejecución sin cambios, causa elegida con varios pasos, línea «Paso n de m · plantilla», omitible, confirmaciones, factura que retiene la respuesta, condición en palabras, validación del editor y que «templates» no es preferencia | `npm test` (`tests/tasks-text.test.mjs`) | Pasa |
| Pestaña «Plantillas»: elegirla no cambia `nestadmin.tasks.view`; con flechas del teclado se llega a ella y el foco queda en la pestaña | Vista previa, script y teclado | Pasa |
| Estado vacío con el texto acordado y «Crear plantilla» | Vista previa sin `?pipelines` | Pasa |
| Indicador igual en lista y tablero (los seis casos) y «Ver pasos» en fila de la tabla y en tarjeta, con «Etapa 1: Facturas adjuntas» y «Etapa 2: Respuesta al correo», su descripción debajo, estado, «Revisar», `linked_after_waive` (solo «Este adjunto llegó después de continuar sin él.») y el fallo de descarga sin código («No se puede importar y no se volverá a descargar.») | Vista previa, script | Pasa |
| «Continuar sin ese paso» y «Cancelar respuesta» con su confirmación (el texto literal), con «No» no cambian nada, con «Sí» la respuesta pasa a «Esperando» o a revisión, o se descarta; el foco pasa a «Ver pasos»; la confirmación de continuar solo habla de un adjunto que llega después si falta el adjunto (`npm test`) | Vista previa, script | Pasa |
| Editor: nombre vacío, dominio con «@», dirección inválida (mensaje concreto, `aria-invalid`, `aria-describedby`, foco al campo); conflicto de condición del backend en un aviso con `role=alert`; «Activar» sin condición pide confirmar y con «No» no cambia | Vista previa, script | Pasa |
| Archivar con confirmación, «Archivadas» y «Restaurar» | Vista previa, script | Pasa |
| Descartar en Pendientes: una factura que retiene una respuesta pide confirmar; con «No» no se descarta; una factura sin ejecución se descarta sin aviso | Vista previa, script | Pasa |
| Sin desbordamiento horizontal en lista, tablero y plantillas a 860 y 1180 px; tarjeta abierta sin desbordar; texto mínimo de 12 px; tema oscuro con el fondo y los colores de los tokens | Vista previa, medidas del DOM | Pasa |
| Sin errores en consola durante el recorrido | Vista previa | Pasa |
| Ajustes de la crítica visual: chip «Bloqueada»/«Esperando», filtro «Bloqueadas», «Ver pasos» tras la línea de contexto, «Cancelar respuesta», etapas sin repetición y mayúscula inicial del motivo | `npm test` y vista previa, por script (sin capturas) | Pasa |

También tienen prueba de Rust, en `crates/app/tests/pipelines.rs`: `linked_after_waive` (falso con un paso fallido o cancelado que se omite, verdadero solo con el adjunto que llega después) y la simulación de unificar reglas con una respuesta de paso omitido (`the_merge_simulation_does_not_count_a_reply_with_a_waived_step_as_automatable`).

## Sin comprobar

- **Aplicación de escritorio:** la ventana con WebView2 no se ha recorrido a mano ni está automatizada; el recorrido en la aplicación de escritorio con `tauri dev` sigue pendiente. Por eso T26 sigue parcial.
- **Teclado y lectores de pantalla:** los roles ARIA (secciones con título, listas, pestañas) se han revisado por código; el recorrido con teclado se hizo en Edge sin ventana sobre el simulado, no en la aplicación de escritorio, y no se probó con Narrador.
- **Auditoría automática:** la herramienta Axe no estaba disponible en esta sesión.
- **Plantillas de pipeline (fase 4):** la interfaz existe y se recorrió en la vista previa (ver «Interfaz de plantillas»), pero **no hay recorrido real en Windows con WebView2**, no se probó con Narrador y las capturas y el teclado se hicieron en Edge sin ventana sobre el simulado, y el aspecto en claro y oscuro se comprobó además por medidas del DOM y estilos calculados. El motor, los comandos IPC y sus pruebas de Rust son los de D148–D151 (`crates/core/tests/pipeline.rs`, `crates/app/tests/pipelines.rs`, `crates/app/tests/reprocess.rs`, `crates/desktop/src/recorrido.rs`).
