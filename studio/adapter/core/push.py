import pathlib
from datetime import datetime, timezone

from orchestrator.pipeline_api import LayerSpec
from pipeline import pngutil

from .archive import PoseRecord, write_sidecar
from .kis import Kis
from .layout import MODEL_GROUP, layer_name, pose_group_name


def push_session(kis: Kis, session: dict, layers: list[LayerSpec],
                 archives_dir: pathlib.Path) -> pathlib.Path:
    """Build the Krita document for a rendered session and save its archive."""
    width, height = session["resolution"]
    archives_dir = pathlib.Path(archives_dir)
    kra = archives_dir / f"pose_{session['id']}.kra"
    if kra.exists():
        raise FileExistsError(
            f"archive {kra} already exists; keeping the user's document")
    for layer in layers:
        size = pngutil.read_size(layer.png_path)
        if size != (width, height):
            raise ValueError(
                f"layer {layer.part} is {size[0]}x{size[1]}, "
                f"session is {width}x{height}; never rescale")
    doc = kis.create_document(width, height, f"Bridge — {session['id']}")
    model = doc.group(MODEL_GROUP)
    for layer in layers:  # list order, first part bottom
        doc.layer_from_png(layer_name(layer.part), model, layer.png_path)
    doc.set_locked(model, True)
    doc.group(pose_group_name(session["id"]))
    archives_dir.mkdir(parents=True, exist_ok=True)
    doc.save(kra)
    record = PoseRecord(
        pose=session["pose"],
        resolution=(width, height),
        snapshot_id=session.get("snapshot_id") or "",
        created=datetime.now(timezone.utc).isoformat(timespec="seconds"),
    )
    write_sidecar(kra, {session["id"]: record})
    return kra
