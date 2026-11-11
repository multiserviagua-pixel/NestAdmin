"""Revisa los PDF del recorrido T15b: lee el QR con OpenCV y guarda imágenes de las páginas.

Uso: python revisar.py <carpeta de salida del recorrido>
Necesita pypdfium2, pillow, numpy y opencv-python-headless. Escribe <salida>/qr.json y
<salida>/pdf-<documento>-<página>.jpg (primera y última página).
"""
import json
import pathlib
import sys

import cv2
import numpy as np
import pypdfium2 as pdfium

out = pathlib.Path(sys.argv[1])
report = {}
for path in sorted((out / "pdf").glob("*.pdf")):
    doc = pdfium.PdfDocument(str(path))
    pages = len(doc)
    qr = None
    for scale in (2, 3, 4):
        gray = np.array(doc[0].render(scale=scale).to_pil().convert("L"))
        value, _, _ = cv2.QRCodeDetector().detectAndDecode(gray)
        if value:
            qr = value
            break
    text = doc[0].get_textpage().get_text_range()
    report[path.name] = {
        "paginas": pages,
        "qr": qr,
        "leyenda_verifactu": "VERI*FACTU" in text,
        "pagina_n_de_m": f"Página 1 de {pages}" in text,
    }
    for index in sorted({0, pages - 1}):
        image = doc[index].render(scale=1.2).to_pil().convert("RGB")
        image.save(out / f"pdf-{path.stem}-{index + 1}.jpg", quality=72)
(out / "qr.json").write_text(json.dumps(report, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
print(json.dumps(report, indent=2, ensure_ascii=False))
