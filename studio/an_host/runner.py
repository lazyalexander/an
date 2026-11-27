"""Harness-neutral agent runner.

The runner lives in the controller process that holds task turns. It watches
each task's events and wakes the configured agent command when the person
does something the agent must react to: an instruction, an answered surface,
resuming a paused task, or deciding an intent whose graph still needs submitting.

Contract with an agent command (any harness):

- It is started with the working directory set to the task's private state
  directory, which persists across wakes so the agent can resume a session.
- Its stdin is one JSON object: ``task``, ``cursor`` (last event covered by
  this wake), ``events`` (the triggering events), ``mcp_servers`` (An MCP
  configuration for this task, including its delegated credential) and ``state_dir``.
- Its environment holds only ``HOME``, ``PATH``, ``TMPDIR``, the names listed
  in ``agent.env``, and ``AN_HOME``/``AN_AGENT_TOKEN``/``AN_TASK``/``AN_BINARY``.
- It acts only through An MCP. Exit 0 means the wake was handled.

The checkpoint records the covered events before launch. A definite spawn
failure restores the prior cursor for retry. A launched command may already
have acted, so failure or loss of its result is reported without replay.
"""

import fcntl
import json
import os
import shutil
import signal
import subprocess
import threading
import time
from pathlib import Path

TRIGGERS = {("user", "instruction"), ("user", "responded"), ("user", "resumed")}
BASE_ENV = ("HOME", "PATH", "TMPDIR")


def state_root() -> Path:
    home = os.environ.get("AN_HOME")
    return (Path(home) if home else Path.home() / ".an") / "agents"


def checkpoint(directory):
    try:
        value = json.loads((directory / "cursor").read_text())
    except FileNotFoundError:
        value = 0
    # Legacy runners wrote only the integer event cursor.
    return {"cursor": value, "state": "idle"} if isinstance(value, int) else value


def save_checkpoint(directory, value):
    temp = directory / "cursor.tmp"
    with temp.open("w") as stream:
        json.dump(value, stream)
        stream.flush()
        os.fsync(stream.fileno())
    temp.replace(directory / "cursor")


def wake_events(events):
    # The first wake may have submitted between the person's approval and
    # its own exit. Wait for the lease, then check the full unread batch.
    submitted = {e.get("instruction_cursor") for e in events
                 if e.get("actor") == "agent" and e.get("kind") == "submitted"}
    latest = max((e["cursor"] for e in events if (e.get("actor"), e.get("kind"))
                  == ("user", "instruction")), default=0)
    triggers = []
    for event in events:
        key = event.get("actor"), event.get("kind")
        if key == ("user", "intent_decided"):
            cursor = event["data"]["instruction_cursor"]
            if cursor >= latest and cursor not in submitted:
                triggers.append(event)
        elif key in TRIGGERS:
            triggers.append(event)
    return triggers


def signal_wake(proc, sig=signal.SIGTERM):
    # The adapter and its CLI/MCP children belong to the session we created.
    # Either the command or the entire group may already have exited.
    try:
        os.killpg(proc.pid, sig)
    except ProcessLookupError:
        pass


class Runner:
    def __init__(self, binary, settings, turns, mcp, root=None, retire=None, refresh=None):
        # The agent starts in its own directory: hand it an absolute binary path.
        found = shutil.which(binary)
        self.binary = str(Path(found).resolve()) if found else binary
        self.command = settings["command"]
        self.poll_s = settings["poll_ms"] / 1000
        self.timeout_s = settings["timeout_s"]
        self.passthrough = settings["env"]
        self.turns = turns          # task -> turn, owned by the controller
        self.mcp = mcp              # task -> MCP server config for the agent
        self.root = Path(root) if root else state_root()
        self.status_by_task = {}
        self.procs = {}
        self.finishers = {}
        self.guard = threading.Lock()
        self.stop_event = threading.Event()
        self.thread = None
        self.retire = retire
        self.refresh = refresh

    # -- lifecycle -------------------------------------------------------
    def start(self):
        if self.command is None and self.retire is None:
            return
        self.thread = threading.Thread(target=self.loop, daemon=True)
        self.thread.start()

    def close(self):
        self.stop_event.set()
        if self.thread:
            self.thread.join(timeout=5)
        with self.guard:
            active = [(proc, self.finishers[task]) for task, proc in self.procs.items()]
        for proc, _ in active:
            signal_wake(proc)
        deadline = time.monotonic() + 2
        for proc, worker in active:
            worker.join(timeout=max(0, deadline - time.monotonic()))
            if worker.is_alive():
                signal_wake(proc, signal.SIGKILL)
                worker.join(timeout=2)

    def status(self, task):
        if self.command is None:
            return {"state": "off"}
        with self.guard:
            if task in self.status_by_task:
                return dict(self.status_by_task[task])
        saved = checkpoint(self.root / task)
        return {k: saved[k] for k in ("state", "detail") if k in saved}

    # -- polling ---------------------------------------------------------
    def loop(self):
        generations = {}
        while not self.stop_event.is_set():
            snapshot = self.turns.copy()
            if snapshot:
                try:
                    proc = subprocess.run([self.binary, "task-cursors"], input=json.dumps(snapshot),
                                          capture_output=True, text=True, check=True)
                    states = json.loads(proc.stdout)
                except (OSError, ValueError, subprocess.CalledProcessError):
                    states = {}
                for task, row in states.items():
                    if row["closed"] or row["ended"]:
                        if self.retire:
                            self.retire(task)
                        generations.pop(task, None)
                        continue
                    if not row["active"]:
                        self.set_status(task, "waiting", "Controller credential required")
                        continue
                    try:
                        if self.refresh and generations.get(task) != row["generation"]:
                            self.refresh(task)
                        generations[task] = row["generation"]
                        saved = checkpoint(self.root / task)
                        if self.command and row["cursor"] > saved["cursor"] and time.time() >= saved.get("retry_at", 0):
                            if not row["paused"] or task in self.procs:
                                self.poll(task)
                    except (OSError, ValueError, KeyError) as exc:
                        self.set_status(task, "failed", str(exc))
            self.stop_event.wait(self.poll_s)

    def an(self, task, action, body):
        env = dict(os.environ)
        if self.turns.get(task):
            env["AN_TURN"] = self.turns[task]
        proc = subprocess.run([self.binary, "graph", "--task", task, action],
                              input=json.dumps(body), text=True, capture_output=True,
                              env=env, check=False)
        if proc.returncode:
            raise ValueError(proc.stderr.strip() or "An command failed")
        return json.loads(proc.stdout)

    def directory(self, task):
        path = self.root / task
        path.mkdir(parents=True, exist_ok=True, mode=0o700)
        return path

    def poll(self, task):
        directory = self.directory(task)
        with self.guard:
            running = self.procs.get(task)
        if running is not None:
            state = self.an(task, "state", {"after": checkpoint(directory)["cursor"]})
            if any(e.get("actor") == "user" and e.get("kind") == "stopped" for e in state["events"]):
                signal_wake(running)
            return
        # Hold the lease through settlement. A still-running child also holds it
        # if its controller exits while waiting for its result.
        lease = (directory / "wake.lock").open("a")
        try:
            try:
                fcntl.flock(lease, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError:
                return
            if self.launch_if_ready(task, directory, lease):
                lease = None  # finish owns it now
        finally:
            if lease is not None:
                lease.close()

    def launch_if_ready(self, task, directory, lease):
        saved = checkpoint(directory)
        if time.time() < saved.get("retry_at", 0):
            return
        cursor = saved["cursor"]
        state = self.an(task, "state", {"after": cursor})
        events = state.get("events", [])
        triggers = wake_events(events)
        if any(e.get("kind") == "closed" for e in events):
            triggers = []
        elif triggers and state.get("control") == "paused":
            return  # keep the events; resuming wakes the agent once
        new_cursor = state.get("cursor", cursor)
        if new_cursor == cursor:
            return
        if not triggers:
            save_checkpoint(directory, {**saved, "cursor": new_cursor})
            return
        return self.wake(task, directory, new_cursor, triggers, saved, lease)

    # -- waking ----------------------------------------------------------
    def set_status(self, task, state, detail=None):
        with self.guard:
            self.status_by_task[task] = {"state": state, **({"detail": detail} if detail else {})}

    def wake(self, task, directory, cursor, triggers, saved, lease):
        env = {k: os.environ[k] for k in (*BASE_ENV, *self.passthrough) if k in os.environ}
        env.pop("AN_TURN", None)
        env.update(AN_TASK=task, AN_BINARY=str(self.binary),
                   AN_HOME=os.environ.get("AN_HOME", str(self.root.parent)),
                   AN_AGENT_TOKEN=self.mcp[task]["an"]["env"]["AN_AGENT_TOKEN"])
        payload = {"task": task, "cursor": cursor, "events": triggers,
                   "mcp_servers": self.mcp.get(task, {}), "state_dir": str(directory)}
        with self.guard:
            if self.stop_event.is_set():
                return False
            save_checkpoint(directory, {"cursor": cursor, "state": "failed",
                                        "detail": f"Outcome of agent wake through event {cursor} is unknown"})
            try:
                proc = subprocess.Popen(self.command, cwd=directory, env=env, encoding="utf-8", errors="replace",
                                        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                        pass_fds=(lease.fileno(),), start_new_session=True)
            except (OSError, ValueError) as exc:
                delay = min(60, 2 * saved.get("retry_delay", 0) or 1)
                detail = f"Could not start agent: {exc}. Retrying in {delay}s"
                save_checkpoint(directory, {"cursor": saved["cursor"], "state": "failed", "detail": detail,
                                            "retry_at": time.time() + delay, "retry_delay": delay})
                self.status_by_task[task] = {"state": "failed", "detail": detail}
                return False
            worker = threading.Thread(target=self.finish, args=(task, directory, proc, payload, lease),
                                      daemon=True)
            self.procs[task] = proc
            self.finishers[task] = worker
            self.status_by_task[task] = {"state": "running"}
            worker.start()
        return True

    def finish(self, task, directory, proc, payload, lease):
        state, detail = "failed", "Agent wake outcome is unknown"
        try:
            timed_out = False
            try:
                out, err = proc.communicate(json.dumps(payload), timeout=self.timeout_s)
            except subprocess.TimeoutExpired:
                timed_out = True
                signal_wake(proc, signal.SIGKILL)
                out, err = proc.communicate()
                err = (err or "") + "\nagent wake timed out"
            if proc.returncode == 0 and not timed_out:
                state, detail = "idle", None
            else:
                tail = (err or "").strip().splitlines()[-1:] or [f"exit {proc.returncode}"]
                detail = tail[0][:300]
            save_checkpoint(directory, {"cursor": payload["cursor"], "state": state,
                                        **({"detail": detail} if detail else {})})
            (directory / "last-wake.log").write_text((out or "") + "\n--- stderr ---\n" + (err or ""),
                                                      encoding="utf-8")
        finally:
            with self.guard:
                self.procs.pop(task, None)
                self.finishers.pop(task, None)
                self.status_by_task[task] = {"state": state, **({"detail": detail} if detail else {})}
            lease.close()
