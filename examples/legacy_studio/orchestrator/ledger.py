import json
import os
import time
from collections.abc import Callable
from pathlib import Path

_DEFAULTS = {"ts": 0.0, "kind": "", "summary": "", "bytes_moved": 0, "latency_ms": 0.0}


class Ledger:
    """Append-only JSONL crossing ledger. One JSON object per line."""

    def __init__(self, path: Path, now: Callable[[], float] = time.time):
        self._path = Path(path)
        self._path.parent.mkdir(parents=True, exist_ok=True)
        self._now = now

    def record(self, kind: str, summary: str, bytes_moved: int = 0,
               latency_ms: float = 0.0, **fields) -> dict:
        entry = {
            "ts": self._now(),
            "kind": kind,
            "summary": summary,
            "bytes_moved": bytes_moved,
            "latency_ms": latency_ms,
            **fields,
        }
        line = json.dumps(entry, ensure_ascii=False)
        with self._path.open("a", encoding="utf-8") as fh:
            fh.write(line + "\n")
            fh.flush()
            os.fsync(fh.fileno())
        return entry

    def read_all(self) -> list[dict]:
        if not self._path.exists():
            return []
        entries = []
        for line in self._path.read_text(encoding="utf-8").splitlines():
            if not line.strip():
                continue
            try:
                raw = json.loads(line)
            except json.JSONDecodeError:
                continue
            if not isinstance(raw, dict):
                continue
            entry = {**_DEFAULTS, **raw}
            entries.append(entry)
        return entries
