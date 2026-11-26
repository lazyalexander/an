"""The agent runner wakes the agent on the person's actions, once, under controls."""

import json
import os
import sqlite3
import subprocess
import sys
import time
from pathlib import Path

import pytest
from orchestrator import config
from an_host.runner import Runner

ROOT = Path(__file__).resolve().parents[2]
BINARY = ROOT / "target" / "debug" / "an"
FAKE = Path(__file__).with_name("fake_agent.py")

pytestmark = pytest.mark.skipif(not BINARY.exists(), reason="build An with cargo build")


@pytest.fixture
def task(tmp_path, monkeypatch):
    home = tmp_path / "an"
    home.mkdir()
    monkeypatch.setenv("AN_HOME", str(home))
    monkeypatch.setenv("AN_SECRET_SHOULD_NOT_LEAK", "x")
    subprocess.run([BINARY, "graph", "--task", "t", "view"], input="{}", text=True,
                   capture_output=True, check=False)
    db = sqlite3.connect(home / "registry.sqlite3")
    db.execute("CREATE TABLE IF NOT EXISTS objects (kind, id, body, PRIMARY KEY(kind,id))")
    db.execute("INSERT OR REPLACE INTO objects VALUES('task','t',?)",
               (json.dumps({"work": {}, "inputs": {}}),))
    db.execute("INSERT OR REPLACE INTO objects VALUES('turn','t',?)", (json.dumps("active"),))
    db.commit()
    return home


def host(route, **body):
    proc = subprocess.run([BINARY, "graph", "--task", "t", route], input=json.dumps(body),
                          text=True, capture_output=True, check=True,
                          env={**os.environ, "AN_TURN": "active"})
    return json.loads(proc.stdout)


def make_runner(home):
    settings = {**config.load()["agent"], "command": [sys.executable, str(FAKE)], "poll_ms": 50,
                "timeout_s": 30}
    mcp = {"t": {"an": {"command": str(BINARY), "args": ["mcp", "--task", "t"],
                        "env": {"AN_HOME": str(home), "AN_TURN": "active"}}}}
    return Runner(str(BINARY), settings, {"t": "active"}, mcp)


def messages():
    events = host("state", after=0)["events"]
    return [e["data"]["text"] for e in events if e["actor"] == "agent" and e["kind"] == "message"]


def until(predicate, timeout=20):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if predicate():
            return
        time.sleep(0.1)
    raise AssertionError("timed out")


def wakes(home):
    path = home / "agents" / "t" / "wakes"
    return [json.loads(line) for line in path.read_text().splitlines()] if path.exists() else []


def test_instructions_wake_the_same_agent_session_once(task):
    runner = make_runner(task)
    runner.start()
    try:
        host("instruct", text="做一张秋天的海报", revision=0)
        until(lambda: len(messages()) == 1)
        until(lambda: runner.status("t")["state"] == "idle")
        host("instruct", text="暖色调一点", revision=0)
        until(lambda: len(messages()) == 2)
    finally:
        runner.close()
    first, second = messages()
    assert first.endswith("收到：做一张秋天的海报") and second.endswith("收到：暖色调一点")
    assert first.split("]")[0] == second.split("]")[0], "same agent session resumed"
    seen = json.loads((task / "agents" / "t" / "env.json").read_text())
    assert "AN_SECRET_SHOULD_NOT_LEAK" not in seen and "AN_TURN" in seen

    # A restarted controller never wakes the agent again for handled events.
    restarted = make_runner(task)
    for _ in range(5):
        restarted.poll("t")
    assert len(wakes(task)) == 2 and len(messages()) == 2


def test_pause_defers_and_resume_wakes_once(task):
    runner = make_runner(task)
    host("control", action="pause")
    host("instruct", text="先别动", revision=0)
    for _ in range(3):
        runner.poll("t")
    assert wakes(task) == []
    host("control", action="resume")
    runner.poll("t")
    until(lambda: runner.status("t")["state"] == "idle")
    assert [w["kinds"] for w in wakes(task)] == [["instruction", "resumed"]]
    assert len(messages()) == 1


def test_stop_ends_a_running_wake(task, monkeypatch):
    monkeypatch.setenv("FAKE_AGENT_SLEEP", "30")
    runner = make_runner(task)
    host("instruct", text="慢慢来", revision=0)
    runner.poll("t")
    assert runner.status("t")["state"] == "running"
    host("control", action="stop")
    runner.poll("t")
    until(lambda: runner.status("t")["state"] == "failed", timeout=10)
    assert messages() == []


def test_without_a_command_the_runner_is_off(task):
    settings = config.load()["agent"]
    assert settings["command"] is None
    runner = Runner(str(BINARY), settings, {"t": "active"}, {})
    runner.start()
    assert runner.thread is None and runner.status("t") == {"state": "off"}


def test_relative_binary_is_resolved_for_the_agent(tmp_path, monkeypatch):
    monkeypatch.chdir(BINARY.parent)
    runner = Runner("./an", {**config.load()["agent"], "command": ["true"]}, {}, {})
    assert runner.binary == str(BINARY.resolve())
