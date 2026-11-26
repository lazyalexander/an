"""Text and settings live in studio/config, never in code."""

import json
import re
import subprocess
import sys
from pathlib import Path

from orchestrator import config

STUDIO = Path(__file__).resolve().parent.parent
CJK = re.compile(r"[一-鿿]")
CODE = [p for root in ("orchestrator", "adapter", "blender_addon", "pipeline")
        for p in (STUDIO / root).rglob("*")
        if p.suffix in {".py", ".js", ".html"} and "__pycache__" not in p.parts]


def _lookup(tree, key):
    node = tree
    for part in key.split("."):
        node = node[part]
    assert isinstance(node, str), key
    return node


def test_defaults_load_and_name_an_existing_locale():
    loaded = config.load()
    assert "legacy" in config.strings(loaded["locale"])


def test_no_user_facing_text_is_hardcoded_in_code():
    offenders = [f"{p.relative_to(STUDIO)}:{n}"
                 for p in CODE
                 for n, line in enumerate(p.read_text(encoding="utf-8").splitlines(), 1)
                 if CJK.search(line)]
    assert offenders == []


def test_every_text_key_used_by_the_pages_and_docker_exists():
    strings = config.strings(config.load()["locale"])
    static = STUDIO / "orchestrator" / "macro" / "static"
    legacy = set(re.findall(r"chrome\.(\w+)", (static / "app.js").read_text(encoding="utf-8")))
    legacy |= set(re.findall(r'data-t="(\w+)"', (static / "index.html").read_text(encoding="utf-8")))
    for key in legacy:
        _lookup(strings["legacy"], key)

    docker = (STUDIO / "adapter" / "krita_plugin" / "bridge_studio" / "docker.py").read_text()
    for key in set(re.findall(r'_t\("(\w+)"', docker)):
        _lookup(strings["krita"], key)


def test_boot_rejects_a_misspelled_override(tmp_path):
    bad = tmp_path / "studio.json"
    bad.write_text(json.dumps({"graph": {"poll": 5}}))
    proc = subprocess.run([sys.executable, "-m", "orchestrator", "--fake", "--config", str(bad)],
                          cwd=STUDIO, capture_output=True, text=True, timeout=30, check=False)
    assert proc.returncode == 2
    assert "unknown setting graph" in proc.stderr
