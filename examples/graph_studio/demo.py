"""Run either headless experiment with a human at the macro UI."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import time



ROOT = Path(__file__).resolve().parents[2]


def initial_driver(experiment, spool):
    return {"id": "start", "kind": "expand", "spool": spool,
            "argv": ["python3", "/spool/code/driver.py", experiment, spool],
            "outputs": ["subgraph.json"]}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("experiment", choices=["bridge", "cad"])
    parser.add_argument("--an", type=Path, default=ROOT / "target/debug/an")
    parser.add_argument("--image", default="an-graph-studio:headless")
    parser.add_argument("--port", type=int, default=0)
    parser.add_argument("--resume", metavar="V1_DELIVERY")
    options = parser.parse_args()
    if options.resume and options.experiment != "bridge":
        parser.error("--resume requires bridge")
    binary = str(options.an.resolve())
    env = dict(os.environ)

    def command(*args, data=None):
        return subprocess.check_output([binary, *args], text=True, input=json.dumps(data or {}), env=env).strip()

    def call(*args, data=None):
        return json.loads(command(*args, data=data))

    spool = command("register", options.image)
    if options.resume:
        task = call("task", "resume", options.resume)["task"]
        state = call("state", "--task", task)
        workspace = state["workspace"]
        if spool not in state["work"]:
            parser.error("Select the same versioned spool image used for V1")
        package = call("graph", "--task", task, "open", data={"delivery": options.resume})
        required = (("selected", "selected.svg"), ("choice", "response.json"),
                    ("painting", "painting.ora"))
        missing = [f"{binding}/{name}" for binding, name in required
                   if name not in package["files"].get(binding, {})]
        if missing:
            parser.error("Incompatible Bridge V1 delivery; missing " + ", ".join(missing))
    else:
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
        graph("submit", base_revision=0, idempotency_key="start",
              nodes=[initial_driver("revise" if options.resume else options.experiment, spool)])
        print(f"Workspace {workspace} · task {task}", flush=True)
        while studio.poll() is None:
            if not closed:
                state = graph("view")
                for node_id, activity in state["activity"].items():
                    if activity["status"] == "failed":
                        raise RuntimeError(f"{node_id} failed: {activity.get('error', 'unknown error')}")
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
