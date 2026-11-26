"""A workspace keeps handling instructions after its Studio process exits."""
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import sys
import time
from urllib.request import Request, urlopen

from an_host.client import connect
from orchestrator.config import load

STUDIO = Path(__file__).resolve().parents[1]
BINARY = STUDIO.parent / "target/debug/an"


def setup_workspace(tmp_path):
    with sqlite3.connect(tmp_path / "registry.sqlite3") as db:
        db.execute("CREATE TABLE objects(kind,id,body,PRIMARY KEY(kind,id))")
        db.execute("INSERT INTO objects VALUES('image','demo','\"demo\"')")
    workspace = json.loads(subprocess.check_output(
        [BINARY, "workspace", "create", "--spool", "demo"]))["workspace"]
    config = tmp_path / "runner.toml"
    config.write_text("[agent]\npoll_ms = 1000\ncommand = " +
                      json.dumps([sys.executable, str(STUDIO / "tests/fake_agent.py")]))
    return workspace, config


def test_restart_studio(tmp_path, monkeypatch):
    monkeypatch.setenv("AN_HOME", str(tmp_path))
    workspace, config = setup_workspace(tmp_path)
    args = [sys.executable, "-m", "orchestrator", "--workspace", workspace, "--an", str(BINARY),
            "--port", "0", "--config", str(config)]

    def launch():
        process = subprocess.Popen(args, cwd=STUDIO, stdout=subprocess.PIPE, text=True)
        line = process.stdout.readline()
        assert line.startswith("macro UI: "), line
        url, key = line.strip().split()[-1].split("#")

        def api(action, body, task=""):
            req = Request(url.replace("/ui", "/api/") + action, json.dumps(body).encode(),
                          {"Authorization": "Bearer " + key, "X-An-Task": task})
            with urlopen(req, timeout=10) as reply:
                return json.load(reply)

        return process, api

    request = connect(str(BINARY), None, workspace, "studio", load(config)["agent"])
    proc = None
    try:
        proc, api = launch()
        task = api("start", {"text": "first"})["task"]
        proc.terminate()
        proc.wait(timeout=5)
        request("instruct", task, {"text": "while offline", "revision": 0})
        wait_for_reply(request, task, "while offline")
        proc, api = launch()
        assert api("view", {}, task)["authorized"]
    finally:
        if proc is not None:
            proc.terminate()
            proc.wait(timeout=5)
        request("host-stop", None, {})


def test_host_restores_task_and_keeps_runner_cursor(tmp_path, monkeypatch):
    from an_host.controller import controller

    monkeypatch.setenv("AN_HOME", str(tmp_path))
    workspace, config = setup_workspace(tmp_path)
    options = dict(binary=str(BINARY), workspace=workspace, harness="studio", task=None,
                   agent=load(config)["agent"])
    directory = tmp_path / "scope"
    with controller(directory, options) as (request, _):
        task = request("start", None, {"text": "first"})["task"]
        wait_for_reply(request, task, "first")
    with controller(directory, options) as (request, _):
        assert request("view", task, {})["authorized"]
        request("instruct", task, {"text": "second", "revision": 0})
        wait_for_reply(request, task, "second")
    wakes = (tmp_path / "agents" / task / "wakes").read_text().splitlines()
    assert len(wakes) == 2


def wait_for_reply(request, task, text):
    deadline = time.monotonic() + 15
    while time.monotonic() < deadline:
        view = request("view", task, {})
        if "收到：" + text in json.dumps(view, ensure_ascii=False):
            return
        time.sleep(0.1)
    raise AssertionError(view)


def test_presentation_import_boundary():
    import ast
    import tomllib

    rules = tomllib.loads((STUDIO / "architecture.toml").read_text())["architecture"]
    module = ast.parse((STUDIO / rules["presentation"]).read_text())
    imports = set()
    for node in ast.walk(module):
        if isinstance(node, ast.Import):
            imports.update(alias.name for alias in node.names)
        elif isinstance(node, ast.ImportFrom):
            if node.module:
                imports.add(node.module)
                imports.update(f"{node.module}.{alias.name}" for alias in node.names)
            elif node.level:
                imports.update(f"orchestrator.{alias.name}" for alias in node.names)
    for name in imports:
        assert not any(name == root or name.startswith(root + ".")
                       for root in rules["forbidden_imports"]), name
        if name == "an_host" or name.startswith("an_host."):
            assert any(name == root or name.startswith(root + ".")
                       for root in rules["host_imports"]), name


def test_ended_turn_retires_even_without_agent(tmp_path, monkeypatch):
    from an_host.controller import controller

    monkeypatch.setenv("AN_HOME", str(tmp_path))
    workspace, _ = setup_workspace(tmp_path)
    options = dict(binary=str(BINARY), workspace=workspace, harness="studio", task=None,
                   agent={**load()["agent"], "command": None})
    with controller(tmp_path / "scope", options) as (request, servers):
        task = request("start", None, {"text": "retire"})["task"]
        turn = json.loads((tmp_path / "controllers/scope/tasks.json").read_text())[task]["owner"]
        subprocess.run([BINARY, "permission", "end", "--task", task],
                       env={**os.environ, "AN_TURN": turn}, check=True)
        file = tmp_path / "controllers/scope/tasks.json"
        deadline = time.monotonic() + 5
        while task in json.loads(file.read_text()) and time.monotonic() < deadline:
            time.sleep(.05)
        assert task not in servers and turn not in file.read_text()
        assert not request("view", task, {})["authorized"]


def test_agent_can_propose_and_only_operator_can_decide(tmp_path, monkeypatch):
    from an_host.controller import controller
    from fake_agent import Mcp

    monkeypatch.setenv("AN_HOME", str(tmp_path))
    workspace, _ = setup_workspace(tmp_path)
    options = dict(binary=str(BINARY), workspace=workspace, harness="studio", task=None,
                   agent={**load()["agent"], "command": None})
    with controller(tmp_path / "scope", options) as (request, servers):
        task = request("start", None, {"text": "review the draft"})["task"]
        agent = Mcp(servers[task]["an"])
        try:
            tools = agent.tools()
            assert "an_scope_propose" in tools and not any("decide" in name for name in tools)
            state = agent.tool("an_state", {})
            spool = next(iter(state["spools"]))
            route = {"operation": "publish", "origin": spool, "source": {"producer_port": "subgraph.json"},
                     "destination": "graph-metadata", "producer_argv": ["generate"]}
            card = agent.tool("an_scope_propose", {"routes": [route]})
            assert card["status"] == "pending"
            assert request("view", task, {})["intent"]["id"] == card["id"]
            decision = request("intent-decide", task, {"id": card["id"], "decisions": ["allow"]})
            assert decision["status"] == "decided"
        finally:
            agent.close()


def test_agent_token_and_rotation(tmp_path, monkeypatch):
    from an_host.controller import controller
    from fake_agent import Mcp

    monkeypatch.setenv("AN_HOME", str(tmp_path))
    workspace, _ = setup_workspace(tmp_path)
    options = dict(binary=str(BINARY), workspace=workspace, harness="studio", task=None,
                   agent={**load()["agent"], "command": None, "poll_ms": 30})
    with controller(tmp_path / "scope", options) as (request, servers):
        task = request("start", None, {"text": "first"})["task"]
        server = servers[task]["an"]
        assert "AN_TURN" not in server["env"]
        token = server["env"]["AN_AGENT_TOKEN"]
        for action in ("begin", "end", "delegate", "decide", "intent-decide", "policy"):
            # A caller can rename its own token. The host must verify the role.
            proc = subprocess.run([BINARY, "permission", action, "--task", task],
                                  env={**os.environ, "AN_TURN": token, "AN_AGENT_TOKEN": token},
                                  capture_output=True)
            assert proc.returncode and b"required" in proc.stderr, (action, proc.stderr)
        stale = Mcp(server)
        try:
            owner = json.loads((tmp_path / "controllers/scope/tasks.json").read_text())[task]["owner"]
            subprocess.run([BINARY, "permission", "begin", "--task", task],
                           env={**os.environ, "AN_TURN": owner}, check=True, capture_output=True)
            import pytest
            with pytest.raises(RuntimeError, match="Active host turn required"):
                stale.tool("an_state", {})
            deadline = time.monotonic() + 5
            while servers[task]["an"]["env"]["AN_AGENT_TOKEN"] == token and time.monotonic() < deadline:
                time.sleep(.03)
            fresh = Mcp(servers[task]["an"])
            try:
                fresh.tool("an_state", {})
                assert request("view", task, {})["authorized"]
            finally:
                fresh.close()
        finally:
            stale.close()
