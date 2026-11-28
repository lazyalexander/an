"""Recover a primary agent's inbox after a separate task has finished and been collected."""

import json
from pathlib import Path
import tempfile

from mcp_client import Mcp

ROOT = Path(__file__).resolve().parents[1]
from provenance_host import Host, content, endpoint, output, version

FIXTURE = ROOT / "examples/minutes/minutes.json"


def exercise(worker, primary):
    image = worker.call("register", "an-minutes-organize:provenance")
    role = ["--harness", "acceptance", "--session", "primary"]
    producer = ["--harness", "acceptance", "--session", "writer"]
    workspace = worker.call("workspace", "create", "--spool", image, *role)["workspace"]
    worker.call("workspace", "bind", workspace, *producer)
    source = endpoint("schedule", "proposal")
    policy = worker.home / "routes.json"
    policy.write_text(
        json.dumps(
            [
                ["input", "minutes.json", image],
                ["preview", image, "provenance"],
                ["submit_review", source, workspace],
            ]
        )
    )
    worker.call("workspace", "policy", "set", workspace, "--file", policy)
    primary.begin(primary.call("task", "new", *role)["task"])
    worker.begin(worker.call("task", "new", *producer, "--input", FIXTURE)["task"])
    reader = primary.provenance(
        "grant", target=primary.task, rights=[{"right": "read_inbox"}]
    )

    def reconnect():
        return Mcp(
            {
                "command": str(ROOT / "target/debug/an"),
                "args": ["mcp", *role],
                "env": {
                    **primary.env,
                    "AN_TURN": "",
                    "AN_AGENT_TOKEN": "",
                    "AN_PROVENANCE_TOKEN": reader["token"],
                },
            }
        )

    def inbox(client):
        return client.tool("an_provenance", {"action": "inbox", "payload": {}})["items"]

    client = reconnect()
    try:
        assert inbox(client) == []
    finally:
        client.close()
    files = ["draft-a.md", "draft-b.md", "draft-c.md", "proposal.md"]
    text = "Propose a Friday release."
    private = "Internal-only note."
    program = """from pathlib import Path
for name in sys.argv[1:4]:
    Path('/output',name).write_text('Private draft: '+name)
Path('/output/proposal.md').write_text(sys.argv[4]+'\\n'+sys.argv[5])
print('Private review log')
"""
    worker.submit(
        "drafts",
        [
            {
                "id": "write",
                "kind": "run",
                "spool": image,
                "argv": ["python3", "-c", program, *files[:3], text, private],
                "inputs": {"minutes": {"input": "minutes.json"}},
                "outputs": files,
            }
        ],
    )
    run = worker.wait("write")
    assert set(run["outputs"]) == set(files)
    material = output("write", run, "proposal.md")
    worker.provenance("admit-material", material=material)
    data = (text + "\n" + private).encode()
    worker.provenance(
        "commit",
        idempotency_key="register",
        versions=[
            version(
                "schedule",
                [content("proposal", text, data), content("internal", private, data)],
                file="proposal.md",
                material=material,
            )
        ],
    )

    def send_right(block):
        return [{"right": "submit_review", "source": block}]

    sibling = endpoint("schedule", "internal")
    worker.provenance(
        "grant",
        target=worker.task,
        rights=send_right(sibling),
        denied="Denied by workspace flow policy",
    )
    worker.provenance("inbox", denied="outside authority")
    sender = worker.provenance("grant", target=worker.task, rights=send_right(source))

    def submit(block, key, denied=None):
        return worker.provenance(
            "submit-review",
            token=sender["token"],
            idempotency_key=key,
            source=block,
            denied=denied,
        )

    receipt = submit(source, "send")
    assert (
        receipt == submit(source, "send")
        and receipt["attempt"] == run["id"]
        and receipt["status"] == "pending"
    )
    submit(sibling, "send", denied="different content")
    submit(sibling, "other", denied="outside authority")
    worker.provenance(
        "submit-review",
        token=sender["token"],
        idempotency_key="wrong-purpose",
        source=source,
        purpose="publish",
        denied="unknown field",
    )
    worker.graph("finish")
    worker.task = None
    for _ in range(2):
        client = reconnect()
        try:
            items = inbox(client)
            assert items == [{"text": text, "receipt": receipt}]
            assert all(
                marker not in json.dumps(items)
                for marker in [*files[:3], private, "Private review log"]
            )
            try:
                client.tool("an_provenance", {"action": "sources", "payload": sibling})
            except RuntimeError as error:
                assert "outside authority" in str(error), error
            else:
                raise AssertionError("inbox credential exposed a private source")
        finally:
            client.close()

    primary.provenance("revoke", grant=reader["grant"])
    primary.provenance("inbox", token=reader["token"], denied="Inactive delegation")


if __name__ == "__main__":
    if not __debug__:
        raise SystemExit("Run with assertions enabled")
    with tempfile.TemporaryDirectory(prefix="workspace-inbox-") as root:
        worker, primary = Host(Path(root)), Host(Path(root))
        try:
            exercise(worker, primary)
        finally:
            for host in (worker, primary):
                host.cleanup()
    print("Verified offline hand-in, scoped inbox and reconnect after task closure.")
