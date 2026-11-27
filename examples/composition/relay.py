"""Exercise a fixed forked artifact graph using Apple Container, independently of Codex."""

import argparse
import json
from pathlib import Path
import subprocess
import tomllib

from catalog import COMMAND, CONTAINER, HERE, SPOOLS
from live import Runtime
from scoring import assess_safely


def reference(case, images):
    def output(name):
        return {"node": "n-" + name, "port": SPOOLS[name]["output"]}

    nodes = []
    for name, parent in case["parents"].items():
        spec = SPOOLS[name]
        source = {"input": parent} if parent in case["inputs"] else output(parent)
        nodes.append(
            dict(
                id="n-" + name,
                kind="run",
                spool=images[name],
                argv=COMMAND.copy(),
                inputs={spec["input"]: source},
                outputs=[spec["output"]],
            )
        )
    if case["deliver"]:
        nodes.append(
            dict(
                id="results",
                kind="delivery",
                outputs=[],
                inputs={str(i): output(name) for i, name in enumerate(case["deliver"])},
            )
        )
    return {
        "decision": "plan" if nodes else "unsupported",
        "nodes": list(reversed(nodes)),
    }


def read_plan(path, case):
    plan = json.loads(path.read_text())
    aliases = {name: name for name in SPOOLS}
    if not all(assess_safely(plan, case, aliases).values()):
        raise ValueError("Planner output failed fixture grading")
    for node in plan["nodes"]:
        fields = {"id", "kind", "title", "after", "inputs", "outputs"}
        if node["kind"] == "run":
            fields.update({"spool", "argv"})
        if node.keys() - fields:
            raise ValueError("Unexpected fixture node field")
    return plan


def check_access(report):
    checks = report["access"] = []
    for args in (
        ["--version"],
        ["system", "status"],
        ["list", "--all", "--format", "json"],
    ):
        cmd = [CONTAINER, *args]
        check = {"argv": cmd}
        checks.append(check)
        result = subprocess.run(cmd, capture_output=True, text=True, timeout=30)
        check.update(exit_code=result.returncode, stderr=result.stderr.strip())
        if result.returncode:
            raise RuntimeError(f"{' '.join(cmd)}: {result.stderr.strip()}")
        if args[0] != "list":
            check["stdout"] = result.stdout.strip()
        elif not isinstance(json.loads(result.stdout), list):
            raise ValueError("Expected a container list")


def verify_cleanup(runtime, report):
    report["cleanup"] = False
    try:
        if report.get("instances") and "instance_names" not in report:
            instances = runtime.cli("state", "--task", runtime.task)["instances"]
            report["instance_names"] = {i["name"]: i["id"] for i in instances}
    finally:
        report["host_cleanup"] = runtime.close()
    if not report["host_cleanup"] or not report.get("instances"):
        return
    listed = json.loads(
        subprocess.check_output(
            [CONTAINER, "list", "--all", "--format", "json"], timeout=30
        )
    )
    if not isinstance(listed, list):
        raise ValueError("Expected a container list")
    # A changed/missing label cannot establish that a same-name container
    # disappeared. A matching label under another name is residual work too.
    names = report["instance_names"]
    owned = set(report["instances"])
    if set(names.values()) != owned or len(names) != len(owned):
        raise ValueError("Missing host container names")
    remaining = set()
    for item in listed:
        name = item["id"]
        if not isinstance(name, str) or not name:
            raise ValueError("Invalid container inventory ID")
        if name in names:
            remaining.add(names[name])
        label = item["configuration"].get("labels", {}).get("an.instance")
        if label in owned:
            remaining.add(label)
    report["remaining_instances"] = sorted(remaining)
    report["cleanup"] = not report["remaining_instances"]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", required=True, type=Path)
    parser.add_argument("--an", type=Path, default=HERE.parents[1] / "target/debug/an")
    parser.add_argument(
        "--plan", type=Path, help="Replay a recorded flat-colour planner answer"
    )
    args = parser.parse_args()
    directory = args.out.resolve()
    directory.mkdir(mode=0o700, parents=True, exist_ok=False)
    case = next(
        c
        for c in tomllib.loads((HERE / "cases.toml").read_text())["case"]
        if c["name"] == "flat-colour"
    )
    (directory / case["name"]).mkdir()
    runtime = None
    report = {"case": case["name"], "stage": "access", "passed": False}
    try:
        plan = None
        if args.plan:
            report["stage"] = "grade"
            plan = read_plan(args.plan, case)
            report["plan_source"] = "recorded"
        report["stage"] = "access"
        check_access(report)
        report["stage"] = "provision"
        runtime = Runtime(directory, args.an.resolve(), case["parents"])
        report["images"] = runtime.images
        report["stage"] = "submit"
        runtime.begin(case)
        if plan is None:
            plan = reference(case, runtime.images)
        else:
            for node in plan["nodes"]:
                if node["kind"] == "run":
                    node["spool"] = runtime.images[node["spool"]]
        (directory / "submitted-plan.json").write_text(json.dumps(plan, indent=2))
        receipt = runtime.graph(
            "submit",
            nodes=plan["nodes"],
            base_revision=runtime.graph("state")["revision"],
            idempotency_key="relay",
        )
        if (
            receipt["status"] != "accepted"
            or runtime.cli("state", "--task", runtime.task)["instances"]
        ):
            raise ValueError(
                "Expected accepted graph with no allocated instances while paused"
            )
        report["stage"] = "execute"
        report.update(runtime.execute(plan, case))
        graph = runtime.graph("state")
        (directory / "executed-graph.json").write_text(json.dumps(graph, indent=2))
        instances = runtime.cli("state", "--task", runtime.task)["instances"]
        expected = {runtime.images[name] for name in case["parents"]}
        if {i["spool"] for i in instances} != expected:
            raise ValueError("Expected a task instance for every selected spool")
        report["instances"] = [i["id"] for i in instances]
        report["instance_names"] = {i["name"]: i["id"] for i in instances}
        report["stage"] = "cleanup"
    except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as exc:
        report["error"] = str(exc)
    finally:
        if runtime and runtime.task:
            try:
                verify_cleanup(runtime, report)
            except (
                OSError,
                ValueError,
                KeyError,
                TypeError,
                AttributeError,
                subprocess.SubprocessError,
            ) as exc:
                report["cleanup_error"] = str(exc)
        report["passed"] = (
            report["stage"] == "cleanup"
            and report.get("cleanup") is True
            and "error" not in report
        )
        (directory / "report.json").write_text(json.dumps(report, indent=2))
        print(json.dumps(report))
    return int(not report["passed"])


if __name__ == "__main__":
    raise SystemExit(main())
