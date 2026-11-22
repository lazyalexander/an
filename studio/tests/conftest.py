import os
import time

import pytest


def pytest_collection_modifyitems(config, items):
    if os.environ.get("STUDIO_CONTAINER") == "1":
        return
    skip = pytest.mark.skip(reason="requires STUDIO_CONTAINER=1 and the Apple Container service")
    for item in items:
        if "container" in item.keywords:
            item.add_marker(skip)


def wait_until(predicate, timeout=2.0, interval=0.02):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if predicate():
            return True
        time.sleep(interval)
    raise AssertionError("wait_until timed out")


class FakeClock:
    def __init__(self, start=1_000_000.0):
        self._t = start

    def now(self):
        return self._t

    def advance(self, seconds):
        self._t += seconds


class FakePipeline:
    def __init__(self, barrier=None):
        self.calls = []
        self.barrier = barrier

    def render(self, session, scene_dir, out_dir):
        from orchestrator.pipeline_api import LayerSpec
        self.calls.append(session)
        if self.barrier:
            self.barrier.wait(2)
        return [LayerSpec(part="cube_a", png_path=out_dir / "cube_a.png")]


@pytest.fixture
def running_server(tmp_path):
    import http.client
    import threading

    from orchestrator.app import Orchestrator
    from orchestrator.ledger import Ledger
    from orchestrator.server import make_server

    app = Orchestrator(Ledger(tmp_path / "l.jsonl"), FakePipeline(), (1600, 1200),
                       run_dir=tmp_path / "run")
    srv = make_server(app, port=0)
    thread = threading.Thread(target=srv.serve_forever, daemon=True)
    thread.start()
    conn = http.client.HTTPConnection("127.0.0.1", srv.server_address[1])
    yield conn, app
    conn.close()
    srv.shutdown()
    srv.server_close()


def get(conn, path):
    import json
    conn.request("GET", path)
    response = conn.getresponse()
    return response.status, json.loads(response.read())


def post(conn, path, body):
    import json
    conn.request("POST", path, json.dumps(body), {"Content-Type": "application/json"})
    response = conn.getresponse()
    return response.status, json.loads(response.read())
