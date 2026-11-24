"""Exercise actual spool execution via stdio MCP, Studio host APIs and optionally Chrome."""
import argparse
import base64
import io
import json
import os
import subprocess
import sys
import tempfile
import time
from zipfile import ZipFile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "examples/graph_studio"))
import experiments as ex


class Agent:
    def __init__(self, binary, task, env):
        self.proc = subprocess.Popen([binary, "mcp", "--task", task], env=env, text=True,
                                     stdin=subprocess.PIPE, stdout=subprocess.PIPE)
        self.sequence = 0
        self.request("initialize", {"protocolVersion": "2025-03-26", "capabilities": {},
                                    "clientInfo": {"name": "studio-acceptance", "version": "1"}})
        self.write({"method": "notifications/initialized"})

    def write(self, obj):
        self.proc.stdin.write(json.dumps({"jsonrpc": "2.0", **obj}) + "\n")
        self.proc.stdin.flush()

    def request(self, method, params):
        self.sequence += 1
        self.write({"id": self.sequence, "method": method, "params": params})
        while line := self.proc.stdout.readline():
            response = json.loads(line)
            if response.get("id") == self.sequence:
                return response["result"]
        raise RuntimeError("MCP process exited")

    def graph(self, name, **args):
        result = self.request("tools/call", {"name": "an_graph_" + name, "arguments": args})
        if result.get("isError"):
            raise ValueError(result["content"][0]["text"])
        return json.loads(result["content"][0]["text"])

    def close(self):
        self.proc.stdin.close()
        self.proc.wait(timeout=30)


def exercise(directory, image, browser, experiment):
    env = {**os.environ, "AN_HOME": str(directory)}
    binary = str(ROOT / "target/debug/an")

    def cli(*args, body=None):
        return subprocess.check_output([binary, *args], env=env, text=True,
                                       input=json.dumps(body or {})).strip()

    spool = cli("register", image)
    workspace = json.loads(cli("workspace", "create", "--spool", spool))["workspace"]
    selector = ["--harness", "validation", "--session", experiment]
    cli("workspace", "bind", workspace, *selector)
    task = json.loads(cli("task", "new", *selector))["task"]
    env["AN_TURN"] = json.loads(cli("permission", "begin", "--task", task))["turn"]

    def host(action, **args):
        return json.loads(cli("graph", "--task", task, action, body=args))

    def wait(node, desired):
        deadline = time.monotonic() + 240
        while time.monotonic() < deadline:
            state = agent.graph("state", after=0)
            for r in host("requests"):
                cli("permission", "decide", "--task", task, "--request-id", r["id"], "--decision", "once")
            activity = state["activity"][node]
            if activity["status"] == desired:
                return activity
            if activity["status"] in ("failed", "unknown"):
                raise AssertionError(activity)
            time.sleep(.2)
        raise TimeoutError(state["activity"])

    def start_studio():
        proc = subprocess.Popen([sys.executable, "-m", "orchestrator", "--an", binary, "--task", task,
                                 "--port", "0"], cwd=ROOT / "studio", env=env,
                                stdout=subprocess.PIPE, text=True)
        return proc, proc.stdout.readline().strip().split("macro UI: ")[1]

    def chrome(mode):
        subprocess.run(["node", str(ROOT / "tests/graph_studio_browser.cjs"), mode, experiment,
                        str(directory)], input=url, env=env, text=True, check=True, timeout=90)

    agent = Agent(binary, task, env)
    studio = None
    try:
        bridge = experiment == "bridge"
        nodes = ex.bridge(spool) if bridge else ex.cad(spool)
        if bridge:
            for candidate in nodes[:2]:
                candidate["argv"][-1]="import time; time.sleep(2)\n"+candidate["argv"][-1]
        nodes += [dict(ex.run("failure", spool, "raise SystemExit(7)", []), after=[nodes[0]["id"]])]
        first = dict(base_revision=0, idempotency_key="first", nodes=nodes)
        receipt = agent.graph("submit", **first)
        assert receipt["revision"] == 1
        ids = {k: v["id"] for k, v in host("view")["activity"].items()}
        agent.close()
        agent = Agent(binary, task, env)
        assert agent.graph("submit", **first) == receipt
        assert agent.graph("submit", **{**first, "idempotency_key": "stale"})["status"] == "conflict"
        decision = "compare" if bridge else "timing"
        delivery = "report" if bridge else "animation"
        state = wait(decision, "waiting")
        assert "apply" not in host("view")["nodes"]
        request = dict(node=decision, request=state["id"])
        if browser:
            studio, url = start_studio()
            chrome("respond")
        else:
            assert host("panel", **request)["html"]
            host("viewed", **request)
            response = dict(choice="B", x=.5, y=.25, note="edge") if bridge else dict(seconds=2)
            host("draft", **request, value=response)
            assert host("panel", **request)["draft"] == response
            host("respond", **request, value=response)
        responded = "saving" if bridge else "done"
        response = wait(decision, responded)["response"]
        assert host("respond", **request, value=response)["status"] == responded
        if bridge:
            assert set(host("view")["nodes"]).isdisjoint({"apply", "report"})
            agent.graph("submit", base_revision=1, idempotency_key="after-response",
                        nodes=ex.bridge_result(spool, response))
        published = wait(delivery, "done")["delivery"]
        state = agent.graph("state")
        assert state["activity"]["failure"]["status"] == "failed"
        assert state["activity"]["failure"]["evidence"]["exit_code"] == 7
        assert all(ids[k] == state["activity"][k]["id"] for k in ids)
        if bridge:
            candidates=[state["activity"][key] for key in ("A","B")]
            assert candidates[0]["instance"] != candidates[1]["instance"]
            assert max(c["evidence"]["started_at"] for c in candidates) < min(c["evidence"]["exited_at"] for c in candidates)
        assert sum(e["kind"] == "responded" for e in state["events"]) == 1
        cursor = state["events"][-1]["cursor"]
        assert not agent.graph("state", after=cursor)["events"]
        trace = host("sources", delivery=published)
        assert trace[decision]["execution"]["response"] == response
        assert trace["apply" if bridge else "animate"]["execution"]["evidence"]["log"]
        for i in json.loads(cli("state", "--task", task))["instances"]:
            cli("unload", "--task", task, "--instance", i["id"])
        assert host("finish")["closed"]
        assert host("view")["closed"]
        assert published in [d["id"] for d in json.loads(cli("deliveries","--task",task))["items"]]
        pkg = host("open", delivery=published)
        if bridge:
            with ZipFile(io.BytesIO(base64.b64decode(pkg["files"]["painting"]["painting.ora"]))) as archive:
                assert archive.testzip() is None
                assert {"stack.xml","data/reference.png","data/annotation.png"} <= set(archive.namelist())
            assert "edge" in base64.b64decode(pkg["files"]["choice"]["response.json"]).decode()
        else:
            mesh=base64.b64decode(pkg["files"]["model"]["plate.stl"])
            assert len(mesh)==84+50*int.from_bytes(mesh[80:84],"little") and len(mesh)>84
            assert 'dur="2s"' in base64.b64decode(pkg["files"]["animation"]["turntable.svg"]).decode()
        if browser:
            chrome("reopen")
        fresh = host("resume", delivery=published)["task"]
        assert json.loads(cli("graph", "--task", fresh, "open", body={"delivery": published})) == pkg
        env["AN_TURN"] = json.loads(cli("permission","begin","--task",fresh))["turn"]
        destination = directory / "exported"
        export = ["export", published, str(destination), "--task", fresh]
        pending = json.loads(cli(*export))
        assert pending["status"] == "permission_required" and not destination.exists()
        cli("permission", "decide", "--task", fresh, "--request-id", pending["request_id"], "--decision", "once")
        cli(*export, "--request-id", pending["request_id"])
        for binding, files in pkg["files"].items():
            for name, entry in files.items():
                encoded = entry["data"] if isinstance(entry, dict) else entry
                assert (destination / binding / name).read_bytes() == base64.b64decode(encoded)

        def reopened(action, **args):
            return json.loads(cli("graph","--task",fresh,action,body=args))
        for kind in ("code","work","input","log"):
            args=dict(delivery=published,source="apply" if bridge else "animate",type=kind)
            permission = reopened("evidence", **args)
            assert permission["status"]=="permission_required"
            cli("permission","decide","--task",fresh,"--request-id",permission["request_id"],"--decision","once")
            evidence=reopened("evidence",**args)
            if kind=="code":
                assert "spool/code/annotate.py" in evidence
            if kind=="input":
                assert "choice/response.json" in evidence
            if kind=="log" and bridge:
                assert "Applied annotation to B" in evidence
        cli("permission","end","--task",fresh)
        print(experiment, "passed", len(state["events"]), "events", flush=True)
    finally:
        agent.close()
        if studio:
            studio.terminate()
            studio.wait(timeout=15)
        for i in json.loads(cli("state", "--task", task))["instances"]:
            if i["status"] != "unloaded":
                cli("unload", "--task", task, "--instance", i["id"])


if __name__ == "__main__":
    p = argparse.ArgumentParser()
    p.add_argument("--image", default="an-graph-studio:headless")
    p.add_argument("--browser", action="store_true")
    p.add_argument("--directory", type=Path)
    p.add_argument("--experiment", choices=["bridge", "cad"], default="bridge")
    args = p.parse_args()
    if args.directory:
        args.directory.mkdir(parents=True, exist_ok=True)
        exercise(args.directory.resolve(), args.image, args.browser, args.experiment)
    else:
        with tempfile.TemporaryDirectory() as tmp:
            exercise(Path(tmp).resolve(), args.image, args.browser, args.experiment)
