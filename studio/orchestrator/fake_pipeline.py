"""Deterministic pipeline for --fake runs: one solid-color layer per fixed part."""

from pathlib import Path

from .pipeline_api import LayerSpec, Session

FAKE_PARTS = ["cube_a", "cube_b"]


class FakePipeline:
    def render(self, session: Session, scene_dir: Path, out_dir: Path) -> list[LayerSpec]:
        return [LayerSpec(part=part, png_path=out_dir / f"{part}.png")
                for part in FAKE_PARTS]
