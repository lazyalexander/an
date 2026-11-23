"""Fake-backed tests for the Krita docker's flattened preview path.

docker.py imports krita/PyQt5 at module scope (it runs inside Krita), so
these tests install in-memory fakes into sys.modules and load docker.py
from its file path. The fakes model the reference machine: Krita 5.x,
where Document has pixelData() but no projection()/duplicate().
"""

import importlib.util
import pathlib
import struct
import sys
import types
import zlib

import pytest

DOCKER_PATH = (pathlib.Path(__file__).resolve().parents[1]
               / "adapter" / "krita_plugin" / "bridge_studio" / "docker.py")

_PNG_SIGNATURE = b"\x89PNG\r\n\x1a\n"


def _png_chunk(tag, payload):
    return (struct.pack(">I", len(payload)) + tag + payload
            + struct.pack(">I", zlib.crc32(tag + payload) & 0xFFFFFFFF))


def _encode_png_rgba(width, height, rgba):
    rows = b"".join(b"\x00" + rgba[y * width * 4:(y + 1) * width * 4]
                    for y in range(height))
    ihdr = struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0)
    return (_PNG_SIGNATURE + _png_chunk(b"IHDR", ihdr)
            + _png_chunk(b"IDAT", zlib.compress(rows))
            + _png_chunk(b"IEND", b""))


def _decode_png_rgba(data):
    """Decode the filter-0 8-bit RGBA PNGs that _encode_png_rgba emits."""
    assert data[:8] == _PNG_SIGNATURE
    pos, width, height, idat = 8, None, None, b""
    while pos < len(data):
        length, = struct.unpack(">I", data[pos:pos + 4])
        tag, payload = data[pos + 4:pos + 8], data[pos + 8:pos + 8 + length]
        if tag == b"IHDR":
            width, height, depth, color = struct.unpack(">IIBB", payload[:10])
            assert (depth, color) == (8, 6)
        elif tag == b"IDAT":
            idat += payload
        pos += 12 + length
    raw = zlib.decompress(idat)
    stride = width * 4 + 1
    pixels = b""
    for y in range(height):
        row = raw[y * stride:(y + 1) * stride]
        assert row[0] == 0  # the fake encoder only writes filter None
        pixels += row[1:]
    return width, height, pixels


class FakeQImage:
    """QImage stand-in honoring Qt's documented memory layouts.

    Format_ARGB32 stores 0xAARRGGBB in a native uint32, i.e. B,G,R,A in
    memory on little-endian; Format_RGBA8888 is explicitly R,G,B,A bytes.
    save() encodes a true-color RGBA PNG from the declared semantics.
    """

    Format_ARGB32 = "ARGB32"
    Format_RGBA8888 = "RGBA8888"

    def __init__(self, data, width, height, fmt):
        self._data = bytes(data)
        self._width = width
        self._height = height
        self._format = fmt

    def isNull(self):
        return len(self._data) != self._width * self._height * 4

    def save(self, buffer, fmt):
        assert fmt == "PNG"
        rgba = bytearray(self._data)
        if self._format == self.Format_ARGB32:  # B,G,R,A bytes in memory
            rgba[0::4], rgba[2::4] = self._data[2::4], self._data[0::4]
        buffer.write(_encode_png_rgba(self._width, self._height, bytes(rgba)))
        return True


class FailingQImage(FakeQImage):
    def save(self, buffer, fmt):
        return False


class NullQImage(FakeQImage):
    def isNull(self):
        return True


class FakeQBuffer:
    def __init__(self):
        self._data = bytearray()

    def open(self, mode):
        return True

    def write(self, data):
        self._data += data

    def close(self):
        pass

    def data(self):
        return bytes(self._data)


class FakeQIODevice:
    WriteOnly = 1


class FakeKritaDocument:
    """Krita 5.x Document surface: pixelData yes; projection/duplicate no."""

    def __init__(self, width, height, pixels=b"", raises=None,
                 color_model="RGBA", color_depth="U8"):
        self._width = width
        self._height = height
        self._pixels = pixels
        self._raises = raises
        self._color_model = color_model
        self._color_depth = color_depth
        self.reads = []

    def width(self):
        return self._width

    def height(self):
        return self._height

    def colorModel(self):
        return self._color_model

    def colorDepth(self):
        return self._color_depth

    def pixelData(self, x, y, w, h):
        self.reads.append((x, y, w, h))
        if self._raises is not None:
            raise self._raises
        return self._pixels


def _load_docker(monkeypatch, qimage_class=FakeQImage):
    krita = types.ModuleType("krita")
    krita.DockWidget = object
    krita.Krita = object
    qtcore = types.ModuleType("PyQt5.QtCore")
    qtcore.QBuffer = FakeQBuffer
    qtcore.QIODevice = FakeQIODevice
    qtcore.QTimer = object
    qtcore.Qt = types.SimpleNamespace(IgnoreAspectRatio=1, SmoothTransformation=2)
    qtgui = types.ModuleType("PyQt5.QtGui")
    qtgui.QImage = qimage_class
    qtwidgets = types.ModuleType("PyQt5.QtWidgets")
    qtwidgets.QLabel = object
    qtwidgets.QVBoxLayout = object
    qtwidgets.QWidget = object
    monkeypatch.setitem(sys.modules, "krita", krita)
    monkeypatch.setitem(sys.modules, "PyQt5", types.ModuleType("PyQt5"))
    monkeypatch.setitem(sys.modules, "PyQt5.QtCore", qtcore)
    monkeypatch.setitem(sys.modules, "PyQt5.QtGui", qtgui)
    monkeypatch.setitem(sys.modules, "PyQt5.QtWidgets", qtwidgets)
    spec = importlib.util.spec_from_file_location(
        "bridge_docker_under_test", DOCKER_PATH)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def test_flattened_png_roundtrips_krita_bgra_bytes(monkeypatch):
    docker = _load_docker(monkeypatch)
    rgba = bytes([255, 0, 0, 255, 0, 255, 0, 128])  # red, half-alpha green
    krita_bgra = bytes([0, 0, 255, 255, 0, 255, 0, 128])  # B,G,R,A per pixel
    doc = FakeKritaDocument(2, 1, krita_bgra)
    assert not hasattr(doc, "projection")  # Krita 5.x: neither exists
    assert not hasattr(doc, "duplicate")
    width, height, pixels = _decode_png_rgba(docker._flattened_png_bytes(doc))
    assert (width, height) == (2, 1)
    assert pixels == rgba
    assert doc.reads == [(0, 0, 2, 1)]


def test_flattened_png_propagates_pixel_read_failure(monkeypatch):
    docker = _load_docker(monkeypatch)
    doc = FakeKritaDocument(2, 1, raises=RuntimeError("no image yet"))
    with pytest.raises(RuntimeError, match="no image yet"):
        docker._flattened_png_bytes(doc)


def _counting_qimage(builds):
    class CountingQImage(FakeQImage):
        def __init__(self, *args):
            builds.append(args)
            super().__init__(*args)
    return CountingQImage


def test_flattened_png_rejects_non_rgba_document(monkeypatch):
    # GRAYA/U8 pixelData is w*h*2 bytes: 4 here, where BGRA expects 8.
    # PyQt5's QImage(data, ...) does no length validation — constructing
    # it from this buffer would read out of bounds at save() time.
    builds = []
    docker = _load_docker(monkeypatch, qimage_class=_counting_qimage(builds))
    doc = FakeKritaDocument(2, 1, bytes([128, 255, 64, 255]),
                            color_model="GRAYA", color_depth="U8")
    with pytest.raises(RuntimeError, match="RGBA/U8"):
        docker._flattened_png_bytes(doc)
    assert builds == []  # QImage must never see the short buffer


def test_flattened_png_rejects_short_pixel_data(monkeypatch):
    builds = []
    docker = _load_docker(monkeypatch, qimage_class=_counting_qimage(builds))
    doc = FakeKritaDocument(2, 1, bytes([0, 0, 255]))  # 3 bytes, expected 8
    with pytest.raises(RuntimeError, match="bytes"):
        docker._flattened_png_bytes(doc)
    assert builds == []


def test_flattened_png_raises_when_encode_fails(monkeypatch):
    docker = _load_docker(monkeypatch, qimage_class=FailingQImage)
    doc = FakeKritaDocument(1, 1, bytes([1, 2, 3, 4]))
    with pytest.raises(RuntimeError, match="encode"):
        docker._flattened_png_bytes(doc)


def test_flattened_png_raises_when_projection_read_is_null(monkeypatch):
    docker = _load_docker(monkeypatch, qimage_class=NullQImage)
    doc = FakeKritaDocument(1, 1, bytes([1, 2, 3, 4]))
    with pytest.raises(RuntimeError, match="preview projection read failed"):
        docker._flattened_png_bytes(doc)
