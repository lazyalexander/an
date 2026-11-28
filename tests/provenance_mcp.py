"""Exercise an explicitly delegated MCP session against the real binary."""

import json
import os
from pathlib import Path
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
from mcp_client import Mcp


def check():
    binary = str(ROOT / "target/debug/an")
    with tempfile.TemporaryDirectory() as folder:
        env = {**os.environ, "AN_HOME": folder, "AN_TURN": ""}
        env.pop("AN_PROVENANCE_TOKEN", None)

        def cli(*args, body=None):
            result = subprocess.check_output(
                [binary, *args], env=env, text=True, input=json.dumps(body or {})
            )
            return json.loads(result)

        task = cli("task", "new", "--model", "local", "--model-calls", "0")["task"]
        env["AN_TURN"] = cli("permission", "begin", "--task", task)["turn"]

        def provenance(action, **args):
            return cli("provenance", "--task", task, action, body=args)

        provenance(
            "commit",
            idempotency_key="propose",
            versions=[
                {
                    "artifact": "private",
                    "expected": 0,
                    "kind": "document",
                    "blocks": [
                        {"id": "text", "text": "private-data", "original": True}
                    ],
                },
                {
                    "artifact": "choice",
                    "expected": 0,
                    "kind": "decision",
                    "blocks": [
                        {
                            "id": "conclusion",
                            "text": "public",
                            "cites": [
                                {
                                    "relation": "based_on",
                                    "source": {
                                        "artifact": "private",
                                        "version": 1,
                                        "block": "text",
                                    },
                                }
                            ],
                        }
                    ],
                },
            ],
        )
        grant = provenance(
            "grant",
            target=task,
            rights=[
                {"right": "read", "artifact": "choice", "version": 1},
                {"right": "decide", "artifact": "choice", "version": 1},
            ],
        )
        bound = {
            **env,
            "AN_TURN": "",
            "AN_AGENT_TOKEN": "",
            "AN_PROVENANCE_TOKEN": grant["token"],
        }
        client = Mcp({"command": binary, "args": ["mcp", "--task", task], "env": bound})
        try:
            assert client.tools() == {"an_provenance"}

            def call(action, **payload):
                return client.tool(
                    "an_provenance", {"action": action, "payload": payload}
                )

            result = call("sources", artifact="choice", version=1)
            assert (
                len(result["blocks"]) == 1
                and not result["edges"]
                and "private" not in json.dumps(result)
            )
            receipt = call(
                "decide",
                artifact="choice",
                version=1,
                seen_cursor=1,
                choice="reject",
                idempotency_key="reject",
                reported_participant="reported-reviewer",
            )
            assert (
                receipt["grant"] == grant["grant"]
                and receipt["actor"] != "reported-reviewer"
            )
            provenance("revoke", grant=grant["grant"])
            try:
                call("sources", artifact="choice", version=1)
            except RuntimeError as error:
                assert "Inactive delegation" in str(error)
            else:
                raise AssertionError("live MCP session retained revoked authority")
        finally:
            client.close()
        cli("graph", "--task", task, "finish")
        print(
            "Verified MCP identity, projected edges, delegated rejection and live revocation."
        )


if __name__ == "__main__":
    if not __debug__:
        raise SystemExit("Run with assertions enabled")
    check()
