import shutil
import threading
import time
import uuid
from collections.abc import Callable
from dataclasses import asdict
from pathlib import Path

from adapter.core.archive import list_versions, read_version, set_version_status

from .ledger import Ledger
from .phase import InvalidTransition, NoPose, Phase
from .pipeline_api import LayerSpec, Pipeline, Session

DEFAULT_RUN_DIR = Path(__file__).resolve().parents[1] / "run"
POSE_WINDOW_S = 1.0


class Orchestrator:
    def __init__(self, ledger: Ledger, pipeline: Pipeline, resolution: tuple[int, int],
                 now: Callable[[], float] = time.time, run_dir: Path = DEFAULT_RUN_DIR,
                 scenes=None, archives_dir: Path | None = None):
        self._ledger = ledger
        self._pipeline = pipeline
        self._resolution = tuple(resolution)
        self._now = now
        self._run_dir = Path(run_dir)
        self._archives_dir = (Path(archives_dir) if archives_dir is not None
                              else self._run_dir / "archives")
        self._scenes = scenes
        self._guard = threading.Lock()
        self._phase = Phase.ADJUSTMENT
        self._session: Session | None = None
        self._layers: list[LayerSpec] | None = None
        self._current_pose: dict | None = None
        self._pose_ts: float | None = None
        self._pose_window_start: float | None = None
        self._pose_window_count = 0
        self._lock_ts: float | None = None
        self._render_done_ts: float | None = None
        self._pending_restore: dict | None = None
        self._last_heartbeat_ts: float | None = None

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
            snapshot_id = ""
            if self._scenes is not None:
                if self._scenes.current is None:
                    raise InvalidTransition("no scene imported")
                snapshot_id = self._scenes.current.id
            self._flush_pose_window_locked()
            session = Session(
                id=time.strftime("%Y%m%d-%H%M%S") + "-" + uuid.uuid4().hex[:6],
                pose=dict(self._current_pose),
                resolution=self._resolution,
                snapshot_id=snapshot_id,
                state="rendering",
            )
            self._session = session
            self._layers = None
            self._phase = Phase.LOCKED
            self._lock_ts = self._now()
            self._render_done_ts = None
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
            lock_duration_ms = self._lock_duration_ms_locked()
            self._phase = Phase.ADJUSTMENT
            self._session = None
            self._layers = None
            self._lock_ts = None
            self._ledger.record("unlock", f"session {session_id} released",
                                session=session_id, lock_duration_ms=lock_duration_ms)

    def adopt(self) -> None:
        with self._guard:
            if self._phase is not Phase.LOCKED or self._session is None:
                raise InvalidTransition("not locked")
            session = self._session
            if session.state != "rendered":
                raise InvalidTransition(
                    f"session {session.id} is {session.state}, not rendered")
            kra = self._archives_dir / f"pose_{session.id}.kra"
            if not kra.exists():
                raise InvalidTransition("archive not pushed yet")
            status = read_version(kra)["status"]
            if status != "pushed":
                raise InvalidTransition(f"archive already {status}")
            set_version_status(kra, "adopted")
            self._ledger.record("decision", f"session {session.id} adopted",
                                latency_ms=self._decision_latency_ms_locked(),
                                decision="adopt", session=session.id)

    def rerender(self) -> None:
        with self._guard:
            if self._phase is not Phase.LOCKED:
                raise InvalidTransition("not locked")
            session_id = self._session.id if self._session else ""
            if self._session is not None:
                kra = self._archives_dir / f"pose_{self._session.id}.kra"
                if kra.exists():
                    set_version_status(kra, "superseded")
            latency_ms = self._decision_latency_ms_locked()
            lock_duration_ms = self._lock_duration_ms_locked()
            self._phase = Phase.ADJUSTMENT
            self._session = None
            self._layers = None
            self._lock_ts = None
            self._ledger.record("decision",
                                f"session {session_id} re-render: back to adjustment",
                                latency_ms=latency_ms, decision="re-render",
                                session=session_id, lock_duration_ms=lock_duration_ms)

    def retry(self) -> None:
        with self._guard:
            if self._phase is not Phase.LOCKED or self._session is None:
                raise InvalidTransition("not locked")
            session = self._session
            if session.state != "failed":
                raise InvalidTransition(
                    f"session {session.id} is {session.state}, not failed")
            session.state = "rendering"
            self._render_done_ts = None
            self._ledger.record("render_retry",
                                f"session {session.id}: retrying render",
                                session=session.id)
        thread = threading.Thread(target=self._render, args=(session,), daemon=True)
        thread.start()

    def restore(self, version_id: str) -> dict:
        with self._guard:
            versions = {v["id"]: v for v in list_versions(self._archives_dir)}
            version = versions.get(version_id)
            if version is None:
                raise InvalidTransition(f"unknown version {version_id!r}")
            src_kra = Path(version["kra"])
            if not src_kra.exists():
                raise InvalidTransition(f"version {version_id!r} has no .kra file")
            stamp = time.strftime("%Y%m%d-%H%M%S", time.localtime(self._now()))
            new_id = f"{version_id}--restore-{stamp}"
            new_kra = self._archives_dir / f"pose_{new_id}.kra"
            src_sidecar = src_kra.with_name(src_kra.stem + ".bridge.json")
            new_sidecar = new_kra.with_name(new_kra.stem + ".bridge.json")
            shutil.copy2(src_kra, new_kra)
            shutil.copy2(src_sidecar, new_sidecar)
            set_version_status(new_kra, "pushed", restored_from=version_id)
            self._pending_restore = {"id": new_id, "kra": str(new_kra)}
            self._ledger.record("decision",
                                f"restore {version_id} as {new_id}",
                                decision="restore", restored_from=version_id,
                                version=new_id)
            return dict(self._pending_restore)

    @property
    def pending_restore(self) -> dict | None:
        with self._guard:
            return dict(self._pending_restore) if self._pending_restore is not None else None

    def ack_restore(self) -> None:
        with self._guard:
            self._pending_restore = None

    def note_heartbeat(self) -> None:
        with self._guard:
            self._last_heartbeat_ts = self._now()

    def heartbeat_age_ms(self) -> int | None:
        with self._guard:
            if self._last_heartbeat_ts is None:
                return None
            return int((self._now() - self._last_heartbeat_ts) * 1000)

    def _lock_duration_ms_locked(self) -> int:
        if self._lock_ts is None:
            return 0
        return int((self._now() - self._lock_ts) * 1000)

    def _decision_latency_ms_locked(self) -> int:
        # render-done time unknown (e.g. orchestrator restarted): latency is
        # observability data, not correctness — record 0 rather than refuse.
        if self._render_done_ts is None:
            return 0
        return int((self._now() - self._render_done_ts) * 1000)

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
                "snapshot_id": (self._scenes.current.id
                                if self._scenes is not None and self._scenes.current
                                else None),
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
        if self._scenes is not None and session.snapshot_id:
            scene_dir = self._scenes.root / session.snapshot_id
        else:
            scene_dir = self._run_dir / "scene"
        try:
            out_dir.mkdir(parents=True, exist_ok=True)
            t0 = time.monotonic()
            layers = self._pipeline.render(session, scene_dir, out_dir)
            latency_ms = (time.monotonic() - t0) * 1000
            nbytes = sum(l.png_path.stat().st_size for l in layers if l.png_path.exists())
            with self._guard:
                if self._session is not session:
                    orphaned = True
                else:
                    orphaned = False
                    self._layers = layers
                    session.state = "rendered"
                    self._render_done_ts = self._now()
            if orphaned:  # unlock/re-lock superseded this session mid-render
                self._ledger.record(
                    "render_orphaned",
                    f"session {session.id}: superseded before render finished",
                    bytes_moved=nbytes, latency_ms=latency_ms, session=session.id)
                return
            self._ledger.record("render_done",
                                f"session {session.id}: {len(layers)} layers",
                                bytes_moved=nbytes, latency_ms=latency_ms,
                                session=session.id)
        except Exception as exc:  # failure is data
            with self._guard:
                session.state = "failed"
            self._ledger.record("render_failed", f"session {session.id}: {exc}",
                                session=session.id)
