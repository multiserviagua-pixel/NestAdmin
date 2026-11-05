# Evidencia T01 (parcial): ventana, área de notificación e instancia única

Cubre la parte de R01 y del flujo 5 que se pudo comprobar de forma automática en Windows. La aplicación de escritorio está en `crates/desktop` (Tauri 2 con el WebView2 del sistema) y la interfaz en `ui/` (Vite y TypeScript). La lógica no depende de la ventana: vive en `crates/app` (ver [T07](t07-revision-reglas.md)).

## Comportamiento implementado

- La ventana tiene icono y paleta propios (`crates/desktop/icons/icon.svg`), título «NestAdmin» y tema claro u oscuro según Windows.
- Cerrar la ventana la oculta (`CloseRequested` → `prevent_close` + `hide`). La cola se procesa en un hilo propio (`nestadmin-cola`), así que el trabajo sigue con la ventana oculta.
- El icono del área de notificación tiene dos acciones: «Mostrar NestAdmin» (también con clic izquierdo) y «Salir de NestAdmin por completo». Salir deja de tomar tareas nuevas y termina el proceso; una tarea interrumpida vuelve a la cola al reabrir (recuperación de T02).
- `tauri-plugin-single-instance`: una segunda ejecución termina y muestra la ventana existente.
- Los comandos solo se exponen a la ventana `main` (capacidad `default`) y la CSP limita los orígenes a la propia aplicación y su IPC.

## Cómo reproducir

```powershell
npm ci --prefix ui
cargo build -p nestadmin-desktop
npm run dev --prefix ui            # en otra consola; la compilación de depuración carga la interfaz desde aquí
$env:NESTADMIN_DATA_DIR = "$env:TEMP\nestadmin-prueba"
$p = Start-Process target\debug\nestadmin.exe -PassThru; Start-Sleep 8; $p.Refresh(); $p.MainWindowTitle
$p.CloseMainWindow(); Start-Sleep 3; $p.Refresh(); $p.HasExited      # False: sigue en ejecución
$s = Start-Process target\debug\nestadmin.exe -PassThru; Start-Sleep 5; $s.HasExited, $s.ExitCode
```

## Ejecución registrada

- Fecha: 2026-11-03
- Plataforma: Windows 11 Pro 10.0.26300, Intel Core Ultra 5 125U, 16 GB
- Rust 1.99.1 (MSVC), Tauri 2.12.5, Node.js 26.13.0, Vite 8.3.5, TypeScript 7.0.2

| Comprobación | Resultado |
| --- | --- |
| Arranque con datos en un directorio temporal | Ventana «NestAdmin»; se crean la base de datos, los archivos de los simuladores y `documentos/`. |
| Cerrar la ventana (`WM_CLOSE`) | El proceso sigue vivo y la ventana principal deja de estar visible. |
| Segunda ejecución | Termina con código 0; la ventana de la primera vuelve a ser visible; queda un único proceso. |
| Interfaz en navegador con datos simulados (solo desarrollo) | Vistas de pendientes, factura con incidencias, correo con borrador, reglas y configuración renderizadas sin errores. |

## Parte automatizada desde la capa de comandos (2026-11-05, D48)

`cargo test -p nestadmin-desktop --locked` ejecuta la aplicación con el runtime simulado de Tauri (ver [T07](t07-revision-reglas.md#recorrido-desde-la-ventana-d48)). Para R01 cubre:

- `salir_por_completo_conserva_la_cola_y_reabrir_la_retoma`: la misma parada que usa «Salir de NestAdmin por completo» (`stop_worker`) detiene la cola; una importación posterior queda en cola sin llamar al modelo y, al volver a abrir, se procesa sin repetir el registro ya hecho.
- `interrumpir_el_proceso_no_repite_efectos`: el proceso se termina con una tarea en curso (llamada al modelo sin responder); al reabrir, la tarea vuelve a la cola con el intento contado. Es lo que ocurre al salir desde la bandeja con trabajo en marcha.

No cubre la bandeja de Windows ni la ventana WebView2: la prueba no crea el icono para no tocar el área de notificación del equipo.

## Lista de comprobación manual (unos 2 minutos)

Con la versión instalada de NestAdmin, en Windows. Anotar fecha, versión y resultado de cada paso.

1. Abrir NestAdmin desde el menú Inicio. Se ve la ventana «NestAdmin» con su icono y, en el área de notificación (junto al reloj; quizá en la flecha «^»), el icono de NestAdmin con la indicación «NestAdmin» al pasar el ratón.
2. Importar un documento de prueba (por ejemplo, un `.txt` sintético) y, mientras aparece «En proceso», cerrar la ventana con la X. La ventana desaparece y el icono sigue en el área de notificación.
3. En el Administrador de tareas (Ctrl+Mayús+Esc) sigue apareciendo `nestadmin.exe`.
4. Clic izquierdo en el icono del área de notificación: la ventana vuelve. La tarea del paso 2 ha avanzado (en revisión o con error visible), no se ha perdido.
5. Cerrar otra vez la ventana. Clic derecho en el icono → «Mostrar NestAdmin»: la ventana vuelve.
6. Abrir NestAdmin otra vez desde el menú Inicio con la ventana visible: no aparece una segunda ventana ni un segundo icono.
7. Clic derecho en el icono → «Salir de NestAdmin por completo». El icono desaparece (puede tardar en irse hasta pasar el ratón por encima) y, en el Administrador de tareas, ya no hay `nestadmin.exe` en unos segundos.
8. Abrir NestAdmin: las tareas anteriores siguen en la lista con su estado.

## Pendiente

- Ejecutar la lista de comprobación manual y registrar el resultado; hasta entonces, restaurar y salir **desde el menú del área de notificación** no tiene evidencia.
- Construir con `tauri build` (sin el servidor de desarrollo) forma parte del instalador (T08).
