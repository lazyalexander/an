"""Pixel byte-order conversions.

QImage.Format_RGBA8888 memory is R,G,B,A per pixel; Krita's RGBA/U8
setPixelData expects B,G,R,A (api.kde.org: "the actual order of channels is
most often BGR"). Every pushed layer crosses this boundary exactly once.
"""


def rgba8888_to_krita_u8(data: bytes) -> bytes:
    if len(data) % 4:
        raise ValueError("RGBA8888 data length must be a multiple of 4")
    out = bytearray(data)
    out[0::4] = data[2::4]
    out[2::4] = data[0::4]
    return bytes(out)
