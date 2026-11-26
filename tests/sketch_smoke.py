"""Sketch surface end to end: draw with a pen in Chrome, finish in a container, deliver.

Requires Apple Container, the example image (examples/graph_studio/spool) and,
for the browser leg, Chrome with playwright-core (PLAYWRIGHT_CORE).
"""
import argparse
import base64
import json
import os
import subprocess
import sys
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(Path(__file__).resolve().parent))
from graph_studio_smoke import Agent, initial_driver  # noqa: E402

BROWSER = r"""
const {chromium}=require(process.env.PLAYWRIGHT_CORE||"playwright-core");
(async()=>{const b=await chromium.launch({channel:"chrome",headless:true});
let p;try{p=await b.newPage({viewport:{width:1100,height:900}});p.setDefaultTimeout(30000);
const errors=[];p.on("pageerror",e=>errors.push(e.message));
await p.goto(require("fs").readFileSync(0,"utf8").trim());
const canvas=p.locator(".sketch canvas");await canvas.waitFor();
await canvas.evaluate(c=>{const r=c.getBoundingClientRect();
 const ev=(t,x,y,pr)=>c.dispatchEvent(new PointerEvent(t,{pointerId:7,pointerType:"pen",bubbles:true,
   clientX:r.left+x*r.width,clientY:r.top+y*r.height,pressure:pr}));
 ev("pointerdown",.2,.6,.05);for(let i=1;i<=40;i++)ev("pointermove",.2+i*.015,.6-Math.sin(i/8)*.25,Math.min(1,.05+i*.024));ev("pointerup",.8,.6,0);});
await p.locator("label",{hasText:"note"}).locator("input").fill("roof line");
await p.waitForTimeout(300);
await p.screenshot({path:process.argv.at(-1)+"/sketch.png"});
const sent=p.waitForResponse(r=>r.url().endsWith("/api/respond")&&r.ok());
await p.getByRole("button",{name:"提交"}).click();
await sent;
if(errors.length)throw Error(errors.join(";"));}catch(e){if(p){await p.screenshot({path:process.argv.at(-1)+"/failure.png"}).catch(()=>{});
console.error("status:",await p.locator("#status").innerText().catch(()=>"?"));}throw e;}finally{await b.close();}})()
.catch(e=>{console.error(String(e));process.exitCode=1});
"""


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--image", default="an-graph-studio:headless")
    parser.add_argument("--browser", action="store_true")
    parser.add_argument("--directory", type=Path)
    args = parser.parse_args()
    directory = args.directory or Path(tempfile.mkdtemp(prefix="an-sketch-"))
    directory.mkdir(parents=True, exist_ok=True)
    env = {**os.environ, "AN_HOME": str(directory)}
    binary = str(ROOT / "target/debug/an")

    def cli(*command, body=None):
        return subprocess.check_output([binary, *command], env=env, text=True,
                                       input=json.dumps(body or {})).strip()

    spool = cli("register", args.image)
    workspace = json.loads(cli("workspace", "create", "--spool", spool))["workspace"]
    selector = ["--harness", "validation", "--session", "sketch"]
    cli("workspace", "bind", workspace, *selector)
    task = json.loads(cli("task", "new", *selector))["task"]
    env["AN_TURN"] = json.loads(cli("permission", "begin", "--task", task))["turn"]

    def host(action, **body):
        return json.loads(cli("graph", "--task", task, action, body=body))

    def wait(node, desired, deadline=240):
        end = time.monotonic() + deadline
        while time.monotonic() < end:
            for request in host("requests"):
                cli("permission", "decide", "--task", task, "--request-id", request["id"],
                    "--decision", "once")
            state = host("view")["activity"].get(node, {})
            if state.get("status") == desired:
                return state
            if state.get("status") in ("failed", "unknown"):
                raise AssertionError(state)
            time.sleep(0.3)
        raise TimeoutError(host("view")["activity"])

    agent = Agent(binary, task, env)
    agent.graph("submit", base_revision=0, idempotency_key="sketch",
                nodes=[initial_driver("sketch", spool)])
    state = wait("draw", "waiting")
    if args.browser:
        studio = subprocess.Popen([sys.executable, "-m", "orchestrator", "--an", binary, "--task",
                                   task, "--port", "0"], cwd=ROOT / "studio", env=env,
                                  stdout=subprocess.PIPE, text=True)
        try:
            url = studio.stdout.readline().strip().split("macro UI: ")[1]
            subprocess.run(["node", "-e", BROWSER, "--", str(directory)], input=url, env=env,
                           text=True, check=True, timeout=240)
        finally:
            studio.terminate()
    else:
        request = {"node": "draw", "request": state["id"]}
        host("panel", **request)
        host("viewed", **request)
        host("respond", **request, value={"note": "roof line", "strokes": [
            {"color": "#1d3557", "size": 12,
             "points": [[0.2 + i * 0.015, 0.6, min(1, 0.05 + i * 0.024)] for i in range(41)]}]})
    wait("result", "done")
    delivery = host("deliveries")[0]
    files = host("open", delivery=delivery["id"])["files"]
    svg = base64.b64decode(files["finished"]["finished.svg"]).decode()
    sketch = json.loads(base64.b64decode(files["sketch"]["response.json"]))
    pressures = [p[2] for s in sketch["strokes"] for p in s["points"]]
    assert "<path" in svg and max(pressures) - min(pressures) > 0.5, (svg[:200], pressures[:5])
    (directory / "finished.svg").write_text(svg)
    agent.close()
    print(f"sketch passed: {len(pressures)} points, pressure {min(pressures)}–{max(pressures)}, "
          f"svg {len(svg)} bytes in {directory}")


if __name__ == "__main__":
    main()
