"""Pure macro-state builder: one JSON dict per poll.

Single `ledger.read_all()` and one `list_versions()` per call; never mutates
the app or the ledger.
"""

import time
from collections.abc import Callable
from pathlib import Path

from adapter.core.archive import list_versions

from ..app import Orchestrator
from ..ledger import Ledger
from ..scenes import SceneStore

POSE_LIVE_MS = 2000
ADAPTER_LIVE_MS = 3000
PREVIEW_URL = "/api/macro/preview.png"

_RENDER_STAGE = {"rendering": "running", "rendered": "done", "failed": "failed"}
_SNAPSHOT_KINDS = frozenset({"file_drop", "import_done"})
_FAILURE_KINDS = frozenset({"render_failed", "import_failed"})


def build_state(app: Orchestrator, archives_dir: Path, ledger: Ledger,
                scenes: SceneStore | None, now: Callable[[], float] = time.time,
                max_events: int = 200) -> dict:
    status = app.status()
    versions = list_versions(archives_dir)
    entries = ledger.read_all()
    window = entries[-max_events:] if max_events > 0 else []
    session = status["session"]
    return {
        "stages": _stages(entries, versions, scenes, session, status["pose_age_ms"]),
        "phase": status["phase"],
        "pose_age_ms": status["pose_age_ms"],
        "session": session,
        "pending": _pending(status, session, versions),
        "archive": [_archive_entry(version) for version in versions],
        "drawer": _drawer(window),
        "health": _health(app, status["pose_age_ms"]),
        "preview": _preview(app),
    }


def _stages(entries, versions, scenes, session, pose_age_ms) -> list[dict]:
    return [
        _import_stage(entries, scenes),
        _pose_stage(pose_age_ms),
        _render_stage(session),
        {"id": "archive", "status": "done" if versions else "waiting",
         "detail": len(versions)},
    ]


def _import_stage(entries, scenes) -> dict:
    last_done = last_failed = None
    for entry in entries:
        if entry["kind"] == "import_done":
            last_done = entry
        elif entry["kind"] == "import_failed":
            last_failed = entry
    if last_failed is not None and (last_done is None
                                    or last_failed["ts"] > last_done["ts"]):
        return {"id": "import", "status": "failed", "detail": last_failed["summary"]}
    current = getattr(scenes, "current", None)
    if current is not None:
        return {"id": "import", "status": "done", "detail": current.id}
    return {"id": "import", "status": "waiting", "detail": None}


def _pose_stage(pose_age_ms) -> dict:
    live = pose_age_ms is not None and pose_age_ms <= POSE_LIVE_MS
    return {"id": "pose", "status": "running" if live else "waiting",
            "detail": pose_age_ms}


def _render_stage(session) -> dict:
    if session is None:
        return {"id": "render", "status": "waiting", "detail": None}
    return {"id": "render",
            "status": _RENDER_STAGE.get(session["state"], "waiting"),
            "detail": session["id"]}


def _pending(status, session, versions) -> list[dict]:
    if status["phase"] != "LOCKED" or session is None:
        return []
    if session["state"] == "failed":
        return [{"kind": "retry", "session": session["id"]}]
    if session["state"] == "rendered":
        version = next((v for v in versions if v["id"] == session["id"]), None)
        if (version is not None and version["status"] == "pushed"
                and Path(version["kra"]).exists()):
            return [{"kind": "adopt_or_rerender", "session": session["id"]}]
    return []


def _archive_entry(version) -> dict:
    return {
        "id": version["id"],
        "kra": str(version["kra"]),
        "created": version["created"],
        "status": version["status"],
        "restored_from": version["restored_from"],
        "thumb": f"/api/macro/archive-thumb?id={version['id']}",
    }


def _drawer(entries) -> dict:
    drawer = {"poses": [], "snapshots": [], "failures": []}
    for entry in reversed(entries):  # newest first
        kind = entry["kind"]
        if kind == "pose_stream":
            drawer["poses"].append(entry["summary"])
        elif kind in _SNAPSHOT_KINDS:
            drawer["snapshots"].append(entry["summary"])
        elif kind in _FAILURE_KINDS:
            drawer["failures"].append(entry)
    return drawer


def _health(app, pose_age_ms) -> dict:
    adapter_age = app.heartbeat_age_ms()
    return {
        "adapter": {"ok": adapter_age is not None and adapter_age <= ADAPTER_LIVE_MS,
                    "stale_ms": adapter_age},
        "pose": {"ok": pose_age_ms is not None and pose_age_ms <= POSE_LIVE_MS},
        "pipeline": _pipeline_health(app),
    }


def _pipeline_health(app) -> dict:
    health = getattr(getattr(app, "_pipeline", None), "health", None)
    if health is None:
        return {"ok": True, "detail": "unmonitored"}
    try:
        return health()
    except Exception as exc:  # noqa: BLE001 — failure is data, like app._render:
        # a health probe must degrade the poll, never break it
        return {"ok": False, "detail": str(exc)}


def _preview(app) -> str | None:
    run_dir = getattr(app, "_run_dir", None)
    if run_dir is None:
        return None
    current = Path(run_dir) / "preview" / "current.png"
    return PREVIEW_URL if current.exists() else None
