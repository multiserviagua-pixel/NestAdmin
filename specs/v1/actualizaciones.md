# Política de actualizaciones v1

Concreta R14 y el flujo 7 de [requisitos](requisitos.md); las decisiones son D60–D64 de [decisiones.md](decisiones.md). Regla común: **nada se actualiza sin que el usuario lo acepte, nada se descarga en segundo plano y ninguna actualización cambia de proveedor de IA**. Estado: la copia previa a migrar, la versión mínima de Ollama al abrir y el digest del modelo están implementados y probados; el actualizador de NestAdmin es una propuesta sin implementar porque necesita una clave de firma que solo debe tener el editor.

## Qué se actualiza y cómo

| Componente | Cómo se actualiza | Quién decide | Estado |
| --- | --- | --- | --- |
| NestAdmin (aplicación, PDFium, Tesseract, datos de idioma, avisos de licencia) | Versión nueva completa del instalador, firmada y publicada como *release* de GitHub (D63) | El usuario, al abrir, acepta o pospone | Propuesta |
| Base de datos local | Migraciones numeradas al arrancar, con copia previa (D61) | Automático tras instalar una versión aceptada | Implementado |
| Ollama | No lo actualiza NestAdmin. Al abrir se comprueba la versión mínima (0.9.0); si es anterior, la ventana abre el asistente, que ofrece instalar la 0.40.14 fijada (D39, D64) | El usuario, desde el asistente | Implementado |
| Modelo `qwen3:8b` | Fijado por etiqueta y digest; nunca se cambia solo (D62) | Decisión del proyecto con evaluación nueva (R10, R12) | Implementado |
| WebView2 | Lo mantiene Windows (Evergreen) | Windows | Sin acción de NestAdmin |
| Herramientas de desarrollo | `scripts/setup-dev.ps1` con versiones fijadas (D60) | El equipo de desarrollo | Implementado |

## NestAdmin: propuesta con el actualizador de Tauri (D63)

1. **Publicación.** Cada versión sube a una *release* de GitHub el instalador NSIS, su firma (`.sig`) y un `latest.json` con versión, notas, fecha y, por plataforma, la URL del instalador y su firma. La versión es la de `crates/desktop/tauri.conf.json` y sigue SemVer.
2. **Clave de firma.** El actualizador de Tauri exige un par de claves minisign propio, distinto de la firma Authenticode del instalador: `tauri signer generate -w <ruta fuera del repositorio>`. **La genera y la guarda el editor**, fuera del repositorio y de este equipo de pruebas (por ejemplo, en un gestor de contraseñas y con copia sin conexión), protegida con contraseña. La clave **pública** va en `tauri.conf.json` (`plugins.updater.pubkey`); la privada solo se usa al empaquetar mediante `TAURI_SIGNING_PRIVATE_KEY` y `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`. Si se pierde, las instalaciones existentes no aceptarán versiones nuevas y habrá que reinstalar a mano; si se filtra, alguien podría publicar una actualización aceptada. `.gitignore` ya excluye `*.key`.
3. **Comprobación al abrir.** Con `tauri-plugin-updater`, una consulta a `https://github.com/multiserviagua-pixel/NestAdmin/releases/latest/download/latest.json` tras arrancar, en un hilo aparte, sin proxy implícito (como D12) y sin enviar datos del usuario. El repositorio es privado: antes de activarlo, las *releases* deben publicarse en un repositorio o URL accesible sin credenciales. Sin red o con un `latest.json` ilegible no se avisa de nada y se registra el motivo.
4. **Aviso y aceptación.** Si hay una versión mayor, la ventana muestra versión y notas con «Actualizar ahora» y «Más tarde». Posponer no vuelve a avisar hasta el siguiente arranque. No hay actualización silenciosa.
5. **Nunca a mitad de una tarea.** Aceptar solo descarga; instalar exige que la cola no tenga tareas en curso. Si las hay, se espera a que terminen (las nuevas no empiezan) o el usuario cancela. Antes de instalar, la aplicación cierra la cola como en una salida completa, de modo que lo pendiente se recupera sin duplicar efectos (R02). *Implementado en la capa de aplicación (2026-11-06):* `App::set_queue_paused` impide empezar tareas nuevas sin perder las que esperan, y `App::update_readiness` dice si hay tareas ejecutándose. La pausa no se guarda, así que tras reinstalar la cola continúa. Prueba `flows::an_update_waits_for_running_work_and_keeps_the_queue`. El actualizador que lo usará (D63) sigue sin implementar.
6. **Instalación.** El plugin comprueba la firma minisign con la clave pública incluida antes de ejecutar nada; una firma no válida descarta la descarga. Ejecuta el instalador NSIS en modo pasivo por usuario, que sustituye los archivos del programa y vuelve a abrir NestAdmin. El paso `--preparar` del instalador (D36) se repite y solo actúa si falta algo.
7. **Migración de datos.** Al arrancar, la versión nueva copia `nestadmin.db` como `nestadmin.esquema-N.db` antes de aplicar migraciones (D61).
8. **Vuelta atrás.** Instalar a mano el instalador de la versión anterior (las *releases* se conservan). Esa versión se niega a abrir una base de esquema más reciente y su mensaje indica qué copia restaurar: con NestAdmin cerrado por completo, sustituir `nestadmin.db` por `nestadmin.esquema-N.db` (y borrar `nestadmin.db-wal` y `nestadmin.db-shm` si quedaran). Lo hecho después de actualizar no está en esa copia.

### Pasos para implementarlo

1. El editor genera la clave y la guarda fuera del repositorio (punto 2); comunica solo la pública.
2. Añadir `tauri-plugin-updater` (y `tauri-plugin-process` para reiniciar) con versiones fijadas, su permiso en `capabilities/` limitado a la ventana principal, `bundle.createUpdaterArtifacts: true` en `tauri.installer.json` y `plugins.updater` (`pubkey`, `endpoints`, `windows.installMode: "passive"`) en `tauri.conf.json`.
3. Comando IPC que compruebe la versión, otro que descargue y otro que instale solo si la cola está vacía; pruebas con un servidor simulado en 127.0.0.1 que sirva un `latest.json` y un instalador sintético firmados con una clave de prueba generada en la propia prueba (nunca la del editor): firma válida, firma inválida, sin red, versión igual o menor, tarea en curso.
4. Flujo de publicación: empaquetar con las variables de la clave en la máquina del editor (o un secreto de CI de GitHub), subir instalador, `.sig` y `latest.json` a la *release*.
5. Añadir el caso a la matriz (T14) y ensayarlo en Windows con dos versiones reales.

## Ollama (D64)

NestAdmin nunca actualiza Ollama por su cuenta: puede compartirlo con otras aplicaciones y el instalador de Ollama tiene su propio aviso de actualización. Al abrir con Ollama elegido (D39) se comprueba la versión del servicio: si es anterior a la mínima (0.9.0, por `think` y la salida con esquema), la ventana abre el asistente y el usuario decide si instala la 0.40.14 fijada (SHA-256 y firma de Ollama Inc., D37). Una versión posterior a la fijada se reutiliza. Subir la mínima o la fijada es una decisión nueva con su prueba (T08b).

## Modelo (D62)

`qwen3:8b` está fijado por etiqueta y por el digest de su manifiesto (`500a1f06…b41`, el evaluado en T10). Ollama comprueba al descargar la huella de cada capa. Si la etiqueta se republicara con otro contenido, la preparación lo indica («digest distinto del evaluado») y el informe lleva `model_pinned: false`, pero no bloquea ni descarga otra cosa. Cambiar de modelo, de cuantización o aceptar un digest nuevo es una decisión registrada con una nueva ejecución del corpus (R10) y, cuando sea posible, medición en el equipo objetivo (R12).

## OCR y PDF

Tesseract, sus DLL, los datos de idioma y PDFium viajan en el instalador (D38) y se actualizan solo con una versión nueva de NestAdmin: no hay descarga aparte. Cambiar su versión exige actualizar `scripts/setup-ocr.ps1` (versión y SHA-256), la tabla de versiones, el inventario de `scripts/third_party.py` y repetir T04.

## WebView2

NestAdmin usa el WebView2 Evergreen que Windows mantiene actualizado; el instalador solo ejecuta el *bootstrapper* de Microsoft si falta. No se fija una versión ni se empaqueta un runtime propio.
