"""Own the controller socket and dispatch same-user IPC requests."""
import fcntl
import json
import os
from pathlib import Path
import socketserver
import sys
import threading

from .controller import controller


def serve(directory, options):
    class Handler(socketserver.StreamRequestHandler):
        def handle(self):
            try:
                request = json.loads(self.rfile.readline())
                if request["action"] == "connect":
                    if request["options"] != options:
                        raise ValueError("Controller configuration changed; restart the host controller")
                    result = {}
                elif request["action"] == "host-stop":
                    threading.Thread(target=self.server.shutdown, daemon=True).start()
                    result = {}
                elif request["action"] == "host-agent":
                    result = mcp[request["task"]]
                else:
                    result = dispatch(request["action"], request.get("task"), request["body"])
                response = {"result": result}
            except (ValueError, KeyError, TypeError, OSError) as exc:
                response = {"error": str(exc)}
            self.wfile.write(json.dumps(response).encode() + b"\n")

    socket = directory / "socket"
    socket.unlink(missing_ok=True)
    with socketserver.UnixStreamServer(str(socket), Handler) as server:
        with controller(directory, options) as (dispatch, mcp):
            server.serve_forever()


if __name__ == "__main__":
    directory = Path(sys.argv[1])
    os.umask(0o077)
    with (directory / "lock").open("a") as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            sys.exit(0)
        serve(directory, json.load(sys.stdin))
