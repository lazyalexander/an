"""Prepare or verify a persistent test case across an operator-initiated Mac reboot."""

import argparse
import fcntl
import hashlib
import json
import os
import subprocess
import tempfile
from pathlib import Path
from uuid import UUID

from apple_reliability import Host, candidate_bytes, until


def boot_session():
    value = subprocess.check_output(
        ["/usr/sbin/sysctl", "-n", "kern.bootsessionuuid"], text=True
    )
    return str(UUID(value.strip()))


def checkpoint(path, data):
    # The operator can interrupt power once prepare returns. Commit the fixture
    # receipt separately from An's database, including the directory entry.
    temp = path.with_suffix(".tmp")
    with temp.open("w") as file:
        json.dump(data, file)
        file.flush()
        os.fsync(file.fileno())
    temp.replace(path)
    parent = os.open(path.parent, os.O_RDONLY)
    try:
        os.fsync(parent)
    finally:
        os.close(parent)
    # Flush the device cache after the rename and directory sync.
    with path.open("rb") as committed:
        fcntl.fcntl(committed.fileno(), fcntl.F_FULLFSYNC)


def prepare(directory, image):
    boot = boot_session()
    directory.mkdir(mode=0o700)
    host = Host.create(directory, image, slots=1)
    turn = None
    try:
        task, follower = host.new_task(), host.new_task()
        cleanup_case = {"task": task, "image": host.image}
        checkpoint(directory / "cleanup.json", cleanup_case)
        turn = host.cli("permission", "begin", "--task", task)["turn"]
        # The isolated case directory is private to the operator. Keep this
        # credential separate from the report and outside every guest mount.
        credential = directory / "operator"
        fd = os.open(credential, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
        with os.fdopen(fd, "w") as file:
            file.write(turn)
            file.flush()
            fcntl.fcntl(file.fileno(), fcntl.F_FULLFSYNC)
        payload = os.urandom(32).hex() + "\r\n"
        published = host.run(task, 'printf %s "$1" > /output/result', payload)
        host.wait(task, published, "awaiting_publication")
        host.approved(task, turn, "publish", "--task", task, published)
        run = host.run(task, "touch /output/started; exec sleep 1800")
        output = host.home / "instances" / host.instances[task]["id"] / "data/output"
        until(
            lambda: (
                (output / "started").is_file()
                and host.record("run", run).get("started_at")
            ),
            "guest active before reboot",
        )
        queued = host.run(follower, "printf queued > /output/result")
        assert (
            host.cli("status", "--task", follower, queued)["waiting"]["position"] == 1
        )
        assert host.rows(
            "SELECT run, admitted FROM execution_queue ORDER BY ticket"
        ) == [(run, 1), (queued, 0)]
        checkpoint(
            directory / "prepared.json",
            {
                "boot": boot,
                "image": host.image,
                "task": task,
                "run": run,
                "follower": follower,
                "queued": queued,
                "published": published,
                "started_at": host.record("run", run)["started_at"],
                "expected_sha256": hashlib.sha256(payload.encode()).hexdigest(),
            },
        )
    except BaseException:
        try:
            if turn is not None:
                host.cli("permission", "end", "--task", task, turn=turn)
                (directory / "operator").unlink(missing_ok=True)
        finally:
            host.cleanup()
        raise
    return {
        "prepared": True,
        "directory": str(directory),
        "interrupt_within_minutes": 30,
    }


def check_recovery(directory, case, boot):
    host = Host(directory, case["image"])
    task, follower = case["task"], case["follower"]
    observed = directory / "observed.json"
    if not observed.exists():
        assert host.rows("PRAGMA integrity_check", recover=True) == [("ok",)]
        assert {
            row[0] for row in host.rows("SELECT id FROM objects WHERE kind='run'")
        } == {
            case["published"],
            case["run"],
            case["queued"],
        }
        assert host.rows(
            "SELECT run, admitted FROM execution_queue ORDER BY ticket"
        ) == [(case["run"], 1), (case["queued"], 0)]
        for owner, run in ((task, case["run"]), (follower, case["queued"])):
            assert host.cli("status", "--task", owner, run)["status"] == "unknown"
            assert not host.record("run", run).get("candidate")
        assert host.record("run", case["run"])["started_at"] == case["started_at"]
        assert not host.record("run", case["queued"]).get("started_at")
        checkpoint(observed, {"boot": boot, "running": "unknown", "queued": "unknown"})
    elif json.loads(observed.read_text())["boot"] != boot:
        raise ValueError("The machine rebooted again during verification")

    for owner, run in ((task, case["run"]), (follower, case["queued"])):
        assert host.cli("cancel", "--task", owner, run)["status"] == "cancelled"
    host.cleanup()
    turn = (directory / "operator").read_text()
    # Export requires a new directory, including when retrying a partial check.
    destination = (
        Path(tempfile.mkdtemp(dir=directory, prefix="verification-")) / "export"
    )
    host.approved(task, turn, "export", case["published"], destination)
    digest = hashlib.sha256((destination / "result").read_bytes()).hexdigest()
    assert digest == case["expected_sha256"]
    # Use fresh work to prove that both loaded-instance and execution capacity recovered.
    try:
        replacement = host.new_task()
        fresh = host.run(replacement, "printf resumed > /output/result")
        host.wait(replacement, fresh, "awaiting_publication")
        assert candidate_bytes(host, fresh) == b"resumed"
    finally:
        host.cleanup()
    return {
        "export_sha256": digest,
        "replacement_completed": True,
        "cleanup": "confirmed",
    }


def end_turn(host, directory, case):
    credential = directory / "operator"
    if credential.exists():
        if host.record("turn", case["task"]) is not None:
            host.cli(
                "permission", "end", "--task", case["task"], turn=credential.read_text()
            )
        credential.unlink()


def verify(directory):
    case = json.loads((directory / "prepared.json").read_text())
    boot = boot_session()
    if boot == case["boot"]:
        raise ValueError("Verification requires a different macOS boot session")
    receipt = directory / "verified.json"
    if receipt.exists():
        report = json.loads(receipt.read_text())
    else:
        report = {
            "before_boot": case["boot"],
            "after_boot": boot,
            **check_recovery(directory, case, boot),
        }
        checkpoint(receipt, report)
    end_turn(Host(directory, case["image"]), directory, case)
    return report


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    actions = parser.add_subparsers(dest="action", required=True)
    setup = actions.add_parser("prepare")
    setup.add_argument("directory", type=Path)
    setup.add_argument("image")
    for name in ("verify", "cleanup"):
        actions.add_parser(name).add_argument("directory", type=Path)
    args = parser.parse_args()
    directory = args.directory.resolve()
    if args.action == "prepare":
        result = prepare(directory, args.image)
    elif args.action == "verify":
        result = verify(directory)
    else:
        metadata = directory / "cleanup.json"
        if not metadata.exists():
            metadata = directory / "prepared.json"
        case = json.loads(metadata.read_text())
        host = Host(directory, case["image"])
        try:
            end_turn(host, directory, case)
        finally:
            host.cleanup()
        result = {"cleanup": "confirmed"}
    print(json.dumps(result))
