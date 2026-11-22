import typing
from dataclasses import dataclass
from pathlib import Path


@dataclass
class LayerSpec:
    part: str
    png_path: Path


@dataclass
class Session:
    id: str
    pose: dict
    resolution: tuple[int, int]
    snapshot_id: str
    state: str  # "rendering" | "rendered" | "failed"


class Pipeline(typing.Protocol):
    def render(self, session: Session, scene_dir: Path, out_dir: Path) -> list[LayerSpec]:
        ...
