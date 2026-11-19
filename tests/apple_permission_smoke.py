import os,tempfile,subprocess,json,time
from pathlib import Path
with tempfile.TemporaryDirectory(dir="/private/tmp") as tmp:
    os.environ["AN_HOME"]=tmp
    def cli(*args):
        return subprocess.check_output(["target/debug/an",*args],text=True).strip()
    def call(*args): return json.loads(cli(*args))
    image=cli("register","an-spool-v05:v1")
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
