from orchestrator.fake_pipeline import FakePipeline
from orchestrator.pipeline_api import Session
from pipeline import pngutil


def _session():
    return Session(id="t-1", pose={}, resolution=(1600, 1200), snapshot_id="",
                   state="rendering")


def test_fake_pipeline_writes_real_pngs(tmp_path):
    out = tmp_path / "layers"
    out.mkdir()
    layers = FakePipeline().render(_session(), tmp_path / "scene", out)
    assert [l.part for l in layers] == ["cube_a", "cube_b"]
    assert all(pngutil.read_size(l.png_path) == (1600, 1200) for l in layers)
