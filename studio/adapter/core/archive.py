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
    with zipfile.ZipFile(Path(kra_path)) as archive:
        maindoc = archive.read("maindoc.xml")
    root = ElementTree.fromstring(maindoc)
    width = int(root.attrib["image-width"])
    height = int(root.attrib["image-height"])

    groups: dict[str, list[str]] = {"": []}
    layers_el = root.find("layers")

    def walk(parent_el, group_name: str) -> None:
        for layer in parent_el.findall("layer"):
            node_type = layer.attrib.get("node-type", "")
            name = layer.attrib.get("name", "")
            if node_type == "grouplayer":
                groups.setdefault(name, [])
                walk(layer, name)
            elif node_type == "paintlayer":
                groups.setdefault(group_name, []).append(name)

    if layers_el is not None:
        walk(layers_el, "")
    return KraInfo(width=width, height=height, groups=groups)
