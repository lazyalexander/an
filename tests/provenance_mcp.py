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
                [binary, *args],
                env=env,
                text=True,
                input=json.dumps(body or {}),
                stderr=subprocess.PIPE,
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
        check_conditional_rejection(provenance, binary, env, task)
        cli("graph", "--task", task, "finish")
        print(
            "Verified MCP identity, projected edges, report-bound rejection, replay and revocation."
        )


def check_conditional_rejection(provenance, binary, env, task):
    target = {"artifact": "choice", "version": 2}
    report = {"artifact": "review", "version": 2, "block": "finding"}
    checked = {**target, "block": "conclusion"}
    provenance(
        "commit",
        idempotency_key="second-review",
        versions=[
            {
                "artifact": "choice",
                "expected": 1,
                "kind": "decision",
                "blocks": [
                    {"id": "conclusion", "text": "new proposal", "original": True}
                ],
            },
            {
                "artifact": "review",
                "expected": 0,
                "kind": "report",
                "blocks": [
                    {"id": "finding", "text": "pending review", "original": True}
                ],
            },
        ],
    )

    def session(grant):
        return Mcp(
            {
                "command": binary,
                "args": ["mcp", "--task", task],
                "env": {**env, "AN_TURN": "", "AN_PROVENANCE_TOKEN": grant["token"]},
            }
        )

    reviewer = provenance(
        "grant",
        target=task,
        rights=[{"right": "revise", "artifact": "review"}, {"right": "read", **target}],
    )
    client = session(reviewer)
    try:
        client.tool(
            "an_provenance",
            {
                "action": "commit",
                "payload": {
                    "idempotency_key": "signed-finding",
                    "versions": [
                        {
                            "artifact": "review",
                            "expected": 1,
                            "kind": "report",
                            "blocks": [
                                {
                                    "id": "finding",
                                    "text": "evidence missing",
                                    "cites": [
                                        {"relation": "verifies", "source": checked}
                                    ],
                                }
                            ],
                        }
                    ],
                },
            },
        )
        signed = provenance("sources", **report)["blocks"][0]
        assert signed["registered_by"] == "agent:" + reviewer["grant"]
        try:
            client.tool(
                "an_provenance",
                {
                    "action": "decide",
                    "payload": {
                        **target,
                        "review": report,
                        "choice": "reject",
                        "seen_cursor": 4,
                        "idempotency_key": "review-only",
                    },
                },
            )
        except RuntimeError as error:
            assert "outside authority" in str(error), error
        else:
            raise AssertionError("reviewer could reject with review-only rights")
    finally:
        client.close()

    right = {"right": "reject_with_report", **target, "report": report}
    try:
        provenance("grant", target=task, rights=[{**right, "version": 1}])
    except subprocess.CalledProcessError as error:
        assert "outside authority" in error.stderr, error.stderr
    else:
        raise AssertionError("unverified target version was granted")
    grant = provenance("grant", target=task, rights=[right])
    client = session(grant)

    def decide(**overrides):
        payload = {
            **target,
            "seen_cursor": 4,
            "idempotency_key": "bounded-rejection",
            "choice": "reject",
            "review": report,
            **overrides,
        }
        return client.tool("an_provenance", {"action": "decide", "payload": payload})

    try:
        for change, reason in [
            ({"review": None}, "outside authority"),
            ({"review": {**report, "version": 1}}, "outside authority"),
            ({"choice": "accept"}, "must reject"),
            ({"version": 1}, "outside authority"),
        ]:
            try:
                decide(**change)
            except RuntimeError as error:
                assert reason in str(error), error
            else:
                raise AssertionError(change)
        receipt = decide()
        assert receipt == decide() and receipt["review"] == report
        assert receipt["grant"] == grant["grant"] and receipt["choice"] == "reject"
        assert receipt["delegated_by"] == f"controller:{task}"
        provenance("revoke", grant=grant["grant"])
        historic = provenance("sources", **checked)["blocks"][0]["decision"]
        assert historic == receipt
    finally:
        client.close()


if __name__ == "__main__":
    if not __debug__:
        raise SystemExit("Run with assertions enabled")
    check()
