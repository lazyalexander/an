"""Read-only snapshot handed to an instance provider each poll.

Framework owns this shape; instances only read it.
"""

from dataclasses import dataclass
from typing import Any


@dataclass(frozen=True)
class Context:
    app: Any            # Orchestrator
    status: dict        # app.status()
    entries: list[dict]  # ledger.read_all()
    versions: list[dict]  # adapter.core.archive.list_versions()
    scenes: Any         # SceneStore | None
    now: float
