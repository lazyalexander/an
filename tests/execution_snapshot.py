"""Exercise a scoped snapshot on the workspace inbox case's registered material."""
from mcp_client import Mcp
from provenance_host import ROOT, content, version


def check_snapshot(worker, primary, role, source, receipt_text):
    # Publication is denied by this workspace. Its settled attempt can be retried.
    def delivery(name):
        return {"id": name, "kind": "delivery", "inputs": {"doc": {"node": "write", "port": "proposal.md"}},
                "outputs": []}

    worker.submit("inspect-failure", [delivery("inspect")])
    state = worker.graph("state")
    attempt = state["activity"]["inspect"]
    assert attempt["status"] == "failed", attempt
    selection = dict(cursor=state["cursor"], node="inspect", source=source)
    worker.provenance("freeze-execution", **{**selection, "source": {**source, "version": 2}},
                      denied="Block unavailable")
    ref = worker.provenance("freeze-execution", **selection)
    assert worker.provenance("freeze-execution", **selection) == ref

    def grant(snapshot, denied=None):
        return worker.provenance("grant", target=primary.task,
                                 rights=[{"right": "inspect_snapshot", **snapshot}], denied=denied)

    private = worker.provenance("freeze-execution", **{**selection, "source": {**source, "block": "internal"}})
    grant(private, denied="Denied by workspace flow policy")
    reader = grant(ref)
    client = Mcp({"command": str(ROOT / "target/debug/an"), "args": ["mcp", *role],
                  "env": {**primary.env, "AN_TURN": "", "AN_AGENT_TOKEN": "",
                          "AN_PROVENANCE_TOKEN": reader["token"]}})
    try:
        def read():
            return client.tool("an_provenance", {"action": "read-execution", "payload": ref})

        before = read()
        assert before == {"workspace": before["workspace"], "task": worker.task, "cursor": state["cursor"],
                          "revision": state["revision"], "node": "inspect", "attempt": attempt["id"],
                          "status": "failed", "source": source, "text": receipt_text}
        worker.submit("inspect-again", [delivery("later")], retry=["inspect"])
        assert worker.graph("state")["activity"]["inspect"]["id"] != before["attempt"]
        assert read() == before
        primary.provenance("freeze-execution", token=reader["token"], **selection, denied="Controller required")
        primary.provenance("commit", token=reader["token"], idempotency_key="mutate-source",
                           versions=[version("schedule", [content("proposal", "changed")], expected=1)],
                           denied="outside authority")
        primary.provenance("read-execution", token=reader["token"], **private, denied="outside authority")
        primary.provenance("read-execution", token=reader["token"], **ref, text="altered", denied="unknown field")
        primary.provenance("history", token=reader["token"], cursor=state["cursor"], denied="outside authority")
        worker.provenance("commit", idempotency_key="inspection-report", versions=[
            version("inspection-report", [content("quotation", before["text"], source=before["source"])])
        ])
        worker.provenance("revoke", grant=reader["grant"])
        primary.provenance("read-execution", token=reader["token"], **ref, denied="Inactive delegation")
    finally:
        client.close()
