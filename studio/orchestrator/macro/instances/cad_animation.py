"""CAD → Animation instance: a second, deliberately fake instance.

It exists to prove the framework is instance-agnostic: new stage ids, new
surface names, new decision kinds, new drawer sections and new health copy —
with no change to the state builder, the server or the page.
"""

from ..context import Context
from ..health import adapter, pipeline

MANIFEST = {
    "instance": "cad-animation",
    "title": "CAD → 动画",
    "stages": [
        {"id": "import", "label": "导入 CAD", "kind": "headless"},
        {"id": "rig", "label": "绑定", "kind": "headless"},
        {"id": "animate", "label": "动画", "kind": "interaction", "surface": "stage"},
        {"id": "export", "label": "导出", "kind": "artifact"},
    ],
    "surfaces": [
        {"id": "stage", "label": "动画台", "kind": "pixels",
         "phase": "ADJUSTMENT", "inputs": ["pointer", "decision"]},
        {"id": "review", "label": "审片", "kind": "pixels",
         "phase": "LOCKED", "inputs": ["decision"]},
    ],
    "decisions": [
        {"kind": "approve_rig", "label": "绑定结果", "actions": [
            {"action": "approve_rig", "label": "通过"},
        ]},
    ],
    "drawer": [
        {"id": "imports", "label": "导入记录",
         "kinds": ["file_drop", "import_done"], "format": "summary"},
        {"id": "failures", "label": "失败记录",
         "kinds": ["import_failed", "render_failed"], "format": "entry"},
    ],
    "ports": {"input": ["pointer", "decision"], "output": ["preview", "archive"]},
    "archive": {"label": "版本"},
    "document": {"dirty": "预览未保存", "saved": "预览已保存"},
    "pending_decision": "等待适配器保存…",
    "health": {"adapter": "审片器未连接",
               "pipeline": "容器管线不可用"},
}


def health(ctx: Context) -> dict:
    return {"adapter": adapter(ctx.app), "pipeline": pipeline(ctx.app)}


def actions(app) -> dict:
    return {"approve_rig": lambda body: _approve_rig(app)}


def inputs(app) -> dict:
    return {"pointer": lambda body: {"ok": True}}


def reads(app) -> dict:
    return {"timeline": lambda query: {"frames": []}}


def thumbnail(app, version_id: str):
    return None


def _approve_rig(app) -> dict:
    app.ledger.record("rig_approved", "rig approved by the user")
    return {"ok": True}


def stages(ctx: Context) -> list[dict]:
    imported = _import_stage(ctx.entries, ctx.scenes)
    rigged = _has(ctx.entries, "rig_done")
    approved = _has(ctx.entries, "rig_approved")
    animated = _has(ctx.entries, "animation_done")
    exported = _has(ctx.entries, "export_done")
    return [
        imported,
        {"id": "rig",
         "status": "done" if approved else ("running" if rigged else "waiting"),
         "detail": "awaiting approval" if rigged and not approved else None},
        {"id": "animate",
         "status": "done" if animated else ("running" if approved else "waiting"),
         "detail": None},
        {"id": "export", "status": "done" if exported else "waiting",
         "detail": None},
    ]


def pending(ctx: Context) -> list[dict]:
    if _has(ctx.entries, "rig_done") and not _has(ctx.entries, "rig_approved"):
        return [{"kind": "approve_rig"}]
    return []


def _has(entries, kind) -> bool:
    return any(entry["kind"] == kind for entry in entries)


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
