from conftest import get, post, wait_until

from adapter.core.archive import PoseRecord, read_version, write_sidecar


def _write_archive(archives_dir, version_id):
    archives_dir.mkdir(parents=True, exist_ok=True)
    kra = archives_dir / f"pose_{version_id}.kra"
    kra.write_bytes(b"kra")
    write_sidecar(kra, {version_id: PoseRecord(
        pose={}, resolution=(1600, 1200), snapshot_id="",
        created="2026-11-23T00:00:00+00:00")})
    return kra


def test_full_http_cycle(running_server):
    c, app = running_server
    assert post(c, "/api/pose", {"matrix": [1.0] * 16, "location": [0, 0, 0]})[0] == 200
    code, session = post(c, "/api/lock", {})
    assert code == 200 and session["resolution"] == [1600, 1200]
    assert get(c, "/api/status")[1]["phase"] == "LOCKED"
    code, body = post(c, "/api/pose", {"matrix": [2.0] * 16, "location": [1, 1, 1]})
    assert code == 409
    wait_until(lambda: get(c, "/api/status")[1]["session"]["state"] == "rendered")
    code, layers = get(c, "/api/session/layers")
    assert code == 200 and layers["layers"][0]["part"] == "cube_a"
    assert post(c, "/api/unlock", {})[0] == 200


def test_lock_without_pose_is_409(running_server):
    c, _ = running_server
    code, body = post(c, "/api/lock", {})
    assert code == 409 and "no current pose" in body["error"]


def test_malformed_pose_is_400(running_server):
    c, _ = running_server
    assert post(c, "/api/pose", {"matrix": [1.0]})[0] == 400


def test_decisions_require_key(running_server):
    c, _ = running_server  # fixture boots with ui_token="testkey"
    code, body = post(c, "/api/decision/adopt", {})
    assert code == 403 and body["error"] == "bad key"
    code, body = post(c, "/api/decision/adopt?key=testkey", {})
    assert code == 409 and "not locked" in body["error"]  # authorized, bad state
    code, _ = post(c, "/api/decision/adopt", {}, headers={"X-Bridge-Key": "testkey"})
    assert code == 409
    code, _ = post(c, "/api/decision/adopt?key=nope", {})
    assert code == 403
    code, _ = post(c, "/api/decision/adopt", {}, headers={"X-Bridge-Key": "nope"})
    assert code == 403


def test_adopt_rerender_cycle_over_http(running_server, tmp_path):
    c, _ = running_server
    assert post(c, "/api/pose", {"matrix": [1.0] * 16, "location": [0, 0, 0]})[0] == 200
    code, session = post(c, "/api/lock", {})
    assert code == 200
    wait_until(lambda: get(c, "/api/status")[1]["session"]["state"] == "rendered")
    kra = _write_archive(tmp_path / "run" / "archives", session["id"])
    code, body = post(c, "/api/decision/adopt?key=testkey", {})
    assert code == 200 and body == {"ok": True}
    assert read_version(kra)["status"] == "adopted"
    code, body = post(c, "/api/decision/rerender?key=testkey", {})
    assert code == 200 and body == {"ok": True}
    assert read_version(kra)["status"] == "superseded"
    assert get(c, "/api/status")[1]["phase"] == "ADJUSTMENT"


def test_restore_flow_over_http(running_server, tmp_path):
    c, _ = running_server
    _write_archive(tmp_path / "run" / "archives", "s1")
    code, body = post(c, "/api/decision/restore?key=testkey", {"version": "s1"})
    assert code == 200 and body["id"].startswith("s1--restore-")
    assert body["kra"].endswith(".kra")
    assert get(c, "/api/status")[1]["pending_restore"]["id"] == body["id"]
    code, ack = post(c, "/api/decision/restore-ack?key=testkey", {"id": body["id"]})
    assert code == 200 and ack == {"ok": True}
    assert get(c, "/api/status")[1]["pending_restore"] is None


def test_restore_validates_input(running_server):
    c, _ = running_server
    assert post(c, "/api/decision/restore?key=testkey", {})[0] == 400
    assert post(c, "/api/decision/restore?key=testkey", {"version": "  "})[0] == 400
    assert post(c, "/api/decision/restore?key=testkey", {"version": "nope"})[0] == 409


def test_heartbeat_over_http(running_server):
    c, app = running_server
    assert post(c, "/api/macro/heartbeat", {})[0] == 403
    code, body = post(c, "/api/macro/heartbeat?key=testkey", {})
    assert code == 200 and body == {"ok": True}
    assert app.heartbeat_age_ms() is not None
