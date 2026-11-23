from adapter.core.preview import atomic_write, due


def test_atomic_write_replaces(tmp_path):
    p = tmp_path / "current.png"
    atomic_write(p, b"one"); atomic_write(p, b"two")
    assert p.read_bytes() == b"two" and not (tmp_path / "current.tmp").exists()


def test_due_cadence():
    assert due(10.0, 13.5, 3.0) and not due(10.0, 12.9, 3.0)
