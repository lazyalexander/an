"""Bridge Studio instance: painting pipeline over the shared kernel.

Everything here is instance content. The framework (state builder, page,
server) reads `MANIFEST`, `stages(ctx)` and `pending(ctx)` and knows nothing
about imports, poses, renders or archives.
"""

from pathlib import Path

from adapter.core.archive import list_versions, read_thumbnail

from ...phase import InvalidTransition
from ...config import instance_manifest
from ..context import Context
from ..health import adapter, pose
from ..health import pipeline as pipeline_health
from ..ports import BadRequest, force

PREVIEW_URL = "/api/macro/preview.png"

MANIFEST = instance_manifest("bridge-studio")


def stages(ctx: Context) -> list[dict]:
    return [
        _import_stage(ctx.entries, ctx.scenes),
        _pose_stage(ctx.status["phase"], ctx.status["pose_age_ms"]),
        _render_stage(ctx.status["session"]),
        _archive_stage(ctx.versions),
    ]


def health(ctx: Context) -> dict:
    return {"adapter": adapter(ctx.app),
            "pose": pose(ctx.status),
            "pipeline": pipeline_health(ctx.app)}


def actions(app) -> dict:
    """Instance-owned commands; the framework only routes and gates them."""
    return {
        "adopt": lambda body: app.adopt(),
        "rerender": lambda body: app.rerender(force=force(body)),
        "retry": lambda body: _ok(app.retry()),
        "restore": lambda body: _restore(app, body),
        "restore_ack": lambda body: _ok(app.ack_restore()),
        "heartbeat": lambda body: _heartbeat(app, body),
        "ack": lambda body: _ack(app, body),
    }


def inputs(app) -> dict:
    """Event channels the user or a carrier can push into this instance."""
    return {"pose": lambda body: _pose(app, body)}


def reads(app) -> dict:
    return {"layers": lambda query: _layers(app)}


def thumbnail(app, version_id: str) -> bytes | None:
    version = next((v for v in list_versions(app.archives_dir)
                    if v["id"] == version_id), None)
    if version is None:
        return None
    return read_thumbnail(version["kra"])


def versions(app) -> list[dict]:
    return list_versions(app.archives_dir)


def outputs(ctx: Context) -> dict:
    preview = PREVIEW_URL if _preview_path(ctx.app).is_file() else None
    return {"preview": preview,
            "artifacts": [_archive_entry(version) for version in ctx.versions]}


def preview(app) -> bytes | None:
    path = _preview_path(app)
    return path.read_bytes() if path.is_file() else None


def pipeline():
    """The instance's execution backend; lazy so --fake needs no container."""
    from pipeline.container_pipeline import ContainerPipeline
    return ContainerPipeline()


def _preview_path(app) -> Path:
    return app.run_dir / "preview" / "current.png"


def _archive_entry(version) -> dict:
    return {
        "id": version["id"],
        "kra": str(version["kra"]),
        "created": version["created"],
        "status": version["status"],
        "restored_from": version["restored_from"],
        "thumb": f"/api/macro/archive-thumb?id={version['id']}",
    }


def _ok(value=None) -> dict:
    return {"ok": True}


def _pose(app, body) -> dict:
    matrix, location = body.get("matrix"), body.get("location")
    numbers = lambda seq: all(isinstance(v, (int, float)) for v in seq)
    if (not isinstance(matrix, list) or len(matrix) != 16 or not numbers(matrix)
            or not isinstance(location, list) or len(location) != 3
            or not numbers(location)):
        raise BadRequest("pose needs matrix[16] and location[3] of numbers")
    app.submit_pose({"matrix": [float(v) for v in matrix],
                     "location": [float(v) for v in location]})
    return {"ok": True}


def _heartbeat(app, body) -> dict:
    document = body.get("document")
    if document is not None and not _document_ok(document):
        raise BadRequest("document needs name, dirty, session")
    app.note_heartbeat(document)
    return {"ok": True}


def _ack(app, body) -> dict:
    kind = body.get("kind")
    ok = body.get("ok", True)
    detail = body.get("detail")
    if not isinstance(kind, str) or not kind:
        raise BadRequest("ack needs a decision kind")
    if not isinstance(ok, bool):
        raise BadRequest("ack ok must be a boolean")
    if detail is not None and not isinstance(detail, str):
        raise BadRequest("ack detail must be text")
    return app.ack_decision(kind, ok, detail)


def _restore(app, body) -> dict:
    version = body.get("version")
    if not isinstance(version, str) or not version.strip():
        raise BadRequest("restore needs a version id")
    return app.restore(version)


def _layers(app) -> dict:
    session = app.session
    if session is None:
        raise InvalidTransition("no session")
    if session.state == "rendering":
        raise InvalidTransition("rendering")
    if session.state == "failed":
        raise InvalidTransition("render failed")
    return {"layers": [{"part": layer.part, "png_path": str(layer.png_path)}
                       for layer in app.layers()]}


def _document_ok(document) -> bool:
    return (isinstance(document, dict)
            and isinstance(document.get("name"), str)
            and isinstance(document.get("dirty"), bool)
            and (document.get("session") is None
                 or isinstance(document.get("session"), str)))


def pending(ctx: Context) -> list[dict]:
    status, session, versions = ctx.status, ctx.status["session"], ctx.versions
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


def _pose_stage(phase, pose_age_ms) -> dict:
    if phase == "LOCKED":  # the pose stream is frozen by design, not lost
        return {"id": "pose", "status": "done", "detail": "frozen"}
    live = pose_age_ms is not None and pose_age_ms <= 2000
    return {"id": "pose", "status": "running" if live else "waiting",
            "detail": pose_age_ms}


def _render_stage(session) -> dict:
    if session is None:
        return {"id": "render", "status": "waiting", "detail": None}
    status = {"rendering": "running", "rendered": "done",
              "failed": "failed"}.get(session["state"], "waiting")
    return {"id": "render", "status": status, "detail": session["id"]}


def _archive_stage(versions) -> dict:
    return {"id": "archive", "status": "done" if versions else "waiting",
            "detail": len(versions)}
