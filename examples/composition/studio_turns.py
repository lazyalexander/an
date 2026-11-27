"""Change the goal while Studio is holding the first line-art publication for approval."""

import argparse
import json
import subprocess
import time
import tomllib
from pathlib import Path

from catalog import CONTAINER, COMMAND, HERE, SPOOLS
from live import Runtime
from relay import check_access, verify_cleanup
from studio_client import studio
from studio_evidence import held_publication, verify_delivery, verify_plan, verify_wakes

STAGES = (
    ("ink", "把这些笔迹整理成干净的黑白线稿，只交付线稿。"),
    (
        "flat-colour",
        "改一下：保留刚生成的黑白线稿，直接用同一版线稿做一张平涂彩图，把线稿和彩图分别交给我。",
    ),
)
CASES = {
    case["name"]: case
    for case in tomllib.loads((HERE / "cases.toml").read_text())["case"]
}


def allowed_flows(runtime, stage):
    images = runtime.images

    def handoff(origin, dest):
        return (
            "handoff",
            images[origin],
            SPOOLS[origin]["output"],
            images[dest],
            tuple(COMMAND),
        )

    def publish(origin):
        return (
            "publish",
            images[origin],
            SPOOLS[origin]["output"],
            runtime.workspace,
            tuple(COMMAND),
        )

    if stage == "ink":
        return {
            ("input", "task-input:strokes.json", "strokes.json", images["sketch"], ()),
            handoff("sketch", "line-art"),
            publish("line-art"),
        }
    return {handoff("line-art", "painting"), publish("painting")}


def approve(request, card, expected, hold_publish=False):
    rows = card["routes"]
    actual = set()
    for row in rows:
        source = row["source"]
        port = source.get("input") or source.get("producer_port") or source.get("port")
        actual.add(
            (
                row["operation"],
                row["origin"],
                port,
                row["destination"],
                tuple(row.get("producer_argv", [])),
            )
        )
    if (
        card["turn_scope"] is not True
        or actual != expected
        or len(rows) != len(expected)
    ):
        raise ValueError("Proposed scope differs from the requested fixture flows")
    decisions = [
        "ask" if hold_publish and row["operation"] == "publish" else "allow"
        for row in rows
    ]
    request("intent-decide", id=card["id"], decisions=decisions)
    return {"id": card["id"], "routes": rows, "decisions": decisions}


def await_stage(
    request, runtime, case, receipt, record, timeout, previous, held_request
):
    deadline = time.monotonic() + timeout
    initial = case["name"] == "ink"
    while time.monotonic() < deadline:
        view = request("view")
        card = view.get("intent")
        if card and card["status"] == "pending":
            if "approval" in record:
                raise ValueError("Unexpected additional approval card")
            if card.get("instruction_cursor") != receipt["cursor"]:
                raise ValueError("Scope proposal belongs to another instruction")
            record["approval"] = approve(
                request,
                card,
                allowed_flows(runtime, case["name"]),
                hold_publish=initial,
            )
        agent = view["agent"]
        if agent["state"] == "failed":
            raise RuntimeError(
                "Codex wake failed: " + agent.get("detail", "see agent wake log")
            )
        states = [a["status"] for a in view["activity"].values()]
        if any(s in ("failed", "cancelled", "unknown", "superseded") for s in states):
            raise ValueError(f"Unsettled graph: {states}")
        if "approval" in record and view["revision"] > receipt["revision"]:
            pending = view["permissions"]
            scope_decided = card and card["status"] == "decided"
            at_hold = initial and any(p["operation"] == "publish" for p in pending)
            at_extension = not initial and "released_request" not in record
            finished = (
                not initial
                and states
                and all(s == "done" for s in states)
                and agent["state"] == "idle"
            )
            if not scope_decided or not (at_hold or at_extension or finished):
                time.sleep(0.5)
                continue
            graph = runtime.graph("state")
            try:
                added = verify_plan(runtime, case, graph, previous, receipt["cursor"])
            except ValueError:
                if agent["state"] != "running":
                    raise
                # The agent may submit the delta in several calls within one wake.
                time.sleep(0.5)
                continue
            record["added_nodes"] = added
            if at_hold:
                record["held_publication"] = held_publication(
                    runtime, graph, pending, view["deliveries"]
                )
                record["revision"] = graph["revision"]
                return graph
            if at_extension:
                if not any(p["id"] == held_request for p in pending):
                    raise ValueError(
                        "Line art was released before the second graph was submitted"
                    )
                # The second instruction has now changed the graph while the original
                # delivery still awaits its one-time decision.
                request("decide", request=held_request, decision="once")
                record["released_request"] = held_request
            elif finished:
                record.update(verify_delivery(request, case, graph))
                return graph
        time.sleep(0.5)
    raise TimeoutError(
        "Timed out waiting for Codex, approved graph and fixture publication"
    )


def exercise(runtime, report, timeout):
    selector = ["--harness", "studio-acceptance", "--session", "replan"]
    runtime.cli("workspace", "bind", runtime.workspace, *selector)
    runtime.task = runtime.cli(
        "task", "new", *selector, "--input", HERE / "inputs/strokes.json"
    )["task"]
    runtime.env["AN_TURN"] = runtime.cli("permission", "begin", "--task", runtime.task)[
        "controller"
    ]
    report.update(task=runtime.task, images=runtime.images, stage="studio")
    previous = {}
    held_request = None
    with studio(runtime) as request:
        for name, text in STAGES:
            report["stage"] = name
            record = {"case": name, "instruction": text}
            report["instructions"].append(record)
            receipt = request(
                "instruct", text=text, revision=request("view")["revision"]
            )
            record["cursor"] = receipt["cursor"]
            if receipt["stale"]:
                raise ValueError("Studio instruction targeted a stale graph revision")
            previous = await_stage(
                request,
                runtime,
                CASES[name],
                receipt,
                record,
                timeout,
                previous,
                held_request,
            )
            if name == "ink":
                held = record["held_publication"]
                held_request = held["request"]
                instances = runtime.cli("state", "--task", runtime.task)["instances"]
                producer = next(a for a in previous["activity"].values() if a["id"] == held["producer_attempt"])
                instance = next(i for i in instances if i["id"] == producer["instance"] and i["spool"] == runtime.images["line-art"])
                result = runtime.cli("unload", "--task", runtime.task, "--instance", instance["id"])
                if result["status"] != "unloaded":
                    raise ValueError("Line-art instance could not be reclaimed")
                inventory = json.loads(subprocess.check_output([CONTAINER, "list", "--all", "--format", "json"], timeout=30))
                if not isinstance(inventory,list) or any(item["id"] == instance["name"] or item["configuration"].get("labels", {}).get("an.instance") == instance["id"] for item in inventory):
                    raise ValueError("Line-art instance still present before redirect")
                record["reclaimed_before_redirect"] = instance["id"]
        report["codex_thread"] = verify_wakes(
            runtime.directory, runtime.task, report["instructions"]
        )
    instances = runtime.cli("state", "--task", runtime.task)["instances"]
    if {i["spool"] for i in instances} != {
        runtime.images[n] for n in CASES["flat-colour"]["parents"]
    }:
        raise ValueError(
            "Expected a task container for each of the three selected spools"
        )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", required=True, type=Path)
    parser.add_argument("--an", type=Path, default=HERE.parents[1] / "target/debug/an")
    parser.add_argument(
        "--timeout", type=int, default=420, help="seconds per Studio instruction"
    )
    args = parser.parse_args()
    directory = args.out.resolve()
    directory.mkdir(parents=True, mode=0o700, exist_ok=False)
    report = {"passed": False, "stage": "access", "instructions": []}
    runtime = None
    try:
        check_access(report)
        report["stage"] = "provision"
        runtime = Runtime(directory, args.an.resolve())
        exercise(runtime, report, args.timeout)
        report["stage"] = "cleanup"
    except (
        OSError,
        ValueError,
        RuntimeError,
        KeyError,
        TypeError,
        subprocess.SubprocessError,
    ) as exc:
        report["error"] = str(exc)
    finally:
        if runtime and runtime.task:
            try:
                try:
                    instances = runtime.cli("state", "--task", runtime.task)[
                        "instances"
                    ]
                    report["instances"] = [i["id"] for i in instances]
                    report["instance_names"] = {i["name"]: i["id"] for i in instances}
                finally:
                    verify_cleanup(runtime, report)
            except (
                OSError,
                ValueError,
                RuntimeError,
                KeyError,
                TypeError,
                subprocess.SubprocessError,
            ) as exc:
                report["cleanup_error"] = str(exc)
        report["passed"] = (
            report["stage"] == "cleanup"
            and report.get("cleanup") is True
            and "error" not in report
        )
        (directory / "report.json").write_text(
            json.dumps(report, ensure_ascii=False, indent=2) + "\n"
        )
    print(directory / "report.json")
    return int(not report["passed"])


if __name__ == "__main__":
    raise SystemExit(main())
