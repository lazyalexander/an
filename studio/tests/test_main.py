import http.client
import socket
import subprocess
import sys
import threading
from pathlib import Path

from conftest import get_text, wait_until

STUDIO = Path(__file__).resolve().parent.parent


def test_help_exits_zero():
    proc = subprocess.run(
        [sys.executable, "-m", "orchestrator", "--help"],
        cwd=STUDIO, capture_output=True, text=True, timeout=30)
    assert proc.returncode == 0, proc.stderr
    assert "--inbox" in proc.stdout


def _accepts(port):
    try:
        with socket.create_connection(("127.0.0.1", port), timeout=0.2):
            return True
    except OSError:
        return False


def _stop(servers, thread):
    for srv in servers:
        srv.shutdown()
        srv.server_close()
    thread.join(timeout=5)
    assert not thread.is_alive()


def test_main_writes_token_and_prints_url(tmp_path, capsys, monkeypatch):
    import orchestrator.__main__ as boot

    servers = []
    real_make_server = boot.make_server

    def capturing_make_server(*args, **kwargs):
        srv = real_make_server(*args, **kwargs)
        servers.append(srv)
        return srv

    monkeypatch.setattr(boot, "DEFAULT_RUN_DIR", tmp_path / "run")
    monkeypatch.setattr(boot, "make_server", capturing_make_server)
    thread = threading.Thread(target=boot.main,
                              args=(["--fake", "--port", "0"],), daemon=True)
    thread.start()
    try:
        wait_until(lambda: bool(servers))
        port = servers[0].server_address[1]
        wait_until(lambda: _accepts(port))  # serving ⇒ the URL line is printed
        token = (tmp_path / "run" / "ui_token").read_text().strip()
        assert token
        out = capsys.readouterr().out
        assert f"macro UI: http://127.0.0.1:{port}/ui?key={token}" in out
        assert (tmp_path / "run" / "preview").is_dir()
        conn = http.client.HTTPConnection("127.0.0.1", port)
        assert get_text(conn, "/ui")[0] == 403
        assert get_text(conn, f"/ui?key={token}")[0] == 200
        conn.close()
    finally:
        _stop(servers, thread)


def test_main_open_ui_opens_browser(tmp_path, monkeypatch):
    import orchestrator.__main__ as boot

    opened = []
    servers = []
    real_make_server = boot.make_server

    def capturing_make_server(*args, **kwargs):
        srv = real_make_server(*args, **kwargs)
        servers.append(srv)
        return srv

    monkeypatch.setattr(boot, "DEFAULT_RUN_DIR", tmp_path / "run")
    monkeypatch.setattr(boot, "make_server", capturing_make_server)
    monkeypatch.setattr(boot.webbrowser, "open", opened.append)
    thread = threading.Thread(
        target=boot.main, args=(["--fake", "--port", "0", "--open-ui"],),
        daemon=True)
    thread.start()
    try:
        wait_until(lambda: bool(opened))
        token = (tmp_path / "run" / "ui_token").read_text().strip()
        port = servers[0].server_address[1]
        assert opened == [f"http://127.0.0.1:{port}/ui?key={token}"]
    finally:
        _stop(servers, thread)
