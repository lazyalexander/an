from pathlib import Path

from adapter.core.decisions import execute


class Doc:
    def __init__(self):
        self.saved = []

    def save(self, path):
        self.saved.append(Path(path))
        Path(path).write_bytes(b"painted")


def test_execute_saves_the_live_document_over_its_archive(tmp_path):
    archive = tmp_path / "pose_s.kra"
    archive.write_bytes(b"render")
    doc = Doc()
    assert execute("adopt", doc, archive) == (True, None)
    assert doc.saved == [archive]
    assert archive.read_bytes() == b"painted"


def test_execute_before_a_render_never_fabricates_an_archive(tmp_path):
    archive = tmp_path / "pose_s.kra"
    doc = Doc()
    ok, detail = execute("unlock", doc, archive)
    assert ok and detail == "no archive yet"
    assert doc.saved == []
    assert not archive.exists()


def test_execute_without_an_open_document_uses_the_archive(tmp_path):
    archive = tmp_path / "pose_s.kra"
    archive.write_bytes(b"render")
    ok, detail = execute("re-render", None, archive)
    assert ok and detail == "no open document"
    assert archive.read_bytes() == b"render"


def test_execute_rejects_unknown_kinds(tmp_path):
    archive = tmp_path / "pose_s.kra"
    archive.write_bytes(b"render")
    ok, detail = execute("publish", Doc(), archive)
    assert not ok and "unknown decision" in detail
