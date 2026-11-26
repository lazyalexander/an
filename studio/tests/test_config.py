"""Text and settings live in studio/config, never in code."""

import json
import re
import socket
import subprocess
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path

import pytest
from orchestrator import config

STUDIO = Path(__file__).resolve().parent.parent
CJK = re.compile(r"[一-鿿]")
CODE = [p for root in ("orchestrator",)
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
    assert loaded["graph"]["poll_ms"] > 0
    assert "graph" in config.strings(loaded["locale"])


@pytest.mark.parametrize("override, message", [
    ({"graph": {"poll_msec": 5}}, "unknown setting graph.poll_msec"),
    ({"colour": "red"}, "unknown setting colour"),
    ({"graph": {"poll_ms": 0}}, "positive integer"),
    ({"graph": {"poll_ms": True}}, "positive integer"),
    ({"graph": 5}, "must be an object"),
    ({"locale": "xx-YY"}, "unknown locale"),
])
def test_overrides_reject_typos_and_bad_values(tmp_path, override, message):
    path = tmp_path / "studio.json"
    path.write_text(json.dumps(override))
    with pytest.raises(ValueError, match=message):
        config.load(path)


def test_override_replaces_a_known_setting(tmp_path):
    path = tmp_path / "studio.json"
    path.write_text(json.dumps({"graph": {"poll_ms": 250}}))
    loaded = config.load(path)
    assert loaded["graph"]["poll_ms"] == 250
    assert loaded["graph"]["panel_timeout_ms"] == config.load()["graph"]["panel_timeout_ms"]


def test_no_user_facing_text_is_hardcoded_in_code():
    offenders = [f"{p.relative_to(STUDIO)}:{n}"
                 for p in CODE
                 for n, line in enumerate(p.read_text(encoding="utf-8").splitlines(), 1)
                 if CJK.search(line)]
    assert offenders == []


def test_every_text_key_used_by_the_pages_and_docker_exists():
    strings = config.strings(config.load()["locale"])
    graph_dir = STUDIO / "orchestrator" / "macro" / "graph"
    script = (graph_dir / "graph.js").read_text(encoding="utf-8")
    html = (graph_dir / "index.html").read_text(encoding="utf-8")
    graph_keys = {k for k in re.findall(r'\bt\("([\w.]+)"', script) if not k.endswith(".")}
    graph_keys |= set(re.findall(r'data-t(?:-label|-placeholder)?="([\w.]+)"', html))
    graph_keys |= {f"status.{s}" for s in ("queued", "running", "waiting", "saving", "done",
                                           "failed", "unknown", "superseded",
                                           "permission_required", "cancelled")}
    graph_keys |= {f"evidence.{k}" for k in ("code", "work", "input", "log")}
    graph_keys |= {"instruction.stale", "instruction.sent"}
    graph_keys |= {f"stage.{k}" for k in ("agent", "driver", "running", "done", "failed", "stopped")}
    graph_keys |= {f"failure.{k}" for k in ("denied", "authority", "exit", "output", "budget",
                                            "declaration", "unknown", "other")}
    for key in graph_keys:
        _lookup(strings["graph"], key)

def _free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def test_task_page_serves_text_and_timing_from_config():
    port = _free_port()
    proc = subprocess.Popen(
        [sys.executable, "-m", "orchestrator", "--task", "t", "--an", "/usr/bin/false",
         "--port", str(port)], cwd=STUDIO, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        for _ in range(100):
            try:
                with urllib.request.urlopen(f"http://127.0.0.1:{port}/config.json", timeout=1) as r:
                    served = json.loads(r.read())
                break
            except OSError:
                time.sleep(0.05)
        else:
            raise AssertionError("task page did not start")
    finally:
        proc.terminate()
        proc.wait(timeout=5)
    loaded = config.load()
    assert served["locale"] == loaded["locale"]
    assert served["settings"] == {"poll_ms": loaded["graph"]["poll_ms"],
                                  "panel_timeout_ms": loaded["graph"]["panel_timeout_ms"]}
    assert served["strings"] == config.strings(loaded["locale"])["graph"]
    assert "max_body_bytes" not in served["settings"]


def test_boot_rejects_a_misspelled_override(tmp_path):
    bad = tmp_path / "studio.json"
    bad.write_text(json.dumps({"graph": {"poll": 5}}))
    proc = subprocess.run([sys.executable, "-m", "orchestrator", "--task", "example", "--config", str(bad)],
                          cwd=STUDIO, capture_output=True, text=True, timeout=30, check=False)
    assert proc.returncode == 2
    assert "unknown setting graph.poll" in proc.stderr


def test_task_page_labels_statuses_only_through_config():
    # Regression: a leftover hardcoded label table left `labels` undefined,
    # which stopped rendering before any panel could mount.
    script = (STUDIO / "orchestrator" / "macro" / "graph" / "graph.js").read_text(encoding="utf-8")
    assert "labels[" not in script
    assert "statusLabel(s.status)" in script


def test_workspace_studio_exposes_start_without_leaking_the_turn():
    port = _free_port()
    proc = subprocess.Popen(
        [sys.executable, "-m", "orchestrator", "--workspace", "w", "--an", "/usr/bin/false",
         "--port", str(port)], cwd=STUDIO, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
        text=True)
    try:
        url = proc.stdout.readline().strip().split("macro UI: ")[1]
        with urllib.request.urlopen(f"http://127.0.0.1:{port}/config.json", timeout=2) as r:
            served = json.loads(r.read())
        assert served["workspace"] is True and served["task"] is None
        key = url.split("#", 1)[1]
        request = urllib.request.Request(
            f"http://127.0.0.1:{port}/api/view", data=b"{}", method="POST",
            headers={"Authorization": "Bearer " + key, "Content-Type": "application/json"})
        try:
            urllib.request.urlopen(request, timeout=2)
            raise AssertionError("view without a task must fail")
        except urllib.error.HTTPError as exc:
            assert exc.code == 409 and b"Unknown task" in exc.read()
    finally:
        proc.terminate()
        proc.wait(timeout=5)
    assert "AN_TURN" not in json.dumps(served)
