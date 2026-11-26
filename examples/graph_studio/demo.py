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
    parser.add_argument("experiment", choices=["bridge", "bridge-model", "bridge-native", "cad", "markdown"])
    parser.add_argument("--an", type=Path, default=ROOT / "target/debug/an")
    parser.add_argument("--image")
    parser.add_argument("--review-case", choices=["compatible","conflict"], default="conflict")
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

    markdown = options.experiment == "markdown"
    spool = command("register", options.image or ("an-markdown-review:headless" if markdown else "an-graph-studio:headless"))
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
        grants = ["--model", "local"] if markdown or options.experiment == "bridge-model" else []
        if markdown:
            fixtures = ROOT / "examples/markdown_review/fixtures"
            grants.extend(["--input", str(fixtures/"original.md"), "--input", str(fixtures/options.review_case/"reviews.json")])
        task = call("task", "new", *selector, *grants)["task"]
    env["AN_TURN"] = call("permission", "begin", "--task", task)["turn"]

    def graph(action, **args):
        return call("graph", "--task", task, action, data=args)

    studio = subprocess.Popen([sys.executable, "-m", "orchestrator", "--task", task, "--an", binary,
                               "--port", str(options.port)], cwd=ROOT / "studio", env=env)
    channel = None
    closed = False
    try:
        if options.experiment == "bridge-native":
            channel = subprocess.Popen([binary,"native","serve","--task",task],env=env,stdout=subprocess.PIPE,text=True)
            connection = channel.stdout.readline().strip()
            if not connection:
                raise RuntimeError("Krita channel failed to start")
            print(f"Krita connection file: {connection}",flush=True)
        graph("submit", base_revision=0, idempotency_key="start",
              nodes=[initial_driver("revise" if options.resume else options.experiment, spool)])
        print(f"Workspace {workspace} · task {task}", flush=True)
        # Keep the review turn open while its author reads the decision history.
        # A failed review also remains available for inspection and graph retry.
        while studio.poll() is None:
            if not closed and not markdown:
                state = graph("view")
                for node_id, activity in state["activity"].items():
                    if activity["status"] in ("failed", "unknown"):
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
        try:
            if not closed:
                try:
                    if markdown and any(s.get("delivery") for s in graph("view")["activity"].values()):
                        graph("finish")
                        closed = True
                finally:
                    if not closed:
                        call("permission", "end", "--task", task)
                for instance in call("state", "--task", task)["instances"]:
                    if instance["status"] != "unloaded":
                        call("unload", "--task", task, "--instance", instance["id"])
        finally:
            if channel:
                channel.terminate()
                channel.wait()
            studio.terminate()
            studio.wait()


if __name__ == "__main__":
    main()
