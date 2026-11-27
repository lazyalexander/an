"""Measure the Rust host importing, publishing twice and exporting one binary artifact."""

import argparse
import hashlib
import json
import os
import platform
import random
import subprocess
import sys
import tempfile
import time
from pathlib import Path

MIB = 1 << 20
ROOT = Path(__file__).resolve().parents[2]
SLOTS = ("left", "right")


def checksum(path):
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(MIB), b""):
            digest.update(block)
    return digest.hexdigest()


def cli(binary, env, *args, body=None, measure=None):
    """Get the maximum resident set for this exact child, independent of earlier calls."""
    with tempfile.TemporaryFile() as stdin, tempfile.TemporaryFile() as stdout:
        stdin.write(json.dumps(body or {}).encode())
        stdin.seek(0)
        start = time.perf_counter()
        with subprocess.Popen(
            [binary, *args], env=env, stdin=stdin, stdout=stdout
        ) as proc:
            _, status, usage = os.wait4(proc.pid, 0)
            proc.returncode = os.waitstatus_to_exitcode(status)
            if proc.returncode:
                raise subprocess.CalledProcessError(proc.returncode, proc.args)
        if measure is not None:
            measure["elapsed_ms"] = round((time.perf_counter() - start) * 1000, 2)
            scale = 1 if sys.platform == "darwin" else 1024
            measure["peak_rss_mib"] = round(usage.ru_maxrss * scale / MIB, 2)
        stdout.seek(0)
        data = stdout.read()
    return json.loads(data) if data.strip() else None


def trial(binary, directory, size_mib):
    home = directory / "an"
    env = {
        k: v for k, v in os.environ.items() if k not in {"AN_TURN", "AN_AGENT_TOKEN"}
    }
    env["AN_HOME"] = str(home)
    source = directory / "payload.bin"
    generator = random.Random(42)
    with source.open("wb") as stream:
        for _ in range(size_mib):
            stream.write(generator.randbytes(MIB))
    expected = checksum(source)
    result = {
        "input_mib": size_mib,
        "sha256": expected,
        "import": {},
        "publish_two": {},
        "exports": {},
    }

    def an(*args, **kwargs):
        return cli(binary, env, *args, **kwargs)

    def registry_mib():
        return round(
            sum(p.stat().st_size for p in home.glob("registry.sqlite3*")) / MIB, 3
        )

    task = an(
        "task", "new", "--input", source, "--model", "local", measure=result["import"]
    )["task"]
    result["registry_after_import_mib"] = registry_mib()
    env["AN_TURN"] = an("permission", "begin", "--task", task)["turn"]
    binding = {"input": source.name}
    scope = {
        "operation": "publish",
        "origin": "task-input:" + source.name,
        "source": binding,
        "destination": task,
    }
    scope_path = directory / "scope.json"
    scope_path.write_text(json.dumps({"turn_scope": [scope]}))
    card = an("permission", "intent-propose", "--task", task, "--file", scope_path)
    an(
        "permission",
        "intent-decide",
        "--task",
        task,
        "--request-id",
        card["id"],
        "--decision",
        "allow",
    )

    nodes = [
        {"id": slot, "kind": "delivery", "inputs": {slot: binding}, "outputs": []}
        for slot in SLOTS
    ]
    receipt = an(
        "graph",
        "--task",
        task,
        "submit",
        measure=result["publish_two"],
        body={"base_revision": 0, "idempotency_key": "fork", "nodes": nodes},
    )
    if receipt["status"] != "accepted":
        raise ValueError(f"Unexpected submission receipt: {receipt}")
    # Read the projection without advancing the scheduler outside the timed submit.
    state = an("graph", "--task", task, "view")["activity"]
    if any(state[s]["status"] != "done" for s in SLOTS):
        raise ValueError("Fan-out deliveries did not finish")
    revisions = {state[s]["bindings"][s]["revision"] for s in SLOTS}
    deliveries = {state[s]["delivery"] for s in SLOTS}
    if len(revisions) != 1 or len(deliveries) != len(SLOTS):
        raise ValueError(
            "Expected separate deliveries of one immutable source revision"
        )
    result["registry_after_publish_mib"] = registry_mib()

    for slot in SLOTS:
        directory_out = directory / ("export-" + slot)
        export = ["export", state[slot]["delivery"], directory_out, "--task", task]
        pending = an(*export)
        if pending["status"] != "permission_required" or directory_out.exists():
            raise ValueError("Export did not stop for its own authorization")
        request = pending["request_id"]
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
        metrics = result["exports"][slot] = {}
        an(*export, "--request-id", request, measure=metrics)
        if checksum(directory_out / slot / source.name) != expected:
            raise ValueError(f"Exported bytes differ on {slot}")
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--an", type=Path, default=ROOT / "target/release/an")
    parser.add_argument("--sizes-mib", type=int, nargs="+", default=[1, 16, 64])
    parser.add_argument("--trials", type=int, default=3)
    args = parser.parse_args()
    if min([args.trials, *args.sizes_mib]) < 1:
        parser.error("Sizes and trial count must be positive")
    if sys.platform != "darwin" and not sys.platform.startswith("linux"):
        parser.error("Per-process RSS measurement requires macOS or Linux")
    binary = args.an.resolve()
    result = {
        "platform": platform.platform(),
        "binary_sha256": checksum(binary),
        "trials": [],
    }
    for size in args.sizes_mib:
        for repetition in range(1, args.trials + 1):
            with tempfile.TemporaryDirectory(prefix="an-artifact-") as tmp:
                sample = trial(binary, Path(tmp), size)
                sample["trial"] = repetition
                result["trials"].append(sample)
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
