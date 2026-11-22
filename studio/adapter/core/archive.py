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


def read_sidecar(kra_path: Path) -> dict[str, PoseRecord]:
    sidecar = _sidecar_path(kra_path)
    if not sidecar.exists():
        return {}
    try:
        payload = json.loads(sidecar.read_text(encoding="utf-8"))
    except json.JSONDecodeError as exc:
        raise ValueError(f"malformed sidecar {sidecar}: {exc}") from exc
    records = {}
    for pose_id, raw in payload.get("poses", {}).items():
        records[pose_id] = PoseRecord(
            pose=raw["pose"],
            resolution=tuple(raw["resolution"]),
            snapshot_id=raw["snapshot_id"],
            created=raw["created"],
        )
    return records


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
