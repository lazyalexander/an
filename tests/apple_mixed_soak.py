"""Cycle real Apple Container tasks in one registry. Emit progress and measurements as JSONL."""

import argparse
import json
import re
import shutil
import subprocess
import tempfile
import time
from pathlib import Path

from apple_reliability import Host, candidate_bytes, memory_exhaustion, until


def resources(host):
    # -x avoids double-counting the disk image and its mounted contents.
    kib = subprocess.check_output(["du", "-skx", host.home], text=True, timeout=30)
    stats = subprocess.check_output(["/usr/bin/vm_stat"], text=True, timeout=10)
    page_bytes = int(re.search(r"page size of (\d+) bytes", stats)[1])
    pages = {
        name: int(count)
        for name, count in re.findall(r"^([^:\n]+):\s+(\d+)\.", stats, re.MULTILINE)
    }
    return {
        "registry_allocated_kib": int(kib.split()[0]),
        "host_wired_bytes": pages["Pages wired down"] * page_bytes,
        "host_compressor_bytes": pages["Pages occupied by compressor"] * page_bytes,
    }


def cycle(host, number, inject_oom):
    owner, follower = host.new_task(), host.new_task()
    turn = host.cli("permission", "begin", "--task", owner)["turn"]
    follower_turn = host.cli("permission", "begin", "--task", follower)["turn"]
    receipt = memory_exhaustion(host, owner, follower, turn) if inject_oom else None

    # Verify that cancelling an executing run hands its slot to a publishing run.
    active = host.run(owner, "touch /output/sleeping; exec sleep 120")
    mount = host.home / "instances" / host.instances[owner]["id"] / "data"
    until((mount / "output/sleeping").is_file, "cancellable run entered guest")
    payload = f"mixed workload, cycle {number}\r\n"
    published = host.run(follower, 'printf %s "$1" > /output/result', payload)
    assert host.cli("status", "--task", follower, published)["waiting"] == {
        "reason": "execution_capacity",
        "position": 1,
    }
    loaded = resources(host)
    assert host.cli("cancel", "--task", owner, active)["status"] == "cancelled"
    host.wait(follower, published, "awaiting_publication")
    assert candidate_bytes(host, published) == payload.encode()
    host.approved(follower, follower_turn, "publish", "--task", follower, published)
    destination = host.home.parent / f"export-{number}"
    host.approved(follower, follower_turn, "export", published, destination)
    assert (destination / "result").read_bytes() == payload.encode()
    shutil.rmtree(destination)
    for task, credential in ((owner, turn), (follower, follower_turn)):
        assert host.cli("graph", "--task", task, "finish", turn=credential) == {
            "closed": True,
            "cleanup_pending": [],
        }
        instance = host.instances.pop(task)
        assert host.record("instance", instance["id"])["status"] == "unloaded"
        assert not (host.home / "instances" / instance["id"]).exists()
    assert host.rows("SELECT count(*) FROM execution_queue") == [(0,)]
    return {
        "cycle": number,
        "oom": receipt,
        "loaded": loaded,
        "closed": resources(host),
    }


def exercise(host, seconds):
    start = time.monotonic()
    next_oom = start
    completed = oom_count = 0
    print(
        json.dumps(
            {
                "event": "start",
                "requested_seconds": seconds,
                "registry": str(host.home),
                "before": resources(host),
            }
        ),
        flush=True,
    )
    while time.monotonic() - start < seconds:
        now = time.monotonic()
        inject_oom = now >= next_oom
        row = cycle(host, completed + 1, inject_oom)
        if inject_oom:
            oom_count += 1
            next_oom = now + 900
        completed += 1
        print(
            json.dumps({**row, "elapsed_s": round(time.monotonic() - start, 2)}),
            flush=True,
        )
    return {
        "cycles": completed,
        "oom_cycles": oom_count,
        "workload_seconds": round(time.monotonic() - start, 2),
    }


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("image")
    parser.add_argument("--seconds", type=int, default=7200)
    args = parser.parse_args()
    if args.seconds < 1:
        parser.error("--seconds must be positive")
    directory = Path(tempfile.mkdtemp(prefix="an-mixed-soak-", dir="/private/tmp"))
    host = Host.create(directory, args.image, slots=1)
    try:
        report = exercise(host, args.seconds)
    finally:
        host.cleanup()
    # Keep the ledger for diagnosis when an assertion or subprocess fails.
    shutil.rmtree(directory)
    print(json.dumps({**report, "cleanup": "confirmed"}), flush=True)
