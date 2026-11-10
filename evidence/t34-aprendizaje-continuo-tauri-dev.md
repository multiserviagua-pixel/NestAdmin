# T34e — Aprendizaje continuo en la versión de desarrollo

2026-11-10, Windows 11, `scripts/dev-pruebas.ps1 -Datos <carpeta aparte>` (`tauri dev`), commit `cebf498`, IA en Ollama cloud con `gemma4:31b`. Solo datos sintéticos y recuentos. La carpeta de datos era nueva y solo tenía el buzón simulado: la carpeta de pruebas habitual tiene cuentas reales conectadas y no se usó, porque el aprendizaje continuo arranca solo.

## Buzón simulado

5 correos sintéticos con hilo, además de los 6 de ejemplo del simulador (sin hilo):

| Correo | Remitente | Clasificación de la IA |
|---|---|---|
| 3 peticiones de presupuesto | el mismo cliente | `presupuesto`, requiere respuesta |
| 1 petición de presupuesto | otro cliente | `presupuesto`, requiere respuesta |
| 1 queja | el primer cliente | `reclamacion`, requiere respuesta |

Los 6 de ejemplo quedaron en `otros`, `pedido`, `acuse` e `informacion`. Las 11 tareas de correo quedaron en revisión.

Después, con la aplicación cerrada (el simulador carga su estado al arrancar), se añadieron a Enviados del simulador 4 respuestas propias dirigidas al primer cliente (sus 3 presupuestos y la queja) y, antes de una de ellas, un reenvío interno a un compañero. El correo del otro cliente quedó sin respuesta.

## Pasada

| Comprobación | Resultado |
|---|---|
| Primera pasada al abrir | 5 correos comprobados, 4 respuestas observadas, 4 tareas cerradas |
| Tareas cerradas | las 3 peticiones de presupuesto y la queja, como `replied_outside` con la fecha de la respuesta |
| Reenvío interno antes de la respuesta | no impidió encontrar la respuesta al cliente |
| Evidencia `learned` | 3 filas, las de `presupuesto`; la queja (`reclamacion`) se cerró sin evidencia |
| Correo sin respuesta | sigue en revisión |
| Correos sin hilo | no se consultan |
| Historial | 4 entradas «Tarea cerrada: ya estaba respondido fuera de NestAdmin», sin contenido |
| «Consultar correo» más de 2 minutos después | adelantó la pasada: 6 correos comprobados, sin cambios |

## Pantalla

- **Cuentas de correo → Aprendizaje continuo:** casilla «Seguir aprendiendo de mis respuestas» marcada y la línea «Última comprobación: hace 2 minutos. Desde que abrió NestAdmin: 5 correos comprobados, 4 respuestas suyas, 4 tareas cerradas.»
- **Tareas → Lista:** las 4 tareas con «Respondido fuera de NestAdmin» y «Usted ya respondió a este correo desde su programa de correo el 10/11/2026. NestAdmin no envió nada.»
- **Reglas → Sugeridas:** 1 sugerencia, «Usted respondió 3 veces a <dirección> sobre Petición de presupuesto (observado en su correo; no son aprobaciones en NestAdmin).», también en el recuadro «Reglas aprendidas de sus respuestas».
- **«Crear regla (inactiva)» desde la tarjeta:** la regla queda «Inactiva · Aprendida · En rodaje: 0 de 3», con `evidence_origin = learned` y `rodaje = {left: 3, of: 3}`.

No se envió ningún correo: 0 enviados y 0 borradores en el simulador, y 0 acciones de envío.

## Lo que no cubre

- Cuentas reales de Gmail y Outlook, ni la consulta real de metadatos de Enviados.
- Cuenta desconectada o sin credenciales, y errores del proveedor.
- El caso de dos correos del mismo hilo con un envío de NestAdmin entre medias (solo en las pruebas automáticas).
- El interruptor en la aplicación real (comprobado en la vista previa del navegador con datos sintéticos).
- El rodaje con un correo nuevo tras activar la regla.
