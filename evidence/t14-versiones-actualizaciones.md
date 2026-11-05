# Evidencia T14 (parcial): versiones, licencias y actualizaciones

Cubre R14 (D60–D64, [política](../specs/v1/actualizaciones.md)). Datos sintéticos; ninguna prueba toca `%LOCALAPPDATA%\NestAdmin` ni instala nada.

## Ejecución registrada

- Fecha: 2026-11-05
- Plataforma: Windows 11 Pro 10.0.26300 (equipo de desarrollo)
- Rama: `claude/fase-9i-versiones-actualizaciones-8fad19`

| Comprobación | Comando | Resultado |
| --- | --- | --- |
| Herramientas en las versiones fijadas, sin cambiar nada | `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/setup-dev.ps1 -Comprobar` | Python 3.12.10, Node v26.13.0, rustup 1.29.2, rustc 1.99.1, Build Tools 2022 con MSVC x64 y SDK, WebView2 156.0.4314.58: todo «correcto», código 0. El modo de instalación **no** se ha ejecutado en este equipo ni en uno limpio. |
| Inventario de terceros al día | `python scripts/third_party.py --check` | Al día: 283 crates enlazados para `x86_64-pc-windows-msvc`, 1 paquete npm de producción (`@tauri-apps/api`). |
| Textos de licencia para el instalador | `python scripts/third_party.py --textos <temporal> --vendor <vendor de desarrollo>` | 212 textos distintos (unos 800 KB); 5 crates sin archivo propio (`alloc-stdlib`, `selectors`, `webview2-com`, `webview2-com-macros`, `webview2-com-sys`), que figuran con su licencia declarada. Sin `vendor/` falla con código 1 y nombra las licencias del OCR que faltan. |
| Generador | `python -m unittest discover -s scripts/tests` | 13 pruebas correctas, 6 de ellas del inventario: solo dependencias normales y externas, textos idénticos una sola vez, faltas comunicadas, npm de desarrollo excluido, tablas escapadas y DLL iguales a las que copia `setup-ocr.ps1`. |
| Copia previa a migrar y esquema más reciente (D61) | `cargo test -p nestadmin-core --test schema` | 3 pruebas correctas: base nueva sin copia; base de esquema 1 migrada con copia `nestadmin.esquema-1.db` intacta y sin `.part`; restaurarla y abrirla de nuevo funciona; un esquema más reciente se rechaza sin modificarse y el error nombra la copia. |
| Digest del modelo (D62) | `cargo test -p nestadmin-setup` | El caso de equipo limpio informa `model_pinned: true`; `model_with_other_digest_is_reported_without_blocking`: otro digest se avisa en el paso `model`, la preparación termina y no se vuelve a descargar. |
| Digest real fijado | `GET http://127.0.0.1:11434/api/tags` y SHA-256 de `registry.ollama.ai/v2/library/qwen3/manifests/8b` | Ambos `500a1f067a9f782620b40bee6f7b0c89e17ae61f686b92c24933e4ca4b2b8b41`, igual que en T10. |
| Versión mínima de Ollama al abrir (D64) | pruebas existentes de `startup` en `crates/setup` (T08c) | Sin cambios: un Ollama anterior a 0.9.0 abre el asistente. |

Resto del espacio de trabajo: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings` y `cargo test --workspace --locked` (resultado en la descripción del PR).

## Pendiente

- Generar el instalador con `THIRD_PARTY_NOTICES.txt` y `pdfium/licenses/` y comprobar que se instalan (no se ha empaquetado en esta rama: hay una prueba del instalador en curso en este equipo).
- Ejecutar `scripts/setup-dev.ps1` sin `-Comprobar` en una máquina limpia.
- Resolver la redistribución de `libjbig-0.dll` (GPL) e `libiconv-2.dll` (LGPL) y confirmar las licencias de las DLL con los paquetes de la build.
- Actualizador firmado (D63): sin implementar hasta tener la clave del editor; sus casos están en la matriz.
