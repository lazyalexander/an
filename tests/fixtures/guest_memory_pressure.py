"""Run inside the 512 MiB test guest. Emit kernel counters before the worker settles."""

import json
import signal
import subprocess
import sys
import time
from pathlib import Path


def counters():
    fields = dict(
        line.split()
        for line in Path("/sys/fs/cgroup/memory.events").read_text().splitlines()
    )
    return {name: int(fields[name]) for name in ("oom", "oom_kill")}


def main():
    limit = int(Path("/sys/fs/cgroup/memory.max").read_text())
    if limit != 512 * 1024**2:
        raise RuntimeError(f"Expected a 512 MiB test cgroup, got {limit}")
    before = counters()
    output = Path("/output")
    (output / "ready").touch()
    deadline = time.monotonic() + 45
    while not (output / "proceed").exists():
        if time.monotonic() >= deadline:
            raise TimeoutError("waiting for the competing task to queue")
        time.sleep(0.1)

    # Keep the observer alive while a single child touches one bounded allocation.
    child = subprocess.run(
        [
            sys.executable,
            "-c",
            (
                "data = bytearray(640 * 1024**2)\n"
                "data[::4096] = bytes([1]) * (len(data) // 4096)"
            ),
        ],
        timeout=45,
        check=False,
    )
    after = counters()
    print(
        json.dumps(
            {
                "limit_bytes": limit,
                "before": before,
                "after": after,
                "child_returncode": child.returncode,
            }
        ),
        flush=True,
    )
    return 128 + signal.SIGKILL if child.returncode == -signal.SIGKILL else 1


if __name__ == "__main__":
    sys.exit(main())
