# Asistente administrativo local con aplicación de escritorio

## Resumen

Crear una aplicación de escritorio para Windows que procese facturas, correos y documentos con IA local. Tendrá ventana e icono propios y utilizará **Tauri con WebView2**, basado en Chromium.

La primera entrega incluirá el programa con sistemas simulados, para comprobar los flujos completos mientras se identifican el correo y el sistema administrativo reales. La IA local será la opción inicial y el usuario podrá seleccionar una API externa.

## Cambios principales

- **SDD:** crear especificaciones versionadas con objetivos, flujos, decisiones técnicas, criterios de aceptación y tareas. Incluir contratos para la IA, el correo y el sistema administrativo, junto con una matriz que relacione requisitos y pruebas.
- **Harness de desarrollo:** preparar instrucciones del proyecto, comprobaciones automáticas y un flujo reproducible para construir, verificar y empaquetar la aplicación.
- **Harness de evaluación:** crear datos sintéticos, resultados esperados y pruebas que midan extracción correcta, errores, datos inventados, clasificación, calidad de borradores, tiempo y consumo de memoria. Mantener los resultados de evaluación separados del contenido generado para el usuario.
- **Aplicación de escritorio:** crear una interfaz propia con Tauri y WebView2, conectada a un servicio local con almacenamiento y procesamiento en cola. Mostrar tareas, resultados, incidencias y acciones pendientes de revisión. Al cerrar la ventana, el programa seguirá trabajando en segundo plano y ofrecerá una opción para cerrarlo por completo.
- **IA:** integrar Ollama con Qwen3 8B como primer modelo local. Añadir Groq como primera API externa configurable mediante una interfaz común. El cambio de proveedor será explícito y no habrá envío automático de datos a la nube si falla la IA local.
- **Documentos:** admitir archivos de texto, PDF con texto y facturas escaneadas mediante OCR local. Extraer campos de factura y aplicar comprobaciones programadas de importes y posibles duplicados.
- **Correos y administración:** implementar simuladores que permitan consultar mensajes, crear borradores y registrar facturas. Definir interfaces para sustituirlos posteriormente por conectores reales, una vez identificados los productos y sus capacidades.
- **Acciones:** comenzar con revisión humana. Permitir activar reglas expresas para automatizar tipos concretos de envío o registro, con historial visible de las acciones.
- **Instalación:** preparar un instalador ligero para Windows que compruebe la aplicación, WebView2, Ollama, Qwen3 8B y los componentes de OCR. Descargará e instalará desde Internet lo que falte, reutilizará las instalaciones compatibles y mostrará el progreso. Solo pedirá intervención al usuario cuando sea necesaria para completar un paso. Permitirá reintentar las descargas fallidas y comprobará que la IA responde al finalizar. Las credenciales se mantendrán protegidas en el equipo.

## Pruebas y aceptación

- Ejecutar una colección inicial de **10 documentos y 20 correos sintéticos**, con resultados esperados y casos de datos ausentes, duplicados y errores.
- Comparar los resultados del modelo local y la API externa sobre los mismos casos; registrar calidad, tiempo y memoria sin fijar una precisión prometida antes de medirla.
- Verificar PDF con texto y escaneados, cálculos, duplicados, borradores, revisión y reglas automáticas mediante los simuladores.
- Comprobar fallos de modelo, OCR, conexión y credenciales, así como la recuperación de tareas interrumpidas.
- Verificar que cambiar a la API externa requiere selección explícita y que el modo local no envía documentos a proveedores de IA.
- Probar la ventana, el funcionamiento en segundo plano y el cierre completo.
- Probar el instalador con componentes ausentes y ya instalados, descargas interrumpidas y pasos que requieran intervención.
- Medir la aplicación en un equipo con características equivalentes al ordenador indicado: **16 GB de RAM e Intel Core Ultra 5**.

## Supuestos

- La primera entrega funciona con simuladores; la conexión a cuentas y sistemas reales se concretará cuando se conozcan sus proveedores.
- El procesamiento de IA y OCR es local por defecto. Las futuras conexiones a correo o servicios externos requerirán comunicación con esos servicios.
- La instalación inicial necesita Internet para descargar los componentes ausentes. Qwen3 8B supone una descarga aproximada de **5,2 GB**.
- La automatización se activa mediante reglas concretas; sin una regla activa, las acciones de envío y registro pasan por revisión.
- La capacidad y velocidad del modelo de 8B se determinarán mediante pruebas en el equipo, con posibilidad de ajustar el modelo si los resultados lo requieren.
