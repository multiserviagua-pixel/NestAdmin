# Evidencia T25 (parcial): rediseño de la interfaz

Cubre la legibilidad de R01 y D136–D138. Todos los datos son sintéticos (`ui/src/mock.ts`, dominio `.test`).

## Cómo reproducir

```sh
cd ui && npm ci && npm run build && npm test
npx vite --port 5181
```

La vista previa admite `?muchos` para una lista grande y los parámetros anteriores (`?sin-ia`, `?preparar`, etc.).

## Resultados

| Comprobación | Método | Resultado |
|---|---|---|
| Tipos y compilación | `npm run build` | Pasa |
| Contraste AA de los tokens, en claro y oscuro; mínimo de 12 px | `npm test` (`tests/tokens.test.mjs`) | Pasa |
| Pestañas, orden, marcas con su porqué, ausencia explícita, siguiente al resolver | `npm test` (`tests/pendientes.test.mjs`) | Pasa |
| Flechas en lista y pestañas, sin perder el foco | Vista previa, por script | Pasa |
| El desplazamiento de la lista se conserva al seleccionar | Vista previa con `?muchos`, por script | Pasa |
| Al descartar se pasa a la siguiente | Vista previa con `?muchos`, por script | Pasa |
| «Aprobar» bloqueado con importes dudosos y motivo escrito | Vista previa, por script | Pasa |
| Un segundo clic rápido en la barra no resuelve la tarea siguiente; uno deliberado sí | Vista previa con `?muchos`, por script | Pasa |
| Sin desbordamiento horizontal en las 7 vistas | Vista previa a 1180 px, por script | Pasa |
| Aspecto de Pendientes, Tareas, Cuentas de correo, Configuración y Preparación | Capturas a 860 o 1180 px | Revisado a ojo |

Capturas: [Pendientes en oscuro a 860 px](t25/pendientes-oscuro-860.jpg), [Correos](t25/pendientes-correos.jpg), [Tareas](t25/tareas.jpg), [Cuentas de correo](t25/cuentas-860.jpg).

## Sin comprobar

- **Windows real:** la ventana con WebView2 no se ha recorrido a mano ni está automatizada. Por eso T25 sigue parcial.
- **Lectores de pantalla:** los roles ARIA se han revisado por código; no se ha probado con Narrador.
- **Auditoría automática:** la herramienta Axe no se ha podido ejecutar en esta sesión; el contraste se comprueba con la prueba propia, solo sobre los pares de tokens, no sobre cada pantalla.
- **Comparación con usuarios:** la investigación se basó en documentación de productos, no en pruebas de uso.
