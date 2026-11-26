"""Local controller for a Studio workspace or an explicitly attached task."""
from contextlib import contextmanager
import json
import os
import subprocess
import threading
import tomllib
import uuid
from pathlib import Path

from .runner import Runner

ACTIONS = set(tomllib.loads(Path(__file__).with_name("controller.toml").read_text())["studio"]["graph_actions"])


def task_view(graph, agent_status):
    result = graph("view", {})
    if result["authorized"] and result["revision"]:
        graph("state", {"after": 2**53})
        result = graph("view", {})
    # Keep execution evidence behind the explicit sources/evidence actions.
    for collection, fields in (("nodes", ("id", "kind", "title")),
                        ("activity", ("id", "status"))):
        result[collection] = {node: {field: row[field] for field in fields if field in row}
                       for node, row in result[collection].items()}
    result["permissions"] = graph("requests", {}) if result["authorized"] else []
    result["deliveries"] = graph("deliveries", {})
    result["agent"] = agent_status
    return result


@contextmanager
def controller(directory, options):
    binary = options["binary"]
    task = options["task"]
    workspace = options["workspace"]
    harness = options["harness"]
    storage = Path(os.environ["AN_HOME"]) / "controllers" / directory.name
    storage.mkdir(parents=True, mode=0o700, exist_ok=True)
    state = storage / "tasks.json"
    mcp = json.loads(state.read_text()) if state.exists() else {}
    guard = threading.Lock()
    turns = {key: servers["an"]["env"]["AN_TURN"] for key, servers in mcp.items()}
    if task:
        turn = os.environ.get("AN_TURN")
        turns[task] = turn
        mcp[task] = {"an": {"command": binary, "args": ["mcp", "--task", task],
                            "env": {k: os.environ[k] for k in ("AN_HOME", "AN_TURN") if k in os.environ}}}
    def retire(key):
        with guard:
            if key not in turns:
                return
            history = storage / "closed"
            history.mkdir(exist_ok=True, mode=0o700)
            (history / key).touch(mode=0o600)
            mcp.pop(key, None)
            turns.pop(key)
            save()
        with runner.guard:
            proc = runner.procs.get(key)
            if proc and proc.poll() is None:
                proc.terminate()
            runner.status_by_task.pop(key, None)

    runner = Runner(binary, options["agent"], turns, mcp, retire=retire)

    def save():
        pending = state.with_suffix(".tmp")
        pending.write_text(json.dumps(mcp))
        os.replace(pending, state)

    def an(command, body, turn=None):
        env = {**os.environ, "AN_TURN": turn} if turn is not None else None
        proc = subprocess.run([binary, *command], input=json.dumps(body), text=True,
                              capture_output=True, check=False, env=env)
        if proc.returncode:
            raise ValueError(proc.stderr.strip() or "An command failed")
        return json.loads(proc.stdout)

    def start(text):
        """Create a workspace task whose first instruction is the person's sentence."""
        session = uuid.uuid4().hex
        selector = ["--harness", harness, "--session", session]
        bound = an(["workspace", "bind", workspace, *selector], {})
        created = an(["task", "new", *selector], {})["task"]
        turn = an(["permission", "begin", "--task", created], {})["turn"]
        an(["graph", "--task", created, "instruct"], {"text": text, "revision": 0}, turn)
        servers = bound["mcp_servers"]
        for server in servers.values():
            server.setdefault("env", {})["AN_TURN"] = turn
        with guard:
            mcp[created] = servers
            save()
            turns[created] = turn  # expose to the runner after saving its connection
        return {"task": created}

    def dispatch(action, current, body):
        if action == "start":
            if workspace is None:
                raise ValueError("Studio was not started for a workspace")
            text = body.get("text")
            if not isinstance(text, str) or not text.strip():
                raise ValueError("Empty instruction")
            return start(text)
        if (isinstance(current, str) and current.isalnum() and current not in turns
                and action in ("view", "deliveries", "open") and (storage / "closed" / current).is_file()):
            def read(name, args):
                return an(["graph", "--task", current, name], args, "")
            if action == "view":
                return task_view(read, {"state": "off"})
            return read(action, body)
        if current not in turns:
            raise ValueError("Unknown task")
        turn = turns[current]

        def graph(name, args):
            return an(["graph", "--task", current, name], args, turn)

        if action == "view":
            return task_view(graph, runner.status(current))
        elif action == "intent-decide":
            result = an(["permission","intent-decide","--task",current,"--request-id",body["id"],
                         "--decision",json.dumps(body["decisions"])], {}, turn)
        elif action == "decide":
            if body["decision"] not in ("deny", "once") or body["request"] not in {
                    item["id"] for item in graph("requests", {})}:
                raise ValueError("Unknown permission decision")
            result = an(["permission", "decide", "--task", current, "--request-id",
                         body["request"], "--decision", body["decision"]], {}, turn)
        elif action in ACTIONS:
            result = graph(action, body)
        elif action == "evidence":
            result = graph("evidence", body)
        else:
            raise ValueError("Unknown action")
        return result

    runner.start()
    try:
        yield dispatch, mcp
    finally:
        runner.close()
