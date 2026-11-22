import zipfile

from adapter.core.archive import PoseRecord, inspect_kra, read_sidecar, write_sidecar

MINIMAL_MAINDOC = """<?xml version="1.0" encoding="UTF-8"?>
<DOC image-width="1600" image-height="1200" name="t">
 <layers>
  <layer name="cube_a" node-type="paintlayer"/>
  <layer name="Model (protected)" node-type="grouplayer">
   <layer name="cube_b" node-type="paintlayer"/>
  </layer>
 </layers>
</DOC>"""


def make_kra(path):
    with zipfile.ZipFile(path, "w") as z:
        z.writestr("maindoc.xml", MINIMAL_MAINDOC)


def test_inspect_kra(tmp_path):
    kra = tmp_path / "t.kra"
    make_kra(kra)
    info = inspect_kra(kra)
    assert (info.width, info.height) == (1600, 1200)
    assert info.groups == {"": ["cube_a"], "Model (protected)": ["cube_b"]}


def test_sidecar_roundtrip(tmp_path):
    kra = tmp_path / "t.kra"
    kra.touch()
    rec = PoseRecord(pose={"matrix": [1.0] * 16, "location": [0, 0, 0]},
                     resolution=(1600, 1200), snapshot_id="snap-0001",
                     created="2026-11-22T10:00:00")
    sidecar = write_sidecar(kra, {"s-1": rec})
    assert sidecar.name == "t.bridge.json"
    assert read_sidecar(kra) == {"s-1": rec}
