import json
from dataclasses import asdict
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import parse_qs, urlparse

from adapter.core.archive import list_versions, read_thumbnail

from .app import Orchestrator
from .ledger import Ledger
from .macro.instances import load as load_instance
from .macro.state import build_state
from .phase import InvalidTransition, NoPose
from .scenes import SceneStore

GATED_PREFIXES = ("/api/decision/", "/api/macro/", "/ui", "/static/")
STATIC_DIR = Path(__file__).parent / "macro" / "static"
STATIC_CONTENT_TYPES = {"index.html": "text/html",
                        "app.js": "text/javascript",
                        "style.css": "text/css"}


def _well_formed_document(document) -> bool:
    return (isinstance(document, dict)
            and isinstance(document.get("name"), str)
            and isinstance(document.get("dirty"), bool)
            and (document.get("session") is None
                 or isinstance(document.get("session"), str)))


def make_server(app: Orchestrator, host: str = "127.0.0.1", port: int = 8791,
                ui_token: str | None = None, archives_dir: Path | None = None,
                ledger: Ledger | None = None, scenes: SceneStore | None = None,
                preview_path: Path | None = None, instance=None) -> ThreadingHTTPServer:
    if instance is None:
        instance = load_instance("bridge-studio")
    if archives_dir is None:
        archives_dir = getattr(app, "_archives_dir", None)
    if ledger is None:
        ledger = getattr(app, "_ledger", None)
    if scenes is None:
        scenes = getattr(app, "_scenes", None)
    if preview_path is None:
        run_dir = getattr(app, "_run_dir", None)
        preview_path = (Path(run_dir) / "preview" / "current.png"
                        if run_dir is not None else None)

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, format, *args):  # the ledger is the record
            pass

        def _send(self, code: int, obj) -> None:
            body = json.dumps(obj).encode()
            self.send_response(code)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def _send_bytes(self, code: int, data: bytes, content_type: str) -> None:
            self.send_response(code)
            self.send_header("Content-Type", content_type)
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

        def _read_json(self) -> dict:
            length = int(self.headers.get("Content-Length") or 0)
            if length == 0:
                return {}
            return json.loads(self.rfile.read(length))

        def _authorized(self) -> bool:
            if ui_token is None:
                return True
            keys = parse_qs(urlparse(self.path).query).get("key", [])
            if keys and keys[0] == ui_token:
                return True
            return self.headers.get("X-Bridge-Key") == ui_token

        def do_POST(self) -> None:
            path = urlparse(self.path).path
            if path.startswith(GATED_PREFIXES) and not self._authorized():
                self._send(403, {"error": "bad key"})
                return
            if path == "/api/pose":
                self._pose()
            elif path == "/api/lock":
                self._lock()
            elif path == "/api/decision/unlock":
                self._decision(app.unlock, accepts_force=True)
            elif path == "/api/decision/adopt":
                self._decision(app.adopt)
            elif path == "/api/decision/rerender":
                self._decision(app.rerender, accepts_force=True)
            elif path == "/api/decision/retry":
                self._decision(app.retry)
            elif path == "/api/decision/restore":
                self._restore()
            elif path == "/api/decision/restore-ack":
                self._restore_ack()
            elif path == "/api/decision/ack":
                self._ack()
            elif path == "/api/macro/heartbeat":
                self._heartbeat()
            else:
                self._send(404, {"error": "not found"})

        def do_GET(self) -> None:
            parsed = urlparse(self.path)
            path = parsed.path
            if path.startswith(GATED_PREFIXES) and not self._authorized():
                self._send(403, {"error": "bad key"})
                return
            if path == "/api/status":
                self._send(200, app.status())
            elif path == "/api/session/layers":
                self._layers()
            elif path == "/api/macro/manifest":
                self._send(200, instance.MANIFEST)
            elif path == "/api/macro/state":
                self._state()
            elif path == "/api/macro/preview.png":
                self._preview_png()
            elif path == "/api/macro/archive-thumb":
                self._archive_thumb(parsed)
            elif path == "/ui":
                self._static("index.html")
            elif path.startswith("/static/"):
                self._static(path[len("/static/"):])
            else:
                self._send(404, {"error": "not found"})

        def _state(self) -> None:
            self._send(200, build_state(app, archives_dir, ledger, scenes,
                                        instance=instance))

        def _static(self, name: str) -> None:
            content_type = STATIC_CONTENT_TYPES.get(name)
            if content_type is None:
                self._send(404, {"error": "not found"})
                return
            self._send_bytes(200, (STATIC_DIR / name).read_bytes(), content_type)

        def _preview_png(self) -> None:
            if preview_path is None or not Path(preview_path).is_file():
                self._send(404, {"error": "no preview"})
                return
            self._send_bytes(200, Path(preview_path).read_bytes(), "image/png")

        def _archive_thumb(self, parsed) -> None:
            ids = parse_qs(parsed.query).get("id", [])
            if not ids or not ids[0]:
                self._send(400, {"error": "missing id"})
                return
            version = next((v for v in list_versions(archives_dir)
                            if v["id"] == ids[0]), None)
            if version is None:
                self._send(404, {"error": "unknown version"})
                return
            data = read_thumbnail(version["kra"])
            if data is None:
                self._send(404, {"error": "no thumbnail"})
                return
            self._send_bytes(200, data, "image/png")

        def _pose(self) -> None:
            try:
                body = self._read_json()
            except json.JSONDecodeError:
                self._send(400, {"error": "invalid json"})
                return
            matrix, location = body.get("matrix"), body.get("location")
            numbers = lambda seq: all(isinstance(v, (int, float)) for v in seq)
            if (not isinstance(matrix, list) or len(matrix) != 16 or not numbers(matrix)
                    or not isinstance(location, list) or len(location) != 3
                    or not numbers(location)):
                self._send(400, {"error": "pose needs matrix[16] and location[3] of numbers"})
                return
            try:
                app.submit_pose({"matrix": [float(v) for v in matrix],
                                 "location": [float(v) for v in location]})
            except InvalidTransition as exc:
                self._send(409, {"error": str(exc)})
                return
            self._send(200, {"ok": True})

        def _lock(self) -> None:
            try:
                session = app.lock()
            except NoPose:
                self._send(409, {"error": "no current pose"})
                return
            except InvalidTransition as exc:
                self._send(409, {"error": str(exc)})
                return
            self._send(200, asdict(session))

        def _decision(self, action, accepts_force: bool = False) -> None:
            try:
                body = self._read_json()
            except json.JSONDecodeError:
                self._send(400, {"error": "invalid json"})
                return
            force = body.get("force", False)
            if not isinstance(force, bool):
                self._send(400, {"error": "force must be a boolean"})
                return
            try:
                result = action(force=force) if accepts_force else action()
            except (InvalidTransition, ValueError) as exc:
                self._send(409, {"error": str(exc)})
                return
            self._send(200, result or {"ok": True})

        def _ack(self) -> None:
            try:
                body = self._read_json()
            except json.JSONDecodeError:
                self._send(400, {"error": "invalid json"})
                return
            kind = body.get("kind")
            ok = body.get("ok", True)
            detail = body.get("detail")
            if not isinstance(kind, str) or not kind:
                self._send(400, {"error": "ack needs a decision kind"})
                return
            if not isinstance(ok, bool):
                self._send(400, {"error": "ack ok must be a boolean"})
                return
            if detail is not None and not isinstance(detail, str):
                self._send(400, {"error": "ack detail must be text"})
                return
            try:
                result = app.ack_decision(kind, ok, detail)
            except (InvalidTransition, ValueError) as exc:
                self._send(409, {"error": str(exc)})
                return
            self._send(200, result)

        def _restore(self) -> None:
            try:
                body = self._read_json()
            except json.JSONDecodeError:
                self._send(400, {"error": "invalid json"})
                return
            version = body.get("version")
            if not isinstance(version, str) or not version.strip():
                self._send(400, {"error": "restore needs a version id"})
                return
            try:
                pending = app.restore(version)
            except (InvalidTransition, ValueError) as exc:
                self._send(409, {"error": str(exc)})
                return
            self._send(200, pending)

        def _restore_ack(self) -> None:
            try:
                self._read_json()
            except json.JSONDecodeError:
                self._send(400, {"error": "invalid json"})
                return
            app.ack_restore()
            self._send(200, {"ok": True})

        def _heartbeat(self) -> None:
            try:
                body = self._read_json()
            except json.JSONDecodeError:
                self._send(400, {"error": "invalid json"})
                return
            document = body.get("document")
            if document is not None and not _well_formed_document(document):
                self._send(400, {"error": "document needs name, dirty, session"})
                return
            app.note_heartbeat(document)
            self._send(200, {"ok": True})

        def _layers(self) -> None:
            session = app.session
            if session is None:
                self._send(409, {"error": "no session"})
                return
            if session.state == "rendering":
                self._send(409, {"error": "rendering"})
                return
            if session.state == "failed":
                self._send(409, {"error": "render failed"})
                return
            layers = app.layers()
            self._send(200, {"layers": [{"part": l.part, "png_path": str(l.png_path)}
                                        for l in layers]})

    return ThreadingHTTPServer((host, port), Handler)
