#!/usr/bin/env python3
"""Comprueba la coherencia mínima de la especificación; no prueba la aplicación."""
from pathlib import Path
import re
import sys

ROOT = Path(__file__).resolve().parent.parent
REQUIRED = (
    "PLAN.md", "AGENTS.md", "README.md", "specs/v1/README.md", "specs/v1/requisitos.md",
    "specs/v1/decisiones.md", "specs/v1/contratos.md", "specs/v1/trazabilidad.md",
    "specs/v1/tareas.md", "specs/v1/pruebas-instalador.md", "specs/v1/actualizaciones.md",
    "specs/v1/numeracion.md",
)
STATUSES = ("Pendiente", "Parcial", "Cumplido")
ROW = re.compile(r"^\| (R\d{2}) \| (T\d{2}[a-z]?): .+ \| (.+?) \|$", re.MULTILINE)
LINK = re.compile(r"\[[^\]]*\]\(([^)\s]+)\)")
DECISION = re.compile(r"^- \*\*D(\d+)\b", re.MULTILINE)
DECISION_REF = re.compile(r"\bD(\d+)\b")
BLOCK = re.compile(r"^\| D(\d+)[–-]D(\d+) \| .+? \| (Asignado|Reservado) \|$", re.MULTILINE)


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


def check_decisions(texts):
    """Decisiones únicas, dentro de un bloque de numeracion.md y sin referencias huérfanas."""
    errors = []
    decisions_text = texts.get("specs/v1/decisiones.md")
    blocks_text = texts.get("specs/v1/numeracion.md")
    if decisions_text is None or blocks_text is None:
        return errors
    numbers = [int(n) for n in DECISION.findall(decisions_text)]
    seen = set()
    for n in numbers:
        if n in seen:
            errors.append(f"La decisión D{n:02d} está repetida.")
        seen.add(n)
    blocks = [(int(a), int(b), state) for a, b, state in BLOCK.findall(blocks_text)]
    if not blocks:
        errors.append("specs/v1/numeracion.md no tiene bloques de decisiones.")
    for i, (a, b, _) in enumerate(blocks):
        if a > b:
            errors.append(f"Bloque mal formado: D{a:02d}–D{b:02d}.")
        for c, d, _ in blocks[i + 1:]:
            if a <= d and c <= b:
                errors.append(f"Los bloques D{a:02d}–D{b:02d} y D{c:02d}–D{d:02d} se solapan.")
    for n in sorted(seen):
        if not any(a <= n <= b for a, b, _ in blocks):
            errors.append(f"La decisión D{n:02d} no está en ningún bloque de specs/v1/numeracion.md.")
    reserved = [(a, b) for a, b, state in blocks if state == "Reservado"]
    for name, text in texts.items():
        if name == "specs/v1/numeracion.md":
            continue
        for n in sorted({int(x) for x in DECISION_REF.findall(text)}):
            if n not in seen and not any(a <= n <= b for a, b in reserved):
                errors.append(f"{name}: cita la decisión inexistente D{n:02d}.")
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
    errors.extend(check_decisions(texts))
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
