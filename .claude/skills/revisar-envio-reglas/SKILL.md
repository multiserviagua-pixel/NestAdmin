---
name: revisar-envio-reglas
description: Lista de revisión para cambios de NestAdmin que tocan envío, reglas, aprobación, cola, instrucciones de IA o lo que sale hacia un proveedor externo. Úsela antes de dar por terminado uno de esos cambios.
---

# Revisar cambios de envío y reglas

Reglas que no cambian: pruebas manuales sobre `tauri dev`; instalador solo para la versión final o si el usuario lo pide; git solo local, sin push ni PR; fixtures siempre sintéticos.

Lea el diff entero y compruebe cada punto en el código. Si no puede comprobar uno, dígalo; no lo dé por bueno.

## Fallo cerrado

- Un dato ausente, antiguo o sin comprobar nunca permite que una regla envíe. Pruebe tareas guardadas antes del cambio.
- Ninguna preferencia ni dato del correo desbloquea una regla (D158, D161).
- Los envíos y registros requieren revisión salvo una regla expresa activa (R07).

## Efectos sin duplicar

- ¿Un corte entre dos transacciones puede repetir un envío, una tarea o una llamada a la IA? Exija una prueba de corte.
- Clave de idempotencia y versión del borrador intactas (D41, D156).
- Las transiciones de `crates/core/src/queue.rs` no reabren tareas finales.

## Evidencia

- D125: el motivo no lleva contenido del correo.
- D126: remitente obligatorio donde corresponda.
- D129: Archivar no cuenta como rechazo y la evidencia no impide la acción.

## Datos que salen (D101, D102)

- Exija una prueba espía de qué sale en cada llamada, también en el camino de fallo.
- Respaldo solo del externo elegido a la IA local; nunca hacia otro externo ni de local a externo, tampoco entre dos llamadas de una misma tarea.
- Perfil solo en la redacción y con consentimiento vigente (D163).

## Inyección (D160)

- Ejecute la suite de inyección: un solo envío no autorizado o una filtración es un fallo.
- Que un dato aparezca en el correo no lo avala.

## Cierre

Pase `security-review` y `code-review` en nivel `medium`, y pida a `arquitecto` que revise el diff sin darle su conclusión. Informe de lo que no se ha podido verificar, con la salida literal de cualquier fallo.
