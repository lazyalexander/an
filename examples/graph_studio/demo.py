"""Run either headless experiment with a human at the macro UI."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import time

from experiments import bridge, bridge_result, cad

ROOT = Path(__file__).resolve().parents[2]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("experiment", choices=["bridge", "cad"])
    parser.add_argument("--an", type=Path, default=ROOT / "target/debug/an")
    parser.add_argument("--image", default="an-graph-studio:headless")
    parser.add_argument("--port", type=int, default=0)
    options = parser.parse_args()
    binary = str(options.an.resolve())
    env = dict(os.environ)

    def command(*args, data=None):
        return subprocess.check_output([binary, *args], text=True, input=json.dumps(data or {}), env=env).strip()

    def call(*args, data=None):
        return json.loads(command(*args, data=data))

    spool = command("register", options.image)
    workspace = call("workspace", "create", "--spool", spool)["workspace"]
    selector = ["--harness", "graph-demo", "--key", workspace]
    call("workspace", "bind", workspace, *selector)
    task = call("task", "new", *selector)["task"]
    env["AN_TURN"] = call("permission", "begin", "--task", task)["turn"]

    def graph(action, **args):
        return call("graph", "--task", task, action, data=args)

    studio = subprocess.Popen([sys.executable, "-m", "orchestrator", "--task", task, "--an", binary,
                               "--port", str(options.port)], cwd=ROOT / "studio", env=env)
    closed = False
    try:
        is_bridge = options.experiment == "bridge"
        graph("submit", base_revision=0, idempotency_key="start",
              nodes=bridge(spool) if is_bridge else cad(spool))
        print(f"Workspace {workspace} · task {task}", flush=True)
        while studio.poll() is None:
            if not closed:
                state = graph("view")
                review = state["activity"].get("compare", {})
                if is_bridge and review.get("status") == "saving" and state["revision"] == 1:
                    graph("submit", base_revision=1, idempotency_key="apply-selection",
                          nodes=bridge_result(spool, review["response"]))
                if any(node.get("delivery") for node in state["activity"].values()):
                    for instance in call("state", "--task", task)["instances"]:
                        call("unload", "--task", task, "--instance", instance["id"])
                    graph("finish")
                    closed = True
                    print("Task closed. Studio is serving the published version.", flush=True)
            time.sleep(.5)
    except KeyboardInterrupt:
        pass
    finally:
        studio.terminate()
        studio.wait()
        if not closed:
            call("permission", "end", "--task", task)
            for instance in call("state", "--task", task)["instances"]:
                if instance["status"] != "unloaded":
                    call("unload", "--task", task, "--instance", instance["id"])


if __name__ == "__main__":
    main()
