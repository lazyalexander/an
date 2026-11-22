from adapter.core.statusview import lock_enabled, parse_status


def test_parse_status_accepts_well_formed():
    parsed = parse_status({"phase": "ADJUSTMENT", "pose_age_ms": 42,
                           "session": None})
    assert parsed == {"phase": "ADJUSTMENT", "pose_age_ms": 42, "session": None}
    parsed = parse_status({"phase": "LOCKED", "pose_age_ms": None,
                           "session": {"id": "s-1", "state": "rendered"}})
    assert parsed["session"]["id"] == "s-1"


def test_parse_status_rejects_garbage():
    assert parse_status([1, 2, 3]) is None
    assert parse_status({"phase": "BOGUS"}) is None
    assert parse_status({"phase": "ADJUSTMENT", "pose_age_ms": "old"}) is None
    assert parse_status({"phase": "ADJUSTMENT", "pose_age_ms": True}) is None
    assert parse_status({"phase": "LOCKED", "session": "s-1"}) is None
    assert parse_status({"phase": "LOCKED", "session": {"state": "rendered"}}) is None


def test_lock_enabled_requires_adjustment_with_live_pose():
    assert lock_enabled("ADJUSTMENT", 42) is True
    assert lock_enabled("ADJUSTMENT", 2000) is True
    assert lock_enabled("ADJUSTMENT", 5000) is False  # stale stream
    assert lock_enabled("ADJUSTMENT", None) is False  # never streamed
    assert lock_enabled("LOCKED", 42) is False
