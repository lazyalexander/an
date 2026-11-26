"""Browser presentation for the local An controller."""
import json
import secrets
import webbrowser
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import urlsplit

from . import config as studio_config
from an_host.client import connect

ASSETS = Path(__file__).parent / "macro" / "graph"


def serve(task, binary, port, launch=False, config=None, workspace=None, harness="studio"):
    config = config or studio_config.load()
    settings = config["graph"]
    # Public, like the page assets: text and timing only, never the session key.
    page_config = json.dumps({"locale": config["locale"],
                              "strings": studio_config.strings(config["locale"])["graph"],
                              "settings": {k: settings[k] for k in ("poll_ms", "panel_timeout_ms")},
                              "task": task, "workspace": workspace is not None}).encode()
    request = connect(binary, task, workspace, harness, config["agent"])
    key = secrets.token_urlsafe(32)

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def send(self, status, data, mime="application/json"):
            self.send_response(status)
            if mime == "text/html":
                # The embedding document controls network navigations of its child.
                # The trusted bootstrap is an inline srcdoc, requiring no frame fetch.
                self.send_header("Content-Security-Policy", "frame-src 'none'; object-src 'none'; base-uri 'none'")
            self.send_header("Cache-Control", "no-store")
            self.send_header("Referrer-Policy", "no-referrer")
            self.send_header("X-Content-Type-Options", "nosniff")
            self.send_header("Content-Type", mime)
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

        def do_GET(self):
            assets = {"/ui": ("index.html", "text/html"), "/graph.css": ("graph.css", "text/css"),
                      "/graph.js": ("graph.js", "text/javascript")}
            if urlsplit(self.path).path == "/config.json":
                return self.send(200, page_config)
            asset = assets.get(urlsplit(self.path).path)
            if asset:
                return self.send(200, (ASSETS / asset[0]).read_bytes(), asset[1])
            self.send(404, b"{}")

        def do_POST(self):
            if not secrets.compare_digest(self.headers.get("Authorization", ""), "Bearer " + key):
                return self.send(403, b'{"error":"Invalid Studio session"}')
            action = self.path.removeprefix("/api/")
            if action.startswith("host-") or action == "connect":
                return self.send(404, b"{}")
            current = self.headers.get("X-An-Task") or task
            try:
                size = int(self.headers.get("Content-Length", 0))
                if not 0 < size <= settings["max_body_bytes"]:
                    return self.send(413, b"{}")
                body = json.loads(self.rfile.read(size))
                if not isinstance(body, dict):
                    raise ValueError("Expected object")
                result = request(action, current, body)
                self.send(200, json.dumps(result).encode())
            except (KeyError, ValueError, OSError) as exc:
                self.send(409, json.dumps({"error": str(exc)}).encode())

    with ThreadingHTTPServer(("127.0.0.1", port), Handler) as server:
        url = f"http://127.0.0.1:{server.server_port}/ui#{key}"
        print(f"macro UI: {url}", flush=True)
        if launch:
            webbrowser.open(url)
        server.serve_forever()
