"""Instance contract tests: a new instance is data + a small provider.

The framework (state builder, page, server) must not know any instance's
stage ids, decision kinds, drawer sections, surface names or health copy.
These tests lock that: every registered instance must satisfy the same
contract, and the static page must carry no instance-specific paths.
"""

import http.client
import threading
from pathlib import Path

import pytest
from conftest import FakeClock, FakePipeline, get
from orchestrator.app import Orchestrator
from orchestrator.ledger import Ledger
from orchestrator.macro.instances import INSTANCES, load, names
from orchestrator.macro.state import build_state
from orchestrator.scenes import SceneStore

STATIC = Path(__file__).resolve().parent.parent / "orchestrator" / "macro" / "static"


def _rig(tmp_path):
    clock = FakeClock()
    ledger = Ledger(tmp_path / "l.jsonl", now=clock.now)
    scenes = SceneStore(tmp_path / "scene")
    app = Orchestrator(ledger, FakePipeline(), (640, 480), now=clock.now,
                       run_dir=tmp_path / "run", scenes=scenes,
                       archives_dir=tmp_path / "archives")
    return clock, ledger, scenes, app


def _state(tmp_path, instance):
    clock, ledger, scenes, app = _rig(tmp_path)
    return build_state(app, tmp_path / "archives", ledger, scenes,
                       instance=instance, now=clock.now)


def test_registry_lists_and_loads_instances():
    assert names() == sorted(INSTANCES)
    assert {"bridge-studio", "cad-animation"} <= set(names())
    assert load("bridge-studio") is INSTANCES["bridge-studio"]
    with pytest.raises(ValueError) as excinfo:
        load("nope")
    assert "bridge-studio" in str(excinfo.value)


@pytest.mark.parametrize("name", names())
def test_every_instance_manifest_matches_the_contract(tmp_path, name):
    instance = load(name)
    manifest = instance.MANIFEST
    assert manifest["instance"] == name
    assert isinstance(manifest["title"], str) and manifest["title"].strip()
    stage_ids = [stage["id"] for stage in manifest["stages"]]
    assert stage_ids and len(set(stage_ids)) == len(stage_ids)
    for stage in manifest["stages"]:
        assert isinstance(stage["label"], str) and stage["label"].strip()
        assert stage["kind"] in {"headless", "interaction", "artifact"}
    surface_ids = [surface["id"] for surface in manifest["surfaces"]]
    assert len(set(surface_ids)) == len(surface_ids)
    for surface in manifest["surfaces"]:
        assert surface["kind"] in {"native", "pixels"}
        assert surface["phase"] in {"ADJUSTMENT", "LOCKED"}
        assert isinstance(surface["inputs"], list)
    for decision in manifest["decisions"]:
        assert decision["kind"] and decision["label"]
        assert decision["actions"]
        for action in decision["actions"]:
            assert action["label"]
            assert isinstance(action["action"], str) and action["action"]
    for section in manifest["drawer"]:
        assert section["id"] and section["label"]
        assert section["kinds"] and section["format"] in {"summary", "entry"}
    for channel in ("input", "output"):
        assert isinstance(manifest["ports"][channel], list)
    assert isinstance(manifest["archive"]["label"], str)
    assert isinstance(manifest["health"], dict)
    assert callable(instance.health)
    assert callable(instance.stages) and callable(instance.pending)

    _, _, _, app = _rig(tmp_path / "ports")
    actions = instance.actions(app)
    assert isinstance(actions, dict) and all(callable(h) for h in actions.values())
    for decision in manifest["decisions"]:
        for action in decision["actions"]:
            assert action["action"] in actions, action["action"]
    if "restore" in manifest:
        assert manifest["restore"]["action"] in actions
    assert all(callable(h) for h in instance.inputs(app).values())
    assert all(callable(h) for h in instance.reads(app).values())
    assert callable(instance.thumbnail)

    state = _state(tmp_path, instance)
    assert state["instance"] == name
    assert [stage["id"] for stage in state["stages"]] == stage_ids
    assert set(state["drawer"]) == {section["id"] for section in manifest["drawer"]}
    assert [surface["id"] for surface in state["surfaces"]] == surface_ids
    assert set(state["health"]) <= set(manifest["health"])


def test_demo_instance_runs_without_framework_changes(tmp_path):
    state = _state(tmp_path, load("cad-animation"))
    assert [stage["id"] for stage in state["stages"]] == [
        "import", "rig", "animate", "export"]
    assert "pose" not in {stage["id"] for stage in state["stages"]}
    assert [section for section in state["drawer"]] == ["imports", "failures"]
    assert state["ports"]["input"] == ["pointer", "decision"]
    # the demo has no pose channel, so the page must not invent one
    assert set(state["health"]) == {"adapter", "pipeline"}


def test_demo_drawer_uses_its_own_kinds(tmp_path):
    clock, ledger, scenes, app = _rig(tmp_path)
    ledger.record("pose_stream", "8 poses in 1s window")
    ledger.record("file_drop", "a.obj -> snap-0001")
    ledger.record("import_done", "snap-0001: 2 parts")
    state = build_state(app, tmp_path / "archives", ledger, scenes,
                        instance=load("cad-animation"), now=clock.now)
    assert "poses" not in state["drawer"]
    assert state["drawer"]["imports"] == ["snap-0001: 2 parts",
                                          "a.obj -> snap-0001"]


def test_surfaces_track_the_phase(tmp_path):
    clock, ledger, scenes, app = _rig(tmp_path)
    scenes.current = scenes.new_snapshot()
    adjustment = build_state(app, tmp_path / "archives", ledger, scenes,
                             now=clock.now)
    assert [(s["id"], s["active"]) for s in adjustment["surfaces"]] == [
        ("adjust", True), ("paint", False)]
    app.submit_pose({"matrix": [1.0] * 16, "location": [0, 0, 0]})
    session = app.lock()
    locked = build_state(app, tmp_path / "archives", ledger, scenes,
                         now=clock.now)
    from conftest import wait_until
    wait_until(lambda: session.state == "rendered")
    assert [(s["id"], s["active"]) for s in locked["surfaces"]] == [
        ("adjust", False), ("paint", True)]
    assert locked["ports"]["active_surface"] == "paint"


def test_ports_and_health_copy_come_from_the_manifest(tmp_path):
    bridge = _state(tmp_path, load("bridge-studio"))
    assert bridge["ports"] == {
        "input": ["pen", "decision"],
        "output": ["preview", "archive"],
        "active_surface": "adjust",
    }
    assert bridge["health_copy"]["adapter"] == "Krita 未连接"
    demo = _state(tmp_path / "demo", load("cad-animation"))
    assert demo["health_copy"]["adapter"] == "审片器未连接"


def test_page_carries_no_instance_specific_paths():
    script = (STATIC / "app.js").read_text(encoding="utf-8")
    for path in ("/api/decision/adopt", "/api/decision/rerender",
                 "/api/decision/retry", "/api/decision/restore",
                 "adopt_or_rerender", "Krita", "Blender"):
        assert path not in script, path
    html = (STATIC / "index.html").read_text(encoding="utf-8")
    assert 'id="decision-bar"' in html
    for path in ("/api/decision/adopt", "/api/decision/restore"):
        assert path not in html, path


def test_server_serves_the_injected_instance(tmp_path):
    instance = load("cad-animation")
    ledger = Ledger(tmp_path / "l.jsonl")
    app = Orchestrator(ledger, FakePipeline(), (640, 480),
                       run_dir=tmp_path / "run")
    from orchestrator.server import make_server
    srv = make_server(app, port=0, ui_token="testkey",
                      archives_dir=tmp_path / "run" / "archives",
                      ledger=ledger, scenes=None, instance=instance)
    thread = threading.Thread(target=srv.serve_forever, daemon=True)
    thread.start()
    conn = http.client.HTTPConnection("127.0.0.1", srv.server_address[1])
    try:
        code, manifest = get(conn, "/api/macro/manifest?key=testkey")
        assert code == 200 and manifest["instance"] == "cad-animation"
        code, state = get(conn, "/api/macro/state?key=testkey")
        assert code == 200 and state["instance"] == "cad-animation"
        assert [stage["id"] for stage in state["stages"]] == [
            "import", "rig", "animate", "export"]
    finally:
        conn.close()
        srv.shutdown()
        srv.server_close()
        thread.join(timeout=5)
