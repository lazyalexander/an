from pipeline import pngutil


def test_write_and_read_size(tmp_path):
    p = tmp_path / "a.png"
    pngutil.write_solid(p, 8, 4, (10, 20, 30))
    assert pngutil.read_size(p) == (8, 4)


def test_written_png_is_rgba_color_type(tmp_path):
    p = tmp_path / "b.png"
    pngutil.write_solid(p, 2, 2, (0, 0, 0))
    data = p.read_bytes()
    assert data[:8] == b"\x89PNG\r\n\x1a\n"
    assert data[25] == 6  # IHDR color type: truecolour with alpha


def test_read_size_rejects_non_png(tmp_path):
    p = tmp_path / "c.png"
    p.write_bytes(b"not a png at all, definitely")
    try:
        pngutil.read_size(p)
        assert False, "expected ValueError"
    except ValueError:
        pass
