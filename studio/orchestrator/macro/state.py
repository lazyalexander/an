"""Generic macro-state builder: one JSON dict per poll.

The builder knows the kernel (phase, session, ledger, health) but no
instance. Stage statuses, pending decisions, health channels and the output
port (artifacts + preview) come from the instance module; labels, surfaces,
decisions, drawer sections and health copy come from its manifest.

Single `ledger.read_all()` and one `versions(app)` call per poll; never
mutates the app or the ledger.
"""

import time
from collections.abc import Callable

from ..app import Orchestrator
from ..ledger import Ledger
from ..scenes import SceneStore
from .context import Context
from .instances import load as load_instance


def build_state(app: Orchestrator, ledger: Ledger,
                scenes: SceneStore | None, instance=None,
                now: Callable[[], float] = time.time,
                max_events: int = 200) -> dict:
    if instance is None:
        instance = load_instance("bridge-studio")
    manifest = instance.MANIFEST
    status = app.status()
    versions = _hook(instance, "versions", app, [])
    entries = ledger.read_all()
    window = entries[-max_events:] if max_events > 0 else []
    ctx = Context(app=app, status=status, entries=entries, versions=versions,
                  scenes=scenes, now=now())
    surfaces = _surfaces(manifest, status)
    output = _hook(instance, "outputs", ctx, {"preview": None, "artifacts": []})
    return {
        "instance": manifest.get("instance"),
        "stages": instance.stages(ctx),
        "surfaces": surfaces,
        "ports": _ports(manifest, surfaces),
        "phase": status["phase"],
        "pose_age_ms": status["pose_age_ms"],
        "session": status["session"],
        "pending_decision": status.get("pending_decision"),
        "document": status.get("document"),
        "pending": instance.pending(ctx),
        "archive": output.get("artifacts", []),
        "drawer": _drawer(window, manifest.get("drawer", [])),
        "health": instance.health(ctx),
        "health_copy": manifest.get("health", {}),
        "preview": output.get("preview"),
    }


def _hook(instance, name: str, argument, default):
    hook = getattr(instance, name, None)
    if hook is None:
        return default
    return hook(argument)


def _surfaces(manifest, status) -> list[dict]:
    surfaces = []
    for declared in manifest.get("surfaces", []):
        surface = {key: declared[key] for key in
                   ("id", "label", "kind", "app", "inputs") if key in declared}
        surface["active"] = declared.get("phase") == status["phase"]
        surfaces.append(surface)
    return surfaces


def _ports(manifest, surfaces) -> dict:
    declared = manifest.get("ports", {})
    active = next((s["id"] for s in surfaces if s["active"]), None)
    return {"input": list(declared.get("input", [])),
            "output": list(declared.get("output", [])),
            "active_surface": active}


def _drawer(entries, sections) -> dict:
    drawer = {}
    for section in sections:
        kinds = set(section.get("kinds", []))
        as_entry = section.get("format") == "entry"
        items = []
        for entry in reversed(entries):  # newest first
            if entry["kind"] not in kinds:
                continue
            items.append(entry if as_entry else entry["summary"])
        drawer[section["id"]] = items
    return drawer
