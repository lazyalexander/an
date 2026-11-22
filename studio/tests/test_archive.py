import zipfile

from adapter.core.archive import PoseRecord, inspect_kra, read_sidecar, write_sidecar

MINIMAL_MAINDOC = """<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE DOC PUBLIC '-//KDE//DTD krita 2.0//EN' 'http://www.calligra.org/DTD/krita-2.0.dtd'>
<DOC xmlns="http://www.calligra.org/DTD/krita" syntaxVersion="2.0" editor="Krita">
 <IMAGE height="1200" mime="application/x-kra" name="t" colorspacename="RGBA" width="1600">
  <layers>
   <layer visible="1" nodetype="paintlayer" filename="layer1" name="cube_a" locked="0"/>
   <layer visible="1" nodetype="grouplayer" filename="layer2" name="Model (protected)" locked="1">
    <layers>
     <layer visible="1" nodetype="paintlayer" filename="layer3" name="cube_b" locked="0"/>
    </layers>
   </layer>
  </layers>
 </IMAGE>
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
    assert info.locked_groups == frozenset({"Model (protected)"})


def test_sidecar_roundtrip(tmp_path):
    kra = tmp_path / "t.kra"
    kra.touch()
    rec = PoseRecord(pose={"matrix": [1.0] * 16, "location": [0, 0, 0]},
                     resolution=(1600, 1200), snapshot_id="snap-0001",
                     created="2026-11-22T10:00:00")
    sidecar = write_sidecar(kra, {"s-1": rec})
    assert sidecar.name == "t.bridge.json"
    assert read_sidecar(kra) == {"s-1": rec}
