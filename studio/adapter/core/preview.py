"""Pure helpers for the docker's periodic preview export.

The docker exports a flattened PNG of the open document every few seconds
to run/preview/current.png so the macro UI page can show what is on the
canvas. Writes are atomic so the web server never serves a partial file.
"""

import os
from pathlib import Path

PREVIEW_INTERVAL_S = 3.0
PREVIEW_MAX_SIDE = 800


def scaled_size(width: int, height: int, max_side: int = PREVIEW_MAX_SIDE) -> tuple[int, int]:
    """Preview is a glance, not the canvas: keep the long edge within budget."""
    longest = max(width, height)
    if longest <= max_side:
        return width, height
    ratio = max_side / longest
    return max(1, round(width * ratio)), max(1, round(height * ratio))


def atomic_write(path: Path, data: bytes) -> None:
    """Write data to path via a sibling temp file, then replace."""
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_suffix(".tmp")
    tmp.write_bytes(data)
    os.replace(tmp, path)


def due(last_export: float, now: float, interval_s: float = PREVIEW_INTERVAL_S) -> bool:
    return now - last_export >= interval_s


def clear(path: Path) -> None:
    """Drop a preview that no longer belongs to a live document."""
    Path(path).unlink(missing_ok=True)
