"""Exercise the five provenance contracts using the real minutes spools and a disposable registry."""

import base64
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parents[2]
FIXTURE = Path(__file__).with_name("minutes.json")
COMMAND = ["python3", "/spool/code/process.py"]


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


def step(name, spool, port, source, file, *options):
    return dict(
        id=name,
        kind="run",
        spool=spool,
        argv=COMMAND + list(options),
        inputs={port: source},
        outputs=[file, "blocks.json"],
    )


def create_task(host):
    organize, formatter = (
        host.call("register", f"an-minutes-{name}:provenance")
        for name in ("organize", "format")
    )
    workspace = host.call(
        "workspace", "create", "--spool", organize, "--spool", formatter
    )["workspace"]
    selector = ["--harness", "minutes-provenance", "--key", workspace]
    host.call("workspace", "bind", workspace, *selector)
    dest = host.home.resolve() / "export"
    policy = host.home / "flows.json"
    policy.write_text(
        json.dumps(
            [
                ["input", "minutes.json", organize],
                ["block_input", endpoint("minutes", "opening"), formatter],
                ["handoff", organize, formatter],
                ["handoff", organize, organize],
                ["publish", organize, "graph-metadata"],
                ["preview", "task-input:minutes.json", "provenance"],
                ["preview", organize, "provenance"],
                ["preview", formatter, "provenance"],
                ["publish", formatter, workspace],
                ["export", formatter, str(dest)],
            ]
        )
    )
    host.call("workspace", "policy", "set", workspace, "--file", policy)
    host.begin(host.call("task", "new", *selector, "--input", FIXTURE)["task"])
    return organize, formatter, dest


def propose(host, organize):
    raw = FIXTURE.read_bytes()
    monday, free = json.loads(raw)["public_items"]
    source = {"kind": "input", "name": "minutes.json"}
    host.provenance("admit-material", material=source)
    host.provenance(
        "commit",
        idempotency_key="minutes",
        versions=[
            version(
                "minutes",
                [content("opening", monday, raw), content("registration", free, raw)],
                material=source,
                file="minutes.json",
            )
        ],
    )
    host.submit(
        "organize",
        [
            step(
                "organize",
                organize,
                "minutes",
                {"input": "minutes.json"},
                "summary.json",
                "--provenance",
            )
        ],
    )
    run = host.wait("organize")
    summary = output("organize", run, "summary.json")
    declaration = output("organize", run, "blocks.json")
    for material in [declaration, summary]:
        host.provenance("admit-material", material=material)
    snapshot = host.provenance("freeze", attempt=run["id"], start=0, length=1024)[
        "snapshot"
    ]
    second_snapshot = host.provenance("freeze", attempt=run["id"], start=1, length=3)[
        "snapshot"
    ]
    drafts = [
        version(
            "opening",
            [],
            kind="decision",
            material=summary,
            declaration=declaration,
            file="summary.json",
            evidence=[snapshot],
        ),
        version(
            "registration",
            [],
            material=summary,
            declaration=declaration,
            file="summary.json",
        ),
    ]
    host.provenance("commit", versions=drafts, idempotency_key="proposal")
    return monday, free, snapshot, second_snapshot


def accept_and_expand(host, organize, formatter):
    summary_ref = {"node": "organize", "port": "summary.json"}
    host.submit(
        "continuation",
        [
            dict(
                id="continue",
                kind="expand",
                spool=organize,
                argv=["python3", "/spool/code/continuation.py", formatter],
                inputs={"minutes": summary_ref},
                outputs=["subgraph.json"],
                await_decision={"artifact": "opening", "version": 1},
            )
        ],
    )
    position = host.graph("state")["cursor"]
    before = host.provenance("history", cursor=position)
    assert (
        before["decisions"][0]["state"] == "proposed"
        and "format" not in before["nodes"]
    )
    choice = dict(
        artifact="opening",
        version=1,
        choice="accept",
        seen_cursor=position,
        reason="Approved the recorded opening day",
        idempotency_key="choose",
    )
    receipt = host.provenance("decide", **choice)
    assert host.provenance("decide", **choice) == receipt
    after = host.provenance("history", cursor=receipt["accepted"]["cursor"])
    assert after["decisions"][0]["state"] == "accept" and "format" not in after["nodes"]
    assert before["nodes"]["continue"]["activity"]["waiting_reason"] == {
        "reason": "decision"
    }
    host.wait("continue")
    expanded = next(e for e in host.graph("state")["events"] if e["kind"] == "expanded")
    assert expanded["data"]["decision_receipt"] == receipt["id"]
    assert "format" in host.provenance("history", cursor=expanded["cursor"])["nodes"]
    assert host.provenance("history", cursor=position) == before
    return receipt


def publish(host):
    documents = []
    for node, file, artifact in [
        ("format", "announcement.md", "announcement"),
        ("actions", "actions.md", "actions"),
    ]:
        state = host.wait(node)
        material = output(node, state, file)
        host.provenance("admit-material", material=material)
        declaration = output(node, state, "blocks.json")
        host.provenance("admit-material", material=declaration)
        documents.append(
            version(artifact, [], material=material, file=file, declaration=declaration)
        )
    host.provenance("commit", versions=documents, idempotency_key="documents")
    host.submit(
        "publish",
        [
            dict(
                id="published",
                kind="delivery",
                outputs=[],
                inputs={
                    "announcement": {"node": "format", "port": "announcement.md"},
                    "actions": {"node": "actions", "port": "actions.md"},
                },
                versions={
                    name: {"artifact": name, "version": 1}
                    for name in ("announcement", "actions")
                },
            )
        ],
    )
    return host.wait("published")["delivery"]


def check_trace(host, delivery, receipt, organize, formatter):
    trace = host.provenance(
        "sources", delivery=delivery, binding="announcement", block="opening"
    )
    assert [b["ref"]["artifact"] for b in trace["blocks"]] == [
        "announcement",
        "opening",
        "minutes",
    ]
    assert trace["blocks"][1]["decision"]["id"] == receipt["id"]
    assert [b["material"]["execution"]["spool"] for b in trace["blocks"][:2]] == [
        formatter,
        organize,
    ]
    assert all(
        b["material"]["declaration"]["port"] == "blocks.json"
        and b["material"]["execution"]["argv"] == [*COMMAND, "--provenance"]
        for b in trace["blocks"][:2]
    )
    uses = host.provenance("impact", **endpoint("opening", "conclusion"))
    assert {b["ref"]["artifact"] for b in uses["blocks"]} == {
        "opening",
        "announcement",
        "actions",
    }
    phases = {flow["phase"] for block in uses["blocks"] for flow in block["file_flows"]}
    assert phases >= {"authorized", "handed_off", "published"}


def revise(host, resumed, delivery, monday, free, snapshot, second_snapshot):
    resumed.begin(host.call("task", "resume", delivery)["task"])
    agent = host.provenance(
        "grant",
        target=resumed.task,
        rights=[
            {"right": "revise", "artifact": "opening"},
            {"right": "read", "artifact": "opening", "version": 1},
        ],
    )
    tuesday = monday.replace("Monday", "Tuesday")
    change = version(
        "opening", [content("conclusion", tuesday)], kind="decision", expected=1
    )
    revised = resumed.provenance(
        "commit",
        token=agent["token"],
        idempotency_key="change",
        versions=[change],
        reported_participant="editor",
    )
    assert revised["versions"] == [{"artifact": "opening", "version": 2}]
    registered = host.provenance("sources", **endpoint("opening", "conclusion", 2))[
        "blocks"
    ][0]
    assert (
        registered["delegation"]["grant"] == agent["grant"]
        and registered["reported_participant"] == "editor"
    )
    reader = host.provenance(
        "grant",
        target=resumed.task,
        rights=[{"right": "evidence", "snapshot": snapshot}],
    )
    original_log = resumed.provenance(
        "read-evidence", token=reader["token"], snapshot=snapshot
    )["base64"]
    assert base64.b64decode(original_log) == b"Selected 2 public items\n"
    resumed.provenance(
        "read-evidence",
        token=reader["token"],
        snapshot=second_snapshot,
        denied="outside authority",
    )
    resumed.provenance(
        "commit",
        token=reader["token"],
        idempotency_key="bad",
        versions=[change],
        denied="outside authority",
    )
    host.provenance("revoke", grant=reader["grant"])
    resumed.provenance(
        "read-evidence",
        token=reader["token"],
        snapshot=snapshot,
        denied="Inactive delegation",
    )
    host.provenance(
        "commit",
        idempotency_key="adopt",
        versions=[
            version(
                "announcement",
                [
                    content(
                        "opening", tuesday, source=endpoint("opening", "conclusion", 2)
                    ),
                    content(
                        "registration", free, source=endpoint("registration", "rule")
                    ),
                ],
                expected=1,
            )
        ],
    )
    assert (
        host.provenance("sources", artifact="announcement", version=2, block="opening")[
            "blocks"
        ][0]["content"]["text"]
        == tuesday
    )
    old = host.provenance(
        "sources", delivery=delivery, binding="announcement", block="opening"
    )["blocks"][0]
    assert old["content"]["text"] == monday
    still_using = host.provenance("impact", **endpoint("opening", "conclusion"))
    assert {
        (b["ref"]["artifact"], b["ref"]["version"]) for b in still_using["blocks"]
    } == {("opening", 1), ("announcement", 1), ("actions", 1), ("review", 1)}

    return original_log


def check_export(host, delivery, dest):
    # Interrupt the final receipt transaction after staging real files.
    registry = host.home / "registry/registry.sqlite3"
    with sqlite3.connect(registry) as db:
        db.execute(
            "CREATE TRIGGER interrupt_confirmation BEFORE UPDATE ON pv_export BEGIN SELECT RAISE(ABORT,'lost confirmation'); END"
        )
    export_args = ["export", delivery, dest, "--task", host.task]
    host.call(*export_args, denied="lost confirmation")
    dest.rename(host.home / "unconfirmed")
    with sqlite3.connect(registry) as db:
        db.execute("DROP TRIGGER interrupt_confirmation")
    assert host.call(*export_args)["status"] == "unknown" and not dest.exists()
    impacts = host.provenance("impact", **endpoint("opening", "conclusion"))
    phases = {e["phase"] for b in impacts["blocks"] for e in b.get("file_flows", [])}
    assert "unknown" in phases and "exported" not in phases
    assert host.call(*export_args, "--retry")["status"] == "exported"
    assert (dest / "announcement/announcement.md").read_bytes() == (
        host.home / "unconfirmed/announcement/announcement.md"
    ).read_bytes()


def check_block_input(host, formatter, monday):
    host.provenance(
        "handoff-block",
        source=endpoint("minutes", "registration"),
        recipient=formatter,
        denied="Denied by workspace flow policy",
    )
    ticket = host.provenance(
        "handoff-block", source=endpoint("minutes", "opening"), recipient=formatter
    )
    probe = """from pathlib import Path
import sys
root=Path('/input')
files=[p.relative_to(root).as_posix() for p in root.rglob('*') if p.is_file()]
assert files==['summary/block.txt'], files
assert (root/files[0]).read_bytes()==sys.argv[1].encode()
"""
    instance = host.call("load", "--task", host.task, "--spool", formatter)["id"]
    args = [
        "run",
        "--task",
        host.task,
        "--instance",
        instance,
        "--inputs",
        json.dumps({"summary": ticket["handoff"]}),
        "--",
        "python3",
        "-c",
        probe,
        monday,
    ]
    run = host.call(*args)
    deadline = time.monotonic() + 90
    while time.monotonic() < deadline:
        state = host.call("status", "--task", host.task, run)["status"]
        if state != "running":
            assert state == "completed", state
            break
        time.sleep(0.2)
    else:
        raise TimeoutError(run)
    host.call(*args, denied="Invalid or revoked handoff")


def exercise(host, resumed):
    organize, formatter, dest = create_task(host)
    monday, free, snapshot, second_snapshot = propose(host, organize)
    check_block_input(host, formatter, monday)
    receipt = accept_and_expand(host, organize, formatter)
    delivery = publish(host)
    check_trace(host, delivery, receipt, organize, formatter)
    host.provenance(
        "commit",
        idempotency_key="review",
        versions=[
            version(
                "review",
                [
                    content(
                        "checked",
                        monday,
                        source=endpoint("announcement", "opening"),
                        relation="verifies",
                    )
                ],
                kind="report",
            )
        ],
    )
    reader = host.provenance(
        "grant",
        target=host.task,
        rights=[{"right": "read", "artifact": "announcement", "version": 1}],
    )
    visible = host.provenance(
        "sources", token=reader["token"], **endpoint("announcement", "opening")
    )
    assert len(visible["blocks"]) == 1 and not visible["edges"]
    host.provenance(
        "commit",
        token=reader["token"],
        denied="outside authority",
        idempotency_key="overwrite",
        versions=[
            version(
                "opening",
                [content("conclusion", "Changed")],
                kind="decision",
                expected=1,
            )
        ],
    )
    original_log = revise(
        host, resumed, delivery, monday, free, snapshot, second_snapshot
    )
    reviewed = host.provenance("sources", **endpoint("review", "checked"))
    assert reviewed["edges"][0]["source"] == endpoint("announcement", "opening", 1)
    check_export(host, delivery, dest)
    host.graph("finish")
    host.task = None
    assert resumed.provenance("reopen-evidence", delivery=delivery, snapshot=snapshot)[
        "authorized"
    ]
    assert (
        resumed.provenance("read-evidence", snapshot=snapshot)["base64"] == original_log
    )
    print(
        "Verified block references, three graph positions, delegated revision, GC retention and unknown export recovery."
    )


if __name__ == "__main__":
    if not __debug__:
        raise SystemExit("Run acceptance with Python assertions enabled")
    with tempfile.TemporaryDirectory(prefix="minutes-provenance-") as root:
        original, resumed = Host(Path(root)), Host(Path(root))
        try:
            exercise(original, resumed)
        finally:
            for host in (original, resumed):
                host.cleanup()
