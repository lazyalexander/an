"""Exercise the shipped CLI against a counted, controllable loopback model."""
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

AN = Path(__file__).resolve().parents[1] / "target/debug/an"


def exercise(root, server):
    env = {**os.environ, "AN_HOME": str(root)}
    (root / "config.json").write_text(json.dumps({"ollama_port": server.server_port}))
    request = root / "request.json"
    request.write_text(json.dumps({"messages": [{"role": "user", "content": "fixture"}]}))

    def cli(*args, body=None):
        out = subprocess.check_output([AN, *args], env=env, input=json.dumps(body or {}), text=True)
        return json.loads(out)

    db = sqlite3.connect(root / "registry.sqlite3")

    def task(budget):
        task_id = cli("task", "new", "--input", str(request),
                      "--model", "local", "--model-calls", str(budget))["task"]
        env["AN_TURN"] = cli("permission", "begin", "--task", task_id)["turn"]
        return task_id

    def graph(action, **body):
        return cli("graph", "--task", tid, action, body=body)

    def decide(value):
        for req in graph("requests"):
            cli("permission", "decide", "--task", tid, "--request-id", req["id"], "--decision", value)

    def submit(*ids):
        nodes = [{"id": k, "kind": "model", "model": "local", "inputs":{"request":{"input":"request.json"}},
                  "outputs":["result.json"]} for k in ids]
        graph("submit", base_revision=0, nodes=nodes, idempotency_key="submit")

    def wait(node, status):
        for _ in range(150):
            row = graph("state")["activity"][node]
            if row["status"] == status:
                return row
            time.sleep(.05)
        raise AssertionError(row)

    tid = task(1)
    submit("deny")
    decide("deny")
    wait("deny","failed")
    assert not graph("state")["model_calls"] and server.count == 0

    tid = task(1)
    cli("graph","--task",tid,"control",body={"action":"pause"})
    submit("a","b")
    assert not graph("requests") and server.count == 0
    cli("graph","--task",tid,"control",body={"action":"resume"})
    # Two ready nodes compete for one reservation.
    first = graph("requests")[0]
    cli("permission","decide","--task",tid,"--request-id",first["id"],"--decision","once")
    statuses = [graph("state")["activity"][k]["status"] for k in ("a","b")]
    winner = "b" if statuses[0]=="failed" else "a"
    assert "1/1" in wait("a" if winner=="b" else "b","failed")["error"]
    row = wait(winner,"done")
    assert row["evidence"]["usage"] == {"input_tokens":3,"output_tokens":1}
    for _ in range(3):
        assert graph("state")["activity"][winner]["outputs"] == row["outputs"]
    assert server.count == 1

    tid = task(2)
    server.drop_next = True
    submit("job")
    decide("once")
    wait("job","unknown")
    assert server.count == 2
    graph("submit",base_revision=1,idempotency_key="retry",nodes=[],retry=["job"])
    decide("once")
    wait("job","done")
    assert server.count == 3 and len(graph("state")["model_calls"]) == 2

    for intervention in ("stop","expire"):
        tid = task(1)
        server.release.clear()
        server.entered.clear()
        submit("job")
        decide("once")
        assert server.entered.wait(5)
        if intervention == "stop":
            cli("graph","--task",tid,"control",body={"action":"stop"})
        else:
            env["AN_TURN"] = cli("permission","begin","--task",tid)["turn"]
        server.release.set()
        row = wait("job","cancelled" if intervention=="stop" else "failed")
        assert "outputs" not in row
    # Reopen durable crash snapshots in new CLI processes. A reserved call
    # may send, an abandoned send must not, and a committed output only wakes.
    for phase, expected in [("reserved","done"),("sending","unknown"),("completed","done"),
                            ("stopped_send","cancelled"),("expired_send","failed")]:
        tid = task(1)
        submit("job")
        state = graph("state")
        attempt = state["activity"]["job"]["id"]
        template = json.loads(db.execute("select body from objects where kind='model_call' and json_extract(body,'$.state')='completed' limit 1").fetchone()[0])
        template.update(task=tid, node="job", attempt=attempt, state=phase,
                        authority=state["activity"]["job"]["authority"])
        if phase != "completed":
            for field in ["output","usage","actual_model","settled_at"]:
                template.pop(field,None)
        if phase in ("stopped_send", "expired_send"):
            template["state"] = "sending"
            saved = json.loads(db.execute("select body from objects where kind='graph' and id=?",(tid,)).fetchone()[0])
            saved["activity"]["job"]["status"] = "running"
            db.execute("update objects set body=? where kind='graph' and id=?", (json.dumps(saved),tid))
        db.execute("insert into objects values ('model_call',?,?)",(attempt,json.dumps(template)))
        db.commit()
        if phase == "stopped_send":
            cli("graph","--task",tid,"control",body={"action":"stop"})
        elif phase == "expired_send":
            env["AN_TURN"] = cli("permission","begin","--task",tid)["turn"]
        count = server.count
        wait("job",expected)
        if phase.endswith("_send"):
            assert graph("state")["model_calls"][0]["state"] == "unknown"
        for _ in range(3):
            graph("state")
        assert server.count == count + (phase == "reserved")
    metadata = server.metadata
    for denied in [{**metadata,"remote_host":"https://example.invalid"}, {**metadata,"remote_model":"remote"}, {}]:
        server.metadata = denied
        tid = task(1)
        submit("remote")
        count = server.count
        decide("once")
        wait("remote","failed")
        assert graph("state")["model_calls"][0]["state"] == "withdrawn" and server.count == count
    server.metadata = metadata
    tid = task(1)
    submit("lease")
    leases = root / "model-leases"
    leases.rename(root / "old-leases")
    leases.write_text("blocked")
    decide("once")
    wait("lease","failed")
    assert graph("state")["model_calls"][0]["state"]=="withdrawn"
    leases.unlink()
    (root / "old-leases").rename(leases)
    graph("submit",base_revision=1,idempotency_key="fixed",nodes=[],retry=["lease"])
    decide("once")
    wait("lease","done")
    tid = task(1)
    submit("digest")
    server.digest_probe = True
    decide("once")
    assert server.lookup_started.wait(5)
    assert graph("state")["activity"]["digest"]["status"] == "done"
    server.finish_lookup.set()
    db.close()


class Backend(BaseHTTPRequestHandler):
    def log_message(self,*args):
        pass

    def reply(self, data):
        self.send_response(200)
        self.end_headers()
        self.wfile.write(json.dumps(data).encode())

    def do_GET(self):
        if self.server.digest_probe:
            self.server.lookup_started.set()
            self.server.finish_lookup.wait(3)
        self.reply({"models":[]})

    def do_POST(self):
        data = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        if self.path == "/api/show":
            assert set(data) == {"model"} and data["model"].endswith(":local")
            return self.reply(self.server.metadata)
        assert self.path == "/api/chat" and data["model"].endswith(":local")
        assert data["messages"][0]["content"] == "fixture"
        self.server.count += 1
        self.server.entered.set()
        self.server.release.wait(10)
        if self.server.drop_next:
            self.server.drop_next = False
            return
        self.reply({"model":"fixture","done":True,"message":{"content":"answer"},
                    "prompt_eval_count":3,"eval_count":1})


if __name__ == "__main__":
    with tempfile.TemporaryDirectory() as directory, ThreadingHTTPServer(("127.0.0.1",0), Backend) as server:
        server.metadata = {"details":{"format":"gguf"},"model_info":{"general.architecture":"fixture"}}
        server.digest_probe = False
        server.lookup_started = threading.Event()
        server.finish_lookup = threading.Event()
        server.count = 0
        server.drop_next = False
        server.entered = threading.Event()
        server.release = threading.Event()
        server.release.set()
        threading.Thread(target=server.serve_forever,daemon=True).start()
        try:
            exercise(Path(directory), server)
        finally:
            server.shutdown()
        print("Model permissions, budget, replay, retry, pause, stop and expiry passed")
