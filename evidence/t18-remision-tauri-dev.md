# T18 (parcial): remisión Verifactu con el simulador local en la versión de desarrollo

Prueba del 2026-11-12 en Windows 11, con `tauri dev` de la rama `claude/fase-11-conexion` en el commit `ae6358f` (E5, D247–D261).

- **Datos:** solo sintéticos (empresa, cliente y productor de Verifactu inventados), en una carpeta nueva y vacía.
- **Entorno:** `simulador`. La remisión va al simulador de la AEAT que la aplicación arranca en el propio equipo (127.0.0.1), así que no sale nada hacia la AEAT.
- **Fuera de esta prueba:** el certificado, la preproducción y la producción. Corresponden a T18b.

## Cómo reproducir

La misma carpeta de datos se usa en tres arranques de la ventana. Lo que cambia entre ellos es el modo de depuración del simulador (`NESTADMIN_VERIFACTU_SIM`, que solo existe en compilaciones de depuración, D254):

```powershell
# Comunes, desde crates\desktop, con un verifactu-productor.json sintético en <datos>:
$env:NESTADMIN_DATA_DIR = "<datos>"; $env:NESTADMIN_PDFIUM_DIR = "<repo>\vendor\pdfium\bin"
$env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = "--remote-debugging-port=9333"
# 1. Carpeta nueva y el simulador rechaza el primer lote
$env:NESTADMIN_VERIFACTU_SIM = "rechazar-siguiente"; ..\..\ui\node_modules\.bin\tauri.cmd dev
node evidence\t18\recorrido-remision.mjs rechazo <salida>
# 2. Sin red
$env:NESTADMIN_VERIFACTU_SIM = "sin-red"; ..\..\ui\node_modules\.bin\tauri.cmd dev
node evidence\t18\recorrido-remision.mjs sin-red <salida>
# 3. Con red otra vez
Remove-Item Env:NESTADMIN_VERIFACTU_SIM; ..\..\ui\node_modules\.bin\tauri.cmd dev
node evidence\t18\recorrido-remision.mjs reanudar <salida>
```

[`recorrido-remision.mjs`](t18/recorrido-remision.mjs) maneja la ventana real por el protocolo de depuración de WebView2. Pulsa los botones y acepta las confirmaciones, aunque antes guarda su texto. El estado lo lee con el mismo comando que usa la pantalla. La remisión la hace la propia aplicación: tras emitir, en su ciclo de fondo o con «Remitir ahora».

Los resultados completos están en [`remision-resultados.json`](t18/remision-resultados.json).

## Recorrido

| Paso | Resultado | Captura |
|---|---|---|
| Emitir F-2026-0001 | La emisión vuelve en 22 ms, sin esperar a la red. A los 3 s la aplicación ya ha remitido sola y el simulador ha rechazado el lote: «Rechazado. Código 1100: Rechazo simulado», con el botón «Subsanar». | [01](t18/remision-01-rechazado.jpg) |
| «Subsanar» | La confirmación explica que se crea un registro nuevo con los mismos datos y que, si el problema estaba en los datos de la factura, hay que hacer una rectificativa. Aparece «Se creó el registro 2 de subsanación». | — |
| «Remitir ahora» durante la espera de la AEAT | «Todavía no toca enviar: la AEAT pide una espera entre lotes y faltan 51 s». No envía nada. | [02](t18/remision-02-espera.jpg) |
| Pasada la espera (54 s) | La subsanación se acepta sola. El registro 1 queda «Subsanado», con «Rechazo previo: Código 1100… Lo subsana el registro 2», y el registro 2 se ve como «Alta (subsanación del registro 1)» y «Aceptado». | [03](t18/remision-03-subsanado-aceptado.jpg) |
| Emitir F-2026-0002 y anularla | El alta y la anulación van en lotes distintos (D257), separados por la espera, y las dos acaban aceptadas (121 s). Último lote: «Correcto», con su CSV. | [04](t18/remision-04-todo-aceptado.jpg) |
| Configuración > Verifactu | Solo el simulador se puede elegir. Preproducción y producción aparecen deshabilitadas con la nota de T18b. Si se piden por el comando, la respuesta es `environment_unavailable` y el entorno no cambia. | [05](t18/remision-05-configuracion.jpg) |
| Segundo arranque, sin red: emitir F-2026-0003 | La emisión vuelve en 8 ms. La cabecera muestra «Último error de la remisión [unavailable]: sin red (simulado)…» y «Lote 5 (1 registro) sin respuesta tras 1 intento: se reenviará igual, sin duplicar nada». El registro queda «en envío». | [06](t18/remision-06-sin-red-pendiente.jpg) |
| Tercer arranque, con red | En 12 s, sin pulsar nada, el mismo lote 5 se reenvía (2.º intento) y la respuesta es «Correcto». No se abre ningún lote nuevo ni se crea ningún registro. Los registros 2 a 5 quedan aceptados y el 1, subsanado. | [07](t18/remision-07-reanudado.jpg) |
| Tareas | Hay 5 tareas «Remisión Verifactu» cerradas. Ninguna en cola, en curso o con error. | [08](t18/remision-08-tareas.jpg) |

## Qué se ve en la prueba

- **La emisión no espera a la red** (D250, D261). Emitir tardó entre 8 y 40 ms, también en el arranque sin red.
- **Control de flujo** (D251). Entre lotes se respeta la espera de 60 s que devuelve el simulador, y la ventana dice cuánto falta.
- **Corte sin pérdida ni duplicado** (D252). Un lote sin respuesta se conserva y se reenvía igual, sin crear registros nuevos.
- **Rechazo y subsanación** (D253). La subsanación es un registro nuevo y encadenado. El rechazado queda como estaba y vinculado a su subsanación.
- **Simulador sembrado** (D254). Tras reabrir, el simulador conoce lo ya aceptado.

## Límites y observaciones

- **Simulador, no AEAT.** Las respuestas son las del simulador de `crates/verifactu`. La correspondencia con la AEAT real (códigos, `RechazoPrevio` y `SinRegistroPrevio` en la subsanación, 3001 de una anulación reenviada) se ensaya en preproducción (T18b, anotado en D253 y DV-17).
- **QR de pruebas.** Las facturas emitidas en `simulador` llevan para siempre el QR de pruebas, como avisa la propia ventana. Cómo empieza la cadena real queda abierto para T18b (D256).
- **CSV del simulador.** El número de CSV del simulador vuelve a empezar al reabrir; el del lote 5 coincide con el del lote 4 de la sesión anterior. Es propio del simulador, que no guarda estado entre arranques (D254).
- **Entorno de desarrollo.** Los dos avisos fijos de las capturas (componentes de OCR que faltan e IA sin configurar) son propios de esta prueba. PDFium se tomó de la copia principal. No se ha probado sobre un instalador: según `AGENTS.md`, eso queda para la versión final.
