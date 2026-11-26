"""Prepare an image model request from sketch strokes (standard library only).

Writes control.png (dark lines on white, width following pressure, the
ControlNet scribble convention) and request.json with the prompt built from
the person's note. The finished painting is produced by An's host-side image
model node, not in this container.
"""
import json
import struct
import sys
import zlib
from pathlib import Path

SIZE = 1024
STYLE = sys.argv[1] if len(sys.argv) > 1 else "a finished warm watercolor illustration"


def png(pixels, size):
    raw = b"".join(b"\x00" + bytes(pixels[y * size:(y + 1) * size]) for y in range(size))
    chunk = lambda tag, data: (struct.pack(">I", len(data)) + tag + data
                               + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF))
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 0, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(raw, 6)) + chunk(b"IEND", b""))


def disc(pixels, cx, cy, r):
    r2 = r * r
    for y in range(max(0, int(cy - r)), min(SIZE, int(cy + r) + 1)):
        dy = (y - cy) ** 2
        row = y * SIZE
        for x in range(max(0, int(cx - r)), min(SIZE, int(cx + r) + 1)):
            if (x - cx) ** 2 + dy <= r2:
                pixels[row + x] = 0


response = json.loads(Path("/input/choice/response.json").read_text())
pixels = bytearray([255]) * (SIZE * SIZE)
for stroke in response["strokes"]:
    pts = stroke["points"]
    for a, b in zip(pts, pts[1:] or pts):
        r = max(1.5, stroke["size"] * 0.4 * (0.3 + 0.7 * (a[2] + b[2]) / 2))
        steps = max(1, int(max(abs(b[0] - a[0]), abs(b[1] - a[1])) * SIZE / max(1, r / 2)))
        for i in range(steps + 1):
            t = i / steps
            disc(pixels, (a[0] + (b[0] - a[0]) * t) * SIZE, (a[1] + (b[1] - a[1]) * t) * SIZE, r)
Path("/output/control.png").write_bytes(png(pixels, SIZE))
note = response.get("note", "").strip()
prompt = f"{STYLE} of {note}" if note else STYLE
prompt += ". Follow the sketch closely: keep its composition, shapes and positions."
Path("/output/request.json").write_text(json.dumps({"prompt": prompt, "size": "1024x1024"}))
