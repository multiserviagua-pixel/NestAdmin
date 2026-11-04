# Requisitos, flujos y decisiones v1

## Objetivo y límites

Procesar documentos y correo mediante una aplicación Windows propia que prioriza IA y OCR locales, con supervisión humana y conectores simulados. Una API externa es opcional y requiere elección explícita. La interfaz, cola, instalador y conectores descritos son objetivos hasta que se implementen y prueben.

## Requisitos verificables

- **R01** La ventana Tauri usa WebView2 y tiene identidad visual propia; cerrarla oculta la ventana y mantiene el trabajo. Una acción identificable termina el programa completamente.
- **R02** El servicio local conserva tareas y resultados, muestra estado, incidencias y pendientes, y recupera tareas interrumpidas sin repetir acciones externas ya confirmadas.
- **R03** Ollama con Qwen3 8B es la selección inicial. Groq es una opción explícita mediante el mismo contrato. Un error local nunca provoca un cambio automático ni envío de contenido a la nube.
- **R04** Texto, PDF con texto y PDF escaneado se procesan; el OCR se ejecuta localmente. Los fallos se comunican sin presentar una extracción incompleta como completa.
- **R05** Se extraen campos de factura con ausencia explícita, se comprueban importes mediante aritmética decimal y se señalan posibles duplicados sin descartarlos silenciosamente.
- **R06** Los simuladores permiten consultar mensajes, crear borradores y registrar facturas mediante interfaces reemplazables; no acceden a cuentas reales.
- **R07** Enviar y registrar requieren revisión por defecto. Solo una regla concreta, activa y aplicable permite automatizar; cada decisión y acción queda en historial visible.
- **R08** El instalador Windows deja la aplicación lista en una sola ejecución: identifica y reutiliza componentes compatibles, instala o descarga lo que falta, informa de progreso, permite reintentos e intervención cuando sea necesaria y no termina con éxito hasta verificar una respuesta real de la IA local. Al abrir la aplicación, un componente detenido se arranca sin intervención y uno ausente se ofrece reparar desde la propia ventana; nunca se pasa a un proveedor externo.
- **R09** Las credenciales se protegen en el equipo y nunca se incluyen en registros, evaluaciones ni artefactos del usuario.
- **R10** El conjunto de evaluación contiene 10 documentos y 20 correos sintéticos con expectativas, ausencias, duplicados y errores; mide extracción, errores, invenciones, clasificación, borradores, tiempo y memoria sin presentar una precisión predeterminada.
- **R11** El desarrollo dispone de instrucciones, verificaciones y pasos reproducibles de construcción y empaquetado; las evidencias de evaluación quedan separadas de los resultados del usuario.
- **R12** La aceptación incluye mediciones en un equipo equivalente a 16 GB RAM e Intel Core Ultra 5, con proveedor local y externo sobre los mismos casos y condiciones documentadas.
- **R13** Conectores directos de Microsoft 365 (Graph) y Gmail (API) cumplen el contrato de correo con varias cuentas a la vez. Cada cuenta inicia sesión con OAuth de escritorio y sus credenciales viven en el almacén protegido. El contenido solo viaja entre el equipo y el proveedor de esa cuenta, sin intermediarios. Un envío ocurre una sola vez por clave, también tras una interrupción, y el fallo de una cuenta no detiene a las demás.

## Flujos

1. **Documento:** importar → encolar → extraer texto u OCR → solicitar extracción al proveedor elegido → validar estructura y cálculos → detectar posibles duplicados → presentar resultado e incidencias → revisar → registrar en el simulador o aplicar una regla activa.
2. **Correo:** consultar simulador → encolar clasificación y propuesta de borrador → presentar mensaje, borrador y evidencias → revisar y enviar mediante simulador o aplicar regla concreta activa.
3. **Fallo y recuperación:** registrar error y estado recuperable → reintento explícito o política delimitada → reanudar tareas. Una acción confirmada se identifica de forma estable y no se duplica en recuperación.
4. **Proveedor:** local por defecto; el usuario elige expresamente el externo y configura credenciales. Un fallo informa y mantiene la selección; nunca hace transferencia de respaldo. Volver a local es una selección explícita.
5. **Ventana:** cerrar oculta; un control accesible en el área de notificación permite restaurar o salir por completo. Al salir, persistir el trabajo en curso y permitir recuperación posterior.
6. **Instalación:** un único instalador copia la aplicación e instala WebView2 si falta → antes de terminar, el asistente de preparación detecta versión compatible de Ollama, servicio y modelo → instala o descarga únicamente lo faltante → muestra progreso, errores y reintentos → verifica una respuesta real del modelo → el instalador termina. Los pasos que necesiten elevación, aceptación o interacción la solicitan cuando corresponda. Al abrir la aplicación se repite una comprobación rápida: si Ollama está detenido se arranca; si falta algo, la ventana muestra el mismo asistente.

## Decisiones técnicas y riesgos abiertos

Las decisiones concretas, sus fechas y las versiones fijadas se registran en [decisiones.md](decisiones.md).

- Tauri usa el WebView2 que proporciona Windows; no se presupone un Chromium empaquetado propio. Servicio local con cola y almacenamiento persistente; el mecanismo concreto debe impedir acceso no autorizado desde otras páginas o procesos.
- Fechas e importes se almacenan de forma normalizada, conservando también la evidencia original. Los campos desconocidos son nulos, no valores inferidos. El motor de reglas opera sobre datos validados y una versión explícita de la regla.
- Las credenciales se guardan mediante un almacén protegido del sistema operativo, nunca en texto plano ni registros. Los eventos de auditoría evitan guardar secretos.
- Es necesario concretar versiones, formatos, distribución y licencias de componentes OCR, WebView2, Ollama y modelo antes de empaquetar. La descarga aproximada de 5,2 GB depende de la variante y versión. El rendimiento en el equipo objetivo aún no está medido.
- La conexión al sistema administrativo real y sus permisos quedan fuera de esta primera entrega. El correo real entra por R13 (cambio de alcance del 2026-11-03); su calidad con correo real no está medida.
