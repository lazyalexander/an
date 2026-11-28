"""Restart the local CLI between outbox operations and interrupt an acknowledgement."""

import json
import os
from pathlib import Path
import sqlite3
import subprocess
import tempfile

BINARY = Path(__file__).resolve().parents[1] / "target/debug/an"


def exercise(home):
    env = {**os.environ, "AN_HOME": home, "AN_TURN": ""}
    for key in ("AN_PROVENANCE_TOKEN", "AN_AGENT_TOKEN"):
        env.pop(key, None)

    def cli(*args, payload=None, token=None, denied=None):
        scope = {**env, "AN_TURN": "", "AN_PROVENANCE_TOKEN": token} if token else env
        run = subprocess.run(
            [BINARY, *args],
            env=scope,
            text=True,
            capture_output=True,
            input=json.dumps(payload or {}),
            timeout=15,
        )
        if denied:
            assert run.returncode and denied in run.stderr, run.stderr
            return
        assert run.returncode == 0, run.stderr
        return json.loads(run.stdout)

    task = cli("task", "new", "--model", "local", "--model-calls", "0")["task"]
    env["AN_TURN"] = cli("permission", "begin", "--task", task)["turn"]

    def op(action, token=None, denied=None, **payload):
        return cli(
            "provenance",
            "--task",
            task,
            action,
            payload=payload,
            token=token,
            denied=denied,
        )

    names = ["draft", "second"]
    op(
        "commit",
        idempotency_key="proposals",
        versions=[
            {
                "artifact": n,
                "expected": 0,
                "kind": "decision",
                "blocks": [{"id": "text", "text": "private draft", "original": True}],
            }
            for n in names
        ],
    )

    def grant(rights):
        return op("grant", target=task, rights=rights)

    recipient = grant(
        [{"right": "receive_notice", "artifact": n, "version": 1} for n in names]
    )
    sender = grant(
        [
            r
            for n in names
            for r in [
                {"right": "decide", "artifact": n, "version": 1},
                {
                    "right": "notify",
                    "artifact": n,
                    "version": 1,
                    "recipient": recipient["grant"],
                },
            ]
        ]
    )

    def decide(name):
        return op(
            "decide",
            token=sender["token"],
            artifact=name,
            version=1,
            seen_cursor=1,
            choice="reject",
            reason="private reason",
            notify_grant=recipient["grant"],
            idempotency_key=name,
        )

    def take(notice, **args):
        return op("take-notification", token=recipient["token"], id=notice, **args)

    def ack(attempt, status, **args):
        return op(
            "ack-notification",
            token=recipient["token"],
            attempt=attempt,
            status=status,
            **args,
        )

    def history(notice):
        return next(
            n["attempts"]
            for n in op("notifications")["notifications"]
            if n["id"] == notice
        )

    receipt = decide("draft")
    notice = receipt["notification"]["id"]
    assert decide("draft") == receipt and history(notice) == []
    op(
        "take-notification",
        token=sender["token"],
        id=notice,
        denied="Notification unavailable",
    )
    first = take(notice)
    assert (
        set(first) == {"id", "attempt", "decision", "artifact", "version", "choice"}
        and first["choice"] == "reject"
    )
    ack(first["attempt"], "failed")
    second = take(notice, retry_after=first["attempt"])
    with sqlite3.connect(Path(home) / "registry.sqlite3") as db:
        db.execute(
            "CREATE TRIGGER lose_ack BEFORE UPDATE ON pv_notice_attempt BEGIN SELECT RAISE(ABORT,'receipt lost'); END"
        )
    ack(second["attempt"], "received", denied="receipt lost")
    with sqlite3.connect(Path(home) / "registry.sqlite3") as db:
        db.execute("DROP TRIGGER lose_ack")
    assert decide("draft") == receipt
    assert [a["status"] for a in history(notice)] == ["failed", "unknown"]
    take(notice, denied="Explicit retry")
    take(notice, retry_after=first["attempt"], denied="Explicit retry")
    third = take(notice, retry_after=second["attempt"])
    assert ack(third["attempt"], "received") == ack(third["attempt"], "received")
    take(notice, retry_after=third["attempt"], denied="already received")
    pending = decide("second")["notification"]["id"]
    op("revoke", grant=sender["grant"])
    take(pending, denied="Inactive delegation")
    assert history(pending) == []
    check_corrections(
        op, grant, lambda: cli("graph", "--task", task, "state")["cursor"]
    )
    cli("graph", "--task", task, "finish")


def check_corrections(op, grant, cursor):
    for original_status in ("received", "unknown", "unclaimed"):
        name = f"correction-{original_status}"
        for expected in (0, 1):
            op(
                "commit",
                idempotency_key=f"{name}-v{expected + 1}",
                versions=[
                    {
                        "artifact": name,
                        "expected": expected,
                        "kind": "decision",
                        "blocks": [
                            {
                                "id": "text",
                                "text": f"draft {expected + 1}",
                                "original": True,
                            }
                        ],
                    }
                ],
            )
        reader = grant(
            [
                {"right": "receive_notice", "artifact": name, "version": n}
                for n in (1, 2)
            ]
        )
        first = op(
            "decide",
            idempotency_key=name,
            artifact=name,
            version=1,
            seen_cursor=cursor(),
            choice="accept",
            notify_grant=reader["grant"],
        )
        old_notice = first["notification"]["id"]

        def take(notice, **kwargs):
            return op("take-notification", token=reader["token"], id=notice, **kwargs)

        def ack(attempt):
            return op(
                "ack-notification",
                token=reader["token"],
                attempt=attempt,
                status="received",
            )

        if original_status != "unclaimed":
            old_attempt = take(old_notice)["attempt"]
            if original_status == "received":
                ack(old_attempt)
        change = dict(
            idempotency_key=name + "-amend",
            artifact=name,
            version=2,
            seen_cursor=cursor(),
            choice="reject",
            corrects=first["id"],
        )
        narrow = grant([{"right": "decide", "artifact": name, "version": 2}])
        op("decide", token=narrow["token"], denied="outside authority", **change)
        assert op("corrections", decision=first["id"])["corrections"] == []
        revised = op("decide", **change)
        assert revised["corrects"] == first["id"] and op("decide", **change) == revised

        def inspect():
            before = op("corrections", decision=first["id"])
            assert before == op("corrections", decision=revised["id"])
            (link,) = before["corrections"]
            assert (link["previous"], link["replacement"]) == (
                first["id"],
                revised["id"],
            )
            (route,) = link["recipients"]
            return route

        assert inspect() == {
            "recipient": reader["grant"],
            "original": original_status,
            "correction": "not_requested",
        }
        take(old_notice, denied="Decision corrected")

        def queue():
            return op(
                "notify-correction", decision=revised["id"], recipient=reader["grant"]
            )

        notice = queue()
        assert queue() == notice and inspect()["correction"] == "unclaimed"
        delivery = take(notice["id"])
        assert delivery["corrects"] == first["id"] and delivery["choice"] == "reject"
        assert inspect()["correction"] == "unknown"
        ack(delivery["attempt"])
        assert inspect()["correction"] == "received"
        if original_status == "unknown":
            ack(old_attempt)
            assert inspect()["original"] == "received"
        op(
            "corrections",
            token=reader["token"],
            decision=first["id"],
            denied="Controller required",
        )


if __name__ == "__main__":
    if not __debug__:
        raise SystemExit("Run with assertions enabled")
    with tempfile.TemporaryDirectory() as home:
        exercise(home)
    print(
        "Verified notification recovery, revocation, decision corrections and late receipts."
    )
