import json
from dataclasses import asdict
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlparse

from .app import Orchestrator
from .phase import InvalidTransition, NoPose

GATED_PREFIXES = ("/api/decision/", "/api/macro/")


def make_server(app: Orchestrator, host: str = "127.0.0.1", port: int = 8791,
                ui_token: str | None = None) -> ThreadingHTTPServer:

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
            elif path == "/api/unlock":
                self._unlock()
            elif path == "/api/decision/adopt":
                self._decision(app.adopt)
            elif path == "/api/decision/rerender":
                self._decision(app.rerender)
            elif path == "/api/decision/retry":
                self._decision(app.retry)
            elif path == "/api/decision/restore":
                self._restore()
            elif path == "/api/decision/restore-ack":
                self._restore_ack()
            elif path == "/api/macro/heartbeat":
                self._heartbeat()
            else:
                self._send(404, {"error": "not found"})

        def do_GET(self) -> None:
            if self.path == "/api/status":
                self._send(200, app.status())
            elif self.path == "/api/session/layers":
                self._layers()
            else:
                self._send(404, {"error": "not found"})

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

        def _unlock(self) -> None:
            try:
                app.unlock()
            except InvalidTransition as exc:
                self._send(409, {"error": str(exc)})
                return
            self._send(200, {"ok": True})

        def _decision(self, action) -> None:
            try:
                action()
            except (InvalidTransition, ValueError) as exc:
                self._send(409, {"error": str(exc)})
                return
            self._send(200, {"ok": True})

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
            app.note_heartbeat()
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
