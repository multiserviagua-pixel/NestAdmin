# Tareas y puertas de aceptación v1

## Secuencia de implementación

Cada casilla se marca solo cuando todos sus requisitos están en `Cumplido` en la [matriz](trazabilidad.md); mientras tanto se anota su estado. Revisado el 2026-11-05.

- [ ] Fijar versiones y licencias de Rust/Tauri, WebView2, Ollama/modelo y OCR; automatizar instalación de herramientas de desarrollo y compilación Windows. — *Parcial (R11, R14)*: versiones en la tabla de [decisiones](decisiones.md), modelo fijado también por digest (D62); inventario de licencias reproducible en [THIRD_PARTY_NOTICES.md](../../THIRD_PARTY_NOTICES.md), comprobado en CI y con sus textos en el instalador (D60); `scripts/setup-dev.ps1` instala o comprueba las herramientas con versiones fijadas y `scripts/build-clean.ps1` compila desde un clon limpio; [política de actualizaciones](actualizaciones.md) con copia previa a migrar (D61). Falta: ensayo en una máquina limpia, resolver la redistribución de las DLL GPL/LGPL de Tesseract y el actualizador firmado (D63), que espera la clave del editor.
- [ ] Implementar almacenamiento, cola durable, recuperación idempotente y exposición local protegida; probar interrupciones. — *Parcial (R02)*: núcleo y capa de aplicación probados, reinicio sin efectos duplicados; sin ensayo desde la ventana.
- [ ] Implementar contratos y simuladores de correo y administración; probar que los efectos son rastreables e idempotentes. — *Parcial (R06, R13)*: simuladores locales con idempotencia y reconciliación; conectores de Microsoft 365 y Gmail solo como biblioteca con servicios simulados, sin cuentas reales ni integración en la aplicación.
- [ ] Integrar Ollama y Groq, selección explícita y protección de credenciales; comprobar que no existe respaldo automático a la nube. — *Parcial (R03); pendiente (R09)*: adaptadores y selección explícita probados con servidor simulado, sin Groq real; la auditoría de credenciales (T09) no se ha hecho.
- [ ] Implementar texto, PDF y OCR local, extracción estructurada, comprobaciones decimales y aviso de duplicados. — *Parcial (R04, R05)*: texto, PDF y OCR local sobre documentos sintéticos, validación decimal y duplicados; umbral de páginas escaneadas sin calibrar.
- [ ] Implementar interfaz, revisión, reglas explícitas, historial, área de notificación y salida completa. — *Parcial (R01, R07)*: cerrar oculta e instancia única comprobados; bandeja, salida y recorrido de revisión y reglas desde la ventana por comprobar a mano.
- [ ] Preparar instalador con detección, reutilización, progreso, reintentos y comprobación real del modelo; ensayar casos de instalación Windows. — *Parcial (R08)*: T08b cumplido con dobles, incluido el aviso de memoria (D46); T08c y T08 parciales, en el equipo de desarrollo y sin Windows limpio; cliente OAuth en el paquete y programa separado de los datos (D51, D52) implementados, sin construir el instalador ni instalarlo.
- [ ] Ejecutar la colección sintética completa con ambos proveedores y almacenar métricas fuera de los resultados de usuario. — *Parcial (R10)*: corpus completo con Ollama/Qwen3 8B; falta Groq y la medición de memoria.
- [ ] Medir en el hardware objetivo y registrar condiciones y limitaciones; actualizar matriz con evidencia verificable. — *Pendiente (R12)*: sin mediciones en el equipo objetivo. Hasta entonces, los umbrales de memoria de D46 son provisionales.

## Puertas de entrega

Una entrega solo cumple el plan cuando todos los requisitos aplicables de la matriz tienen evidencia satisfactoria, incluida la ejecución de Windows y la medición en hardware equivalente. Fallos, casos omitidos, credenciales o hardware no disponibles deben declararse como pendientes. La comparación de modelos comunica mediciones observadas, no una precisión garantizada.
