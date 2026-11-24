"""Apply a normalized point annotation to the example SVG."""
import json
from pathlib import Path

choice = json.loads(Path("/input/choice/response.json").read_text())
drawing = Path("/input/source/candidate.svg").read_text()
marker = '<circle cx="%f" cy="%f" r="6" fill="none" stroke="white"/>' % (
    220 * choice["x"], 110 * choice["y"])
Path("/output/selected.svg").write_text(drawing.replace("</svg>", marker + "</svg>"))
print("Applied annotation to", choice["choice"])

# OpenRaster keeps the editable paint and reference on separate layers.
import struct
import zlib
from xml.etree import ElementTree as ET
from zipfile import ZipFile

scene = ET.fromstring(drawing)
background, circle = list(scene)
width, height = 220, 110


def color(hex_value):
    return bytes.fromhex(hex_value.lstrip("#")) + b"\xff"


def png(pixels):
    def chunk(kind, data):
        return (struct.pack(">I", len(data)) + kind + data
                + struct.pack(">I", zlib.crc32(kind + data)))

    raw = b"".join(b"\0" + pixels[y*width*4:(y+1)*width*4] for y in range(height))
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(raw)) + chunk(b"IEND", b""))


base, ink, merged = bytearray(), bytearray(), bytearray()
cx, cy, radius = (float(circle.attrib[k]) for k in ("cx", "cy", "r"))
paper, fill = color(background.attrib["fill"]), color(circle.attrib["fill"])
for y in range(height):
    for x in range(width):
        pixel = fill if (x-cx)**2 + (y-cy)**2 <= radius**2 else paper
        marked = 25 <= (x-width*choice["x"])**2 + (y-height*choice["y"])**2 <= 49
        base.extend(pixel)
        ink.extend(b"\xff"*4 if marked else b"\0"*4)
        merged.extend(b"\xff"*4 if marked else pixel)

root = ET.Element("image", version="0.0.3", w=str(width), h=str(height))
stack = ET.SubElement(root, "stack")
for name in ("annotation", "reference"):
    ET.SubElement(stack, "layer", name=name, src=f"data/{name}.png")
document = Path("/output/painting.ora")
with ZipFile(document, "w") as archive:
    archive.writestr("mimetype", "image/openraster")
    archive.writestr("stack.xml", ET.tostring(root, encoding="utf-8"))
    archive.writestr("data/reference.png", png(base))
    archive.writestr("data/annotation.png", png(ink))
    archive.writestr("mergedimage.png", png(merged))
with ZipFile(document) as saved:
    if saved.testzip() or len(ET.fromstring(saved.read("stack.xml")).findall("stack/layer")) != 2:
        raise ValueError("Saved layer stack is incomplete")
