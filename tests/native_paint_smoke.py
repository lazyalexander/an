"""Run the isolated Bridge native graph. --pencil leaves drawing to the operator."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import socket
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
AN = ROOT / "target/debug/an"
RUNNER = "/Applications/krita.app/Contents/MacOS/kritarunner"


def run(folder,image,pencil,resume=None):
    env = {**os.environ, "AN_HOME":str(folder/"registry")}
    def cli(*args,body=None):
        out = subprocess.check_output([AN,*args],env=env,input=json.dumps(body or {}),text=True).strip()
        return json.loads(out) if out.startswith(("{","[",'"')) else out

    if resume:
        task=resume
    else:
        spool = cli("register",image)
        workspace=cli("workspace","create","--spool",spool)["workspace"]
        selector=["--harness","native-acceptance","--key",workspace]
        cli("workspace","bind",workspace,*selector)
        task=cli("task","new",*selector)["task"]
    env["AN_TURN"]=cli("permission","begin","--task",task)["turn"]
    def graph(action,**args):
        return cli("graph","--task",task,action,body=args)
    def approve():
        for req in graph("requests"):
            cli("permission","decide","--task",task,"--request-id",req["id"],"--decision","once")
    def wait(node,status,timeout=300):
        end=time.monotonic()+timeout
        while time.monotonic()<end:
            state=graph("state")["activity"]
            if state.get(node,{}).get("status")==status:
                return state[node]
            for id,s in state.items():
                if s["status"]=="failed":
                    raise ValueError((id,s.get("error")))
            approve()
            time.sleep(.25)
        raise TimeoutError(node)
    def serve():
        p=subprocess.Popen([AN,"native","serve","--task",task],env=env,stdout=subprocess.PIPE,text=True)
        descriptor=Path(p.stdout.readline().strip())
        return p,descriptor,json.loads(descriptor.read_text())["socket"]
    def send(op,**args):
        with socket.socket(socket.AF_UNIX) as channel:
            channel.settimeout(30)
            channel.connect(address)
            channel.sendall(json.dumps(dict(op=op,**args)).encode()+b"\n")
            return json.loads(channel.makefile("rb").readline())
    def krita(method,**extra):
        # The macOS bundle replaces PYTHONPATH with its own resource paths.
        resource = Path.home() / "Library/Application Support/kritarunner"
        resource.mkdir(parents=True,exist_ok=True)
        with tempfile.TemporaryDirectory(prefix="an_probe_",dir=resource) as package:
            module = Path(package)
            shutil.copyfile(ROOT/"tests/krita_saved_pixels.py",module/"__init__.py")
            with (folder/"kritarunner.log").open("a") as log:
                subprocess.run([RUNNER,"-s",module.name,"-f",method],
                    env={**{k:v for k,v in os.environ.items() if k not in ("AN_HOME","AN_TURN")},
                         "QT_QPA_PLATFORM":"offscreen",**extra},
                    stdout=log,stderr=log,check=True,timeout=60)

    if not resume:
        graph("submit",base_revision=0,idempotency_key="native",nodes=[dict(id="start",kind="expand",
            spool=spool,argv=["python3","/spool/code/driver.py","bridge-native",spool],outputs=["subgraph.json"])])
        wait("compare","waiting")
        cli("decision","answer","--task",task,"--node","compare","--json","-",
            body=dict(choice="A",x=.5,y=.5,note="Native painting acceptance"))
    paint=wait("paint","waiting")
    assert paint["native_phase"]=="awaiting_adapter"
    server,descriptor,address=serve()
    try:
        if pencil:
            print(f"Krita connection: {descriptor}\nEvidence directory: {folder}",flush=True)
            job=send("poll")
            work=Path(job["directory"])
            (work/"check-pencil").touch()
            with (folder/"krita.log").open("a") as log:
                subprocess.Popen(["/Applications/krita.app/Contents/MacOS/krita","--nosplash"],
                    env={**{k:v for k,v in os.environ.items() if k not in ("AN_TURN","AN_HOME")},
                         "AN_KRITA_CONNECTION":str(descriptor)},stdout=log,stderr=log)
        else:
            job=send("poll")
            ref={k:job[k] for k in ("node","request","input")}
            assert job["request"]==paint["id"]
            assert "error" in send("receipt",**ref,digest="absent")
            assert send("opened",**ref)["status"]=="waiting"
            server.terminate();server.wait()
            server,_,address=serve()
            assert send("poll")==job
            work=Path(job["directory"])
            jobfile=folder/"job.json"
            jobfile.write_text(json.dumps(job))
            krita("fixture",AN_NATIVE_JOB=str(jobfile))
            assert (work/"working.kra").is_file(), (folder/"kritarunner.log").read_text()
            assert graph("view")["activity"]["paint"]["status"]=="waiting"
            saved=work/"submitted.kra"
            saved.write_bytes(b"damaged document")
            assert "error" in send("receipt",**ref,digest=hashlib.sha256(saved.read_bytes()).hexdigest())
            assert graph("view")["activity"]["paint"]["native_phase"]=="save_failed"
            shutil.copyfile(work/"working.kra",saved)
            digest=hashlib.sha256(saved.read_bytes()).hexdigest()
            assert "error" in send("receipt",**ref,digest="incorrect")
            assert "error" in send("receipt",**{**ref,"input":"stale"},digest=digest)
            # Stop wins against a late save. Retry has a fresh directory and attempt.
            cli("graph","--task",task,"control",body={"action":"stop"})
            assert "error" in send("receipt",**ref,digest=digest)
            assert graph("view")["activity"]["paint"]["status"]=="cancelled"
            g=graph("state")
            graph("submit",base_revision=g["revision"],idempotency_key="retry",nodes=[],retry=["paint","report"])
            cli("graph","--task",task,"control",body={"action":"resume"})
            wait("paint","waiting")
            job=send("poll")
            assert job["request"]!=ref["request"] and Path(job["directory"])!=work
            ref={k:job[k] for k in ("node","request","input")}
            send("opened",**ref)
            shutil.copyfile(saved,Path(job["directory"])/"submitted.kra")
            receipt=send("receipt",**ref,digest=digest)
            assert receipt["status"]=="done",receipt
            assert send("receipt",**ref,digest=digest)==receipt
        delivery=wait("report","done",86400 if pencil else 300)["delivery"]
        if pencil:
            pressure=json.loads((work/"pressure.json").read_text())
            assert pressure["request"]==paint["id"] and pressure["ordinary_save"],pressure
            assert any(s["samples"]>=3 and s["maximum"]-s["minimum"]>=0.1 for s in pressure["strokes"]),pressure
            shutil.copyfile(work/"pressure.json",folder/"pressure.json")
        dest=folder/"export"
        args=["export",delivery,str(dest),"--task",task]
        pending=cli(*args)
        cli("permission","decide","--task",task,"--request-id",pending["request_id"],"--decision","once")
        cli(*args,"--request-id",pending["request_id"])
        path=dest/"painting"/"painting.kra"
        receipt=graph("state")["activity"]["paint"]["receipt"]
        assert hashlib.sha256(path.read_bytes()).hexdigest()==receipt["digest"]
        report=folder/"pixels.json"
        krita("verify",AN_VERIFY_KRA=str(path),AN_PIXEL_REPORT=str(report))
        pixels=json.loads(report.read_text())
        assert pixels["kra_sha256"]==receipt["digest"]
        if pencil:
            assert pixels["columns"]>=12 and pixels["width_p90"]>pixels["width_p10"],pixels
        (folder/"result.json").write_text(json.dumps(dict(task=task,receipt=receipt,delivery=delivery,pixels=pixels,manual=pencil),indent=2))
        graph("finish")
        print(f"Export verified: {path}",flush=True)
    finally:
        server.terminate(); server.wait()


if __name__=="__main__":
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--image",default="an-bridge-native:test")
    parser.add_argument("--directory",type=Path)
    parser.add_argument("--pencil",action="store_true")
    parser.add_argument("--resume-task",help="Resume a waiting native test in its existing evidence directory")
    args=parser.parse_args()
    if args.resume_task and not (args.directory and args.pencil):
        parser.error("--resume-task requires --pencil and the original --directory")
    if args.directory:
        args.directory.mkdir(parents=True,exist_ok=bool(args.resume_task))
        run(args.directory.resolve(),args.image,args.pencil,args.resume_task)
    elif args.pencil:
        parser.error("--pencil requires a new evidence directory")
    else:
        with tempfile.TemporaryDirectory() as tmp:
            run(Path(tmp),args.image,False)
