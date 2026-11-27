"""Pass a local spool image with a valid /spool/spool.yaml."""
import argparse
import os,tempfile,subprocess,json,time
from pathlib import Path
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("image")
args = parser.parse_args()
with tempfile.TemporaryDirectory(dir="/private/tmp") as tmp:
    os.environ["AN_HOME"]=tmp
    def cli(*args):
        return subprocess.check_output(["target/debug/an",*args],text=True).strip()
    def call(*args): return json.loads(cli(*args))
    image=cli("register", args.image)
    src=Path(tmp)/"seed"
    src.write_text("round trip")
    task=call("task","new","--spool",image,"--input",str(src))["task"]
    def op(name,*args): return call(name,"--task",task,*args)
    os.environ["AN_TURN"]=call("permission","begin","--task",task)["turn"]
    def approved(name,args,tail=[]):
        req=op(name,*args,*tail)["request_id"]
        call("permission","decide","--task",task,"--request-id",req,"--decision","once")
        return op(name,"--request-id",req,*args,*tail)
    i=op("load","--spool",image)["id"]
    try:
        # Exercise the guest boundary through the same public run path as the jobs below.
        os.environ["AN_SMOKE_HOST_ONLY"] = "host-only-marker"
        probe = op("run", "--instance", i, "--", "/bin/sh", "-ec", """
            test -z "${AN_SMOKE_HOST_ONLY+x}"
            test -z "${AN_TURN+x}"
            test -z "${AN_HOME+x}"
            for iface in /sys/class/net/*; do test "$iface" = /sys/class/net/lo; done
            if touch /input/forbidden 2>/dev/null; then exit 1; fi
            if touch /forbidden 2>/dev/null; then exit 1; fi
            touch /output/probe
        """)
        for _ in range(120):
            status = op("status", probe)["status"]
            if status != "running":
                break
            time.sleep(.2)
        assert status == "completed", status
        for source,dest in [("seed","result"),("result","final")]:
            run=approved("run",["--instance",i,"--input",source if source=="seed" else run,"--output",dest],["--","/bin/cp","/input/"+source,"/output/"+dest])
            for n in range(120):
                status=op("status",run)["status"]
                if status!="running": break
                time.sleep(.2)
            assert status=="awaiting_publication",status
            approved("publish",[run])
        out=Path(tmp)/"out"
        req=call("export",run,str(out))["request_id"]
        call("permission","decide","--task",task,"--request-id",req,"--decision","once")
        cli("export","--request-id",req,run,str(out))
        assert (out/"final").read_text()=="round trip"
        print("round trip passed")
    finally: op("unload","--instance",i)
