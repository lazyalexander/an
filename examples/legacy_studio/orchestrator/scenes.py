import pathlib
from dataclasses import dataclass, field


@dataclass
class Snapshot:
    id: str
    dir: pathlib.Path
    parts: list[str] = field(default_factory=list)


class SceneStore:
    """Versioned scene snapshots: snap-NNNN, incrementing, never reusing."""

    def __init__(self, root: pathlib.Path):
        self.root = pathlib.Path(root)
        self.root.mkdir(parents=True, exist_ok=True)
        self.current: Snapshot | None = None

    def new_snapshot(self) -> Snapshot:
        highest = 0
        for path in self.root.glob("snap-*"):
            if path.is_dir():
                suffix = path.name.split("-", 1)[1]
                if suffix.isdigit():
                    highest = max(highest, int(suffix))
        snap_id = "snap-{:04d}".format(highest + 1)
        snap_dir = self.root / snap_id
        (snap_dir / "source").mkdir(parents=True)
        return Snapshot(id=snap_id, dir=snap_dir)
