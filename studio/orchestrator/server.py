import json
from dataclasses import asdict
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import parse_qs, urlparse

from . import config
from .app import Orchestrator
from .ledger import Ledger
from .macro.instances import load as load_instance
from .macro.ports import BadRequest, force
from .macro.state import build_state
from .phase import InvalidTransition
from .scenes import SceneStore

GATED_PREFIXES = ("/api/action/", "/api/input/", "/api/read/",
                  "/api/macro/", "/ui", "/static/")
STATIC_DIR = Path(__file__).parent / "macro" / "static"
STATIC_CONTENT_TYPES = {"index.html": "text/html",
                        "app.js": "text/javascript",
                        "style.css": "text/css"}


def make_server(app: Orchestrator, host: str = "127.0.0.1", port: int = 8791,
                ui_token: str | None = None,
                ledger: Ledger | None = None, scenes: SceneStore | None = None,
                instance=None) -> ThreadingHTTPServer:
    if instance is None:
        instance = load_instance("bridge-studio")
    chrome = config.strings(config.load()["locale"])["legacy"]
    if ledger is None:
        ledger = getattr(app, "_ledger", None)
    if scenes is None:
        scenes = getattr(app, "_scenes", None)
    # The phase machine is core: every instance gets lock/unlock. An
    # instance may override them, and adds its own actions/inputs/reads.
    actions = {"lock": lambda body: asdict(app.lock()),
               "unlock": lambda body: app.unlock(force=force(body))}
    actions.update(getattr(instance, "actions", lambda app: {})(app))
    inputs = getattr(instance, "inputs", lambda app: {})(app)
    reads = getattr(instance, "reads", lambda app: {})(app)

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

        def _guarded(self, path: str) -> bool:
            if path.startswith(GATED_PREFIXES) and not self._authorized():
                self._send(403, {"error": "bad key"})
                return False
            return True

        def do_POST(self) -> None:
            path = urlparse(self.path).path
            if not self._guarded(path):
                return
            if path.startswith("/api/action/"):
                self._dispatch(actions, path[len("/api/action/"):])
            elif path.startswith("/api/input/"):
                self._dispatch(inputs, path[len("/api/input/"):])
            else:
                self._send(404, {"error": "not found"})

        def do_GET(self) -> None:
            parsed = urlparse(self.path)
            path = parsed.path
            if not self._guarded(path):
                return
            if path == "/api/status":
                self._send(200, app.status())
            elif path.startswith("/api/read/"):
                self._dispatch_read(reads, path[len("/api/read/"):], parsed)
            elif path == "/api/macro/strings":
                self._send(200, chrome)
            elif path == "/api/macro/manifest":
                self._send(200, instance.MANIFEST)
            elif path == "/api/macro/state":
                self._send(200, build_state(app, ledger, scenes,
                                            instance=instance))
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

        def _dispatch(self, handlers, name: str) -> None:
            handler = handlers.get(name)
            if handler is None:
                self._send(404, {"error": "unknown port"})
                return
            try:
                body = self._read_json()
            except json.JSONDecodeError:
                self._send(400, {"error": "invalid json"})
                return
            if not isinstance(body, dict):
                self._send(400, {"error": "expected a JSON object"})
                return
            self._run(handler, body)

        def _dispatch_read(self, handlers, name: str, parsed) -> None:
            handler = handlers.get(name)
            if handler is None:
                self._send(404, {"error": "unknown port"})
                return
            self._run(handler, {key: values[0]
                                for key, values in parse_qs(parsed.query).items()})

        def _run(self, handler, argument) -> None:
            try:
                result = handler(argument)
            except BadRequest as exc:
                self._send(400, {"error": str(exc)})
                return
            except (InvalidTransition, ValueError) as exc:
                self._send(409, {"error": str(exc)})
                return
            except Exception as exc:  # noqa: BLE001 — instance failures are data
                self._send(500, {"error": str(exc)})
                return
            self._send(200, result or {"ok": True})

        def _static(self, name: str) -> None:
            content_type = STATIC_CONTENT_TYPES.get(name)
            if content_type is None:
                self._send(404, {"error": "not found"})
                return
            self._send_bytes(200, (STATIC_DIR / name).read_bytes(), content_type)

        def _preview_png(self) -> None:
            preview = getattr(instance, "preview", None)
            data = preview(app) if preview is not None else None
            if data is None:
                self._send(404, {"error": "no preview"})
                return
            self._send_bytes(200, data, "image/png")

        def _archive_thumb(self, parsed) -> None:
            ids = parse_qs(parsed.query).get("id", [])
            if not ids or not ids[0]:
                self._send(400, {"error": "missing id"})
                return
            thumb = getattr(instance, "thumbnail", None)
            data = thumb(app, ids[0]) if thumb is not None else None
            if data is None:
                self._send(404, {"error": "no thumbnail"})
                return
            self._send_bytes(200, data, "image/png")

    return ThreadingHTTPServer((host, port), Handler)
