"""Review a pair of Markdown edits in the workspace Studio."""

import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

from intent import routes

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--an", type=Path, default=ROOT / "target/debug/an")
    parser.add_argument("--image", default="an-markdown-review:headless")
    parser.add_argument(
        "--review-case", choices=["compatible", "conflict"], default="conflict"
    )
    parser.add_argument("--intent", action="store_true")
    parser.add_argument("--port", type=int, default=0)
    options = parser.parse_args()
    binary = str(options.an.resolve())
    env = dict(os.environ)

    def command(*args, body=None):
        return subprocess.check_output(
            [binary, *args], input=json.dumps(body or {}), text=True, env=env
        ).strip()

    def call(*args, body=None):
        return json.loads(command(*args, body=body))

    spool = command("register", options.image)
    workspace = call("workspace", "create", "--spool", spool)["workspace"]
    selector = ["--harness", "markdown-review", "--key", workspace]
    call("workspace", "bind", workspace, *selector)
    fixtures = HERE / "fixtures"
    task = call(
        "task",
        "new",
        *selector,
        "--model",
        "local",
        "--input",
        fixtures / "original.md",
        "--input",
        fixtures / options.review_case / "reviews.json",
    )["task"]
    graph = ["graph", "--task", task]
    env["AN_TURN"] = call("permission", "begin", "--task", task)["turn"]
    studio = None
    finished = False
    try:
        if options.intent:
            with tempfile.NamedTemporaryFile(mode="w", suffix=".json") as spec:
                json.dump(routes(spool, workspace), spec)
                spec.flush()
                call(
                    "permission", "intent-propose", "--task", task, "--file", spec.name
                )
        call(
            *graph,
            "submit",
            body={
                "base_revision": 0,
                "idempotency_key": "start",
                "nodes": [
                    {
                        "id": "start",
                        "kind": "expand",
                        "spool": spool,
                        "outputs": ["subgraph.json"],
                        "argv": ["python3", "/spool/code/driver.py", "markdown", spool],
                    }
                ],
            },
        )
        studio = subprocess.Popen(
            [
                sys.executable,
                "-m",
                "orchestrator",
                "--task",
                task,
                "--an",
                binary,
                "--port",
                str(options.port),
            ],
            cwd=ROOT / "studio",
            env=env,
        )
        print(f"Workspace {workspace} · task {task}", flush=True)
        try:
            studio.wait()
        except KeyboardInterrupt:
            pass
        if any(s.get("delivery") for s in call(*graph, "view")["activity"].values()):
            call(*graph, "finish")
            finished = True
    finally:
        try:
            if not finished:
                call("permission", "end", "--task", task)
            for instance in call("state", "--task", task)["instances"]:
                if instance["status"] != "unloaded":
                    call("unload", "--task", task, "--instance", instance["id"])
        finally:
            if studio is not None:
                studio.terminate()
                studio.wait()


if __name__ == "__main__":
    main()
