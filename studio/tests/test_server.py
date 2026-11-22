from conftest import get, post, wait_until


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
