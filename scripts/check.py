#!/usr/bin/env python3
"""Comprueba la coherencia mínima de la especificación; no prueba la aplicación."""
from pathlib import Path
import re
import sys

ROOT = Path(__file__).resolve().parent.parent
REQUIRED = (
    "PLAN.md", "AGENTS.md", "specs/v1/README.md", "specs/v1/requisitos.md",
    "specs/v1/contratos.md", "specs/v1/trazabilidad.md", "specs/v1/tareas.md",
)


def main() -> int:
    errors = []
    for name in REQUIRED:
        path = ROOT / name
        if not path.is_file() or not path.read_text(encoding="utf-8").strip():
            errors.append(f"Falta o está vacío: {name}")
    requirements = ROOT / "specs/v1/requisitos.md"
    matrix = ROOT / "specs/v1/trazabilidad.md"
    if requirements.is_file() and matrix.is_file():
        req_text = requirements.read_text(encoding="utf-8")
        matrix_text = matrix.read_text(encoding="utf-8")
        ids = re.findall(r"^- \*\*(R\d{2})\*\*", req_text, re.MULTILINE)
        rows = re.findall(r"^\| (R\d{2}) \| (T\d{2}):", matrix_text, re.MULTILINE)
        if len(ids) != 12 or len(set(ids)) != len(ids):
            errors.append("Se esperan 12 requisitos con identificadores únicos.")
        if {req for req, _ in rows} != set(ids) or len(rows) != len(ids):
            errors.append("La matriz debe contener una fila de prueba por requisito.")
        if len({test for _, test in rows}) != len(rows):
            errors.append("Los identificadores de prueba deben ser únicos.")
    if errors:
        for error in errors:
            print(f"ERROR: {error}", file=sys.stderr)
        return 1
    print("Especificación: archivos y trazabilidad básicos correctos.")
    print("No se ha probado la aplicación ni sus criterios de aceptación.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
