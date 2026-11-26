"""Pressure probe server: serve the test canvas on the LAN and save uploaded reports.

Run on the Mac:  python3 examples/pressure_canvas/serve.py
Open the printed URL on the iPad (Safari, Apple Pencil) and on the Mac
(Chrome/Safari with the injector). Reports land in ./reports/.
"""
import json
import secrets
import ipaddress
import subprocess
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

HERE = Path(__file__).parent
REPORTS = HERE / "reports"
KEY = secrets.token_urlsafe(16)


def lan_addresses():
    """Private IPv4 addresses an iPad on the same Wi-Fi can reach.

    The default route may be a VPN/proxy tunnel (e.g. 198.18.0.0/15), which an
    iPad cannot reach, so list the macOS interfaces instead of trusting it.
    """
    found = []
    for interface in ("en0", "en1", "en2"):
        out = subprocess.run(["ipconfig", "getifaddr", interface],
                             capture_output=True, text=True, check=False).stdout.strip()
        if out and ipaddress.ip_address(out).is_private and not out.startswith("198.18."):
            found.append(out)
    return found or ["127.0.0.1"]


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def do_GET(self):
        if self.path.split("#")[0] not in ("/", "/index.html"):
            return self.send_error(404)
        body = (HERE / "index.html").read_bytes()
        self.send_response(200)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.send_header("Cache-Control", "no-store")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_POST(self):
        if self.path != "/report" or not secrets.compare_digest(
                self.headers.get("Authorization", ""), "Bearer " + KEY):
            return self.send_error(403)
        size = int(self.headers.get("Content-Length", 0))
        if not 0 < size <= 32 * 2**20:
            return self.send_error(413)
        report = json.loads(self.rfile.read(size))
        REPORTS.mkdir(exist_ok=True)
        name = REPORTS / f"{time.strftime('%Y%m%d-%H%M%S')}.json"
        name.write_text(json.dumps(report))
        print("report:", name.name, json.dumps(report["stats"], ensure_ascii=False), flush=True)
        self.send_response(204)
        self.end_headers()


if __name__ == "__main__":
    with ThreadingHTTPServer(("0.0.0.0", 8792), Handler) as server:
        for address in lan_addresses():
            print(f"open on iPad/Mac: http://{address}:8792/#{KEY}", flush=True)
        server.serve_forever()
