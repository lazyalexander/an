"""Route controller requests and supervise the harness for a task or workspace."""

from contextlib import contextmanager
import json
import os
from pathlib import Path
import tomllib

from .commands import Commands
from .runner import Runner, signal_wake
from .task_registry import TaskRegistry
from .task_view import task_view

ACTIONS = set(
    tomllib.loads(Path(__file__).with_name("controller.toml").read_text())["studio"][
        "graph_actions"
    ]
)


@contextmanager
def controller(directory, options):
    workspace = options["workspace"]
    harness = options["harness"]
    an = Commands(options["binary"])
    registry = TaskRegistry(
        Path(os.environ["AN_HOME"]) / "controllers" / directory.name, an
    )
    registry.restore(options["task"])

    def retire(key):
        if not registry.retire(key):
            return
        with runner.guard:
            proc = runner.procs.get(key)
            if proc and proc.poll() is None:
                signal_wake(proc)
            runner.status_by_task.pop(key, None)

    runner = Runner(
        an.binary,
        options["agent"],
        registry.turns,
        registry.mcp,
        retire=retire,
        refresh=lambda key: registry.delegate(key, registry.turns[key]),
    )

    def dispatch(action, current, body):
        if action == "start":
            if workspace is None:
                raise ValueError("Studio was not started for a workspace")
            text = body.get("text")
            if not isinstance(text, str) or not text.strip():
                raise ValueError("Empty instruction")
            return registry.start(workspace, harness, text)
        if action in ("view", "deliveries", "open") and registry.is_retired(current):

            def read(name, args):
                return an(["graph", "--task", current, name], args, "")

            if action == "view":
                return task_view(read, {"state": "off"})
            return read(action, body)
        if current not in registry.turns:
            raise ValueError("Unknown task")
        turn = registry.turns[current]

        def graph(name, args):
            return an(["graph", "--task", current, name], args, turn)

        if action == "view":
            return task_view(graph, runner.status(current))
        if action == "intent-decide":
            return an(
                [
                    "permission",
                    "intent-decide",
                    "--task",
                    current,
                    "--request-id",
                    body["id"],
                    "--decision",
                    json.dumps(body["decisions"]),
                ],
                {},
                turn,
            )
        if action == "decide":
            if body["decision"] not in ("deny", "once") or body["request"] not in {
                item["id"] for item in graph("requests", {})
            }:
                raise ValueError("Unknown permission decision")
            return an(
                [
                    "permission",
                    "decide",
                    "--task",
                    current,
                    "--request-id",
                    body["request"],
                    "--decision",
                    body["decision"],
                ],
                {},
                turn,
            )
        if action in ACTIONS:
            return graph(action, body)
        if action == "evidence":
            return graph("evidence", body)
        raise ValueError("Unknown action")

    runner.start()
    try:
        yield dispatch, registry.mcp
    finally:
        runner.close()
