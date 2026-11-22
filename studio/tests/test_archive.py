import zipfile

import pytest

from adapter.core.archive import (
    PoseRecord,
    inspect_kra,
    list_versions,
    read_sidecar,
    read_version,
    set_version_status,
    write_sidecar,
)

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


def make_version(tmp_path, name, created):
    kra = tmp_path / name
    kra.write_bytes(b"x")
    write_sidecar(kra, {"s1": PoseRecord(pose={}, resolution=(1600, 1200),
                                         snapshot_id="", created=created)})
    return kra


def test_set_version_status_roundtrip(tmp_path):
    kra = tmp_path / "pose_s1.kra"
    kra.write_bytes(b"x")
    write_sidecar(kra, {"s1": PoseRecord(pose={}, resolution=(1600, 1200), snapshot_id="", created="2026-11-23T00:00:00+00:00")})
    set_version_status(kra, "adopted")
    v = read_version(kra)
    assert v["status"] == "adopted" and "s1" in v["poses"]


def test_legacy_sidecar_defaults_to_pushed(tmp_path):
    # sidecar written by the old format (only "poses") reads as pushed/None
    kra = make_version(tmp_path, "pose_old.kra", "2026-11-22T10:00:00+00:00")
    v = read_version(kra)
    assert v["status"] == "pushed"
    assert v["restored_from"] is None
    assert set(v["poses"]) == {"s1"}


def test_set_version_status_restored_from(tmp_path):
    kra = make_version(tmp_path, "pose_r1.kra", "2026-11-23T03:00:00+00:00")
    set_version_status(kra, "pushed", restored_from="s1")
    v = read_version(kra)
    assert v["status"] == "pushed"
    assert v["restored_from"] == "s1"


def test_set_version_status_rejects_unknown(tmp_path):
    kra = make_version(tmp_path, "pose_s1.kra", "2026-11-23T00:00:00+00:00")
    with pytest.raises(ValueError):
        set_version_status(kra, "deleted")


def test_list_versions_sorted_and_skips_broken(tmp_path):
    # three sidecars, one malformed; expect two entries sorted by created
    make_version(tmp_path, "pose_b.kra", "2026-11-23T02:00:00+00:00")
    make_version(tmp_path, "pose_a.kra", "2026-11-23T01:00:00+00:00")
    (tmp_path / "broken.bridge.json").write_text("{not json", encoding="utf-8")
    versions = list_versions(tmp_path)
    assert [v["id"] for v in versions] == ["a", "b"]
    assert versions[0]["kra"] == tmp_path / "pose_a.kra"
    assert versions[0]["created"] == "2026-11-23T01:00:00+00:00"
    assert versions[0]["status"] == "pushed"
    assert versions[0]["restored_from"] is None
    assert list_versions(tmp_path / "missing") == []
