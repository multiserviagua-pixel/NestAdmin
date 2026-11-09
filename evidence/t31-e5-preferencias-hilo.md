# T31 (E5) — Preferencias por remitente y estado del hilo

Pruebas automáticas con proveedor y correo simulados y datos sintéticos, en Windows, más un recorrido de la interfaz en la vista previa del navegador (`ui-main`, puerto 5182) con datos sintéticos. No hay recorrido en `tauri dev`, ni prueba con una cuenta real de Gmail, ni consulta del hilo en Graph.

## Qué se comprueba

- `crates/core/src/mail_signals.rs`: `decide_follows_the_precedence_table`, una fila por caso de la precedencia de D161 (rebote, hilo propio, preferencia `informative` y `review`, señal fuerte, débil y ninguna) y sus combinaciones: preferencia `informative` más rebote es rebote; hilo propio más preferencia `review` es informativo por el hilo; `review` más señal fuerte va a la IA; hilo desconocido o ajeno no cambia nada.
- `crates/core/tests/schema.rs`: migración 006 desde el esquema 5 con copia previa, tabla vacía, y los `CHECK` rechazan ámbito, ruta o clave sin normalizar.
- `crates/core/tests/sender_preferences.rs`: claves normalizadas (de `Nombre <a@b>` a `a@b`, dominio con `@` o desde una dirección), un evento del historial por cambio real en la misma transacción y sin más campos que ámbito, clave, ruta y la anterior; un fallo del evento no deja la preferencia.
- `crates/app/tests/preferences.rs`:
  - claves normalizadas, repetir no escribe otro evento, entradas inválidas rechazadas, quitar;
  - dominios públicos rechazados por dominio (y una dirección de ese servicio, admitida);
  - aviso (sin bloquear) si contradice una regla activa de envío por dirección o dominio; las inactivas, archivadas o de otro dominio no avisan, y `review` no avisa;
  - una prueba por fila en `fetch_mail`: primera lectura (la tarea de lectura no cambia y no consulta el hilo), rebote (sin consulta del hilo y sin cambio por preferencia o hilo), hilo propio (informativo `already_replied`, sin IA, también contra `review`), preferencia `informative` por dirección y por dominio (sin IA, `triage.source = "preference"`), dirección antes que dominio, `review` con señal fuerte (pasa por la IA, queda en revisión, la regla que coincidiría no envía y no hay acciones), señal fuerte y IA como antes;
  - la compuerta no cambia: con `review` el correo limpio sigue saliendo por regla y el automático queda en revisión con `automatic_sender`;
  - hilo desconocido, ajeno, con error de red o de credenciales y sin clave de hilo: flujo normal, nada oculto;
  - espía de la IA: ninguna petición lleva el texto de otro mensaje del hilo, ni la clave del hilo ni su estado; con hilo propio no sale ninguna;
  - corte al cerrar por preferencia o por hilo: no queda tarea a medias, no llega a la IA, repetir la consulta no duplica;
  - cambiar una preferencia no toca las tareas ya creadas.
- `crates/mail/tests/gmail.rs`: `threads.get` solo con `format=metadata` y la cabecera propia (ningún `format=full`, ningún mensaje suelto, ninguna escritura); el último mensaje se decide por `internalDate`, ignora borradores y papelera, cae al orden de la lista sin fechas; 404, 200 sin mensajes o solo borradores es desconocido; 500 y 401 son error, no un valor.
- `crates/mail/tests/graph.rs`: el estado del hilo es siempre desconocido y no hace ninguna llamada.
- `ui/tests/preferences.test.mjs`: normalización igual que el backend, dirección antes que dominio, qué se ofrece, dominio público solo por dirección, textos de la confirmación (el efecto y que solo afecta a lo nuevo), aviso de regla contradicha, líneas del Historial y los motivos «Ya respondió en este hilo» y de preferencia.
- Vista previa (`ui-main`): Informativos con el motivo «Ya respondió en este hilo» y el de preferencia; «Siempre a Pendientes» en el detalle de un informativo y «Siempre a Informativos» en un correo en revisión (no en un rebote); confirmación con el efecto explicado y la elección entre dirección y dominio; nota «Su preferencia» con «Quitar»; lista en Cuentas de correo; entrada legible en el Historial; sin errores en consola.

## Límites

- Graph: `last_message_is_ours` queda en desconocido. Filtrar `/me/messages` por `conversationId`, ordenar por fecha y decidir qué es «propio» (dirección o carpeta de enviados, borradores) no se ha podido comprobar contra un buzón real, y un valor inventado ocultaría correo. Con desconocido el correo sigue el flujo normal.
- Gmail: la consulta del hilo está probada contra un servidor simulado con la forma de `threads.get`, no contra una cuenta real.
- Una consulta del hilo por cada correo nuevo que no sea un rebote: una llamada de metadatos más por correo en Gmail. Un fallo se trata como desconocido.
- Las preferencias solo cambian los correos nuevos. No hay recorrido en `tauri dev` ni en WebView2.
