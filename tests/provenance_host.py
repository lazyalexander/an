"""Host and payload helpers for headless provenance acceptance cases."""
import json
import os
from pathlib import Path
import subprocess
import time

ROOT = Path(__file__).resolve().parents[1]


class Host:
    def __init__(self, directory):
        self.home = directory
        self.env = {
            k: v
            for k, v in os.environ.items()
            if k not in ("AN_AGENT_TOKEN", "AN_TURN", "AN_PROVENANCE_TOKEN")
        }
        self.env["AN_HOME"] = str(directory / "registry")
        self.task = None

    def call(self, *args, body=None, token=None, denied=None):
        env = self.env.copy()
        if token:
            env.pop("AN_TURN", None)
            env["AN_PROVENANCE_TOKEN"] = token
        proc = subprocess.run(
            [ROOT / "target/debug/an", *args],
            input=json.dumps(body or {}),
            text=True,
            env=env,
            capture_output=True,
            timeout=120,
        )
        if proc.returncode == 0 and args[0] == "export" and "--request-id" not in args:
            pending = json.loads(proc.stdout)
            if pending.get("request_id"):
                self.approve(pending["request_id"])
                return self.call(
                    *args,
                    "--request-id",
                    pending["request_id"],
                    body=body,
                    denied=denied,
                )
        if denied:
            assert proc.returncode and denied in proc.stderr, (
                args,
                proc.stdout,
                proc.stderr,
            )
            return
        if proc.returncode:
            raise RuntimeError(f"{args}: {proc.stderr}")
        output = proc.stdout.strip()
        return json.loads(output) if output.startswith(("{", "[", '"')) else output

    def graph(self, action, **args):
        return self.call("graph", "--task", self.task, action, body=args)

    def provenance(self, action, token=None, denied=None, **args):
        result = self.call(
            "provenance",
            "--task",
            self.task,
            action,
            token=token,
            body=args,
            denied=denied,
        )
        if result and result.get("request_id") and not token:
            self.approve(result["request_id"])
            return self.call(
                "provenance",
                "--task",
                self.task,
                action,
                body={**args, "request_id": result["request_id"]},
            )
        return result

    def approve(self, request):
        self.call(
            "permission",
            "decide",
            "--task",
            self.task,
            "--request-id",
            request,
            "--decision",
            "once",
        )

    def begin(self, task):
        policy = self.home / "host.json"
        policy.write_text(json.dumps({"default": "ask", "effects": []}))
        self.env["AN_TURN"] = self.call(
            "permission", "begin", "--task", task, "--file", policy
        )["turn"]
        self.task = task

    def submit(self, key, nodes, **args):
        return self.graph(
            "submit",
            idempotency_key=key,
            nodes=nodes,
            base_revision=self.graph("state")["revision"],
            **args,
        )

    def wait(self, node):
        deadline = time.monotonic() + 90
        while time.monotonic() < deadline:
            item = self.graph("state")["activity"][node]
            if item["status"] == "done":
                return item
            if item["status"] == "permission_required" or item["status"] == "queued":
                for request in self.graph("requests"):
                    self.approve(request["id"])
            elif item["status"] != "running":
                raise RuntimeError(item)
            time.sleep(0.2)
        raise TimeoutError(node)

    def cleanup(self):
        if self.task:
            self.call("graph", "--task", self.task, "control", body={"action": "stop"})
            self.graph("finish")


def endpoint(artifact, block, version=1):
    return dict(artifact=artifact, version=version, block=block)


def content(name, text, file_bytes=None, source=None, relation="quotes"):
    b = dict(id=name, text=text)
    if file_bytes is not None:
        start = file_bytes.index(text.encode())
        b["bytes"] = [start, start + len(text.encode())]
    if source:
        b["cites"] = [dict(relation=relation, source=source)]
    else:
        b["original"] = True
    return b


def version(id_, blocks, kind="document", expected=0, **args):
    return dict(artifact=id_, expected=expected, kind=kind, blocks=blocks, **args)


def output(node, state, port):
    return dict(kind="output", node=node, attempt=state["id"], port=port)
