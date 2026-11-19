"""Issue a grant with the former runtime, then consume it with the Rust binary."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile


def main():
    with tempfile.TemporaryDirectory() as tmp:
        os.environ["AN_HOME"] = tmp
        sys.path.insert(0, str(Path(__file__).resolve().parent / "reference"))
        import kernel
        import permissions

        with kernel.connect() as db:
            task = kernel.task_new(db, [], [], {})
            rev = kernel.revision(db, {"报告.txt": kernel.blob(b"previous revision")})
            run = kernel.uid()
            kernel.put(db, "run", run, {"id": run, "task": task, "status": "delivered",
                                        "output": rev, "spool": "example"})
        with kernel.connect() as db:
            turn = permissions.control(db, task, "begin", None, None)["turn"]
        os.environ["AN_TURN"] = turn
        destination = str(Path(tmp).resolve() / "交付")
        with kernel.connect() as db:
            req = permissions.gate(db, task, "export", [run, rev, destination], {},
                                   flow=["export","example",destination])["request_id"]
        def cli(*args, check=True):
            return subprocess.run(["target/debug/an", *args], check=check, capture_output=True,
                                  text=True, env={**os.environ, "AN_PYTHON": "/missing"})
        assert json.loads(cli("permission","pending","--task",task).stdout)[0]["id"] == req
        cli("permission","decide","--task",task,"--request-id",req,"--decision","once")
        cli("export","--request-id",req,run,destination)
        assert (Path(destination)/"报告.txt").read_bytes() == b"previous revision"
        with kernel.connect() as db:
            assert kernel.get(db, "request", req)["state"] == "used"
        assert "already used" in cli("export","--request-id",req,run,destination,check=False).stderr


if __name__ == "__main__":
    main()
