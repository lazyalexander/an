"""Port routing: the framework knows only /api/{input,action,read}/<name>."""

import http.client
import threading

from conftest import FakePipeline, get, post
from orchestrator.app import Orchestrator
from orchestrator.ledger import Ledger
from orchestrator.macro.instances import load
from orchestrator.server import make_server


def test_unknown_ports_are_404(running_server):
    c, _ = running_server
    assert post(c, "/api/action/nope?key=testkey", {})[0] == 404
    assert post(c, "/api/input/nope?key=testkey", {})[0] == 404
    assert get(c, "/api/read/nope?key=testkey")[0] == 404


def test_ports_reject_non_object_bodies(running_server):
    c, _ = running_server
    assert post(c, "/api/input/pose?key=testkey", [1, 2, 3])[0] == 400


def test_demo_instance_ports_are_generic(tmp_path):
    instance = load("cad-animation")
    ledger = Ledger(tmp_path / "l.jsonl")
    app = Orchestrator(ledger, FakePipeline(), (640, 480),
                       run_dir=tmp_path / "run")
    srv = make_server(app, port=0, ui_token="testkey",
                      archives_dir=tmp_path / "run" / "archives",
                      ledger=ledger, scenes=None, instance=instance)
    thread = threading.Thread(target=srv.serve_forever, daemon=True)
    thread.start()
    conn = http.client.HTTPConnection("127.0.0.1", srv.server_address[1])
    try:
        code, body = post(conn, "/api/action/approve_rig?key=testkey", {})
        assert code == 200 and body == {"ok": True}
        assert any(e["kind"] == "rig_approved" for e in ledger.read_all())
        code, body = get(conn, "/api/read/timeline?key=testkey")
        assert code == 200 and body == {"frames": []}
        code, body = post(conn, "/api/input/pointer?key=testkey", {"x": 1})
        assert code == 200 and body == {"ok": True}
    finally:
        conn.close()
        srv.shutdown()
        srv.server_close()
        thread.join(timeout=5)
