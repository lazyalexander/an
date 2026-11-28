"""Compile the domain with its declared dependencies and check the production adapters."""

import json
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import tomllib

ROOT = Path(__file__).resolve().parents[1]


def verify(condition, message):
    if not condition:
        raise SystemExit(message)


def isolated_domain(contract):
    package = tomllib.loads((ROOT / "Cargo.toml").read_text())
    manifest = [
        "[package]",
        'name="an-domain-boundary"',
        'version="0.0.0"',
        'edition="2021"',
        "[dependencies]",
    ]
    for name in contract["dependencies"]:
        dependency = package["dependencies"][name]
        if isinstance(dependency, dict):
            fields = ", ".join(
                f"{key} = {json.dumps(value)}" for key, value in dependency.items()
            )
            manifest.append(f"{name} = {{ {fields} }}")
        else:
            manifest.append(f"{name} = {json.dumps(dependency)}")
    with tempfile.TemporaryDirectory() as tmp:
        folder = Path(tmp)
        (folder / "Cargo.toml").write_text("\n".join(manifest))
        (folder / "src").mkdir()
        for source, valid in (
            (contract["source"], True),
            (contract["negative_fixture"], False),
        ):
            shutil.copy(ROOT / source, folder / "src/lib.rs")
            check = subprocess.run(
                [
                    "cargo",
                    "check",
                    "--quiet",
                    "--offline",
                    "--target-dir",
                    ROOT / "target/architecture",
                ],
                cwd=folder,
                text=True,
                capture_output=True,
            )
            if valid:
                verify(check.returncode == 0, check.stderr)
            else:
                verify(
                    check.returncode != 0 and "unresolved import" in check.stderr,
                    "dependency fixture escaped the domain",
                )


def check():
    spec = tomllib.loads((ROOT / "architecture.toml").read_text())
    isolated_domain(spec["domain"])
    api = spec["provenance"]
    for path in api["callers"]:
        code = (ROOT / path).read_text()
        calls = set(re.findall(r"\bprovenance\s*::\s*([\w:]+)", code))
        verify(
            api["entries"][path] in calls and calls <= set(api["adapter_api"]),
            f"{path}: unexpected application entry points {calls}",
        )
        verify(
            api["storage_prefix"] not in code
            and not re.search(r"use\s+[^;]*\bprovenance\b", code),
            f"{path}: access provenance through the named facade",
        )
    inbox = spec["workspace_inbox"]
    private_table = re.compile(r"\b(?:" + "|".join(map(re.escape, inbox["private_tables"])) + r")\b")
    for path in inbox["sources"]:
        verify(not private_table.search((ROOT / path).read_text()), f"{path}: use frozen block projection")
    verify(private_table.search((ROOT / inbox["negative_fixture"]).read_text()), "private SQL fixture escaped")
    gc = (ROOT / spec["retention"]["consumer"]).read_text()
    verify(api["storage_prefix"] not in gc, "GC must use the provenance retention API")
    core = spec["core"]
    for file in (ROOT / core["root"]).rglob("*.rs"):
        imports = re.findall(
            r'(?:include(?:_str|_bytes)?!\(\s*|#\[path\s*=\s*)"([^"]+)"',
            file.read_text(),
        )
        for target in imports:
            path = (file.parent / target).resolve()
            verify(
                not any(
                    path.is_relative_to(ROOT / root)
                    for root in core["excluded_sources"]
                ),
                f"{file}: imports {target}",
            )


if __name__ == "__main__":
    check()
