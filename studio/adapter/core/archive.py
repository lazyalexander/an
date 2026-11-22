import json
import zipfile
from dataclasses import asdict, dataclass
from pathlib import Path
from xml.etree import ElementTree


@dataclass
class PoseRecord:
    pose: dict
    resolution: tuple[int, int]
    snapshot_id: str
    created: str


@dataclass
class KraInfo:
    width: int
    height: int
    groups: dict[str, list[str]]  # group-layer name -> direct child layer names; "" = top level
    locked_groups: frozenset[str]  # group-layer names with locked="1"


def _local(tag: str) -> str:
    return tag.rsplit("}", 1)[-1]


def _sidecar_path(kra_path: Path) -> Path:
    kra_path = Path(kra_path)
    return kra_path.with_suffix("").with_name(kra_path.stem + ".bridge.json")


def write_sidecar(kra_path: Path, poses: dict[str, PoseRecord]) -> Path:
    sidecar = _sidecar_path(kra_path)
    payload = {
        "poses": {
            pose_id: {**asdict(record), "resolution": list(record.resolution)}
            for pose_id, record in poses.items()
        }
    }
    sidecar.write_text(json.dumps(payload, ensure_ascii=False, indent=2), encoding="utf-8")
    return sidecar


VALID_STATUSES = frozenset({"pushed", "adopted", "superseded"})


def _load_payload(sidecar: Path) -> dict:
    try:
        return json.loads(sidecar.read_text(encoding="utf-8"))
    except json.JSONDecodeError as exc:
        raise ValueError(f"malformed sidecar {sidecar}: {exc}") from exc


def _parse_poses(payload: dict) -> dict[str, PoseRecord]:
    records = {}
    for pose_id, raw in payload.get("poses", {}).items():
        records[pose_id] = PoseRecord(
            pose=raw["pose"],
            resolution=tuple(raw["resolution"]),
            snapshot_id=raw["snapshot_id"],
            created=raw["created"],
        )
    return records


def read_sidecar(kra_path: Path) -> dict[str, PoseRecord]:
    sidecar = _sidecar_path(kra_path)
    if not sidecar.exists():
        return {}
    return _parse_poses(_load_payload(sidecar))


def read_version(kra_path: Path) -> dict:
    sidecar = _sidecar_path(kra_path)
    if not sidecar.exists():
        return {"poses": {}, "status": "pushed", "restored_from": None}
    payload = _load_payload(sidecar)
    return {
        "poses": _parse_poses(payload),
        "status": payload.get("status", "pushed"),
        "restored_from": payload.get("restored_from"),
    }


def set_version_status(kra_path: Path, status: str, restored_from: str | None = None) -> None:
    if status not in VALID_STATUSES:
        raise ValueError(f"unknown version status {status!r}; expected one of {sorted(VALID_STATUSES)}")
    sidecar = _sidecar_path(kra_path)
    payload = _load_payload(sidecar)
    payload["status"] = status
    payload["restored_from"] = restored_from
    sidecar.write_text(json.dumps(payload, ensure_ascii=False, indent=2), encoding="utf-8")


def list_versions(archives_dir: Path) -> list[dict]:
    archives_dir = Path(archives_dir)
    if not archives_dir.is_dir():
        return []
    versions = []
    for sidecar in archives_dir.glob("*.bridge.json"):
        try:
            payload = _load_payload(sidecar)
        except (OSError, ValueError):
            continue
        base = sidecar.name[: -len(".bridge.json")]
        poses = payload.get("poses", {})
        created = min((raw.get("created", "") for raw in poses.values()), default="")
        versions.append({
            "id": base.removeprefix("pose_"),
            "kra": sidecar.with_name(base + ".kra"),
            "created": created,
            "status": payload.get("status", "pushed"),
            "restored_from": payload.get("restored_from"),
        })
    versions.sort(key=lambda version: version["created"])
    return versions


def inspect_kra(kra_path: Path) -> KraInfo:
    """Parse a real Krita maindoc.xml (DOC > IMAGE > layers tree, any xmlns)."""
    with zipfile.ZipFile(Path(kra_path)) as archive:
        maindoc = archive.read("maindoc.xml")
    doc = ElementTree.fromstring(maindoc)
    image = next((el for el in doc.iter() if _local(el.tag) == "IMAGE"), None)
    if image is None:
        raise ValueError("maindoc.xml has no IMAGE element")
    width = int(image.attrib["width"])
    height = int(image.attrib["height"])

    groups: dict[str, list[str]] = {"": []}
    locked_groups: set[str] = set()
    layers_el = next((c for c in image if _local(c.tag) == "layers"), None)

    def walk(parent_el, group_name: str) -> None:
        for layer in parent_el:
            if _local(layer.tag) != "layer":
                continue
            node_type = layer.attrib.get("nodetype", "")
            name = layer.attrib.get("name", "")
            if node_type == "grouplayer":
                groups.setdefault(name, [])
                if layer.attrib.get("locked") == "1":
                    locked_groups.add(name)
                children = next((c for c in layer if _local(c.tag) == "layers"), None)
                if children is not None:
                    walk(children, name)
            elif node_type == "paintlayer":
                groups.setdefault(group_name, []).append(name)

    if layers_el is not None:
        walk(layers_el, "")
    return KraInfo(width=width, height=height, groups=groups,
                   locked_groups=frozenset(locked_groups))
