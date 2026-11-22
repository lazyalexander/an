"""Instance manifest: the only place Bridge-specific labels live."""

MANIFEST = {
    "instance": "bridge-studio",
    "title": "Bridge Studio",
    "stages": [{"id": "import", "label": "导入"},
               {"id": "pose", "label": "位姿"},
               {"id": "render", "label": "渲染"},
               {"id": "archive", "label": "建档"}],
    "drawer": [{"id": "poses", "label": "位姿历史"},
               {"id": "snapshots", "label": "快照"},
               {"id": "failures", "label": "失败记录"}],
}
