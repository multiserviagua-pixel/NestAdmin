"""Genera los membretes sintéticos del recorrido T15b (sin datos reales).

Uso: python membretes.py <carpeta>   (necesita pypdf y cryptography)
"""
import pathlib
import sys

import pypdf
from pypdf.generic import DecodedStreamObject, NameObject

A4 = (595, 842)
TEAL, RED, GREEN, NAVY = "0.1 0.35 0.45", "0.6 0.2 0.2", "0.2 0.6 0.2", "0.05 0.1 0.2"


def bands(size, color):
    """Banda superior de 70 pt (24,7 mm) e inferior de 40 pt (14,1 mm): fuera del área útil por defecto."""
    w, h = size
    return f"{color} rg 0 {h - 70} {w} 70 re f 0 0 {w} 40 re f"


def make(out, name, pages, password=None, pad=0):
    writer = pypdf.PdfWriter()
    for size, content in pages:
        page = writer.add_blank_page(width=size[0], height=size[1])
        stream = DecodedStreamObject()
        stream.set_data(content.encode())
        page[NameObject("/Contents")] = writer._add_object(stream)
    if pad:
        writer.add_metadata({"/Keywords": "x" * pad})
    if password is not None:
        writer.encrypt(user_password=password, owner_password="propietario-sintetico", algorithm="AES-256")
    writer.write(out / name)


def main():
    out = pathlib.Path(sys.argv[1])
    out.mkdir(parents=True, exist_ok=True)
    make(out, "dos-paginas.pdf", [(A4, bands(A4, TEAL)), (A4, bands(A4, RED))])
    make(out, "tres-paginas.pdf", [(A4, bands(A4, TEAL))] * 3)
    make(out, "apaisado.pdf", [((842, 595), bands((842, 595), TEAL))])
    make(out, "a5.pdf", [((420, 595), bands((420, 595), TEAL))])
    make(out, "con-clave.pdf", [(A4, bands(A4, TEAL))], password="clave-sintetica")
    make(out, "restringido.pdf", [(A4, bands(A4, GREEN))], password="")
    make(out, "grande.pdf", [(A4, bands(A4, TEAL))], pad=5 * 1024 * 1024 + 100)
    make(out, "oscuro.pdf", [(A4, f"{NAVY} rg 0 0 595 842 re f")])
    # Banda oscura de 60 mm en mitad del área útil.
    make(out, "banda-en-texto.pdf", [(A4, f"{NAVY} rg 0 400 595 170 re f")])
    (out / "vacio.pdf").write_bytes(b"")
    (out / "no-es-pdf.txt").write_text("esto no es un PDF\n", encoding="utf-8")
    for f in sorted(out.iterdir()):
        print(f.name, f.stat().st_size)


if __name__ == "__main__":
    main()
