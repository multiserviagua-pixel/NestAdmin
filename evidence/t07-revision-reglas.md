# Evidencia T07 (parcial): revisión predeterminada, reglas e historial

Cubre la lógica de R07 y de los flujos 1–3 en `crates/app`, que une la cola durable, la IA, la validación de facturas, los simuladores y las reglas. La ventana (T01) solo llama a esta capa.

## Comportamiento implementado

- Toda tarea con acción (`register_invoice`, `send_draft`) termina en `awaiting_review` salvo que **exactamente una** regla activa coincida y no haya bloqueos: incidencias o campos ilegibles en una factura, o un borrador sin destinatario.
- Condiciones admitidas: para registrar, proveedor (sin distinguir mayúsculas) y total máximo opcional, comparado como decimal; para enviar, la categoría. Una regla con condiciones no válidas se rechaza al crearla.
- Las reglas se crean inactivas; cada cambio crea una versión nueva y una fila en el historial. La automatización guarda la regla y su versión como autorización.
- La aprobación indica la versión vista. Editar un borrador crea una versión nueva; aprobar la anterior se rechaza (`StaleVersion`) y no se envía nada.
- La acción se ejecuta mediante `Store::perform_action` con clave `task-{id}/{acción}/v{versión}`. El borrador previo al envío usa su propia clave por versión, así que repetir tras un corte no crea otro.
- La llamada al modelo se hace sin el cerrojo de la aplicación (`begin_next` → `Job::run` → `finish`).

## Cómo reproducir

```sh
cargo test -p nestadmin-app
cargo test -p nestadmin-app --test ollama -- --ignored --nocapture   # requiere Ollama con qwen3:8b
```

## Ejecución registrada

- Fecha: 2026-11-03
- Plataforma: Windows 11 Pro 10.0.26300, Intel Core Ultra 5 125U, 16 GB
- Rust 1.99.1 (MSVC); Ollama 0.40.14 con `qwen3:8b`
- Resultado: 10 pruebas superadas con proveedor simulado; la prueba con Ollama local, superada en 76 s.

| Prueba | Qué comprueba |
| --- | --- |
| `invoice_waits_for_review_and_registers_once_after_approval` | Sin regla, la factura espera revisión y no registra; aprobada, registra una vez; aprobarla otra vez es error y no repite el registro. |
| `issues_and_duplicates_are_reported_by_code` | `total_mismatch` y `possible_duplicate` calculados por código, con el candidato a duplicado. |
| `an_active_matching_rule_automates_only_clean_invoices` | Regla inactiva → revisión; activa → registro automático con autorización `rule` y versión; incidencias, otro proveedor o dos reglas coincidentes → revisión. |
| `invalid_rules_are_rejected` | Sin proveedor, importe sin dos decimales, categoría desconocida o acción desconocida. |
| `mail_is_fetched_once_and_edited_drafts_need_new_approval` | Consultar dos veces no duplica tareas; editar crea la versión 2; aprobar la 1 se rechaza sin enviar; aprobar la 2 envía una vez con la clave `v2` y el texto editado. |
| `a_draft_without_sender_is_never_sent_by_a_rule` | Con una regla de envío activa, el mensaje sin remitente queda en revisión. |
| `model_failure_is_visible_and_retry_requeues` | Fallo del proveedor → tarea fallida con código `unavailable` y sin resultado; reintento explícito. |
| `empty_documents_and_unknown_tasks_are_rejected` | Documento vacío y aprobación de una tarea inexistente. |
| `restart_recovers_work_without_duplicate_effects` | Tarea en curso al cerrar → vuelve a la cola al reabrir y se procesa; ningún registro repetido; el registro de duplicados se reconstruye tras reiniciar. |
| `audit_log_records_actions_and_rules` | El historial contiene la regla y el registro confirmado. |
| `document_and_mail_flow_with_local_ollama` (ignorada por defecto) | Con Ollama real: factura sintética extraída (total 121.00) y en revisión; aprobada, registrada; correo clasificado con borrador en revisión y sin envíos. |

## Recorrido desde la ventana (D48)

`crates/desktop/src/recorrido.rs` arranca la aplicación de escritorio real con el runtime simulado de Tauri: la misma `configure` que `main` registra comandos y eventos de ventana, la misma `start` abre datos, cola e hilo de trabajo, y una ventana `main` llama a los comandos por IPC con los nombres y argumentos de `ui/src/api.ts` (incluidos `actionKind` en camelCase y el cuerpo binario de `import_file`). El proveedor es el adaptador real de Ollama, elegido en `configuracion.json`, contra un Ollama simulado en 127.0.0.1; correo y administración son los simuladores; credenciales en memoria; sin bandeja, sin `%LOCALAPPDATA%\NestAdmin` ni el Ollama del equipo. Datos sintéticos (dominio `.test`) en carpetas temporales.

```powershell
cargo test -p nestadmin-desktop --locked recorrido::
```

- Fecha: 2026-11-05
- Plataforma: Windows 11 Pro 10.0.26300, Intel Core Ultra 5 125U, 16 GB; Rust 1.99.1 (MSVC), Tauri 2.12.5 (`tauri::test`)
- Resultado: 7 superadas, 0 fallidas (1 ignorada: `proceso_hijo`, que solo se lanza como proceso hijo); repetida 3 veces seguidas, entre 3 y 16 s. CI la repite en `windows-latest` (en Linux, `nestadmin-desktop` se excluye como el resto de pruebas de escritorio).

| Prueba | Qué recorre |
| --- | --- |
| `la_interfaz_solo_llama_a_comandos_registrados` | Los comandos que invoca `api.ts` son exactamente los registrados en `configure`. |
| `documento_importado_se_revisa_y_se_registra_una_sola_vez` | Importar dos facturas → procesar → una sin incidencias y otra con `total_mismatch`; nada registrado sin aprobación; descartar una (`cancelled`, sin acciones); aprobar la versión vista registra una vez; repetir la aprobación es error; historial con `confirmed` y autorización «aprobación». |
| `correo_simulado_borrador_editado_se_envia_una_sola_vez` | Consultar el correo (informe por cuenta del simulador); consultar otra vez no duplica tareas; borrador en revisión; editarlo crea la versión 2; aprobar la 1 se rechaza sin enviar; aprobar la 2 envía una vez el texto editado. |
| `regla_expresa_se_activa_y_desactiva_desde_la_ventana` | Crear regla (inactiva; una regla sin proveedor se rechaza) → la factura espera revisión; activarla → la siguiente se registra sola con la regla y su versión como autorización; otro proveedor o una incidencia siguen en revisión; desactivarla → vuelve la revisión; versiones crecientes e historial con la regla. Activar no ejecuta la factura que ya esperaba. |
| `fallo_del_modelo_es_visible_y_el_reintento_es_explicito` | Ollama responde 500 → tarea `failed` con código y sin resultado, sin reintento automático; elegir Groq sin clave se rechaza y la selección local se mantiene; «Reintentar» la procesa (2 intentos). |
| `salir_por_completo_conserva_la_cola_y_reabrir_la_retoma` | Proceso hijo: registra una factura, detiene la cola como «Salir por completo», importa otra (queda en cola sin llamar al modelo) y termina con código 0. Al reabrir, la pendiente se procesa y el registro no se repite. |
| `interrumpir_el_proceso_no_repite_efectos` | Procesos hijos terminados con `TerminateProcess`: con la llamada al modelo en curso, con la factura en revisión y justo tras aprobarla. Al reabrir: la tarea vuelve a la cola con 2 intentos, la revisión se conserva, la aprobada sigue completada, un único registro y un único `confirmed`, y no se vuelve a llamar al modelo. Base y simuladores quedan libres tras cada corte. |

Límites: no se ejecuta el JavaScript de `ui/` ni WebView2 ni se pulsan botones (D48); el corte entre el efecto externo y su confirmación se prueba en el núcleo y los simuladores ([T02](t02-nucleo.md), [T06](t06-simuladores.md)), no con el proceso completo.

## Pendiente

- El mismo flujo con la ventana WebView2 (clics y pintado) solo se ha visto a mano; no hay prueba automática de la interfaz gráfica.
- La moneda no forma parte de la extracción: se registra como ausente.
- La importación de PDF y documentos escaneados se prueba en [T04](t04-pdf-ocr.md).
