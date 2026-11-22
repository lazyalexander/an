import json
import threading

import pytest

from orchestrator.app import Orchestrator
from orchestrator.ledger import Ledger
from orchestrator.phase import InvalidTransition, NoPose, Phase
from orchestrator.pipeline_api import LayerSpec

from conftest import FakeClock, FakePipeline, wait_until


class GatedPipeline:
    """Blocks inside the first render call until released; later calls pass."""

    def __init__(self, first_gate):
        self.calls = []
        self._gate = first_gate

    def render(self, session, scene_dir, out_dir):
        first = not self.calls
        self.calls.append(session)
        if first:
            self._gate.wait(5)
        out_dir.mkdir(parents=True, exist_ok=True)
        path = out_dir / "{}.png".format(session.id)
        path.write_bytes(b"png")
        return [LayerSpec(part="cube_a", png_path=path)]


def test_lock_freezes_pose_and_renders_async(tmp_path):
    led = Ledger(tmp_path / "l.jsonl")
    app = Orchestrator(led, FakePipeline(), resolution=(1600, 1200))
    app.submit_pose({"matrix": [1.0] * 16, "location": [0, 0, 0]})
    s = app.lock()
    assert s.state in ("rendering", "rendered")
    wait_until(lambda: app.session.state == "rendered")
    assert app.layers()[0].part == "cube_a"
    kinds = [e["kind"] for e in led.read_all()]
    assert "pose_stream" in kinds and "lock" in kinds and "render_done" in kinds


def test_pose_rejected_while_locked(tmp_path):
    app = Orchestrator(Ledger(tmp_path / "l.jsonl"), FakePipeline(), (1600, 1200))
    app.submit_pose({"matrix": [1.0] * 16, "location": [0, 0, 0]})
    app.lock()
    with pytest.raises(InvalidTransition):
        app.submit_pose({"matrix": [2.0] * 16, "location": [1, 1, 1]})


def test_pose_aggregation(tmp_path):
    clock = FakeClock()  # manual advance
    app = Orchestrator(Ledger(tmp_path / "l.jsonl"), FakePipeline(), (1600, 1200), now=clock.now)
    for _ in range(25):
        app.submit_pose({"matrix": [0.0] * 16, "location": [0, 0, 0]})
    clock.advance(1.1)
    app.submit_pose({"matrix": [0.0] * 16, "location": [0, 0, 0]})
    clock.advance(1.1)
    app.submit_pose({"matrix": [0.0] * 16, "location": [0, 0, 0]})
    entries = [e for e in (tmp_path / "l.jsonl").read_text().splitlines() if e]
    streams = [json.loads(e) for e in entries if '"pose_stream"' in e]
    assert len(streams) == 2
    assert streams[0]["count"] == 25 and streams[1]["count"] == 1


def test_unlock_returns_to_adjustment(tmp_path):
    app = Orchestrator(Ledger(tmp_path / "l.jsonl"), FakePipeline(), (1600, 1200))
    app.submit_pose({"matrix": [1.0] * 16, "location": [0, 0, 0]})
    app.lock()
    wait_until(lambda: app.session is not None and app.session.state == "rendered")
    app.unlock()
    assert app.phase is Phase.ADJUSTMENT and app.session is None


def test_orphaned_render_does_not_overwrite_new_session(tmp_path):
    gate_s1 = threading.Event()
    pipe = GatedPipeline(gate_s1)
    led = Ledger(tmp_path / "l.jsonl")
    app = Orchestrator(led, pipe, (64, 64), run_dir=tmp_path / "run")
    app.submit_pose({"matrix": [1.0] * 16, "location": [0, 0, 0]})
    s1 = app.lock()
    wait_until(lambda: len(pipe.calls) == 1)  # s1's thread is inside render
    app.unlock()
    app.submit_pose({"matrix": [2.0] * 16, "location": [1, 1, 1]})
    s2 = app.lock()
    wait_until(lambda: s2.state == "rendered")
    gate_s1.set()  # the orphaned render finishes last
    wait_until(lambda: any(e["kind"] == "render_orphaned" for e in led.read_all()))
    assert app.layers()[0].png_path.name == "{}.png".format(s2.id)
    assert s1.state != "rendered"
