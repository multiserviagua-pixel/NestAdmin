# Plan de redistribución de Tesseract (D69)

Estado: **propuesta en curso**. La compilación mínima se ha validado en Linux. En Windows (2026-11-07) compila y solo depende de `KERNEL32.dll`, pero el antivirus del equipo de desarrollo bloquea ejecutarla: faltan las pruebas y la comparación del paso 2 antes de cambiar el paquete ([evidencia](../../evidence/t04-pdf-ocr.md)). No es asesoramiento jurídico: la conclusión se apoya en las licencias que declaran los proyectos y debe confirmarla quien publique el instalador.

## Problema

El instalador lleva la compilación de Tesseract 5.4.0 de UB Mannheim (D38): `tesseract.exe` y 26 DLL de MSYS2. Dos de ellas no son permisivas:

| DLL | Proyecto | Licencia | Obligación al redistribuir |
| --- | --- | --- | --- |
| `libjbig-0.dll` | JBIG-KIT | GPL-2.0-or-later | Licencia y código fuente correspondiente. Como Leptonica y libtiff la enlazan, el programa combinado debería distribuirse bajo GPL con todo su código fuente. |
| `libiconv-2.dll` | GNU libiconv | LGPL-2.1-or-later | Licencia, código fuente de la biblioteca y posibilidad de sustituirla. |

`libgcc_s_seh-1.dll` y `libstdc++-6.dll` son GPL-3.0 con la excepción de biblioteca de GCC, que permite distribuirlas, pero hay que incluir sus avisos. `libzstd.dll` admite BSD-3-Clause.

## Por qué ahora es fácil

Tras D67, Tesseract **solo recibe PNG que genera NestAdmin**: las páginas de PDF las rasteriza PDFium y las fotos las decodifica la aplicación con el crate `image`. Tesseract no necesita leer TIFF, JPEG, WebP, GIF ni JPEG 2000, ni datos de idioma comprimidos (libarchive), ni descargar nada (curl). De esas funciones vienen JBIG-KIT (por libtiff), libiconv (por libarchive) y la mayoría de las demás DLL.

## Propuesta: Tesseract mínimo compilado con MSVC

`scripts/build-tesseract.ps1` compila desde fuentes fijadas por commit:

| Componente | Versión | Licencia |
| --- | --- | --- |
| Tesseract | 5.5.0 (`64eab6c4`) | Apache-2.0 |
| Leptonica | 1.85.0 (`63aef18d`), solo PNG y zlib | BSD-2-Clause |
| libpng | 1.6.44 (`f5e92d76`) | libpng-2.0 |
| zlib | 1.3.1 (`51b7f2ab`) | Zlib |

Todo va enlazado en estático con el runtime de MSVC (`/MT`), sin OpenMP, así que no hace falta ni el redistribuible de Visual C++ ni el runtime de GCC. El script falla si `tesseract.exe` depende de alguna DLL que no sea del sistema. El resultado es un solo ejecutable más cuatro licencias permisivas, que solo exigen incluir su aviso, y que ya cubre el inventario de D60.

### Validación hecha (Linux, 2026-11-06)

- Leptonica 1.85.0 y Tesseract 5.5.0 compilados con las mismas opciones (`ENABLE_GIF/JPEG/TIFF/WEBP/OPENJPEG=OFF`, `DISABLE_TIFF/ARCHIVE/CURL=ON`, `OPENMP_BUILD=OFF`, sin herramientas de entrenamiento). Dependencias del ejecutable: solo libpng, zlib y las bibliotecas del sistema.
- `cargo test -p nestadmin-extract --test images -- --include-ignored` con ese ejecutable: 4 de 4 superadas (foto girada con EXIF y dos facturas en una imagen).
- Texto reconocido en las dos imágenes sintéticas: idéntico, carácter a carácter, al de Tesseract 5.3.4 completo de Debian.

## Pasos

1. **Compilar en Windows.** Ejecutar `scripts/build-tesseract.ps1` en el equipo de desarrollo. Comprobar que pasa la verificación de dependencias.
2. **Comparar.** Con `NESTADMIN_TESSERACT=vendor\tesseract-min\tesseract.exe`, ejecutar `cargo test -p nestadmin-extract -- --include-ignored` (PDF escaneado, mixto e imágenes) y comparar el texto con el de la compilación de UB Mannheim sobre los mismos documentos sintéticos. Medir el tiempo por página en el equipo objetivo. Sin OpenMP puede ser algo más lento por página, pero la cola procesa un documento cada vez.
3. **Cambiar el paquete.**
   - `scripts/setup-ocr.ps1` deja de instalar Tesseract con winget y de copiar las 26 DLL; llama a `build-tesseract.ps1` y deja el resultado en `vendor\tesseract` (`tesseract.exe` y `licenses\`).
   - `scripts/third_party.py` sustituye la tabla de DLL por los cuatro componentes de arriba.
   - La tabla de versiones de `decisiones.md` pasa a Tesseract 5.5.0 (compilación propia).
   - `installer-hooks.nsh` borra en `NSIS_HOOK_PREINSTALL` las DLL de la versión anterior que queden en `$INSTDIR\tesseract\`. Si no, una actualización dejaría allí las DLL GPL antiguas.
   - CI ejecuta `--check` del inventario, como hasta ahora.
4. **Repetir T04 y T08** con el instalador nuevo, incluida la actualización desde una versión que tenía las 26 DLL (T08-11): no debe quedar ninguna.
5. **Cerrar el riesgo** de `decisiones.md` y la parte de R14 que depende de él.

## Si el paso 1 o el 2 fallan

**Plan B: mantener la compilación de UB Mannheim y cumplir las licencias.** Publicar junto a cada versión del instalador el código fuente correspondiente de JBIG-KIT y libiconv (los paquetes fuente de MSYS2 de las versiones exactas), y el de Tesseract, Leptonica y libtiff. Incluir el texto de la GPL-2.0, la GPL-3.0 y la LGPL-2.1 en `THIRD_PARTY_NOTICES.txt` y una oferta escrita en el instalador. Es viable, pero obliga a conservar y publicar esas fuentes en cada versión y a resolver dónde, porque el repositorio es privado. Por eso es la alternativa, no la propuesta.

**Descartado:** el OCR integrado de Windows (D10). Depende de paquetes de idioma que el usuario tiene que instalar y no se puede fijar ni probar igual.

## Mientras tanto

Instalar NestAdmin en los equipos propios para probar no es redistribuir a terceros. No se debe entregar el instalador a nadie fuera de la organización hasta completar el paso 4 o el plan B.
