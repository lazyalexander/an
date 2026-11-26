"""Agent-side subgraphs and admitted panel references for the two Studio experiments."""
import json
import sys
from pathlib import Path


def ref(node, port):
    return {"node": node, "port": port}


def run(node, spool, operation, outputs, **inputs):
    return {"id": node, "kind": "run", "spool": spool,
            "argv": ["python3", "/spool/code/asset.py", operation],
            "outputs": outputs, "inputs": inputs}


def bridge(spool):
    nodes = [run(name, spool, name, ["candidate.svg"]) for name in ("A", "B")]
    nodes.append({"id": "compare", "title": "比较候选图", "kind": "surface", "spool": spool, "panel": "compare",
                  "save": ref("apply", "painting.ora"), "inputs": {k: ref(k, "candidate.svg") for k in ("A", "B")},
                  "outputs": ["response", "document"]})
    nodes[-1]["continuation"] = "select"
    nodes.append({"id": "select", "kind": "expand", "spool": spool,
                  "argv": ["python3", "/spool/code/driver.py", "select", spool],
                  "inputs": {"choice": ref("compare", "response")}, "outputs": ["subgraph.json"]})
    return nodes


def bridge_result(spool, response, attach=False):
    chosen = response["choice"]
    data = {"original": ref(chosen,"candidate.svg"), "selected": ref("apply", "selected.svg"), "choice": ref("compare", "response")}
    data["painting"] = ref("compare", "document")
    if attach:
        data["annotation"] = ref("annotation-note","annotation.txt")
    process = {"id": "apply", "kind": "run", "spool": spool,
               "argv": ["python3", "/spool/code/annotate.py"], "outputs": ["selected.svg", "painting.ora"],
               "inputs": {"source": ref(chosen, "candidate.svg"), "choice": ref("compare", "response")}}
    return [process,
            {"id": "report", "title": "绘画参考与保存版本", "kind": "delivery", "spool": spool, "panel": "report",
             "inputs": data, "outputs": []}]


def revise(spool):
    source = {"input":"original"}
    return [run("preview", spool, "preview",
                ["selected.svg"], source=source),
            {"id":"edit","kind":"surface","spool":spool,"panel":"edit",
             "inputs":{"preview":ref("preview","selected.svg")},
             "save":ref("apply","painting.ora"),"outputs":["response","document"]},
            {"id":"apply","kind":"run","spool":spool,"argv":["python3","/spool/code/annotate.py","revise"],
             "inputs":{"source":source,"choice":ref("edit","response")},"outputs":["selected.svg","painting.ora"]},
            {"id":"revision","kind":"delivery","title":"第二版绘画","inputs":{
             "selected":ref("apply","selected.svg"),"painting":ref("edit","document")},"outputs":[]}]


def sketch(spool, model=None):
    """Sketch, clean line art and, with an image model grant, a finished painting."""
    nodes = [
        {"id": "draw", "title": "画一张草稿", "kind": "surface", "spool": spool, "panel": "sketch",
         "inputs": {}, "outputs": ["response"]},
        {"id": "finish", "title": "整理线稿", "kind": "run", "spool": spool,
         "argv": ["python3", "/spool/code/finish.py"],
         "inputs": {"choice": ref("draw", "response")}, "outputs": ["finished.svg"]}]
    delivery = {"sketch": ref("draw", "response"), "finished": ref("finish", "finished.svg")}
    if model:
        nodes += [
            {"id": "prepare", "title": "准备成品请求", "kind": "run", "spool": spool,
             "argv": ["python3", "/spool/code/prepare.py"],
             "inputs": {"choice": ref("draw", "response")}, "outputs": ["control.png", "request.json"]},
            {"id": "paint", "title": "生成成品", "kind": "model", "model": model,
             "inputs": {"request": ref("prepare", "request.json"), "image": ref("prepare", "control.png")},
             "outputs": ["result.png"]}]
        delivery["painting"] = ref("paint", "result.png")
    nodes.append({"id": "result", "title": "草稿与成品", "kind": "delivery", "spool": spool,
                  "panel": "finished", "inputs": delivery, "outputs": []})
    return nodes


def cad(spool):
    return [
            {"id": "timing", "kind": "surface", "spool": spool, "panel": "timing",
             "inputs": {}, "outputs": ["response"]},
            run("plate", spool, "plate", ["plate.json"]),
            {"id": "animate", "kind": "run", "spool": spool, "argv": ["python3", "/spool/code/turntable.py"],
             "inputs": {"source": ref("plate", "plate.json"), "choice": ref("timing", "response")},
             "outputs": ["plate.stl", "turntable.svg"]},
            {"id": "animation", "kind": "delivery", "title": "零件模型与旋转动画", "inputs": {
                "model": ref("animate", "plate.stl"), "animation": ref("animate", "turntable.svg")},
             "outputs": []}]


if __name__ == "__main__":
    stage, spool, *options = sys.argv[1:]
    if stage in ("select", "model-select"):
        response = json.loads(Path("/input/choice/response.json").read_text())
        nodes = bridge_result(spool, response)
        if stage == "model-select":
            nodes = nodes[:1] + [
                {"id":"prompt","kind":"run","spool":spool,"argv":["python3","/spool/code/driver.py","prompt",spool],
                 "inputs":{"choice":ref("compare","response")},"outputs":["request.json"]},
                {"id":"decision","kind":"model","model":"local","inputs":{"request":ref("prompt","request.json")},"outputs":["result.json"]},
                {"id":"finalize","kind":"expand","spool":spool,"argv":["python3","/spool/code/driver.py","finalize",spool],
                 "inputs":{"source":ref("decision","result.json"),"choice":ref("compare","response")},"outputs":["subgraph.json"]}]
    elif stage == "prompt":
        note = json.loads(Path("/input/choice/response.json").read_text())["note"]
        messages = [{"role":"system","content":"Classify whether the user wants a separate annotation text file with the painting. Reply with exactly ATTACH or PAINTING. Default to PAINTING."},
                    {"role":"user","content":note}]
        Path("/output/request.json").write_text(json.dumps({"messages":messages}))
        sys.exit(0)
    elif stage == "finalize":
        answer = json.loads(Path("/input/source/result.json").read_text())["text"].strip()
        if answer not in ("ATTACH", "PAINTING"):
            raise ValueError("Unexpected Bridge model decision")
        choice = json.loads(Path("/input/choice/response.json").read_text())
        nodes = bridge_result(spool, choice, attach=answer=="ATTACH")[1:]
        if answer == "ATTACH":
            nodes.insert(0, run("annotation-note", spool, "note", ["annotation.txt"],
                                choice=ref("compare", "response")))
    else:
        if stage == "sketch":
            model = next((o.split("=", 1)[1] for o in options if o.startswith("--model=")), None)
            Path("/output/subgraph.json").write_text(json.dumps({"nodes": sketch(spool, model)}))
            sys.exit(0)
        nodes = {"bridge": bridge, "bridge-model":bridge, "cad": cad, "revise": revise, "sketch": sketch}[stage](spool)
        if stage == "bridge-model":
            nodes[-1]["argv"][2] = "model-select"
    if stage in ("bridge","bridge-model") and options == ["--verify-overlap"]:
        for candidate in nodes[:2]:
            candidate["argv"].append("--verify-overlap")
    Path("/output/subgraph.json").write_text(json.dumps({"nodes": nodes}))
