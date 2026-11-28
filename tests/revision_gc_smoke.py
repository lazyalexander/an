"""Finish a real task, resume its graph delivery, and verify the exported bytes."""

import hashlib
import json
import os
import sqlite3
import subprocess
import sys
import tempfile
import time
from pathlib import Path


def exercise(home, image):
    env = {**os.environ, "AN_HOME": str(home / "registry")}
    env.pop("AN_TURN", None)
    env.pop("AN_AGENT_TOKEN", None)

    def an(*args, body=None):
        output = subprocess.check_output(
            ["target/debug/an", *args], env=env, text=True, input=json.dumps(body or {})
        ).strip()
        return json.loads(output) if output.startswith(("{", "[", '"')) else output

    spool = an("register", image)
    workspace = an("workspace", "create", "--spool", spool)["workspace"]
    selector = ["--harness", "gc-smoke", "--session", "one"]
    an("workspace", "bind", workspace, *selector)
    source = home / "kept.md"
    source.write_bytes(b"# Kept\r\n")
    task = an("task", "new", *selector, "--input", source)["task"]
    env["AN_TURN"] = an("permission", "begin", "--task", task)["turn"]
    instance = an("load", "--task", task, "--spool", spool)["id"]
    try:
        run = an(
            "run",
            "--task",
            task,
            "--instance",
            instance,
            "--output",
            "scratch",
            "--",
            "/bin/sh",
            "-ec",
            "head -c 1048576 /dev/zero >/output/scratch",
        )
        deadline = time.monotonic() + 60
        while time.monotonic() < deadline:
            status = an("status", "--task", task, run)["status"]
            if status != "running":
                assert status == "awaiting_publication", status
                break
            time.sleep(0.2)
        else:
            raise TimeoutError(run)
        destination = home / "export"
        origin = "task-input:kept.md"
        policy = home / "policy.json"
        policy.write_text(
            json.dumps(
                [["publish", origin, workspace], ["export", origin, str(destination)]]
            )
        )
        an("workspace", "policy", "set", workspace, "--file", policy)
        node = {
            "id": "result",
            "kind": "delivery",
            "inputs": {"doc": {"input": "kept.md"}},
            "outputs": [],
        }
        an(
            "graph",
            "--task",
            task,
            "submit",
            body={"base_revision": 0, "idempotency_key": "publish", "nodes": [node]},
        )

        def result():
            return an("graph", "--task", task, "state")["activity"]["result"]

        request = result()["requests"]["doc"]
        an(
            "permission",
            "decide",
            "--task",
            task,
            "--request-id",
            request,
            "--decision",
            "once",
        )
        delivery = result()["delivery"]
        with sqlite3.connect(home / "registry/registry.sqlite3") as db:

            def object_row(kind, key):
                row = db.execute(
                    "SELECT body FROM objects WHERE kind=? AND id=?", (kind, key)
                ).fetchone()
                return json.loads(row[0]) if row else None

            def reusable_bytes():
                page_size = db.execute("PRAGMA page_size").fetchone()[0]
                free_pages = db.execute("PRAGMA freelist_count").fetchone()[0]
                return page_size * free_pages

            free_before = reusable_bytes()
            candidate = object_row("run", run)["candidate"]
            assert object_row("revision", candidate)
            finish = an("graph", "--task", task, "finish")
            assert finish == {"closed": True, "cleanup_pending": []}, finish
            assert object_row("revision", candidate) is None
            metrics = object_row("revision_gc", task)
            assert metrics["payload_bytes_reclaimed"] >= 1048576
            metrics.update(
                reusable_bytes_before=free_before, reusable_bytes_after=reusable_bytes()
            )
    finally:
        an("unload", "--task", task, "--instance", instance)
    env.pop("AN_TURN")
    resumed = an("task", "resume", delivery)["task"]
    env["AN_TURN"] = an("permission", "begin", "--task", resumed)["turn"]
    lineage = an(
        "provenance", "--task", resumed, "sources",
        body={"delivery": delivery, "binding": "doc"},
    )
    assert lineage["precision"] == "file" and "doc" in lineage["bindings"]
    args = ["export", delivery, destination, "--task", resumed]
    request = an(*args)["request_id"]
    an(
        "permission",
        "decide",
        "--task",
        resumed,
        "--request-id",
        request,
        "--decision",
        "once",
    )
    an(*args, "--request-id", request)
    exported = (destination / "doc/kept.md").read_bytes()
    assert exported == source.read_bytes()
    return {**metrics, "export_sha256": hashlib.sha256(exported).hexdigest()}


if __name__ == "__main__":
    with tempfile.TemporaryDirectory(dir="/private/tmp") as directory:
        print(json.dumps(exercise(Path(directory), sys.argv[1])))
