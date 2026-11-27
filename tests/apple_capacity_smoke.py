"""Check cross-task FIFO admission and cancellation on a real Apple Container host."""
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
    (home / "config.json").write_text(json.dumps({"max_concurrent_runs": 1}))

    def cli(*args):
        return subprocess.check_output(["target/debug/an", *args], env=env, text=True).strip()

    image = cli("register", reference)
    jobs = []

    def op(index, command, *args):
        task, instance, run = jobs[index]
        return json.loads(cli(command, "--task", task, *args))

    def status(index):
        return op(index, "status", jobs[index][2])

    def until(index, predicate):
        deadline = time.monotonic() + 45
        while time.monotonic() < deadline:
            state = status(index)
            if predicate(state):
                return
            time.sleep(.1)
        raise AssertionError(state)

    try:
        for seconds in (120, 120, 0):
            task = json.loads(cli("task", "new", "--spool", image))["task"]
            instance = json.loads(cli("load", "--task", task, "--spool", image))["id"]
            jobs.append([task, instance, None])
            jobs[-1][2] = json.loads(cli("run", "--task", task, "--instance", instance,
                                         "--", "/bin/sleep", str(seconds)))
        for index in (1, 2):
            assert status(index)["waiting"] == {"reason": "execution_capacity", "position": index}
        op(0, "cancel", jobs[0][2])
        until(1, lambda s: s["status"] == "running" and "waiting" not in s)
        assert status(2)["waiting"]["position"] == 1
        op(1, "cancel", jobs[1][2])
        until(2, lambda s: s["status"] == "completed")
    finally:
        for index, (_, instance, _) in enumerate(jobs):
            assert op(index, "unload", "--instance", instance)["status"] == "unloaded"


if __name__ == "__main__":
    with tempfile.TemporaryDirectory(dir="/private/tmp") as directory:
        exercise(Path(directory), sys.argv[1])
    print("Cross-task FIFO, cancellation and cleanup passed")
