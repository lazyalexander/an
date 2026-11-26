"""Exercise a fixed forked artifact graph using Apple Container, independently of Codex."""

import argparse
import json
from pathlib import Path
import subprocess
import tomllib

from catalog import COMMAND, CONTAINER, HERE, SPOOLS
from live import Runtime


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
    labels = {
        item["configuration"].get("labels", {}).get("an.instance") for item in listed
    }
    report["remaining_instances"] = sorted(labels.intersection(report["instances"]))
    report["cleanup"] = not report["remaining_instances"]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", required=True, type=Path)
    parser.add_argument("--an", type=Path, default=HERE.parents[1] / "target/debug/an")
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
        check_access(report)
        report["stage"] = "provision"
        runtime = Runtime(directory, args.an.resolve(), case["parents"])
        report["images"] = runtime.images
        report["stage"] = "submit"
        runtime.begin(case)
        plan = reference(case, runtime.images)
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
