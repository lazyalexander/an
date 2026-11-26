"""Harness-neutral agent runner.

The runner lives in the controller process that holds task turns. It watches
each task's events and wakes the configured agent command when the person
does something the agent must react to: an instruction, an answered surface,
or resuming a paused task.

Contract with an agent command (any harness):

- It is started with the working directory set to the task's private state
  directory, which persists across wakes so the agent can resume a session.
- Its stdin is one JSON object: ``task``, ``cursor`` (last event covered by
  this wake), ``events`` (the triggering events), ``mcp_servers`` (An MCP
  configuration for this task, including its turn) and ``state_dir``.
- Its environment holds only ``HOME``, ``PATH``, ``TMPDIR``, the names listed
  in ``agent.env``, and ``AN_HOME``/``AN_TURN``/``AN_TASK``/``AN_BINARY``.
- It acts only through An MCP. Exit 0 means the wake was handled.

Delivery is at most once: the cursor is written before the agent starts, the
same rule An applies to downstream calls with an uncertain outcome. A failed
wake is reported, not retried; the person's next action wakes the agent again.
"""

import json
import os
import select
import sqlite3
from contextlib import ExitStack, closing
import shutil
import subprocess
import threading
from pathlib import Path

TRIGGERS = {("user", "instruction"), ("user", "responded"), ("user", "resumed")}
BASE_ENV = ("HOME", "PATH", "TMPDIR")


def state_root() -> Path:
    home = os.environ.get("AN_HOME")
    return (Path(home) if home else Path.home() / ".an") / "agents"


class Runner:
    def __init__(self, binary, settings, turns, mcp, root=None, retire=None):
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
        self.guard = threading.Lock()
        self.stop_event = threading.Event()
        self.thread = None
        self.retire = retire

    # -- lifecycle -------------------------------------------------------
    def start(self):
        if self.command is None and self.retire is None:
            return
        self.thread = threading.Thread(target=self.loop, daemon=True)
        self.thread.start()

    def close(self):
        self.stop_event.set()
        with self.guard:
            procs = list(self.procs.values())
        for proc in procs:
            proc.terminate()
        if self.thread:
            self.thread.join(timeout=5)

    def status(self, task):
        if self.command is None:
            return {"state": "off"}
        with self.guard:
            return dict(self.status_by_task.get(task, {"state": "idle"}))

    # -- polling ---------------------------------------------------------
    def loop(self):
        path = self.root.parent / "registry.sqlite3"
        while not self.stop_event.is_set():
            try:
                with ExitStack() as stack:
                    db = stack.enter_context(closing(sqlite3.connect(path.as_uri() + "?mode=ro", uri=True)))
                    queue = None
                    if hasattr(select, "kqueue"):
                        file = stack.enter_context(path.open("rb"))
                        queue = stack.enter_context(closing(select.kqueue()))
                        event = select.kevent(file.fileno(), filter=select.KQ_FILTER_VNODE,
                                              flags=select.KQ_EV_ADD | select.KQ_EV_CLEAR,
                                              fflags=select.KQ_NOTE_WRITE)
                        queue.control([event], 0, 0)
                    while not self.stop_event.is_set():
                        for task, turn in list(self.turns.items()):
                            row = db.execute("""
                                SELECT json_extract(t.body,'$.closed'), u.body,
                                       coalesce(json_array_length(g.body,'$.events'),0)
                                FROM objects t
                                LEFT JOIN objects u ON u.kind='turn' AND u.id=t.id
                                LEFT JOIN objects g ON g.kind='graph' AND g.id=t.id
                                WHERE t.kind='task' AND t.id=?
                            """, (task,)).fetchone()
                            if row is None:
                                continue
                            if row[0] or (turn and (row[1] is None or json.loads(row[1]) != turn)):
                                if self.retire:
                                    self.retire(task)
                                continue
                            cursor = self.root / task / "cursor"
                            if self.command and row[2] > (int(cursor.read_text()) if cursor.exists() else 0):
                                try:
                                    self.poll(task)
                                except (OSError, ValueError) as exc:
                                    self.set_status(task, "failed", str(exc))
                        if queue:
                            queue.control(None, 1, min(1, self.poll_s))
                        else:
                            self.stop_event.wait(self.poll_s)
            except (OSError, sqlite3.Error):
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
        cursor_file = directory / "cursor"
        cursor = int(cursor_file.read_text()) if cursor_file.exists() else 0
        state = self.an(task, "state", {"after": cursor})
        events = state.get("events", [])
        with self.guard:
            running = self.procs.get(task)
        if running is not None:
            # A stop is the person's brake: end the agent's current wake too.
            if any(e.get("actor") == "user" and e.get("kind") == "stopped" for e in events):
                running.terminate()
            return
        triggers = [e for e in events if (e.get("actor"), e.get("kind")) in TRIGGERS]
        if any(e.get("kind") == "closed" for e in events):
            triggers = []
        elif triggers and state.get("control") == "paused":
            return  # keep the events; resuming wakes the agent once
        new_cursor = state.get("cursor", cursor)
        if new_cursor == cursor:
            return
        tmp = cursor_file.with_suffix(".tmp")
        tmp.write_text(str(new_cursor))
        os.replace(tmp, cursor_file)  # durable before launch: at most once
        if triggers:
            self.wake(task, directory, new_cursor, triggers)

    # -- waking ----------------------------------------------------------
    def set_status(self, task, state, detail=None):
        with self.guard:
            self.status_by_task[task] = {"state": state, **({"detail": detail} if detail else {})}

    def wake(self, task, directory, cursor, triggers):
        env = {k: os.environ[k] for k in (*BASE_ENV, *self.passthrough) if k in os.environ}
        env.update(AN_TASK=task, AN_BINARY=str(self.binary),
                   AN_HOME=os.environ.get("AN_HOME", str(self.root.parent)),
                   AN_TURN=self.turns.get(task) or "")
        payload = {"task": task, "cursor": cursor, "events": triggers,
                   "mcp_servers": self.mcp.get(task, {}), "state_dir": str(directory)}
        proc = subprocess.Popen(self.command, cwd=directory, env=env, text=True,
                                stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                stderr=subprocess.PIPE)
        with self.guard:
            self.procs[task] = proc
        self.set_status(task, "running")
        threading.Thread(target=self.finish, args=(task, directory, proc, payload),
                         daemon=True).start()

    def finish(self, task, directory, proc, payload):
        try:
            out, err = proc.communicate(json.dumps(payload), timeout=self.timeout_s)
        except subprocess.TimeoutExpired:
            proc.kill()
            out, err = proc.communicate()
            err = (err or "") + "\nagent wake timed out"
        (directory / "last-wake.log").write_text((out or "") + "\n--- stderr ---\n" + (err or ""))
        with self.guard:
            self.procs.pop(task, None)
        if proc.returncode == 0:
            self.set_status(task, "idle")
        else:
            tail = (err or "").strip().splitlines()[-1:] or [f"exit {proc.returncode}"]
            self.set_status(task, "failed", tail[0][:300])
