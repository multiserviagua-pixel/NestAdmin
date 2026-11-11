# T15b (parcial) — PDF de venta y membrete en la versión de desarrollo

2026-11-11, Windows 11, `tauri dev` de la rama `claude/fase-11-conexion` en el commit `60ee8af` (E4a, D222–D228). Solo datos sintéticos: empresa, cliente, productor de Verifactu y membretes inventados, carpeta de datos nueva y vacía. Cubre la parte del PDF y del membrete de T15b; el envío por correo (E4b) sigue pendiente.

## Cómo reproducir

```powershell
# 1. Membretes sintéticos (pypdf y cryptography)
python evidence\t15b\membretes.py <membretes>
# 2. Carpeta de datos nueva con un verifactu-productor.json sintético; después, desde crates\desktop:
$env:NESTADMIN_DATA_DIR = "<datos>"
$env:NESTADMIN_PDFIUM_DIR = "<repo>\vendor\pdfium\bin"
$env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = "--remote-debugging-port=9333"
..\..\ui\node_modules\.bin\tauri.cmd dev
# 3. Con la ventana abierta: recorrido y revisión de los PDF (pypdfium2, pillow, opencv-python-headless)
node evidence\t15b\recorrido.mjs <membretes> <salida>
python evidence\t15b\revisar.py <salida>
```

[`recorrido.mjs`](t15b/recorrido.mjs) maneja la ventana real por el protocolo de depuración de WebView2. Pulsa los botones, elige los archivos con el selector de la sección «Membrete» y escribe en los campos de márgenes. Los datos de partida (empresa, cliente y documentos) se crean con los mismos comandos que usa la ventana. [`revisar.py`](t15b/revisar.py) lee el QR de cada PDF con OpenCV, que es un lector independiente de la aplicación, y guarda imágenes de las páginas. Los resultados completos están en [`resultados.json`](t15b/resultados.json) y [`qr.json`](t15b/qr.json), y los PDF generados en [`t15b/pdf`](t15b/pdf).

## Documentos generados

| Documento | Membrete y márgenes | Páginas | QR leído por OpenCV | Imagen |
|---|---|---|---|---|
| Factura F-2026-0001: 3 líneas, IVA 21 % y 10 %, un descuento del 10 %, total 171,93 € | sin membrete (diseño normal) | 1 | `nif=B12345674&numserie=F-2026-0001&fecha=11-11-2026&importe=171.93` | [ver](t15b/pdf-F-2026-0001-1.jpg) |
| Presupuesto P-2026-0001 | sin membrete | 1 | sin QR, como corresponde a un presupuesto | [ver](t15b/pdf-P-2026-0001-1.jpg) |
| Rectificativa FR-2026-0001, R4 por diferencias de F-2026-0001, −54,45 € | sin membrete | 1 | `importe=-54.45` | [ver](t15b/pdf-FR-2026-0001-1.jpg) |
| Factura F-2026-0002, 40 líneas, 484,00 € | membrete de dos páginas, márgenes 40/20/25/20 mm | 2: página 1 sobre el fondo azul y página 2 sobre el rojo | `importe=484.00` | [p. 1](t15b/pdf-F-2026-0002-1.jpg), [p. 2](t15b/pdf-F-2026-0002-2.jpg) |
| Factura F-2026-0003, 12 líneas de 1.234,56 × 98.765,43 con 12,5 % de descuento, 1.478.728.501,87 € | márgenes mínimos: 100/55/47/55 mm, 100 mm de ancho útil | 4 | `importe=1478728501.87` | [p. 1](t15b/pdf-F-2026-0003-1.jpg), [p. 4](t15b/pdf-F-2026-0003-4.jpg) |

- **Leyenda y paginación:** las cuatro facturas llevan la leyenda VERI\*FACTU y «Página 1 de N», comprobado sobre el texto del PDF.
- **Importes:** se recalcularon a mano. 171,93 € = 108,00 + 22,68 + 37,50 + 3,75. 1.478.728.501,87 € = 6 × 106.690.368,10 × 1,21 + 6 × 106.690.368,10 × 1,10.
- **Márgenes mínimos (F-2026-0003):** ninguna cifra de la tabla ni del desglose se solapa, y los totales bajan debajo del desglose. Corrige el fallo que se vio en la prueba anterior y que arregló el commit `e5481d4`.

## Membrete desde la ventana

| Archivo o acción | Resultado mostrado | Membrete guardado después |
|---|---|---|
| PDF de 3 páginas | «Datos no válidos: el membrete debe tener una o dos páginas y este tiene 3» | no |
| A4 apaisado | «… todas las páginas deben ser A4 vertical (210 x 297 mm); la página 1 mide 297 x 210 mm» | no |
| A5 | «… la página 1 mide 148 x 210 mm» | no |
| PDF con contraseña de apertura | «Datos no válidos: el PDF está protegido con contraseña» | no |
| PDF de más de 5 MB | «El PDF supera los 5 MB.» | no |
| Archivo vacío | «El archivo está vacío.» | no |
| Archivo que no es PDF | «El membrete debe ser un archivo PDF.» | no |
| Fondo oscuro en toda la página (D228) | «Datos no válidos: la zona donde va el texto de la página 1 es demasiado oscura y el texto no se leería; use un membrete con fondo claro en esa zona o ajuste antes los márgenes». [Captura](t15b/pantalla-02-rechazo-oscuro.jpg) | no |
| Banda oscura en mitad de la zona de texto (D228) | el mismo mensaje | no |
| PDF con restricciones de propietario y sin contraseña de apertura | «Membrete guardado.» | sí |
| Membrete de dos páginas con bandas solo en los márgenes | «Membrete guardado.» [Captura](t15b/pantalla-03-membrete-guardado.jpg) | sí |
| «Quitar membrete», con la confirmación aceptada | «Membrete quitado.» [Captura](t15b/pantalla-06-membrete-quitado.jpg) | no |

[Pantalla de partida](t15b/pantalla-01-membrete-vacio.jpg): la ayuda ya explica que la zona del texto debe ser clara.

## Márgenes desde la ventana

Con el membrete de dos páginas guardado:

| Valores (sup./der./inf./izq., mm) | Resultado | Márgenes guardados después |
|---|---|---|
| «abc»/20/25/20 | «Margen superior (mm): escriba milímetros enteros, de 0 a 100.» | 35/20/25/20, sin cambio |
| −5/20/25/20 | el mismo mensaje | sin cambio |
| 200/20/25/20 | «Margen superior (mm): el máximo es 100 mm.» | sin cambio |
| 35/60/25/60 | «Los márgenes izquierdo y derecho dejan menos de 100 mm de ancho útil.» | sin cambio |
| 5/20/5/20, con las bandas del membrete dentro de la zona de texto (D228) | mensaje de zona demasiado oscura. [Captura](t15b/pantalla-04-margen-rechazado-banda.jpg) | sin cambio |
| 40/20/25/20 | «Márgenes guardados.» | 40/20/25/20 |
| 100/55/47/55 | «Márgenes guardados.» | 100/55/47/55 |

## Detalle, «Ver PDF» y regeneración

- **Detalle de F-2026-0002:** se abre desde «Documentos» y muestra el botón «Ver PDF». [Captura](t15b/pantalla-05-detalle-factura.jpg)
- **«Ver PDF»:** al pulsarlo no aparece ningún error. Abre el PDF con el visor del sistema; que el visor se abriera no se comprobó de forma automática.
- **Regeneración:** al final, con el membrete ya quitado, se borraron los cinco PDF y se generaron de nuevo. Los cinco salen con los mismos bytes que antes, incluidas las dos facturas emitidas con membrete. Quitar el membrete o cambiar los márgenes no altera lo ya emitido (D226, D227).

| PDF | SHA-256, primeros 16 caracteres | Igual al regenerar |
|---|---|---|
| F-2026-0001 | `19f09bc92eb3b018` | sí |
| P-2026-0001 | `8c41ff0a24e7f2f5` | sí |
| FR-2026-0001 | `fd819d103d1a5ccd` | sí |
| F-2026-0002 | `ba1503bdf4e6ff78` | sí |
| F-2026-0003 | `6d8def8ad25b0fad` | sí |

## Límites y observaciones

- **QR:** lo leyó un lector independiente (OpenCV), con los datos esperados. No se ha contrastado con el servicio de cotejo de la AEAT. La dirección es la de producción; E5 decidirá qué entorno lleva el QR de cada instalación.
- **Entorno de desarrollo:** los dos avisos fijos de las capturas (componentes de OCR que faltan e IA sin configurar) son propios de esta prueba. La copia de trabajo no tiene `vendor/`, así que PDFium se tomó de la copia principal. No se ha probado sobre un instalador: según `AGENTS.md`, se deja para la versión final.
- **Límite del criterio D228:** el umbral es más del 5 % de la zona de texto con contraste insuficiente para el texto negro. Un logotipo oscuro mediano dentro de esa zona puede quedar por debajo y aceptarse.
- **Mensaje de error del descuento:** en el comando `sales_create_draft` el descuento viaja como fracción (0,10 = 10 %); la ventana lo convierte desde el porcentaje. Una primera ejecución del recorrido envió «10» por error. El mensaje del núcleo dijo «línea 1» para todas las líneas, aunque el índice del campo (`lines[3]`) era correcto. La ventana valida el descuento antes de enviarlo, así que el usuario no llega a verlo, pero el número de línea del mensaje es incorrecto y queda anotado como observación.
