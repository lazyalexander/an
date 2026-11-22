import struct
import zlib
from pathlib import Path

_SIGNATURE = b"\x89PNG\r\n\x1a\n"


def _chunk(tag: bytes, payload: bytes) -> bytes:
    return (struct.pack(">I", len(payload)) + tag + payload
            + struct.pack(">I", zlib.crc32(tag + payload) & 0xFFFFFFFF))


def write_solid(path, width: int, height: int, rgb: tuple[int, int, int]) -> None:
    path = Path(path)
    r, g, b = rgb
    row = b"\x00" + bytes((r, g, b, 255)) * width
    ihdr = struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0)
    png = (_SIGNATURE + _chunk(b"IHDR", ihdr)
           + _chunk(b"IDAT", zlib.compress(row * height))
           + _chunk(b"IEND", b""))
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(png)


def read_size(path) -> tuple[int, int]:
    data = Path(path).read_bytes()
    if data[:8] != _SIGNATURE or data[12:16] != b"IHDR":
        raise ValueError(f"not a PNG: {path}")
    width, height = struct.unpack(">II", data[16:24])
    return width, height
