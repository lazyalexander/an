"""Framework health probes shared by instance providers.

Each instance chooses which channels it has; the page renders whatever the
provider returns, with copy from the instance manifest.
"""

from ..app import ADAPTER_LIVE_MS

POSE_LIVE_MS = 2000


def adapter(app) -> dict:
    age = app.heartbeat_age_ms()
    return {"ok": age is not None and age <= ADAPTER_LIVE_MS, "stale_ms": age}


def pose(status) -> dict:
    age = status["pose_age_ms"]
    # while LOCKED the pose stream is frozen by design — stillness is not
    # a disconnect, so it must not raise the banner
    ok = (status["phase"] == "LOCKED"
          or (age is not None and age <= POSE_LIVE_MS))
    return {"ok": ok}


def pipeline(app) -> dict:
    probe = getattr(getattr(app, "_pipeline", None), "health", None)
    if probe is None:
        return {"ok": True, "detail": "unmonitored"}
    try:
        return probe()
    except Exception as exc:  # noqa: BLE001 — failure is data, like app._render:
        # a health probe must degrade the poll, never break it
        return {"ok": False, "detail": str(exc)}
