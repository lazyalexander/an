"""Run from the repository root with two admitted-format local image references."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "headless"))


def main(sender, receiver):
    def cli(*args):
        return subprocess.check_output(["target/debug/an", *args], text=True).strip()

    def call(*args):
        return json.loads(cli(*args))

    def op(name, *args):
        return call(name, "--task", task, *args)

    def reject(name, *args):
        try:
            op(name, *args)
        except subprocess.CalledProcessError:
            return
        raise AssertionError("Expected rejection")

    def wait(run):
        for _ in range(240):
            state = op("status", run)["status"]
            if state != "running":
                return state
            time.sleep(.25)
        raise TimeoutError(run)

    with tempfile.TemporaryDirectory(dir="/private/tmp") as tmp:
        os.environ["AN_HOME"] = tmp
        import workspaces
        a, b = [cli("register", image) for image in (sender, receiver)]
        assert a.split("@")[1] != b.split("@")[1]
        workspace = call("workspace", "create", "--spool", a, "--spool", b)["workspace"]
        call("workspace", "bind", workspace, "--harness", "test", "--session", "test")
        task = workspaces.resolve(argparse.Namespace(harness="test", session="test", key=None))
        flowfile = Path(tmp)/"flows.json"
        flowfile.write_text(json.dumps([["handoff", a, b]]))
        call("workspace","policy","set",workspace,"--file",str(flowfile))
        os.environ["AN_TURN"] = call("permission","begin","--task",task)["turn"]
        loaded = []
        try:
            for image in (a,b):
                loaded.append(op("load","--spool",image)["id"])
            run = op("run","--instance",loaded[0],"--output","item","--","/bin/sh","-c","printf sample >/output/item")
            assert wait(run) == "awaiting_publication"
            def handoff():
                args = ["--recipient-spool",b,run]
                req = op("handoff",*args)["request_id"]
                call("permission","decide","--task",task,"--request-id",req,"--decision","once")
                return op("handoff","--request-id",req,*args)["handoff"]
            ticket = handoff()
            digest = hashlib.sha256(b"sample").hexdigest()
            def receive(instance, ticket, fail=False):
                action = reject if fail else op
                return action("run","--instance",instance,"--input",ticket,"--","/bin/sh","-c",
                              f'echo "{digest}  /input/item" | sha256sum -c -')
            receive(loaded[0], ticket, True)
            assert wait(receive(loaded[1],ticket)) == "completed"
            receive(loaded[1],ticket,True)
            assert run not in json.dumps(op("deliveries"))
            assert subprocess.run(["target/debug/an","export",ticket,str(Path(tmp)/"out")],capture_output=True).returncode
            ticket = handoff()
            flowfile.write_text('{"default":"unavailable","effects":[]}')
            call("permission","policy","--task",task,"--file",str(flowfile))
            receive(loaded[1],ticket,True)
        finally:
            for instance in loaded:
                op("unload","--instance",instance)


if __name__ == "__main__":
    main(*sys.argv[1:])
