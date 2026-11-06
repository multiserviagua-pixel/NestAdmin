"""Genera las imágenes sintéticas de prueba del OCR de imágenes (D67).

Datos inventados: proveedores, NIF y números ficticios. Requiere Pillow y la fuente
DejaVu Sans. Las imágenes generadas se versionan para que la prueba no dependa de Pillow:

    python crates/extract/tests/fixtures/generar.py
"""
import random
from pathlib import Path

from PIL import Image, ImageDraw, ImageFilter, ImageFont

HERE = Path(__file__).parent
FONT = "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf"
BOLD = "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf"


def invoice(draw, x, y, lines, title):
    draw.text((x, y), title, font=ImageFont.truetype(BOLD, 44), fill=20)
    for i, line in enumerate(lines):
        draw.text((x, y + 80 + 52 * i), line, font=ImageFont.truetype(FONT, 34), fill=30)
    draw.rectangle((x - 30, y - 30, x + 1050, y + 80 + 52 * len(lines) + 20), outline=90, width=3)


def photo_like(img, seed):
    """Fondo no uniforme, leve desenfoque y ruido: como una foto, no un escaneo limpio."""
    rnd = random.Random(seed)
    img = img.filter(ImageFilter.GaussianBlur(0.8))
    px = img.load()
    w, h = img.size
    for yy in range(0, h, 2):
        for xx in range(0, w, 2):
            v = px[xx, yy] - int(25 * yy / h) + rnd.randint(-12, 12)
            px[xx, yy] = max(0, min(255, v))
    return img


def single_rotated():
    img = Image.new("L", (1240, 1000), 235)
    invoice(ImageDraw.Draw(img), 80, 80, [
        "Proveedor: Alba Solar SL",
        "NIF: B00000001",
        "Fecha: 2026-01-10",
        "Base imponible: 100.00 EUR",
        "IVA 21%: 21.00 EUR",
        "Total: 121.00 EUR",
    ], "FACTURA F-2026-077")
    img = photo_like(img, 1)
    # Guardada girada 90° con la etiqueta EXIF que dice cómo mostrarla (orientación 6),
    # como hace la cámara de un móvil sostenido en vertical.
    stored = img.transpose(Image.Transpose.ROTATE_90)
    exif = Image.Exif()
    exif[0x0112] = 6
    stored.convert("RGB").save(HERE / "factura-girada-exif.jpg", quality=70, exif=exif)


def two_invoices():
    img = Image.new("L", (2400, 1050), 238)
    draw = ImageDraw.Draw(img)
    invoice(draw, 80, 80, [
        "Proveedor: Río Azul SL",
        "NIF: B00000002",
        "Fecha: 2026-02-03",
        "Base imponible: 200.00 EUR",
        "IVA 21%: 42.00 EUR",
        "Total: 242.00 EUR",
    ], "FACTURA RA-0412")
    invoice(draw, 1290, 80, [
        "Proveedor: Talleres Norte SA",
        "NIF: A00000003",
        "Fecha: 2026-02-05",
        "Base imponible: 50.00 EUR",
        "IVA 21%: 10.50 EUR",
        "Total: 60.50 EUR",
    ], "FACTURA TN-88")
    photo_like(img, 2).convert("RGB").save(HERE / "dos-facturas.jpg", quality=70)


if __name__ == "__main__":
    single_rotated()
    two_invoices()
