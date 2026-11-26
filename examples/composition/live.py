"""Run a scored plan on the real An host in a fresh registry and inspect its artifacts."""

import base64
import json
import os
import runpy
import subprocess
import sys
import time

from catalog import HERE, SPOOLS, build

sys.path.insert(0, str(HERE.parent / "markdown_review"))
from codex_session import Session


def expected_outputs(case):
    sources = {name: (HERE / "inputs" / name).read_bytes() for name in case["inputs"]}
    remaining = dict(case["parents"])
    while remaining:
        ready = [name for name, parent in remaining.items() if parent in sources]
        if not ready:
            raise ValueError("Invalid oracle dependency graph")
        for name in ready:
            transform = runpy.run_path(str(HERE / "catalog" / f"{name}.py"))[
                "transform"
            ]
            sources[name] = transform(sources[remaining.pop(name)])
    return {SPOOLS[name]["output"]: sources[name] for name in case["deliver"]}


class Runtime:
    def __init__(self, directory, binary):
        self.binary = binary
        self.directory = directory
        self.env = {**os.environ, "AN_HOME": str(directory / "registry")}
        self.env.pop("AN_TURN", None)
        self.env.pop("AN_AGENT_TOKEN", None)
        self.task = None
        self.images = build(directory / "images", binary, self.cli)
        grants = [arg for image in self.images.values() for arg in ("--spool", image)]
        self.workspace = self.cli("workspace", "create", *grants)["workspace"]

    def cli(self, *args, body=None):
        data = subprocess.check_output(
            [self.binary, *args],
            env=self.env,
            text=True,
            input=json.dumps(body or {}),
            timeout=240,
        ).strip()
        return json.loads(data) if data.startswith(("{", "[")) else data

    def graph(self, operation, **args):
        return self.cli("graph", "--task", self.task, operation, body=args)

    def begin(self, case):
        selector = ["--harness", "composition-eval", "--session", case["name"]]
        self.cli("workspace", "bind", self.workspace, *selector)
        inputs = [
            arg
            for name in case["inputs"]
            for arg in ("--input", HERE / "inputs" / name)
        ]
        self.task = self.cli("task", "new", *selector, *inputs)["task"]
        self.env["AN_TURN"] = self.cli("permission", "begin", "--task", self.task)[
            "turn"
        ]
        self.graph("control", action="pause")
        token = self.cli("permission", "delegate", "--task", self.task)["agent_token"]
        return token

    def plan(self, case, directory, model):
        token = self.begin(case)
        brief = (
            "Discover available task inputs and spool interfaces using an_catalog. Read an_skill "
            "for selected spools. Submit your own graph through an_graph_submit, linking actual "
            "output filenames to input slots. Use run and delivery nodes. The host holds the graph "
            "paused for review. Do not execute it or change permissions. If the task is unsupported, "
            "submit no nodes and send the exact text UNSUPPORTED using an_graph_say.\n"
            + case["sentence"]
        )
        agent_env = {k: v for k, v in self.env.items() if k != "AN_TURN"}
        agent_env["AN_AGENT_TOKEN"] = token
        session = Session(
            directory,
            self.binary,
            self.task,
            agent_env,
            model_name=model,
            tools={
                "an_catalog",
                "an_skill",
                "an_graph_state",
                "an_graph_submit",
                "an_graph_say",
            },
        )
        session.turn(brief, "planner")
        graph = self.graph("state")
        if self.cli("state", "--task", self.task)["instances"]:
            raise ValueError("Planning allocated task containers")
        nodes = list(graph["nodes"].values())
        declined = any(
            e.get("actor") == "agent"
            and e.get("kind") == "message"
            and e["data"].get("text") == "UNSUPPORTED"
            for e in graph["events"]
        )
        return {
            "decision": "unsupported" if declined and not nodes else "plan",
            "nodes": nodes,
        }

    def execute(self, plan, case):
        # Reuse the host's intent policy. Authorize only the selected fixture edges.
        routes = []
        nodes = {n["id"]: n for n in plan["nodes"]}
        for n in nodes.values():
            for ref in n["inputs"].values():
                producer = nodes.get(ref.get("node"))
                op = "input" if producer is None else "handoff"
                dest = n.get("spool")
                if n["kind"] == "delivery":
                    op, dest = "publish", self.workspace
                route = {
                    "source": ref,
                    "origin": producer["spool"]
                    if producer
                    else "task-input:" + ref["input"],
                    "operation": op,
                    "destination": dest,
                }
                if producer:
                    route["producer_argv"] = producer["argv"]
                if route not in routes:
                    routes.append(route)
        scope_file = self.directory / case["name"] / "reviewed-scope.json"
        scope_file.write_text(json.dumps({"turn_scope": routes}, indent=2))
        card = self.cli(
            "permission", "intent-propose", "--task", self.task, "--file", scope_file
        )
        self.cli(
            "permission",
            "intent-decide",
            "--task",
            self.task,
            "--request-id",
            card["id"],
            "--decision",
            "allow",
        )
        self.graph("control", action="resume")
        deadline = time.monotonic() + 180
        while time.monotonic() < deadline:
            graph = self.graph("state")
            states = {k: v["status"] for k, v in graph["activity"].items()}
            if all(s == "done" for s in states.values()):
                break
            if any(
                s in ("failed", "cancelled", "unknown", "permission_required")
                for s in states.values()
            ):
                raise ValueError(f"Unsettled graph: {states}")
            time.sleep(0.25)
        else:
            raise TimeoutError("Fixture execution timed out")
        actual = {}
        for node in nodes.values():
            activity = graph["activity"][node["id"]]
            for slot, ref in node["inputs"].items():
                if "node" in ref:
                    upstream = graph["activity"][ref["node"]]["outputs"][ref["port"]]
                    if activity["bindings"][slot]["revision"] != upstream["revision"]:
                        raise ValueError("Consumer did not bind the produced revision")
            if node["kind"] == "delivery":
                for files in self.graph("open", delivery=activity["delivery"])[
                    "files"
                ].values():
                    for name, data in files.items():
                        actual[name] = base64.b64decode(
                            data if isinstance(data, str) else data["data"]
                        )
        if actual != expected_outputs(case):
            raise ValueError("Delivered bytes differ from the fixture oracle")
        return {"verified_files": sorted(actual), "task": self.task}

    def close(self):
        if not self.task:
            return True
        try:
            view = self.graph("view")
            if any(
                s["status"] not in ("done", "failed", "superseded", "cancelled")
                for s in view["activity"].values()
            ):
                self.graph("control", action="stop")
            closed = view["closed"]
            for _ in range(20):
                if not closed:
                    self.graph("state", after=0)
                try:
                    result = self.graph("finish")
                except subprocess.CalledProcessError:
                    time.sleep(0.25)
                    continue
                closed = result["closed"]
                if closed and not result["cleanup_pending"]:
                    instances = self.cli("state", "--task", self.task)["instances"]
                    return all(i["status"] == "unloaded" for i in instances)
                time.sleep(0.25)
            return False
        finally:
            self.task = None
            self.env.pop("AN_TURN", None)
