import zipfile

from conftest import get, get_bytes, post, wait_until

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


def test_manifest_and_state_endpoints(running_server):
    c, _ = running_server
    code, manifest = get(c, "/api/macro/manifest?key=testkey")
    assert code == 200
    assert manifest["instance"] == "bridge-studio"
    assert [stage["id"] for stage in manifest["stages"]] == [
        "import", "pose", "render", "archive"]
    code, state = get(c, "/api/macro/state?key=testkey")
    assert code == 200
    assert "stages" in state and "health" in state
    assert [stage["id"] for stage in state["stages"]] == [
        "import", "pose", "render", "archive"]


def test_preview_404_then_200(running_server, tmp_path):
    c, _ = running_server
    code, body = get(c, "/api/macro/preview.png?key=testkey")
    assert code == 404 and body["error"] == "no preview"
    preview = tmp_path / "run" / "preview" / "current.png"
    preview.parent.mkdir(parents=True, exist_ok=True)
    payload = b"\x89PNG\r\n\x1a\npreview-bytes"
    preview.write_bytes(payload)
    code, content_type, data = get_bytes(c, "/api/macro/preview.png?key=testkey")
    assert code == 200
    assert content_type == "image/png"
    assert data == payload


def test_archive_thumb_serves_kra_mergedimage(running_server, tmp_path):
    c, _ = running_server
    archives = tmp_path / "run" / "archives"
    _write_kra_with_thumbnail(archives, "v1", b"\x89PNG\r\n\x1a\nthumb-bytes")
    code, content_type, data = get_bytes(
        c, "/api/macro/archive-thumb?id=v1&key=testkey")
    assert code == 200
    assert content_type == "image/png"
    assert data == b"\x89PNG\r\n\x1a\nthumb-bytes"


def test_archive_thumb_validates_id(running_server, tmp_path):
    c, _ = running_server
    archives = tmp_path / "run" / "archives"
    _write_kra_with_thumbnail(archives, "v1", b"thumb")
    assert get(c, "/api/macro/archive-thumb?key=testkey")[0] == 400
    assert get(c, "/api/macro/archive-thumb?id=&key=testkey")[0] == 400
    assert get(c, "/api/macro/archive-thumb?id=nope&key=testkey")[0] == 404


def test_archive_thumb_404_without_mergedimage(running_server, tmp_path):
    c, _ = running_server
    _write_archive(tmp_path / "run" / "archives", "v2")  # b"kra": not a zip
    assert get(c, "/api/macro/archive-thumb?id=v2&key=testkey")[0] == 404


def _write_kra_with_thumbnail(archives_dir, version_id, thumb_bytes):
    archives_dir.mkdir(parents=True, exist_ok=True)
    kra = archives_dir / f"pose_{version_id}.kra"
    with zipfile.ZipFile(kra, "w") as archive:
        archive.writestr("mergedimage.png", thumb_bytes)
    write_sidecar(kra, {version_id: PoseRecord(
        pose={}, resolution=(1600, 1200), snapshot_id="",
        created="2026-11-23T00:00:00+00:00")})
    return kra


def test_macro_endpoints_reject_bad_key(running_server):
    c, _ = running_server
    for path in ("/api/macro/manifest", "/api/macro/state",
                 "/api/macro/preview.png", "/api/macro/archive-thumb?id=v1"):
        separator = "&" if "?" in path else "?"
        code, body = get(c, f"{path}{separator}key=wrong")
        assert code == 403 and body["error"] == "bad key", path
        code, body = get(c, path)
        assert code == 403 and body["error"] == "bad key", path
