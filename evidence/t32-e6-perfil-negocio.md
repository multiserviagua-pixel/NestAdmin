# T32 (E6) — Perfil del negocio

Pruebas automáticas con proveedor espía y correo simulados y datos sintéticos, en Windows, más un recorrido de la interfaz en la vista previa del navegador (`ui-main`, puerto 5182) con datos sintéticos. No hay recorrido en `tauri dev`, ni proveedor real, ni medición con un modelo.

## Qué se comprueba

- `crates/core/tests/schema.rs`: migración 007 desde el esquema 6 con copia previa (`nestadmin.esquema-6.db` sin la tabla), fila única, un evento del historial por guardado o borrado sin el texto, y el `CHECK` rechaza más de 2.000 caracteres en total y una segunda fila.
- `crates/app/tests/profile.rs` (proveedor espía que guarda cada petición):
  - el perfil solo va en la llamada de redacción: ni la clasificación ni la extracción de una factura lo llevan;
  - sin consentimiento un proveedor externo no recibe nada y la redacción sigue funcionando, sin `profile_fp`;
  - el consentimiento cae al cambiar el modelo y no revive al volver al destino anterior; `grant_profile_consent` rechaza modelo, servidor o huella distintos de los vistos;
  - cambiar el contenido lo retira (guardar lo mismo no) y borrar el perfil también;
  - con la IA local se usa sin consentimiento (y conceder uno es un error); el respaldo local lo recibe sin consentimiento y el externo no; con consentimiento y una redacción externa caída, el externo y el local lo reciben y la huella es la misma; si la clasificación ya usó el respaldo, la redacción se queda en local con el perfil;
  - «Redactar respuesta» lleva el perfil igual, y solo con consentimiento;
  - el resultado guarda solo `profile_fp` (16 hexadecimales): ni el centinela, ni el nombre, ni el teléfono ni el horario en el resultado, en `tasks`, en el historial ni en `rule_settings`;
  - la vista previa es literalmente lo enviado: `instrucciones == base + "\n\n" + block`, el destino (proveedor, modelo, servidor) y `fp` coinciden con los de la petición;
  - los contactos del perfil (correo, enlace, teléfono) no se marcan en `draft_flags`; un contacto ajeno sí; sin perfil enviado se marcan todos;
  - guardado: se bloquean IBAN, tarjeta (con y sin separadores), `sk-…`, `contraseña:`, un token largo, una clave privada y el exceso; el tope exacto pasa; teléfono, correo, fecha y una tarjeta que no cumple Luhn no bloquean (teléfono y correo avisan); un perfil en blanco no existe;
  - el examen no se esquiva con escrituras alternativas: tarjeta con espacios no separables, puntos, dobles espacios, ` - `, guion suave o cifras de ancho completo; IBAN con otra separación o en minúsculas; `sk-` partido por un carácter invisible;
  - durante el periodo de 10 minutos de D101 (con y sin consentimiento): la IA local va primero y recibe el perfil, el externo solo lo recibe con consentimiento, `profile_fp` solo consta si el perfil salió hacia quien redactó, y si fallan los dos la tarea no llega a revisión, sin borrador ni huella y sin envío; ninguna clasificación lleva el perfil;
  - un perfil que ya no pasa el examen (base editada a mano con un IBAN) no se envía y la vista previa lo dice;
  - precarga: nombre, dirección y correo de `company`, sin NIF ni IBAN, y no guarda nada;
  - un correo informativo se cierra en local sin llamar a ningún proveedor aunque haya perfil y consentimiento.
- `crates/ai/tests/triage_tasks.rs`: `profile_section` es la única función del bloque (petición y vista previa), y `draft_flags_allowing` admite los contactos del perfil y marca las variantes (`xhola@…`, `….test.evil.test`, `….test@evil.test`, `www.` delante, otro teléfono por una cifra, un IBAN aunque esté en el perfil).
- `ui/tests/profile.test.mjs`: contador por carácter con punto de millar y tope, cuándo se puede guardar, precarga que no pisa lo escrito, textos de bloqueos y avisos, aviso al guardar, frase «Se enviará a {proveedor} ({modelo}) al redactar respuestas», estado y acciones ofrecidas por caso y texto del consentimiento.
- Vista previa (`ui-main`, Configuración > Perfil del negocio): aviso fijo, precarga en los campos vacíos, contador, bloqueo por IBAN con su mensaje, guardado con avisos de teléfono y correo, bloque literal, recuadro de consentimiento con el destino nombrado, «Autorizado», retirada tras cambiar un campo («vuelva a autorizarlo»), «Retirar autorización» y «Borrar perfil»; sin errores en consola.

## Límites

- «Si un dato no consta, el borrador lo dice» descansa en la instrucción ya existente de `draft_request`; no se ha medido con un modelo real. Con la suite actual solo se comprueba que la instrucción sigue ahí y que el perfil es la única fuente que se le ofrece.
- El consentimiento y el contenido se comprueban al tomar la tarea: una retirada o un cambio durante una llamada ya en curso no la alcanza.
- Un corte entre guardar el perfil y retirar el consentimiento deja un consentimiento dormido con la huella anterior: no coincide con el bloque de ahora y no sale nada (probado con la base editada a mano). Al arrancar sin clave guardada, `refresh_provider` llama a `select_provider` con un proveedor en espera y, si no coincide con el consentido, lo retira (falla cerrado; igual que el consentimiento de D134).
- El perfil admite teléfono y correo (se avisa, no se bloquea): el negocio puede querer publicarlos. Una inyección puede pedir su contenido; el riesgo residual de D163 sigue siendo la revisión humana.
- Un IBAN, una tarjeta o una clave que no se reconozcan con las reglas (formatos raros) no se bloquean. El examen es conservador (por ejemplo, una palabra de 32 o más cifras hexadecimales se toma por clave) y puede rechazar texto legítimo.
- Los datos de la empresa aún no tienen pantalla propia en la aplicación (`company` solo existe en la capa de ventas): la precarga devuelve campos vacíos si no hay nada guardado.
- No se ha construido instalador (no procede) ni probado `tauri dev`.
