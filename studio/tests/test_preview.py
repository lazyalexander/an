from adapter.core.preview import atomic_write, due, scaled_size


def test_scaled_size_keeps_small_documents_untouched():
    assert scaled_size(640, 480) == (640, 480)
    assert scaled_size(800, 800) == (800, 800)


def test_scaled_size_clamps_the_long_edge():
    assert scaled_size(1600, 1200) == (800, 600)
    assert scaled_size(1200, 1600) == (600, 800)
    assert scaled_size(1, 10000) == (1, 800)


def test_atomic_write_replaces(tmp_path):
    p = tmp_path / "current.png"
    atomic_write(p, b"one"); atomic_write(p, b"two")
    assert p.read_bytes() == b"two" and not (tmp_path / "current.tmp").exists()


def test_due_cadence():
    assert due(10.0, 13.5, 3.0) and not due(10.0, 12.9, 3.0)
