"""Loopback projection of Rust-owned tasks and their published workspace deliveries.

With a workspace, Studio is also the controller that starts tasks from a
sentence: it creates the task, begins its turn and records the sentence as the
first instruction. The turn stays in this process and is printed only to the
operator's stdout for the agent harness; the browser never receives it.
"""
import json
import os
import uuid
import secrets
import subprocess
import webbrowser
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import urlsplit

from . import config as studio_config
from .runner import Runner

ASSETS = Path(__file__).parent / "macro" / "graph"
ACTIONS = {"view", "panel", "viewed", "draft", "respond", "open", "deliveries", "sources", "resume",
           "control", "instruct"}


def serve(task, binary, port, launch=False, config=None, workspace=None, harness="studio"):
    config = config or studio_config.load()
    settings = config["graph"]
    # Public, like the page assets: text and timing only, never the session key.
    page_config = json.dumps({"locale": config["locale"],
                              "strings": studio_config.strings(config["locale"])["graph"],
                              "settings": {k: settings[k] for k in ("poll_ms", "panel_timeout_ms")},
                              "task": task, "workspace": workspace is not None}).encode()
    # task -> turn credential held by this controller (None: inherit AN_TURN).
    turns = {task: None} if task else {}
    # task -> MCP config handed to the agent; the browser never sees it.
    mcp = {}
    if task:
        mcp[task] = {"an": {"command": str(Path(binary).resolve()) if os.sep in binary else binary,
                            "args": ["mcp", "--task", task],
                            "env": {k: os.environ[k] for k in ("AN_HOME", "AN_TURN")
                                    if k in os.environ}}}
    runner = Runner(binary, config["agent"], turns, mcp)
    key = secrets.token_urlsafe(32)

    def an(command, body, turn=None):
        env = {**os.environ, "AN_TURN": turn} if turn else None
        proc = subprocess.run([binary, *command], input=json.dumps(body), text=True,
                              capture_output=True, check=False, env=env)
        if proc.returncode:
            raise ValueError(proc.stderr.strip() or "An command failed")
        return json.loads(proc.stdout)

    def start(text):
        """Create a workspace task whose first instruction is the person's sentence."""
        session = uuid.uuid4().hex
        selector = ["--harness", harness, "--session", session]
        bound = an(["workspace", "bind", workspace, *selector], {})
        created = an(["task", "new", *selector], {})["task"]
        turn = an(["permission", "begin", "--task", created], {})["turn"]
        an(["graph", "--task", created, "instruct"], {"text": text, "revision": 0}, turn)
        servers = bound["mcp_servers"]
        for server in servers.values():
            server.setdefault("env", {})["AN_TURN"] = turn
        mcp[created] = servers
        turns[created] = turn  # the runner picks the task up from here
        # Operator channel for an external harness; never sent to the browser.
        print("agent: " + json.dumps({"task": created, "mcp_servers": servers}), flush=True)
        return {"task": created}

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
            current = self.headers.get("X-An-Task") or task
            try:
                size = int(self.headers.get("Content-Length", 0))
                if not 0 < size <= settings["max_body_bytes"]:
                    return self.send(413, b"{}")
                body = json.loads(self.rfile.read(size))
                if not isinstance(body, dict):
                    raise ValueError("Expected object")
                if action == "start":
                    if workspace is None:
                        raise ValueError("Studio was not started for a workspace")
                    text = body.get("text")
                    if not isinstance(text, str) or not text.strip():
                        raise ValueError("Empty instruction")
                    return self.send(200, json.dumps(start(text)).encode())
                if current not in turns:
                    raise ValueError("Unknown task")
                turn = turns[current]

                def graph(name, args):
                    return an(["graph", "--task", current, name], args, turn)

                if action == "view":
                    result = graph("view", {})
                    if result["authorized"] and result["revision"]:
                        graph("state", {"after": 2**53})
                        result = graph("view", {})
                    # Keep execution evidence behind the explicit sources/evidence actions.
                    for collection, fields in (("nodes", ("id", "kind", "title")),
                                        ("activity", ("id", "status"))):
                        result[collection] = {node: {field: row[field] for field in fields if field in row}
                                       for node, row in result[collection].items()}
                    result["permissions"] = graph("requests", {}) if result["authorized"] else []
                    result["deliveries"] = graph("deliveries", {})
                    result["agent"] = runner.status(current)
                elif action == "intent-decide":
                    result = an(["permission","intent-decide","--task",current,"--request-id",body["id"],
                                 "--decision",json.dumps(body["decisions"])], {}, turn)
                elif action == "decide":
                    if body["decision"] not in ("deny", "once") or body["request"] not in {
                            item["id"] for item in graph("requests", {})}:
                        raise ValueError("Unknown permission decision")
                    result = an(["permission", "decide", "--task", current, "--request-id",
                                 body["request"], "--decision", body["decision"]], {}, turn)
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
        runner.start()
        try:
            server.serve_forever()
        finally:
            runner.close()
