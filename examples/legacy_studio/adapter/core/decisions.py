"""Decision execution against the open Krita document.

The orchestrator must never claim an archive is adopted, or drop a session,
before the adapter has persisted the live canvas. These helpers are pure:
they take the Document protocol from adapter.core.kis and the archive path.
"""

from pathlib import Path

DECIDABLE = ("adopt", "re-render", "unlock")


def execute(kind: str, document, archive_path) -> tuple[bool, str | None]:
    """Persist painted work, then report whether the decision may proceed.

    A missing archive means the render never landed: there is nothing to
    preserve, so the decision proceeds without writing a partial document.
    """
    if kind not in DECIDABLE:
        return False, f"unknown decision {kind!r}"
    archive = Path(archive_path)
    if not archive.exists():
        return True, "no archive yet"
    if document is None:
        # Krita was restarted or the tab was closed: nothing unsaved exists,
        # so the archive on disk is the artifact the decision applies to.
        return True, "no open document"
    save_as = getattr(document, "saveAs", None)
    if callable(save_as):  # Krita 5 Document: save() takes no path
        save_as(str(archive))
    else:  # the Kis protocol's Document.save(path)
        document.save(archive)
    return True, None
