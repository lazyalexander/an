"""Exercise delegated registration of external decisions through a real MCP session."""

import json
import os
from pathlib import Path
import subprocess
import tempfile

from mcp_client import Mcp

BINARY = str(Path(__file__).resolve().parents[1] / "target/debug/an")


def exercise(directory):
    env = {**os.environ, "AN_HOME": directory, "AN_TURN": ""}
    for name in ("AN_PROVENANCE_TOKEN", "AN_AGENT_TOKEN"):
        env.pop(name, None)

    def cli(*args, payload=None):
        out = subprocess.check_output(
            [BINARY, *args], env=env, text=True, input=json.dumps(payload or {})
        )
        return json.loads(out)

    task = cli("task", "new", "--model", "local", "--model-calls", "0")["task"]
    env["AN_TURN"] = cli("permission", "begin", "--task", task)["turn"]

    def host(action, **payload):
        return cli("provenance", "--task", task, action, payload=payload)

    statements = {
        "approved": "Lin approved Friday's release.",
        "suggested": "Lin suggested Friday's release.",
        "authority": "Lin owns the release schedule.",
    }
    host(
        "commit",
        idempotency_key="sources",
        versions=[
            *[
                {
                    "artifact": key,
                    "expected": 0,
                    "kind": "document",
                    "blocks": [{"id": "statement", "text": text, "original": True}],
                }
                for key, text in statements.items()
            ],
            {
                "artifact": "release",
                "expected": 0,
                "kind": "decision",
                "blocks": [
                    {"id": "entry", "text": "Pending registration", "original": True}
                ],
            },
        ],
    )
    rights = [
        {"right": "register", "artifact": "release"},
        *[{"right": "read", "artifact": key, "version": 1} for key in statements],
    ]
    grant = host("grant", target=task, rights=rights)
    client = Mcp(
        {
            "command": BINARY,
            "args": ["mcp", "--task", task],
            "env": {**env, "AN_TURN": "", "AN_PROVENANCE_TOKEN": grant["token"]},
        }
    )

    def register(expected, outcome, authority, text=None):
        statement = {"artifact": outcome, "version": 1, "block": "statement"}
        cites = [{"source": statement, "relation": "quotes"}]
        basis = {**statement, "artifact": "authority"} if authority else None
        if basis:
            cites.append({"source": basis, "relation": "based_on"})
        payload = {
            "idempotency_key": f"entry-{expected}",
            "versions": [
                {
                    "artifact": "release",
                    "expected": expected,
                    "kind": "decision",
                    "attribution": {
                        "outcome": outcome,
                        "speaker": "Lin",
                        "source_time": "09:30",
                        "statement": statement,
                        "authority": basis,
                    },
                    "blocks": [
                        {
                            "id": "entry",
                            "text": text or statements[outcome],
                            "cites": cites,
                        }
                    ],
                }
            ],
        }
        return client.tool("an_provenance", {"action": "commit", "payload": payload})

    try:
        try:
            register(1, "approved", True, text="Different statement")
        except RuntimeError as error:
            assert "Quoted bytes differ" in str(error), error
        else:
            raise AssertionError("accepted altered quotation")
        for expected, (outcome, basis, state) in enumerate(
            [
                ("approved", True, "recorded_approval"),
                ("suggested", True, "proposed"),
                ("approved", False, "unverified"),
            ],
            start=1,
        ):
            receipt = register(expected, outcome, basis)
            assert register(expected, outcome, basis) == receipt
            trace = host(
                "sources", artifact="release", version=expected + 1, block="entry"
            )
            entry = trace["blocks"][0]
            assert entry["registered_by"] == "agent:" + grant["grant"]
            record = entry["attribution"]
            assert record["state"] == state and record["registered_at"] > 0
            assert (
                record["source"]["speaker"] == "Lin"
                and record["source"]["source_time"] == "09:30"
            )
            assert "decision" not in entry
            assert trace["blocks"][0]["content"]["text"] == statements[outcome]
        cursor = cli("graph", "--task", task, "state")["cursor"]
        assert all(
            d["state"] == "proposed"
            for d in host("history", cursor=cursor)["decisions"]
        )
    finally:
        client.close()
    cli("graph", "--task", task, "finish")


if __name__ == "__main__":
    if not __debug__:
        raise SystemExit("Run with assertions enabled")
    with tempfile.TemporaryDirectory() as directory:
        exercise(directory)
    print(
        "Verified recorded approval, suggestion, missing authority and exact source quotation."
    )
