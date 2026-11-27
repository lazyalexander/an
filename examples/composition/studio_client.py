"""Manage a private Studio presentation and controller for a fixture task."""

import json
import select
import signal
import subprocess
import sys
from contextlib import contextmanager
from pathlib import Path
from urllib.error import HTTPError
from urllib.parse import urlsplit
from urllib.request import ProxyHandler, Request, build_opener

ROOT = Path(__file__).resolve().parents[2]


def serve(task, binary):
    sys.path.insert(0, str(ROOT / "studio"))
    from an_host.client import connect
    from orchestrator import config, graph_server

    settings = config.load()
    settings["agent"].update(
        command=[sys.executable, str(ROOT / "examples/agents/codex_agent.py")],
        env=["CODEX_HOME"],
    )
    # Keep the controller's private stop handle out of the HTTP API.
    control = connect(binary, task, None, "studio", settings["agent"])
    try:
        graph_server.serve(task, binary, 0, config=settings)
    except KeyboardInterrupt:
        pass
    finally:
        control("host-stop", task, {})


@contextmanager
def studio(runtime):
    log_path = runtime.directory / "studio.log"
    with log_path.open("w") as log:
        process = subprocess.Popen(
            [sys.executable, "-u", __file__, runtime.task, str(runtime.binary)],
            env=runtime.env,
            stdout=subprocess.PIPE,
            stderr=log,
            text=True,
        )
    try:
        if not select.select([process.stdout], [], [], 30)[0]:
            raise TimeoutError("Studio startup timed out")
        banner = process.stdout.readline().strip()
        url = urlsplit(banner.removeprefix("macro UI: "))
        if url.hostname != "127.0.0.1" or url.path != "/ui" or not url.fragment:
            detail = log_path.read_text().strip().splitlines()
            raise RuntimeError(
                "Studio failed to start: " + (detail[-1] if detail else str(log_path))
            )
        endpoint = f"http://127.0.0.1:{url.port}/api/"
        opener = build_opener(ProxyHandler({}))

        def request(action, **body):
            req = Request(
                endpoint + action,
                data=json.dumps(body).encode(),
                headers={
                    "Authorization": "Bearer " + url.fragment,
                    "Content-Type": "application/json",
                },
            )
            try:
                with opener.open(req, timeout=90) as response:
                    return json.load(response)
            except HTTPError as exc:
                raise RuntimeError(
                    f"Studio {action} returned {exc.code}: {exc.read().decode()}"
                ) from exc

        yield request
    finally:
        if process.poll() is None:
            process.send_signal(signal.SIGINT)
            try:
                process.wait(timeout=20)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
                raise RuntimeError("Studio did not shut down gracefully")
        process.stdout.close()


if __name__ == "__main__":
    serve(*sys.argv[1:])
