#!/usr/bin/env python3
"""Renumera decisiones (`D<n>`) en una sola pasada sobre los archivos de texto versionados.

Uso:
    python scripts/renumerar.py D92-D98=D106 D99-D104=D113          # prueba: solo lista cambios
    python scripts/renumerar.py D92-D98=D106 D99-D104=D113 --aplicar

Cada argumento traslada un número (`D92=D106`) o un tramo (`D92-D98=D106`, que lleva D92 a
D106, D93 a D107…). La sustitución es simultánea, así que un cambio nunca se encadena con
otro, y solo afecta a `D<dígitos>` como palabra completa (no toca `DV-1` ni `3D`).
Se niega a seguir si un destino ya es una decisión de `specs/v1/decisiones.md` que no se
renumera. Renumere en la rama antes de fusionarle `main` (ver specs/v1/numeracion.md).
"""
from pathlib import Path
import argparse
import re
import subprocess
import sys

ROOT = Path(__file__).resolve().parent.parent
REF = re.compile(r"\bD(\d+)\b")
DECISION = re.compile(r"^- \*\*D(\d+)\b", re.MULTILINE)
ARG = re.compile(r"^D(\d+)(?:[-–]D(\d+))?=D(\d+)$")
TEXT_SUFFIXES = {
    ".md", ".rs", ".ts", ".js", ".json", ".toml", ".py", ".ps1", ".nsh", ".html", ".css",
    ".txt", ".yml", ".yaml",
}


def width(n):
    """Los números de una cifra se escriben con cero delante (D01)."""
    return f"D{n:02d}"


def parse_mapping(args):
    mapping = {}
    for arg in args:
        m = ARG.match(arg)
        if not m:
            raise ValueError(f"argumento no válido: {arg} (se espera D92=D106 o D92-D98=D106)")
        start = int(m.group(1))
        end = int(m.group(2) or start)
        target = int(m.group(3))
        if end < start:
            raise ValueError(f"tramo invertido: {arg}")
        for offset, n in enumerate(range(start, end + 1)):
            if n in mapping:
                raise ValueError(f"D{n} aparece dos veces")
            mapping[n] = target + offset
    targets = list(mapping.values())
    if len(set(targets)) != len(targets):
        raise ValueError("dos decisiones irían al mismo número")
    return mapping


def check_targets(mapping, decisions_text):
    existing = {int(n) for n in DECISION.findall(decisions_text)}
    staying = existing - set(mapping)
    clash = sorted(set(mapping.values()) & staying)
    if clash:
        raise ValueError("destinos ya ocupados: " + ", ".join(width(n) for n in clash))


def renumber(text, mapping):
    count = 0

    def repl(m):
        nonlocal count
        n = int(m.group(1))
        if n not in mapping:
            return m.group(0)
        count += 1
        return width(mapping[n])

    return REF.sub(repl, text), count


def tracked_text_files(root):
    out = subprocess.run(["git", "ls-files", "-z"], cwd=root, check=True, capture_output=True)
    for name in out.stdout.decode("utf-8").split("\0"):
        if name and Path(name).suffix.lower() in TEXT_SUFFIXES:
            yield root / name


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("tramos", nargs="+")
    parser.add_argument("--aplicar", action="store_true", help="escribe los cambios")
    parser.add_argument("--raiz", type=Path, default=ROOT)
    args = parser.parse_args(argv)
    try:
        mapping = parse_mapping(args.tramos)
        decisions = args.raiz / "specs/v1/decisiones.md"
        check_targets(mapping, decisions.read_text(encoding="utf-8"))
    except (ValueError, OSError) as error:
        print(f"ERROR: {error}", file=sys.stderr)
        return 1
    total = 0
    for path in tracked_text_files(args.raiz):
        try:
            raw = path.read_bytes().decode("utf-8")
        except UnicodeDecodeError:
            continue
        new, count = renumber(raw, mapping)
        if count:
            total += count
            print(f"{count:4d}  {path.relative_to(args.raiz).as_posix()}")
            if args.aplicar:
                path.write_bytes(new.encode("utf-8"))
    print(f"{total} referencias {'cambiadas' if args.aplicar else 'por cambiar (prueba; use --aplicar)'}.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
