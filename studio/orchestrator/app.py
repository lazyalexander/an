import threading
import time
import uuid
from collections.abc import Callable
from dataclasses import asdict
from pathlib import Path

from .ledger import Ledger
from .phase import InvalidTransition, NoPose, Phase
from .pipeline_api import LayerSpec, Pipeline, Session

DEFAULT_RUN_DIR = Path(__file__).resolve().parents[1] / "run"
POSE_WINDOW_S = 1.0


class Orchestrator:
    def __init__(self, ledger: Ledger, pipeline: Pipeline, resolution: tuple[int, int],
                 now: Callable[[], float] = time.time, run_dir: Path = DEFAULT_RUN_DIR):
        self._ledger = ledger
        self._pipeline = pipeline
        self._resolution = tuple(resolution)
        self._now = now
        self._run_dir = Path(run_dir)
        self._guard = threading.Lock()
        self._phase = Phase.ADJUSTMENT
        self._session: Session | None = None
        self._layers: list[LayerSpec] | None = None
        self._current_pose: dict | None = None
        self._pose_ts: float | None = None
        self._pose_window_start: float | None = None
        self._pose_window_count = 0

    @property
    def phase(self) -> Phase:
        with self._guard:
            return self._phase

    @property
    def session(self) -> Session | None:
        with self._guard:
            return self._session

    def submit_pose(self, pose: dict) -> None:
        with self._guard:
            if self._phase is Phase.LOCKED:
                raise InvalidTransition("locked: pose stream is frozen for the session")
            t = self._now()
            if self._pose_window_start is None or t - self._pose_window_start >= POSE_WINDOW_S:
                self._flush_pose_window_locked()
                self._pose_window_start = t
            self._pose_window_count += 1
            self._current_pose = pose
            self._pose_ts = t

    def lock(self) -> Session:
        with self._guard:
            if self._phase is Phase.LOCKED:
                raise InvalidTransition("already locked")
            if self._current_pose is None:
                raise NoPose("no current pose")
            self._flush_pose_window_locked()
            session = Session(
                id=time.strftime("%Y%m%d-%H%M%S") + "-" + uuid.uuid4().hex[:6],
                pose=dict(self._current_pose),
                resolution=self._resolution,
                snapshot_id="",
                state="rendering",
            )
            self._session = session
            self._layers = None
            self._phase = Phase.LOCKED
            self._ledger.record("lock", f"session {session.id} frozen at streamed pose",
                                session=session.id)
        thread = threading.Thread(target=self._render, args=(session,), daemon=True)
        thread.start()
        return session

    def unlock(self) -> None:
        with self._guard:
            if self._phase is not Phase.LOCKED:
                raise InvalidTransition("not locked")
            session_id = self._session.id if self._session else ""
            self._phase = Phase.ADJUSTMENT
            self._session = None
            self._layers = None
            self._ledger.record("unlock", f"session {session_id} released", session=session_id)

    def layers(self) -> list[LayerSpec]:
        with self._guard:
            if self._session is None or self._session.state != "rendered":
                raise InvalidTransition("no rendered session")
            return list(self._layers or [])

    def status(self) -> dict:
        with self._guard:
            pose_age_ms = None
            if self._pose_ts is not None:
                pose_age_ms = int((self._now() - self._pose_ts) * 1000)
            return {
                "phase": self._phase.name,
                "pose_age_ms": pose_age_ms,
                "session": asdict(self._session) if self._session else None,
                "snapshot_id": None,
                "roster": {"adjustment": "Blender", "locked": "Krita"},
            }

    def _flush_pose_window_locked(self) -> None:
        if self._pose_window_count:
            self._ledger.record("pose_stream",
                                f"{self._pose_window_count} poses in 1s window",
                                count=self._pose_window_count, window_ms=1000)
            self._pose_window_count = 0

    def _render(self, session: Session) -> None:
        out_dir = self._run_dir / "sessions" / session.id / "layers"
        scene_dir = self._run_dir / "scene"
        try:
            out_dir.mkdir(parents=True, exist_ok=True)
            t0 = time.monotonic()
            layers = self._pipeline.render(session, scene_dir, out_dir)
            latency_ms = (time.monotonic() - t0) * 1000
            nbytes = sum(l.png_path.stat().st_size for l in layers if l.png_path.exists())
            with self._guard:
                self._layers = layers
                session.state = "rendered"
            self._ledger.record("render_done",
                                f"session {session.id}: {len(layers)} layers",
                                bytes_moved=nbytes, latency_ms=latency_ms,
                                session=session.id)
        except Exception as exc:  # failure is data
            with self._guard:
                session.state = "failed"
            self._ledger.record("render_failed", f"session {session.id}: {exc}",
                                session=session.id)
