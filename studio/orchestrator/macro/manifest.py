"""Default instance manifest (Bridge Studio).

Kept as an import alias for callers that predate the instance registry; the
authority is `orchestrator.macro.instances.bridge`.
"""

from .instances.bridge import MANIFEST

__all__ = ["MANIFEST"]
