"""Containerized pipeline: one `container run` per phase against the pinned image.

Runtime is Apple Container (project standard per README), not Docker Desktop:
amd64 runs go through its per-container lightweight VMs with --rosetta.
"""

import json
import pathlib
import subprocess

from orchestrator.pipeline_api import LayerSpec


class ContainerPipeline:
    def __init__(self, image: str = "bridge-studio-blender:5.2.3"):
        self.image = image
        self._scripts = pathlib.Path(__file__).resolve().parent

    def _run(self, blender_args, mounts):
        cmd = ["container", "run", "--rm", "--platform", "linux/amd64", "--rosetta"]
        for host, container in mounts:
            cmd += ["-v", "{}:{}".format(host, container)]
        cmd += ["-v", "{}:/opt/bridge".format(self._scripts)]
        cmd.append(self.image)
        cmd += blender_args
        proc = subprocess.run(cmd, capture_output=True, text=True)
        if proc.returncode != 0:
            tail = "\n".join((proc.stderr or proc.stdout).splitlines()[-20:])
            raise RuntimeError(
                "container run failed (exit {}): {}".format(proc.returncode, tail))
        return proc

    def import_asset(self, src: pathlib.Path, out_dir: pathlib.Path) -> None:
        src = pathlib.Path(src).resolve()
        out_dir = pathlib.Path(out_dir).resolve()
        out_dir.mkdir(parents=True, exist_ok=True)
        self._run(
            ["--python", "/opt/bridge/import_scene.py", "--",
             "/input/{}".format(src.name), "/out"],
            [(src.parent, "/input"), (out_dir, "/out")])

    def render(self, session, scene_dir: pathlib.Path, out_dir: pathlib.Path) -> list[LayerSpec]:
        scene_dir = pathlib.Path(scene_dir).resolve()
        out_dir = pathlib.Path(out_dir).resolve()
        out_dir.mkdir(parents=True, exist_ok=True)
        parts = json.loads((scene_dir / "parts.json").read_text())["parts"]
        pose_path = out_dir / ".pose.json"
        pose_path.write_text(json.dumps(session.pose))
        width, height = session.resolution
        try:
            self._run(
                ["--python", "/opt/bridge/render_layers.py", "--",
                 "/scene/scene.blend", "/scene/parts.json", "/out/.pose.json",
                 "/out", str(width), str(height)],
                [(scene_dir, "/scene"), (out_dir, "/out")])
        finally:
            pose_path.unlink(missing_ok=True)
        return [LayerSpec(part=part, png_path=out_dir / "{}.png".format(part))
                for part in parts]
