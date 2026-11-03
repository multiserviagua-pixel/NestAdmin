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

## Pendiente

- Restaurar y salir **desde el menú del área de notificación** no se ha automatizado (requiere interactuar con la bandeja de Windows); falta comprobarlo a mano y confirmar que el proceso termina.
- Cerrar la ventana **con una tarea en curso** y comprobar que termina igualmente.
- Construir con `tauri build` (sin el servidor de desarrollo) forma parte del instalador (T08).
