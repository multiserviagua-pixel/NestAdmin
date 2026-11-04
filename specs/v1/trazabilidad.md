# Matriz de trazabilidad y verificación v1

Estado inicial: **pendiente**, salvo que una ejecución reproducible aporte evidencia. Un caso descrito aquí es una especificación de prueba, no una prueba superada. Registrar plataforma, versiones, fecha, comandos, resultado y ubicación de evidencia. Las pruebas que dependan de Windows y hardware objetivo deben ejecutarse allí.

Formato que comprueba `scripts/check.py`: cada requisito tiene al menos una fila; cada fila tiene un identificador de prueba único (`T05`, `T05b`…); el estado es `Pendiente`, `Parcial` o `Cumplido`, y los dos últimos deben enlazar a un archivo de evidencia existente, por ejemplo `Parcial — [evidencia](../../evidence/t05.md)`.

| Requisito | Casos de verificación previstos | Estado |
| --- | --- | --- |
| R01 | T01: cerrar ventana con tarea en curso, restaurar y salir desde el área de notificación; comprobar que el proceso termina. | Parcial — cerrar oculta e instancia única comprobados; bandeja y salida por comprobar a mano, [evidencia](../../evidence/t01-escritorio.md) |
| R02 | T02: interrumpir en distintos estados, reiniciar, verificar persistencia y ausencia de efectos duplicados. | Parcial — núcleo y capa de aplicación (reinicio sin efectos duplicados), sin ensayo desde la ventana, [evidencia](../../evidence/t02-nucleo.md) |
| R03 | T03: verificar selección local inicial; provocar error local y observar que no hay peticiones a Groq; seleccionar externo explícitamente y comprobar adaptador. | Parcial — biblioteca con servidor simulado y selección explícita en la ventana, sin Groq real, [evidencia](../../evidence/t03-proveedores.md) |
| R04 | T04: procesar texto, PDF textual y escaneado; forzar fallo de OCR y verificar incidencia recuperable. | Parcial — texto, PDF con texto y escaneado con OCR local sobre documentos sintéticos; fallos explícitos; umbral sin calibrar ni empaquetado, [evidencia](../../evidence/t04-pdf-ocr.md) |
| R05 | T05: comprobar ausencias, operaciones decimales, importes incoherentes y duplicados con explicación. | Parcial — validación decimal y duplicados sin extracción por IA, PDF ni OCR, [evidencia](../../evidence/t05-validacion.md) |
| R06 | T06: consultar correo, crear borrador y registrar factura mediante simuladores; verificar aislamiento de cuentas reales. | Parcial — contratos como traits reemplazables y simuladores locales sin red con idempotencia, persistencia y reconciliación con el núcleo, sin interfaz, [evidencia](../../evidence/t06-simuladores.md) |
| R07 | T07: comprobar revisión predeterminada, regla activada y desactivada, condiciones no coincidentes, historial y cambios tras aprobación. | Parcial — lógica probada con proveedor simulado y con Ollama local, sin recorrido desde la ventana, [evidencia](../../evidence/t07-revision-reglas.md) |
| R08 | T08b: preparación automática con Ollama, descargas y sistema simulados (casos T08b-01 a T08b-13 de [pruebas del instalador](pruebas-instalador.md)). | Cumplido — 19 pruebas de `crates/setup` con Ollama y servidor de descargas en 127.0.0.1 y un doble de `System`, [evidencia](../../evidence/t08-instalador.md) |
| R08 | T08c: comprobación al abrir (casos T08c-01 a T08c-04). | Parcial — lógica de `startup` probada con dobles; la apertura del asistente en la ventana solo se ha visto con el servidor simulado, [evidencia](../../evidence/t08-instalador.md) |
| R08 | T08: instalación real en Windows (casos T08-01 a T08-09). | Parcial — en el equipo de desarrollo, con Ollama y modelo ya presentes; sin equipo limpio ni cortes reales de red, [evidencia](../../evidence/t08-instalador.md) |
| R09 | T09: inspeccionar persistencia, registros y artefactos con credenciales de prueba; verificar almacén protegido y ausencia de secretos. | Pendiente |
| R10 | T10: ejecutar corpus sintético y calcular métricas por caso, incluidas ausencias y valores inventados. | Parcial — corpus completo con Ollama/Qwen3 8B, sin Groq ni memoria, [evidencia](../../evidence/t10-ollama-qwen3.md) |
| R11 | T11: ejecutar las comprobaciones, construcción y empaquetado documentados desde un entorno limpio; verificar separación de resultados. | Pendiente |
| R12 | T12: comparar ambos proveedores en los mismos casos y medir tiempo y memoria en equipo objetivo, registrando configuración y errores. | Pendiente |

Las pruebas de errores de red y credenciales se aplican a T03 y T09; los fallos de modelo a T03; los de OCR a T04; la recuperación a T02. Los datos para evaluación deben ser sintéticos, y las métricas separadas de la información del usuario.
