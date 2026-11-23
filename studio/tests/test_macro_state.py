import json

from conftest import FakeClock, FakePipeline, wait_until

from adapter.core.archive import PoseRecord, write_sidecar
from orchestrator.app import Orchestrator
from orchestrator.ledger import Ledger
from orchestrator.macro.manifest import MANIFEST
from orchestrator.macro.state import build_state
from orchestrator.scenes import SceneStore


class FailingPipeline:
    def render(self, session, scene_dir, out_dir):
        raise RuntimeError("boom")


class _Rig:
    """Orchestrator + collaborators wired against tmp paths on a FakeClock."""

    def __init__(self, tmp_path, pipeline=None):
        self.clock = FakeClock()
        self.ledger = Ledger(tmp_path / "l.jsonl", now=self.clock.now)
        self.scenes = SceneStore(tmp_path / "scene")
        self.archives = tmp_path / "archives"
        self.run = tmp_path / "run"
        self.app = Orchestrator(self.ledger, pipeline or FakePipeline(),
                                (1600, 1200), now=self.clock.now,
                                run_dir=self.run, scenes=self.scenes,
                                archives_dir=self.archives)

    def state(self, **kwargs):
        return build_state(self.app, self.archives, self.ledger, self.scenes,
                           now=self.clock.now, **kwargs)


def _write_archive(archives_dir, session_id):
    archives_dir.mkdir(parents=True, exist_ok=True)
    kra = archives_dir / f"pose_{session_id}.kra"
    kra.write_bytes(b"kra")
    write_sidecar(kra, {session_id: PoseRecord(
        pose={}, resolution=(1600, 1200), snapshot_id="",
        created="2026-11-23T00:00:00+00:00")})
    return kra


def _stages(state):
    return {stage["id"]: stage for stage in state["stages"]}


def _imported(rig):
    rig.scenes.current = rig.scenes.new_snapshot()
    return rig.scenes.current


def _locked_rendered(rig):
    _imported(rig)
    rig.app.submit_pose({"matrix": [1.0] * 16, "location": [0, 0, 0]})
    session = rig.app.lock()
    wait_until(lambda: session.state == "rendered")
    return session


def test_stages_mapping_adjustment_idle(tmp_path):
    rig = _Rig(tmp_path)
    state = rig.state()
    assert [s["id"] for s in state["stages"]] == ["import", "pose", "render", "archive"]
    stages = _stages(state)
    assert stages["import"] == {"id": "import", "status": "waiting", "detail": None}
    assert stages["pose"] == {"id": "pose", "status": "waiting", "detail": None}
    assert stages["render"] == {"id": "render", "status": "waiting", "detail": None}
    assert stages["archive"] == {"id": "archive", "status": "waiting", "detail": 0}
    assert state["phase"] == "ADJUSTMENT"
    assert state["pose_age_ms"] is None
    assert state["session"] is None
    assert state["pending"] == []
    assert state["archive"] == []
    assert state["preview"] is None


def test_pending_after_render(tmp_path):
    rig = _Rig(tmp_path)
    session = _locked_rendered(rig)
    _write_archive(rig.archives, session.id)
    state = rig.state()
    assert state["pending"] == [{"kind": "adopt_or_rerender", "session": session.id}]
    stages = _stages(state)
    assert stages["render"] == {"id": "render", "status": "done", "detail": session.id}
    assert stages["archive"] == {"id": "archive", "status": "done", "detail": 1}
    entry = state["archive"][0]
    assert entry["id"] == session.id
    assert entry["thumb"] == f"/api/macro/archive-thumb?id={session.id}"
    json.dumps(state)  # the whole state is one JSON dict


def test_pending_cleared_after_adopt(tmp_path):
    rig = _Rig(tmp_path)
    session = _locked_rendered(rig)
    _write_archive(rig.archives, session.id)
    rig.app.adopt()
    state = rig.state()
    assert state["pending"] == []
    assert state["archive"][0]["status"] == "adopted"


def test_drawer_bounded(tmp_path):
    rig = _Rig(tmp_path)
    for i in range(300):
        rig.ledger.record("pose_stream", f"{i} poses in 1s window", count=i)
    state = rig.state()
    poses = state["drawer"]["poses"]
    assert len(poses) <= 200
    assert poses[0] == "299 poses in 1s window"  # newest first
    assert poses[-1] == "100 poses in 1s window"


def test_health_adapter_stale(tmp_path):
    rig = _Rig(tmp_path)
    state = rig.state()
    assert state["health"]["adapter"] == {"ok": False, "stale_ms": None}
    assert state["health"]["pose"] == {"ok": False}
    assert state["health"]["pipeline"] == {"ok": True, "detail": "unmonitored"}


def test_failed_render_stage_and_retry_pending(tmp_path):
    rig = _Rig(tmp_path, pipeline=FailingPipeline())
    _imported(rig)
    rig.app.submit_pose({"matrix": [1.0] * 16, "location": [0, 0, 0]})
    session = rig.app.lock()
    wait_until(lambda: session.state == "failed")
    state = rig.state()
    stages = _stages(state)
    assert stages["render"] == {"id": "render", "status": "failed",
                                "detail": session.id}
    assert state["pending"] == [{"kind": "retry", "session": session.id}]
    failures = state["drawer"]["failures"]
    assert len(failures) == 1
    assert failures[0]["kind"] == "render_failed"
    assert "boom" in failures[0]["summary"]


def test_manifest_shape():
    assert MANIFEST == {
        "instance": "bridge-studio",
        "title": "Bridge Studio",
        "stages": [{"id": "import", "label": "导入"},
                   {"id": "pose", "label": "位姿"},
                   {"id": "render", "label": "渲染"},
                   {"id": "archive", "label": "建档"}],
        "drawer": [{"id": "poses", "label": "位姿历史"},
                   {"id": "snapshots", "label": "快照"},
                   {"id": "failures", "label": "失败记录"}],
    }


def test_import_stage_failed_then_recovered(tmp_path):
    rig = _Rig(tmp_path)
    snapshot = _imported(rig)
    rig.ledger.record("import_done", "snap-0001: 2 parts", snapshot="snap-0001")
    rig.clock.advance(1)
    rig.ledger.record("import_failed", "snap-0002: boom", snapshot="snap-0002")
    stages = _stages(rig.state())
    assert stages["import"] == {"id": "import", "status": "failed",
                                "detail": "snap-0002: boom"}
    rig.clock.advance(1)
    rig.ledger.record("import_done", "snap-0002: 1 parts", snapshot="snap-0002")
    stages = _stages(rig.state())
    assert stages["import"] == {"id": "import", "status": "done",
                                "detail": snapshot.id}


def test_drawer_categories_newest_first(tmp_path):
    rig = _Rig(tmp_path)
    rig.ledger.record("file_drop", "a.obj -> snap-0001", snapshot="snap-0001")
    rig.ledger.record("import_done", "snap-0001: 2 parts", snapshot="snap-0001")
    rig.ledger.record("pose_stream", "3 poses in 1s window", count=3)
    rig.ledger.record("render_failed", "session s-1: boom", session="s-1")
    rig.ledger.record("import_failed", "snap-0002: nope", snapshot="snap-0002")
    rig.ledger.record("lock", "session s-1 frozen at streamed pose", session="s-1")
    drawer = rig.state()["drawer"]
    assert drawer["poses"] == ["3 poses in 1s window"]
    assert drawer["snapshots"] == ["snap-0001: 2 parts", "a.obj -> snap-0001"]
    assert [f["kind"] for f in drawer["failures"]] == ["import_failed", "render_failed"]


def test_preview_and_pose_stage(tmp_path):
    rig = _Rig(tmp_path)
    assert rig.state()["preview"] is None
    preview = rig.run / "preview" / "current.png"
    preview.parent.mkdir(parents=True)
    preview.write_bytes(b"png")
    rig.app.submit_pose({"matrix": [1.0] * 16, "location": [0, 0, 0]})
    state = rig.state()
    assert state["preview"] == "/api/macro/preview.png"
    assert state["health"]["pose"] == {"ok": True}
    stages = _stages(state)
    assert stages["pose"] == {"id": "pose", "status": "running", "detail": 0}
    rig.clock.advance(3)
    state = rig.state()
    assert _stages(state)["pose"] == {"id": "pose", "status": "waiting",
                                      "detail": 3000}
    assert state["health"]["pose"] == {"ok": False}


def test_health_adapter_live_then_stale(tmp_path):
    rig = _Rig(tmp_path)
    rig.app.note_heartbeat()
    assert rig.state()["health"]["adapter"] == {"ok": True, "stale_ms": 0}
    rig.clock.advance(4)
    assert rig.state()["health"]["adapter"] == {"ok": False, "stale_ms": 4000}


def test_locked_frozen_pose_stream_is_not_an_alarm(tmp_path):
    rig = _Rig(tmp_path)
    _locked_rendered(rig)
    rig.clock.advance(200)  # pose stream frozen by design while LOCKED
    state = rig.state()
    assert state["phase"] == "LOCKED"
    assert state["pose_age_ms"] == 200_000  # stale, yet not a disconnect
    assert state["health"]["pose"] == {"ok": True}
    assert _stages(state)["pose"] == {"id": "pose", "status": "done",
                                      "detail": "frozen"}


def test_locked_pose_stage_is_never_waiting(tmp_path):
    rig = _Rig(tmp_path)
    _locked_rendered(rig)  # fresh pose (age 0), still LOCKED
    state = rig.state()
    stage = _stages(state)["pose"]
    assert stage["status"] != "waiting"
    assert stage == {"id": "pose", "status": "done", "detail": "frozen"}
    assert state["health"]["pose"]["ok"] is True


def test_build_state_does_not_mutate(tmp_path):
    rig = _Rig(tmp_path)
    _imported(rig)
    rig.app.submit_pose({"matrix": [1.0] * 16, "location": [0, 0, 0]})
    before_entries = rig.ledger.read_all()
    before_status = rig.app.status()
    rig.state()
    assert rig.ledger.read_all() == before_entries
    assert rig.app.status() == before_status
