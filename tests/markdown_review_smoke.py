"""Run the three Markdown paths through the shipped CLI and container scheduler."""
import argparse
import base64
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import subprocess
import tempfile
import threading
import time

ROOT = Path(__file__).resolve().parents[1]
FIXTURES = ROOT / "examples/markdown_review/fixtures"


class Backend(BaseHTTPRequestHandler):
    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        if self.path == "/api/show":
            result = {"details":{"format":"gguf"},"model_info":{"general.architecture":"fixture"}}
        else:
            assert self.path == "/api/chat"
            edits = json.loads(body["messages"][-1]["content"])["edits"]
            result = {"done":True,"model":body["model"],"prompt_eval_count":10,"eval_count":4,
                      "message":{"content":json.dumps({"conflict":edits[0]["before"] == edits[1]["before"]})}}
        self.send_response(200)
        self.end_headers()
        self.wfile.write(json.dumps(result).encode())

    def do_GET(self):
        self.send_response(200)
        self.end_headers()
        self.wfile.write(b'{"models":[]}')


def exercise(directory, case, choice, browser=False):
    directory.mkdir(exist_ok=True)
    env = {**os.environ, "AN_HOME":str(directory)}
    def cli(*args, body=None):
        raw = subprocess.check_output([ROOT/"target/debug/an", *args], env=env,
                                      input=json.dumps(body or {}), text=True).strip()
        return json.loads(raw) if raw.startswith(("{","[")) else raw

    spool = cli("register","an-markdown-review:headless")
    task = cli("task","new","--spool",spool,"--model","local","--input",FIXTURES/"original.md",
               "--input",FIXTURES/case/"reviews.json")["task"]
    env["AN_TURN"] = cli("permission","begin","--task",task)["turn"]
    def graph(action, **body):
        return cli("graph","--task",task,action,body=body)
    graph("submit",base_revision=0,idempotency_key="start",nodes=[{
        "id":"start","kind":"expand","spool":spool,"argv":["python3","/spool/code/driver.py","markdown",spool],
        "outputs":["subgraph.json"]}])
    studio = chrome = None
    try:
        for _ in range(600):
            for r in graph("requests"):
                cli("permission","decide","--task",task,"--request-id",r["id"],"--decision","once")
            states = graph("state")["activity"]
            for node,state in states.items():
                assert state["status"] not in ("failed","unknown"), (node,state)
            if states.get("resolve",{}).get("status") == "waiting":
                assert choice
                if browser and studio is None:
                    import sys
                    studio = subprocess.Popen([sys.executable,"-m","orchestrator","--an",str(ROOT/"target/debug/an"),
                                               "--task",task],cwd=ROOT/"studio",env=env,stdout=subprocess.PIPE,text=True)
                    url = studio.stdout.readline().strip().split("macro UI: ")[1]
                    chrome = subprocess.Popen(["node",ROOT/"tests/markdown_review_browser.cjs",choice,str(directory)],
                                              stdin=subprocess.PIPE,text=True,env=env)
                    chrome.stdin.write(url)
                    chrome.stdin.close()
                elif not browser:
                    cli("decision","answer","--task",task,"--node","resolve","--request",states["resolve"]["id"],
                        "--json","-",body={"choice":choice})
            if states.get("delivery",{}).get("status") == "done":
                break
            if chrome and chrome.poll() not in (None,0):
                raise RuntimeError("Browser failed")
            time.sleep(.4)
        else:
            raise TimeoutError(states)
        rows = graph("view")["timeline"]
        assert [r["response"]["choice"] for r in rows if r["kind"]=="decision"] == ([choice] if choice else [])
        delivery = next(r["delivery"] for r in rows if r["kind"]=="delivery")
        files = graph("open",delivery=delivery)["files"]
        def read(binding, name):
            entry = files[binding][name]
            return base64.b64decode(entry["data"] if isinstance(entry,dict) else entry)
        revised = read("document","revised.md")
        edits = json.loads((FIXTURES/case/"reviews.json").read_text())
        expected = (FIXTURES/"original.md").read_text()
        for e in ([edits["AB".index(choice)]] if choice else edits):
            expected = expected.replace(e["before"], e["after"])
        assert revised == expected.encode()
        original = directory/"original.md"
        original.write_bytes((FIXTURES/"original.md").read_bytes())
        subprocess.run(["patch",str(original)],input=read("diff","changes.diff"),check=True)
        assert original.read_bytes() == revised
        if chrome:
            assert chrome.wait(timeout=60)==0
        graph("finish")
        assert [r["kind"] for r in graph("view")["timeline"]] == ["delivery"]
        print(case,choice,delivery,flush=True)
    finally:
        for process in (chrome,studio):
            if process and process.poll() is None:
                process.terminate()
                process.wait()
        for instance in cli("state","--task",task)["instances"]:
            if instance["status"] != "unloaded":
                cli("unload","--task",task,"--instance",instance["id"])


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--real",action="store_true")
    parser.add_argument("--browser",action="store_true")
    args = parser.parse_args()
    directory = Path(tempfile.mkdtemp(prefix="markdown-review-"))
    print(directory,flush=True)
    server = None
    try:
        if not args.real:
            server = ThreadingHTTPServer(("127.0.0.1",0),Backend)
            threading.Thread(target=server.serve_forever,daemon=True).start()
        for case,choice in [("compatible",None),("conflict","A"),("conflict","B")]:
            folder = directory/(choice or case)
            if server:
                folder.mkdir()
                (folder/"config.json").write_text(json.dumps({"ollama_port":server.server_port}))
            else:
                config = Path(os.environ.get("AN_HOME", Path.home()/".an"))/"config.json"
                if config.is_file():
                    settings = json.loads(config.read_text())
                    folder.mkdir()
                    (folder/"config.json").write_text(json.dumps({
                        k:v for k,v in settings.items() if k == "ollama_port" or k.startswith("model_")
                    }))
            exercise(folder, case, choice, browser=args.browser and choice=="B")
    finally:
        if server:
            server.shutdown()
