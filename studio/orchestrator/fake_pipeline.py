"""Deterministic pipeline for --fake runs: one solid-color layer per fixed part."""

from pathlib import Path

from pipeline import pngutil

from .pipeline_api import LayerSpec, Session

FAKE_PARTS = [("cube_a", (220, 40, 40)), ("cube_b", (40, 180, 60))]


class FakePipeline:
    def render(self, session: Session, scene_dir: Path, out_dir: Path) -> list[LayerSpec]:
        width, height = session.resolution
        specs = []
        for part, rgb in FAKE_PARTS:
            png = out_dir / f"{part}.png"
            pngutil.write_solid(png, width, height, rgb)
            specs.append(LayerSpec(part=part, png_path=png))
        return specs
