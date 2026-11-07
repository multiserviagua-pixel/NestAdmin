#!/usr/bin/env python3
"""Inventario de componentes de terceros de NestAdmin (D60).

Genera de forma reproducible, sin red ni herramientas aparte de cargo:

- ``THIRD_PARTY_NOTICES.md`` (versionado): componentes distribuidos o descargados, crates
  de Rust enlazados en ``nestadmin.exe`` para Windows y paquetes npm de producción, con
  versión y licencia declarada.
- con ``--textos SALIDA``: el mismo inventario seguido del texto completo de las licencias
  (archivos LICENSE/COPYING/NOTICE de cada crate y de cada componente presente en
  ``vendor/``). Es el ``THIRD_PARTY_NOTICES.txt`` que viaja en el instalador.

``--check`` falla si ``THIRD_PARTY_NOTICES.md`` no coincide con lo que se generaría, por
ejemplo tras cambiar ``Cargo.lock``. Las licencias son las que declara cada paquete; no es
una revisión legal.
"""
from __future__ import annotations

import argparse
import json
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
NOTICES = ROOT / "THIRD_PARTY_NOTICES.md"
TARGET = "x86_64-pc-windows-msvc"
ROOT_CRATE = "nestadmin-desktop"
LICENSE_PREFIXES = ("LICENSE", "LICENCE", "COPYING", "NOTICE", "UNLICENSE", "COPYRIGHT")

# Componentes que no salen de Cargo.lock ni de package-lock.json. Cada fila: nombre,
# versión fijada, cómo llega al equipo, verificación, licencia y obligaciones. Cambiar una
# versión aquí exige cambiarla también donde se fija (decisiones.md, «Versiones»).
COMPONENTS = [
    ("PDFium (bblanchon/pdfium-binaries)", "chromium/8125", "En el instalador (`pdfium\\`)",
     "SHA-256 del archivo en `scripts/setup-ocr.ps1`", "BSD-3-Clause; empaquetado MIT; "
     "terceros en `licenses/` (FreeType, ICU, libjpeg-turbo, OpenJPEG, lcms, zlib…)",
     "Incluir `LICENSE` y `licenses/` del paquete"),
    ("Tesseract OCR (build UB Mannheim)", "5.4.0.20240606", "En el instalador (`tesseract\\`)",
     "Versión exacta comprobada por `scripts/setup-ocr.ps1`; winget comprueba el SHA-256 del "
     "instalador", "Apache-2.0", "Incluir `LICENSE`; las DLL de la tabla siguiente tienen "
     "licencias propias"),
    ("Datos de idioma Tesseract (`spa`, `eng`)", "tessdata 4.1.0", "En el instalador "
     "(`tessdata\\`)", "SHA-256 en `scripts/setup-ocr.ps1`", "Apache-2.0", "Incluir aviso"),
    ("Ollama", "0.40.14 (mínima 0.9.0)", "Lo descarga `--preparar` de GitHub si falta",
     "SHA-256 fijado y firma Authenticode «Ollama Inc.» (`crates/setup`)", "MIT",
     "Ninguna para NestAdmin: no se redistribuye, se descarga del editor"),
    ("Qwen3 8B (`qwen3:8b`, Q4_K_M)", "digest `500a1f067a9f782620b40bee6f7b0c89e17ae61f686b92c"
     "24933e4ca4b2b8b41`", "Lo descarga Ollama (`/api/pull`)", "Ollama comprueba el SHA-256 "
     "de cada capa; NestAdmin compara el digest del manifiesto (D62)", "Apache-2.0",
     "Ninguna para NestAdmin: no se redistribuye"),
    ("Microsoft Edge WebView2 Runtime", "Evergreen (lo mantiene Windows)", "Viene con "
     "Windows 11; si falta, el instalador ejecuta el *bootstrapper* de Microsoft",
     "Descarga de Microsoft por HTTPS (bundler de Tauri)", "Términos de distribución de "
     "WebView2 de Microsoft", "Se usa el *bootstrapper*, cuya distribución Microsoft permite"),
    ("NSIS y nsis_tauri_utils", "los que fija tauri-cli 2.12.4", "Solo en la máquina de "
     "empaquetado (el instalador generado contiene el código de NSIS)", "tauri-cli comprueba "
     "su hash al descargarlos", "zlib/libpng (NSIS); MIT/Apache-2.0 (utilidades)",
     "Ninguna adicional conocida"),
]

# DLL que acompañan a tesseract.exe (lista de `scripts/setup-ocr.ps1`). La licencia es la
# del proyecto original; la build de UB Mannheim usa paquetes de MSYS2 que conviene
# confirmar antes de publicar (riesgo abierto en decisiones.md).
TESSERACT_DLLS = [
    ("libtesseract-5.dll", "Tesseract", "Apache-2.0"),
    ("libleptonica-6.dll", "Leptonica", "BSD-2-Clause"),
    ("libarchive-13.dll", "libarchive", "BSD-2-Clause"),
    ("libb2-1.dll", "BLAKE2", "CC0-1.0 OR OpenSSL OR Apache-2.0"),
    ("libbz2-1.dll", "bzip2", "bzip2-1.0.6"),
    ("libcrypto-3-x64.dll", "OpenSSL 3", "Apache-2.0"),
    ("libdeflate.dll", "libdeflate", "MIT"),
    ("libexpat-1.dll", "Expat", "MIT"),
    ("libgcc_s_seh-1.dll", "GCC runtime", "GPL-3.0-or-later WITH GCC-exception-3.1"),
    ("libgif-7.dll", "giflib", "MIT"),
    ("libiconv-2.dll", "GNU libiconv", "LGPL-2.1-or-later"),
    ("libjbig-0.dll", "JBIG-KIT", "GPL-2.0-or-later"),
    ("libjpeg-8.dll", "libjpeg-turbo", "IJG AND BSD-3-Clause AND Zlib"),
    ("liblerc.dll", "LERC", "Apache-2.0"),
    ("liblz4.dll", "LZ4", "BSD-2-Clause"),
    ("liblzma-5.dll", "XZ Utils", "0BSD"),
    ("libopenjp2-7.dll", "OpenJPEG", "BSD-2-Clause"),
    ("libpng16-16.dll", "libpng", "libpng-2.0"),
    ("libsharpyuv-0.dll", "libwebp", "BSD-3-Clause"),
    ("libstdc++-6.dll", "GCC runtime", "GPL-3.0-or-later WITH GCC-exception-3.1"),
    ("libtiff-6.dll", "LibTIFF", "libtiff"),
    ("libwebp-7.dll", "libwebp", "BSD-3-Clause"),
    ("libwebpmux-3.dll", "libwebp", "BSD-3-Clause"),
    ("libwinpthread-1.dll", "mingw-w64 winpthreads", "MIT AND BSD-3-Clause"),
    ("libzstd.dll", "Zstandard", "BSD-3-Clause OR GPL-2.0-only"),
    ("zlib1.dll", "zlib", "Zlib"),
]

# Textos de licencia de componentes empaquetados, relativos a vendor/.
VENDOR_LICENSES = [
    ("PDFium (empaquetado)", "pdfium/LICENSE"),
    ("PDFium y sus terceros", "pdfium/licenses"),
    ("Tesseract OCR", "tesseract/LICENSE"),
]


def cargo_metadata(root: Path) -> dict:
    out = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--locked",
         "--filter-platform", TARGET, "--manifest-path", str(root / "Cargo.toml")],
        check=True, capture_output=True,
    ).stdout
    return json.loads(out.decode("utf-8"))


def linked_crates(metadata: dict, root_crate: str = ROOT_CRATE) -> list[dict]:
    """Crates externos alcanzables desde ``root_crate`` por dependencias normales.

    Las de compilación (``build``) y de pruebas (``dev``) no se enlazan en el ejecutable.
    """
    packages = {p["id"]: p for p in metadata["packages"]}
    nodes = {n["id"]: n for n in metadata["resolve"]["nodes"]}
    start = [i for i, p in packages.items() if p["name"] == root_crate]
    if not start:
        raise SystemExit(f"No se encontró el crate {root_crate}")
    seen: set[str] = set()
    stack = start
    while stack:
        current = stack.pop()
        if current in seen:
            continue
        seen.add(current)
        for dep in nodes[current]["deps"]:
            if any(kind["kind"] is None for kind in dep["dep_kinds"]):
                stack.append(dep["pkg"])
    crates = [packages[i] for i in seen if packages[i].get("source")]
    return sorted(crates, key=lambda p: (p["name"], p["version"]))


def npm_production(lock: dict) -> list[dict]:
    """Paquetes de ``package-lock.json`` (v3) que no son de desarrollo."""
    rows = []
    for path, info in lock.get("packages", {}).items():
        if not path or info.get("dev") or info.get("devOptional"):
            continue
        rows.append({
            "name": path.split("node_modules/")[-1],
            "version": info.get("version", ""),
            "license": info.get("license", ""),
        })
    return sorted(rows, key=lambda r: (r["name"], r["version"]))


def cell(text: str) -> str:
    return text.replace("|", "\\|").replace("\n", " ")


def render(crates: list[dict], npm: list[dict]) -> str:
    lines = [
        "# Avisos de terceros",
        "",
        "Generado por `python scripts/third_party.py` a partir de `Cargo.lock`, "
        "`ui/package-lock.json` y el inventario del propio script (D60). No lo edite a mano. "
        "Las licencias son las que declara cada componente; no es una revisión legal. "
        "El instalador lleva además el texto completo de cada licencia "
        "(`THIRD_PARTY_NOTICES.txt`).",
        "",
        "## Componentes distribuidos o descargados",
        "",
        "| Componente | Versión | Cómo llega | Verificación | Licencia | Obligaciones |",
        "| --- | --- | --- | --- | --- | --- |",
    ]
    lines += ["| " + " | ".join(cell(c) for c in row) + " |" for row in COMPONENTS]
    lines += [
        "",
        "## Bibliotecas que acompañan a Tesseract",
        "",
        "Licencia del proyecto original de cada DLL de `tesseract\\`. JBIG-KIT (GPL) y "
        "libiconv (LGPL) obligan, al redistribuirlas, a dar su licencia y acceso al código "
        "fuente correspondiente; está pendiente de resolver antes de publicar (decisiones, "
        "riesgos abiertos).",
        "",
        "| Archivo | Proyecto | Licencia |",
        "| --- | --- | --- |",
    ]
    lines += [f"| `{f}` | {p} | {cell(l)} |" for f, p, l in TESSERACT_DLLS]
    lines += [
        "",
        f"## Crates de Rust enlazados en `nestadmin.exe` ({len(crates)})",
        "",
        f"Dependencias normales de `{ROOT_CRATE}` para `{TARGET}`; se excluyen las de "
        "compilación y pruebas.",
        "",
        "| Crate | Versión | Licencia | Repositorio |",
        "| --- | --- | --- | --- |",
    ]
    for p in crates:
        license_ = p.get("license") or f"archivo {p.get('license_file') or 'sin declarar'}"
        lines.append(
            f"| {p['name']} | {p['version']} | {cell(license_)} | {p.get('repository') or ''} |"
        )
    lines += [
        "",
        f"## Paquetes npm incluidos en la interfaz ({len(npm)})",
        "",
        "| Paquete | Versión | Licencia |",
        "| --- | --- | --- |",
    ]
    lines += [f"| {r['name']} | {r['version']} | {cell(r['license'])} |" for r in npm]
    return "\n".join(lines) + "\n"


def license_files(directory: Path) -> list[Path]:
    if not directory.is_dir():
        return []
    return sorted(
        f for f in directory.iterdir()
        if f.is_file() and f.name.upper().startswith(LICENSE_PREFIXES)
    )


def full_texts(crates: list[dict], vendor: Path) -> tuple[str, list[str]]:
    """Textos de licencia y lista de componentes sin archivo propio.

    Un mismo texto (por ejemplo, la Apache-2.0 sin cambios) se escribe una sola vez con la
    lista de componentes a los que corresponde; los avisos de copyright propios se conservan
    porque cambian el texto.
    """
    groups: dict[str, list[str]] = {}
    missing: list[str] = []

    def add(owner: str, f: Path) -> None:
        # read_text ya convierte los finales de línea de Windows.
        text = f.read_text("utf-8", "replace").strip()
        groups.setdefault(text, []).append(f"{owner} ({f.name})")

    for title, rel in VENDOR_LICENSES:
        path = vendor / rel
        files = sorted(path.iterdir()) if path.is_dir() else [path] if path.is_file() else []
        if not files:
            missing.append(f"{title} (vendor/{rel})")
        for f in files:
            add(title, f)
    for p in crates:
        files = license_files(Path(p["manifest_path"]).parent)
        if not files:
            missing.append(f"{p['name']} {p['version']} ({p.get('license')})")
        for f in files:
            add(f"{p['name']} {p['version']}", f)
    parts = [f"===== {', '.join(owners)} =====\n{text}" for text, owners in groups.items()]
    return "\n\n".join(parts), missing


def groups_hint(texts: str) -> list[str]:
    return [line for line in texts.splitlines() if line.startswith("===== ")]


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n", 1)[0])
    parser.add_argument("--check", action="store_true",
                        help="falla si THIRD_PARTY_NOTICES.md no está al día")
    parser.add_argument("--textos", type=Path,
                        help="escribe el inventario con los textos completos de licencia")
    parser.add_argument("--vendor", type=Path, default=ROOT / "vendor")
    args = parser.parse_args(argv)
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8")
        sys.stderr.reconfigure(encoding="utf-8")

    crates = linked_crates(cargo_metadata(ROOT))
    npm = npm_production(json.loads((ROOT / "ui/package-lock.json").read_text("utf-8")))
    summary = render(crates, npm)

    if args.check:
        current = NOTICES.read_text("utf-8") if NOTICES.is_file() else ""
        if current.replace("\r\n", "\n") != summary:
            print("THIRD_PARTY_NOTICES.md no está al día: ejecute python scripts/third_party.py",
                  file=sys.stderr)
            return 1
        print("THIRD_PARTY_NOTICES.md al día")
        return 0

    if not args.textos:
        NOTICES.write_text(summary, encoding="utf-8", newline="\n")
        print(f"{NOTICES.name}: {len(crates)} crates, {len(npm)} paquetes npm")
        return 0

    texts, missing = full_texts(crates, args.vendor)
    absent = [m for m in missing if "(vendor/" in m]
    if absent:
        # Sin estos textos el instalador no cumpliría las licencias del OCR (D60).
        print("Faltan licencias del OCR empaquetado; ejecute scripts/setup-ocr.ps1: "
              + "; ".join(absent), file=sys.stderr)
        return 1
    note = ""
    if missing:
        note = ("\nSin archivo de licencia propio (se aplica la licencia declarada arriba):\n"
                + "\n".join(f"- {m}" for m in missing) + "\n")
    args.textos.parent.mkdir(parents=True, exist_ok=True)
    args.textos.write_text(f"{summary}{note}\n\n{texts}\n", encoding="utf-8")
    print(f"{args.textos}: {len(groups_hint(texts))} textos, {len(missing)} crates sin archivo")
    return 0


if __name__ == "__main__":
    sys.exit(main())

