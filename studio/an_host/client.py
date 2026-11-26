"""Connect presentation clients to a detached, same-user An controller."""
import hashlib
import json
import os
from pathlib import Path
import shutil
import stat
import socket
import subprocess
import sys
import time


def connect(binary, task, workspace, harness, agent):
    home = Path(os.environ.get("AN_HOME", Path.home() / ".an")).resolve()
    scope = [str(home), task, workspace, harness, os.environ.get("AN_TURN") if task else None]
    digest = hashlib.sha256(json.dumps(scope).encode()).hexdigest()[:32]
    # AF_UNIX paths are length-limited on macOS. Keep the socket path short.
    directory = Path("/tmp") / f"an-{os.getuid()}-{digest}"
    directory.mkdir(mode=0o700, exist_ok=True)
    info = directory.lstat()
    if not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid() or info.st_mode & 0o077:
        raise ValueError("Unsafe controller directory")
    options = dict(binary=str(Path(shutil.which(binary) or binary).resolve()), task=task,
                   workspace=workspace, harness=harness, agent=agent)

    def call(message):
        with socket.socket(socket.AF_UNIX) as stream:
            stream.connect(str(directory / "socket"))
            stream.sendall(json.dumps(message).encode() + b"\n")
            with stream.makefile("rb") as reply:
                response = json.load(reply)
        if "error" in response:
            raise ValueError(response["error"])
        return response["result"]

    hello = dict(action="connect", options=options)
    try:
        call(hello)
    except (FileNotFoundError, ConnectionRefusedError):
        with (directory / "controller.log").open("ab") as log:
            process = subprocess.Popen([sys.executable, "-m", "an_host.controller", str(directory)],
                                       cwd=Path(__file__).resolve().parents[1], stdin=subprocess.PIPE,
                                       stdout=log, stderr=log, start_new_session=True,
                                       env={**os.environ, "AN_HOME": str(home)})
            process.stdin.write(json.dumps(options).encode())
            process.stdin.close()
        for _ in range(100):
            try:
                call(hello)
                break
            except (FileNotFoundError, ConnectionRefusedError):
                time.sleep(0.05)
        else:
            raise OSError(f"Controller failed to start; see {directory / 'controller.log'}")

    def request(action, current, body):
        return call(dict(action=action, task=current, body=body))

    return request
