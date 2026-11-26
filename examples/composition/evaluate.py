"""Blind Codex composition benchmark. No containers are required to measure planning."""

import argparse
import json
import os
from pathlib import Path
import random
import subprocess
import sys
import tempfile
import tomllib

from catalog import HERE, SPOOLS, manifest, skill
from scoring import assess_safely

sys.path.insert(0, str(HERE.parent / "markdown_review"))
from codex_session import restricted_exec

CASES = tomllib.loads((HERE / "cases.toml").read_text())["case"]
PROTOCOL = """Return exactly one JSON object with decision ("plan" or "unsupported") and nodes.
Use only the supplied input names and catalog. For an unsupported request return no nodes.
Use An run nodes with id, kind, spool (catalog key), argv, inputs and outputs. A run input
maps its named input slot to {"input": filename} or {"node": producer_id, "port": filename}.
Use the command and filenames in the skill. Deliver requested artifacts via one or more
delivery nodes with id, kind, inputs (named source references) and outputs: [].
Do not use tools or invent operations. Choose the spools and their connections yourself."""


class InvalidPlan(ValueError):
    """The planner completed its turn but returned invalid JSON."""


def ask(case, catalog, directory, model, seed):
    entries = list(catalog.items())
    random.Random(seed).shuffle(entries)
    prompt = (
        PROTOCOL
        + "\n"
        + json.dumps(
            {
                "catalog": dict(entries),
                "available_inputs": case["inputs"],
                "request": case["sentence"],
            },
            ensure_ascii=False,
        )
    )
    (directory / "prompt.json").write_text(json.dumps(prompt, ensure_ascii=False))
    answer = directory / "answer.txt"
    args = restricted_exec(directory, model) + ["--ephemeral", "-o", str(answer)]
    # An empty workdir separates the agent from the evaluator's oracle and cases.
    with tempfile.TemporaryDirectory() as work:
        home = Path(work) / "codex-home"
        home.mkdir(mode=0o700)
        auth = Path(os.environ.get("CODEX_HOME", Path.home() / ".codex")) / "auth.json"
        if auth.is_file():
            (home / "auth.json").symlink_to(auth.resolve())
        planner_env = {
            k: os.environ[k]
            for k in ("HOME", "PATH", "TMPDIR", "OPENAI_API_KEY")
            if k in os.environ
        }
        run = subprocess.run(
            args + ["-"],
            input=prompt,
            cwd=work,
            text=True,
            env={**planner_env, "CODEX_HOME": str(home)},
            capture_output=True,
            timeout=150,
        )
    if run.returncode:
        raise RuntimeError("Codex failed: " + run.stderr[-1200:])
    raw = answer.read_text().strip()
    if raw.startswith("```") and raw.endswith("```"):
        raw = "\n".join(raw.splitlines()[1:-1])
    try:
        return json.loads(raw)
    except json.JSONDecodeError as exc:
        raise InvalidPlan(str(exc)) from exc


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--out", required=True, type=Path, help="new private results directory"
    )
    parser.add_argument("--model", default="gpt-6.2-sol")
    parser.add_argument("--seed", type=int, default=41)
    parser.add_argument("--case", choices=[c["name"] for c in CASES])
    parser.add_argument(
        "--execute",
        action="store_true",
        help="also build, register and run real images",
    )
    parser.add_argument("--an", default=HERE.parents[1] / "target/debug/an", type=Path)
    args = parser.parse_args()
    directory = args.out.resolve()
    directory.mkdir(mode=0o700, parents=True, exist_ok=False)
    images = {name: name for name in SPOOLS}
    catalog = {
        name: {"interface": manifest(name, spec), "skill": skill(spec)}
        for name, spec in SPOOLS.items()
    }
    if args.execute:
        from live import Runtime

        try:
            runtime = Runtime(directory, args.an.resolve())
        except (OSError, subprocess.SubprocessError, ValueError) as exc:
            error = {"stage": "provision", "error": str(exc), "scored": 0}
            (directory / "report.json").write_text(json.dumps(error, indent=2))
            print(json.dumps(error), flush=True)
            return 1
        images = runtime.images
        (directory / "images.json").write_text(json.dumps(images, indent=2))
    cases = [c for c in CASES if not args.case or c["name"] == args.case]
    report = {
        "mode": "execute" if args.execute else "plan",
        "model": args.model,
        "seed": args.seed,
        "cases": [],
        "requested": len(cases),
    }
    for case in cases:
        folder = directory / case["name"]
        folder.mkdir()
        row = {"case": case["name"]}
        try:
            if args.execute:
                plan = runtime.plan(case, folder, args.model)
            else:
                plan = ask(
                    case, catalog, folder, args.model, args.seed + CASES.index(case)
                )
            (folder / "plan.json").write_text(json.dumps(plan, indent=2))
            row.update(assess_safely(plan, case, images))
            if (
                args.execute
                and all(row[k] for k in ("selection", "wiring", "delivery"))
                and case["parents"]
            ):
                row["execution"] = runtime.execute(plan, case)
        except InvalidPlan:
            row.update(
                selection=False,
                wiring=False,
                delivery=False,
                error="Invalid planner JSON",
            )
        except (OSError, RuntimeError, subprocess.SubprocessError, ValueError) as exc:
            row["error"] = str(exc)
        finally:
            if args.execute:
                try:
                    row["cleanup"] = runtime.close()
                except (OSError, ValueError, subprocess.SubprocessError) as exc:
                    row.update(cleanup=False, cleanup_error=str(exc))
        report["cases"].append(row)
        print(json.dumps(row, ensure_ascii=False), flush=True)
        scored = [r for r in report["cases"] if "selection" in r]
        report["scored"] = len(scored)
        report["passed"] = {
            k: sum(r[k] for r in scored) for k in ("selection", "wiring", "delivery")
        }
        (directory / "report.json").write_text(
            json.dumps(report, ensure_ascii=False, indent=2)
        )
        if row.get("cleanup") is False or ("error" in row and "selection" not in row):
            break  # provider/host outage is not an incorrect model answer
    return int(
        any(report["passed"][k] != len(cases) for k in report["passed"])
        or any(r.get("cleanup") is False or "error" in r for r in report["cases"])
    )


if __name__ == "__main__":
    raise SystemExit(main())
