import pytest

from adapter.core.archive import read_sidecar
from adapter.core.layout import MODEL_GROUP, pose_group_name
from adapter.core.push import push_session
from orchestrator.pipeline_api import LayerSpec
from pipeline import pngutil

from kisfake import FakeKis


def _session():
    return {"id": "s-1", "pose": {"matrix": [1.0] * 16, "location": [0, 0, 0]},
            "resolution": [1600, 1200], "snapshot_id": "snap-0001", "state": "rendered"}


def test_push_builds_structure_and_archive(tmp_path):
    layers_dir = tmp_path / "layers"
    layers_dir.mkdir()
    pngutil.write_solid(layers_dir / "cube_a.png", 1600, 1200, (255, 0, 0))
    pngutil.write_solid(layers_dir / "cube_b.png", 1600, 1200, (0, 255, 0))
    layers = [LayerSpec("cube_a", layers_dir / "cube_a.png"),
              LayerSpec("cube_b", layers_dir / "cube_b.png")]
    kis = FakeKis()
    kra = push_session(kis, _session(), layers, tmp_path / "arch")
    doc = kis.documents[0]
    assert (doc.width, doc.height) == (1600, 1200)
    assert doc.top_level_names() == [MODEL_GROUP, pose_group_name("s-1")]
    assert doc.children_of(MODEL_GROUP) == ["cube_a", "cube_b"]
    assert doc.is_locked(MODEL_GROUP) is True
    assert read_sidecar(kra)["s-1"].snapshot_id == "snap-0001"


def test_push_rejects_size_mismatch(tmp_path):
    # an 800x600 layer into a 1600x1200 session must fail, never rescale
    layers_dir = tmp_path / "layers"
    layers_dir.mkdir()
    pngutil.write_solid(layers_dir / "cube_a.png", 800, 600, (255, 0, 0))
    layers = [LayerSpec("cube_a", layers_dir / "cube_a.png")]
    with pytest.raises(ValueError):
        push_session(FakeKis(), _session(), layers, tmp_path / "arch")


def test_push_refuses_to_clobber_existing_archive(tmp_path):
    layers_dir = tmp_path / "layers"
    layers_dir.mkdir()
    pngutil.write_solid(layers_dir / "cube_a.png", 1600, 1200, (255, 0, 0))
    layers = [LayerSpec("cube_a", layers_dir / "cube_a.png")]
    kra = push_session(FakeKis(), _session(), layers, tmp_path / "arch")
    kra.write_bytes(b"user-painted-content")  # the user painted and saved
    kis = FakeKis()
    with pytest.raises(FileExistsError):
        push_session(kis, _session(), layers, tmp_path / "arch")
    assert kra.read_bytes() == b"user-painted-content"
    assert kis.documents == []  # no half-built document left in Krita
