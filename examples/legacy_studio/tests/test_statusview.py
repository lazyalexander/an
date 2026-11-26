from adapter.core.statusview import lock_enabled, parse_status


def test_parse_status_accepts_well_formed():
    parsed = parse_status({"phase": "ADJUSTMENT", "pose_age_ms": 42,
                           "session": None})
    assert parsed == {"phase": "ADJUSTMENT", "pose_age_ms": 42,
                      "session": None, "pending_restore": None,
                      "pending_decision": None}
    parsed = parse_status({"phase": "LOCKED", "pose_age_ms": None,
                           "session": {"id": "s-1", "state": "rendered"}})
    assert parsed["session"]["id"] == "s-1"


def test_parse_status_passes_pending_decision():
    base = {"phase": "LOCKED", "pose_age_ms": 5,
            "session": {"id": "s-1", "state": "rendered"}}
    parsed = parse_status({**base, "pending_decision": {
        "kind": "adopt", "session": "s-1"}})
    assert parsed["pending_decision"] == {"kind": "adopt", "session": "s-1"}
    for bad in ("adopt", 42, {"kind": "publish", "session": "s-1"},
                {"kind": "adopt"}, {"kind": "adopt", "session": 1}):
        parsed = parse_status({**base, "pending_decision": bad})
        assert parsed["pending_decision"] is None


def test_parse_status_rejects_garbage():
    assert parse_status([1, 2, 3]) is None
    assert parse_status({"phase": "BOGUS"}) is None
    assert parse_status({"phase": "ADJUSTMENT", "pose_age_ms": "old"}) is None
    assert parse_status({"phase": "ADJUSTMENT", "pose_age_ms": True}) is None
    assert parse_status({"phase": "LOCKED", "session": "s-1"}) is None
    assert parse_status({"phase": "LOCKED", "session": {"state": "rendered"}}) is None


def test_parse_status_passes_pending_restore():
    s = parse_status({"phase": "LOCKED", "pose_age_ms": 5,
                      "session": {"id": "x", "state": "rendered"},
                      "pending_restore": {"id": "r1", "kra": "/tmp/a.kra"}})
    assert s["pending_restore"]["id"] == "r1"
    assert s["pending_restore"]["kra"] == "/tmp/a.kra"


def test_parse_status_none_when_no_pending_restore():
    s = parse_status({"phase": "ADJUSTMENT", "pose_age_ms": 42,
                      "session": None})
    assert s["pending_restore"] is None
    s = parse_status({"phase": "ADJUSTMENT", "pose_age_ms": 42,
                      "session": None, "pending_restore": None})
    assert s["pending_restore"] is None


def test_parse_status_malformed_pending_restore_becomes_none():
    base = {"phase": "LOCKED", "pose_age_ms": None,
            "session": {"id": "x", "state": "rendered"}}
    for bad in ("r1", 42, ["r1"], {"id": "r1"}, {"kra": "/tmp/a.kra"},
                {"id": 1, "kra": "/tmp/a.kra"},
                {"id": "r1", "kra": None}):
        s = parse_status({**base, "pending_restore": bad})
        assert s is not None
        assert s["pending_restore"] is None


def test_lock_enabled_requires_adjustment_with_live_pose():
    assert lock_enabled("ADJUSTMENT", 42) is True
    assert lock_enabled("ADJUSTMENT", 2000) is True
    assert lock_enabled("ADJUSTMENT", 5000) is False  # stale stream
    assert lock_enabled("ADJUSTMENT", None) is False  # never streamed
    assert lock_enabled("LOCKED", 42) is False
