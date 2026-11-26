"""Run a real Codex thread against an isolated An task using public text fixtures."""

import argparse
import json
import os
import subprocess
import sys
import tempfile
import time
from pathlib import Path

from codex_session import Session
from intent import routes

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
AN = ROOT / "target/debug/an"


def commands(env):
    def cli(*args, body=None):
        raw = subprocess.check_output(
            [AN, *args], env=env, text=True, input=json.dumps(body or {})
        ).strip()
        return json.loads(raw) if raw.startswith(("{", "[")) else raw

    return cli


def checkpoint(path, value):
    temp = path.with_suffix(".tmp")
    temp.write_text(json.dumps(value))
    temp.replace(path)


def drive(job, env):
    task, spool = json.loads((job / "task.json").read_text())
    cli = commands(env)

    def graph(action, **data):
        return cli("graph", "--task", task, action, body=data)

    agent = Session(job, AN, task, env)
    position = job / "cursor.json"
    cursor = json.loads(position.read_text()) if position.exists() else 0
    if not graph("state")["revision"]:
        agent.turn(
            f"Review original.md using reviews.json with the admitted spool {spool}. Read its skill. "
            "Use its local model review workflow; request a human decision for conflicts. "
            "Submit the initial graph, then yield while An runs. Only the person may answer decisions.",
            "initial",
        )
    deadline = time.monotonic() + 600
    while time.monotonic() < deadline:
        state = graph("state", after=cursor)
        events = state["events"]
        if not (job / "disconnect.json").exists() and any(
            a["status"] == "waiting" for a in state["activity"].values()
        ):
            checkpoint(
                job / "disconnect.json",
                {
                    "cursor": cursor,
                    "thread": agent.session_file.read_text(),
                    "calls": [
                        c["attempt"]
                        for c in state["model_calls"]
                        if c["state"] == "completed"
                    ],
                },
            )
            return 75  # The supervisor reconnects a fresh process to this same thread.
        for event in events:
            if event["kind"] == "instruction":
                n = event["cursor"]
                # A restart after submit but before the local checkpoint can
                # replay this event. The host receipt is authoritative.
                if not any(
                    e.get("instruction_cursor") == n
                    for e in graph("state", after=n)["events"]
                ):
                    agent.turn(
                        f"Read the new user instruction from graph state after cursor {n - 1}. "
                        "Replace obsolete pending work including dependent nodes. Attach instruction_cursor "
                        f"and use idempotency key instruction-{n} for the submission. Use existing outputs and "
                        "admitted spool capabilities. Explain in the deliverable which requested edits were applied or left unapplied, then yield for authorization.",
                        f"instruction-{n}",
                    )
                    if not any(
                        e.get("instruction_cursor") == n
                        for e in graph("state", after=n)["events"]
                    ):
                        raise RuntimeError("No submission linked to the instruction")
            cursor = event["cursor"]
        if events:
            checkpoint(position, cursor)
        if any(
            a["status"] in ("failed", "unknown") for a in state["activity"].values()
        ) or graph("deliveries"):
            return 0
        time.sleep(0.4)
    raise TimeoutError("Waiting for instruction or delivery")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--browser-test",
        choices=("allow", "deny"),
        help="Drive Studio with the public acceptance fixture",
    )
    parser.add_argument("--drive", type=Path, help=argparse.SUPPRESS)
    args = parser.parse_args()
    if args.drive:
        return drive(args.drive, os.environ)
    job = Path(tempfile.mkdtemp(prefix="an-replan-"))
    print("Evidence directory", job, flush=True)
    env = {**os.environ, "AN_HOME": str(job / "registry")}
    cli = commands(env)
    spool = cli("register", "an-markdown-review:replan")
    fixtures = HERE / "fixtures"
    base_config = Path(os.environ.get("AN_HOME", Path.home() / ".an")) / "config.json"
    if base_config.exists():
        cfg = json.loads(base_config.read_text())
        (job / "registry/config.json").write_text(
            json.dumps(
                {
                    k: v
                    for k, v in cfg.items()
                    if k.startswith("model_") or k == "ollama_port"
                }
            )
        )
    task = cli(
        "task",
        "new",
        "--spool",
        spool,
        "--model",
        "local",
        "--input",
        fixtures / "original.md",
        "--input",
        fixtures / "conflict/reviews.json",
    )["task"]
    env["AN_TURN"] = cli("permission", "begin", "--task", task)["turn"]

    def graph(action, **data):
        return cli("graph", "--task", task, action, body=data)

    studio = browser = worker = None
    try:
        policy = routes(spool, task)
        card = job / "intent.json"
        card.write_text(json.dumps(policy))
        cli("permission", "intent-propose", "--task", task, "--file", card)
        checkpoint(job / "task.json", [task, spool])
        studio = subprocess.Popen(
            [
                sys.executable,
                "-m",
                "orchestrator",
                "--task",
                task,
                "--an",
                AN,
                "--port",
                "0",
            ],
            cwd=ROOT / "studio",
            env=env,
            stdout=subprocess.PIPE,
            text=True,
        )
        url = studio.stdout.readline().strip().removeprefix("macro UI: ")
        if args.browser_test:
            browser = subprocess.Popen(
                [
                    "node",
                    ROOT / "tests/replan_browser.cjs",
                    args.browser_test,
                    str(job / "browser.json"),
                ],
                stdin=subprocess.PIPE,
                text=True,
            )
            browser.stdin.write(url)
            browser.stdin.close()
        else:
            print("Studio", url, flush=True)
        while True:
            worker = subprocess.Popen(
                [sys.executable, __file__, "--drive", job], env=env
            )
            while worker.poll() is None:
                if browser and browser.poll() not in (None, 0):
                    raise RuntimeError("Studio browser test failed")
                time.sleep(0.4)
            if worker.returncode != 75:
                break
        if worker.returncode:
            raise RuntimeError("Harness worker failed")
        state = graph("state")
        deliveries = graph("deliveries")
        failed = [
            a
            for a in state["activity"].values()
            if a["status"] in ("failed", "unknown")
        ]
        handled = [e["cursor"] for e in state["events"] if e["kind"] == "instruction"]
        from verify_replan import check

        if args.browser_test:
            browser.wait(timeout=60)
            if browser.returncode:
                raise RuntimeError("Studio acceptance failed")
            report = check(
                graph, state, policy, handled, args.browser_test, fixtures, job
            )
            (job / "result.json").write_text(
                json.dumps(report, ensure_ascii=False, indent=2)
            )
            print(json.dumps(report, ensure_ascii=False), flush=True)
        elif failed:
            raise RuntimeError(failed)
        if deliveries:
            graph("finish")
            if not args.browser_test:
                print("Delivery ready in Studio. Press Ctrl-C after downloading.", flush=True)
                try:
                    studio.wait()
                except KeyboardInterrupt:
                    pass
    finally:
        try:
            if not graph("view")["closed"]:
                cli("permission", "end", "--task", task)
        finally:
            for proc in (worker, browser, studio):
                if proc and proc.poll() is None:
                    proc.terminate()
                    proc.wait()
            (job / "codex/auth.json").unlink(missing_ok=True)
            for i in cli("state", "--task", task)["instances"]:
                if i["status"] != "unloaded":
                    cli("unload", "--task", task, "--instance", i["id"])


if __name__ == "__main__":
    sys.exit(main())
