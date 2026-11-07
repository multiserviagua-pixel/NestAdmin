# Asistente administrativo local con aplicación de escritorio

## Resumen

Crear una aplicación de escritorio para Windows que procese facturas, correos y documentos con IA. Tendrá ventana e icono propios y utilizará **Tauri con WebView2**, basado en Chromium.

La primera entrega incluirá el programa con sistemas simulados, para comprobar los flujos completos mientras se identifican el correo y el sistema administrativo reales. Desde el cambio de alcance del 2026-11-07, un proveedor de IA externo (cloud/API) es la opción inicial y la IA local es opcional y de respaldo.

## Cambios principales

- **SDD:** crear especificaciones versionadas con objetivos, flujos, decisiones técnicas, criterios de aceptación y tareas. Incluir contratos para la IA, el correo y el sistema administrativo, junto con una matriz que relacione requisitos y pruebas.
- **Harness de desarrollo:** preparar instrucciones del proyecto, comprobaciones automáticas y un flujo reproducible para construir, verificar y empaquetar la aplicación.
- **Harness de evaluación:** crear datos sintéticos, resultados esperados y pruebas que midan extracción correcta, errores, datos inventados, clasificación, calidad de borradores, tiempo y consumo de memoria. Mantener los resultados de evaluación separados del contenido generado para el usuario.
- **Aplicación de escritorio:** crear una interfaz propia con Tauri y WebView2, conectada a un servicio local con almacenamiento y procesamiento en cola. Mostrar tareas, resultados, incidencias y acciones pendientes de revisión. Al cerrar la ventana, el programa seguirá trabajando en segundo plano y ofrecerá una opción para cerrarlo por completo.
- **IA:** usar por defecto Ollama cloud (`gemma4:31b`) mediante una interfaz común, con otros proveedores externos (compatible con OpenAI, Claude) y Ollama con Qwen3 8B como IA local opcional. Ningún contenido sale del equipo hasta que el usuario guarda la clave y confirma el destino. Si el proveedor externo no responde y la IA local está preparada, la tarea se repite con ella y se avisa; nunca se pasa a otro proveedor externo ni de la IA local a uno externo (D100–D102). Groq queda fuera.
- **Documentos:** admitir archivos de texto, PDF con texto y facturas escaneadas mediante OCR local. Extraer campos de factura y aplicar comprobaciones programadas de importes y posibles duplicados.
- **Correos y administración:** implementar simuladores que permitan consultar mensajes, crear borradores y registrar facturas. Definir interfaces para sustituirlos posteriormente por conectores reales, una vez identificados los productos y sus capacidades.
- **Acciones:** comenzar con revisión humana. Permitir activar reglas expresas para automatizar tipos concretos de envío o registro, con historial visible de las acciones.
- **Instalación:** preparar un instalador ligero para Windows que instale la aplicación, WebView2 si falta y los componentes de OCR, sin preparar la IA local (D103). Al abrir, un asistente configura el proveedor externo y comprueba que responde. Si el usuario elige la IA local, el asistente comprueba Ollama y Qwen3 8B, descarga e instala lo que falte, reutiliza las instalaciones compatibles, muestra el progreso, solo pide intervención cuando hace falta, permite reintentar y comprueba que el modelo responde. Las credenciales se mantendrán protegidas en el equipo.

## Pruebas y aceptación

- Ejecutar una colección inicial de **10 documentos y 20 correos sintéticos**, con resultados esperados y casos de datos ausentes, duplicados y errores.
- Comparar los resultados del modelo local y la API externa sobre los mismos casos; registrar calidad, tiempo y memoria sin fijar una precisión prometida antes de medirla.
- Verificar PDF con texto y escaneados, cálculos, duplicados, borradores, revisión y reglas automáticas mediante los simuladores.
- Comprobar fallos de modelo, OCR, conexión y credenciales, así como la recuperación de tareas interrumpidas.
- Verificar que no sale contenido hacia un proveedor externo sin su clave y la confirmación del destino, que el respaldo solo va del externo elegido a la IA local y se avisa, y que con la IA local elegida no se envían documentos a proveedores de IA.
- Probar la ventana, el funcionamiento en segundo plano y el cierre completo.
- Probar el instalador con componentes ausentes y ya instalados, descargas interrumpidas y pasos que requieran intervención.
- Medir la aplicación en un equipo con características equivalentes al ordenador indicado: **16 GB de RAM e Intel Core Ultra 5**.

## Supuestos

- La primera entrega funciona con simuladores; la conexión a cuentas y sistemas reales se concretará cuando se conozcan sus proveedores.
- **Cambio de alcance (2026-11-07):** a petición del usuario, porque arrancar con la IA local ralentiza y sobrecarga la puesta en marcha (42–63 s hasta la primera respuesta de `qwen3:8b` en el equipo objetivo), la aplicación usa por defecto un proveedor externo (Ollama cloud) y la IA local pasa a ser opcional y de respaldo; el instalador ya no descarga ni verifica el modelo local, y Groq sale del diseño (R03, R08, D100–D104; revisa D36). Pendiente de implementar tras integrar los proveedores de la fase 11b.
- **Cambio de alcance (2026-11-06):** a petición del usuario, se procesan fotos de facturas (JPG, PNG, TIFF, BMP, WebP) con OCR local, también varias facturas en una misma imagen, y los adjuntos del correo se importan como documentos (R04, R05, R13, D66–D68). Groq limita las peticiones en su plan gratuito: se reintenta con el mismo proveedor (D65).
- **Cambio de alcance (2026-11-05):** se añade una política de actualizaciones (R14, D60–D64, [specs/v1/actualizaciones.md](specs/v1/actualizaciones.md)): versiones fijadas y verificadas, avisos de licencia en el instalador, ninguna actualización sin aceptación del usuario ni durante una tarea, y copia de los datos antes de migrarlos.
- **Cambio de alcance (2026-11-05):** a petición del usuario, la revisión muestra el origen de cada correo (cuenta con nombre y dirección del buzón, remitente, destinatarios, fecha) y de cada documento (archivo o correo, huella SHA-256, copia guardada que se puede abrir y página de cada campo cuando se localiza); las cuentas se pueden renombrar (R05, R13, D48–D49).
- **Cambio de alcance (2026-11-03):** el correo real entra en la v1 con conectores directos para Microsoft 365 y Gmail, varias cuentas y el mismo contrato que el simulador (R13, D30–D35). El sistema administrativo real sigue fuera.
- El OCR y la extracción de texto son locales. La IA es externa por defecto desde el 2026-11-07, así que el texto mínimo de cada tarea sale hacia el proveedor elegido (D102). Las conexiones a correo o servicios externos requieren comunicación con esos servicios.
- La instalación inicial solo necesita Internet para WebView2 si falta, y la IA externa necesita conexión. Instalar la IA local es opcional; Qwen3 8B supone una descarga aproximada de **5,2 GB**.
- La automatización se activa mediante reglas concretas; sin una regla activa, las acciones de envío y registro pasan por revisión.
- La capacidad y velocidad del modelo de 8B se determinarán mediante pruebas en el equipo, con posibilidad de ajustar el modelo si los resultados lo requieren.
