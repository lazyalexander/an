import json
import pathlib

import pytest

from pipeline import pngutil

FIXTURE_OBJ = pathlib.Path(__file__).parents[1] / "pipeline" / "fixtures" / "two_parts.obj"

# Camera at (0, -6, 2) looking at the origin (Blender -Z forward, +Y up),
# computed offline and recorded as a literal.
TEST_POSE_4X4 = [
    1.0, 0.0, 0.0, 0.0,
    0.0, 0.316228, -0.948683, -6.0,
    0.0, 0.948683, 0.316228, 2.0,
    0.0, 0.0, 0.0, 1.0,
]


@pytest.mark.container
def test_import_and_render_two_parts(tmp_path):
    from orchestrator.pipeline_api import Session
    from pipeline.container_pipeline import ContainerPipeline

    pipe = ContainerPipeline()
    scene_dir = tmp_path / "scene"
    pipe.import_asset(FIXTURE_OBJ, scene_dir)
    parts = json.loads((scene_dir / "parts.json").read_text())["parts"]
    assert parts == ["cube_a", "cube_b"]
    session = Session(id="t-1",
                      pose={"matrix": TEST_POSE_4X4, "location": [0, -6, 2]},
                      resolution=(320, 240), snapshot_id="snap-0001",
                      state="rendering")
    layers = pipe.render(session, scene_dir, tmp_path / "layers")
    assert [layer.part for layer in layers] == ["cube_a", "cube_b"]
    assert [pngutil.read_size(layer.png_path) for layer in layers] == [(320, 240), (320, 240)]
