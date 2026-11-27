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
from an_host.controller import controller
from an_host.runner import Runner, checkpoint
from fake_agent import Mcp

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
    db.execute("INSERT OR REPLACE INTO objects VALUES('revision','seed',?)",
               (json.dumps({"note.txt": "aGk="}),))
    db.execute("INSERT OR REPLACE INTO objects VALUES('task','t',?)",
               (json.dumps({"work": {}, "inputs": {"note.txt": "seed"}}),))
    db.commit()
    db.close()
    keys = json.loads(subprocess.check_output([BINARY,"permission","begin","--task","t"]))
    monkeypatch.setenv("AN_TURN", keys["turn"])
    return home


def host(route, **body):
    proc = subprocess.run([BINARY, "graph", "--task", "t", route], input=json.dumps(body),
                          text=True, capture_output=True, check=True,
                          env=os.environ)
    return json.loads(proc.stdout)


def make_runner(home):
    settings = {**config.load()["agent"], "command": [sys.executable, str(FAKE)], "poll_ms": 50,
                "timeout_s": 30}
    token = json.loads(subprocess.check_output([BINARY,"permission","delegate","--task","t"]))["agent_token"]
    mcp = {"t": {"an": {"command": str(BINARY), "args": ["mcp", "--task", "t"],
                        "env": {"AN_HOME": str(home), "AN_AGENT_TOKEN": token}}}}
    return Runner(str(BINARY), settings, {"t": os.environ["AN_TURN"]}, mcp)


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
    assert "AN_SECRET_SHOULD_NOT_LEAK" not in seen and "AN_TURN" not in seen and "AN_AGENT_TOKEN" in seen

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
    runner = Runner(str(BINARY), settings, {"t": os.environ["AN_TURN"]}, {})
    runner.start()
    assert runner.thread is None and runner.status("t") == {"state": "off"}


def test_relative_binary_is_resolved_for_the_agent(tmp_path, monkeypatch):
    monkeypatch.chdir(BINARY.parent)
    runner = Runner("./an", {**config.load()["agent"], "command": ["true"]}, {}, {})
    assert runner.binary == str(BINARY.resolve())


def test_failed_spawn_retries_original_instruction_after_restart(task):
    host("instruct", text="recover me", revision=0)
    runner = make_runner(task)
    # Start with a cursor left by an older controller.
    (runner.directory("t") / "cursor").write_text("0")
    runner.command = [str(task / "missing-agent")]
    runner.poll("t")
    assert wakes(task) == []
    assert "Could not start agent" in runner.status("t")["detail"]
    restarted = make_runner(task)
    assert restarted.status("t") == runner.status("t")
    restarted.start()
    try:
        until(lambda: len(messages()) == 1)
        until(lambda: restarted.status("t")["state"] == "idle")
        assert messages()[0].endswith("收到：recover me")
        assert len(wakes(task)) == 1
    finally:
        restarted.close()


def test_controller_crash_during_launched_wake_does_not_replay(task):
    host("instruct", text="first", revision=0)
    # Keep the launched harness alive across controller death, then let it exit.
    pilot = """
from pathlib import Path
import sys, time
from test_runner import make_runner
runner = make_runner(Path(sys.argv[1]))
runner.command = [sys.executable, "-c", sys.argv[2]]
runner.poll("t")
time.sleep(20)
"""
    child = """
from pathlib import Path
import json, sys, time
wake = json.load(sys.stdin)
Path("received").write_text(str(wake["cursor"]))
end = time.monotonic() + 15
while not Path("settle").exists() and time.monotonic() < end:
    time.sleep(.02)
"""
    directory = task / "agents" / "t"
    env = {**os.environ, "PYTHONPATH": os.pathsep.join(map(str, [ROOT / "studio", FAKE.parent]))}
    proc = subprocess.Popen([sys.executable, "-c", pilot, str(task), child], env=env)
    try:
        until(lambda: (directory / "received").exists())
        proc.kill()
        proc.wait(timeout=5)
        restarted = make_runner(task)
        assert "unknown" in restarted.status("t")["detail"]
        restarted.poll("t")
        host("instruct", text="next", revision=0)
        restarted.poll("t")
        assert not wakes(task), "a surviving harness still holds the wake lease"
        (directory / "settle").touch()
        restarted.start()
        try:
            until(lambda: len(messages()) == 1)
            assert messages()[0].endswith("收到：next")
            assert len(wakes(task)) == 1
        finally:
            restarted.close()
    finally:
        proc.kill()
        proc.wait()
        (directory / "settle").touch()


@pytest.mark.parametrize("sequence", ["wait", "submit_first", "submit_later", "instruct_first", "instruct_later"])
def test_scope_decision_resumes_only_unsubmitted_work_after_restart(task, sequence):
    settings = {**config.load()["agent"], "command": [sys.executable, str(FAKE)], "poll_ms": 50}
    options = dict(binary=str(BINARY), workspace=None, harness="studio", task="t", agent=settings)
    scope = task / "scope"
    with controller(scope, options) as (request, servers):
        cursor = request("instruct", "t", {"text": "deliver note", "revision": 0})["cursor"]
        until(lambda: len(wakes(task)) == 1 and request("view", "t", {})["agent"]["state"] == "idle")
        agent = Mcp(servers["t"]["an"])
        try:
            route = {"operation": "publish", "source": {"input": "note.txt"},
                     "origin": "task-input:note.txt", "destination": "t"}
            card = agent.tool("an_scope_propose", {"routes": [route], "instruction_cursor": cursor})
        finally:
            agent.close()

    # Handle the person's decision while the harness is offline. In the
    # racing case, the original agent submits after the decision was recorded.
    with controller(scope, {**options, "agent": {**settings, "command": None}}) as (request, servers):
        assert request("view", "t", {})["intent"]["status"] == "pending"
        agent = Mcp(servers["t"]["an"])
        def submit():
            return agent.tool("an_graph_submit", {"base_revision": 0, "instruction_cursor": cursor,
                              "idempotency_key": "deliver-note", "nodes": [{"id": "note", "kind": "delivery",
                              "inputs": {"note": {"input": "note.txt"}}, "outputs": []}]})
        try:
            if sequence == "submit_first":
                submit()
            if sequence == "instruct_first":
                request("instruct", "t", {"text": "change goal", "revision": 0})
            for _ in range(2):
                request("intent-decide", "t", {"id": card["id"], "decisions": ["allow"]})
            if sequence == "submit_later":
                submit()
            if sequence == "instruct_later":
                request("instruct", "t", {"text": "change goal", "revision": 0})
        finally:
            agent.close()
    graph = host("state")
    decisions = [e for e in graph["events"] if e["kind"] == "intent_decided"]
    assert len(decisions) == int(sequence not in ("submit_first", "instruct_first"))
    if decisions:
        assert decisions[0]["data"] == {"intent": card["id"], "decisions": ["allow"],
                                        "instruction_cursor": cursor}
    with controller(scope, options) as (request, _):
        directory = task / "agents" / "t"
        until(lambda: checkpoint(directory)["cursor"] >= graph["cursor"]
              and request("view", "t", {})["agent"]["state"] == "idle")
    expected = [["instruction"]]
    if sequence == "wait":
        expected.append(["intent_decided"])
    if sequence.startswith("instruct"):
        expected.append(["instruction"])
    assert [w["kinds"] for w in wakes(task)] == expected


def test_launched_command_failure_preserves_log_and_never_replays(task):
    runner = make_runner(task)
    cursor = host("instruct", text="first", revision=0)["cursor"]
    runner.command = [sys.executable, "-c",
                      "import sys; print('started'); sys.stderr.buffer.write(b'bad \\xff bytes'); sys.exit(9)"]
    runner.poll("t")
    until(lambda: runner.status("t")["state"] == "failed")
    restarted = make_runner(task)
    assert restarted.status("t")["detail"] == "bad \ufffd bytes"
    directory = restarted.directory("t")
    assert (directory / "last-wake.log").read_text() == "started\n\n--- stderr ---\nbad \ufffd bytes"
    restarted.poll("t")
    assert not wakes(task) and checkpoint(directory)["cursor"] == cursor


def test_stop_terminates_the_harness_child_process(task):
    runner = make_runner(task)
    child = """
import signal, sys, time
from pathlib import Path
def stop(*_):
    Path("child-stopped").touch()
    sys.exit(0)
signal.signal(signal.SIGTERM, stop)
Path("child-ready").touch()
time.sleep(10)
"""
    adapter = "import subprocess,sys; subprocess.run([sys.executable, '-c', sys.argv[1]])"
    runner.command = [sys.executable, "-c", adapter, child]
    directory = runner.directory("t")
    host("instruct", text="start", revision=0)
    runner.poll("t")
    try:
        until(lambda: (directory / "child-ready").exists())
        host("control", action="stop")
        runner.poll("t")
        until(lambda: runner.status("t")["state"] == "failed", timeout=3)
        assert (directory / "child-stopped").exists()
    finally:
        runner.close()
