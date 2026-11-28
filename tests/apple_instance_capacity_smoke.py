"""Use the Markdown review spool to exercise a shared one-instance limit."""
from concurrent.futures import ThreadPoolExecutor
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time


def exercise(home, reference):
    env = {**os.environ, "AN_HOME": str(home)}
    env.pop("AN_TURN", None)
    env.pop("AN_AGENT_TOKEN", None)
    (home / "config.json").write_text('{"max_loaded_instances":1}')

    def command(*args, body=None, check=True):
        result = subprocess.run(["target/debug/an", *args], env=env, text=True,
                              input=json.dumps(body or {}), capture_output=True,
                              check=False, timeout=90)
        if check and result.returncode:
            raise AssertionError(result.stderr)
        return result

    def cli(*args, body=None):
        return json.loads(command(*args, body=body).stdout)

    spool = command("register", reference).stdout.strip()
    seed = home / "seed.md"
    seed.write_text("capacity probe")
    tasks = [cli("task", "new", "--spool", spool, "--input", str(seed))["task"] for _ in range(2)]

    def load(task):
        return command("load", "--task", task, "--spool", spool, check=False)

    try:
        with ThreadPoolExecutor(2) as pool:
            results = list(pool.map(load, tasks))
        assert sum(r.returncode == 0 for r in results) == 1, results
        first = next(i for i, r in enumerate(results) if r.returncode == 0)
        holder, waiter = tasks[first], tasks[1-first]
        instance = json.loads(results[first].stdout)["id"]
        assert "Loaded instance capacity reached" in results[1-first].stderr
        assert json.loads(load(holder).stdout)["id"] == instance
        assert not cli("state", "--task", waiter)["instances"]

        env["AN_TURN"] = cli("permission", "begin", "--task", waiter)["turn"]

        def graph(action, **body):
            return cli("graph", "--task", waiter, action, body=body)

        graph("submit", base_revision=0, idempotency_key="capacity", nodes=[
            {"id":"probe", "kind":"run", "spool":spool, "argv":["/bin/true"],
             "inputs":{"source":{"input":"seed.md"}}, "outputs":[]}])
        queued = graph("state")["activity"]["probe"]
        assert queued["status"] == "queued", queued
        assert queued["waiting_reason"] == {"reason":"instance_capacity", "limit":1}
        assert not cli("state", "--task", waiter)["instances"]
        assert cli("unload", "--task", holder, "--instance", instance)["status"] == "unloaded"
        deadline = time.monotonic() + 60
        while time.monotonic() < deadline:
            state = graph("state")["activity"]["probe"]
            assert state["id"] == queued["id"] and state["status"] != "failed", state
            if state["status"] == "permission_required":
                cli("permission", "decide", "--task", waiter, "--request-id",
                    state["requests"]["source"], "--decision", "once")
            if state["status"] == "done":
                assert "waiting_reason" not in state
                break
            time.sleep(.2)
        else:
            raise AssertionError(state)
    finally:
        env.pop("AN_TURN", None)
        for task in tasks:
            for instance in cli("state", "--task", task)["instances"]:
                if instance["status"] != "unloaded":
                    assert cli("unload", "--task", task, "--instance", instance["id"])["status"] == "unloaded"


if __name__ == "__main__":
    with tempfile.TemporaryDirectory(dir="/private/tmp") as directory:
        exercise(Path(directory), sys.argv[1])
    print("Concurrent load, reuse, graph wait and recovery passed")
