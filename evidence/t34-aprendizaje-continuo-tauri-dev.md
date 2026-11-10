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

## Cuentas reales (solo recuentos)

2026-11-10, misma versión, con la carpeta de pruebas habitual y las dos cuentas reales conectadas (una de Gmail y una de Outlook), con autorización expresa. Copia previa de la base: `nestadmin.antes-continuo.db` en esa carpeta.

| Comprobación | Gmail | Outlook |
|---|---|---|
| Tareas de correo en revisión | 28 | 4 |
| De ellas, con el dato del hilo (consultables) | 1 | 1 |
| Pasada al abrir | 1 comprobado, 0 respuestas, 0 cerradas, sin error | 1 comprobado, 0 respuestas, 0 cerradas, sin error |
| «Consultar correo» | sin mensajes nuevos | sin mensajes nuevos |

La consulta de metadatos de Enviados respondió sin error en las dos cuentas reales (Gmail `format=metadata`, Graph `$select` sin cuerpo). Ninguna de las dos tareas consultables tenía respuesta propia posterior, así que no se cerró nada ni hubo evidencia nueva. No se envió nada y no se creó ni activó ninguna regla.

**Nota:** a los pocos segundos de abrir esta versión, otra sesión de trabajo volvió a abrir su propia versión de desarrollo sobre la misma carpeta de datos. Esa versión ya incluía el mismo código de D178 (`cebf498`), así que los recuentos no dependen de qué ventana los mostró.

**Límite:** las demás tareas reales se descargaron antes de que NestAdmin guardara el hilo del mensaje y el aprendizaje continuo no puede consultarlas; y no había correo nuevo. Falta ver con cuentas reales un cierre por respuesta hecha fuera, que exige un correo nuevo respondido desde Gmail u Outlook.

## Cuentas reales con hilos recuperados (solo recuentos)

2026-11-10, commit `31e9ead` (recuperación del hilo de las tareas antiguas, D178), misma carpeta y mismas cuentas, con autorización expresa. Copia previa de la base: `nestadmin.antes-hilos.db` en esa carpeta. Pasada al abrir y otra adelantada con «Consultar correo».

| Comprobación | Gmail | Outlook |
|---|---|---|
| Tareas con el hilo recuperado (`thread_recovered`) | 26 | 3 |
| Tareas que siguen sin hilo | 1 en revisión | 2 completadas (con envío de NestAdmin) |
| Pantalla «Aprendizaje continuo» | 24 correos comprobados, 2 respuestas suyas, 2 tareas cerradas | 2 correos comprobados, 0 respuestas suyas, 0 tareas cerradas |
| Tareas cerradas como `replied_outside` | 2 | 0 |
| Evidencia `learned` nueva | 0 | 0 |

- **Primer cierre real por respuesta hecha fuera:** 2 tareas de Gmail, con la fecha de la respuesta encontrada en Enviados por metadatos.
- **Sin evidencia nueva:** todas las tareas reales antiguas están en `otros` y sin `needs_reply`, porque se clasificaron antes del catálogo D177 y del triaje. D178 no da evidencia con `otros` ni sin `needs_reply` explícito, así que las filas `learned` siguen en 3, todas del recorrido sintético.
- **Outlook:** las 2 tareas con envío confirmado de NestAdmin no tienen hilo guardado. Por la regla que falla cerrado, las 3 tareas de Outlook con hilo recuperado no se cierran ni dan evidencia.
- **Nada enviado:** 2 acciones de envío, las mismas que antes de la prueba, y ninguna regla creada ni activada.
