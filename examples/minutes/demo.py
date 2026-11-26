"""Run the headless minutes experiment; display decisions in task Studio."""

import argparse
import base64
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[2]


def run(directory, mode, attack=False):
    env = {**os.environ, "AN_HOME": str(directory)}

    def cli(*args, body=None):
        raw = subprocess.check_output(
            [ROOT / "target/debug/an", *args],
            env=env,
            text=True,
            input=json.dumps(body or {}),
        ).strip()
        return json.loads(raw) if raw.startswith(("[", "{")) else raw

    organize, format_spool = [
        cli("register", f"an-minutes-{name}:test") for name in ("organize", "format")
    ]
    workspace = cli(
        "workspace", "create", "--spool", organize, "--spool", format_spool
    )["workspace"]
    selector = ["--harness", "minutes", "--key", workspace]
    cli("workspace", "bind", workspace, *selector)
    flows = [
        ["input", "minutes.json", organize],
        ["handoff", organize, format_spool],
        ["publish", format_spool, workspace],
    ]
    policy = directory / "flows.json"
    policy.write_text(json.dumps(flows))
    cli("workspace", "policy", "set", workspace, "--file", policy)
    task = cli(
        "task", "new", *selector, "--input", Path(__file__).with_name("minutes.json")
    )["task"]
    env["AN_TURN"] = cli("permission", "begin", "--task", task)["turn"]

    def graph(action, **body):
        return cli("graph", "--task", task, action, body=body)

    def process(node, spool, port, source, output):
        return dict(
            id=node,
            kind="run",
            spool=spool,
            argv=["python3", "/spool/code/process.py"],
            inputs={port: source},
            outputs=[output],
        )

    nodes = [
        process(
            "organize", organize, "minutes", {"input": "minutes.json"}, "summary.json"
        ),
        process(
            "format",
            format_spool,
            "summary",
            {"node": "organize", "port": "summary.json"},
            "announcement.md",
        ),
        dict(
            id="draft",
            kind="delivery",
            inputs={"draft": {"node": "format", "port": "announcement.md"}},
            outputs=[],
        ),
    ]
    routes = [
        dict(
            operation=op,
            origin=("task-input:" + origin if op == "input" else origin),
            source=next(iter(node["inputs"].values())),
            destination=dest,
        )
        for (op, origin, dest), node in zip(flows, nodes)
    ]
    routes[1]["producer_argv"] = nodes[0]["argv"].copy()
    routes[2]["producer_argv"] = nodes[1]["argv"].copy()
    if attack:
        nodes[1]["inputs"]["summary"] = {"input": "minutes.json"}
    studio = None
    requests = {}
    try:
        if mode != "baseline":
            path = directory / "intent.json"
            path.write_text(json.dumps(routes))
            card = cli("permission", "intent-propose", "--task", task, "--file", path)
            if mode == "intent":
                cli(
                    "permission",
                    "intent-decide",
                    "--task",
                    task,
                    "--request-id",
                    card["id"],
                    "--decision",
                    "allow",
                )
            else:
                studio = subprocess.Popen(
                    [
                        sys.executable,
                        "-m",
                        "orchestrator",
                        "--task",
                        task,
                        "--an",
                        ROOT / "target/debug/an",
                        "--port",
                        "0",
                    ],
                    cwd=ROOT / "studio",
                    env=env,
                )
        graph("submit", base_revision=0, idempotency_key="minutes", nodes=nodes)
        for _ in range(900):
            for req in graph("requests"):
                if mode == "baseline":
                    result = cli(
                        "permission",
                        "decide",
                        "--task",
                        task,
                        "--request-id",
                        req["id"],
                        "--decision",
                        "once",
                    )
                    requests[req["id"]] = {**req, "decision":result["decision"]}
                elif mode == "intent":
                    raise AssertionError(req)
            states = graph("state")["activity"]
            if attack and states["draft"]["status"] == "done":
                raise AssertionError("The forbidden formatter input produced a delivery")
            if attack and states["format"]["status"] == "failed":
                if states["format"]["error"] != "Denied by workspace flow policy" or states["format"].get("run"):
                    raise AssertionError(states["format"])
                print("Formatter denied by workspace flow policy before run submission", flush=True)
                break
            assert all(v["status"] != "failed" for v in states.values()), states
            if states["draft"]["status"] == "done":
                file = graph("open", delivery=states["draft"]["delivery"])["files"][
                    "draft"
                ]["announcement.md"]
                actual = base64.b64decode(
                    file if isinstance(file, str) else file["data"]
                ).decode()
                public = json.loads(
                    Path(__file__).with_name("minutes.json").read_text()
                )["public_items"]
                assert actual == "# Announcement\n\n" + "".join(
                    "- " + item + "\n" for item in public
                )
                break
            time.sleep(0.3)
        else:
            raise TimeoutError(states)
        (directory / "requests.json").write_text(json.dumps(requests, indent=2))
        print(
            mode,
            "individual approvals",
            len(requests),
            "business decisions",
            0,
            flush=True,
        )
        if studio:
            studio.wait()
    finally:
        if studio and studio.poll() is None:
            studio.terminate()
            studio.wait()
        cli("permission", "end", "--task", task)
        for instance in cli("state", "--task", task)["instances"]:
            if instance["status"] != "unloaded":
                cli("unload", "--task", task, "--instance", instance["id"])


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--mode", choices=["baseline", "intent", "studio"], default="studio"
    )
    parser.add_argument("--attack", action="store_true")
    args = parser.parse_args()
    directory = Path(tempfile.mkdtemp(prefix="minutes-"))
    print(directory, flush=True)
    run(directory, args.mode, args.attack)
