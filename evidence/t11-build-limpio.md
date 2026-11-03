# Evidencia T11 (parcial): construcción desde un clon limpio

Cubre R11. `scripts/build-clean.ps1` clona el repositorio en una carpeta nueva, con su propio directorio de compilación. Allí ejecuta, en orden, los pasos documentados en README.md:

1. `setup-ocr.ps1`
2. `check.py` y las pruebas del verificador y del evaluador
3. `npm ci` y la compilación de la interfaz
4. `cargo fmt`, `clippy -D warnings` y `cargo test`
5. El instalador NSIS

Al terminar comprueba la separación de resultados. El árbol de trabajo del clon debe seguir limpio según `git status`, es decir, todo lo generado está ignorado. Los resultados y predicciones de evaluación y los binarios descargados deben estar ignorados por Git. Las pruebas no deben haber creado la carpeta de datos del usuario `%LOCALAPPDATA%\NestAdmin`. Cualquier fallo detiene el guion con el paso que falló.

## Cómo reproducir

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/build-clean.ps1
# Opciones: -Source <ruta o URL> -Ref <rama> -Destination <carpeta nueva> -SkipInstaller
```

## Ejecución registrada

- Fecha: 2026-11-03
- Plataforma: Windows 11 Pro 10.0.26300, Intel Core Ultra 5 125U, 16 GB
- Origen: commit `5f3b8a6` (`main` tras fusionar la fase 7), clonado con `git clone --no-local`
- Herramientas: Git 2.56.0, Python 3.12.10, Node.js 26.13.0, npm 11.20.0, rustup 1.29.2, Rust 1.99.1 (MSVC)

| Paso | Segundos | Resultado |
| --- | ---: | --- |
| Versiones de herramientas | 3 | Las fijadas en `.node-version` y `rust-toolchain.toml`. |
| `scripts/setup-ocr.ps1` | 60 | Descargó de nuevo PDFium `chromium/8125` y los datos de idioma `spa` y `eng`, con SHA-256 comprobado. Tesseract ya estaba instalado y se reutilizó. |
| `scripts/check.py` y pruebas del verificador y del evaluador | 2 | Correctas. |
| `npm ci` y `npm run build` | 10 | Correctas. |
| `cargo fmt --check` | 2 | Sin cambios. |
| `cargo clippy --workspace --all-targets --locked -D warnings` | 149 | Sin avisos. |
| `cargo test --workspace --locked` | 147 | 83 superadas, 0 fallidas. 7 ignoradas porque necesitan Ollama o el equipo real; se ejecutan aparte (T04, T08, T10). |
| `tauri build --config tauri.installer.json` | 276 | `NestAdmin_0.1.0_x64-setup.exe`, 21 917 489 bytes, SHA-256 `4F9C676B…A7A571EB`. |
| Separación de resultados | 0 | Árbol de trabajo limpio. `evaluation/results/`, `evaluation/predictions/` y `vendor/` ignorados. `%LOCALAPPDATA%\NestAdmin` no se creó. |

Total: unos 11 minutos.

## Limitaciones

- Es un clon limpio en el equipo de desarrollo, no un equipo limpio. Python, Node.js, Rust, MSVC y Tesseract ya estaban instalados (`scripts/setup-dev.ps1` y `setup-ocr.ps1` los reutilizan), y se aprovecharon las cachés de descargas de Cargo y npm. Falta ejecutarlo en una máquina virtual de Windows sin herramientas, empezando por `scripts/setup-dev.ps1`.
- El instalador no es reproducible byte a byte. Su SHA-256 difiere del generado en T08 a partir del mismo código, y la causa no se ha investigado; es probable que sean las marcas de tiempo de la compilación o del empaquetado.
- La separación se comprueba por las reglas de Git y la ausencia de la carpeta de datos del usuario. El código de la aplicación no escribe en `evaluation/`, y la evaluación no escribe en la base de datos (D03); eso último se apoya en las pruebas existentes, no en este guion.
