import json
import threading
import time

import pytest

from orchestrator.app import Orchestrator
from orchestrator.ledger import Ledger
from orchestrator.phase import InvalidTransition
from orchestrator.scenes import SceneStore
from orchestrator.watch import Watcher

from conftest import FakePipeline, wait_until


def fake_import(calls):
    def _import(src, out):
        import json
        calls.append((src, out))
        (out / "parts.json").write_text(json.dumps({"parts": ["cube_a", "cube_b"]}))
    return _import


def test_stable_file_triggers_import(tmp_path):
    store = SceneStore(tmp_path / "scene")
    imports = []
    w = Watcher(tmp_path / "inbox", store, fake_import(imports),
                Ledger(tmp_path / "l.jsonl"), poll_s=0.05, settle_s=0.05)
    w.start()
    (tmp_path / "inbox").mkdir(exist_ok=True)
    (tmp_path / "inbox" / "two_parts.obj").write_text("o cube_a\n")
    wait_until(lambda: len(imports) == 1, timeout=3)
    w.stop()
    assert store.current is not None and store.current.id == "snap-0001"
    src = imports[0][0]
    assert src.parent.name == "source" and src.exists()  # moved, not copied


def test_partial_file_waits_for_stability(tmp_path):
    store = SceneStore(tmp_path / "scene")
    imports = []
    w = Watcher(tmp_path / "inbox", store, fake_import(imports),
                Ledger(tmp_path / "l.jsonl"), poll_s=0.05, settle_s=0.2)
    (tmp_path / "inbox").mkdir(exist_ok=True)
    target = tmp_path / "inbox" / "part.obj"
    target.write_text("o cube_a\n")
    w.start()
    stop_writing = threading.Event()

    def append_forever():
        while not stop_writing.is_set():
            with target.open("a") as fh:
                fh.write("v 0 0 0\n")
            time.sleep(0.03)

    writer = threading.Thread(target=append_forever, daemon=True)
    writer.start()
    time.sleep(0.4)
    assert imports == []  # still appending: never size-stable
    stop_writing.set()
    writer.join()
    wait_until(lambda: len(imports) == 1, timeout=3)
    w.stop()


def test_redrop_creates_new_snapshot_never_overwrites(tmp_path):
    store = SceneStore(tmp_path / "scene")
    imports = []
    w = Watcher(tmp_path / "inbox", store, fake_import(imports),
                Ledger(tmp_path / "l.jsonl"), poll_s=0.05, settle_s=0.05)
    (tmp_path / "inbox").mkdir(exist_ok=True)
    w.start()
    (tmp_path / "inbox" / "a.obj").write_text("o cube_a\n")
    wait_until(lambda: len(imports) == 1, timeout=3)
    (tmp_path / "inbox" / "b.obj").write_text("o cube_b\n")
    wait_until(lambda: len(imports) == 2, timeout=3)
    w.stop()
    assert (store.root / "snap-0001").is_dir()
    assert (store.root / "snap-0002").is_dir()
    for snap_id, name in (("snap-0001", "a.obj"), ("snap-0002", "b.obj")):
        assert (store.root / snap_id / "source" / name).exists()
    assert store.current.id == "snap-0002"


def test_lock_without_scene_raises(tmp_path):
    app = Orchestrator(Ledger(tmp_path / "l.jsonl"), FakePipeline(), (1600, 1200),
                       run_dir=tmp_path / "run", scenes=SceneStore(tmp_path / "scene"))
    app.submit_pose({"matrix": [1.0] * 16, "location": [0, 0, 0]})
    with pytest.raises(InvalidTransition):
        app.lock()


def test_failed_import_is_data_and_watcher_survives(tmp_path):
    store = SceneStore(tmp_path / "scene")
    ledger = Ledger(tmp_path / "l.jsonl")
    calls = []

    def fail_first(src, out):
        calls.append(src.name)
        if len(calls) == 1:
            raise RuntimeError("container service stopped")
        (out / "parts.json").write_text(json.dumps({"parts": ["cube_a"]}))

    w = Watcher(tmp_path / "inbox", store, fail_first, ledger,
                poll_s=0.05, settle_s=0.05)
    (tmp_path / "inbox").mkdir(exist_ok=True)
    w.start()
    (tmp_path / "inbox" / "bad.obj").write_text("o bad\n")
    wait_until(lambda: any(e["kind"] == "import_failed"
                           for e in ledger.read_all()), timeout=3)
    (tmp_path / "inbox" / "good.obj").write_text("o good\n")
    wait_until(lambda: store.current is not None, timeout=3)  # loop alive
    w.stop()
    assert calls == ["bad.obj", "good.obj"]
    snap_ids = sorted(p.name for p in store.root.iterdir())
    assert snap_ids == ["snap-0001", "snap-0002"]  # nothing overwritten


def test_lock_binds_current_snapshot(tmp_path):
    store = SceneStore(tmp_path / "scene")
    snapshot = store.new_snapshot()
    app = Orchestrator(Ledger(tmp_path / "l.jsonl"), FakePipeline(), (1600, 1200),
                       run_dir=tmp_path / "run", scenes=store)
    store.current = snapshot
    app.submit_pose({"matrix": [1.0] * 16, "location": [0, 0, 0]})
    session = app.lock()
    assert session.snapshot_id == "snap-0001"
