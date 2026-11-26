"""Pure status shaping for the Krita docker shim.

Validates the orchestrator's /api/status payload before the shim indexes
into it, and owns the Lock-enable rule so "live pose" is testable outside
Krita.
"""

LIVE_POSE_MAX_AGE_MS = 2000
DECISION_KINDS = frozenset({"adopt", "re-render", "unlock"})


def parse_status(data):
    """Return the well-formed subset of a status payload, or None."""
    if not isinstance(data, dict):
        return None
    phase = data.get("phase")
    if phase not in ("ADJUSTMENT", "LOCKED"):
        return None
    age = data.get("pose_age_ms")
    if age is not None and (isinstance(age, bool)
                            or not isinstance(age, (int, float)) or age < 0):
        return None
    session = data.get("session")
    if session is not None:
        if not isinstance(session, dict):
            return None
        if not isinstance(session.get("id"), str):
            return None
        if not isinstance(session.get("state"), str):
            return None
    pending = data.get("pending_restore")
    if pending is not None and not _well_formed_pending(pending):
        pending = None
    decision = data.get("pending_decision")
    if decision is not None and not _well_formed_decision(decision):
        decision = None
    return {"phase": phase, "pose_age_ms": age, "session": session,
            "pending_restore": pending, "pending_decision": decision}


def _well_formed_pending(pending):
    return (isinstance(pending, dict)
            and isinstance(pending.get("id"), str)
            and isinstance(pending.get("kra"), str))


def _well_formed_decision(decision):
    return (isinstance(decision, dict)
            and decision.get("kind") in DECISION_KINDS
            and isinstance(decision.get("session"), str))


def lock_enabled(phase, pose_age_ms, max_age_ms: int = LIVE_POSE_MAX_AGE_MS) -> bool:
    return (phase == "ADJUSTMENT" and pose_age_ms is not None
            and pose_age_ms <= max_age_ms)
