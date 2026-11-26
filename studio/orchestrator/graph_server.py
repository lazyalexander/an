"""Loopback projection of one Rust-owned task and its published workspace deliveries."""
import json
import secrets
import subprocess
import webbrowser
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import urlsplit

from . import config as studio_config

ASSETS = Path(__file__).parent / "macro" / "graph"
ACTIONS = {"view", "panel", "viewed", "draft", "respond", "open", "deliveries", "sources", "resume",
           "control", "instruct"}


def serve(task, binary, port, launch=False, config=None):
    config = config or studio_config.load()
    settings = config["graph"]
    # Public, like the page assets: text and timing only, never the session key.
    page_config = json.dumps({"locale": config["locale"],
                              "strings": studio_config.strings(config["locale"])["graph"],
                              "settings": {k: settings[k] for k in ("poll_ms", "panel_timeout_ms")}}).encode()
    key = secrets.token_urlsafe(32)

    def an(command, body):
        proc = subprocess.run([binary, *command], input=json.dumps(body), text=True,
                              capture_output=True, check=False)
        if proc.returncode:
            raise ValueError(proc.stderr.strip() or "An command failed")
        return json.loads(proc.stdout)

    def graph(action, body):
        return an(["graph", "--task", task, action], body)

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
            try:
                size = int(self.headers.get("Content-Length", 0))
                if not 0 < size <= settings["max_body_bytes"]:
                    return self.send(413, b"{}")
                body = json.loads(self.rfile.read(size))
                if not isinstance(body, dict):
                    raise ValueError("Expected object")
                if action == "view":
                    result = graph("view", {})
                    if result["authorized"] and result["revision"]:
                        graph("state", {"after": 2**53})
                        result = graph("view", {})
                    # Keep execution evidence behind the explicit sources/evidence actions.
                    for collection, fields in (("nodes", ("id", "kind", "title")),
                                        ("activity", ("id", "status", "native_phase", "native_error"))):
                        result[collection] = {node: {field: row[field] for field in fields if field in row}
                                       for node, row in result[collection].items()}
                    result["permissions"] = graph("requests", {}) if result["authorized"] else []
                    result["deliveries"] = graph("deliveries", {})
                elif action == "decide":
                    if body["decision"] not in ("deny", "once") or body["request"] not in {
                            item["id"] for item in graph("requests", {})}:
                        raise ValueError("Unknown permission decision")
                    result = an(["permission", "decide", "--task", task, "--request-id",
                                 body["request"], "--decision", body["decision"]], {})
                elif action in ACTIONS:
                    result = graph(action, body)
                elif action == "evidence":
                    result = graph("evidence", body)
                else:
                    return self.send(404, b"{}")
                self.send(200, json.dumps(result).encode())
            except (KeyError, ValueError, OSError) as exc:
                self.send(409, json.dumps({"error": str(exc)}).encode())

    with ThreadingHTTPServer(("127.0.0.1", port), Handler) as server:
        url = f"http://127.0.0.1:{server.server_port}/ui#{key}"
        print(f"macro UI: {url}", flush=True)
        if launch:
            webbrowser.open(url)
        server.serve_forever()
