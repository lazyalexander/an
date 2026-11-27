"""Verify storage limits and reuse with an isolated registry and a local spool."""
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import sys
import tempfile
import time


def exercise(home, reference):
    env = dict(os.environ, AN_HOME=str(home))
    for key in ("AN_TURN", "AN_AGENT_TOKEN"):
        env.pop(key, None)
    (home / "config.json").write_text(json.dumps({
        "instance_disk_mib": 64, "run_output_bytes": 8, "run_log_bytes": 1024,
    }))

    def cli(*args):
        return subprocess.check_output(["target/debug/an", *args], env=env, text=True).strip()

    spool = cli("register", reference)
    task = json.loads(cli("task", "new", "--spool", spool))["task"]

    def op(name, *args):
        return json.loads(cli(name, "--task", task, *args))

    instance = op("load", "--spool", spool)["id"]

    def run(script, *outputs):
        flags = [arg for name in outputs for arg in ("--output", name)]
        run_id = op("run", "--instance", instance, *flags, "--", "/bin/sh", "-ec", script)
        deadline = time.monotonic() + 90
        while time.monotonic() < deadline:
            if op("status", run_id)["status"] != "running":
                with sqlite3.connect(home / "registry.sqlite3") as db:
                    row = db.execute("SELECT body FROM objects WHERE kind='run' AND id=?", (run_id,)).fetchone()
                return json.loads(row[0])
            time.sleep(.2)
        raise TimeoutError(run_id)

    def failed(row, reason):
        assert row["status"] == "failed" and "candidate" not in row, row
        assert reason in row["error"], row

    directory = home / "instances" / instance
    try:
        assert (directory / "data").is_mount()
        failed(run("printf 12345 >/output/a; printf 6789 >/output/b", "a", "b"), "output budget")
        log_run = run("head -c 8192 /dev/zero")
        failed(log_run, "log budget")
        assert (home / "runs" / log_run["id"] / "log").stat().st_size == 1024
        full = run("dd if=/dev/zero of=/output/scratch bs=1M count=96")
        failed(full, "Guest command exited")
        assert b"No space left on device" in (home / "runs" / full["id"] / "log").read_bytes()
        assert run("printf 12345678 >/output/result", "result")["status"] == "awaiting_publication"
    finally:
        assert op("unload", "--instance", instance)["status"] == "unloaded"
    assert not directory.exists()


if __name__ == "__main__":
    with tempfile.TemporaryDirectory(dir="/private/tmp") as directory:
        exercise(Path(directory), sys.argv[1])
    print("Disk exhaustion, aggregate output, log limit, recovery and detach passed")
