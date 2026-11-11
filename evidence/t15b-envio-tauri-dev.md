# T15b (parcial): envío por correo de un documento de venta en la versión de desarrollo

Prueba del 2026-11-11 en Windows 11, con `tauri dev` de la rama `claude/fase-11-conexion` a partir del commit `ef662e1` (E4b, D229–D246) y las correcciones de fechas descritas abajo.

- **Datos:** solo sintéticos, en una carpeta nueva y vacía: empresa, clientes y productor de Verifactu inventados.
- **Cuentas de correo:** no hay ninguna real. La única cuenta es el buzón simulado de la aplicación, así que no sale ningún correo del equipo.
- **Alcance:** cubre el envío con revisión desde la ventana.
- **Lo que no cubre:**
  - Gmail y Graph reales.
  - El corte a mitad de envío.
  - La regla por cliente.

  Esas tres cosas están cubiertas por pruebas automáticas, enlazadas en T15b de la [trazabilidad](../specs/v1/trazabilidad.md).

## Cómo reproducir

```powershell
# 1. Carpeta de datos nueva con un verifactu-productor.json sintético; después, desde crates\desktop:
$env:NESTADMIN_DATA_DIR = "<datos>"
$env:NESTADMIN_PDFIUM_DIR = "<repo>\vendor\pdfium\bin"
$env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = "--remote-debugging-port=9333"
..\..\ui\node_modules\.bin\tauri.cmd dev
# 2. Con la ventana abierta:
node evidence\t15b\recorrido-envio.mjs <datos> <salida>
```

[`recorrido-envio.mjs`](t15b/recorrido-envio.mjs) maneja la ventana real por el protocolo de depuración de WebView2:

- **Lo que hace en la ventana:** pulsa los botones, escribe en los campos de la tarjeta y acepta la confirmación. Antes de aceptarla, guarda su texto.
- **Datos de partida:** empresa, clientes y facturas se crean con los mismos comandos que usa la ventana.
- **Qué salió del buzón:** lo lee del estado del simulador (`simulador-correo.json`), no de lo que dice la tarea.

Los resultados completos están en [`envio-resultados.json`](t15b/envio-resultados.json).

## Recorrido

| Paso | Resultado | Captura |
|---|---|---|
| Detalle de F-2026-0001, cliente con correo | «Envíos por correo: Todavía no se ha enviado por correo» y el botón «Enviar por correo». | [01](t15b/envio-01-detalle-antes.jpg) |
| «Enviar por correo» | Abre la tarjeta en Pendientes → Correos. **Para:** `compras@alfa.test`. **Cuenta:** «Correo simulado». **Asunto y cuerpo:** plantilla fija, sin IA. **Adjunto:** `F-2026-0001.pdf · 25 KB`. **Versión:** 1. Aviso: «No se envía nada hasta que lo apruebe». | [02](t15b/envio-02-tarjeta-revision.jpg) |
| Cambiar el asunto y «Guardar cambios» | «Cambios guardados como versión nueva; apruébela para enviar». La tarea pasa a la versión 2 con `edited: true`. El simulador sigue con 0 enviados. | — |
| «Aprobar y enviar» | Confirmación: «Se enviará Factura F-2026-0001 con su PDF adjunto a compras@alfa.test, desde Correo simulado · administracion@nestadmin.test. Un correo enviado no se puede retirar. ¿Enviar ahora?». Tras aceptar: «Enviado a compras@alfa.test. La entrega al destinatario no se comprueba.». No queda ninguna tarea abierta. | [03](t15b/envio-03-tras-aprobar.jpg) |
| Lo que salió del buzón simulado | Un mensaje nuevo, fuera de hilo. **Para:** `compras@alfa.test`. **Asunto:** el editado, «Factura F-2026-0001 - revisada». **Adjunto:** un PDF `F-2026-0001.pdf` de 25.111 bytes, con el **mismo SHA-256 que el PDF emitido** (`361486cd…`). **Recibo:** `S-0001`. **Clave:** `simulador/task-1/create_sales_draft/v2`. | — |
| Detalle después del envío | La factura pasa a «Enviado». El historial tiene una fila: «11/11/26, 18:28 · compras@alfa.test · administracion@nestadmin.test · Aprobado por usted · F-2026-0001.pdf (25 KB)». | [04](t15b/envio-04-historial.jpg) |
| Segundo envío del mismo documento y «Descartar» | La tarjeta abre una versión 1 nueva. Al descartar se muestra «Envío descartado». El historial sigue con 1 fila y el simulador con 1 enviado. | — |
| F-2026-0002, cliente sin correo | La tarjeta abre con «Para» vacío y el aviso «El cliente no tiene correo: escriba el destinatario para poder enviarlo». En la lista aparece la marca «Sin destinatario». | [05](t15b/envio-05-sin-correo.jpg) |
| «Aprobar y enviar» sin destinatario | «Sin destinatario: escríbalo para poder enviar.». No se pide confirmación y no sale nada (el simulador sigue con 1). | — |
| Destinatario «no-es-un-correo» y «Guardar cambios» | «El destinatario no es una dirección de correo válida: escriba una sola dirección.» | [06](t15b/envio-06-destinatario-invalido.jpg) |
| Fecha de la tarea en la lista de Pendientes | «11 nov». | [07](t15b/envio-07-fecha-pendientes.jpg) |

## Fallos encontrados y corregidos en esta prueba

Los dos fallos eran de unidades de tiempo en la ventana. El núcleo guarda las marcas de tiempo en milisegundos (`now_ms`) y la ventana las interpretaba como segundos:

1. **Fecha del historial de envíos.** Salía «1/11/32, 23:02», una fecha del año 58 000 que el formato recortaba. `sentAtText` y el tipo `SalesSend.sent_at` pasan a milisegundos. La simulación de la ventana también usa ahora `Date.now()`. La prueba de `ui/tests/ventas-envio.test.mjs` comprueba ahora la fecha y la hora exactas con valores en milisegundos.
2. **Fecha de la tarjeta de envío en la lista de Pendientes.** Mostraba «04 nov» y «05 nov» para tareas creadas el 11 nov. La causa es que `shortDate` multiplica por 1000 los números. La fila del envío le pasa ahora `created_at / 1000`.

Las pruebas automáticas no los detectaban porque usaban la misma unidad equivocada que la ventana.

## Límites y observaciones

- **Buzón simulado:** el envío llega al simulador de la aplicación. Gmail y Graph se prueban con sus simuladores HTTP en las pruebas automáticas, no con cuentas reales.
- **Entrega:** la aplicación no comprueba que el correo llegue al destinatario y lo dice en la propia tarjeta.
- **Clave del borrador en el simulador:** la clave interna dice `create_sales_draft`, aunque la acción del registro es `send_sales_document` (`task-1/send_sales_document/v2`). Es un nombre interno y no afecta a la deduplicación. Queda anotado.
- **Entorno de desarrollo:** los dos avisos fijos de las capturas (componentes de OCR que faltan e IA sin configurar) son propios de esta prueba. PDFium se tomó de la copia principal. No se ha probado sobre un instalador: según `AGENTS.md`, se deja para la versión final.
