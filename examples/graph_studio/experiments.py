"""Agent-side subgraphs and generated panel packages for the two Studio experiments."""
import json


def ref(node, port):
    return {"node": node, "port": port}


def run(node, spool, program, outputs, **inputs):
    return {"id": node, "kind": "run", "spool": spool, "argv": ["python3", "-c", program],
            "outputs": outputs, "inputs": inputs}


def generate(node, spool, files):
    prelude = "from pathlib import Path\n"
    program = prelude + "\n".join(f"Path('/output', {name!r}).write_text({data!r})"
                                  for name, data in files.items())
    return run(node, spool, program, list(files))


def package(html, bindings, response=None):
    return json.dumps({"version": 1, "entry": "index.html", "files": {"index.html": html},
                       "bindings": bindings, "response": response})


def form_schema(**properties):
    return {"type": "object", "properties": properties, "required": list(properties)}


STYLE = """<style>*{box-sizing:border-box}body{margin:24px;font:16px/1.5 system-ui;color:#234239}
h2{font-size:20px}fieldset{border:0;padding:0;display:flex;gap:14px}label{display:block}
figure{margin:0;position:relative}img{width:100%;display:block;border-radius:6px}
button{padding:8px 12px;margin-top:10px}textarea{display:block;width:100%;min-height:52px}
mark{position:absolute;pointer-events:none;transform:translate(-50%,-50%);border:3px solid #fff;
border-radius:50%;width:22px;height:22px;background:transparent;box-shadow:0 0 0 2px #b14924}</style>"""

COMPARE = STYLE + """
<h2>选择绘画参考</h2><fieldset id="choices"></fieldset><label>修改说明<textarea id="note"></textarea></label>
<button id="send">提交选择与标注</button><script>
addEventListener("an-ready",()=>{
 const draft=an.draft || {choice:"A",x:0.5,y:0.5,note:""};
 const mark=document.createElement("mark");
 function place(){document.getElementById("fig"+draft.choice).append(mark);
   mark.style.left=draft.x*100+"%";mark.style.top=draft.y*100+"%";}
 for(const key of ["A","B"]){
   const label=document.createElement("label"),fig=document.createElement("figure");
   const radio=document.createElement("input"),img=document.createElement("img");
   radio.type="radio";radio.name="choice";radio.id=key;radio.checked=draft.choice===key;
   radio.onchange=()=>{draft.choice=key;place();an.saveDraft(draft)};
   fig.id="fig"+key;img.src="data:image/svg+xml;base64,"+an.files[key]["candidate.svg"];
   img.onclick=e=>{const box=img.getBoundingClientRect();radio.checked=true;draft.choice=key;
     draft.x=(e.clientX-box.left)/box.width;draft.y=(e.clientY-box.top)/box.height;place();an.saveDraft(draft)};
   fig.append(img);label.append(radio,key,fig);choices.append(label);
 }
 place();note.value=draft.note;note.oninput=()=>{draft.note=note.value;an.saveDraft(draft)};
 send.onclick=()=>an.respond(draft);
});
</script>"""


def bridge(spool):
    schema = form_schema(choice={"type": "string", "enum": ["A", "B"]}, note={"type": "string"},
                         x={"type": "number"}, y={"type": "number"})
    nodes = []
    for name, color, x in [("A", "#75988b", 40), ("B", "#b78050", 130)]:
        program = ("from pathlib import Path\nfrom xml.etree import ElementTree as ET\n"
                   "svg=ET.Element('svg',xmlns='http://www.w3.org/2000/svg',viewBox='0 0 220 110')\n"
                   f"ET.SubElement(svg,'rect',width='220',height='110',fill={color!r})\n"
                   f"ET.SubElement(svg,'circle',cx='{x}',cy='49',r='31',fill='#f2dbac')\n"
                   "Path('/output/candidate.svg').write_bytes(ET.tostring(svg))")
        nodes.append(run(name, spool, program, ["candidate.svg"]))
    nodes.append(generate("interface", spool, {"panel.json": package(COMPARE, ["A", "B"], schema)}))
    nodes.append({"id": "compare", "title": "比较候选图", "kind": "surface", "package": "ui",
                  "save": ref("apply", "painting.ora"), "response": schema, "inputs": {
                      "ui": ref("interface", "panel.json"), **{k: ref(k, "candidate.svg") for k in ("A", "B")}},
                  "outputs": ["response", "document"]})
    return nodes


def bridge_result(spool, response):
    chosen = response["choice"]
    html = STYLE + """<h2>绘画参考与修改记录</h2>
    <button id="original">原始参考</button> <button id="annotated">显示标注</button>
    <img id="picture" alt="所选参考图"><p id="note"></p>
    <script>addEventListener("an-ready",()=>{
      const show=annotate=>{
        picture.src="data:image/svg+xml;base64,"+(annotate?an.files.selected["selected.svg"]:an.files.original["candidate.svg"]);
        original.setAttribute("aria-pressed",String(!annotate));annotated.setAttribute("aria-pressed",String(annotate));
      };
      original.onclick=()=>show(false);annotated.onclick=()=>show(true);show(true);
      const result=JSON.parse(new TextDecoder().decode(Uint8Array.from(atob(an.files.choice["response.json"]),c=>c.charCodeAt(0))));
      note.textContent=result.choice+" · "+result.note+" ("+result.x.toFixed(2)+", "+result.y.toFixed(2)+")";
    });</script>"""
    data = {"original": ref(chosen,"candidate.svg"), "selected": ref("apply", "selected.svg"), "choice": ref("compare", "response")}
    data["painting"] = ref("compare", "document")
    process = {"id": "apply", "kind": "run", "spool": spool,
               "argv": ["python3", "/spool/code/annotate.py"], "outputs": ["selected.svg", "painting.ora"],
               "inputs": {"source": ref(chosen, "candidate.svg"), "choice": ref("compare", "response")}}
    return [process,
            generate("report-ui", spool, {"report.json": package(html, list(data))}),
            {"id": "report", "title": "绘画参考与保存版本", "kind": "delivery", "package": "ui",
             "inputs": {"ui": ref("report-ui", "report.json"), **data}, "outputs": []}]


def cad(spool):
    schema = form_schema(seconds={"type": "integer", "enum": [1, 2, 3, 4, 5]})
    html = STYLE + """<h2>设置零件旋转周期</h2><label>秒 <input id="speed" type="range" min="1" max="5">
      <output id="duration"></output></label><button id="apply">生成动画</button>
      <script>addEventListener("an-ready",()=>{speed.value=an.draft?.seconds || 3;duration.value=speed.value;
      speed.oninput=()=>{duration.value=speed.value;an.saveDraft({seconds:Number(speed.value)})};
      apply.onclick=()=>an.respond({seconds:Number(speed.value)})});</script>"""
    return [generate("parameters", spool, {"panel.json": package(html, [], schema)}),
            {"id": "timing", "kind": "surface", "package": "ui", "response": schema,
             "inputs": {"ui": ref("parameters", "panel.json")}, "outputs": ["response"]},
            generate("plate", spool, {"plate.json": json.dumps(
                {"width": 76, "depth": 45, "height": 12, "chamfer": 8})}),
            {"id": "animate", "kind": "run", "spool": spool, "argv": ["python3", "/spool/code/turntable.py"],
             "inputs": {"source": ref("plate", "plate.json"), "choice": ref("timing", "response")},
             "outputs": ["plate.stl", "turntable.svg"]},
            {"id": "animation", "kind": "delivery", "title": "零件模型与旋转动画", "inputs": {
                "model": ref("animate", "plate.stl"), "animation": ref("animate", "turntable.svg")},
             "outputs": []}]
