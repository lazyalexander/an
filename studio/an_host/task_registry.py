"""Controller-owned turns and delegated MCP credentials for attached tasks."""

import json
import os
import threading
import uuid


class TaskRegistry:
    def __init__(self, directory, commands):
        self.directory = directory
        self.directory.mkdir(parents=True, mode=0o700, exist_ok=True)
        self.state = directory / "tasks.json"
        self.commands = commands
        self.guard = threading.Lock()
        self.turns = {}
        self.mcp = {}

    def restore(self, attached_task):
        records = json.loads(self.state.read_text()) if self.state.exists() else {}
        for key, record in records.items():
            self.turns[key] = record.get("owner") or record.get("an", {}).get(
                "env", {}
            ).get("AN_TURN")
        if attached_task:
            self.turns.setdefault(attached_task, os.environ.get("AN_TURN"))
        for key, owner in self.turns.items():
            self.mcp[key] = {}
            if not owner:
                continue
            observed = self.commands(["task-cursors"], {key: owner}).get(key, {})
            if observed.get("active") and not observed["ended"]:
                try:
                    self.delegate(key, owner)
                except ValueError as exc:
                    if "legacy credential" not in str(exc):
                        raise
                    owner = self.commands(
                        ["permission", "begin", "--task", key], {}, owner
                    )["controller"]
                    self.turns[key] = owner
                    self.save()
                    self.delegate(key, owner)

    def save(self):
        pending = self.state.with_suffix(".tmp")
        pending.write_text(
            json.dumps({key: {"owner": owner} for key, owner in self.turns.items()})
        )
        os.replace(pending, self.state)

    def delegate(self, key, owner):
        token = self.commands(["permission", "delegate", "--task", key], {}, owner)[
            "agent_token"
        ]
        self.mcp[key] = {
            "an": {
                "command": self.commands.binary,
                "args": ["mcp", "--task", key],
                "env": {"AN_HOME": os.environ["AN_HOME"], "AN_AGENT_TOKEN": token},
            }
        }

    def retire(self, key):
        with self.guard:
            if key not in self.turns:
                return False
            history = self.directory / "closed"
            history.mkdir(exist_ok=True, mode=0o700)
            (history / key).touch(mode=0o600)
            self.mcp.pop(key, None)
            self.turns.pop(key)
            self.save()
            return True

    def is_retired(self, key):
        return (
            isinstance(key, str)
            and key.isalnum()
            and key not in self.turns
            and (self.directory / "closed" / key).is_file()
        )

    def start(self, workspace, harness, text):
        """Create a workspace task whose first instruction is the person's sentence."""
        session = uuid.uuid4().hex
        selector = ["--harness", harness, "--session", session]
        self.commands(["workspace", "bind", workspace, *selector], {})
        created = self.commands(["task", "new", *selector], {})["task"]
        turn = self.commands(["permission", "begin", "--task", created], {})[
            "controller"
        ]
        self.commands(
            ["graph", "--task", created, "instruct"],
            {"text": text, "revision": 0},
            turn,
        )
        with self.guard:
            self.delegate(created, turn)
            self.turns[created] = turn
            self.save()
        return {"task": created}
