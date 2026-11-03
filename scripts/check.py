#!/usr/bin/env python3
"""Comprueba la coherencia mínima de la especificación; no prueba la aplicación."""
from pathlib import Path
import re
import sys

ROOT = Path(__file__).resolve().parent.parent
REQUIRED = (
    "PLAN.md", "AGENTS.md", "README.md", "specs/v1/README.md", "specs/v1/requisitos.md",
    "specs/v1/decisiones.md", "specs/v1/contratos.md", "specs/v1/trazabilidad.md",
    "specs/v1/tareas.md",
)
STATUSES = ("Pendiente", "Parcial", "Cumplido")
ROW = re.compile(r"^\| (R\d{2}) \| (T\d{2}[a-z]?): .+ \| (.+?) \|$", re.MULTILINE)
LINK = re.compile(r"\[[^\]]*\]\(([^)\s]+)\)")


def check_links(root, name, text):
    errors = []
    # Los ejemplos dentro de código no son enlaces.
    text = re.sub(r"```.*?```", "", text, flags=re.DOTALL)
    text = re.sub(r"`[^`\n]*`", "", text)
    for target in LINK.findall(text):
        if re.match(r"^[a-z]+:", target) or target.startswith("#"):
            continue
        path = (root / name).parent / target.split("#", 1)[0]
        if not path.exists():
            errors.append(f"{name}: enlace roto a {target}")
    return errors


def check(root):
    errors = []
    texts = {}
    for name in REQUIRED:
        path = root / name
        if not path.is_file() or not path.read_text(encoding="utf-8").strip():
            errors.append(f"Falta o está vacío: {name}")
        else:
            texts[name] = path.read_text(encoding="utf-8")
            errors.extend(check_links(root, name, texts[name]))
    req_text = texts.get("specs/v1/requisitos.md")
    matrix_text = texts.get("specs/v1/trazabilidad.md")
    if req_text is None or matrix_text is None:
        return errors

    ids = re.findall(r"^- \*\*(R\d{2})\*\*", req_text, re.MULTILINE)
    expected = [f"R{n:02d}" for n in range(1, len(ids) + 1)]
    if not ids or ids != expected:
        errors.append("Los requisitos deben numerarse R01, R02, ... sin huecos ni repeticiones.")

    rows = ROW.findall(matrix_text)
    covered = {req for req, _, _ in rows}
    for req in ids:
        if req not in covered:
            errors.append(f"{req} no tiene ningún caso de verificación en la matriz.")
    for req, test, status in rows:
        if req not in ids:
            errors.append(f"La matriz cita un requisito inexistente: {req}.")
        state = status.split(" ", 1)[0]
        if state not in STATUSES:
            errors.append(f"{test}: estado desconocido '{status}'.")
        elif state != "Pendiente" and not LINK.search(status):
            errors.append(f"{test}: el estado {state} requiere un enlace a la evidencia.")
    tests = [test for _, test, _ in rows]
    if len(set(tests)) != len(tests):
        errors.append("Los identificadores de prueba deben ser únicos.")
    return errors


def main() -> int:
    errors = check(ROOT)
    if errors:
        for error in errors:
            print(f"ERROR: {error}", file=sys.stderr)
        return 1
    print("Especificación: archivos, enlaces y trazabilidad básicos correctos.")
    print("No se ha probado la aplicación ni sus criterios de aceptación.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
