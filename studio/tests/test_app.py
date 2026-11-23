import json
import re
import threading
from pathlib import Path

import pytest

from adapter.core.archive import (
    PoseRecord,
    list_versions,
    read_version,
    write_sidecar,
)
from orchestrator.app import Orchestrator
from orchestrator.ledger import Ledger
from orchestrator.phase import InvalidTransition, Phase
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


class FlakyPipeline:
    """Fails the first render call; later calls succeed."""

    def __init__(self):
        self.calls = []

    def render(self, session, scene_dir, out_dir):
        self.calls.append(session)
        if len(self.calls) == 1:
            raise RuntimeError("boom")
        out_dir.mkdir(parents=True, exist_ok=True)
        path = out_dir / "cube_a.png"
        path.write_bytes(b"png")
        return [LayerSpec(part="cube_a", png_path=path)]


def _write_archive(archives_dir, session_id):
    archives_dir.mkdir(parents=True, exist_ok=True)
    kra = archives_dir / f"pose_{session_id}.kra"
    kra.write_bytes(b"kra")
    write_sidecar(kra, {session_id: PoseRecord(
        pose={}, resolution=(1600, 1200), snapshot_id="",
        created="2026-11-23T00:00:00+00:00")})
    return kra


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


def test_unlock_is_an_adapter_acked_decision(tmp_path):
    app = Orchestrator(Ledger(tmp_path / "l.jsonl"), FakePipeline(), (1600, 1200))
    app.submit_pose({"matrix": [1.0] * 16, "location": [0, 0, 0]})
    app.lock()
    wait_until(lambda: app.session is not None and app.session.state == "rendered")
    assert app.unlock() == {"kind": "unlock", "session": app.session.id}
    # the phase must not move before the adapter has saved the live canvas
    assert app.phase is Phase.LOCKED and app.session is not None
    app.ack_decision("unlock")
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
    app.ack_decision("unlock")
    app.submit_pose({"matrix": [2.0] * 16, "location": [1, 1, 1]})
    s2 = app.lock()
    wait_until(lambda: s2.state == "rendered")
    gate_s1.set()  # the orphaned render finishes last
    wait_until(lambda: any(e["kind"] == "render_orphaned" for e in led.read_all()))
    assert app.layers()[0].png_path.name == "{}.png".format(s2.id)
    assert s1.state != "rendered"


def test_adopt_is_an_adapter_acked_decision(tmp_path):
    clock = FakeClock()
    led = Ledger(tmp_path / "l.jsonl")
    archives = tmp_path / "archives"
    app = Orchestrator(led, FakePipeline(), (1600, 1200), now=clock.now,
                       run_dir=tmp_path / "run", archives_dir=archives)
    app.submit_pose({"matrix": [1.0] * 16, "location": [0, 0, 0]})
    s = app.lock()
    wait_until(lambda: s.state == "rendered")
    kra = _write_archive(archives, s.id)
    clock.advance(1.5)
    assert app.adopt() == {"kind": "adopt", "session": s.id}
    # request only: the painted archive is still untouched
    assert read_version(kra)["status"] == "pushed"
    app.ack_decision("adopt")
    assert read_version(kra)["status"] == "adopted"
    entries = led.read_all()
    assert [e["kind"] for e in entries if e["kind"].startswith("decision")] == [
        "decision_request", "decision"]
    decisions = [e for e in entries if e["kind"] == "decision"]
    assert decisions[0]["decision"] == "adopt"
    assert decisions[0]["session"] == s.id
    assert decisions[0]["latency_ms"] >= 0


def test_adopt_before_push_is_refused(tmp_path):
    app = Orchestrator(Ledger(tmp_path / "l.jsonl"), FakePipeline(), (1600, 1200),
                       run_dir=tmp_path / "run", archives_dir=tmp_path / "archives")
    app.submit_pose({"matrix": [1.0] * 16, "location": [0, 0, 0]})
    s = app.lock()
    wait_until(lambda: s.state == "rendered")
    with pytest.raises(InvalidTransition, match="archive not pushed yet"):
        app.adopt()


def test_adopt_refuses_second_adopt(tmp_path):
    archives = tmp_path / "archives"
    app = Orchestrator(Ledger(tmp_path / "l.jsonl"), FakePipeline(), (1600, 1200),
                       run_dir=tmp_path / "run", archives_dir=archives)
    app.submit_pose({"matrix": [1.0] * 16, "location": [0, 0, 0]})
    s = app.lock()
    wait_until(lambda: s.state == "rendered")
    _write_archive(archives, s.id)
    app.adopt()
    with pytest.raises(InvalidTransition, match="decision pending"):
        app.adopt()
    app.ack_decision("adopt")
    with pytest.raises(InvalidTransition, match="already adopted"):
        app.adopt()


def test_decision_requires_a_matching_request(tmp_path):
    archives = tmp_path / "archives"
    app = Orchestrator(Ledger(tmp_path / "l.jsonl"), FakePipeline(), (1600, 1200),
                       run_dir=tmp_path / "run", archives_dir=archives)
    with pytest.raises(InvalidTransition, match="no matching pending decision"):
        app.ack_decision("unlock")
    app.submit_pose({"matrix": [1.0] * 16, "location": [0, 0, 0]})
    s = app.lock()
    wait_until(lambda: s.state == "rendered")
    _write_archive(archives, s.id)
    app.adopt()
    with pytest.raises(InvalidTransition, match="no matching pending decision"):
        app.ack_decision("unlock")


def test_failed_ack_clears_pending_and_records(tmp_path):
    led = Ledger(tmp_path / "l.jsonl")
    app = Orchestrator(led, FakePipeline(), (1600, 1200),
                       run_dir=tmp_path / "run", archives_dir=tmp_path / "archives")
    app.submit_pose({"matrix": [1.0] * 16, "location": [0, 0, 0]})
    s = app.lock()
    wait_until(lambda: s.state == "rendered")
    app.rerender()
    assert app.ack_decision("re-render", False, "save failed") == {"ok": False}
    assert app.pending_decision is None
    assert app.phase is Phase.LOCKED  # nothing advanced on failure
    failed = [e for e in led.read_all() if e["kind"] == "decision_failed"]
    assert failed and "save failed" in failed[0]["summary"]


def test_rerender_is_adapter_acked_and_returns_to_adjustment(tmp_path):
    led = Ledger(tmp_path / "l.jsonl")
    archives = tmp_path / "archives"
    app = Orchestrator(led, FakePipeline(), (1600, 1200),
                       run_dir=tmp_path / "run", archives_dir=archives)
    app.submit_pose({"matrix": [1.0] * 16, "location": [0, 0, 0]})
    s = app.lock()
    wait_until(lambda: s.state == "rendered")
    kra = _write_archive(archives, s.id)
    assert app.rerender() == {"kind": "re-render", "session": s.id}
    # nothing moves until the adapter has preserved the painted canvas
    assert app.phase is Phase.LOCKED and app.session is not None
    assert read_version(kra)["status"] == "pushed"
    app.ack_decision("re-render")
    assert app.phase is Phase.ADJUSTMENT
    assert app.session is None
    assert read_version(kra)["status"] == "superseded"
    entries = led.read_all()
    decisions = [e for e in entries if e["kind"] == "decision"]
    assert len(decisions) == 1
    assert decisions[0]["decision"] == "re-render"
    assert decisions[0]["session"] == s.id
    assert not any(e["kind"] == "unlock" for e in entries)


def test_rerender_records_lock_duration(tmp_path):
    clock = FakeClock()
    led = Ledger(tmp_path / "l.jsonl")
    app = Orchestrator(led, FakePipeline(), (1600, 1200), now=clock.now,
                       run_dir=tmp_path / "run", archives_dir=tmp_path / "archives")
    app.submit_pose({"matrix": [1.0] * 16, "location": [0, 0, 0]})
    s = app.lock()
    wait_until(lambda: s.state == "rendered")
    clock.advance(2)
    app.rerender()
    app.ack_decision("re-render")
    decisions = [e for e in led.read_all() if e["kind"] == "decision"]
    assert decisions[0]["decision"] == "re-render"
    assert decisions[0]["lock_duration_ms"] > 0


def test_retry_rerenders_failed_session(tmp_path):
    led = Ledger(tmp_path / "l.jsonl")
    pipe = FlakyPipeline()
    app = Orchestrator(led, pipe, (1600, 1200), run_dir=tmp_path / "run")
    app.submit_pose({"matrix": [1.0] * 16, "location": [0, 0, 0]})
    s = app.lock()
    wait_until(lambda: s.state == "failed")
    app.retry()
    assert s.state in ("rendering", "rendered")
    wait_until(lambda: s.state == "rendered")
    assert len(pipe.calls) == 2
    assert pipe.calls[0] is pipe.calls[1]
    kinds = [e["kind"] for e in led.read_all()]
    assert "render_failed" in kinds and "render_retry" in kinds


def test_restore_copies_to_new_version(tmp_path):
    led = Ledger(tmp_path / "l.jsonl")
    archives = tmp_path / "archives"
    app = Orchestrator(led, FakePipeline(), (1600, 1200),
                       run_dir=tmp_path / "run", archives_dir=archives)
    src = _write_archive(archives, "s1")
    pending = app.restore("s1")
    assert re.fullmatch(r"s1--restore-\d{8}-\d{6}", pending["id"])
    new_kra = Path(pending["kra"])
    assert new_kra.exists() and "--restore-" in new_kra.name
    assert new_kra.read_bytes() == src.read_bytes()
    restored = read_version(new_kra)
    assert restored["status"] == "pushed"
    assert restored["restored_from"] == "s1"
    assert read_version(src)["restored_from"] is None  # original untouched
    assert app.pending_restore == pending
    app.ack_restore()
    assert app.pending_restore is None
    assert len(list_versions(archives)) == 2
    decisions = [e for e in led.read_all() if e["kind"] == "decision"]
    assert len(decisions) == 1 and decisions[0]["decision"] == "restore"


def test_restore_unknown_version_refused(tmp_path):
    app = Orchestrator(Ledger(tmp_path / "l.jsonl"), FakePipeline(), (1600, 1200),
                       run_dir=tmp_path / "run", archives_dir=tmp_path / "archives")
    with pytest.raises(InvalidTransition):
        app.restore("nope")


def test_force_unlock_applies_when_the_adapter_is_offline(tmp_path):
    led = Ledger(tmp_path / "l.jsonl")
    app = Orchestrator(led, FakePipeline(), (1600, 1200))
    app.submit_pose({"matrix": [1.0] * 16, "location": [0, 0, 0]})
    app.lock()
    wait_until(lambda: app.session is not None and app.session.state == "rendered")
    assert app.unlock(force=True) == {"ok": True}
    assert app.phase is Phase.ADJUSTMENT and app.session is None
    assert app.pending_decision is None
    kinds = [e["kind"] for e in led.read_all()]
    assert "decision_forced" in kinds and "unlock" in kinds


def test_force_unlock_does_not_bypass_a_live_adapter(tmp_path):
    app = Orchestrator(Ledger(tmp_path / "l.jsonl"), FakePipeline(), (1600, 1200))
    app.note_heartbeat(None)
    app.submit_pose({"matrix": [1.0] * 16, "location": [0, 0, 0]})
    app.lock()
    wait_until(lambda: app.session is not None and app.session.state == "rendered")
    assert app.unlock(force=True)["kind"] == "unlock"
    assert app.phase is Phase.LOCKED  # a live adapter must save first
    app.ack_decision("unlock")
    assert app.phase is Phase.ADJUSTMENT
    clock = FakeClock()
    app = Orchestrator(Ledger(tmp_path / "l.jsonl"), FakePipeline(), (1600, 1200),
                       now=clock.now)
    assert app.heartbeat_age_ms() is None
    app.note_heartbeat()
    assert app.heartbeat_age_ms() == 0
    clock.advance(4)
    assert app.heartbeat_age_ms() >= 3000


def test_unlock_records_lock_duration(tmp_path):
    clock = FakeClock()
    led = Ledger(tmp_path / "l.jsonl")
    app = Orchestrator(led, FakePipeline(), (1600, 1200), now=clock.now)
    app.submit_pose({"matrix": [1.0] * 16, "location": [0, 0, 0]})
    app.lock()
    wait_until(lambda: app.session is not None and app.session.state == "rendered")
    clock.advance(3)
    app.unlock()
    app.ack_decision("unlock")
    unlocks = [e for e in led.read_all() if e["kind"] == "unlock"]
    assert len(unlocks) == 1
    assert unlocks[0]["lock_duration_ms"] == 3000
