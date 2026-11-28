"""Exercise the shipped scheduler on a Mac with a cached, admitted-format image."""

import argparse
import base64
import hashlib
import json
import os
import shutil
import signal
import sqlite3
import subprocess
import tempfile
import time
from concurrent.futures import ThreadPoolExecutor
from contextlib import closing
from pathlib import Path

if not __debug__:
    raise RuntimeError(
        "Acceptance checks require Python assertions; disable optimization"
    )

AN = Path("target/debug/an").resolve()
APPLE = "/usr/local/bin/container"


class Host:
    def __init__(self, directory, image):
        self.home = directory / "registry"
        self.image = image
        self.instances = {}
        self.env = {
            k: v
            for k, v in os.environ.items()
            if k not in {"AN_TURN", "AN_AGENT_TOKEN"}
        }
        self.env["AN_HOME"] = str(self.home)

    @classmethod
    def create(cls, directory, image, slots):
        host = cls(directory, image)
        host.home.mkdir(mode=0o700)
        (host.home / "config.json").write_text(
            json.dumps(
                {
                    "max_concurrent_runs": slots,
                    "max_loaded_instances": 4,
                    "container_cpus": 1,
                    "container_memory_mib": 512,
                    "instance_disk_mib": 64,
                }
            )
        )
        host.image = host.cli("register", image)
        return host

    def cli(self, *args, turn=None):
        env = self.env if turn is None else {**self.env, "AN_TURN": turn}
        result = subprocess.check_output(
            [AN, *args], env=env, input="{}", text=True, timeout=120
        ).strip()
        return json.loads(result) if result.startswith(("{", "[", '"')) else result

    def approved(self, task, turn, *args):
        request = self.cli(*args, turn=turn)["request_id"]
        self.cli(
            "permission",
            "decide",
            "--task",
            task,
            "--request-id",
            request,
            "--decision",
            "once",
            turn=turn,
        )
        return self.cli(*args, "--request-id", request, turn=turn)

    def new_task(self):
        task = self.cli("task", "new", "--spool", self.image)["task"]
        self.instances[task] = self.cli("load", "--task", task, "--spool", self.image)
        return task

    def run(self, task, script, *args):
        return self.cli(
            "run",
            "--task",
            task,
            "--instance",
            self.instances[task]["id"],
            "--output",
            "result",
            "--",
            "/bin/sh",
            "-ec",
            script,
            "sh",
            *args,
        )

    def wait(self, task, run, expected):
        deadline = time.monotonic() + 90
        while time.monotonic() < deadline:
            state = self.cli("status", "--task", task, run)["status"]
            if state == expected:
                return
            if state not in ("running", "cancel_pending"):
                raise AssertionError((run, state, expected))
            time.sleep(0.2)
        raise TimeoutError((run, expected))

    def rows(self, sql, params=(), *, recover=False):
        database = (self.home / "registry.sqlite3").as_uri()
        mode = "rw" if recover else "ro"
        with closing(sqlite3.connect(f"{database}?mode={mode}", uri=True)) as db:
            return db.execute(sql, params).fetchall()

    def record(self, kind, key):
        [(body,)] = self.rows(
            "SELECT body FROM objects WHERE kind=? AND id=?", (kind, key)
        )
        return json.loads(body)

    def cleanup(self):
        errors = []
        # A load can record an instance before it fails to return its ID.
        instances = [
            json.loads(body)
            for (body,) in self.rows("SELECT body FROM objects WHERE kind='instance'")
        ]
        for instance in instances:
            try:
                if instance["status"] != "unloaded":
                    result = self.cli(
                        "unload",
                        "--task",
                        instance["task"],
                        "--instance",
                        instance["id"],
                    )
                    assert result["status"] == "unloaded", result
                assert not (self.home / "instances" / instance["id"]).exists()
            except (
                AssertionError,
                OSError,
                subprocess.SubprocessError,
                ValueError,
            ) as exc:
                errors.append(str(exc))
        assert not errors, (self.home, errors)
        inventory = json.loads(
            subprocess.check_output(
                [APPLE, "list", "--all", "--format", "json"], timeout=45
            )
        )
        names = {row["id"] for row in inventory}
        assert all(i["name"] not in names for i in instances)
        if self.rows("SELECT 1 FROM sqlite_schema WHERE name='execution_queue'"):
            assert self.rows("SELECT count(*) FROM execution_queue") == [(0,)]


def pressure(host, seconds):
    tasks = [host.new_task() for _ in range(4)]
    start = time.monotonic()
    rounds = 0
    peak = 0

    def execute(task, round_number):
        payload = f"{round_number}/{task}"
        run = host.run(task, 'sleep 1; printf %s "$1" >/output/result', payload)
        host.wait(task, run, "awaiting_publication")
        record = host.record("run", run)
        files = host.record("revision", record["candidate"])
        assert base64.b64decode(files["result"], validate=True) == payload.encode()
        return record["started_at"], record["exited_at"]

    with ThreadPoolExecutor(len(tasks)) as pool:
        while time.monotonic() - start < seconds:
            # The durable execution intervals cover even peaks between polls.
            events = []
            for entered, exited in pool.map(execute, tasks, [rounds] * len(tasks)):
                assert entered < exited
                events.extend([(entered, 1), (exited, -1)])
            active = 0
            for _, delta in sorted(events):
                active += delta
                assert 0 <= active <= 2
                peak = max(peak, active)
            assert host.rows("SELECT count(*) FROM execution_queue") == [(0,)]
            rounds += 1
    assert peak == 2, "The workload did not exercise both execution slots"
    return {
        "mode": "pressure",
        "tasks": len(tasks),
        "rounds": rounds,
        "runs_verified": rounds * len(tasks),
        "peak_concurrent_guest_commands": peak,
        "workload_seconds": round(time.monotonic() - start, 2),
    }


def until(predicate, description):
    deadline = time.monotonic() + 60
    while time.monotonic() < deadline:
        if predicate():
            return
        time.sleep(0.1)
    raise TimeoutError(description)


def candidate_bytes(host, run):
    revision = host.record("run", run)["candidate"]
    return base64.b64decode(host.record("revision", revision)["result"], validate=True)


def memory_exhaustion(host, owner, follower, turn):
    probe = Path(__file__).with_name("fixtures") / "guest_memory_pressure.py"
    victim = host.run(owner, 'exec python3 -c "$1"', probe.read_text())
    instance = host.instances[owner]
    assert instance["resources"]["memory_bytes"] == 512 * 1024**2
    output = host.home / "instances" / instance["id"] / "data/output"
    until((output / "ready").is_file, "memory probe ready")
    queued = host.run(follower, "printf recovered >/output/result")
    assert host.cli("status", "--task", follower, queued)["waiting"] == {
        "reason": "execution_capacity",
        "position": 1,
    }
    (output / "proceed").touch(exist_ok=False)
    host.wait(owner, victim, "failed")
    record = host.record("run", victim)
    assert record["exit_code"] == 137 and not record.get("candidate"), record
    log = host.approved(owner, turn, "log", "--task", owner, victim)
    evidence = json.loads(log.splitlines()[0])
    assert evidence["child_returncode"] == -signal.SIGKILL, evidence
    assert evidence["after"]["oom"] > evidence["before"]["oom"], evidence
    assert evidence["after"]["oom_kill"] > evidence["before"]["oom_kill"], evidence
    assert evidence["limit_bytes"] == instance["resources"]["memory_bytes"]
    host.wait(follower, queued, "awaiting_publication")
    assert candidate_bytes(host, queued) == b"recovered"
    retry = host.run(owner, "printf reused >/output/result")
    host.wait(owner, retry, "awaiting_publication")
    assert candidate_bytes(host, retry) == b"reused"
    assert host.rows("SELECT count(*) FROM execution_queue") == [(0,)]
    return {
        **evidence,
        "run_exit_code": record["exit_code"],
        "queued_run_completed": True,
        "instance_reused": True,
    }


def recovery(host, mode):
    owner = host.new_task()
    follower = host.new_task()
    turn = host.cli("permission", "begin", "--task", owner)["turn"]
    payload = "published before interruption\r\n"
    published = host.run(owner, 'printf %s "$1" >/output/result', payload)
    host.wait(owner, published, "awaiting_publication")
    host.approved(owner, turn, "publish", "--task", owner, published)

    # Only interrupt the locally created run, after its guest has actually written.
    instance = host.instances[owner]
    victim = host.run(owner, "touch /output/started; exec sleep 120")
    marker = host.home / "instances" / instance["id"] / "data/output/started"
    until(
        lambda: marker.is_file() and host.record("run", victim).get("started_at"),
        "guest start marker and execution timestamp",
    )
    started = host.record("run", victim)
    queued = host.run(follower, "printf next >/output/result")

    def waiting():
        return host.cli("status", "--task", follower, queued).get("waiting")

    assert waiting() == {"reason": "execution_capacity", "position": 1}
    if mode == "worker-loss":
        pid = started["worker_pid"]
        command = subprocess.check_output(
            ["ps", "-ww", "-p", str(pid), "-o", "command="], text=True
        ).strip()
        invocation, descriptor = command.rsplit(" ", 1)
        assert invocation == f"{AN} _worker {victim}" and descriptor.isdecimal(), (
            command
        )
        os.kill(pid, signal.SIGKILL)
        host.wait(owner, victim, "unknown")
    else:
        assert instance["name"] == f"an-{instance['id']}"
        subprocess.run(
            [APPLE, "delete", "--force", instance["name"]],
            check=True,
            timeout=60,
            stdout=subprocess.DEVNULL,
        )
        # Do not trigger reconciliation while checking that the reservation survived the fault.
        until(
            lambda: host.record("run", victim)["status"] == "unknown",
            "worker recorded guest loss",
        )
    assert waiting() == {"reason": "execution_capacity", "position": 1}
    assert host.rows("SELECT admitted FROM execution_queue WHERE run=?", (victim,)) == [
        (1,)
    ]
    assert host.cli("status", "--task", owner, victim)["status"] == "unknown"
    assert host.cli("cancel", "--task", owner, victim)["status"] == "cancelled"
    assert host.cli("cancel", "--task", owner, victim)["status"] == "cancelled"
    assert host.record("instance", instance["id"])["observed"] in (
        "stopped",
        "absent",
        "exited",
        "created",
    )
    assert (
        host.rows("SELECT admitted FROM execution_queue WHERE run=?", (victim,)) == []
    )
    settled = host.record("run", victim)
    assert not settled.get("candidate")
    assert settled["started_at"] == started["started_at"]
    assert settled["worker_pid"] == started["worker_pid"]
    assert host.rows(
        "SELECT id FROM objects WHERE kind='run' AND json_extract(body, '$.task')=? ORDER BY id",
        (owner,),
    ) == [(run,) for run in sorted((published, victim))]
    host.wait(follower, queued, "awaiting_publication")
    assert candidate_bytes(host, queued) == b"next"
    destination = host.home.parent / "export"
    host.approved(owner, turn, "export", published, destination)
    exported = (destination / "result").read_bytes()
    assert exported == payload.encode()
    assert host.cli("graph", "--task", owner, "finish", turn=turn) == {
        "closed": True,
        "cleanup_pending": [],
    }
    return {
        "mode": mode,
        "cancelled": True,
        "queued_run_completed": True,
        "export_sha256": hashlib.sha256(exported).hexdigest(),
    }


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("image")
    parser.add_argument(
        "--mode",
        choices=("pressure", "worker-loss", "guest-loss", "oom"),
        default="pressure",
    )
    parser.add_argument("--seconds", type=int, default=60)
    options = parser.parse_args()
    if options.seconds <= 0:
        parser.error("--seconds must be positive")
    directory = Path(tempfile.mkdtemp(prefix="an-reliability-", dir="/private/tmp"))
    host = Host.create(
        directory, options.image, slots=2 if options.mode == "pressure" else 1
    )
    try:
        if options.mode == "oom":
            owner = host.new_task()
            turn = host.cli("permission", "begin", "--task", owner)["turn"]
            report = {
                "mode": "oom",
                **memory_exhaustion(host, owner, host.new_task(), turn),
            }
        elif options.mode == "pressure":
            report = pressure(host, options.seconds)
        else:
            report = recovery(host, options.mode)
    finally:
        # Preserve the registry and its mounts if unloading cannot be confirmed.
        host.cleanup()
        shutil.rmtree(directory)
    print(json.dumps({**report, "cleanup": "confirmed"}))
