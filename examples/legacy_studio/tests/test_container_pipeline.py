import json
import pathlib

import pytest

from pipeline import pngutil

FIXTURE_OBJ = pathlib.Path(__file__).parents[1] / "pipeline" / "fixtures" / "two_parts.obj"


class _Proc:
    def __init__(self, returncode=0, stdout="", stderr=""):
        self.returncode = returncode
        self.stdout = stdout
        self.stderr = stderr


def fake_ok_runner(cmd, **kwargs):
    return _Proc(0, stdout="CONTAINER ID  IMAGE  COMMAND")


def fake_failing_runner(cmd, **kwargs):
    raise FileNotFoundError(2, "No such file or directory", "container")


def test_container_health_ok_and_down():
    from pipeline.container_pipeline import ContainerPipeline

    ok = ContainerPipeline(memory="4g", cpus=4).health(runner=fake_ok_runner)
    assert ok["ok"] is True
    down = ContainerPipeline(memory="4g", cpus=4).health(runner=fake_failing_runner)
    assert down["ok"] is False


def test_container_health_probe_command_and_cache():
    from pipeline.container_pipeline import ContainerPipeline

    calls = []

    def counting_runner(cmd, **kwargs):
        calls.append((cmd, kwargs))
        return _Proc(1, stderr="daemon not running")

    pipe = ContainerPipeline(memory="4g", cpus=4)
    first = pipe.health(runner=counting_runner)
    assert first["ok"] is False and "daemon not running" in first["detail"]
    second = pipe.health(runner=counting_runner)
    assert second == first
    assert len(calls) == 1  # second call served from the 10 s cache
    cmd, kwargs = calls[0]
    assert cmd[:2] == ["container", "ls"]
    assert kwargs["timeout"] == 3

# Camera at (0, -6, 2) looking at the origin (Blender -Z forward, +Y up),
# computed offline and recorded as a literal.
TEST_POSE_4X4 = [
    1.0, 0.0, 0.0, 0.0,
    0.0, 0.316228, -0.948683, -6.0,
    0.0, 0.948683, 0.316228, 2.0,
    0.0, 0.0, 0.0, 1.0,
]


@pytest.mark.container
def test_imported_scene_has_camera(tmp_path):
    from pipeline.container_pipeline import ContainerPipeline

    pipe = ContainerPipeline()
    scene_dir = tmp_path / "scene"
    pipe.import_asset(FIXTURE_OBJ, scene_dir)
    pipe._run(
        ["/scene/scene.blend", "--python-exit-code", "1", "--python-expr",
         "import bpy; assert bpy.context.scene.camera is not None, "
         "'imported scene has no camera — pose stream cannot start'"],
        [(scene_dir, "/scene")])


@pytest.mark.container
def test_render_at_production_resolution(tmp_path):
    from orchestrator.pipeline_api import Session
    from pipeline.container_pipeline import ContainerPipeline

    pipe = ContainerPipeline()
    scene_dir = tmp_path / "scene"
    pipe.import_asset(FIXTURE_OBJ, scene_dir)
    session = Session(id="t-full",
                      pose={"matrix": TEST_POSE_4X4, "location": [0, -6, 2]},
                      resolution=(1600, 1200), snapshot_id="snap-0001",
                      state="rendering")
    layers = pipe.render(session, scene_dir, tmp_path / "layers")
    assert [pngutil.read_size(layer.png_path) for layer in layers] == \
        [(1600, 1200), (1600, 1200)]


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
