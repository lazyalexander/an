"""Bridge Studio instance: painting pipeline over the shared kernel.

Everything here is instance content. The framework (state builder, page,
server) reads `MANIFEST`, `stages(ctx)` and `pending(ctx)` and knows nothing
about imports, poses, renders or archives.
"""

from pathlib import Path

from ..context import Context
from ..health import adapter, pipeline, pose

MANIFEST = {
    "instance": "bridge-studio",
    "title": "Bridge Studio",
    "stages": [
        {"id": "import", "label": "导入", "kind": "headless"},
        {"id": "pose", "label": "位姿", "kind": "interaction", "surface": "adjust"},
        {"id": "render", "label": "渲染", "kind": "headless"},
        {"id": "archive", "label": "建档", "kind": "artifact"},
    ],
    "surfaces": [
        {"id": "adjust", "label": "调姿", "kind": "native", "app": "Blender",
         "phase": "ADJUSTMENT", "inputs": ["pointer"]},
        {"id": "paint", "label": "绘画", "kind": "native", "app": "Krita",
         "phase": "LOCKED", "inputs": ["pen", "decision"]},
    ],
    "decisions": [
        {"kind": "adopt_or_rerender", "label": "采用或重渲", "actions": [
            {"action": "adopt", "label": "采用", "style": "primary",
             "path": "/api/decision/adopt"},
            {"action": "rerender", "label": "重渲",
             "path": "/api/decision/rerender",
             "force_when_adapter_down": True},
        ]},
        {"kind": "retry", "label": "重试", "actions": [
            {"action": "retry", "label": "重试", "role": "retry",
             "path": "/api/decision/retry"},
        ]},
    ],
    "drawer": [
        {"id": "poses", "label": "位姿历史", "kinds": ["pose_stream"],
         "format": "summary"},
        {"id": "snapshots", "label": "快照",
         "kinds": ["file_drop", "import_done"], "format": "summary"},
        {"id": "failures", "label": "失败记录",
         "kinds": ["render_failed", "import_failed"], "format": "entry"},
    ],
    "restore": {"label": "恢复", "path": "/api/decision/restore"},
    "archive": {"label": "建档"},
    "document": {"dirty": "Krita 未保存", "saved": "Krita 已保存"},
    "pending_decision": "等待 Krita 保存…",
    "ports": {"input": ["pen", "decision"], "output": ["preview", "archive"]},
    "health": {"adapter": "Krita 未连接",
               "pipeline": "容器管线不可用",
               "pose": "位姿流已断开"},
}


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
            "pipeline": pipeline(ctx.app)}


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
