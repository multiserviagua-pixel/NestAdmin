# Tareas y puertas de aceptación v1

## Secuencia de implementación

- [ ] Fijar versiones y licencias de Rust/Tauri, WebView2, Ollama/modelo y OCR; automatizar instalación de herramientas de desarrollo y compilación Windows.
- [ ] Implementar almacenamiento, cola durable, recuperación idempotente y exposición local protegida; probar interrupciones.
- [ ] Implementar contratos y simuladores de correo y administración; probar que los efectos son rastreables e idempotentes.
- [ ] Integrar Ollama y Groq, selección explícita y protección de credenciales; comprobar que no existe respaldo automático a la nube.
- [ ] Implementar texto, PDF y OCR local, extracción estructurada, comprobaciones decimales y aviso de duplicados.
- [ ] Implementar interfaz, revisión, reglas explícitas, historial, área de notificación y salida completa.
- [ ] Preparar instalador con detección, reutilización, progreso, reintentos y comprobación real del modelo; ensayar casos de instalación Windows.
- [ ] Ejecutar la colección sintética completa con ambos proveedores y almacenar métricas fuera de los resultados de usuario.
- [ ] Medir en el hardware objetivo y registrar condiciones y limitaciones; actualizar matriz con evidencia verificable.

## Puertas de entrega

Una entrega solo cumple el plan cuando todos los requisitos aplicables de la matriz tienen evidencia satisfactoria, incluida la ejecución de Windows y la medición en hardware equivalente. Fallos, casos omitidos, credenciales o hardware no disponibles deben declararse como pendientes. La comparación de modelos comunica mediciones observadas, no una precisión garantizada.
