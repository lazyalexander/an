"""Inbox watcher: size-stable CAD exports become versioned scene snapshots."""

import json
import pathlib
import shutil
import threading
import time
from collections.abc import Callable

from .ledger import Ledger
from .scenes import SceneStore

WATCH_EXTS = {".obj", ".stp", ".step"}


class Watcher:
    def __init__(self, inbox: pathlib.Path, store: SceneStore,
                 import_asset: Callable[[pathlib.Path, pathlib.Path], None],
                 ledger: Ledger, poll_s: float = 1.0, settle_s: float = 0.5):
        self._inbox = pathlib.Path(inbox)
        self._store = store
        self._import_asset = import_asset
        self._ledger = ledger
        self._poll_s = poll_s
        self._settle_s = settle_s
        self._stop = threading.Event()
        self._thread = threading.Thread(target=self._loop, daemon=True)
        self._sizes: dict[pathlib.Path, tuple[int, float]] = {}

    def start(self) -> None:
        self._inbox.mkdir(parents=True, exist_ok=True)
        self._thread.start()

    def stop(self) -> None:
        self._stop.set()
        self._thread.join(timeout=5)

    def _loop(self) -> None:
        while not self._stop.is_set():
            self._scan()
            self._stop.wait(self._poll_s)

    def _scan(self) -> None:
        try:
            candidates = sorted(self._inbox.iterdir())
        except FileNotFoundError:
            return
        for path in candidates:
            if not path.is_file() or path.suffix.lower() not in WATCH_EXTS:
                continue
            size = path.stat().st_size
            now = time.monotonic()
            last = self._sizes.get(path)
            if last is None or last[0] != size:
                self._sizes[path] = (size, now)
                continue
            if now - last[1] < self._settle_s:
                continue
            del self._sizes[path]
            self._ingest(path, size)

    def _ingest(self, path: pathlib.Path, size: int) -> None:
        t0 = time.monotonic()
        snapshot = self._store.new_snapshot()
        dest = snapshot.dir / "source" / path.name
        shutil.move(str(path), dest)
        self._ledger.record("file_drop", "{} -> {}".format(path.name, snapshot.id),
                            bytes_moved=size, snapshot=snapshot.id)
        try:
            self._import_asset(dest, snapshot.dir)
            parts = json.loads((snapshot.dir / "parts.json").read_text())["parts"]
        except Exception as exc:  # failure is data; the loop must survive
            self._ledger.record(
                "import_failed", "{}: {}".format(snapshot.id, exc),
                bytes_moved=size, snapshot=snapshot.id,
                latency_ms=(time.monotonic() - t0) * 1000)
            return
        snapshot.parts = list(parts)
        self._store.current = snapshot
        self._ledger.record(
            "import_done", "{}: {} parts".format(snapshot.id, len(parts)),
            bytes_moved=size, latency_ms=(time.monotonic() - t0) * 1000,
            snapshot=snapshot.id)
